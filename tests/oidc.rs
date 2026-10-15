// Keyless proofs: inside GitHub Actions, GitHub signs an OIDC token whose audience names a Kern seal, and anyone can check it later against
// GitHub's published keys. The fixtures are a test key's public JWKS and finished tokens; fixtures/oidc/README.md says how they were made.
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kern::oidc::{self, ISSUER, WORKFLOW};
use kern::util::*;
use serde_json::{Value, json};

fn keys() -> Value { serde_json::from_str(include_str!("fixtures/oidc/jwks.json")).unwrap() }
fn token(name: &str) -> String {
    let all: Value = serde_json::from_str(include_str!("fixtures/oidc/tokens.json")).unwrap();
    all[name].as_str().unwrap_or_else(|| panic!("no fixture token {name}")).to_string()
}
fn seal() -> Value { json!({ "v": 1, "head": "abc" }) }
fn aud() -> String { oidc::audience(&seal()).unwrap() }
/// What a receipt check asks: this seal, Kern's workflow, the customer's repository.
fn check(t: &str) -> R<Value> { oidc::verify(t, &keys(), &aud(), WORKFLOW, Some("acme/shop")) }
fn denied<T>(r: R<T>, needle: &str) {
    match r { Ok(_) => panic!("expected denial containing {needle:?}"), Err(e) => assert!(e.0.contains(needle), "{:?} does not contain {needle:?}", e.0) }
}
fn parts(t: &str) -> (String, String, String) {
    let p: Vec<&str> = t.split('.').collect();
    (p[0].to_string(), p[1].to_string(), p[2].to_string())
}

