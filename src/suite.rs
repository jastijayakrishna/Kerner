use crate::policy::{Edits, Kind, Limits, Operator, Runner, Tests};
use crate::proof::{self, Report};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const FOLDERS: [&str; 11] = ["test", "tests", "__tests__", "spec", "specs", "e2e", "__e2e__", "cypress", "playwright", "__mocks__", "__fixtures__"];
const NAMES: [&str; 4] = [".test.", ".spec.", ".e2e.", ".cy."];
const TIMEOUT_SECONDS: u64 = 900;

pub fn detect(package: Option<&[u8]>) -> Option<Tests> {
    let (name, command) = crate::init::runner(package)?;
    Some(Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::parse(name)?, command, edits: Edits::Checked })
}

fn test_grant(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    if let Some(i) = lower[..lower.len().saturating_sub(1)].iter().position(|d| FOLDERS.contains(&d.as_str())) { return Some(format!("{}/", parts[..=i].join("/"))); }
    let name = lower.last()?;
    NAMES.iter().any(|m| name.contains(m)).then(|| path.to_string())
}

pub fn is_test_path(path: &str) -> bool { test_grant(path).is_some() }

pub fn test_paths(paths: &[&str]) -> Vec<String> { paths.iter().filter_map(|p| test_grant(p)).collect::<BTreeSet<_>>().into_iter().collect() }
fn sum(r: &Report) -> Value {
    let (p, f, s) = r.tests.values().fold((0, 0, 0), |(p, f, s), c| (p + c.passed, f + c.failed, s + c.skipped));
    json!({ "passed": p, "failed": f, "skipped": s })
}

fn count(r: &Report, k: &str) -> proof::Count { r.tests.get(k).copied().unwrap_or_default() }

pub fn compare(before: &Report, after: &Report, again: Option<&Report>) -> Value {
    let keys: BTreeSet<&String> = before.tests.keys().chain(after.tests.keys()).collect();
    let (mut kept, mut broke, mut flaky, mut gone, mut newly_skipped, mut added, mut already_failing) = (0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for k in keys {
        let (b, a) = (count(before, k), count(after, k));
        if b.total() == 0 { added.push(k.clone()); continue; }
        if b.failed > 0 { already_failing.push(k.clone()); }
        kept += u64::from(b.passed.min(a.passed));
        if b.passed > 0 && a.failed > b.failed {
            match again.map(|g| count(g, k)) {
                Some(g) if g.failed <= b.failed && g.passed >= b.passed => { flaky.push(k.clone()); kept += u64::from(b.passed - b.passed.min(a.passed)); }
                _ => broke.push(k.clone()),
            }
        } else if b.passed > 0 && a.skipped > b.skipped {
            newly_skipped.push(k.clone());
        } else if b.ran() > 0 && a.total() < b.total() {
            gone.push(k.clone());
        }
    }
    already_failing.extend(before.problems.iter().cloned());
    let new_problems: Vec<&String> = after.problems.iter().filter(|p| !before.problems.contains(*p) && again.is_none_or(|a| a.problems.contains(*p))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(again.unwrap_or(after)), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
        "newly_skipped": newly_skipped, "added": added, "already_failing": already_failing, "new_problems": new_problems })
}

pub fn no_results(why: &str, ending: &[u8], errors: &[u8], output: &[u8]) -> String {
    let text = |b: &[u8]| -> String {
        let s = String::from_utf8_lossy(b);
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' { if chars.peek() == Some(&'[') { chars.next(); for d in chars.by_ref() { if d.is_ascii_alphabetic() { break; } } } continue; }
            out.push(if c.is_control() && c != '\n' { ' ' } else { c });
        }
        out
    };
    let source = if errors.iter().any(|b| !b.is_ascii_whitespace()) { text(errors) } else { text(output) };
    let last: Vec<&str> = source.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let from = last.iter().rposition(|l| !l.starts_with("at ")).unwrap_or(0).saturating_sub(1);
    let tail = last[from..].iter().take(3).copied().collect::<Vec<_>>().join(" / ");
    let mut s = format!("{why} ({})", String::from_utf8_lossy(ending).trim());
    if !tail.is_empty() { s += &format!("; the last lines it printed: {tail}"); }
    if s.chars().count() > 600 { s = format!("{}…", s.chars().take(599).collect::<String>()); }
    s
}

