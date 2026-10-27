use kern::drive;
use kern::util::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-drive-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
    fn join(&self, s: &str) -> PathBuf { self.0.join(s) }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
fn read(dir: &Path, path: &str) -> String { std::fs::read_to_string(dir.join(path)).unwrap_or_else(|e| panic!("{path}: {e}")) }

#[test]
fn changed_lines_on_each_side() {
    let (old, new) = drive::changed("a\nb\nc\nd", "a\nB\nc\nd\ne");
    assert_eq!(old, vec![2], "the old line the change replaced");
    assert_eq!(new, vec![2, 5], "the new line in its place and the added one");
    let (old, new) = drive::changed("x\ny\n", "x\ny\n");
    assert!(old.is_empty() && new.is_empty(), "an unchanged file has no changed lines");
    let (old, new) = drive::changed("", "one\ntwo");
    assert!(old.is_empty() && new == vec![1, 2], "a new file is all new lines: {new:?}");
}

#[test]
fn code_lines_skip_comments_imports_types_and_lone_brackets() {
    let source = [
        "// a comment",
        "import { x } from './x';",
        "export interface Opts {",
        "  rate: number;",
        "}",
        "export function tax(",
        "  amount: number,",
        "  opts: {",
        "    rate: number;",
        "  },",
        "): number {",
        "  /* block",
        "     comment */",
        "  return Math.round(amount * opts.rate + x);",
        "}",
        "export type Mode = 'a' | 'b';",
        "export { tax as default2 };",
        "declare const g: number;",
        "const y = 1; // trailing comment",
        "",
    ].join("\n");
    let lines: Vec<u32> = drive::measured("src/tax.ts", &source).into_keys().collect();
    assert_eq!(lines, vec![6, 7, 8, 11, 14, 19]);
    let jsx = "export function Card({ name }) {\n  return (\n    <div className=\"card\">\n      {/* a note */}\n      {name}\n    </div>\n  );\n}\n";
    let lines: Vec<u32> = drive::measured("src/Card.jsx", jsx).into_keys().collect();
    assert_eq!(lines, vec![1, 2, 3, 5, 6], "JSX lines run when the component renders; a lone JSX comment and closing brackets do not count");
    assert_eq!(drive::measured("src/Card.jsx", jsx)[&3], "    <div className=\"card\">", "each line keeps its text for the agent");
}

#[test]
fn exported_functions_are_found() {
    let esm = [
        "export function a() {}",
        "export const b = () => 1;",
        "const c = function () {};",
        "export { c as d };",
        "export default function () {}",
        "export const VALUE = 1;",
        "export class K {}",
        "export async function e() {}",
        "export type T = string;",
        "export { x } from './y';",
        "export * from './z';",
    ].join("\n");
    let e = drive::exported("src/m.ts", &esm);
    assert_eq!(e.functions, vec!["a", "b", "d", "default", "e"], "functions only: no values, classes, types or re-exports");
    assert!(!e.cjs);
    let arrow_default = drive::exported("src/n.js", "const run = async (x) => x;\nexport default run;\n");
    assert_eq!(arrow_default.functions, vec!["default"]);
    let cjs = drive::exported("src/c.js", "function f() {}\nmodule.exports = { f, g: () => 2 };\n");
    assert!(cjs.cjs, "CommonJS exports are wrapped when the module is required, by name at run time");
    assert!(drive::exported("src/d.js", "exports.h = function () {};\n").cjs);
    for (src, has) in [("export function a() {}\nexport default memo(Page);\n", true), ("export function a() {}\nexport default class K {}\n", true),
        ("export function a() {}\nconst x = 1;\nexport { x as default };\n", true), ("export function a() {}\nexport { default } from './y';\n", true),
        ("export function a() {}\nexport * from './z';\n", false)] {
        let e = drive::exported("src/p.tsx", src);
        assert_eq!((e.default, e.functions.contains(&"default".to_string())), (has, false), "{src}");
    }
}

fn project() -> (Temp, String, String) {
    let t = Temp::new();
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q"]);
    write(&r, "package.json", "{\"name\":\"shop\",\"private\":true}\n");
    write(&r, "README.md", "# Shop\n");
    write(&r, "src/price.ts", "// Prices.\nexport function priceFor(items: number): number {\n  if (items > 10) {\n    return items * 9;\n  }\n  return items * 10;\n}\n");
    write(&r, "src/price.test.ts", "import { priceFor } from './price';\ntest('ten', () => expect(priceFor(1)).toBe(10));\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "base"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    write(&r, "README.md", "# Shop\n\nBulk discounts.\n");
    write(&r, "src/price.ts", "// Prices.\nimport { round } from './util';\nexport function priceFor(items: number): number {\n  if (items >= 10) {\n    return round(items * 8.5);\n  }\n  return items * 10;\n}\nexport const free = () => 0;\n");
    write(&r, "src/util.js", "export function round(x) {\n  return Math.round(x);\n}\n");
    write(&r, "src/price.test.ts", "import { priceFor } from './price';\ntest('bulk', () => expect(priceFor(10)).toBe(85));\n");
    write(&r, "dist/bundle.min.js", &format!("var a={};{}\n", "1", "x+".repeat(3000)));
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Bulk price from ten items"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    (t, base, head)
}

#[test]
fn prepare_writes_the_task_for_a_commit_pair() {
    let (t, base, head) = project();
    let out = t.join("out");
    let summary = drive::prepare(&t.join("repo"), &base, &head, &out, Some("# Bulk price\n\nTen or more items cost 8.5 each.\n")).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    assert_eq!(d["base"], base.as_str());
    let files: Vec<&str> = d["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, vec!["src/price.ts", "src/util.js"], "only changed code is measured: not tests, docs or package.json");
    let price = &d["files"][0];
    assert_eq!(price["old"], serde_json::json!([3, 4]), "the old condition and the old discount line");
    assert_eq!(price["new"], serde_json::json!([4, 5, 9]), "the new condition, the new discount line and the new export; the import is not code");
    assert_eq!(d["files"][1]["new"], serde_json::json!([1, 2]), "a new file is measured on the new side only");
    assert_eq!(d["totals"], serde_json::json!({ "new": 5, "old": 2 }));
    assert_eq!(summary["totals"], d["totals"]);
    let skipped = d["skipped"].as_array().unwrap();
    assert!(skipped.iter().any(|s| s["path"] == "dist/bundle.min.js" && s["why"].as_str().unwrap().contains("minified")), "{skipped:?}");
    let modules = d["modules"].as_array().unwrap();
    let price_mod = modules.iter().find(|m| m["path"] == "src/price.ts").unwrap();
    assert_eq!(price_mod["new"]["functions"], serde_json::json!(["free", "priceFor"]));
    assert_eq!(price_mod["old"]["functions"], serde_json::json!(["priceFor"]));
    assert!(read(&out, "task/task.md").contains("driver.mjs"));
    assert!(read(&out, "task/pr.md").contains("Ten or more items"));
    assert!(read(&out, "task/diff.patch").contains("src/price.ts") && !read(&out, "task/diff.patch").contains("bundle.min.js"));
    let lines = read(&out, "task/lines.md");
    assert!(lines.contains("return round(items * 8.5);") && lines.contains("if (items > 10) {"), "{lines}");
    assert_eq!(read(&out, "task/new/src/price.ts"), read(&t.join("repo"), "src/price.ts"));
    assert!(read(&out, "task/old/src/price.ts").contains("items > 10"));
    assert!(out.join("task/new/src/util.js").exists() && !out.join("task/old/src/util.js").exists());
    assert!(!out.join("task/new/dist/bundle.min.js").exists(), "the agent's copy leaves out generated bundles");
}

#[test]
fn the_comment_names_changed_files_kern_did_not_measure() {
    let (t, base, head) = project();
    let out = t.join("out");
    drive::prepare(&t.join("repo"), &base, &head, &out, None).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    let kind = |p: &str| d["unmeasured"].as_array().unwrap().iter().find(|u| u["path"] == p).map(|u| u["why"].as_str().unwrap().to_string());
    assert_eq!((kind("README.md").as_deref(), kind("src/price.test.ts").as_deref()), (Some("documentation"), Some("tests")), "{}", d["unmeasured"]);
    let all = covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (6, 1), (8, 1), (9, 1)]);
    let mut r = run(&[], all.clone(), &[], &[], "");
    r.coverage["/workspace/src/util.js"] = serde_json::json!({ "statementMap": { "0": { "start": { "line": 1 }, "end": { "line": 3 } } }, "s": { "0": 1 } });
    let comment = drive::comment(&d, &drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1));
    assert!(comment.contains("Every changed line Kern measured ran") && !comment.contains("Every changed line of code ran"), "{comment}");
    assert!(comment.contains("did not measure") && comment.contains("README.md") && comment.contains("price.test.ts"), "{comment}");
}

