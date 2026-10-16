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
        let p = std::env::temp_dir().join(format!("kern-attack12-{}", uuid4().unwrap()));
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
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }
fn listed(rules: &Value, key: &str) -> Vec<String> {
    rules[key].as_array().into_iter().flatten().map(|x| format!("{}: {}", x["path"].as_str().unwrap_or(""), x["why"].as_str().unwrap_or(""))).collect()
}

type Written = Vec<(&'static str, String)>;
const PACKAGE: &str = r#"{"name":"shop","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;
const MATH: &str = "export function sum(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\n";
const TOTAL: &str = "import { sum } from './math.js';\n\nexport function total(items) {\n  return sum(items);\n}\n";
const TOTAL_TEST: &str = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([2, 3])).toBe(5));\n";

#[test]
fn a_change_that_rewrites_what_an_existing_test_expects_is_not_cleared() {
    let snapshot_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([2, 3])).toMatchSnapshot());\n";
    let snapshot = |n: u32| format!("// Vitest Snapshot v1, https://vitest.dev/guide/snapshot.html\n\nexports[`adds 1`] = `{n}`;\n");
    let broken = TOTAL.replace("return sum(items);", "return sum(items) + 1;");
    let cases: [(&str, Written, Written); 2] = [
        ("the expected value", vec![("test/total.test.js", TOTAL_TEST.to_string())], vec![("src/total.js", broken.clone()), ("test/total.test.js", TOTAL_TEST.replace("toBe(5)", "toBe(6)"))]),
        ("the snapshot", vec![("test/total.test.js", snapshot_test.to_string()), ("test/__snapshots__/total.test.js.snap", snapshot(5))],
            vec![("src/total.js", broken.clone()), ("test/__snapshots__/total.test.js.snap", snapshot(6))]),
    ];
    let mut cleared = Vec::new();
    for (what, tests_before, change) in cases {
        let t = Temp::new();
        let mut files: Vec<(&str, &str)> = vec![("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL)];
        files.extend(tests_before.iter().map(|(p, s)| (*p, s.as_str())));
        let (r, base) = repo(&t, &files);
        let head = commit(&r, &change.iter().map(|(p, s)| (*p, s.as_str())).collect::<Vec<_>>());
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let key = "test/total.test.js > adds";
        let old = report(&[(key, 1, 0, 0)], &[]);
        let plan = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
        let copy = "test/total.kern-kept.test.js";
        assert_eq!((plan.run.iter().map(String::as_str).collect::<Vec<_>>(), plan.files.get(copy)), (vec![copy], Some(&tests_before[0].1)), "{what}: the old test file is put back beside the new one: {:?}", plan.files.keys());
        if tests_before.len() == 2 { assert_eq!(plan.files.get("test/__snapshots__/total.kern-kept.test.js.snap"), Some(&snapshot(5)), "{what}: with its old snapshot"); }
        let kept_new = suite::unkept(&report(&[(&format!("{copy} > adds"), 0, 1, 0)], &[]), &plan.back);
        let kept_old = suite::unkept(&report(&[(&format!("{copy} > adds"), 1, 0, 0)], &[]), &plan.back);
        let mut tests = suite::compare(&old, &report(&[(key, 1, 0, 0)], &[]), None);
        tests["rewritten"] = json!(suite::rewritten(&old, &kept_new, Some(&kept_old)));
        let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
        if receipt["decision"] == "clear" { cleared.push(format!("{what}:\n{}", receipt::markdown(&receipt, None))); }
    }
    assert!(cleared.is_empty(),
        "total([2, 3]) now returns 6 and the change rewrites the test's expectation to 6 so that it passes; the old test fails on the new code, yet the receipt clears the change and says the test that passed before still passes, because Kern never runs the old tests on the new code and never mentions an edited expectation:\n{}", cleared.join("\n"));
}

#[test]
fn a_setup_file_the_runner_config_names_is_a_settings_item() {
    let t = Temp::new();
    let config = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { setupFiles: ['./test/setup.js'] } });\n";
    let setup = "import { expect } from 'vitest';\nimport * as matchers from '@testing-library/jest-dom/matchers';\nexpect.extend(matchers);\n";
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("vitest.config.js", config), ("test/setup.js", setup), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST)]);
    let mocked = format!("{setup}import {{ vi }} from 'vitest';\nvi.mock('../src/math.js', () => ({{ sum: () => 5 }}));\n");
    let head = commit(&r, &[("test/setup.js", &mocked), ("src/math.js", &MATH.replace("a + b", "a - b"))]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let settings = listed(&rules, "settings");
    assert!(settings.iter().any(|s| s.starts_with("test/setup.js")),
        "vitest.config.js names test/setup.js as the setup file every test file runs first; the change mocks src/math.js there, so every test now passes on broken code. vitest.setup.js or setupTests.js would be a settings item, and so would a setup file the test script names, but the setup file the config names, the usual place, is not: settings {settings:?}");
}

