use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const WORKSPACE: &str = "/workspace";
pub const HOME: &str = "/home/kern";
pub const RESERVED: [&str; 6] = [WORKSPACE, "/tmp", HOME, "/proc", "/sys", "/dev"];
const ENGINE_ENV: [&str; 4] = ["PATH", "HOSTNAME", "container", "TERM"];

#[derive(Clone, Debug)]
pub struct Mount { pub source: PathBuf, pub target: String }
/// `feedback`: Kern's verdict on the agent's previous attempt (KERN_FEEDBACK); `delta`: what the task states will change (KERN_DELTA).
pub struct BoxSpec<'a> {
    pub image: &'a str, pub command: &'a [String], pub workspace: &'a Path, pub mounts: &'a [Mount], pub env: &'a [String],
    pub network: bool, pub timeout_seconds: u64, pub memory_mb: u64, pub pids: u64, pub feedback: Option<&'a str>, pub delta: Option<&'a str>,
}

#[derive(Clone, Debug)]
pub struct Engine {
    pub cli: PathBuf, pub name: String, pub podman: bool, pub rootless: bool, pub limits: bool, pub gvisor: bool, pub seccomp: bool, pub selinux: bool, pub desktop_vm: bool,
    pub default_caps: Vec<String>,
}
impl Engine {
    pub fn usable(&self) -> R<()> {
        let hint = if self.podman && self.rootless { " (rootless Podman needs cgroup v2 with the memory and pids controllers delegated)" }
            else if self.podman { "" } else { " (Docker needs memory, swap and process limit support, as on any cgroup v2 host)" };
        ensure!(self.limits, "{} cannot enforce memory and process limits on this machine{hint}; Kern will not run an agent without them", self.name);
        ensure!(self.seccomp || self.gvisor, "{} runs containers without seccomp; Kern will not run an agent without it", self.name);
        Ok(())
    }
    pub fn isolation(&self) -> &'static str { if self.gvisor { "gVisor" } else { "standard container" } }
}
fn listed(v: &Value) -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() }
pub fn engine_from_info(cli: &Path, info: &Value) -> R<Engine> {
    if let Some(host) = info.get("host") {
        let os = host["os"].as_str().unwrap_or("unknown");
        ensure!(os == "linux", "Podman reports a {os} engine; Kern needs Linux containers");
        let (controllers, security) = (listed(&host["cgroupControllers"]), &host["security"]);
        let default_caps = security["capabilities"].as_str().unwrap_or("").split(',').map(|c| c.trim().to_ascii_uppercase()).filter(|c| !c.is_empty()).collect();
        return Ok(Engine { cli: cli.into(), name: format!("Podman {}", info["version"]["Version"].as_str().unwrap_or("unknown")), podman: true, rootless: security["rootless"] == true,
            limits: ["memory", "pids"].iter().all(|c| controllers.iter().any(|x| x == c)), gvisor: false, seccomp: security["seccompEnabled"] == true,
            selinux: security["selinuxEnabled"] == true, desktop_vm: false, default_caps });
    }
    if let Some(error) = listed(&info["ServerErrors"]).first() { deny!("{error}") }
    let os = info["OSType"].as_str().unwrap_or("");
    ensure!(!os.is_empty(), "the Docker engine is not running");
    ensure!(os == "linux", "Docker is running {os} containers; Kern needs Linux containers (in Docker Desktop, choose Switch to Linux containers)");
    let (options, system) = (listed(&info["SecurityOptions"]), info["OperatingSystem"].as_str().unwrap_or("unknown"));
    Ok(Engine { cli: cli.into(), name: format!("Docker {} ({system})", info["ServerVersion"].as_str().unwrap_or("unknown")), podman: false,
        rootless: options.iter().any(|o| o.contains("name=rootless")), limits: ["MemoryLimit", "SwapLimit", "PidsLimit"].iter().all(|k| info[*k] == true),
        gvisor: info["Runtimes"].get("runsc").is_some(), seccomp: options.iter().any(|o| o.starts_with("name=seccomp") && !o.contains("profile=unconfined")),
        selinux: options.iter().any(|o| o.starts_with("name=selinux")),
        desktop_vm: system.starts_with("Docker Desktop") && info["KernelVersion"].as_str().is_some_and(|k| k.contains("linuxkit")), default_caps: Vec::new() })
}

