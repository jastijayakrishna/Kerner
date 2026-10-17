use crate::policy::Runner;
use crate::proof::Report;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const ROLES: [&str; 12] = ["old-built-related", "new-related", "old-related", "new-alone", "new-again", "old-alone", "old-again", "new-kept", "old-kept", "old-built", "new", "old"];
const OLD: [&str; 4] = ["old-built-related", "old-related", "old-built", "old"];
const REPORT_BYTES_MAX: u64 = 64 << 20;
const ENTRIES_COUNT_MAX: usize = 10_000;
const SUITES_COUNT_MAX: usize = 16;

struct Judged { verdict: Value, runner: Runner, suite_dir: String, unconfirmed: bool, uneven: bool }
fn role(stem: &str) -> Option<(&'static str, &str)> {
    ROLES.iter().find_map(|r| stem.strip_prefix(r).filter(|rest| rest.is_empty() || rest.starts_with('-')).map(|rest| (*r, rest)))
}
pub fn runners(text: &str) -> Vec<(Runner, String)> {
    text.split(", ").filter_map(|part| {
        let mut words = part.splitn(3, ' ');
        let runner = Runner::parse(words.next()?)?;
        let dir = match (words.next(), words.next()) { (Some("in"), Some(rest)) => rest.split(' ').next().unwrap_or("").to_string(), (None, _) => String::new(), _ => return None };
        Some((runner, dir))
    }).collect()
}
fn report(path: &Path, runner: Runner) -> Option<Report> {
    if std::fs::metadata(path).ok()?.len() > REPORT_BYTES_MAX { return None; }
    crate::proof::read_report(runner, &std::fs::read(path).ok()?).ok()
}
fn suite(dir: &Path, label: &str, runner: Runner, unconfirmed: bool) -> Option<Value> {
    let run = |r: &str| report(&dir.join(format!("tests-{r}{label}.out")), runner);
    let old_alone = run("old-alone").filter(|r| !r.tests.is_empty() || !r.problems.is_empty()).or_else(|| run("old-again"));
    let (alone, mut whole) = (run("new-alone"), run("new-again"));
    let old = OLD.iter().find(|r| dir.join(format!("tests-{r}{label}.out")).is_file()).and_then(|r| run(r))?;
    let new = run("new-related").or_else(|| run("new")).or_else(|| if alone.is_none() { whole.take() } else { None })?;
    let c = crate::suite::verdict(&old, &new, alone.as_ref(), whole.as_ref(), old_alone.as_ref(), unconfirmed);
    (c["status"] == "RAN").then_some(c)
}
const FAULTS_BYTES_MAX: u64 = 16 << 20;
fn faults(dir: &Path, suites: &[(Runner, String)], stored: &Value) -> Option<Vec<Value>> {
    let mut outcomes = Vec::new();
    for (runner, suite_dir) in suites {
        let label = if suite_dir.is_empty() { String::new() } else { format!("-{}", suite_dir.replace('/', "_")) };
        let path = dir.join(format!("faults{label}.json"));
        if !path.is_file() { continue; }
        if std::fs::metadata(&path).ok()?.len() > FAULTS_BYTES_MAX { return None; }
        let raw: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        let new = ["new-related", "new"].iter().find_map(|r| report(&dir.join(format!("tests-{r}{label}.out")), *runner))?;
        outcomes.extend(raw.iter().map(|x| {
            let text = |k: &str| x[k].as_str().unwrap_or("").as_bytes().to_vec();
            crate::proof::fault_verdict(*runner, &new, (x["finished"] == true, x["served"] == true), &text("output"), &text("errors"))
        }));
    }
    let stored = stored.as_array()?;
    (outcomes.len() == stored.len()).then(|| stored.iter().zip(outcomes).map(|(f, o)| { let mut f = f.clone(); f["outcome"] = json!(o); f }).collect())
}
pub fn rejudge(dir: &Path) -> R<Value> {
    let stored = read_json(dir.join("receipt.json"))?;
    let mut tests = stored["tests"].clone();
    let suites = runners(tests["runner"].as_str().unwrap_or(""));
    let mut labels = BTreeSet::new();
    for e in std::fs::read_dir(dir)?.flatten().take(ENTRIES_COUNT_MAX) {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_prefix("tests-").and_then(|n| n.strip_suffix(".out")) else { continue };
        if let Some((r, label)) = role(stem) && (r == "new" || r == "new-related") { labels.insert(label.to_string()); }
    }
    let stored_uneven = tests["uneven_why"].as_array().is_some_and(|a| !a.is_empty());
    let setup = read_json(dir.join("setup.json")).ok();
    let judgeable = tests["status"] == "RAN" && !labels.is_empty() && labels.len() <= SUITES_COUNT_MAX;
    let judged: Option<Vec<Judged>> = judgeable.then(|| labels.iter().map(|label| {
        let (runner, suite_dir) = suites.iter().find(|(_, d)| d.replace('/', "_") == label.trim_start_matches('-'))?.clone();
        let unconfirmed = tests["unconfirmed"].as_array().into_iter().flatten().filter_map(Value::as_str)
            .any(|k| suite_dir.is_empty() || k.starts_with(&format!("{suite_dir}/")) || k.starts_with(&format!("{suite_dir}: ")));
        let uneven = stored_uneven || setup.as_ref().is_some_and(|s| crate::suite::setup_differs(s, &suite_dir).is_some());
        suite(dir, label, runner, unconfirmed).map(|verdict| Judged { verdict, runner, suite_dir, unconfirmed, uneven })
    }).collect()).flatten();
    if let Some(judged) = &judged {
        let mut merged: serde_json::Map<String, Value> = crate::suite::LISTS.iter().map(|l| (l.to_string(), json!([]))).collect();
        merged.insert("problem_why".into(), json!({}));
        for j in judged { crate::suite::fold(&mut merged, &j.verdict, &|k| crate::suite::in_suite(j.runner, &j.suite_dir, k), j.unconfirmed, j.uneven); }
        let kept_runs = labels.iter().any(|label| dir.join(format!("tests-new-kept{label}.out")).is_file());
        for l in crate::suite::LISTS.iter().filter(|l| **l != "rewritten" && !(kept_runs && **l == "gone")) { tests[*l] = merged[*l].clone(); }
        tests["problem_why"] = merged["problem_why"].clone();
    }
    if let Some(f) = faults(dir, &suites, &tests["faults"]) { tests["faults"] = json!(f); }
    let d = json!({ "base": stored["base"], "head": stored["head"], "skipped": [], "unmeasured": [] });
    let receipt = crate::receipt::build(&d, &stored["rules"], &tests, &stored["behaviour"]);
    Ok(json!({ "replayed": judged.as_ref().map_or(0, Vec::len), "suites": labels.len(), "receipt": receipt }))
}
