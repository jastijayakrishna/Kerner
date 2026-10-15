use crate::attest;
use crate::capability;
use crate::container::{self, BoxSpec, Mount};
use crate::flow::{self, Source};
use crate::git::{self, Entry, Files};
use crate::policy::{Contract, Job, MAX_LIMITS, Operator, Pin, contract, image, operator, pin, pinned};
use crate::util::*;
use crate::{deny, ensure};
use ed25519_dalek::Signature;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() >= 2 && rest.as_bytes()[1] == b':' => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}
fn resolved(p: &Path) -> PathBuf {
    let parts: Vec<std::path::Component> = p.components().collect();
    for keep in (1..=parts.len()).rev() {
        let Ok(head) = std::fs::canonicalize(parts[..keep].iter().collect::<PathBuf>()) else { continue };
        let mut out = strip_verbatim(&head);
        for part in &parts[keep..] {
            match part {
                std::path::Component::ParentDir => { out.pop(); }
                std::path::Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        return out;
    }
    p.to_path_buf()
}
fn abs_existing(s: &str, label: &str) -> R<PathBuf> {
    let p = Path::new(s);
    ensure!(p.is_absolute(), "{label} must be an absolute path: {s}");
    let c = std::fs::canonicalize(p).map_err(|e| Denied(format!("{label} {s}: {e}")))?;
    Ok(strip_verbatim(&c))
}

pub struct BoxJob { pub image: String, pub workspace: PathBuf, pub command: Vec<String>, pub network: bool, pub env: Vec<String>, pub mounts: Vec<Mount>, pub timeout_seconds: u64, pub memory_mb: u64, pub pids: u64 }
impl BoxJob {
    fn spec(&self) -> BoxSpec<'_> {
        BoxSpec { image: &self.image, command: &self.command, workspace: &self.workspace, mounts: &self.mounts, env: &self.env, network: self.network, timeout_seconds: self.timeout_seconds, memory_mb: self.memory_mb, pids: self.pids, feedback: None, delta: None }
    }
}
fn present<'a>(v: &Value, required: &[&'a str], optional: &[&'a str]) -> Vec<&'a str> {
    required.iter().chain(optional.iter().filter(|k| v.get(**k).is_some())).copied().collect()
}
fn pinned_image(v: &Value) -> R<String> {
    let s = text(v, "image")?;
    ensure!(image(s), "image must be a Linux image pinned by digest, for example docker.io/library/node:22-bookworm-slim@sha256:<digest> \
        (docker image inspect --format '{{{{index .RepoDigests 0}}}}' <image> prints it)");
    Ok(s.to_string())
}
fn env_names(v: &Value) -> R<Vec<String>> {
    if v.get("env").is_none() { return Ok(Vec::new()); }
    let names = strings(v, "env", 32)?;
    let valid = |n: &String| n.len() <= 128 && n != "HOME" && n.bytes().next().is_some_and(|b| !b.is_ascii_digit()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    ensure!(names.iter().enumerate().all(|(i, n)| valid(n) && !names[..i].contains(n)),
        "env must list unique environment variable names (letters, digits and _) other than HOME; the agent gets their values from Kern's environment");
    Ok(names)
}
fn mounts(v: &Value, private: &[&Path]) -> R<Vec<Mount>> {
    let Some(raw) = v.get("mounts") else { return Ok(Vec::new()) };
    let Some(list) = raw.as_array().filter(|l| l.len() <= 16) else { deny!("mounts must be an array of at most 16 read-only mounts") };
    let mut out: Vec<Mount> = Vec::new();
    for m in list {
        exact(m, &["source", "target"], "mount")?;
        let target = text(m, "target")?.to_string();
        let parts: Vec<&str> = target.split('/').skip(1).collect();
        let path = target.starts_with('/') && target.len() <= 256 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
        let reserved = container::RESERVED.iter().any(|r| target == *r || target.starts_with(&format!("{r}/")));
        ensure!(path && !reserved && !out.iter().any(|o| o.target == target), "mount target {target} must be a unique absolute container path outside {}", container::RESERVED.join(", "));
        let source = abs_existing(text(m, "source")?, "mount source")?;
        ensure!(!source.to_string_lossy().contains([',', '"', '\n', '\r']), "mount source {} cannot contain a comma, quote or line break", source.display());
        for p in private {
            ensure!(!source.starts_with(p) && !p.starts_with(&source), "mount source {} overlaps {}; the agent may only see its own workspace", source.display(), p.display());
        }
        out.push(Mount { source, target });
    }
    Ok(out)
}
fn flag(v: &Value, key: &str) -> R<bool> { v[key].as_bool().ok_or_else(|| Denied(format!("{key} must be a boolean"))) }
pub fn box_job(v: &Value) -> R<BoxJob> {
    ensure!(v["version"] != 1, "Box job v1 ran on the retired Windows box; the local box now runs the command in a Linux container. Replace tools and denied with an image (and read-only mounts), see examples/box-job.template.json");
    ensure!(v["version"] == 2, "Box job v2 required");
    exact(v, &present(v, &["version", "image", "workspace", "command", "network", "timeout_seconds", "memory_mb", "pids"], &["env", "mounts"]), "box job")?;
    let workspace = abs_existing(text(v, "workspace")?, "workspace")?;
    ensure!(workspace.is_dir(), "workspace must be an existing directory");
    let m = MAX_LIMITS;
    Ok(BoxJob {
        image: pinned_image(v)?, command: argv(&v["command"]).map_err(|_| Denied("command must start with an absolute container path, such as /usr/local/bin/node".into()))?,
        network: flag(v, "network")?, env: env_names(v)?, mounts: mounts(v, &[&workspace])?, timeout_seconds: int(v, "timeout_seconds", 1, m.timeout_seconds)?,
        memory_mb: int(v, "memory_mb", 32, m.memory_mb)?, pids: int(v, "pids", 1, m.pids)?, workspace,
    })
}
pub fn run_box_job(job: &BoxJob) -> R<Value> {
    let o = container::run(&job.spec())?;
    Ok(json!({
        "status": if o.exit_code == 0 { "OK" } else { "EXITED" }, "stage": "box", "exit_code": o.exit_code, "oom_killed": o.oom_killed,
        "workspace": job.workspace.display().to_string(), "image": job.image, "network": job.network, "engine": o.engine, "isolation": o.isolation, "container": o.container,
    }))
}

#[cfg(unix)]
fn editable(dir: &Path) -> R<()> {
    use std::os::unix::fs::PermissionsExt;
    for e in std::fs::read_dir(dir)? {
        let path = e?.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() { editable(&path)?; } else { std::fs::set_permissions(&path, std::fs::Permissions::from_mode(meta.permissions().mode() | 0o200))?; }
    }
    Ok(())
}
#[cfg(not(unix))]
fn editable(_: &Path) -> R<()> { Ok(()) }

pub fn read_workspace(root: &Path, base: &Files, file_bytes: u64, total_bytes: u64) -> R<Files> {
    let dirs = base.keys().flat_map(|k| k.match_indices('/').map(move |(i, _)| &k[..i])).collect();
    let mut r = WorkspaceReader { base, dirs, file_bytes, total_bytes, total: 0, files: Files::new() };
    r.read(root, "", false)?;
    for (k, e) in base.iter().filter(|(_, e)| e.link()) { r.files.entry(k.clone()).or_insert_with(|| e.clone()); }
    Ok(r.files)
}
struct WorkspaceReader<'a> { base: &'a Files, dirs: std::collections::HashSet<&'a str>, file_bytes: u64, total_bytes: u64, total: u64, files: Files }
impl WorkspaceReader<'_> {
    fn limits_hint(&self) -> String { format!("The contract allows {} bytes per file and {} in total; raise limits.file_bytes or limits.total_bytes if this is intended", self.file_bytes, self.total_bytes) }
    fn read(&mut self, dir: &Path, prefix: &str, tracked_only: bool) -> R<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            if tracked_only && !self.base.contains_key(&rel) && !self.dirs.contains(rel.as_str()) { continue; }
            let meta = std::fs::symlink_metadata(e.path())?;
            if meta.file_type().is_symlink() { deny!("Symlink in workspace is unsupported: {rel}. Kern only accepts regular files"); }
            if meta.is_dir() {
                let lower = name.to_ascii_lowercase();
                if lower == ".git" { continue; }
                self.read(&e.path(), &rel, tracked_only || lower == "node_modules")?;
            } else if meta.is_file() {
                file_path(&rel)?;
                let committed = self.base.get(&rel);
                let mode = committed.map_or("100644", |b| b.mode.as_str()).to_string();
                if meta.len() > self.file_bytes {
                    let unchanged = |b: &&Entry| b.bytes.is_none() && b.size == meta.len() && git::file_oid(&e.path(), meta.len()).is_ok_and(|oid| oid == b.oid);
                    let Some(asset) = committed.filter(unchanged) else { deny!("Workspace output exceeds byte limits at {rel}. {}", self.limits_hint()) };
                    self.files.insert(rel, Entry { mode, oid: asset.oid.clone(), size: meta.len(), bytes: None });
                    continue;
                }
                self.total += meta.len();
                ensure!(self.total <= self.total_bytes, "Workspace output exceeds byte limits at {rel}. {}", self.limits_hint());
                let data = std::fs::read(e.path())?;
                self.files.insert(rel, Entry { mode, oid: git::object_id("blob", &data), size: data.len() as u64, bytes: Some(data) });
            } else {
                deny!("Special file in workspace is unsupported: {rel}. Kern only accepts regular files");
            }
        }
        Ok(())
    }
}

