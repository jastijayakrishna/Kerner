use kern::proof::FaultKind;
use kern::receipt;
use serde_json::json;
use std::collections::BTreeSet;

#[test]
fn faults_are_planted_only_in_the_lines_a_change_touches() {
    let old = "export function discount(amount) {\n  return amount >= 100 ? amount * 0.9 : amount;\n}\nexport const total = (items) => items.reduce((s, i) => s + (i.price > 0 ? i.price : 0), 0);\n";
    let new = "export function discount(amount) {\n  return amount > 150 ? amount * 0.8 : amount;\n}\nexport const total = (items) => items.reduce((s, i) => s + (i.price > 0 ? i.price : 0), 0);\n";
    let planted = kern::suite::faults(&[("src/price.js".to_string(), old.to_string(), new.to_string())], 6);
    assert!(!planted.is_empty());
    assert!(planted.iter().all(|m| m.line == 2), "only the changed line 2 gets faults: {:?}", planted.iter().map(|m| m.describe()).collect::<Vec<_>>());
}

#[test]
fn faults_are_planted_only_in_product_code() {
    for p in [".github/pr-webhook-utils.cjs", "scripts/post-evalite-results.ts", "apps/examples/src/flex.tsx", "docs/app.js", "vite.config.ts", "src/button.stories.tsx", "e2e/login.ts"] {
        assert!(!kern::suite::product_code(p), "{p}");
    }
    for p in ["src/auth/provider.ts", "packages/core/src/index.ts", "app/editor/menus/block.tsx", "lib/total.js"] { assert!(kern::suite::product_code(p), "{p}"); }
}

#[test]
fn a_fault_that_only_puts_back_the_old_line_is_not_planted() {
    let file = |old: &str, new: &str| vec![("src/a.js".to_string(), old.to_string(), new.to_string())];
    let operators = |planted: Vec<kern::proof::Mutant>| planted.into_iter().filter(|m| m.kind == FaultKind::Operator).collect::<Vec<_>>();
    let back = operators(kern::suite::faults(&file("export const f = (a) => a >= 1;\n", "export const f = (a) => a > 1;\n"), 6));
    assert!(back.is_empty(), "turning > back into >= is the old code, which the undo fault already is: {:?}", back.iter().map(|m| m.describe()).collect::<Vec<_>>());
    let other = operators(kern::suite::faults(&file("export const f = (a) => (a >= 1 ? 1 : 0);\n", "export const f = (a) => (a > 1 ? 1 : 0);\n"), 6));
    assert_eq!(other.iter().map(|m| m.what().to_string()).collect::<Vec<_>>(), ["negates the condition"], "the next fault on the line is planted instead");
}

#[test]
fn faults_skip_logging_calls() {
    let file = vec![("src/a.js".to_string(), "export function f() {\n  return 1;\n}\n".to_string(), "export function f() {\n  logger.info('start');\n  debug('x');\n  return 1;\n}\n".to_string())];
    assert!(kern::suite::faults(&file, 6).is_empty());
}

#[test]
fn faults_spread_across_changed_files_and_stop_at_the_limit() {
    let file = |n: usize| (format!("src/f{n}.js"), "export const f = (a) => a;\n".to_string(), "export const f = (a) => a > 1 ? a + 1 : a - 1;\n".to_string());
    let planted = kern::suite::faults(&(0..10).map(file).collect::<Vec<_>>(), 6);
    assert_eq!(planted.len(), 6);
    let files: BTreeSet<&str> = planted.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(files.len(), 6, "one per file before a second in any");
}

fn applied(m: &kern::proof::Mutant, new: &str) -> String { format!("{}{}{}", &new[..m.start], m.replacement, &new[m.end..]) }

#[test]
fn undoing_the_change_comes_first_in_every_changed_file_and_gives_back_the_old_code() {
    let zustand_old = "export const persist = (config) => {\n  let hydrating = false;\n  const clear = () => storage.removeItem(name);\n  return { clear, hydrating };\n};\n";
    let zustand_new = "export const persist = (config) => {\n  let hydrating = false;\n  let version = 0;\n  const clear = () => { version += 1; storage.removeItem(name); };\n  return { clear, hydrating, current: () => version > 0 };\n};\n";
    let other_old = "export const f = (a) => a + 1;\n";
    let other_new = "export const f = (a) => (a > 2 ? a + 1 : a);\n";
    let files = vec![("src/middleware/persist.ts".to_string(), zustand_old.to_string(), zustand_new.to_string()), ("src/f.ts".to_string(), other_old.to_string(), other_new.to_string())];
    let planted = kern::suite::faults(&files, 24);
    let kinds: Vec<(&str, FaultKind)> = planted.iter().map(|m| (m.path.as_str(), m.kind)).collect();
    assert_eq!(&kinds[..2], [("src/middleware/persist.ts", FaultKind::Undo), ("src/f.ts", FaultKind::Undo)], "pmndrs/zustand#3555 was reverted with its tests passing: first ask whether any test can tell the old code from the new: {kinds:?}");
    assert!(kinds[2..].iter().all(|(_, k)| *k == FaultKind::Operator) && kinds.len() > 2, "{kinds:?}");
    assert_eq!(applied(&planted[0], zustand_new), zustand_old);
    assert_eq!(applied(&planted[1], other_new), other_old);
    assert_eq!(planted[0].line, 3, "the undo starts at the first changed line");
    assert_eq!(planted[0].what(), "undoes the change");
    assert!(planted.iter().filter(|m| m.path == "src/f.ts" && m.kind == FaultKind::Operator).count() <= 3, "at most three small faults per file, spread over its changed lines");
}

