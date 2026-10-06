use crate::attest::{self, KeySource};
use crate::capability;
use crate::git::{self, Files};
use crate::policy::{BUILTIN_CHECKS, CheckSpec, Contract, Job, Limits, Operator, contract_digest, require_role, verifier_digest};
use crate::sandbox::{self, Spec, Volume};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SETUP: Limits = Limits { files_changed: 1, lines_changed: 1, file_bytes: 1048576, total_bytes: 16777216, timeout_seconds: 900, memory_mb: 3072, pids: 512 };

pub struct Dependencies { pub volume: Volume, pub record: Value }
pub fn validate_lockfile(files: &Files, registry: &str) -> R<Value> {
    ensure!(files.contains_key("package.json"), "Dependency installation requires a committed package.json");
    let lock = files.get("package-lock.json").ok_or_else(|| Denied("Dependency installation requires a committed package-lock.json".into()))?;
    let v: Value = serde_json::from_slice(&lock.bytes)?;
    ensure!(matches!(v["lockfileVersion"].as_u64(), Some(2 | 3)), "package-lock.json v2 or v3 required");
    let Some(packages) = v["packages"].as_object() else { deny!("package-lock.json has no packages map") };
    ensure!(packages.len() <= 20000, "Too many locked packages");
    let prefixes = [format!("https://{registry}/"), format!("http://{registry}/")];
    let (mut pinned, mut sha1_only) = (0u64, 0u64);
    for (key, p) in packages.iter().filter(|(k, _)| !k.is_empty()) {
        ensure!(p["link"] != true, "Linked workspace packages are unsupported: {key}");
        if p["inBundle"] == true { continue; }
        ensure!(p["resolved"].as_str().is_some_and(|r| prefixes.iter().any(|x| r.starts_with(x.as_str()))), "Locked package {key} does not resolve from {registry}");
        let hashes: Vec<&str> = p["integrity"].as_str().unwrap_or("").split_whitespace().filter_map(|h| h.split_once('-').map(|x| x.0)).collect();
        ensure!(!hashes.is_empty() && hashes.iter().all(|a| ["sha512", "sha384", "sha256", "sha1"].contains(a)), "Locked package {key} lacks a standard integrity pin");
        pinned += 1;
        if hashes.iter().all(|a| *a == "sha1") { sha1_only += 1; }
    }
    Ok(json!({ "packages": pinned, "sha1_only": sha1_only, "lockfile_sha256": sha256(&lock.bytes) }))
}
pub fn install_dependencies(files: &Files, op: &Operator, root: &Path, image: &str) -> R<Dependencies> {
    let registry = op.registry.as_deref().ok_or_else(|| Denied("Operator must name a registry for dependency installation".into()))?;
    let lockfile = validate_lockfile(files, registry)?;
    let input = root.join("dependency-input");
    let manifests: Files = files.iter().filter(|(k, _)| *k == "package.json" || *k == "package-lock.json").map(|(k, v)| (k.clone(), v.clone())).collect();
    git::materialize(&manifests, &input, None)?;
    std::fs::create_dir(input.join("node_modules"))?;
    let control = root.join("dependency-control");
    sandbox::control_dir(&control, Some(("setup.json", &json!({ "registry": registry, "timeout_seconds": SETUP.timeout_seconds }))))?;
    let volume = Volume::create()?;
    let out = sandbox::execute(&Spec { image, snapshot: &input, control: &control, writable: &[], limits: &SETUP, entry: vec!["__setup".into()], network: "bridge", volumes: vec![(&volume.0, "/workspace/node_modules", false)], root: true, home_mb: 1024 })?;
    let record: Value = serde_json::from_slice(&out.stdout)?;
    exact(&record, &["version", "entries", "bytes", "sha256"], "dependency record")?;
    ensure!(record["sha256"].as_str().is_some_and(is_hex64), "Invalid dependency record");
    Ok(Dependencies { volume, record: json!({ "sha256": record["sha256"], "entries": record["entries"], "bytes": record["bytes"], "lockfile": lockfile }) })
}

