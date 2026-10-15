//! Keyless proofs signed by GitHub. Inside a job with `permissions: id-token: write`, Kern asks GitHub for an OIDC token whose audience is
//! the fingerprint of a seal; GitHub signs it (RS256) with claims naming the repository, commit, run and the workflow that asked. Anyone can
//! check it later against GitHub's published keys, with no key of Kern's own to keep safe. The RSA check is written out here rather than taken
//! from an RSA crate: it is a little modular arithmetic, and comparing against the one block a valid signature can open to, rather than parsing
//! what it opened to, closes the door on the lenient-parser forgeries (Bleichenbacher 2006 and its descendants).
use crate::github::env;
use crate::util::*;
use crate::{deny, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use num_bigint::BigUint;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

pub const ISSUER: &str = "https://token.actions.githubusercontent.com";
/// Kern's reusable workflow; job_workflow_ref claims start with this, followed by a ref (refs/heads/..., refs/tags/...).
pub const WORKFLOW: &str = "jastijayakrishna/Kerner/.github/workflows/kern.yml@";
/// DER of DigestInfo { SHA-256, NULL }, which precedes the hash inside every RS256 signature (RFC 8017 section 9.2, note 1).
const SHA256_INFO: [u8; 19] = [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];

/// The audience that binds a token to a seal: "kern-receipt:" + sha256 of the seal's canonical JSON (util::canonical).
pub fn audience(seal: &Value) -> R<String> { Ok(format!("kern-receipt:{}", digest(seal)?)) }

/// One query value: everything but RFC 3986's unreserved characters percent-encoded.
fn query_value(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(char::from(b)); } else { out.extend(['%', char::from(HEX[usize::from(b >> 4)]), char::from(HEX[usize::from(b & 15)])]); }
    }
    out
}
fn agent() -> ureq::Agent { ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into() }

/// Inside GitHub Actions: GET $ACTIONS_ID_TOKEN_REQUEST_URL with an extra query parameter audience=<url-encoded aud>,
/// header "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN"; returns the "value" field. GitHub sets both variables only for a job
/// granted `permissions: id-token: write`, so their absence is reported as that. The environment is read on each call.
pub fn request(audience: &str) -> R<String> {
    let (Some(url), Some(bearer)) = (env("ACTIONS_ID_TOKEN_REQUEST_URL"), env("ACTIONS_ID_TOKEN_REQUEST_TOKEN")) else {
        deny!("GitHub offers this job no OIDC token: the workflow needs permissions: id-token: write")
    };
    ensure!(url.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p)), "ACTIONS_ID_TOKEN_REQUEST_URL must use https");
    let url = format!("{url}{}audience={}", if url.contains('?') { '&' } else { '?' }, query_value(audience));
    let mut r = agent().get(&url).header("Authorization", &format!("bearer {bearer}")).header("Accept", "application/json").call().map_err(|e| Denied(format!("GitHub's OIDC token service is unreachable: {e}")))?;
    ensure!(r.status().is_success(), "GitHub answered {} when asked for an OIDC token", r.status().as_u16());
    let v: Value = serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?;
    v["value"].as_str().filter(|t| !t.is_empty()).map(str::to_string).ok_or_else(|| Denied("GitHub's OIDC token service answered without a token".into()))
}

/// GitHub's published keys: GET {ISSUER}/.well-known/jwks (20 s timeout, no redirects).
pub fn jwks() -> R<Value> {
    let mut r = agent().get(&format!("{ISSUER}/.well-known/jwks")).header("Accept", "application/json").call().map_err(|e| Denied(format!("GitHub's OIDC keys are unreachable: {e}")))?;
    ensure!(r.status().is_success(), "GitHub answered {} for its OIDC keys", r.status().as_u16());
    let keys: Value = serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?;
    ensure!(keys["keys"].is_array(), "GitHub's OIDC keys are not a key set");
    Ok(keys)
}

/// A value from a token or key set inside an error message: JSON-quoted, so nothing in it can pass for more of the message, and clipped.
fn shown(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 200 { s } else { format!("{}...", s.chars().take(200).collect::<String>()) }
}
/// Strict base64url: no padding and no stray trailing bits, so each token has exactly one spelling.
fn base64url(s: &str, what: &str) -> R<Vec<u8>> { URL_SAFE_NO_PAD.decode(s).map_err(|_| Denied(format!("{what} is not base64url"))) }
fn object(s: &str, what: &str) -> R<Value> {
    let v: Value = serde_json::from_slice(&base64url(s, what)?).map_err(|_| Denied(format!("{what} is not JSON")))?;
    ensure!(v.is_object(), "{what} is not a JSON object");
    Ok(v)
}

