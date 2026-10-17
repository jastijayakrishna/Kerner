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
    assert_eq!(planted[0].what(), "undoes the change to persist");
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

fn undos(files: &[(&str, &str, &str)]) -> Vec<kern::proof::Mutant> {
    let files: Vec<(String, String, String)> = files.iter().map(|(p, o, n)| (p.to_string(), o.to_string(), n.to_string())).collect();
    kern::suite::faults(&files, 23).into_iter().filter(|m| m.kind == FaultKind::Undo).collect()
}

#[test]
fn foxschema_133_each_changed_function_is_undone_on_its_own_so_a_checked_one_cannot_hide_an_unchecked_one() {
    let old = "export function split(sql) {\n  return sql.split(';');\n}\nexport function trim(s) {\n  return s.trim();\n}\nexport const quote = (s) => \"'\" + s + \"'\";\nexport function keep(s) { return s; }\n";
    let new = "export function split(sql) {\n  return sql.split(/;\\s*/);\n}\nexport function trim(s) {\n  return s.trim().replace(/--.*$/, '');\n}\nexport const quote = (s) => \"'\" + s.replace(/'/g, \"''\") + \"'\";\nexport function keep(s) { return s; }\n";
    let planted = undos(&[("packages/core/src/modules/sql-splitter.ts", old, new)]);
    let targets: Vec<Option<String>> = planted.iter().map(|m| m.target.clone()).collect();
    assert_eq!(targets, [Some("split".to_string()), Some("trim".to_string()), Some("quote".to_string())], "one undo per changed function, none for the unchanged one, and no whole-file undo when the functions cover the change: {planted:?}");
    for m in &planted {
        let back = applied(m, new);
        assert!(back != new && back != old, "each undo puts back only its own function: {back}");
        assert!(m.what().contains(m.target.as_deref().unwrap()), "{}", m.what());
    }
    assert_eq!(planted[1].line, 5, "the undo is reported at the first changed line inside the function");
}

#[test]
fn a_change_outside_any_function_and_a_class_method_get_their_own_undos() {
    let old = "export const LIMIT = 10;\nexport class Repo {\n  find(id) { return this.rows[id]; }\n  all() { return this.rows; }\n}\n";
    let new = "export const LIMIT = 20;\nexport class Repo {\n  find(id) { return this.rows[id] ?? null; }\n  all() { return this.rows; }\n}\n";
    let planted = undos(&[("src/repo.js", old, new)]);
    assert_eq!(planted.iter().map(|m| m.target.clone()).collect::<Vec<_>>(), [Some("Repo.find".to_string()), None],
        "the method is undone on its own, and the rest of the change (LIMIT) by an undo of its own: {planted:?}");
    assert_eq!(applied(&planted[1], new), new.replace("LIMIT = 20", "LIMIT = 10"),
        "the rest of the change puts back only what no function undo covers, so a test that catches Repo.find cannot make LIMIT look checked");
    assert!(planted[1].what().contains("rest of the change"), "{}", planted[1].what());
}

#[test]
fn faults_some_jest_commonjs_exports_are_undone_one_function_at_a_time() {
    let old = "exports.discount = (amount) => (amount >= 100 ? amount * 0.9 : amount);\nexports.free = (total) => total >= 50;\n";
    let new = "exports.discount = (amount) => (amount < 100 ? amount : amount * 0.9);\nexports.free = (total) => total > 60;\n";
    let planted = undos(&[("src/price.js", old, new)]);
    assert_eq!(planted.iter().map(|m| m.target.clone()).collect::<Vec<_>>(), [Some("discount".to_string()), Some("free".to_string())], "{planted:?}");
    let old = "module.exports.load = function (k) { return db[k]; };\nmodule.exports = { save(k, v) { db[k] = v; }, drop: (k) => delete db[k] };\n";
    let new = "module.exports.load = function (k) { return db[k] ?? null; };\nmodule.exports = { save(k, v) { db[k] = v ?? 0; }, drop: (k) => delete db[k] };\n";
    let planted = undos(&[("src/store.js", old, new)]);
    assert_eq!(planted.iter().map(|m| m.target.clone()).collect::<Vec<_>>(), [Some("load".to_string()), Some("save".to_string())], "{planted:?}");
}

