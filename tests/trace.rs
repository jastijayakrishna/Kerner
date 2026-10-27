use kern::trace::instrument;

fn appended(out: &str) -> Vec<&str> { out.lines().filter(|l| l.contains("__kern_trace")).collect() }

#[test]
fn every_exported_function_of_an_es_module_is_wrapped_after_it_is_defined() {
    let source = "import { db } from './db';\nexport function total(items: number[]): number { return items.reduce((s, i) => s + i, 0); }\nexport const fmt = (s: string) => s.trim(), other = 1;\nexport async function load(id: string) { return db.get(id); }\nconst inner = () => 2;\nexport class Repo { find(id: string) { return id; } }\nexport default function main() { return inner(); }\n";
    let out = instrument("packages/backend/src/queryBuilder.ts", source).expect("an ES module with exported functions is instrumented");
    assert!(out.starts_with(&source[..source.find("export const").unwrap()]), "nothing before the first wrapped binding changes:\n{out}");
    assert!(out.contains("export let fmt = (s: string) => s.trim(), other = 1;"), "a const that is wrapped later becomes let:\n{out}");
    for (name, label, how) in [("total", "total", "fn"), ("fmt", "fmt", "value"), ("other", "other", "value"), ("load", "load", "fn"), ("Repo", "Repo", "class"), ("main", "default", "fn")] {
        assert!(out.contains(&format!("{name} = __kern_trace.{how}(\"packages/backend/src/queryBuilder.ts\", \"{label}\", {name});")) || (how == "class" && out.contains(&format!("__kern_trace.class(\"packages/backend/src/queryBuilder.ts\", \"{label}\", {name});"))),
            "{name} is wrapped as {how} under the name importers use, {label}:\n{out}");
    }
    assert!(!out.contains("\"inner\""), "a function the module does not export is left alone:\n{out}");
    assert!(out.contains("import * as __kern_trace_module from \"/kern/trace.cjs\";"), "{out}");
    assert!(appended(&out).len() >= 7, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().enumerate().filter(|(_, l)| l.contains("__kern_trace")).all(|(i, _)| i > 0 && lines[i - 1] == "// @ts-ignore"),
        "every added line sits right below a type-checker directive, so ts-jest's diagnostics do not fail a test:\n{out}");
}

#[test]
fn names_exported_at_the_bottom_are_wrapped_too() {
    let source = "function sqlComment(s) { return s.replace(/--.*$/gm, ''); }\nconst parse = (s) => s.split(';');\nconst LIMIT = 10;\nexport { sqlComment, parse as parseAll, LIMIT };\n";
    let out = instrument("src/sql.js", source).unwrap();
    assert!(out.contains("sqlComment = __kern_trace.fn(\"src/sql.js\", \"sqlComment\", sqlComment);"), "{out}");
    assert!(out.contains("let parse = (s) => s.split(';');") && out.contains("parse = __kern_trace.value(\"src/sql.js\", \"parseAll\", parse);"), "recorded under the name importers use:\n{out}");
    assert!(out.contains("let LIMIT = 10;") && out.contains("LIMIT = __kern_trace.value(\"src/sql.js\", \"LIMIT\", LIMIT);"), "a value the runtime finds is not a function stays as it is:\n{out}");
}

#[test]
fn a_commonjs_module_wraps_what_it_exports_once_it_has_run() {
    let source = "const fs = require('fs');\nfunction read(p) { return fs.readFileSync(p, 'utf8'); }\nmodule.exports = { read };\n";
    let out = instrument("lib/read.js", source).unwrap();
    assert!(out.starts_with(source), "{out}");
    assert!(out.ends_with("module.exports = require(\"/kern/trace.cjs\").exports(\"lib/read.js\", module.exports);\n"), "{out}");
}

