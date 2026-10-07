use crate::audit::esc;
use crate::git;
use crate::github::{Api, env, repository as valid_repository};
use crate::promote::{self, Merged};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "kern-merge-audit/v1";
const CONFIG: &str = ".github/kern";
const FETCH: &str = "git fetch origin '+refs/pull/*/head:refs/remotes/pull/*'";
const STATEMENT: &str = "Kern checked every change that reached the branch in this period. A Kern proof shows that an agent's change stayed inside its task contract, gave the code no new authority, \
    passed the operator's checks on that exact commit in a gVisor sandbox without network and showed no new runtime behaviour there, and, where its contract requires a test proof, kept every existing \
    test running and passing and, for a bug fix, came with a test that failed before the change and passed after it. Kern re-verified each proof for this report. A proof describes the change and how \
    it was verified, not what the agent did while writing it. Changes that did not come from a Kern agent branch are listed as ordinary: Kern cannot tell whether a person used AI tools to write them.";
const PLACEMENT: &str = "Changes are placed by their order on the branch, not by the date in the commit: a commit dated earlier than the change before it counts at that change's time, and one dated in the \
    future counts at the time of the change before it, so a false date cannot move a change out of the period.";

fn epoch(date: &str) -> i64 {
    let (y, m, d) = (date[0..4].parse::<i64>().unwrap_or(0), date[5..7].parse::<i64>().unwrap_or(1), date[8..10].parse::<i64>().unwrap_or(1));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    (era * 146097 + doe - 719468) * 86400
}
fn branch_name(checkout: &Path, git_ref: &str) -> R<String> {
    let full = git::trimmed(git::git(checkout, &["rev-parse", "--symbolic-full-name", git_ref], None)?);
    if let Some(b) = full.strip_prefix("refs/heads/") { return Ok(b.to_string()); }
    if let Some((_, b)) = full.strip_prefix("refs/remotes/").and_then(|r| r.split_once('/')) { return Ok(b.to_string()); }
    deny!("{git_ref} is not a branch. Pass the branch to audit, for example origin/main")
}

pub fn period(p: &str) -> R<(String, String)> {
    let bad = || Denied(format!("Unsupported period {p}: use a quarter such as 2026-Q3, or dates such as 2026-07-01..2026-09-30"));
    let date = |d: &str| d.len() == 10 && d.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() })
        && (1..=12).contains(&d[5..7].parse::<u32>().unwrap_or(0)) && (1..=31).contains(&d[8..10].parse::<u32>().unwrap_or(0));
    if let Some((year, q)) = p.split_once("-Q") {
        ensure!(year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()), "{}", bad().0);
        let (from, to) = match q { "1" => ("01-01", "03-31"), "2" => ("04-01", "06-30"), "3" => ("07-01", "09-30"), "4" => ("10-01", "12-31"), _ => return Err(bad()) };
        return Ok((format!("{year}-{from}"), format!("{year}-{to}")));
    }
    let (from, until) = p.split_once("..").ok_or_else(bad)?;
    ensure!(date(from) && date(until) && from <= until, "{}", bad().0);
    Ok((from.to_string(), until.to_string()))
}

