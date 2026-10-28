use kern::leaf::{self, Recorded};
use kern::policy::Runner;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn bin(name: &str) -> String { format!("/workspace/node_modules/.bin/{name}") }
fn words(list: &[&str]) -> Vec<String> { list.iter().map(|s| s.to_string()).collect() }
fn calypso() -> Value {
    json!({ "scripts": { "test": "run-s -s test-client test-packages test-server test-build-tools", "test-client": "TZ=UTC jest -c=test/client/jest.config.js",
        "test-packages": "jest -c=test/packages/jest.config.js", "test-server": "jest -c=test/server/jest.config.js", "test-build-tools": "jest -c=test/build-tools/jest.config.js",
        "test-integration": "jest -c=test/integration/jest.config.js" } })
}
fn run(runner: &str, cwd: &str, argv: &[&str], event: &str) -> Value {
    json!({ "runner": runner, "cwd": cwd, "argv": argv, "node": [], "env": { "TZ": "UTC", "INIT_CWD": "/workspace", "TURBO_FORCE": "true" }, "event": event })
}
fn recorded(runs: &[Value], exit: i64) -> Recorded {
    let mut text: String = runs.iter().map(|r| format!("{r}\n")).collect();
    text += &format!("{}\n", json!({ "end": true, "exit": exit, "signal": null, "error": null }));
    leaf::read(text.as_bytes()).unwrap()
}
fn calypso_runs() -> Vec<Value> {
    ["client", "packages", "server", "build-tools"].iter().map(|c| run("jest", "/workspace", &[&format!("-c=test/{c}/jest.config.js")], &format!("test-{c}"))).collect()
}

#[test]
fn wp_calypso_112403_a_run_s_script_over_four_jest_configs_is_run_through_its_recorded_leaves() {
    assert_eq!(kern::script::chained(&calypso()), Some("test".to_string()), "run-s starts four jest runs, and Kern followed only the first, whose config cannot see the change");
    let kern::script::Found::Run(first) = kern::script::found(&calypso()) else { panic!("calypso's script is still read the same way") };
    assert_eq!(first.command, words(&[&bin("jest"), "-c=test/client/jest.config.js"]), "the rules still see the first runner, so no rule finding moves");
    assert!(matches!(kern::suite::plan(Some(calypso().to_string().as_bytes())), kern::suite::Plan::Run(..)), "suite::plan is unchanged for the rules that read it");
    let two = json!({ "scripts": { "test": "pnpm test:unit && pnpm test:integration", "test:unit": "vitest run tests/unit", "test:integration": "vitest run tests/integration" } });
    assert_eq!(kern::script::chained(&two), Some("test".to_string()));
    let ci = json!({ "scripts": { "test:ci": "jest -c a.js && jest -c b.js" } });
    assert_eq!(kern::script::chained(&ci), Some("test:ci".to_string()), "the chain runs the script Kern follows, by its own name");
}

#[test]
fn a_script_that_starts_one_runner_keeps_todays_path() {
    for script in ["jest", "vitest run", "npm run build && jest", "cross-env NODE_OPTIONS=--max-old-space-size=3072 vitest run", "jest --ci; echo done", "turbo run build && vitest run",
        "npm-run-all --npm-path npm -s lint jest", "node --test a.test.js && jest && jest -c b.js", "tsx --test a.ts && vitest run && vitest run --project b", "turbo run test", "npm run test --workspaces", "vp test run"] {
        let package = json!({ "scripts": { "test": script, "lint": "eslint .", "build": "tsc", "jest": "jest" } });
        assert_eq!(kern::script::chained(&package), None, "{script}: one runner, a fan-out, a node --test run or nothing Kern can follow keeps today's path");
    }
}

#[test]
fn a_recorded_run_becomes_the_runner_command_kern_runs_today() {
    let r = recorded(&[run("jest", "/workspace", &["-c=test/packages/jest.config.js", "--watch", "--coverage"], "test-packages")], 0);
    let (tests, env) = leaf::side(&r.leaves[0]).unwrap();
    assert_eq!(tests.command, words(&[&bin("jest"), "-c=test/packages/jest.config.js"]), "Kern drops the flags it drops for a script it follows");
    assert!(env.is_empty(), "TZ=UTC is Kern's own, and INIT_CWD and TURBO_FORCE belong to the package manager and to Kern: {env:?}");
    let mut v = run("vitest", "/workspace/packages/core", &["run", "--project", "unit"], "test");
    v["node"] = json!(["--expose-gc"]);
    v["env"] = json!({ "NODE_OPTIONS": "--max-old-space-size=3072", "SUITE": "unit", "TZ": "America/New_York", "npm_config_x": "1", "PNPM_SCRIPT_SRC_DIR": "/workspace", "PATH": "/x", "HOME": "/y" });
    let r = recorded(&[v], 0);
    assert_eq!(r.leaves[0].dir.as_deref(), Some("packages/core"));
    let (tests, env) = leaf::side(&r.leaves[0]).unwrap();
    assert_eq!(tests.command, words(&[&bin("vitest"), "run", "--project", "unit"]));
    assert_eq!(env.get("NODE_OPTIONS").map(String::as_str), Some("--max-old-space-size=3072 --expose-gc"), "the chain's NODE_OPTIONS and the runner's node flags both reach the leaf's run");
    assert_eq!(env.get("SUITE").map(String::as_str), Some("unit"), "a variable cross-env set for this run is kept");
    assert_eq!(env.get("TZ").map(String::as_str), Some("America/New_York"));
    assert_eq!(env.len(), 3, "{env:?}");
    let outside = recorded(&[run("jest", "/tmp/elsewhere", &[], "test")], 0);
    assert!(leaf::side(&outside.leaves[0]).is_err(), "a run outside the project's folder is not repeated");
    let mut crowded = run("jest", "/workspace", &[], "test");
    crowded["env"] = Value::Object((0..40).map(|i| (format!("VAR_{i}"), json!("x"))).collect());
    assert!(leaf::side(&recorded(&[crowded], 0).leaves[0]).unwrap_err().contains("32 variables"));
    assert!(leaf::informational(&recorded(&[run("jest", "/workspace", &["--version"], "")], 0).leaves[0]));
    assert!(leaf::informational(&recorded(&[run("vitest", "/workspace", &["list"], "")], 0).leaves[0]));
    assert!(!leaf::informational(&recorded(&[run("vitest", "/workspace", &["run"], "")], 0).leaves[0]));
}

