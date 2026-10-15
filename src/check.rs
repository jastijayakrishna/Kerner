//! `kern check`: one receipt for one pull request, from the parts Kern already has. In order:
//! 1. kern drive's prepare: what changed, which lines hold code, and the task folder for the AI's driver;
//! 2. the static rules (seconds, no sandbox): new powers, test tricks, and the files that steer builds, secrets and agents;
//! 3. the project's own tests on the old and the new code, each on its installed tree in gVisor with the network off;
//! 4. with an AI agent, every changed line run on the old and the new code, sorted by what the pull request says;
//! 5. the receipt: receipt.json and receipt.md in `out`.
//!
//! A part that cannot run says why in the receipt; only a broken repository or sandbox stops the check itself.
use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::path::Path;

/// What the AI part needs: the agent's command line (kern drive's default is Claude Code in restricted mode) and how many rounds it may take.
pub struct Ai<'a> { pub agent: &'a [String], pub rounds: usize }

/// Whether the change has anything to install and run: changed code Kern measures, or changed test files, or a changed package.json.
fn runnable(d: &Value, rules: &Value) -> bool {
    let code = d["totals"]["new"].as_u64().unwrap_or(0) + d["totals"]["old"].as_u64().unwrap_or(0) > 0;
    let paths: Vec<&str> = rules["changed"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
    code || !crate::suite::test_paths(&paths).is_empty() || paths.iter().any(|p| p.rsplit('/').next() == Some("package.json"))
}

pub fn check(repo: &Path, base: &str, head: &str, out: &Path, op: &Operator, pr: Option<&str>, ai: Option<&Ai>) -> R<Value> {
    let started = now_ms();
    crate::drive::prepare(repo, base, head, out, pr)?;
    let d = read_json(out.join("drive.json"))?;
    let rules = crate::rules::rules(repo, base, head)?;
    let run = runnable(&d, &rules);
    let tests = if !run {
        json!({ "status": "NOTHING_TO_RUN", "why": "this change touches no code, tests or package.json" })
    } else {
        crate::suite::run(out, op).unwrap_or_else(|e| json!({ "status": "NOT_RUN", "why": format!("Kern could not run the tests: {}", e.0) }))
    };
    let behaviour = match ai {
        _ if !run => json!({ "status": "NOTHING_TO_RUN" }),
        None => json!({ "status": "OFF", "why": "no AI key" }),
        Some(a) => match crate::drive::auto(repo, base, head, out, op, pr, a.agent, a.rounds) {
            Ok(v) => json!({ "status": v["status"], "why": v["rounds"].as_array().and_then(|r| r.last()).and_then(|r| r["error"].as_str()).unwrap_or(""),
                "report": read_json(out.join("report.json")).unwrap_or(Value::Null) }),
            Err(e) => json!({ "status": "FAILED", "why": e.0 }),
        },
    };
    let comment = std::fs::read_to_string(out.join("comment.md")).ok().filter(|_| behaviour["status"] == "RAN");
    let receipt = crate::receipt::build(&d, &rules, &tests, &behaviour);
    crate::receipt::write(out, &receipt, comment.as_deref())?;
    Ok(json!({ "status": "CHECKED", "decision": receipt["decision"], "blocked": receipt["blocked"].as_array().map_or(0, Vec::len),
        "look": receipt["look"].as_array().map_or(0, Vec::len), "tests": tests["status"], "behaviour": behaviour["status"], "seconds": (now_ms() - started) / 1000,
        "receipt": out.join("receipt.md").display().to_string() }))
}
