use kern::proof::{Count, Report};
use kern::suite::{self, Evidence, Observed, Unproven};
use serde_json::Value;
use std::collections::BTreeMap;

fn report(tests: &[(&str, u32, u32, u32)], problems: &[&str], why: &[(&str, &str)]) -> Report {
    let mut t = BTreeMap::new();
    for (k, passed, failed, skipped) in tests { t.insert(k.to_string(), Count { passed: *passed, failed: *failed, skipped: *skipped }); }
    Report { tests: t, problems: problems.iter().map(|p| p.to_string()).collect(), why: why.iter().map(|(k, w)| (k.to_string(), w.to_string())).collect() }
}
fn listed(c: &Value, key: &str) -> Vec<String> { c[key].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect() }

#[test]
fn every_combination_of_run_outcomes_blocks_only_with_full_proof() {
    use Observed::*;
    let all = [Passed, Failed, Skipped, Missing];
    let maybe: Vec<Option<Observed>> = std::iter::once(None).chain(all.iter().copied().map(Some)).collect();
    let (mut combinations, mut blocked) = (0u32, 0u32);
    for old in all { for old_again in &maybe { for new in all { for a in &maybe { for b in &maybe { for shaky in [false, true] { for sandbox in [false, true] { for uneven in [false, true] {
        let e = Evidence { old, old_again: *old_again, new, new_again: [*a, *b], shaky, sandbox, uneven };
        let reruns: Vec<Observed> = [*a, *b].into_iter().flatten().filter(|o| *o != Missing).collect();
        let proven = old == Passed && *old_again == Some(Passed) && new == Failed && !reruns.is_empty() && reruns.iter().all(|o| *o == Failed) && !shaky && !sandbox;
        let full = proven && !uneven;
        combinations += 1;
        assert_eq!(suite::blockable(e).is_ok(), full, "a block needs the test to pass in every old run including an old rerun, to fail in every new run that included it with at least one rerun, in a steady file, with no sandbox error, on two sides Kern set up the same way: {e:?}");
        if proven && uneven { assert_eq!(suite::blockable(e), Err(Unproven::Uneven), "sides set up differently can never block: {e:?}"); }
        if *old_again == Some(Failed) { assert_eq!(suite::blockable(e), Err(Unproven::OldFailsToo), "{e:?}"); }
        else if sandbox && old == Passed { assert_eq!(suite::blockable(e), Err(Unproven::Sandbox), "{e:?}"); }
        else if old == Passed && new == Failed && (shaky || reruns.iter().any(|o| *o != Failed)) { assert_eq!(suite::blockable(e), Err(Unproven::Unsteady), "{e:?}"); }
        if full { blocked += 1; }
    }}}}}}}}
    assert_eq!((combinations, blocked), (16000, 5), "of 16000 combinations only 5 block: both reruns failed, or one failed and the other did not run or did not include the test, and the sides were set up the same way");
}

#[test]
fn ghost_28552_a_test_skipped_when_its_file_ran_alone_never_blocks_and_a_missing_native_module_is_the_sandbox() {
    let gift = "ghost/core/test/unit/server/web/gift-preview/image.test.js > Gift Preview Image > generateGiftPreviewImage > returns different results for different accent colors";
    let bindings = "Error: Knex: run\n$ npm install sqlite3 --save\nCannot find module 'bindings'\nRequire stack:\n- /workspace/node_modules/.pnpm/sqlite3@5.1.7/node_modules/sqlite3/lib/sqlite3-binding.js";
    let (old, old_alone) = (report(&[(gift, 1, 0, 0)], &[], &[]), report(&[(gift, 1, 0, 0)], &[], &[]));
    let alone = report(&[(gift, 0, 0, 1)], &[], &[]);
    let plain = report(&[(gift, 0, 1, 0)], &[], &[(gift, "expected 'a' to not equal 'a'")]);
    let c = suite::verdict(&old, &plain, Some(&alone), Some(&plain), Some(&old_alone), false, false);
    assert_eq!((listed(&c, "broke"), listed(&c, "unsteady")), (vec![], vec![gift.to_string()]), "the run of its file alone skipped the test instead of failing it, so it did not fail in every new run: {c}");
    let native = report(&[(gift, 0, 1, 0)], &[], &[(gift, bindings)]);
    let c = suite::verdict(&old, &native, Some(&alone), Some(&native), Some(&old_alone), false, false);
    assert_eq!((listed(&c, "broke"), listed(&c, "sandbox")), (vec![], vec![gift.to_string()]), "sqlite3's native part was missing from the install, which is the sandbox, not the change: {c}");
}

