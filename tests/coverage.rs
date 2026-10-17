use kern::{receipt, suite};
use serde_json::{Value, json};

fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn clean_rules() -> Value { json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] }) }
fn off() -> Value { json!({ "status": "OFF", "why": "no AI key" }) }
fn argv(s: &str) -> Vec<String> { s.split_whitespace().map(str::to_string).collect() }

#[test]
fn azure_appconfiguration_330_tests_that_run_from_build_output_are_built_first() {
    assert!(suite::builds_tests(&argv("/workspace/node_modules/.bin/mocha out/esm/test/*.test.js out/commonjs/test/*.test.js --parallel --timeout 100000")));
    assert!(suite::builds_tests(&argv("/workspace/node_modules/.bin/mocha ./dist/tests/**/*.js")));
    assert!(suite::builds_tests(&argv("/workspace/node_modules/.bin/jest --rootDir build")));
    for plain in ["/workspace/node_modules/.bin/mocha test/*.js", "/workspace/node_modules/.bin/vitest run", "/workspace/node_modules/.bin/mocha --require ts-node/register src/**/*.spec.ts",
        "/workspace/node_modules/.bin/jest --config jest.config.js"] {
        assert!(!suite::builds_tests(&argv(plain)), "{plain} runs files that are in the repository");
    }
}

#[test]
fn azu_ecmascript_proposals_json_20_a_test_script_that_finds_no_test_files_means_no_tests() {
    let mocha = "the output contains no mocha report (exit 1); the last lines it printed: Warning: Could not find any test files matching pattern: test/*.js / No test files found";
    assert!(suite::no_test_files(mocha));
    assert!(suite::no_test_files("the output contains no vitest report (exit 1); the last lines it printed: No test files found, exiting with code 1"));
    assert!(!suite::no_test_files("the output contains no mocha report (exit 1); its first error: Error: Cannot find module './setup'"));
    let tests = json!({ "status": "NOT_RUN", "why": "the project's test script finds no test files, so no test checks this change", "no_tests": true });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "clear", "a project with no tests has nothing for Kern to run; the receipt says so instead of asking for a look: {r}");
    let md = receipt::markdown(&r, None);
    assert!(md.contains("finds no test files") && md.contains("rules only"), "{md}");
}

#[test]
fn a_package_whose_test_script_finds_no_tests_is_named_next_to_the_ones_that_ran() {
    let tests = json!({ "status": "RAN", "runner": "vitest in packages/a", "before": { "passed": 2, "failed": 0, "skipped": 0 }, "after": { "passed": 2, "failed": 0, "skipped": 0 }, "kept": 2,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "no_tests_in": ["packages/docs"] });
    let r = receipt::build(&drive_json(), &clean_rules(), &tests, &off());
    assert_eq!(r["decision"], "clear", "{r}");
    assert!(receipt::markdown(&r, None).contains("Tests in `packages/docs`: none"), "{}", receipt::markdown(&r, None));
}

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-coverage-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

