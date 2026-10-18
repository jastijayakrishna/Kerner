use crate::policy::{Control, Limits, control, image, implementation_digest, pinned};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MONITOR: &str = include_str!("monitor.cjs");
pub const JEST_REPORT: &str = include_str!("jest-report.cjs");
pub const JEST_PROGRESS: &str = include_str!("jest-progress.cjs");
pub const VITEST_PROGRESS: &str = include_str!("vitest-progress.mjs");
pub const FAULT: &str = include_str!("fault.cjs");
pub const NODE_REPORT: &str = include_str!("node-report.mjs");
pub const TRACE: &str = include_str!("trace.cjs");
pub const NODE_REPORTER: &str = "--test-reporter=/kern/node-report.mjs";
pub const INSTALL_ENV: [(&str, &str); 22] = [("COREPACK_ENABLE_AUTO_PIN", "0"), ("COREPACK_DEFAULT_TO_LATEST", "0"),("HUSKY", "0"), ("HUSKY_SKIP_INSTALL", "1"), ("PUPPETEER_CACHE_DIR", "/workspace/node_modules/.cache/puppeteer"), ("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/home/kern"), ("LANG", "C.UTF-8"), ("CI", "true"),
    ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"), ("COREPACK_HOME", "/home/kern/corepack"), ("YARN_NODE_LINKER", "node-modules"), ("YARN_ENABLE_GLOBAL_CACHE", "false"),
    ("npm_config_loglevel", "error"), ("YARN_IGNORE_PATH", "1"), ("TMPDIR", "/workspace/.kern-install/tmp"), ("npm_config_cache", "/workspace/.kern-install/npm"),
    ("npm_config_store_dir", "/workspace/.kern-install/pnpm-store"), ("YARN_CACHE_FOLDER", "/workspace/.kern-install/yarn"), ("XDG_CACHE_HOME", "/workspace/.kern-install/cache"),
    ("XDG_DATA_HOME", "/workspace/.kern-install/data"), ("XDG_STATE_HOME", "/workspace/.kern-install/state")];