#[test]
fn finance_tracker_36_a_test_file_whose_jest_worker_crashed_never_blocks() {
    let file = "apps/api/src/expenses/expenses.service.spec.ts";
    let test = "apps/api/src/expenses/expenses.service.spec.ts > ExpensesService > creates an expense";
    let crash = "A jest worker process (pid=49) was terminated by another process: signal=SIGSEGV, exitCode=null. Operating system logs may contain more information on why this occurred.";
    let (old, old_alone) = (report(&[(test, 1, 0, 0)], &[], &[]), report(&[(test, 1, 0, 0)], &[], &[]));
    let crashed = report(&[], &[file], &[(file, crash)]);
    let c = suite::verdict(&old, &crashed, Some(&crashed), None, Some(&old_alone), false, false);
    assert!(listed(&c, "new_problems").is_empty() && listed(&c, "broke").is_empty(), "a crashed process is the machine, not proof the change broke the file: {c}");
    assert_eq!(listed(&c, "sandbox"), [file], "{c}");
}

#[test]
fn condo_6842_a_test_file_the_machine_killed_on_the_old_code_is_kerns_limit_not_already_failing() {
    let file = "apps/condo/domains/user/integration/passport/utils/config.spec.js";
    let other = "apps/condo/domains/ticket/schema/Ticket.test.js > creates";
    let killed = "A jest worker process (pid=150) was terminated by another process: signal=SIGTERM, exitCode=null. Operating system logs may contain more information on why this occurred.";
    let old = report(&[(other, 1, 0, 0)], &[file], &[(file, killed)]);
    let new = report(&[(other, 1, 0, 0)], &[file], &[(file, killed)]);
    let c = suite::verdict(&old, &new, Some(&new), None, Some(&old), false, false);
    assert_eq!(listed(&c, "old_sandbox"), [file], "3,946 tests in this file never ran because Kern's machine killed the worker; that is Kern's limit: {c}");
    assert!(listed(&c, "already_failing").is_empty(), "a file the machine killed was not failing before the change: {c}");
    let mut tests = c.clone();
    tests["runner"] = serde_json::json!("jest");
    let r = kern::receipt::build(&serde_json::json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] }),
        &serde_json::json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] }), &tests);
    assert_eq!(r["decision"], "unverified", "{r}");
    assert!(r["unverified"].as_array().unwrap().iter().any(|x| x["kind"] == "test-old-sandbox" && x["what"].as_str().unwrap().contains("config.spec.js")), "{r}");
    let md = kern::receipt::markdown(&r);
    assert!(!md.contains("✅ Tests") && !md.contains("already failing before this change"), "{md}");
}

#[test]
fn condo_6842_test_workers_and_their_heaps_fit_the_sandboxs_memory() {
    use kern::sandbox::{box_cpus, cpu_list};
    assert_eq!(box_cpus(12288, 16), 4, "a 12 GB box shows 4 CPUs, so jest and vitest start 3 workers by their own rule");
    assert_eq!(box_cpus(4096, 16), 2, "a 4 GB box shows 2 CPUs: one worker");
    assert_eq!(box_cpus(12288, 2), 2, "never more CPUs than the machine has");
    assert_eq!(box_cpus(12288, 0), 1);
    assert_eq!(suite::worker_heap_mb(12288, 4), 3072, "3 workers and the parent share 12 GB");
    assert_eq!(suite::worker_heap_mb(4096, 2), 2048);
    assert_eq!(cpu_list("0-3,8-11", 4).as_deref(), Some("0,1,2,3"), "the first CPUs this process may use");
    assert_eq!(cpu_list("2,5-6\n", 3).as_deref(), Some("2,5,6"));
    assert_eq!(cpu_list("0-1", 4), None, "fewer CPUs than wanted: no set, and the runners see what the machine shows");
    assert_eq!(cpu_list("3-1", 1), None);
    assert_eq!(cpu_list("", 1), None);
    let command = kern::suite::test_command(&kern::policy::Tests { paths: vec![], kind: kern::policy::Kind::Feature, runner: kern::policy::Runner::Vitest,
        command: vec!["/workspace/node_modules/.bin/vitest".into(), "run".into()], edits: kern::policy::Edits::Checked });
    assert!(!command.iter().any(|a| a.contains("Workers")), "Kern adds no worker flag: vitest 2 refuses --maxWorkers below its default minimum and then finds no tests ({command:?})");
}

