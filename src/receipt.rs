use crate::github::Api;
use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};
use std::path::Path;

pub const MARK: &str = "<!-- kern-receipt -->";
pub const SEAL_MARK: &str = "<!-- kern-seal ";
fn author() -> String { crate::github::env("KERN_COMMENT_AUTHOR").unwrap_or_else(|| "github-actions[bot]".into()) }
const SHOWN: usize = 15;

fn code(s: &str, n: usize) -> String {
    let flat: String = s.chars().map(|c| match c { '`' => '\'', '<' => '‹', c if c.is_control() => ' ', c => c }).collect();
    let flat = flat.trim();
    let clipped: String = if flat.chars().count() > n { format!("{}…", flat.chars().take(n).collect::<String>()) } else { flat.to_string() };
    format!("`{clipped}`")
}
fn strings(v: &Value) -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() }
fn item(kind: &str, what: String) -> Value { json!({ "kind": kind, "what": what }) }

fn behaviour_items(report: &Value) -> (Vec<String>, Vec<String>) {
    if !report.is_object() { return (Vec::new(), Vec::new()); }
    let items = crate::drive::difference_items(report);
    let has_record = items.iter().any(|(_, rec)| *rec);
    let sorted = &report["sorted"];
    let (mut described, mut surprises) = (Vec::new(), Vec::new());
    for (i, (line, rec)) in items.iter().enumerate() {
        if has_record && !*rec { continue; }
        let says = sorted[format!("D{}", i + 1)]["says"].as_str();
        if says == Some("yes") { described.push(line.clone()) } else { surprises.push(line.clone()) }
    }
    (described, surprises)
}

pub fn build(d: &Value, rules: &Value, tests: &Value, behaviour: &Value) -> Value {
    let (mut blocked, mut look) = (Vec::new(), Vec::new());
    for t in strings(&tests["broke"]) { blocked.push(item("test-broke", format!("A test that passed before now fails: {}", code(&t, 200)))); }
    for t in strings(&tests["new_problems"]) { blocked.push(item("test-file", format!("A test file no longer runs, or a hook now fails: {}", code(&t, 200)))); }
    for x in rules["tricks"].as_array().into_iter().flatten() { blocked.push(item("trick", format!("{}: {}", code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""))))); }
    let (described, surprises) = behaviour_items(&behaviour["report"]);
    let sorted = behaviour["report"]["sorted"].is_object();
    for s in &surprises { look.push(item(if sorted { "not-mentioned" } else { "difference" }, if sorted { format!("Not mentioned by the pull request: {s}") } else { format!("Behaves differently: {s}") })); }
    for p in rules["powers"].as_array().into_iter().flatten() {
        let caps: Vec<String> = strings(&p["added"]).iter().map(|c| code(c, 80)).collect();
        look.push(item("power", format!("{} can now do something it could not before: {}", code(p["path"].as_str().unwrap_or(""), 160), caps.join(", "))));
    }
    for t in strings(&tests["gone"]) { look.push(item("test-gone", format!("A test that ran before no longer runs: {}", code(&t, 200)))); }
    for t in strings(&tests["newly_skipped"]) { look.push(item("test-skipped", format!("A test that passed before is now skipped: {}", code(&t, 200)))); }
    let path_of = |x: &Value| code(x["path"].as_str().unwrap_or(""), 160);
    for (key, kind, says) in [("settings", "settings", "changes the test runner's settings, which decide which tests run"), ("protected", "protected", "is a CI, secret or configuration file")] {
        for x in rules[key].as_array().into_iter().flatten() { look.push(item(kind, format!("{} {says}", path_of(x)))); }
    }
    let deps: Vec<&Value> = rules["dependencies"].as_array().into_iter().flatten().collect();
    if !deps.is_empty() {
        let moved: Vec<String> = deps.iter().flat_map(|d| strings(&d["packages"])).collect();
        let mut what = String::from("The project's dependencies change");
        if !moved.is_empty() {
            what += &format!(": {}", moved.iter().take(SHOWN).map(|m| code(m, 120)).collect::<Vec<_>>().join(", "));
            if moved.len() > SHOWN { what += &format!(" and {} more", moved.len() - SHOWN); }
        }
        look.push(item("dependencies", format!("{what} ({})", deps.iter().map(|d| path_of(d)).collect::<Vec<_>>().join(", "))));
    }
    for x in rules["lockfile"].as_array().into_iter().flatten() { look.push(item("lockfile", format!("{}: {}", path_of(x), plain(x["why"].as_str().unwrap_or(""))))); }
    for x in rules["agent_notes"].as_array().into_iter().flatten() { look.push(item("agent-notes", format!("{} changes what coding agents are told", path_of(x)))); }
    let decision = if !blocked.is_empty() { "blocked" } else if !look.is_empty() { "look" } else { "clear" };
    let report = &behaviour["report"];
    let lines = json!({ "ran": report["coverage"]["new"]["ran"].as_u64().unwrap_or(0) + report["coverage"]["old"]["ran"].as_u64().unwrap_or(0),
        "total": report["coverage"]["new"]["total"].as_u64().unwrap_or(0) + report["coverage"]["old"]["total"].as_u64().unwrap_or(0) });
    let not_checked: Vec<Value> = rules["not_checked"].as_array().into_iter().flatten().map(|x| json!({ "what": format!("{}: {}", code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""))) }))
        .chain(d["skipped"].as_array().into_iter().flatten().map(|x| json!({ "what": format!("{}: {}", code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""))) }))).collect();
    json!({ "version": 1, "kern": env!("CARGO_PKG_VERSION"), "base": d["base"], "head": d["head"], "decision": decision, "blocked": blocked, "look": look, "tests": tests,
        "behaviour": { "status": behaviour["status"], "why": behaviour["why"], "percent": report["coverage"]["percent"], "lines": lines, "described": described, "not_mentioned": surprises },
        "rules": rules, "not_checked": not_checked })
}
fn plain(s: &str) -> String { s.chars().map(|c| match c { '<' => '‹', '`' => '\'', c if c.is_control() => ' ', c => c }).take(400).collect() }

