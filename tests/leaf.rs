use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Tree(PathBuf);
impl Tree {
    fn new(name: &str) -> Tree {
        let dir = std::env::temp_dir().join(format!("kern-leaf-{name}-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        Tree(dir)
    }
    fn write(&self, path: &str, text: &str) {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    fn runners(&self) {
        let real = "process.stdout.write('real ' + process.argv.slice(2).join(' ')); process.exit(3);\n";
        self.write("node_modules/jest/package.json", r#"{"name":"jest","version":"29.7.0","bin":"./bin/jest.js"}"#);
        self.write("node_modules/jest/bin/jest.js", real);
        self.write("node_modules/vitest/package.json", r#"{"name":"vitest","version":"3.2.4","type":"module","bin":{"vitest":"./vitest.mjs"}}"#);
        self.write("node_modules/vitest/vitest.mjs", "process.stdout.write('real vitest'); process.exit(4);\n");
        self.write("node_modules/mocha/package.json", r#"{"name":"mocha","version":"10.8.2","bin":{"mocha":"./bin/mocha.js","_mocha":"./bin/_mocha"}}"#);
        self.write("node_modules/mocha/bin/_mocha", real);
        self.write("node_modules/eslint/package.json", r#"{"name":"eslint","version":"9.0.0","bin":{"eslint":"./bin/eslint.js"}}"#);
        self.write("node_modules/eslint/bin/eslint.js", "process.stdout.write('real eslint'); process.exit(5);\n");
    }
}
impl Drop for Tree {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn hook() -> String { format!("--require={}", Path::new(env!("CARGO_MANIFEST_DIR")).join("src/leaf.cjs").display()) }
fn node(tree: &Tree, args: &[&str], env: &[(&str, &str)]) -> Option<std::process::Output> {
    let mut c = Command::new("node");
    c.args(args).current_dir(&tree.0).env_clear();
    for k in ["PATH", "SystemRoot", "TEMP", "TMP"] { if let Some(v) = std::env::var_os(k) { c.env(k, v); } }
    c.envs(env.iter().copied());
    match c.output() {
        Ok(out) => Some(out),
        Err(_) => { eprintln!("node is not installed here; the recorder is checked where it is"); None }
    }
}
fn records(file: &Path) -> Vec<Value> {
    std::fs::read_to_string(file).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn wp_calypso_112403_a_runner_the_test_script_starts_is_recorded_and_not_run() {
    let t = Tree::new("record");
    t.runners();
    let leaves = t.0.join("leaves.jsonl");
    let leaves_text = leaves.display().to_string();
    let Some(out) = node(&t, &[&hook(), "node_modules/jest/bin/jest.js", "-c=test/server/jest.config.js", "--ci"],
        &[("KERN_LEAVES", &leaves_text), ("TZ", "UTC"), ("npm_lifecycle_event", "test-server"), ("npm_config_cache", "/x"), ("KERN_REPORT", "/y"), ("FOO", "bar")]) else { return };
    assert!(out.status.success(), "a recorded runner ends with exit 0 so the rest of the chain runs: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("real"), "the real runner does not run while Kern records");
    let r = records(&leaves);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["runner"], "jest");
    assert_eq!(r[0]["argv"], json!(["-c=test/server/jest.config.js", "--ci"]));
    assert_eq!(r[0]["event"], "test-server");
    assert_eq!(Path::new(r[0]["cwd"].as_str().unwrap()).canonicalize().unwrap(), t.0.canonicalize().unwrap());
    assert_eq!(r[0]["env"]["TZ"], "UTC");
    assert_eq!(r[0]["env"]["FOO"], "bar");
    for gone in ["npm_config_cache", "npm_lifecycle_event", "KERN_REPORT", "KERN_LEAVES", "PATH", "NODE_OPTIONS"] {
        assert!(r[0]["env"].get(gone).is_none(), "{gone} is the package manager's or Kern's, not the chain's: {}", r[0]["env"]);
    }
    assert_eq!(r[0]["node"], json!([]), "Kern's own preload is not one of the runner's node flags");
}

#[test]
fn every_runner_bin_is_recognised_by_its_package_and_a_tool_that_is_not_one_runs_as_it_is() {
    let t = Tree::new("bins");
    t.runners();
    let leaves = t.0.join("leaves.jsonl");
    let leaves_text = leaves.display().to_string();
    let env = [("KERN_LEAVES", leaves_text.as_str())];
    let Some(vitest) = node(&t, &[&hook(), "node_modules/vitest/vitest.mjs", "run", "--project", "unit"], &env) else { return };
    assert!(vitest.status.success(), "an ES module runner is stopped before it loads: {}", String::from_utf8_lossy(&vitest.stderr));
    let mocha = node(&t, &[&hook(), "--expose-gc", "node_modules/mocha/bin/_mocha", "test/a.js"], &env).unwrap();
    assert!(mocha.status.success());
    let eslint = node(&t, &[&hook(), "node_modules/eslint/bin/eslint.js", "."], &env).unwrap();
    assert_eq!(eslint.status.code(), Some(5), "a program that is not a test runner runs for real with its own exit code");
    assert_eq!(String::from_utf8_lossy(&eslint.stdout), "real eslint");
    let r = records(&leaves);
    assert_eq!(r.iter().map(|x| x["runner"].as_str().unwrap()).collect::<Vec<_>>(), ["vitest", "mocha"], "{r:?}");
    assert_eq!(r[0]["argv"], json!(["run", "--project", "unit"]));
    assert_eq!(r[1]["node"], json!(["--expose-gc"]), "node flags given on the command line are kept for the leaf's run");
}

#[test]
fn without_kerns_record_file_the_preload_changes_nothing_and_the_exit_code_is_the_runners() {
    let t = Tree::new("pass");
    t.runners();
    let Some(out) = node(&t, &[&hook(), "node_modules/jest/bin/jest.js", "--ci"], &[]) else { return };
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "real --ci");
}

#[test]
fn a_runner_that_loads_the_preload_twice_is_recorded_once() {
    let t = Tree::new("twice");
    t.runners();
    let leaves = t.0.join("leaves.jsonl");
    let leaves_text = leaves.display().to_string();
    let Some(out) = node(&t, &[&hook(), &hook(), "node_modules/jest/bin/jest.js"], &[("KERN_LEAVES", &leaves_text), ("NODE_OPTIONS", &hook())]) else { return };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r = records(&leaves);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0]["node"], json!([]));
    assert!(r[0]["env"].get("NODE_OPTIONS").is_none(), "Kern's own preload is taken out of NODE_OPTIONS: {}", r[0]["env"]);
}

#[test]
fn a_runner_whose_environment_or_arguments_pass_the_records_limits_is_recorded_as_cut() {
    let t = Tree::new("cut");
    t.runners();
    let leaves = t.0.join("leaves.jsonl");
    let leaves_text = leaves.display().to_string();
    let many: Vec<(String, String)> = (0..130).map(|i| (format!("VAR_{i}"), "x".to_string())).collect();
    let mut env: Vec<(&str, &str)> = many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    env.push(("KERN_LEAVES", &leaves_text));
    let Some(out) = node(&t, &[&hook(), "node_modules/jest/bin/jest.js"], &env) else { return };
    assert!(out.status.success());
    let long = "y".repeat(9000);
    node(&t, &[&hook(), "node_modules/jest/bin/jest.js", &long], &[("KERN_LEAVES", &leaves_text)]).unwrap();
    let r = records(&leaves);
    assert_eq!(r.len(), 2);
    for x in &r { assert_eq!(x["cut"], true, "a run Kern could not repeat exactly is never repeated in part: {x}"); }
}

#[test]
fn a_record_that_cannot_be_written_stops_the_runner_with_an_error() {
    let t = Tree::new("unwritable");
    t.runners();
    let missing = t.0.join("no-such-folder").join("leaves.jsonl").display().to_string();
    let Some(out) = node(&t, &[&hook(), "node_modules/jest/bin/jest.js"], &[("KERN_LEAVES", &missing)]) else { return };
    assert_eq!(out.status.code(), Some(70), "a lost record must not look like a passing run");
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not record a jest run"));
}

#[test]
fn mcpjam_inspector_4342_the_driver_runs_the_chain_and_writes_every_leaf_and_how_it_ended() {
    let t = Tree::new("drive");
    t.runners();
    t.write("run-tests.js", "const { spawnSync } = require('child_process');\nfor (const c of ['a', 'b']) spawnSync(process.execPath, ['node_modules/jest/bin/jest.js', '-c=' + c], { stdio: 'inherit' });\nprocess.exit(6);\n");
    let report = t.0.join("report.jsonl");
    let report_text = report.display().to_string();
    let leaf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/leaf.cjs").display().to_string();
    let Some(out) = node(&t, &[&leaf, "node", "run-tests.js"], &[("KERN_REPORT", &report_text)]) else { return };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r = records(&report);
    assert_eq!(r.len(), 3, "{r:?}");
    assert_eq!(r[0]["argv"], json!(["-c=a"]));
    assert_eq!(r[1]["argv"], json!(["-c=b"]));
    assert_eq!(r[2], json!({ "end": true, "exit": 6, "signal": null, "error": null }), "the last line says the chain ended and how");
    let missing = Tree::new("drive-missing");
    let report = missing.0.join("report.jsonl");
    let report_text = report.display().to_string();
    node(&missing, &[&leaf, "kern-no-such-program"], &[("KERN_REPORT", &report_text)]).unwrap();
    let r = records(&report);
    assert_eq!(r.len(), 1);
    assert!(r[0]["end"] == true && r[0]["exit"].is_null() && r[0]["error"].is_string(), "a launcher that cannot start is said, not taken for a chain with no runners: {r:?}");
}
