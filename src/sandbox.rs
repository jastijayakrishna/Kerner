use crate::policy::{Control, Limits, control, image, implementation_digest, pinned};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MONITOR: &str = include_str!("monitor.cjs");
pub const JEST_REPORT: &str = include_str!("jest-report.cjs");
pub type Invocation = (PathBuf, Vec<String>, Vec<(String, String)>);
pub fn docker_invocation(c: &Control, args: &[&str], command: &[&str], dir: &Path) -> R<Invocation> {
    ensure!(!args.iter().any(|a| ["--config", "--context", "--host", "--tls"].iter().any(|f| a == f || a.starts_with(&format!("{f}="))) || a.starts_with("-H") || a.starts_with("-c")), "Docker connection overrides are forbidden");
    let binary = pinned(&c.docker)?;
    let mut full = vec!["--config".to_string(), dir.display().to_string(), "--host".into(), c.endpoint.clone()];
    full.extend(args.iter().chain(command).map(|a| a.to_string()));
    let mut env: Vec<(String, String)> = safe_env(dir).into_iter().filter(|(k, _)| k != "PATH").collect();
    env.extend([("PATH".into(), String::new()), ("DOCKER_CONFIG".into(), dir.display().to_string()), ("XDG_CONFIG_HOME".into(), dir.display().to_string())]);
    Ok((binary, full, env))
}
pub const DRIVE_MARK: &str = "\u{1e}kern-drive ";
pub const DRIVE_PARTS: [&str; 5] = ["output", "errors", "coverage", "calls", "behavior"];
pub const TEST_PARTS: [&str; 4] = ["output", "errors", "behavior", "ending"];
pub fn test_dir(cwd: &str) -> R<String> {
    if cwd.is_empty() { return Ok("/workspace".into()); }
    ensure!(cwd.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains(['\\', '\0'])), "A test run starts in a folder inside the tree");
    Ok(format!("/workspace/{cwd}"))
}
pub fn runner_in(cwd: &str, command: &str, exists: impl Fn(&str) -> bool) -> String {
    let Some(name) = command.strip_prefix("/workspace/node_modules/.bin/") else { return command.to_string() };
    let mut dir = cwd.trim_matches('/').to_string();
    loop {
        let candidate = if dir.is_empty() { format!("/workspace/node_modules/.bin/{name}") } else { format!("/workspace/{dir}/node_modules/.bin/{name}") };
        if exists(&candidate) { return candidate; }
        if dir.is_empty() { return command.to_string(); }
        dir = dir.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
    }
}
pub fn drive_parts(stdout: &[u8]) -> R<std::collections::BTreeMap<String, Vec<u8>>> { parts(stdout, &DRIVE_PARTS) }
pub fn parts(stdout: &[u8], names: &[&str]) -> R<std::collections::BTreeMap<String, Vec<u8>>> {
    let (mut parts, mut at) = (std::collections::BTreeMap::new(), 0usize);
    for name in names {
        let head = format!("{DRIVE_MARK}{name} ");
        ensure!(stdout.get(at..at + head.len()) == Some(head.as_bytes()), "The run's report is incomplete");
        at += head.len();
        let nl = stdout[at..].iter().position(|b| *b == b'\n').map(|p| p + at).ok_or_else(|| Denied("The run's report is incomplete".into()))?;
        let len: usize = std::str::from_utf8(&stdout[at..nl]).ok().and_then(|s| s.parse().ok()).ok_or_else(|| Denied("The run's report is malformed".into()))?;
        let body = stdout.get(nl + 1..nl + 1 + len).ok_or_else(|| Denied("The run's report is incomplete".into()))?;
        parts.insert(name.to_string(), body.to_vec());
        at = nl + 1 + len;
    }
    ensure!(at == stdout.len(), "The run's report has unexpected data");
    Ok(parts)
}
pub const BEHAVIOR: &str = "\n\u{1e}kern-behavior\u{1e}\n";
fn with_docker<T>(args: &[&str], command: &[&str], f: impl FnOnce(&Path, &[String], Vec<(String, String)>, &Path) -> R<T>) -> R<T> {
    let c = control()?;
    let dir = fresh_dir(std::env::temp_dir().join(format!("kern-docker-{}", uuid4()?)))?;
    let config = dir.join("config.json");
    let result = create_new(&config, b"{}\n", 0o600).and_then(|_| {
        let (binary, full, env) = docker_invocation(c, args, command, &dir)?;
        f(&binary, &full, env, &dir)
    });
    let _ = std::fs::remove_file(&config);
    let _ = std::fs::remove_dir(&dir);
    result
}
pub fn docker(args: &[&str], timeout: Duration, max_output: usize) -> R<Vec<u8>> { docker_with(args, &[], timeout, max_output) }
fn docker_with(args: &[&str], command: &[&str], timeout: Duration, max_output: usize) -> R<Vec<u8>> {
    with_docker(args, command, |b, full, env, dir| run(b, full, Run { cwd: Some(dir), env, input: None, timeout, max_output }))
}
fn docker_full(args: &[&str], timeout: Duration, max_output: usize) -> R<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    with_docker(args, &[], |b, full, env, dir| run_full(b, full, Run { cwd: Some(dir), env, input: None, timeout, max_output }))
}
pub fn split_behavior(stdout: &[u8]) -> (&[u8], Option<std::collections::BTreeSet<String>>) {
    let mark = BEHAVIOR.as_bytes();
    match stdout.windows(mark.len()).rposition(|w| w == mark) {
        Some(i) => (&stdout[..i], Some(String::from_utf8_lossy(&stdout[i + mark.len()..]).lines().filter(|l| !l.is_empty() && !l.starts_with("count ")).map(str::to_string).collect())),
        None => (stdout, None),
    }
}
pub fn behavior_counts(stdout: &[u8]) -> std::collections::BTreeMap<String, u64> {
    let mark = BEHAVIOR.as_bytes();
    let mut counts = std::collections::BTreeMap::new();
    if let Some(i) = stdout.windows(mark.len()).rposition(|w| w == mark) {
        for key in String::from_utf8_lossy(&stdout[i + mark.len()..]).lines().filter_map(|l| l.strip_prefix("count ")) { *counts.entry(key.to_string()).or_insert(0) += 1; }
    }
    counts
}
fn docker_json(args: &[&str]) -> R<Value> { Ok(serde_json::from_slice(&docker(args, Duration::from_secs(60), 16 * 1048576)?)?) }

