use crate::attest;
use crate::capability;
use crate::flow::{self, Source};
use crate::git::{self, Entry, Files};
use crate::github::{self, Api};
use crate::local::{self, Transport, Verifier};
use crate::policy::{Job, contract, operator};
use crate::promote::{self, Claim, Decision};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;

pub struct Hosted { url: String, token: String }
impl Hosted {
    pub fn from_env() -> Option<R<Hosted>> {
        let url = github::env("KERN_VERIFIER_URL")?.trim_end_matches('/').to_string();
        Some((|| {
            ensure!(url.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| url.starts_with(p)), "KERN_VERIFIER_URL must use https");
            let token = github::env("KERN_VERIFIER_TOKEN").ok_or_else(|| Denied("Set KERN_VERIFIER_TOKEN (the verifier-token input) to re-verify pushes".into()))?;
            ensure!(!token.contains(['\r', '\n']), "KERN_VERIFIER_TOKEN must be one bearer token");
            Ok(Hosted { url, token })
        })())
    }
}

pub struct Pr<'a> { pub repository: &'a str, pub number: u64, pub head: &'a str, pub head_ref: &'a str, pub base_ref: &'a str, pub task: &'a str, pub run_id: &'a str, pub body: &'a str }

pub fn merge(from: &Files, onto: &Files, pushed: &Files, base_ref: &str) -> R<Files> {
    let same = |a: Option<&Entry>, b: Option<&Entry>| a.map(|e| (&e.mode, &e.oid)) == b.map(|e| (&e.mode, &e.oid));
    let names: BTreeSet<&String> = from.keys().chain(onto.keys()).chain(pushed.keys()).collect();
    let (mut out, mut both) = (Files::new(), Vec::new());
    for name in names {
        let (f, o, p) = (from.get(name), onto.get(name), pushed.get(name));
        let take = if same(p, f) { o } else if same(o, f) || same(o, p) { p } else { both.push(name.as_str()); continue };
        if let Some(e) = take { out.insert(name.clone(), e.clone()); }
    }
    ensure!(both.is_empty(), "The pushed change and {base_ref} both changed: {}. Merge {base_ref} into the branch, resolve the conflicts and push again", both.join(", "));
    Ok(out)
}

fn fetch(checkout: &Path, server: &str, repository: &str, token: &str, commits: &[&str]) -> R<()> {
    let missing: Vec<&str> = commits.iter().copied().filter(|c| git::git(checkout, &["cat-file", "-e", &format!("{c}^{{commit}}")], None).is_err()).collect();
    if missing.is_empty() { return Ok(()); }
    let url = format!("{server}/{repository}.git");
    let mut args = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head", "--depth=1", url.as_str()];
    args.extend(missing);
    git::remote(checkout, &args, server, token).map(|_| ()).map_err(|e| Denied(format!("Kern could not fetch the base branch to re-verify the push: {}", e.0)))
}

