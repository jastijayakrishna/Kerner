use crate::attest::{self, KeySource};
use crate::policy::{contract, contract_digest, operator, require_role};
use crate::util::*;
use crate::{deny, ensure, flow, git};
use base64::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const MAX_BODY: usize = 96 << 20;
const MAX_BUNDLE: usize = 64 << 20;
const MAX_CONNECTIONS: usize = 32;

struct Tenant { operator: PathBuf, tasks: PathBuf, tokens: Vec<String> }
pub struct Service { signer: KeySource, profile: Value, tenants: BTreeMap<String, Tenant>, work: PathBuf, audit: PathBuf, one: Mutex<()>, open: AtomicUsize }

pub fn load(config: &Path) -> R<(SocketAddr, Service)> {
    require_role("verifier")?;
    let dir = std::path::absolute(config)?.parent().map(Path::to_path_buf).unwrap_or_default();
    let v = read_json(config)?;
    exact(&v, &["version", "listen", "signer", "profile", "repositories", "work"], "verifier service")?;
    ensure!(v["version"] == 1, "Unsupported verifier service configuration");
    let listen: SocketAddr = text(&v, "listen")?.parse().map_err(|_| Denied("listen: expected ip:port".into()))?;
    ensure!(listen.ip().is_loopback(), "Bind the verifier service to a loopback address and terminate TLS in a reverse proxy in front of it");
    let signer_path = dir.join(text(&v, "signer")?);
    let signer_text = std::fs::read_to_string(&signer_path).map_err(|e| Denied(format!("signer: {e}")))?;
    let signer = if signer_text.trim_start().starts_with("-----BEGIN PRIVATE KEY-----") { KeySource::file(&signer_text)? } else { KeySource::external(&signer_path)? };
    let profile = crate::init::load_profile(&dir.join(text(&v, "profile")?).display().to_string())?;
    ensure!(attest::public_key(text(&profile, "public_key")?)? == signer.verifying_key(), "The profile's public key does not match the signer");
    let Some(repos) = v["repositories"].as_object() else { deny!("repositories: expected object") };
    let mut tenants = BTreeMap::new();
    for (name, r) in repos {
        exact(r, &["operator", "tasks", "token_sha256"], "repository")?;
        let tokens = strings(r, "token_sha256", 64)?;
        ensure!(!tokens.is_empty() && tokens.iter().all(|t| is_hex64(t)), "{name}: token_sha256 must list SHA-256 digests of bearer tokens");
        let tenant = Tenant { operator: dir.join(text(r, "operator")?), tasks: dir.join(text(r, "tasks")?), tokens };
        ensure!(operator(&read_json(&tenant.operator)?)?.repository == *name, "{name}: operator.json names a different repository");
        tenants.insert(name.clone(), tenant);
    }
    let work = dir.join(text(&v, "work")?);
    std::fs::create_dir_all(&work)?;
    let audit = std::env::var_os("KERN_AUDIT_LOG").map_or_else(|| work.join("audit.jsonl"), PathBuf::from);
    Ok((listen, Service { signer, profile, tenants, work, audit, one: Mutex::new(()), open: AtomicUsize::new(0) }))
}
pub fn serve(config: &Path) -> R<()> {
    let (listen, service) = load(config)?;
    let listener = TcpListener::bind(listen)?;
    say(&json!({ "status": "LISTENING", "address": listener.local_addr()?.to_string() }).to_string());
    let service: &'static Service = Box::leak(Box::new(service));
    for stream in listener.incoming().flatten() {
        if service.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            service.open.fetch_sub(1, Ordering::SeqCst);
            let _ = respond(&stream, 503, &denial("Too many concurrent requests"));
            continue;
        }
        std::thread::spawn(move || { let _ = handle(service, &stream); service.open.fetch_sub(1, Ordering::SeqCst); });
    }
    Ok(())
}