pub struct Doctor { pub ready: bool, pub builder: Option<(String, Value)>, pub report: Value }
pub fn doctor() -> Doctor {
    let attempt = || -> R<Doctor> {
        let info = docker_json(&["info", "--format", "{{json .}}"])?;
        let limits = ["MemoryLimit", "SwapLimit", "PidsLimit", "CpuCfsQuota"].iter().all(|k| info[*k] == true);
        let runtimes: Vec<String> = info["Runtimes"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let ready = info["OSType"] == "linux" && runtimes.iter().any(|r| r == "runsc") && limits;
        let builder = if ready { Some(builder_identity(&info, &docker_json(&["version", "--format", "{{json .}}"])?)?) } else { None };
        let report = json!({ "ready": ready, "builder": builder.as_ref().map(|b| json!({ "sha256": b.0, "identity": b.1 })), "os": info["OSType"], "runtimes": runtimes, "resource_limits": limits, "required": "runsc",
            "explanation": "Operator must pin genuine gVisor (runsc), its daemon mapping, toolchain and host policy, and enable memory/swap/PID/CPU limits. There is no fallback runtime." });
        Ok(Doctor { ready, builder, report })
    };
    attempt().unwrap_or_else(|e| Doctor { ready: false, builder: None, report: json!({ "ready": false, "required": "runsc", "explanation": e.0 }) })
}
fn builder_identity(info: &Value, versions: &Value) -> R<(String, Value)> {
    ensure!(cfg!(target_os = "linux"), "Production builder identity requires a native Linux controller");
    let c = control()?;
    let runtime = pinned(&c.runtime)?;
    let mapping = &info["Runtimes"]["runsc"];
    ensure!(mapping["path"].as_str() == Some(runtime.to_string_lossy().as_ref()) && mapping["runtimeArgs"].as_array().is_none_or(Vec::is_empty), "Daemon runtime mapping does not match the pinned runtime");
    let version = String::from_utf8_lossy(&run(&runtime, &["--version".into()], Run::default())?).trim().to_string();
    ensure!(version == c.runtime_version, "Runtime version changed");
    let git = pinned(&c.git)?;
    let git_version = String::from_utf8_lossy(&run(&git, &["--version".into()], Run::default())?).trim().to_string();
    let identity = json!({
        "version": 2, "role": c.role, "machine_id_sha256": sha256(std::fs::read("/etc/machine-id")?),
        "docker": { "file": c.docker.file, "sha256": c.docker.sha256, "endpoint": c.endpoint, "versions": versions },
        "runtime": { "file": runtime, "sha256": c.runtime.sha256, "version": version, "arguments": [] },
        "kern": { "version": env!("CARGO_PKG_VERSION"), "sha256": implementation_digest()? },
        "git": { "file": git, "sha256": c.git.sha256, "version": git_version },
        "host_policy": { "file": pinned(&c.host_policy)?, "sha256": c.host_policy.sha256 },
    });
    Ok((digest(&identity)?, identity))
}
pub fn validate_image(reference: &str) -> R<String> {
    ensure!(image(reference), "Image must be pinned by sha256 digest");
    let info = docker_json(&["image", "inspect", reference])?;
    let i = &info[0];
    ensure!(i["Os"] == "linux", "Linux image required");
    ensure!(i["Config"]["Volumes"].as_object().is_none_or(|v| v.is_empty()), "Images declaring volumes are refused");
    Ok(text(i, "Id")?.to_string())
}

pub struct Volume(pub String, bool);
impl Volume {
    pub fn create() -> R<Volume> {
        let name = format!("kern-deps-{}", uuid4()?);
        docker(&["volume", "create", "--label", "kern=dependencies", &name], Duration::from_secs(60), 4096)?;
        Ok(Volume(name, false))
    }
    pub fn cached(name: String) -> Volume { Volume(name, true) }
    pub fn keep(&mut self) { self.1 = true; }
    pub fn kept(&self) -> bool { self.1 }
}
impl Drop for Volume {
    fn drop(&mut self) { if !self.1 { remove_volume(&self.0); } }
}
pub fn volume_exists(name: &str) -> bool { docker(&["volume", "inspect", name], Duration::from_secs(60), 1 << 20).is_ok() }
pub fn yarnrc_without_code(text: &str) -> String {
    let (mut out, mut skipping) = (String::new(), false);
    for line in text.lines() {
        let top = !line.is_empty() && !line.starts_with([' ', '\t', '-', '#']);
        if top { skipping = ["yarnPath", "plugins"].iter().any(|k| line.strip_prefix(k).is_some_and(|r| r.trim_start().starts_with(':'))); }
        if !skipping { out.push_str(line); out.push('\n'); }
    }
    out
}
pub fn bun_for_npm(text: &str, root: &str) -> Option<String> {
    let mut pkg: Value = serde_json::from_str(text).ok()?;
    let root: Value = serde_json::from_str(root).unwrap_or(Value::Null);
    let catalog = |group: &str, name: &str| [&root, &root["workspaces"]].iter()
        .find_map(|at| (if group.is_empty() { &at["catalog"] } else { &at["catalogs"][group] })[name].as_str().map(str::to_string));
    let mut changed = false;
    if pkg["workspaces"].is_object() { pkg["workspaces"] = pkg["workspaces"]["packages"].take(); changed = true; }
    for field in ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"] {
        for (name, version) in pkg[field].as_object_mut().into_iter().flatten() {
            let new = match version.as_str() {
                Some(v) if v.starts_with("workspace:") => "*".to_string(),
                Some(v) if v.starts_with("catalog:") => catalog(v["catalog:".len()..].trim(), name).unwrap_or_else(|| "*".into()),
                _ => continue,
            };
            *version = Value::String(new);
            changed = true;
        }
    }
    changed.then(|| serde_json::to_string_pretty(&pkg).unwrap_or_default())
}
pub fn remove_volume(name: &str) { let _ = docker(&["volume", "rm", "--force", name], Duration::from_secs(120), 4096); }