struct Context<'a> { checkout: &'a Path, api: &'a Api, repository: &'a str, branch: &'a str, auditor_key: Option<&'a str>, now: u64 }
fn check_run(c: &Context, head: &str) -> &'static str {
    match c.api.get(&format!("/repos/{}/commits/{head}/check-runs?check_name=kern-verified&per_page=100", c.repository)) {
        Ok(v) => {
            let runs = v["check_runs"].as_array().cloned().unwrap_or_default();
            if runs.iter().any(|r| r["conclusion"] == "success") { "success" } else if runs.is_empty() { "missing" } else { "failure" }
        }
        Err(_) => "unknown",
    }
}
fn agent(c: &Context, pr: &Value, sha: &str, parent: Option<&str>) -> (&'static str, Value, Option<String>) {
    let (head, branch) = (pr["head"]["sha"].as_str().unwrap_or(""), pr["head"]["ref"].as_str().unwrap_or(""));
    if pr["merge_commit_sha"] != sha { return ("invalid-proof", Value::Null, Some("A Kern branch reached the branch as more than one commit, so no single proof covers this one".into())); }
    if git::oid(head).is_err() || git::git(c.checkout, &["cat-file", "-e", &format!("{head}^{{commit}}")], None).is_err() {
        return ("unverifiable", Value::Null, Some(format!("The pull request head {head} is not in this checkout. Fetch the pull request heads and run the audit again: {FETCH}")));
    }
    let merged = Merged { repository: c.repository, branch, head, config: CONFIG, auditor_key: c.auditor_key, now: c.now };
    let mut proof = match promote::extract_attestation(pr["body"].as_str().unwrap_or("")).and_then(|envelope| promote::verify_merged(c.checkout, &envelope, &merged)) {
        Ok(p) => p,
        Err(e) => return ("invalid-proof", Value::Null, Some(e.0)),
    };
    let tree = |x: &str| git::git(c.checkout, &["rev-parse", &format!("{x}^{{tree}}")], None).map(git::trimmed).ok();
    if parent != proof["base"].as_str() || tree(sha).is_none() || tree(sha) != tree(head) {
        return ("invalid-proof", Value::Null, Some("The merged result differs from the verified commit: the pull request was not up to date with the base it was verified on, or it changed after verification".into()));
    }
    proof["check_run"] = json!(check_run(c, head));
    ("verified", proof, None)
}
fn change(c: &Context, sha: &str, parent: Option<&str>, date: &str, author: &str, subject: &str, mut flags: Vec<&'static str>) -> R<Value> {
    let range: Vec<&str> = match parent { Some(p) => vec!["diff-tree", "--no-commit-id", "--name-only", "-r", p, sha], None => vec!["diff-tree", "--no-commit-id", "--name-only", "-r", "--root", sha] };
    let touched = String::from_utf8_lossy(&git::git(c.checkout, &range, None)?).to_string();
    if touched.lines().any(|f| f.starts_with(".github/kern/") || f.starts_with(".github/workflows/")) { flags.push("gate-config"); }
    let pulls = c.api.get(&format!("/repos/{}/commits/{sha}/pulls?per_page=100", c.repository))?;
    // Only a pull request into this branch can cover a change to it; one merged elsewhere does not.
    let merged: Vec<&Value> = pulls.as_array().into_iter().flatten().filter(|p| p["merged_at"].is_string() && p["base"]["ref"] == c.branch).collect();
    let pr = merged.iter().find(|p| p["merge_commit_sha"] == sha).or(merged.first()).copied();
    let (category, proof, reason) = match pr {
        None => ("direct-push", Value::Null, Some(format!("No pull request merged into {} contains this commit: it reached the branch without the pull request gate", c.branch))),
        Some(p) if p["head"]["ref"].as_str().is_some_and(|r| r.starts_with("kern/")) => agent(c, p, sha, parent),
        Some(_) => ("ordinary", Value::Null, None),
    };
    if proof["check_run"].as_str().is_some_and(|s| s != "success") { flags.push("merged-without-passing-check"); }
    Ok(json!({
        "commit": sha, "date": date, "author": author, "subject": subject, "category": category, "gap": matches!(category, "direct-push" | "invalid-proof" | "unverifiable"), "flags": flags,
        "pull_request": pr.map_or(Value::Null, |p| json!({ "number": p["number"], "url": p["html_url"], "branch": p["head"]["ref"], "author": p["user"]["login"], "merged_at": p["merged_at"] })),
        "proof": proof, "reason": reason,
    }))
}

