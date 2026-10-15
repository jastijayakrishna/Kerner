use kern::script::{Found, found};
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-attack6-{}", uuid4().unwrap()));
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
fn command(scripts: Value) -> Vec<String> {
    match found(&json!({ "scripts": scripts })) {
        Found::Run(i) => i.command,
        other => panic!("expected a runner, got {other:?}"),
    }
}
fn bin(name: &str) -> String { format!("/workspace/node_modules/.bin/{name}") }

const OLD_MATH: &str = "exports.sum = (items) => items.reduce((a, b) => a + b, 0);\n";
const BROKEN_MATH: &str = "exports.sum = (items) => items.reduce((a, b) => a - b, 0);\n";
const MATH_TEST: &str = "const { sum } = require('../src/math.js');\ntest('sums', () => expect(sum([2, 3])).toBe(5));\n";

fn decision_when_tests_pass(rules: &Value) -> Value {
    let d = json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] });
    let tests = json!({ "status": "RAN", "runner": "jest", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] });
    kern::receipt::build(&d, rules, &tests, &json!({ "status": "OFF", "why": "no AI key" }))["decision"].clone()
}

#[test]
fn a_config_file_the_test_script_names_is_a_settings_item() {
    let cases = [
        ("jest --config=.jestconfig.json", ".jestconfig.json", r#"{"testEnvironment":"node"}"#,
            r#"{"testEnvironment":"node","moduleNameMapper":{"^\\.\\./src/math\\.js$":"<rootDir>/test/fixtures/math-old.js"}}"#),
        ("vitest run -c vitest.unit.config.mjs", "vitest.unit.config.mjs", "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { include: ['test/**/*.test.js'] } });\n",
            "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ resolve: { alias: { '../src/math.js': '/workspace/test/fixtures/math-old.js' } }, test: { include: ['test/**/*.test.js'] } });\n"),
    ];
    let mut missed = Vec::new();
    for (script, config, before, after) in cases {
        let t = Temp::new();
        let package = format!(r#"{{"name":"shop","scripts":{{"test":"{script}"}},"devDependencies":{{"jest":"30.0.0","vitest":"3.2.0"}}}}"#);
        let (r, b) = repo(&t, &[("package.json", &package), (config, before), ("src/math.js", OLD_MATH), ("test/math.test.js", MATH_TEST)]);
        let h = commit(&r, &[(config, after), ("test/fixtures/math-old.js", OLD_MATH), ("src/math.js", BROKEN_MATH)]);
        let rules = kern::rules::rules(&r, &b, &h).unwrap();
        let named = rules["settings"].as_array().into_iter().flatten().any(|s| s["path"] == config);
        if !named { missed.push(format!("{script}: settings {} / tricks {} / decision once the tests pass {}", rules["settings"], rules["tricks"], decision_when_tests_pass(&rules))); }
    }
    assert!(missed.is_empty(), "the test script points the runner at this config, the change maps ../src/math.js to an old copy so the tests pass on broken code, and no settings item is raised:\n{}", missed.join("\n"));
}

#[test]
fn a_test_script_that_fans_out_to_every_script_is_read_in_bounded_time() {
    let mut scripts = serde_json::Map::new();
    scripts.insert("test".into(), json!("run-p *"));
    for i in 0..100 { scripts.insert(format!("s{i:03}"), json!("run-p *")); }
    let package = json!({ "scripts": scripts });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(found(&package)); });
    let answer = rx.recv_timeout(std::time::Duration::from_secs(8));
    assert!(answer.is_ok(), "a 2 KB package.json whose scripts each run every script (run-p *) keeps script::found busy for well over 8 s (the DEPTH limit of 4 does not help: run_all re-expands * to all 101 scripts at every level, so 101^4 calls); rules::rules reads the pull request's own package.json this way before any time limit applies");
}

#[test]
fn a_test_script_that_repeats_a_package_manager_stays_within_the_depth_limit() {
    let package = json!({ "scripts": { "test": format!("{}jest", "yarn ".repeat(300)) } });
    let r = std::thread::Builder::new().stack_size(64 << 20).spawn(move || found(&package)).unwrap().join();
    assert!(r.is_ok(), "manager and command call each other once per word with no depth check, and the u8 depth counter passes 255: a debug build panics, a release build wraps it back to 0, so the depth limit of 4 never holds");
}

#[test]
fn an_option_kern_leaves_out_never_leaves_its_value_behind_as_a_test_filter() {
    let cases = [
        ("vitest run --bail 1", vec![bin("vitest"), "run".to_string()]),
        ("vitest run --coverage --coverage.reporter text", vec![bin("vitest"), "run".to_string()]),
        ("jest --bail 1 --ci", vec![bin("jest"), "--ci".to_string()]),
        ("jest -t 'adds numbers'", vec![bin("jest"), "-t".to_string(), "adds numbers".to_string()]),
    ];
    let wrong: Vec<String> = cases.iter().map(|(script, want)| (script, want, command(json!({ "test": script })))).filter(|(_, want, got)| got != *want)
        .map(|(script, want, got)| format!("{script}: Kern runs {got:?}, want {want:?}")).collect();
    assert!(wrong.is_empty(), "an option Kern leaves out takes its value with it, or the value stays behind as a path filter that narrows the tests; options the project's own script uses to choose its tests, such as jest -t, stay as they are:\n{}", wrong.join("\n"));
}

#[test]
fn a_shell_comment_in_the_test_script_is_not_a_test_filter() {
    let got = command(json!({ "test": "jest --ci # unit tests only" }));
    assert_eq!(got, vec![bin("jest"), "--ci".to_string()], "sh ignores everything after #; Kern hands jest the patterns '#', 'unit', 'tests', 'only', so jest runs only test files whose path matches them");
}