#[test]
fn a_failure_with_the_error_old_tests_already_fail_with_here_is_the_sandbox() {
    let (seed, cart) = ("test/db.test.js > seeds", "test/cart.test.js > adds");
    let refused = "Error: connect ECONNREFUSED 127.0.0.1:5432";
    let old = report(&[(seed, 0, 1, 0), (cart, 1, 0, 0)], &[], &[(seed, refused)]);
    let new = report(&[(seed, 0, 1, 0), (cart, 0, 1, 0)], &[], &[(seed, refused), (cart, refused)]);
    let old_alone = report(&[(cart, 1, 0, 0)], &[], &[]);
    let c = suite::verdict(&old, &new, Some(&new), None, Some(&old_alone), false, false);
    assert_eq!((listed(&c, "broke"), listed(&c, "sandbox")), (vec![], vec![cart.to_string()]), "the cart test now reaches the database the sandbox does not have, exactly as the seed test already did on the old code: {c}");
}

#[test]
fn a_test_broken_in_every_run_with_an_old_rerun_that_passes_still_blocks() {
    let total = "test/total.test.js > discount from 100";
    let (old, old_alone) = (report(&[(total, 1, 0, 0)], &[], &[]), report(&[(total, 1, 0, 0)], &[], &[]));
    let new = report(&[(total, 0, 1, 0)], &[], &[(total, "AssertionError: expected 100 to be 90")]);
    let c = suite::verdict(&old, &new, Some(&new), Some(&new), Some(&old_alone), false, false);
    assert_eq!(listed(&c, "broke"), [total], "the e2e calc break: failed in all three new runs, passed in both old runs: {c}");
}

#[test]
fn a_break_with_no_old_rerun_is_unconfirmed_never_blocked() {
    let total = "test/total.test.js > discount from 100";
    let old = report(&[(total, 1, 0, 0)], &[], &[]);
    let new = report(&[(total, 0, 1, 0)], &[], &[(total, "AssertionError: expected 100 to be 90")]);
    let c = suite::verdict(&old, &new, None, Some(&new), None, false, false);
    assert_eq!((listed(&c, "broke"), listed(&c, "unconfirmed")), (vec![], vec![total.to_string()]), "mocha and vitest with file arguments got only a second new run; without an old rerun nothing can block (CLAUDE.md): {c}");
}

#[test]
fn a_new_run_cut_short_by_kerns_own_limits_never_proves_the_tests_broke() {
    use suite::Side::{Cut, Nothing, Slow};
    let runner = || Nothing("the runner stopped before running any test (exit 1); its first error: Error: the test config cannot load".to_string());
    assert!(suite::broke_run(&runner(), &runner(), false), "the runner itself gave no results twice where the old code's tests ran: the e2e 'broke' case blocks");
    assert!(!suite::broke_run(&runner(), &runner(), true), "the sides were set up differently");
    for (first, again) in [(Slow("the tests did not finish in 15 minutes".into()), Slow("the tests did not finish in 15 minutes".into())),
        (Cut("the run was stopped: Sandbox exceeded limits: it ran out of memory (it allows 4096 MiB)".into()), Cut("the run was stopped: Sandbox exceeded limits: it ran out of memory (it allows 4096 MiB)".into())),
        (Cut("the test runner was stopped by a signal, so Kern does not trust its report".into()), runner()), (runner(), Slow("no test file finished in 7 minutes".into()))] {
        assert!(!suite::broke_run(&first, &again, false), "review A1: Kern's own time, memory or signal limit is not proof the change broke the tests: {first:?} then {again:?}");
    }
}

