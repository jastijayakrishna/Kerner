use crate::attest;
use crate::capability;
use crate::flow::{self, Source};
use crate::git::{self, Entry, Files};
use crate::policy::{Contract, Job, Operator, Pin, contract, operator, pin, pinned};
use crate::util::*;
use crate::{deny, ensure};
use ed25519_dalek::Signature;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access { ReadWrite, ReadExec, Deny }

pub struct BoxSpec<'a> {
    pub argv: &'a [String],
    pub grants: &'a [(PathBuf, Access)],
    pub net: bool,
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    pub timeout_ms: u32,
}

#[cfg(windows)]
pub fn enforce(spec: &BoxSpec) -> R<u32> { crate::winbox::run_box(spec) }
#[cfg(not(windows))]
pub fn enforce(_: &BoxSpec) -> R<u32> { deny!("The native Kern box currently runs on Windows only; use the gVisor executor on Linux") }

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() >= 2 && rest.as_bytes()[1] == b':' => PathBuf::from(rest),
        _ => p.to_path_buf(),
    }
}
fn abs_existing(s: &str, label: &str) -> R<PathBuf> {
    let p = Path::new(s);
    ensure!(p.is_absolute(), "{label} must be an absolute path: {s}");
    let c = std::fs::canonicalize(p).map_err(|e| Denied(format!("{label} {s}: {e}")))?;
    Ok(strip_verbatim(&c))
}

pub struct BoxJob { pub workspace: PathBuf, pub tools: Vec<PathBuf>, pub denied: Vec<PathBuf>, pub network: bool, pub command: Vec<String>, pub timeout_seconds: u64 }
pub fn box_job(v: &Value) -> R<BoxJob> {
    ensure!(v["version"] == 1, "Box job v1 required");
    exact(v, &["version", "workspace", "tools", "denied", "network", "command", "timeout_seconds"], "box job")?;
    let workspace = abs_existing(text(v, "workspace")?, "workspace")?;
    ensure!(workspace.is_dir(), "workspace must be an existing directory");
    let tools = strings(v, "tools", 32)?.iter().map(|t| abs_existing(t, "tool")).collect::<R<Vec<_>>>()?;
    ensure!(tools.iter().all(|t| t.is_dir()), "each tool must be an existing directory");
    let denied = strings(v, "denied", 64)?.iter().map(|d| abs_existing(d, "denied path")).collect::<R<Vec<_>>>()?;
    ensure!(v["network"].is_boolean(), "network must be a boolean");
    let timeout_seconds = int(v, "timeout_seconds", 1, 3600)?;
    let mut command = argv_native(&v["command"])?;
    let exe = abs_existing(&command[0], "command executable")?;
    ensure!(exe.is_file(), "command executable must be an existing file: {}", command[0]);
    command[0] = exe.display().to_string();
    Ok(BoxJob { workspace, tools, denied, network: v["network"] == true, command, timeout_seconds })
}
fn timeout_ms(seconds: u64) -> u32 { (seconds * 1000).min(u32::MAX as u64) as u32 }
fn argv_native(v: &Value) -> R<Vec<String>> {
    let Some(a) = v.as_array() else { deny!("command must be an array") };
    ensure!(!a.is_empty() && a.len() <= 256, "command must have 1..256 arguments");
    let args: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    ensure!(args.len() == a.len() && args.iter().all(|s| s.len() <= 8192 && !s.contains('\0')), "command arguments must be plain strings");
    Ok(args)
}

