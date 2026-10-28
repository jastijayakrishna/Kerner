#[test]
fn recallnet_js_recall_1130_a_setup_that_fails_in_the_sandbox_says_why() {
    let errors = b"warn: something minor\nError: Unsupported input file type\n";
    let said = kern::drive::setup_failure("old", 1, errors);
    assert!(said.starts_with("Setting up the old code failed in the sandbox (exit code 1)") && said.contains("Unsupported input file type"),
        "the receipt said only the exit code, so nobody could tell why the setup of js-recall#1130 and trpc#7350 stopped: {said}");
    assert_eq!(kern::drive::setup_failure("new", 137, b""), "Setting up the new code failed in the sandbox (exit code 137)");
    let long = format!("Error: {}", "x".repeat(2000));
    assert!(kern::drive::setup_failure("old", 1, long.as_bytes()).chars().count() < 400, "the quote is cut to a sane length");
}

#[test]
fn ci_runs_breaks_every_request_kern_sends_the_test_box_has_exactly_the_fields_the_box_accepts() {
    let fields = ["\"version\": 1", "\"command\"", "\"timeout_seconds\"", "\"cwd\"", "\"env\"", "\"files\"", "\"early_secs\"", "\"faults\"", "\"serve\"", "\"steps\""];
    let mut requests = 0;
    for entry in std::fs::read_dir("src").unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "rs") { continue; }
        let src = std::fs::read_to_string(&path).unwrap();
        for (i, line) in src.lines().enumerate() {
            if !line.contains("json!({ \"version\": 1, \"command\"") { continue; }
            let request: String = src.lines().skip(i).take(3).collect::<Vec<_>>().join(" ");
            if !request.contains("\"cwd\"") { continue; }
            requests += 1;
            for f in fields { assert!(request.contains(f), "{}:{}: a box request without {f}: the box denies it and the run reads as 'The run\'s report is incomplete' (the recorder's request on CI run 40714685904)", path.display(), i + 1); }
            assert!(!request.contains("\"early\":"), "{}:{}: the box reads early_secs, not early", path.display(), i + 1);
        }
    }
    assert!(requests >= 7, "{requests} box requests found in src");
}

#[test]
fn review_2_d_kern_has_one_markdown_escaper_and_it_neutralises_markup() {
    assert_eq!(kern::util::code("a`b<details>\n\u{1b}c", 200), "`a'b‹details>  c`");
    assert_eq!(kern::util::code("  abcdef ", 3), "`abc…`");
    assert_eq!(kern::util::plain("a`<b>\nc", 400), "a'‹b> c");
    assert_eq!(kern::util::plain("abcdef", 2), "ab");
    for file in ["receipt", "drive"] {
        let src = std::fs::read_to_string(format!("src/{file}.rs")).unwrap();
        for own in ["fn code(", "fn span(", "fn plain(s: &str"] {
            assert!(!src.contains(own), "src/{file}.rs keeps its own escaper ({own}); one escaper means one place to get it right, and drive's missed '<'");
        }
    }
}

#[test]
fn review_2_d_one_list_says_which_files_are_code_in_another_language() {
    for other in ["src/App.vue", "lib/a.py", "docs/page.mdx", "run.bash", "x.erl", "core.clj", "Main.fs", "View.m"] {
        assert!(kern::capability::other_code(other), "{other}");
    }
    for not in ["src/a.ts", "README.md", "package.json", "node_modules/x/a.py"] { assert!(!kern::capability::other_code(not), "{not}"); }
    let copies: usize = ["drive", "receipt", "capability", "rules", "check"].iter().map(|f| std::fs::read_to_string(format!("src/{f}.rs")).unwrap().matches("const OTHER_CODE").count()).sum();
    assert_eq!(copies, 1, "the drive's list missed bash and the receipt's missed mdx, so the two parts disagreed about the same file");
}

#[test]
fn review_1_c_a_pinned_tool_is_hashed_once_per_process_and_a_changed_one_is_still_caught() {
    let dir = std::env::temp_dir().join(format!("kern-pin-{}", kern::util::uuid4().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("docker");
    std::fs::write(&file, b"the pinned docker").unwrap();
    let pin = kern::policy::Pin { file: file.clone(), sha256: kern::util::sha256(b"the pinned docker") };
    assert!(kern::policy::pinned(&pin).is_ok() && kern::policy::pinned(&pin).is_ok());
    assert_eq!(kern::policy::pinned_reads(&file), 1, "a second check of the same unchanged file hashes nothing again");
    std::fs::write(&file, b"a swapped docker binary").unwrap();
    assert!(kern::policy::pinned(&pin).is_err(), "a binary changed after the first check is still refused");
    assert_eq!(kern::policy::pinned_reads(&file), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn review_1_c_an_install_retried_after_running_out_of_memory_gets_only_the_time_that_is_left() {
    let now = kern::util::now_ms();
    assert!(kern::drive::setup_seconds(now + 60_000).is_err(), "a minute is too little to install a project");
    assert_eq!(kern::drive::setup_seconds(now + 3_600_000).unwrap(), kern::drive::SETUP_SECS_MAX);
    let left = kern::drive::setup_seconds(now + 600_000).unwrap();
    assert!((590..=600).contains(&left), "a retry after a long first try gets what is left before the check's deadline, not a fresh {} s: {left}", kern::drive::SETUP_SECS_MAX);
}

#[test]
fn review_1_c_a_check_sets_up_the_project_at_most_once() {
    let out = std::env::temp_dir().join(format!("kern-setup-once-{}", kern::util::uuid4().unwrap()));
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("drive.json"), serde_json::json!({ "repo": out.join("no-such-repo"), "base": "1".repeat(40), "head": "2".repeat(40), "files": [], "skipped": [], "unmeasured": [] }).to_string()).unwrap();
    let op = kern::policy::operator(&serde_json::json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }],
        "registry": "registry.npmjs.org", "permissions": {} })).unwrap();
    let until = kern::util::now_ms() + 3_600_000;
    let first = kern::drive::setup(&out, &op, until).unwrap_err().0;
    assert!(out.join("setup-failed.txt").is_file(), "the failure is recorded for the rest of the check");
    let second = kern::drive::setup(&out, &op, until).unwrap_err().0;
    assert_eq!(second, first, "the type check asked for the same failed setup again and burned up to 25 more minutes on it");
    let _ = std::fs::remove_dir_all(&out);
}