fn plural(n: usize, one: &str, many: &str) -> String { format!("{n} {}", if n == 1 { one } else { many }) }
fn list(md: &mut String, items: &[Value]) {
    for x in items.iter().take(SHOWN) { md.push_str(&format!("- {}\n", x["what"].as_str().unwrap_or(""))); }
    if items.len() > SHOWN { md.push_str(&format!("- … and {} more in Kern's receipt.json\n", items.len() - SHOWN)); }
}

pub fn markdown(r: &Value, drive_comment: Option<&str>) -> String {
    let blocked = r["blocked"].as_array().cloned().unwrap_or_default();
    let look = r["look"].as_array().cloned().unwrap_or_default();
    let tests = &r["tests"];
    let behaviour = &r["behaviour"];
    let tests_ran = tests["status"] == "RAN";
    let behaviour_ran = behaviour["status"] == "RAN";
    let nothing = tests["status"] == "NOTHING_TO_RUN";
    let mut md = String::from(MARK);
    md.push('\n');
    md += &match r["decision"].as_str() {
        Some("blocked") => format!("### ❌ Kern: Blocked — {}\n\n", plural(blocked.len(), "problem to fix", "problems to fix")),
        Some("look") => format!("### ⚠️ Kern: {} a look\n\n", plural(look.len(), "thing needs", "things need")),
        _ if tests_ran || behaviour_ran || nothing => "### ✅ Kern: Nothing unexpected\n\n".to_string(),
        _ => "### ✅ Kern: Nothing unexpected in the rules it checked\n\nNot run: the project's tests and its changed code (see below), so this receipt covers the rules only.\n\n".to_string(),
    };
    if !blocked.is_empty() {
        md += "**Blocked: fix these.** An approval cannot clear them.\n\n";
        list(&mut md, &blocked);
        md += "\n";
    }
    if !look.is_empty() {
        md += "**Needs a look.** If each one is meant, a maintainer comments `/kern approve`.\n\n";
        list(&mut md, &look);
        md += "\n";
    }
    md += "**What stayed the same**\n\n";
    if tests_ran {
        let kept = tests["kept"].as_u64().unwrap_or(0) as usize;
        let mut line = format!("- {} Tests: {} before and still pass ({})", if blocked.iter().any(|b| b["kind"] == "test-broke") { "⚠️" } else { "✅" },
            plural(kept, "test passed", "tests passed"), tests["runner"].as_str().unwrap_or("tests"));
        let added = tests["added"].as_array().map_or(0, Vec::len);
        let failing = tests["already_failing"].as_array().map_or(0, Vec::len);
        let flaky = tests["flaky"].as_array().map_or(0, Vec::len);
        if added > 0 { line += &format!(" · {} new", added); }
        if flaky > 0 { line += &format!(" · {} failed once and passed when run again (flaky)", flaky); }
        if failing > 0 { line += &format!(" · {} already failing before this change (set aside)", failing); }
        md += &line;
        md += "\n";
        for n in tests["not_run"].as_array().into_iter().flatten() {
            md += &format!("- ⏭️ Tests in {}: Not run — {}\n", code(n["dir"].as_str().unwrap_or(""), 160), plain(n["why"].as_str().unwrap_or("")));
        }
    } else if nothing {
        md += "- ✅ Tests: nothing to run — this change touches no code, tests or package.json\n";
    } else {
        md += &format!("- ⏭️ Tests: Not run — {}\n", plain(tests["why"].as_str().unwrap_or("Kern found no tests it can run")));
    }
    let powers = r["rules"]["powers"].as_array().map_or(0, Vec::len);
    if powers == 0 { md += "- ✅ No new powers: no changed code newly starts programs, reaches the network or reads secrets\n"; }
    if r["rules"]["tricks"].as_array().is_none_or(Vec::is_empty) && tests["gone"].as_array().is_none_or(Vec::is_empty) && tests["newly_skipped"].as_array().is_none_or(Vec::is_empty) {
        md += "- ✅ No skipped, focused or removed tests\n";
    }
    match behaviour["status"].as_str() {
        Some("RAN") => md += &format!("- {} {}% of the changed lines ran, on the old and the new code\n", if behaviour["percent"].as_u64().unwrap_or(0) >= 80 { "✅" } else { "⚠️" }, behaviour["percent"].as_u64().unwrap_or(0)),
        Some("NOTHING_TO_RUN") => md += "- ✅ No changed line of code to run: only tests, docs, configuration or types changed\n",
        Some("OFF") => md += "- ⏭️ Changed lines: Not run. Add an AI key (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`) and Kern runs every changed line on the old and the new code\n",
        _ => md += &format!("- ⏭️ Changed lines: Not run — {}\n", plain(behaviour["why"].as_str().unwrap_or("the run did not finish"))),
    }
    let described = behaviour["described"].as_array().map_or(0, Vec::len);
    if described > 0 { md += &format!("- ✅ {} the pull request describes\n", plural(described, "behaviour change", "behaviour changes")); }
    md += "\n";
    if let Some(c) = drive_comment.filter(|c| !c.trim().is_empty()) {
        let body = c.lines().skip_while(|l| l.starts_with("### ") || l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        md += &format!("<details><summary>What this change does when it runs, line by line</summary>\n\n{body}\n\n</details>\n\n");
    }
    let not_checked = r["not_checked"].as_array().cloned().unwrap_or_default();
    if !not_checked.is_empty() {
        md += "**Not checked**\n\n";
        list(&mut md, &not_checked);
        md += "\n";
    }
    let short = |k: &str| r[k].as_str().filter(|h| h.len() >= 7).map(|h| h[..7].to_string()).unwrap_or_default();
    let sandbox = if tests_ran || behaviour_ran { ", and ran the code only in a sealed sandbox with the network off" } else { "" };
    md += &format!("<sub>Kern {} checked `{}` against `{}`{sandbox}.</sub>\n", r["kern"].as_str().unwrap_or(""), short("head"), short("base"));
    if md.len() > 60_000 {
        let mut cut = 60_000;
        while !md.is_char_boundary(cut) { cut -= 1; }
        md.truncate(md[..cut].rfind('\n').unwrap_or(cut));
        md += "\n… (cut short; the full receipt is in the job's receipt.json)\n";
    }
    md
}

pub fn write(out: &Path, r: &Value, drive_comment: Option<&str>) -> R<()> {
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("receipt.json"), format!("{}\n", serde_json::to_string_pretty(r)?))?;
    std::fs::write(out.join("receipt.md"), markdown(r, drive_comment))?;
    Ok(())
}