fn locate(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.is_absolute() { return p.is_file().then(|| p.to_path_buf()); }
    if p.components().count() != 1 { return None; }
    let file = if cfg!(windows) && p.extension().is_none() { format!("{name}.exe") } else { name.to_string() };
    std::env::split_paths(&std::env::var_os("PATH")?).filter(|d| d.is_absolute()).map(|d| d.join(&file)).find(|c| c.is_file())
}
fn ambient(timeout: Duration) -> Run<'static> { Run { cwd: None, env: std::env::vars().collect(), input: None, timeout, max_output: 16 * 1048576 } }
fn brief(s: &str) -> String { s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(400).collect() }
pub fn engine() -> R<Engine> {
    let names = match std::env::var("KERN_CONTAINER_CLI") { Ok(c) if !c.is_empty() => vec![c], _ => vec!["docker".to_string(), "podman".into()] };
    let mut reasons = Vec::new();
    for name in &names {
        let found = locate(name).ok_or_else(|| Denied("not installed".into())).and_then(|cli| {
            let info = crate::util::run(&cli, &["info".into(), "--format".into(), "{{json .}}".into()], ambient(Duration::from_secs(60)))?;
            let e = engine_from_info(&cli, &serde_json::from_slice(&info)?)?;
            e.usable()?;
            Ok(e)
        });
        match found { Ok(e) => return Ok(e), Err(err) => reasons.push(format!("{name}: {}", brief(&err.0))) }
    }
    deny!("Kern runs agents in a Linux container, but no container runtime is ready ({}). Start or install Docker Desktop, Podman or Rancher Desktop (on Windows they run on WSL2), \
           or Docker Engine or Podman on Linux; on Windows you can also run the Linux kern inside WSL2. Or run this task in CI or on a Kern verifier", reasons.join("; "))
}
pub fn doctor() -> (bool, Value) {
    match engine() {
        Ok(e) => (true, json!({ "ready": true, "local_box": { "cli": e.cli, "engine": e.name, "isolation": e.isolation(), "rootless": e.rootless, "selinux": e.selinux },
            "explanation": format!("kern box and kern run will run agents in Linux containers on {} with {} isolation.", e.name, e.isolation()) })),
        Err(err) => (false, json!({ "ready": false, "explanation": err.0 })),
    }
}