pub fn reverify(pr: &Pr, checkout: &Path, config: &str, now: u64, hosted: &Hosted, stale: Denied) -> R<Value> {
    let api = Api::from_env("re-verify pushes (the workflow needs contents: write, checks: write and pull-requests: write)")?;
    let server = github::server()?;
    let base_ref = pr.base_ref;
    ensure!(github::repository(pr.repository), "Invalid repository");
    ensure!(!base_ref.is_empty() && base_ref.len() <= 255 && !base_ref.contains("..") && base_ref.bytes().all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c)), "Invalid base branch");
    let tip = text(&api.get(&format!("/repos/{}/git/ref/heads/{}", pr.repository, github::ref_path(base_ref)))?["object"], "sha")?.to_string();
    git::oid(&tip)?;
    fetch(checkout, &server, pr.repository, api.token(), &[&tip])?;
    let trusted = |name: &str| -> R<Vec<u8>> { git::git(checkout, &["cat-file", "blob", &format!("{tip}:{config}/{name}")], None).map_err(|_| Denied(format!("The base branch {base_ref} lacks {config}/{name}"))) };
    let policy = contract(&serde_json::from_slice(&trusted(&format!("tasks/{}.json", pr.task))?)?)?;
    let op = operator(&serde_json::from_slice(&trusted("operator.json")?)?)?;
    let key_pem = String::from_utf8(trusted("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))?;
    ensure!(policy.task == pr.task && op.repository == pr.repository, "Trusted configuration does not match this repository and task");
    let key = attest::public_key(&key_pem)?;
    let Some((old, k)) = promote::extract_attestation(pr.body).ok().and_then(|e| attest::signed(&e, &key).ok().map(|k| (e, k))) else { return Err(stale) };
    if k["run_id"] != pr.run_id || k["task"] != pr.task || k["repository"] != pr.repository { return Err(stale); }
    let compare = api.get(&format!("/repos/{}/compare/{tip}...{}?per_page=1", pr.repository, pr.head))?;
    ensure!(matches!(compare["status"].as_str(), Some("ahead" | "diverged")), "The pull request has nothing to merge into {base_ref}: everything on its branch is already there");
    let fork = text(&compare["merge_base_commit"], "sha")?.to_string();
    git::oid(&fork)?;
    fetch(checkout, &server, pr.repository, api.token(), &[&fork])?;
    let l = &policy.limits;
    let read = |c: &str| git::read_commit(checkout, c, l.file_bytes, l.total_bytes);
    let (onto, pushed) = (read(&tip)?, read(pr.head)?);
    let merged = merge(&read(&fork)?, &onto, &pushed, base_ref)?;
    let commit = git::write_commit(checkout, &merged, Some(&tip), &policy.task)?;
    let reused = k["commit"] == commit.as_str() && k["base"] == tip.as_str();
    let (envelope, evidence) = if reused { (old, promote::extract_evidence(pr.body).unwrap_or(Value::Null)) } else { verified(pr.run_id, checkout, &tip, &commit, &policy, &op, &key_pem, hosted)? };
    let evidence = Some(evidence).filter(|e| crate::compare::attested(&envelope, e));
    let at = if reused { now } else { now_ms() + CLOCK_SKEW_MS };
    let k = promote::verify_against(checkout, &envelope, &key, &policy, &op, Claim { run_id: pr.run_id, base: &tip, commit: &commit, now: at, enforce_expiry: false })?;
    let candidate = read(&commit)?;
    let changed = git::changes(&onto, &candidate, &policy)?;
    let capabilities = capability::check(&onto, &candidate, &changed, &op.permissions)?;
    let moved = commit != pr.head;
    if moved {
        let url = format!("{server}/{}.git", pr.repository);
        let lease = format!("--force-with-lease=refs/heads/{}:{}", pr.head_ref, pr.head);
        let target = format!("{commit}:refs/heads/{}", pr.head_ref);
        if let Err(e) = git::remote(checkout, &["push", "--porcelain", "--no-verify", &lease, &url, &target], &server, api.token()) {
            let now_at = api.get(&format!("/repos/{}/git/ref/heads/{}", pr.repository, github::ref_path(pr.head_ref))).ok().and_then(|r| r["object"]["sha"].as_str().map(str::to_string));
            match now_at.as_deref() {
                Some(at) if at == commit => {}
                Some(at) if at != pr.head => return Ok(json!({ "status": "SUPERSEDED", "head": pr.head, "reason": "The branch changed while Kern was re-verifying it; the newer push is checked on its own" })),
                _ => deny!("Kern re-verified the change but could not move the branch {} to it: {}. The workflow needs contents: write", pr.head_ref, e.0),
            }
        }
    }
    let d = Decision { repository: pr.repository.into(), branch: pr.head_ref.into(), base: tip.clone(), commit: commit.clone(), run_id: pr.run_id.into(), attestation: Value::Null };
    if !reused { api.patch(&format!("/repos/{}/pulls/{}", pr.repository, pr.number), &json!({ "body": promote::with_attestation(pr.body, pr.task, &d, &envelope, evidence.as_ref())? }))?; }
    if moved {
        let name = github::env("KERN_CHECK_NAME").unwrap_or_else(|| "kern-verified".into());
        ensure!(name.len() <= 100 && !name.chars().any(char::is_control), "KERN_CHECK_NAME must be a short check name");
        let summary = format!("Kern verified the change again after the push of {}, on {base_ref} at {}, and moved the branch to {commit}.", pr.head, &tip[..12]);
        api.post(&format!("/repos/{}/check-runs", pr.repository), &json!({ "name": name, "head_sha": commit, "status": "completed", "conclusion": "success",
            "output": { "title": "Kern re-verified this change", "summary": summary } }))?;
    }
    let mut result = json!({ "status": "PASS", "head": commit, "base": tip, "task": pr.task, "run_id": pr.run_id, "executor": k["executor"]["kind"], "verified_at": iso8601(k["issued_at"].as_u64().unwrap_or(0)),
        "changes": changed, "capabilities": capabilities, "reverified": { "pushed": pr.head, "commit": commit, "base": tip, "base_ref": base_ref, "attestation_sha256": digest(&envelope)? } });
    if let Some(e) = &evidence { result["what_changed"] = json!(crate::compare::markdown(e)); }
    Ok(result)
}

pub fn check_merge_group(event: &Value, config: &str, now: u64) -> R<Value> {
    let g = &event["merge_group"];
    let (head, base) = (text(g, "head_sha")?, text(g, "base_sha")?);
    git::oid(head)?;
    git::oid(base)?;
    let repository = text(&event["repository"], "full_name")?;
    ensure!(github::repository(repository) && !config.contains("..") && !config.starts_with('/'), "Invalid repository or configuration path");
    let base_ref = text(g, "base_ref")?.strip_prefix("refs/heads/").ok_or_else(|| Denied("The merge group names no base branch".into()))?;
    let group_ref = text(g, "head_ref")?;
    let number: u64 = group_ref.rsplit('/').next().and_then(|leaf| leaf.strip_prefix("pr-")).and_then(|r| r.split('-').next()).and_then(|n| n.parse().ok())
        .ok_or_else(|| Denied(format!("The merge group {group_ref} names no pull request")))?;
    let api = Api::from_env("check merge-queue groups (the workflow needs pull-requests: write)")?;
    let pr = api.get(&format!("/repos/{repository}/pulls/{number}"))?;
    let head_ref = text(&pr["head"], "ref")?;
    let Some(rest) = head_ref.strip_prefix("kern/") else { return Ok(json!({ "status": "SKIPPED", "reason": "Not a Kern agent branch; ordinary review rules apply", "head": head })) };
    let (task, run_id) = rest.split_once('/').ok_or_else(|| Denied("Kern branches must be kern/<task>/<run_id>".into()))?;
    ensure!(crate::policy::task_id(task) && is_uuid(run_id), "Invalid Kern branch");
    ensure!(pr["base"]["ref"] == base_ref && pr["head"]["repo"]["full_name"] == repository, "The merge group does not match pull request #{number}");
    let pushed = text(&pr["head"], "sha")?;
    git::oid(pushed)?;
    let body = pr["body"].as_str().unwrap_or("");
    let work = fresh_dir(std::env::temp_dir().join(format!("kern-queue-{}", short_id()?)))?;
    let outcome = (|| -> R<Value> {
        let store = git::init_store(&work.join("objects.git"))?;
        fetch(&store, &github::server()?, repository, api.token(), &[head, base, pushed])?;
        let trusted = |name: &str| -> R<Vec<u8>> { git::git(&store, &["cat-file", "blob", &format!("{base}:{config}/{name}")], None).map_err(|_| Denied(format!("The base branch {base_ref} lacks {config}/{name}"))) };
        let policy = contract(&serde_json::from_slice(&trusted(&format!("tasks/{task}.json"))?)?)?;
        let op = operator(&serde_json::from_slice(&trusted("operator.json")?)?)?;
        let key_pem = String::from_utf8(trusted("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))?;
        ensure!(policy.task == task && op.repository == repository, "Trusted configuration does not match this repository and task");
        let key = attest::public_key(&key_pem)?;
        let tree = |c: &str| -> R<String> { Ok(git::trimmed(git::git(&store, &["rev-parse", &format!("{c}^{{tree}}")], None)?)) };
        let exact = promote::extract_attestation(body)
            .and_then(|e| promote::verify_against(&store, &e, &key, &policy, &op, Claim { run_id, base, commit: pushed, now, enforce_expiry: false }))
            .and_then(|k| { ensure!(tree(head)? == tree(pushed)?, "The merge group holds more than the change Kern verified for pull request #{number}"); Ok(k) });
        let (k, commit, again) = match exact {
            Ok(k) => (k, pushed.to_string(), false),
            Err(stale) => {
                let Some(hosted) = Hosted::from_env() else { return Err(stale) };
                let hosted = hosted?;
                let signed = promote::extract_attestation(body).ok().and_then(|e| attest::signed(&e, &key).ok());
                if !signed.is_some_and(|s| s["run_id"] == run_id && s["task"] == task && s["repository"] == repository) { return Err(stale); }
                let l = &policy.limits;
                let commit = git::write_commit(&store, &git::read_commit(&store, head, l.file_bytes, l.total_bytes)?, Some(base), &policy.task)?;
                let (envelope, evidence) = verified(run_id, &store, base, &commit, &policy, &op, &key_pem, &hosted)?;
                let k = promote::verify_against(&store, &envelope, &key, &policy, &op, Claim { run_id, base, commit: &commit, now: now_ms() + CLOCK_SKEW_MS, enforce_expiry: false })?;
                let d = Decision { repository: repository.into(), branch: head_ref.into(), base: base.into(), commit: commit.clone(), run_id: run_id.into(), attestation: Value::Null };
                let evidence = Some(evidence).filter(|e| crate::compare::attested(&envelope, e));
                api.patch(&format!("/repos/{repository}/pulls/{number}"), &json!({ "body": promote::with_attestation(body, task, &d, &envelope, evidence.as_ref())? }))?;
                (k, commit, true)
            }
        };
        let l = &policy.limits;
        let (original, candidate) = (git::read_commit(&store, base, l.file_bytes, l.total_bytes)?, git::read_commit(&store, &commit, l.file_bytes, l.total_bytes)?);
        let changed = git::changes(&original, &candidate, &policy)?;
        let capabilities = capability::check(&original, &candidate, &changed, &op.permissions)?;
        Ok(json!({ "status": "PASS", "head": head, "base": base, "task": task, "run_id": run_id, "pull_request": number, "verified_commit": commit, "verified_again": again,
            "executor": k["executor"]["kind"], "verified_at": iso8601(k["issued_at"].as_u64().unwrap_or(0)), "changes": changed, "capabilities": capabilities }))
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

#[allow(clippy::too_many_arguments)]
fn verified(run_id: &str, checkout: &Path, tip: &str, commit: &str, policy: &crate::policy::Contract, op: &crate::policy::Operator, key_pem: &str, hosted: &Hosted) -> R<(Value, Value)> {
    let work = fresh_dir(std::env::temp_dir().join(format!("kern-reverify-{}", short_id()?)))?;
    let outcome = (|| -> R<(Value, Value)> {
        let key_file = fresh_dir(work.join("key"))?.join("verifier.pub");
        create_new(&key_file, key_pem.as_bytes(), 0o600)?;
        let job = Job { repo: checkout.to_path_buf(), base: tip.into(), contract: policy.clone(), operator: op.clone(), command: None, verification_key: key_file.clone(), output: work.join("out"), run_id: Some(run_id.into()) };
        let candidate = flow::task(&job, Source::Ingest(checkout, commit))?;
        let blocked = candidate["checks"].as_array().into_iter().flatten().find(|c| c["status"] == "BLOCKED").and_then(|c| c["detail"].as_str());
        ensure!(candidate["status"] == "CANDIDATE", "{}", candidate["reason"].as_str().or(blocked).unwrap_or("Kern could not prepare the change for the verifier"));
        let v = Verifier { transport: Transport::Https(hosted.url.clone(), hosted.token.clone()), public_key_pem: key_pem.into(), public_key_path: key_file, operator: op.clone() };
        let (envelope, evidence) = local::invoke_transport(&v, &PathBuf::from(text(&candidate, "bundle")?).join("handoff"), &work.join("attestation.json"), commit)?;
        local::confirm_attestation(&envelope, key_pem, commit)?;
        Ok((envelope, evidence))
    })();
    let _ = std::fs::remove_dir_all(&work);
    outcome
}

pub fn upgrade(event: &Value, checkout: &Path, config: &str, now: u64) -> Option<R<Value>> {
    let pr = &event["pull_request"];
    let head_ref = pr["head"]["ref"].as_str()?;
    let prefixes = github::env("KERN_UPGRADE_BRANCHES").unwrap_or_else(|| "dependabot/,renovate/".into());
    if !prefixes.split(',').map(str::trim).any(|p| !p.is_empty() && head_ref.starts_with(p)) { return None; }
    let repository = pr["base"]["repo"]["full_name"].as_str()?;
    if pr["head"]["repo"]["full_name"].as_str() != Some(repository) || config.contains("..") || config.starts_with('/') { return None; }
    let (head, base, base_ref) = (pr["head"]["sha"].as_str()?, pr["base"]["sha"].as_str()?, pr["base"]["ref"].as_str().unwrap_or("the base branch"));
    let task = github::env("KERN_UPGRADE_TASK").unwrap_or_else(|| "upgrade".into());
    if !crate::policy::task_id(&task) || git::oid(head).is_err() || git::oid(base).is_err() { return None; }
    git::git(checkout, &["cat-file", "-e", &format!("{base}:{config}/tasks/{task}.json")], None).ok()?;
    let Some(hosted) = Hosted::from_env() else {
        return Some(Ok(json!({ "status": "SKIPPED", "head": head, "reason": "Upgrade pull requests are verified through a hosted verifier, and this run has none: set verifier-url and verifier-token. GitHub gives runs that Dependabot starts no secrets, so those stay unverified; Renovate, or kern ingest, can verify them" })));
    };
    Some((|| -> R<Value> {
        let hosted = hosted?;
        ensure!(github::repository(repository), "Invalid repository");
        ensure!(git::parents(checkout, head)? == vec![base.to_string()], "Kern verifies an upgrade as one commit on {base_ref}: rebase it on {base_ref} (for Dependabot, comment @dependabot rebase)");
        let trusted = |name: &str| -> R<Vec<u8>> { git::git(checkout, &["cat-file", "blob", &format!("{base}:{config}/{name}")], None).map_err(|_| Denied(format!("The base branch lacks {config}/{name}"))) };
        let policy = contract(&serde_json::from_slice(&trusted(&format!("tasks/{task}.json"))?)?)?;
        let op = operator(&serde_json::from_slice(&trusted("operator.json")?)?)?;
        let key_pem = String::from_utf8(trusted("verifier.pub")?).map_err(|_| Denied("Invalid verifier key".into()))?;
        ensure!(policy.task == task && op.repository == repository, "Trusted configuration does not match this repository and task");
        ensure!(policy.approved.iter().any(|p| p == crate::deps::ANY), "The upgrade task must allow any package the project uses to move (dependencies.allow [\"*\"])");
        let key = attest::public_key(&key_pem)?;
        let l = &policy.limits;
        let (original, pushed) = (git::read_commit(checkout, base, l.file_bytes, l.total_bytes)?, git::read_commit(checkout, head, l.file_bytes, l.total_bytes)?);
        let changed = git::changes(&original, &pushed, &policy)?;
        let capabilities = capability::check(&original, &pushed, &changed, &op.permissions)?;
        let commit = git::write_commit(checkout, &pushed, Some(base), &policy.task)?;
        let run_id = uuid4()?;
        let (envelope, evidence) = verified(&run_id, checkout, base, &commit, &policy, &op, &key_pem, &hosted)?;
        let k = promote::verify_against(checkout, &envelope, &key, &policy, &op, Claim { run_id: &run_id, base, commit: &commit, now: now_ms().max(now) + CLOCK_SKEW_MS, enforce_expiry: false })?;
        let tree = |c: &str| -> R<String> { Ok(git::trimmed(git::git(checkout, &["rev-parse", &format!("{c}^{{tree}}")], None)?)) };
        ensure!(tree(&commit)? == tree(head)?, "The upgrade's tree is not the tree Kern verified");
        let mut result = json!({ "status": "PASS", "head": head, "base": base, "task": policy.task, "run_id": run_id, "upgrade": true, "verified_commit": commit,
            "attestation_sha256": digest(&envelope)?, "executor": k["executor"]["kind"], "verified_at": iso8601(k["issued_at"].as_u64().unwrap_or(0)), "changes": changed, "capabilities": capabilities });
        if crate::compare::attested(&envelope, &evidence) { result["what_changed"] = json!(crate::compare::markdown(&evidence)); }
        Ok(result)
    })())
}
