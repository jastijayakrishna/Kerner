use kern::policy::Runner;
use kern::proof::{Count, Report, read_report};
use kern::{receipt, suite};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect(), why: BTreeMap::new() }
}
fn names(c: &Value, key: &str) -> Vec<String> {
    c[key].as_array().unwrap_or_else(|| panic!("{key}: {c}")).iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()
}
fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn rules_for(changed: &[(&str, &str)]) -> Value {
    json!({ "changed": changed.iter().map(|(p, op)| json!({ "path": p, "operation": op })).collect::<Vec<_>>(), "powers": [], "tricks": [], "settings": [], "protected": [],
        "dependencies": [], "agent_notes": [], "not_checked": [] })
}
fn receipt_for(tests: &Value, changed: &[(&str, &str)]) -> Value { receipt::build(&drive_json(), &rules_for(changed), tests) }
fn decision(tests: &Value, changed: &[(&str, &str)]) -> String { receipt_for(tests, changed)["decision"].as_str().unwrap_or("").to_string() }

fn test_point(n: usize, name: &str) -> String { format!("# Subtest: {name}\nok {n} - {name}\n  ---\n  duration_ms: 0.5\n  type: 'test'\n  ...\n") }
fn suite_point(n: usize, name: &str, file: &str, tests: &[&str], after_fails: Option<&str>) -> String {
    let mut s = format!("# Subtest: {name}\n");
    for (i, t) in tests.iter().enumerate() { s += &format!("    # Subtest: {t}\n    ok {} - {t}\n      ---\n      duration_ms: 0.6\n      type: 'test'\n      ...\n", i + 1); }
    s += &format!("    1..{}\n", tests.len());
    match after_fails {
        None => s += &format!("ok {n} - {name}\n  ---\n  duration_ms: 2.1\n  type: 'suite'\n  ...\n"),
        Some(error) => s += &format!("not ok {n} - {name}\n  ---\n  duration_ms: 2.1\n  type: 'suite'\n  location: '/workspace/{file}:3:1'\n  failureType: 'hookFailed'\n  error: '{error}'\n  code: 'ERR_TEST_FAILURE'\n  ...\n"),
    }
    s
}
fn file_point(n: usize, file: &str) -> String {
    format!("# Subtest: {file}\nnot ok {n} - {file}\n  ---\n  duration_ms: 23.9\n  type: 'test'\n  location: '/workspace/{file}:1:1'\n  failureType: 'testCodeFailure'\n  exitCode: 1\n  signal: ~\n  error: 'test failed'\n  code: 'ERR_TEST_FAILURE'\n  ...\n")
}
fn tap(points: &[String]) -> Report { read_report(Runner::Node, format!("TAP version 13\n{}1..{}\n", points.concat(), points.len()).as_bytes()).unwrap() }

fn mocha(passes: &[(&str, &str)], hooks: &[(&str, &str, &str)]) -> Report {
    let test = |(file, full): &(&str, &str)| json!({ "title": full, "fullTitle": full, "file": format!("/workspace/{file}"), "duration": 1, "currentRetry": 0, "speed": "fast", "err": {} });
    let hook = |(file, full, message): &(&str, &str, &str)| json!({ "title": full, "fullTitle": full, "file": format!("/workspace/{file}"), "duration": 2, "currentRetry": 0,
        "err": { "message": message, "stack": format!("Error: {message}\n    at Context.<anonymous> (/workspace/{file}:4:11)") } });
    let v = json!({ "stats": { "suites": 2, "tests": passes.len(), "passes": passes.len(), "pending": 0, "failures": hooks.len() },
        "tests": passes.iter().map(test).collect::<Vec<_>>(), "pending": [], "failures": hooks.iter().map(hook).collect::<Vec<_>>(), "passes": passes.iter().map(test).collect::<Vec<_>>() });
    read_report(Runner::Mocha, v.to_string().as_bytes()).unwrap()
}