pub fn identity(e: &Engine) -> (String, bool) {
    #[cfg(unix)]
    {
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        if (e.rootless && !e.podman) || (e.desktop_vm && cfg!(target_os = "linux")) { return ("0:0".into(), false); }
        (format!("{uid}:{gid}"), e.rootless && e.podman)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        ("1000:1000".into(), false)
    }
}
fn bind(source: &Path, target: &str, options: &str) -> R<String> {
    let s = source.display().to_string();
    ensure!(!s.contains([',', '"', '\n', '\r']), "Kern cannot mount {s}: the path contains a comma, quote or line break");
    Ok(format!("--mount=type=bind,source={s},target={target}{options}"))
}
fn workspace(path: &Path, e: &Engine) -> R<String> {
    if e.selinux && !e.podman {
        let s = path.display().to_string();
        ensure!(!s.contains([':', ',', '"', '\n', '\r']), "Kern cannot mount {s}: the path contains a colon, comma, quote or line break");
        return Ok(format!("--volume={s}:{WORKSPACE}:Z"));
    }
    bind(path, WORKSPACE, if e.selinux { ",relabel=private" } else { "" })
}
pub fn create_args(spec: &BoxSpec, e: &Engine, name: &str) -> R<Vec<String>> {
    ensure!(spec.command.first().is_some_and(|c| c.starts_with('/')), "The agent command must start with an absolute container path");
    let (user, keep_id) = identity(e);
    let (uid, gid) = user.split_once(':').unwrap_or(("0", "0"));
    let memory = format!("{}m", spec.memory_mb);
    let owner = if e.podman { "U".to_string() } else { format!("uid={uid},gid={gid}") };
    let mut a = vec![
        "create".to_string(), format!("--name={name}"), format!("--label=kern.box={name}"), "--pull=never".into(), "--read-only".into(), "--cap-drop=ALL".into(),
        "--security-opt=no-new-privileges".into(), format!("--pids-limit={}", spec.pids), format!("--memory={memory}"), format!("--memory-swap={memory}"), format!("--user={user}"),
        "--ipc=private".into(), "--log-driver=none".into(), "--ulimit=core=0".into(), format!("--tmpfs=/tmp:rw,exec,nosuid,nodev,size={memory},mode=1777"),
        format!("--tmpfs={HOME}:rw,exec,nosuid,nodev,size={memory},mode=0700,{owner}"), format!("--workdir={WORKSPACE}"), format!("--env=HOME={HOME}"), format!("--entrypoint={}", spec.command[0]),
    ];
    if e.podman { a.push("--http-proxy=false".into()); }
    if !spec.network { a.push("--network=none".into()); }
    if keep_id { a.push("--userns=keep-id".into()); }
    if e.gvisor { a.push("--runtime=runsc".into()); }
    a.push(workspace(spec.workspace, e)?);
    for m in spec.mounts { a.push(bind(&m.source, &m.target, ",readonly")?); }
    a.extend(spec.env.iter().map(|n| format!("--env={n}")));
    if let Some(f) = spec.feedback { a.push(format!("--env=KERN_FEEDBACK={f}")); }
    if let Some(d) = spec.delta { a.push(format!("--env=KERN_DELTA={d}")); }
    a.push(spec.image.into());
    a.extend(spec.command[1..].iter().cloned());
    Ok(a)
}
pub fn confirm(i: &Value, spec: &BoxSpec, e: &Engine, image_env: &[String]) -> R<()> {
    let (h, n) = (&i["HostConfig"], &e.name);
    ensure!(h["ReadonlyRootfs"] == true && h["Privileged"] == false, "{n} did not make the container read-only and unprivileged; Kern will not run the agent in a weaker box");
    let mode = h["NetworkMode"].as_str().unwrap_or("");
    let network = if spec.network { !mode.is_empty() && mode != "host" && !mode.starts_with("container:") && !mode.starts_with("ns:") } else { mode == "none" };
    ensure!(network, "{n} did not apply the box's network setting ({mode})");
    let dropped: Vec<String> = listed(&h["CapDrop"]).iter().map(|c| c.to_ascii_uppercase()).collect();
    let all = dropped.iter().any(|c| c == "ALL") || (e.podman && !e.default_caps.is_empty() && e.default_caps.iter().all(|c| dropped.contains(c)));
    ensure!(all && h["CapAdd"].as_array().is_none_or(Vec::is_empty), "{n} did not drop every capability");
    let options = listed(&h["SecurityOpt"]);
    ensure!(options.iter().any(|o| o.starts_with("no-new-privileges")), "{n} did not apply no-new-privileges");
    ensure!(!options.iter().any(|o| o.contains("unconfined") || o.starts_with("label=disable") || o.starts_with("label:disable")), "{n} ran the container unconfined ({})", options.join(", "));
    ensure!(matches!(h["PidMode"].as_str(), Some("" | "private")) && h["IpcMode"] == "private", "{n} shared a process or IPC namespace with the host");
    ensure!(h["Devices"].as_array().is_none_or(Vec::is_empty), "{n} gave the container host devices");
    let memory = spec.memory_mb * 1048576;
    ensure!(h["Memory"] == memory && h["MemorySwap"] == memory && h["PidsLimit"] == spec.pids, "{n} did not apply the memory and process limits ({} MB without extra swap, {} processes)", spec.memory_mb, spec.pids);
    ensure!(i["Config"]["User"] == identity(e).0.as_str(), "{n} did not run the agent as the expected user");
    ensure!(!e.gvisor || h["Runtime"] == "runsc", "{n} did not use gVisor (runsc)");
    let mounts = i["Mounts"].as_array().cloned().unwrap_or_default();
    let mut expected = std::iter::once((WORKSPACE, true)).chain(spec.mounts.iter().map(|m| (m.target.as_str(), false)));
    ensure!(mounts.len() == 1 + spec.mounts.len() && expected.all(|(t, rw)| mounts.iter().any(|m| m["Type"] == "bind" && m["Destination"] == t && m["RW"] == rw)),
        "{n} mounted something other than the workspace and the job's read-only mounts");
    let name = |entry: &str| entry.split('=').next().unwrap_or("").to_string();
    let known = |var: &str| var == "HOME" || (var == "KERN_FEEDBACK" && spec.feedback.is_some()) || (var == "KERN_DELTA" && spec.delta.is_some()) || ENGINE_ENV.contains(&var)
        || spec.env.iter().any(|x| x == var) || image_env.iter().any(|x| name(x) == var);
    let mut added: Vec<String> = listed(&i["Config"]["Env"]).iter().map(|x| name(x)).filter(|x| !known(x)).collect();
    added.dedup();
    ensure!(added.is_empty(), "{n} added environment variables the job did not list ({}). Docker copies proxy settings from ~/.docker/config.json and Podman from containers.conf; \
        list them in env if the agent should have them", added.join(", "));
    Ok(())
}
pub fn finished(s: &Value) -> R<(i64, bool)> {
    if let Some(error) = s["Error"].as_str().filter(|x| !x.is_empty()) {
        deny!("The container runtime could not start the agent ({error}). The agent did not run; try again, and if it keeps failing check command[0] exists in the image or raise limits.memory_mb and limits.pids")
    }
    ensure!(s["Running"] == false && s["Status"] != "running", "Kern lost contact with the agent while its container was still running; the container was removed and nothing was accepted");
    let started = s["StartedAt"].as_str().unwrap_or("");
    ensure!(s["Status"] == "exited" && !started.is_empty() && !started.starts_with("0001-01-01"),
        "The agent's container never started (state {}); nothing was accepted", s["Status"].as_str().unwrap_or("unknown"));
    Ok((s["ExitCode"].as_i64().unwrap_or(-1), s["OOMKilled"] == true))
}
pub fn agent_line(raw: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(raw).chars().filter(|c| *c == '\t' || !(c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))).collect();
    format!("[agent] {}", text.trim_end())
}

