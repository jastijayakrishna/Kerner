use kern::receipt;
use serde_json::{Value, json};

fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn clean_rules() -> Value { json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] }) }
fn ran() -> Value {
    json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 12, "failed": 0, "skipped": 0 }, "after": { "passed": 12, "failed": 0, "skipped": 0 }, "kept": 12, "broke": [], "flaky": [], "gone": [],
        "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] })
}
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }
fn kinds(r: &Value, list: &str) -> Vec<String> { r[list].as_array().into_iter().flatten().map(|x| x["kind"].as_str().unwrap_or("").to_string()).collect() }
fn said(r: &Value, list: &str) -> String { r[list].as_array().into_iter().flatten().map(|x| x["what"].as_str().unwrap_or("")).collect::<Vec<_>>().join("\n") }
fn with(mut tests: Value, key: &str, value: Value) -> Value { tests[key] = value; tests }

#[test]
fn docusaurus_12655_tests_kern_could_not_run_are_not_verified_never_a_look_and_never_clear() {
    let tests = json!({ "status": "NOT_RUN", "why": "the test runner ran but found no tests on either side" });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "unverified", "Kern found nothing to look at but could not run the tests: {r}");
    assert!(kinds(&r, "look").is_empty(), "a check Kern could not run is not a finding: {}", r["look"]);
    assert_eq!(kinds(&r, "unverified"), ["tests-not-run"], "{}", r["unverified"]);
    assert!(said(&r, "unverified").contains("found no tests on either side"), "{}", r["unverified"]);
    let md = receipt::markdown(&r, None);
    assert!(md.contains("### ⏭️ Kern: Not verified") && !md.contains("✅ Kern") && !md.contains("### ⚠️") && md.contains("/kern approve"), "{md}");
    let mut rules = clean_rules();
    rules["protected"] = json!([{ "path": ".github/workflows/ci.yml", "operation": "modify" }]);
    let r = receipt::build(&drive_json(), &rules, &tests, &off());
    assert_eq!(r["decision"], "look", "the static checks still apply when the tests cannot run: {r}");
    assert_eq!((kinds(&r, "look"), kinds(&r, "unverified")), (vec!["protected".to_string()], vec!["tests-not-run".to_string()]), "{r}");
}

#[test]
fn storybook_36653_kerns_own_environment_problem_is_not_verified_and_says_so() {
    let tests = json!({ "status": "NOT_RUN", "why": "Kern could not run the tests: Sandbox exceeded limits: it ran out of memory (it allows 5995 MiB)" });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "unverified", "{r}");
    assert!(said(&r, "unverified").contains("Kern's environment had a problem"), "{}", r["unverified"]);
    let none = json!({ "status": "NOT_RUN", "why": "the project has no test script, so no test checks this change", "no_tests": true });
    assert_eq!(receipt::build(&drive_json(), &clean_rules(), &none, &off())["decision"], "clear", "a project with no tests keeps its rules-only receipt");
}

#[test]
fn mastra_15774_a_change_no_test_reaches_is_not_verified_and_said_plainly() {
    let file = "packages/core/src/agent/message-list/merge/MessageMerger.ts";
    let fault = |line: u64, outcome: &str| json!({ "path": file, "line": line, "what": "a condition turned around", "outcome": outcome });
    let tests = with(ran(), "faults", json!([fault(22, "unreached"), fault(31, "untested")]));
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "unverified", "no test reached the change: a lack of evidence, not a finding: {r}");
    assert_eq!(kinds(&r, "unverified"), ["untested-change"], "{r}");
    assert!(said(&r, "unverified").contains(file), "{}", r["unverified"]);
    let md = receipt::markdown(&r, None);
    assert!(!md.contains("✅ Tests") && md.contains("⚠️ Tests"), "{md}");
}