#[test]
fn a_refusal_from_kerns_own_sandbox_entry_is_kerns_limit_never_a_block() {
    for reason in ["Agent namespace did not quiesce", "No space left on device (os error 28)", "Invalid test run environment", "A test run file may not replace a project file: src/a.kern-kept.test.js", "Too many planted faults"] {
        let side = suite::unreadable(Some(reason.into()), "The run's report is incomplete".into());
        assert!(matches!(&side, suite::Side::Cut(w) if w.contains(reason)), "only a refusal the change can cause (a runner it does not install) is about the change; {reason:?} is Kern's own: {side:?}");
        assert!(!suite::broke_run(&side, &side, false), "twice on the new code is still Kern's limit, never a block: {reason:?}");
    }
    assert_eq!(suite::capped("cut: the test report was larger than Kern's limit of 64 MiB"), Some("the test report was larger than Kern's limit of 64 MiB".to_string()), "a report cut at a cap is a run Kern could not read, so it is not checked");
    assert_eq!(suite::capped("exit 1"), None);
}

#[test]
fn a_sandbox_that_refuses_for_a_reason_in_the_project_is_about_the_change_and_a_broken_report_is_kerns_limit() {
    let refused = suite::unreadable(Some("The test runner is not installed in the project: /workspace/node_modules/.bin/jest does not exist".into()), "The run's report is incomplete".into());
    assert!(matches!(&refused, suite::Side::Nothing(w) if w.contains("not installed in the project")), "the e2e 'missing' case: the change switched the test script to a runner it does not install: {refused:?}");
    assert!(suite::broke_run(&refused, &suite::unreadable(Some("The test runner is not installed in the project".into()), String::new()), false), "twice, where the old code's tests ran: a block");
    assert!(matches!(suite::unreadable(None, "The run's report is incomplete".into()), suite::Side::Cut(_)), "a report cut short with no reason given is Kern's own limit");
}

#[test]
fn random_runs_never_block_a_test_the_raw_runs_do_not_prove_broken() {
    let names = ["a.test.js > one", "a.test.js > two", "b.test.js > three", "b.test.js > four", "c.test.js > five"];
    let errors = ["expected 1 to be 2", "connect ECONNREFUSED 127.0.0.1:5432", "Cannot find module 'bindings'", "TypeError: x is not a function"];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move |n: u64| { state ^= state << 13; state ^= state >> 7; state ^= state << 17; state % n };
    let (mut checked, mut blocks) = (0u32, 0u32);
    for _ in 0..20_000 {
        let mut run = |kept: bool| -> Option<Report> {
            if !kept && next(4) == 0 { return None; }
            let (mut tests, mut why) = (Vec::new(), Vec::new());
            for n in names {
                match next(10) { 0 => {}, 1 => tests.push((n, 0, 0, 1)), 2..=4 => { tests.push((n, 0, 1, 0)); why.push((n, errors[next(4) as usize])); }, _ => tests.push((n, 1, 0, 0)) }
            }
            Some(report(&tests, &[], &why))
        };
        let (old, new) = (run(true).unwrap(), run(true).unwrap());
        let (alone, whole, old_alone) = (run(false), run(false), run(false));
        let c = suite::verdict(&old, &new, alone.as_ref(), whole.as_ref(), old_alone.as_ref(), false, false);
        checked += 1;
        for k in listed(&c, "broke") {
            blocks += 1;
            let at = |r: &Report| r.tests.get(&k).copied().unwrap_or_default();
            assert!(at(&old).passed > 0 && at(&old).failed == 0, "{k} blocked without passing on the old code: {c}");
            assert!(old_alone.as_ref().is_some_and(|o| at(o).passed > 0 && at(o).failed == 0 && at(o).skipped == 0), "{k} blocked without passing in an old rerun: {c}");
            assert!(at(&new).failed > 0, "{k} blocked without failing on the new code: {c}");
            let reruns: Vec<Count> = [alone.as_ref(), whole.as_ref()].into_iter().flatten().map(at).filter(|x| x.passed + x.failed + x.skipped > 0).collect();
            assert!(!reruns.is_empty() && reruns.iter().all(|x| x.failed > 0 && x.passed == 0 && x.skipped == 0), "{k} blocked without failing in every new rerun that ran it: {c}");
            assert!(new.why.get(&k).is_none_or(|w| !w.contains("ECONNREFUSED") && !w.contains("bindings")), "{k} blocked on a sandbox error: {c}");
        }
    }
    assert_eq!(checked, 20_000);
    assert!(blocks > 0, "the random runs include real breaks, and those still block");
}