#[test]
fn test_support_code_outside_the_test_folders_does_not_block_when_it_uses_the_runner() {
    let package = r#"{"name":"shop","scripts":{"test":"jest"},"devDependencies":{"jest":"30.0.0"}}"#;
    let test = "const { total } = require('../src/total.js');\nconst { items } = require('../src/test-utils.js');\ntest('adds', () => expect(total(items(2))).toBe(3));\n";
    let utils = "exports.items = (n) => Array.from({ length: n }, (_, i) => i + 1);\n";
    let total = "exports.total = (items) => items.reduce((a, b) => a + b, 0);\n";
    let mut blocked = Vec::new();
    let setup = "require('@testing-library/jest-dom');\n";
    for (what, path, after) in [
        ("a jest setup file that jest.config.js names", "jest/setup.js", format!("{setup}beforeEach(() => jest.clearAllMocks());\n")),
        ("a test helper module only the tests import", "src/test-utils.js", format!("{utils}exports.spy = () => jest.fn();\n")),
    ] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", package), ("jest.config.js", "module.exports = { setupFilesAfterEnv: ['<rootDir>/jest/setup.js'] };\n"),
            ("jest/setup.js", setup), ("src/test-utils.js", utils), ("src/total.js", total), ("test/total.test.js", test)]);
        let head = commit(&r, &[(path, &after)]);
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let key = "test/total.test.js > adds";
        let tests = suite::compare(&report(&[(key, 1, 0, 0)], &[]), &report(&[(key, 1, 0, 0)], &[]), None);
        let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
        if receipt["decision"] == "blocked" { blocked.push(format!("{what}: {:?}", listed(&rules, "tricks"))); }
    }
    assert!(blocked.is_empty(),
        "jest runs these files only for the tests, and jest's globals are how such files are written; Kern knows test code only by folder or name, so each counts as production code that reaches into the test runner, a block no approval clears:\n{}", blocked.join("\n"));
}

const SKIPPING_TEST: &str = "import { total } from '../src/total.js';\nimport { it, expect } from 'vitest';\nit('adds', () => expect(total([2, 3])).toBe(5));\nit.skip('adds in another currency', () => {});\n";

#[test]
fn moving_a_test_file_with_a_skip_it_already_had_does_not_block() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", SKIPPING_TEST)]);
    git(&r, &["mv", "test/total.test.js", "test/total.test.ts"]);
    let head = commit(&r, &[]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let before = report(&[("test/total.test.js > adds", 1, 0, 0), ("test/total.test.js > adds in another currency", 0, 0, 1)], &[]);
    let after = report(&[("test/total.test.ts > adds", 1, 0, 0), ("test/total.test.ts > adds in another currency", 0, 0, 1)], &[]);
    let tests = suite::compare(&before, &after, Some(&after));
    assert!(names(&tests, "gone").is_empty() && names(&tests, "newly_skipped").is_empty(), "the move itself is understood: {tests}");
    let r = receipt::build(&drive_json(), &rules, &tests, &off());
    assert_ne!(r["decision"], "blocked",
        "a plain rename of a test file from .js to .ts (git mv, not one byte changed) whose one skipped test was skipped before too; Kern compares the new path with nothing, calls the old skip a new trick, and because a test file was deleted the trick blocks, which an approval cannot clear: {:?}", listed(&rules, "tricks"));
}

