//! The project's own tests on the old and the new code, for the receipt: what passed before and still passes, what broke, what no longer
//! runs, what is newly skipped and what is new. Tests that already failed before the change are set aside: the change did not cause them.
//! Each side runs on its installed tree from `drive::setup`, read-only and with the network off (`__drive_test`).
use crate::policy::{Edits, Kind, Limits, Operator, Runner, Tests};
use crate::proof::{self, Report};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

/// The test folders `kern init` knows, wherever they are in the tree.
const FOLDERS: [&str; 4] = ["test", "tests", "__tests__", "spec"];
/// How long one run of a project's tests may take.
const TIMEOUT_SECONDS: u64 = 900;

/// The project's test runner and command, from its package.json: vitest, jest, mocha or `node --test`. None when it uses another one,
/// or a script Kern cannot read the results of (turbo, nx): the receipt then says the tests did not run, rather than guessing.
pub fn detect(package: Option<&[u8]>) -> Option<Tests> {
    let (name, command) = crate::init::runner(package)?;
    Some(Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::parse(name)?, command, edits: Edits::Checked })
}

/// Which of these paths are test files, as grants the test proof understands: the test folders (at any depth, as folder grants) and files
/// named like tests (`*.test.*`, `*.spec.*`).
pub fn test_paths(paths: &[&str]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for p in paths {
        let parts: Vec<&str> = p.split('/').collect();
        if let Some(i) = parts[..parts.len().saturating_sub(1)].iter().position(|d| FOLDERS.contains(d)) { out.insert(format!("{}/", parts[..=i].join("/"))); continue; }
        let name = parts.last().copied().unwrap_or("");
        if name.contains(".test.") || name.contains(".spec.") { out.insert(p.to_string()); }
    }
    out.into_iter().collect()
}

fn failing(r: &Report, k: &str) -> bool { r.tests.get(k).is_some_and(|c| c.failed > 0) }
fn passing(r: &Report, k: &str) -> bool { r.tests.get(k).is_some_and(|c| c.failed == 0 && c.passed > 0) }
fn skipped_only(r: &Report, k: &str) -> bool { r.tests.get(k).is_some_and(|c| c.passed == 0 && c.failed == 0 && c.skipped > 0) }
fn ran(r: &Report, k: &str) -> bool { r.tests.get(k).is_some_and(|c| c.passed + c.failed > 0) }
fn sum(r: &Report) -> Value {
    let (p, f, s) = r.tests.values().fold((0, 0, 0), |(p, f, s), c| (p + c.passed, f + c.failed, s + c.skipped));
    json!({ "passed": p, "failed": f, "skipped": s })
}

/// The old code's results against the new code's. With a second run of the new code (made when the first one failed), a test counts as
/// broken only when it failed both times; failing once is flaky.
pub fn compare(before: &Report, after: &Report, again: Option<&Report>) -> Value {
    let keys = |r: &Report| -> BTreeSet<String> { r.tests.keys().cloned().collect() };
    let (old, new) = (keys(before), keys(after));
    let mut broke = Vec::new();
    let mut flaky = Vec::new();
    let mut kept = 0u64;
    for k in &old {
        if !passing(before, k) { continue; }
        if passing(after, k) { kept += 1; continue; }
        if !failing(after, k) { continue; }
        match again {
            Some(a) if passing(a, k) => { flaky.push(k.clone()); kept += 1; }
            _ => broke.push(k.clone()),
        }
    }
    let gone: Vec<&String> = old.iter().filter(|k| ran(before, k) && !new.contains(*k)).collect();
    let newly_skipped: Vec<&String> = old.iter().filter(|k| passing(before, k) && skipped_only(after, k)).collect();
    let added: Vec<&String> = new.iter().filter(|k| !old.contains(*k)).collect();
    let already_failing: Vec<&String> = old.iter().filter(|k| failing(before, k)).chain(before.problems.iter()).collect();
    // A failure outside any test (a test file that no longer loads, a failing hook) that the old code did not have: on both runs, if two.
    let new_problems: Vec<&String> = after.problems.iter().filter(|p| !before.problems.contains(*p) && again.is_none_or(|a| a.problems.contains(*p))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(again.unwrap_or(after)), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
        "newly_skipped": newly_skipped, "added": added, "already_failing": already_failing, "new_problems": new_problems })
}

/// One run of a side's tests in its sandbox: the runner's report, or why there is none.
fn run_side(out: &Path, op: &Operator, volume: &str, tests: &Tests, label: &str) -> R<Result<Report, String>> {
    let control = out.join(format!("tests-{label}"));
    if control.exists() { std::fs::remove_dir_all(&control)?; }
    crate::sandbox::control_dir(&control, Some(("drive-test.json", &json!({ "version": 1, "command": proof::command(tests), "timeout_seconds": TIMEOUT_SECONDS }))))?;
    let input = out.join("tests-input");
    std::fs::create_dir_all(&input)?;
    let limits = Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: TIMEOUT_SECONDS + 120, memory_mb: 4096, pids: 1024 };
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
    Ok(proof::read_report(tests.runner, &parts["output"]))
}

/// Runs the project's tests on the old and the new code, each on its installed tree (`drive::setup` first if needed), and compares them.
/// The runner comes from each side's package.json; a side without one, or a run without a readable report, is named in the result.
pub fn run(out: &Path, op: &Operator) -> R<Value> {
    let d = read_json(out.join("drive.json"))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let package = |commit: &str| -> Option<Vec<u8>> { crate::git::git(repo, &["show", &format!("{commit}:package.json")], None).ok() };
    let (old_tests, new_tests) = (detect(package(d["base"].as_str().unwrap_or("")).as_deref()), detect(package(d["head"].as_str().unwrap_or("")).as_deref()));
    let Some(new_tests) = new_tests else {
        return Ok(json!({ "status": "NOT_RUN", "why": "no test runner Kern can read was found in package.json (Kern runs vitest, jest, mocha and node --test)" }));
    };
    if !out.join("setup.json").exists() { crate::drive::setup(out, op)?; }
    let s = read_json(out.join("setup.json"))?;
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    let after = match run_side(out, op, &volume("new"), &new_tests, "new")? {
        Ok(r) => r,
        Err(why) => return Ok(json!({ "status": "NOT_RUN", "runner": new_tests.runner.name(), "why": format!("the tests on the new code gave no results: {why}") })),
    };
    let Some(old_tests) = old_tests else {
        // Tests the change itself brings: nothing to compare against, so they are all new.
        let mut c = compare(&Report::default(), &after, None);
        c["runner"] = json!(new_tests.runner.name());
        c["note"] = json!("the old code had no test runner Kern can read, so every test is new");
        return Ok(c);
    };
    let before = match run_side(out, op, &volume("old"), &old_tests, "old")? {
        Ok(r) => r,
        Err(why) => return Ok(json!({ "status": "NOT_RUN", "runner": new_tests.runner.name(), "why": format!("the tests on the old code gave no results: {why}") })),
    };
    // A failure the old code did not have is run once more before it counts, so a flaky test is told apart from a broken one.
    let fails_new = after.tests.iter().any(|(k, c)| c.failed > 0 && passing(&before, k)) || after.problems.iter().any(|p| !before.problems.contains(p));
    let again = if fails_new { run_side(out, op, &volume("new"), &new_tests, "new-again")?.ok() } else { None };
    let mut c = compare(&before, &after, again.as_ref());
    c["runner"] = json!(new_tests.runner.name());
    Ok(c)
}