fn box_env(workspace: &Path, tools: &[PathBuf]) -> Vec<(String, String)> {
    let mut path: Vec<String> = tools.iter().map(|t| t.display().to_string()).collect();
    if let Ok(root) = std::env::var("SystemRoot") {
        path.push(format!(r"{root}\System32"));
        path.push(format!(r"{root}\System32\WindowsPowerShell\v1.0"));
        path.push(root);
    }
    let ws = workspace.display().to_string();
    let mut env = vec![("PATH".into(), path.join(";")), ("HOME".into(), ws.clone()), ("TEMP".into(), ws.clone()), ("TMP".into(), ws.clone())];
    for key in ["SystemRoot", "WINDIR", "SystemDrive", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "COMSPEC", "PATHEXT",
                "USERPROFILE", "LOCALAPPDATA", "APPDATA", "ALLUSERSPROFILE", "PROGRAMDATA", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "CommonProgramFiles", "PUBLIC", "OS", "COMPUTERNAME", "USERNAME"] {
        if let Ok(v) = std::env::var(key) { env.push((key.into(), v)); }
    }
    for (k, v) in [("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_CONFIG_GLOBAL", "NUL"), ("GIT_TERMINAL_PROMPT", "0"), ("GIT_OPTIONAL_LOCKS", "0")] {
        env.push((k.into(), v.into()));
    }
    env
}
pub fn grants_for(job: &BoxJob) -> Vec<(PathBuf, Access)> {
    let mut g = vec![(job.workspace.clone(), Access::ReadWrite)];
    g.extend(job.tools.iter().map(|t| (t.clone(), Access::ReadExec)));
    g.extend(job.denied.iter().map(|d| (d.clone(), Access::Deny)));
    g
}
pub fn run_box_job(job: &BoxJob) -> R<Value> {
    let grants = grants_for(job);
    let env = box_env(&job.workspace, &job.tools);
    let code = enforce(&BoxSpec { argv: &job.command, grants: &grants, net: job.network, cwd: &job.workspace, env: &env, timeout_ms: timeout_ms(job.timeout_seconds) })?;
    Ok(json!({
        "status": if code == 0 { "OK" } else { "EXITED" }, "stage": "box", "exit_code": code,
        "workspace": job.workspace.display().to_string(), "network": job.network,
        "granted": { "read_write": [job.workspace.display().to_string()], "read_execute": job.tools.iter().map(|t| t.display().to_string()).collect::<Vec<_>>(), "denied": job.denied.iter().map(|d| d.display().to_string()).collect::<Vec<_>>() },
    }))
}

fn run_env(ws: &Path, tmp: &Path, tools: &[PathBuf]) -> Vec<(String, String)> {
    let mut env = box_env(ws, tools);
    let tmp = tmp.display().to_string();
    for (k, v) in env.iter_mut() {
        if k == "TEMP" || k == "TMP" { *v = tmp.clone(); }
    }
    env
}

pub fn read_workspace(root: &Path, file_bytes: u64, total_bytes: u64) -> R<Files> {
    let mut files = Files::new();
    let mut total = 0u64;
    read_into(root, "", &mut files, &mut total, file_bytes, total_bytes)?;
    Ok(files)
}
fn read_into(dir: &Path, prefix: &str, files: &mut Files, total: &mut u64, file_bytes: u64, total_bytes: u64) -> R<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let name = e.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let meta = std::fs::symlink_metadata(e.path())?;
        if meta.file_type().is_symlink() { deny!("Symlink in workspace is unsupported: {rel}"); }
        if meta.is_dir() {
            let lower = name.to_ascii_lowercase();
            if lower == ".git" || lower == "node_modules" { continue; }
            read_into(&e.path(), &rel, files, total, file_bytes, total_bytes)?;
        } else if meta.is_file() {
            file_path(&rel)?;
            *total += meta.len();
            ensure!(meta.len() <= file_bytes && *total <= total_bytes, "Workspace output exceeds byte limits at {rel}");
            files.insert(rel, Entry { mode: "100644".into(), bytes: std::fs::read(e.path())? });
        } else {
            deny!("Special file in workspace is unsupported: {rel}");
        }
    }
    Ok(())
}

pub struct Verifier { pub transport: Pin, pub arguments: Vec<String>, pub public_key_pem: String, pub public_key_path: PathBuf, pub operator: Operator }
fn verifier(v: &Value, dir: &Path) -> R<Verifier> {
    exact(v, &["transport", "arguments", "public_key", "operator"], "verifier")?;
    let transport = pin(&v["transport"], "verifier transport")?;
    pinned(&transport)?;
    let arguments = strings(v, "arguments", 32)?;
    ensure!(arguments.iter().all(|a| a.len() <= 4096 && !a.contains('\0')), "Invalid verifier transport arguments");
    let public_key_path = std::path::absolute(dir.join(text(v, "public_key")?))?;
    let public_key_pem = std::fs::read_to_string(&public_key_path).map_err(|e| Denied(format!("verifier public_key: {e}")))?;
    crate::attest::public_key(&public_key_pem)?;
    let op = operator(&read_json(dir.join(text(v, "operator")?))?)?;
    Ok(Verifier { transport, arguments, public_key_pem, public_key_path, operator: op })
}