#[test]
fn a_huge_comment_is_cut_on_a_character_and_keeps_its_notices() {
    let lines: Vec<u64> = (1..40_000).step_by(2).collect();
    let d = serde_json::json!({ "files": [{ "path": "src/größe.ts", "status": "modified", "old": [], "new": lines, "old_text": [],
        "new_text": lines.iter().map(|l| serde_json::json!([l, "x();"])).collect::<Vec<_>>() }], "skipped": [], "totals": { "new": lines.len(), "old": 0 } });
    let mut report = drive::score(&d, &[run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], "")], 1);
    report["stubs"] = serde_json::json!({ "used": ["react-native"], "refused": [] });
    let comment = drive::comment(&d, &report);
    assert!(comment.len() <= 61_000 && comment.contains("react-native"), "{} bytes: {}", comment.len(), &comment[..comment.len().min(600)]);
}

#[cfg(unix)]
#[test]
fn file_names_are_never_read_as_git_patterns() {
    let (t, base, _) = project();
    let r = t.join("repo");
    write(&r, ":(zz)evil.js", "export const evil = () => 1;\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "a file named like a pattern"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    drive::prepare(&r, &base, &head, &t.join("out"), None).unwrap();
    assert!(read(&t.join("out"), "task/diff.patch").contains(":(zz)evil.js"));
}

#[test]
fn the_comment_warns_of_installs_without_the_lockfile_and_of_code_that_touches_kern_s_measurement() {
    let d = priced();
    let r = run(&[], covered("src/price.ts", &[(3, 1), (4, 1)]), &[], &[], "");
    let mut report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    report["installs"] = serde_json::json!({ "new": [{ "dir": ".", "manager": "npm", "result": "installed from package.json (the lockfile is out of date)" }],
        "old": [{ "dir": "app", "manager": "bun", "result": "installed from package.json (the project has no lockfile)" }] });
    let comment = drive::comment(&d, &report);
    assert!(comment.contains("without the project's lockfile") && comment.contains("`.` (new code: the lockfile is out of date)")
        && comment.contains("`app` (old code: the project has no lockfile)"), "{comment}");
    let (t, base, _) = project();
    let r = t.join("repo");
    write(&r, "src/report.ts", "export const out = () => process.env.NODE_V8_COVERAGE;\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "reads the coverage folder"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    drive::prepare(&r, &base, &head, &t.join("out"), None).unwrap();
    let d: Value = serde_json::from_str(&read(&t.join("out"), "drive.json")).unwrap();
    assert_eq!(d["measurement"], serde_json::json!(["src/report.ts"]), "{}", d["measurement"]);
    let comment = drive::comment(&d, &drive::score(&d, &[run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], ""), run(&[], Value::Null, &[], &[], "")], 1));
    assert!(comment.contains("Kern's own measurement") && comment.contains("src/report.ts"), "{comment}");
}

#[test]
fn a_yarnrc_loses_the_code_it_would_run_and_keeps_its_settings() {
    let rc = "yarnPath: .yarn/releases/evil.cjs\nnodeLinker: node-modules\nplugins:\n  - path: .yarn/plugins/evil.cjs\n    spec: evil\n- path: .yarn/plugins/b.cjs\nnpmRegistryServer: \"https://registry.npmjs.org\"\n";
    let clean = kern::sandbox::yarnrc_without_code(rc);
    assert!(!clean.contains("yarnPath") && !clean.contains("plugins") && !clean.contains("evil") && !clean.contains("b.cjs"), "{clean}");
    assert!(clean.contains("nodeLinker: node-modules") && clean.contains("npmRegistryServer"), "{clean}");
}

