use kern::exec::{self, Cached, Ran, Stored};
use kern::plan::{self, Kind, Outcome, Plan, Run, StepRecord};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

fn plan_json() -> Value {
    json!({
        "key": "b".repeat(64), "source": { "ci": { "file": ".github/workflows/ci.yml", "job": "test" } },
        "toolchain": { "node": 22, "manager": { "name": "npm", "version": null, "integrity": null }, "browsers": ["chromium"] },
        "env": { "CI": "true" },
        "steps": [
            { "id": "install-1", "kind": "install", "dir": "", "line": "npm ci" },
            { "id": "install-2", "kind": "install", "dir": "packages/x", "line": "npm ci" },
            { "id": "install-3", "kind": "install", "dir": "", "line": "npx playwright install chromium" },
            { "id": "run-4", "kind": "run", "dir": "", "line": "npm run build" },
            { "id": "test-5", "kind": "test", "dir": "packages/x", "line": "npx vitest run" },
            { "id": "check-6", "kind": "check", "dir": "", "line": "npx tsc --noEmit" },
            { "id": "test-7", "kind": "test", "dir": "", "line": "npm test" }
        ],
        "needs": []
    })
}
fn a_plan() -> Plan { plan::parse(&plan_json()).unwrap() }
fn leaf_line(runner: &str, cwd: &str, argv: &[&str]) -> String { json!({ "runner": runner, "cwd": cwd, "argv": argv, "node": [], "env": {}, "event": "test" }).to_string() }
fn ended(exit: i64) -> String { json!({ "end": true, "exit": exit, "signal": null, "error": null }).to_string() }
fn setup(old: &str, new: &str) -> Value {
    let side = |result: &str| json!({ "volume": "kern-deps-x", "installs": [{ "dir": ".", "manager": "npm", "result": result, "parts": [] }], "browser": { "why": "playwright", "installed": ["1.49.1"] }, "scripts": true });
    json!({ "version": 1, "old": side(old), "new": side(new) })
}

#[test]
fn exec_pair_is_the_only_caller_of_the_plan_runner_so_two_sides_cannot_run_two_plans() {
    let exec = std::fs::read_to_string("src/exec.rs").unwrap();
    let pair = exec.split("pub fn pair(").nth(1).unwrap().split("\n}\n").next().unwrap();
    assert_eq!(exec.matches("fn side(").count(), 1, "one private runner of a plan on one side");
    assert_eq!(exec.matches("side(out, plan, setup, (").count(), 2, "the runner is called once per side, nowhere else");
    assert_eq!(pair.matches("side(out, plan, setup, (").count(), 2, "both calls are inside pair, with the same plan");
    assert!(pair.contains("(\"old\", old)") && pair.contains("(\"new\", new)"));
    assert_eq!(exec.matches("listed(plan)").count(), 1, "the box's plan steps are built only for the runner");
    assert!(exec.split("fn side(").nth(1).unwrap().split("\n}\n").next().unwrap().contains("listed(plan)"));
    for file in std::fs::read_dir("src").unwrap().flatten() {
        let name = file.file_name().to_string_lossy().to_string();
        if !name.ends_with(".rs") { continue; }
        let src = std::fs::read_to_string(file.path()).unwrap();
        let expected = if name == "suite.rs" { 1 } else { 0 };
        assert_eq!(src.matches("exec::pair(").count(), expected, "src/{name}: exec::pair is called from the check's ci_suites and nowhere else outside exec");
        if name == "exec.rs" { assert_eq!(src.matches("= pair(out, ").count(), 1, "the learn path is exec's one own caller of pair"); }
        if name != "exec.rs" { assert_eq!(src.matches("\"plan-").count(), 0, "src/{name}: only exec runs a plan in the box"); }
    }
    let sandbox = std::fs::read_to_string("src/sandbox.rs").unwrap();
    assert_eq!(sandbox.matches("fn plan_step(").count(), 1);
    assert!(sandbox.contains("if step.get(\"line\").is_some() { return plan_step(step, (env, options), wait); }"), "a plan step is one shape of build step, run by the one step runner");
    assert!(sandbox.contains("wait.min(Duration::from_secs(super::PLAN_STEP_SECS_MAX))"), "each plan step is bounded on its own");
    assert!(sandbox.contains("command.envs(super::LEAVES_ENV).env(\"KERN_REPORT\", &leaves)") && sandbox.contains("command.args([\"/kern/leaf.cjs\", \"/bin/bash\"]);"), "a test step runs under the recorder with the caches off");
    assert!(sandbox.contains("\"-e\", \"-c\", line"), "every step is one bash -e line");
}