pub fn audit(checkout: &Path, git_ref: &str, range: &str, auditor_key: Option<&str>, now: u64) -> R<Value> {
    ensure!(!git_ref.is_empty() && git_ref.len() <= 200 && !git_ref.starts_with('-') && git_ref.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)), "Invalid branch name: {git_ref}");
    let (from, until) = period(range)?;
    ensure!(git::trimmed(git::git(checkout, &["rev-parse", "--is-shallow-repository"], None)?) == "false",
        "The checkout is shallow, so some changes would be missed. Fetch the full history and the pull request heads first: git fetch --unshallow origin; {FETCH}");
    let promisor = git::git(checkout, &["config", "--get-regexp", r"^(remote\..*\.promisor|extensions\.partialclone)$"], None).map(git::trimmed).unwrap_or_default();
    ensure!(!promisor.lines().any(|l| l.starts_with("extensions.") || l.ends_with(" true")),
        "The checkout is a partial clone, so objects the audit needs may be missing. Use a full clone with the pull request heads: git clone <url>; {FETCH}");
    let branch = branch_name(checkout, git_ref)?;
    let repository = match env("GITHUB_REPOSITORY") { Some(r) => r, None => crate::init::origin(checkout)? };
    ensure!(valid_repository(&repository), "Invalid repository name: {repository}");
    let key_id = match auditor_key { Some(k) => Some(crate::attest::key_id(&crate::attest::public_key(k)?)?), None => None };
    let api = Api::from_env("read the repository's pull requests and check runs")?;
    let c = Context { checkout, api: &api, repository: &repository, branch: &branch, auditor_key, now };
    // The whole first-parent history, not a date-filtered walk: git stops a --since walk at the first commit that claims to be older.
    let log = git::git(checkout, &["log", "--first-parent", "--format=%H%x1f%P%x1f%ct%x1f%cI%x1f%an <%ae>%x1f%s%x1e", git_ref, "--"], None)?;
    let text = String::from_utf8_lossy(&log);
    let mut records = Vec::new();
    for record in text.split('\x1e').map(str::trim).filter(|r| !r.is_empty()) {
        let f: Vec<&str> = record.split('\x1f').collect();
        let &[sha, parents, time, date, author, subject] = f.as_slice() else { deny!("Unexpected git log output") };
        git::oid(sha)?;
        let parent = parents.split(' ').next().filter(|p| !p.is_empty());
        records.push((sha, parent, time.parse::<i64>().map_err(|_| Denied("Unexpected git log output".into()))?, date, author, subject));
    }
    ensure!(records.windows(2).all(|w| w[0].1 == Some(w[1].0)) && records.last().is_none_or(|r| r.1.is_none()), "The branch history could not be read in order");
    let (start, end, now_s) = (epoch(&from), epoch(&until) + 86399, (now / 1000) as i64);
    let (mut position, mut selected) = (None::<i64>, Vec::new());
    for &(sha, parent, time, date, author, subject) in records.iter().rev() {
        let future = time > now_s + 86400;
        let flag = if future { Some("future-dated") } else if position.is_some_and(|p| time + 3600 < p) { Some("backdated") } else { None };
        let placed = match (position, future) { (Some(p), true) => p, (None, true) => now_s, (p, false) => p.map_or(time, |p| p.max(time)) };
        position = Some(placed);
        if (start..=end).contains(&placed) { selected.push((sha, parent, date, author, subject, flag)); }
    }
    let mut changes = Vec::new();
    for &(sha, parent, date, author, subject, flag) in &selected {
        changes.push(change(&c, sha, parent, date, author, subject, flag.into_iter().collect())?);
    }
    let count = |category: &str| changes.iter().filter(|x| x["category"] == category).count();
    let (verified, invalid, unverifiable) = (count("verified"), count("invalid-proof"), count("unverifiable"));
    let gaps = changes.iter().filter(|x| x["gap"] == true).count();
    let summary = json!({
        "status": if gaps == 0 { "COVERED" } else { "GAPS" }, "changes": changes.len(), "verified": verified, "ordinary": count("ordinary"), "direct_pushes": count("direct-push"),
        "invalid_proofs": invalid, "unverifiable": unverifiable, "gaps": gaps, "agent_changes": verified + invalid + unverifiable,
        "gate_config_changes": changes.iter().filter(|x| x["flags"].as_array().is_some_and(|f| f.iter().any(|g| g == "gate-config"))).count(),
        "date_anomalies": selected.iter().filter(|s| s.5.is_some()).count(),
    });
    let controls = json!([
        { "framework": "SOC 2 (2017 Trust Services Criteria)", "control": "CC8.1 Change management", "evidence": "Each verified agent change was authorised by a task contract, checked on the exact commit and merged through a pull request into this branch; every change that reached the branch without one is listed as a gap." },
        { "framework": "ISO/IEC 27001:2022", "control": "A.8.32 Change management", "evidence": "The same per-change record: who merged what, under which task contract, and with which re-verified proof." },
        { "framework": "ISO/IEC 27001:2022", "control": "A.8.29 Security testing in development and acceptance", "evidence": "Agent changes passed the capability, runtime-behaviour and operator checks, and, where their contract requires a test proof, the project's tests, before they were merged." },
    ]);
    let mut report = json!({
        "schema": SCHEMA, "repository": repository, "branch": branch, "period": { "from": from, "until": until, "requested": range }, "generated_at": iso8601(now), "kern": env!("CARGO_PKG_VERSION"),
        "history": { "ref": git_ref, "tip": records.first().map(|r| r.0), "first": selected.first().map(|s| s.0), "last": selected.last().map(|s| s.0), "walked": records.len(), "placement": PLACEMENT },
        "trusted_key": key_id.map_or_else(|| "the verifier key committed in each change's base".to_string(), |k| format!("key {k} supplied by the auditor")),
        "summary": summary, "statement": STATEMENT, "controls": controls,
        "controls_note": "These map Kern's evidence to controls an auditor may test. They are not a claim of compliance.", "changes": changes,
    });
    report["sha256"] = json!(digest(&report)?);
    Ok(report)
}