pub enum Transport { Script(Pin, Vec<String>), Https(String, String) }
pub struct Verifier { pub transport: Transport, pub public_key_pem: String, pub public_key_path: PathBuf, pub operator: Operator }
fn verifier(v: &Value, dir: &Path) -> R<Verifier> {
    let transport = if v.get("url").is_some() {
        exact(v, &["url", "token_file", "public_key", "operator"], "verifier")?;
        let url = text(v, "url")?.trim_end_matches('/').to_string();
        ensure!(url.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p)), "The verifier URL must use https");
        let token = std::fs::read_to_string(dir.join(text(v, "token_file")?)).map_err(|e| Denied(format!("verifier token_file: {e}")))?.trim().to_string();
        ensure!(!token.is_empty() && !token.contains(['\r', '\n']), "The verifier token file must hold one bearer token");
        Transport::Https(url, token)
    } else {
        exact(v, &["transport", "arguments", "public_key", "operator"], "verifier")?;
        let transport = pin(&v["transport"], "verifier transport")?;
        pinned(&transport)?;
        let arguments = strings(v, "arguments", 32)?;
        ensure!(arguments.iter().all(|a| a.len() <= 4096 && !a.contains('\0')), "Invalid verifier transport arguments");
        Transport::Script(transport, arguments)
    };
    let public_key_path = std::path::absolute(dir.join(text(v, "public_key")?))?;
    let public_key_pem = std::fs::read_to_string(&public_key_path).map_err(|e| Denied(format!("verifier public_key: {e}")))?;
    crate::attest::public_key(&public_key_pem)?;
    let op = operator(&read_json(dir.join(text(v, "operator")?))?)?;
    Ok(Verifier { transport, public_key_pem, public_key_path, operator: op })
}