#[test]
fn a_mocha_hook_that_already_failed_is_not_new_because_the_change_adds_a_test_to_its_suite() {
    let refused = "connect ECONNREFUSED 127.0.0.1:5432";
    let before = mocha(&[("test/total.test.js", "total adds")], &[("test/db.test.js", "db \"before all\" hook for \"connects\"", refused)]);
    let after = mocha(&[("test/total.test.js", "total adds")], &[("test/db.test.js", "db \"before all\" hook for \"reconnects after a drop\"", refused)]);
    let c = suite::judged(&before, &after, Some(&after), None);
    assert!(names(&c, "new_problems").is_empty() && decision(&c, &[("src/db.js", "modify"), ("test/db.test.js", "modify")]) != "blocked",
        "the db suite's before-all hook needs a database the sandbox does not have, so it fails on the old code and on the new code alike; the change only adds a test at the top of that suite. mocha names a before-all hook after the first test of its suite, so the same failure now carries another name, counts as new, repeats on the rerun and blocks with 'A test file no longer runs, or a hook now fails', which an approval cannot clear: {c}");
    let gone = "getaddrinfo ENOTFOUND my-bucket.s3.amazonaws.com";
    let before = mocha(&[("test/upload.test.js", "uploads stores a file"), ("test/upload.test.js", "uploads lists files")], &[("test/upload.test.js", "uploads \"after all\" hook for \"lists files\"", gone)]);
    let after = mocha(&[("test/upload.test.js", "uploads stores a file"), ("test/upload.test.js", "uploads lists files"), ("test/upload.test.js", "uploads removes a file")],
        &[("test/upload.test.js", "uploads \"after all\" hook for \"removes a file\"", gone)]);
    let c = suite::judged(&before, &after, Some(&after), None);
    assert!(names(&c, "new_problems").is_empty() && decision(&c, &[("src/upload.js", "modify"), ("test/upload.test.js", "modify")]) != "blocked",
        "the same with an after-all hook that needs the network: mocha names it after the last test of its suite, so adding a test at the end of the suite, the most common edit to a test file, turns an old failure into a block: {c}");
}

#[test]
fn a_node_suite_named_with_a_number_whose_after_hook_the_change_breaks_blocks() {
    let total = test_point(2, "adds");
    let before = tap(&[suite_point(1, "server on port 41000", "test/server.test.js", &["answers"], None), total.clone()]);
    let after = tap(&[suite_point(1, "server on port 41007", "test/server.test.js", &["answers"], Some("Server is not running.")), total.clone()]);
    let again = tap(&[suite_point(1, "server on port 52011", "test/server.test.js", &["answers"], Some("Server is not running.")), total]);
    let c = suite::judged(&before, &after, Some(&again), None);
    assert_eq!(names(&c, "new_problems").len(), 1,
        "the change breaks the server suite's after hook (node --test); it fails on the run and again on the rerun. The suite is named after a random port, and node names a top-level suite with no ' > ', so has() compares the two names as written, calls the repeated failure flaky and the receipt is clear: {c}");
    assert_eq!(decision(&c, &[("src/server.js", "modify")]), "blocked", "{c}");
}

#[test]
fn a_node_suite_named_after_a_url_with_a_random_port_is_not_gone_on_every_change() {
    let run = |port: u32| tap(&[suite_point(1, &format!("server at http://localhost:{port}"), "test/server.test.js", &["answers"], None), test_point(2, "adds")]);
    let c = suite::judged(&run(41000), &run(41007), Some(&run(52011)), None);
    assert!(names(&c, "gone").is_empty() && names(&c, "added").is_empty(),
        "the same healthy node --test suite, named after the URL of a server on a random port, on every run; masked() takes a first name part holding a '/' or a '.' for a file path and keeps its digits, so the names never pair and every change that runs these tests gets 'A test that ran before no longer runs': {c}");
    assert_eq!(decision(&c, &[("src/server.js", "modify")]), "clear", "{c}");
}

