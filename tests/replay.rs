use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Temp(PathBuf);
impl Temp {
    fn new(name: &str) -> Temp {
        let p = std::env::temp_dir().join(format!("kern-replay-{name}-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn vitest(results: &[(&str, &str, &str)]) -> String {
    let mut files: std::collections::BTreeMap<&str, Vec<Value>> = Default::default();
    for (file, title, status) in results {
        files.entry(file).or_default().push(json!({ "ancestorTitles": [], "fullName": title, "status": status, "title": title, "failureMessages": if *status == "failed" { json!(["expected 90 to be 91"]) } else { json!([]) } }));
    }
    let test_results: Vec<Value> = files.into_iter().map(|(f, a)| json!({ "name": format!("/workspace/{f}"), "status": "passed", "message": "", "assertionResults": a })).collect();
    json!({ "numTotalTests": results.len(), "success": true, "testResults": test_results }).to_string()
}

fn stored(dir: &Path, runner: &str, tests: Value, runs: &[(&str, String)]) {
    std::fs::create_dir_all(dir).unwrap();
    let mut t = json!({ "status": "RAN", "runner": runner, "before": {}, "after": {}, "kept": 0, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "new_failing": [],
        "already_failing": [], "new_problems": [], "environmental": [], "same_failure": [], "unproven": [], "together": [], "unconfirmed": [], "uneven": [], "uneven_why": [], "not_run": [],
        "run_broke": [], "run_unconfirmed": [], "rewritten": [], "problem_why": {} });
    for (k, v) in tests.as_object().unwrap() { t[k] = v.clone(); }
    let receipt = json!({ "version": 1, "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "decision": "clear", "blocked": [], "look": [],
        "tests": t, "rules": { "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] },
        "behaviour": { "status": "OFF", "why": "no AI key" } });
    std::fs::write(dir.join("receipt.json"), receipt.to_string()).unwrap();
    for (role, text) in runs { std::fs::write(dir.join(format!("tests-{role}.out")), text).unwrap(); }
}

const ADDS: &str = "test/total.test.js";

#[test]
fn a_stored_check_is_judged_again_from_its_own_runs_with_todays_rules() {
    let t = Temp::new("break");
    let dir = t.0.join("receipt-acme__shop__1");
    stored(&dir, "vitest", json!({}), &[
        ("old", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "failed")])),
        ("old-alone", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["decision"], "blocked", "the stored receipt said clear, but its own runs show a test that passed before failing twice on the new code: {r}");
    assert_eq!(r["receipt"]["tests"]["broke"], json!([format!("{ADDS} > discounts")]));
    assert_eq!(r["replayed"], 1);
}

#[test]
fn a_failure_that_does_not_repeat_when_its_file_runs_alone_stays_flaky() {
    let t = Temp::new("flaky");
    let dir = t.0.join("receipt-acme__shop__2");
    stored(&dir, "vitest", json!({ "broke": [format!("{ADDS} > discounts")] }), &[
        ("old", vitest(&[(ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "discounts", "passed")])),
        ("new-again", vitest(&[(ADDS, "discounts", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["decision"], "clear", "{r}");
    assert_eq!(r["receipt"]["tests"]["flaky"], json!([format!("{ADDS} > discounts")]));
}

#[test]
fn each_package_suite_is_judged_from_the_runs_its_label_names() {
    let t = Temp::new("packages");
    let dir = t.0.join("receipt-acme__mono__3");
    stored(&dir, "vitest in packages/kit, jest in apps/web", json!({}), &[
        ("old-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "passed")])),
        ("new-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "failed")])),
        ("new-alone-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "failed")])),
        ("old-alone-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "passed")])),
        ("old-apps_web", vitest(&[("apps/web/test/a.test.js", "renders", "passed")])),
        ("new-apps_web", vitest(&[("apps/web/test/a.test.js", "renders", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["tests"]["broke"], json!(["packages/kit/test/pad.test.js > pads"]), "{r}");
    assert_eq!(r["replayed"], 2);
}

#[test]
fn the_old_run_kern_judged_is_its_last_one_after_a_build_step_or_a_narrowing() {
    let t = Temp::new("built");
    let dir = t.0.join("receipt-acme__shop__5");
    stored(&dir, "vitest, after Kern ran packages/shared build", json!({}), &[
        ("old", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-built", vitest(&[(ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-alone", vitest(&[(ADDS, "discounts", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["tests"]["broke"], json!([format!("{ADDS} > discounts")]),
        "Ghost#28552: the first old run came before Kern built a missing package; the run after the build is the one Kern compared: {r}");
    let narrowed = t.0.join("receipt-acme__shop__6");
    stored(&narrowed, "vitest", json!({}), &[
        ("old", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-built", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-related", vitest(&[(ADDS, "discounts", "passed")])),
        ("new-related", vitest(&[(ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-alone", vitest(&[(ADDS, "discounts", "passed")])),
    ]);
    assert_eq!(kern::replay::rejudge(&narrowed).unwrap()["receipt"]["tests"]["broke"], json!([format!("{ADDS} > discounts")]));
}

#[test]
fn a_check_with_a_suite_that_cannot_be_judged_again_is_not_judged_again_in_part() {
    let t = Temp::new("part");
    let dir = t.0.join("receipt-acme__mono__7");
    stored(&dir, "vitest in packages/kit", json!({ "broke": ["apps/web/test/a.test.js > renders"], "kept": 41 }), &[
        ("old-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "passed")])),
        ("new-packages_kit", vitest(&[("packages/kit/test/pad.test.js", "pads", "passed")])),
        ("old-apps_web", "not a report".to_string()),
        ("new-apps_web", vitest(&[("apps/web/test/a.test.js", "renders", "failed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["replayed"], 0, "apps/web cannot be judged again, so the check keeps every stored list rather than mixing old and new verdicts: {r}");
    assert_eq!(r["receipt"]["tests"]["broke"], json!(["apps/web/test/a.test.js > renders"]));
    assert_eq!(r["receipt"]["tests"]["kept"], 41);
}

#[test]
fn a_check_that_ran_the_old_versions_of_edited_tests_keeps_its_stored_gone_list() {
    let t = Temp::new("kept");
    let dir = t.0.join("receipt-acme__shop__8");
    stored(&dir, "vitest", json!({ "gone": [], "rewritten": [format!("{ADDS} > adds")] }), &[
        ("old", vitest(&[(ADDS, "supports custom api", "passed"), (ADDS, "adds", "passed")])),
        ("new", vitest(&[(ADDS, "supports a custom api", "passed"), (ADDS, "adds", "passed")])),
        ("new-kept", vitest(&[(ADDS, "supports custom api", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["tests"]["gone"], json!([]), "next-sanity#3597: the renamed test's old version still passes on the new code, which only the kept runs show: {r}");
    assert_eq!(r["receipt"]["tests"]["rewritten"], json!([format!("{ADDS} > adds")]), "what the kept runs found stays as stored: {r}");
    assert_eq!(r["replayed"], 1);
}

#[test]
fn a_stored_setup_whose_sides_differ_is_judged_with_todays_setup_comparison() {
    let t = Temp::new("setup");
    let dir = t.0.join("receipt-acme__shop__9");
    stored(&dir, "vitest", json!({}), &[
        ("old", vitest(&[(ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-alone", vitest(&[(ADDS, "discounts", "passed")])),
    ]);
    let side = |scripts: bool| json!({ "installs": [{ "dir": ".", "manager": "npm", "result": "installed", "parts": [] }], "scripts": scripts, "node": { "major": 22 } });
    std::fs::write(dir.join("setup.json"), json!({ "version": 1, "old": side(true), "new": side(false) }).to_string()).unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!((r["receipt"]["tests"]["broke"].clone(), r["receipt"]["tests"]["uneven"].clone()), (json!([]), json!([format!("{ADDS} > discounts")])),
        "the new side lost its install scripts to a memory retry, so its failure cannot block: {r}");
}

#[test]
fn an_old_file_run_that_gave_nothing_falls_back_to_the_old_whole_rerun() {
    let t = Temp::new("old-again");
    let dir = t.0.join("receipt-acme__shop__10");
    stored(&dir, "vitest", json!({}), &[
        ("old", vitest(&[(ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "discounts", "failed")])),
        ("new-alone", vitest(&[(ADDS, "discounts", "failed")])),
        ("old-alone", vitest(&[])),
        ("old-again", vitest(&[(ADDS, "discounts", "passed")])),
    ]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["receipt"]["tests"]["broke"], json!([format!("{ADDS} > discounts")]), "the file alone on the old code ran no test, so the old whole suite ran again and passed it: {r}");
}

#[test]
fn a_result_whose_runs_were_not_kept_keeps_its_stored_verdict() {
    let t = Temp::new("bare");
    let dir = t.0.join("receipt-acme__shop__4");
    stored(&dir, "vitest", json!({ "broke": [format!("{ADDS} > discounts")] }), &[]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!(r["replayed"], 0);
    assert_eq!(r["receipt"]["decision"], "blocked", "with no runs to judge again, the stored lists are rebuilt into a receipt as they are: {r}");
}

#[test]
fn orval_labs_orval_3772_stored_fault_runs_are_judged_again_with_todays_rules() {
    let t = Temp::new("faults");
    let dir = t.0.join("receipt-orval-labs__orval__3772");
    let faults = json!([{ "path": "src/getters.ts", "line": 12, "what": "a condition turned around", "outcome": "unknown" },
        { "path": "src/getters.ts", "line": 20, "what": "a call left out", "outcome": "unknown" }]);
    stored(&dir, "vitest", json!({ "kept": 2, "faults": faults }), &[
        ("old", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
    ]);
    let crashed = json!({ "finished": true, "served": false, "output": "", "errors": "Error: Failed to resolve entry for package \"@orval/core\"." });
    std::fs::write(dir.join("faults.json"), json!([crashed, crashed]).to_string()).unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    let outcomes: Vec<&str> = r["receipt"]["tests"]["faults"].as_array().unwrap().iter().map(|f| f["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["unrun", "unrun"], "the stored runs crashed before any related test ran: never read as no test reaching the file: {r}");
    assert_eq!(r["receipt"]["decision"], "unverified", "Kern cannot say a test reaches the change, which it says plainly: {}", r["receipt"]["unverified"]);
    assert!(r["receipt"]["unverified"].as_array().unwrap().iter().all(|x| x["kind"] == "unrun-change") && r["receipt"]["challenges"] == json!([]), "{}", r["receipt"]);
}

#[test]
fn stored_fault_runs_with_a_control_are_judged_against_it() {
    let t = Temp::new("placebo");
    let dir = t.0.join("receipt-pmndrs__jotai__3363");
    let faults = json!([{ "path": "src/vanilla/utils/unwrap.ts", "line": 40, "what": "undoes the change", "kind": "undo", "outcome": "unknown" },
        { "path": "src/vanilla/utils/unwrap.ts", "line": 52, "what": "changes > to >=", "kind": "operator", "outcome": "unknown" }]);
    stored(&dir, "vitest", json!({ "kept": 2, "faults": faults }), &[
        ("old", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
        ("new", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
    ]);
    let run = |results: &[(&str, &str, &str)]| json!({ "finished": true, "served": true, "output": vitest(results), "errors": "" });
    let placebo = run(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "failed")]);
    let runs = [run(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "failed")]), run(&[(ADDS, "adds", "failed"), (ADDS, "discounts", "passed")])];
    std::fs::write(dir.join("faults.json"), json!({ "placebo": placebo, "faults": runs }).to_string()).unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    let outcomes: Vec<&str> = r["receipt"]["tests"]["faults"].as_array().unwrap().iter().map(|f| f["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["missed", "caught"], "discounts also fails in the control run, so its failure under the undo is not the undo's doing: {r}");
}

#[test]
fn stored_traces_of_untouched_tests_are_compared_again() {
    let t = Temp::new("traces");
    let dir = t.0.join("receipt-lightdash__lightdash__12942");
    stored(&dir, "vitest", json!({ "kept": 2, "bystanders": { "status": "RAN", "compared": 0, "unstable": 0, "changed": 0, "tests": 4, "differences": [] } }), &[
        ("old", vitest(&[(ADDS, "adds", "passed")])),
        ("new", vitest(&[(ADDS, "adds", "passed")])),
    ]);
    let line = |o: &str| format!("{}\n", json!({ "k": "packages/backend/src/queryBuilder.ts#stripComments", "a": "[\"SELECT 'http://a'\"]", "o": o }));
    for (side, o) in [("old", "returns \"SELECT 'http://a'\""), ("old-again", "returns \"SELECT 'http://a'\""), ("new", "returns \"SELECT 'http:\"")] {
        std::fs::write(dir.join(format!("trace-{side}.jsonl")), line(o)).unwrap();
    }
    let r = kern::replay::rejudge(&dir).unwrap();
    let seen = &r["receipt"]["tests"]["bystanders"];
    assert_eq!((seen["compared"].as_u64(), seen["changed"].as_u64(), seen["tests"].as_u64()), (Some(1), Some(1), Some(4)), "{seen}");
    assert_eq!(r["receipt"]["notes"][0]["kind"], "bystander", "{}", r["receipt"]);
}

#[test]
fn calcom_28622_a_stored_old_test_failure_is_read_again_with_its_values() {
    let t = Temp::new("polarity");
    let dir = t.0.join("receipt-calcom__cal.diy__28622");
    let key = "packages/lib/ssrfProtection.test.ts > validateUrlForSSRFSync > blocks https://127.0.0.1/logo.png";
    let (kept, old) = ("packages/lib/ssrfProtection.kern-kept.test.ts", "packages/lib/ssrfProtection.test.ts");
    let failed = json!({ "numTotalTests": 1, "testResults": [{ "name": format!("/workspace/{kept}"), "assertionResults": [{ "ancestorTitles": ["validateUrlForSSRFSync"],
        "title": "blocks https://127.0.0.1/logo.png", "status": "failed", "failureMessages": ["AssertionError: expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }\n    at /workspace/x:1:1"] }] }] }).to_string();
    let passed = json!({ "numTotalTests": 1, "testResults": [{ "name": format!("/workspace/{old}"), "assertionResults": [{ "ancestorTitles": ["validateUrlForSSRFSync"],
        "title": "blocks https://127.0.0.1/logo.png", "status": "passed", "failureMessages": [] }] }] }).to_string();
    stored(&dir, "vitest", json!({ "rewritten": [key], "kept": 1 }), &[("old", passed.clone()), ("new", passed), ("new-kept", failed)]);
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!((r["receipt"]["decision"].as_str(), r["receipt"]["tests"]["polarity"][key]["kind"].as_str()), (Some("look"), Some("unknown")),
        "vitest cut both objects short, so the stored run alone cannot show what moved: {r}");
    let values = json!({ "file": format!("/workspace/{kept}"), "names": ["validateUrlForSSRFSync"], "name": "blocks https://127.0.0.1/logo.png", "message": "expected { isValid: false, …(1) } to deeply equal { isValid: false, …(1) }",
        "expected": "Object {\n  \"error\": \"Private IP address\",\n  \"isValid\": false,\n}", "actual": "Object {\n  \"error\": \"Blocked hostname\",\n  \"isValid\": false,\n}" });
    std::fs::write(dir.join("tests-new-kept.values"), format!("{values}\n")).unwrap();
    let r = kern::replay::rejudge(&dir).unwrap();
    assert_eq!((r["receipt"]["decision"].as_str(), r["receipt"]["tests"]["polarity"][key]["kind"].as_str()), (Some("clear"), Some("changed")), "{r}");
    let note = r["receipt"]["notes"][0]["what"].as_str().unwrap();
    assert!(note.contains("Private IP address") && note.contains("Blocked hostname"), "{note}");
}

#[test]
fn kerns_kept_copy_of_an_old_test_file_maps_back_to_the_file_it_came_from() {
    assert_eq!(kern::suite::kept_back("packages/lib/ssrfProtection.kern-kept.test.ts").as_deref(), Some("packages/lib/ssrfProtection.test.ts"));
    assert_eq!(kern::suite::kept_back("a.kern-kept3.spec.js").as_deref(), Some("a.spec.js"));
    for name in ["packages/lib/ssrfProtection.test.ts", "kern-kept.test.ts", "a.kern-keptx.test.ts"] { assert_eq!(kern::suite::kept_back(name), None, "{name}"); }
    for (path, n) in [("packages/lib/ssrfProtection.test.ts", 0), ("src/a.spec.js", 4)] {
        assert_eq!(kern::suite::kept_back(&kern::suite::kept_name(path, n).unwrap()).as_deref(), Some(path), "kept_back undoes kept_name");
    }
}

fn farm(root: &Path, name: &str, count: usize, broken: bool) {
    for n in 0..count {
        let dir = root.join(name).join(format!("receipt-acme__{name}__{n}"));
        let (new, stored_lists) = if broken { ("failed", json!({})) } else { ("passed", json!({})) };
        stored(&dir, "vitest", stored_lists, &[
            ("old", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
            ("new", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", new)])),
            ("new-alone", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", new)])),
            ("old-alone", vitest(&[(ADDS, "adds", "passed"), (ADDS, "discounts", "passed")])),
        ]);
    }
}

#[test]
fn the_replay_gate_judges_a_whole_farm_in_bounded_time_and_names_every_changed_answer() {
    let t = Temp::new("gate");
    farm(&t.0, "steady", 40, false);
    farm(&t.0, "broke", 40, true);
    let big: Vec<(String, String, &str)> = (0..20_000).map(|i| (format!("test/file{}.test.js", i % 200), format!("case {i}"), "passed")).collect();
    let big: Vec<(&str, &str, &str)> = big.iter().map(|(f, c, s)| (f.as_str(), c.as_str(), *s)).collect();
    let webpack = t.0.join("steady").join("receipt-webpack__webpack__22695");
    stored(&webpack, "jest", json!({}), &[("old", vitest(&big)), ("new", vitest(&big)), ("new-alone", vitest(&big)), ("old-alone", vitest(&big))]);
    let torn = t.0.join("steady").join("receipt-acme__torn__1");
    std::fs::create_dir_all(&torn).unwrap();
    std::fs::write(torn.join("receipt.json"), "{ not json").unwrap();
    let started = std::time::Instant::now();
    let mut out = Vec::new();
    let summary = kern::replay::run(std::slice::from_ref(&t.0), 4, &mut out).unwrap();
    let secs = started.elapsed().as_secs();
    assert!(secs < 60, "82 stored checks, one with 20,000 assertions, took {secs} s: {summary}");
    assert_eq!((summary["checks"].as_u64(), summary["judged"].as_u64(), summary["errors"].as_u64()), (Some(82), Some(81), Some(1)), "{summary}");
    assert_eq!((summary["changed"].as_u64(), summary["changed_decisions"].as_u64()), (Some(40), Some(40)), "every broke/ check was stored as clear but its runs show a break: {summary}");
    let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 82);
    let names: Vec<&str> = lines.iter().map(|l| l["receipt"].as_str().unwrap()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "lines come in the order the folders are found, so two outputs diff line by line");
    let changed: Vec<&Value> = lines.iter().filter(|l| l["changed"] == true).collect();
    assert_eq!(changed.len(), 40);
    assert!(changed.iter().all(|l| l["receipt"].as_str().unwrap().starts_with("broke/") && l["before"]["decision"] == "clear" && l["after"]["decision"] == "blocked" && l["after"]["blocked"] == json!(["test-broke"])), "{}", changed[0]);
    let error = lines.iter().find(|l| l["receipt"] == "steady/receipt-acme__torn__1").unwrap();
    assert!(error["error"].as_str().is_some_and(|e| !e.is_empty()) && error["after"].is_null(), "a torn receipt is named, never judged in part: {error}");
    let big_line = lines.iter().find(|l| l["receipt"] == "steady/receipt-webpack__webpack__22695").unwrap();
    assert_eq!((big_line["changed"].as_bool(), big_line["replayed"].as_u64()), (Some(false), Some(1)), "{big_line}");
}

#[test]
fn the_replay_finds_stored_checks_at_a_root_a_farm_or_a_folder_of_farms_and_no_deeper() {
    let t = Temp::new("roots");
    for rel in ["receipt-acme__a__1", "farm/receipt-acme__b__2", "data/farm/receipt-acme__c__3", "data/x/farm/receipt-acme__d__4", "farm/notes-acme__e__5"] {
        let dir = t.0.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("receipt.json"), "{}").unwrap();
    }
    std::fs::create_dir_all(t.0.join("farm/receipt-acme__empty__6")).unwrap();
    let found: Vec<String> = kern::replay::checks(std::slice::from_ref(&t.0)).iter().map(|p| p.strip_prefix(&t.0).unwrap().to_string_lossy().replace('\\', "/")).collect();
    assert_eq!(found, ["data/farm/receipt-acme__c__3", "farm/receipt-acme__b__2", "receipt-acme__a__1"]);
    let one = kern::replay::checks(&[t.0.join("farm/receipt-acme__b__2")]);
    assert_eq!(one, [t.0.join("farm/receipt-acme__b__2")], "a check's own folder is one check");
}