#[test]
fn vuetifyjs_0_840_a_runner_that_dies_because_the_sandbox_is_full_never_blocks() {
    let errors = b"node:events:505\n    throw er; // Unhandled 'error' event\n    ^\n\nError: ENOSPC: no space left on device, write\n    at writeSync (node:fs:1237:3)\n    at SyncWriteStream._write (node:internal/fs/sync_write_stream:27:5)\n\nNode.js v26.12.0\n";
    let full = suite::without_results("the output contains no vitest report", b"exit 1", errors, b"");
    assert!(matches!(&full, suite::Side::Cut(w) if w.contains("ENOSPC: no space left on device")), "the disk the sandbox gave the run filled up, which is Kern's limit, and the receipt names it: {full:?}");
    assert!(!suite::broke_run(&full, &full, false), "twice out of space proves nothing about the change");
    for limit in ["Error: EMFILE: too many open files, open '/workspace/a.json'", "FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory", "Error: spawn ENOMEM"] {
        assert!(matches!(suite::without_results("no report", b"exit 1", limit.as_bytes(), b""), suite::Side::Cut(_)), "{limit}");
    }
    let broken = suite::without_results("the output contains no vitest report", b"exit 1", b"failed to load config from /workspace/vitest.config.ts\nSyntaxError: Unexpected token '}'\n", b"");
    assert!(matches!(broken, suite::Side::Nothing(_)), "a config the change broke gives nothing for a reason in the project: {broken:?}");
}

#[test]
fn tryghost_ghost_28552_a_file_that_failed_once_and_passed_when_run_again_is_chance_even_with_no_old_record() {
    let file = "ghost/core/test/unit/frontend/services/assets-minification/minifier.test.js";
    let skipped = format!("{file} > Minifier > minifies css");
    let other = "ghost/core/test/unit/other.test.js > works";
    let before = report(&[(skipped.as_str(), 0, 0, 13), (other, 5, 0, 0)], &[], &[]);
    let after = report(&[(skipped.as_str(), 0, 0, 13), (other, 5, 0, 0)], &[file], &[(file, "Error: Hook timed out in 10000ms")]);
    let again = report(&[(skipped.as_str(), 0, 0, 13), (other, 5, 0, 0)], &[], &[]);
    let c = suite::judged(&before, &after, Some(&again), None);
    assert!(listed(&c, "unproven").is_empty(), "it passed when the new code ran again, so chance made it fail once, and chance never decides: {c}");
    assert!(listed(&c, "flaky").iter().any(|k| k == file), "{c}");
    let twice = suite::judged(&before, &after, Some(&after), None);
    assert_eq!(listed(&twice, "unproven"), [file], "failing every time with no old record is still unproven: {twice}");
}

#[test]
fn automattic_wp_calypso_113274_test_names_that_change_between_runs_of_the_same_code_are_chance() {
    let file = "client/my-sites/checkout/src/test/wpcom-store-state.ts";
    let name = |field: &str| format!("{file} > updateManagedContactDetailsShape > data.A is defined at {field}");
    let (city, email, fax, zip) = (name("city"), name("email"), name("fax"), name("postalCode"));
    let other = "client/other.test.ts > works";
    let before = report(&[(city.as_str(), 1, 0, 0), (email.as_str(), 1, 0, 0), (other, 3, 0, 0)], &[], &[]);
    let after = report(&[(zip.as_str(), 1, 0, 0), (fax.as_str(), 1, 0, 0), (other, 3, 0, 0)], &[], &[]);
    let random_old = report(&[(fax.as_str(), 1, 0, 0), (email.as_str(), 1, 0, 0)], &[], &[]);
    let c = suite::judged(&before, &after, Some(&after), Some(&random_old));
    assert!(listed(&c, "gone").is_empty(), "the old code itself named this file's tests differently in two runs, so a name missing from the new run proves nothing: {c}");
    assert!(listed(&c, "flaky").contains(&city), "{c}");
    let steady_old = report(&[(city.as_str(), 1, 0, 0), (email.as_str(), 1, 0, 0)], &[], &[]);
    let c = suite::judged(&before, &after, Some(&after), Some(&steady_old));
    assert!(listed(&c, "gone").contains(&city), "names the old code repeats exactly are real tests, and one that no longer runs is gone: {c}");
}