fn csv(fields: &[String]) -> String {
    fields.iter().map(|f| {
        let f = f.replace(['\r', '\n'], " ");
        let f = if f.starts_with(['=', '+', '-', '@', '\t']) { format!("'{f}") } else { f };
        format!("\"{}\"", f.replace('"', "\"\""))
    }).collect::<Vec<_>>().join(",")
}
fn text_of(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
pub fn write(out: &Path, report: &Value) -> R<Vec<PathBuf>> {
    let dir = fresh_dir(out)?;
    let changes = report["changes"].as_array().cloned().unwrap_or_default();
    let mut rows = vec![csv(&["date", "commit", "category", "gap", "flags", "pull_request", "branch", "author", "task", "verified_at", "reason", "subject"].map(String::from))];
    for c in &changes {
        rows.push(csv(&[text_of(&c["date"]), text_of(&c["commit"]), text_of(&c["category"]), text_of(&c["gap"]), c["flags"].as_array().map(|f| f.iter().map(text_of).collect::<Vec<_>>().join(" ")).unwrap_or_default(),
            text_of(&c["pull_request"]["url"]), text_of(&c["pull_request"]["branch"]), text_of(&c["author"]), text_of(&c["proof"]["task"]), text_of(&c["proof"]["verified_at"]), text_of(&c["reason"]), text_of(&c["subject"])]));
    }
    let s = &report["summary"];
    let tile = |label: &str, value: &Value| format!("<div class=\"tile\"><b>{}</b>{}</div>", esc(&text_of(value)), esc(label));
    let row = |c: &Value| {
        let pr = c["pull_request"]["number"].as_u64().map_or_else(|| "none".to_string(), |n| format!("<a href=\"{}\">#{n}</a>", esc(c["pull_request"]["url"].as_str().unwrap_or(""))));
        let detail = c["reason"].as_str().map(str::to_string).or_else(|| c["proof"]["task"].as_str().map(|t| format!("task {t}, verified {}", c["proof"]["verified_at"].as_str().unwrap_or("")))).unwrap_or_default();
        format!("<tr><td><code>{}</code></td><td><code>{}</code></td><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td class=\"reason\">{}</td></tr>",
            esc(&text_of(&c["date"])), esc(&text_of(&c["commit"])[..12.min(text_of(&c["commit"]).len())]), if c["gap"] == true { "bad" } else { "ok" }, esc(&text_of(&c["category"])), pr,
            esc(&text_of(&c["author"])), esc(&text_of(&c["subject"])), esc(&c["flags"].as_array().map(|f| f.iter().map(text_of).collect::<Vec<_>>().join(", ")).unwrap_or_default()), esc(&detail))
    };
    let gaps: String = changes.iter().filter(|c| c["gap"] == true).map(row).collect();
    let all: String = changes.iter().map(row).collect();
    let controls: String = report["controls"].as_array().into_iter().flatten().map(|c| format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>", esc(&text_of(&c["framework"])), esc(&text_of(&c["control"])), esc(&text_of(&c["evidence"])))).collect();
    let head = "<tr><th>Committed (UTC)</th><th>Commit</th><th>Category</th><th>Pull request</th><th>Author</th><th>Subject</th><th>Flags</th><th>Detail</th></tr>";
    let status = text_of(&s["status"]);
    let html = format!(r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Kern merge audit</title><style>
:root{{--bg:#fafaf9;--fg:#1c1917;--muted:#78716c;--card:#fff;--line:#e7e5e4;--ok:#15803d;--bad:#b91c1c}}
@media (prefers-color-scheme:dark){{:root{{--bg:#0c0a09;--fg:#e7e5e4;--muted:#a8a29e;--card:#1c1917;--line:#292524;--ok:#4ade80;--bad:#f87171}}}}
body{{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{max-width:1180px;margin:0 auto;padding:24px 16px}} h1{{font-size:20px;margin:0 0 4px}} h2{{font-size:15px;margin:28px 0 8px}} p{{max-width:880px}}
.muted{{color:var(--muted)}} .ok{{color:var(--ok)}} .bad{{color:var(--bad)}} .wrap{{overflow-x:auto}} code{{font-size:12px}} a{{color:inherit}}
.tiles{{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;margin:20px 0}}
.tile{{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px 14px;color:var(--muted)}} .tile b{{display:block;font-size:24px;color:var(--fg)}}
table{{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line)}}
th,td{{text-align:left;padding:8px 10px;border-bottom:1px solid var(--line);vertical-align:top}} th{{font-size:12px;color:var(--muted);font-weight:600}} td.reason{{word-break:break-word;min-width:260px}}
</style></head><body><main>
<h1>Kern merge audit: {repo} <code>{branch}</code></h1>
<div class="muted">{from} to {until} &middot; generated {generated} &middot; proofs checked against {key}</div>
<h2 class="{status_class}">{status}</h2>
<div class="tiles">{t1}{t2}{t3}{t4}{t5}</div>
<p>{statement}</p>
<h2>Gaps</h2><div class="wrap"><table>{head}{gaps}</table></div>
<h2>Every change, oldest first</h2><div class="wrap"><table>{head}{all}</table></div>
<h2>Controls this evidence supports</h2><div class="wrap"><table><tr><th>Framework</th><th>Control</th><th>Evidence</th></tr>{controls}</table></div>
<p class="muted">{note} Report SHA-256 <code>{sha}</code>.</p>
</main></body></html>
"#, repo = esc(&text_of(&report["repository"])), branch = esc(&text_of(&report["branch"])), from = esc(&text_of(&report["period"]["from"])), until = esc(&text_of(&report["period"]["until"])),
        generated = esc(&text_of(&report["generated_at"])), key = esc(&text_of(&report["trusted_key"])), status_class = if status == "COVERED" { "ok" } else { "bad" },
        status = if status == "COVERED" { "Covered: every change carries a valid proof or came through an ordinary pull request".to_string() } else { format!("{} gap(s) found", text_of(&s["gaps"])) },
        t1 = tile("changes", &s["changes"]), t2 = tile("verified agent changes", &s["verified"]), t3 = tile("ordinary changes", &s["ordinary"]), t4 = tile("gaps", &s["gaps"]),
        t5 = tile("gate configuration changes", &s["gate_config_changes"]), statement = esc(&text_of(&report["statement"])), note = esc(&text_of(&report["controls_note"])), sha = esc(&text_of(&report["sha256"])));
    let files = [(dir.join("report.json"), format!("{}\n", serde_json::to_string_pretty(report)?)), (dir.join("report.html"), html), (dir.join("report.csv"), format!("{}\n", rows.join("\n")))];
    for (path, body) in &files { create_new(path, body.as_bytes(), 0o644)?; }
    Ok(files.into_iter().map(|(p, _)| p).collect())
}