#[derive(Clone)]
pub struct Spec<'a> {
    pub image: &'a str, pub snapshot: &'a Path, pub control: &'a Path, pub writable: &'a [String], pub limits: &'a Limits, pub entry: Vec<String>,
    pub network: &'a str, pub volumes: Vec<(&'a str, &'a str, bool)>, pub root: bool, pub home_mb: u64,
    pub scratch: bool,
    pub workspace: Option<(&'a str, bool)>,
}
pub const CACHE_DIRS: [&str; 3] = [".cache", ".vite", ".vite-temp"];
fn mount(source: &Path, target: &str) -> R<[String; 2]> {
    let s = std::path::absolute(source)?.display().to_string();
    ensure!(!s.contains([',', '\n', '"']), "Unsupported mount source");
    Ok(["--mount".into(), format!("type=bind,source={s},target={target},readonly")])
}
fn cpus(spec: &Spec) -> usize {
    if spec.entry.first().is_some_and(|e| e == "__drive_setup" || e == "__drive_test") { std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4) } else { 1 }
}
pub fn container_args(spec: &Spec, name: &str) -> R<Vec<String>> {
    let (drive, drive_setup) = (spec.entry.first().is_some_and(|e| e == "__drive"), spec.entry.first().is_some_and(|e| e == "__drive_setup"));
    let drive_test = spec.entry.first().is_some_and(|e| e == "__drive_test");
    ensure!(spec.network == "none" || (spec.network == "bridge" && spec.root && spec.writable.is_empty()), "Only dependency setup may use a network");
    if drive_test {
        ensure!(spec.workspace.is_some() && spec.network == "none" && !spec.root && spec.writable.is_empty() && !spec.scratch && spec.volumes.is_empty(),
            "A test run uses one installed tree, as an unprivileged user without a network or another volume");
    }
    ensure!(spec.volumes.iter().all(|(v, t, ro)| v.starts_with("kern-deps-") && (*t == "/workspace/node_modules" || (*t == "/kern-tools/node_modules" && *ro && drive))), "Unexpected volume");
    if drive { ensure!(matches!(spec.workspace, Some((_, true))) && spec.network == "none" && !spec.root && spec.writable.is_empty() && !spec.scratch, "A drive run reads an installed tree read-only, as an unprivileged user without a network"); }
    if drive_setup { ensure!(matches!(spec.workspace, Some((_, false))) && spec.network == "bridge" && spec.root && spec.volumes.is_empty() && spec.writable.is_empty() && !spec.scratch, "A drive setup installs into its own tree volume as root with a network, and mounts nothing else"); }
    ensure!(spec.workspace.is_none_or(|(v, _)| (drive || drive_setup || drive_test) && v.starts_with("kern-deps-")) && (spec.workspace.is_some() || !(drive || drive_setup || drive_test)), "Only drive runs, their test runs and their setup mount a tree volume, and only one Kern made");
    ensure!(spec.entry.first().is_some_and(|e| e.starts_with("__")), "Sandbox entry must be a Kern entry point");
    ensure!(!spec.scratch || (spec.writable.is_empty() && !spec.root && spec.network == "none"), "A scratch workspace is only for an unprivileged run without write grants or network");
    ensure!(spec.scratch || !matches!(spec.entry[0].as_str(), "__exec" | "__mutate"), "Tests run only on a throwaway copy of the workspace");
    let l = spec.limits;
    let user = if spec.root { "0:0" } else { "1000:1000" };
    let cpus = cpus(spec);
    let files = if drive || drive_setup || drive_test { "nofile=65536:65536" } else { "nofile=1024:1024" };
    let mut a: Vec<String> = [
        "create", "--name", name, "--pull=never", "--runtime=runsc", &format!("--network={}", spec.network), "--read-only", &format!("--user={user}"), "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true", "--pids-limit", &l.pids.to_string(), "--memory", &format!("{}m", l.memory_mb), "--memory-swap", &format!("{}m", l.memory_mb), &format!("--cpus={cpus}"),
        "--ulimit", files, "--ulimit", "core=0", "--ipc=private", "--shm-size=8m", "--log-driver=none", "--no-healthcheck", "--workdir=/workspace", "--entrypoint=/kern/kern",
        "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=67108864,mode=1777",
    ].iter().map(|s| s.to_string()).collect();
    match spec.workspace {
        Some((volume, readonly)) => {
            a.extend(["--mount".into(), format!("type=volume,source={volume},target=/workspace{}", if readonly { ",readonly" } else { "" })]);
            for dir in CACHE_DIRS.iter().filter(|_| readonly) {
                a.extend(["--tmpfs".into(), format!("/workspace/node_modules/{dir}:rw,nosuid,nodev,size=268435456,uid=1000,gid=1000,mode=0700")]);
            }
        }
        None if spec.scratch => a.extend(["--tmpfs".into(), format!("/workspace:rw,nosuid,nodev,size={},uid=1000,gid=1000,mode=0700", l.total_bytes * 2 + 268435456)]),
        None => a.extend(mount(spec.snapshot, "/workspace")?),
    }
    a.extend(mount(spec.snapshot, "/input")?);
    a.extend(mount(spec.control, "/kern")?);
    for grant in spec.writable {
        a.extend(["--tmpfs".into(), format!("/workspace/{}:rw,nosuid,nodev,size={},uid=1000,gid=1000,mode=0700", grant.trim_end_matches('/'), l.total_bytes)]);
    }
    if spec.home_mb > 0 { a.extend(["--tmpfs".into(), format!("/home/kern:rw,nosuid,nodev,size={}m,mode=0700", spec.home_mb)]); }
    for (volume, target, readonly) in &spec.volumes {
        a.extend(["--mount".into(), format!("type=volume,source={volume},target={target}{}", if *readonly { ",readonly" } else { "" })]);
        for dir in CACHE_DIRS.iter().filter(|_| *readonly && *target == "/workspace/node_modules") {
            a.extend(["--tmpfs".into(), format!("{target}/{dir}:rw,nosuid,nodev,size=268435456,uid=1000,gid=1000,mode=0700")]);
        }
    }
    a.push(spec.image.into());
    Ok(a)
}
pub struct Execution { pub stdout: Vec<u8>, pub exit_code: i64, pub builder: (String, Value) }
pub fn execute(spec: &Spec) -> R<Execution> {
    let status = doctor();
    ensure!(status.ready, "Sandbox unavailable: {}", status.report["explanation"].as_str().unwrap_or("runsc not ready"));
    let builder = status.builder.ok_or_else(|| Denied("Builder identity unavailable".into()))?;
    validate_image(spec.image)?;
    let name = format!("kern-{}", uuid4()?);
    let args = container_args(spec, &name)?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let entry: Vec<&str> = spec.entry.iter().map(String::as_str).collect();
    docker_with(&refs, &entry, Duration::from_secs(120), 65536)?;
    let outcome = (|| -> R<(Vec<u8>, i64)> {
        let i = &docker_json(&["inspect", &name])?[0];
        let h = &i["HostConfig"];
        let memory = spec.limits.memory_mb * 1048576;
        ensure!(h["Runtime"] == "runsc" && h["NetworkMode"] == spec.network && h["ReadonlyRootfs"] == true && i["Config"]["User"] == if spec.root { "0:0" } else { "1000:1000" }, "Container configuration does not match the required boundary");
        ensure!(h["Memory"] == memory && h["MemorySwap"] == memory && h["PidsLimit"] == spec.limits.pids && h["NanoCpus"] == cpus(spec) as u64 * 1_000_000_000, "Container resource limits not applied");
        ensure!(h["Privileged"] == false && h["CapDrop"].as_array().is_some_and(|c| c.iter().any(|x| x == "ALL")) && h["SecurityOpt"].as_array().is_some_and(|s| s.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("no-new-privileges")))), "Container privilege constraints not applied");
        let mounts = i["Mounts"].as_array().cloned().unwrap_or_default();
        let binds: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "bind").collect();
        let volumes: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "volume").collect();
        ensure!(binds.len() == if spec.scratch || spec.workspace.is_some() { 2 } else { 3 } && binds.iter().all(|m| m["RW"] == false), "Unexpected or writable host mount");
        let tree: Vec<(&str, &str, bool)> = spec.workspace.iter().map(|(v, ro)| (*v, "/workspace", *ro)).collect();
        ensure!(volumes.len() == spec.volumes.len() + tree.len() && spec.volumes.iter().chain(&tree).all(|(v, t, ro)| volumes.iter().any(|m| m["Name"] == *v && m["Destination"] == *t && m["RW"] == !ro)), "Unexpected volume mount");
        ensure!(mounts.iter().all(|m| m["Type"] == "bind" || m["Type"] == "volume" || m["Type"] == "tmpfs"), "Unexpected mount type");
        let (_, out, _) = docker_full(&["start", "--attach", &name], Duration::from_secs(spec.limits.timeout_seconds + 5), (spec.limits.total_bytes as usize) * 2 + 1048576)?;
        let f = &docker_json(&["inspect", &name])?[0]["State"];
        if f["OOMKilled"] == true { crate::deny!("Sandbox exceeded limits: it ran out of memory (it allows {} MiB)", spec.limits.memory_mb); }
        ensure!(f["Running"] == false && f["ExitCode"].is_i64(), "Sandbox failed or exceeded limits");
        Ok((out, f["ExitCode"].as_i64().unwrap_or(-1)))
    })();
    docker(&["rm", "--force", &name], Duration::from_secs(120), 65536).map_err(|e| Denied(format!("Sandbox removal failed; no artifact accepted: {e}")))?;
    let (stdout, exit_code) = outcome?;
    Ok(Execution { stdout, exit_code, builder })
}
pub fn control_dir(dir: &Path, config: Option<(&str, &Value)>) -> R<()> {
    std::fs::create_dir(dir)?;
    let target = dir.join("kern");
    std::fs::copy(std::env::current_exe()?, &target)?;
    #[cfg(unix)]
    std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o555))?;
    create_new(dir.join("monitor.cjs"), MONITOR.as_bytes(), 0o444)?;
    create_new(dir.join("jest-report.cjs"), JEST_REPORT.as_bytes(), 0o444)?;
    create_new(dir.join("probe.cjs"), crate::compare::PROBE.as_bytes(), 0o444)?;
    if let Some((name, value)) = config { create_new(dir.join(name), format!("{}\n", canonical(value)?).as_bytes(), 0o444)?; }
    Ok(())
}

