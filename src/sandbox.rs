use crate::policy::{Control, Limits, control, image, implementation_digest, pinned};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MONITOR: &str = include_str!("monitor.cjs");
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
        Some(i) => (&stdout[..i], Some(String::from_utf8_lossy(&stdout[i + mark.len()..]).lines().filter(|l| !l.is_empty()).map(str::to_string).collect())),
        None => (stdout, None),
    }
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

pub struct Volume(pub String);
impl Volume {
    pub fn create() -> R<Volume> {
        let name = format!("kern-deps-{}", uuid4()?);
        docker(&["volume", "create", "--label", "kern=dependencies", &name], Duration::from_secs(60), 4096)?;
        Ok(Volume(name))
    }
}
impl Drop for Volume {
    fn drop(&mut self) { let _ = docker(&["volume", "rm", "--force", &self.0], Duration::from_secs(120), 4096); }
}

pub struct Spec<'a> {
    pub image: &'a str, pub snapshot: &'a Path, pub control: &'a Path, pub writable: &'a [String], pub limits: &'a Limits, pub entry: Vec<String>,
    pub network: &'a str, pub volumes: Vec<(&'a str, &'a str, bool)>, pub root: bool, pub home_mb: u64,
}
fn mount(source: &Path, target: &str) -> R<[String; 2]> {
    let s = std::path::absolute(source)?.display().to_string();
    ensure!(!s.contains([',', '\n', '"']), "Unsupported mount source");
    Ok(["--mount".into(), format!("type=bind,source={s},target={target},readonly")])
}
pub fn container_args(spec: &Spec, name: &str) -> R<Vec<String>> {
    ensure!(spec.network == "none" || (spec.network == "bridge" && spec.root && spec.writable.is_empty()), "Only dependency setup may use a network");
    ensure!(spec.volumes.iter().all(|(v, t, _)| v.starts_with("kern-deps-") && *t == "/workspace/node_modules"), "Unexpected volume");
    ensure!(spec.entry.first().is_some_and(|e| e.starts_with("__")), "Sandbox entry must be a Kern entry point");
    let l = spec.limits;
    let user = if spec.root { "0:0" } else { "1000:1000" };
    let mut a: Vec<String> = [
        "create", "--name", name, "--pull=never", "--runtime=runsc", &format!("--network={}", spec.network), "--read-only", &format!("--user={user}"), "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true", "--pids-limit", &l.pids.to_string(), "--memory", &format!("{}m", l.memory_mb), "--memory-swap", &format!("{}m", l.memory_mb), "--cpus=1",
        "--ulimit", "nofile=1024:1024", "--ulimit", "core=0", "--ipc=private", "--shm-size=8m", "--log-driver=none", "--no-healthcheck", "--workdir=/workspace", "--entrypoint=/kern/kern",
        "--tmpfs", "/tmp:rw,noexec,nosuid,nodev,size=67108864,mode=1777",
    ].iter().map(|s| s.to_string()).collect();
    a.extend(mount(spec.snapshot, "/workspace")?);
    a.extend(mount(spec.snapshot, "/input")?);
    a.extend(mount(spec.control, "/kern")?);
    for grant in spec.writable {
        a.extend(["--tmpfs".into(), format!("/workspace/{}:rw,nosuid,nodev,size={},uid=1000,gid=1000,mode=0700", grant.trim_end_matches('/'), l.total_bytes)]);
    }
    if spec.home_mb > 0 { a.extend(["--tmpfs".into(), format!("/home/kern:rw,nosuid,nodev,size={}m,mode=0700", spec.home_mb)]); }
    for (volume, target, readonly) in &spec.volumes {
        a.extend(["--mount".into(), format!("type=volume,source={volume},target={target}{}", if *readonly { ",readonly" } else { "" })]);
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
        ensure!(h["Memory"] == memory && h["MemorySwap"] == memory && h["PidsLimit"] == spec.limits.pids && h["NanoCpus"] == 1_000_000_000u64, "Container resource limits not applied");
        ensure!(h["Privileged"] == false && h["CapDrop"].as_array().is_some_and(|c| c.iter().any(|x| x == "ALL")) && h["SecurityOpt"].as_array().is_some_and(|s| s.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("no-new-privileges")))), "Container privilege constraints not applied");
        let mounts = i["Mounts"].as_array().cloned().unwrap_or_default();
        let binds: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "bind").collect();
        let volumes: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "volume").collect();
        ensure!(binds.len() == 3 && binds.iter().all(|m| m["RW"] == false), "Unexpected or writable host mount");
        ensure!(volumes.len() == spec.volumes.len() && spec.volumes.iter().all(|(v, t, ro)| volumes.iter().any(|m| m["Name"] == *v && m["Destination"] == *t && m["RW"] == !ro)), "Unexpected volume mount");
        ensure!(mounts.iter().all(|m| m["Type"] == "bind" || m["Type"] == "volume" || m["Type"] == "tmpfs"), "Unexpected mount type");
        let (_, out, _) = docker_full(&["start", "--attach", &name], Duration::from_secs(spec.limits.timeout_seconds + 5), (spec.limits.total_bytes as usize) * 2 + 1048576)?;
        let f = &docker_json(&["inspect", &name])?[0]["State"];
        ensure!(f["Running"] == false && f["OOMKilled"] == false && f["ExitCode"].is_i64(), "Sandbox failed or exceeded limits");
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
    if let Some((name, value)) = config { create_new(dir.join(name), format!("{}\n", canonical(value)?).as_bytes(), 0o444)?; }
    Ok(())
}

#[cfg(unix)]
mod inside {
    use super::*;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::process::{Command, Stdio};

    const ENV: [(&str, &str); 3] = [("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/tmp"), ("LANG", "C.UTF-8")];
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
        println!("{}", json!({ "version": 1, "entries": totals.0, "bytes": totals.1, "sha256": digest(&json!(entries))? }));
        Ok(())
    }
    pub fn exec(command: &[String]) -> R<()> {
        let c = argv(&json!(command))?;
        let log = Path::new("/tmp/.kern-behavior");
        let status = Command::new(&c[0]).args(&c[1..]).current_dir("/workspace").env_clear().envs(ENV)
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
}
pub use inside::{exec, runner, setup};