#[test]
fn a_record_that_is_cut_malformed_unfinished_or_too_long_is_never_read_in_part() {
    let end = json!({ "end": true, "exit": 0, "signal": null, "error": null }).to_string();
    let one = run("jest", "/workspace", &[], "test").to_string();
    assert!(leaf::read(format!("{one}\n").as_bytes()).unwrap_err().contains("stopped before"), "a record with no end line is from a script that was stopped");
    assert!(leaf::read(format!("{one}\n{{\"runner\": 1}}\n{end}\n").as_bytes()).unwrap_err().contains("could not read"));
    assert!(leaf::read(format!("{one}\nnot json\n{end}\n").as_bytes()).is_err());
    let many: String = (0..=leaf::LEAVES_COUNT_MAX).map(|_| format!("{one}\n")).collect();
    assert!(leaf::read(format!("{many}{end}\n").as_bytes()).unwrap_err().contains("more than 64"));
    assert!(leaf::read(&vec![b' '; leaf::LEAF_RECORD_BYTES_MAX + 1]).unwrap_err().contains("larger than"));
    assert!(leaf::read(b"").is_err());
    let cut = leaf::read(format!("{{\"runner\":\"jest\",\"cwd\":\"/workspace\",\"cut\":true}}\n{end}\n").as_bytes()).unwrap();
    assert!(cut.leaves[0].cut && leaf::side(&cut.leaves[0]).unwrap_err().contains("could not record this run whole"));
    let stopped = leaf::read(format!("{}\n", json!({ "end": true, "exit": null, "signal": "SIGTERM", "error": null })).as_bytes()).unwrap();
    assert_eq!((stopped.exit, stopped.why.as_deref()), (None, Some("it was stopped by SIGTERM")));
}

#[test]
fn wp_calypso_114128_every_jest_config_of_the_chain_is_planned_on_both_sides_and_named_by_its_script() {
    let changed = ["packages/odie-client/src/types.ts"];
    let (old, new) = (recorded(&calypso_runs(), 0), recorded(&calypso_runs(), 0));
    let e = leaf::expand("", Ok(&old), Ok(&new), &changed);
    assert!(e.not_run.is_empty(), "{:?}", e.not_run);
    let names: Vec<&str> = e.leaves.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["test-build-tools", "test-client", "test-packages", "test-server"], "sorted by what runs, so a parallel chain plans the same order every time");
    assert!(e.leaves.iter().all(|p| p.old.is_some() && p.dir.is_empty()));
    let packages = e.leaves.iter().find(|p| p.name == "test-packages").unwrap();
    assert_eq!(packages.new.0.command, words(&[&bin("jest"), "-c=test/packages/jest.config.js"]));
}

#[test]
fn a_run_on_one_side_only_or_one_kern_cannot_repeat_is_named_and_never_dropped() {
    let changed = ["src/a.ts"];
    let old = recorded(&[run("jest", "/workspace", &["-c=a.js"], "test-a"), run("jest", "/workspace", &["-c=b.js"], "test-b")], 0);
    let mut cut = run("jest", "/workspace", &[], "test-d");
    cut["cut"] = json!(true);
    let new = recorded(&[run("jest", "/workspace", &["-c=a.js"], "test-a"), run("jest", "/workspace", &["-c=c.js"], "test-c"), cut], 0);
    let e = leaf::expand("", Ok(&old), Ok(&new), &changed);
    let planned: Vec<(&str, bool)> = e.leaves.iter().map(|p| (p.name.as_str(), p.old.is_some())).collect();
    assert_eq!(planned, [("test-a", true), ("test-c", false)], "a run only the new code starts has no old side, so its tests are new, and cannot break");
    let why: Vec<&str> = e.not_run.iter().filter_map(|n| n["why"].as_str()).collect();
    assert!(why.iter().any(|w| w.contains("starts `jest -c=b.js` on the old code but not on the new code")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("could not record this run whole")), "{why:?}");
}

#[test]
fn qwen_code_6756_a_script_whose_runs_are_all_outside_the_change_says_so_and_mcpjam_4342_one_with_no_runner_says_kern_saw_none() {
    let changed = ["scripts/generate-changelog.js"];
    let packages: Vec<Value> = ["cli", "core"].iter().map(|p| run("vitest", &format!("/workspace/packages/{p}"), &["run"], "test")).collect();
    let e = leaf::expand("", Ok(&recorded(&packages, 0)), Ok(&recorded(&packages, 0)), &changed);
    assert!(e.leaves.is_empty());
    assert_eq!(e.not_run.len(), 1);
    let why = e.not_run[0]["why"].as_str().unwrap();
    assert!(why.contains("runs its tests only in packages/cli, packages/core, and this change is outside them"), "{why}");
    let none = leaf::expand("cli", Ok(&recorded(&[], 0)), Ok(&recorded(&[], 0)), &["cli/src/a.ts"]);
    assert!(none.leaves.is_empty() && none.not_run[0]["why"].as_str().unwrap().contains(leaf::NO_LEAF), "{:?}", none.not_run);
    assert!(none.not_run[0]["why"].as_str().unwrap().contains("the test script of cli"), "a package's own script is named as the package's: {:?}", none.not_run);
    assert_eq!(none.not_run[0]["dir"], "cli");
    let failed = leaf::expand("", Err("it did not reach the end in 5 minutes".into()), Ok(&recorded(&packages, 0)), &changed);
    assert!(failed.leaves.is_empty() && failed.not_run[0]["why"].as_str().unwrap().starts_with("Kern could not learn which test runs the project's test script starts on the old code"));
}

#[test]
fn a_chain_that_stopped_while_kern_recorded_it_runs_what_it_saw_and_says_it_may_have_missed_runs() {
    let changed = ["src/a.ts"];
    let e = leaf::expand("", Ok(&recorded(&[run("jest", "/workspace", &[], "test")], 0)), Ok(&recorded(&[run("jest", "/workspace", &[], "test")], 2)), &changed);
    assert_eq!(e.leaves.len(), 1, "the run it saw is still evidence");
    assert!(e.not_run.iter().any(|n| n["why"].as_str().unwrap().contains("on the new code") && n["why"].as_str().unwrap().contains("may not have reached every test run")), "a chain that stopped early is never a full green: {:?}", e.not_run);
    let many: Vec<Value> = (0..12).map(|i| run("jest", "/workspace", &[&format!("-c={i:02}.js")], &format!("test-{i:02}"))).collect();
    let e = leaf::expand("", Ok(&recorded(&many, 0)), Ok(&recorded(&many, 0)), &changed);
    assert_eq!(e.leaves.len(), leaf::LEAF_SUITES_COUNT_MAX);
    assert_eq!(e.not_run.len(), 12 - leaf::LEAF_SUITES_COUNT_MAX, "every run over the cap is listed as not run");
}

