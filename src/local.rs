use crate::capability;
use crate::git::{self, Entry, Files};
use crate::policy::{Contract, contract};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access { ReadWrite, ReadExec, Deny }

pub struct BoxSpec<'a> {
    pub argv: &'a [String],
    pub grants: &'a [(PathBuf, Access)],
    pub net: bool,
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
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

pub struct BoxJob { pub workspace: PathBuf, pub tools: Vec<PathBuf>, pub denied: Vec<PathBuf>, pub network: bool, pub command: Vec<String> }
pub fn box_job(v: &Value) -> R<BoxJob> {
    ensure!(v["version"] == 1, "Box job v1 required");
    exact(v, &["version", "workspace", "tools", "denied", "network", "command"], "box job")?;
    let workspace = abs_existing(text(v, "workspace")?, "workspace")?;
    ensure!(workspace.is_dir(), "workspace must be an existing directory");
    let tools = strings(v, "tools", 32)?.iter().map(|t| abs_existing(t, "tool")).collect::<R<Vec<_>>>()?;
    ensure!(tools.iter().all(|t| t.is_dir()), "each tool must be an existing directory");
    let denied = strings(v, "denied", 64)?.iter().map(|d| abs_existing(d, "denied path")).collect::<R<Vec<_>>>()?;
    ensure!(v["network"].is_boolean(), "network must be a boolean");
    let mut command = argv_native(&v["command"])?;
    let exe = abs_existing(&command[0], "command executable")?;
    ensure!(exe.is_file(), "command executable must be an existing file: {}", command[0]);
    command[0] = exe.display().to_string();
    Ok(BoxJob { workspace, tools, denied, network: v["network"] == true, command })
}
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
    let code = enforce(&BoxSpec { argv: &job.command, grants: &grants, net: job.network, cwd: &job.workspace, env: &env })?;
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

pub struct RunJob { pub repo: PathBuf, pub base: String, pub contract: Contract, pub tools: Vec<PathBuf>, pub command: Vec<String>, pub network: bool, pub output: PathBuf, pub promote: bool }
pub fn run_job(path: &Path) -> R<RunJob> {
    let file = std::path::absolute(path)?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let raw = read_json(&file)?;
    ensure!(raw["version"] == 1, "Run job v1 required");
    exact(&raw, &["version", "repo", "base", "contract", "tools", "command", "network", "output", "promote"], "run job")?;
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
    Ok(RunJob { repo, base, contract, tools, command, network: raw["network"] == true, output: std::path::absolute(dir.join(text(&raw, "output")?))?, promote: raw["promote"] == true })
}

fn gate(label: &str, f: impl FnOnce() -> R<Value>, checks: &mut Vec<Value>) -> Option<Value> {
    match f() {
        Ok(v) => { checks.push(json!({ "id": label, "status": "PASS" })); Some(v) }
        Err(e) => { checks.push(json!({ "id": label, "status": "BLOCKED", "detail": e.0 })); None }
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
    let exit = enforce(&BoxSpec { argv: &job.command, grants: &grants, net: job.network, cwd: &ws, env: &env })?;

    let mut checks = vec![json!({ "id": "agent-exit", "status": if exit == 0 { "PASS" } else { "BLOCKED" }, "detail": exit })];
    let candidate = read_workspace(&ws, l.file_bytes, l.total_bytes)?;
    let empty: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let changed = gate("contract", || git::changes(&base_files, &candidate, &job.contract), &mut checks);
    let caps = changed.as_ref().and_then(|ch| gate("capabilities", || capability::check(&base_files, &candidate, ch, &empty), &mut checks));
    let green = exit == 0 && checks.iter().all(|c| c["status"] == "PASS");

    let mut result = json!({
        "status": if green { "GREEN" } else { "BLOCKED" }, "stage": "run", "task": job.contract.task, "base": job.base,
        "network": job.network, "workspace": ws.display().to_string(), "checks": checks,
        "changes": changed.clone().unwrap_or(Value::Null), "capabilities": caps.unwrap_or(Value::Null),
    });
    let files_changed = changed.as_ref().and_then(|c| c["files"].as_array().map(Vec::len)).unwrap_or(0);
    let task = &job.contract.task;
    if green && job.promote {
        let run_id = uuid4()?;
        let branch = format!("kern/{}/{run_id}", job.contract.task);
        let commit = git::write_commit(&job.repo, &candidate, Some(&job.base), &job.contract.task)?;
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