#[test]
fn ocean_web_1278_a_change_typescript_erases_is_not_behaviour() {
    let same = [
        ("export type { Row } from './content';\n", "export type {\n  Row,\n  Item,\n  Total,\n} from './content';\n"),
        ("export { default } from './Summary';\nexport type { Props } from './Summary';\n", "export { default } from './Summary';\nexport type { Props } from './Summary';\nexport type { Row, Item } from './content';\n"),
        ("export interface P { a: string }\nexport type Q = P;\n", "export interface P { a: string; b?: number }\nexport type Q = P & { c: boolean };\n"),
        ("export function f(a: string): string { return a; }\n", "export function f(a: string | number): string { return a as string; }\n"),
        ("import { type A, b } from 'm';\nexport const g = (x: A) => b(x!);\n", "import { type A, type C, b } from 'm';\nexport const g = <T,>(x: A | C) => b(x!);\n"),
        ("export class K implements I { m(): void {} }\n", "export class K<T> implements I, J { m(): void {} }\n"),
        ("declare const x: number;\nexport function h(v: number): number;\nexport function h(v: any) { return v; }\n", "declare const x: string;\nexport function h(v: number): number;\nexport function h(v: string): string;\nexport function h(v: any) { return v; }\n"),
    ];
    for (old, new) in same {
        assert!(kern::proof::same_behaviour("src/a.ts", old, new), "only TypeScript types changed, which do not run:\n{old}\n->\n{new}");
        assert!(undos(&[("src/a.ts", old, new)]).is_empty(), "nothing to undo:\n{new}");
    }
    let differ = [
        ("export function f(a: string): string { return a; }\n", "export function f(a: string): string { return a.trim() as string; }\n"),
        ("export enum E { A = 1 }\n", "export enum E { A = 2 }\n"),
        ("import { type A, b } from 'm';\nexport const g = (x: A) => b(x);\n", "import { type A, c } from 'm';\nexport const g = (x: A) => c(x);\n"),
    ];
    for (old, new) in differ {
        assert!(!kern::proof::same_behaviour("src/a.ts", old, new), "a change that runs is a change, however many types surround it:\n{old}\n->\n{new}");
    }
}

#[test]
fn a_name_kern_cannot_tell_apart_falls_back_to_the_whole_file() {
    let old = "export function f(a) { return a; }\nfunction g() { function f() { return 1; } return f(); }\n";
    let new = "export function f(a) { return a + 1; }\nfunction g() { function f() { return 1; } return f(); }\n";
    let planted = undos(&[("src/f.js", old, new)]);
    assert!(planted.iter().any(|m| m.target.as_deref() == Some("f")), "{planted:?}");
    let twice_old = "if (a) { var h = () => 1; } else { var h = () => 2; }\nexport { h };\n";
    let twice_new = "if (a) { var h = () => 3; } else { var h = () => 2; }\nexport { h };\n";
    let planted = undos(&[("src/h.js", twice_old, twice_new)]);
    assert_eq!(planted.iter().map(|m| m.target.clone()).collect::<Vec<_>>(), [None], "h is declared twice, so only the whole file is undone: {planted:?}");
}

#[test]
fn a_test_the_pull_request_wrote_that_catches_the_undo_says_what_it_claims_the_change_does() {
    use kern::policy::Runner::Jest;
    let result = |status: &str, message: &str| json!({ "ancestorTitles": ["label"], "title": "labels many", "status": status, "failureMessages": if message.is_empty() { json!([]) } else { json!([message]) } });
    let report = |status: &str, message: &str| json!({ "numTotalTests": 1, "testResults": [{ "name": "/workspace/test/label.test.js", "status": "passed", "assertionResults": [result(status, message)] }] }).to_string();
    let new = kern::proof::read_report(Jest, report("passed", "").as_bytes()).unwrap();
    let placebo = json!({ "finished": true, "served": true, "output": report("passed", ""), "errors": "" });
    let caught = json!({ "finished": true, "served": true, "output": report("failed", "expect(received).toBe(expected)\n\nExpected: \"3 things\"\nReceived: \"3 items\"\n    at Object.<anonymous> (/workspace/test/label.test.js:4:30)"), "errors": "" });
    let missed = json!({ "finished": true, "served": true, "output": report("passed", ""), "errors": "" });
    let claims = kern::suite::fault_claims(Jest, &new, Some(&placebo), &[caught, missed]);
    assert_eq!(claims[0], json!({ "caught_by": [{ "test": "test/label.test.js > label > labels many", "now": "\"3 things\"", "before": "\"3 items\"" }] }),
        "the test expects the new answer and the old code gave the old one, so the reviewer reads the change as one claim");
    assert_eq!(claims[1], json!({ "ran": ["test/label.test.js"] }), "a missed undo names the test files that ran the code and still passed");
}