#[cfg(unix)]
mod inside {
    use super::*;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::process::{Command, Stdio};

    const ENV: [(&str, &str); 3] = [("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/tmp"), ("LANG", "C.UTF-8")];
    const TEST_ENV: [(&str, &str); 4] = [ENV[0], ENV[1], ENV[2], ("CI", "true")];
    fn config(name: &str) -> R<Value> { read_json(Path::new("/kern").join(name)) }
    fn wait(mut child: std::process::Child, timeout: Duration) -> R<bool> {
        let started = std::time::Instant::now();
        loop {
            if let Some(s) = child.try_wait()? { return Ok(s.success()); }
            if started.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); return Ok(false); }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fn copy_tree(from: &Path, to: &Path) -> R<()> {
        std::fs::create_dir_all(to)?;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o700))?;
        if !from.exists() { return Ok(()); }
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.is_dir() { copy_tree(&src, &dst)?; } else if meta.is_file() {
                std::fs::copy(&src, &dst)?;
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(if meta.mode() & 0o111 != 0 { 0o700 } else { 0o600 }))?;
            } else { deny!("Unsupported input file type"); }
        }
        Ok(())
    }
    fn quiesce() -> R<()> {
        ensure!(std::process::id() == 1, "Capture requires namespace init");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            unsafe {
                libc::kill(-1, libc::SIGKILL);
                while libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) > 0 {}
            }
            let mut live = 0;
            for e in std::fs::read_dir("/proc")?.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name == "1" || !name.bytes().all(|b| b.is_ascii_digit()) { continue; }
                if let Ok(stat) = std::fs::read_to_string(format!("/proc/{name}/stat")) {
                    let state = stat.rsplit_once(')').map(|(_, r)| r.trim_start().chars().next().unwrap_or('?')).unwrap_or('?');
                    if !matches!(state, 'Z' | 'X' | 'x') { live += 1; }
                }
            }
            if live == 0 { return Ok(()); }
            ensure!(std::time::Instant::now() < deadline, "Agent namespace did not quiesce");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn walk(dir: &Path, prefix: &str, depth: u32, limits: (u64, u64), state: &mut (u64, u64, Vec<Value>)) -> R<()> {
        ensure!(depth <= 32, "Output depth limit");
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            state.1 += 1;
            ensure!(state.1 <= 10000, "Output entry limit");
            let name = e.file_name().to_string_lossy().to_string();
            let (path, meta) = (e.path(), std::fs::symlink_metadata(e.path())?);
            if meta.file_type().is_symlink() { deny!("Symlink output refused"); }
            if meta.is_dir() { walk(&path, &format!("{prefix}{name}/"), depth + 1, limits, state)?; continue; }
            ensure!(meta.is_file() && meta.nlink() == 1, "Special or linked output refused");
            state.0 += meta.len();
            ensure!(meta.len() <= limits.0 && state.0 <= limits.1, "Output byte limit");
            let mut file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(&path)?;
            let mut bytes = Vec::with_capacity(meta.len() as usize + 1);
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, meta.len() + 1), &mut bytes)?;
            ensure!(bytes.len() as u64 == meta.len(), "Unstable output file");
            state.2.push(json!({ "path": format!("{prefix}{name}"), "content": b64(&bytes) }));
        }
        Ok(())
    }
    pub fn runner() -> R<()> {
        let c = config("config.json")?;
        let write = strings(&c, "write", 64)?;
        let command = argv(&c["command"])?;
        let l = &c["limits"];
        for grant in &write {
            copy_tree(&Path::new("/input").join(grant.trim_end_matches('/')), &Path::new("/workspace").join(grant.trim_end_matches('/')))?;
        }
        let child = Command::new(&command[0]).args(&command[1..]).current_dir("/workspace").env_clear().envs(ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        ensure!(wait(child, Duration::from_secs(int(l, "timeout_seconds", 1, 3600)?))?, "Agent command failed or timed out");
        quiesce()?;
        let mut state = (0u64, 0u64, Vec::new());
        for grant in &write {
            walk(&Path::new("/workspace").join(grant.trim_end_matches('/')), grant, 0, (int(l, "file_bytes", 1, u64::MAX)?, int(l, "total_bytes", 1, u64::MAX)?), &mut state)?;
        }
        println!("{}", json!({ "version": 1, "files": state.2 }));
        Ok(())
    }
    fn tree_digest(dir: &Path, prefix: &str, out: &mut Vec<Value>, totals: &mut (u64, u64)) -> R<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let (path, name) = (e.path(), format!("{prefix}{}", e.file_name().to_string_lossy()));
            let meta = std::fs::symlink_metadata(&path)?;
            totals.0 += 1;
            ensure!(totals.0 <= 300_000, "Dependency tree entry limit");
            if meta.file_type().is_symlink() { out.push(json!({ "path": name, "link": std::fs::read_link(&path)?.to_string_lossy() })); }
            else if meta.is_dir() { tree_digest(&path, &format!("{name}/"), out, totals)?; }
            else if meta.is_file() {
                totals.1 += meta.len();
                ensure!(totals.1 <= 4 << 30, "Dependency tree byte limit");
                out.push(json!({ "path": name, "executable": meta.mode() & 0o111 != 0, "size": meta.len(), "sha256": sha256(std::fs::read(&path)?) }));
            } else { deny!("Special file in dependency tree"); }
        }
        Ok(())
    }
    pub fn setup() -> R<()> {
        let c = config("setup.json")?;
        let registry = text(&c, "registry")?;
        let args = ["/usr/local/lib/node_modules/npm/bin/npm-cli.js", "ci", "--ignore-scripts", "--no-audit", "--no-fund", "--no-update-notifier", &format!("--registry=https://{registry}/"), "--replace-registry-host=always", "--cache=/home/kern/.npm"];
        let child = Command::new("/usr/local/bin/node").args(args).current_dir("/workspace").env_clear().envs([("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/home/kern"), ("LANG", "C.UTF-8"), ("npm_config_loglevel", "error")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
        ensure!(wait(child, Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?))?, "npm ci failed or timed out");
        let (mut entries, mut totals) = (Vec::new(), (0u64, 0u64));
        tree_digest(Path::new("/workspace/node_modules"), "", &mut entries, &mut totals)?;
        for dir in CACHE_DIRS { std::fs::create_dir_all(Path::new("/workspace/node_modules").join(dir))?; }
        println!("{}", json!({ "version": 1, "entries": totals.0, "bytes": totals.1, "sha256": digest(&json!(entries))? }));
        Ok(())
    }
    fn copy_entry(from: &Path, to: &Path) -> R<()> {
        let meta = std::fs::symlink_metadata(from)?;
        if meta.is_dir() { return copy_tree(from, to); }
        ensure!(meta.is_file(), "Unsupported input file type");
        std::fs::copy(from, to)?;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(if meta.mode() & 0o111 != 0 { 0o700 } else { 0o600 }))?;
        Ok(())
    }
    fn remove(path: &Path) -> R<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.is_dir() => Ok(std::fs::remove_dir_all(path)?),
            Ok(_) => Ok(std::fs::remove_file(path)?),
            Err(_) => Ok(()),
        }
    }
    pub fn mutate() -> R<()> {
        let plan = config("mutate.json")?;
        exact(&plan, &["version", "command", "tests_paths", "budget_seconds", "old_files", "runner", "known", "mutants"], "planting plan")?;
        ensure!(plan["version"] == 1, "Unsupported planting plan");
        let command = argv(&plan["command"])?;
        let runner = crate::policy::Runner::parse(text(&plan, "runner")?).ok_or_else(|| Denied("Unsupported planting plan".into()))?;
        let known: std::collections::BTreeSet<String> = strings(&plan, "known", 100_000)?.into_iter().collect();
        let paths = strings(&plan, "tests_paths", 16)?;
        for p in &paths { capability(p)?; }
        let budget = Duration::from_secs(int(&plan, "budget_seconds", 1, 3600)?);
        let old_files = strings(&plan, "old_files", 10000)?;
        for f in &old_files { file_path(f)?; }
        let mutants = plan["mutants"].as_array().cloned().unwrap_or_default();
        ensure!(mutants.len() <= 200, "Too many planted bugs");
        let started = std::time::Instant::now();
        let work = Path::new("/workspace");
        let reset = |old: bool| -> R<()> {
            for e in std::fs::read_dir(work)? { let e = e?; if e.file_name() != "node_modules" { remove(&e.path())?; } }
            for e in std::fs::read_dir("/tmp")? { remove(&e?.path())?; }
            for e in std::fs::read_dir("/input")? { let e = e?; if e.file_name() != "node_modules" { copy_entry(&e.path(), &work.join(e.file_name()))?; } }
            if old {
                for p in &paths {
                    let rel = p.trim_end_matches('/');
                    remove(&work.join(rel))?;
                    let source = Path::new("/kern/old").join(rel);
                    if source.exists() {
                        if let Some(parent) = work.join(rel).parent() { std::fs::create_dir_all(parent)?; }
                        copy_entry(&source, &work.join(rel))?;
                    }
                }
            }
            Ok(())
        };
        let errors = Path::new("/tmp/.kern-baseline-errors");
        let run = |old: bool, mutant: Option<&Value>, files: &[String], limit: Option<Duration>| -> R<Option<bool>> {
            let Some(timeout) = crate::proof::run_limit(budget.saturating_sub(started.elapsed()), limit) else { return Ok(None) };
            reset(old)?;
            if let Some(m) = mutant {
                let path = text(m, "path")?;
                file_path(path)?;
                let (start, end) = (int(m, "start", 0, u32::MAX.into())? as usize, int(m, "end", 0, u32::MAX.into())? as usize);
                let target = work.join(path);
                let mut bytes = std::fs::read(&target)?;
                ensure!(start <= end && bytes.get(start..end) == Some(text(m, "original")?.as_bytes()), "A planted bug does not match its file");
                bytes.splice(start..end, text(m, "replacement")?.bytes());
                std::fs::write(&target, bytes)?;
            }
            let stderr = if mutant.is_none() { Stdio::from(std::fs::File::create(errors)?) } else { Stdio::null() };
            let stdout = if known.is_empty() { Stdio::null() } else { Stdio::piped() };
            let mut child = Command::new(&command[0]).args(&command[1..]).args(files).current_dir(work).env_clear().envs(TEST_ENV).stdin(Stdio::null()).stdout(stdout).stderr(stderr).spawn()?;
            let reader = child.stdout.take().map(|out| std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut std::io::Read::take(out, 64 << 20), &mut buf);
                buf
            }));
            let passed = wait(child, timeout)?;
            quiesce()?;
            let report = reader.map(|r| r.join().unwrap_or_default()).unwrap_or_default();
            Ok(Some(crate::proof::caught(runner, &known, &report, passed)))
        };
        let timed = |old: bool, files: &[String]| -> R<(bool, Duration, String)> {
            let at = std::time::Instant::now();
            let ok = run(old, None, files, None)? == Some(false);
            let error = if ok { String::new() } else { std::fs::read(errors).map(|b| String::from_utf8_lossy(&b[b.len().saturating_sub(2000)..]).to_string()).unwrap_or_default() };
            Ok((ok, at.elapsed(), error))
        };
        let (mut old_args, mut old_base) = (old_files.clone(), timed(true, &old_files)?);
        if !old_base.0 && !old_files.is_empty() { (old_args, old_base) = (Vec::new(), timed(true, &[])?); }
        let ((old_ok, old_time, old_error), (new_ok, new_time, new_error)) = (old_base, timed(false, &[])?);
        let mut results = Vec::new();
        if old_ok && new_ok {
            let (old_limit, new_limit) = (old_time * 3 + Duration::from_secs(5), new_time * 3 + Duration::from_secs(5));
            let usable: std::cell::RefCell<std::collections::HashMap<Vec<String>, bool>> = Default::default();
            for m in &mutants {
                let focus = strings(m, "tests", 10000)?;
                for f in &focus { file_path(f)?; }
                let result = crate::proof::decide(&mut |suite| match suite {
                    crate::proof::Suite::Old => run(true, Some(m), &old_args, Some(old_limit)),
                    crate::proof::Suite::NewFull => run(false, Some(m), &[], Some(new_limit)),
                    crate::proof::Suite::NewFocused => {
                        let known = usable.borrow().get(&focus).copied();
                        let fine = match known { Some(fine) => fine, None => { let fine = run(false, None, &focus, Some(new_limit))? == Some(false); usable.borrow_mut().insert(focus.clone(), fine); fine } };
                        if fine { run(false, Some(m), &focus, Some(new_limit)) } else { Ok(Some(false)) }
                    }
                }, !focus.is_empty())?;
                results.push(json!({ "id": m["id"], "result": result }));
            }
        }
        println!("{}", json!({ "version": 1, "baseline": { "old": old_ok, "new": new_ok, "old_error": old_error, "new_error": new_error },
            "milliseconds": { "old": old_time.as_millis() as u64, "new": new_time.as_millis() as u64, "total": started.elapsed().as_millis() as u64 }, "mutants": results }));
        Ok(())
    }
    fn copy_readable(from: &Path, to: &Path) -> R<()> {
        std::fs::create_dir_all(to)?;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755))?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.is_dir() { copy_readable(&src, &dst)?; }
            else if meta.is_file() {
                std::fs::copy(&src, &dst)?;
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(if meta.mode() & 0o111 != 0 { 0o755 } else { 0o644 }))?;
            }
        }
        Ok(())
    }
    fn install_plan(dir: &Path) -> (&'static str, Vec<Vec<String>>) {
        let node = |args: &[&str]| -> Vec<String> { ["/usr/local/bin/node"].iter().chain(args).map(|s| s.to_string()).collect() };
        let npm = "/usr/local/lib/node_modules/npm/bin/npm-cli.js";
        let corepack = "/usr/local/lib/node_modules/corepack/dist/corepack.js";
        let quiet = ["--no-audit", "--no-fund", "--no-update-notifier"];
        if dir.join("pnpm-lock.yaml").is_file() {
            let store = "--store-dir=/workspace/.kern-pnpm-store";
            return ("pnpm", vec![node(&[corepack, "pnpm", "install", "--frozen-lockfile", "--ignore-scripts", "--ignore-pnpmfile", store]),
                node(&[corepack, "pnpm", "install", "--no-frozen-lockfile", "--ignore-scripts", "--ignore-pnpmfile", store])]);
        }
        if dir.join("yarn.lock").is_file() {
            if dir.join(".yarnrc.yml").is_file() { return ("yarn", vec![node(&[corepack, "yarn", "install", "--immutable", "--mode=skip-build"]), node(&[corepack, "yarn", "install", "--mode=skip-build"])]); }
            return ("yarn", vec![node(&[corepack, "yarn", "install", "--frozen-lockfile", "--ignore-scripts", "--non-interactive"]), node(&[corepack, "yarn", "install", "--ignore-scripts", "--non-interactive"])]);
        }
        let plain: Vec<String> = node(&[npm, "install", "--ignore-scripts", "--no-package-lock"]).into_iter().chain(quiet.iter().map(|s| s.to_string())).collect();
        if dir.join("package-lock.json").is_file() || dir.join("npm-shrinkwrap.json").is_file() {
            return ("npm", vec![node(&[npm, "ci", "--ignore-scripts"]).into_iter().chain(quiet.iter().map(|s| s.to_string())).collect(), plain]);
        }
        if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
            return ("bun", vec![plain.into_iter().chain(["--legacy-peer-deps".to_string()]).collect()]);
        }
        ("npm", vec![plain])
    }
    fn bun_rewrite(dir: &Path) -> R<Vec<(PathBuf, Vec<u8>)>> {
        let root = std::fs::read_to_string(dir.join("package.json"))?;
        let (mut originals, mut folders) = (Vec::new(), vec![(dir.to_path_buf(), 0)]);
        while let Some((folder, depth)) = folders.pop() {
            for e in std::fs::read_dir(&folder)? {
                let e = e?;
                let (name, kind) = (e.file_name().to_string_lossy().to_string(), e.file_type()?);
                if kind.is_dir() && depth < 6 && name != "node_modules" && !name.starts_with('.') { folders.push((e.path(), depth + 1)); }
                else if kind.is_file() && name == "package.json" {
                    let text = std::fs::read(e.path())?;
                    if let Some(new) = bun_for_npm(&String::from_utf8_lossy(&text), &root) { std::fs::write(e.path(), new)?; originals.push((e.path(), text)); }
                }
            }
        }
        Ok(originals)
    }
    fn own(path: &Path) {
        let _ = std::os::unix::fs::lchown(path, Some(1000), Some(1000));
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) && let Ok(entries) = std::fs::read_dir(path) {
            for e in entries.flatten() { own(&e.path()); }
        }
    }
    pub fn drive_setup() -> R<()> {
        let c = config("drive-setup.json")?;
        exact(&c, &["version", "dirs", "timeout_seconds"], "drive setup")?;
        ensure!(c["version"] == 1, "Unsupported drive setup");
        let dirs = strings(&c, "dirs", 64)?;
        for d in dirs.iter().filter(|d| *d != ".") { file_path(d)?; }
        let budget = Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?);
        let started = std::time::Instant::now();
        copy_readable(Path::new("/input"), Path::new("/workspace"))?;
        let scratch = Path::new("/workspace/.kern-install");
        std::fs::create_dir_all(scratch.join("tmp"))?;
        let mut records = Vec::new();
        for d in &dirs {
            let dir = if d == "." { PathBuf::from("/workspace") } else { Path::new("/workspace").join(d) };
            if !dir.join("package.json").is_file() { continue; }
            if d != "." && dir.join("node_modules").exists() { records.push(json!({ "dir": d, "manager": null, "result": "installed with the workspace" })); continue; }
            let (manager, attempts) = install_plan(&dir);
            let berry = std::fs::read_to_string(dir.join(".yarnrc.yml")).ok().map(|t| yarnrc_without_code(&t));
            if let Some(rc) = &berry { std::fs::write(dir.join(".kern-yarnrc.yml"), rc)?; }
            let originals = if manager == "bun" { bun_rewrite(&dir)? } else { Vec::new() };
            let mut result = json!("failed");
            let log = Path::new("/home/kern/install.log");
            for (i, argv) in attempts.iter().enumerate() {
                let left = budget.saturating_sub(started.elapsed());
                if left.is_zero() { result = json!("out of time"); break; }
                let log_file = std::fs::File::create(log)?;
                let child = Command::new(&argv[0]).args(&argv[1..]).current_dir(&dir).env_clear()
                    .envs([("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/home/kern"), ("LANG", "C.UTF-8"), ("CI", "true"), ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
                        ("COREPACK_HOME", "/workspace/.kern-install/corepack"), ("YARN_NODE_LINKER", "node-modules"), ("YARN_ENABLE_GLOBAL_CACHE", "false"),
                        ("npm_config_loglevel", "error"), ("YARN_IGNORE_PATH", "1"), ("TMPDIR", "/workspace/.kern-install/tmp"),
                        ("npm_config_cache", "/workspace/.kern-install/npm"), ("npm_config_store_dir", "/workspace/.kern-install/pnpm-store"),
                        ("YARN_CACHE_FOLDER", "/workspace/.kern-install/yarn"), ("XDG_CACHE_HOME", "/workspace/.kern-install/cache"),
                        ("XDG_DATA_HOME", "/workspace/.kern-install/data"), ("XDG_STATE_HOME", "/workspace/.kern-install/state")])
                    .envs(berry.as_ref().map(|_| ("YARN_RC_FILENAME", ".kern-yarnrc.yml")))
                    .stdin(Stdio::null()).stdout(Stdio::from(log_file.try_clone()?)).stderr(Stdio::from(log_file)).spawn()?;
                if wait(child, left)? {
                    result = json!(if manager == "bun" { "installed from package.json (Kern does not read Bun's lockfile)" }
                        else if i == 0 { "installed" } else { "installed from package.json (the lockfile is out of date)" });
                    break;
                }
                let text = std::fs::read(log).unwrap_or_default();
                result = json!(format!("failed: {}", String::from_utf8_lossy(&text[text.len().saturating_sub(600)..]).trim()));
            }
            for (path, text) in &originals { std::fs::write(path, text)?; }
            records.push(json!({ "dir": d, "manager": manager, "result": result }));
        }
        let _ = std::fs::remove_dir_all(scratch);
        for dir in CACHE_DIRS { std::fs::create_dir_all(Path::new("/workspace/node_modules").join(dir))?; }
        own(Path::new("/workspace"));
        println!("{}", json!({ "version": 1, "installs": records }));
        Ok(())
    }
    fn part(name: &str, bytes: &[u8]) -> R<()> {
        let mut out = std::io::stdout().lock();
        std::io::Write::write_all(&mut out, format!("{DRIVE_MARK}{name} {}\n", bytes.len()).as_bytes())?;
        std::io::Write::write_all(&mut out, bytes)?;
        Ok(())
    }
    fn some_of(path: &str, max: usize, end: bool) -> Vec<u8> {
        let b = std::fs::read(path).unwrap_or_default();
        if b.len() <= max { b } else if end { b[b.len() - max..].to_vec() } else { b[..max].to_vec() }
    }
    fn files_only(dir: &Path) -> R<()> {
        for e in std::fs::read_dir(dir)? {
            let path = e?.path();
            let Ok(mut v) = serde_json::from_slice::<Value>(&std::fs::read(&path)?) else { continue };
            if let Some(result) = v["result"].as_array_mut() { result.retain(|s| s["url"].as_str().is_some_and(|u| u.starts_with("file://"))); }
            if let Some(maps) = v["source-map-cache"].as_object_mut() { maps.retain(|k, _| k.starts_with("file://")); }
            std::fs::write(&path, serde_json::to_vec(&v)?)?;
        }
        Ok(())
    }
    pub fn drive() -> R<()> {
        let c = config("drive-run.json")?;
        exact(&c, &["version", "timeout_seconds"], "drive run")?;
        ensure!(c["version"] == 1, "Unsupported drive run");
        let timeout = Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?);
        std::fs::create_dir_all("/tmp/.kern-v8")?;
        let child = Command::new("/usr/local/bin/node").args(["--import", "/kern/drive-loader.mjs", "/kern/drive-run.mjs"])
            .current_dir("/workspace").env_clear().envs(TEST_ENV).env("TZ", "UTC").env("NODE_V8_COVERAGE", "/tmp/.kern-v8")
            .env("NODE_OPTIONS", "--require=/kern/monitor.cjs").env("KERN_MONITOR", "/tmp/.kern-behavior").env("KERN_DRIVE_CALLS", "/tmp/.kern-calls").env("TSX_TSCONFIG_PATH", "/kern/tsconfig.json")
            .stdin(Stdio::null()).stdout(Stdio::from(std::fs::File::create("/tmp/.kern-out")?)).stderr(Stdio::from(std::fs::File::create("/tmp/.kern-err")?)).spawn()?;
        let finished = wait(child, timeout)?;
        quiesce()?;
        let mut errors = some_of("/tmp/.kern-err", 64 << 10, true);
        if !finished { errors.extend_from_slice(b"\nkern: the driver did not finish in time and was stopped\n"); }
        files_only(Path::new("/tmp/.kern-v8"))?;
        let report = Command::new("/usr/local/bin/node")
            .args(["--import", "/kern/drive-c8.mjs", "/kern-tools/node_modules/c8/bin/c8.js", "report", "--reporter=json", "--temp-directory=/tmp/.kern-v8", "--report-dir=/tmp/.kern-coverage",
                "--exclude=**/node_modules/**", "--extension=.js", "--extension=.cjs", "--extension=.mjs", "--extension=.jsx", "--extension=.ts", "--extension=.cts",
                "--extension=.mts", "--extension=.tsx"])
            .current_dir("/workspace").env_clear().envs(TEST_ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(std::fs::File::create("/tmp/.kern-c8-err")?)).spawn()?;
        if !wait(report, Duration::from_secs(120))? {
            errors.extend_from_slice(b"\nkern: line coverage could not be read: ");
            errors.extend(some_of("/tmp/.kern-c8-err", 4096, true));
        }
        quiesce()?;
        part("output", &some_of("/tmp/.kern-out", 4 << 20, false))?;
        part("errors", &errors)?;
        part("coverage", &some_of("/tmp/.kern-coverage/coverage-final.json", 64 << 20, false))?;
        part("calls", &some_of("/tmp/.kern-calls", 8 << 20, false))?;
        part("behavior", &some_of("/tmp/.kern-behavior", 4 << 20, false))?;
        std::io::Write::flush(&mut std::io::stdout())?;
        Ok(())
    }
    pub fn drive_test() -> R<()> {
        let c = config("drive-test.json")?;
        exact(&c, &["version", "command", "timeout_seconds", "cwd"], "test run")?;
        ensure!(c["version"] == 1, "Unsupported test run");
        let mut command = argv(&c["command"])?;
        let cwd = text(&c, "cwd")?;
        let dir = super::test_dir(cwd)?;
        command[0] = super::runner_in(cwd, &command[0], |p| Path::new(p).exists());
        let timeout = Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?);
        let mut child = Command::new(&command[0]).args(&command[1..]).current_dir(&dir).env_clear().envs(TEST_ENV).env("TZ", "UTC")
            .env("NODE_OPTIONS", "--require=/kern/monitor.cjs").env("KERN_MONITOR", "/tmp/.kern-behavior")
            .stdin(Stdio::null()).stdout(Stdio::from(std::fs::File::create("/tmp/.kern-test-out")?)).stderr(Stdio::from(std::fs::File::create("/tmp/.kern-test-err")?)).spawn()?;
        let started = std::time::Instant::now();
        let ending = loop {
            if let Some(s) = child.try_wait()? { break format!("exit {}", s.code().unwrap_or(-1)); }
            if started.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); break "timeout".to_string(); }
            std::thread::sleep(Duration::from_millis(100));
        };
        quiesce()?;
        part("output", &some_of("/tmp/.kern-test-out", 32 << 20, false))?;
        part("errors", &some_of("/tmp/.kern-test-err", 64 << 10, true))?;
        part("behavior", &some_of("/tmp/.kern-behavior", 4 << 20, false))?;
        part("ending", ending.as_bytes())?;
        std::io::Write::flush(&mut std::io::stdout())?;
        Ok(())
    }
    pub fn exec(command: &[String]) -> R<()> {
        let c = argv(&json!(command))?;
        for e in std::fs::read_dir("/input")? { let e = e?; if e.file_name() != "node_modules" { copy_entry(&e.path(), &Path::new("/workspace").join(e.file_name()))?; } }
        let log = Path::new("/tmp/.kern-behavior");
        let status = Command::new(&c[0]).args(&c[1..]).current_dir("/workspace").env_clear().envs(TEST_ENV)
            .env("NODE_OPTIONS", "--require=/kern/monitor.cjs").env("KERN_MONITOR", log).stdin(Stdio::null()).status()?;
        quiesce()?;
        print!("{BEHAVIOR}{}", std::fs::read_to_string(log).unwrap_or_default());
        std::io::Write::flush(&mut std::io::stdout())?;
        ensure!(status.success(), "Check command failed: {status}");
        Ok(())
    }
}
#[cfg(not(unix))]
mod inside {
    use super::*;
    pub fn runner() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn setup() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn exec(_: &[String]) -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn mutate() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn drive() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn drive_setup() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
    pub fn drive_test() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
}
pub use inside::{drive, drive_setup, drive_test, exec, mutate, runner, setup};
