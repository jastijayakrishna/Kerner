use base64::Engine;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Denied(pub String);
pub type R<T> = Result<T, Denied>;

impl fmt::Display for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}
impl fmt::Debug for Denied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}
impl From<std::io::Error> for Denied {
    fn from(e: std::io::Error) -> Self { Denied(e.to_string()) }
}
impl From<serde_json::Error> for Denied {
    fn from(e: serde_json::Error) -> Self { Denied(format!("Invalid JSON: {e}")) }
}

#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => { if !$cond { return Err($crate::util::Denied(format!($($msg)+))); } };
}
#[macro_export]
macro_rules! deny {
    ($($msg:tt)+) => { return Err($crate::util::Denied(format!($($msg)+))) };
}

pub fn say(text: &str) { let _ = writeln!(std::io::stdout().lock(), "{text}"); }
pub fn sha256(bytes: impl AsRef<[u8]>) -> String { hex::encode(Sha256::digest(bytes.as_ref())) }
pub fn is_hex64(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }
pub fn is_oid(s: &str) -> bool { s.len() == 40 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) }

fn integral(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_i64() || n.is_u64(),
        Value::Array(a) => a.iter().all(integral),
        Value::Object(o) => o.values().all(integral),
        _ => true,
    }
}
pub fn canonical(v: &Value) -> R<String> {
    ensure!(integral(v), "Canonical JSON permits integers only");
    Ok(serde_json::to_string(v)?)
}
pub fn digest(v: &Value) -> R<String> { Ok(sha256(canonical(v)?)) }

pub fn exact<'a>(v: &'a Value, keys: &[&str], label: &str) -> R<&'a Map<String, Value>> {
    let Some(o) = v.as_object() else { deny!("{label}: expected object") };
    let mut actual: Vec<&str> = o.keys().map(String::as_str).collect();
    let mut wanted = keys.to_vec();
    actual.sort_unstable();
    wanted.sort_unstable();
    ensure!(actual == wanted, "{label}: missing or unknown fields");
    Ok(o)
}
pub fn text<'a>(v: &'a Value, key: &str) -> R<&'a str> {
    v.get(key).and_then(Value::as_str).ok_or_else(|| Denied(format!("{key}: expected string")))
}
pub fn int(v: &Value, key: &str, min: u64, max: u64) -> R<u64> {
    let n = v.get(key).and_then(Value::as_u64).ok_or_else(|| Denied(format!("{key}: expected integer")))?;
    ensure!(n >= min && n <= max, "{key}: expected integer {min}..{max}");
    Ok(n)
}
pub fn strings(v: &Value, key: &str, max: usize) -> R<Vec<String>> {
    let Some(a) = v.get(key).and_then(Value::as_array) else { deny!("{key}: expected array") };
    ensure!(a.len() <= max, "{key}: too many entries");
    a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| Denied(format!("{key}: expected strings")))).collect()
}

const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];
pub fn file_path(p: &str) -> R<()> {
    ensure!(!p.is_empty() && p.len() <= 1024, "Invalid path length");
    for part in p.split('/') {
        let lower = part.to_ascii_lowercase();
        let stem = lower.split('.').next().unwrap_or("");
        let device = RESERVED.contains(&stem) || ((stem.starts_with("com") || stem.starts_with("lpt")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
        ensure!(
            !part.is_empty() && part.len() <= 255 && part != "." && part != ".." && !part.ends_with('.') && !part.ends_with(' ') && !device
                && !part.chars().any(|c| c.is_control() || "\\:*?\"<>|".contains(c)),
            "Nonportable path: {p}"
        );
        ensure!(lower != ".git" && !lower.starts_with(".kern"), "Reserved path: {p}");
    }
    Ok(())
}
pub fn capability(grant: &str) -> R<()> { file_path(grant.strip_suffix('/').unwrap_or(grant)) }
pub fn covers(grant: &str, candidate: &str) -> bool { grant == candidate || (grant.ends_with('/') && candidate.starts_with(grant)) }
pub fn allowed(grants: &[String], candidate: &str) -> bool { grants.iter().any(|g| covers(g, candidate)) }
pub fn tree_paths<'a>(names: impl IntoIterator<Item = &'a str>, check_prefixes: bool) -> R<()> {
    let names: Vec<&str> = names.into_iter().collect();
    let files: std::collections::HashSet<&str> = names.iter().copied().collect();
    let mut prefixes = std::collections::HashMap::<String, String>::new();
    for name in &names {
        file_path(name)?;
        let parts: Vec<&str> = name.split('/').collect();
        for i in 1..=parts.len() {
            let prefix = parts[..i].join("/");
            if let Some(seen) = prefixes.get(&prefix.to_lowercase()) {
                ensure!(*seen == prefix, "Case-colliding tree path");
            }
            ensure!(!check_prefixes || i == parts.len() || !files.contains(prefix.as_str()), "File/directory collision");
            prefixes.insert(prefix.to_lowercase(), prefix);
        }
    }
    Ok(())
}
pub fn argv(v: &Value) -> R<Vec<String>> {
    let Some(a) = v.as_array() else { deny!("Invalid command argv") };
    ensure!(!a.is_empty() && a.len() <= 64, "Invalid command argv");
    let args: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
    ensure!(args.len() == a.len() && args.iter().all(|s| s.len() <= 8192 && !s.contains('\0')), "Invalid command argv");
    ensure!(args[0].starts_with('/') && args[0].bytes().all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b)), "Executable must be an absolute container path");
    Ok(args)
}

