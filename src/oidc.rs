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
pub const WORKFLOW: &str = "jastijayakrishna/Kerner/.github/workflows/kern.yml@";
const SHA256_INFO: [u8; 19] = [0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20];

pub fn audience(seal: &Value) -> R<String> { Ok(format!("kern-receipt:{}", digest(seal)?)) }

fn query_value(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { out.push(char::from(b)); } else { out.extend(['%', char::from(HEX[usize::from(b >> 4)]), char::from(HEX[usize::from(b & 15)])]); }
    }
    out
}
fn agent() -> ureq::Agent { ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into() }

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

pub fn jwks() -> R<Value> {
    let mut r = agent().get(&format!("{ISSUER}/.well-known/jwks")).header("Accept", "application/json").call().map_err(|e| Denied(format!("GitHub's OIDC keys are unreachable: {e}")))?;
    ensure!(r.status().is_success(), "GitHub answered {} for its OIDC keys", r.status().as_u16());
    let keys: Value = serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?;
    ensure!(keys["keys"].is_array(), "GitHub's OIDC keys are not a key set");
    Ok(keys)
}

fn shown(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 200 { s } else { format!("{}...", s.chars().take(200).collect::<String>()) }
}
fn base64url(s: &str, what: &str) -> R<Vec<u8>> { URL_SAFE_NO_PAD.decode(s).map_err(|_| Denied(format!("{what} is not base64url"))) }
fn object(s: &str, what: &str) -> R<Value> {
    let v: Value = serde_json::from_slice(&base64url(s, what)?).map_err(|_| Denied(format!("{what} is not JSON")))?;
    ensure!(v.is_object(), "{what} is not a JSON object");
    Ok(v)
}

struct Rsa { n: BigUint, e: BigUint, k: usize }
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
    ensure!(e.bit(0) && e.bits() > 1 && e < n, "Key {name} has an unusable public exponent");
    Ok(Rsa { k: bits.div_ceil(8) as usize, n, e })
}
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
