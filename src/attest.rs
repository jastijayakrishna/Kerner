use crate::policy::{Pin, pin, pinned, require_role};
use crate::util::*;
use crate::{deny, ensure};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey, spki::der::pem::LineEnding};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

pub const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
pub const STATEMENT: &str = "https://in-toto.io/Statement/v1";
pub const PREDICATE: &str = "https://slsa.dev/verification_summary/v1";
pub const EXTENSION: &str = "https://kern.invalid/attestation/v3";
pub const LIFETIME_MS: u64 = 3_600_000;
const FIELDS: [&str; 17] = ["run_id", "task", "repository", "base", "commit", "manifest_sha256", "contract_sha256", "verifier_sha256", "executor", "verifier_builder_sha256", "deployment", "image", "runtime", "dependencies_sha256", "checks", "issued_at", "expires_at"];

pub fn pae(kind: &str, payload: &[u8]) -> Vec<u8> {
    let mut out = format!("DSSEv1 {} {kind} {} ", kind.len(), payload.len()).into_bytes();
    out.extend_from_slice(payload);
    out
}
pub fn public_key(pem: &str) -> R<VerifyingKey> {
    ensure!(pem.trim_start().starts_with("-----BEGIN PUBLIC KEY-----"), "Ed25519 public-key PEM required");
    VerifyingKey::from_public_key_pem(pem.trim()).map_err(|_| Denied("Ed25519 public-key PEM required".into()))
}
pub fn key_id(key: &VerifyingKey) -> R<String> {
    Ok(sha256(key.to_public_key_der().map_err(|_| Denied("Unencodable public key".into()))?.as_bytes()))
}
pub fn public_pem(key: &VerifyingKey) -> R<String> { key.to_public_key_pem(LineEnding::LF).map_err(|_| Denied("Unencodable public key".into())) }
pub fn keypair() -> R<(String, String)> {
    let key = SigningKey::from_bytes(&random_bytes::<32>()?);
    let private = key.to_pkcs8_pem(LineEnding::LF).map_err(|_| Denied("Unencodable private key".into()))?.to_string();
    Ok((private, public_pem(&key.verifying_key())?))
}

pub enum KeySource { File(SigningKey), External { executable: Pin, arguments: Vec<String>, public: VerifyingKey } }
impl KeySource {
    pub fn file(pem: &str) -> R<Self> {
        require_role("verifier")?;
        Ok(KeySource::File(SigningKey::from_pkcs8_pem(pem.trim()).map_err(|_| Denied("Ed25519 PKCS#8 signing key required".into()))?))
    }
    pub fn external(spec_file: &Path) -> R<Self> {
        require_role("verifier")?;
        let spec = read_json(spec_file)?;
        exact(&spec, &["version", "executable", "arguments", "public_key"], "external signer")?;
        ensure!(spec["version"] == 1, "Unsupported signer specification");
        let arguments = strings(&spec, "arguments", 32)?;
        ensure!(arguments.iter().all(|a| a.len() <= 4096 && !a.contains('\0')), "Invalid signer arguments");
        let executable = pin(&spec["executable"], "signer executable")?;
        pinned(&executable)?;
        let key_file = Path::new(text(&spec, "public_key")?);
        ensure!(key_file.is_absolute() && std::fs::symlink_metadata(key_file).is_ok_and(|m| m.is_file()), "Explicit regular public-key file required");
        Ok(KeySource::External { executable, arguments, public: public_key(&std::fs::read_to_string(key_file)?)? })
    }
    pub fn verifying_key(&self) -> VerifyingKey {
        match self { KeySource::File(k) => k.verifying_key(), KeySource::External { public, .. } => *public }
    }
    fn sign(&self, message: &[u8]) -> R<Signature> {
        let signature = match self {
            KeySource::File(k) => k.sign(message),
            KeySource::External { executable, arguments, .. } => {
                let exe = pinned(executable)?;
                let out = run(&exe, arguments, Run { cwd: Some(&std::env::temp_dir()), env: vec![("PATH".into(), String::new())], input: Some(message), timeout: Duration::from_secs(15), max_output: 1024 })?;
                let raw = unb64(String::from_utf8_lossy(&out).trim())?;
                Signature::from_slice(&raw).map_err(|_| Denied("External signer returned an invalid Ed25519 signature".into()))?
            }
        };
        ensure!(self.verifying_key().verify_strict(message, &signature).is_ok(), "Signer returned a signature that does not verify");
        Ok(signature)
    }
}