fn receipt_with(faults: serde_json::Value, bystanders: serde_json::Value) -> (serde_json::Value, String) {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "RAN", "runner": "jest", "before": { "passed": 4, "failed": 0, "skipped": 0 }, "after": { "passed": 4, "failed": 0, "skipped": 0 },
        "kept": 4, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "faults": faults, "bystanders": bystanders,
        "faults_missing": [{ "path": "src/notes.js", "why": "Kern found nothing in its change to put a fault in (only comments, formatting, logging or TypeScript types changed, or no line it knows how to change)" }] });
    let r = receipt::build(&d, &rules, &tests, &json!({ "status": "OFF", "why": "no AI key" }));
    let md = receipt::markdown(&r, None);
    (r, md)
}

#[test]
fn every_change_no_test_checks_becomes_an_exact_task_for_the_agent_and_the_claims_its_tests_make_are_listed() {
    let faults = json!([
        { "path": "src/label.js", "line": 1, "what": "undoes the change to label", "kind": "undo", "target": "label", "outcome": "missed", "ran": ["test/label.test.js"] },
        { "path": "src/price.js", "line": 3, "what": "undoes the change to discount", "kind": "undo", "target": "discount", "outcome": "caught",
          "caught_by": [{ "test": "test/price.test.js > discount > from 100", "now": "90", "before": "100" }] },
        { "path": "packages/backend/src/services/ProjectService/ProjectService.ts", "line": 160, "what": "undoes the change to ProjectService.compile", "kind": "undo", "target": "ProjectService.compile", "outcome": "untested" },
        { "path": "src/price.js", "line": 4, "what": "changes > to >=", "kind": "operator", "outcome": "caught" }]);
    let seen = json!({ "status": "RAN", "compared": 2, "unstable": 0, "changed": 1, "tests": 1,
        "differences": [{ "key": "src/label.js#label", "input": "[3]", "was": "returns \"3 items\"", "now": "returns \"3 things\"", "kind": "changed" }] });
    let (r, md) = receipt_with(faults, seen);
    assert_eq!(r["decision"], "clear", "tasks and claims never decide on their own: {r}");
    let tasks: Vec<(String, String)> = r["challenges"].as_array().unwrap().iter().map(|c| (c["target"].as_str().unwrap().to_string(), c["kind"].as_str().unwrap().to_string())).collect();
    assert_eq!(tasks, [("label".to_string(), "unasserted".to_string()), ("ProjectService.compile".to_string(), "unreached".to_string())], "{r}");
    assert!(md.contains("For the agent that wrote this change"), "{md}");
    assert!(md.contains("`test/label.test.js` runs it") && md.contains("`label(3)`") && md.contains("`\"3 items\"`") && md.contains("`\"3 things\"`"),
        "the task names the test file that already runs the code and an input whose answer changed: {md}");
    assert!(md.contains("No test reaches") && md.contains("ProjectService.ts") && md.contains("say in the pull request why"),
        "where no test reaches the code, the agent may explain instead of mocking a database: {md}");
    assert!(md.contains("`test/price.test.js > discount > from 100` expects `90`; with the change to `discount` undone it gets `100`"), "{md}");
    assert!(md.contains("Checked by a test: 1 of 3 changed functions"), "comment- or format-only files do not count against it: {md}");
    assert!(!md.contains("✅ Tests"), "the tests line never implies the change is tested when part of it is not: {md}");
}