#[test]
fn a_plan_s_steps_are_read_from_the_box_s_typed_records_never_from_log_text() {
    let plan = a_plan();
    let results = json!([
        { "name": "run-4", "kind": "run", "finished": true, "exit": 0, "errors": "\n", "leaves": null },
        { "name": "test-5", "kind": "test", "finished": true, "exit": 0, "errors": "", "leaves": format!("{}\n{}\n", leaf_line("vitest", "/workspace/packages/x", &["run"]), ended(0)) },
        { "name": "check-6", "kind": "check", "finished": true, "exit": 2, "errors": "src/a.ts(3,1): error TS2322: Type 'string' is not assignable\nnpm ERR! Lifecycle script failed\n", "leaves": null }
    ]);
    let (steps, leaves) = exec::read_run(&plan, results.to_string().as_bytes()).unwrap();
    let ids: Vec<&str> = steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["run-4", "test-5", "check-6", "test-7"], "install steps come from the install box, every other step from the plan in order");
    assert_eq!((steps[0].exit, steps[0].error.as_str()), (Some(0), ""));
    assert_eq!(steps[1].exit, Some(0));
    assert_eq!(steps[2].exit, Some(2));
    assert!(steps[2].error.contains("error TS2322"), "a failed step's first error is quoted: {}", steps[2].error);
    assert_eq!((steps[3].exit, steps[3].error.as_str()), (None, exec::NOT_REACHED), "a step with no record was never reached");
    assert_eq!(leaves.len(), 2);
    let five = leaves[0].1.as_ref().unwrap();
    assert_eq!((leaves[0].0.as_str(), five.leaves.len(), five.leaves[0].dir.as_deref(), five.exit), ("test-5", 1, Some("packages/x"), Some(0)));
    assert_eq!(leaves[1], ("test-7".to_string(), Err(exec::NOT_REACHED.to_string())));
    let stopped = json!([{ "name": "run-4", "kind": "run", "finished": false, "exit": null, "errors": "still building", "leaves": null }]);
    let (steps, leaves) = exec::read_run(&plan, stopped.to_string().as_bytes()).unwrap();
    assert_eq!(steps[0].exit, None);
    assert!(steps[0].error.contains("did not finish in the 900 s"), "{}", steps[0].error);
    assert!(leaves.iter().all(|(_, l)| l == &Err(exec::NOT_REACHED.to_string())));
    let cut = json!([{ "name": "test-5", "kind": "test", "finished": true, "exit": 0, "errors": "", "leaves": leaf_line("vitest", "/workspace", &[]) }]);
    let (_, leaves) = exec::read_run(&plan, cut.to_string().as_bytes()).unwrap();
    assert!(leaves[0].1.as_ref().unwrap_err().contains("stopped before Kern's recorder could say how it ended"), "a record without its end line is not trusted: {:?}", leaves[0].1);
    assert!(exec::read_run(&plan, b"not json").is_err());
    assert!(exec::read_run(&plan, json!([{ "name": "run-4", "finished": true, "exit": "0" }]).to_string().as_bytes()).is_err(), "an exit status that is not a number is refused");
    assert_eq!(exec::listed(&plan).len(), 4);
    assert!(exec::listed(&plan).iter().all(|s| s["kind"] != "install" && s["line"].is_string() && s["id"].is_string()));
}

