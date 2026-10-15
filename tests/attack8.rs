use kern::proof::{Count, Report};
use kern::suite;
use serde_json::Value;
use std::collections::BTreeMap;

fn report(tests: &[(&str, u32, u32, u32)]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests {
        t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped });
    }
    Report { tests: t, problems: Vec::new(), why: BTreeMap::new() }
}
fn names(c: &Value, key: &str) -> Vec<String> {
    c[key].as_array().unwrap_or_else(|| panic!("{key}: {c}")).iter().map(|x| x.as_str().unwrap_or("").to_string()).collect()
}

#[test]
fn a_flaky_test_whose_name_carries_the_run_time_is_not_blocked() {
    let before = report(&[("throws for Thu Oct 15 2026 19:50:54", 1, 0, 0)]);
    let after = report(&[("throws for Thu Oct 15 2026 19:51:02", 0, 1, 0)]);
    let again = report(&[("throws for Thu Oct 15 2026 19:51:19", 1, 0, 0)]);
    let c = suite::compare(&before, &after, Some(&again));
    assert!(names(&c, "broke").is_empty(),
        "the test passed before, failed once on the new code, and passed again on the rerun, so it is flaky, not broken; but because the rerun ran under a third clock-stamped name that the before/after rename map cannot translate, the flaky rescue looks it up by a name the rerun does not have and blocks an honest, unclearable pull request: {c}");
    assert_eq!(names(&c, "flaky").len(), 1, "a test that failed once and passed on the rerun is flaky even when its name carries the run time: {c}");
}

#[test]
fn a_clock_named_test_that_vanishes_but_returns_on_the_rerun_is_not_gone() {
    let before = report(&[("row rendered at 100ms", 1, 0, 0)]);
    let after = report(&[]);
    let again = report(&[("row rendered at 240ms", 1, 0, 0)]);
    let c = suite::compare(&before, &after, Some(&again));
    assert!(names(&c, "gone").is_empty(),
        "the test was missing from the first new-code run but came back on the rerun under a new clock-stamped name, so it did not go; the rerun guard looks it up by the before name, which the rerun does not use, so Kern reports a healthy test as gone: {c}");
}

#[test]
fn a_broken_test_renamed_behind_a_passing_decoy_is_not_counted_as_still_passing() {
    let before = report(&[("discount applies 1", 1, 0, 0)]);
    let after = report(&[("discount applies 2", 1, 0, 0), ("discount applies 3", 0, 1, 0)]);
    let c = suite::compare(&before, &after, Some(&after));
    let broke = names(&c, "broke");
    let gone = names(&c, "gone");
    assert!(!broke.is_empty() || gone.iter().any(|g| g == "discount applies 1"),
        "the only test that passed before was renamed and now fails, while a brand-new passing test took its old masked name; the greedy first-unused pairing gives the old name to the decoy, so Kern counts it as kept ({} kept) and files the real regression only as a new failing test that an approval can clear, never telling the maintainer the pre-existing test stopped passing: {c}", c["kept"]);
}
