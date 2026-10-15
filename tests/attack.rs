use kern::proof::{Count, Report};
use kern::suite;
use kern::util::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-attack-{}", uuid4().unwrap()));
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
fn repo(files: &[(&str, &str)]) -> (Temp, String) {
    let t = Temp::new();
    git(&t.0, &["init", "-q", "-b", "main"]);
    git(&t.0, &["config", "core.autocrlf", "false"]);
    for (p, text) in files { write(&t.0, p, text); }
    git(&t.0, &["add", "-A"]);
    git(&t.0, &["commit", "-q", "-m", "base"]);
    let base = git(&t.0, &["rev-parse", "HEAD"]);
    (t, base)
}
const TOTAL: &str = "export function total(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\n";
fn project() -> (Temp, String) {
    repo(&[("package.json", r#"{"name":"shop","devDependencies":{"vitest":"3.2.0"}}"#), ("src/total.js", TOTAL),
        ("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([1, 2])).toBe(3));\n")])
}
fn commit(t: &Temp, files: &[(&str, &str)], deleted: &[&str]) -> String {
    for (p, text) in files { write(&t.0, p, text); }
    for p in deleted { std::fs::remove_file(t.0.join(p)).unwrap(); }
    git(&t.0, &["add", "-A"]);
    git(&t.0, &["commit", "-q", "--allow-empty", "-m", "change"]);
    git(&t.0, &["rev-parse", "HEAD"])
}
fn reset(t: &Temp, to: &str) { git(&t.0, &["reset", "-q", "--hard", to]); }
fn rules(t: &Temp, base: &str, head: &str) -> Value { kern::rules::rules(&t.0, base, head).unwrap() }
fn listed(r: &Value, key: &str) -> Vec<String> { r[key].as_array().unwrap_or_else(|| panic!("{key}: {r}")).iter().map(|x| x["path"].as_str().unwrap_or("").to_string()).collect() }
fn missed_tricks(t: &Temp, base: &str, cases: &[(&str, &str)]) -> Vec<String> {
    let mut missed = Vec::new();
    for (name, text) in cases {
        let head = commit(t, &[("test/total.test.js", text)], &[]);
        if listed(&rules(t, base, &head), "tricks").is_empty() { missed.push(name.to_string()); }
        reset(t, base);
    }
    missed
}
fn report(tests: &[(&str, u32, u32, u32)]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: Vec::new() }
}