#[test]
fn rewriting_a_test_that_was_already_skipped_does_not_block() {
    let as_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([2, 3])).toBe(5));\ntest.skip('adds in another currency', () => {});\n";
    let retitled = SKIPPING_TEST.replace("adds in another currency", "adds amounts in another currency");
    let jest = "const { total } = require('../src/total.js');\nit('adds', () => expect(total([2, 3])).toBe(5));\nxit('adds in another currency', () => {});\n";
    let mut blocked = Vec::new();
    for (what, old, text) in [("it written as test, as eslint's consistent-test-it fix writes it", SKIPPING_TEST, as_test.to_string()),
        ("the skipped test's title reworded", SKIPPING_TEST, retitled), ("a jest file moved to vitest, where xit is written it.skip", jest, SKIPPING_TEST.to_string())] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", old)]);
        let head = commit(&r, &[("test/total.test.js", &text)]);
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let tricks = listed(&rules, "tricks");
        if !tricks.is_empty() { blocked.push(format!("{what}: {tricks:?}")); }
    }
    assert!(blocked.is_empty(),
        "the file had one skipped test before and has the same one skipped test after, and every test that ran before still runs; Kern keys each skip by how it is spelled (it.skip versus test.skip) and by its exact title, so restyling or retitling it is a new trick that blocks:\n{}", blocked.join("\n"));
}

#[test]
fn a_test_whose_describe_block_is_renamed_is_the_same_test() {
    let old = |t: &str| format!("test/money.test.ts > formatPrice > {t}");
    let new = |t: &str| format!("test/money.test.ts > formatMoney > {t}");
    let before = report(&[(&old("rounds to cents"), 1, 0, 0), (&old("adds the symbol"), 1, 0, 0)], &[]);
    let after = report(&[(&new("rounds to cents"), 0, 1, 0), (&new("adds the symbol"), 1, 0, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&after), Some(&before));
    assert!(names(&c, "broke").len() == 1 && names(&c, "gone").is_empty(),
        "the change renames formatPrice to formatMoney, the describe title with it, and breaks rounding: the test that passed before fails twice and passes alone on the old code. Kern reads every test of the file as gone and the failing one as a new test, so the break is a look an approval clears, and the receipt says tests that still run no longer run: {c}");
}

#[test]
fn a_test_moved_into_a_new_file_while_its_old_file_stays_is_not_gone() {
    let before = report(&[("src/cart.test.ts > adds", 1, 0, 0), ("src/cart.test.ts > applies a discount", 1, 0, 0)], &[]);
    let after = report(&[("src/cart.test.ts > adds", 1, 0, 0), ("src/discount.test.ts > applies a discount", 1, 0, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&after), Some(&before));
    assert!(names(&c, "gone").is_empty() && c["kept"] == 2,
        "the change splits cart.test.ts and moves one test, unchanged, into the new discount.test.ts, where it passes; a move is only recognised when the old file is gone, so the receipt says a test that passed before no longer runs: {c}");
}

#[test]
fn a_moved_file_does_not_need_a_look_for_what_it_already_did() {
    let t = Temp::new();
    let api = "export const load = () => fetch(`${process.env.API_URL}/items`).then((r) => r.json());\n";
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL), ("src/api.js", api), ("test/total.test.js", TOTAL_TEST)]);
    std::fs::create_dir_all(r.join("src/services")).unwrap();
    std::fs::create_dir_all(r.join("test/unit")).unwrap();
    git(&r, &["mv", "src/api.js", "src/services/api.js"]);
    git(&r, &["mv", "test/total.test.js", "test/unit/total.test.js"]);
    let head = commit(&r, &[]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let tests = suite::compare(&report(&[("test/total.test.js > adds", 1, 0, 0)], &[]), &report(&[("test/unit/total.test.js > adds", 1, 0, 0)], &[]), Some(&report(&[("test/unit/total.test.js > adds", 1, 0, 0)], &[])));
    let r = receipt::build(&drive_json(), &rules, &tests, &off());
    let items: Vec<String> = r["look"].as_array().unwrap().iter().map(|x| x["what"].as_str().unwrap_or("").to_string()).collect();
    assert_eq!(r["decision"], "clear",
        "two files are moved with git mv and not one byte changes; the test still passes under its new path. Kern diffs without rename detection, so the moved module 'can now' fetch and read the environment and the moved test file is a deleted test file: {items:#?}");
}