#[test]
fn a_changed_file_with_its_own_source_map_is_named_as_touching_kern_s_measurement() {
    let (t, base, _) = project();
    let r = t.join("repo");
    write(&r, "src/mapped.js", "export const f = () => 1;\n//# sourceMappingURL=data:application/json;base64,e30=\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "a file with its own source map"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    drive::prepare(&r, &base, &head, &t.join("out"), None).unwrap();
    let d: Value = serde_json::from_str(&read(&t.join("out"), "drive.json")).unwrap();
    assert_eq!(d["measurement"], serde_json::json!(["src/mapped.js"]), "{}", d["measurement"]);
}

#[test]
fn a_change_to_many_files_fits_any_command_line() {
    let (t, base, _) = project();
    let r = t.join("repo");
    for i in 0..600 { write(&r, &format!("docs/a-rather-long-folder-name-for-guides/page-number-{i:04}-of-the-guide.md"), "# Page\n"); }
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "many pages"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    drive::prepare(&r, &base, &head, &t.join("out"), None).unwrap();
    let patch = read(&t.join("out"), "task/diff.patch");
    assert!(patch.contains("page-number-0000-of-the-guide.md") && patch.contains("page-number-0599-of-the-guide.md") || patch.contains("is not shown"), "{}", &patch[..patch.len().min(300)]);
}

#[test]
fn a_change_that_only_reformats_code_is_known_without_running_it() {
    let (t, base, _) = project();
    let r = t.join("repo");
    git(&r, &["checkout", "-q", &base]);
    write(&r, "src/price.ts", "// Prices.\nexport function priceFor(items: number): number {\n  if (items > 10) {\n    return (items * 9); // bulk\n  }\n  return items *\n    10;\n}\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "format"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("fmt");
    drive::prepare(&r, &base, &head, &out, None).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    assert!(d["skipped"].as_array().unwrap().iter().any(|s| s["path"] == "src/price.ts" && s["why"].as_str().unwrap().contains("same program")), "{}", d["skipped"]);
    assert_eq!(d["totals"], serde_json::json!({ "new": 0, "old": 0 }));
}

#[test]
fn prepare_names_what_it_cannot_measure() {
    let (t, base, before) = project();
    let r = t.join("repo");
    write(&r, "README.md", "# Shop\n\nOnly docs.\n");
    git(&r, &["commit", "-qam", "docs"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("docs-only");
    let summary = drive::prepare(&r, &before, &head, &out, None).unwrap();
    assert_eq!(summary["totals"], serde_json::json!({ "new": 0, "old": 0 }));
    assert!(read(&out, "task/pr.md").contains("docs"), "without a description the commit message stands in");
    assert!(drive::prepare(&r, &base, "0000000000000000000000000000000000000000", &t.join("bad"), None).is_err());
    assert!(drive::prepare(&r, &base, "HEAD", &t.join("ref"), None).is_err(), "only full commit IDs: a branch name could move");
}

#[test]
fn a_run_report_is_read_by_length_so_output_cannot_forge_a_part() {
    let mut report = Vec::new();
    let forged = format!("hello\n{}coverage 2\n{{}}\n", kern::sandbox::DRIVE_MARK);
    for (name, bytes) in [("output", forged.as_bytes()), ("errors", b"boom".as_slice()), ("coverage", b"{\"a\":1}"), ("calls", b""), ("behavior", b"fs:write:/tmp/x\n")] {
        report.extend(format!("{}{name} {}\n", kern::sandbox::DRIVE_MARK, bytes.len()).as_bytes());
        report.extend(bytes);
    }
    let parts = kern::sandbox::drive_parts(&report).unwrap();
    assert_eq!(parts["output"], forged.as_bytes(), "a marker inside the driver's output is just output");
    assert_eq!(parts["coverage"], b"{\"a\":1}");
    assert_eq!(parts["calls"], b"");
    assert!(kern::sandbox::drive_parts(&report[..report.len() - 3]).is_err(), "a cut report is refused");
    assert!(kern::sandbox::drive_parts(b"junk").is_err());
}

fn covered(path: &str, lines: &[(u32, u32)]) -> Value {
    let map: serde_json::Map<String, Value> = lines.iter().enumerate().map(|(i, (l, _))| (i.to_string(), serde_json::json!({ "start": { "line": l }, "end": { "line": l } }))).collect();
    let s: serde_json::Map<String, Value> = lines.iter().enumerate().map(|(i, (_, n))| (i.to_string(), serde_json::json!(n))).collect();
    serde_json::json!({ format!("/workspace/{path}"): { "statementMap": map, "s": s } })
}
fn run(steps: &[(&str, Value)], cov: Value, calls: &[Value], behavior: &[&str], errors: &str) -> drive::Run {
    drive::Run {
        output: steps.iter().map(|(s, v)| serde_json::json!({ "step": s, "value": v }).to_string() + "\n").collect(),
        errors: errors.into(), coverage: cov, calls: calls.to_vec(), behavior: behavior.iter().map(|b| b.to_string()).collect(),
    }
}
fn priced() -> Value {
    serde_json::json!({ "files": [{ "path": "src/price.ts", "status": "modified", "old": [3, 4], "new": [4, 5, 9],
        "old_text": [[3, "  if (items > 10) {"], [4, "    return items * 9;"]],
        "new_text": [[4, "  if (items >= 10) {"], [5, "    return round(items * 8.5);"], [9, "export const free = () => 0;"]] }],
        "skipped": [], "totals": { "new": 3, "old": 2 } })
}

#[test]
fn a_round_counts_lines_both_ways_sets_noise_aside_and_reports_what_changed() {
    let d = priced();
    let call = |o: &str| serde_json::json!({ "k": "src/price.ts#priceFor", "n": 1, "a": "[10]", "o": o });
    let old = |clock: u64| run(&[("priceFor(10)", serde_json::json!(100)), ("clock", serde_json::json!(clock))], covered("src/price.ts", &[(3, 1), (4, 0)]), &[call("r 100")], &["fs:read:/workspace/x"], "");
    let new = |clock: u64, nine: u32| run(&[("priceFor(10)", serde_json::json!(85)), ("clock", serde_json::json!(clock))], covered("src/price.ts", &[(4, 1), (5, 2), (9, nine)]), &[call("r 85")],
        &["fs:read:/workspace/x", "net:api.example.com:443"], "TypeError: boom");
    let report = drive::score(&d, &[old(1), old(2), new(7, 0), new(8, 1)], 1);
    assert_eq!(report["coverage"]["new"], serde_json::json!({ "ran": 3, "total": 3 }), "a line counts when it ran in either run of its side");
    assert_eq!(report["coverage"]["old"], serde_json::json!({ "ran": 1, "total": 2 }));
    assert_eq!(report["coverage"]["percent"], 80);
    assert_eq!(report["files"][0]["old"]["missed"], serde_json::json!([[4, "    return items * 9;"]]));
    assert_eq!(report["steps"], serde_json::json!({ "old": 2, "new": 2, "same": 0, "different": 1, "unstable": 1 }), "a step that differs between two runs of one side is noise");
    assert_eq!(report["differences"][0]["step"], "priceFor(10)");
    assert_eq!(report["calls"]["different"], 1, "Kern's own record catches the change whatever the driver printed");
    assert_eq!(report["call_differences"][0]["call"], "src/price.ts#priceFor");
    assert_eq!(report["behavior"]["new"], serde_json::json!(["net:api.example.com:443"]));
    let feedback = drive::feedback(&d, &report);
    assert!(feedback.contains("    4:     return items * 9;") && feedback.contains("TypeError: boom") && feedback.contains("1 of 2"), "{feedback}");
    let comment = drive::comment(&d, &report);
    assert!(comment.contains("80%") && comment.contains("priceFor") && comment.contains("api.example.com"), "{comment}");
    assert!(comment.contains("src/price.ts") && comment.contains("line 4"), "the comment names the lines nobody ran: {comment}");
}

#[test]
fn the_comment_is_honest_when_nothing_ran_and_safe_with_hostile_values() {
    let d = priced();
    let empty = run(&[], Value::Null, &[], &[], "kern: the driver failed: SyntaxError");
    let report = drive::score(&d, &[empty.clone(), empty.clone(), empty.clone(), empty], 1);
    assert_eq!(report["coverage"]["percent"], 0);
    let comment = drive::comment(&d, &report);
    assert!(comment.contains("0%") && comment.contains("not run"), "{comment}");
    let evil = "`</details><img src=x onerror=alert(1)>\n# heading";
    let old = run(&[(evil, serde_json::json!(evil.repeat(5000)))], covered("src/price.ts", &[(3, 1), (4, 1)]), &[], &[], "");
    let new = run(&[(evil, serde_json::json!(1))], covered("src/price.ts", &[(4, 1), (5, 1), (9, 1)]), &[], &[], "");
    let report = drive::score(&d, &[old.clone(), old, new.clone(), new], 1);
    let comment = drive::comment(&d, &report);
    let outside: String = comment.split('`').step_by(2).collect();
    assert!(!outside.contains("<img") && !outside.contains("</details>") && !comment.lines().any(|l| l.starts_with("# heading")) && comment.len() < 60_000, "{}", &comment[..comment.len().min(2000)]);
}

#[test]
fn dependencies_are_installed_where_the_changed_code_lives() {
    let paths: std::collections::BTreeSet<String> = ["package.json", "pnpm-workspace.yaml", "packages/a/package.json", "packages/a/src/x.ts", "tools/b/package.json", "tools/b/y.js", "web/src/z.ts"]
        .iter().map(|s| s.to_string()).collect();
    let code = vec!["packages/a/src/x.ts".to_string(), "tools/b/y.js".to_string(), "web/src/z.ts".to_string()];
    let dirs = drive::install_dirs(&paths, &code, Some("{\"name\":\"r\"}"), Some("packages:\n  - 'packages/*'\n"));
    assert_eq!(dirs, vec![".", "tools/b"], "the root installs its workspace packages; a folder outside the workspace gets its own install");
    let npm = drive::install_dirs(&paths, &code, Some("{\"workspaces\":[\"tools/*\"]}"), None);
    assert_eq!(npm, vec![".", "packages/a"]);
    let none: std::collections::BTreeSet<String> = ["src/a.js".to_string()].into_iter().collect();
    assert!(drive::install_dirs(&none, &["src/a.js".to_string()], None, None).is_empty(), "nothing to install without a package.json");
}

#[test]
fn kern_s_driver_tools_pass_its_own_install_rules() {
    let tools = drive::tools();
    kern::flow::validate_lockfile(&tools, "registry.npmjs.org").expect("every tool is pinned and comes from the public registry");
    let pkg: Value = serde_json::from_slice(tools["package.json"].bytes.as_ref().unwrap()).unwrap();
    for name in ["tsx", "c8", "jsdom"] { assert!(pkg["dependencies"][name].as_str().is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.')), "{name} is pinned exactly"); }
    for (has, extends) in [(true, true), (false, false)] {
        let t: Value = serde_json::from_str(&drive::tsconfig(has)).unwrap();
        assert_eq!(t["extends"] == "/workspace/tsconfig.json", extends);
        assert_eq!(t["compilerOptions"]["jsx"], "react-jsx");
        assert_eq!(t["include"], serde_json::json!(["/workspace/**/*"]));
    }
}

#[test]
fn the_sandbox_scripts_are_valid_javascript() {
    let dir = Temp::new();
    for (name, text) in [("drive-loader.mjs", drive::LOADER), ("drive-hooks.mjs", drive::HOOKS), ("drive-support.mjs", drive::SUPPORT), ("drive-browser.mjs", drive::BROWSER), ("drive-run.mjs", drive::RUNNER)] {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        match Command::new("node").arg("--check").arg(&p).output() {
            Ok(out) => assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr)),
            Err(_) => eprintln!("node is not installed here; the syntax of {name} is checked where it is"),
        }
    }
}

#[test]
fn rounds_continue_only_while_they_help() {
    assert!(drive::another_round(&[], 3), "the first round always runs");
    assert!(drive::another_round(&[40], 3), "a second round follows the first unless it is done");
    assert!(!drive::another_round(&[96], 3), "95% or more is done");
    assert!(drive::another_round(&[40, 70], 3), "a gain of 5 points or more earns another round");
    assert!(!drive::another_round(&[40, 42], 3), "a round that barely helped ends the loop");
    assert!(!drive::another_round(&[10, 40, 70], 3), "never more rounds than allowed");
}

type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;
fn stub_github(route: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> (String, Seen) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Default::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut reader = std::io::BufReader::new(s.try_clone().unwrap());
            let mut request = String::new();
            if std::io::BufRead::read_line(&mut reader, &mut request).is_err() { continue; }
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = v.trim().parse().unwrap_or(0); }
            }
            let mut body = vec![0u8; length];
            let _ = std::io::Read::read_exact(&mut reader, &mut body);
            let parts: Vec<&str> = request.split_whitespace().collect();
            let (method, path) = (parts.first().copied().unwrap_or("").to_string(), parts.get(1).copied().unwrap_or("").to_string());
            log.lock().unwrap().push((method.clone(), path.clone(), String::from_utf8_lossy(&body).to_string()));
            let (status, reply) = route(&method, &path);
            let _ = std::io::Write::write_all(&mut s, format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes());
        }
    });
    (url, seen)
}