pub struct RunJob { pub repo: PathBuf, pub base: String, pub contract: Contract, pub tools: Vec<PathBuf>, pub command: Vec<String>, pub network: bool, pub output: PathBuf, pub promote: bool, pub verifier: Option<Verifier> }
pub fn run_job(path: &Path) -> R<RunJob> {
    let file = std::path::absolute(path)?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let raw = read_json(&file)?;
    ensure!(raw["version"] == 1, "Run job v1 required");
    let mut fields = vec!["version", "repo", "base", "contract", "tools", "command", "network", "output", "promote"];
    if raw.get("verifier").is_some() { fields.push("verifier"); }
    exact(&raw, &fields, "run job")?;
    let repo = strip_verbatim(&std::fs::canonicalize(dir.join(text(&raw, "repo")?)).map_err(|e| Denied(format!("repo: {e}")))?);
    ensure!(repo.is_dir(), "repo must be an existing directory");
    let base = text(&raw, "base")?.to_string();
    ensure!(is_oid(&base), "base must be a full 40-hex commit id");
    let contract = contract(&read_json(dir.join(text(&raw, "contract")?))?)?;
    let tools = strings(&raw, "tools", 32)?.iter().map(|t| abs_existing(t, "tool")).collect::<R<Vec<_>>>()?;
    ensure!(tools.iter().all(|t| t.is_dir()), "each tool must be an existing directory");
    ensure!(raw["network"].is_boolean() && raw["promote"].is_boolean(), "network and promote must be booleans");
    let mut command = argv_native(&raw["command"])?;
    let exe = abs_existing(&command[0], "command executable")?;
    ensure!(exe.is_file(), "command executable must exist: {}", command[0]);
    command[0] = exe.display().to_string();
    let verifier = match raw.get("verifier") { Some(v) => Some(verifier(v, &dir)?), None => None };
    Ok(RunJob { repo, base, contract, tools, command, network: raw["network"] == true, output: std::path::absolute(dir.join(text(&raw, "output")?))?, promote: raw["promote"] == true, verifier })
}

