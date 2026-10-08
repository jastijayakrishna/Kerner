use crate::attest::{self, KeySource};
use crate::capability;
use crate::git::{self, Files};
use crate::policy::{BUILTIN_CHECKS, CheckSpec, Contract, HONEST_FIX, Job, Limits, Operator, contract_digest, require_role, verifier_digest};
use crate::proof;
use crate::sandbox::{self, Spec, Volume};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SETUP: Limits = Limits { files_changed: 1, lines_changed: 1, file_bytes: 1048576, total_bytes: 16777216, timeout_seconds: 900, memory_mb: 3072, pids: 512 };

pub struct Dependencies { pub volume: Volume, pub record: Value }
/// Finished dependency installs, kept as read-only volumes and reused when the manifests, registry and image are the same. The index lives on
/// the verifier host (KERN_CACHE_DIR, or ~/.cache/kern); KERN_DEPS_CACHE=off turns it off.
pub struct DepsCache { dir: PathBuf, max: usize }
/// Version of what an install volume holds besides the packages; 2 added the mount points for sandbox::CACHE_DIRS.
const LAYOUT: u64 = 2;
impl DepsCache {
    pub fn at(dir: PathBuf, max: usize) -> DepsCache { DepsCache { dir, max: max.max(1) } }
    pub fn from_env() -> Option<DepsCache> {
        if std::env::var("KERN_DEPS_CACHE").as_deref() == Ok("off") { return None; }
        let root = std::env::var_os("KERN_CACHE_DIR").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("kern")))?;
        Some(DepsCache::at(root.join("deps"), 12))
    }
    pub fn key(files: &Files, registry: &str, image: &str) -> R<String> {
        let hash = |name: &str| files.get(name).map(|e| e.oid.clone());
        digest(&json!({ "kern": env!("CARGO_PKG_VERSION"), "layout": LAYOUT, "registry": registry, "image": image, "package_json": hash("package.json"), "lockfile": hash("package-lock.json") }))
    }
    fn entry(&self, key: &str) -> Option<PathBuf> { is_hex64(key).then(|| self.dir.join(format!("{key}.json"))) }
    /// A well-formed entry's volume, record and whether its install has the current layout.
    fn read(&self, key: &str) -> Option<(String, Value, bool)> {
        let v: Value = serde_json::from_slice(&std::fs::read(self.entry(key)?).ok()?).ok()?;
        let volume = v["volume"].as_str().filter(|n| n.strip_prefix("kern-deps-").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')))?;
        let record = v["record"].clone();
        (record["sha256"].as_str().is_some_and(is_hex64) && record["entries"].is_u64() && record["bytes"].is_u64()).then(|| (volume.to_string(), record, v["layout"] == LAYOUT))
    }
    pub fn get(&self, key: &str) -> Option<(String, Value)> { self.read(key).filter(|e| e.2).map(|(volume, record, _)| (volume, record)) }
    /// True when this install became the cached one; an entry that already exists is never replaced.
    pub fn put(&self, key: &str, volume: &str, record: &Value) -> R<bool> {
        let Some(path) = self.entry(key) else { return Ok(false) };
        std::fs::create_dir_all(&self.dir)?;
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
        Ok(create_new(&path, json!({ "volume": volume, "record": record, "created_ns": at, "layout": LAYOUT }).to_string().as_bytes(), 0o600).is_ok())
    }
    /// Drops entries of an older layout, then the oldest beyond the limit, and returns their volumes for the caller to remove.
    pub fn evict(&self) -> Vec<String> {
        let mut entries: Vec<(bool, u64, String, PathBuf, String)> = std::fs::read_dir(&self.dir).into_iter().flatten().flatten().filter_map(|e| {
            let path = e.path();
            let key = path.file_stem()?.to_str()?.to_string();
            // A damaged entry is dropped, and never names a volume to remove.
            let Some((volume, _, current)) = self.read(&key) else { let _ = std::fs::remove_file(&path); return None };
            let created = serde_json::from_slice::<Value>(&std::fs::read(&path).ok()?).ok()?["created_ns"].as_u64().unwrap_or(0);
            Some((current, created, key, path, volume))
        }).collect();
        entries.sort();
        let excess = entries.iter().filter(|e| !e.0).count().max(entries.len().saturating_sub(self.max));
        entries.into_iter().take(excess).filter_map(|(_, _, _, path, volume)| std::fs::remove_file(&path).ok().map(|_| volume)).collect()
    }
}
pub fn validate_lockfile(files: &Files, registry: &str) -> R<Value> {
    ensure!(files.contains_key("package.json"), "Dependency installation requires a committed package.json");
    let lock = files.get("package-lock.json").ok_or_else(|| Denied("Dependency installation requires a committed package-lock.json".into()))?;
    let lock_bytes = lock.bytes.as_deref().ok_or_else(|| Denied("package-lock.json is too large to read".into()))?;
    let v: Value = serde_json::from_slice(lock_bytes)?;
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
    Ok(json!({ "packages": pinned, "sha1_only": sha1_only, "lockfile_sha256": sha256(lock_bytes) }))
}
pub fn install_dependencies(files: &Files, op: &Operator, root: &Path, image: &str) -> R<Dependencies> {
    let registry = op.registry.as_deref().ok_or_else(|| Denied("Operator must name a registry for dependency installation".into()))?;
    let lockfile = validate_lockfile(files, registry)?;
    let input = root.join("dependency-input");
    let manifests: Files = files.iter().filter(|(k, _)| *k == "package.json" || *k == "package-lock.json").map(|(k, v)| (k.clone(), v.clone())).collect();
    git::materialize(None, &manifests, &input, None)?;
    std::fs::create_dir(input.join("node_modules"))?;
    let cache = DepsCache::from_env();
    let key = DepsCache::key(files, registry, image)?;
    if let Some((name, record)) = cache.as_ref().and_then(|c| c.get(&key)).filter(|(name, _)| sandbox::volume_exists(name)) {
        return Ok(Dependencies { volume: Volume::cached(name), record });
    }
    let control = root.join("dependency-control");
    sandbox::control_dir(&control, Some(("setup.json", &json!({ "registry": registry, "timeout_seconds": SETUP.timeout_seconds }))))?;
    let mut volume = Volume::create()?;
    let out = sandbox::execute(&Spec { image, snapshot: &input, control: &control, writable: &[], limits: &SETUP, entry: vec!["__setup".into()], network: "bridge", volumes: vec![(&volume.0, "/workspace/node_modules", false)], root: true, home_mb: 1024, scratch: false })?;
    ensure!(out.exit_code == 0, "Dependency installation failed in the sandbox");
    let record: Value = serde_json::from_slice(&out.stdout)?;
    exact(&record, &["version", "entries", "bytes", "sha256"], "dependency record")?;
    ensure!(record["sha256"].as_str().is_some_and(is_hex64), "Invalid dependency record");
    let record = json!({ "sha256": record["sha256"], "entries": record["entries"], "bytes": record["bytes"], "lockfile": lockfile });
    if let Some(c) = &cache && c.put(&key, &volume.0, &record)? {
        volume.keep();
        for old in c.evict() { sandbox::remove_volume(&old); }
    }
    Ok(Dependencies { volume, record })
}