#[test]
fn finance_tracker_36_crashed_workers_and_old_tests_kern_could_not_run_are_not_verified() {
    let mut tests = ran();
    tests["sandbox"] = json!(["apps/api/src/realtime/realtime.gateway.spec.ts"]);
    tests["edited_unchecked"] = json!([{ "path": "apps/api/src/users/users.service.spec.ts", "why": "A jest worker process (pid=50) was terminated by another process: signal=SIGSEGV" }]);
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "unverified", "{r}");
    let mut found = kinds(&r, "unverified");
    found.sort();
    assert_eq!(found, ["edited-unchecked", "test-sandbox"], "{r}");
    let md = receipt::markdown(&r, None);
    assert!(!md.contains("✅ Tests"), "{md}");
}

#[test]
fn a_new_package_that_fails_to_install_is_a_look_and_its_failed_install_is_not_verified_once() {
    let why = "the tests could not run on the new code, where installing its dependencies with npm failed: npm error code E404 (it worked on the old code)";
    let tests = json!({ "status": "NOT_RUN", "why": why, "run_unconfirmed": [{ "dir": "", "why": why }] });
    let mut rules = clean_rules();
    rules["dependencies"] = json!([{ "path": "package.json", "operation": "modify", "packages": ["@kern-e2e/does-not-exist 1.0.0 (new)"], "moves": [{ "name": "@kern-e2e/does-not-exist", "to": "1.0.0", "age": "unknown" }] }]);
    let r = receipt::build(&drive_json(), &rules, &tests, &off());
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["dependencies".to_string()]), "the new package is a finding: {r}");
    assert_eq!(kinds(&r, "unverified"), ["test-unconfirmed"], "one failed install is one item, not two: {}", r["unverified"]);
}

#[test]
fn failures_kern_could_not_prove_or_rule_out_are_not_verified() {
    for (key, value, kind) in [
        ("together", json!(["apps/condo/domains/news/tasks/notifyResidentsAboutNewsItem.helpers.spec.js"]), "test-together"),
        ("unproven", json!(["src/a.test.ts"]), "test-unproven"),
        ("uneven", json!(["src/a.test.ts > adds"]), "test-uneven"),
        ("unconfirmed", json!(["src/a.test.ts > adds"]), "test-unconfirmed"),
        ("run_unconfirmed", json!([{ "dir": "", "why": "the tests finished on the old code but not on the new code: the run was stopped" }]), "test-unconfirmed"),
    ] {
        let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), key, value), &off());
        assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec![kind.to_string()]), "condo#6842 and its class: {key}: {r}");
        assert!(kinds(&r, "look").is_empty(), "{key}: {r}");
    }
}

#[test]
fn a_failure_in_a_file_that_passed_and_failed_by_chance_is_not_verified_never_clear() {
    let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), "unsteady", json!(["src/a.test.ts > retries"])), &off());
    assert_eq!(r["decision"], "unverified", "unsteady also holds tests that failed in every new run of a file whose other tests came and went, so it never turns green: {r}");
    assert_eq!(kinds(&r, "unverified"), ["test-unsteady"], "{r}");
}

fn rewritten(polarity: Option<Value>) -> Value {
    let key = "packages/lib/ssrfProtection.test.ts > validateUrlForSSRFSync > blocks https://127.0.0.1/logo.png";
    let mut tests = with(ran(), "rewritten", json!([key]));
    tests["gone"] = json!([key]);
    if let Some(p) = polarity { tests["polarity"] = json!({ key: p }); }
    tests
}

#[test]
fn calcom_28622_an_old_test_whose_expected_value_moved_is_a_note_with_both_values() {
    let changed = json!({ "kind": "changed", "was": "\"error\": \"Private IP address\",", "now": "\"error\": \"Blocked hostname\"," });
    let r = receipt::build(&drive_json(), &clean_rules(), &rewritten(Some(changed)), &off());
    assert_eq!(r["decision"], "clear", "the old test still refuses the address, only the reason moved: {r}");
    assert_eq!(kinds(&r, "notes"), ["test-value"], "{r}");
    let note = said(&r, "notes");
    assert!(note.contains("Private IP address") && note.contains("Blocked hostname") && note.contains("blocks https://127.0.0.1/logo.png"), "{note}");
    assert!(kinds(&r, "look").is_empty(), "a renamed test whose old version fails is the same event, not a second look: {}", r["look"]);
    let md = receipt::markdown(&r, None);
    assert!(md.contains("Private IP address") && md.contains("Blocked hostname"), "{md}");
}