#[test]
fn audience_binds_a_token_to_one_seal() {
    let a = aud();
    assert_eq!(a, format!("kern-receipt:{}", sha256(r#"{"head":"abc","v":1}"#)), "sha256 of the seal's canonical JSON");
    assert_eq!(a, "kern-receipt:84eb1a5ec7e3c7852cf993a24e5526aea2c4dc7d7496933f74e3a0cfb7b407d4", "stable: the fixture tokens were made for it");
    assert_eq!(a, oidc::audience(&json!({ "head": "abc", "v": 1 })).unwrap(), "the same seal written in another order");
    assert_ne!(a, oidc::audience(&json!({ "v": 1, "head": "abd" })).unwrap());
    denied(oidc::audience(&json!({ "v": 1.5 })), "integers");
}

#[test]
fn a_token_github_signed_for_this_seal_verifies_and_returns_its_claims() {
    let claims = check(&token("valid")).unwrap();
    assert_eq!((claims["iss"].as_str(), claims["aud"].as_str(), claims["repository"].as_str()), (Some(ISSUER), Some(aud().as_str()), Some("acme/shop")));
    assert_eq!(claims["job_workflow_ref"], format!("{WORKFLOW}refs/heads/one"));
    assert_eq!((claims["sha"].as_str(), claims["run_id"].as_str()), (Some("9f1c2e3d4b5a69788796a5b4c3d2e1f0a9b8c7d6"), Some("11850203412")));
    assert!(claims["exp"].as_u64().unwrap() * 1000 < now_ms(), "the token expired long ago: receipts are checked long after the job, so exp is not enforced");
    // the repository is checked only when the caller names one, and a longer prefix pins the workflow's ref
    assert_eq!(oidc::verify(&token("valid"), &keys(), &aud(), &format!("{WORKFLOW}refs/heads/one"), None).unwrap(), claims);
    // aud may be a list that contains the seal's audience
    assert_eq!(check(&token("audience-list")).unwrap()["aud"], json!(["https://github.com/acme", aud()]));
}

#[test]
fn a_token_for_another_seal_workflow_issuer_or_repository_is_refused() {
    denied(check(&token("wrong-audience")), "audience");
    denied(oidc::verify(&token("audience-list"), &keys(), &oidc::audience(&json!({ "v": 1, "head": "abd" })).unwrap(), WORKFLOW, None), "audience");
    denied(check(&token("wrong-workflow")), "workflow");
    denied(oidc::verify(&token("valid"), &keys(), &aud(), &format!("{WORKFLOW}refs/tags/"), None), "workflow");
    denied(oidc::verify(&token("valid"), &keys(), &aud(), "", None), "workflow");
    denied(oidc::verify(&token("valid"), &keys(), &aud(), WORKFLOW, Some("acme/other")), "repository");
    denied(check(&token("wrong-issuer")), "issued by");
    denied(check(&token("iat-text")), "iat");
}

#[test]
fn a_token_signed_by_another_key_or_algorithm_is_refused() {
    denied(check(&token("unknown-kid")), "not among the published keys");
    denied(check(&token("hs256")), "accepts only RS256");
    denied(check(&token("other-key")), "signature does not match");
    let (head, body, sig) = parts(&token("valid"));
    // a header that asks for extensions Kern does not know, or is not JSON at all
    let crit = URL_SAFE_NO_PAD.encode(json!({ "alg": "RS256", "kid": "test-key-1", "typ": "JWT", "crit": ["exp"] }).to_string());
    denied(check(&format!("{crit}.{body}.{sig}")), "crit");
    denied(check(&format!("{}.{body}.{sig}", URL_SAFE_NO_PAD.encode("not json"))), "header");
    // a "signature" that is the modulus itself, or larger
    let n = URL_SAFE_NO_PAD.decode(keys()["keys"][0]["n"].as_str().unwrap()).unwrap();
    denied(check(&format!("{head}.{body}.{}", URL_SAFE_NO_PAD.encode(&n))), "smaller than");
    denied(check(&format!("{head}.{body}.{}", URL_SAFE_NO_PAD.encode([0xff; 256]))), "smaller than");
}

/// The test key's own raw signatures over blocks a parsing verifier would take: the hash followed by garbage (Bleichenbacher 2006), and a
/// DigestInfo without its NULL. Comparing s^e mod n with the one rebuilt block refuses both.
#[test]
fn a_signature_block_only_a_lenient_parser_accepts_is_refused() {
    denied(check(&token("garbage-after-hash")), "signature does not match");
    denied(check(&token("digest-info-without-null")), "signature does not match");
}

#[test]
fn only_rsa_keys_of_2048_bits_or_more_with_a_sound_exponent_are_used() {
    let valid = token("valid");
    let with = |change: &dyn Fn(&mut Value)| {
        let mut set = keys();
        change(&mut set["keys"][0]);
        oidc::verify(&valid, &set, &aud(), WORKFLOW, None)
    };
    let n = URL_SAFE_NO_PAD.decode(keys()["keys"][0]["n"].as_str().unwrap()).unwrap();
    denied(with(&|k| k["n"] = json!(URL_SAFE_NO_PAD.encode(&n[..128]))), "1024 bits");
    denied(with(&|k| k["kty"] = json!("EC")), "not an RSA key");
    denied(with(&|k| k["e"] = json!("AQ")), "exponent");
    denied(with(&|k| k["alg"] = json!("RS512")), "RS256");
    denied(with(&|k| k["n"] = json!("not base64url!")), "base64url");
    let key = keys()["keys"][0].clone();
    denied(oidc::verify(&valid, &json!({ "keys": [key, key] }), &aud(), WORKFLOW, None), "more than once");
    denied(oidc::verify(&valid, &json!({}), &aud(), WORKFLOW, None), "keys");
}

#[test]
fn a_changed_or_truncated_token_is_refused() {
    let t = token("valid");
    let (head, body, sig) = parts(&t);
    // one character of the payload changed
    let mid = body.len() / 2;
    let changed = format!("{}{}{}", &body[..mid], if &body[mid..=mid] == "A" { "B" } else { "A" }, &body[mid + 1..]);
    assert_ne!(changed, body);
    denied(check(&format!("{head}.{changed}.{sig}")), "signature");
    // cut short inside the signature, before it, and to nothing
    denied(check(&t[..t.len() - 1]), "signature");
    denied(check(&t[..t.len() - 40]), "signature");
    denied(check(&format!("{head}.{body}.")), "signature");
    denied(check(&format!("{head}.{body}")), "three parts");
    denied(check(""), "three parts");
}

/// The only test that touches the environment, so nothing races these variables.
#[test]
fn request_asks_github_for_a_token_bound_to_the_seal_and_needs_id_token_write() {
    let (url, bearer) = ("ACTIONS_ID_TOKEN_REQUEST_URL", "ACTIONS_ID_TOKEN_REQUEST_TOKEN");
    unsafe { std::env::remove_var(url); std::env::remove_var(bearer); }
    denied(oidc::request(&aud()), "permissions: id-token: write");
    unsafe { std::env::set_var(url, "https://example.invalid/token"); }
    denied(oidc::request(&aud()), "permissions: id-token: write");
    unsafe { std::env::set_var(url, "http://example.invalid/token"); std::env::set_var(bearer, "request-token"); }
    denied(oidc::request(&aud()), "https");
    // a stand-in for GitHub's token service: it records each request and answers as GitHub does
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let root = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let mut heads = Vec::new();
        for _ in 0..2 {
            let (mut s, _) = listener.accept().unwrap();
            let (mut head, mut byte) = (Vec::new(), [0u8; 1]);
            while !head.ends_with(b"\r\n\r\n") && std::io::Read::read(&mut s, &mut byte).unwrap() == 1 { head.push(byte[0]); }
            let body = r#"{"count":1,"value":"header.payload.signature"}"#;
            std::io::Write::write_all(&mut s, format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
            heads.push(String::from_utf8(head).unwrap());
        }
        heads
    });
    unsafe { std::env::set_var(url, format!("{root}/token?api-version=2.0")); }
    assert_eq!(oidc::request(&aud()).unwrap(), "header.payload.signature");
    unsafe { std::env::set_var(url, format!("{root}/token")); }
    assert_eq!(oidc::request("a b/c").unwrap(), "header.payload.signature");
    let heads = server.join().unwrap();
    let hash = &aud()["kern-receipt:".len()..];
    assert!(heads[0].starts_with(&format!("GET /token?api-version=2.0&audience=kern-receipt%3A{hash} HTTP/1.1\r\n")), "{}", heads[0]);
    assert!(heads[1].starts_with("GET /token?audience=a%20b%2Fc HTTP/1.1\r\n"), "{}", heads[1]);
    assert!(heads.iter().all(|h| h.to_ascii_lowercase().contains("\r\nauthorization: bearer request-token\r\n")), "{heads:?}");
    unsafe { std::env::remove_var(url); std::env::remove_var(bearer); }
}

/// Run with --ignored: GitHub's real keys pass every key check, so a token naming one of them reaches the signature check.
#[test]
#[ignore = "asks GitHub over the network"]
fn githubs_published_keys_are_keys_kern_can_use() {
    let live = oidc::jwks().unwrap();
    let list = live["keys"].as_array().unwrap();
    assert!(!list.is_empty());
    let (_, body, sig) = parts(&token("valid"));
    for k in list {
        let head = URL_SAFE_NO_PAD.encode(json!({ "alg": "RS256", "kid": k["kid"], "typ": "JWT" }).to_string());
        denied(oidc::verify(&format!("{head}.{body}.{sig}"), &live, &aud(), WORKFLOW, None), "signature does not match");
    }
}