#[test]
fn equal_test_names_from_two_runs_of_one_chain_never_collide() {
    let t = |runner: Runner, command: &[&str]| kern::policy::Tests { paths: Vec::new(), kind: kern::policy::Kind::Feature, runner, command: words(command), edits: kern::policy::Edits::Checked };
    let (int, unit, plain, mocha) = (t(Runner::Vitest, &[&bin("vitest"), "run", "--project", "int"]), t(Runner::Vitest, &[&bin("vitest"), "run", "--project", "unit"]), t(Runner::Vitest, &[&bin("vitest"), "run"]), t(Runner::Mocha, &[&bin("mocha")]));
    let none = |t: kern::policy::Tests| (t, kern::suite::Env::new());
    let (int, unit, plain, mocha) = (none(int), none(unit), none(plain), none(mocha));
    assert_eq!(leaf::names(&[("test".into(), &int), ("test".into(), &unit), ("lint".into(), &plain), (String::new(), &mocha)]), ["test (--project int)", "test (--project unit)", "lint", "mocha"],
        "two runs one script starts are told apart by what differs");
    assert_eq!(leaf::names(&[("test".into(), &plain), ("test".into(), &plain)]), ["test #1", "test #2"]);
    let build = (plain.0.clone(), kern::suite::Env::from([("VITE_TEST_BUILD".to_string(), "1".to_string())]));
    assert_eq!(leaf::names(&[("test".into(), &plain), ("test".into(), &build)]), ["test", "test (VITE_TEST_BUILD=1)"], "and by the variables that differ");
    let env = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<kern::suite::Env>();
    let cross_unit = (unit.0.clone(), env(&[("SUITE", "unit")]));
    let cross_int = (int.0.clone(), env(&[("SUITE", "int"), ("NODE_OPTIONS", "--max-old-space-size=1536")]));
    assert_eq!(leaf::names(&[("test".into(), &cross_unit), ("test".into(), &cross_int)]), ["test (--project unit)", "test (--project int)"],
        "the cross e2e's two runs: their arguments tell them apart, so their variables stay out of the name");
    let other = (plain.0.clone(), env(&[("VITE_TEST_BUILD", "1"), ("CI", "true")]));
    let same_ci = (plain.0.clone(), env(&[("CI", "true")]));
    assert_eq!(leaf::names(&[("test".into(), &same_ci), ("test".into(), &other)]), ["test", "test (VITE_TEST_BUILD=1)"], "a variable every run shares names none of them");
    let key = "src/a.test.ts > adds";
    let report = |status: &str| kern::proof::read_report(Runner::Vitest, json!({ "numTotalTests": 1, "testResults": [{ "name": "/workspace/src/a.test.ts", "status": "passed", "message": "",
        "assertionResults": [{ "ancestorTitles": [], "title": "adds", "status": status, "failureMessages": [] }] }] }).to_string().as_bytes()).unwrap();
    let broke = kern::suite::verdict(&report("passed"), &report("failed"), Some(&report("failed")), None, Some(&report("passed")), false, false);
    let kept = kern::suite::verdict(&report("passed"), &report("passed"), None, None, None, false, false);
    assert_eq!(broke["broke"], json!([key]));
    let mut merged: serde_json::Map<String, Value> = kern::suite::LISTS.iter().map(|l| (l.to_string(), json!([]))).collect();
    merged.insert("problem_why".into(), json!({}));
    kern::suite::fold(&mut merged, &kept, &|k| leaf::named(Some("unit"), Runner::Vitest, "", k));
    kern::suite::fold(&mut merged, &broke, &|k| leaf::named(Some("integration"), Runner::Vitest, "", k));
    assert_eq!(merged["broke"], json!([format!("integration: {key}")]), "the break is the integration run's, and the unit run's same test name is another test");
    assert_eq!(leaf::named(None, Runner::Node, "packages/a", "adds"), kern::suite::in_suite(Runner::Node, "packages/a", "adds"), "a suite that is not a leaf is named as today");
}