pub const TEST_ENV: [(&str, &str); 10] = [("PATH", "/workspace/.kern-tools/bin:/usr/local/bin:/usr/bin:/bin"), ("HOME", "/tmp"), ("LANG", "C.UTF-8"), ("CI", "true"),
    ("PLAYWRIGHT_BROWSERS_PATH", "/ms-playwright"), ("PUPPETEER_CACHE_DIR", "/workspace/node_modules/.cache/puppeteer"), ("COREPACK_HOME", "/workspace/.kern-tools/corepack-home"),
    ("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"), ("COREPACK_ENABLE_AUTO_PIN", "0"), ("COREPACK_DEFAULT_TO_LATEST", "0")];
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
pub fn missing_platform_parts(node_modules: &Path) -> Vec<(String, String)> {
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let mut packages = Vec::new();
    for e in std::fs::read_dir(node_modules).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') { continue; }
        if name.starts_with('@') { packages.extend(std::fs::read_dir(e.path()).into_iter().flatten().flatten().map(|s| s.path())); } else { packages.push(e.path()); }
    }
    let mut out = std::collections::BTreeMap::new();
    for p in packages {
        let Some(v) = std::fs::read(p.join("package.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        for (dep, spec) in v["optionalDependencies"].as_object().into_iter().flatten() {
            let d = dep.to_ascii_lowercase();
            let fits = d.contains("linux") && (d.contains(arch) || (arch == "x64" && d.contains("linux-64"))) && !d.contains("musl");
            if fits && file_path(dep).is_ok() && !node_modules.join(dep).exists() { out.insert(dep.clone(), spec.as_str().unwrap_or("*").to_string()); }
        }
    }
    out.into_iter().take(20).collect()
}
pub const DRIVE_MARK: &str = "\u{1e}kern-drive ";
pub const DRIVE_PARTS: [&str; 5] = ["output", "errors", "coverage", "calls", "behavior"];
pub const TEST_PARTS: [&str; 7] = ["output", "errors", "behavior", "created", "ending", "values", "trace"];
pub const TRACE_BYTES_MAX: usize = 8 << 20;
pub const INSTALL_SCRATCH: [&str; 1] = ["/workspace/.kern-install"];
pub const PNPM_STORE: &str = "/workspace/.kern-pnpm-store";
const STORE_VERSIONS_COUNT_MAX: usize = 16;
pub fn store_links_back(store: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(store).into_iter().flatten().flatten().take(STORE_VERSIONS_COUNT_MAX).map(|e| e.path().join("projects")).filter(|p| p.symlink_metadata().is_ok()).collect()
}
pub const SERVE_FILES_COUNT_MAX: usize = 40;
pub const VALUES_BYTES_MAX: usize = 1 << 20;
pub const FAULT_VALUES_BYTES_MAX: usize = 64 << 10;
pub const FAULTS_COUNT_MAX: usize = 24;
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
pub fn runtime_name() -> &'static str { crate::policy::control().map_or("runsc", |c| c.runtime_name.as_str()) }
pub fn throwaway() -> bool { runtime_name() == "runc" }
pub fn doctor() -> Doctor {
    let required = runtime_name();
    let attempt = || -> R<Doctor> {
        let info = docker_json(&["info", "--format", "{{json .}}"])?;
        let limits = ["MemoryLimit", "SwapLimit", "PidsLimit", "CpuCfsQuota"].iter().all(|k| info[*k] == true);
        let runtimes: Vec<String> = info["Runtimes"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
        let ready = info["OSType"] == "linux" && runtimes.iter().any(|r| r == required) && limits;
        let builder = if ready { Some(builder_identity(&info, &docker_json(&["version", "--format", "{{json .}}"])?)?) } else { None };
        let report = json!({ "ready": ready, "builder": builder.as_ref().map(|b| json!({ "sha256": b.0, "identity": b.1 })), "os": info["OSType"], "runtimes": runtimes, "resource_limits": limits, "required": required,
            "explanation": "Operator must pin genuine gVisor (runsc), or runc on a throwaway machine that holds no keys, with its daemon mapping, toolchain and host policy, and enable memory/swap/PID/CPU limits." });
        Ok(Doctor { ready, builder, report })
    };
    attempt().unwrap_or_else(|e| Doctor { ready: false, builder: None, report: json!({ "ready": false, "required": required, "explanation": e.0 }) })
}
fn builder_identity(info: &Value, versions: &Value) -> R<(String, Value)> {
    ensure!(cfg!(target_os = "linux"), "Production builder identity requires a native Linux controller");
    let c = control()?;
    let runtime = pinned(&c.runtime)?;
    let mapping = &info["Runtimes"][c.runtime_name.as_str()];
    let mapped = mapping["path"].as_str().unwrap_or("");
    let same = mapped == runtime.to_string_lossy() || (!mapped.contains('/') && runtime.file_name().is_some_and(|n| n.to_string_lossy() == mapped));
    ensure!(same && mapping["runtimeArgs"].as_array().is_none_or(Vec::is_empty), "Daemon runtime mapping does not match the pinned runtime");
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
const YARN_SETTINGS: [&str; 92] = ["approvedGitRepositories", "cacheFolder", "cacheMigrationMode", "changesetBaseRefs", "changesetIgnorePatterns", "checksumBehavior",
    "cloneConcurrency", "compressionLevel", "constraintsPath", "defaultLanguageName", "defaultProtocol", "defaultSemverRangePrefix", "deferredVersionFolder", "enableColors",
    "enableConstraintsChecks", "enableGlobalCache", "enableHardenedMode", "enableHyperlinks", "enableImmutableCache", "enableImmutableInstalls", "enableInlineBuilds",
    "enableInlineHunks", "enableMessageNames", "enableMirror", "enableNetwork", "enableOfflineMode", "enableProgressBars", "enableScripts", "enableStrictSsl",
    "enableTelemetry", "enableTimers", "enableTransparentWorkspaces", "globalFolder", "httpProxy", "httpRetry", "httpTimeout", "httpsCaFilePath", "httpsCertFilePath",
    "httpsKeyFilePath", "httpsProxy", "ignoreCwd", "immutablePatterns", "initFields", "initScope", "initEditorConfig", "injectEnvironmentFiles", "installStatePath",
    "lockfileFilename", "logFilters", "networkConcurrency", "networkSettings", "nmHoistingLimits", "nmMode", "nmSelfReferences", "nodeLinker", "npmAlwaysAuth",
    "npmAuditExcludePackages", "npmAuditIgnoreAdvisories", "npmAuditRegistry", "npmAuthIdent", "npmAuthToken", "npmMinimalAgeGate", "npmPreapprovedPackages",
    "npmPublishAccess", "npmPublishProvenance", "npmPublishRegistry", "npmRegistries", "npmRegistryServer", "npmScopes", "packageExtensions", "patchFolder", "pnpDataPath",
    "pnpEnableEsmLoader", "pnpEnableInlining", "pnpFallbackMode", "pnpIgnorePatterns", "pnpMode", "pnpShebang", "pnpUnpluggedFolder", "preferDeferredVersions",
    "preferInteractive", "preferReuse", "preferTruncatedLines", "progressBarStyle", "supportedArchitectures", "taskPoolConcurrency", "taskPoolMode", "telemetryInterval",
    "telemetryUserId", "tsEnableAutoTypes", "unsafeHttpWhitelist", "virtualFolder"];
pub fn yarnrc_without_code(text: &str) -> String {
    let (mut out, mut skipping) = (String::new(), false);
    for line in text.lines() {
        let top = !line.is_empty() && !line.starts_with([' ', '\t', '-', '#']);
        if top {
            let key = line.split_once(':').map_or("", |(k, _)| k.trim().trim_matches(['"', '\'']));
            skipping = !YARN_SETTINGS.contains(&key);
        }
        if !skipping { out.push_str(line); out.push('\n'); }
    }
    out
}
pub fn locked_mode(mode: u32, dir: bool) -> u32 { if dir { 0o1777 } else { (mode & 0o755) | 0o444 } }
pub const PROGRESS_ARGS: [&str; 3] = ["--reporter=/kern/vitest-progress.mjs", "--reporters=default", "--reporters=/kern/jest-progress.cjs"];
const LOAD_FAILURES: [&str; 3] = ["Cannot find module", "Failed to load url", "ERR_MODULE_NOT_FOUND"];
pub fn kern_failed(errors: &str) -> bool { errors.lines().any(|l| l.contains("/kern/") && LOAD_FAILURES.iter().any(|m| l.contains(m))) }
pub fn retried_command(command: &[String], errors: &str) -> Option<Vec<String>> {
    let unknown = |arg: &str| {
        let name = arg.trim_start_matches("--").trim_start_matches("no-").split('=').next().unwrap_or("");
        [name, name.split('.').next().unwrap_or("")].iter().any(|n| errors.contains(&format!("Unknown option `--{n}`")))
    };
    let progress = errors.contains("/kern/vitest-progress.mjs") || errors.contains("/kern/jest-progress.cjs");
    let shorter: Vec<String> = command.iter().filter(|a| !(["--no-cache", "--bail=0", "--sequence.shuffle=false"].contains(&a.as_str()) && unknown(a))
        && !(progress && PROGRESS_ARGS.contains(&a.as_str()))).cloned().collect();
    (shorter.len() < command.len()).then_some(shorter)
}
pub fn own_reporters(show: &str) -> bool {
    let Some(start) = show.find('{') else { return false };
    let Some(Ok(v)) = serde_json::Deserializer::from_str(&show[start..]).into_iter::<Value>().next() else { return false };
    v["globalConfig"]["reporters"].as_array().is_some_and(|list| list.iter().any(|r| {
        let name = r.as_str().or_else(|| r[0].as_str()).unwrap_or("");
        !["default", "summary", "github-actions"].contains(&name)
    }))
}
pub fn early_stop(started_ms: u64, now_ms: u64, timeout: u64, progress: &str) -> Option<String> {
    let (mut total, mut seen, mut times) = (0u64, std::collections::BTreeSet::new(), Vec::new());
    for line in progress.lines() {
        if let Some(n) = line.strip_prefix("total ") { total = total.max(n.trim().parse().unwrap_or(0)); continue; }
        let Some((at, path)) = line.strip_prefix("done ").and_then(|r| r.split_once(' ')) else { continue };
        if let Ok(at) = at.parse::<u64>() && seen.insert(path) { times.push(at.saturating_sub(started_ms) / 1000); }
    }
    let (elapsed, done) = (now_ms.saturating_sub(started_ms) / 1000, times.len() as u64);
    if total < 10 || done >= total || elapsed < (timeout / 6).max(10) { return None; }
    if done == 0 { return (total >= 20 && 2 * elapsed >= timeout).then(|| format!("stalled {elapsed}")); }
    let span = elapsed.saturating_sub(times.iter().copied().min().unwrap_or(0));
    if done < 10 || span < 30 { return None; }
    let left = total - done;
    let window = (elapsed / 3).max(30);
    let recent = times.iter().filter(|t| **t + window >= elapsed).count() as u64;
    let steady = elapsed + left * span / (done - 1);
    let lately = if recent >= 3 { elapsed + left * window / recent } else { steady };
    let projected = steady.min(lately);
    (projected > 4 * timeout).then(|| format!("projected {projected} {elapsed}"))
}
pub fn first_error(text: &str) -> Option<&str> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && l.len() <= 400).collect();
    let thrown = |l: &str| l.split_once(": ").is_some_and(|(head, _)| head.len() < 40 && !head.contains(' ') && (head.ends_with("Error") || head.ends_with("Exception")) || head == "Usage Error");
    let marked = |l: &str| {
        let x = l.to_ascii_lowercase();
        ["error", "invalid", "incompatible", "not compatible", "failed"].iter().any(|w| x.contains(w)) && !x.contains("failed with errors")
    };
    lines.iter().copied().find(|l| thrown(l)).or_else(|| lines.iter().copied().find(|l| marked(l)))
}
const STALE: &str = "installed from package.json (the lockfile is out of date)";
pub const UNSCRIPTED: &str = "installed without its packages' install scripts, because they failed";
pub const COREPACK: &str = "corepack@0.36.0";
pub fn plain_version(v: &str) -> bool {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    v.len() <= 64 && core.split('.').count() == 3 && core.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && (!v.contains('-') || (!pre.is_empty() && pre.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')))
}
pub fn version_key(v: &str) -> (Vec<u64>, bool, String) {
    let (core, pre) = v.split_once('-').unwrap_or((v, ""));
    (core.split('.').map(|p| p.parse().unwrap_or(0)).collect(), pre.is_empty(), pre.to_string())
}
const VERSION_DIRS_COUNT_MAX: usize = 50_000;
const VERSION_DEPTH_COUNT_MAX: u32 = 32;
const PACKAGE_JSON_BYTES_MAX: u64 = 1 << 20;
pub fn installed_versions(root: &Path, names: &[&str]) -> Vec<String> {
    fn version(dir: &Path, name: &str) -> Option<String> {
        let file = dir.join("package.json");
        if std::fs::metadata(&file).ok()?.len() > PACKAGE_JSON_BYTES_MAX { return None; }
        let v: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
        v["version"].as_str().filter(|s| v["name"] == name && plain_version(s)).map(str::to_string)
    }
    fn dirs(dir: &Path, dirs_left: &mut usize) -> Vec<(String, PathBuf)> {
        if *dirs_left == 0 { return Vec::new(); }
        *dirs_left -= 1;
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        entries.flatten().filter(|e| e.file_type().is_ok_and(|k| k.is_dir())).map(|e| (e.file_name().to_string_lossy().to_string(), e.path())).collect()
    }
    fn modules(dir: &Path, names: &[&str], depth: u32, dirs_left: &mut usize, found: &mut std::collections::BTreeSet<String>) {
        if depth > VERSION_DEPTH_COUNT_MAX { return; }
        for (name, path) in dirs(dir, dirs_left) {
            if names.contains(&name.as_str()) { found.extend(version(&path, &name)); }
            let inner: Vec<PathBuf> = if name.starts_with('@') || name == ".pnpm" { dirs(&path, dirs_left).into_iter().map(|(_, p)| p.join("node_modules")).collect() } else { vec![path.join("node_modules")] };
            for n in inner.into_iter().filter(|n| n.is_dir()) { modules(&n, names, depth + 1, dirs_left, found); }
        }
    }
    fn tree(dir: &Path, names: &[&str], depth: u32, dirs_left: &mut usize, found: &mut std::collections::BTreeSet<String>) {
        if depth > VERSION_DEPTH_COUNT_MAX { return; }
        for (name, path) in dirs(dir, dirs_left) {
            if name == "node_modules" { modules(&path, names, depth + 1, dirs_left, found); }
            else if ![".git", ".kern-install", ".kern-pnpm-store", ".kern-tools"].contains(&name.as_str()) { tree(&path, names, depth + 1, dirs_left, found); }
        }
    }
    let (mut dirs_left, mut found) = (VERSION_DIRS_COUNT_MAX, std::collections::BTreeSet::new());
    tree(root, names, 0, &mut dirs_left, &mut found);
    let mut list: Vec<String> = found.into_iter().collect();
    list.sort_by_key(|v| version_key(v));
    list
}
const RESTORE_ENTRIES_COUNT_MAX: usize = 4_000_000;
pub fn restore(committed: &Path, tree: &Path, keep: &[PathBuf]) -> R<()> {
    fn clear(dir: &Path, keep: &[PathBuf]) -> R<()> {
        for e in std::fs::read_dir(dir)?.flatten() {
            let path = e.path();
            if keep.contains(&path) { continue; }
            let kind = e.file_type()?;
            if kind.is_dir() && keep.iter().any(|k| k.starts_with(&path)) { clear(&path, keep)?; }
            else if kind.is_dir() { std::fs::remove_dir_all(&path)?; }
            else { std::fs::remove_file(&path)?; }
        }
        Ok(())
    }
    fn copy(from: &Path, to: &Path, keep: &[PathBuf], entries_left: &mut usize) -> R<()> {
        std::fs::create_dir_all(to)?;
        #[cfg(unix)]
        std::fs::set_permissions(to, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
        for e in std::fs::read_dir(from)?.flatten() {
            ensure!(*entries_left > 0, "The committed tree holds more than {RESTORE_ENTRIES_COUNT_MAX} entries");
            *entries_left -= 1;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if keep.contains(&dst) { continue; }
            let meta = std::fs::symlink_metadata(&src)?;
            if meta.is_dir() { copy(&src, &dst, keep, entries_left)?; continue; }
            if !meta.is_file() { continue; }
            std::fs::copy(&src, &dst)?;
            #[cfg(unix)]
            std::fs::set_permissions(&dst, std::os::unix::fs::PermissionsExt::from_mode(if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o111 != 0 { 0o755 } else { 0o644 }))?;
        }
        Ok(())
    }
    if tree.is_dir() { clear(tree, keep)?; }
    copy(committed, tree, keep, &mut RESTORE_ENTRIES_COUNT_MAX.clone())
}
pub fn with_workspace(dir: &str, has_node_modules: bool, records: &[Value]) -> bool {
    dir != "." && has_node_modules && records.iter().any(|r| r["dir"] == "." && r["result"].as_str().is_some_and(|s| s.starts_with("installed")))
}
pub fn install_plan_for(dir: &Path, scripts: bool) -> (&'static str, Vec<(Vec<String>, &'static str)>) {
    let (manager, attempts) = install_plan(dir);
    if !scripts { return (manager, attempts); }
    let mut scripted: Vec<(Vec<String>, &'static str)> = attempts.iter().map(|(argv, said)| {
        (argv.iter().filter(|a| !["--ignore-scripts", "--ignore-pnpmfile", "--mode=skip-build"].contains(&a.as_str())).cloned().collect(), *said)
    }).collect();
    scripted.extend(attempts.into_iter().map(|(argv, _)| (argv, UNSCRIPTED)));
    (manager, scripted)
}
pub struct Capped { head: Vec<u8>, tail: std::collections::VecDeque<u8>, dropped: u64, head_max: usize, tail_max: usize }
impl Capped {
    pub fn new(head_max: usize, tail_max: usize) -> Capped { Capped { head: Vec::new(), tail: std::collections::VecDeque::new(), dropped: 0, head_max, tail_max } }
    pub fn push(&mut self, bytes: &[u8]) {
        let (first, rest) = bytes.split_at(self.head_max.saturating_sub(self.head.len()).min(bytes.len()));
        self.head.extend_from_slice(first);
        self.tail.extend(rest);
        let over = self.tail.len().saturating_sub(self.tail_max);
        if over > 0 { self.tail.drain(..over); self.dropped += over as u64; }
    }
    pub fn finish(self) -> Vec<u8> {
        let mut out = self.head;
        if self.dropped > 0 { out.extend_from_slice(format!("\n… ({} bytes left out)\n", self.dropped).as_bytes()); }
        out.extend(self.tail);
        out
    }
}
pub const TOOLS: &str = "/workspace/.kern-tools";
const IMAGE_COREPACK: &str = "/usr/local/lib/node_modules/corepack/dist/corepack.js";
fn own_corepack() -> String { format!("{TOOLS}/corepack/node_modules/corepack/dist/corepack.js") }
const PNPM_OWN_COREPACK_MAJOR_MIN: u64 = 12;
fn pinned_major(dir: &Path, manager: &str) -> Option<u64> {
    let declared = |d: &Path| std::fs::read(d.join("package.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["packageManager"].as_str().map(str::to_string));
    let spec = declared(dir).or_else(|| declared(Path::new("/workspace")))?;
    let (name, version) = spec.split_once('@')?;
    if name != manager { return None; }
    version.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}
const PNPM_RANGE_CHARS_MAX: usize = 64;
fn pnpm_range(dir: &Path) -> Option<String> {
    let read = |d: &Path| std::fs::read(d.join("package.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let packages: Vec<Value> = [read(dir), read(Path::new("/workspace"))].into_iter().flatten().collect();
    if packages.iter().any(|p| p["packageManager"].is_string()) { return None; }
    let range = packages.iter().find_map(|p| p["engines"]["pnpm"].as_str())?.trim();
    let valid = range.len() <= PNPM_RANGE_CHARS_MAX && range.bytes().any(|b| b.is_ascii_digit()) && range.bytes().all(|b| b.is_ascii_digit() || b" .*xX^~<>=|-".contains(&b));
    valid.then(|| range.to_string())
}
pub fn shim(launcher: &[String]) -> String {
    let words: Vec<String> = launcher.iter().map(|a| format!("'{}'", a.replace('\'', r"'\''"))).collect();
    format!("#!/bin/sh\nexec {} \"$@\"\n", words.join(" "))
}
pub fn install_plan(dir: &Path) -> (&'static str, Vec<(Vec<String>, &'static str)>) {
    let node = |args: &[&str]| -> Vec<String> { ["/usr/local/bin/node"].iter().chain(args).map(|s| s.to_string()).collect() };
    let npm = |args: &[&str]| -> Vec<String> {
        node(&[&["/usr/local/lib/node_modules/npm/bin/npm-cli.js"], args, &["--engine-strict=false", "--no-audit", "--no-fund", "--no-update-notifier"]].concat())
    };
    let range = pnpm_range(dir);
    let own = range.is_some() || pinned_major(dir, "pnpm").is_some_and(|m| m >= PNPM_OWN_COREPACK_MAJOR_MIN);
    let corepack = if own { own_corepack() } else { IMAGE_COREPACK.to_string() };
    let (corepack, pnpm_name) = (corepack.as_str(), range.map_or_else(|| "pnpm".to_string(), |r| format!("pnpm@{r}")));
    let pnpm = |args: &[&str]| -> Vec<String> { node(&[&[corepack, pnpm_name.as_str()], args].concat()) };
    if dir.join("pnpm-lock.yaml").is_file() {
        let store = &format!("--store-dir={PNPM_STORE}");
        return ("pnpm", vec![(pnpm(&["install", "--frozen-lockfile", "--ignore-scripts", "--ignore-pnpmfile", store]), "installed"),
            (pnpm(&["install", "--no-frozen-lockfile", "--ignore-scripts", "--ignore-pnpmfile", store]), STALE)]);
    }
    if dir.join("yarn.lock").is_file() {
        if dir.join(".yarnrc.yml").is_file() {
            return ("yarn", vec![(node(&[corepack, "yarn", "install", "--immutable", "--mode=skip-build"]), "installed"),
                (node(&[corepack, "yarn", "install", "--no-immutable", "--mode=skip-build"]), STALE)]);
        }
        return ("yarn", vec![(node(&[corepack, "yarn", "install", "--frozen-lockfile", "--ignore-scripts", "--non-interactive", "--ignore-engines"]), "installed"),
            (node(&[corepack, "yarn", "install", "--ignore-scripts", "--non-interactive", "--ignore-engines"]), STALE)]);
    }
    let loose = npm(&["install", "--ignore-scripts", "--no-package-lock", "--legacy-peer-deps"]);
    if dir.join("package-lock.json").is_file() || dir.join("npm-shrinkwrap.json").is_file() {
        return ("npm", vec![(npm(&["ci", "--ignore-scripts"]), "installed"), (npm(&["install", "--ignore-scripts"]), STALE),
            (loose, "installed from package.json (the lockfile is out of date and npm's peer dependency checks were relaxed)")]);
    }
    let declared = std::fs::read(dir.join("package.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["packageManager"].as_str().map(|s| s.split('@').next().unwrap_or("").to_string())).unwrap_or_default();
    if declared == "pnpm" || dir.join("pnpm-workspace.yaml").is_file() {
        let store = &format!("--store-dir={PNPM_STORE}");
        return ("pnpm", vec![(pnpm(&["install", "--no-frozen-lockfile", "--ignore-scripts", "--ignore-pnpmfile", store]), "installed from package.json (the project has no lockfile)")]);
    }
    if declared == "yarn" && dir.join(".yarnrc.yml").is_file() {
        return ("yarn", vec![(node(&[corepack, "yarn", "install", "--no-immutable", "--mode=skip-build"]), "installed from package.json (the project has no lockfile)")]);
    }
    if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
        return ("bun", vec![(loose, "installed from package.json (Kern does not read Bun's lockfile)")]);
    }
    ("npm", vec![(npm(&["install", "--ignore-scripts", "--no-package-lock"]), "installed"), (loose, "installed from package.json (npm's peer dependency checks were relaxed)")])
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
pub fn image_major(image: &str) -> Option<u64> {
    let info = docker_json(&["image", "inspect", image]).ok()?;
    info[0]["Config"]["Env"].as_array()?.iter().filter_map(Value::as_str).find_map(|e| e.strip_prefix("NODE_VERSION="))?.split('.').next()?.trim_start_matches('v').parse().ok()
}
pub fn pull_node(major: u64) -> R<String> {
    ensure!(crate::nodeapi::NODE_MAJORS.contains(&major), "Kern runs Node {}", crate::nodeapi::NODE_MAJORS.map(|m| m.to_string()).join(", "));
    let tag = format!("node:{major}-bookworm-slim");
    let pulled = docker(&["pull", "-q", &tag], Duration::from_secs(600), 1 << 20).map(|_| ());
    let id = pulled_or_kept(pulled, || docker_json(&["image", "inspect", &tag])?[0]["Id"].as_str().map(str::to_string).ok_or_else(|| Denied(format!("Docker has no {tag}"))))?;
    validate_image(&id)
}
pub fn pulled_or_kept(pulled: R<()>, on_disk: impl FnOnce() -> R<String>) -> R<String> {
    match (pulled, on_disk()) { (_, Ok(id)) => Ok(id), (Err(e), Err(_)) | (Ok(()), Err(e)) => Err(e) }
}
pub fn build_browser(base: &str, dockerfile: &dyn Fn(&str, &[String]) -> String, versions: &[String]) -> R<String> {
    ensure!(versions.len() <= 8 && versions.iter().all(|v| plain_version(v)), "Unexpected Playwright version");
    let base = validate_image(base)?;
    let tag = format!("kern-browser:{}", &sha256(format!("{base} {}", versions.join(" ")).as_bytes())[..16]);
    if let Ok(i) = docker_json(&["image", "inspect", &tag]) && let Some(id) = i[0]["Id"].as_str() { return validate_image(id); }
    let short = format!("kern-base:{}", &base.trim_start_matches("sha256:")[..16]);
    docker(&["tag", &base, &short], Duration::from_secs(60), 1 << 20)?;
    let dir = std::env::temp_dir().join(format!("kern-browser-{}", uuid4()?));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("Dockerfile"), dockerfile(&short, versions))?;
    let built = docker(&["build", "--quiet", "--tag", &tag, "--file", &dir.join("Dockerfile").to_string_lossy(), &dir.to_string_lossy()], Duration::from_secs(900), 1 << 20);
    let _ = std::fs::remove_dir_all(&dir);
    built?;
    let id = docker_json(&["image", "inspect", &tag])?[0]["Id"].as_str().map(str::to_string).ok_or_else(|| Denied("The browser image was not built".into()))?;
    validate_image(&id)
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
pub fn container_args(spec: &Spec, name: &str) -> R<Vec<String>> { container_args_for(spec, name, runtime_name()) }
pub fn container_args_for(spec: &Spec, name: &str, runtime: &str) -> R<Vec<String>> {
    ensure!(matches!(runtime, "runsc" | "runc"), "Unknown container runtime");
    let (drive, drive_setup) = (spec.entry.first().is_some_and(|e| e == "__drive"), spec.entry.first().is_some_and(|e| e == "__drive_setup"));
    let drive_test = spec.entry.first().is_some_and(|e| e == "__drive_test");
    let drive_seal = spec.entry.first().is_some_and(|e| e == "__drive_seal");
    ensure!(spec.network == "none" || (spec.network == "bridge" && spec.root && spec.writable.is_empty()), "Only dependency setup may use a network");
    if drive_seal {
        ensure!(matches!(spec.workspace, Some((_, false))) && spec.network == "none" && spec.root && spec.writable.is_empty() && !spec.scratch && spec.volumes.is_empty(),
            "A seal takes ownership of what a prepare step made, as root without a network, in one tree volume");
    }
    if drive_test {
        ensure!(spec.workspace.is_some() && spec.network == "none" && !spec.root && spec.writable.is_empty() && !spec.scratch && spec.volumes.is_empty(),
            "A test run uses one installed tree, as an unprivileged user without a network or another volume");
    }
    ensure!(spec.volumes.iter().all(|(v, t, ro)| v.starts_with("kern-deps-") && (*t == "/workspace/node_modules" || (*t == "/kern-tools/node_modules" && *ro && drive))), "Unexpected volume");
    if drive { ensure!(matches!(spec.workspace, Some((_, true))) && spec.network == "none" && !spec.root && spec.writable.is_empty() && !spec.scratch, "A drive run reads an installed tree read-only, as an unprivileged user without a network"); }
    if drive_setup { ensure!(matches!(spec.workspace, Some((_, false))) && spec.network == "bridge" && spec.root && spec.volumes.is_empty() && spec.writable.is_empty() && !spec.scratch, "A drive setup installs into its own tree volume as root with a network, and mounts nothing else"); }
    ensure!(spec.workspace.is_none_or(|(v, _)| (drive || drive_setup || drive_test || drive_seal) && v.starts_with("kern-deps-")) && (spec.workspace.is_some() || !(drive || drive_setup || drive_test || drive_seal)),
        "Only drive runs, their test runs and their setup mount a tree volume, and only one Kern made");
    ensure!(spec.entry.first().is_some_and(|e| e.starts_with("__")), "Sandbox entry must be a Kern entry point");
    ensure!(!spec.scratch || (spec.writable.is_empty() && !spec.root && spec.network == "none"), "A scratch workspace is only for an unprivileged run without write grants or network");
    ensure!(spec.scratch || !matches!(spec.entry[0].as_str(), "__exec" | "__mutate"), "Tests run only on a throwaway copy of the workspace");
    let l = spec.limits;
    let user = if spec.root { "0:0" } else { "1000:1000" };
    let cpus = cpus(spec);
    let files = if drive || drive_setup || drive_test { "nofile=65536:65536" } else { "nofile=1024:1024" };
    let mut a: Vec<String> = [
        "create", "--name", name, "--pull=never", &format!("--runtime={runtime}"), &format!("--network={}", spec.network), "--read-only", &format!("--user={user}"), "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true", "--pids-limit", &l.pids.to_string(), "--memory", &format!("{}m", l.memory_mb), "--memory-swap", &format!("{}m", l.memory_mb), &format!("--cpus={cpus}"),
        "--ulimit", files, "--ulimit", "core=0", "--ipc=private", if drive_test { "--shm-size=512m" } else { "--shm-size=8m" }, "--log-driver=none", "--no-healthcheck", "--workdir=/workspace", "--entrypoint=/kern/kern",
    ].iter().map(|s| s.to_string()).collect();
    if drive_seal { a.extend(["--cap-add=CHOWN".to_string(), "--cap-add=DAC_OVERRIDE".to_string()]); }
    if drive_test { a.extend(["--mount".to_string(), "type=volume,target=/tmp".to_string()]); } else { a.extend(["--tmpfs".to_string(), format!("/tmp:rw,noexec,nosuid,nodev,size={},mode=1777", 64 << 20)]); }
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
    if spec.home_mb > 0 { a.extend(["--tmpfs".into(), format!("/home/kern:rw,exec,nosuid,nodev,size={}m,mode=0700", spec.home_mb)]); }
    for (volume, target, readonly) in &spec.volumes {
        a.extend(["--mount".into(), format!("type=volume,source={volume},target={target}{}", if *readonly { ",readonly" } else { "" })]);
        for dir in CACHE_DIRS.iter().filter(|_| *readonly && *target == "/workspace/node_modules") {
            a.extend(["--tmpfs".into(), format!("{target}/{dir}:rw,nosuid,nodev,size=268435456,uid=1000,gid=1000,mode=0700")]);
        }
    }
    a.push(spec.image.into());
    Ok(a)
}
pub struct Execution { pub stdout: Vec<u8>, pub exit_code: i64, pub builder: (String, Value), pub errors: Vec<u8> }
pub fn refusal(errors: &[u8]) -> Option<String> {
    String::from_utf8_lossy(errors).lines().rev().find_map(|l| serde_json::from_str::<Value>(l.trim()).ok().and_then(|v| v["reason"].as_str().map(str::to_string)))
}
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
    let outcome = (|| -> R<(Vec<u8>, Vec<u8>, i64)> {
        let i = &docker_json(&["inspect", &name])?[0];
        let h = &i["HostConfig"];
        let memory = spec.limits.memory_mb * 1048576;
        ensure!(h["Runtime"] == runtime_name() && h["NetworkMode"] == spec.network && h["ReadonlyRootfs"] == true && i["Config"]["User"] == if spec.root { "0:0" } else { "1000:1000" }, "Container configuration does not match the required boundary");
        ensure!(h["Memory"] == memory && h["MemorySwap"] == memory && h["PidsLimit"] == spec.limits.pids && h["NanoCpus"] == cpus(spec) as u64 * 1_000_000_000, "Container resource limits not applied");
        ensure!(h["Privileged"] == false && h["CapDrop"].as_array().is_some_and(|c| c.iter().any(|x| x == "ALL")) && h["SecurityOpt"].as_array().is_some_and(|s| s.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("no-new-privileges")))), "Container privilege constraints not applied");
        let added: Vec<String> = h["CapAdd"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|c| c.trim_start_matches("CAP_").to_string()).collect();
        let allowed: &[&str] = if spec.entry.first().is_some_and(|e| e == "__drive_seal") { &["CHOWN", "DAC_OVERRIDE"] } else { &[] };
        ensure!(added.len() == allowed.len() && allowed.iter().all(|w| added.iter().any(|a| a == w)), "Container capabilities do not match the required boundary");
        let mounts = i["Mounts"].as_array().cloned().unwrap_or_default();
        let binds: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "bind").collect();
        let volumes: Vec<&Value> = mounts.iter().filter(|m| m["Type"] == "volume").collect();
        ensure!(binds.len() == if spec.scratch || spec.workspace.is_some() { 2 } else { 3 } && binds.iter().all(|m| m["RW"] == false), "Unexpected or writable host mount");
        let tree: Vec<(&str, &str, bool)> = spec.workspace.iter().map(|(v, ro)| (*v, "/workspace", *ro)).collect();
        let temporary = usize::from(spec.entry.first().is_some_and(|e| e == "__drive_test"));
        let anonymous = |m: &Value| m["Destination"] == "/tmp" && m["RW"] == true && m["Name"].as_str().is_some_and(|n| n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit()));
        ensure!(volumes.len() == spec.volumes.len() + tree.len() + temporary && spec.volumes.iter().chain(&tree).all(|(v, t, ro)| volumes.iter().any(|m| m["Name"] == *v && m["Destination"] == *t && m["RW"] == !ro))
            && volumes.iter().filter(|m| anonymous(m)).count() == temporary, "Unexpected volume mount");
        ensure!(mounts.iter().all(|m| m["Type"] == "bind" || m["Type"] == "volume" || m["Type"] == "tmpfs"), "Unexpected mount type");
        let (_, out, err) = docker_full(&["start", "--attach", &name], Duration::from_secs(spec.limits.timeout_seconds + 5), (spec.limits.total_bytes as usize) * 2 + 1048576)?;
        let f = &docker_json(&["inspect", &name])?[0]["State"];
        if f["OOMKilled"] == true { crate::deny!("Sandbox exceeded limits: it ran out of memory (it allows {} MiB)", spec.limits.memory_mb); }
        ensure!(f["Running"] == false && f["ExitCode"].is_i64(), "Sandbox failed or exceeded limits");
        Ok((out, err[err.len().saturating_sub(65536)..].to_vec(), f["ExitCode"].as_i64().unwrap_or(-1)))
    })();
    docker(&["rm", "--force", "--volumes", &name], Duration::from_secs(120), 65536).map_err(|e| Denied(format!("Sandbox removal failed; no artifact accepted: {e}")))?;
    let (stdout, errors, exit_code) = outcome?;
    Ok(Execution { stdout, exit_code, builder, errors })
}
pub fn control_dir(dir: &Path, config: Option<(&str, &Value)>) -> R<()> {
    std::fs::create_dir(dir)?;
    let target = dir.join("kern");
    std::fs::copy(std::env::current_exe()?, &target)?;
    #[cfg(unix)]
    std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(0o555))?;
    create_new(dir.join("monitor.cjs"), MONITOR.as_bytes(), 0o444)?;
    create_new(dir.join("jest-report.cjs"), JEST_REPORT.as_bytes(), 0o444)?;
    create_new(dir.join("jest-progress.cjs"), JEST_PROGRESS.as_bytes(), 0o444)?;
    create_new(dir.join("vitest-progress.mjs"), VITEST_PROGRESS.as_bytes(), 0o444)?;
    create_new(dir.join("fault.cjs"), FAULT.as_bytes(), 0o444)?;
    create_new(dir.join("node-report.mjs"), NODE_REPORT.as_bytes(), 0o444)?;
    create_new(dir.join("trace.cjs"), TRACE.as_bytes(), 0o444)?;
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
    const OUT_HEAD_BYTES_MAX: usize = 1 << 20;
    const OUT_TAIL_BYTES_MAX: usize = 32 << 20;
    const ERR_HEAD_BYTES_MAX: usize = 64 << 10;
    const ERR_TAIL_BYTES_MAX: usize = 1 << 20;
    fn capture<T: std::io::Read + Send + 'static>(from: Option<T>, path: &'static str, head: usize, tail: usize) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut kept = super::Capped::new(head, tail);
            if let Some(mut source) = from {
                let mut buf = vec![0u8; 64 << 10];
                while let Ok(n) = std::io::Read::read(&mut source, &mut buf) { if n == 0 { break; } kept.push(&buf[..n]); }
            }
            let _ = std::fs::write(path, kept.finish());
        })
    }
    fn config(name: &str) -> R<Value> { read_json(Path::new("/kern").join(name)) }
    fn ended(mut child: std::process::Child, timeout: Duration) -> R<bool> {
        let started = std::time::Instant::now();
        loop {
            if child.try_wait()?.is_some() { return Ok(true); }
            if started.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); return Ok(false); }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
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
    fn lock_down(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        if meta.file_type().is_symlink() { return; }
        let mode = meta.permissions().mode() & 0o7777;
        let wanted = super::locked_mode(mode, meta.is_dir());
        if wanted != mode { let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(wanted)); }
        if meta.is_dir() && let Ok(entries) = std::fs::read_dir(path) {
            for e in entries.flatten() { lock_down(&e.path()); }
        }
    }
    const TEST_USER: u32 = 1000;
    const KERN_DIRS: [&str; 2] = [".kern-pnpm-store", ".kern-tools"];
    pub fn read_only_tree(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return };
        if !meta.is_dir() && !meta.is_file() { return; }
        let wanted = if meta.is_dir() || meta.mode() & 0o111 != 0 { 0o755 } else { 0o644 };
        if meta.mode() & 0o7777 != wanted { let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(wanted)); }
        if meta.is_dir() { for e in std::fs::read_dir(path).into_iter().flatten().flatten() { read_only_tree(&e.path()); } }
    }
    fn clear_added(path: &Path) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for e in entries.flatten() {
            if KERN_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) { continue; }
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.uid() == TEST_USER {
                let _ = if meta.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            } else if meta.is_dir() {
                clear_added(&p);
            }
        }
    }
    const CODE: [&str; 9] = ["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx", "node"];
    fn added_code(root: &Path, path: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() {
                if name != "node_modules" && name != ".git" && !KERN_DIRS.contains(&name.as_str()) { added_code(root, &p, out); }
                continue;
            }
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            if meta.uid() == TEST_USER && CODE.contains(&ext.as_str()) && !name.contains(".timestamp-") && out.len() < 200 {
                out.push(p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/"));
            }
        }
    }
    pub fn drive_setup() -> R<()> {
        let c = config("drive-setup.json")?;
        exact(&c, &["version", "dirs", "timeout_seconds", "scripts", "browsers"], "drive setup")?;
        let scripts = c["scripts"].as_bool().ok_or_else(|| Denied("Invalid drive setup".into()))?;
        let browsers = c["browsers"].as_bool().ok_or_else(|| Denied("Invalid drive setup".into()))?;
        ensure!(c["version"] == 1, "Unsupported drive setup");
        let dirs = strings(&c, "dirs", 64)?;
        for d in dirs.iter().filter(|d| *d != ".") { file_path(d)?; }
        let budget = Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?);
        let started = std::time::Instant::now();
        super::restore(Path::new("/input"), Path::new("/workspace"), &[])?;
        let scratch = Path::new("/workspace/.kern-install");
        std::fs::create_dir_all(scratch.join("tmp"))?;
        let (mut records, mut dirty, mut launchers) = (Vec::new(), false, std::collections::BTreeMap::new());
        for d in &dirs {
            let dir = if d == "." { PathBuf::from("/workspace") } else { Path::new("/workspace").join(d) };
            if !dir.join("package.json").is_file() { continue; }
            if super::with_workspace(d, dir.join("node_modules").exists(), &records) { records.push(json!({ "dir": d, "manager": null, "result": "installed with the workspace" })); continue; }
            let keep: Vec<PathBuf> = super::INSTALL_SCRATCH.iter().chain([&super::PNPM_STORE, &super::TOOLS]).map(PathBuf::from)
                .chain(records.iter().filter_map(|r| r["dir"].as_str()).filter(|r| *r != ".").map(|r| Path::new("/workspace").join(r))).collect();
            let (record, launcher) = install_dir(d, &dir, scripts, (budget, started), &keep, &mut dirty)?;
            if let (Some(m), Some(l)) = (record["manager"].as_str(), launcher) { launchers.entry(m.to_string()).or_insert(l); }
            records.push(record);
        }
        for dir in super::INSTALL_SCRATCH.iter().map(PathBuf::from).chain(super::store_links_back(Path::new(super::PNPM_STORE))) { let _ = std::fs::remove_dir_all(dir); }
        let (tools, bin) = (Path::new(super::TOOLS), Path::new(super::TOOLS).join("bin"));
        for (manager, launcher) in &launchers {
            std::fs::create_dir_all(&bin)?;
            std::fs::write(bin.join(manager), super::shim(launcher))?;
            std::fs::set_permissions(bin.join(manager), std::fs::Permissions::from_mode(0o755))?;
        }
        let cache = Path::new("/home/kern/corepack");
        if !launchers.is_empty() && cache.is_dir() { copy_entry(cache, &tools.join("corepack-home"))?; }
        if tools.is_dir() { std::fs::write(tools.join("package.json"), "{\"type\":\"commonjs\"}\n")?; }
        let playwright = if browsers { super::installed_versions(Path::new("/workspace"), &["playwright", "playwright-core"]) } else { Vec::new() };
        for dir in CACHE_DIRS { std::fs::create_dir_all(Path::new("/workspace/node_modules").join(dir))?; }
        lock_down(Path::new("/workspace"));
        for d in KERN_DIRS { read_only_tree(&Path::new("/workspace").join(d)); }
        println!("{}", json!({ "version": 1, "installs": records, "browsers": playwright }));
        Ok(())
    }
    fn npm_into(prefix: &Path, package: &str) {
        let _ = Command::new("/usr/local/bin/node").args(["/usr/local/lib/node_modules/npm/bin/npm-cli.js", "install", "--prefix", &prefix.to_string_lossy(), package, "--ignore-scripts",
            "--no-audit", "--no-fund", "--no-update-notifier"]).env_clear().envs(super::INSTALL_ENV).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    fn install_dir(d: &str, dir: &Path, scripts: bool, (budget, started): (Duration, std::time::Instant), keep: &[PathBuf], dirty: &mut bool) -> R<(Value, Option<Vec<String>>)> {
        let (manager, mut attempts) = super::install_plan_for(dir, scripts);
        let own = super::own_corepack();
        if !Path::new(super::IMAGE_COREPACK).exists() {
            for (argv, _) in attempts.iter_mut() { for a in argv.iter_mut() { if a == super::IMAGE_COREPACK { a.clone_from(&own); } } }
        }
        if attempts.iter().any(|(argv, _)| argv.contains(&own)) && !Path::new(&own).exists() {
            npm_into(&Path::new(super::TOOLS).join("corepack"), super::COREPACK);
            if !Path::new(&own).exists() {
                return Ok((json!({ "dir": d, "manager": manager, "result": format!("failed: Kern could not install {} with npm", super::COREPACK), "parts": [] }), None));
            }
        }
        let launcher: Option<Vec<String>> = attempts.first().and_then(|(argv, _)| argv.iter().position(|a| a == "install").map(|i| argv[..i].to_vec())).filter(|_| manager == "pnpm" || manager == "yarn");
        let berry = std::fs::read_to_string(dir.join(".yarnrc.yml")).ok().map(|t| yarnrc_without_code(&t));
        let committed = if d == "." { PathBuf::from("/input") } else { Path::new("/input").join(d) };
        let (mut result, mut originals, mut scripts_failed) = (json!("failed"), Vec::new(), None);
        let log = Path::new("/home/kern/install.log");
        for (argv, said) in &attempts {
            if *said == super::UNSCRIPTED && scripts_failed.is_none() { scripts_failed = result.as_str().map(str::to_string); }
            let left = budget.saturating_sub(started.elapsed());
            if left.is_zero() { result = json!("out of time"); break; }
            if *dirty { quiesce()?; super::restore(&committed, dir, keep)?; }
            *dirty = true;
            if let Some(rc) = &berry { std::fs::write(dir.join(".kern-yarnrc.yml"), rc)?; }
            originals = if manager == "bun" { bun_rewrite(dir)? } else { Vec::new() };
            let log_file = std::fs::File::create(log)?;
            let child = Command::new(&argv[0]).args(&argv[1..]).current_dir(dir).env_clear()
                .envs(super::INSTALL_ENV)
                .envs(berry.as_ref().map(|_| ("YARN_RC_FILENAME", ".kern-yarnrc.yml")))
                .stdin(Stdio::null()).stdout(Stdio::from(log_file.try_clone()?)).stderr(Stdio::from(log_file)).spawn()?;
            if wait(child, left)? { result = json!(said); break; }
            let text = std::fs::read(log).unwrap_or_default();
            let tail = String::from_utf8_lossy(&text[text.len().saturating_sub(600)..]).trim().to_string();
            let first = super::first_error(&String::from_utf8_lossy(&text)).map(|f| f.chars().take(300).collect::<String>()).filter(|f| !tail.contains(f.as_str()));
            result = json!(format!("failed: {}{tail}", first.map_or(String::new(), |f| format!("{f}\n…\n"))));
        }
        quiesce()?;
        for (path, text) in &originals { std::fs::write(path, text)?; }
        let parts = if result.as_str().is_some_and(|r| r.starts_with("installed")) && manager != "pnpm" { platform_parts(dir, (budget, started))? } else { Vec::new() };
        let mut record = json!({ "dir": d, "manager": manager, "result": result, "parts": parts });
        if result == super::UNSCRIPTED && let Some(why) = scripts_failed { record["scripts_failed"] = json!(why); }
        Ok((record, launcher))
    }
    fn platform_parts(dir: &Path, (budget, started): (Duration, std::time::Instant)) -> R<Vec<String>> {
        let dest = Path::new("/workspace/.kern-install/parts");
        std::fs::create_dir_all(dest)?;
        let mut parts = Vec::new();
        for (name, spec) in super::missing_platform_parts(&dir.join("node_modules")) {
            if budget.saturating_sub(started.elapsed()).is_zero() { break; }
            let packed = Command::new("/usr/local/bin/node").args(["/usr/local/lib/node_modules/npm/bin/npm-cli.js", "pack", &format!("{name}@{spec}"), "--ignore-scripts", "--silent"])
                .arg(format!("--pack-destination={}", dest.display())).current_dir(dir).env_clear()
                .envs([("PATH", "/usr/local/bin:/usr/bin:/bin"), ("HOME", "/home/kern"), ("LANG", "C.UTF-8"), ("npm_config_cache", "/workspace/.kern-install/npm"),
                    ("npm_config_loglevel", "error"), ("TMPDIR", "/workspace/.kern-install/tmp")]).stdin(Stdio::null()).output()?;
            let file = String::from_utf8_lossy(&packed.stdout).lines().last().unwrap_or("").trim().to_string();
            if !packed.status.success() || file.is_empty() || file.contains('/') { continue; }
            let target = dir.join("node_modules").join(&name);
            std::fs::create_dir_all(&target)?;
            let unpacked = Command::new("/bin/tar").arg("-xzf").arg(dest.join(&file)).arg("-C").arg(&target).args(["--strip-components=1", "--no-same-owner"]).status()?;
            if unpacked.success() { parts.push(format!("{name}@{spec}")); }
        }
        Ok(parts)
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
    fn seal(path: &Path, inside: bool, roots: &mut Vec<PathBuf>, count: &mut usize) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if path.file_name().is_some_and(|n| n == "node_modules") && e.file_name().to_str().is_some_and(|n| CACHE_DIRS.contains(&n)) { continue; }
            let made = meta.uid() == TEST_USER && std::os::unix::fs::lchown(&p, Some(0), Some(0)).is_ok();
            if made { *count += 1; if !inside { roots.push(p.clone()); } }
            if meta.is_dir() { seal(&p, inside || made, roots, count); }
        }
    }
    pub fn drive_seal() -> R<()> {
        let c = config("drive-seal.json")?;
        exact(&c, &["version"], "seal")?;
        ensure!(c["version"] == 1, "Unsupported seal");
        let (mut roots, mut count) = (Vec::new(), 0usize);
        seal(Path::new("/workspace"), false, &mut roots, &mut count);
        for r in &roots { lock_down(r); }
        println!("{}", json!({ "version": 1, "sealed": count }));
        Ok(())
    }
    fn served(c: &Value) -> R<bool> {
        let serve = c["serve"].as_object().filter(|f| f.len() <= super::SERVE_FILES_COUNT_MAX).ok_or_else(|| Denied("Invalid test run served files".into()))?;
        if serve.is_empty() { return Ok(false); }
        std::fs::create_dir_all("/tmp/.kern-serve")?;
        let mut manifest = serde_json::Map::new();
        for (i, (p, text)) in serve.iter().enumerate() {
            file_path(p)?;
            let text = text.as_str().ok_or_else(|| Denied("Invalid test run served files".into()))?;
            let body = format!("/tmp/.kern-serve/{i}");
            std::fs::write(&body, text)?;
            manifest.insert(format!("/workspace/{p}"), json!(body));
        }
        std::fs::write("/tmp/.kern-serve.json", serde_json::to_vec(&manifest)?)?;
        Ok(true)
    }
    fn fault_run(run: &[String], dir: &str, (env, options): (&[(String, String)], &str), target: &Path, wait: Duration) -> R<Value> {
        for f in ["/tmp/.kern-report.json", "/tmp/.kern-fault-served", "/tmp/.kern-fault-values"] { let _ = std::fs::remove_file(f); }
        let child = Command::new(&run[0]).args(&run[1..]).current_dir(dir).env_clear().envs(TEST_ENV).env("TZ", "UTC").envs(env.iter().cloned())
            .env("NODE_OPTIONS", format!("--require=/kern/fault.cjs{options}")).env("KERN_REPORT", "/tmp/.kern-report.json")
            .env("KERN_FAULT_FILE", target).env("KERN_FAULT_SOURCE", "/tmp/.kern-fault-src").env("KERN_FAULT_SERVED", "/tmp/.kern-fault-served").env("KERN_VALUES", "/tmp/.kern-fault-values")
            .stdin(Stdio::null()).stdout(Stdio::from(std::fs::File::create("/tmp/.kern-fault-out")?)).stderr(Stdio::from(std::fs::File::create("/tmp/.kern-fault-err")?)).spawn()?;
        let finished = ended(child, wait)?;
        quiesce()?;
        let report = some_of("/tmp/.kern-report.json", 4 << 20, false);
        let reported = report.iter().any(|b| !b.is_ascii_whitespace());
        let output = if reported { report } else { some_of("/tmp/.kern-fault-out", 4 << 20, true) };
        Ok(json!({ "finished": finished, "served": Path::new("/tmp/.kern-fault-served").exists(), "output": String::from_utf8_lossy(&output), "report": reported,
            "errors": String::from_utf8_lossy(&some_of("/tmp/.kern-fault-err", 16 << 10, true)), "values": String::from_utf8_lossy(&some_of("/tmp/.kern-fault-values", super::FAULT_VALUES_BYTES_MAX, false)) }))
    }
    pub fn drive_test() -> R<()> {
        let c = config("drive-test.json")?;
        exact(&c, &["version", "command", "timeout_seconds", "cwd", "env", "files", "early", "faults", "serve"], "test run")?;
        let early = c["early"].as_bool().ok_or_else(|| Denied("Invalid test run".into()))?;
        ensure!(c["version"] == 1, "Unsupported test run");
        let vars = c["env"].as_object().ok_or_else(|| Denied("Invalid test run environment".into()))?;
        ensure!(vars.len() <= 32, "Invalid test run environment");
        let mut env = Vec::new();
        for (k, v) in vars {
            let v = v.as_str().filter(|v| v.len() <= 4096 && !v.contains('\0')).ok_or_else(|| Denied("Invalid test run environment".into()))?;
            ensure!(crate::script::env_name(k), "Invalid test run environment");
            env.push((k.clone(), v.to_string()));
        }
        let options = env.iter().find(|(k, _)| k == "NODE_OPTIONS").map_or_else(String::new, |(_, v)| format!(" {v}"));
        env.retain(|(k, _)| k != "NODE_OPTIONS");
        let mut command = argv(&c["command"])?;
        let cwd = text(&c, "cwd")?;
        let dir = super::test_dir(cwd)?;
        command[0] = super::runner_in(cwd, &command[0], |p| Path::new(p).exists());
        ensure!(Path::new(&command[0]).exists(), "The test runner is not installed in the project: {} does not exist (Kern needs node_modules; Yarn Plug'n'Play projects have none)", command[0]);
        let timeout = Duration::from_secs(int(&c, "timeout_seconds", 1, 3600)?);
        clear_added(Path::new("/workspace"));
        let files = c["files"].as_object().filter(|f| f.len() <= 128).ok_or_else(|| Denied("Invalid test run files".into()))?;
        for (p, text) in files {
            file_path(p)?;
            let text = text.as_str().ok_or_else(|| Denied("Invalid test run files".into()))?;
            let target = Path::new("/workspace").join(p);
            ensure!(!p.split('/').any(|c| c == "node_modules") && std::fs::symlink_metadata(&target).is_err(), "A test run file may not replace a project file: {p}");
            if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
            create_new(&target, text.as_bytes(), 0o644)?;
        }
        let serving = served(&c)?;
        let faults = c["faults"].as_array().cloned().unwrap_or_default();
        ensure!(faults.len() <= super::FAULTS_COUNT_MAX, "Too many planted faults");
        if !faults.is_empty() {
            let started = std::time::Instant::now();
            let (mut results, mut plain) = (Vec::new(), false);
            for f in &faults {
                let path = text(f, "path")?;
                file_path(path)?;
                let (start, end) = (f["start"].as_u64().unwrap_or(0) as usize, f["end"].as_u64().unwrap_or(0) as usize);
                let (original, replacement) = (text(f, "original")?, text(f, "replacement")?);
                let mut run = argv(&f["command"])?;
                run[0] = super::runner_in(cwd, &run[0], |p| Path::new(p).exists());
                if plain { run.retain(|a| !super::PROGRESS_ARGS.contains(&a.as_str())); }
                let target = Path::new("/workspace").join(path);
                let body = std::fs::read_to_string(&target).unwrap_or_default();
                if started.elapsed() > timeout || body.get(start..end) != Some(original) { results.push(json!({ "finished": false })); continue; }
                std::fs::write("/tmp/.kern-fault-src", format!("{}{}{}", &body[..start], replacement, &body[end..]))?;
                let left = || timeout.saturating_sub(started.elapsed()).min(Duration::from_secs(120));
                let mut result = fault_run(&run, &dir, (&env, &options), &target, left())?;
                if result["report"] == false && let Some(shorter) = super::retried_command(&run, result["errors"].as_str().unwrap_or("")) {
                    plain = true;
                    result = fault_run(&shorter, &dir, (&env, &options), &target, left())?;
                }
                if let Some(r) = result.as_object_mut() { r.remove("report"); }
                results.push(result);
            }
            for (name, body) in [("output", serde_json::to_vec(&results)?), ("errors", Vec::new()), ("behavior", Vec::new()), ("created", Vec::new()), ("ending", b"faults".to_vec()), ("values", Vec::new()),
                ("trace", Vec::new())] { part(name, &body)?; }
            std::io::Write::flush(&mut std::io::stdout())?;
            return Ok(());
        }
        if command.iter().any(|a| a == super::PROGRESS_ARGS[2]) {
            let probe: Vec<String> = command.iter().filter(|a| !super::PROGRESS_ARGS.contains(&a.as_str())).cloned().chain(std::iter::once("--showConfig".to_string())).collect();
            let shown = std::fs::File::create("/tmp/.kern-show")?;
            let child = Command::new(&probe[0]).args(&probe[1..]).current_dir(&dir).env_clear().envs(TEST_ENV).env("TZ", "UTC").envs(env.iter().cloned())
                .env("NODE_OPTIONS", options.trim()).stdin(Stdio::null()).stdout(Stdio::from(shown)).stderr(Stdio::null()).spawn()?;
            let _ = wait(child, Duration::from_secs(60));
            if super::own_reporters(&std::fs::read_to_string("/tmp/.kern-show").unwrap_or_default()) { command.retain(|a| !super::PROGRESS_ARGS.contains(&a.as_str())); }
            let _ = std::fs::remove_file("/tmp/.kern-show");
        }
        let started = std::time::Instant::now();
        let epoch = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let mut ending;
        loop {
            for f in ["/tmp/.kern-progress", "/tmp/.kern-values", "/tmp/.kern-trace"] { let _ = std::fs::remove_file(f); }
            let hooks = if serving { format!("--require=/kern/monitor.cjs --require=/kern/fault.cjs{options}") } else { format!("--require=/kern/monitor.cjs{options}") };
            let tracing: Vec<(&str, &str)> = if serving { vec![("KERN_SERVE", "/tmp/.kern-serve.json"), ("KERN_TRACE", "/tmp/.kern-trace")] } else { Vec::new() };
            let (begun, mut look) = (epoch(), Duration::from_secs(5));
            let mut child = Command::new(&command[0]).args(&command[1..]).current_dir(&dir).env_clear().envs(TEST_ENV).env("TZ", "UTC").envs(env.iter().cloned())
                .env("NODE_OPTIONS", &hooks).env("KERN_MONITOR", "/tmp/.kern-behavior").env("KERN_REPORT", "/tmp/.kern-report.json")
                .env("KERN_PROGRESS", "/tmp/.kern-progress").env("KERN_VALUES", "/tmp/.kern-values").envs(tracing.iter().copied())
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
            let readers = [capture(child.stdout.take(), "/tmp/.kern-test-out", OUT_HEAD_BYTES_MAX, OUT_TAIL_BYTES_MAX),
                capture(child.stderr.take(), "/tmp/.kern-test-err", ERR_HEAD_BYTES_MAX, ERR_TAIL_BYTES_MAX)];
            ending = loop {
                if let Some(s) = child.try_wait()? { break format!("exit {}", s.code().unwrap_or(-1)); }
                if started.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); break "timeout".to_string(); }
                if early && started.elapsed() >= look {
                    look = started.elapsed() + Duration::from_secs(5);
                    let progress = String::from_utf8_lossy(&some_of("/tmp/.kern-progress", 8 << 20, false)).to_string();
                    if let Some(stop) = super::early_stop(begun, epoch(), timeout.as_secs(), &progress) { let _ = child.kill(); let _ = child.wait(); break stop; }
                }
                std::thread::sleep(Duration::from_millis(100));
            };
            quiesce()?;
            for r in readers { let _ = r.join(); }
            let errors = String::from_utf8_lossy(&some_of("/tmp/.kern-test-err", 64 << 10, true)).to_string();
            let reported = std::fs::metadata("/tmp/.kern-report.json").is_ok_and(|m| m.len() > 0);
            match super::retried_command(&command, &errors).filter(|_| ending.starts_with("exit") && !reported) {
                Some(shorter) => {
                    command = shorter;
                    for f in ["/tmp/.kern-report.json", "/tmp/.kern-behavior"] { let _ = std::fs::remove_file(f); }
                }
                None => break,
            }
        }
        quiesce()?;
        let report = some_of("/tmp/.kern-report.json", 64 << 20, false);
        part("output", &if report.iter().any(|b| !b.is_ascii_whitespace()) { report } else { some_of("/tmp/.kern-test-out", 32 << 20, true) })?;
        part("errors", &some_of("/tmp/.kern-test-err", 64 << 10, true))?;
        part("behavior", &some_of("/tmp/.kern-behavior", 4 << 20, false))?;
        let mut created = Vec::new();
        added_code(Path::new("/workspace"), Path::new("/workspace"), &mut created);
        part("created", created.join("\n").as_bytes())?;
        part("ending", ending.as_bytes())?;
        part("values", &some_of("/tmp/.kern-values", super::VALUES_BYTES_MAX, false))?;
        part("trace", &some_of("/tmp/.kern-trace", super::TRACE_BYTES_MAX, false))?;
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
    pub fn drive_seal() -> R<()> { deny!("Sandbox entry points run only inside a Linux sandbox") }
}
pub use inside::{drive, drive_seal, drive_setup, drive_test, exec, mutate, runner, setup};
#[cfg(unix)]
pub use inside::read_only_tree;