pub struct RunJob {
    pub repo: PathBuf, pub base: String, pub contract: Contract, pub image: String, pub command: Vec<String>, pub network: bool, pub env: Vec<String>, pub mounts: Vec<Mount>,
    pub output: PathBuf, pub promote: bool, pub verifier: Option<Verifier>, pub attempts: u64,
}
pub fn run_job(path: &Path) -> R<RunJob> {
    let file = std::path::absolute(path)?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let raw = read_json(&file)?;
    ensure!(raw["version"] != 1, "Run job v1 ran the agent on the retired Windows box; kern run now runs it in a Linux container. Replace tools with an image, \
        make command[0] a path inside that image and add read-only mounts for host files the agent needs, see examples/run-job.template.json");
    ensure!(raw["version"] == 2, "Run job v2 required");
    exact(&raw, &present(&raw, &["version", "repo", "base", "contract", "image", "command", "network", "output", "promote"], &["env", "mounts", "verifier", "attempts"]), "run job")?;
    let repo = strip_verbatim(&std::fs::canonicalize(dir.join(text(&raw, "repo")?)).map_err(|e| Denied(format!("repo: {e}")))?);
    ensure!(repo.is_dir(), "repo must be an existing directory");
    let base = text(&raw, "base")?.to_string();
    ensure!(is_oid(&base), "base must be a full 40-hex commit id");
    let contract = contract(&read_json(dir.join(text(&raw, "contract")?))?)?;
    let output = std::path::absolute(dir.join(text(&raw, "output")?))?;
    ensure!(!output.to_string_lossy().contains([',', '"', '\n', '\r']), "output cannot contain a comma, quote or line break, because the agent's workspace is mounted from it");
    let command = argv(&raw["command"]).map_err(|_| Denied("command must start with an absolute container path, such as /usr/local/bin/node".into()))?;
    let verifier = match raw.get("verifier") { Some(v) => Some(verifier(v, &dir)?), None => None };
    Ok(RunJob { image: pinned_image(&raw)?, command, network: flag(&raw, "network")?, env: env_names(&raw)?, mounts: mounts(&raw, &[&repo, &resolved(&output)])?,
        promote: flag(&raw, "promote")?, attempts: if raw.get("attempts").is_some() { int(&raw, "attempts", 1, 10)? } else { 1 }, repo, base, contract, output, verifier })
}