#[test]
fn the_comment_is_posted_once_and_then_updated_in_place() {
    let t = Temp::new();
    let out = t.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("comment.md"), "### Kern: what this change does when it runs\n\nKern ran **100%**\n").unwrap();
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[{\"id\":1,\"body\":\"looks good\"}]".into()),
        ("POST", "/repos/acme/shop/issues/7/comments") => (201, "{\"id\":2}".into()),
        _ => (404, "{}".into()),
    });
    let r = drive::post_with(&out, "acme/shop", 7, &url, "t0k3n", false).unwrap();
    assert_eq!(r["status"], "POSTED");
    let posted = seen.lock().unwrap().iter().find(|(m, _, _)| m == "POST").cloned().unwrap();
    let body: Value = serde_json::from_str(&posted.2).unwrap();
    assert!(body["body"].as_str().unwrap().contains("<!-- kern-drive -->") && body["body"].as_str().unwrap().contains("100%"));
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, concat!("[{\"id\":1,\"body\":\"looks good\",\"user\":{\"login\":\"dev\"}},",
            "{\"id\":5,\"body\":\"<!-- kern-drive -->\\nfake\",\"user\":{\"login\":\"mallory\"}},",
            "{\"id\":9,\"body\":\"<!-- kern-drive -->\\nold\",\"user\":{\"login\":\"github-actions[bot]\"}}]").into()),
        ("PATCH", "/repos/acme/shop/issues/comments/9") => (200, "{\"id\":9}".into()),
        _ => (404, "{}".into()),
    });
    let r = drive::post_with(&out, "acme/shop", 7, &url, "t0k3n", false).unwrap();
    assert_eq!(r["status"], "UPDATED");
    assert!(!seen.lock().unwrap().iter().any(|(m, _, _)| m == "POST"), "no second comment");
    assert!(!seen.lock().unwrap().iter().any(|(m, p, _)| m == "PATCH" && p.ends_with("/5")), "a comment someone else wrote with Kern's mark is never edited");
    assert!(drive::post_with(&out, "acme/shop/../x", 7, &url, "t0k3n", false).is_err(), "repository names are checked");
}

#[test]
fn a_run_of_an_older_commit_never_replaces_the_comment() {
    let t = Temp::new();
    let out = t.join("out");
    write(&out, "comment.md", "### Kern\n\nKern ran **100%**\n");
    write(&out, "drive.json", r#"{"head":"1111111111111111111111111111111111111111"}"#);
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", "/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        ("POST", _) => (201, "{\"id\":3}".into()),
        _ => (404, "{}".into()),
    });
    assert_eq!(drive::post_with(&out, "acme/shop", 7, &url, "t0k3n", false).unwrap()["status"], "STALE");
    assert!(!seen.lock().unwrap().iter().any(|(m, _, _)| m == "POST" || m == "PATCH"), "nothing written");
    let d = serde_json::json!({ "head": "1111111111111111111111111111111111111111", "files": [], "skipped": [], "totals": { "new": 0, "old": 0 } });
    let r = run(&[], Value::Null, &[], &[], "");
    assert!(drive::comment(&d, &drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1)).contains("1111111"), "the comment names the commit");
}

#[test]
fn a_change_with_nothing_to_run_only_updates_an_earlier_comment() {
    let t = Temp::new();
    let out = t.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("comment.md"), "### Kern: what this change does when it runs\n\nThis change touches no line of code Kern can run.\n").unwrap();
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        _ => (404, "{}".into()),
    });
    assert_eq!(drive::post_with(&out, "acme/shop", 7, &url, "t0k3n", true).unwrap()["status"], "SKIPPED");
    assert!(!seen.lock().unwrap().iter().any(|(m, _, _)| m == "POST"), "nothing posted");
    let (url, _) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[{\"id\":9,\"body\":\"<!-- kern-drive -->\\nold\",\"user\":{\"login\":\"github-actions[bot]\"}}]".into()),
        ("PATCH", "/repos/acme/shop/issues/comments/9") => (200, "{\"id\":9}".into()),
        _ => (404, "{}".into()),
    });
    assert_eq!(drive::post_with(&out, "acme/shop", 7, &url, "t0k3n", true).unwrap()["status"], "UPDATED");
}