#[test]
fn nothing_is_undone_when_only_comments_formatting_or_logging_changed() {
    let old = "export function total(items) {\n  return items.reduce((s, i) => s + i.price, 0);\n}\n";
    for (why, new) in [
        ("a comment", "// adds up the prices\nexport function total(items) {\n  return items.reduce((s, i) => s + i.price, 0);\n}\n"),
        ("formatting", "export function total(items) {\n  return items.reduce(\n    (s, i) => s + i.price,\n    0,\n  );\n}\n"),
        ("a log line", "export function total(items) {\n  console.debug('total', items.length);\n  return items.reduce((s, i) => s + i.price, 0);\n}\n"),
    ] {
        let planted = kern::suite::faults(&[("src/total.js".to_string(), old.to_string(), new.to_string())], 24);
        assert!(planted.iter().all(|m| m.kind != FaultKind::Undo), "{why}: no behaviour changed, so undoing it proves nothing");
    }
    let real = "export function total(items) {\n  return items.reduce((s, i) => s + i.price * i.count, 0);\n}\n";
    assert!(kern::suite::faults(&[("src/total.js".to_string(), old.to_string(), real.to_string())], 24).iter().any(|m| m.kind == FaultKind::Undo));
}

#[test]
fn a_change_too_big_to_diff_line_by_line_still_gets_faults() {
    let old: String = (0..1200).map(|i| format!("export const v{i} = {i};\n")).collect();
    let new: String = (0..1200).map(|i| format!("export const v{i} = {i} > 1 ? {i} : 0;\n")).collect();
    let (removed, added) = kern::compare::changed_lines(&old.lines().collect::<Vec<_>>(), &new.lines().collect::<Vec<_>>());
    assert_eq!((removed.len(), added.len()), (1200, 1200), "a diff past the step limit counts every line in the changed middle, never none");
    let planted = kern::suite::faults(&[("src/consts.js".to_string(), old.clone(), new.clone())], 24);
    let lines: BTreeSet<usize> = planted.iter().filter(|m| m.kind == FaultKind::Operator).map(|m| m.line).collect();
    assert!(lines.len() >= 2 && lines.first() < Some(&100) && lines.last() > Some(&1100), "faults go where the change is, spread across it: {lines:?}");
}

fn jest(tests: &[(&str, &str)]) -> String {
    let results: Vec<serde_json::Value> = tests.iter().map(|(t, s)| json!({ "ancestorTitles": ["price"], "title": t, "status": s, "failureMessages": [] })).collect();
    json!({ "numTotalTests": tests.len(), "testResults": [{ "name": "/workspace/a.test.js", "status": "passed", "assertionResults": results }] }).to_string()
}

#[test]
fn a_test_that_fails_with_nothing_changed_proves_nothing_about_a_fault() {
    use kern::policy::Runner::Jest;
    let new = kern::proof::read_report(Jest, jest(&[("rounds", "passed"), ("discounts", "passed")]).as_bytes()).unwrap();
    let run = |tests: &[(&str, &str)]| json!({ "finished": true, "served": true, "output": jest(tests), "errors": "" });
    let faults = [run(&[("rounds", "passed"), ("discounts", "failed")]), run(&[("rounds", "failed"), ("discounts", "passed")])];
    assert_eq!(kern::suite::fault_outcomes(Jest, &new, None, &faults), ["caught", "caught"], "stored runs without a control are judged as before");
    let placebo = run(&[("rounds", "passed"), ("discounts", "failed")]);
    assert_eq!(kern::suite::fault_outcomes(Jest, &new, Some(&placebo), &faults), ["missed", "caught"],
        "discounts fails under Kern's fault machinery even when the served file is unchanged, so its failure under a fault is not the fault's doing: a test that looks for Kern, or a flaky one, cannot catch a fault");
    let unfinished = json!({ "finished": false, "served": false, "output": "", "errors": "" });
    assert_eq!(kern::suite::fault_outcomes(Jest, &new, Some(&unfinished), &faults), ["unknown", "unknown"], "without a control run, a caught fault is not evidence");
    let missed = [run(&[("rounds", "passed"), ("discounts", "passed")])];
    assert_eq!(kern::suite::fault_outcomes(Jest, &new, Some(&unfinished), &missed), ["missed"], "a fault no test noticed needs no control");
}