#[test]
fn install_steps_are_judged_by_todays_install_box_records() {
    let plan = a_plan();
    let fine = exec::install_records(&plan, &setup("installed from the lockfile", "installed from the lockfile"), "old");
    assert_eq!(fine.iter().map(|s| (s.id.as_str(), s.kind, s.exit)).collect::<Vec<_>>(), [("install-1", Kind::Install, Some(0)), ("install-2", Kind::Install, Some(0)), ("install-3", Kind::Install, Some(0))]);
    assert!(fine.iter().all(|s| s.error.is_empty()));
    let failed = exec::install_records(&plan, &setup("installed from the lockfile", "failed: npm ERR! code ERESOLVE\nnpm ERR! could not resolve"), "new");
    assert_eq!(failed[0].exit, None);
    assert!(failed[0].error.starts_with("installing its dependencies with npm failed: npm ERR!"), "{}", failed[0].error);
    let mut none = setup("installed from the lockfile", "installed from the lockfile");
    none["old"]["installs"] = json!([]);
    none["old"]["browser"] = Value::Null;
    let missing = exec::install_records(&plan, &none, "old");
    assert_eq!((missing[0].exit, missing[0].error.as_str()), (None, "Kern did not install in ."));
    assert_eq!((missing[1].exit, missing[1].error.as_str()), (None, "Kern did not install in packages/x"));
    assert_eq!((missing[2].exit, missing[2].error.as_str()), (None, "Kern did not set up a browser on this side"));
    let mut broken = setup("installed from the lockfile", "installed from the lockfile");
    broken["new"]["browser"]["error"] = json!("the image could not be built");
    let browser = exec::install_records(&plan, &broken, "new");
    assert_eq!((browser[2].exit, browser[2].error.as_str()), (None, "Kern could not set up the browser: the image could not be built"));
}

fn record(id: &str, kind: Kind, exit: Option<i32>, error: &str) -> StepRecord { StepRecord { id: id.into(), kind, exit, error: error.into() } }

#[test]
fn a_step_that_did_not_finish_or_failed_on_one_side_only_makes_the_sides_uneven_never_a_block() {
    let fine = vec![record("install-1", Kind::Install, Some(0), ""), record("run-4", Kind::Run, Some(0), ""), record("test-5", Kind::Test, Some(1), "")];
    assert_eq!(exec::uneven(&fine, &fine), None, "the same steps with the same exits are even, whatever a test step's exit");
    let both_fail = vec![record("install-1", Kind::Install, Some(0), ""), record("run-4", Kind::Run, Some(1), "boom"), record("test-5", Kind::Test, None, exec::NOT_REACHED)];
    assert_eq!(exec::uneven(&both_fail, &both_fail), None, "a build that fails the same way on both sides, and the steps neither side reached, keep the sides even");
    let new_broke = vec![record("install-1", Kind::Install, Some(0), ""), record("run-4", Kind::Run, Some(1), "boom"), record("test-5", Kind::Test, None, exec::NOT_REACHED)];
    let said = exec::uneven(&fine, &new_broke).unwrap();
    assert!(said.contains("run-4 worked on one side only") && said.contains("test-5 has no recorded result on one side"), "{said}");
    let stopped = vec![record("install-1", Kind::Install, Some(0), ""), record("run-4", Kind::Run, None, "it did not finish in the 900 s Kern gives one step"), record("test-5", Kind::Test, None, exec::NOT_REACHED)];
    assert!(exec::uneven(&fine, &stopped).unwrap().contains("run-4 worked on one side only"), "a step Kern's time limit stopped on one side is uneven");
    let install_failed = vec![record("install-1", Kind::Install, None, "installing its dependencies with npm failed"), record("run-4", Kind::Run, Some(1), ""), record("test-5", Kind::Test, Some(1), "")];
    let said = exec::uneven(&fine, &install_failed).unwrap();
    assert!(said.contains("install-1") && said.contains("worked on one side only"), "{said}");
}