fn run_side(out: &Path, op: &Operator, volume: &str, tests: &Tests, dir: &str, label: &str) -> R<Result<Report, String>> {
    let control = out.join(format!("tests-{label}"));
    if control.exists() { std::fs::remove_dir_all(&control)?; }
    crate::sandbox::control_dir(&control, Some(("drive-test.json", &json!({ "version": 1, "command": proof::command(tests), "timeout_seconds": TIMEOUT_SECONDS, "cwd": dir }))))?;
    let input = out.join("tests-input");
    std::fs::create_dir_all(&input)?;
    let limits = Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: TIMEOUT_SECONDS + 120, memory_mb: test_memory_mb(std::fs::read_to_string("/proc/meminfo").ok().as_deref()), pids: 4096 };
    let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &op.image, snapshot: &input, control: &control, writable: &[], limits: &limits, entry: vec!["__drive_test".into()],
        network: "none", volumes: vec![], root: false, home_mb: 0, scratch: false, workspace: Some((volume, true)) });
    let _ = std::fs::remove_dir_all(&control);
    let run = match run {
        Ok(r) => r,
        Err(e) if e.0.contains("exceeded limits") || e.0.contains("timed out") => return Ok(Err(format!("the run was stopped: {}", e.0))),
        Err(e) => return Err(e),
    };
    let parts = match crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) { Ok(p) => p, Err(e) => return Ok(Err(e.0)) };
    std::fs::write(out.join(format!("tests-{label}.out")), &parts["output"])?;
    std::fs::write(out.join(format!("tests-{label}.err")), &parts["errors"])?;
    if parts["ending"] == b"timeout" { return Ok(Err(format!("the tests did not finish in {} minutes", TIMEOUT_SECONDS / 60))); }
    Ok(proof::read_report(tests.runner, &parts["output"]).map_err(|why| no_results(&why, &parts["ending"], &parts["errors"], &parts["output"])))
}

const MAX_PACKAGES: usize = 5;

pub fn test_memory_mb(meminfo: Option<&str>) -> u64 {
    let total_kb = meminfo.and_then(|m| m.lines().find_map(|l| l.strip_prefix("MemTotal:"))).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok());
    total_kb.map_or(4096, |kb| (kb / 1024 * 3 / 4).clamp(4096, 12288))
}

pub fn packages(changed: &[&str], has_manifest: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = BTreeSet::new();
    for p in changed.iter().filter(|p| !p.split('/').any(|c| c == "node_modules")) {
        let mut dir = p.rsplit_once('/').map(|(d, _)| d);
        while let Some(d) = dir {
            if has_manifest(d) { out.insert(d.to_string()); break; }
            dir = d.rsplit_once('/').map(|(x, _)| x);
        }
    }
    out.into_iter().collect()
}

struct Suite { dir: String, old: Option<Tests>, new: Tests }

fn absorb(into: &mut Report, from: Report, dir: &str, runner: Runner) {
    let name = |k: String| if runner == Runner::Node && !dir.is_empty() { format!("{dir}: {k}") } else { k };
    for (k, c) in from.tests {
        let e = into.tests.entry(name(k)).or_default();
        e.passed += c.passed;
        e.failed += c.failed;
        e.skipped += c.skipped;
    }
    into.problems.extend(from.problems.into_iter().map(name));
}