fn cli(e: &Engine, args: &[&str], timeout: Duration) -> R<Vec<u8>> {
    crate::util::run(&e.cli, &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(), ambient(timeout))
}
fn notice(text: &str) { let _ = writeln!(std::io::stderr(), "kern: {text}"); }
fn present(e: &Engine, image: &str) -> R<Vec<String>> {
    let env = || -> R<Vec<String>> { Ok(listed(&serde_json::from_slice(&cli(e, &["image", "inspect", "--format", "{{json .Config.Env}}", image], Duration::from_secs(60))?)?)) };
    if let Ok(found) = env() { return Ok(found); }
    ensure!(image.contains('@'), "Image {image} is not available locally");
    notice(&format!("pulling {image}"));
    let mut child = Command::new(&e.cli).args(["pull", image]).stdin(Stdio::null()).stdout(Stdio::from(std::io::stderr())).stderr(Stdio::from(std::io::stderr())).spawn()
        .map_err(|err| Denied(format!("{} could not start: {err}", e.cli.display())))?;
    let deadline = Instant::now() + Duration::from_secs(1800);
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "Could not pull {image}; check the reference and your access to its registry");
            return env();
        }
        if Instant::now() > deadline { let _ = child.kill(); let _ = child.wait(); deny!("Pulling {image} took longer than 30 minutes") }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn inspect(e: &Engine, id: &str) -> R<Value> {
    let v: Value = serde_json::from_slice(&cli(e, &["inspect", "--type", "container", id], Duration::from_secs(60))?)?;
    ensure!(v[0].is_object(), "{} returned no description of the agent's container", e.name);
    Ok(v[0].clone())
}
fn discard<'a>(e: &Engine, id: &'a str) -> Vec<&'a str> { if e.podman { vec!["rm", "--force", "--time=0", id] } else { vec!["rm", "--force", id] } }
fn sweep(e: &Engine) {
    let Ok(names) = cli(e, &["ps", "--all", "--filter", "label=kern.box", "--format", "{{.Names}}"], Duration::from_secs(60)) else { return };
    let now = now_ms();
    for name in String::from_utf8_lossy(&names).split_whitespace() {
        let deadline = name.strip_prefix("kern-box-").and_then(|r| r.split('-').next()).and_then(|d| d.parse::<u64>().ok());
        if deadline.is_some_and(|d| d.saturating_add(60_000) < now) { let _ = cli(e, &discard(e, name), Duration::from_secs(60)); }
    }
}
fn remove(e: &Engine, id: &str, name: &str) -> R<()> {
    for _ in 0..5 {
        let _ = cli(e, &discard(e, id), Duration::from_secs(120));
        let left = cli(e, &["ps", "--all", "--quiet", "--filter", &format!("label=kern.box={name}")], Duration::from_secs(60))?;
        if left.iter().all(u8::is_ascii_whitespace) { return Ok(()); }
        std::thread::sleep(Duration::from_secs(1));
    }
    deny!("Could not remove the agent's container {name}; nothing was accepted")
}