#[test]
fn the_test_steps_leaves_are_merged_per_side_and_a_step_kern_could_not_record_fails_that_side() {
    let read = |text: &str| kern::leaf::read(text.as_bytes());
    let five = read(&format!("{}\n{}\n", leaf_line("vitest", "/workspace/packages/x", &["run"]), ended(0))).unwrap();
    let seven = read(&format!("{}\n{}\n{}\n", leaf_line("jest", "/workspace", &["--ci"]), leaf_line("mocha", "/workspace", &[]), ended(1))).unwrap();
    let ran = Ran { steps: Vec::new(), leaves: vec![("test-5".into(), Ok(five.clone())), ("test-7".into(), Ok(seven.clone()))] };
    let all = exec::recorded(&ran).unwrap();
    assert_eq!(all.leaves.len(), 3);
    assert_eq!((all.exit, all.why), (Some(1), None), "the first test step that did not end with exit 0 gives the side's ending");
    let unreached = Ran { steps: Vec::new(), leaves: vec![("test-5".into(), Ok(five)), ("test-7".into(), Err(exec::NOT_REACHED.into()))] };
    let why = exec::recorded(&unreached).unwrap_err();
    assert!(why.starts_with("its test step `test-7` could not be recorded: it was not run"), "{why}");
    let mut many = String::new();
    for _ in 0..40 { many += &leaf_line("vitest", "/workspace", &[]); many += "\n"; }
    many += &ended(0);
    let forty = read(&many).unwrap();
    let over = Ran { steps: Vec::new(), leaves: vec![("a".into(), Ok(forty.clone())), ("b".into(), Ok(forty))] };
    assert!(exec::recorded(&over).unwrap_err().contains("more than 64 test runs"));
    let count = |passed: u32, failed: u32| kern::proof::Count { passed, failed, skipped: 0 };
    let reports = [kern::proof::Report { tests: [("packages/x/test/a.test.js > adds".to_string(), count(1, 0)), ("packages/x/test/a.test.js > fails".to_string(), count(0, 1))].into_iter().collect(), problems: Vec::new(), why: BTreeMap::new() }];
    let run = exec::run_of(&Ran { steps: vec![record("run-4", Kind::Run, Some(0), "")], leaves: Vec::new() }, &reports);
    assert_eq!(run.reports, 1);
    assert_eq!(run.outcomes.get("packages/x/test/a.test.js > adds"), Some(&Outcome::Passed));
    assert_eq!(run.outcomes.get("packages/x/test/a.test.js > fails"), Some(&Outcome::Failed));
}

#[test]
fn a_plan_whose_needs_or_size_the_box_cannot_give_is_said_up_front_never_run() {
    let mut v = plan_json();
    v["needs"] = json!([{ "service": "postgres:16" }, { "secret": "DATABASE_URL" }]);
    let plan = plan::parse(&v).unwrap();
    let why = exec::unsupported(&plan).unwrap();
    assert!(why.starts_with("not supported here: needs postgres:16, the secret DATABASE_URL"), "{why}");
    assert_eq!(exec::unsupported(&a_plan()), None);
    assert_eq!(exec::fits(&a_plan()), Ok(()));
    let mut wide = plan_json();
    wide["env"] = (0..33).map(|i| (format!("VAR_{i}"), json!("x"))).collect::<serde_json::Map<_, _>>().into();
    assert!(exec::fits(&plan::parse(&wide).unwrap()).unwrap_err().contains("test box's limits"), "a plan with more variables than the box allows is not run");
    let mut tall = plan_json();
    tall["steps"] = (0..33).map(|i| json!({ "id": format!("test-{i}"), "kind": "test", "dir": "", "line": "npm test" })).collect::<Vec<_>>().into();
    assert!(exec::fits(&plan::parse(&tall).unwrap()).unwrap_err().contains("at most 32"));
}

fn fresh(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kern-exec-{name}-{}", kern::util::uuid4().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn a_run(outcome: Outcome) -> Run {
    let mut outcomes = BTreeMap::new();
    outcomes.insert("test/a.test.js > adds".to_string(), Outcome::Passed);
    outcomes.insert("test/a.test.js > rounds".to_string(), outcome);
    Run { steps: vec![record("install-1", Kind::Install, Some(0), ""), record("test-5", Kind::Test, Some(0), "")], reports: 1, outcomes }
}

