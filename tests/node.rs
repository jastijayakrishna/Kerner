use kern::nodeapi::{satisfies, supported_major, wanted_major};
use kern::receipt;
use serde_json::json;
use std::collections::BTreeMap;

fn files(list: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: BTreeMap<String, String> = list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |p: &str| map.get(p).cloned()
}

#[test]
fn node_version_ranges_are_read_the_way_npm_reads_them() {
    let at = |m: u64| (m, 999, 999);
    assert_eq!(satisfies(">=18", at(22)), Some(true));
    assert_eq!(satisfies("^20 || ^22", at(22)), Some(true));
    assert_eq!(satisfies("^20 || ^22", at(24)), Some(false));
    assert_eq!(satisfies(">=24.21.0", at(22)), Some(false));
    assert_eq!(satisfies(">=24.21.0", at(24)), Some(true));
    assert_eq!(satisfies("18.x", at(18)), Some(true));
    assert_eq!(satisfies("18.x", at(20)), Some(false));
    assert_eq!(satisfies("20 - 22", at(22)), Some(true));
    assert_eq!(satisfies("20 - 22", at(24)), Some(false));
    assert_eq!(satisfies("<20", at(18)), Some(true));
    assert_eq!(satisfies("<20", at(20)), Some(false));
    assert_eq!(satisfies(">=14 <21", at(20)), Some(true));
    assert_eq!(satisfies(">=14 <21", at(22)), Some(false));
    assert_eq!(satisfies(">= 18", at(22)), Some(true), "a space after the operator");
    assert_eq!(satisfies("~20.10", (20, 10, 5)), Some(true));
    assert_eq!(satisfies("~20.10", (20, 11, 0)), Some(false));
    assert_eq!(satisfies("*", at(26)), Some(true));
    assert_eq!(satisfies("not a range", at(22)), None);
}