fn denial(reason: &str) -> Value { json!({ "status": "DENIED", "reason": reason }) }
fn respond(mut stream: &TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let text = body.to_string();
    let phrase = match status { 200 => "OK", 401 => "Unauthorized", 404 => "Not Found", 422 => "Unprocessable Content", 503 => "Service Unavailable", _ => "Bad Request" };
    write!(stream, "HTTP/1.1 {status} {phrase}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{text}", text.len())
}
struct Request<'s> { method: String, path: String, token: Option<String>, length: usize, reader: BufReader<&'s TcpStream> }
impl Request<'_> {
    fn body(&mut self) -> R<Vec<u8>> {
        ensure!(self.length <= MAX_BODY, "Request body too large");
        let mut body = Vec::new();
        (&mut self.reader).take(self.length as u64).read_to_end(&mut body)?;
        ensure!(body.len() == self.length, "Incomplete request body");
        Ok(body)
    }
}
fn request(stream: &TcpStream) -> R<Request<'_>> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut reader = BufReader::new(stream);
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let n = (&mut reader).take(8192).read_line(&mut line)?;
        ensure!(n > 0 && line.ends_with('\n'), "Malformed request");
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.is_empty() { break; }
        ensure!(lines.len() < 64, "Too many request headers");
        lines.push(line);
    }
    let mut first = lines.first().map(String::as_str).unwrap_or("").split(' ');
    let (method, path) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let (mut length, mut token) = (0usize, None);
    for h in lines.iter().skip(1) {
        let (k, v) = h.split_once(':').ok_or_else(|| Denied("Malformed request header".into()))?;
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = v.trim().parse().map_err(|_| Denied("Malformed Content-Length".into()))?,
            "transfer-encoding" => deny!("Send a Content-Length body; chunked requests are not supported"),
            "authorization" => token = v.trim().strip_prefix("Bearer ").map(str::to_string),
            _ => {}
        }
    }
    Ok(Request { method, path, token, length, reader })
}
fn handle(s: &Service, stream: &TcpStream) -> std::io::Result<()> {
    let (status, body) = match request(stream) {
        Err(e) => (400, denial(&e.0)),
        Ok(mut r) => match (std::mem::take(&mut r.method).as_str(), std::mem::take(&mut r.path).as_str()) {
            ("GET", "/healthz") => (200, json!({ "status": "OK" })),
            ("GET", "/v1/profile") => (200, s.profile.clone()),
            ("POST", "/v1/verify") => verify(s, &mut r),
            _ => (404, denial("Not found")),
        },
    };
    respond(stream, status, &body)?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = std::io::copy(&mut stream.take(1 << 20), &mut std::io::sink());
    Ok(())
}
fn verify(s: &Service, r: &mut Request) -> (u16, Value) {
    let token = r.token.as_deref().map(sha256);
    if !token.as_ref().is_some_and(|t| s.tenants.values().any(|x| x.tokens.contains(t))) { return (401, denial("A valid bearer token is required")); }
    let parsed = (|| -> R<(Value, String, bool)> {
        let req: Value = serde_json::from_slice(&r.body()?)?;
        exact(&req, if req.get("evidence").is_some() { &["run", "bundle", "evidence"] } else { &["run", "bundle"] }, "verification request")?;
        git::validate_run(&req["run"])?;
        Ok((req["run"].clone(), text(&req, "bundle")?.to_string(), req["evidence"] == true))
    })();
    let (run, bundle, with_evidence) = match parsed { Ok(p) => p, Err(e) => return (400, denial(&e.0)) };
    let repo = run["repository"].as_str().unwrap_or("").to_string();
    let Some(tenant) = s.tenants.get(&repo) else { return (422, denial(&format!("This verifier does not serve {repo}"))) };
    if !token.as_ref().is_some_and(|t| tenant.tokens.contains(t)) { return (401, denial("This token may not verify that repository")); }
    let outcome = verification(s, tenant, &run, &bundle);
    let summary = outcome.as_ref().map(|(env, task, _)| json!({ "status": "VERIFIED", "task": task, "repository": repo, "commit": run["commit"], "attestation_sha256": digest(env).unwrap_or_default() }));
    if let Err(e) = crate::audit::append(&s.audit, &crate::audit::event("serve", &summary.map_err(|e| Denied(e.0.clone())))) { eprintln!("{}", json!({ "warning": format!("Audit log not written: {e}") })); }
    match outcome { Ok((env, _, evidence)) => (200, if with_evidence { json!({ "attestation": env, "evidence": evidence }) } else { env }), Err(e) => (422, denial(&e.0)) }
}
fn verification(s: &Service, tenant: &Tenant, run: &Value, bundle: &str) -> R<(Value, String, Value)> {
    let op = operator(&read_json(&tenant.operator)?)?;
    let want = text(run, "contract_sha256")?;
    let mut matched = None;
    for e in std::fs::read_dir(&tenant.tasks)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "json") && let Ok(v) = read_json(&p) && let Ok(c) = contract(&v) && contract_digest(&c).is_ok_and(|d| d == want) { matched = Some(c); break; }
    }
    let policy = matched.ok_or_else(|| Denied("No trusted task contract matches this run".into()))?;
    ensure!(bundle.len() <= MAX_BUNDLE.div_ceil(3) * 4, "Handoff bundle exceeds 64 MiB");
    let bytes = base64::engine::general_purpose::STANDARD.decode(bundle).map_err(|_| Denied("Invalid bundle encoding".into()))?;
    let _one = s.one.lock().unwrap_or_else(|e| e.into_inner());
    let root = fresh_dir(s.work.join(uuid4()?))?;
    let handoff = fresh_dir(root.join("handoff"))?;
    write_json(handoff.join("run.json"), run)?;
    create_new(handoff.join("artifact.bundle"), &bytes, 0o600)?;
    let outcome = git::import_handoff(&handoff, &root.join("run")).and_then(|_| flow::verify(&root.join("run"), &policy, &op, &root.join("out"), &s.signer));
    let _ = std::fs::remove_dir_all(&handoff);
    let _ = std::fs::remove_dir_all(root.join("run"));
    let envelope = outcome?;
    let evidence = read_json(root.join("out").join("compare.json")).unwrap_or(Value::Null);
    Ok((envelope, policy.task, evidence))
}
