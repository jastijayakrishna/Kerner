use kern::proof::{Count, Report};
use kern::util::*;
use kern::{check, policy, receipt, suite};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-attack10-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
fn repo(t: &Temp, files: &[(&str, &str)]) -> (PathBuf, String) {
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    for (p, text) in files { write(&r, p, text); }
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    (r, base)
}
fn commit(r: &Path, files: &[(&str, &str)]) -> String {
    for (p, text) in files { write(r, p, text); }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "change"]);
    git(r, &["rev-parse", "HEAD"])
}
fn operator() -> policy::Operator {
    policy::operator(&json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }],
        "registry": "registry.npmjs.org", "permissions": {} })).unwrap()
}
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
fn clean_tests() -> Value { json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 12, "failed": 0, "skipped": 0 }, "after": { "passed": 12, "failed": 0, "skipped": 0 }, "kept": 12, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] }) }
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }
fn decision(tests: &Value, changed: &[(&str, &str)]) -> String {
    receipt::build(&drive_json(), &rules_for(changed), tests, &off())["decision"].as_str().unwrap_or("").to_string()
}

#[test]
fn a_test_whose_file_did_not_load_and_that_fails_on_every_rerun_still_blocks() {
    let cart = "src/cart.test.ts > adds an item";
    let total = "src/total.test.ts > sums";
    let before = report(&[(cart, 1, 0, 0), (total, 1, 0, 0)], &[]);
    let after = report(&[(total, 1, 0, 0)], &["src/cart.test.ts"]);
    let alone_new = report(&[(cart, 0, 1, 0)], &[]);
    let whole_again = report(&[(cart, 0, 1, 0), (total, 1, 0, 0)], &[]);
    let alone_old = report(&[(cart, 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert!(!names(&c, "broke").is_empty() || !names(&c, "new_problems").is_empty(),
        "the change breaks src/cart.test.ts: on the whole run the file does not load, alone and on the whole rerun it loads and its test fails, and on the old code alone the test passes. The load failure is called flaky because the reruns loaded the file, and the failing test is in no list because the first run has no failure for it and the reruns have it, so nothing is reported: {c}");
    assert_eq!(decision(&c, &[("src/cart.ts", "modify")]), "blocked", "a test that passed before is missing from one run and fails in two more, yet the receipt is clear: {c}");
}

#[test]
fn a_test_that_fails_whole_and_whose_file_does_not_load_alone_is_not_flaky() {
    let cart = "src/cart.test.ts > adds an item";
    let before = report(&[(cart, 1, 0, 0)], &[]);
    let after = report(&[(cart, 0, 1, 0)], &[]);
    let alone_new = report(&[], &["src/cart.test.ts"]);
    let whole_again = report(&[(cart, 1, 0, 0)], &[]);
    let alone_old = report(&[(cart, 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert_eq!(names(&c, "broke"), [cart],
        "the test fails on the whole run; alone its file no longer loads on the new code while the old code alone loads and passes it; the whole rerun passes it. The alone run may only add failures, and its failure here is the file's, which worse() keeps as a problem the first run did not have and never turns into a failure of the test, so the test is called flaky: {c}");
}

#[test]
fn a_node_suite_named_with_a_number_pairs_in_a_package_as_it_does_at_the_root() {
    let root = suite::judged(&report(&[("server on port 41000 > answers", 1, 0, 0)], &[]), &report(&[("server on port 41007 > answers", 1, 0, 0)], &[]), None, None);
    assert!(names(&root, "gone").is_empty() && names(&root, "added").is_empty(), "{root}");
    let before = report(&[("packages/api: server on port 41000 > answers", 1, 0, 0)], &[]);
    let after = report(&[("packages/api: server on port 41007 > answers", 1, 0, 0)], &[]);
    let c = suite::judged(&before, &after, None, None);
    assert!(names(&c, "gone").is_empty() && names(&c, "added").is_empty(),
        "the same node --test suite in packages/api: absorb() prefixes its names with 'packages/api: ', and masked() then takes 'packages/api: server on port 41000' for a file path (it holds a '/') and keeps its digits, so the names no longer pair; run() paired the unprefixed names and saw nothing wrong, judged() reports a healthy test as gone: {c}");
    let broken = report(&[("packages/api: server on port 41007 > answers", 0, 1, 0)], &[]);
    let again = report(&[("packages/api: server on port 52011 > answers", 0, 1, 0)], &[]);
    let c = suite::judged(&before, &broken, Some(&again), None);
    assert_eq!(names(&c, "broke").len(), 1, "a real break of that test becomes a gone test and a new failing one, a look an approval clears: {c}");
}

#[test]
fn a_numbered_test_that_fails_alone_keeps_that_failure_when_the_whole_rerun_passes_it() {
    let port = "src/api.test.ts > answers on port 41000";
    let pool = "src/db.test.ts > keeps one pool";
    let before = report(&[(port, 1, 0, 0), (pool, 1, 0, 0)], &[]);
    let after = report(&[("src/api.test.ts > answers on port 41007", 0, 1, 0), (pool, 0, 1, 0)], &[]);
    let alone_new = report(&[("src/api.test.ts > answers on port 9013", 0, 1, 0), (pool, 1, 0, 0)], &[]);
    let whole_again = report(&[("src/api.test.ts > answers on port 52019", 1, 0, 0), (pool, 0, 1, 0)], &[]);
    let alone_old = report(&[("src/api.test.ts > answers on port 41500", 1, 0, 0), (pool, 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert_eq!((names(&c, "broke"), names(&c, "together")), (vec![port.to_string()], vec![pool.to_string()]),
        "the port test fails on the whole run and again when its file runs alone; the pool test does not repeat alone, so the whole suite runs again and worse() merges the two reruns, where the alone run may only add failures. worse() matches the alone run by its raw name, which carries another random port, so the alone failure is dropped and the test is called flaky: {c}");
}

#[test]
fn a_hook_named_with_a_number_that_fails_on_both_runs_blocks() {
    let hook = |port: &str| format!("test/server.test.js > server on port {port} \"after all\" hook in \"server on port {port}\"");
    let before = report(&[("test/server.test.js > server on port 41000 answers", 1, 0, 0)], &[]);
    let after = report(&[("test/server.test.js > server on port 41007 answers", 1, 0, 0)], &[&hook("41007")]);
    let again = report(&[("test/server.test.js > server on port 52011 answers", 1, 0, 0)], &[&hook("52011")]);
    let c = suite::judged(&before, &after, Some(&again), None);
    assert_eq!(names(&c, "new_problems").len(), 1,
        "the change breaks the server's after-all hook, which fails on the whole run and again on the rerun (mocha has no alone run); the tests' names pair by their masked name, but a problem's name is compared as written, so the hook under the rerun's port is a different name and the failure is called flaky: {c}");
    assert_eq!(decision(&c, &[("src/server.js", "modify")]), "blocked", "{c}");
}

#[test]
fn a_hook_that_fails_in_a_test_file_the_change_adds_needs_a_look_not_a_block() {
    let total = "test/total.test.js > total adds";
    let jest_shape = "test/db.test.js";
    let mocha_shape = "test/db.test.js > db \"before all\" hook in \"db\"";
    let changed = [("src/db.js", "add"), ("test/db.test.js", "add")];
    let before = report(&[(total, 1, 0, 0)], &[]);
    let after = |problem: &str| report(&[(total, 1, 0, 0)], &[problem]);
    let jest = suite::judged(&before, &after(jest_shape), Some(&after(jest_shape)), None);
    assert_eq!(decision(&jest, &changed), "look", "a new jest or vitest test file that cannot reach its database here needs a look: {jest}");
    let mocha = suite::judged(&before, &after(mocha_shape), Some(&after(mocha_shape)), None);
    assert_eq!(names(&mocha, "unproven"), [mocha_shape], "a file with no record of passing on the old code is not proof of a break: {mocha}");
    assert_eq!(decision(&mocha, &changed), "look",
        "the same new test file under mocha: its before-all hook needs a database the sandbox does not have. mocha names the failure after the hook, not the file, so the receipt does not see that the change adds the file and blocks with 'A test file no longer runs', which an approval cannot clear: {mocha}");
}

#[test]
fn a_skipped_test_in_a_numbered_group_pairs_with_the_skipped_one_whatever_numbers_they_drew() {
    let run = |pass: &str, skip: &str| -> Value {
        let before = report(&[(pass, 1, 0, 0), (skip, 0, 0, 1)], &[]);
        let after = report(&[("src/rows.test.ts > row 23 renders", 0, 1, 0), ("src/rows.test.ts > row 58 renders", 0, 0, 1)], &[]);
        let again = report(&[("src/rows.test.ts > row 31 renders", 0, 1, 0), ("src/rows.test.ts > row 77 renders", 0, 0, 1)], &[]);
        suite::judged(&before, &after, Some(&again), None)
    };
    let drew_high = run("src/rows.test.ts > row 42 renders", "src/rows.test.ts > row 17 renders");
    assert_eq!(names(&drew_high, "broke").len(), 1, "{drew_high}");
    let drew_low = run("src/rows.test.ts > row 17 renders", "src/rows.test.ts > row 42 renders");
    assert_eq!(names(&drew_low, "broke").len(), 1,
        "the passing test of the group now fails twice and the skipped one stays skipped; pairing sorts a skipped test with the passing ones, so when the passing test drew the lower number the new skip pairs with it and the new failure with the old skip: newly skipped plus a new failing test, a look an approval clears instead of a block: {drew_low}");
}

#[test]
fn a_receipt_whose_tests_did_not_run_does_not_open_with_an_unqualified_pass() {
    let tests = json!({ "status": "NOT_RUN", "why": "no test runner Kern can read was found in package.json, at the root or in a package this change touches (Kern runs vitest, jest, mocha and node --test)" });
    let behaviour = json!({ "status": "RAN", "report": { "coverage": { "percent": 100, "new": { "ran": 4, "total": 4 }, "old": { "ran": 4, "total": 4 } } } });
    let r = receipt::build(&drive_json(), &rules_for(&[("src/total.js", "modify")]), &tests, &behaviour);
    assert_eq!(r["decision"], "clear", "{r}");
    let md = receipt::markdown(&r, None);
    assert!(!md.contains("### ✅ Kern: Nothing unexpected\n"),
        "the project's tests never ran (an ava project with an AI key), yet the receipt opens with the bare answer that README defines as 'every test that passed before still passes', while the Kern status for the same receipt says the tests did not run:\n{md}");
}

#[test]
fn a_receipt_makes_no_pass_claim_about_changed_code_it_skipped() {
    let receipt_md = |skipped: Value, tests: Value, behaviour: Value| -> String {
        let mut d = drive_json();
        let changed: Vec<(String, String)> = skipped.as_array().unwrap().iter().map(|s| (s["path"].as_str().unwrap().to_string(), "modify".to_string())).collect();
        let changed: Vec<(&str, &str)> = changed.iter().map(|(p, o)| (p.as_str(), o.as_str())).collect();
        d["skipped"] = skipped;
        receipt::markdown(&receipt::build(&d, &rules_for(&changed), &tests, &behaviour), None)
    };
    let vue = receipt_md(json!([{ "path": "src/components/Checkout.vue", "why": "a .vue file; Kern does not run .vue code yet" },
        { "path": "tests/test_refunds.py", "why": "a .py file; Kern does not run .py code yet" }]), clean_tests(), off());
    let python = receipt_md(json!([{ "path": "app/payments.py", "why": "a .py file; Kern does not run .py code yet" }, { "path": "public/checkout.min.js", "why": "generated or minified code" }]),
        json!({ "status": "NOT_RUN", "why": "no test runner Kern can read was found in package.json" }), json!({ "status": "NOTHING_TO_RUN" }));
    let claims = ["✅ No new powers", "✅ No skipped, focused or removed tests", "✅ No changed line of code to run"];
    let made: Vec<String> = [("a Vue component and a Python test file in a vitest project", &vue), ("Python code and a minified bundle, with an AI key", &python)].iter()
        .flat_map(|(case, md)| claims.iter().filter(|c| md.contains(*c)).map(move |c| format!("{case}: {c}"))).collect();
    assert!(made.is_empty(),
        "every changed file here is listed under Not checked: the rules parse no .vue or .py file (so neither powers nor skips were looked for), and no line of the Python code or the minified bundle ran; yet the receipt passes them with these lines:\n{}\n\n{vue}\n{python}", made.join("\n"));
}

#[test]
fn documentation_the_tests_read_by_folder_still_runs_the_tests() {
    let t = Temp::new();
    let package = r#"{"name":"site","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let test = "import { readdirSync, readFileSync } from 'node:fs';\nimport { test, expect } from 'vitest';\nconst dir = new URL('../docs/', import.meta.url);\nfor (const name of readdirSync(dir)) {\n  test(name, () => expect(readFileSync(new URL(name, dir), 'utf8').startsWith('# ')).toBe(true));\n}\n";
    let (r, base) = repo(&t, &[("package.json", package), ("test/docs.test.js", test), ("docs/guide.md", "# Guide\n\nHow to use it.\n"), ("docs/intro.md", "# Intro\n\nWelcome.\n")]);
    let head = commit(&r, &[("docs/guide.md", "How to use it.\n")]);
    let out = check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
    let md = std::fs::read_to_string(t.0.join("out").join("receipt.md")).unwrap();
    assert_ne!(out["tests"], "NOTHING_TO_RUN",
        "test/docs.test.js reads every page under docs/ and checks it has a title; the change removes the title of docs/guide.md, so the test that passed before fails. Kern sees only .md files, finds no code that names guide.md, skips the tests and says:\n{md}");
}