#[test]
fn a_test_marked_to_pass_when_it_fails_is_surfaced() {
    let (t, base) = project();
    let missed = missed_tricks(&t, &base, &[("vitest test.fails", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.fails('adds', () => expect(total([1, 2])).toBe(3));\n"),
        ("jest it.failing", "const { total } = require('../src/total.js');\nit.failing('adds', () => expect(total([1, 2])).toBe(3));\n")]);
    assert!(missed.is_empty(), "a test whose check is inverted so it passes only when it throws hides a regression; not surfaced: {missed:?}");
}

#[test]
fn a_test_skipped_by_skip_if_or_run_if_is_a_trick() {
    let (t, base) = project();
    let missed = missed_tricks(&t, &base, &[("describe.skipIf(true)", "import { total } from '../src/total.js';\nimport { describe, test, expect } from 'vitest';\ndescribe.skipIf(true)('total', () => { test('adds', () => expect(total([1, 2])).toBe(3)); });\n"),
        ("test.runIf(false)", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.runIf(false)('adds', () => expect(total([1, 2])).toBe(3));\n")]);
    assert!(missed.is_empty(), "an unconditional skipIf/runIf disables the test; not surfaced as a trick: {missed:?}");
}

#[test]
fn a_focused_or_skipped_test_reached_through_a_chain_or_another_name_is_a_trick() {
    let (t, base) = project();
    let missed = missed_tricks(&t, &base, &[("test.concurrent.only", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.concurrent.only('adds', () => expect(total([1, 2])).toBe(3));\ntest('rounds', () => expect(total([0.1, 0.2])).toBeCloseTo(0.3));\n"),
        ("import test as check; check.only", "import { total } from '../src/total.js';\nimport { test as check, expect } from 'vitest';\ncheck.only('adds', () => expect(total([1, 2])).toBe(3));\ncheck('rounds', () => expect(total([0.1, 0.2])).toBeCloseTo(0.3));\n"),
        ("const it2 = test; it2.skip", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\nconst it2 = test;\nit2.skip('adds', () => expect(total([1, 2])).toBe(3));\n")]);
    assert!(missed.is_empty(), "the screen only recognises .only/.skip on a direct runner global; these equivalent forms are not surfaced: {missed:?}");
}

#[test]
fn a_new_skip_in_a_file_that_already_skipped_a_test_is_a_trick() {
    let (t, base) = repo(&[("package.json", r#"{"name":"shop","devDependencies":{"vitest":"3.2.0"}}"#), ("src/total.js", TOTAL),
        ("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.skip('needs the network', () => {});\ntest('adds', () => expect(total([1, 2])).toBe(3));\n")]);
    let head = commit(&t, &[("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.skip('needs the network', () => {});\ntest.skip('adds', () => expect(total([1, 2])).toBe(3));\n")], &[]);
    assert!(!listed(&rules(&t, &base, &head), "tricks").is_empty(), "the test 'adds' is newly skipped, but the set-based marker diff is empty because .skip already appeared, so it is not surfaced");
}

#[test]
fn a_new_end_to_end_test_file_is_not_blocked_as_a_test_runner_trick() {
    let (t, base) = project();
    let head = commit(&t, &[("e2e/checkout.js", "describe('checkout', () => {\n  it('charges once', () => { expect(1).toBe(1); });\n});\n")], &[]);
    let r = rules(&t, &base, &head);
    assert!(listed(&r, "tricks").is_empty(), "a new end-to-end test (not under test/ or *.test.*) is wrongly blocked as production code reaching into the test runner: {r}");
}

#[test]
fn a_real_test_skipped_behind_a_same_named_passing_decoy_is_not_silently_kept() {
    let before = report(&[("suite > login works", 1, 0, 0)]);
    let after = report(&[("suite > login works", 1, 0, 1)]);
    let c = suite::compare(&before, &after, None);
    let newly: Vec<String> = c["newly_skipped"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    assert!(newly.contains(&"suite > login works".to_string()),
        "the real assertion is skipped while a decoy of the same name passes; merged counts make compare report it as fully kept: {c}");
}

#[test]
fn a_lockfile_tarball_on_a_foreign_host_that_copies_the_registry_layout_is_a_surprise() {
    let entry = |host: &str| json!({ "packages": { "": { "name": "shop" },
        "node_modules/left-pad": { "version": "1.3.0", "resolved": format!("https://{host}/left-pad/-/left-pad-1.3.0.tgz") } } });
    let out = kern::deps::lock_surprises(&entry("registry.npmjs.org"), &entry("evil.example"));
    assert!(out.iter().any(|w| w.contains("left-pad") && w.contains("evil.example")),
        "the tarball now downloads from a foreign host that only copies the registry path layout; tarball_of checks the path but not the host: {out:?}");
}

#[test]
fn rules_flag_a_lockfile_whose_tarball_host_moved_to_an_attacker() {
    let lock = |host: &str| format!(r#"{{"name":"shop","lockfileVersion":3,"packages":{{"":{{"name":"shop"}},"node_modules/left-pad":{{"version":"1.3.0","resolved":"https://{host}/left-pad/-/left-pad-1.3.0.tgz"}}}}}}"#);
    let (t, _base) = project();
    let mid = commit(&t, &[("package.json", r#"{"name":"shop","dependencies":{"left-pad":"^1.3.0"},"devDependencies":{"vitest":"3.2.0"}}"#), ("package-lock.json", &lock("registry.npmjs.org"))], &[]);
    let head = commit(&t, &[("package-lock.json", &lock("evil.example"))], &[]);
    let r = rules(&t, &mid, &head);
    assert!(r["lockfile"].as_array().unwrap().iter().any(|x| x["why"].as_str().unwrap_or("").contains("evil.example")),
        "a push that only moves the tarball download to an attacker's host is not flagged: {r}");
}