#[test]
fn authkit_session_46_a_claim_quotes_the_full_values_the_test_compared_not_chai_s_shortened_message() {
    use kern::policy::Runner::Vitest;
    let title = "gives a user-facing message";
    let short = "expected PKCECookieMissingError: PKCE verifier coo\u{2026} { data: undefined } to match object { Object (name, message) }";
    let result = |status: &str, message: &str| json!({ "ancestorTitles": ["AuthKitCore"], "title": title, "status": status, "failureMessages": if message.is_empty() { json!([]) } else { json!([message]) } });
    let report = |status: &str, message: &str| json!({ "numTotalTests": 1, "testResults": [{ "name": "/workspace/src/core/AuthKitCore.spec.ts", "status": "passed", "assertionResults": [result(status, message)] }] }).to_string();
    let new = kern::proof::read_report(Vitest, report("passed", "").as_bytes()).unwrap();
    let placebo = json!({ "finished": true, "served": true, "output": report("passed", ""), "errors": "", "values": "" });
    let values = json!({ "file": "/workspace/src/core/AuthKitCore.spec.ts", "names": ["AuthKitCore"], "name": title, "message": short,
        "expected": "Object {\n  \"message\": \"Sign-in session could not be verified.\",\n}", "actual": "Object {\n  \"message\": \"PKCE verifier cookie missing\",\n}" }).to_string();
    let caught = json!({ "finished": true, "served": true, "errors": "", "values": format!("{values}\n{{\"file\": \"cut off"), "output": report("failed", &format!("Error: {short}")) });
    let claims = kern::suite::fault_claims(Vitest, &new, Some(&placebo), &[caught]);
    let claim = &claims[0]["caught_by"][0];
    assert!(claim["now"].as_str().is_some_and(|v| v.contains("Sign-in session could not be verified.")) && claim["before"].as_str().is_some_and(|v| v.contains("PKCE verifier cookie missing")),
        "the claim shows what the test expects and what it got in full, and a values file cut off mid-line is read up to the cut: {claims:?}");
}

#[test]
fn a_test_the_change_never_touched_that_fails_with_one_of_two_linked_functions_undone_is_not_called_the_change_s_own() {
    let faults = json!([
        { "path": "src/store.js", "line": 2, "what": "undoes the change to save", "kind": "undo", "target": "save", "outcome": "caught",
          "caught_by": [{ "test": "test/store.test.js > round trip", "now": "{\"total\":3}", "before": "undefined" }] },
        { "path": "src/store.js", "line": 6, "what": "undoes the change to load", "kind": "undo", "target": "load", "outcome": "caught",
          "caught_by": [{ "test": "test/store.test.js > round trip", "now": "{\"total\":3}", "before": "undefined" }] }]);
    let (r, md) = receipt_with(faults, json!({}));
    assert_eq!(r["decision"], "clear", "{r}");
    assert!(!md.contains("written or edited by this pull request") && !md.contains("before this change `undefined`"),
        "a test the change never touched can fail on a mix of old and new functions; the receipt must not say the change wrote it or that the old code gave that value: {md}");
    assert!(md.contains("expects `{\"total\":3}`; with the change to `save` undone it gets `undefined`"), "{md}");
}

#[test]
fn a_refactor_whose_code_the_tests_guard_counts_as_checked_unless_an_untouched_test_saw_its_answer_change() {
    let faults = |extra: serde_json::Value| {
        let mut list = vec![
            json!({ "path": "src/price.js", "line": 1, "span": [0, 60], "what": "undoes the change to discount", "kind": "undo", "target": "discount", "outcome": "missed", "ran": ["test/price.test.js"] }),
            json!({ "path": "src/price.js", "line": 1, "span": [30, 31], "what": "changes < to <=", "kind": "operator", "outcome": "caught" })];
        if !extra.is_null() { list.push(extra); }
        json!(list)
    };
    let (r, md) = receipt_with(faults(serde_json::Value::Null), json!({}));
    assert_eq!(r["challenges"], json!([]), "the tests catch a small fault in discount, and no answer of it changed, so undoing a same-behaviour rewrite proves nothing: {r}");
    assert!(md.contains("✅ Checked by a test: every changed function (1)"), "{md}");
    let seen = json!({ "status": "RAN", "compared": 1, "unstable": 0, "changed": 1, "tests": 1,
        "differences": [{ "key": "src/price.js#discount", "input": "[150]", "was": "returns 135", "now": "returns 150", "kind": "changed" }] });
    let (r, md) = receipt_with(faults(serde_json::Value::Null), seen);
    assert_eq!(r["challenges"][0]["kind"], "unasserted", "an untouched test saw discount(150) change and no test asserts it: {r}");
    assert!(md.contains("`discount(150)`") && md.contains("`135`") && md.contains("`150`"), "{md}");
    let (r, _) = receipt_with(faults(json!({ "path": "src/price.js", "line": 9, "span": [200, 201], "what": "changes > to >=", "kind": "operator", "outcome": "caught" })), json!({}));
    assert_eq!(r["challenges"], json!([]), "{r}");
    let elsewhere = json!([json!({ "path": "src/price.js", "line": 1, "span": [0, 60], "what": "undoes the change to discount", "kind": "undo", "target": "discount", "outcome": "missed", "ran": ["test/price.test.js"] }),
        json!({ "path": "src/price.js", "line": 1, "span": [70, 71], "what": "changes > to >=", "kind": "operator", "outcome": "caught" })]);
    let (r, md) = receipt_with(elsewhere, json!({}));
    assert_eq!(r["challenges"][0]["kind"], "unasserted", "a fault caught in another function says nothing about this one: {r}");
    assert!(md.contains("if it keeps behaviour the same, say so"), "{md}");
}