pub fn read_json(path: impl AsRef<Path>) -> R<Value> {
    let path = path.as_ref();
    let meta = std::fs::metadata(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    ensure!(meta.len() <= 4 * 1048576, "JSON input too large: {}", path.display());
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
pub fn create_new(path: impl AsRef<Path>, bytes: &[u8], mode: u32) -> R<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(path.as_ref()).map_err(|e| Denied(format!("{}: {e}", path.as_ref().display())))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
pub fn write_json(path: impl AsRef<Path>, v: &Value) -> R<()> { create_new(path, format!("{}\n", canonical(v)?).as_bytes(), 0o600) }
pub fn fresh_dir(path: impl AsRef<Path>) -> R<PathBuf> {
    let path = path.as_ref();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    Ok(std::path::absolute(path)?)
}

pub fn b64(bytes: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(bytes) }
pub fn unb64(s: &str) -> R<Vec<u8>> {
    ensure!(s.len() <= 32 * 1048576, "Invalid base64");
    let bytes = base64::engine::general_purpose::STANDARD.decode(s).map_err(|_| Denied("Invalid base64".into()))?;
    ensure!(b64(&bytes) == s, "Noncanonical base64");
    Ok(bytes)
}

pub fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }
pub fn iso8601(ms: u64) -> String {
    let (secs, millis) = (ms / 1000, ms % 1000);
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}
pub fn random_bytes<const N: usize>() -> R<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| Denied(format!("OS randomness unavailable: {e}")))?;
    Ok(buf)
}
pub fn uuid4() -> R<String> {
    let mut b = random_bytes::<16>()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}
pub fn short_id() -> R<String> { Ok(hex::encode(random_bytes::<6>()?)) }
pub fn is_uuid(s: &str) -> bool { s.len() == 36 && s.bytes().enumerate().all(|(i, b)| if [8, 13, 18, 23].contains(&i) { b == b'-' } else { matches!(b, b'0'..=b'9' | b'a'..=b'f') }) }

pub fn safe_env(home: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = ["PATH", "SystemRoot", "WINDIR", "TEMP", "TMP"].iter().filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v))).collect();
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let home = home.display().to_string();
    for (k, v) in [("HOME", home.as_str()), ("USERPROFILE", home.as_str()), ("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_CONFIG_GLOBAL", null), ("GIT_TERMINAL_PROMPT", "0"), ("GIT_NO_REPLACE_OBJECTS", "1"), ("GIT_NO_LAZY_FETCH", "1"), ("GIT_OPTIONAL_LOCKS", "0"), ("LC_ALL", "C")] {
        env.push((k.into(), v.into()));
    }
    env
}

pub struct Run<'a> {
    pub cwd: Option<&'a Path>,
    pub env: Vec<(String, String)>,
    pub input: Option<&'a [u8]>,
    pub timeout: Duration,
    pub max_output: usize,
}
impl Default for Run<'_> {
    fn default() -> Self { Run { cwd: None, env: safe_env(&std::env::temp_dir()), input: None, timeout: Duration::from_secs(60), max_output: 16 * 1048576 } }
}
fn bounded(mut reader: impl Read + Send + 'static, max: usize) -> std::thread::JoinHandle<(Vec<u8>, bool)> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut chunk = [0u8; 65536];
        let mut overflow = false;
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 { break; }
            if out.len() + n > max { overflow = true; } else { out.extend_from_slice(&chunk[..n]); }
        }
        (out, overflow)
    })
}
pub fn run(exe: &Path, args: &[String], opts: Run) -> R<Vec<u8>> {
    let (status, stdout, stderr) = run_full(exe, args, opts)?;
    let name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tail = |b: &[u8]| String::from_utf8_lossy(&b[b.len().saturating_sub(1500)..]).trim().to_string();
    ensure!(status.success(), "{name} failed ({}): {} {}", status.code().map_or("signal".into(), |c| c.to_string()), tail(&stderr), tail(&stdout));
    Ok(stdout)
}
pub fn run_full(exe: &Path, args: &[String], opts: Run) -> R<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    let name = exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut cmd = Command::new(exe);
    cmd.args(args).env_clear().envs(opts.env.iter().map(|(k, v)| (k, v))).stdin(if opts.input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = opts.cwd { cmd.current_dir(cwd); }
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| Denied(format!("{name} could not start: {e}")))?;
    let _writer = opts.input.map(|input| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        let data = input.to_vec();
        std::thread::spawn(move || { let _ = stdin.write_all(&data); })
    });
    let out = bounded(child.stdout.take().expect("piped stdout"), opts.max_output);
    let err = bounded(child.stderr.take().expect("piped stderr"), 65536);
    let started = Instant::now();
    let mut exited = None;
    let status = loop {
        if exited.is_none() { exited = child.try_wait()?; }
        if let Some(status) = exited.filter(|_| out.is_finished() && err.is_finished()) { break status; }
        if started.elapsed() > opts.timeout {
            if exited.is_some() { deny!("{name} exited but left a background process holding its output") }
            let _ = child.kill();
            let _ = child.wait();
            deny!("{name} timed out after {}s", opts.timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    let (stdout, overflow) = out.join().map_err(|_| Denied("output reader failed".into()))?;
    let (stderr, _) = err.join().map_err(|_| Denied("error reader failed".into()))?;
    ensure!(!overflow, "{name} output exceeded {} bytes", opts.max_output);
    Ok((status, stdout, stderr))
}