fn url(p: &std::path::Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

#[test]
fn the_runtime_records_what_each_exported_function_gave_for_what_it_was_given() {
    let dir = std::env::temp_dir().join(format!("kern-trace-{}", kern::util::uuid4().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = dir.join("trace.cjs");
    std::fs::write(&runtime, kern::sandbox::TRACE).unwrap();
    let esm = instrument("src/sql.mjs", "export function strip(s) { return s.replace(/--.*$/gm, '').trim(); }\nexport const join = async (a, b) => a + b;\nexport class Q { build(t) { return 'SELECT * FROM ' + t; } }\n").unwrap();
    std::fs::write(dir.join("sql.mjs"), esm.replace("\"/kern/trace.cjs\"", &format!("\"{}\"", url(&runtime)))).unwrap();
    let cjs = instrument("lib/add.js", "function add(a, b) { return a + b; }\nmodule.exports = { add, version: 2 };\n").unwrap();
    std::fs::write(dir.join("add.cjs"), cjs.replace("\"/kern/trace.cjs\"", &format!("{:?}", runtime.to_string_lossy()))).unwrap();
    let script = format!("import {{ strip, join, Q }} from {:?};\nimport {{ createRequire }} from 'node:module';\nconst {{ add, version }} = createRequire(import.meta.url)({:?});\n\
        strip('a -- b'); strip('x'); await join(1, 2); new Q().build('t'); try {{ strip(null); }} catch {{}}\nif (add(2, 3) !== 5 || version !== 2) process.exit(3);\n", url(&dir.join("sql.mjs")), dir.join("add.cjs").to_string_lossy());
    let trace = dir.join("trace.jsonl");
    let ran = std::process::Command::new("node").args(["--input-type=module", "-e", &script]).env("KERN_TRACE", &trace).output();
    let Ok(ran) = ran else { eprintln!("node is not installed here; the runtime is checked where it is"); let _ = std::fs::remove_dir_all(&dir); return };
    assert!(ran.status.success(), "the wrapped modules behave as before: {}", String::from_utf8_lossy(&ran.stderr));
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&trace).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let has = |k: &str, a: &str, o: &str| lines.iter().any(|l| l["k"] == k && l["a"] == a && l["o"].as_str().is_some_and(|x| x.starts_with(o)));
    assert!(has("src/sql.mjs#strip", "[\"a -- b\"]", "returns \"a\""), "{lines:?}");
    assert!(has("src/sql.mjs#strip", "[\"x\"]", "returns \"x\""), "{lines:?}");
    assert!(has("src/sql.mjs#strip", "[null]", "throws TypeError"), "{lines:?}");
    assert!(has("src/sql.mjs#join", "[1,2]", "resolves 3"), "an async result is recorded when it settles: {lines:?}");
    assert!(has("src/sql.mjs#Q#build", "[\"t\"]", "returns \"SELECT * FROM t\""), "{lines:?}");
    assert!(has("lib/add.js#add", "[2,3]", "returns 5"), "{lines:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn line(k: &str, a: &str, o: &str) -> String { format!("{}\n", serde_json::json!({ "k": k, "a": a, "o": o })) }

#[test]
fn untouched_tests_that_saw_the_changed_code_answer_differently_are_named_and_chance_is_set_aside() {
    let strip = "packages/backend/src/queryBuilder.ts#stripComments";
    let old = [line(strip, "[\"SELECT 1 -- note\"]", "returns \"SELECT 1\""), line(strip, "[\"SELECT 'http://a'\"]", "returns \"SELECT 'http://a'\""),
        line("src/id.ts#make", "[]", "returns \"a81f\""), line("src/user.ts#find", "[1]", "returns User{\"id\":1}"), line("src/user.ts#find", "[null]", "throws TypeError: no id"),
        line("src/user.ts#all", "[]", "returns [1,2]"), line("src/pay.ts#charge", "[0]", "throws Error: zero")].concat();
    let again = old.replace(&line("src/id.ts#make", "[]", "returns \"a81f\""), &line("src/id.ts#make", "[]", "returns \"77c2\""));
    assert_ne!(again, old);
    let new = [line(strip, "[\"SELECT 1 -- note\"]", "returns \"SELECT 1\""), line(strip, "[\"SELECT 'http://a'\"]", "returns \"SELECT 'http:\""),
        line("src/id.ts#make", "[]", "returns \"0d9e\""), line("src/user.ts#find", "[1]", "throws Error: not found"), line("src/user.ts#find", "[null]", "throws TypeError: no id"),
        line("src/user.ts#all", "[]", "returns []"), line("src/pay.ts#charge", "[0]", "returns 0"), line("src/new.ts#only", "[]", "returns 1")].concat();
    let c = kern::trace::compare(&old, &again, &new);
    let found: Vec<(String, String)> = c["differences"].as_array().unwrap().iter().map(|d| (d["key"].as_str().unwrap().to_string(), d["kind"].as_str().unwrap().to_string())).collect();
    assert_eq!(found, [("src/pay.ts#charge".to_string(), "accepts".to_string()), ("src/user.ts#find".to_string(), "throws".to_string()),
        ("src/user.ts#all".to_string(), "empty".to_string()), (strip.to_string(), "changed".to_string())],
        "worst first: an input the old code refused and the new code accepts, then a new throw, then an answer gone empty, then a plain change (lightdash/lightdash#12942 cut SQL at // inside a string): {c}");
    let sql = &c["differences"][3];
    assert_eq!((sql["input"].as_str(), sql["was"].as_str(), sql["now"].as_str()), (Some("[\"SELECT 'http://a'\"]"), Some("returns \"SELECT 'http://a'\""), Some("returns \"SELECT 'http:\"")));
    assert_eq!(c["unstable"], 1, "make() gave two answers on the old code alone, so its new answer proves nothing: {c}");
    assert_eq!(c["compared"], 7, "inputs seen on both sides, set-aside ones included: {c}");
}

#[test]
fn the_map_line_counts_in_plain_words_one_symbol_one_file_one_room() {
    let d = serde_json::json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = serde_json::json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let map = serde_json::json!({ "status": "RAN", "provenance": "scip-typescript 0.4.0", "documents": 4, "symbols": 4, "references": 8, "changed_functions": 1, "changed_types": 1, "files_running": 1, "files_loading": 1, "files_mentioning": 2, "files_declaring": 1, "test_files": 1,
        "nearest": "src/report.js", "complete": false, "rooms_reached": 1, "rooms": [{ "room": "src/cli.js", "kind": "command acme", "hops": 1, "how": "runs" }], "not_indexed": 0 });
    let tests = serde_json::json!({ "status": "RAN", "runner": "jest", "before": { "passed": 9, "failed": 0, "skipped": 0 }, "after": { "passed": 9, "failed": 0, "skipped": 0 },
        "kept": 9, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "map": map });
    let r = kern::receipt::build(&d, &rules, &tests, &serde_json::json!({ "status": "OFF", "why": "no AI key" }));
    let md = kern::receipt::markdown(&r, None);
    assert!(md.contains("Map: 4 files, 4 symbols and 8 references, from scip-typescript 0.4.0. This change edits 1 function; 1 file runs it (nearest `src/report.js`); 1 file runs it when they load; 1 test file calls it; 2 files only import it; edits 1 type that 1 file declares against; rooms reached: `src/cli.js` (1 away, runs) (at least: Kern stopped at its limit)"), "{md}");
    let mut commonjs = tests.clone();
    commonjs["map"] = serde_json::json!({ "status": "RAN", "provenance": "scip-typescript 0.4.0", "documents": 4, "symbols": 4, "references": 8, "changed_functions": 0, "changed_types": 0, "changed_modules": 1, "files_running": 0, "files_loading": 1, "files_mentioning": 0, "files_declaring": 0, "test_files": 2, "nearest": "src/report.js", "complete": true, "rooms_reached": 0, "rooms": [], "not_indexed": 0 });
    let md = kern::receipt::markdown(&kern::receipt::build(&d, &rules, &commonjs, &serde_json::json!({ "status": "OFF", "why": "no AI key" })), None);
    assert!(md.contains("This change edits the top level of 1 file; 0 files run it (nearest `src/report.js`); 1 file runs it when they load; 2 test files call it"), "a CommonJS file whose exports the index cannot see is edited at its top level: {md}");
    let mut unheld = tests.clone();
    unheld["map"] = serde_json::json!({ "status": "NOT_RUN", "why": "the index does not hold src/x.ts" });
    let md = kern::receipt::markdown(&kern::receipt::build(&d, &rules, &unheld, &serde_json::json!({ "status": "OFF", "why": "no AI key" })), None);
    assert!(md.contains("⏭️ Map: not used — the index does not hold src/x.ts; Kern used the files' imports instead"), "{md}");
}

#[test]
fn no_untouched_test_reached_the_changed_code_says_so() {
    let c = kern::trace::compare("", "", "");
    assert_eq!((c["compared"].as_u64(), c["differences"].as_array().map(Vec::len)), (Some(0), Some(0)));
    let cut = "not json\n{\"k\": 1}\n".repeat(3);
    assert_eq!(kern::trace::compare(&cut, &cut, &cut)["compared"], 0, "lines Kern cannot read are skipped, never guessed at");
}

#[test]
fn a_changed_answer_untouched_tests_did_not_check_is_a_note_with_both_values_and_never_decides() {
    let d = serde_json::json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = serde_json::json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let old = line("packages/backend/src/queryBuilder.ts#stripComments", "[\"SELECT 'http://a'\"]", "returns \"SELECT 'http://a'\"");
    let new = line("packages/backend/src/queryBuilder.ts#stripComments", "[\"SELECT 'http://a'\"]", "returns \"SELECT 'http:\"");
    let mut seen = kern::trace::compare(&old, &old, &new);
    seen["status"] = serde_json::json!("RAN");
    seen["tests"] = serde_json::json!(3);
    let tests = serde_json::json!({ "status": "RAN", "runner": "jest", "before": { "passed": 9, "failed": 0, "skipped": 0 }, "after": { "passed": 9, "failed": 0, "skipped": 0 },
        "kept": 9, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "bystanders": seen });
    let r = kern::receipt::build(&d, &rules, &tests, &serde_json::json!({ "status": "OFF", "why": "no AI key" }));
    assert_eq!(r["decision"], "clear", "until the farm shows what a changed answer predicts, it is shown, never decides: {r}");
    let note = r["notes"][0]["what"].as_str().unwrap_or("");
    assert!(note.contains("stripComments") && note.contains("SELECT 'http://a'") && note.contains("SELECT 'http:"), "{note}");
    let md = kern::receipt::markdown(&r, None);
    assert!(md.contains("🔍 Untouched tests") && md.contains("1 changed (see Notes)"), "{md}");
    let same = kern::trace::compare(&old, &old, &old);
    let mut same = same;
    same["status"] = serde_json::json!("RAN");
    let mut quiet = tests.clone();
    quiet["bystanders"] = same;
    assert!(kern::receipt::markdown(&kern::receipt::build(&d, &rules, &quiet, &serde_json::json!({ "status": "OFF", "why": "no AI key" })), None).contains("every answer stayed the same"));
}

#[test]
fn results_of_several_suites_add_up_and_a_suite_that_did_not_run_keeps_its_reason() {
    let ran = |n: u64| serde_json::json!({ "status": "RAN", "compared": n, "unstable": 1, "changed": 1, "tests": 2, "differences": [{ "key": format!("k{n}"), "input": "[]", "was": "returns 1", "now": "returns 2", "kind": "changed" }] });
    let both = kern::trace::merged(&ran(3), &ran(4));
    assert_eq!((both["compared"].as_u64(), both["changed"].as_u64(), both["differences"].as_array().map(Vec::len)), (Some(7), Some(2), Some(2)));
    let none = serde_json::json!({ "status": "NONE", "why": "no test this change leaves untouched loads the changed code" });
    assert_eq!(kern::trace::merged(&serde_json::json!({}), &none), none);
    assert_eq!(kern::trace::merged(&none, &ran(3))["status"], "RAN");
}

#[test]
fn files_kern_cannot_safely_rewrite_are_left_alone() {
    assert_eq!(instrument("src/broken.ts", "export function f( {"), None, "a file that does not parse");
    assert_eq!(instrument("src/types.ts", "export interface User { id: string }\nexport type Id = string;\n"), None, "nothing at run time to wrap");
    assert_eq!(instrument("src/index.ts", "export * from './a';\nexport { b } from './b';\n"), None, "re-exports belong to the files they come from");
    assert_eq!(instrument("src/data.json", "{\"a\": 1}"), None);
}