fn candidate_files(base: &Files, wire: &Value, policy: &Contract) -> R<Files> {
    exact(wire, &["version", "files"], "candidate")?;
    let Some(items) = wire["files"].as_array().filter(|f| wire["version"] == 1 && f.len() <= 10000) else { deny!("Invalid candidate format") };
    // The agent never saw symlinks or submodules (no snapshot holds them), so they stay as they were unless the agent wrote a file there.
    let mut result: Files = base.iter().filter(|(n, e)| e.link() || !allowed(&policy.write, n)).map(|(k, v)| (k.clone(), v.clone())).collect();
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
        result.insert(p.into(), git::Entry { mode: base.get(p).map_or("100644".into(), |e| e.mode.clone()), oid: git::object_id("blob", &bytes), size: bytes.len() as u64, bytes: Some(bytes) });
    }
    ensure!(result.values().filter_map(|e| e.bytes.as_ref().map(|b| b.len() as u64)).sum::<u64>() <= policy.limits.total_bytes, "Artifact budget exceeded");
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
    ensure!(policy.write.iter().all(|w| w.ends_with('/')), "The kern-gvisor executor requires directory write grants; use file-level grants through the box/ingest path");
    let command = job.command.clone().ok_or_else(|| Denied("Job has no agent command".into()))?;
    let root = fresh_dir(output)?;
    let store = git::import_base(&job.repo, &job.base, &root.join("objects.git"))?;
    let original = git::read_commit(&store, &job.base, policy.limits.file_bytes, policy.limits.total_bytes)?;
    let snapshot = root.join("agent-input");
    git::materialize(Some(&store), &original, &snapshot, Some(&policy.read))?;
    for grant in &policy.write { std::fs::create_dir_all(snapshot.join(grant.trim_end_matches('/')))?; }
    let deps = if policy.dependencies {
        ensure!(!original.keys().any(|k| k.split('/').next() == Some("node_modules")), "Committed node_modules conflicts with dependency installation");
        std::fs::create_dir_all(snapshot.join("node_modules"))?;
        Some(install_dependencies(&original, op, &root, &op.image)?)
    } else { None };
    let control = root.join("agent-control");
    sandbox::control_dir(&control, Some(("config.json", &json!({ "write": policy.write, "limits": policy.value["limits"], "command": command }))))?;
    let volumes = deps.as_ref().map(|d| vec![(d.volume.0.as_str(), "/workspace/node_modules", true)]).unwrap_or_default();
    let exec = sandbox::execute(&Spec { image: &op.image, snapshot: &snapshot, control: &control, writable: &policy.write, limits: &policy.limits, entry: vec!["__runner".into()], network: "none", volumes, root: false, home_mb: 0, scratch: false })?;
    ensure!(exec.exit_code == 0, "Agent command failed or exceeded its limits in the sandbox");
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
        ensure!(job.contract.write.iter().filter(|g| g.ends_with('/')).all(|g| !files.contains_key(g.trim_end_matches('/'))), "Write directory overlaps a source file");
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
    let root = fresh_dir(job.output.join(format!("{}-{}", job.contract.task, short_id()?)))?;
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
        Err(e) => {
            result["status"] = json!("BLOCKED");
            result["stage"] = json!("execution");
            result["feedback"] = crate::feedback::explain(&e.0, Some(&job.contract));
            result["reason"] = json!(e.0);
        }
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
/// One operator check's evidence digest and the behaviour it saw.
type Checked = R<(String, BTreeSet<String>)>;
fn behavioral(spec: &CheckSpec, root: &Path, image: &str, snapshot: &Path, limits: &Limits, volume: Option<&str>, judge: bool) -> R<(String, BTreeSet<String>)> {
    let cases = read_check(spec)?;
    let control = root.join(format!("check-{}", spec.id));
    if !control.exists() { sandbox::control_dir(&control, None)?; }
    let deadline = now_ms() + limits.timeout_seconds * 1000;
    let (mut evidence, mut seen) = (Vec::new(), BTreeSet::new());
    for (i, (command, expected)) in cases.iter().enumerate() {
        let remaining = deadline.saturating_sub(now_ms()) / 1000;
        ensure!(remaining > 0, "Behavioral verification exceeded its total deadline");
        let case_limits = Limits { timeout_seconds: remaining.min(limits.timeout_seconds), ..limits.clone() };
        let entry = std::iter::once("__exec".to_string()).chain(command.iter().cloned()).collect();
        let volumes = volume.map(|v| vec![(v, "/workspace/node_modules", true)]).unwrap_or_default();
        let out = sandbox::execute(&Spec { image, snapshot, control: &control, writable: &[], limits: &case_limits, entry, network: "none", volumes, root: false, home_mb: 0, scratch: true })
            .map_err(|e| Denied(format!("Check {} case {} failed: {e}", spec.id, i + 1)))?;
        let (stdout, behavior) = sandbox::split_behavior(&out.stdout);
        seen.extend(behavior.ok_or_else(|| Denied(format!("Check {} case {} left no behavior record{}", spec.id, i + 1, if judge { "" } else { " on the base commit; re-run or raise limits.timeout_seconds" })))?);
        if judge {
            ensure!(out.exit_code == 0, "Check {} case {} failed (exit code {})", spec.id, i + 1, out.exit_code);
            if let Some(want) = expected { ensure!(stdout == want.as_bytes(), "Check {} case {} output differs from the expected answer", spec.id, i + 1); }
        }
        evidence.push(json!({ "case": i + 1, "stdout_sha256": sha256(stdout), "builder_sha256": out.builder.0 }));
    }
    Ok((digest(&json!(evidence))?, seen))
}
fn secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL", "API_KEY", "APIKEY", "PRIVATE_KEY", "ACCESS_KEY", "AUTH", "SESSION", "COOKIE"].iter().any(|s| n.contains(s))
        || ["AWS_", "AZURE_", "GCP_", "GOOGLE_", "GITHUB_", "GH_", "GITLAB_", "NPM_", "SLACK_", "STRIPE_", "OPENAI_", "ANTHROPIC_", "DATABASE_URL"].iter().any(|p| n.starts_with(p))
}
pub fn behavior_diff(base: &BTreeSet<String>, candidate: &BTreeSet<String>, allowed: &[String]) -> (Vec<String>, Vec<String>) {
    let new: Vec<String> = candidate.difference(base).filter(|b| !allowed.contains(b)).cloned().collect();
    let blocking = new.iter().filter(|b| match b.split_once(':') {
        Some(("net", t)) => t != "loopback" && t != "ipc",
        Some(("env", name)) => name == "*" || secret(name),
        _ => true,
    }).cloned().collect();
    (new, blocking)
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
    git::materialize(Some(&store), &candidate, &snapshot, None)?;
    let deps = if policy.dependencies {
        ensure!(!candidate.keys().any(|k| k.split('/').next() == Some("node_modules")), "Committed node_modules conflicts with dependency installation");
        std::fs::create_dir(snapshot.join("node_modules"))?;
        Some(install_dependencies(&candidate, op, &root, &op.image)?)
    } else { None };
    let volume = deps.as_ref().map(|d| d.volume.0.as_str());
    let specs = policy.checks.iter().map(|id| op.checks.iter().find(|c| &c.id == id).ok_or_else(|| Denied(format!("No trusted check: {id}")))).collect::<R<Vec<_>>>()?;
    let baseline_snapshot = root.join("baseline");
    git::materialize(Some(&store), &original, &baseline_snapshot, None)?;
    if policy.dependencies { std::fs::create_dir(baseline_snapshot.join("node_modules"))?; }
    // Each check runs on the new and the old code at the same time, at most as many sandboxes at once as the gate allows.
    let gate = Gate::new(parallelism());
    for spec in &specs { sandbox::control_dir(&root.join(format!("check-{}", spec.id)), None)?; }
    let results: Vec<(Checked, Checked)> = std::thread::scope(|s| {
        let pending: Vec<_> = specs.iter().map(|spec| {
            let (root, image, snapshot, baseline_snapshot, gate) = (&root, &op.image, &snapshot, &baseline_snapshot, &gate);
            (s.spawn(move || gate.run(|| behavioral(spec, root, image, snapshot, l, volume, true))), s.spawn(move || gate.run(|| behavioral(spec, root, image, baseline_snapshot, l, volume, false))))
        }).collect();
        pending.into_iter().map(|(new, old)| (new.join().unwrap_or_else(|_| Err(Denied("A check stopped unexpectedly".into()))), old.join().unwrap_or_else(|_| Err(Denied("A check stopped unexpectedly".into()))))).collect()
    });
    let (mut behavior, mut baseline) = (BTreeSet::new(), BTreeSet::new());
    let mut olds = Vec::new();
    for (spec, (new, old)) in specs.iter().zip(results) {
        let (evidence, seen) = new?;
        behavior.extend(seen);
        checks.push(json!({ "id": spec.id, "status": "PASS", "evidence_sha256": evidence }));
        olds.push(old);
    }
    for old in olds { baseline.extend(old?.1); }
    let (new, blocking) = behavior_diff(&baseline, &behavior, &policy.behaviors);
    let observed = json!({ "base": baseline, "candidate": behavior, "new": new, "blocking": blocking });
    write_json(root.join("behavior.json"), &observed)?;
    ensure!(blocking.is_empty(), "Tests show new runtime behavior: {}. If this is intended, allow it in the task contract (version 3): \"behaviors\": {}", blocking.join(", "), serde_json::to_string(&blocking)?);
    checks.push(json!({ "id": "behavior", "status": "PASS", "evidence_sha256": digest(&observed)? }));
    if let Some(tests) = &policy.tests {
        let proof_root = root.join("honest-fix");
        std::fs::create_dir(&proof_root)?;
        let control = proof_root.join("control");
        sandbox::control_dir(&control, None)?;
        // The old code with the change's tests, and (when tests were rewritten or removed) the new code with the old tests.
        let swap = |code: &Files, tests_from: &Files| -> Files {
            code.iter().filter(|(k, _)| !allowed(&tests.paths, k)).chain(tests_from.iter().filter(|(k, _)| allowed(&tests.paths, k))).map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let alone = proof::old_tests_alone(&changed["files"], &changed["tests"], tests);
        let old_snapshot = proof_root.join("old");
        let kept_snapshot = proof_root.join("kept");
        for (dir, files, needed) in [(&old_snapshot, swap(&original, &candidate), true), (&kept_snapshot, swap(&candidate, &original), alone)] {
            if !needed { continue; }
            git::materialize(Some(&store), &files, dir, None)?;
            if policy.dependencies { std::fs::create_dir(dir.join("node_modules"))?; }
        }
        let entry: Vec<String> = std::iter::once("__exec".to_string()).chain(proof::command(tests)).collect();
        let run_tests = |which: proof::Run, files: &[String]| -> R<proof::Output> {
            gate.run(|| {
                let volumes = volume.map(|v| vec![(v, "/workspace/node_modules", true)]).unwrap_or_default();
                let at = match which { proof::Run::Base => &baseline_snapshot, proof::Run::New => &snapshot, proof::Run::Kept => &kept_snapshot, proof::Run::Old => &old_snapshot };
                let entry = entry.iter().chain(files).cloned().collect();
                let out = sandbox::execute(&Spec { image: &op.image, snapshot: at, control: &control, writable: &[], limits: l, entry, network: "none", volumes, root: false, home_mb: 0, scratch: true })?;
                let (report, behavior) = sandbox::split_behavior(&out.stdout);
                ensure!(behavior.is_some(), "the test command did not run to completion");
                Ok(proof::Output { stdout: report.to_vec(), exit_code: out.exit_code })
            })
        };
        // Planting bugs runs in one sandbox: the new code is its read-only input, the old tests travel in its control directory.
        // The planted bugs are shared out over sandboxes that run side by side, as many as the gate allows; each reports its part.
        let mut plant = |plan: &proof::Plan| -> R<Value> {
            let old_tests: Files = original.iter().filter(|(k, _)| allowed(&tests.paths, k)).map(|(k, v)| (k.clone(), v.clone())).collect();
            let parts = proof::split_planting(plan, parallelism());
            let answers: Vec<R<Value>> = std::thread::scope(|s| {
                let running: Vec<_> = parts.iter().enumerate().map(|(i, part)| {
                    let (old_tests, gate, proof_root, store, snapshot) = (&old_tests, &gate, &proof_root, &store, &snapshot);
                    s.spawn(move || -> R<Value> {
                        let planting = proof_root.join(format!("planting-{i}"));
                        sandbox::control_dir(&planting, Some(("mutate.json", &part.json(l.timeout_seconds.saturating_sub(15).max(1)))))?;
                        git::materialize(Some(store), old_tests, &planting.join("old"), None)?;
                        let volumes = volume.map(|v| vec![(v, "/workspace/node_modules", true)]).unwrap_or_default();
                        let out = gate.run(|| sandbox::execute(&Spec { image: &op.image, snapshot, control: &planting, writable: &[], limits: l, entry: vec!["__mutate".into()], network: "none", volumes, root: false, home_mb: 0, scratch: true }))?;
                        ensure!(out.exit_code == 0, "the planting run failed (exit code {})", out.exit_code);
                        let text = String::from_utf8_lossy(&out.stdout);
                        Ok(serde_json::from_str(text.lines().rev().find(|line| line.starts_with('{')).unwrap_or("{}"))?)
                    })
                }).collect();
                running.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(Denied("A planting run stopped unexpectedly".into())))).collect()
            });
            proof::merge_planted(answers.into_iter().collect::<R<Vec<_>>>()?)
        };
        let evidence = proof::prove(tests, &original, &candidate, &changed, &run_tests, &mut plant)?;
        write_json(root.join("honest-fix.json"), &evidence)?;
        checks.push(json!({ "id": proof::CHECK, "status": "PASS", "evidence_sha256": digest(&evidence)? }));
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
pub fn required_checks(policy: &Contract) -> Vec<String> {
    BUILTIN_CHECKS.iter().map(|s| s.to_string()).chain(policy.tests.as_ref().map(|_| HONEST_FIX.to_string())).chain(policy.checks.iter().cloned()).collect()
}