#[test]
fn a_jest_test_file_that_cannot_load_says_why() {
    let t = Temp::new();
    let processor = t.0.join("jest-report.cjs");
    std::fs::write(&processor, kern::sandbox::JEST_REPORT).unwrap();
    let results = json!({ "numTotalTests": 1, "testResults": [
        { "testFilePath": "/workspace/test/total.test.js", "failureMessage": null, "testResults": [{ "ancestorTitles": [], "title": "adds", "status": "passed" }] },
        { "testFilePath": "/workspace/test/cart.test.js", "testExecError": { "message": "Cannot find module '../src/generated/prices' from 'test/cart.test.js'" },
          "failureMessage": "  ● Test suite failed to run\n\n    Cannot find module '../src/generated/prices' from 'test/cart.test.js'\n", "testResults": [] }] });
    let script = format!("require({})({});", json!(processor.to_string_lossy()), results);
    let out = match Command::new("node").arg("-e").arg(&script).env_remove("KERN_REPORT").output() {
        Ok(o) => o,
        Err(_) => { eprintln!("node is not installed here; the jest report processor is checked where it is"); return; }
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let after = kern::proof::read_report(policy::Runner::Jest, &out.stdout).unwrap();
    let before = report(&[("test/total.test.js > adds", 1, 0, 0), ("test/cart.test.js > totals", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&after));
    let md = receipt::markdown(&receipt::build(&drive_json(), &json!({ "changed": [{ "path": "src/cart.js", "operation": "modify" }] }), &c, &off()), None);
    assert!(after.problems == ["test/cart.test.js"] && md.contains("Cannot find module"),
        "jest says why test/cart.test.js could not run, but Kern's jest report processor drops the message, so the block (and the not-run reason when no file loads) never says why, as it does for vitest: why {:?}\n{md}", after.why);
}

#[test]
fn a_lockfile_that_gains_an_npm_alias_or_a_workspace_package_installs_no_other_package() {
    let tarball = |n: &str, v: &str| format!("https://registry.npmjs.org/{n}/-/{}-{v}.tgz", n.rsplit('/').next().unwrap());
    let old = json!({ "name": "mono", "lockfileVersion": 3, "packages": {
        "": { "name": "mono", "workspaces": ["packages/*"], "devDependencies": { "vitest": "^3.2.0" } },
        "node_modules/vitest": { "version": "3.2.0", "resolved": tarball("vitest", "3.2.0"), "integrity": "sha512-v" },
        "node_modules/@acme/core": { "resolved": "packages/core", "link": true },
        "packages/core": { "name": "@acme/core", "version": "1.0.0" } } });
    let mut new = old.clone();
    let p = new["packages"].as_object_mut().unwrap();
    p.insert("node_modules/rimraf".into(), json!({ "version": "5.0.10", "resolved": tarball("rimraf", "5.0.10"), "integrity": "sha512-r", "dependencies": { "glob": "^10.3.7" } }));
    p.insert("node_modules/@isaacs/cliui".into(), json!({ "version": "8.0.2", "resolved": tarball("@isaacs/cliui", "8.0.2"), "integrity": "sha512-c",
        "dependencies": { "string-width": "^5.1.2", "string-width-cjs": "npm:string-width@^4.2.0", "strip-ansi-cjs": "npm:strip-ansi@^6.0.1" } }));
    p.insert("node_modules/string-width-cjs".into(), json!({ "name": "string-width", "version": "4.2.3", "resolved": tarball("string-width", "4.2.3"), "integrity": "sha512-s" }));
    p.insert("node_modules/strip-ansi-cjs".into(), json!({ "name": "strip-ansi", "version": "6.0.1", "resolved": tarball("strip-ansi", "6.0.1"), "integrity": "sha512-a" }));
    p.insert("node_modules/@acme/ui".into(), json!({ "resolved": "packages/ui", "link": true }));
    p.insert("packages/ui".into(), json!({ "name": "@acme/ui", "version": "0.1.0", "devDependencies": { "rimraf": "^5.0.10" } }));
    let surprises = kern::deps::lock_surprises(&old, &new);
    assert!(surprises.is_empty(),
        "the change adds a workspace package, packages/ui, whose dev dependency rimraf brings in @isaacs/cliui, which declares string-width-cjs as npm:string-width@^4.2.0 (glob 10 pulls it into countless projects); npm writes both entries under a name that differs from their path, and Kern reads each as a supply-chain swap: {surprises:#?}");
}

#[test]
fn a_page_in_a_nested_docs_folder_the_tests_read_still_runs_the_tests() {
    let t = Temp::new();
    let test = "import { readdirSync, readFileSync } from 'node:fs';\nimport { test, expect } from 'vitest';\nconst dir = new URL('../docs/', import.meta.url);\nfor (const name of readdirSync(dir, { recursive: true }).filter((n) => n.endsWith('.md'))) {\n  test(name, () => expect(readFileSync(new URL(name, dir), 'utf8').startsWith('# ')).toBe(true));\n}\n";
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("test/docs.test.js", test), ("docs/intro.md", "# Intro\n\nWelcome.\n"), ("docs/guides/setup.md", "# Setup\n\nInstall it.\n")]);
    let head = commit(&r, &[("docs/guides/setup.md", "Install it.\n")]);
    let out = check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
    let md = std::fs::read_to_string(t.0.join("out").join("receipt.md")).unwrap();
    assert_ne!(out["tests"], "NOTHING_TO_RUN",
        "test/docs.test.js reads every page under docs/, nested folders included, and the change removes the title of docs/guides/setup.md; Kern looks for code that names the file or its own folder 'guides', finds none, and clears the change as documentation:\n{md}");
}