pub fn invoke_transport(v: &Verifier, handoff: &Path, out: &Path, commit: &str) -> R<(Value, Value)> {
    let _ = std::fs::remove_file(out);
    let beside = out.with_file_name("compare.json");
    let _ = std::fs::remove_file(&beside);
    let (pin, arguments) = match &v.transport {
        Transport::Script(pin, arguments) => (pin, arguments),
        Transport::Https(url, token) => {
            let body = json!({ "run": read_json(handoff.join("run.json"))?, "bundle": b64(&std::fs::read(handoff.join("artifact.bundle"))?), "evidence": true }).to_string();
            let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(1200))).build().into();
            let sent = agent.post(&format!("{url}/v1/verify")).header("Authorization", &format!("Bearer {token}")).header("Content-Type", "application/json").send(body);
            let mut r = sent.map_err(|e| Denied(format!("independent verifier unreachable: {e}")))?;
            let reply: Value = serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(format!("independent verifier unreachable: {e}")))?)?;
            ensure!(r.status().is_success(), "independent verifier rejected the change: {}", reply["reason"].as_str().unwrap_or("no reason given"));
            let (envelope, evidence) = if reply.get("attestation").is_some() { (reply["attestation"].clone(), reply["evidence"].clone()) } else { (reply, Value::Null) };
            create_new(out, format!("{envelope}\n").as_bytes(), 0o600)?;
            if !evidence.is_null() { create_new(&beside, format!("{evidence}\n").as_bytes(), 0o600)?; }
            return Ok((envelope, evidence));
        }
    };
    let script = strip_verbatim(&pinned(pin)?);
    let (exe, mut args): (PathBuf, Vec<String>) = if cfg!(windows) {
        (PathBuf::from("cmd.exe"), vec!["/c".into(), script.display().to_string()])
    } else {
        (PathBuf::from("/bin/sh"), vec![script.display().to_string()])
    };
    args.extend(arguments.iter().cloned());
    args.push(handoff.display().to_string());
    args.push(out.display().to_string());
    args.push(commit.to_string());
    run(&exe, &args, Run { cwd: None, env: safe_env(&std::env::temp_dir()), input: None, timeout: Duration::from_secs(1200), max_output: 1048576 })
        .map_err(|e| Denied(format!("independent verifier unreachable or rejected the change: {}", e.0)))?;
    ensure!(std::fs::symlink_metadata(out).is_ok_and(|m| m.is_file()), "the independent verifier returned no attestation");
    let evidence = if std::fs::symlink_metadata(&beside).is_ok_and(|m| m.is_file()) { read_json(&beside)? } else { Value::Null };
    Ok((read_json(out)?, evidence))
}
pub fn confirm_attestation(envelope: &Value, trusted_pem: &str, commit: &str) -> R<()> {
    exact(envelope, &["payloadType", "payload", "signatures"], "attestation envelope")?;
    ensure!(envelope["payloadType"] == attest::PAYLOAD_TYPE, "not an in-toto DSSE envelope");
    let key = attest::public_key(trusted_pem)?;
    let Some([sig]) = envelope["signatures"].as_array().map(Vec::as_slice) else { deny!("exactly one signature required") };
    exact(sig, &["keyid", "sig"], "signature")?;
    ensure!(text(sig, "keyid")? == attest::key_id(&key)?, "attestation is not signed by the trusted verifier key");
    let bytes = unb64(text(envelope, "payload")?)?;
    let signature = Signature::from_slice(&unb64(text(sig, "sig")?)?).map_err(|_| Denied("invalid signature encoding".into()))?;
    ensure!(key.verify_strict(&attest::pae(attest::PAYLOAD_TYPE, &bytes), &signature).is_ok(), "attestation signature does not verify against the trusted key");
    let stmt: Value = serde_json::from_slice(&bytes)?;
    ensure!(stmt["subject"][0]["digest"]["gitCommit"] == commit, "attestation does not bind the exact approved commit");
    ensure!(stmt["predicate"]["verificationResult"] == "PASSED", "independent verification did not pass");
    let Some(checks) = stmt["predicate"][attest::EXTENSION]["checks"].as_array() else { deny!("attestation carries no checks") };
    ensure!(!checks.is_empty() && checks.iter().all(|c| c["status"] == "PASS"), "a required check did not pass");
    Ok(())
}