/// An RSA public key: modulus n, exponent e and k, the length of n in bytes (the length of every signature it makes).
struct Rsa { n: BigUint, e: BigUint, k: usize }
/// The one key in `jwks` named `kid`: an RSA key for RS256 of 2048 bits or more.
fn rsa_key(jwks: &Value, kid: &str) -> R<Rsa> {
    let Some(keys) = jwks["keys"].as_array() else { deny!("The key set has no keys list") };
    let name = shown(&Value::from(kid));
    let named: Vec<&Value> = keys.iter().filter(|k| k["kid"] == kid).collect();
    ensure!(!named.is_empty(), "The token names key {name}, which is not among the published keys");
    let [key] = named[..] else { deny!("The published keys name key {name} more than once") };
    ensure!(key["kty"] == "RSA", "Key {name} is not an RSA key");
    ensure!(key.get("use").is_none_or(|u| u == "sig") && key.get("alg").is_none_or(|a| a == "RS256"), "Key {name} is not published for RS256 signatures");
    let int = |field: &str| -> R<BigUint> {
        let Some(s) = key[field].as_str() else { deny!("Key {name} has no {field}") };
        Ok(BigUint::from_bytes_be(&base64url(s, &format!("The {field} of key {name}"))?))
    };
    let (n, e) = (int("n")?, int("e")?);
    let bits = n.bits();
    ensure!(bits >= 2048, "Key {name} is {bits} bits; Kern accepts RSA keys of 2048 bits or more");
    // RFC 8017 wants an odd e from 3 to n - 1; e = 1 would make every block its own signature
    ensure!(e.bit(0) && e.bits() > 1 && e < n, "Key {name} has an unusable public exponent");
    Ok(Rsa { k: bits.div_ceil(8) as usize, n, e })
}
/// RSASSA-PKCS1-v1_5 with SHA-256 (RFC 8017 section 8.2.2): s^e mod n must be, byte for byte, the one block a signature over `message` opens
/// to, 0x00 0x01 FF..FF 0x00 || DigestInfo(SHA-256) || hash. Rebuilding it and comparing means nothing in the opened block is ever parsed.
fn rs256(key: &Rsa, message: &[u8], signature: &[u8], name: &str) -> R<()> {
    ensure!(signature.len() == key.k, "The token signature is {} bytes, but key {name} makes signatures of {} bytes", signature.len(), key.k);
    let s = BigUint::from_bytes_be(signature);
    ensure!(s < key.n, "The token signature is not smaller than the modulus of key {name}");
    let opened = s.modpow(&key.e, &key.n).to_bytes_be();
    let mut got = vec![0u8; key.k.saturating_sub(opened.len())];
    got.extend_from_slice(&opened);
    let mut want = vec![0x00, 0x01];
    want.resize(key.k - 1 - SHA256_INFO.len() - 32, 0xff);
    want.push(0x00);
    want.extend_from_slice(&SHA256_INFO);
    want.extend_from_slice(Sha256::digest(message).as_slice());
    ensure!(got == want, "The token signature does not match key {name}");
    Ok(())
}

/// Verifies an RS256 token against these keys and returns its claims. It checks:
/// - the header: alg RS256, a kid that names an RSA key in jwks.keys (kty RSA, n and e base64url), no `crit` extensions;
/// - the signature: RSASSA-PKCS1-v1_5 with SHA-256 over "<header>.<payload>", by rebuilding the expected block (see `rs256`); s must be
///   smaller than n, and only keys of 2048 bits or more count;
/// - the claims: iss == ISSUER; aud equals `audience` (a string, or an array that contains it); job_workflow_ref starts with
///   `workflow_prefix` (which must not be empty); repository equals `repository` when Some.
///
/// exp is not enforced: receipts are verified long after the job that made them. iat must be a number. Each failure is a Denied with a
/// plain-English reason that says which check failed.
pub fn verify(token: &str, jwks: &Value, audience: &str, workflow_prefix: &str, repository: Option<&str>) -> R<Value> {
    ensure!(!workflow_prefix.is_empty(), "No workflow to check the token against: the workflow prefix is empty");
    ensure!(token.len() <= 65536, "The token is longer than any GitHub OIDC token");
    let parts: Vec<&str> = token.split('.').collect();
    let [head, body, signature] = parts[..] else { deny!("The token is not a JWT: it needs three parts separated by dots") };
    let header = object(head, "The token header")?;
    ensure!(header["alg"] == "RS256", "The token is signed with {}; Kern accepts only RS256, the algorithm GitHub signs with", shown(&header["alg"]));
    ensure!(header.get("crit").is_none(), "The token header lists critical extensions (crit) that Kern does not support");
    let Some(kid) = header["kid"].as_str() else { deny!("The token header names no key (kid)") };
    let key = rsa_key(jwks, kid)?;
    rs256(&key, format!("{head}.{body}").as_bytes(), &base64url(signature, "The token signature")?, &shown(&Value::from(kid)))?;
    let claims = object(body, "The token payload")?;
    ensure!(claims["iss"] == ISSUER, "The token was issued by {}, not by GitHub Actions ({ISSUER})", shown(&claims["iss"]));
    let aud = &claims["aud"];
    ensure!(*aud == audience || aud.as_array().is_some_and(|list| list.iter().any(|a| a == audience)), "The token audience {} is not this receipt's seal ({audience})", shown(aud));
    let from = &claims["job_workflow_ref"];
    ensure!(from.as_str().is_some_and(|w| w.starts_with(workflow_prefix)), "The token comes from workflow {}, not from {workflow_prefix}", shown(from));
    if let Some(repo) = repository { ensure!(claims["repository"] == repo, "The token is for repository {}, not {repo}", shown(&claims["repository"])); }
    ensure!(claims["iat"].is_number(), "The token has no issue time (iat) as a number");
    Ok(claims)
}