#[test]
fn wp_calypso_112403_each_changed_file_gets_its_faults_in_the_run_whose_tests_sit_nearest() {
    let report = |files: &[&str]| -> kern::proof::Report {
        let results: Vec<Value> = files.iter().map(|f| json!({ "name": format!("/workspace/{f}"), "status": "passed", "message": "", "assertionResults": [{ "ancestorTitles": [], "title": "t", "status": "passed", "failureMessages": [] }] })).collect();
        kern::proof::read_report(Runner::Jest, json!({ "numTotalTests": files.len(), "testResults": results }).to_string().as_bytes()).unwrap()
    };
    let (client, packages, server) = (report(&["client/state/test/a.js"]), report(&["packages/odie-client/src/hooks/test/use-get-combined-chat.test.tsx"]), report(&["client/server/pages/test/index.js"]));
    let leaves = [(0usize, "", &client), (1, "", &packages), (2, "", &server)];
    let paths = words(&["packages/odie-client/src/types.ts", "client/server/pages/index.js", "client/dashboard/app/router/index.tsx"]);
    let got = leaf::assigned(&paths, &leaves);
    assert_eq!(got.get(&1), Some(&words(&["packages/odie-client/src/types.ts"])));
    assert_eq!(got.get(&2), Some(&words(&["client/server/pages/index.js"])));
    assert_eq!(got.get(&0), Some(&words(&["client/dashboard/app/router/index.tsx"])), "a tie keeps the first run in order");
    let only = [(0usize, "packages/a", &client)];
    assert!(leaf::assigned(&words(&["packages/b/src/x.ts"]), &only).is_empty(), "a run never gets faults in a file outside its folder");
}

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Temp {
        let p = std::env::temp_dir().join(format!("kern-chain-{name}-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn jest(results: &[(&str, &str)]) -> String {
    let files: Vec<Value> = results.iter().map(|(f, status)| json!({ "name": format!("/workspace/{f}"), "status": "passed", "message": "",
        "assertionResults": [{ "ancestorTitles": [], "title": "adds", "status": status, "failureMessages": if *status == "failed" { json!(["expected 3 to be 4"]) } else { json!([]) } }] })).collect();
    json!({ "numTotalTests": results.len(), "testResults": files }).to_string()
}
fn stored(dir: &Path, tests: Value, leaves: Value, runs: &[(&str, String)]) {
    std::fs::create_dir_all(dir).unwrap();
    let mut t = json!({ "status": "RAN", "runner": "jest (`test-app`, from the project's test script), jest (`test-lib`, from the project's test script)", "before": {}, "after": {}, "kept": 0,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "new_failing": [], "already_failing": [], "new_problems": [], "environmental": [], "same_failure": [], "unproven": [],
        "together": [], "unconfirmed": [], "uneven": [], "uneven_why": [], "not_run": [], "run_broke": [], "run_unconfirmed": [], "rewritten": [], "problem_why": {} });
    for (k, v) in tests.as_object().unwrap() { t[k] = v.clone(); }
    let receipt = json!({ "version": 1, "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "decision": "clear", "blocked": [], "look": [],
        "tests": t, "rules": { "changed": [{ "path": "lib/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] } });
    std::fs::write(dir.join("receipt.json"), receipt.to_string()).unwrap();
    std::fs::write(dir.join("leaves.json"), leaves.to_string()).unwrap();
    for (role, text) in runs { std::fs::write(dir.join(format!("tests-{role}.out")), text).unwrap(); }
}
fn leaves_json() -> Value {
    let side = |config: &str| json!({ "runner": "jest", "command": [bin("jest"), format!("-c={config}")], "env": {} });
    json!([{ "label": "-leaf1", "name": "test-app", "prefix": "test-app", "suite": { "dir": "", "old": side("app.js"), "new": side("app.js"), "uneven": null } },
        { "label": "-leaf2", "name": "test-lib", "prefix": "test-lib", "suite": { "dir": "", "old": side("lib.js"), "new": side("lib.js"), "uneven": null } }])
}

#[test]
fn wp_calypso_114081_a_stored_chain_is_judged_again_leaf_by_leaf_with_the_same_names() {
    let t = Temp::new("replay");
    let dir = t.0.join("receipt-acme__chain__1");
    let same = "test/same.test.js";
    stored(&dir, json!({}), leaves_json(), &[
        ("old-leaf1", jest(&[(same, "passed")])), ("new-leaf1", jest(&[(same, "passed")])),
        ("old-leaf2", jest(&[(same, "passed")])), ("new-leaf2", jest(&[(same, "failed")])),
        ("new-alone-leaf2", jest(&[(same, "failed")])), ("old-alone-leaf2", jest(&[(same, "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["replayed"], 2, "{r}");
    assert_eq!(r["receipt"]["tests"]["broke"], json!([format!("test-lib: {same} > adds")]), "the second config's break is found and named by its run, though the first config has a test of the same name");
    assert_eq!(r["receipt"]["decision"], "blocked");
}

#[test]
fn a_leaf_whose_new_run_left_no_report_adds_nothing_and_never_passes_for_the_chain() {
    let t = Temp::new("cut");
    let dir = t.0.join("receipt-acme__chain__2");
    let same = "test/same.test.js";
    let not_run = json!([{ "dir": "", "why": "the tests of `test-lib` on the new code gave no results: the output contains no jest report (exit 1)" }]);
    stored(&dir, json!({ "not_run": not_run }), leaves_json(), &[
        ("old-leaf1", jest(&[(same, "passed")])), ("new-leaf1", jest(&[(same, "passed")])),
        ("old-leaf2", jest(&[(same, "passed")])), ("new-leaf2", "the output was cut off before".to_string()),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_ne!(r["receipt"]["decision"], "clear", "a leaf with no results keeps the answer from being clear: {}", r["receipt"]);
    assert_ne!(r["receipt"]["decision"], "blocked", "and a missing report is never a break");
    assert!(r["receipt"]["unverified"].as_array().unwrap().iter().any(|x| x["kind"] == "tests-not-run"));
}

#[test]
fn a_check_without_a_leaf_record_is_replayed_exactly_as_before() {
    let t = Temp::new("plain");
    let dir = t.0.join("receipt-acme__shop__1");
    stored(&dir, json!({ "runner": "jest" }), json!([]), &[("old", jest(&[("test/a.test.js", "passed")])), ("new", jest(&[("test/a.test.js", "passed")]))]);
    std::fs::remove_file(dir.join("leaves.json")).unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["replayed"], 1);
    assert_eq!(r["receipt"]["tests"]["broke"], json!([]));
    std::fs::write(dir.join("leaves.json"), "{ not json").unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["replayed"], 0, "a leaf record Kern cannot read whole is not read in part, so the stored lists stand");
}

#[test]
fn every_box_gets_the_recorder_and_the_node_that_loads_it() {
    let t = Temp::new("control");
    let control = t.0.join("control");
    kern::sandbox::control_dir(&control, None).unwrap();
    assert_eq!(std::fs::read_to_string(control.join("leaf.cjs")).unwrap(), kern::sandbox::LEAF);
    assert_eq!(std::fs::read_to_string(control.join("bin").join("node")).unwrap(), "#!/bin/sh\nexec /usr/local/bin/node --require=/kern/leaf.cjs \"$@\"\n");
    for (name, value) in kern::sandbox::LEAVES_ENV { assert!(kern::script::env_name(name) && !value.is_empty(), "{name} reaches the recorded run through the test box's own checks"); }
}

fn vite() -> Value {
    json!({ "name": "@vitejs/vite-monorepo", "private": true, "scripts": { "test": "pnpm test-unit && pnpm test-serve && pnpm test-build", "test-serve": "vitest run -c vitest.config.e2e.ts",
        "test-build": "VITE_TEST_BUILD=1 vitest run -c vitest.config.e2e.ts", "test-unit": "vitest run", "debug-build": "VITE_TEST_BUILD=1 VITE_PRESERVE_BUILD_ARTIFACTS=1 vitest run -c vitest.config.e2e.ts" },
        "devDependencies": { "vitest": "^3.2.4" }, "packageManager": "pnpm@10.18.3" })
}
fn with_env(mut r: Value, env: Value) -> Value {
    for (k, v) in env.as_object().unwrap() { r["env"][k] = v.clone(); }
    r
}
fn vite_runs() -> Vec<Value> {
    let e2e = ["run", "-c", "vitest.config.e2e.ts"];
    vec![run("vitest", "/workspace", &["run"], "test-unit"), run("vitest", "/workspace", &e2e, "test-serve"), with_env(run("vitest", "/workspace", &e2e, "test-build"), json!({ "VITE_TEST_BUILD": "1" }))]
}

#[test]
fn vitejs_vite_bb85bd7_test_serve_and_test_build_differ_only_by_a_variable_and_both_run() {
    assert_eq!(kern::script::chained(&vite()), Some("test".to_string()));
    let changed = ["packages/vite/src/node/build.ts"];
    let e = leaf::expand("", Ok(&recorded(&vite_runs(), 0)), Ok(&recorded(&vite_runs(), 0)), &changed);
    assert!(e.not_run.is_empty(), "{:?}", e.not_run);
    let mut names: Vec<&str> = e.leaves.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["test-build", "test-serve", "test-unit"], "the build-mode e2e run is its own run, though its command is test-serve's");
    let build = e.leaves.iter().find(|p| p.name == "test-build").unwrap();
    assert_eq!(build.new.1.get("VITE_TEST_BUILD").map(String::as_str), Some("1"), "{:?}", build.new.1);
    assert_eq!(build.old.as_ref().map(|o| &o.1), Some(&build.new.1), "each side runs it with its own variable");
    let serve = e.leaves.iter().find(|p| p.name == "test-serve").unwrap();
    assert!(serve.new.1.is_empty() && serve.new.0.command == build.new.0.command, "{:?}", serve.new);
}

#[test]
fn a_pr_that_sets_a_later_runners_node_options_gets_the_test_script_look_and_its_old_run_is_named() {
    let base = json!({ "scripts": { "test": "run-s test-app test-lib", "test-app": "jest -c=test/app/jest.config.js", "test-lib": "jest -c=test/lib/jest.config.js" } });
    let head = json!({ "scripts": { "test": "run-s test-app test-lib", "test-app": "jest -c=test/app/jest.config.js", "test-lib": "NODE_OPTIONS=--require=./lib/fix.js jest -c=test/lib/jest.config.js" } });
    assert!(!kern::script::same_tests(&base, &head), "the PR loads its own file into every run of the second config: the test script changes");
    let int = |inner: &str| json!({ "scripts": { "test": "run-s unit integration", "unit": "jest -c unit.js", "integration": inner, "integration-inner": "jest -c int.js" } });
    assert!(!kern::script::same_tests(&int("npm run integration-inner"), &int("cross-env NODE_OPTIONS=--require=./setup.js npm run integration-inner")),
        "a variable set around a later runner counts, though the script's name says nothing about tests");
    assert!(kern::script::same_tests(&int("npm run integration-inner"), &int("npm run integration-inner")));
    let shown = kern::script::runs(&head);
    assert_eq!(shown.len(), 2);
    assert!(kern::script::files(&shown[1]).iter().any(|f| f == "./lib/fix.js"), "{:?}", shown[1]);
    assert_eq!(kern::script::runs(&int("cross-env NODE_OPTIONS=--require=./setup.js npm run integration-inner"))[1].env.get("NODE_OPTIONS").map(String::as_str), Some("--require=./setup.js"));
    let old = recorded(&[run("jest", "/workspace", &["-c=test/app/jest.config.js"], "test-app"), run("jest", "/workspace", &["-c=test/lib/jest.config.js"], "test-lib")], 0);
    let new = recorded(&[run("jest", "/workspace", &["-c=test/app/jest.config.js"], "test-app"),
        with_env(run("jest", "/workspace", &["-c=test/lib/jest.config.js"], "test-lib"), json!({ "NODE_OPTIONS": "--require=./lib/fix.js" }))], 0);
    let e = leaf::expand("", Ok(&old), Ok(&new), &["lib/total.js"]);
    let why: Vec<&str> = e.not_run.iter().filter_map(|n| n["why"].as_str()).collect();
    assert!(why.iter().any(|w| w.contains("starts `jest -c=test/lib/jest.config.js` on the old code but not on the new code")), "{why:?}");
    let lib = e.leaves.iter().find(|p| p.name == "test-lib").unwrap();
    assert!(lib.old.is_none(), "the old code never ran the second config with the PR's preload, so nothing on the old side pairs with it: {:?}", lib.old);
}

#[test]
fn the_same_run_reached_from_two_scripts_is_named_the_same_on_every_check() {
    let a = run("jest", "/workspace", &["-c=x.js"], "test:b");
    let b = run("jest", "/workspace", &["-c=x.js"], "test:a");
    let other = run("jest", "/workspace", &["-c=y.js"], "test:c");
    let names = |runs: &[Value]| -> Vec<String> { leaf::expand("", Ok(&recorded(runs, 0)), Ok(&recorded(runs, 0)), &["src/x.ts"]).leaves.into_iter().map(|p| p.name).collect() };
    assert_eq!(names(&[a.clone(), b.clone(), other.clone()]), names(&[b, a, other]), "a parallel chain records its runs in any order");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn committed(r: &Path, files: &[(&str, &str)], message: &str) -> String {
    for (p, text) in files {
        let path = r.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "--allow-empty", "-m", message]);
    git(r, &["rev-parse", "HEAD"])
}

#[test]
fn a_file_a_later_runner_of_the_chain_preloads_is_named_by_the_test_script() {
    let t = Temp::new("rules");
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    let package = r#"{"name":"shop","scripts":{"test":"run-s test-app test-lib","test-app":"jest -c=test/app/jest.config.js","test-lib":"NODE_OPTIONS=--require=./lib/fix.js jest -c=test/lib/jest.config.js"},"devDependencies":{"jest":"30.0.0","npm-run-all":"4.1.5"}}"#;
    let base = committed(&r, &[("package.json", package), ("lib/fix.js", "module.exports = {};\n"), ("lib/total.js", "exports.total = (a, b) => a + b;\n")], "base");
    let head = committed(&r, &[("lib/fix.js", "const assert = require('assert');\nassert.strictEqual = () => {};\n"), ("lib/total.js", "exports.total = (a, b) => a - b;\n")], "change");
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    assert!(rules["settings"].as_array().into_iter().flatten().any(|s| s["path"] == "lib/fix.js"), "the second config's runs load lib/fix.js, which the change rewrites: {}", rules["settings"]);
}

#[test]
fn wp_calypso_114128_a_narrowed_jest_run_that_matches_nothing_reports_zero_tests_instead_of_no_results() {
    let files: std::collections::BTreeSet<String> = ["packages/odie-client/src/index.tsx".to_string(), "packages/odie-client/src/types.ts".to_string()].into();
    let client = kern::policy::Tests { paths: Vec::new(), kind: kern::policy::Kind::Feature, runner: Runner::Jest, command: words(&[&bin("jest"), "-c=test/client/jest.config.js"]), edits: kern::policy::Edits::Checked };
    let narrowed = kern::suite::related(&client, &files, "").unwrap();
    assert_eq!(narrowed.command, words(&[&bin("jest"), "-c=test/client/jest.config.js", "--passWithNoTests", "--findRelatedTests", "/workspace/packages/odie-client/src/index.tsx", "/workspace/packages/odie-client/src/types.ts"]),
        "jest exits 1 before Kern's report when no test of this config is related, and calypso's client config sees none of packages/");
    assert!(kern::suite::test_command(&narrowed).iter().any(|a| a == "--passWithNoTests"));
    assert!(kern::suite::passes_with_none(&narrowed) && !kern::suite::passes_with_none(&client), "only Kern's narrowing passes with no tests");
    let warning = "\u{25cf} Validation Warning:\n\n  Unknown option \"verbose\" with value false was found.\n  This is probably a typing mistake. Fixing it will remove this message.\n";
    let empty = b"{\"numTotalTests\":0,\"testResults\":[]}";
    assert!(!kern::suite::failed_exit(b"exit 0") && !kern::suite::stopped_early(b"exit 0", warning.as_bytes(), empty), "calypso's own warning is not an error");
    assert!(kern::proof::read_report(Runner::Jest, empty).is_ok_and(|r| r.tests.is_empty() && r.problems.is_empty()));
    let jest = kern::policy::Tests { command: words(&[&bin("jest"), "--findRelatedTests", "/workspace/a.ts"]), ..client };
    assert!(!kern::suite::passes_with_none(&jest), "a project's own --findRelatedTests without --passWithNoTests keeps today's reading");
    let own = kern::policy::Tests { command: words(&[&bin("jest"), "--passWithNoTests", "--findRelatedTests", "src/a.ts"]), ..jest.clone() };
    assert!(!kern::suite::passes_with_none(&own), "a project's own command with both flags is not Kern's narrowing, which ends in the changed files' paths in the box");
}

type Files = std::collections::BTreeMap<(String, String), Vec<u8>>;
fn files(list: &[(&str, &str, Value)]) -> Files { list.iter().map(|(c, p, v)| ((c.to_string(), p.to_string()), v.to_string().into_bytes())).collect() }
fn planned(files: &Files, paths: &[&str]) -> kern::suite::Plans {
    let read = |c: &str, p: &str, bytes_max: usize| files.get(&(c.to_string(), p.to_string())).filter(|b| b.len() <= bytes_max).cloned();
    let has = |c: &str, p: &str| files.contains_key(&(c.to_string(), p.to_string()));
    kern::suite::plans(&kern::suite::Tree { read: &read, has: &has, base: "base", head: "head" }, paths)
}

#[test]
fn a_chain_the_pr_cuts_to_one_runner_is_recorded_on_both_sides_and_its_script_change_is_a_look() {
    let scripts = |test: &str| json!({ "scripts": { "test": test, "test-unit": "jest -c=unit.js", "test-int": "jest -c=int.js" }, "devDependencies": { "jest": "29.7.0", "npm-run-all": "4.1.5" } });
    let (both, one) = (scripts("run-s test-unit test-int"), scripts("run-s test-unit"));
    assert!(!kern::script::same_tests(&both, &one), "the PR drops the integration config from what the tests check");
    let p = planned(&files(&[("base", "package.json", both.clone()), ("head", "package.json", one.clone())]), &["src/a.ts"]);
    assert_eq!(p.chains, [(String::new(), vec!["test".to_string()])], "the old code's chain is recorded on both sides, so its second config is named as not run on the new code");
    assert!(p.suites.is_empty());
    let p = planned(&files(&[("base", "package.json", one.clone()), ("head", "package.json", one.clone())]), &["src/a.ts"]);
    assert!(p.chains.is_empty() && p.suites.len() == 1, "one runner on both sides keeps today's path");
    let p = planned(&files(&[("base", "package.json", one.clone()), ("head", "package.json", both.clone())]), &["src/a.ts"]);
    assert_eq!(p.chains, [(String::new(), vec!["test".to_string()])]);
    let root = json!({ "private": true, "workspaces": ["packages/*"], "scripts": { "test": "turbo run test" } });
    let p = planned(&files(&[("base", "package.json", root.clone()), ("head", "package.json", root.clone()), ("base", "packages/x/package.json", both), ("head", "packages/x/package.json", one)]),
        &["packages/x/src/a.ts"]);
    assert_eq!(p.chains, [("packages/x".to_string(), vec!["test".to_string()])], "the same holds for a package's own script");
}

#[test]
fn a_changed_file_no_recorded_run_holds_is_named_so_it_is_never_silently_unplanted() {
    let runs = [run("jest", "/workspace/pkg/unit", &[], "test"), run("jest", "/workspace/pkg/int", &[], "test")];
    let changed = ["pkg/src/x.ts", "pkg/unit/x.test.ts", "pkg/int/helper.ts", "other/y.ts"];
    let e = leaf::expand("pkg", Ok(&recorded(&runs, 0)), Ok(&recorded(&runs, 0)), &changed);
    assert_eq!(e.leaves.iter().map(|p| p.dir.as_str()).collect::<Vec<_>>(), ["pkg/int", "pkg/unit"]);
    assert_eq!(e.unheld, ["pkg/src/x.ts"], "pkg/src is in no recorded run's folder, so no run's tests would get its faults; other/ is not this script's to judge");
    assert_eq!(leaf::unheld_why("pkg"), "no test run of the test script of pkg starts in its folder");
    let one_side = leaf::expand("pkg", Ok(&recorded(&runs, 0)), Ok(&recorded(&runs[..1], 0)), &changed);
    assert_eq!(one_side.unheld, ["pkg/src/x.ts"], "a run on either side holds its folder");
    let failed = leaf::expand("pkg", Err("it did not reach the end in 5 minutes".into()), Ok(&recorded(&runs, 0)), &changed);
    assert!(failed.unheld.is_empty() && !failed.not_run.is_empty(), "a recording Kern could not read says so, and names nothing it did not learn");
}

#[test]
fn a_package_script_kern_cannot_follow_is_recorded_only_when_its_words_or_its_program_name_a_runner() {
    let root = json!({ "private": true, "workspaces": ["apps/*", "packages/*"], "scripts": { "test": "turbo run test" }, "devDependencies": { "jest": "29.7.0", "vitest": "3.2.4" } });
    let package = |test: &str| json!({ "name": "p", "scripts": { "test": test, "test:unit": "vitest run src/*.test.ts" } });
    let mut list = vec![("head", "package.json", root.clone()), ("base", "package.json", root)];
    for (dir, test) in [("apps/e2e", "playwright test"), ("packages/cli", "node scripts/run-tests.mjs"), ("packages/tsx", "node --require ./setup.cjs scripts/run-tests.mjs"),
        ("packages/glob", "npm run test:unit")] {
        for commit in ["base", "head"] { list.push((commit, Box::leak(format!("{dir}/package.json").into_boxed_str()), package(test))); }
    }
    let mut files = files(&list);
    let raw = |text: &str| text.as_bytes().to_vec();
    files.insert(("head".into(), "packages/cli/scripts/run-tests.mjs".into()), raw("import { spawnSync } from 'node:child_process';\nconst r = spawnSync('vitest', ['run', 'test'], { stdio: 'inherit' });\n"));
    files.insert(("head".into(), "packages/tsx/scripts/run-tests.mjs".into()), raw("import { spawnSync } from 'node:child_process';\nspawnSync('tsx', ['--test', 'test/*.ts'], { stdio: 'inherit' });\n"));
    let p = planned(&files, &["apps/e2e/src/a.ts", "packages/cli/src/a.ts", "packages/tsx/src/a.ts", "packages/glob/src/a.ts"]);
    let chained: Vec<&str> = p.chains.iter().map(|(d, _)| d.as_str()).collect();
    assert_eq!(chained, ["packages/cli", "packages/glob"], "only scripts that start vitest, jest or mocha by name, themselves or through their program, are recorded: {:?}", p.not_run);
    let why: Vec<&str> = p.not_run.iter().filter_map(|n| n["why"].as_str()).collect();
    assert_eq!(why, ["Kern cannot run this package's test script: playwright test", "Kern cannot run this package's test script: node --require ./setup.cjs scripts/run-tests.mjs"],
        "a Playwright package in a monorepo whose root installs jest keeps today's answer, with no recording, and so does MCPJam/inspector#4342's tsx --test program");
    assert!(kern::script::names_runner("npx jest-cli --ci") && kern::script::names_runner("node_modules/.bin/mocha") && !kern::script::names_runner("ajestic majesty"));
    assert_eq!(kern::script::program_files("FOO=1 node --require ./a.cjs --max-old-space-size=4096 scripts/x.mjs && ./bin/run.sh --fast && tsx t.ts && npx vitest"), ["scripts/x.mjs", "./bin/run.sh", "t.ts"]);
}

#[test]
fn recordings_share_one_time_budget_per_check_so_the_core_runs_keep_their_time() {
    assert_eq!(leaf::recording_secs(0), Some(leaf::DISCOVER_SECS_MAX));
    for spent in 0..=leaf::RECORDINGS_SECS_MAX + 60 {
        if let Some(side) = leaf::recording_secs(spent) {
            assert!(side >= leaf::DISCOVER_SECS_MIN && spent + 2 * side <= leaf::RECORDINGS_SECS_MAX, "{spent} s spent: both sides of the next recording fit the budget");
        }
    }
    assert_eq!(leaf::recording_secs(leaf::RECORDINGS_SECS_MAX - 2 * leaf::DISCOVER_SECS_MIN + 1), None);
    assert!(leaf::out_of_time("apps/web")["why"].as_str().unwrap().contains("ran out before it could learn those of the test script of apps/web"));
}

#[test]
fn a_recorded_run_whose_environment_passes_the_test_boxs_limits_once_kern_adds_its_heap_flag_is_named_never_cut() {
    let crowded = |n: usize, runner: &str| {
        let mut r = run(runner, "/workspace", &[], "test");
        r["env"] = Value::Object((0..n).map(|i| (format!("VAR_{i:02}"), json!("x"))).collect());
        recorded(&[r], 0)
    };
    assert!(leaf::side(&crowded(31, "jest").leaves[0]).is_ok(), "31 of the chain's variables and Kern's heap flag fit the box");
    let why = leaf::side(&crowded(32, "jest").leaves[0]).unwrap_err();
    assert!(why.contains("32 variables"), "{why}");
    assert!(leaf::side(&crowded(32, "mocha").leaves[0]).is_ok(), "Kern sets no heap flag for mocha");
    let mut long = run("vitest", "/workspace", &["run"], "test");
    long["env"]["NODE_OPTIONS"] = json!(format!("{}--expose-gc", "-r ./x.cjs ".repeat(370)));
    assert!(leaf::side(&recorded(&[long], 0).leaves[0]).unwrap_err().contains("4096 characters"));
    let env = kern::suite::Env::from([("NODE_OPTIONS".to_string(), "x".repeat(4090))]);
    assert!(kern::sandbox::box_env_fits(&env) && !kern::sandbox::box_env_fits(&kern::suite::sized_env(Runner::Jest, &env)), "the check reads what the box is sent");
}

#[test]
fn a_jest_cache_clear_and_a_vitest_benchmark_are_not_test_runs_and_a_vitest_type_test_is_named() {
    let r = recorded(&[run("jest", "/workspace", &["--clearCache"], "pretest"), run("vitest", "/workspace", &["bench", "--run"], "test:bench"),
        run("vitest", "/workspace", &["typecheck", "--run"], "test:types"), run("jest", "/workspace", &["-c=a.js"], "test")], 0);
    assert!(leaf::informational(&r.leaves[0]) && leaf::informational(&r.leaves[1]), "a cache clear and a benchmark check no behaviour");
    assert!(!leaf::informational(&r.leaves[2]) && leaf::side(&r.leaves[2]).unwrap_err().contains("Kern cannot run it the way it was started"), "a type test Kern cannot run is named, never a silent pass");
    let e = leaf::expand("", Ok(&r), Ok(&r), &["src/a.ts"]);
    assert_eq!(e.leaves.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["test"]);
    assert_eq!(e.not_run.len(), 2, "the type test, once per side: {:?}", e.not_run);
}

#[test]
fn a_runner_kern_cannot_find_when_it_repeats_a_recorded_run_is_kerns_limit_never_the_changes() {
    use kern::suite::Side;
    let missing = Side::Nothing("Kern could not run the tests in the sandbox: The test runner is not installed in the project: /workspace/packages/cli/node_modules/.bin/jest does not exist".into());
    assert!(matches!(kern::suite::repeated_runner(missing), Side::Cut(w) if w.contains("the project's own test script started this run")));
    assert!(matches!(kern::suite::repeated_runner(Side::Nothing("the output contains no jest report".into())), Side::Nothing(_)), "only the missing runner is read this way");
    assert!(leaf::NO_LEAF.contains("turbo's strict environment mode"), "the limit is named where the receipt says Kern saw no run");
}

fn plain_run(runner: &str, cwd: &str, argv: &[&str], event: &str, env: Value) -> Value {
    json!({ "runner": runner, "cwd": cwd, "argv": argv, "node": [], "env": env, "event": event })
}

#[test]
fn the_program_a_recorded_script_starts_is_named_by_the_rules_so_a_pr_that_rewrites_it_never_gets_clear() {
    let t = Temp::new("program");
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    let root = r#"{"name":"program","private":true,"type":"module","workspaces":["packages/*"],"scripts":{"test":"npm run test --workspaces"},"devDependencies":{"vitest":"3.2.4"}}"#;
    let cli = r#"{"name":"@p/cli","version":"1.0.0","private":true,"type":"module","scripts":{"test":"node scripts/run-tests.mjs"}}"#;
    let both = "import { spawnSync } from 'node:child_process';\nlet r = spawnSync('vitest', ['run', 'test'], { stdio: 'inherit' });\nif (r.status) process.exit(r.status);\nr = spawnSync('vitest', ['run'], { stdio: 'inherit', cwd: 'integration' });\nprocess.exit(r.status ?? 1);\n";
    let one = "import { spawnSync } from 'node:child_process';\nconst r = spawnSync('vitest', ['run', 'test'], { stdio: 'inherit' });\nprocess.exit(r.status ?? 1);\n";
    let base = committed(&r, &[("package.json", root), ("packages/cli/package.json", cli), ("packages/cli/scripts/run-tests.mjs", both), ("packages/cli/src/total.js", "export const total = (a, b) => a + b;\n")], "base");
    let head = committed(&r, &[("packages/cli/scripts/run-tests.mjs", one), ("packages/cli/src/total.js", "export const total = (a, b) => a - b;\n")], "change");
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let named = rules["settings"].as_array().into_iter().flatten().find(|s| s["path"] == "packages/cli/scripts/run-tests.mjs");
    assert!(named.is_some_and(|s| s["why"].as_str().unwrap().contains("starts this program")), "the program decides which test runs start, and the PR rewrote it: {}", rules["settings"]);
    let tests = json!({ "status": "RAN", "runner": "vitest in packages/cli (`test`, from the project's test script)", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "faults": [], "faults_missing": [] });
    let receipt = kern::receipt::build(&json!({ "base": base, "head": head, "skipped": [], "unmeasured": [] }), &rules, &tests);
    assert_eq!(receipt["decision"], "look", "at BASE this check was not verified; hiding the integration run must never make it clear");
    let followed = json!({ "scripts": { "test": "vitest run", "build": "node scripts/build.mjs" } });
    assert!(kern::script::started_programs(&followed).is_empty(), "a script Kern follows runs its runner itself, so the programs its other scripts start decide nothing");
}

#[test]
fn an_old_run_outside_every_changed_folder_that_the_new_code_drops_is_named_not_verified() {
    let unit = plain_run("vitest", "/workspace/packages/cli", &["run", "test"], "test", json!({}));
    let integration = plain_run("vitest", "/workspace/packages/cli/integration", &["run"], "test", json!({}));
    let changed = ["packages/cli/src/total.js", "packages/cli/scripts/run-tests.mjs"];
    let e = leaf::expand("packages/cli", Ok(&recorded(&[unit.clone(), integration], 0)), Ok(&recorded(&[unit], 0)), &changed);
    let why: Vec<&str> = e.not_run.iter().filter_map(|n| n["why"].as_str()).collect();
    assert!(why.iter().any(|w| w.contains("starts `vitest run` in packages/cli/integration on the old code but not on the new code")), "the integration tests the old code ran are gone: {why:?}");
}

#[test]
fn an_old_side_with_no_run_kern_saw_and_a_new_side_with_one_is_not_verified_never_all_new() {
    let unit = plain_run("vitest", "/workspace/packages/cli", &["run", "test"], "test", json!({}));
    let changed = ["packages/cli/src/total.js", "packages/cli/scripts/run-tests.mjs"];
    let e = leaf::expand("packages/cli", Ok(&recorded(&[], 0)), Ok(&recorded(std::slice::from_ref(&unit), 0)), &changed);
    let why: Vec<&str> = e.not_run.iter().filter_map(|n| n["why"].as_str()).collect();
    assert!(why.iter().any(|w| w.starts_with("on the old code, Kern ran the test script of packages/cli with each test runner replaced by a recorder, and it started no vitest, jest or mocha run Kern could see")), "none of the old tests ran on the new code: {why:?}");
    let info = plain_run("vitest", "/workspace/packages/cli", &["--version"], "test", json!({}));
    let e = leaf::expand("packages/cli", Ok(&recorded(&[info], 0)), Ok(&recorded(&[unit], 0)), &changed);
    assert!(!e.not_run.is_empty(), "a run that checks no behaviour is not a run Kern saw");
}

#[test]
fn an_honest_chain_whose_integration_run_sits_outside_the_changed_folder_still_runs_it_after_the_runs_nearest_the_change() {
    let unit = plain_run("jest", "/workspace/pkg", &["-c", "unit.config.js"], "test", json!({}));
    let integration = plain_run("vitest", "/workspace/pkg/integration", &["run"], "test", json!({}));
    let near = plain_run("jest", "/workspace/pkg/src", &[], "test:src", json!({}));
    let runs = [unit, integration, near];
    let e = leaf::expand("pkg", Ok(&recorded(&runs, 0)), Ok(&recorded(&runs, 0)), &["pkg/src/total.js"]);
    let planned: Vec<(&str, bool)> = e.leaves.iter().map(|p| (p.dir.as_str(), p.old.is_some())).collect();
    assert_eq!(planned, [("pkg/src", true), ("pkg", true), ("pkg/integration", true)], "the integration tests may import the change, so they run too, after the runs whose folder holds it");
    assert!(e.not_run.is_empty() && e.unheld.is_empty(), "{:?} {:?}", e.not_run, e.unheld);
}

#[test]
fn a_recording_made_on_one_side_only_or_that_ended_differently_makes_every_later_suite_uneven() {
    let runs = [run("jest", "/workspace", &[], "test")];
    let (fine, failed) = (Ok(recorded(&runs, 0)), Ok(recorded(&runs, 2)));
    assert_eq!(leaf::uneven_recording("pkg", "test", Some(&fine), &fine), None, "the same script run the same way on both trees leaves them even");
    let one = leaf::uneven_recording("pkg", "test", None, &fine).unwrap();
    assert!(one.contains("`npm run test` in pkg") && one.contains("on the new code only"), "{one}");
    assert!(leaf::uneven_recording("pkg", "test", Some(&fine), &failed).unwrap().contains("ended differently"), "a build step that failed on one side left the trees different");
    let cut: Result<Recorded, String> = Err("it did not reach the end in 5 minutes".into());
    assert!(leaf::uneven_recording("", "test", Some(&cut), &cut).unwrap().contains("did not finish"), "a recording that did not finish may have left either tree part-built");
}
