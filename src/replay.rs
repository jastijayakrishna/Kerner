use crate::policy::Runner;
use crate::proof::Report;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const ROLES: [&str; 12] = ["old-built-related", "new-related", "old-related", "new-alone", "new-again", "old-alone", "old-again", "new-kept", "old-kept", "old-built", "new", "old"];
const OLD: [&str; 4] = ["old-built-related", "old-related", "old-built", "old"];
const REPORT_BYTES_MAX: u64 = 64 << 20;
const ENTRIES_COUNT_MAX: usize = 10_000;
const SUITES_COUNT_MAX: usize = 16;

struct Judged { verdict: Value, runner: Runner, suite_dir: String, prefix: Option<String> }
const LEAVES_JSON_BYTES_MAX: u64 = 1 << 20;
struct Part { runner: Runner, dir: String, prefix: Option<String> }
fn leaves(dir: &Path) -> Option<std::collections::BTreeMap<String, Part>> {
    let path = dir.join("leaves.json");
    if !path.is_file() { return Some(std::collections::BTreeMap::new()); }
    if std::fs::metadata(&path).ok()?.len() > LEAVES_JSON_BYTES_MAX { return None; }
    let v: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    v.as_array()?.iter().take(SUITES_COUNT_MAX).map(|l| {
        let part = Part { runner: Runner::parse(l["suite"]["new"]["runner"].as_str()?)?, dir: l["suite"]["dir"].as_str()?.to_string(), prefix: l["prefix"].as_str().map(str::to_string) };
        Some((l["label"].as_str()?.to_string(), part))
    }).collect()
}
fn part(label: &str, suites: &[(Runner, String)], leaves: &std::collections::BTreeMap<String, Part>) -> Option<Part> {
    if let Some(p) = leaves.get(label) { return Some(Part { runner: p.runner, dir: p.dir.clone(), prefix: p.prefix.clone() }); }
    suites.iter().find(|(_, d)| d.replace('/', "_") == label.trim_start_matches('-')).map(|(runner, d)| Part { runner: *runner, dir: d.clone(), prefix: None })
}
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
fn suite(dir: &Path, label: &str, runner: Runner, (unconfirmed, uneven): (bool, bool), changed: &[(&str, &str)]) -> Option<Value> {
    let run = |r: &str| report(&dir.join(format!("tests-{r}{label}.out")), runner);
    let old_alone = run("old-alone").filter(|r| !r.tests.is_empty() || !r.problems.is_empty()).or_else(|| run("old-again"));
    let (alone, mut whole) = (run("new-alone"), run("new-again"));
    let old = OLD.iter().find(|r| dir.join(format!("tests-{r}{label}.out")).is_file()).and_then(|r| run(r))?;
    let new = run("new-related").or_else(|| run("new")).or_else(|| if alone.is_none() { whole.take() } else { None })?;
    let mut c = crate::suite::verdict(&old, &new, alone.as_ref(), whole.as_ref(), old_alone.as_ref(), unconfirmed, uneven);
    let mut held = crate::suite::tests_only(changed.iter().copied(), &old);
    if let Some(kept) = run("new-kept") {
        let back: std::collections::BTreeMap<String, String> = kept.tests.keys().map(|k| crate::suite::file_of(k)).filter_map(|f| Some((f.to_string(), crate::suite::kept_back(f)?))).collect();
        held.extend(crate::suite::held(&old, &crate::suite::unkept(&kept, &back)));
    }
    crate::suite::own_edits(&mut c, &held);
    (c["status"] == "RAN").then_some(c)
}
const FAULTS_BYTES_MAX: u64 = 16 << 20;
fn faults(dir: &Path, suites: &[(Runner, String)], stored: &Value) -> Option<Vec<Value>> {
    let mut outcomes = Vec::new();
    for (runner, suite_dir) in suites {
        let label = crate::suite::label(suite_dir);
        let path = dir.join(format!("faults{label}.json"));
        if !path.is_file() { continue; }
        if std::fs::metadata(&path).ok()?.len() > FAULTS_BYTES_MAX { return None; }
        let raw: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        let (placebo, runs) = match &raw { Value::Array(a) => (None, a.clone()), v => (Some(v["placebo"].clone()), v["faults"].as_array()?.clone()) };
        let new = ["new-related", "new"].iter().find_map(|r| report(&dir.join(format!("tests-{r}{label}.out")), *runner))?;
        outcomes.extend(crate::suite::fault_outcomes(*runner, &new, placebo.as_ref(), &runs).into_iter().zip(crate::suite::fault_claims(*runner, &new, placebo.as_ref(), &runs)));
    }
    let stored = stored.as_array()?;
    (outcomes.len() == stored.len()).then(|| stored.iter().zip(outcomes).map(|(f, (o, claim))| {
        let mut f = f.clone();
        f["outcome"] = json!(o);
        if f["kind"] == "undo" { for (k, v) in claim.as_object().into_iter().flatten() { f[k] = v.clone(); } }
        f
    }).collect())
}
const VALUES_BYTES_MAX: u64 = 4 << 20;
const TRACE_BYTES_MAX: u64 = 16 << 20;
fn readings(dir: &Path, parts: &[(String, Part)], wanted: &BTreeSet<String>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for (label, Part { runner, prefix, .. }) in parts {
        let read = |kind: &str, max: u64| -> Vec<u8> {
            let path = dir.join(format!("tests-new-kept{label}.{kind}"));
            if std::fs::metadata(&path).map_or(true, |m| m.len() > max) { return Vec::new(); }
            std::fs::read(path).unwrap_or_default()
        };
        let found = crate::proof::failures(*runner, &read("out", REPORT_BYTES_MAX), &read("values", VALUES_BYTES_MAX));
        let own: BTreeSet<String> = wanted.iter().filter_map(|k| match prefix { Some(p) => k.strip_prefix(&format!("{p}: ")).map(str::to_string), None => Some(k.clone()) }).collect();
        let named = |k: String| match prefix { Some(p) => format!("{p}: {k}"), None => k };
        out.extend(crate::suite::polarity(&found, &crate::suite::kept_back, &own).into_iter().map(|(k, v)| (named(k), v)));
    }
    out
}
pub fn rejudge(dir: &Path) -> R<Value> {
    let stored = read_json(dir.join("receipt.json"))?;
    let mut tests = stored["tests"].clone();
    let suites = runners(tests["runner"].as_str().unwrap_or(""));
    let recorded = leaves(dir);
    let mut labels = BTreeSet::new();
    for e in std::fs::read_dir(dir)?.flatten().take(ENTRIES_COUNT_MAX) {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(stem) = name.strip_prefix("tests-").and_then(|n| n.strip_suffix(".out")) else { continue };
        if let Some((r, label)) = role(stem) && (r == "new" || r == "new-related") { labels.insert(label.to_string()); }
    }
    let stored_uneven = tests["uneven_why"].as_array().is_some_and(|a| !a.is_empty());
    let setup = read_json(dir.join("setup.json")).ok();
    let parts: Vec<(String, Part)> = labels.iter().filter_map(|l| Some((l.clone(), part(l, &suites, recorded.as_ref()?)?))).collect();
    let judgeable = tests["status"] == "RAN" && !labels.is_empty() && labels.len() <= SUITES_COUNT_MAX && parts.len() == labels.len();
    let changed: Vec<(&str, &str)> = stored["rules"]["changed"].as_array().into_iter().flatten().filter_map(|c| Some((c["path"].as_str()?, c["operation"].as_str()?))).collect();
    let judged: Option<Vec<Judged>> = judgeable.then(|| parts.iter().map(|(label, Part { runner, dir: suite_dir, prefix })| {
        let mine = |k: &str| match prefix { Some(p) => k.starts_with(&format!("{p}: ")), None => suite_dir.is_empty() || k.starts_with(&format!("{suite_dir}/")) || k.starts_with(&format!("{suite_dir}: ")) };
        let unconfirmed = tests["unconfirmed"].as_array().into_iter().flatten().filter_map(Value::as_str).any(mine);
        let uneven = stored_uneven || setup.as_ref().is_some_and(|s| crate::suite::setup_differs(s, suite_dir).is_some());
        suite(dir, label, *runner, (unconfirmed, uneven), &changed).map(|verdict| Judged { verdict, runner: *runner, suite_dir: suite_dir.clone(), prefix: prefix.clone() })
    }).collect()).flatten();
    if let Some(judged) = &judged {
        let mut merged: serde_json::Map<String, Value> = crate::suite::LISTS.iter().map(|l| (l.to_string(), json!([]))).collect();
        merged.insert("problem_why".into(), json!({}));
        for j in judged { crate::suite::fold(&mut merged, &j.verdict, &|k| crate::leaf::named(j.prefix.as_deref(), j.runner, &j.suite_dir, k)); }
        let kept_runs = labels.iter().any(|label| dir.join(format!("tests-new-kept{label}.out")).is_file());
        for l in crate::suite::LISTS.iter().filter(|l| **l != "rewritten" && !(kept_runs && **l == "gone")) { tests[*l] = merged[*l].clone(); }
        tests["problem_why"] = merged["problem_why"].clone();
    }
    if let Some(f) = faults(dir, &suites, &tests["faults"]).filter(|_| recorded.as_ref().is_some_and(|r| r.is_empty())) { tests["faults"] = json!(f); }
    let mut seen = json!({});
    for label in &labels {
        let read = |side: &str| std::fs::metadata(dir.join(format!("trace-{side}{label}.jsonl"))).ok().filter(|m| m.len() <= TRACE_BYTES_MAX)
            .and_then(|_| std::fs::read_to_string(dir.join(format!("trace-{side}{label}.jsonl"))).ok());
        if let (Some(old), Some(again), Some(new)) = (read("old"), read("old-again"), read("new")) {
            let mut c = crate::trace::compare(&old, &again, &new);
            c["status"] = json!("RAN");
            c["tests"] = tests["bystanders"]["tests"].clone();
            seen = crate::trace::merged(&seen, &c);
        }
    }
    if seen["status"] == "RAN" { tests["bystanders"] = seen; }
    let wanted: BTreeSet<String> = tests["rewritten"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    if !wanted.is_empty() && recorded.is_some() { tests["polarity"] = Value::Object(readings(dir, &parts, &wanted)); }
    if tests["builds"]["status"] == "CHECKED" { tests["builds"] = crate::build::rejudge(dir, &tests["builds"], setup.as_ref()); }
    if tests["probes"]["status"] == "RAN" { tests["probes"] = crate::probe::replayed(dir, &tests["probes"], setup.as_ref(), &crate::suite::ran_suites(dir).unwrap_or_default()); }
    let d = json!({ "base": stored["base"], "head": stored["head"], "skipped": [], "unmeasured": [] });
    let receipt = crate::receipt::build(&d, &stored["rules"], &tests);
    Ok(json!({ "replayed": judged.as_ref().map_or(0, Vec::len), "suites": labels.len(), "receipt": receipt }))
}
pub const WORKERS_COUNT_MAX: usize = 4;
pub const FOLDERS_COUNT_MAX: usize = 100_000;
const ROOT_DEPTH_MAX: usize = 3;
fn is_check(dir: &Path) -> bool { dir.file_name().is_some_and(|n| n.to_string_lossy().starts_with("receipt-")) && dir.join("receipt.json").is_file() }
fn checks_under(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if is_check(dir) { out.push(dir.to_path_buf()); return; }
    if depth == 0 || out.len() >= FOLDERS_COUNT_MAX { return; }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dirs: Vec<PathBuf> = entries.flatten().take(FOLDERS_COUNT_MAX).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs { checks_under(&d, depth - 1, out); }
}
pub fn checks(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots { checks_under(root, ROOT_DEPTH_MAX, &mut out); }
    out.truncate(FOLDERS_COUNT_MAX);
    out
}
fn kinds(r: &Value, list: &str) -> Vec<String> {
    let mut k: Vec<String> = r[list].as_array().into_iter().flatten().map(|i| i["kind"].as_str().unwrap_or("?").to_string()).collect();
    k.sort();
    k
}
pub fn answer(r: &Value) -> Value {
    json!({ "decision": r["decision"].as_str().unwrap_or("?"), "blocked": kinds(r, "blocked"), "look": kinds(r, "look"), "unverified": kinds(r, "unverified"), "notes": kinds(r, "notes") })
}
pub fn judged_line(dir: &Path, shown: &str) -> Value {
    let started = std::time::Instant::now();
    let before = read_json(dir.join("receipt.json")).map(|r| answer(&r));
    let result = rejudge(dir);
    let ms = started.elapsed().as_millis() as u64;
    match (before, result) {
        (Ok(before), Ok(r)) => {
            let after = answer(&r["receipt"]);
            json!({ "receipt": shown, "before": before, "after": after, "changed": before != after, "replayed": r["replayed"], "suites": r["suites"], "ms": ms })
        }
        (Err(e), _) | (_, Err(e)) => json!({ "receipt": shown, "error": e.0, "ms": ms }),
    }
}
fn shown_name(dir: &Path) -> String {
    let parts: Vec<String> = dir.components().rev().take(2).map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    parts.into_iter().rev().collect::<Vec<_>>().join("/")
}
pub fn run(roots: &[PathBuf], workers: usize, out: &mut dyn std::io::Write) -> R<Value> {
    let started = std::time::Instant::now();
    let dirs = checks(roots);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, Value)>();
    let mut summary = json!({ "checks": dirs.len(), "judged": 0, "errors": 0, "changed": 0, "changed_decisions": 0, "slowest_ms": 0, "elapsed_secs": 0 });
    std::thread::scope(|s| {
        for _ in 0..workers.clamp(1, WORKERS_COUNT_MAX) {
            let (tx, next, dirs) = (tx.clone(), &next, &dirs);
            s.spawn(move || loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some(dir) = dirs.get(i) else { break };
                if tx.send((i, judged_line(dir, &shown_name(dir)))).is_err() { break; }
            });
        }
        drop(tx);
        let (mut pending, mut due) = (std::collections::BTreeMap::new(), 0usize);
        for (i, line) in rx {
            pending.insert(i, line);
            while let Some(line) = pending.remove(&due) {
                tally(&mut summary, &line);
                let _ = writeln!(out, "{line}");
                due += 1;
            }
        }
    });
    summary["elapsed_secs"] = json!(started.elapsed().as_secs());
    Ok(summary)
}
fn tally(summary: &mut Value, line: &Value) {
    let bump = |s: &mut Value, k: &str| s[k] = json!(s[k].as_u64().unwrap_or(0) + 1);
    if line["error"].is_string() { bump(summary, "errors"); } else { bump(summary, "judged"); }
    if line["changed"] == true { bump(summary, "changed"); }
    if line["before"]["decision"] != line["after"]["decision"] && line["after"].is_object() { bump(summary, "changed_decisions"); }
    let ms = line["ms"].as_u64().unwrap_or(0);
    if ms > summary["slowest_ms"].as_u64().unwrap_or(0) { summary["slowest_ms"] = json!(ms); summary["slowest"] = line["receipt"].clone(); }
}
