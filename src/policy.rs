use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const BUILTIN_CHECKS: [&str; 4] = ["artifact", "behavior", "capabilities", "path-and-budget"];
pub const HONEST_FIX: &str = "honest-fix";

fn identifier(s: &str, digit_first: bool) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && (b[0].is_ascii_lowercase() || (digit_first && b[0].is_ascii_digit())) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}
pub fn task_id(s: &str) -> bool { identifier(s, true) }
fn repository(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.len() <= 100 && p.bytes().all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)))
}
pub fn image(s: &str) -> bool {
    let digest = s.rsplit_once('@').map_or(s, |(name, d)| if name.bytes().all(|c| c.is_ascii_alphanumeric() || b"./:_-".contains(&c)) { d } else { "" });
    digest.strip_prefix("sha256:").is_some_and(is_hex64)
}

#[derive(Clone, Debug)]
pub struct Limits { pub files_changed: u64, pub lines_changed: u64, pub file_bytes: u64, pub total_bytes: u64, pub timeout_seconds: u64, pub memory_mb: u64, pub pids: u64 }
pub const MAX_LIMITS: Limits = Limits { files_changed: 1000, lines_changed: 100000, file_bytes: 1048576, total_bytes: 64 << 20, timeout_seconds: 3600, memory_mb: 8192, pids: 1024 };

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Bugfix, Feature, Refactor, Change }
impl Kind {
    pub fn name(self) -> &'static str { match self { Kind::Bugfix => "bugfix", Kind::Feature => "feature", Kind::Refactor => "refactor", Kind::Change => "change" } }
}
/// What may happen to existing tests: rewritten or removed when the verifier proves the old behaviour and the safety net intact, or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edits { Checked, Locked }
impl Edits {
    pub fn name(self) -> &'static str { match self { Edits::Checked => "checked", Edits::Locked => "locked" } }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner { Mocha, Jest, Vitest, Node }
impl Runner {
    pub fn name(self) -> &'static str { match self { Runner::Mocha => "mocha", Runner::Jest => "jest", Runner::Vitest => "vitest", Runner::Node => "node" } }
    pub fn parse(s: &str) -> Option<Runner> { [Runner::Mocha, Runner::Jest, Runner::Vitest, Runner::Node].into_iter().find(|r| r.name() == s) }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tests { pub paths: Vec<String>, pub kind: Kind, pub runner: Runner, pub command: Vec<String>, pub edits: Edits }
#[derive(Clone, Debug)]
pub struct Contract { pub value: Value, pub task: String, pub read: Vec<String>, pub write: Vec<String>, pub dependencies: bool, pub limits: Limits, pub checks: Vec<String>, pub behaviors: Vec<String>, pub tests: Option<Tests> }

pub fn contract(input: &Value) -> R<Contract> {
    let version = input.get("version").and_then(Value::as_u64);
    ensure!(matches!(version, Some(1..=4)), "Unsupported contract version");
    let mut fields = vec!["version", "task", "filesystem", "network", "secrets", "publish", "limits", "required_checks"];
    if version >= Some(2) { fields.push("dependencies"); }
    if version >= Some(3) { fields.push("behaviors"); }
    if version >= Some(4) { fields.push("tests"); }
    exact(input, &fields, "contract")?;
    let task = text(input, "task")?.to_string();
    ensure!(task_id(&task), "Invalid task ID");
    let fs = &input["filesystem"];
    exact(fs, &["read", "write"], "filesystem")?;
    let mut lists = Vec::new();
    for action in ["read", "write"] {
        let mut list = strings(fs, action, 256)?;
        for g in &list { capability(g)?; }
        ensure!(list.iter().enumerate().all(|(i, g)| list.iter().enumerate().all(|(j, h)| i == j || !covers(h, g))), "Overlapping or duplicate capabilities are ambiguous");
        ensure!(list.iter().all(|g| g.split('/').next() != Some("node_modules")), "Dependency trees are not grantable paths");
        list.sort();
        lists.push(list);
    }
    let (read, write) = (lists[0].clone(), lists[1].clone());
    ensure!(write.iter().all(|w| allowed(&read, w)), "Write authority requires read authority");
    tree_paths(read.iter().chain(&write).map(|g| g.strip_suffix('/').unwrap_or(g)), false)?;
    ensure!(input["network"].as_array().is_some_and(Vec::is_empty), "Contracts grant no network");
    ensure!(input["secrets"].as_array().is_some_and(Vec::is_empty), "Contracts grant no secrets");
    ensure!(input["publish"] == json!(false), "Agent publication is prohibited");
    let dependencies = match input.get("dependencies") {
        None | Some(Value::Null) => false,
        Some(d) => {
            exact(d, &["manager", "lockfile"], "dependencies")?;
            ensure!(d["manager"] == "npm" && d["lockfile"] == "package-lock.json", "Only npm package-lock.json dependency installation is supported");
            true
        }
    };
    let l = &input["limits"];
    exact(l, &["files_changed", "lines_changed", "file_bytes", "total_bytes", "timeout_seconds", "memory_mb", "pids"], "limits")?;
    let m = MAX_LIMITS;
    let limits = Limits {
        files_changed: int(l, "files_changed", 1, m.files_changed)?, lines_changed: int(l, "lines_changed", 1, m.lines_changed)?,
        file_bytes: int(l, "file_bytes", 1, m.file_bytes)?, total_bytes: int(l, "total_bytes", 1, m.total_bytes)?,
        timeout_seconds: int(l, "timeout_seconds", 1, m.timeout_seconds)?, memory_mb: int(l, "memory_mb", 32, m.memory_mb)?, pids: int(l, "pids", 1, m.pids)?,
    };
    ensure!(limits.file_bytes <= limits.total_bytes, "File limit exceeds total");
    let mut checks = strings(input, "required_checks", 16)?;
    checks.sort();
    ensure!(!checks.is_empty() && checks.windows(2).all(|w| w[0] != w[1]) && checks.iter().all(|c| identifier(c, false)), "Required checks must be unique nonempty IDs");
    let mut behaviors = if version >= Some(3) { strings(input, "behaviors", 64)? } else { Vec::new() };
    behaviors.sort();
    let known = |b: &String| b.len() <= 200 && !b.chars().any(char::is_control) && b.split_once(':').is_some_and(|(k, v)| ["net", "dns", "exec", "env"].contains(&k) && !v.is_empty());
    ensure!(behaviors.windows(2).all(|w| w[0] != w[1]) && behaviors.iter().all(known), "behaviors must be unique net:, dns:, exec: or env: entries");
    let mut value = input.clone();
    value["filesystem"] = json!({ "read": read, "write": write });
    value["required_checks"] = json!(checks);
    if version >= Some(3) { value["behaviors"] = json!(behaviors); }
    let tests = match input.get("tests") {
        None | Some(Value::Null) => None,
        Some(t) => {
            exact(t, if t.get("edits").is_some() { &["paths", "kind", "runner", "command", "edits"] } else { &["paths", "kind", "runner", "command"] }, "tests")?;
            let mut paths = strings(t, "paths", 16)?;
            ensure!(!paths.is_empty(), "tests.paths must name at least one test directory or file");
            for p in &paths { capability(p)?; }
            ensure!(paths.iter().enumerate().all(|(i, g)| paths.iter().enumerate().all(|(j, h)| i == j || !covers(h, g))), "Overlapping or duplicate capabilities are ambiguous");
            ensure!(paths.iter().all(|p| allowed(&read, p)), "tests.paths must lie inside filesystem.read, so the agent can read the tests it has to keep passing");
            paths.sort();
            let kind = match text(t, "kind")? { "bugfix" => Kind::Bugfix, "feature" => Kind::Feature, "refactor" => Kind::Refactor, "change" => Kind::Change, _ => deny!("tests.kind must be bugfix, feature, refactor or change") };
            ensure!(kind != Kind::Bugfix || paths.iter().all(|p| allowed(&write, p)), "A bug fix must be able to add a test that proves it: tests.paths must lie inside filesystem.write");
            let runner = t["runner"].as_str().and_then(Runner::parse).ok_or_else(|| Denied("tests.runner must be mocha, jest, vitest or node".into()))?;
            let command = argv(&t["command"])?;
            let reporter = |a: &String| a == "-R" || ["--reporter", "--json", "--test-reporter", "--outputFile", "--output-file", "--testResultsProcessor"].iter().any(|f| a.starts_with(f));
            ensure!(!command.iter().any(reporter), "tests.command must not choose a reporter or an output file: Kern adds the {} reporter it reads", runner.name());
            ensure!(runner != Runner::Vitest || command.iter().any(|a| a == "run"), "tests.command for vitest must use vitest run, so it runs once instead of watching for changes");
            ensure!(runner != Runner::Node || command.iter().any(|a| a == "--test"), "tests.command for node must include --test");
            let edits = match t.get("edits").map(Value::as_str) { None | Some(Some("checked")) => Edits::Checked, Some(Some("locked")) => Edits::Locked, _ => deny!("tests.edits must be checked or locked") };
            value["tests"]["paths"] = json!(paths);
            value["tests"]["edits"] = json!(edits.name());
            Some(Tests { paths, kind, runner, command, edits })
        }
    };
    Ok(Contract { value, task, read, write, dependencies, limits, checks, behaviors, tests })
}
pub fn attenuate(original: &Value, requested: &Value) -> R<Contract> {
    let (a, b) = (contract(original)?, contract(requested)?);
    ensure!(a.task == b.task, "Task identity cannot change");
    ensure!(b.read.iter().all(|g| allowed(&a.read, g)), "Cannot widen read authority");
    ensure!(b.write.iter().all(|g| allowed(&a.write, g)), "Cannot widen write authority");
    let (x, y) = (&a.limits, &b.limits);
    for (key, lo, hi) in [("files_changed", y.files_changed, x.files_changed), ("lines_changed", y.lines_changed, x.lines_changed), ("file_bytes", y.file_bytes, x.file_bytes), ("total_bytes", y.total_bytes, x.total_bytes), ("timeout_seconds", y.timeout_seconds, x.timeout_seconds), ("memory_mb", y.memory_mb, x.memory_mb), ("pids", y.pids, x.pids)] {
        ensure!(lo <= hi, "Cannot increase {key}");
    }
    ensure!(a.checks.iter().all(|c| b.checks.contains(c)), "Cannot remove a required check");
    ensure!(!b.dependencies || a.dependencies, "Cannot add dependency installation");
    ensure!(b.behaviors.iter().all(|x| a.behaviors.contains(x)), "Cannot allow new runtime behavior");
    ensure!(a.tests.is_none() || a.tests == b.tests, "Cannot change or remove the test proof");
    Ok(b)
}
pub fn contract_digest(c: &Contract) -> R<String> { digest(&c.value) }

#[derive(Clone, Debug)]
pub struct CheckSpec { pub id: String, pub file: PathBuf, pub sha256: String }
#[derive(Clone, Debug)]
pub struct Operator {
    pub value: Value, pub repository: String, pub multi_machine: bool, pub executors: Vec<String>, pub image: String,
    pub executor_builder: Option<String>, pub verifier_builder: String, pub checks: Vec<CheckSpec>, pub registry: Option<String>, pub permissions: BTreeMap<String, Vec<String>>,
}
pub fn operator(v: &Value) -> R<Operator> {
    exact(v, &["version", "repository", "deployment", "executors", "image", "builders", "checks", "registry", "permissions"], "operator")?;
    ensure!(v["version"] == 3, "Operator v3 required");
    let repo = text(v, "repository")?.to_string();
    ensure!(repository(&repo), "Repository identity must be owner/name");
    let multi_machine = match text(v, "deployment")? { "single-vm" => false, "multi-machine" => true, _ => deny!("Deployment must be single-vm or multi-machine") };
    let mut executors = strings(v, "executors", 2)?;
    executors.sort();
    ensure!(!executors.is_empty() && executors.windows(2).all(|w| w[0] != w[1]) && executors.iter().all(|e| e == "kern-gvisor" || e == "external"), "Executors must be kern-gvisor and/or external");
    let img = text(v, "image")?.to_string();
    ensure!(image(&img), "Image must be pinned by sha256 digest");
    let b = &v["builders"];
    exact(b, &["executor", "verifier"], "builders")?;
    let verifier_builder = text(b, "verifier")?.to_string();
    let executor_builder = b["executor"].as_str().map(str::to_string);
    ensure!(is_hex64(&verifier_builder) && (b["executor"].is_null() || executor_builder.as_deref().is_some_and(is_hex64)), "Builder identities must be SHA-256 digests");
    ensure!(executor_builder.is_some() == executors.iter().any(|e| e == "kern-gvisor"), "An executor builder is required exactly when kern-gvisor execution is accepted");
    ensure!(executor_builder.as_deref() != Some(verifier_builder.as_str()), "Executor and verifier identities must differ");
    let Some(raw) = v["checks"].as_array() else { deny!("checks: expected array") };
    ensure!(!raw.is_empty() && raw.len() <= 16, "1..16 trusted checks required");
    let mut checks: Vec<CheckSpec> = Vec::new();
    for c in raw {
        exact(c, &["id", "file", "sha256"], "operator check")?;
        let (id, file, sha) = (text(c, "id")?, text(c, "file")?, text(c, "sha256")?);
        ensure!(identifier(id, false) && !BUILTIN_CHECKS.contains(&id) && id != HONEST_FIX && !checks.iter().any(|x| x.id == id), "Duplicate or reserved check ID: {id}");
        ensure!((file.starts_with('/') || Path::new(file).is_absolute()) && is_hex64(sha), "Check {id} needs an absolute file and SHA-256 pin");
        checks.push(CheckSpec { id: id.into(), file: file.into(), sha256: sha.into() });
    }
    checks.sort_by(|a, b| a.id.cmp(&b.id));
    let registry = match &v["registry"] {
        Value::Null => None,
        Value::String(h) if !h.is_empty() && h.len() <= 253 && h.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-') => Some(h.clone()),
        _ => deny!("registry must be null or a lowercase hostname"),
    };
    let Some(p) = v["permissions"].as_object() else { deny!("permissions: expected object") };
    let mut permissions = BTreeMap::new();
    for prefix in p.keys() {
        capability(prefix)?;
        let list = strings(&v["permissions"], prefix, 256)?;
        ensure!(list.iter().all(|c| crate::capability::valid(c)), "Invalid capability in permissions for {prefix}");
        permissions.insert(prefix.clone(), list);
    }
    Ok(Operator { value: v.clone(), repository: repo, multi_machine, executors, image: img, executor_builder, verifier_builder, checks, registry, permissions })
}
pub fn implementation_digest() -> R<String> {
    static DIGEST: OnceLock<String> = OnceLock::new();
    if let Some(d) = DIGEST.get() { return Ok(d.clone()); }
    let d = sha256(std::fs::read(std::env::current_exe()?)?);
    Ok(DIGEST.get_or_init(|| d).clone())
}
pub fn verifier_digest(op: &Operator) -> R<String> {
    let checks: Vec<Value> = op.checks.iter().map(|c| json!({ "id": c.id, "sha256": c.sha256 })).collect();
    digest(&json!({
        "version": 4, "builders": op.value["builders"], "image": op.image, "checks": checks,
        "deployment": op.value["deployment"], "executors": op.executors, "registry": op.registry, "permissions": op.permissions, "capability_policy": crate::capability::POLICY,
    }))
}

#[derive(Clone, Debug)]
pub struct Pin { pub file: PathBuf, pub sha256: String }
pub fn pin(v: &Value, label: &str) -> R<Pin> {
    exact(v, &["file", "sha256"], label)?;
    let (file, sha) = (text(v, "file")?, text(v, "sha256")?);
    ensure!(Path::new(file).is_absolute() && is_hex64(sha), "{label}: absolute file and SHA-256 pin required");
    Ok(Pin { file: file.into(), sha256: sha.into() })
}
pub fn pinned(p: &Pin) -> R<PathBuf> {
    let meta = std::fs::symlink_metadata(&p.file).map_err(|e| Denied(format!("{}: {e}", p.file.display())))?;
    ensure!(meta.is_file(), "Pinned path must be a regular file: {}", p.file.display());
    ensure!(sha256(std::fs::read(&p.file)?) == p.sha256, "Pinned tool/policy changed: {}", p.file.display());
    Ok(std::fs::canonicalize(&p.file)?)
}

#[derive(Clone, Debug)]
pub struct Control { pub role: String, pub docker: Pin, pub endpoint: String, pub git: Pin, pub runtime: Pin, pub runtime_version: String, pub host_policy: Pin }
static CONTROL: OnceLock<Control> = OnceLock::new();
static GIT: OnceLock<Pin> = OnceLock::new();
pub const ENDPOINT: &str = if cfg!(windows) { "npipe:////./pipe/docker_engine" } else { "unix:///var/run/docker.sock" };

pub fn validate_control(v: &Value) -> R<Control> {
    exact(v, &["version", "role", "docker", "git", "runtime", "host_policy"], "control plane")?;
    ensure!(v["version"] == 1 && matches!(v["role"].as_str(), Some("executor" | "verifier")), "Explicit executor or verifier role required");
    let d = &v["docker"];
    exact(d, &["file", "sha256", "endpoint"], "Docker pin")?;
    ensure!(d["endpoint"] == ENDPOINT, "Only the fixed local Docker socket is permitted");
    let docker = pin(&json!({ "file": d["file"], "sha256": d["sha256"] }), "Docker pin")?;
    let (git, host_policy) = (pin(&v["git"], "Git pin")?, pin(&v["host_policy"], "host policy pin")?);
    for p in [&docker, &git, &host_policy] { pinned(p)?; }
    let r = &v["runtime"];
    exact(r, &["file", "sha256", "version", "arguments"], "runtime pin")?;
    let (file, sha, version) = (text(r, "file")?, text(r, "sha256")?, text(r, "version")?);
    ensure!(file.starts_with('/') && is_hex64(sha) && !version.is_empty() && version.len() < 1024, "Pinned Linux runtime required");
    ensure!(r["arguments"].as_array().is_some_and(Vec::is_empty), "Only reviewed default runsc arguments are permitted");
    Ok(Control { role: text(v, "role")?.into(), docker, endpoint: ENDPOINT.into(), git, runtime: Pin { file: file.into(), sha256: sha.into() }, runtime_version: version.into(), host_policy })
}
pub fn configure(v: &Value) -> R<()> {
    let c = validate_control(v)?;
    configure_git(c.git.clone())?;
    CONTROL.set(c).map_err(|_| Denied("Control plane cannot change during a process".into()))
}
pub fn configure_git(p: Pin) -> R<()> {
    pinned(&p)?;
    GIT.set(p).map_err(|_| Denied("Git pin cannot change during a process".into()))
}
pub fn control() -> R<&'static Control> { CONTROL.get().ok_or_else(|| Denied("Supply --control <operator-owned-control.json>; ambient Docker configuration is disabled".into())) }
pub fn require_role(role: &str) -> R<()> {
    ensure!(control()?.role == role, "This operation requires a separate {role} controller");
    Ok(())
}
pub fn git_exe() -> R<PathBuf> {
    match GIT.get() {
        Some(p) => pinned(p),
        None => Ok(PathBuf::from(if cfg!(windows) { r"C:\Program Files\Git\cmd\git.exe" } else { "/usr/bin/git" })),
    }
}

pub struct Job { pub repo: PathBuf, pub base: String, pub contract: Contract, pub operator: Operator, pub command: Option<Vec<String>>, pub verification_key: PathBuf, pub output: PathBuf }
pub fn job(path: &Path) -> R<Job> {
    let file = std::path::absolute(path)?;
    let dir = file.parent().unwrap_or(Path::new("/")).to_path_buf();
    let raw = read_json(&file)?;
    ensure!(raw["version"] == 3, "Job v3 required");
    exact(&raw, &["version", "repo", "base", "contract", "operator", "command", "verification_key", "output"], "job")?;
    let base = text(&raw, "base")?.to_string();
    ensure!(is_oid(&base), "Expected full SHA-1 base commit");
    let reference = |key: &str| -> R<PathBuf> {
        let s = text(&raw, key)?;
        ensure!(!s.is_empty() && !s.contains(['\0', '\r', '\n']), "Job references must be nonempty paths");
        Ok(std::path::absolute(dir.join(s))?)
    };
    let contract = contract(&read_json(reference("contract")?)?)?;
    let operator = operator(&read_json(reference("operator")?)?)?;
    let command = if raw["command"].is_null() { None } else { Some(argv(&read_json(reference("command")?)?)?) };
    ensure!(contract.checks.iter().all(|id| operator.checks.iter().any(|c| &c.id == id)), "Operator is missing a required check");
    Ok(Job { repo: reference("repo")?, base, contract, operator, command, verification_key: reference("verification_key")?, output: reference("output")? })
}
