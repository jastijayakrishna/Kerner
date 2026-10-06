use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
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