#[cfg(unix)]
#[test]
fn the_agent_writes_the_driver_in_the_task_folder() {
    let t = Temp::new();
    let task = t.join("task");
    std::fs::create_dir_all(&task).unwrap();
    let argv: Vec<String> = ["sh", "-c", "printf '%s' \"$0\" > prompt.txt; echo 'console.log(1)' > driver.mjs; echo '{\"result\":\"done\"}'", "{prompt}"].iter().map(|s| s.to_string()).collect();
    let r = drive::agent_round(&task, &argv, "Read task.md", std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(read(&task, "prompt.txt"), "Read task.md", "the prompt replaces {{prompt}}");
    assert!(task.join("driver.mjs").exists() && r["exit"] == 0);
    let slow: Vec<String> = ["sh", "-c", "sleep 30"].iter().map(|s| s.to_string()).collect();
    let r = drive::agent_round(&task, &slow, "x", std::time::Duration::from_secs(1)).unwrap();
    assert_eq!(r["timed_out"], true, "an agent that runs too long is stopped");
    let loud: Vec<String> = ["sh", "-c", "head -c 3000000 /dev/zero | tr '\\0' x; echo; echo done > loud.txt"].iter().map(|s| s.to_string()).collect();
    let r = drive::agent_round(&task, &loud, "x", std::time::Duration::from_secs(20)).unwrap();
    assert!(r["timed_out"] == false && task.join("loud.txt").exists(), "{r}");
}

#[test]
fn stand_ins_replace_packages_only_and_within_limits() {
    let t = Temp::new();
    write(&t.0, "driver.mjs", "x");
    write(&t.0, "stubs/react-native.mjs", "export const View = 'View';\n");
    write(&t.0, "stubs/@expo/vector-icons.mjs", "export default {};\n");
    write(&t.0, "stubs/expo-router/stack.mjs", "export const Stack = 1;\n");
    write(&t.0, "stubs/notes.md", "why\n");
    write(&t.0, "stubs/@/lib/price.mjs", "export const priceFor = () => 0;\n");
    write(&t.0, "stubs/.hidden.mjs", "x\n");
    write(&t.0, "stubs/big.mjs", &"x".repeat(300 * 1024));
    let (used, refused) = drive::stubs(&t.0).unwrap();
    let names: Vec<&str> = used.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["@expo/vector-icons", "expo-router/stack", "react-native"]);
    for p in ["stubs/notes.md", "stubs/@/lib/price.mjs", "stubs/.hidden.mjs", "stubs/big.mjs"] { assert!(refused.iter().any(|r| r["path"] == p && r["why"].is_string()), "{p} not refused: {refused:?}"); }
    assert!(drive::stubs(&t.join("none")).unwrap().0.is_empty(), "no stubs folder, no stand-ins");
}

#[test]
fn the_comment_names_the_stand_ins_the_driver_used() {
    let d = priced();
    let mut r = run(&[], covered("src/price.ts", &[(3, 1), (4, 1)]), &[], &[], "");
    r.output = serde_json::json!({ "step": "import", "error": "Transform failed: /workspace/node_modules/react-native/index.js:27:7: Unexpected \"typeof\"" }).to_string() + "\n";
    let mut report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    report["stubs"] = serde_json::json!({ "used": ["react-native"], "refused": [{ "path": "stubs/notes.md", "why": "a stand-in is an .mjs file" }] });
    let comment = drive::comment(&d, &report);
    assert!(comment.contains("stand-in") && comment.contains("react-native"), "a reader learns which packages did not really run: {comment}");
    let feedback = drive::feedback(&d, &report);
    assert!(feedback.contains("stubs/notes.md") && feedback.contains("a stand-in is an .mjs file"), "{feedback}");
    assert!(feedback.contains("stubs/<package>.mjs"), "a package that failed to load points the author to stand-ins: {feedback}");
    assert!(feedback.contains("import (old and new code): Transform failed"), "a step that throws the same on both sides is still shown: {feedback}");
    let hint = |error: &str| {
        let mut r = run(&[], covered("src/price.ts", &[(3, 1)]), &[], &[], "");
        r.output = serde_json::json!({ "step": "load", "error": error }).to_string() + "\n";
        drive::feedback(&d, &drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1)).contains("stubs/<package>.mjs")
    };
    for e in ["Could not locate the bindings file. Tried: build/Release/fee.node", "Cannot find module 'sharp'", "Cannot find package '@acme/ui' imported from /workspace/src/x.ts",
        "The module '/x/addon.node' was compiled against a different Node.js version using NODE_MODULE_VERSION 115"] { assert!(hint(e), "{e}"); }
    assert!(!hint("Cannot find module '/workspace/src/features/new.ts' imported from /kern/driver.mjs"), "a new file missing from the old tree is no package");
    for alias in ["Cannot find module '@/rates'", "Cannot find module '~/lib/x'", "Cannot find package '#internal/db'"] { assert!(!hint(alias), "a path alias is the project's own code: {alias}"); }
}

#[test]
fn differences_are_sorted_by_what_the_pull_request_says() {
    let d = priced();
    let lines = covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]);
    let call = |o: &str| serde_json::json!({ "k": "src/price.ts#priceFor", "n": 1, "a": "[10]", "o": o });
    let old = || run(&[("receipt", serde_json::json!("10 items"))], lines.clone(), &[call("r 100")], &[], "");
    let new = || run(&[("receipt", serde_json::json!("10 items, bulk"))], lines.clone(), &[call("r 85")], &["net:api.example.com:443"], "");
    let mut report = drive::score(&d, &[old(), old(), new(), new()], 1);
    let md = drive::differences_md(&report);
    assert!(md.contains("D1") && md.contains("D2") && md.contains("D3") && !md.contains("D4"), "{md}");
    let t = Temp::new();
    std::fs::write(t.join("verdicts.json"), r#"{"D1":{"says":"yes","why":"Ten or more items cost 8.5 each"},"D2":{"says":"no","why":"nothing about a network call"},"D3":{"says":"unclear","why":"`<img src=x onerror=alert(1)>` maybe"}}"#).unwrap();
    report["sorted"] = drive::read_verdicts(&t.join("verdicts.json"), &report);
    let comment = drive::comment(&d, &report);
    let surprises = comment.find("Not mentioned by the pull request").unwrap_or_else(|| panic!("{comment}"));
    let described = comment.find("Described by the pull request").unwrap_or_else(|| panic!("{comment}"));
    let net = comment.find("api.example.com").unwrap();
    assert!(surprises < net && net < described, "the network call nobody asked for comes first: {comment}");
    let folded = comment.find("<details>").unwrap_or_else(|| panic!("{comment}"));
    let receipt = comment.find("receipt").unwrap();
    assert!(described < folded && folded < receipt && comment[receipt..].contains("unclear"), "{comment}");
    assert!(comment.contains("Ten or more items cost 8.5 each"), "{comment}");
    assert!(!comment.split('`').step_by(2).any(|s| s.contains("<img")), "the agent's words stay in code spans: {comment}");
    for bad in [r#"{"D9":{"says":"yes","why":"x"}}"#, r#"{"D1":{"says":"maybe","why":"x"}}"#, "not json", r#"["D1"]"#] {
        std::fs::write(t.join("verdicts.json"), bad).unwrap();
        report["sorted"] = drive::read_verdicts(&t.join("verdicts.json"), &report);
        let comment = drive::comment(&d, &report);
        assert!(!comment.contains("Not mentioned by the pull request") && comment.contains("api.example.com"), "{bad}: {comment}");
    }
}

