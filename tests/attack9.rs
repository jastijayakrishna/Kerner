use kern::proof::{Count, Report};
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
fn decision(tests: &Value, changed: &[(&str, &str)]) -> String {
    let d = json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": changed.iter().map(|(p, op)| json!({ "path": p, "operation": op })).collect::<Vec<_>>(), "powers": [], "tricks": [], "settings": [], "protected": [],
        "dependencies": [], "agent_notes": [], "not_checked": [] });
    receipt::build(&d, &rules, tests)["decision"].as_str().unwrap_or("").to_string()
}

#[test]
fn a_test_that_breaks_in_a_second_project_while_its_twin_already_failed_still_blocks() {
    let key = "packages/ui/src/date.test.ts > formats a date";
    let before = report(&[(key, 1, 1, 0)], &[]);
    let after = report(&[(key, 0, 2, 0)], &[]);
    let alone_new = report(&[(key, 0, 2, 0)], &[]);
    let alone_old = report(&[(key, 1, 1, 0)], &[]);
    assert_eq!(names(&suite::judged(&before, &after, Some(&alone_new), None), "broke"), [key], "without an old-alone run the same evidence blocks");
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&alone_old));
    assert_eq!(names(&c, "broke"), [key],
        "the file runs in two jest projects (or vitest workspace projects) under one key; the old code passes it in one and fails it in the other, whole and alone alike; the new code fails it in both, twice. The old side alone shows only the failure it always had, yet any old failure excuses every new one under the key, so the regression is called flaky and the check is green: {c}");
}

#[test]
fn a_test_removed_from_a_file_that_cannot_pass_alone_is_still_gone() {
    let builds = "packages/cli/src/build.test.ts > builds";
    let cleans = "packages/cli/src/build.test.ts > cleans up";
    let before = report(&[(builds, 1, 0, 0), (cleans, 1, 0, 0)], &[]);
    let after = report(&[(builds, 1, 0, 0)], &[]);
    let alone_new = report(&[(builds, 0, 1, 0)], &[]);
    let alone_old = report(&[(builds, 0, 1, 0), (cleans, 0, 1, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&alone_old));
    assert_eq!(names(&c, "gone"), [cleans],
        "the change deletes a passing test from a file that needs a sibling package built, so the file cannot pass alone; the test is missing from the whole new run and from the new run alone, and on the old code alone it still runs (it fails, but it is there), so it did not go missing by itself. Kern counts a failure alone on the old code as if the test were missing there and drops it: {c}");
    assert_eq!(decision(&c, &[("packages/cli/src/build.test.ts", "modify")]), "look", "the receipt says no test was removed: {c}");
}

#[test]
fn a_test_missing_from_both_whole_runs_is_gone_even_when_it_comes_back_alone() {
    let cache = "src/cache.test.ts > writes the cache";
    let loads = "src/plugins.test.ts > loads";
    let lists = "src/plugins.test.ts > lists every plugin";
    let before = report(&[(cache, 1, 0, 0), (loads, 1, 0, 0), (lists, 1, 0, 0)], &[]);
    let after = report(&[(cache, 0, 1, 0), (loads, 1, 0, 0)], &[]);
    let alone_new = report(&[(cache, 1, 0, 0), (loads, 1, 0, 0), (lists, 1, 0, 0)], &[]);
    let whole_again = report(&[(cache, 0, 1, 0), (loads, 1, 0, 0)], &[]);
    let alone_old = report(&[(cache, 1, 0, 0), (loads, 1, 0, 0), (lists, 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert_eq!(names(&c, "together"), [cache], "a failure that shows only when the whole suite runs needs a look: {c}");
    assert_eq!(names(&c, "gone"), [lists],
        "through the same shared state the change makes a test vanish from both whole runs; it only runs when its file runs alone. worse() keeps a test the whole rerun does not have at its alone result, so missing counts as passing and the test is not gone: {c}");
}

#[test]
fn a_failing_test_whose_file_no_longer_loads_on_the_whole_rerun_is_not_flaky() {
    let session = "src/session.test.ts > keeps the user signed in";
    let cache = "src/cache.test.ts > fills the cache";
    let before = report(&[(session, 1, 0, 0), (cache, 1, 0, 0)], &[]);
    let after = report(&[(session, 0, 1, 0), (cache, 1, 0, 0)], &[]);
    let alone_new = report(&[(session, 1, 0, 0)], &[]);
    let whole_again = report(&[(cache, 1, 0, 0)], &["src/session.test.ts"]);
    let alone_old = report(&[(session, 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert!(!names(&c, "together").is_empty() && !names(&c, "flaky").contains(&session.to_string()),
        "the test fails on the whole run, passes alone, and on the whole rerun its file does not even load; worse() keeps the alone pass for a test the rerun lacks and drops the rerun's load failure because the first run did not have it, so the regression is flaky and the check is green: {c}");
}

#[test]
fn two_tests_named_by_a_number_each_run_keep_their_own_results_on_the_rerun() {
    let before = report(&[("src/api.test.ts > answers on port 41000", 1, 0, 0), ("src/api.test.ts > answers on port 52000", 1, 0, 0)], &[]);
    let after = report(&[("src/api.test.ts > answers on port 41007", 0, 1, 0), ("src/api.test.ts > answers on port 52019", 1, 0, 0)], &[]);
    let alone_new = report(&[("src/api.test.ts > answers on port 9013", 0, 1, 0), ("src/api.test.ts > answers on port 30001", 1, 0, 0)], &[]);
    let alone_old = report(&[("src/api.test.ts > answers on port 41500", 1, 0, 0), ("src/api.test.ts > answers on port 52500", 1, 0, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&alone_old));
    assert_eq!(names(&c, "broke").len(), 1,
        "the first of two tests named with a random port fails on the whole run and again alone, the second passes both times; the rerun is paired to the old names by text order, which the random ports reorder, so the failing test borrows the passing one's rerun and is called flaky: {c}");
}

#[test]
fn a_skipped_test_the_change_turns_on_that_fails_needs_a_look() {
    let before = report(&[("src/round.test.ts > rounds down", 1, 0, 0), ("src/round.test.ts > rounds half to even", 0, 0, 1)], &[]);
    let after = report(&[("src/round.test.ts > rounds down", 1, 0, 0), ("src/round.test.ts > rounds half to even", 0, 1, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&after));
    assert_eq!(decision(&c, &[("src/round.ts", "modify"), ("src/round.test.ts", "modify")]), "look",
        "the change un-skips a test and it fails on the new code, twice; a test the change adds that fails needs a look, but this one is in no list at all, so the receipt is clear: {c}");
}
