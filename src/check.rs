use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

pub struct Ai<'a> { pub agent: &'a [String], pub rounds: usize }

const MINUTES: u64 = 55;
const TESTS_MINUTES: u64 = 25;
pub fn minutes() -> u64 { std::env::var("KERN_CHECK_MINUTES").ok().and_then(|m| m.trim().parse::<u64>().ok()).filter(|m| (1..=1440).contains(m)).unwrap_or(MINUTES) }

const DOCS: [&str; 13] = ["md", "mdx", "markdown", "txt", "rst", "adoc", "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf"];
const DOC_NAMES: [&str; 7] = ["license", "licence", "changelog", "authors", "contributors", "codeowners", "notice"];
fn documentation(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    DOCS.contains(&ext) || (DOC_NAMES.contains(&stem) && ext.is_empty())
}
fn runnable(repo: &Path, head: &str, rules: &Value) -> bool {
    let paths: Vec<&str> = rules["changed"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
    if paths.iter().any(|p| !documentation(p)) { return true; }
    let mut needles: BTreeSet<String> = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).filter(|n| !n.is_empty()).collect();
    for folder in paths.iter().filter_map(|p| p.rsplit_once('/').map(|(d, _)| d)).flat_map(|d| d.split('/')).filter(|f| !f.is_empty()) {
        needles.extend([format!("{folder}/"), format!("'{folder}'"), format!("\"{folder}\""), format!("`{folder}`")]);
    }
    if needles.is_empty() { return false; }
    crate::git::mentions(repo, head, &needles.iter().map(String::as_str).collect::<Vec<_>>()).map_or(true, |found| found.iter().any(|f| crate::capability::is_code(f)))
}

pub fn check(repo: &Path, base: &str, head: &str, out: &Path, op: &Operator, pr: Option<&str>, ai: Option<&Ai>) -> R<Value> {
    let started = now_ms();
    let until = started + minutes() * 60_000;
    crate::drive::prepare(repo, base, head, out, pr)?;
    let d = read_json(out.join("drive.json"))?;
    let rules = crate::rules::rules(repo, base, head)?;
    let run = runnable(repo, head, &rules);
    let checked = now_ms();
    let behaviour = match ai {
        _ if !run => json!({ "status": "NOTHING_TO_RUN" }),
        None => json!({ "status": "OFF", "why": "no AI key" }),
        Some(a) => match crate::drive::auto(repo, base, head, out, op, pr, a.agent, a.rounds, until.saturating_sub(TESTS_MINUTES * 60_000).max(started)) {
            Ok(v) => json!({ "status": v["status"], "why": v["rounds"].as_array().and_then(|r| r.last()).and_then(|r| r["error"].as_str()).unwrap_or(""),
                "report": read_json(out.join("report.json")).unwrap_or(Value::Null) }),
            Err(e) => json!({ "status": "FAILED", "why": e.0 }),
        },
    };
    let behaved = now_ms();
    let tests = if !run {
        json!({ "status": "NOTHING_TO_RUN", "why": "this change touches only documentation" })
    } else {
        crate::suite::run(out, op, until).unwrap_or_else(|e| json!({ "status": "NOT_RUN", "why": format!("Kern could not run the tests: {}", e.0) }))
    };
    let tested = now_ms();
    let comment = std::fs::read_to_string(out.join("comment.md")).ok().filter(|_| behaviour["status"] == "RAN");
    let receipt = crate::receipt::build(&d, &rules, &tests, &behaviour);
    crate::receipt::write(out, &receipt, comment.as_deref())?;
    let keep = std::env::var("KERN_KEEP").is_ok_and(|v| v.trim() == "1");
    let cleaned = !keep && (out.join("setup.json").exists() || out.join("volumes.txt").exists()) && crate::drive::clean(out).is_ok();
    Ok(json!({ "status": "CHECKED", "cleaned": cleaned, "decision": receipt["decision"], "blocked": receipt["blocked"].as_array().map_or(0, Vec::len),
        "look": receipt["look"].as_array().map_or(0, Vec::len), "tests": tests["status"], "behaviour": behaviour["status"], "seconds": (now_ms() - started) / 1000,
        "phases": { "rules": (checked - started) / 1000, "behaviour": (behaved - checked) / 1000, "tests": (tested - behaved) / 1000, "tests_detail": tests["seconds"] },
        "receipt": out.join("receipt.md").display().to_string() }))
}