#[test]
fn a_line_runs_only_when_none_of_its_code_was_left_out() {
    let lines = ["export const price = (n) => n * 1.5;", "  if (isAdmin) wipe();", "  if (items >= 10) {", "    return 1;"];
    let d = serde_json::json!({ "files": [{ "path": "src/a.js", "status": "modified", "old": [], "new": [1, 2, 3, 4], "old_text": [],
        "new_text": lines.iter().enumerate().map(|(i, t)| serde_json::json!([i + 1, t])).collect::<Vec<_>>() }], "skipped": [], "totals": { "new": 4, "old": 0 } });
    let at = |l: u32, c: u32| serde_json::json!({ "line": l, "column": c });
    let cov = serde_json::json!({ "/workspace/src/a.js": {
        "statementMap": (0..4).map(|i| (i.to_string(), serde_json::json!({ "start": at(i + 1, 0), "end": at(i + 1, 40) }))).collect::<serde_json::Map<_, _>>(),
        "s": (0..4).map(|i| (i.to_string(), serde_json::json!(1))).collect::<serde_json::Map<_, _>>(),
        "fnMap": { "0": { "name": "price", "decl": { "start": at(1, 21), "end": at(1, 35) }, "loc": { "start": at(1, 21), "end": at(1, 35) } } },
        "f": { "0": 0 },
        "branchMap": { "0": { "type": "branch", "locations": [{ "start": at(2, 15), "end": at(2, 22) }] }, "1": { "type": "branch", "locations": [{ "start": at(3, 19), "end": at(5, 3) }] } },
        "b": { "0": [0], "1": [0] } } });
    let r = run(&[], cov, &[], &[], "");
    let report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    let missed: Vec<u64> = report["files"][0]["new"]["missed"].as_array().unwrap().iter().map(|x| x[0].as_u64().unwrap()).collect();
    assert_eq!(missed, vec![1, 2, 4], "{}", report["files"]);
    let feedback = drive::feedback(&d, &report);
    assert!(feedback.contains("not run: `wipe();`") && feedback.contains("not run: `(n) => n * 1.5`"), "{feedback}");
    let d = serde_json::json!({ "files": [{ "path": "src/x.js", "status": "modified", "old": [], "new": [1], "old_text": [], "new_text": [[1, "export const x = () => 2;"]] }],
        "skipped": [], "totals": { "new": 1, "old": 0 } });
    let span = |a: u32, b: u32| serde_json::json!({ "start": at(1, a), "end": at(1, b) });
    let cov = serde_json::json!({ "/workspace/src/x.js": {
        "statementMap": { "0": span(0, 25) }, "s": { "0": 1 },
        "fnMap": { "0": { "name": "__export", "loc": span(0, 7) }, "1": { "name": "get", "loc": span(0, 7) }, "2": { "name": "x", "loc": span(0, 7) } },
        "f": { "0": 1, "1": 0, "2": 2 },
        "branchMap": { "0": { "locations": [span(17, 23)] }, "1": { "locations": [span(0, 7)] }, "2": { "locations": [span(17, 25)] } },
        "b": { "0": [0], "1": [0], "2": [1] } } });
    let r = run(&[], cov, &[], &[], "");
    let report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    assert_eq!(report["coverage"]["new"]["ran"], 1, "{}", report["files"]);
}

#[test]
fn a_branch_left_out_counts_where_its_span_starts_at_its_operator() {
    let lines = ["  return isNumber(n) ? twice(Number(n)) + 1 : 0;", "  const label = ok ? 'yes' : 'no';", "  const v = x || fallback();"];
    let d = serde_json::json!({ "files": [{ "path": "src/b.ts", "status": "modified", "old": [], "new": [1, 2, 3], "old_text": [],
        "new_text": lines.iter().enumerate().map(|(i, t)| serde_json::json!([i + 1, t])).collect::<Vec<_>>() }], "skipped": [], "totals": { "new": 3, "old": 0 } });
    let at = |l: u32, c: u32| serde_json::json!({ "line": l, "column": c });
    let span = |l: u32, a: u32, b: u32| serde_json::json!({ "start": at(l, a), "end": at(l, b) });
    let cov = serde_json::json!({ "/workspace/src/b.ts": {
        "statementMap": (0..3).map(|i| (i.to_string(), span(i + 1, 0, 50))).collect::<serde_json::Map<_, _>>(),
        "s": (0..3).map(|i| (i.to_string(), serde_json::json!(1))).collect::<serde_json::Map<_, _>>(),
        "fnMap": {}, "f": {},
        "branchMap": { "0": { "locations": [span(1, 43, 47)] }, "1": { "locations": [span(2, 18, 26)] }, "2": { "locations": [span(3, 13, 27)] } },
        "b": { "0": [0], "1": [0], "2": [0] } } });
    let r = run(&[], cov, &[], &[], "");
    let report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    let missed: Vec<u64> = report["files"][0]["new"]["missed"].as_array().unwrap().iter().map(|x| x[0].as_u64().unwrap()).collect();
    assert_eq!(missed, vec![1, 2, 3], "{}", report["files"]);
}

#[test]
fn the_comment_keeps_what_every_round_saw() {
    let d = priced();
    let call = |o: &str| serde_json::json!({ "k": "src/price.ts#priceFor", "n": 1, "a": "[-1]", "o": o });
    let one_old = run(&[], covered("src/price.ts", &[(3, 1), (4, 1)]), &[call("r 0")], &[], "");
    let one_new = run(&[], covered("src/price.ts", &[(4, 1)]), &[call("e RangeError: bad count")], &[], "");
    let two = run(&[], covered("src/price.ts", &[(5, 1), (9, 1)]), &[], &[], "");
    let r1 = drive::score(&d, &[one_old.clone(), one_old, one_new.clone(), one_new], 1);
    let r2 = drive::score(&d, &[two.clone(), two.clone(), two.clone(), two], 2);
    let merged = drive::merge(&[r1, r2]);
    assert_eq!(merged["coverage"]["percent"], 100, "{}", merged["coverage"]);
    let comment = drive::comment(&d, &merged);
    assert!(comment.contains("RangeError: bad count") && comment.contains("100%"), "{comment}");
}

#[test]
fn a_run_that_ends_early_on_one_side_is_never_reported_as_the_same() {
    let d = priced();
    let lines = covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]);
    let old = || run(&[("load", serde_json::json!(1)), ("priceFor(10)", serde_json::json!(100))], lines.clone(), &[], &[], "");
    let new = || run(&[("load", serde_json::json!(1))], lines.clone(), &[], &[], "kern: uncaught exception: TypeError: items.map is not a function\n    at /workspace/src/price.ts:5:3");
    let report = drive::score(&d, &[old(), old(), new(), new()], 1);
    let comment = drive::comment(&d, &report);
    assert!(!comment.contains("Nothing behaves differently"), "{comment}");
    assert!(comment.contains("priceFor(10)") && comment.contains("(no such step)"), "{comment}");
    assert!(comment.contains("ended differently") && comment.contains("items.map is not a function"), "{comment}");
}

#[test]
fn lines_a_coverage_ignore_comment_hides_never_count_as_run() {
    let src = "a();\n/* c8 ignore next 2 */\nif (x) {\n  b();\n}\n// v8 ignore start\nc();\nd();\n// v8 ignore stop\ne(); /* c8 ignore next */\nf();\n";
    assert_eq!(drive::hinted(src).into_iter().collect::<Vec<_>>(), vec![3, 4, 6, 7, 8, 9, 11]);
    let d = serde_json::json!({ "files": [{ "path": "src/a.js", "status": "modified", "old": [], "new": [3, 4], "old_text": [],
        "new_text": [[3, "if (x) {"], [4, "  b();"]], "new_hinted": [3, 4] }], "skipped": [], "totals": { "new": 2, "old": 0 } });
    let r = run(&[], covered("src/a.js", &[(3, 1), (4, 1)]), &[], &[], "");
    let report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    assert_eq!(report["coverage"]["new"]["ran"], 0, "{}", report["files"]);
}

#[test]
fn a_long_value_is_shown_where_it_differs() {
    let d = priced();
    let lines = covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]);
    let doc = |level: &str| format!("# Schema\n\n{}\n\n{level} Properties\n\n| Property | Type |", "Lorem ipsum dolor sit amet. ".repeat(20));
    let old = || run(&[("render(docker)", serde_json::json!(doc("#")))], lines.clone(), &[], &[], "");
    let new = || run(&[("render(docker)", serde_json::json!(doc("##")))], lines.clone(), &[], &[], "");
    let report = drive::score(&d, &[old(), old(), new(), new()], 1);
    let items = drive::difference_items(&report);
    let line = &items[0].0;
    assert!(line.contains("# Properties") && line.contains("## Properties") && line.contains('…'), "{line}");
}

