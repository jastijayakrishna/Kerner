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
    assert_eq!(outcomes, ["unreached", "unreached"], "the stored runs say no test loaded the changed file: {r}");
    assert_eq!(r["receipt"]["decision"], "look", "{}", r["receipt"]["look"]);
}