#[test]
fn vuejs_core_15867_an_old_test_that_expected_a_refusal_the_new_code_accepts_is_a_look() {
    for kind in ["opened", "flipped"] {
        let reading = json!({ "kind": kind, "was": "throws", "now": "does not throw" });
        let r = receipt::build(&drive_json(), &clean_rules(), &rewritten(Some(reading)), &off());
        assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["test-rewritten".to_string()]), "{kind}: {r}");
        assert!(kinds(&r, "notes").is_empty(), "{r}");
    }
    let r = receipt::build(&drive_json(), &clean_rules(), &rewritten(Some(json!({ "kind": "opened", "was": "throws", "now": "does not throw" }))), &off());
    assert!(said(&r, "look").contains("accepting what the old code refused"), "{}", r["look"]);
}

#[test]
fn budibase_18627_an_old_test_that_cannot_drive_the_new_code_is_not_verified() {
    let reading = json!({ "kind": "harness", "was": "", "now": "", "why": "No \"automationStore\" export is defined on the \"@/stores/builder\" mock" });
    let r = receipt::build(&drive_json(), &clean_rules(), &rewritten(Some(reading)), &off());
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["edited-unchecked".to_string()]), "{r}");
    assert!(said(&r, "unverified").contains("automationStore"), "{}", r["unverified"]);
}

#[test]
fn budibase_18627_thirty_two_old_tests_of_one_file_that_cannot_drive_the_new_code_are_one_item() {
    let file = "packages/builder/src/components/integration/APIEndpointViewer.spec.ts";
    let keys: Vec<String> = (0..32).map(|i| format!("{file} > API Endpoint Viewer > case {i}")).collect();
    let why = "Error: [vitest] No \"automationStore\" export is defined on the \"@/stores/builder\" mock.";
    let mut tests = with(ran(), "rewritten", json!(keys));
    tests["polarity"] = keys.iter().map(|k| (k.clone(), json!({ "kind": "harness", "was": "", "now": "", "why": why }))).collect::<serde_json::Map<String, Value>>().into();
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified").len()), (Some("unverified"), 1), "one file, one item: {}", r["unverified"]);
    let item = said(&r, "unverified");
    assert!(item.contains("32 edited tests") && item.contains(file) && item.matches("automationStore").count() == 1, "{item}");
    let md = receipt::markdown(&r, None);
    assert!(md.contains("Kern could not run the old versions of 32 tests against the new code") && !md.contains("fail on the new code (see above)"), "{md}");
}

#[test]
fn an_old_test_failure_kern_cannot_read_stays_a_look() {
    for reading in [None, Some(json!({ "kind": "unknown", "was": "", "now": "" }))] {
        let r = receipt::build(&drive_json(), &clean_rules(), &rewritten(reading.clone()), &off());
        assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["test-rewritten".to_string()]), "{reading:?}: {r}");
    }
}

#[test]
fn a_pull_request_that_says_it_changes_no_behaviour_but_moves_what_a_test_expects_gets_a_look() {
    let mut rules = clean_rules();
    rules["says_no_change"] = json!("refactor: tidy the SSRF guard");
    let changed = json!({ "kind": "changed", "was": "\"error\": \"Private IP address\",", "now": "\"error\": \"Blocked hostname\"," });
    let r = receipt::build(&drive_json(), &rules, &rewritten(Some(changed)), &off());
    assert_eq!(r["decision"], "look", "the pull request's own words can add a look, never remove one: {r}");
    assert!(kinds(&r, "look").contains(&"says-no-change".to_string()) && said(&r, "look").contains("refactor: tidy the SSRF guard"), "{}", r["look"]);
    assert_eq!(receipt::build(&drive_json(), &rules, &ran(), &off())["decision"], "clear", "the words alone are no finding");
}

