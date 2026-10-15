// kern check: one receipt for a pull request. These tests cover the parts that run without a sandbox: the rules a test run's sandbox
// must follow, how the old and the new code's test results are compared, the static rules, the receipt and the keyless proof.
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
fn a_test_run_reads_its_tree_read_only_without_network() {
    let t = Temp::new();
    let l = limits();
    let run = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &l, entry: vec!["__drive_test".into()], network: "none", volumes: vec![], root: false,
        home_mb: 0, scratch: false, workspace: Some(("kern-deps-tree", true)) };
    let a = sandbox::container_args(&run, "kern-x").unwrap().join(" ");
    for want in ["type=volume,source=kern-deps-tree,target=/workspace,readonly", "--network=none", "--user=1000:1000", "--runtime=runsc", "--read-only", "nofile=65536"] {
        assert!(a.contains(want), "{want}: {a}");
    }
    // Tools keep their caches in memory, as in a drive run: the installed tree itself stays read-only, so no test can change what later runs see.
    for dir in [".cache", ".vite", ".vite-temp"] { assert!(a.contains(&format!("--tmpfs /workspace/node_modules/{dir}:rw")), "{dir}: {a}"); }
    assert!(a.matches("type=bind").count() == 2, "only /input and /kern are bound: {a}");
    // A test run never gets a network, root, write grants, a writable tree, a scratch copy or another volume.
    let write = vec!["src/".to_string()];
    let tools = vec![("kern-deps-tools", "/kern-tools/node_modules", true)];
    denied(sandbox::container_args(&Spec { network: "bridge", ..run.clone() }, "kern-x"), "network");
    for bad in [Spec { root: true, ..run.clone() }, Spec { writable: &write, ..run.clone() }, Spec { workspace: Some(("kern-deps-tree", false)), ..run.clone() },
        Spec { workspace: None, ..run.clone() }, Spec { scratch: true, ..run.clone() }, Spec { volumes: tools.clone(), ..run.clone() }] {
        denied(sandbox::container_args(&bad, "kern-x"), "test run");
    }
}

#[test]
fn a_test_run_report_is_read_by_length() {
    let mut out = Vec::new();
    for (name, body) in [("output", &b"{\"stats\":{}}\n"[..]), ("errors", b""), ("behavior", b"exec:id\n"), ("ending", b"exit 1")] {
        out.extend(format!("{}{name} {}\n", sandbox::DRIVE_MARK, body.len()).as_bytes());
        out.extend(body);
    }
    let parts = sandbox::parts(&out, &sandbox::TEST_PARTS).unwrap();
    assert_eq!(parts["ending"], b"exit 1");
    assert_eq!(parts["behavior"], b"exec:id\n");
    // Output that pretends to end the report early, or extra bytes after it, is refused.
    let mut forged = out.clone();
    forged.extend(b"more");
    denied(sandbox::parts(&forged, &sandbox::TEST_PARTS), "unexpected data");
    denied(sandbox::parts(&out[..out.len() - 2], &sandbox::TEST_PARTS), "incomplete");
}

// ---- Tests on the old and the new code -------------------------------------------------------------------------------------------
use kern::proof::{Count, Report};
use kern::suite;
use std::collections::BTreeMap;

fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect() }
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
fn a_test_that_fails_once_is_flaky_not_broken() {
    let before = report(&[("t > a", 1, 0, 0), ("t > b", 1, 0, 0)], &[]);
    let after = report(&[("t > a", 0, 1, 0), ("t > b", 1, 0, 0)], &[]);
    let again = report(&[("t > a", 1, 0, 0), ("t > b", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&again));
    assert!(names(&c, "broke").is_empty(), "{c}");
    assert_eq!(names(&c, "flaky"), ["t > a"]);
    assert_eq!(c["kept"], 2, "both pass on the second run");
    // With no second run, a failure is a failure.
    assert_eq!(names(&suite::compare(&before, &after, None), "broke"), ["t > a"]);
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