#[test]
fn a_hook_the_change_breaks_is_not_excused_by_an_old_failure_in_a_sibling_named_with_another_number() {
    let inner = |n: usize, name: &str, error: Option<&str>| -> String {
        let mut s = format!("    # Subtest: {name}\n        # Subtest: lists users\n        ok 1 - lists users\n          ---\n          duration_ms: 0.5\n          type: 'test'\n          ...\n        1..1\n");
        s += &match error {
            Some(e) => format!("    not ok {n} - {name}\n      ---\n      duration_ms: 1.0\n      type: 'suite'\n      location: '/workspace/test/api.test.js:{}:3'\n      failureType: 'hookFailed'\n      error: '{e}'\n      code: 'ERR_TEST_FAILURE'\n      ...\n", n * 10),
            None => format!("    ok {n} - {name}\n      ---\n      duration_ms: 1.0\n      type: 'suite'\n      ...\n"),
        };
        s
    };
    let api = |v1: Option<&str>, v2: Option<&str>| -> Report {
        let outer = format!("# Subtest: API\n{}{}    1..2\n{} 1 - API\n  ---\n  duration_ms: 3.0\n  type: 'suite'\n  ...\n", inner(1, "v1", v1), inner(2, "v2", v2), if v1.or(v2).is_some() { "not ok" } else { "ok" });
        tap(&[outer, test_point(2, "adds")])
    };
    let legacy = Some("getaddrinfo ENOTFOUND legacy.example.com");
    let before = api(legacy, None);
    let after = api(legacy, Some("Server is not running."));
    assert_eq!(after.problems, ["API > v1", "API > v2"]);
    let c = suite::judged(&before, &after, Some(&after), None);
    assert_eq!(names(&c, "new_problems"), ["API > v2"],
        "the v1 suite's after hook calls a legacy service and fails in the sandbox on the old code too; the change breaks the v2 suite's after hook, on every run. has() takes any old problem whose name differs only by a number for the same one, so the old v1 failure excuses the new v2 failure, which is in no list at all, and the receipt is clear: {c}");
    assert_eq!(decision(&c, &[("src/api/v2.js", "modify")]), "blocked", "{c}");
}

#[test]
fn an_after_hook_that_fails_in_a_node_test_file_the_change_adds_needs_a_look_not_a_block() {
    let before = tap(&[test_point(1, "adds")]);
    let after = tap(&[test_point(1, "adds"), suite_point(2, "report", "test/report.test.js", &["renders totals"], Some("getaddrinfo ENOTFOUND my-bucket.s3.amazonaws.com"))]);
    let c = suite::judged(&before, &after, Some(&after), None);
    assert_eq!(decision(&c, &[("src/report.js", "add"), ("test/report.test.js", "add")]), "look",
        "a node --test file the change adds: its tests pass, its after hook needs the network the sandbox does not have. The same file under jest or mocha needs a look; node names the failure after the suite, the receipt cannot see that the change adds the file, and blocks with 'A test file no longer runs', which an approval cannot clear: {c}");
}

#[test]
fn a_node_test_file_that_no_longer_loads_blocks_as_a_jest_one_does() {
    let jest_before = report(&[("test/cart.test.js > cart > adds an item", 1, 0, 0), ("test/total.test.js > adds", 1, 0, 0)], &[]);
    let jest_after = report(&[("test/total.test.js > adds", 1, 0, 0)], &["test/cart.test.js"]);
    let jest = suite::judged(&jest_before, &jest_after, Some(&jest_after), None);
    assert_eq!(decision(&jest, &[("src/cart.js", "modify")]), "blocked", "{jest}");
    let before = tap(&[suite_point(1, "cart", "test/cart.test.js", &["adds an item"], None), test_point(2, "adds")]);
    let after = tap(&[file_point(1, "test/cart.test.js"), test_point(2, "adds")]);
    let c = suite::judged(&before, &after, Some(&after), None);
    let r = receipt_for(&c, &[("src/cart.js", "modify")]);
    let md = receipt::markdown(&r);
    assert_eq!(r["decision"], "blocked",
        "the change makes test/cart.test.js throw when it loads, on every run. Under jest that blocks as a test file that no longer runs; node --test reports the file as a failing test named after its path, so its old test is merely gone and the file is filed as a new failing test, a look an approval clears:\n{md}");
    assert!(!md.contains("A test this change adds fails on the new code: `test/cart.test.js`"), "the change adds no test file, yet the receipt says it does:\n{md}");
}

#[test]
fn a_test_that_passed_before_and_fails_after_its_file_is_moved_still_blocks() {
    let before = report(&[("src/__tests__/cart.test.ts > cart > adds an item", 1, 0, 0), ("src/__tests__/cart.test.ts > cart > removes an item", 1, 0, 0)], &[]);
    let after = report(&[("src/cart.test.ts > cart > adds an item", 0, 1, 0), ("src/cart.test.ts > cart > removes an item", 1, 0, 0)], &[]);
    let c = suite::judged(&before, &after, None, None);
    let changed = [("src/__tests__/cart.test.ts", "delete"), ("src/cart.test.ts", "add"), ("src/cart.ts", "modify")];
    let md = receipt::markdown(&receipt_for(&c, &changed));
    assert_eq!(decision(&c, &changed), "blocked",
        "the change breaks adding an item to the cart and moves the cart tests next to the code; the same test, by the same name, passed before and fails now. Kept in place it blocks; moved, its old name is gone, run() finds no suspect file present in the new run so nothing runs again, and the failure is filed as a test the change adds, a look an approval clears, though a test file the change deletes was meant never to turn a failing test into a look:\n{md}");
}