#[test]
fn kern_runs_the_node_version_the_project_asks_for() {
    let none: Vec<String> = Vec::new();
    let pick = |list: &[(&str, &str)]| wanted_major(&files(list), &none).map(|(m, _)| m);
    assert_eq!(pick(&[(".nvmrc", "20\n")]), Some(20));
    assert_eq!(pick(&[(".nvmrc", "v20.11.1")]), Some(20));
    assert_eq!(pick(&[(".nvmrc", "# pinned\nlts/iron\n")]), Some(20));
    assert_eq!(pick(&[(".nvmrc", "lts/*")]), Some(24));
    assert_eq!(pick(&[(".nvmrc", "node")]), Some(26));
    assert_eq!(pick(&[(".node-version", "18.19.0")]), Some(18));
    assert_eq!(pick(&[(".tool-versions", "python 3.12.1\nnodejs 22.3.0\n")]), Some(22));
    assert_eq!(pick(&[("package.json", r#"{"volta":{"node":"20.11.1"}}"#)]), Some(20));
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":">=24.21.0"}}"#)]), Some(24), "FWU-DE/ais-chat's pnpm install refused Node 22");
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":">=18"}}"#)]), Some(22), "a range Node 22 meets keeps Node 22");
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":"^18"}}"#)]), Some(18));
    assert_eq!(pick(&[(".nvmrc", "20"), ("package.json", r#"{"engines":{"node":">=18"}}"#)]), Some(20), "the version file wins over a range");
    assert_eq!(pick(&[(".nvmrc", "21")]), Some(22), "an odd release runs on the next even one");
    assert_eq!(pick(&[(".nvmrc", "16")]), Some(18));
    assert_eq!(pick(&[("package.json", "{}")]), None);
    assert_eq!(supported_major(27), 26);
    let ci = vec!["jobs:\n  test:\n    strategy:\n      matrix:\n        node-version: [18.x, 20.x]\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: ${{ matrix.node-version }}\n".to_string()];
    assert_eq!(wanted_major(&files(&[]), &ci).map(|(m, _)| m), Some(20), "a CI matrix without 22 runs its newest");
    let plain = vec!["      - uses: actions/setup-node@v4\n        with:\n          node-version: '24'\n".to_string()];
    assert_eq!(wanted_major(&files(&[]), &plain), Some((24, "the CI workflow sets up Node 24".to_string())));
}

#[test]
fn the_receipt_says_which_node_ran() {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let off = json!({ "status": "OFF", "why": "no AI key" });
    let tests = |old: serde_json::Value, new: serde_json::Value| json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 1, "failed": 0, "skipped": 0 },
        "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [],
        "not_run": [], "node": { "old": old, "new": new } });
    let md = |t: serde_json::Value| receipt::markdown(&receipt::build(&d, &rules, &t, &off), None);
    assert!(md(tests(json!({ "major": 20, "from": ".nvmrc says 20" }), json!({ "major": 20, "from": ".nvmrc says 20" }))).contains("✅ Node: 20 on both sides, as the project asks (.nvmrc says 20)"));
    assert!(md(tests(json!({ "major": 20, "from": ".nvmrc says 20" }), json!({ "major": 22, "from": ".nvmrc says 22" }))).contains("moves the project from Node 20 to Node 22"));
    assert!(md(tests(json!({ "major": 22, "wanted": 26, "from": ".nvmrc says node", "why": "no network" }), json!({ "major": 22, "wanted": 26, "from": ".nvmrc says node", "why": "no network" })))
        .contains("asks for Node 26"));
    assert!(!md(tests(json!({ "major": 22 }), json!({ "major": 22 }))).contains("Node:"), "nothing to say when the project names no version");
}

#[test]
fn the_node_the_projects_ci_runs_comes_first_and_never_one_its_engines_forbid() {
    let ci = |text: &str| vec![text.to_string()];
    let workers = files(&[("package.json", r#"{"volta":{"node":"20.19.3"},"engines":{"node":">=20"}}"#)]);
    assert_eq!(wanted_major(&workers, &ci("      - uses: actions/setup-node@v4\n        with:\n          node-version: 22 # need this version for `Set` methods\n      - run: pnpm test:ci\n")).map(|(m, _)| m), Some(22),
        "cloudflare/workers-sdk pins volta 20 but tests on 22; on 20 its tests gave no results");
    let strict = files(&[("package.json", r#"{"engines":{"node":">=22"}}"#)]);
    assert_eq!(wanted_major(&strict, &ci("node-version: 20\n")).map(|(m, _)| m), Some(22), "a version engines.node forbids is never chosen");
    let old_pin = files(&[(".nvmrc", "18"), ("package.json", r#"{"engines":{"node":">=20"}}"#)]);
    assert_eq!(wanted_major(&old_pin, &[]).map(|(m, _)| m), Some(22));
}

#[test]
fn review_1_b_an_exact_or_tilde_engines_pin_still_allows_its_own_major() {
    let none: Vec<String> = Vec::new();
    let pick = |list: &[(&str, &str)]| wanted_major(&files(list), &none).map(|(m, _)| m);
    assert_eq!(pick(&[(".nvmrc", "20"), ("package.json", r#"{"engines":{"node":"20.11.1"}}"#)]), Some(20), "an exact pin allows its own major");
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":"~20.11.0"}}"#)]), Some(20));
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":">=18 <=20.10"}}"#)]), Some(20), "of the majors a range allows, the one nearest Node 22");
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":">=24.21.0"}}"#)]), Some(24));
    assert_eq!(pick(&[("package.json", r#"{"engines":{"node":"^18"}}"#)]), Some(18));
}

#[test]
fn tanstack_cli_507_only_a_workflow_that_runs_the_tests_says_which_node_they_need() {
    let skills = "on: pull_request\njobs:\n  check:\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n      - run: npx tsx scripts/check-skills.ts\n".to_string();
    let pr = "on: pull_request\njobs:\n  test:\n    steps:\n      - uses: tanstack/config/.github/setup@main\n      - run: pnpm build\n      - run: pnpm test:unit\n".to_string();
    assert_eq!(wanted_major(&files(&[(".nvmrc", "24.8.0\n")]), &[skills, pr]), Some((24, ".nvmrc says 24.8.0".to_string())),
        "a skills check sets up Node 20 and runs no test; the tests need node:sqlite, which Node 20 lacks");
    let filed = "jobs:\n  test:\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version-file: .node-version\n      - run: npm test\n".to_string();
    assert_eq!(wanted_major(&files(&[(".node-version", "22.11.0\n"), (".nvmrc", "18")]), &[filed]).map(|(m, _)| m), Some(22), "the test workflow reads its Node from .node-version");
    let tested = "jobs:\n  test:\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 22\n      - run: pnpm vitest run\n".to_string();
    assert_eq!(wanted_major(&files(&[(".nvmrc", "20")]), &[tested]).map(|(m, _)| m), Some(22), "the Node the tests run on in CI beats a version file");
}
