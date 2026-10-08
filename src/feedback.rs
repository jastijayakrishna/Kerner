use crate::ensure;
use crate::policy::Contract;
use crate::util::*;
use serde_json::{Map, Value, json};
use crate::github::{Api, env};
use std::collections::BTreeSet;

pub const SCHEMA: &str = "kern-feedback/v1";
/// Every feedback comment carries this marker with the commit it judged, so retries count rejected commits, not re-runs.
pub const MARKER: &str = "<!-- kern-feedback:v1 head=";
const RESOLVED: &str = "<!-- kern-feedback:v1:pass head=";
pub fn rejected(n: u64) -> String { format!("after {n} rejected attempt{}", if n == 1 { "" } else { "s" }) }

/// Text that came from the change (paths, test names, program names) only ever appears inside code formatting.
fn code(s: &str) -> String {
    let flat: String = s.replace('`', "'").replace(['\r', '\n'], " ").trim().chars().take(300).collect();
    format!("`{flat}`")
}
fn codes<'a>(items: impl IntoIterator<Item = &'a str>) -> String { items.into_iter().map(code).collect::<Vec<_>>().join(", ") }
/// A known message with one value between a fixed start and a fixed end; anything else around it means it is not that message.
fn shaped<'a>(r: &'a str, start: &str, end: &str) -> Option<&'a str> { r.strip_prefix(start)?.strip_suffix(end).filter(|v| !v.is_empty()) }
fn digits(s: &str) -> Option<&str> { (!s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit())).then_some(s) }
fn check_id(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') }

/// An independent verifier's verdict arrives wrapped by the transport that carried it; explain the verdict itself.
fn unwrap_reason(reason: &str) -> String {
    let mut r = reason.trim().to_string();
    loop {
        if let Some(inner) = r.strip_prefix("independent verifier rejected the change: ") { r = inner.trim().to_string(); continue; }
        if let Some(rest) = r.strip_prefix("independent verifier unreachable or rejected the change: ") {
            let verdict = rest.match_indices('{').filter_map(|(i, _)| serde_json::Deserializer::from_str(&rest[i..]).into_iter::<Value>().next()?.ok())
                .filter(|v| v["status"] == "DENIED").filter_map(|v| v["reason"].as_str().map(str::to_string)).next_back();
            if let Some(inner) = verdict { r = inner.trim().to_string(); continue; }
        }
        return r;
    }
}

struct Violation { rule: &'static str, path: Option<String>, detail: Option<String>, instruction: String, allow: Option<String> }
fn fix(rule: &'static str, path: Option<&str>, instruction: String) -> Violation { Violation { rule, path: path.map(str::to_string), detail: None, instruction, allow: None } }
fn human(rule: &'static str, instruction: &str) -> (bool, Vec<Violation>) { (false, vec![fix(rule, None, instruction.to_string())]) }

fn classify(r: &str, contract: Option<&Contract>) -> (bool, Vec<Violation>) {
    let grants = contract.map_or_else(|| "the files this task allows".to_string(), |c| codes(c.write.iter().map(String::as_str)));
    if let Some(rest) = r.strip_prefix("Write outside contract: ") {
        let tail = "\" to filesystem.read and filesystem.write in the task contract";
        if let Some(p) = rest.match_indices(". To allow it, add \"").map(|(i, _)| &rest[..i]).find(|p| !p.is_empty() && rest == format!("{p}. To allow it, add \"{p}{tail}")) {
            let mut v = fix("scope", Some(p), format!("Undo the change to {}. This task may only change {grants}.", code(p)));
            v.allow = Some("A human can widen the task's filesystem grants.".into());
            return (true, vec![v]);
        }
    }
    if let Some(p) = shaped(r, "Protected dependency/configuration path: ", ". Agents may never change dependency, CI or secret files; make this change yourself") {
        return (true, vec![fix("protected-path", Some(p), format!("Leave {} unchanged. Agents may not change dependency, CI or secret files, so solve the task without it.", code(p)))]);
    }
    for (start, end) in [("Binary or oversized change unsupported: ", ". Kern verifies text changes up to limits.file_bytes; make this change yourself"), ("Binary changes unsupported: ", ". Kern verifies text changes only; make this change yourself"),
                         ("Invalid UTF-8 change: ", ". Kern verifies UTF-8 text only; make this change yourself")] {
        if let Some(p) = shaped(r, start, end) {
            return (true, vec![fix("unsupported-change", Some(p), format!("Leave {} unchanged, or keep it as UTF-8 text within the size limit. Kern checks text changes only.", code(p)))]);
        }
    }
    if let Some(p) = shaped(r, "Unsupported changed file type: ", ". Kern can inspect JS/TS, JSON, Markdown, text, CSS and HTML; make this change yourself") {
        return (true, vec![fix("unsupported-change", Some(p), format!("Leave {} unchanged. Kern can only check JS/TS, JSON, Markdown, text, CSS and HTML changes.", code(p)))]);
    }
    if let Some(rest) = r.strip_prefix("Change budget exceeded: ").and_then(|x| x.strip_suffix(" lines. Raise limits.files_changed or limits.lines_changed if this is intended"))
        && let Some((_, allows)) = rest.rsplit_once("; the contract allows ") && let Some((f, l)) = allows.split_once(" files and ") && let (Some(f), Some(l)) = (digits(f), digits(l)) {
        return (true, vec![fix("budget", None, format!("Make a smaller change: at most {f} files and {l} changed lines."))]);
    }
    if let Some((body, grant)) = r.strip_prefix("Patch widens module capabilities: ").and_then(|x| x.rsplit_once(". If this is intended, allow it in operator.json: \"permissions\": "))
        && serde_json::from_str::<Value>(grant).is_ok_and(|g| g.is_object()) {
        let allow = Some(format!("A human can allow it in operator.json: \"permissions\": {grant}"));
        let violations = body.split("; ").map(|item| {
            let (path, caps) = item.rsplit_once(" +[").map_or((item, ""), |(p, c)| (p, c.strip_suffix(']').unwrap_or(c)));
            Violation { rule: "capability", path: Some(path.into()), detail: Some(caps.into()), allow: allow.clone(),
                instruction: format!("Remove the new use of {} from {}. This task may not give code new process, network, filesystem, environment or dynamic-code authority.", codes(caps.split(", ")), code(path)) }
        }).collect();
        return (true, violations);
    }
    for (start, end) in [("Changed code cannot be parsed for capability analysis: ", ". Fix its syntax, or make this change yourself"), ("Changed code cannot be parsed for the test proof: ", ". Fix its syntax")] {
        if let Some(p) = shaped(r, start, end) { return (true, vec![fix("syntax", Some(p), format!("Fix the syntax of {} so that it parses.", code(p)))]); }
    }
    if let Some((list, grant)) = r.strip_prefix("Tests show new runtime behavior: ").and_then(|x| x.rsplit_once(". If this is intended, allow it in the task contract (version 3): \"behaviors\": "))
        && serde_json::from_str::<Value>(grant).is_ok_and(|g| g.is_array()) {
        let mut v = fix("behavior", None, format!("Remove what makes the tests newly reach {}. With the old code the tests did not use this network access, start these programs, read these variables or delete or overwrite these files.", codes(list.split(", "))));
        v.detail = Some(list.into());
        v.allow = Some("A human can allow it in the task contract's behaviors list.".into());
        return (true, vec![v]);
    }
    if let Some((list, grant)) = r.strip_prefix("Calling the new code shows new runtime behavior: ").and_then(|x| x.rsplit_once(". If this is intended, allow it in the task contract: \"behaviors\": "))
        && serde_json::from_str::<Value>(grant).is_ok_and(|g| g.is_array()) {
        let mut v = fix("behavior", None, format!("Remove what makes the new code newly reach {} when Kern calls it. Called with the same inputs, the old code did not use this network access, start these programs, read these variables or delete or overwrite these files.", codes(list.split(", "))));
        v.detail = Some(list.into());
        v.allow = Some("A human can allow it in the task contract's behaviors list.".into());
        return (true, vec![v]);
    }
    if let Some((id, rest)) = r.strip_prefix("Check ").and_then(|x| x.split_once(" case ")) && check_id(id) && let Some((case, tail)) = rest.split_once(' ') && let Some(case) = digits(case) {
        let exit = tail.strip_prefix("failed (exit code ").and_then(|x| x.strip_suffix(')')).is_some_and(|c| digits(c.strip_prefix('-').unwrap_or(c)).is_some());
        let instruction = if exit { format!("The verifier's check {} fails on your change (case {case}). Make the code pass it; its expected answers are hidden on purpose.", code(id)) }
            else if tail == "output differs from the expected answer" { format!("Your change gives the wrong result for the verifier's check {} (case {case}). Fix the behaviour, not the check.", code(id)) }
            else if tail.starts_with("failed: ") || tail.starts_with("left no behavior record") { format!("The verifier's check {} did not finish on your change (case {case}). Make sure the code finishes within the time and memory limits.", code(id)) }
            else { String::new() };
        if !instruction.is_empty() { return (true, vec![fix("check-failed", None, instruction)]); }
    }
    // The comparison of the old and the new code (contract version 5): the old code is the rulebook for everything the task does not state.
    let delta_hint = |v: &mut Violation, what: &str| v.allow = Some(format!("A human can state {what} in the task contract's delta."));
    if let Some(list) = shaped(r, "New crash where the old code worked: ", ". The new code must handle every input the old code handled") {
        let mut v = fix("never-worse", None, format!("Make these calls work again as they did on the old code: {}. The new code must handle every input the old code handled.", code(list)));
        delta_hint(&mut v, "this change");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "The new code did not answer calls the old code answered: ", ". The new code must answer every call the old code answered, in about the same time") {
        let mut v = fix("unanswered", None, format!("Make the new code answer these calls as the old code did, in about the same time: {}. It stopped early or became much slower.", code(list)));
        delta_hint(&mut v, "this change");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "Behaviour changed outside the task's stated change: ", ". Everything the task did not state must behave exactly as before") {
        let mut v = fix("unstated-change", None, format!("Keep these calls behaving exactly as on the old code: {}. Only the change the task states may differ.", code(list)));
        delta_hint(&mut v, "this change");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "Removed without the task stating it: ", ". Restore it; deleting files or exports needs the task's delta to list them under removes") {
        let mut v = fix("unstated-removal", None, format!("Restore {}: this task does not remove them.", code(list)));
        delta_hint(&mut v, "the removal");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "New code deletes files: ", ". Code may not newly delete or truncate files unless the task's delta allows delete") {
        let mut v = fix("deletes-files", None, format!("Remove the new calls that delete or truncate files: {}.", code(list)));
        delta_hint(&mut v, "that the task deletes files");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "Access widened: ", ". A route that refused a user before must still refuse them") {
        let mut v = fix("access-widened", None, format!("Restore the access checks, so these routes refuse as before: {}.", code(list)));
        delta_hint(&mut v, "an intended access change");
        return (true, vec![v]);
    }
    if let Some(list) = shaped(r, "The task's examples do not hold: ", ". The new code must give the answers the task states") {
        return (true, vec![fix("examples", None, format!("Make these calls give the answers the task states: {}.", code(list)))]);
    }
    if let Some(list) = shaped(r, "Access changed outside the task's stated change: ", ". Routes must answer as before") {
        return (true, vec![fix("access-changed", None, format!("Make these routes answer as before: {}.", code(list)))]);
    }
    if let Some(list) = shaped(r, "Slower than the old code: ", ". The task did not allow slower code") {
        return (true, vec![fix("slower", None, format!("Make these functions about as fast as before: {}.", code(list)))]);
    }
    if let Some(list) = shaped(r, "More paid API calls than the old code: ", ". The task did not allow more paid API calls") {
        return (true, vec![fix("paid-calls", None, format!("Remove the extra paid API calls: {}.", code(list)))]);
    }
    if r.starts_with("Kern could not compare the old and the new code: ") { return human("compare", "Kern could not compare the old and the new code. A human has to check the verifier and the task's app recipe."); }
    let locked = "The test proof locks existing tests so they keep checking the old behaviour; ";
    for (start, end) in [("Existing test file changed: ", "put new tests in a new file and leave existing ones unchanged"), ("Existing test file deleted: ", "leave existing tests in place")] {
        if let Some(p) = shaped(r, start, &format!(". {locked}{end}")) {
            return (true, vec![fix("test-locked", Some(p), format!("Restore {} exactly as it was. Existing tests stay unchanged; put new tests in a new file.", code(p)))]);
        }
    }
    if let Some(p) = shaped(r, "Test runner configuration cannot change under the test proof: ", ". It decides which tests run and how they check results; leave it unchanged") {
        return (true, vec![fix("test-config", Some(p), format!("Leave {} unchanged: the test runner's settings decide which tests run and how they check results.", code(p)))]);
    }
    if let Some((p, m)) = shaped(r, "A test file skips or focuses tests: ", ". Every test must run, so remove it").and_then(|x| x.rsplit_once(" uses ")) {
        return (true, vec![fix("test-marker", Some(p), format!("Remove {} from {}. Every test has to run, and the result has to come from the tests themselves.", code(m), code(p)))]);
    }
    if let Some((p, m)) = shaped(r, "Code outside the tests reaches into the test runner: ", ". Code may not change how tests run or what they check, or check whether tests are running; remove it").and_then(|x| x.rsplit_once(" uses ")) {
        return (true, vec![fix("test-tamper", Some(p), format!("Remove {} from {}. Code may not change how tests run or what they check, or behave differently while tests run.", code(m), code(p)))]);
    }
    let missing = |paths: &str| (true, vec![fix("test-missing", None, format!("Add a test under {} that the test command runs, and that fails on the old code and passes with your fix.", codes(paths.split(", "))))]);
    if let Some(paths) = r.strip_prefix("A bug fix must add a test that fails before the fix and passes after it: add or change a test file under ").filter(|p| !p.is_empty()) { return missing(paths); }
    if let Some(paths) = shaped(r, "No new test ran in the test suite. A bug fix must add a test under ", " that the test command runs, and that fails before the fix and passes after it") { return missing(paths); }
    if let Some(list) = shaped(r, "None of the new tests fails on the old code, so they do not show the bug: ", ". Write a test that fails before the fix and passes after it; a test that never reaches the changed code (for example because it mocks it) proves nothing") {
        return (true, vec![fix("test-proof", None, format!("Rewrite your new tests so at least one fails on the old code and passes with your fix: call the real code that had the bug, without mocking it. Tests that passed on the old code: {}.", code(list)))]);
    }
    if shaped(r, "The new tests fail on the old code only sometimes: ", ". A test that shows the bug must fail on the old code every time and pass on the new code").is_some() {
        return (true, vec![fix("test-proof", None, "Make your new test fail on the old code every time, not only sometimes: it must not depend on timing, order or chance.".into())]);
    }
    if shaped(r, "The test run on the old code with the new tests added gave no usable results: ", ". A bug-fix test must load on the old code and fail inside a test, so it cannot depend on files the fix adds").is_some() {
        return (true, vec![fix("test-proof", None, "Make your new test load on the old code and fail inside a test there: it may only use code that existed before your fix.".into())]);
    }
    if let Some(list) = shaped(r, "The test suite fails on the new code: ", ". Every test must pass after the change; fix the code, not the tests") {
        return (true, vec![fix("tests-failing", None, format!("Make these tests pass by fixing the code, not the tests: {}.", code(list)))]);
    }
    if r == "The test command failed on the new code although no test failed. Something outside the tests failed, such as a crash after the run, an unhandled error or a coverage threshold; fix it so the command succeeds" {
        return (true, vec![fix("tests-failing", None, "The test command reports failure although every test passed: fix the crash, unhandled error or threshold that fails it.".into())]);
    }
    if let Some(list) = shaped(r, "Tests did not run on the new code: ", ". Every test that ran before must still run, and every new test must run; do not skip, remove or rename tests") {
        return (true, vec![fix("tests-not-run", None, format!("Make these tests run and pass again: {}. Do not skip, remove or rename tests, or stop them from running.", code(list)))]);
    }
    if let Some(list) = shaped(r, "Existing tests fail on the new code when they run without the new tests: ", ". The code has to behave the same however the tests are run, so it must not keep state that changes its answers") {
        return (true, vec![fix("order-dependent", None, format!("Make the code give the same answers however the tests are run: {} fail on your code when your new tests do not run first. Do not keep state between calls that changes the result.", code(list)))]);
    }
    if let Some(list) = shaped(r, "Existing tests fail on the new code: ", ". Tests may be rewritten or removed, but the old tests must still pass on the new code; fix the code") {
        return (true, vec![fix("behavior-changed", None, format!("Make these existing tests pass on your code again: {}. Rewriting or removing a test does not change what its old version checks.", code(list)))]);
    }
    if shaped(r, "The old tests gave no usable results on the new code: ", ". Tests may be rewritten or moved, but the old tests must still load and pass on the new code").is_some() {
        return (true, vec![fix("behavior-changed", None, "Keep the old tests working on your code: they must still load and pass. Moving or renaming the code they use counts as a change of behaviour, which this task does not allow.".into())]);
    }
    if let Some(list) = shaped(r, "Your test changes stop catching bugs the old tests caught: ", ". Keep a test that catches each of them or write a better one; tests may be rewritten or removed only while every bug the old tests caught is still caught") {
        return (true, vec![fix("tests-weakened", None, format!("Add or keep tests that catch these planted bugs: {}. Tests may be rewritten or removed only while every bug the old tests caught is still caught.", code(list)))]);
    }
    if let Some(rest) = r.strip_prefix("Kern could not check the test changes in time: ")
        && rest.ends_with(" planted bugs were checked before the time limit. Leave the existing tests unchanged, or raise limits.timeout_seconds") {
        return (true, vec![fix("test-edits", None, "Leave the existing tests unchanged: Kern could not check your test changes within the time limit.".into())]);
    }
    if shaped(r, "The test run on the new code gave no usable results: ", ". The test command must run to completion; often a test file cannot load or the run crashed").is_some() {
        return (true, vec![fix("test-results", None, "The test run on the new code did not finish with a report: make every test file load and the whole run complete.".into())]);
    }
    for (start, side) in [("Kern could not run the test suite on the new code: ", "the new code"), ("Kern could not run the test suite on the old code with the new tests added: ", "the old code with your new tests")] {
        if let Some(e) = r.strip_prefix(start) && ["timed out", "exceeded limits", "output exceeded"].iter().any(|w| e.contains(w)) {
            return (true, vec![fix("test-limits", None, format!("Make the tests finish on {side} within the time and memory limits, without printing huge output."))]);
        }
    }
    if let Some((p, hint)) = r.strip_prefix("Workspace output exceeds byte limits at ").and_then(|x| x.rsplit_once(". The contract allows "))
        && hint.ends_with(" in total; raise limits.file_bytes or limits.total_bytes if this is intended") {
        return (true, vec![fix("workspace", Some(p), format!("Make {} smaller or remove it: it is over the size limit.", code(p)))]);
    }
    if let Some(p) = shaped(r, "Symlink in workspace is unsupported: ", ". Kern only accepts regular files") {
        return (true, vec![fix("workspace", Some(p), format!("Replace the link {} with a regular file, or remove it.", code(p)))]);
    }
    if let Some(p) = shaped(r, "Special file in workspace is unsupported: ", ". Kern only accepts regular files") {
        return (true, vec![fix("workspace", Some(p), format!("Remove {}: only regular files are accepted.", code(p)))]);
    }
    if let Some(p) = r.strip_prefix("Nonportable path: ").filter(|p| !p.is_empty()) {
        return (true, vec![fix("file-name", Some(p), format!("Rename {}: use file names that work on every system, without device names such as aux or con, the characters :*?\"<>|, or a trailing dot or space.", code(p)))]);
    }
    if let Some(p) = r.strip_prefix("Reserved path: ").filter(|p| !p.is_empty()) {
        return (true, vec![fix("file-name", Some(p), format!("Remove or rename {}: .git and .kern names are reserved.", code(p)))]);
    }
    if let Some(p) = r.strip_prefix("Case-colliding filenames: ").filter(|p| !p.is_empty()) {
        return (true, vec![fix("file-name", Some(p), format!("Rename {}: another file's path differs from it only in upper and lower case.", code(p)))]);
    }
    if r == "Case-colliding tree path" || r == "File/directory collision" {
        return (true, vec![fix("file-name", None, "Rename files so that no two paths differ only in upper and lower case, and no file has the same path as a directory.".into())]);
    }
    if let Some((name, list)) = r.strip_prefix("Dependency change not approved: ").and_then(|x| x.split_once(". The task approves changes only to: ")) {
        return (true, vec![fix("dependency", Some("package.json"), format!("Do not add or change {}: this task may only add or update {}.", code(name), codes(list.split(", "))))]);
    }
    if let Some(fields) = r.strip_prefix("package.json may only change dependency entries, but this change touches: ") {
        return (true, vec![fix("dependency", Some("package.json"), format!("Leave {} in package.json as it was: only the entries of approved packages may change.", code(fields)))]);
    }
    if let Some(names) = r.strip_prefix("The lockfile changes packages the task did not approve: ") {
        return (true, vec![fix("dependency", Some("package-lock.json"), format!("Regenerate package-lock.json so that only the approved packages and what they need change; leave {} as it was.", code(names)))]);
    }
    if r == "The lockfile format changed: keep lockfileVersion as it was" {
        return (true, vec![fix("dependency", Some("package-lock.json"), "Keep package-lock.json in its existing format (lockfileVersion), for example by using the project's npm version.".into())]);
    }
    if let Some(p) = r.strip_prefix("Dependency files must be valid JSON: ") {
        return (true, vec![fix("dependency", Some(p), format!("Fix {} so it is valid JSON.", code(p)))]);
    }
    if let Some(n) = r.strip_prefix("The agent exited with code ") && digits(n.strip_prefix('-').unwrap_or(n)).is_some() {
        return (true, vec![fix("agent", None, format!("Your previous run ended with exit code {n}. Finish the task and exit with code 0."))]);
    }
    if ["Attestation mismatch: commit", "Attestation mismatch: base", "Pull request must carry exactly one Kern attestation block", "Unterminated Kern attestation block"].contains(&r) {
        let mut v = fix("proof-stale", None, "This pull request's Kern proof covers another commit: something was pushed to the kern/ branch, or the base moved, after Kern verified it. A human has to turn on re-verification or run the task through Kern again.".into());
        v.allow = Some("Give the Kern action verifier-url and verifier-token (and the workflow contents, checks and pull-requests write permission), and Kern verifies every push to a kern/ branch again on its own.".into());
        return (false, vec![v]);
    }
    if let Some(rest) = r.strip_prefix("The pushed change and ") && let Some((base, rest)) = rest.split_once(" both changed: ")
        && let Some(files) = rest.strip_suffix(&format!(". Merge {base} into the branch, resolve the conflicts and push again")) {
        return (true, vec![fix("base-conflict", None, format!("Merge {} into this branch, resolve the conflicts in {} and push again.", code(base), codes(files.split(", "))))]);
    }
    if r.starts_with("The pull request has nothing to merge into ") { return human("nothing-to-merge", "Everything on this branch is already on the base branch. A human can close this pull request."); }
    if r.starts_with("The agent ran out of memory") || r.starts_with("The agent did not finish within") { return human("agent-limits", "The agent ran out of memory or time. A human has to raise the limit or narrow the task."); }
    if r.starts_with("The test suite already fails before the change: ") { return human("tests-failing", "The test suite already failed before this change. A human has to fix it on the base branch first."); }
    if r.starts_with("Kern could not run the test suite on ") || r.starts_with("Kern could not read the test results of the old code: ") || r.starts_with("The test suite ran no tests on the new code")
        || r.starts_with("Kern could not check the test changes: ") {
        return human("test-run", "Kern could not run or read the project's tests. A human has to check the task contract's test settings and the verifier.");
    }
    let rule = if ["ttestation", "erifier", "signature", "signing key", "check set"].iter().any(|w| r.contains(w)) { "verification" } else { "environment" };
    human(rule, "This is not something the agent can fix. A human needs to look at the reason.")
}

pub fn explain(reason: &str, contract: Option<&Contract>) -> Value {
    let (fixable, violations) = classify(&unwrap_reason(reason), contract);
    let list: Vec<Value> = violations.into_iter().map(|v| {
        let mut m = Map::new();
        m.insert("rule".into(), json!(v.rule));
        for (k, x) in [("path", v.path), ("detail", v.detail)] { if let Some(x) = x { m.insert(k.into(), json!(x)); } }
        m.insert("instruction".into(), json!(v.instruction));
        if let Some(a) = v.allow { m.insert("allow".into(), json!(a)); }
        Value::Object(m)
    }).collect();
    json!({ "schema": SCHEMA, "status": "BLOCKED", "reason": reason, "agent_fixable": fixable, "violations": list })
}
/// The copy an agent is handed: instructions only, without the raw reason or the hints on how a human could allow the change.
pub fn for_agent(feedback: &Value) -> Value {
    let mut f = feedback.clone();
    if let Some(o) = f.as_object_mut() { o.remove("reason"); }
    for v in f["violations"].as_array_mut().into_iter().flatten() { if let Some(o) = v.as_object_mut() { o.remove("allow"); } }
    f
}

fn hex(s: &str) -> String { s.chars().filter(char::is_ascii_hexdigit).take(64).collect() }
pub fn comment_body(feedback: &Value, attempt: u64, max: u64, mention: Option<&str>, head: &str) -> String {
    let fixable = feedback["agent_fixable"] == true;
    let title = if attempt <= max { format!("### Kern blocked this change (attempt {attempt} of {max})") } else { format!("### Kern blocked this change (attempt {attempt}; agent retries used up)") };
    let lead = match (fixable, attempt <= max, mention) {
        (true, true, Some(m)) => format!("{m} please fix the following and push again:"),
        (true, true, None) => "To get this change accepted:".to_string(),
        (true, false, _) => format!("Kern stopped asking the agent after {max} attempts. A human needs to look at this change:"),
        (false, _, _) => "This is not something the agent can fix. A human needs to look at it:".to_string(),
    };
    let violations = feedback["violations"].as_array().cloned().unwrap_or_default();
    let details = if fixable {
        violations.iter().filter_map(|v| v["instruction"].as_str()).map(|i| format!("- {i}\n")).collect::<String>()
    } else {
        format!("````text\n{}\n````\n", feedback["reason"].as_str().unwrap_or("").replace('`', "'"))
    };
    let machine = serde_json::to_string_pretty(&for_agent(feedback)).unwrap_or_default().replace('`', "\\u0060");
    format!("{title}\n\n{lead}\n\n{details}\nEarlier rejected attempts stay in this thread, so reviewers can see what was tried.\n\n<details><summary>Machine-readable verdict ({SCHEMA})</summary>\n\n````json\n{machine}\n````\n\n</details>\n\n{MARKER}{} -->\n", hex(head))
}

fn mention(raw: &str) -> bool {
    raw.len() <= 80 && raw.strip_prefix('@').is_some_and(|m| m.split('/').count() <= 2 && m.split('/').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.[]".contains(&b))))
}
fn marked(body: &str, prefix: &str) -> Vec<String> {
    body.match_indices(prefix).filter_map(|(i, _)| {
        let rest = &body[i + prefix.len()..];
        let h: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
        ((h.len() == 40 || h.len() == 64) && rest[h.len()..].starts_with(" -->")).then(|| h.to_ascii_lowercase())
    }).collect()
}
#[derive(Default)]
struct History { blocked: BTreeSet<String>, passed: BTreeSet<String> }
struct Thread { api: Api, path: String }
impl Thread {
    fn from(event: &Value) -> R<Thread> {
        let repo = event["repository"]["full_name"].as_str().or(event["pull_request"]["base"]["repo"]["full_name"].as_str()).unwrap_or("");
        ensure!(crate::github::repository(repo), "The event names no repository");
        let number = event["pull_request"]["number"].as_u64().ok_or_else(|| Denied("The event names no pull request number".into()))?;
        Ok(Thread { api: Api::from_env("post feedback comments (the workflow needs pull-requests: write)")?, path: format!("/repos/{repo}/issues/{number}/comments") })
    }
    fn history(&self, author: &str) -> R<History> {
        let mut h = History::default();
        for page in 1..=10 {
            let list = self.api.get(&format!("{}?per_page=100&page={page}", self.path))?;
            let items = list.as_array().cloned().unwrap_or_default();
            for body in items.iter().filter(|c| c["user"]["login"] == author).filter_map(|c| c["body"].as_str()) {
                h.blocked.extend(marked(body, MARKER));
                h.passed.extend(marked(body, RESOLVED));
            }
            if items.len() < 100 { break; }
        }
        Ok(h)
    }
    fn post(&self, body: &str) -> R<Value> { self.api.post(&self.path, &json!({ "body": body })) }
}

/// The note on the pull request after Kern verified a push again and moved the branch to the verified commit.
fn reverified(r: &Value, earlier: u64, head: &str) -> String {
    let short = |k: &str| hex(r[k].as_str().unwrap_or("")).chars().take(12).collect::<String>();
    let after = if earlier > 0 { format!(" It passed {}.", rejected(earlier)) } else { String::new() };
    let base_ref: String = r["base_ref"].as_str().unwrap_or("").chars().filter(|c| c.is_ascii_alphanumeric() || "._/-".contains(*c)).collect();
    format!("### Kern verified the change again\n\nAfter the push of `{}`, Kern verified the change again on `{base_ref}` (`{}`) and moved this branch to one commit, `{}`, which the new proof covers.{after} Fetch the branch before pushing more changes.\n\n{RESOLVED}{head} -->\n",
        short("pushed"), short("base"), short("commit"))
}
pub fn publish(event: &Value, result: &R<Value>) -> String {
    let status = match result { Ok(v) => v["status"].as_str().unwrap_or("UNKNOWN").to_string(), Err(_) => "DENIED".to_string() };
    let mut notes = Vec::new();
    let max = match env("KERN_MAX_ATTEMPTS").map(|v| v.parse::<u64>()) {
        None => 3,
        Some(Ok(n)) if (1..=20).contains(&n) => n,
        _ => { notes.push("KERN_MAX_ATTEMPTS must be 1 to 20; using 3.".to_string()); 3 }
    };
    let who = env("KERN_AGENT_MENTION").filter(|m| {
        let ok = mention(m);
        if !ok { notes.push("KERN_AGENT_MENTION must look like @name or @org/team; the agent was not mentioned.".into()); }
        ok
    });
    let head = event["pull_request"]["head"]["sha"].as_str().map(str::to_ascii_lowercase).filter(|h| (h.len() == 40 || h.len() == 64) && h.bytes().all(|b| b.is_ascii_hexdigit()));
    let github = (env("KERN_PR_COMMENTS").as_deref() == Some("true") && status != "SKIPPED" && event["pull_request"].is_object()).then(|| {
        head.as_ref().ok_or_else(|| Denied("The event names no valid head commit".into())).and_then(|_| Thread::from(event))
    });
    let history = match &github {
        Some(Ok(gh)) => match gh.history(&env("KERN_COMMENT_AUTHOR").unwrap_or_else(|| "github-actions[bot]".into())) {
            Ok(h) => Some(h),
            Err(e) => { notes.push(format!("Kern could not post its feedback comment: {e}")); None }
        },
        Some(Err(e)) => { notes.push(format!("Kern could not post its feedback comment: {e}")); None }
        None => None,
    };
    let head = head.unwrap_or_default();
    // An attempt is a commit Kern rejected: re-runs and edits on the same commit are not new attempts.
    let earlier = history.as_ref().map_or(0, |h| h.blocked.iter().filter(|b| **b != head).count() as u64);
    let feedback = result.as_ref().err().map(|e| {
        let mut f = explain(&e.0, None);
        f["attempt"] = json!(earlier + 1);
        f["max_attempts"] = json!(max);
        f
    });
    let mut outputs = format!("status={status}\n");
    if let (Some(f), Some(path)) = (&feedback, env("KERN_FEEDBACK_PATH")) {
        match std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&for_agent(f)).unwrap_or_default())) {
            Ok(()) => outputs.push_str(&format!("feedback={path}\n")),
            Err(e) => notes.push(format!("Kern could not write {path}: {e}")),
        }
    }
    if let Some(out) = env("GITHUB_OUTPUT") {
        let _ = std::fs::OpenOptions::new().append(true).create(true).open(out).and_then(|mut f| std::io::Write::write_all(&mut f, outputs.as_bytes()));
    }
    if let (Some(Ok(gh)), Some(h)) = (&github, &history) {
        let body = match &feedback {
            Some(_) if h.blocked.contains(&head) => { notes.push("Kern already commented on this commit, so it added no new comment.".into()); None }
            Some(f) => Some(comment_body(f, earlier + 1, max, who.as_deref(), &head)),
            None if status == "PASS" && !h.passed.contains(&head) && result.as_ref().is_ok_and(|r| r["reverified"].is_object()) => result.as_ref().ok().map(|r| reverified(&r["reverified"], earlier, &head)),
            None if status == "PASS" && earlier > 0 && !h.passed.contains(&head) => Some(format!("### Kern verified this change\n\nIt passed {}; the earlier attempts stay in this thread.\n\n{RESOLVED}{head} -->\n", rejected(earlier))),
            None => None,
        };
        if let Some(b) = body && let Err(e) = gh.post(&b) { notes.push(format!("Kern could not post its feedback comment: {e}")); }
    }
    if status == "PASS" && earlier > 0 { notes.push(format!("Verified {}.", rejected(earlier))); }
    if let Some(f) = &feedback {
        let violations = f["violations"].as_array().cloned().unwrap_or_default();
        let lines: String = if f["agent_fixable"] == true {
            violations.iter().filter_map(|v| v["instruction"].as_str()).map(|i| format!("- {i}\n")).collect()
        } else { "- This is not something the agent can fix; a human needs to look at it.\n".into() };
        notes.push(format!("#### What has to change\n\n{lines}"));
        let allow: String = violations.iter().filter_map(|v| v["allow"].as_str()).collect::<BTreeSet<_>>().into_iter().map(|a| format!("{}\n", a.replace('`', "'"))).collect();
        if !allow.is_empty() { notes.push(format!("<details><summary>For maintainers: if this is intended</summary>\n\n````text\n{allow}````\n\n</details>")); }
    }
    notes.iter().map(|n| format!("{n}\n\n")).collect()
}