fn relay(mut source: impl Read + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let emit = |line: &[u8]| { let _ = writeln!(std::io::stderr().lock(), "{}", agent_line(line)); };
        let (mut chunk, mut line) = ([0u8; 8192], Vec::new());
        while let Ok(n) = source.read(&mut chunk) {
            if n == 0 { break; }
            for &b in &chunk[..n] {
                if b == b'\n' || line.len() >= 4096 {
                    emit(&line);
                    line.clear();
                    if b == b'\n' { continue; }
                }
                line.push(b);
            }
        }
        if !line.is_empty() { emit(&line); }
    })
}
fn attach(e: &Engine, id: &str, seconds: u64) -> R<()> {
    let _interrupts = interrupt::catch();
    let mut child = Command::new(&e.cli).args(["start", "--attach", id]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|err| Denied(format!("{} could not start: {err}", e.cli.display())))?;
    let relays = [relay(child.stdout.take().expect("piped stdout")), relay(child.stderr.take().expect("piped stderr"))];
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let result = loop {
        let exited = child.try_wait()?.is_some();
        let stop = if interrupt::requested() { Some("Interrupted: the agent was stopped and nothing was accepted".to_string()) }
            else if exited { break Ok(()) }
            else if Instant::now() >= deadline { Some(format!("The agent did not finish within {seconds} seconds (limits.timeout_seconds); its container was stopped and nothing was accepted")) }
            else { None };
        if let Some(reason) = stop {
            let _ = cli(e, &["kill", id], Duration::from_secs(60));
            let grace = Instant::now() + Duration::from_secs(30);
            while child.try_wait()?.is_none() && Instant::now() < grace { std::thread::sleep(Duration::from_millis(50)); }
            let _ = child.kill();
            let _ = child.wait();
            break Err(Denied(reason));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let settled = Instant::now() + Duration::from_secs(5);
    while !relays.iter().all(std::thread::JoinHandle::is_finished) && Instant::now() < settled { std::thread::sleep(Duration::from_millis(20)); }
    result
}

struct Reaper(Option<std::process::ChildStdin>, std::process::Child);
impl Reaper {
    fn start(e: &Engine, id: &str) -> R<Reaper> {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("__reap").arg(&e.cli).arg(id).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0000_0008 | 0x0000_0200);
        let mut child = cmd.spawn().map_err(|err| Denied(format!("Kern could not start the watchdog that removes the agent's container if Kern stops: {err}")))?;
        Ok(Reaper(child.stdin.take(), child))
    }
    fn release(mut self) {
        if let Some(mut done) = self.0.take() { let _ = done.write_all(b"1"); }
        let _ = self.1.wait();
    }
}
pub fn reap(cli: &Path, id: &str) -> R<()> {
    let mut signal = [0u8; 1];
    if !matches!(std::io::stdin().read(&mut signal), Ok(1)) {
        let _ = crate::util::run(cli, &["kill".into(), id.into()], ambient(Duration::from_secs(60)));
        crate::util::run(cli, &["rm".into(), "--force".into(), id.into()], ambient(Duration::from_secs(120)))?;
    }
    Ok(())
}

pub struct Outcome { pub exit_code: i64, pub oom_killed: bool, pub container: String, pub engine: String, pub isolation: &'static str }
pub fn run(spec: &BoxSpec) -> R<Outcome> {
    for n in spec.env { ensure!(std::env::var_os(n).is_some(), "The job passes {n} to the agent, but {n} is not set"); }
    let e = engine()?;
    sweep(&e);
    let image_env = present(&e, spec.image)?;
    let name = format!("kern-box-{}-{}", now_ms() + spec.timeout_seconds * 1000, short_id()?);
    let args = create_args(spec, &e, &name)?;
    let created = cli(&e, &args.iter().map(String::as_str).collect::<Vec<_>>(), Duration::from_secs(120))?;
    let id = String::from_utf8_lossy(&created).trim().to_string();
    if !(id.len() >= 12 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
        let _ = remove(&e, &name, &name);
        deny!("{} did not report the container it created", e.name)
    }
    let reaper = Reaper::start(&e, &id);
    let outcome = match &reaper {
        Err(err) => Err(Denied(err.0.clone())),
        Ok(_) => (|| -> R<Outcome> {
            confirm(&inspect(&e, &id)?, spec, &e, &image_env)?;
            notice(&format!("running the agent in a Linux container on {} ({} isolation, network {}, {} MB, {} processes, {} s)",
                e.name, e.isolation(), if spec.network { "on" } else { "off" }, spec.memory_mb, spec.pids, spec.timeout_seconds));
            attach(&e, &id, spec.timeout_seconds)?;
            let (exit_code, oom_killed) = finished(&inspect(&e, &id)?["State"])?;
            Ok(Outcome { exit_code, oom_killed, container: name.clone(), engine: e.name.clone(), isolation: e.isolation() })
        })(),
    };
    remove(&e, &id, &name)?;
    if let Ok(r) = reaper { r.release(); }
    outcome
}

mod interrupt {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    static ACTIVE: Mutex<(usize, Option<Saved>)> = Mutex::new((0, None));
    pub fn requested() -> bool { REQUESTED.load(Ordering::SeqCst) }
    pub struct Guard;
    pub fn catch() -> Guard {
        let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
        if active.0 == 0 { REQUESTED.store(false, Ordering::SeqCst); active.1 = Some(install()); }
        active.0 += 1;
        Guard
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            active.0 -= 1;
            if active.0 == 0 && let Some(saved) = active.1.take() { restore(saved); }
        }
    }
    #[cfg(unix)]
    const SIGNALS: [libc::c_int; 4] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT];
    #[cfg(unix)]
    type Saved = [libc::sigaction; 4];
    #[cfg(unix)]
    extern "C" fn on_signal(_: libc::c_int) { REQUESTED.store(true, Ordering::SeqCst); }
    #[cfg(unix)]
    fn install() -> Saved {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            let mut saved: Saved = std::mem::zeroed();
            for (signal, old) in SIGNALS.iter().zip(saved.iter_mut()) { libc::sigaction(*signal, &action, old); }
            saved
        }
    }
    #[cfg(unix)]
    fn restore(saved: Saved) {
        unsafe { for (signal, old) in SIGNALS.iter().zip(saved.iter()) { libc::sigaction(*signal, old, std::ptr::null_mut()); } }
    }
    #[cfg(windows)]
    struct Saved;
    #[cfg(windows)]
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
    }
    #[cfg(windows)]
    unsafe extern "system" fn on_ctrl(kind: u32) -> i32 { i32::from(kind <= 1 && !REQUESTED.swap(true, Ordering::SeqCst)) }
    #[cfg(windows)]
    fn install() -> Saved {
        unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1); }
        Saved
    }
    #[cfg(windows)]
    fn restore(_: Saved) { unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 0); } }
}