fn gate<T>(label: &str, f: impl FnOnce() -> R<T>, checks: &mut Vec<Value>) -> Option<T> {
    match f() {
        Ok(v) => { checks.push(json!({ "id": label, "status": "PASS" })); Some(v) }
        Err(e) => { checks.push(json!({ "id": label, "status": "BLOCKED", "detail": e.0 })); None }
    }
}
fn blocked(task: &str, ux: Vec<&str>, reason: &str) -> Value {
    json!({ "status": "BLOCKED", "ux": ux, "task": task, "reason": reason })
}
fn plain(s: &str) -> String { s.chars().filter(|c| !c.is_control()).collect() }
fn reason_of(r: &Value) -> String {
    r["reason"].as_str().map(str::to_string)
        .or_else(|| r["checks"].as_array().into_iter().flatten().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str().map(str::to_string)))
        .unwrap_or_else(|| "the agent did not exit cleanly".into())
}
pub fn print_run(r: &Value) {
    let attempts = r["attempts"].as_array().cloned().unwrap_or_default();
    for a in attempts.iter().take(attempts.len().saturating_sub(1)) {
        say(&format!("\u{21bb} Attempt {} rejected: {}", a["attempt"], plain(a["feedback"]["reason"].as_str().unwrap_or("unknown"))));
    }
    if let Some(ux) = r["ux"].as_array() {
        for s in ux.iter().filter_map(Value::as_str) { say(&format!("\u{2713} {s}")); }
        say("");
        match r["status"].as_str() {
            Some("VERIFIED") => say(&format!("VERIFIED{}", r["accepted_after"].as_str().map_or(String::new(), |n| format!(" ({n})")))),
            _ => say(&format!("BLOCKED: {}", plain(r["reason"].as_str().unwrap_or("unknown")))),
        }
        if let Some(lines) = r["what_changed"].as_array() {
            say("");
            for l in lines.iter().filter_map(Value::as_str) { say(&format!("  {}", plain(l))); }
        }
    } else if let Some(s) = r["summary"].as_str() {
        say(&plain(s));
    }
}
pub fn local_run(job: &RunJob) -> R<Value> {
    let l = &job.contract.limits;
    let base_files = git::read_commit(&job.repo, &job.base, l.file_bytes, l.total_bytes)?;
    std::fs::create_dir_all(&job.output)?;
    let root = fresh_dir(job.output.join(format!("{}-{}", job.contract.task, short_id()?)))?;
    let ws = root.join("ws");
    git::materialize(Some(&job.repo), &base_files, &ws, None)?;
    editable(&ws)?;
    let (mut history, mut feedback) = (Vec::new(), None::<String>);
    for attempt in 1..=job.attempts {
        let dir = if job.attempts == 1 { root.clone() } else { fresh_dir(root.join(format!("attempt-{attempt}")))? };
        let mut result = run_attempt(job, &dir, &ws, &base_files, feedback.as_deref())?;
        let explained = (result["status"] == "BLOCKED").then(|| crate::feedback::explain(&reason_of(&result), Some(&job.contract)));
        history.push(json!({ "attempt": attempt, "status": result["status"], "feedback": explained.clone().unwrap_or(Value::Null) }));
        if attempt < job.attempts && let Some(mut f) = explained.as_ref().filter(|f| f["agent_fixable"] == true).map(crate::feedback::for_agent) {
            f["attempt"] = json!(attempt);
            f["max_attempts"] = json!(job.attempts);
            feedback = Some(f.to_string());
            continue;
        }
        if let Some(f) = explained { result["feedback"] = f; }
        if job.attempts > 1 {
            let before = (history.len() - 1) as u64;
            if before > 0 && result["status"] != "BLOCKED" {
                result["accepted_after"] = json!(crate::feedback::rejected(before));
                if let Some(s) = result["summary"].as_str() { result["summary"] = json!(format!("{s} Accepted {}.", crate::feedback::rejected(before))); }
            }
            result["attempts"] = json!(history);
        }
        write_json(root.join("result.json"), &result)?;
        return Ok(result);
    }
    deny!("A run job needs at least one attempt")
}
fn run_attempt(job: &RunJob, dir: &Path, ws: &Path, base_files: &Files, feedback: Option<&str>) -> R<Value> {
    let l = &job.contract.limits;
    let task = job.contract.task.clone();
    let delta = job.contract.delta.as_ref().map(|_| job.contract.value["delta"].to_string());
    let spec = BoxSpec { image: &job.image, command: &job.command, workspace: ws, mounts: &job.mounts, env: &job.env, network: job.network,
        timeout_seconds: l.timeout_seconds, memory_mb: l.memory_mb, pids: l.pids, feedback, delta: delta.as_deref() };
    let agent = match container::run(&spec) {
        Ok(o) => o,
        Err(e) => return Ok(blocked(&task, vec![], &e.0)),
    };
    let exit = agent.exit_code;
    let mut checks = vec![match exit {
        0 => json!({ "id": "agent-exit", "status": "PASS", "exit_code": 0 }),
        _ if agent.oom_killed => json!({ "id": "agent-exit", "status": "BLOCKED", "exit_code": exit, "detail": format!("The agent ran out of memory; the contract allows {} MB (limits.memory_mb)", l.memory_mb) }),
        _ => json!({ "id": "agent-exit", "status": "BLOCKED", "exit_code": exit, "detail": format!("The agent exited with code {exit}") }),
    }];
    let boxed = json!({ "engine": agent.engine, "isolation": agent.isolation, "image": job.image, "network": job.network });
    let candidate = gate("workspace", || read_workspace(ws, base_files, l.file_bytes, l.total_bytes), &mut checks);
    let empty: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let changed = candidate.as_ref().and_then(|c| gate("contract", || git::changes(base_files, c, &job.contract), &mut checks));
    let caps = candidate.as_ref().zip(changed.as_ref()).and_then(|(c, ch)| gate("capabilities", || capability::check(base_files, c, ch, &empty), &mut checks));
    let green = exit == 0 && checks.iter().all(|c| c["status"] == "PASS");
    let files_changed = changed.as_ref().and_then(|c| c["files"].as_array().map(Vec::len)).unwrap_or(0);

    if let Some(v) = &job.verifier {
        let Some(candidate) = candidate.as_ref().filter(|_| green) else {
            let reason = checks.iter().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("agent command exited with code {exit}"));
            return Ok(blocked(&task, vec!["Isolated"], &reason));
        };
        let outcome = (|| -> R<(String, Value, Value)> {
            let c = git::write_commit(&job.repo, candidate, Some(&job.base), &task)?;
            let pj = Job { repo: job.repo.clone(), base: job.base.clone(), contract: job.contract.clone(), operator: v.operator.clone(), command: None, verification_key: v.public_key_path.clone(), output: dir.join("ingest"), run_id: None };
            let ingest = flow::task(&pj, Source::Ingest(&job.repo, &c))?;
            let preflight = ingest["checks"].as_array().into_iter().flatten().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str());
            ensure!(ingest["status"] == "CANDIDATE", "{}", ingest["reason"].as_str().or(preflight).unwrap_or("the local gate rejected the change"));
            let bundle = PathBuf::from(text(&ingest, "bundle")?);
            let (envelope, evidence) = invoke_transport(v, &bundle.join("handoff"), &dir.join("attestation.json"), &c)?;
            confirm_attestation(&envelope, &v.public_key_pem, &c)?;
            Ok((c, envelope, evidence))
        })();
        return Ok(match outcome {
            Ok((c, envelope, evidence)) => {
                let mut r = json!({ "status": "VERIFIED", "ux": ["Isolated", "Verified independently", "Tests passed", "Signed"],
                    "task": task, "base": job.base, "commit": c, "files_changed": files_changed, "attestation_sha256": digest(&envelope).unwrap_or_default(), "checks": checks, "box": boxed });
                if crate::compare::attested(&envelope, &evidence) { r["what_changed"] = json!(crate::compare::summary(&evidence)); }
                r
            }
            Err(e) => blocked(&task, vec!["Isolated"], &e.0),
        });
    }

    let mut result = json!({ "status": if green { "GREEN" } else { "BLOCKED" }, "stage": "run", "task": task, "base": job.base,
        "network": job.network, "workspace": ws.display().to_string(), "box": boxed, "checks": checks,
        "changes": changed.clone().unwrap_or(Value::Null), "capabilities": caps.unwrap_or(Value::Null) });
    if let Some(candidate) = candidate.as_ref().filter(|_| green && job.promote) {
        let run_id = uuid4()?;
        let branch = format!("kern/{task}/{run_id}");
        let commit = git::write_commit(&job.repo, candidate, Some(&job.base), &task)?;
        git::git(&job.repo, &["update-ref", &format!("refs/heads/{branch}"), &commit, &"0".repeat(40)], None)?;
        result["summary"] = json!(format!("OK  {task}: {files_changed} file(s) changed, no new module capabilities. Promoted to {branch} (reversible)."));
        result["promotion"] = json!({ "branch": branch, "commit": commit, "review": format!("git -C {} diff {}..{branch}", job.repo.display(), job.base), "undo": format!("git -C {} branch -D {branch}", job.repo.display()) });
    } else if green {
        result["summary"] = json!(format!("OK  {task}: {files_changed} file(s) changed, verified. Promotion disabled; review {}.", ws.display()));
    } else {
        result["summary"] = json!(format!("BLOCKED  {task}: {}. Nothing was promoted; the real repository is untouched.", reason_of(&result)));
    }
    Ok(result)
}