#[test]
fn a_pull_request_says_it_changes_no_behaviour_in_its_title_type_or_in_plain_words() {
    let says = |pr: &str| kern::check::says_no_change(pr);
    assert_eq!(says("# refactor(ssrf): tidy the guard\n\nMoves the list into one constant."), Some("refactor(ssrf): tidy the guard".to_string()));
    assert_eq!(says("# perf: parse dates once\n\n"), Some("perf: parse dates once".to_string()));
    assert!(says("# Fix login redirect\n\nThis has no behavior change for users who are signed in.").is_some_and(|s| s.contains("no behavior change")));
    assert!(says("# Rename helpers\n\nA pure refactor, behaviour-preserving.").is_some());
    for none in ["# Fix login\n\nUsers who sign in land on the dashboard.", "# Add OAuth\n\nThis is not a pure refactor: it adds a provider.", "# feat: refactor-proof parser", ""] {
        assert_eq!(says(none), None, "{none}");
    }
}

#[test]
fn google_cloud_mcp_61_a_test_that_only_reaches_the_network_stays_quiet() {
    let mut rules = clean_rules();
    rules["powers"] = json!([{ "path": "test/unit/services/trace/resources.test.ts", "added": ["global:fetch"], "has": ["global:fetch"] }]);
    let r = receipt::build(&drive_json(), &rules, &ran(), &off());
    assert_eq!(r["decision"], "clear", "a test that calls fetch cannot leak a secret it never reads: {r}");
    for (path, added, has) in [
        ("test/a.test.ts", json!(["env:OPENAI_API_KEY"]), json!(["env:OPENAI_API_KEY", "global:fetch"])),
        ("test/a.test.ts", json!(["global:fetch"]), json!(["env:*", "global:fetch"])),
        ("__test__/mirror-ref-copy-git.test.ts", json!(["node:child_process"]), json!(["node:child_process"])),
        ("tests/qr/showcase.spec.ts", json!(["code:eval"]), json!(["code:eval"])),
        ("src/check-registry.js", json!(["global:fetch"]), json!(["global:fetch"])),
    ] {
        rules["powers"] = json!([{ "path": path, "added": added, "has": has }]);
        let r = receipt::build(&drive_json(), &rules, &ran(), &off());
        assert_eq!(kinds(&r, "look"), ["power"], "{path} {added}: tests run in CI next to its secrets; one that can read a secret and send it out, or run any program, is a look; shipped code always is: {r}");
    }
}

#[test]
fn files_tests_build_into_output_folders_are_not_code_the_change_wrote() {
    let built = json!(["packages/gateway/dist/ui/assets/CompatSmoke-y9ZBvA-6.js", "test/cases/x/.nuxt/test/y7mwvi/app.config.mjs"]);
    let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), "wrote_code", built), &off());
    assert_eq!(r["decision"], "clear", "BYK/loreai#1941 and nuxt-modules/better-auth#588 build into dist/ and .nuxt/: {r}");
    let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), "wrote_code", json!(["src/payload.js"])), &off());
    assert_eq!(kinds(&r, "look"), ["wrote-code"], "{r}");
}

#[test]
fn visgl_loaders_gl_4253_a_readme_in_a_test_data_folder_is_not_test_data() {
    let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), "test_data", json!(["modules/wms/test/data/README.md"])), &off());
    assert_eq!(r["decision"], "clear", "{r}");
    let r = receipt::build(&drive_json(), &clean_rules(), &with(ran(), "test_data", json!(["packages/plugin/test/mock-browser-client.ts"])), &off());
    assert_eq!(kinds(&r, "look"), ["test-data"], "{r}");
}

