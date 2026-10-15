use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::path::Path;

pub struct Ai<'a> { pub agent: &'a [String], pub rounds: usize }

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
    let checked = now_ms();
    let behaviour = match ai {
        _ if !run => json!({ "status": "NOTHING_TO_RUN" }),
        None => json!({ "status": "OFF", "why": "no AI key" }),
        Some(a) => match crate::drive::auto(repo, base, head, out, op, pr, a.agent, a.rounds) {
            Ok(v) => json!({ "status": v["status"], "why": v["rounds"].as_array().and_then(|r| r.last()).and_then(|r| r["error"].as_str()).unwrap_or(""),
                "report": read_json(out.join("report.json")).unwrap_or(Value::Null) }),
            Err(e) => json!({ "status": "FAILED", "why": e.0 }),
        },
    };
    let behaved = now_ms();
    let tests = if !run {
        json!({ "status": "NOTHING_TO_RUN", "why": "this change touches no code, tests or package.json" })
    } else {
        crate::suite::run(out, op).unwrap_or_else(|e| json!({ "status": "NOT_RUN", "why": format!("Kern could not run the tests: {}", e.0) }))
    };
    let tested = now_ms();
    let comment = std::fs::read_to_string(out.join("comment.md")).ok().filter(|_| behaviour["status"] == "RAN");
    let receipt = crate::receipt::build(&d, &rules, &tests, &behaviour);
    crate::receipt::write(out, &receipt, comment.as_deref())?;
    Ok(json!({ "status": "CHECKED", "decision": receipt["decision"], "blocked": receipt["blocked"].as_array().map_or(0, Vec::len),
        "look": receipt["look"].as_array().map_or(0, Vec::len), "tests": tests["status"], "behaviour": behaviour["status"], "seconds": (now_ms() - started) / 1000,
        "phases": { "rules": (checked - started) / 1000, "behaviour": (behaved - checked) / 1000, "tests": (tested - behaved) / 1000, "tests_detail": tests["seconds"] },
        "receipt": out.join("receipt.md").display().to_string() }))
}
