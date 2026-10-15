use crate::attest::{self, Expect};
use crate::capability;
use crate::flow::required_checks;
use crate::git;
use crate::policy::{Contract, Operator, Pin, configure_git, contract, contract_digest, operator, pinned, task_id, verifier_digest};
use crate::util::*;
use crate::{deny, ensure};
use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

pub struct Decision { pub repository: String, pub branch: String, pub base: String, pub commit: String, pub run_id: String, pub attestation: Value }

pub struct Claim<'a> { pub run_id: &'a str, pub base: &'a str, pub commit: &'a str, pub now: u64, pub enforce_expiry: bool }
pub(crate) fn verify_against(store: &Path, envelope: &Value, key: &VerifyingKey, policy: &Contract, op: &Operator, claim: Claim) -> R<Value> {
    let Claim { run_id, base, commit, now, enforce_expiry } = claim;
    let l = &policy.limits;
    let manifest = digest(&git::manifest(store, commit, l.file_bytes, l.total_bytes)?)?;
    let (contract_sha, verifier_sha, deployment) = (contract_digest(policy)?, verifier_digest(op)?, text(&op.value, "deployment")?.to_string());
    let expect = Expect {
        run_id, task: &policy.task, repository: &op.repository, base, commit, manifest_sha256: &manifest, contract_sha256: &contract_sha, verifier_sha256: &verifier_sha,
        verifier_builder: &op.verifier_builder, deployment: &deployment, image: &op.image, executors: &op.executors, executor_builder: op.executor_builder.as_deref(), checks: required_checks(policy),
    };
    let k = attest::verify(envelope, key, &expect, now, enforce_expiry)?;
    ensure!(git::parents(store, commit)? == vec![base.to_string()], "Promotion parent mismatch");
    Ok(k)
}
pub fn authorize(store: &Path, envelope: &Value, key_pem: &str, policy: &Contract, op: &Operator, expected: &Value, now: u64) -> R<Decision> {
    exact(expected, &["run_id", "base", "commit", "branch"], "promotion expectation")?;
    let (run_id, base, commit, branch) = (text(expected, "run_id")?, text(expected, "base")?, text(expected, "commit")?, text(expected, "branch")?);
    git::oid(base)?;
    git::oid(commit)?;
    ensure!(is_uuid(run_id) && branch == format!("kern/{}/{run_id}", policy.task), "Only run-specific kern/<task>/<run_id> branches are permitted");
    let attestation = verify_against(store, envelope, &attest::public_key(key_pem)?, policy, op, Claim { run_id, base, commit, now, enforce_expiry: true })?;
    Ok(Decision { repository: op.repository.clone(), branch: branch.into(), base: base.into(), commit: commit.into(), run_id: run_id.into(), attestation })
}
pub fn consume(directory: &Path, d: &Decision, key_pem: &str) -> R<Value> {
    ensure!(directory.is_absolute(), "Explicit absolute promoter ledger directory required");
    std::fs::create_dir_all(directory)?;
    ensure!(std::fs::symlink_metadata(directory)?.is_dir(), "Ledger must be a real directory, not a symlink");
    let record = json!({ "version": 1, "state": "CONSUMED", "run_id": d.run_id, "key_id": attest::key_id(&attest::public_key(key_pem)?)?, "repository": d.repository, "base": d.base, "commit": d.commit, "branch": d.branch });
    let file = directory.join(format!("{}.json", d.run_id));
    if let Err(e) = create_new(&file, format!("{}\n", canonical(&record)?).as_bytes(), 0o600) {
        if file.exists() { deny!("Authorization already consumed; reconcile manually, never retry publication"); }
        return Err(e);
    }
    #[cfg(unix)]
    std::fs::File::open(directory)?.sync_all()?;
    Ok(record)
}
pub fn promote_local(store: &Path, d: &Decision, key_pem: &str, target: &Path, ledger: Option<&Path>) -> R<Value> {
    ensure!(git::trimmed(git::git(target, &["cat-file", "-t", &d.base], None)?) == "commit", "Target lacks the approved base");
    let metadata = std::path::PathBuf::from(git::trimmed(git::git(target, &["rev-parse", "--absolute-git-dir"], None)?));
    consume(&ledger.map(Path::to_path_buf).unwrap_or_else(|| metadata.join("kern-consumed")), d, key_pem)?;
    let pack = git::git(store, &["pack-objects", "--stdout", "--revs", "-q"], Some(format!("{}\n^{}\n", d.commit, d.base).as_bytes()))?;
    git::git(target, &["index-pack", "--stdin", "--fsck-objects"], Some(&pack))?;
    git::git(target, &["update-ref", &format!("refs/heads/{}", d.branch), &d.commit, &"0".repeat(40)], None)?;
    Ok(json!({ "branch": d.branch, "commit": d.commit, "published": "local-feature-ref" }))
}