#[test]
fn a_changed_file_kern_put_no_fault_in_is_named_with_why() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 3, "failed": 0, "skipped": 0 }, "after": { "passed": 3, "failed": 0, "skipped": 0 },
        "kept": 3, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [],
        "faults": [{ "path": "packages/backend/src/queryBuilder.ts", "line": 347, "what": "changes > to >=", "kind": "operator", "outcome": "caught" }],
        "faults_missing": [{ "path": "packages/backend/src/services/ProjectService/ProjectService.ts", "why": "Kern could not pick the tests related to this file" }] });
    let md = receipt::markdown(&receipt::build(&d, &rules, &tests, &json!({ "status": "OFF", "why": "no AI key" })), None);
    assert!(md.contains("No fault planted in") && md.contains("ProjectService.ts") && md.contains("Kern could not pick the tests related to this file"),
        "lightdash/lightdash#12942 was reverted after its bug landed in ProjectService.ts, a changed file Kern put no fault in and said nothing about: {md}");
}

#[test]
fn a_fault_counts_as_caught_only_when_a_test_that_passed_without_it_fails() {
    use kern::policy::Runner::Jest;
    use kern::proof::{fault_verdict, read_report};
    let report = |tests: &[(&str, &str)], broken_file: bool| {
        let results: Vec<serde_json::Value> = tests.iter().map(|(t, s)| json!({ "ancestorTitles": ["price"], "title": t, "status": s, "failureMessages": [] })).collect();
        let mut files = vec![json!({ "name": "/workspace/a.test.js", "status": "passed", "assertionResults": results })];
        if broken_file { files.push(json!({ "name": "/workspace/b.test.js", "status": "failed", "message": "SyntaxError: Unexpected token", "assertionResults": [] })); }
        json!({ "numTotalTests": tests.len(), "testResults": files }).to_string().into_bytes()
    };
    let clean = read_report(Jest, &report(&[("rounds", "passed"), ("discounts", "passed")], false)).unwrap();
    let already = read_report(Jest, &report(&[("rounds", "passed"), ("discounts", "failed")], false)).unwrap();
    let served = (true, true);
    assert_eq!(fault_verdict(Jest, &clean, served, &report(&[("rounds", "passed"), ("discounts", "failed")], false), b""), "caught");
    assert_eq!(fault_verdict(Jest, &clean, served, &report(&[("rounds", "passed"), ("discounts", "passed")], false), b""), "missed");
    assert_eq!(fault_verdict(Jest, &clean, served, &report(&[("rounds", "passed")], true), b""), "caught", "a fault that stops a test file loading is caught too");
    assert_eq!(fault_verdict(Jest, &already, served, &report(&[("rounds", "passed"), ("discounts", "failed")], false), b""), "missed",
        "a test that already failed without the fault proves nothing, and the test that still works did not notice it (formbricks had 237 tests failing already)");
    assert_eq!(fault_verdict(Jest, &already, served, &report(&[("rounds", "failed"), ("discounts", "failed")], false), b""), "caught", "a working test failed");
    assert_eq!(fault_verdict(Jest, &already, served, &report(&[("discounts", "failed")], false), b""), "unknown", "only broken tests ran, so nothing can be said");
    assert_eq!(fault_verdict(Jest, &clean, (true, false), &report(&[("rounds", "passed"), ("discounts", "failed")], false), b""), "unreached",
        "when the changed file was never loaded with the fault in it, a failure is not the fault's doing, and no test reached the change");
    assert_eq!(fault_verdict(Jest, &clean, (true, false), &report(&[("rounds", "passed"), ("discounts", "passed")], false), b""), "unreached");
    assert_eq!(fault_verdict(Jest, &clean, (false, true), &report(&[("rounds", "passed")], false), b""), "unknown", "a run that did not finish proves nothing");
    assert_eq!(fault_verdict(Jest, &clean, served, b"", b"No tests found, exiting with code 1
Run with `--passWithNoTests` to exit with code 0"), "untested");
    assert_eq!(fault_verdict(Jest, &clean, (true, false), &report(&[], false), b""), "untested", "no test reaches the file at all");
    assert_eq!(fault_verdict(Jest, &clean, served, b"", b"Error: Cannot find module 'x'"), "unknown");
}

#[test]
fn faults_no_test_noticed_are_named_and_a_change_no_test_checks_needs_a_look() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = |outcomes: &[&str]| json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 3, "failed": 0, "skipped": 0 }, "after": { "passed": 3, "failed": 0, "skipped": 0 },
        "kept": 3, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [],
        "faults": outcomes.iter().enumerate().map(|(i, o)| json!({ "path": "src/price.js", "line": i + 2, "what": "changes > to >=", "outcome": o })).collect::<Vec<_>>() });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let none = receipt::build(&d, &rules, &tests(&["missed", "untested", "missed"]), &off);
    assert_eq!(none["decision"], "clear", "no test checking a change is shown, not a look: 14 of 118 real pull requests changed code no test reaches, many of them CI scripts and examples: {none}");
    assert!(receipt::markdown(&none, None).contains("the tests caught 0 of 3 faults"));
    let some = receipt::build(&d, &rules, &tests(&["caught", "missed", "caught"]), &off);
    assert!(!some["look"].as_array().unwrap().iter().any(|x| x["kind"] == "untested-change"));
    let md = receipt::markdown(&some, None);
    assert!(md.contains("caught 2 of 3") && md.contains("src/price.js:3"), "{md}");
}