fn invoke_transport(v: &Verifier, handoff: &Path, out: &Path, commit: &str) -> R<Value> {
    let _ = std::fs::remove_file(out);
    let script = strip_verbatim(&pinned(&v.transport)?);
    let (exe, mut args): (PathBuf, Vec<String>) = if cfg!(windows) {
        (PathBuf::from("cmd.exe"), vec!["/c".into(), script.display().to_string()])
    } else {
        (PathBuf::from("/bin/sh"), vec![script.display().to_string()])
    };
    args.extend(v.arguments.clone());
    args.push(handoff.display().to_string());
    args.push(out.display().to_string());
    args.push(commit.to_string());
    run(&exe, &args, Run { cwd: None, env: safe_env(&std::env::temp_dir()), input: None, timeout: Duration::from_secs(1200), max_output: 1048576 })
        .map_err(|e| Denied(format!("independent verifier unreachable or rejected the change: {}", e.0)))?;
    ensure!(std::fs::symlink_metadata(out).is_ok_and(|m| m.is_file()), "the independent verifier returned no attestation");
    read_json(out)
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

fn gate(label: &str, f: impl FnOnce() -> R<Value>, checks: &mut Vec<Value>) -> Option<Value> {
    match f() {
        Ok(v) => { checks.push(json!({ "id": label, "status": "PASS" })); Some(v) }
        Err(e) => { checks.push(json!({ "id": label, "status": "BLOCKED", "detail": e.0 })); None }
    }
}
fn blocked(task: &str, ux: Vec<&str>, reason: &str) -> Value {
    json!({ "status": "BLOCKED", "ux": ux, "task": task, "reason": reason })
}
fn finish(root: &Path, result: Value) -> R<Value> {
    write_json(root.join("result.json"), &result)?;
    Ok(result)
}
pub fn print_run(r: &Value) {
    if let Some(ux) = r["ux"].as_array() {
        for s in ux.iter().filter_map(Value::as_str) { println!("\u{2713} {s}"); }
        println!();
        match r["status"].as_str() {
            Some("VERIFIED") => println!("VERIFIED"),
            _ => println!("BLOCKED: {}", r["reason"].as_str().unwrap_or("unknown")),
        }
    } else if let Some(s) = r["summary"].as_str() {
        println!("{s}");
    }
}
pub fn local_run(job: &RunJob) -> R<Value> {
    let l = &job.contract.limits;
    let base_files = git::read_commit(&job.repo, &job.base, l.file_bytes, l.total_bytes)?;
    std::fs::create_dir_all(&job.output)?;
    let root = fresh_dir(job.output.join(format!("{}-{}", job.contract.task, uuid4()?)))?;
    let ws = root.join("ws");
    let tmp = root.join("tmp");
    std::fs::create_dir_all(&tmp)?;
    git::materialize(&base_files, &ws, None)?;
    let mut grants = vec![(ws.clone(), Access::ReadWrite), (tmp.clone(), Access::ReadWrite)];
    grants.extend(job.tools.iter().map(|t| (t.clone(), Access::ReadExec)));
    let env = run_env(&ws, &tmp, &job.tools);
    let task = job.contract.task.clone();

    let exit = match enforce(&BoxSpec { argv: &job.command, grants: &grants, net: job.network, cwd: &ws, env: &env, timeout_ms: timeout_ms(l.timeout_seconds) }) {
        Ok(code) => code,
        Err(e) => return finish(&root, blocked(&task, vec![], &e.0)),
    };
    let mut checks = vec![json!({ "id": "agent-exit", "status": if exit == 0 { "PASS" } else { "BLOCKED" }, "detail": exit })];
    let candidate = read_workspace(&ws, l.file_bytes, l.total_bytes)?;
    let empty: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let changed = gate("contract", || git::changes(&base_files, &candidate, &job.contract), &mut checks);
    let caps = changed.as_ref().and_then(|ch| gate("capabilities", || capability::check(&base_files, &candidate, ch, &empty), &mut checks));
    let green = exit == 0 && checks.iter().all(|c| c["status"] == "PASS");
    let files_changed = changed.as_ref().and_then(|c| c["files"].as_array().map(Vec::len)).unwrap_or(0);

    if let Some(v) = &job.verifier {
        if !green {
            let reason = checks.iter().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str().map(str::to_string))
                .unwrap_or_else(|| format!("agent command exited with code {exit}"));
            return finish(&root, blocked(&task, vec!["Isolated"], &reason));
        }
        let outcome = (|| -> R<(String, Value)> {
            let c = git::write_commit(&job.repo, &candidate, Some(&job.base), &task)?;
            let pj = Job { repo: job.repo.clone(), base: job.base.clone(), contract: job.contract.clone(), operator: v.operator.clone(), command: None, verification_key: v.public_key_path.clone(), output: root.join("ingest") };
            let ingest = flow::task(&pj, Source::Ingest(&job.repo, &c))?;
            ensure!(ingest["status"] == "CANDIDATE", "{}", ingest["reason"].as_str().unwrap_or("the local gate rejected the change"));
            let bundle = PathBuf::from(text(&ingest, "bundle")?);
            let envelope = invoke_transport(v, &bundle.join("handoff"), &root.join("attestation.json"), &c)?;
            confirm_attestation(&envelope, &v.public_key_pem, &c)?;
            Ok((c, envelope))
        })();
        return match outcome {
            Ok((c, envelope)) => finish(&root, json!({ "status": "VERIFIED", "ux": ["Isolated", "Verified independently", "Tests passed", "Signed"],
                "task": task, "base": job.base, "commit": c, "files_changed": files_changed, "attestation_sha256": digest(&envelope).unwrap_or_default(), "checks": checks })),
            Err(e) => finish(&root, blocked(&task, vec!["Isolated"], &e.0)),
        };
    }

    let mut result = json!({ "status": if green { "GREEN" } else { "BLOCKED" }, "stage": "run", "task": task, "base": job.base,
        "network": job.network, "workspace": ws.display().to_string(), "checks": checks,
        "changes": changed.clone().unwrap_or(Value::Null), "capabilities": caps.unwrap_or(Value::Null) });
    if green && job.promote {
        let run_id = uuid4()?;
        let branch = format!("kern/{task}/{run_id}");
        let commit = git::write_commit(&job.repo, &candidate, Some(&job.base), &task)?;
        git::git(&job.repo, &["update-ref", &format!("refs/heads/{branch}"), &commit, &"0".repeat(40)], None)?;
        result["summary"] = json!(format!("OK  {task}: {files_changed} file(s) changed, no new module capabilities. Promoted to {branch} (reversible)."));
        result["promotion"] = json!({ "branch": branch, "commit": commit, "review": format!("git -C {} diff {}..{branch}", job.repo.display(), job.base), "undo": format!("git -C {} branch -D {branch}", job.repo.display()) });
    } else if green {
        result["summary"] = json!(format!("OK  {task}: {files_changed} file(s) changed, verified. Promotion disabled; review {}.", ws.display()));
    } else {
        let reason = checks.iter().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str().map(str::to_string)).unwrap_or_else(|| "agent did not exit cleanly".into());
        result["summary"] = json!(format!("BLOCKED  {task}: {reason}. Nothing was promoted; the real repository is untouched."));
    }
    write_json(root.join("result.json"), &result)?;
    Ok(result)
}