#[test]
fn an_accepted_plan_is_cached_with_the_two_runs_that_accepted_it_and_is_judged_again_when_read() {
    let dir = fresh("cache");
    let plan = a_plan();
    let runs = [a_run(Outcome::Failed), a_run(Outcome::Failed)];
    exec::store(&dir, &plan.key, Some(&plan), &Stored::Accepted(runs.clone())).unwrap();
    let file = dir.join(exec::plan_file(&plan.key));
    assert!(file.is_file());
    match exec::cached(&dir, &plan.key) { Some(Cached::Accepted(a)) => { assert_eq!(a.plan(), &plan); assert_eq!(a.tests_count(), 2); } _ => panic!("the stored plan was not accepted again") }
    assert!(exec::cached(&dir, &"c".repeat(64)).is_none(), "a plan for another setup key is not this one's");
    let mut v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(exec::run_parse(&v["accepted"]["runs"][0]).unwrap(), runs[0], "a run round-trips through its JSON");
    v["accepted"]["runs"][1]["outcomes"]["test/a.test.js > rounds"] = json!("passed");
    std::fs::write(&file, v.to_string()).unwrap();
    assert!(exec::cached(&dir, &plan.key).is_none(), "a cache whose two runs disagree is not an accepted plan, whatever the file says");
    exec::store(&dir, &plan.key, Some(&plan), &Stored::Rejected("run 1: no test passed".into())).unwrap();
    assert!(exec::cached(&dir, &plan.key).is_none(), "a rejected plan takes today's path");
    exec::store(&dir, &plan.key, None, &Stored::Unsupported("not supported here: needs postgres:16".into())).unwrap();
    match exec::cached(&dir, &plan.key) { Some(Cached::Unsupported(why)) => assert_eq!(why, "not supported here: needs postgres:16"), _ => panic!("an unsupported plan is said up front") }
    let mut other = plan_json();
    other["key"] = json!("d".repeat(64));
    std::fs::write(&file, json!({ "version": 1, "key": plan.key, "plan": other, "accepted": { "runs": v["accepted"]["runs"] } }).to_string()).unwrap();
    assert!(exec::cached(&dir, &plan.key).is_none(), "a plan whose own key differs from the file's is refused");
    std::fs::write(&file, "{".repeat(10)).unwrap();
    assert!(exec::cached(&dir, &plan.key).is_none());
    std::fs::write(&file, vec![b' '; exec::PLAN_FILE_BYTES_MAX + 1]).unwrap();
    assert!(exec::cached(&dir, &plan.key).is_none(), "a file over the cap is not read");
    assert!(exec::run_parse(&json!({ "steps": [], "reports": 1 })).is_err());
    assert!(exec::run_parse(&json!({ "steps": [{ "id": "a", "kind": "fly", "exit": 0, "error": "" }], "reports": 1, "outcomes": {} })).is_err());
    assert!(exec::run_parse(&json!({ "steps": [], "reports": 1, "outcomes": { "t": "maybe" } })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn committed(r: &Path, files: &[(&str, &str)]) -> String {
    for (p, text) in files {
        let path = r.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "--allow-empty", "-m", "m"]);
    git(r, &["rev-parse", "HEAD"])
}
const CI: &str = "name: ci\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n      - run: npm ci\n      - run: npm test\n";
const PACKAGE: &str = r#"{"name":"calc","version":"1.0.0","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4"}}"#;

#[test]
fn the_setup_key_of_a_commit_is_read_through_git_from_its_workflow_and_setup_files() {
    let r = fresh("repo");
    git(&r, &["init", "-q"]);
    let commit = committed(&r, &[(".github/workflows/ci.yml", CI), (".github/workflows/notes.md", "not a workflow"), (".github/workflows/deep/x.yml", "nested: true"), ("package.json", PACKAGE), ("package-lock.json", "{}"), ("src/a.js", "1")]);
    let files = exec::workflows(&r, &commit).unwrap();
    assert_eq!(files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), [".github/workflows/ci.yml"], "only the workflow files themselves, not notes or nested files");
    let key = exec::setup_key_at(&r, &commit).unwrap();
    assert_eq!(key, plan::key(&[(".github/workflows/ci.yml", CI.as_bytes()), ("package.json", PACKAGE.as_bytes()), ("package-lock.json", b"{}")]), "the key is over the workflow files and the setup files that exist");
    let later = committed(&r, &[("src/a.js", "2")]);
    assert_eq!(exec::setup_key_at(&r, &later).unwrap(), key, "a code change leaves the setup key alone");
    let moved = committed(&r, &[("package-lock.json", "{\"lockfileVersion\":3}")]);
    assert_ne!(exec::setup_key_at(&r, &moved).unwrap(), key, "a lockfile change moves it");
    assert_eq!(exec::repo_name(&r), "unknown/unknown");
    git(&r, &["remote", "add", "origin", "https://github.com/acme/widgets.git"]);
    assert_eq!(exec::repo_name(&r), "acme/widgets");
    let _ = std::fs::remove_dir_all(&r);
}

fn operator() -> kern::policy::Operator {
    kern::policy::operator(&json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }],
        "registry": "registry.npmjs.org", "permissions": {} })).unwrap()
}