#[test]
fn antvis_chart_visualization_skills_54_pnpm_12_comes_from_npm_because_corepack_cannot_run_it() {
    let t = Temp::new();
    std::fs::write(t.0.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    std::fs::write(t.0.join("package.json"), r#"{"name":"skills","packageManager":"pnpm@12.14.0+sha512.0a1b"}"#).unwrap();
    let (manager, attempts) = kern::sandbox::install_plan(&t.0);
    assert_eq!(manager, "pnpm");
    assert_eq!(&attempts[0].0[..2], ["/bin/sh", "/workspace/.kern-tools/pnpm/node_modules/pnpm/pnpm"], "pnpm 12 ships a native program corepack 0.36 cannot find: {:?}", attempts[0].0);
    assert!(attempts[0].0.iter().any(|a| a == "--frozen-lockfile"));
    assert_eq!(kern::sandbox::pnpm_from_npm(&t.0), Some(("12.14.0".to_string(), Some("sha512.0a1b".to_string()))));
    std::fs::write(t.0.join("package.json"), r#"{"name":"skills","packageManager":"pnpm@10.12.1"}"#).unwrap();
    let (_, attempts) = kern::sandbox::install_plan(&t.0);
    assert!(attempts[0].0[1].ends_with("corepack.js") && kern::sandbox::pnpm_from_npm(&t.0).is_none(), "pnpm 10 still comes from corepack: {:?}", attempts[0].0);
}

#[test]
fn a_pinned_package_manager_must_be_the_exact_one_npm_installed() {
    use base64::Engine;
    use sha2::Digest;
    let digest = sha2::Sha512::digest(b"the pnpm tarball");
    let integrity = format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(digest));
    let pinned = format!("sha512.{}", hex::encode(digest));
    assert!(kern::sandbox::hash_matches(&integrity, &pinned));
    assert!(!kern::sandbox::hash_matches(&integrity, "sha512.00ff"), "another tarball under the same version");
    assert!(!kern::sandbox::hash_matches(&integrity, &format!("sha224.{}", hex::encode(&digest[..28]))), "Kern can only compare a sha512 pin with what npm checked");
}

#[test]
fn the_package_manager_the_project_installs_with_is_on_the_path_when_its_tests_run() {
    let shim = kern::sandbox::shim(&["/bin/sh".to_string(), "/workspace/.kern-tools/pnpm/node_modules/pnpm/pnpm".to_string()]);
    assert_eq!(shim, "#!/bin/sh\nexec '/bin/sh' '/workspace/.kern-tools/pnpm/node_modules/pnpm/pnpm' \"$@\"\n");
    let corepack = kern::sandbox::shim(&["/usr/local/bin/node".to_string(), "/workspace/.kern-tools/corepack/node_modules/corepack/dist/corepack.js".to_string(), "yarn".to_string()]);
    assert!(corepack.ends_with("corepack.js' 'yarn' \"$@\"\n"), "{corepack}");
    assert!(kern::sandbox::shim(&["/bin/sh".to_string(), "it's".to_string()]).contains(r#"'it'\''s'"#), "a quote in a path cannot end the word");
}

#[test]
fn vuetifyjs_0_840_a_flood_of_output_is_kept_short_instead_of_filling_the_sandbox() {
    let mut kept = kern::sandbox::Capped::new(4, 6);
    for chunk in [&b"abcdef"[..], b"ghij", b"klmnop"] { kept.push(chunk); }
    assert_eq!(String::from_utf8(kept.finish()).unwrap(), "abcd\n… (6 bytes left out)\nklmnop", "the start and the end are kept, the middle is dropped");
    let mut small = kern::sandbox::Capped::new(4, 6);
    small.push(b"report");
    assert_eq!(small.finish(), b"report", "output that fits is kept whole, so a JSON report on stdout stays readable");
}

fn ran_with_faults(faults: Value) -> Value {
    json!({ "status": "RAN", "runner": "vitest in ghost/core", "before": { "passed": 8, "failed": 0, "skipped": 0 }, "after": { "passed": 8, "failed": 0, "skipped": 0 }, "kept": 8,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "faults": faults })
}

#[test]
fn tryghost_ghost_28552_changed_code_no_test_here_reaches_never_gets_a_check_mark() {
    let file = "ghost/core/core/shared/one-at-a-time.ts";
    let fault = |line: u64, outcome: &str| json!({ "path": file, "line": line, "what": "a condition turned around", "outcome": outcome });
    let r = receipt::build(&drive_json(), &clean_rules(), &ran_with_faults(json!([fault(22, "unreached"), fault(31, "untested"), fault(40, "unreached")])), &off());
    assert_eq!(r["decision"], "look", "8 tests passed, but none of them loads the code this change edits: {r}");
    assert!(r["look"].as_array().unwrap().iter().any(|x| x["kind"] == "untested-change" && x["what"].as_str().unwrap().contains(file)), "{}", r["look"]);
    let md = receipt::markdown(&r, None);
    assert!(!md.contains("✅ Tests") && md.contains("⚠️ Tests"), "{md}");
    for reached in ["caught", "missed"] {
        let r = receipt::build(&drive_json(), &clean_rules(), &ran_with_faults(json!([fault(22, "unreached"), fault(31, reached)])), &off());
        assert!(!r["look"].as_array().unwrap().iter().any(|x| x["kind"] == "untested-change"), "a test loaded the changed code ({reached}): {}", r["look"]);
    }
    let r = receipt::build(&drive_json(), &clean_rules(), &ran_with_faults(json!([fault(22, "unknown")])), &off());
    assert_eq!(r["decision"], "clear", "a fault run Kern could not finish says nothing either way: {r}");
    let r = receipt::build(&drive_json(), &clean_rules(), &ran_with_faults(json!([fault(22, "untested"), fault(31, "untested")])), &off());
    assert_eq!(r["decision"], "clear", "no related test by import is not proof: another package's tests may use this code through its build output: {r}");
}

#[test]
fn a_planted_fault_that_no_test_loaded_is_unreached() {
    let before = kern::proof::Report::default();
    let crashed = b"Error: Failed to resolve entry for package \"@orval/core\". The package may have incorrect main/module/exports specified in its package.json.";
    assert_eq!(kern::proof::fault_verdict(kern::policy::Runner::Vitest, &before, (true, false), b"", crashed), "unreached", "orval-labs/orval#3772: the related tests could not load, so nothing reached the fault");
    assert_eq!(kern::proof::fault_verdict(kern::policy::Runner::Vitest, &before, (false, true), b"", b""), "unknown", "a run Kern stopped proves nothing");
    assert_eq!(kern::proof::fault_verdict(kern::policy::Runner::Vitest, &before, (true, false), b"", b"No test files found, exiting with code 1"), "untested");
}

#[test]
fn a_clock_that_steps_back_during_a_check_never_crashes_kern() {
    let later = kern::util::now_ms() + 60_000;
    assert_eq!(kern::util::secs_since(later), 0, "WSL resyncs its clock with Windows; a check that saw the clock step back panicked in its timing summary and lost the receipt");
    assert_eq!(kern::util::secs_between(later, later - 5_000), 0);
    assert_eq!(kern::util::secs_between(later - 5_000, later), 5);
}