pub fn run(out: &Path, op: &Operator) -> R<Value> {
    let d = read_json(out.join("drive.json"))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let (base, head) = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let manifest = |commit: &str, dir: &str| -> Option<Vec<u8>> {
        let path = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
        crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok()
    };
    let mut not_run = Vec::new();
    let suites: Vec<Suite> = match detect(manifest(head, "").as_deref()) {
        Some(new) => vec![Suite { dir: String::new(), old: detect(manifest(base, "").as_deref()), new }],
        None => {
            let changed = crate::rules::changed(repo, base, head)?;
            let paths: Vec<&str> = changed.iter().map(|(p, _)| p.as_str()).collect();
            let mut list = Vec::new();
            for dir in packages(&paths, |dir| manifest(head, dir).is_some() || manifest(base, dir).is_some()) {
                let Some(new) = detect(manifest(head, &dir).as_deref()) else { continue };
                if list.len() == MAX_PACKAGES { not_run.push(json!({ "dir": dir, "why": format!("Kern runs the tests of at most {MAX_PACKAGES} packages per change") })); continue; }
                list.push(Suite { old: detect(manifest(base, &dir).as_deref()), dir, new });
            }
            list
        }
    };
    if suites.is_empty() {
        return Ok(json!({ "status": "NOT_RUN", "why": "no test runner Kern can read was found in package.json, at the root or in a package this change touches (Kern runs vitest, jest, mocha and node --test)" }));
    }
    if !out.join("setup.json").exists() { crate::drive::setup(out, op)?; }
    let s = read_json(out.join("setup.json"))?;
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    let (mut before, mut after, mut again) = (Report::default(), Report::default(), Report::default());
    let (mut ran, mut rerun, mut all_new) = (Vec::new(), false, Vec::new());
    for suite in &suites {
        let label = if suite.dir.is_empty() { String::new() } else { format!("-{}", suite.dir.replace('/', "_")) };
        let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
        let new = match run_side(out, op, &volume("new"), &suite.new, &suite.dir, &format!("new{label}"))? {
            Ok(r) => r,
            Err(why) => { not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the new code gave no results: {why}") })); continue; }
        };
        let old = match &suite.old {
            None => { all_new.push(if suite.dir.is_empty() { ".".to_string() } else { suite.dir.clone() }); Report::default() }
            Some(t) => match run_side(out, op, &volume("old"), t, &suite.dir, &format!("old{label}"))? {
                Ok(r) => r,
                Err(why) => { not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the old code gave no results: {why}") })); continue; }
            },
        };
        let fails_new = new.tests.iter().any(|(k, c)| { let b = count(&old, k); b.passed > 0 && c.failed > b.failed }) || new.problems.iter().any(|p| !old.problems.contains(p));
        let second = if fails_new { run_side(out, op, &volume("new"), &suite.new, &suite.dir, &format!("new-again{label}"))?.ok() } else { None };
        rerun |= second.is_some();
        absorb(&mut again, second.unwrap_or_else(|| new.clone()), &suite.dir, suite.new.runner);
        absorb(&mut before, old, &suite.dir, suite.new.runner);
        absorb(&mut after, new, &suite.dir, suite.new.runner);
        ran.push(if suite.dir.is_empty() { suite.new.runner.name().to_string() } else { format!("{} in {}", suite.new.runner.name(), suite.dir) });
    }
    if ran.is_empty() {
        let why = not_run.first().and_then(|n| n["why"].as_str()).unwrap_or("no test run gave results").to_string();
        return Ok(json!({ "status": "NOT_RUN", "runner": suites[0].new.runner.name(), "why": why, "not_run": not_run }));
    }
    let mut c = compare(&before, &after, rerun.then_some(&again));
    c["runner"] = json!(ran.join(", "));
    c["not_run"] = json!(not_run);
    if !all_new.is_empty() { c["note"] = json!(format!("the old code had no test runner Kern can read in {}, so those tests are all new", all_new.join(", "))); }
    Ok(c)
}