#[test]
fn learn_reads_the_commits_ci_job_and_says_up_front_what_the_box_cannot_give_before_installing_anything() {
    let r = fresh("learn");
    git(&r, &["init", "-q"]);
    let until = kern::util::now_ms() + 3_600_000;
    let bare = committed(&r, &[("package.json", PACKAGE), ("src/a.js", "1")]);
    let out = fresh("learn-out");
    let _ = std::fs::remove_dir(&out);
    let v = exec::learn(&r, &bare, &out, &operator(), until).unwrap();
    assert_eq!(v["status"], "UNREADABLE", "{v}");
    assert!(!out.join("setup.json").exists() && !out.join("setup-failed.txt").exists(), "nothing was installed for a commit whose CI Kern cannot read");
    let services = "name: ci\non: [pull_request]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    services:\n      db:\n        image: postgres:16\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n      - run: npm ci\n      - run: npm test\n";
    let needs = committed(&r, &[(".github/workflows/ci.yml", services)]);
    let out = fresh("learn-out");
    let _ = std::fs::remove_dir(&out);
    let v = exec::learn(&r, &needs, &out, &operator(), until).unwrap();
    assert_eq!(v["status"], "UNSUPPORTED", "{v}");
    assert!(v["why"].as_str().unwrap().starts_with("not supported here: needs postgres:16"), "{v}");
    let key = exec::setup_key_at(&r, &needs).unwrap();
    assert_eq!(v["key"], key);
    match exec::cached(&out, &key) { Some(Cached::Unsupported(why)) => assert!(why.contains("postgres:16")), _ => panic!("the unsupported plan is cached so a check says it up front") }
    assert!(!out.join("setup.json").exists() && !out.join("setup-failed.txt").exists(), "a plan the box cannot give is said before any install");
    let plain = committed(&r, &[(".github/workflows/ci.yml", CI)]);
    let out = fresh("learn-out");
    let _ = std::fs::remove_dir(&out);
    let e = exec::learn(&r, &plain, &out, &operator(), until).unwrap_err().0;
    assert!(out.join("setup-failed.txt").exists(), "a readable, supported plan goes on to install: here that fails honestly without a sandbox: {e}");
    assert!(!out.join(exec::plan_file(&exec::setup_key_at(&r, &plain).unwrap())).exists(), "no plan is cached when the runs could not be made");
    let _ = std::fs::remove_dir_all(&r);
}