pub fn seal(r: &Value, repository: &str, number: u64) -> R<Value> {
    Ok(json!({ "v": 1, "repository": repository, "pull": number, "base": r["base"], "head": r["head"], "decision": r["decision"], "receipt_sha256": digest(r)? }))
}
pub fn seal_line(seal: &Value, token: &str) -> String { format!("{SEAL_MARK}{} -->", json!({ "seal": seal, "token": token })) }

pub fn sign(out: &Path, repository: &str, number: u64) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let s = seal(&read_json(out.join("receipt.json"))?, repository, number)?;
    let token = crate::oidc::request(&crate::oidc::audience(&s)?)?;
    std::fs::write(out.join("seal.json"), format!("{}\n", serde_json::to_string_pretty(&json!({ "seal": s, "token": token }))?))?;
    Ok(json!({ "status": "SIGNED", "decision": s["decision"], "head": s["head"] }))
}

pub fn signed(seal: &Value, token: &str, jwks: &Value, repository: &str) -> R<Value> {
    let claims = crate::oidc::verify(token, jwks, &crate::oidc::audience(seal)?, crate::oidc::WORKFLOW, Some(repository))?;
    kern_workflow(&claims)?;
    Ok(claims)
}
pub fn kern_workflow(claims: &Value) -> R<()> {
    let at = claims["job_workflow_ref"].as_str().and_then(|r| r.strip_prefix(crate::oidc::WORKFLOW)).unwrap_or("");
    ensure!(at.starts_with("refs/heads/") || at.starts_with("refs/tags/"), "The receipt was not signed by Kern's workflow at a branch or tag of Kern's repository ({})", code(at, 80));
    Ok(())
}