#[test]
fn new_type_errors_kern_could_not_compare_fairly_are_not_verified() {
    let project = |extra: Value| { let mut p = json!({ "config": "tsconfig.json", "result": "new-errors", "count": 1, "added": [{ "file": "src/a.ts", "line": 3, "code": "TS2305", "message": "x" }] }); for (k, v) in extra.as_object().unwrap() { p[k] = v.clone(); } p };
    for (extra, list) in [(json!({ "enforced": true, "generated": true }), "unverified"), (json!({ "enforced": true, "uneven": "the two sides were set up differently" }), "unverified"), (json!({ "enforced": false }), "look")] {
        let tests = with(ran(), "types", json!({ "status": "RAN", "projects": [project(extra.clone())] }));
        let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
        assert_eq!(kinds(&r, list), ["types-new"], "{extra}: {r}");
    }
}

#[test]
fn changed_code_kern_could_not_read_is_not_verified() {
    let mut rules = clean_rules();
    rules["not_checked"] = json!([{ "path": "src/components/TokenMetrics.tsx", "why": "Changed code cannot be parsed for capability analysis" }]);
    let r = receipt::build(&drive_json(), &rules, &ran(), &off());
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-code".to_string()]), "numbpill3d/cyber-prompt-builder#42: {r}");
}