fn candidate_files(base: &Files, wire: &Value, policy: &Contract) -> R<Files> {
    exact(wire, &["version", "files"], "candidate")?;
    let Some(items) = wire["files"].as_array().filter(|f| wire["version"] == 1 && f.len() <= 10000) else { deny!("Invalid candidate format") };
    let mut result: Files = base.iter().filter(|(n, _)| !allowed(&policy.write, n)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let (mut seen, mut total) = (std::collections::HashSet::new(), 0u64);
    for item in items {
        exact(item, &["path", "content"], "candidate file")?;
        let p = text(item, "path")?;
        file_path(p)?;
        ensure!(allowed(&policy.write, p), "Candidate path outside authority: {p}");
        ensure!(seen.insert(p.to_lowercase()), "Duplicate or case-colliding candidate path");
        let bytes = unb64(text(item, "content")?)?;
        total += bytes.len() as u64;
        ensure!(bytes.len() as u64 <= policy.limits.file_bytes && total <= policy.limits.total_bytes, "Candidate byte budget exceeded");
        result.insert(p.into(), git::Entry { mode: base.get(p).map_or("100644".into(), |e| e.mode.clone()), bytes });
    }
    ensure!(result.values().map(|e| e.bytes.len() as u64).sum::<u64>() <= policy.limits.total_bytes, "Artifact budget exceeded");
    tree_paths(result.keys().map(String::as_str), true)?;
    Ok(result)
}
fn finish_run(store: &Path, root: &Path, job: &Job, original: &Files, candidate: &Files, executor: Value, dependencies: Value) -> R<Value> {
    let (policy, op) = (&job.contract, &job.operator);
    git::changes(original, candidate, policy)?;
    let commit = git::write_commit(store, candidate, Some(&job.base), &policy.task)?;
    let m = git::manifest(store, &commit, policy.limits.file_bytes, policy.limits.total_bytes)?;
    let run = json!({ "version": 2, "run_id": uuid4()?, "repository": op.repository, "base": job.base, "commit": commit, "contract_sha256": contract_digest(policy)?, "manifest_sha256": digest(&m)?, "executor": executor, "dependencies": dependencies });
    write_json(root.join("run.json"), &run)?;
    write_json(root.join("manifest.json"), &m)?;
    Ok(run)
}
pub fn run_agent(job: &Job, output: &Path) -> R<Value> {
    require_role("executor")?;
    let (policy, op) = (&job.contract, &job.operator);
    ensure!(op.executors.iter().any(|e| e == "kern-gvisor"), "Operator does not accept kern-gvisor execution");
    let command = job.command.clone().ok_or_else(|| Denied("Job has no agent command".into()))?;
    let root = fresh_dir(output)?;
    let store = git::import_base(&job.repo, &job.base, &root.join("objects.git"))?;
    let original = git::read_commit(&store, &job.base, policy.limits.file_bytes, policy.limits.total_bytes)?;
    let snapshot = root.join("agent-input");
    git::materialize(&original, &snapshot, Some(&policy.read))?;
    for grant in &policy.write { std::fs::create_dir_all(snapshot.join(grant.trim_end_matches('/')))?; }
    let deps = if policy.dependencies {
        ensure!(!original.keys().any(|k| k.split('/').next() == Some("node_modules")), "Committed node_modules conflicts with dependency installation");
        std::fs::create_dir_all(snapshot.join("node_modules"))?;
        Some(install_dependencies(&original, op, &root, &op.image)?)
    } else { None };
    let control = root.join("agent-control");
    sandbox::control_dir(&control, Some(("config.json", &json!({ "write": policy.write, "limits": policy.value["limits"], "command": command }))))?;
    let volumes = deps.as_ref().map(|d| vec![(d.volume.0.as_str(), "/workspace/node_modules", true)]).unwrap_or_default();
    let exec = sandbox::execute(&Spec { image: &op.image, snapshot: &snapshot, control: &control, writable: &policy.write, limits: &policy.limits, entry: vec!["__runner".into()], network: "none", volumes, root: false, home_mb: 0 })?;
    ensure!(Some(&exec.builder.0) == op.executor_builder.as_ref(), "Executor builder identity mismatch");
    let candidate = candidate_files(&original, &serde_json::from_slice(&exec.stdout)?, policy)?;
    let executor = json!({ "kind": "kern-gvisor", "builder_sha256": exec.builder.0, "builder": exec.builder.1, "runtime": "runsc", "image": op.image, "exit_code": 0, "output_sha256": sha256(&exec.stdout) });
    finish_run(&store, &root, job, &original, &candidate, executor, deps.map_or(Value::Null, |d| d.record))
}
pub fn ingest(job: &Job, agent_repo: &Path, agent_commit: &str, output: &Path) -> R<Value> {
    let (policy, op) = (&job.contract, &job.operator);
    ensure!(op.executors.iter().any(|e| e == "external"), "Operator does not accept externally executed candidates");
    git::oid(agent_commit)?;
    let root = fresh_dir(output)?;
    let store = git::import_base(&job.repo, &job.base, &root.join("objects.git"))?;
    let original = git::read_commit(&store, &job.base, policy.limits.file_bytes, policy.limits.total_bytes)?;
    let candidate = git::read_commit(agent_repo, agent_commit, policy.limits.file_bytes, policy.limits.total_bytes)?;
    finish_run(&store, &root, job, &original, &candidate, json!({ "kind": "external", "source_commit": agent_commit }), Value::Null)
}

fn under(root: &Path, candidate: &Path) -> bool { candidate.starts_with(root) }
fn nearest_existing(p: &Path) -> R<PathBuf> {
    let mut cur = p.to_path_buf();
    while !cur.exists() { cur = cur.parent().ok_or_else(|| Denied("No existing output ancestor".into()))?.to_path_buf(); }
    Ok(cur)
}
pub fn preflight(job: &Job, execute_agent: bool) -> Value {
    let mut checks = Vec::new();
    let mut check = |id: &str, f: &mut dyn FnMut() -> R<String>| {
        let r = f();
        let ok = r.is_ok();
        checks.push(match r { Ok(detail) => json!({ "id": id, "status": "PASS", "detail": detail }), Err(e) => json!({ "id": id, "status": "BLOCKED", "detail": e.0 }) });
        ok
    };
    check("source", &mut || {
        ensure!(git::trimmed(git::git(&job.repo, &["rev-parse", "--show-object-format"], None)?) == "sha1", "A SHA-1 Git repository is required");
        let files = git::read_commit(&job.repo, &job.base, job.contract.limits.file_bytes, job.contract.limits.total_bytes)?;
        tree_paths(files.keys().map(String::as_str).chain(job.contract.read.iter().chain(&job.contract.write).map(|g| g.trim_end_matches('/'))), false)?;
        ensure!(job.contract.write.iter().all(|g| !files.contains_key(g.trim_end_matches('/'))), "Write directory overlaps a source file");
        let deps = if job.contract.dependencies {
            let l = validate_lockfile(&files, job.operator.registry.as_deref().unwrap_or(""))?;
            format!("; {} locked packages ({} with legacy SHA-1-only integrity)", l["packages"], l["sha1_only"])
        } else { String::new() };
        Ok(format!("{} committed files; base {}{deps}", files.len(), job.base))
    });
    check("keys", &mut || {
        let key = attest::public_key(&std::fs::read_to_string(&job.verification_key)?)?;
        ensure!(!under(&std::fs::canonicalize(&job.repo)?, &std::fs::canonicalize(&job.verification_key)?), "Keep the verification key outside the source repository");
        Ok(format!("Verifier public key {}; the executor holds no signing key", attest::key_id(&key)?))
    });
    check("output", &mut || {
        let existing = std::fs::canonicalize(nearest_existing(&job.output)?)?;
        let real = existing.join(job.output.strip_prefix(nearest_existing(&job.output)?).unwrap_or(Path::new("")));
        ensure!(!under(&std::fs::canonicalize(&job.repo)?, &real), "Keep task output outside the source repository");
        ensure!(!under(std::fs::canonicalize(&job.verification_key)?.parent().unwrap_or(Path::new("/")), &real), "Keep task output outside key directories");
        Ok("A new unique task directory will be created".into())
    });
    if execute_agent {
        let ready = check("runtime", &mut || {
            let d = sandbox::doctor();
            ensure!(d.ready, "{}", d.report["explanation"].as_str().unwrap_or("runsc unavailable"));
            ensure!(d.builder.map(|b| b.0) == job.operator.executor_builder, "Executor platform identity does not match the operator pin");
            Ok("Pinned runsc and resource limits available".into())
        });
        if ready { check("image", &mut || sandbox::validate_image(&job.operator.image).map(|id| format!("Pinned image {id} is present"))); }
    }
    let ready = checks.iter().all(|c| c["status"] == "PASS");
    json!({ "status": if ready { "READY" } else { "BLOCKED" }, "stage": "preflight", "task": job.contract.task, "repository": job.operator.repository, "base": job.base,
        "authority": { "read": job.contract.read, "write": job.contract.write, "network": [], "secrets": [], "publish": false, "dependencies": job.contract.dependencies }, "checks": checks })
}
pub enum Source<'a> { Agent, Ingest(&'a Path, &'a str) }
pub fn task(job: &Job, source: Source) -> R<Value> {
    let report = preflight(job, matches!(source, Source::Agent));
    if report["status"] != "READY" { return Ok(report); }
    std::fs::create_dir_all(&job.output)?;
    let root = fresh_dir(job.output.join(format!("{}-{}", job.contract.task, uuid4()?)))?;
    let mut result = report.clone();
    let outcome = (|| -> R<Value> {
        let run_dir = root.join("run");
        let run = match source { Source::Agent => run_agent(job, &run_dir)?, Source::Ingest(repo, commit) => ingest(job, repo, commit, &run_dir)? };
        let store = run_dir.join("objects.git");
        let (base, commit) = (text(&run, "base")?, text(&run, "commit")?);
        let l = &job.contract.limits;
        let changed = git::changes(&git::read_commit(&store, base, l.file_bytes, l.total_bytes)?, &git::read_commit(&store, commit, l.file_bytes, l.total_bytes)?, &job.contract)?;
        create_new(root.join("changes.patch"), &git::git(&store, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--binary", "--full-index", base, commit, "--"], None)?, 0o600)?;
        let handoff = git::handoff(&store, &run, &root.join("handoff"))?;
        Ok(json!({ "commit": commit, "run_id": run["run_id"], "executor": run["executor"]["kind"], "changes": changed, "handoff": handoff }))
    })();
    match outcome {
        Ok(v) => {
            for (k, x) in v.as_object().into_iter().flatten() { result[k] = x.clone(); }
            result["status"] = json!("CANDIDATE");
            result["next_action"] = json!("Review changes.patch, then transfer only the handoff directory to the verifier (kern import). This candidate is unsigned and unverified.");
        }
        Err(e) => { result["status"] = json!("BLOCKED"); result["stage"] = json!("execution"); result["reason"] = json!(e.0); }
    }
    result["bundle"] = json!(root.display().to_string());
    write_json(root.join("result.json"), &result)?;
    Ok(result)
}

fn read_check(spec: &CheckSpec) -> R<Vec<(Vec<String>, Option<String>)>> {
    let meta = std::fs::symlink_metadata(&spec.file).map_err(|e| Denied(format!("Check {}: {e}", spec.id)))?;
    ensure!(meta.is_file() && meta.len() <= 1048576, "Check {} is not a small regular file", spec.id);
    let bytes = std::fs::read(&spec.file)?;
    ensure!(sha256(&bytes) == spec.sha256, "Pinned check changed: {}", spec.id);
    let v: Value = serde_json::from_slice(&bytes)?;
    exact(&v, &["version", "cases"], "behavioral check")?;
    let Some(cases) = v["cases"].as_array().filter(|c| v["version"] == 1 && !c.is_empty() && c.len() <= 64) else { deny!("Check {}: expected 1..64 cases", spec.id) };
    cases.iter().map(|c| {
        exact(c, &["command", "stdout"], "behavioral case")?;
        let stdout = match &c["stdout"] { Value::Null => None, Value::String(s) if s.len() <= 65536 => Some(s.clone()), _ => deny!("Expected stdout must be null or bounded text") };
        Ok((argv(&c["command"])?, stdout))
    }).collect()
}
fn behavioral(spec: &CheckSpec, root: &Path, image: &str, snapshot: &Path, limits: &Limits, volume: Option<&str>) -> R<String> {
    let cases = read_check(spec)?;
    let control = root.join(format!("check-{}", spec.id));
    sandbox::control_dir(&control, None)?;
    let deadline = now_ms() + limits.timeout_seconds * 1000;
    let mut evidence = Vec::new();
    for (i, (command, expected)) in cases.iter().enumerate() {
        let remaining = deadline.saturating_sub(now_ms()) / 1000;
        ensure!(remaining > 0, "Behavioral verification exceeded its total deadline");
        let case_limits = Limits { timeout_seconds: remaining.min(limits.timeout_seconds), ..limits.clone() };
        let entry = std::iter::once("__exec".to_string()).chain(command.iter().cloned()).collect();
        let volumes = volume.map(|v| vec![(v, "/workspace/node_modules", true)]).unwrap_or_default();
        let out = sandbox::execute(&Spec { image, snapshot, control: &control, writable: &[], limits: &case_limits, entry, network: "none", volumes, root: false, home_mb: 0 })
            .map_err(|e| Denied(format!("Check {} case {} failed: {e}", spec.id, i + 1)))?;
        if let Some(want) = expected { ensure!(out.stdout == want.as_bytes(), "Check {} case {} output differs from the expected answer", spec.id, i + 1); }
        evidence.push(json!({ "case": i + 1, "stdout_sha256": sha256(&out.stdout), "builder_sha256": out.builder.0 }));
    }
    digest(&json!(evidence))
}
pub fn verify(run_dir: &Path, policy: &Contract, op: &Operator, output: &Path, signer: &KeySource) -> R<Value> {
    require_role("verifier")?;
    let platform = sandbox::doctor();
    let identity = platform.builder.filter(|b| platform.ready && b.0 == op.verifier_builder).ok_or_else(|| Denied("Verifier builder identity mismatch or unavailable".into()))?;
    let run = read_json(run_dir.join("run.json"))?;
    git::validate_run(&run)?;
    let store = run_dir.join("objects.git");
    let (base, commit) = (text(&run, "base")?, text(&run, "commit")?);
    ensure!(run["repository"] == op.repository && run["contract_sha256"] == contract_digest(policy)?, "Run context mismatch");
    let e = &run["executor"];
    let kind = text(e, "kind")?;
    ensure!(op.executors.iter().any(|x| x == kind), "Executor kind {kind} is not accepted by policy");
    if kind == "kern-gvisor" {
        ensure!(e["builder_sha256"].as_str() == op.executor_builder.as_deref() && digest(&e["builder"])? == text(e, "builder_sha256")?, "Unapproved executor identity");
        ensure!(e["runtime"] == "runsc" && e["image"] == op.image.as_str() && e["exit_code"] == 0, "Invalid agent execution evidence");
        ensure!(!op.multi_machine || e["builder"]["machine_id_sha256"] != identity.1["machine_id_sha256"], "Multi-machine deployment requires distinct executor and verifier machines");
    }
    let l = &policy.limits;
    let root = fresh_dir(output)?;
    let m = git::manifest(&store, commit, l.file_bytes, l.total_bytes)?;
    ensure!(digest(&m)? == text(&run, "manifest_sha256")?, "Artifact changed");
    ensure!(git::parents(&store, commit)? == vec![base.to_string()], "Artifact parent mismatch");
    let (original, candidate) = (git::read_commit(&store, base, l.file_bytes, l.total_bytes)?, git::read_commit(&store, commit, l.file_bytes, l.total_bytes)?);
    let changed = git::changes(&original, &candidate, policy)?;
    let capabilities = capability::check(&original, &candidate, &changed, &op.permissions)?;
    let mut checks = vec![json!({ "id": "artifact", "status": "PASS", "evidence_sha256": digest(&m)? }), json!({ "id": "path-and-budget", "status": "PASS", "evidence_sha256": digest(&changed)? }), json!({ "id": "capabilities", "status": "PASS", "evidence_sha256": digest(&capabilities)? })];
    let snapshot = root.join("artifact");
    git::materialize(&candidate, &snapshot, None)?;
    let deps = if policy.dependencies {
        ensure!(!candidate.keys().any(|k| k.split('/').next() == Some("node_modules")), "Committed node_modules conflicts with dependency installation");
        std::fs::create_dir(snapshot.join("node_modules"))?;
        Some(install_dependencies(&candidate, op, &root, &op.image)?)
    } else { None };
    for id in &policy.checks {
        let spec = op.checks.iter().find(|c| &c.id == id).ok_or_else(|| Denied(format!("No trusted check: {id}")))?;
        let evidence = behavioral(spec, &root, &op.image, &snapshot, l, deps.as_ref().map(|d| d.volume.0.as_str()))?;
        checks.push(json!({ "id": id, "status": "PASS", "evidence_sha256": evidence }));
    }
    ensure!(digest(&git::manifest(&store, commit, l.file_bytes, l.total_bytes)?)? == text(&run, "manifest_sha256")?, "Artifact changed during verification");
    ensure!(sandbox::doctor().builder.map(|b| b.0) == Some(op.verifier_builder.clone()), "Verifier platform changed during checks");
    let issued = now_ms();
    let kern = json!({
        "run_id": run["run_id"], "task": policy.task, "repository": op.repository, "base": base, "commit": commit, "manifest_sha256": run["manifest_sha256"], "contract_sha256": contract_digest(policy)?,
        "verifier_sha256": verifier_digest(op)?, "executor": { "kind": kind, "builder_sha256": e.get("builder_sha256").cloned().unwrap_or(Value::Null), "source_commit": e.get("source_commit").cloned().unwrap_or(Value::Null) },
        "verifier_builder_sha256": op.verifier_builder, "deployment": op.value["deployment"], "image": op.image, "runtime": "runsc",
        "dependencies_sha256": deps.as_ref().map_or(Value::Null, |d| d.record["sha256"].clone()), "checks": checks, "issued_at": issued, "expires_at": issued + attest::LIFETIME_MS,
    });
    let envelope = attest::sign(&kern, signer)?;
    write_json(root.join("attestation.json"), &envelope)?;
    write_json(root.join("capabilities.json"), &capabilities)?;
    Ok(envelope)
}
pub fn required_checks(policy: &Contract) -> Vec<String> { BUILTIN_CHECKS.iter().map(|s| s.to_string()).chain(policy.checks.iter().cloned()).collect() }
