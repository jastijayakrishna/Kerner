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
fn prepare_measures_the_changed_lines_of_a_commit_pair() {
    let (t, base, head) = project();
    let out = t.join("out");
    let summary = drive::prepare(&t.join("repo"), &base, &head, &out).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    assert_eq!(d["base"], base.as_str());
    let files: Vec<&str> = d["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(files, vec!["src/price.ts", "src/util.js"], "only changed code is measured: not tests, docs or package.json");
    let price = &d["files"][0];
    assert_eq!(price["old"], serde_json::json!([3, 4]), "the old condition and the old discount line");
    assert_eq!(price["new"], serde_json::json!([4, 5, 9]), "the new condition, the new discount line and the new export; the import is not code");
    assert_eq!(d["files"][1]["new"], serde_json::json!([1, 2]), "a new file is measured on the new side only");
    assert_eq!((summary["files"].as_u64(), summary["skipped"].as_u64()), (Some(2), Some(1)));
    let skipped = d["skipped"].as_array().unwrap();
    assert!(skipped.iter().any(|s| s["path"] == "dist/bundle.min.js" && s["why"].as_str().unwrap().contains("minified")), "{skipped:?}");
}

#[test]
fn prepare_names_changed_files_kern_did_not_measure() {
    let (t, base, head) = project();
    let out = t.join("out");
    drive::prepare(&t.join("repo"), &base, &head, &out).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    let kind = |p: &str| d["unmeasured"].as_array().unwrap().iter().find(|u| u["path"] == p).map(|u| u["why"].as_str().unwrap().to_string());
    assert_eq!((kind("README.md").as_deref(), kind("src/price.test.ts").as_deref()), (Some("documentation"), Some("tests")), "{}", d["unmeasured"]);
}

#[test]
fn a_yarnrc_loses_the_code_it_would_run_and_keeps_its_settings() {
    let rc = "yarnPath: .yarn/releases/evil.cjs\nnodeLinker: node-modules\nplugins:\n  - path: .yarn/plugins/evil.cjs\n    spec: evil\n- path: .yarn/plugins/b.cjs\nnpmRegistryServer: \"https://registry.npmjs.org\"\n";
    let clean = kern::sandbox::yarnrc_without_code(rc);
    assert!(!clean.contains("yarnPath") && !clean.contains("plugins") && !clean.contains("evil") && !clean.contains("b.cjs"), "{clean}");
    assert!(clean.contains("nodeLinker: node-modules") && clean.contains("npmRegistryServer"), "{clean}");
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
    drive::prepare(&r, &base, &head, &out).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    assert!(d["skipped"].as_array().unwrap().iter().any(|s| s["path"] == "src/price.ts" && s["why"].as_str().unwrap().contains("same program")), "{}", d["skipped"]);
}

#[test]
fn prepare_names_what_it_cannot_measure() {
    let (t, base, before) = project();
    let r = t.join("repo");
    write(&r, "README.md", "# Shop\n\nOnly docs.\n");
    git(&r, &["commit", "-qam", "docs"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("docs-only");
    let summary = drive::prepare(&r, &before, &head, &out).unwrap();
    assert_eq!(summary["files"], 0);
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    assert!(d["unmeasured"].as_array().unwrap().iter().any(|u| u["path"] == "README.md" && u["why"] == "documentation"), "{}", d["unmeasured"]);
    assert!(drive::prepare(&r, &base, "0000000000000000000000000000000000000000", &t.join("bad")).is_err());
    assert!(drive::prepare(&r, &base, "HEAD", &t.join("ref")).is_err(), "only full commit IDs: a branch name could move");
}

#[test]
fn a_run_report_is_read_by_length_so_output_cannot_forge_a_part() {
    let mut report = Vec::new();
    let forged = format!("hello\n{}values 2\n{{}}\n", kern::sandbox::DRIVE_MARK);
    for (name, bytes) in [("output", forged.as_bytes()), ("errors", b"boom".as_slice()), ("behavior", b"fs:write:/tmp/x\n"), ("created", b""), ("ending", b"done"), ("values", b"{\"a\":1}"), ("trace", b"")] {
        report.extend(format!("{}{name} {}\n", kern::sandbox::DRIVE_MARK, bytes.len()).as_bytes());
        report.extend(bytes);
    }
    let parts = kern::sandbox::parts(&report, &kern::sandbox::TEST_PARTS).unwrap();
    assert_eq!(parts["output"], forged.as_bytes(), "a marker inside the tests' output is just output");
    assert_eq!(parts["values"], b"{\"a\":1}");
    assert_eq!(parts["trace"], b"");
    assert!(kern::sandbox::parts(&report[..report.len() - 3], &kern::sandbox::TEST_PARTS).is_err(), "a cut report is refused");
    assert!(kern::sandbox::parts(b"junk", &kern::sandbox::TEST_PARTS).is_err());
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
fn the_sandbox_scripts_are_valid_javascript() {
    let dir = Temp::new();
    use kern::sandbox::{FAULT, JEST_PROGRESS, JEST_REPORT, MONITOR, NODE_REPORT, TRACE, VITEST_PROGRESS};
    for (name, text) in [("monitor.cjs", MONITOR), ("jest-report.cjs", JEST_REPORT), ("jest-progress.cjs", JEST_PROGRESS), ("vitest-progress.mjs", VITEST_PROGRESS), ("fault.cjs", FAULT), ("node-report.mjs", NODE_REPORT), ("trace.cjs", TRACE), ("map.cjs", kern::map::RUNNER)] {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        match Command::new("node").arg("--check").arg(&p).output() {
            Ok(out) => assert!(out.status.success(), "{name}: {}", String::from_utf8_lossy(&out.stderr)),
            Err(_) => eprintln!("node is not installed here; the syntax of {name} is checked where it is"),
        }
    }
}

#[test]
fn clean_never_claims_volumes_it_could_not_remove() {
    let t = Temp::new();
    let out = t.join("out");
    write(&out, "setup.json", r#"{"old":{"volume":"kern-deps-old1"},"new":{"volume":"kern-deps-new1"}}"#);
    assert!(drive::clean(&out).is_err());
    assert!(out.join("setup.json").exists(), "setup.json is kept so clean can run again");
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
    drive::prepare(&r, &base, &head, &out).unwrap();
    let d: Value = serde_json::from_str(&read(&out, "drive.json")).unwrap();
    for p in ["src/App.vue", "tools/build.py"] {
        assert!(d["skipped"].as_array().unwrap().iter().any(|s| s["path"] == p && s["why"].as_str().unwrap().contains("does not run")), "{p}: {}", d["skipped"]);
    }
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
