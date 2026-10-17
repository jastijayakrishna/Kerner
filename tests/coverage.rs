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
fn cap_go_capgo_app_2789_a_pattern_the_shell_would_expand_is_never_read_as_no_tests() {
    use kern::script::{Found, found};
    let run = |script: &str| found(&json!({ "scripts": { "test": script } }));
    for script in ["vitest run tests/*.test.ts .github/actions/**/tests/*.test.ts", "jest src/*.spec.js", "vitest run src/[ab]*.test.ts", "vitest run test/?.test.ts"] {
        assert_eq!(run(script), Found::Nothing, "{script}: the shell turns the pattern into file names, Kern runs no shell, and vitest or jest reads the pattern as a name filter that matches nothing");
    }
    for script in [r#"mocha "test/*.js""#, "mocha test/*.js", "node --test test/*.test.js", r#"vitest run "tests/a.test.ts""#, "vitest run tests/", "jest --testPathPattern=src/*"] {
        assert!(matches!(run(script), Found::Run(_)), "{script}: mocha and node expand their own patterns, and a quoted word, a plain name or a flag reaches the runner as it is");
    }
    let capgo = json!({ "scripts": { "test:unit": "vitest run tests/*.unit.test.ts", "test:all": "vitest run tests/*" } });
    assert!(!suite::follows_script(&capgo), "Kern could not follow capgo's own unit test script, so finding no tests says nothing about the project");
    assert!(!suite::follows_script(&json!({ "name": "klaviyo", "devDependencies": { "vitest": "3.2.4" } })),
        "saleor/apps#2046: apps/klaviyo has no test script, so Kern's own vitest run finding nothing is not the project's script finding nothing");
    assert!(suite::follows_script(&json!({ "scripts": { "test": "mocha \"test/*.js\"" } })), "azu/ecmascript-proposals-json#20: mocha itself found no file for the project's own pattern");
}

#[test]
fn cap_go_capgo_app_2789_a_file_tests_write_under_a_random_name_on_both_sides_is_not_the_change() {
    let old = "temp_cli_test/com.cli_835b3249-acdc-4bb4-86db-a132f2dc2d91/capacitor.config.ts";
    let new = "temp_cli_test/com.cli_692abb87-dd91-4d9d-8f3e-0b1c2d3e4f50/capacitor.config.ts";
    assert_eq!(suite::path_shape(old), suite::path_shape(new), "the old code's tests wrote the same file under another random name, so the new code writing it is not the change");
    assert_eq!(suite::path_shape("out/run-1792227091689.js"), suite::path_shape("out/run-1792227099999.js"), "a time stamp is random too");
    assert_ne!(suite::path_shape("temp_cli_test/com.cli_ccr_692abb87-dd91-4d9d-8f3e-0b1c2d3e4f50/index.js"), suite::path_shape(new), "a different file keeps its own shape");
    assert_ne!(suite::path_shape("src/evil.js"), suite::path_shape("src/total.js"), "names with no random part are compared as they are");
}

#[test]
fn saleor_apps_2046_avatax_a_project_that_shuffles_its_tests_still_runs_in_one_fixed_order() {
    use kern::script::{Found, found};
    let run = |script: &str| match found(&json!({ "scripts": { "test": script } })) { Found::Run(i) => i.command, other => panic!("{script}: {other:?}") };
    for script in ["vitest --project units --sequence.shuffle", "vitest run --sequence.shuffle=true src", "vitest --sequence.shuffle.files --sequence.shuffle.tests"] {
        let command = run(script);
        assert!(!command.iter().any(|a| a.starts_with("--sequence.shuffle")), "{script}: Kern runs the tests in one fixed order (it adds --sequence.shuffle=false), and vitest refuses a flag given twice: {command:?}");
    }
    assert!(run("vitest --project units --sequence.shuffle").iter().any(|a| a == "units"), "the project filter stays");
    assert!(run("vitest run --sequence.shuffle src").iter().any(|a| a == "src"), "a flag that takes no value never swallows the name after it");
    let jest = run("jest --randomize --ci");
    assert!(!jest.iter().any(|a| a == "--randomize") && jest.iter().any(|a| a == "--ci"), "jest's random order is dropped the same way, and nothing else is: {jest:?}");
}