#[test]
fn calls_of_one_function_that_now_answer_the_same_are_one_line() {
    let d = priced();
    let lines = covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]);
    let call = |n: u32, a: &str, o: &str| serde_json::json!({ "k": "src/mail.ts#createDomain", "n": n, "a": a, "o": o });
    let old = || run(&[], lines.clone(), &[call(1, "[\"a.co\"]", "e Error: restricted"), call(1, "[\"b.co\"]", "e Error: Nope"), call(1, "[\"c.co\"]", "e Error: Key is Restricted")], &[], "");
    let new = || run(&[], lines.clone(), &[call(1, "[\"a.co\"]", "e Error: Create a full access key"), call(1, "[\"b.co\"]", "e Error: Create a full access key"), call(1, "[\"c.co\"]", "e Error: Create a full access key")], &[], "");
    let report = drive::score(&d, &[old(), old(), new(), new()], 1);
    let items = drive::difference_items(&report);
    assert_eq!(items.len(), 1, "{items:?}");
    let line = &items[0].0;
    for want in ["createDomain", "a.co", "b.co", "1 more input", "restricted", "Nope", "Key is Restricted", "Create a full access key", "(3 calls)"] { assert!(line.contains(want), "{want}: {line}"); }
}

#[test]
fn sorting_uses_every_round_and_rewrites_the_comment() {
    let d = priced();
    let t = Temp::new();
    let out = t.join("out");
    write(&out, "drive.json", &d.to_string());
    std::fs::create_dir_all(out.join("task")).unwrap();
    let call = |o: &str| serde_json::json!({ "k": "src/price.ts#priceFor", "n": 1, "a": "[10]", "o": o });
    let round = |cov: Value, o: &str, n: &str| {
        let old = run(&[], cov.clone(), &[call(o)], &[], "");
        let new = run(&[], cov, &[call(n)], &[], "");
        drive::score(&d, &[old.clone(), old, new.clone(), new], 1)
    };
    write(&out, "rounds/1/report.json", &round(covered("src/price.ts", &[(3, 1)]), "r 100", "r 100").to_string());
    write(&out, "rounds/2/report.json", &round(covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]), "r 100", "r 85").to_string());
    let asked = drive::sort(&out, None).unwrap();
    assert_eq!((asked["status"].as_str(), asked["percent"].as_u64(), asked["differences"].as_u64()), (Some("ASK"), Some(100), Some(1)), "{asked}");
    assert!(read(&out, "task/differences.md").contains("D1: `src/price.ts#priceFor` with `[10]`"));
    write(&out, "task/verdicts.json", r#"{"D1":{"says":"yes","why":"Ten or more items cost 8.5 each"}}"#);
    assert_eq!(drive::sort(&out, Some(&out.join("task/verdicts.json"))).unwrap()["status"], "SORTED");
    assert!(read(&out, "comment.md").contains("Described by the pull request (1)"), "{}", read(&out, "comment.md"));
    assert!(read(&out, "report.json").contains("\"sorted\""), "the verdicts stay with the merged report");
}

#[test]
fn the_comment_names_what_it_could_not_compare() {
    let d = priced();
    let call = |port: u32| serde_json::json!({ "k": "src/price.ts#priceFor", "n": 1, "a": format!("[{{\"host\":\"127.0.0.1:{port}\"}}]"), "o": "r 100" });
    let side = |port: u32| run(&[("GET /price", serde_json::json!(port))], covered("src/price.ts", &[(3, 1), (4, 1), (5, 1), (9, 1)]), &[call(port)], &[], "");
    let report = drive::score(&d, &[side(40001), side(40002), side(40003), side(40004)], 1);
    let comment = drive::comment(&d, &report);
    assert!(!comment.contains("Nothing behaves differently"), "{comment}");
    assert!(comment.contains("two runs of the same code") && comment.contains("src/price.ts#priceFor") && comment.contains("GET /price"), "{comment}");
}

#[test]
fn feedback_shows_what_the_steps_printed_even_when_both_sides_agree() {
    let d = priced();
    let r = run(&[("PATCH /api/processes/1", serde_json::json!({ "status": 401, "body": "unauthenticated" }))], covered("src/price.ts", &[(3, 1)]), &[], &[], "");
    let feedback = drive::feedback(&d, &drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1));
    assert!(feedback.contains("PATCH /api/processes/1") && feedback.contains("401") && feedback.contains("unauthenticated"), "{feedback}");
}

#[test]
fn clean_never_claims_volumes_it_could_not_remove() {
    let t = Temp::new();
    let out = t.join("out");
    write(&out, "setup.json", r#"{"old":{"volume":"kern-deps-old1"},"new":{"volume":"kern-deps-new1"},"tools":{"volume":"kern-deps-tools1","cached":false}}"#);
    assert!(drive::clean(&out).is_err());
    assert!(out.join("setup.json").exists(), "setup.json is kept so clean can run again");
}

#[test]
fn each_round_starts_from_a_task_folder_holding_only_what_kern_expects() {
    let t = Temp::new();
    for f in ["task.md", "pr.md", "diff.patch", "lines.md", "feedback.md", "driver.mjs", "notes.md", "stubs/react-native.mjs", "old/src/a.ts", "new/src/a.ts",
        "CLAUDE.md", ".claude/settings.json", ".mcp.json", "evil.sh"] { write(&t.0, f, "x"); }
    let removed = drive::tidy_task(&t.0).unwrap();
    assert_eq!(removed, vec![".claude", ".mcp.json", "CLAUDE.md", "evil.sh"], "{removed:?}");
    for kept in ["task.md", "driver.mjs", "stubs/react-native.mjs", "old/src/a.ts", "new/src/a.ts"] { assert!(t.join(kept).exists(), "{kept}"); }
}

#[test]
fn the_default_agent_is_restricted_and_takes_only_a_model_name() {
    let a = drive::default_agent("opus").unwrap();
    let after = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].as_str());
    assert_eq!((after("--model"), after("--tools")), (Some("opus"), Some("Read,Write,Edit,Glob,Grep")), "{a:?}");
    assert!(a.contains(&"--restricted".to_string()), "{a:?}");
    for ok in ["sonnet", "claude-sonnet-5-5", "claude-opus-5-5[1m]", "claude-sonnet-4@20250514", "arn:aws:bedrock:us-east-1:1:inference-profile/us.anthropic.x"] { assert!(drive::default_agent(ok).is_ok(), "{ok}"); }
    for bad in ["", "--dangerously-skip-permissions", "sonnet --tools Bash", "a\nb", &"x".repeat(300)] { assert!(drive::default_agent(bad).is_err(), "{bad:?}"); }
}