#[test]
fn faults_some_jest_a_check_mark_is_never_lent_from_a_neighbour_or_from_part_of_a_function() {
    let undo = |target: serde_json::Value, span: [u64; 2]| json!({ "path": "src/price.js", "line": 1, "span": span, "what": "undoes the change", "kind": "undo", "target": target, "outcome": "missed", "ran": ["test/price.test.js"] });
    let op = |at: u64, outcome: &str| json!({ "path": "src/price.js", "line": 1, "span": [at, at + 1], "what": "changes < to <=", "kind": "operator", "outcome": outcome });
    let (r, md) = receipt_with(json!([undo(json!("discount"), [0, 60]), op(10, "caught"), op(40, "missed")]), json!({}));
    assert_eq!(r["challenges"][0]["target"], "discount", "a small fault in discount that no test catches means part of it is unchecked: {r}");
    assert!(!md.contains("✅ Checked by a test"), "{md}");
    let (r, md) = receipt_with(json!([undo(serde_json::Value::Null, [0, 120]), op(10, "caught"), op(70, "caught")]), json!({}));
    assert_eq!(r["challenges"][0]["kind"], "unasserted", "the rest of a file's change is checked only when its own undo is caught: {r}");
    assert!(!md.contains("✅ Checked by a test"), "{md}");
    let (r, _) = receipt_with(json!([undo(json!("discount"), [0, 60]), op(10, "caught"), op(40, "unknown")]), json!({}));
    assert_eq!(r["challenges"][0]["target"], "discount", "a fault whose run Kern could not finish proves nothing: {r}");
}

#[test]
fn authkit_session_46_a_task_reads_right_when_several_test_files_run_the_code() {
    let faults = json!([{ "path": "src/core/AuthKitCore.ts", "line": 9, "what": "undoes the change to AuthKitCore.verifyCallbackState", "kind": "undo",
        "target": "AuthKitCore.verifyCallbackState", "outcome": "missed", "ran": ["src/core/AuthKitCore.spec.ts", "src/service/AuthService.spec.ts"] }]);
    let (_, md) = receipt_with(faults, json!({}));
    assert!(md.contains("`src/core/AuthKitCore.spec.ts`, `src/service/AuthService.spec.ts` run it, but still pass when your change is undone"), "{md}");
}

#[test]
fn a_change_every_test_checks_earns_its_check_mark_and_gives_the_agent_nothing_to_do() {
    let faults = json!([{ "path": "src/price.js", "line": 3, "what": "undoes the change to discount", "kind": "undo", "target": "discount", "outcome": "caught",
        "caught_by": [{ "test": "test/price.test.js > discount > from 100", "now": "90", "before": "100" }] }]);
    let (r, md) = receipt_with(faults, json!({}));
    assert_eq!(r["challenges"], json!([]));
    assert!(md.contains("✅ Checked by a test: every changed function (1)") && !md.contains("For the agent"), "{md}");
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
    assert!(receipt::markdown(&none, None).contains("the tests caught 0 of 3 small faults"));
    let some = receipt::build(&d, &rules, &tests(&["caught", "missed", "caught"]), &off);
    assert!(!some["look"].as_array().unwrap().iter().any(|x| x["kind"] == "untested-change"));
    let md = receipt::markdown(&some, None);
    assert!(md.contains("caught 2 of 3") && md.contains("src/price.js:3"), "{md}");
}
