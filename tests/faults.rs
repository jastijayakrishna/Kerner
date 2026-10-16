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
    let back = kern::suite::faults(&file("export const f = (a) => a >= 1;\n", "export const f = (a) => a > 1;\n"), 6);
    assert!(back.is_empty(), "turning > back into >= is the old code, not a fault: {:?}", back.iter().map(|m| m.describe()).collect::<Vec<_>>());
    let other = kern::suite::faults(&file("export const f = (a) => (a >= 1 ? 1 : 0);\n", "export const f = (a) => (a > 1 ? 1 : 0);\n"), 6);
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
    assert_eq!(fault_verdict(Jest, &clean, (true, false), &report(&[("rounds", "passed"), ("discounts", "failed")], false), b""), "unknown",
        "when the changed file was never loaded with the fault in it, a failure is not the fault's doing");
    assert_eq!(fault_verdict(Jest, &clean, (true, false), &report(&[("rounds", "passed"), ("discounts", "passed")], false), b""), "unknown");
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
    assert!(receipt::markdown(&none, None).contains("the tests caught 0 of 3 small faults"));
    let some = receipt::build(&d, &rules, &tests(&["caught", "missed", "caught"]), &off);
    assert!(!some["look"].as_array().unwrap().iter().any(|x| x["kind"] == "untested-change"));
    let md = receipt::markdown(&some, None);
    assert!(md.contains("caught 2 of 3") && md.contains("src/price.js:3"), "{md}");
}