pub fn verify(api: &Api, repository: &str, number: u64, jwks: &Value, receipt: Option<&Value>) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let Some(c) = kern_comment(api, repository, number)? else { return Ok(json!({ "status": "NO_RECEIPT" })) };
    let Some((seal, token)) = read_seal_line(c["body"].as_str().unwrap_or("")) else { return Ok(json!({ "status": "UNSIGNED", "comment": c["html_url"] })) };
    if seal["repository"] != repository || seal["pull"] != number { return Ok(json!({ "status": "INVALID", "why": "the seal is for another pull request" })); }
    let claims = match signed(&seal, &token, jwks, repository) { Ok(c) => c, Err(e) => return Ok(json!({ "status": "INVALID", "why": e.0 })) };
    if let Some(r) = receipt && seal["receipt_sha256"] != digest(r)?.as_str() {
        return Ok(json!({ "status": "INVALID", "why": "receipt.json is not the receipt this seal names" }));
    }
    let head = head_of(api, repository, number)?;
    Ok(json!({ "status": "VERIFIED", "decision": seal["decision"], "head": seal["head"], "base": seal["base"], "current": seal["head"] == head.as_str(),
        "workflow": claims["job_workflow_ref"], "run": claims["run_id"], "signed_at": claims["iat"], "receipt_checked": receipt.is_some(), "comment": c["html_url"] }))
}
fn read_seal_line(body: &str) -> Option<(Value, String)> {
    let line = body.lines().rev().find(|l| l.starts_with(SEAL_MARK))?;
    let v: Value = serde_json::from_str(line.strip_prefix(SEAL_MARK)?.strip_suffix(" -->")?).ok()?;
    Some((v["seal"].clone(), v["token"].as_str()?.to_string()))
}

fn kern_comment(api: &Api, repository: &str, number: u64) -> R<Option<Value>> {
    let who = author();
    for page in 1..=10 {
        let items = api.get(&format!("/repos/{repository}/issues/{number}/comments?per_page=100&page={page}"))?.as_array().cloned().unwrap_or_default();
        if let Some(c) = items.iter().find(|c| c["body"].as_str().is_some_and(|b| b.starts_with(MARK)) && c["user"]["login"] == who.as_str()) { return Ok(Some(c.clone())); }
        if items.len() < 100 { break; }
    }
    Ok(None)
}
fn head_of(api: &Api, repository: &str, number: u64) -> R<String> {
    Ok(api.get(&format!("/repos/{repository}/pulls/{number}"))?["head"]["sha"].as_str().unwrap_or("").to_string())
}
fn status(api: &Api, repository: &str, sha: &str, state: &str, description: &str, url: Option<&str>) -> R<()> {
    let description: String = description.chars().take(140).collect();
    let mut body = json!({ "state": state, "context": "Kern", "description": description });
    if let Some(u) = url { body["target_url"] = json!(u); }
    api.post(&format!("/repos/{repository}/statuses/{sha}"), &body)?;
    Ok(())
}
fn description(r: &Value) -> String {
    let n = |k: &str| r[k].as_array().map_or(0, Vec::len);
    match r["decision"].as_str() {
        Some("blocked") => format!("Blocked: {}", r["blocked"][0]["what"].as_str().unwrap_or("a problem to fix").replace('`', "")),
        Some("look") => format!("{} — approve with /kern approve", plural(n("look"), "thing needs a look", "things need a look")),
        _ => "Nothing unexpected".to_string(),
    }
}