#[test]
fn a_test_file_kern_never_ran_on_the_old_code_does_not_block_as_one_that_no_longer_runs() {
    let mut after = report(&[("test/math.test.js > adds", 1, 0, 0)], &["test/api.test.js"]);
    after.why.insert("test/api.test.js".into(), "Cannot find module '../dist/index.js' from 'test/api.test.js'".into());
    let c = suite::judged(&Report::default(), &after, Some(&after), None);
    let changed = [("package.json", "modify"), ("test/api.test.js", "modify"), ("test/math.test.js", "modify")];
    let md = receipt::markdown(&receipt_for(&c, &changed));
    assert_ne!(decision(&c, &changed), "blocked",
        "the change moves the project from ava, which Kern cannot read, to vitest, so the old code has no run to compare and run() passes an empty old report; a test file that needs a build cannot load in the sandbox. A failing test of that suite is a new failing test (a look), but a file that cannot load blocks as one that 'no longer runs', though it never ran:\n{md}");
}

#[test]
fn renaming_a_test_file_that_could_not_load_before_does_not_block() {
    let before = report(&[("test/math.test.ts > adds", 1, 0, 0)], &["test/legacy.test.ts"]);
    let after = report(&[("test/math.test.ts > adds", 1, 0, 0)], &["test/legacy-api.test.ts"]);
    let c = suite::judged(&before, &after, Some(&after), None);
    let changed = [("test/legacy.test.ts", "delete"), ("test/legacy-api.test.ts", "add")];
    let md = receipt::markdown(&receipt_for(&c, &changed));
    assert_ne!(decision(&c, &changed), "blocked",
        "test/legacy.test.ts cannot load in the sandbox on the old code (it is set aside as already failing); the change only renames it, and it still cannot load. Problems are matched by exact path, so the renamed file's failure is new, and because a test file is deleted the receipt will not treat the added file as new: it blocks as a file that 'no longer runs':\n{md}");
}

#[test]
fn a_receipt_blocked_because_a_test_file_no_longer_runs_does_not_tick_the_tests_line() {
    let before = report(&[("test/cart.test.js > adds an item", 1, 0, 0), ("test/total.test.js > adds", 1, 0, 0)], &[]);
    let after = report(&[("test/total.test.js > adds", 1, 0, 0)], &["test/cart.test.js"]);
    let c = suite::compare(&before, &after, Some(&after));
    let r = receipt_for(&c, &[("src/cart.js", "modify")]);
    let md = receipt::markdown(&r);
    assert_eq!(r["decision"], "blocked", "{md}");
    assert!(!md.contains("- ✅ Tests:") && !md.contains("✅ No skipped, focused or removed tests"),
        "a test that passed before no longer runs because its file no longer loads, and the receipt is blocked for it; yet under What Kern checked it ticks the tests and says no test was removed:\n{md}");
}

#[test]
fn a_test_that_fails_on_every_run_of_the_new_code_is_not_said_to_pass() {
    let key = "src/session.test.ts > keeps the user signed in";
    let before = report(&[(key, 1, 0, 0), ("src/total.test.ts > adds", 1, 0, 0)], &[]);
    let after = report(&[(key, 0, 1, 0), ("src/total.test.ts > adds", 1, 0, 0)], &[]);
    let alone_new = report(&[(key, 0, 1, 0)], &[]);
    let alone_old = report(&[(key, 0, 1, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&alone_old));
    let md = receipt::markdown(&receipt_for(&c, &[("src/session.ts", "modify")]));
    assert!(!md.contains("2 that passed before still pass") && !md.contains("by chance"),
        "the test fails on the whole run and alone on the new code, every time; it is set aside because the old code alone fails it too. The receipt counts it among the tests that 'still pass' and says it 'failed once and passed when run again', neither of which happened:\n{md}");
}