#[test]
fn calcom_28622_kerns_vitest_reporter_keeps_the_full_values_vitest_cuts_short() {
    let dir = std::env::temp_dir().join(format!("kern-values-{}", kern::util::uuid4().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    let reporter = dir.join("vitest-progress.mjs");
    std::fs::write(&reporter, kern::sandbox::VITEST_PROGRESS).unwrap();
    let values = dir.join("values");
    let script = r#"
const { pathToFileURL } = await import('node:url');
const { default: Reporter } = await import(pathToFileURL(process.env.REPORTER).href);
const file = { filepath: '/workspace/packages/lib/ssrfProtection.kern-kept.test.ts' };
const suite = { name: 'validateUrlForSSRFSync', type: 'suite' };
const task = { id: 't1', type: 'test', name: 'blocks https://127.0.0.1/logo.png', suite, file };
const r = new Reporter();
r.onInit({ state: { idMap: new Map([['t1', task]]) } });
const error = { message: 'expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }',
  expected: 'Object {\n  "error": "Private IP address",\n  "isValid": false,\n}', actual: 'Object {\n  "error": "Blocked hostname",\n  "isValid": false,\n}' };
r.onTaskUpdate([['t1', { state: 'fail', errors: [error] }]]);
r.onTaskUpdate([['t1', { state: 'fail', errors: [{ message: 'a second update of the same test' }] }]]);
"#;
    let ran = std::process::Command::new("node").args(["--input-type=module", "-e", script]).env("REPORTER", &reporter).env("KERN_VALUES", &values).output();
    let Ok(ran) = ran else { eprintln!("node is not installed here; the reporter is checked where it is"); return };
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let lines = std::fs::read(&values).unwrap();
    assert_eq!(String::from_utf8_lossy(&lines).lines().count(), 1, "one line per failed test: {}", String::from_utf8_lossy(&lines));
    let report = json!({ "numTotalTests": 1, "testResults": [{ "name": "/workspace/packages/lib/ssrfProtection.kern-kept.test.ts", "assertionResults": [
        { "ancestorTitles": ["validateUrlForSSRFSync"], "title": "blocks https://127.0.0.1/logo.png", "status": "failed", "failureMessages": [error_text()] }] }] }).to_string();
    let found = kern::proof::failures(kern::policy::Runner::Vitest, report.as_bytes(), &lines);
    let f = &found["packages/lib/ssrfProtection.kern-kept.test.ts > validateUrlForSSRFSync > blocks https://127.0.0.1/logo.png"];
    assert!(f.message.contains("…(1)") && f.expected.as_deref().unwrap().contains("Private IP address") && f.actual.as_deref().unwrap().contains("Blocked hostname"), "{f:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
fn error_text() -> &'static str { "AssertionError: expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }\n    at /workspace/packages/lib/ssrfProtection.kern-kept.test.ts:113:20" }

#[test]
fn a_test_with_two_different_value_records_has_no_values_so_the_code_under_test_cannot_turn_a_look_into_a_note() {
    let key = "test/user.kern-kept.test.ts > finds the user";
    let report = json!({ "numTotalTests": 1, "testResults": [{ "name": "/workspace/test/user.kern-kept.test.ts", "assertionResults": [
        { "ancestorTitles": [], "title": "finds the user", "status": "failed", "failureMessages": ["AssertionError: expected null to be { id: 1 }"] }] }] }).to_string();
    let line = |expected: &str, actual: &str| json!({ "file": "/workspace/test/user.kern-kept.test.ts", "names": [], "name": "finds the user", "message": "expected null to be { id: 1 }", "expected": expected, "actual": actual }).to_string();
    let honest = kern::proof::failures(kern::policy::Runner::Vitest, report.as_bytes(), format!("{}\n", line("{ id: 1 }", "null")).as_bytes());
    assert_eq!(honest[key].expected.as_deref(), Some("{ id: 1 }"));
    let forged = format!("{}\n{}\n", line("{ id: 1 }", "null"), line("\"a\"", "\"b\""));
    let found = kern::proof::failures(kern::policy::Runner::Vitest, report.as_bytes(), forged.as_bytes());
    assert_eq!((found[key].expected.as_deref(), found[key].actual.as_deref()), (None, None),
        "the values file sits where the new code under test can write too; two records that disagree prove someone wrote one, so Kern reads the failure from its message alone");
    assert_eq!(kern::polarity::read(&found[key].message, found[key].expected.as_deref(), found[key].actual.as_deref()).kind, kern::polarity::Kind::Flipped, "null against an object stays a look");
}

#[test]
fn adobe_da_admin_271_mocha_hands_over_what_the_old_test_expected_and_what_the_new_code_gave() {
    let report = json!({ "stats": {}, "tests": [{ "fullTitle": "Version Put returns 500", "file": "/workspace/test/storage/version/put.kern-kept.test.js" }], "passes": [], "pending": [],
        "failures": [{ "fullTitle": "Version Put returns 500", "file": "/workspace/test/storage/version/put.kern-kept.test.js", "err": { "message": "must return 500", "expected": 500, "actual": 201, "operator": "strictEqual" } }] }).to_string();
    let found = kern::proof::failures(kern::policy::Runner::Mocha, report.as_bytes(), b"");
    let f = &found["test/storage/version/put.kern-kept.test.js > Version Put returns 500"];
    assert_eq!((f.message.as_str(), f.expected.as_deref(), f.actual.as_deref()), ("must return 500", Some("500"), Some("201")), "{f:?}");
}

#[test]
fn every_answer_ranks_blocked_then_look_then_not_verified_then_clear() {
    let mut tests = with(ran(), "broke", json!(["src/a.test.ts > adds"]));
    tests["together"] = json!(["src/b.test.ts"]);
    let mut rules = clean_rules();
    rules["protected"] = json!([{ "path": ".github/workflows/ci.yml", "operation": "modify" }]);
    assert_eq!(receipt::build(&drive_json(), &rules, &tests, &off())["decision"], "blocked");
    tests["broke"] = json!([]);
    assert_eq!(receipt::build(&drive_json(), &rules, &tests, &off())["decision"], "look");
    assert_eq!(receipt::build(&drive_json(), &clean_rules(), &tests, &off())["decision"], "unverified");
    assert_eq!(receipt::build(&drive_json(), &clean_rules(), &ran(), &off())["decision"], "clear");
}

#[test]
fn the_signing_job_rebuilds_the_same_answer_from_the_facts() {
    let tests = json!({ "status": "NOT_RUN", "why": "the test runner ran but found no tests on either side" });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    let mut stored = r.clone();
    stored["decision"] = json!("clear");
    stored["unverified"] = json!([]);
    let again = receipt::rebuild(&stored).unwrap();
    assert_eq!((again["decision"].as_str(), kinds(&again, "unverified")), (Some("unverified"), vec!["tests-not-run".to_string()]), "a run job cannot hand the signer a clear it did not earn: {again}");
}