#[test]
fn a_test_script_kern_cannot_follow_still_counts_as_changed_when_its_words_change() {
    let pkg = |s: &str| json!({ "scripts": { "test": s } });
    assert!(!kern::script::same_tests(&pkg("vitest run src/**/*.test.ts"), &pkg("vitest run src/a*.test.ts")), "narrowing a pattern changes which tests run, though Kern can follow neither script");
    assert!(!kern::script::same_tests(&pkg("ava"), &pkg("ava --match '!*slow*'")), "a runner Kern does not run still checks less after this change");
    assert!(!kern::script::same_tests(&json!({ "scripts": { "test:unit": "vitest run tests/*.unit.test.ts" } }), &json!({ "scripts": { "test:unit": "vitest run tests/a.unit.test.ts" } })));
    assert!(kern::script::same_tests(&pkg("vitest run"), &pkg("vitest run --reporter=verbose")), "a reporter flag does not change what the tests check");
    assert!(kern::script::same_tests(&pkg("ava"), &json!({ "scripts": { "test": "ava" }, "version": "2.0.0" })));
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
fn posthog_js_5291_and_openclaw_134760_each_pnpm_runs_through_a_corepack_that_knows_where_it_starts() {
    let t = Temp::new();
    std::fs::write(t.0.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    let image = "/usr/local/lib/node_modules/corepack/dist/corepack.js";
    let own = "/workspace/.kern-tools/corepack/node_modules/corepack/dist/corepack.js";
    for (pin, corepack, why) in [
        ("pnpm@10.12.1", image, "pnpm 10 has run through the image's corepack on every farm"),
        ("pnpm@11.7.0", image, "pnpm 11 still ships bin/pnpm.cjs, the file the image's corepack starts, so PostHog/posthog-js#5291 installed this way on farms 16 to 18"),
        ("pnpm@12.1.0+sha512.d9b8", own, "pnpm 12 ships only bin/pnpm.mjs, which the corepack in Node 24 and older images never looks for (openclaw/openclaw#134760)"),
        ("pnpm@12.14.0", own, "every later pnpm 12 is the same"),
        ("pnpm@13.0.0-rc.1", own, "and so is a release candidate of a later major"),
    ] {
        std::fs::write(t.0.join("package.json"), format!(r#"{{"name":"x","packageManager":"{pin}"}}"#)).unwrap();
        let (manager, attempts) = kern::sandbox::install_plan(&t.0);
        assert_eq!(manager, "pnpm");
        for (argv, _) in &attempts { assert_eq!(&argv[..3], ["/usr/local/bin/node", corepack, "pnpm"], "{pin}: {why}: {argv:?}"); }
        assert!(attempts[0].0.iter().any(|a| a == "--frozen-lockfile"));
    }
}

#[test]
fn expo_expo_49716_and_public_ui_kolibri_10041_a_pnpm_range_in_engines_picks_a_pnpm_inside_it() {
    let t = Temp::new();
    std::fs::write(t.0.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
    let own = "/workspace/.kern-tools/corepack/node_modules/corepack/dist/corepack.js";
    let launcher = |package: &str| {
        std::fs::write(t.0.join("package.json"), package).unwrap();
        let (_, attempts) = kern::sandbox::install_plan(&t.0);
        attempts.iter().map(|(argv, _)| argv[..3].to_vec()).collect::<Vec<_>>()
    };
    for (range, why) in [("^10.33.0", "expo asks for pnpm 10.33 or later in 10, and corepack's own default, 11.24.0, is outside it"), ("^10", "kolibri"), (">=8.6.0 <11", "a range with two ends")] {
        let got = launcher(&format!(r#"{{"name":"x","engines":{{"pnpm":"{range}"}}}}"#));
        assert!(got.iter().all(|a| a == &["/usr/local/bin/node", own, &format!("pnpm@{range}")]), "{why}: corepack picks the newest pnpm inside the range, online when installing and from its cache when the tests run: {got:?}");
    }
    let pinned = launcher(r#"{"name":"x","packageManager":"pnpm@10.12.1","engines":{"pnpm":"^10"}}"#);
    assert_eq!(pinned[0][2], "pnpm", "an exact pin in packageManager wins over a range: {pinned:?}");
    for odd in ["https://example.com/pnpm.tgz", "^10 && curl x", "latest", ""] {
        let got = launcher(&format!(r#"{{"name":"x","engines":{{"pnpm":"{odd}"}}}}"#));
        assert_eq!(got[0][2], "pnpm", "{odd:?} is not a version range, so Kern does not hand it to corepack: {got:?}");
    }
}

#[test]
fn antvis_chart_visualization_skills_54_a_project_that_pins_no_pnpm_gets_the_same_pnpm_on_every_check() {
    for (step, env) in [("install", &kern::sandbox::INSTALL_ENV[..]), ("tests", &kern::sandbox::TEST_ENV[..])] {
        assert!(env.contains(&("COREPACK_DEFAULT_TO_LATEST", "0")),
            "{step}: with no pin corepack fetched the newest pnpm, 12.14.0 that day, which changes from one day to the next and which the image's corepack cannot start; its own known version is the same on every check and needs no network: {env:?}");
    }
}

#[test]
fn webpack_22695_and_vercel_workflow_3052_temporary_files_live_on_disk_outside_the_project() {
    let t = Temp::new();
    let limits = kern::policy::Limits { files_changed: 1, lines_changed: 1, file_bytes: 1, total_bytes: 1 << 20, timeout_seconds: 60, memory_mb: 512, pids: 64 };
    let run = kern::sandbox::Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__drive_test".into()], network: "none", volumes: vec![],
        root: false, home_mb: 0, scratch: false, workspace: Some(("kern-deps-x", false)) };
    let args = kern::sandbox::container_args_for(&run, "kern-x", "runc").unwrap();
    assert!(args.windows(2).any(|w| w[0] == "--mount" && w[1] == "type=volume,target=/tmp"),
        "a test run's /tmp is a fresh volume on disk: gatsby's jest cache outgrew a /tmp in memory, and a temporary folder inside the project made webpack's and vercel/workflow's temporary projects find the repository's own files: {args:?}");
    assert!(!args.iter().any(|a| a.starts_with("/tmp:")), "{args:?}");
    assert!(!kern::sandbox::TEST_ENV.iter().any(|(k, _)| *k == "TMPDIR"), "the tests' temporary folder is /tmp, as on the project's own CI: {:?}", kern::sandbox::TEST_ENV);
    let drive = kern::sandbox::Spec { entry: vec!["__drive".into()], volumes: vec![], workspace: Some(("kern-deps-x", true)), ..run };
    let args = kern::sandbox::container_args_for(&drive, "kern-x", "runc").unwrap();
    assert!(args.iter().any(|a| a.starts_with("/tmp:rw,noexec,nosuid,nodev,size=67108864")) && !args.iter().any(|a| a.ends_with("target=/tmp")), "other runs keep a small /tmp in memory: {args:?}");
}

#[cfg(unix)]
#[test]
fn a_pnpm_12_project_calls_pnpm_in_its_tests_through_files_kern_copied_as_root() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new();
    let home = t.0.join("corepack-home/v1/pnpm/12.14.0");
    std::fs::create_dir_all(&home).unwrap();
    for (name, mode) in [("pnpm-native", 0o700), ("package.json", 0o600), ("pn", 0o555), ("README.md", 0o444)] {
        std::fs::write(home.join(name), b"x").unwrap();
        std::fs::set_permissions(home.join(name), std::fs::Permissions::from_mode(mode)).unwrap();
    }
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
    kern::sandbox::read_only_tree(&t.0);
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
    for (name, want) in [("pnpm-native", 0o755), ("package.json", 0o644), ("pn", 0o755), ("README.md", 0o644)] {
        assert_eq!(mode(&home.join(name)), want, "{name}: the tests run as another user than the root that copied Kern's tools, so each file must be readable and, if it was a program, runnable");
    }
    assert_eq!(mode(&home), 0o755);
}

#[test]
fn the_package_manager_the_project_installs_with_is_on_the_path_when_its_tests_run() {
    let shim = kern::sandbox::shim(&["/usr/local/bin/node".to_string(), "/workspace/.kern-tools/corepack/node_modules/corepack/dist/corepack.js".to_string(), "pnpm".to_string()]);
    assert_eq!(shim, "#!/bin/sh\nexec '/usr/local/bin/node' '/workspace/.kern-tools/corepack/node_modules/corepack/dist/corepack.js' 'pnpm' \"$@\"\n");
    let corepack = kern::sandbox::shim(&["/usr/local/bin/node".to_string(), "/usr/local/lib/node_modules/corepack/dist/corepack.js".to_string(), "yarn".to_string()]);
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