pub const MARKER: &str = "<!-- kern-attestation:v1\n";
pub const EVIDENCE: &str = "<!-- kern-evidence:v1\n";
fn evidence_block(evidence: Option<&Value>) -> R<String> {
    let Some(e) = evidence else { return Ok(String::new()) };
    let text = canonical(e)?;
    Ok(if text.len() > 16_000 { String::new() } else { format!("{EVIDENCE}{}\n-->\n", b64(text.as_bytes())) })
}
fn without_block(body: &str, marker: &str) -> String {
    let Some(start) = body.find(marker) else { return body.to_string() };
    let Some(end) = body[start..].find("\n-->").map(|e| start + e + 4) else { return body.to_string() };
    format!("{}{}", &body[..start], body[end..].strip_prefix('\n').unwrap_or(&body[end..]))
}
pub fn pr_body(task: &str, d: &Decision, envelope: &Value, evidence: Option<&Value>) -> R<String> {
    let what = evidence.map(|e| format!("### What changed\n\n{}\n", crate::compare::markdown(e))).unwrap_or_default();
    Ok(format!("Kern-verified agent change for task `{task}`.\n\nCommit: `{}`\nAttestation SHA-256: `{}`\n\n{what}Review before merging. The `kern-verified` check re-verifies this attestation against the PR head.\n\n{MARKER}{}\n-->\n{}",
        d.commit, digest(envelope)?, b64(canonical(envelope)?.as_bytes()), evidence_block(evidence)?))
}
pub fn with_attestation(body: &str, task: &str, d: &Decision, envelope: &Value, evidence: Option<&Value>) -> R<String> {
    let body = body.replace("\r\n", "\n");
    if body.starts_with("Kern-verified agent change for task") { return pr_body(task, d, envelope, evidence); }
    let body = without_block(&body, EVIDENCE);
    let start = body.find(MARKER).ok_or_else(|| Denied("The pull request description lost its Kern attestation block".into()))?;
    let end = body[start..].find("\n-->").map(|e| start + e + 4).ok_or_else(|| Denied("Unterminated Kern attestation block".into()))?;
    let rest = body[end..].strip_prefix('\n').unwrap_or(&body[end..]);
    Ok(format!("{}{MARKER}{}\n-->\n{}{rest}", &body[..start], b64(canonical(envelope)?.as_bytes()), evidence_block(evidence)?))
}
pub fn extract_evidence(body: &str) -> Option<Value> {
    let body = body.replace("\r\n", "\n");
    if body.len() > 65536 || body.matches(EVIDENCE).count() != 1 { return None; }
    let rest = &body[body.find(EVIDENCE)? + EVIDENCE.len()..];
    serde_json::from_slice(&unb64(rest[..rest.find("\n-->")?].trim()).ok()?).ok()
}
pub fn extract_attestation(body: &str) -> R<Value> {
    let body = body.replace("\r\n", "\n");
    ensure!(body.len() <= 65536 && body.matches(MARKER).count() == 1, "Pull request must carry exactly one Kern attestation block");
    let rest = &body[body.find(MARKER).unwrap_or(0) + MARKER.len()..];
    let end = rest.find("\n-->").ok_or_else(|| Denied("Unterminated Kern attestation block".into()))?;
    Ok(serde_json::from_slice(&unb64(rest[..end].trim())?)?)
}
pub struct GitHub<'a> { pub token: &'a str, pub base_branch: &'a str, pub ledger: &'a Path, pub git: Pin, pub evidence: Option<&'a Value> }
pub fn promote_github(store: &Path, d: &Decision, policy: &Contract, envelope: &Value, key_pem: &str, gh: GitHub) -> R<Value> {
    ensure!(!gh.token.is_empty() && !gh.token.contains(['\r', '\n']), "An explicit publishing token is required");
    let b = gh.base_branch;
    ensure!(!b.is_empty() && b.len() <= 128 && !b.contains("..") && !b.contains("//") && !b.ends_with('/') && b.bytes().all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c)), "Invalid PR base branch");
    let git_exe = pinned(&gh.git)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into();
    let root = format!("https://api.github.com/repos/{}", d.repository);
    let api = |method: &str, suffix: &str, body: Option<Value>| -> R<Value> {
        let url = format!("{root}{suffix}");
        let auth = format!("Bearer {}", gh.token);
        let response = match body {
            Some(b) => agent.post(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(b.to_string()),
            None => agent.get(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").call(),
        };
        let mut r = response.map_err(|e| Denied(format!("GitHub request failed: {method} {suffix}: {e}")))?;
        ensure!(r.status().is_success(), "GitHub request failed: {method} {suffix} ({})", r.status().as_u16());
        Ok(serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?)
    };
    let refpath = |branch: &str| format!("/git/ref/heads/{}", branch.split('/').map(|s| s.replace('%', "%25")).collect::<Vec<_>>().join("/"));
    ensure!(api("GET", &refpath(b), None)?["object"]["sha"] == d.base.as_str(), "PR base moved; re-run against the new base");
    consume(gh.ledger, d, key_pem)?;
    let header = format!("http.https://github.com/.extraHeader=Authorization: Basic {}", b64(format!("x-access-token:{}", gh.token).as_bytes()));
    let mut env: Vec<(String, String)> = safe_env(store).into_iter().filter(|(k, _)| k != "PATH").collect();
    env.extend([("PATH".into(), String::new()), ("GIT_CONFIG_COUNT".into(), "1".into()), ("GIT_CONFIG_KEY_0".into(), header.split_once('=').map(|h| h.0.to_string()).unwrap_or_default()), ("GIT_CONFIG_VALUE_0".into(), header.split_once('=').map(|h| h.1.to_string()).unwrap_or_default())]);
    let reference = format!("refs/heads/{}", d.branch);
    let args: Vec<String> = ["-c", "core.longpaths=true", "-c", "core.hooksPath=/dev/null", "-c", "credential.helper=", "-c", "http.followRedirects=false", "-c", "protocol.allow=never", "-c", "protocol.https.allow=always", "-C"].iter().map(|s| s.to_string())
        .chain([store.display().to_string(), "push".into(), "--no-verify".into(), "--porcelain".into(), format!("--force-with-lease={reference}:"), format!("https://github.com/{}.git", d.repository), format!("{}:{reference}", d.commit)]).collect();
    run(&git_exe, &args, Run { cwd: Some(store), env, input: None, timeout: Duration::from_secs(120), max_output: 1048576 }).map_err(|_| Denied("Exact create-only push failed; no existing branch was overwritten".into()))?;
    ensure!(api("GET", &refpath(&d.branch), None)?["object"]["sha"] == d.commit.as_str(), "Remote branch does not match the verified artifact");
    ensure!(api("GET", &refpath(b), None)?["object"]["sha"] == d.base.as_str(), "PR base moved after push; reconcile the created branch manually");
    let pr = api("POST", "/pulls", Some(json!({ "title": format!("Kern: {}", policy.task), "head": d.branch, "base": b, "draft": true, "body": pr_body(&policy.task, d, envelope, gh.evidence)? })))?;
    ensure!(pr["head"]["sha"] == d.commit.as_str() && pr["base"]["sha"] == d.base.as_str() && pr["draft"] == true, "Created PR does not match the verified head/base or is not a draft; reconcile manually");
    Ok(json!({ "url": pr["html_url"], "number": pr["number"], "commit": d.commit, "branch": d.branch, "draft": true }))
}
pub fn git_pin(file: &Path) -> R<Pin> {
    let p = crate::policy::pin(&read_json(file)?, "Git pin")?;
    configure_git(p.clone())?;
    Ok(p)
}

pub fn step_summary(result: &R<Value>) -> String {
    let (status, detail) = match result { Ok(r) => (r["status"].as_str().unwrap_or("UNKNOWN").to_string(), serde_json::to_string_pretty(r).unwrap_or_default()), Err(e) => ("DENIED".into(), e.0.clone()) };
    let status: String = status.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let mut sections = String::new();
    if let Ok(r) = result {
        if let Some(what) = r["what_changed"].as_str() { sections += &format!("#### What changed\n\n{what}\n"); }
        if let Some(list) = r["removed_code"].as_array().filter(|l| !l.is_empty()) { sections += &format!("#### Who added the code this change removes\n\n{}\n", crate::github::who_markdown(list)); }
    }
    let heading = if sections.is_empty() { "" } else { "#### Details\n\n" };
    format!("### kern-verified: {status}\n\n{sections}{heading}{}\n", detail.lines().map(|l| format!("    {l}\n")).collect::<String>())
}
pub fn check_pr(event: &Value, checkout: &Path, config: &str, now: u64) -> R<Value> {
    if event["merge_group"].is_object() { return crate::reverify::check_merge_group(event, config, now); }
    let pr = &event["pull_request"];
    ensure!(pr.is_object(), "Not a pull_request event");
    let (head, base, head_ref) = (text(&pr["head"], "sha")?, text(&pr["base"], "sha")?, text(&pr["head"], "ref")?);
    let repository = text(&pr["base"]["repo"], "full_name")?;
    let Some(rest) = head_ref.strip_prefix("kern/") else {
        return crate::reverify::upgrade(event, checkout, config, now).unwrap_or_else(|| Ok(json!({ "status": "SKIPPED", "reason": "Not a Kern agent branch; ordinary review rules apply", "head": head })));
    };
    let (task, run_id) = rest.split_once('/').ok_or_else(|| Denied("Kern branches must be kern/<task>/<run_id>".into()))?;
    ensure!(task_id(task) && is_uuid(run_id) && !config.contains("..") && !config.starts_with('/'), "Invalid Kern branch or configuration path");
    git::oid(head)?;
    git::oid(base)?;
    let body = pr["body"].as_str().unwrap_or("");
    let stale = match gate(checkout, config, now, repository, task, run_id, head, base, body) { Ok(v) => return Ok(v), Err(e) => e };
    let same_repository = pr["head"]["repo"]["full_name"].as_str() == Some(repository);
    match crate::reverify::Hosted::from_env() {
        Some(hosted) if same_repository => {
            let number = pr["number"].as_u64().ok_or_else(|| Denied("The event names no pull request number".into()))?;
            let pull = crate::reverify::Pr { repository, number, head, head_ref, base_ref: text(&pr["base"], "ref")?, task, run_id, body };
            crate::reverify::reverify(&pull, checkout, config, now, &hosted?, stale)
        }
        _ => Err(stale),
    }
}
#[allow(clippy::too_many_arguments)]
fn gate(checkout: &Path, config: &str, now: u64, repository: &str, task: &str, run_id: &str, head: &str, base: &str, body: &str) -> R<Value> {
    let envelope = extract_attestation(body)?;
    if let Some(claim) = attest::claimed(&envelope) {
        ensure!(claim["commit"] == head, "Attestation mismatch: commit");
        ensure!(claim["base"] == base, "Attestation mismatch: base");
    }
    let trusted = |name: &str| -> R<Vec<u8>> { git::git(checkout, &["cat-file", "blob", &format!("{base}:{config}/{name}")], None).map_err(|_| Denied(format!("Trusted base commit lacks {config}/{name}"))) };
    let policy = contract(&serde_json::from_slice(&trusted(&format!("tasks/{task}.json"))?)?)?;
    let op = operator(&serde_json::from_slice(&trusted("operator.json")?)?)?;
    let key_pem = String::from_utf8(trusted("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))?;
    ensure!(policy.task == task && op.repository == repository, "Trusted configuration does not match this repository and task");
    let k = verify_against(checkout, &envelope, &attest::public_key(&key_pem)?, &policy, &op, Claim { run_id, base, commit: head, now, enforce_expiry: false })?;
    let l = &policy.limits;
    let (original, candidate) = (git::read_commit(checkout, base, l.file_bytes, l.total_bytes)?, git::read_commit(checkout, head, l.file_bytes, l.total_bytes)?);
    let changed = git::changes(&original, &candidate, &policy)?;
    let capabilities = capability::check(&original, &candidate, &changed, &op.permissions)?;
    let mut result = json!({ "status": "PASS", "head": head, "base": base, "task": task, "run_id": run_id, "executor": k["executor"]["kind"], "verified_at": iso8601(k["issued_at"].as_u64().unwrap_or(0)), "changes": changed, "capabilities": capabilities });
    if let Some(e) = extract_evidence(body).filter(|e| crate::compare::attested(&envelope, e)) { result["what_changed"] = json!(crate::compare::markdown(&e)); }
    if policy.compare {
        let removed = crate::compare::removed_code(&original, &candidate, &result["changes"], &policy);
        if !removed.is_empty() { result["removed_code"] = crate::github::who_added(repository, base, &removed); }
    }
    Ok(result)
}

pub struct Merged<'a> { pub repository: &'a str, pub branch: &'a str, pub head: &'a str, pub config: &'a str, pub auditor_key: Option<&'a str>, pub now: u64 }
pub fn verify_merged(checkout: &Path, envelope: &Value, m: &Merged) -> R<Value> {
    let stmt: Value = serde_json::from_slice(&unb64(text(envelope, "payload")?)?)?;
    let k = &stmt["predicate"][attest::EXTENSION];
    let (task, run_id, base, commit) = (text(k, "task")?.to_string(), text(k, "run_id")?.to_string(), text(k, "base")?.to_string(), text(k, "commit")?.to_string());
    ensure!(task_id(&task) && is_uuid(&run_id) && m.branch == format!("kern/{task}/{run_id}"), "The pull request branch {} does not match its proof", m.branch);
    ensure!(commit == m.head, "The proof is for commit {commit}, not the pull request head {}", m.head);
    git::oid(&base)?;
    let trusted = |name: &str| -> R<Vec<u8>> { git::git(checkout, &["cat-file", "blob", &format!("{base}:{}/{name}", m.config)], None).map_err(|_| Denied(format!("The base commit {base} lacks {}/{name}", m.config))) };
    let policy = contract(&serde_json::from_slice(&trusted(&format!("tasks/{task}.json"))?)?)?;
    let op = operator(&serde_json::from_slice(&trusted("operator.json")?)?)?;
    ensure!(policy.task == task && op.repository == m.repository, "The trusted configuration at {base} does not match this repository and task");
    let key_pem = match m.auditor_key { Some(k) => k.to_string(), None => String::from_utf8(trusted("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))? };
    let key = attest::public_key(&key_pem)?;
    let verified = verify_against(checkout, envelope, &key, &policy, &op, Claim { run_id: &run_id, base: &base, commit: &commit, now: m.now, enforce_expiry: false })?;
    let l = &policy.limits;
    let (original, candidate) = (git::read_commit(checkout, &base, l.file_bytes, l.total_bytes)?, git::read_commit(checkout, &commit, l.file_bytes, l.total_bytes)?);
    let changed = git::changes(&original, &candidate, &policy)?;
    capability::check(&original, &candidate, &changed, &op.permissions)?;
    let checks: Vec<Value> = verified["checks"].as_array().into_iter().flatten().map(|c| c["id"].clone()).collect();
    Ok(json!({ "task": task, "run_id": run_id, "base": base, "commit": commit, "verified_at": iso8601(verified["issued_at"].as_u64().unwrap_or(0)),
        "executor": verified["executor"]["kind"], "checks": checks, "key_id": attest::key_id(&key)? }))
}

pub fn gate_artifact(checkout: &Path, commit: &str, envelope: &Value, config: &Path, now: u64) -> R<Value> {
    git::oid(commit)?;
    let read = |name: &str| -> R<Vec<u8>> { std::fs::read(config.join(name)).map_err(|_| Denied(format!("Trusted deploy config lacks {name}"))) };
    let payload = unb64(text(envelope, "payload")?)?;
    let stmt: Value = serde_json::from_slice(&payload)?;
    let k = &stmt["predicate"][attest::EXTENSION];
    let (task, run_id, base) = (text(k, "task")?.to_string(), text(k, "run_id")?.to_string(), text(k, "base")?.to_string());
    ensure!(task_id(&task), "Invalid task in attestation");
    git::oid(&base)?;
    let op = operator(&serde_json::from_slice(&read("operator.json")?)?)?;
    let key_pem = String::from_utf8(read("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))?;
    let policy = contract(&serde_json::from_slice(&read(&format!("tasks/{task}.json"))?)?)?;
    ensure!(policy.task == task, "Trusted contract task does not match the attestation");
    let k = verify_against(checkout, envelope, &attest::public_key(&key_pem)?, &policy, &op, Claim { run_id: &run_id, base: &base, commit, now, enforce_expiry: false })?;
    let l = &policy.limits;
    let (original, candidate) = (git::read_commit(checkout, &base, l.file_bytes, l.total_bytes)?, git::read_commit(checkout, commit, l.file_bytes, l.total_bytes)?);
    let changed = git::changes(&original, &candidate, &policy)?;
    let capabilities = capability::check(&original, &candidate, &changed, &op.permissions)?;
    Ok(json!({ "status": "ALLOW", "commit": commit, "base": base, "task": task, "repository": op.repository, "executor": k["executor"]["kind"], "verified_at": iso8601(k["issued_at"].as_u64().unwrap_or(0)), "changes": changed, "capabilities": capabilities }))
}