pub fn statement(k: &Value) -> R<Value> {
    let (repo, task) = (text(k, "repository")?, text(k, "task")?);
    Ok(json!({
        "_type": STATEMENT,
        "subject": [{ "name": repo, "digest": { "gitCommit": text(k, "commit")? } }],
        "predicateType": PREDICATE,
        "predicate": {
            "verifier": { "id": format!("https://kern.invalid/verifier/{}", text(k, "verifier_builder_sha256")?), "version": { "kern": env!("CARGO_PKG_VERSION"), "configuration": text(k, "verifier_sha256")? } },
            "timeVerified": iso8601(k["issued_at"].as_u64().unwrap_or(0)),
            "resourceUri": format!("git+https://github.com/{repo}"),
            "policy": { "uri": format!("kern:contract/{task}"), "digest": { "sha256": text(k, "contract_sha256")? } },
            "verificationResult": "PASSED",
            "verifiedLevels": ["KERN_AGENT_CHANGE_VERIFIED"],
            "slsaVersion": "1.1",
            EXTENSION: k,
        }
    }))
}
pub fn sign(kern: &Value, signer: &KeySource) -> R<Value> {
    let bytes = canonical(&statement(kern)?)?.into_bytes();
    let signature = signer.sign(&pae(PAYLOAD_TYPE, &bytes))?;
    Ok(json!({ "payloadType": PAYLOAD_TYPE, "payload": b64(&bytes), "signatures": [{ "keyid": key_id(&signer.verifying_key())?, "sig": b64(&signature.to_bytes()) }] }))
}

pub struct Expect<'a> {
    pub run_id: &'a str, pub task: &'a str, pub repository: &'a str, pub base: &'a str, pub commit: &'a str, pub manifest_sha256: &'a str, pub contract_sha256: &'a str,
    pub verifier_sha256: &'a str, pub verifier_builder: &'a str, pub deployment: &'a str, pub image: &'a str, pub executors: &'a [String], pub executor_builder: Option<&'a str>, pub checks: Vec<String>,
}
pub fn claimed(envelope: &Value) -> Option<Value> {
    let bytes = unb64(envelope["payload"].as_str()?).ok()?;
    Some(serde_json::from_slice::<Value>(&bytes).ok()?["predicate"][EXTENSION].clone())
}
pub fn signed(envelope: &Value, key: &VerifyingKey) -> R<Value> {
    exact(envelope, &["payloadType", "payload", "signatures"], "envelope")?;
    ensure!(envelope["payloadType"] == PAYLOAD_TYPE, "Attestation must be an in-toto DSSE envelope");
    let Some([signature]) = envelope["signatures"].as_array().map(Vec::as_slice) else { deny!("Exactly one signature required") };
    exact(signature, &["keyid", "sig"], "signature")?;
    ensure!(text(signature, "keyid")? == key_id(key)?, "Untrusted signing key");
    let bytes = unb64(text(envelope, "payload")?)?;
    let sig = Signature::from_slice(&unb64(text(signature, "sig")?)?).map_err(|_| Denied("Invalid attestation signature".into()))?;
    ensure!(key.verify_strict(&pae(PAYLOAD_TYPE, &bytes), &sig).is_ok(), "Invalid attestation signature");
    let stmt: Value = serde_json::from_slice(&bytes)?;
    let k = stmt["predicate"][EXTENSION].clone();
    exact(&k, &FIELDS, "Kern attestation")?;
    ensure!(canonical(&statement(&k)?)? == canonical(&stmt)?, "Attestation statement is not the canonical Kern verification summary");
    Ok(k)
}
pub fn verify(envelope: &Value, key: &VerifyingKey, expect: &Expect, now: u64, enforce_expiry: bool) -> R<Value> {
    let k = signed(envelope, key)?;
    for (field, want) in [("run_id", expect.run_id), ("task", expect.task), ("repository", expect.repository), ("base", expect.base), ("commit", expect.commit), ("manifest_sha256", expect.manifest_sha256), ("contract_sha256", expect.contract_sha256), ("verifier_sha256", expect.verifier_sha256), ("verifier_builder_sha256", expect.verifier_builder), ("deployment", expect.deployment), ("image", expect.image)] {
        ensure!(k[field] == want, "Attestation mismatch: {field}");
    }
    ensure!(k["runtime"] == "runsc", "Unsupported verification runtime");
    let e = &k["executor"];
    let kind = text(e, "kind")?;
    ensure!(expect.executors.iter().any(|x| x == kind), "Executor kind {kind} is not accepted by policy");
    if kind == "kern-gvisor" { ensure!(e["builder_sha256"].as_str() == expect.executor_builder && e["builder_sha256"].is_string(), "Attestation mismatch: executor builder"); }
    ensure!(k["dependencies_sha256"].is_null() || k["dependencies_sha256"].as_str().is_some_and(is_hex64), "Invalid dependency digest");
    let (issued, expires) = (k["issued_at"].as_u64().unwrap_or(0), k["expires_at"].as_u64().unwrap_or(0));
    ensure!(issued > 0 && issued <= now && expires > issued && expires - issued <= LIFETIME_MS, "Attestation time window invalid");
    ensure!(!enforce_expiry || expires > now, "Attestation expired; re-verify the artifact");
    let Some(checks) = k["checks"].as_array() else { deny!("Missing checks") };
    let mut seen = std::collections::BTreeSet::new();
    for c in checks {
        exact(c, &["id", "status", "evidence_sha256"], "check")?;
        ensure!(c["status"] == "PASS" && c["evidence_sha256"].as_str().is_some_and(is_hex64) && seen.insert(text(c, "id")?.to_string()), "Invalid, failed or duplicate check");
    }
    let mut want = expect.checks.clone();
    want.sort();
    ensure!(seen.into_iter().collect::<Vec<_>>() == want, "Required check set mismatch");
    Ok(k)
}