#[test]
fn only_edited_test_files_that_ran_are_put_back_and_only_real_failures_count() {
    let t = Temp::new();
    let snapshot_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('snaps', () => expect(total([1])).toMatchSnapshot());\n";
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST), ("test/gone.test.js", TOTAL_TEST),
        ("test/snap.test.js", snapshot_test), ("test/__snapshots__/snap.test.js.snap", "exports[`snaps 1`] = `1`;\n"), ("test/never.test.js", TOTAL_TEST)]);
    std::fs::remove_file(r.join("test/gone.test.js")).unwrap();
    let head = commit(&r, &[("src/total.js", &TOTAL.replace("sum(items)", "sum([...items])")), ("test/total.test.js", &format!("{TOTAL_TEST}test('adds more', () => expect(total([1, 1])).toBe(2));\n")), ("test/new.test.js", TOTAL_TEST),
        ("test/__snapshots__/snap.test.js.snap", "exports[`snaps 1`] = `2`;\n"), ("test/never.test.js", &TOTAL_TEST.replace("toBe(5)", "toBe(6)"))]);
    let old = report(&[("test/total.test.js > adds", 1, 0, 0), ("test/gone.test.js > adds", 1, 0, 0), ("test/snap.test.js > snaps", 1, 0, 0), ("test/never.test.js > adds", 0, 1, 0)], &[]);
    let plan = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
    assert_eq!(plan.run.iter().collect::<Vec<_>>(), ["test/snap.kern-kept.test.js", "test/total.kern-kept.test.js"],
        "an edited test file and the owner of an edited snapshot are put back; a deleted, an added and a never-passing test file are not: {:?}", plan.files.keys());
    assert_eq!(plan.files.get("test/snap.kern-kept.test.js").map(String::as_str), Some(snapshot_test));
    assert_eq!(suite::kept_name("src/__tests__/cart.ts", 0).as_deref(), Some("src/__tests__/cart.kern-kept.ts"));
    assert_eq!(suite::kept_name("test/.hidden", 0), None);
    let kept = |rows: &[(&str, u32, u32, u32)], problems: &[&str]| suite::unkept(&report(rows, problems), &plan.back);
    let passing = kept(&[("test/total.kern-kept.test.js > adds", 1, 0, 0), ("test/snap.kern-kept.test.js > snaps", 1, 0, 0)], &[]);
    assert!(suite::rewritten(&old, &passing, None).is_empty(), "old versions that still pass are not rewritten");
    let unloadable = kept(&[("test/total.kern-kept.test.js > adds", 1, 0, 0)], &["test/snap.kern-kept.test.js"]);
    assert!(suite::rewritten(&old, &unloadable, None).is_empty(), "an old test file that cannot load on the new code (a moved module, say) is not a rewritten expectation");
    let failing = kept(&[("test/total.kern-kept.test.js > adds", 0, 1, 0), ("test/snap.kern-kept.test.js > snaps", 0, 1, 0)], &[]);
    let alone_old = kept(&[("test/total.kern-kept.test.js > adds", 0, 1, 0), ("test/snap.kern-kept.test.js > snaps", 1, 0, 0)], &[]);
    assert_eq!(suite::rewritten(&old, &failing, Some(&alone_old)), ["test/snap.test.js > snaps"], "a copy that fails on the old code too says nothing about the change");
    let mut tests = suite::compare(&old, &old, None);
    tests["edited"] = json!(2);
    let md = receipt::markdown(&receipt::build(&drive_json(), &json!({ "changed": [] }), &tests, &off()), None);
    assert!(md.contains("their old versions still pass on the new code (2 test files)"), "{md}");
}