#[test]
fn a_check_names_where_its_plan_came_from_and_a_plan_the_box_cannot_give_is_not_verified_up_front() {
    assert_eq!(kern::suite::plan_from(&a_plan()), "your CI job `.github/workflows/ci.yml#test`");
    let mut ai = plan_json();
    ai["source"] = json!({ "ai": { "model": "claude-x", "round": 1 } });
    assert_eq!(kern::suite::plan_from(&plan::parse(&ai).unwrap()), "the plan Kern's setup engineer wrote on main");
    assert_eq!(exec::source_json(&a_plan()), json!({ "kind": "ci", "file": ".github/workflows/ci.yml", "job": "test" }));
    let drive = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "NOT_RUN", "why": "not supported here: needs postgres:16 (the project's CI job runs its tests with it, and Kern's sandbox gives tests no services or secrets)" });
    let r = kern::receipt::build(&drive, &rules, &tests);
    assert_eq!(r["decision"], "unverified", "{r}");
    let said = r["unverified"].as_array().unwrap().iter().map(|x| x["what"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n");
    assert!(said.contains("not supported here: needs postgres:16"), "{said}");
    let ran = json!({ "status": "RAN", "runner": "vitest (`test`, from your CI job `.github/workflows/ci.yml#test`)", "before": { "passed": 12, "failed": 0, "skipped": 1 }, "after": { "passed": 12, "failed": 0, "skipped": 1 }, "kept": 12, "broke": [], "flaky": [], "gone": [],
        "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "plan": { "source": { "kind": "ci", "file": ".github/workflows/ci.yml", "job": "test" }, "from": "your CI job `.github/workflows/ci.yml#test`", "tests_count": 13 } });
    let md = kern::receipt::markdown(&kern::receipt::build(&drive, &rules, &ran));
    assert!(md.contains("Setup: Kern ran your CI job `.github/workflows/ci.yml#test` (13 tests)"), "{md}");
    let mut by_ai = ran.clone();
    by_ai["plan"] = json!({ "source": { "kind": "ai", "model": "claude-x", "round": 2 }, "from": "the plan Kern's setup engineer wrote on main", "tests_count": 13 });
    let md = kern::receipt::markdown(&kern::receipt::build(&drive, &rules, &by_ai));
    assert!(md.contains("Setup: Kern's setup engineer wrote this project's plan on main (accepted: reproduced the base's tests twice, 13 tests)"), "{md}");
    let mut plain = ran;
    plain.as_object_mut().unwrap().remove("plan");
    assert!(!kern::receipt::markdown(&kern::receipt::build(&drive, &rules, &plain)).contains("Setup:"), "a check on today's path says nothing about a plan");
}

fn tree_at<'a>(dir: &'a Path, read: &'a dyn Fn(&str) -> Option<Vec<u8>>) -> kern::ci::Tree<'a> {
    let mut workflows: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir.join(".github/workflows")).map(|d| d.flatten().map(|e| (format!(".github/workflows/{}", e.file_name().to_string_lossy()), std::fs::read(e.path()).unwrap())).collect()).unwrap_or_default();
    workflows.sort();
    kern::ci::Tree { repo: "acme/calc".into(), workflows, read }
}
fn written(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = fresh(name);
    for (p, text) in files {
        let path = dir.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}
const TIER_B: &str = "name: ci\non: [push, pull_request]\njobs:\n  unit:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n      - run: npm ci\n      - run: cd packages/x && npx vitest run\n";

#[test]
fn the_e2e_cases_ci_jobs_read_into_the_plans_the_cases_expect() {
    let a = written("tier-a", &[(".github/workflows/ci.yml", CI), ("package.json", PACKAGE), ("package-lock.json", "{}")]);
    let read_a = |p: &str| std::fs::read(a.join(p)).ok();
    let plan = plan::read(&tree_at(&a, &read_a)).unwrap();
    let steps: Vec<(Kind, &str, &str)> = plan.steps.iter().map(|s| (s.kind, s.dir.as_str(), s.line.as_str())).collect();
    assert_eq!(steps, [(Kind::Install, "", "npm ci"), (Kind::Test, "", "npm test")], "{plan:?}");
    assert_eq!((plan.toolchain.node, plan.toolchain.manager.name.as_str(), exec::unsupported(&plan)), (22, "npm", None));
    assert!(matches!(&plan.source, plan::Source::Ci { file, job } if file == ".github/workflows/ci.yml" && job == "test"));
    let root = r#"{"name":"planb","private":true,"type":"module","workspaces":["packages/*"],"devDependencies":{"vitest":"3.2.4"}}"#;
    let b = written("tier-b", &[(".github/workflows/ci.yml", TIER_B), ("package.json", root), ("package-lock.json", "{}"), ("packages/x/package.json", r#"{"name":"@planb/x","version":"1.0.0","private":true,"type":"module"}"#)]);
    let read_b = |p: &str| std::fs::read(b.join(p)).ok();
    let plan = plan::read(&tree_at(&b, &read_b)).unwrap();
    let steps: Vec<(Kind, &str, &str)> = plan.steps.iter().map(|s| (s.kind, s.dir.as_str(), s.line.as_str())).collect();
    assert_eq!(steps, [(Kind::Install, "", "npm ci"), (Kind::Test, "packages/x", "npx vitest run")], "the cd becomes the step's folder: {plan:?}");
    assert!(matches!(&plan.source, plan::Source::Ci { job, .. } if job == "unit"));
    assert_eq!(exec::listed(&plan), vec![json!({ "id": "test-2", "kind": "test", "dir": "packages/x", "line": "npx vitest run" })]);
    for d in [a, b] { let _ = std::fs::remove_dir_all(d); }
}