#[cfg(unix)]
#[test]
fn the_agent_sees_no_ci_token() {
    let (t, base, head) = project();
    let op = serde_json::json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "test-suite", "file": t.join("checks.json"), "sha256": "b".repeat(64) }],
        "registry": null, "permissions": {} });
    std::fs::write(t.join("operator.json"), op.to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).args(["drive", "auto"]).arg(t.join("repo")).args([&base, &head]).arg(t.join("out")).arg(t.join("operator.json"))
        .env("KERN_DRIVE_AGENT", r#"["sh","-c","env > seen.txt; echo '{\"result\":\"no driver today\"}'"]"#).env("KERN_DRIVE_ROUNDS", "1")
        .env("GITHUB_TOKEN", "ghs_secret1").env("GH_TOKEN", "ghs_secret2").env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "secret3").env("KERN_VERIFIER_TOKEN", "secret4")
        .env("ANTHROPIC_API_KEY", "sk-ant-kept").env("DATABASE_URL", "postgres://secret5").env("STRIPE_SECRET", "secret6").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let seen = read(&t.join("out"), "task/seen.txt");
    for secret in ["ghs_secret1", "ghs_secret2", "secret3", "secret4", "secret5", "secret6"] { assert!(!seen.contains(secret), "{secret} reached the agent"); }
    assert!(seen.lines().any(|l| l.starts_with("PATH=")), "the agent can still find its programs");
    assert!(seen.contains("ANTHROPIC_API_KEY=sk-ant-kept"), "the agent keeps its own key");
    let comment = read(&t.join("out"), "comment.md");
    assert!(comment.contains("wrote none") && comment.contains("no driver today"), "{comment}");
}

#[test]
fn the_drive_action_builds_a_static_kern_from_its_own_source_and_keeps_tokens_from_the_agent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let action = std::fs::read_to_string(root.join("drive/action.yml")).unwrap().replace("\r\n", "\n");
    let step = |name: &str| action.split(&format!("- name: {name}\n")).nth(1).unwrap_or_else(|| panic!("no step {name}")).split("\n    - name:").next().unwrap().to_string();
    let install = step("Install Kern");
    for want in ["working-directory: ${{ github.action_path }}/..", "KERN_SOURCE: ${{ github.action_path }}/..", "KERN_STATIC: \"true\""] { assert!(install.contains(want), "{want}: {install}"); }
    for key in ["KERN_VERSION", "KERN_SHA256_LINUX_X64", "KERN_SHA256_LINUX_ARM64"] { assert!(install.contains(&format!("{key}: \"\"")), "{key}: {install}"); }
    assert!(std::fs::read_to_string(root.join("scripts/pin-release.sh")).unwrap().contains("drive/action.yml"));
    let script = std::fs::read_to_string(root.join("scripts/install-kern.sh")).unwrap();
    assert!(script.contains("KERN_SOURCE") && script.contains("unknown-linux-musl"), "the sandbox runs Kern inside Debian: only the static build works there");
    let drive_step = step("Drive");
    assert!(!drive_step.contains("github.token") && !drive_step.contains("GITHUB_TOKEN") && !drive_step.contains("KERN_DRIVE_AGENT"), "{drive_step}");
    assert!(drive_step.contains("KERN_DRIVE_MODEL: ${{ inputs.model }}"), "{drive_step}");
    assert!(step("Comment").contains("GITHUB_TOKEN: ${{ github.token }}"));
    let mut block = None;
    for line in action.lines() {
        let indent = line.len() - line.trim_start().len();
        if block.is_some_and(|i| line.trim().is_empty() || indent > i) { assert!(!line.contains("${{"), "{line}"); continue; }
        block = line.trim_start().starts_with("run:").then_some(indent);
        if block.is_some() { assert!(!line.contains("${{"), "{line}"); }
    }
}

#[test]
fn typescript_declarations_that_emit_no_code_are_not_measured() {
    let source = "export class Cart {\n  items: string[];\n  declare owner: string;\n  count = 0;\n  constructor(items: string[]) {\n    this.items = items;\n  }\n}\n";
    let lines: Vec<u32> = drive::measured("src/cart.ts", source).into_keys().collect();
    assert_eq!(lines, vec![1, 4, 5, 6], "a field with no value is a type only; one with a value runs");
    let dto = "export class User {\n  @MaxLength(255)\n  email: string;\n  [key()]: string;\n}\n";
    let lines: Vec<u32> = drive::measured("src/user.dto.ts", dto).into_keys().collect();
    assert_eq!(lines, vec![1, 2, 3, 4], "{lines:?}");
}

#[test]
fn sandbox_trees_keep_names_only_windows_reserves() {
    for ok in ["src/aux/index.js", "lib/con.ts", "a:b.js", "docs/nul", "x/Trailing. /y.js"] { assert!(drive::linux_path(ok), "{ok}"); }
    for bad in ["", "../x.js", "a/../b.js", "a/./b.js", "/etc/passwd", ".git/config", "a//b.js", "a\0b.js"] { assert!(!drive::linux_path(bad), "{bad:?}"); }
}

#[test]
fn files_kern_does_not_run_are_named() {
    let (t, base, _) = project();
    let r = t.join("repo");
    write(&r, "src/App.vue", "<template><div/></template>\n<script>export default { data: () => ({ n: 1 }) }</script>\n");
    write(&r, "tools/build.py", "print('x')\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "vue and python"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("vue");
    drive::prepare(&r, &base, &head, &out, None).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    for p in ["src/App.vue", "tools/build.py"] {
        assert!(d["skipped"].as_array().unwrap().iter().any(|s| s["path"] == p && s["why"].as_str().unwrap().contains("does not run")), "{p}: {}", d["skipped"]);
    }
}

#[test]
fn the_comment_says_when_dependencies_could_not_be_installed() {
    let d = priced();
    let r = run(&[], covered("src/price.ts", &[(3, 1), (4, 1)]), &[], &[], "");
    let mut report = drive::score(&d, &[r.clone(), r.clone(), r.clone(), r], 1);
    report["installs"] = serde_json::json!({ "new": [{ "dir": ".", "manager": "npm", "result": "failed: npm error 404 Not Found - @acme/private" }], "old": [] });
    let comment = drive::comment(&d, &report);
    assert!(comment.contains("could not be installed") && comment.contains("@acme/private"), "{comment}");
}

#[test]
fn a_yarnrc_keeps_only_the_settings_yarn_itself_knows_once_its_plugins_are_gone() {
    let rc = "plugins:\n  - path: .yarn/plugins/after.cjs\nafterInstall: yarn build\nenableTelemetry: false\nnpmScopes:\n  acme:\n    npmRegistryServer: \"https://npm.acme.test\"\npackageExtensions:\n  \"left-pad@*\":\n    dependencies:\n      lodash: \"*\"\n\"supportedArchitectures\":\n  cpu: [x64]\n";
    let clean = kern::sandbox::yarnrc_without_code(rc);
    assert!(!clean.contains("afterInstall"), "OneKey's afterInstall comes from a plugin Kern removes, and yarn refuses a setting it does not know: {clean}");
    for kept in ["enableTelemetry: false", "npmScopes:", "npm.acme.test", "packageExtensions:", "lodash", "supportedArchitectures", "cpu: [x64]"] {
        assert!(clean.contains(kept), "{kept}: {clean}");
    }
}

#[test]
fn a_side_whose_image_kern_did_not_record_fails_instead_of_running_on_the_default() {
    let t = Temp::new();
    let pinned = format!("sha256:{}", "a".repeat(64));
    std::fs::write(t.join("setup.json"), serde_json::json!({ "old": { "volume": "kern-deps-old", "image": pinned }, "new": { "volume": "kern-deps-new", "image": "node:22" } }).to_string()).unwrap();
    assert_eq!(drive::side_image(&t.0, "kern-deps-old").unwrap(), pinned);
    let unpinned = drive::side_image(&t.0, "kern-deps-new").unwrap_err().0;
    assert!(unpinned.contains("new"), "the new side's image is not pinned by digest, so its tests must not run on another image Kern guesses: {unpinned}");
    assert!(drive::side_image(&t.0, "kern-deps-other").is_err(), "a volume setup.json does not know");
    std::fs::write(t.join("setup.json"), "{ cut").unwrap();
    assert!(drive::side_image(&t.0, "kern-deps-old").is_err(), "an unreadable setup.json is Kern's environment failing, never a reason to run both sides on one default image");
}