pub fn post(api: &Api, out: &Path, repository: &str, number: u64) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let r = read_json(out.join("receipt.json"))?;
    let head = r["head"].as_str().unwrap_or("").to_string();
    let now = head_of(api, repository, number)?;
    if now != head { return Ok(json!({ "status": "STALE", "checked": head, "head": now })); }
    let mut body = std::fs::read_to_string(out.join("receipt.md"))?;
    if let Ok(s) = read_json(out.join("seal.json")) {
        body += &format!("<sub>Signed by GitHub for Kern's own workflow. Anyone can check this receipt: `kern receipt verify {repository} {number}`</sub>\n{}\n",
            seal_line(&s["seal"], s["token"].as_str().unwrap_or("")));
    }
    let (posted, url) = match kern_comment(api, repository, number)? {
        Some(c) => { api.patch(&format!("/repos/{repository}/issues/comments/{}", c["id"]), &json!({ "body": body }))?; ("UPDATED", c["html_url"].as_str().map(str::to_string)) }
        None => { let c = api.post(&format!("/repos/{repository}/issues/{number}/comments"), &json!({ "body": body }))?; ("POSTED", c["html_url"].as_str().map(str::to_string)) }
    };
    let state = if r["decision"] == "clear" { "success" } else { "failure" };
    status(api, repository, &head, state, &description(&r), url.as_deref())?;
    Ok(json!({ "status": posted, "state": state, "decision": r["decision"] }))
}

pub fn approve(api: &Api, repository: &str, number: u64, login: &str, check: &dyn Fn(&Value, &str) -> R<()>) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    ensure!(!login.is_empty() && login.len() <= 100 && login.chars().all(|c| c.is_ascii_alphanumeric() || "-_[]".contains(c)), "Expected a GitHub login");
    let say = |text: &str| -> R<()> { api.post(&format!("/repos/{repository}/issues/{number}/comments"), &json!({ "body": text }))?; Ok(()) };
    let permission = api.get(&format!("/repos/{repository}/collaborators/{login}/permission"))?;
    if !["admin", "maintain", "write"].contains(&permission["permission"].as_str().unwrap_or("")) {
        say(&format!("Kern: only people who can merge into this repository can approve its receipt; @{login} cannot."))?;
        return Ok(json!({ "status": "NOT_ALLOWED" }));
    }
    let head = head_of(api, repository, number)?;
    let Some(c) = kern_comment(api, repository, number)? else { say("Kern: there is no receipt to approve yet.")?; return Ok(json!({ "status": "NO_RECEIPT" })) };
    let body = c["body"].as_str().unwrap_or("").to_string();
    let Some((seal, token)) = read_seal_line(&body) else { say("Kern: this receipt carries no signature, so it cannot be approved.")?; return Ok(json!({ "status": "UNVERIFIED" })) };
    if check(&seal, &token).is_err() || seal["repository"] != repository || seal["pull"] != number {
        say("Kern: this receipt's signature does not check out, so it is not approved.")?;
        return Ok(json!({ "status": "UNVERIFIED" }));
    }
    if seal["head"] != head.as_str() { say("Kern: the receipt is for an older commit. Wait for Kern to check the newest one, then approve.")?; return Ok(json!({ "status": "STALE" })); }
    match seal["decision"].as_str() {
        Some("blocked") => { say("Kern: blocked items need a fix; an approval cannot clear them.")?; Ok(json!({ "status": "BLOCKED" })) }
        Some("look") => {
            status(api, repository, &head, "success", &format!("Approved by @{login}"), c["html_url"].as_str())?;
            let note = format!("\n\n**Approved by @{login}** on {}: every item above was looked at.\n", iso8601(now_ms()));
            let updated = match body.rfind(SEAL_MARK) { Some(at) => format!("{}{note}
{}", body[..at].trim_end(), &body[at..]), None => format!("{body}{note}") };
            api.patch(&format!("/repos/{repository}/issues/comments/{}", c["id"]), &json!({ "body": updated }))?;
            Ok(json!({ "status": "APPROVED", "by": login, "head": head }))
        }
        _ => Ok(json!({ "status": "NOTHING_TO_APPROVE" })),
    }
}
