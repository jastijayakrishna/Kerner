use crate::github::Api;
use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};
use std::path::Path;

pub const MARK: &str = "<!-- kern-receipt -->";
pub const SEAL_MARK: &str = "<!-- kern-seal ";
fn author() -> String { crate::github::env("KERN_COMMENT_AUTHOR").unwrap_or_else(|| "github-actions[bot]".into()) }
const SHOWN: usize = 15;
const RECEIPT_BYTES_MAX: usize = 60_000;
const DETAILS_FRAME_BYTES: usize = 200;

fn strings(v: &Value) -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() }
fn item(kind: &str, what: String) -> Value { json!({ "kind": kind, "what": what }) }
fn tidy(key: &str) -> String { key.split(" > ").map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" > ") }
fn file_of(key: &str) -> &str { key.split_once(" > ").map_or(key, |(f, _)| f) }
fn routine_deps(rules: &Value) -> bool {
    let deps: Vec<&Value> = rules["dependencies"].as_array().into_iter().flatten().collect();
    let moves: Vec<&Value> = deps.iter().flat_map(|d| d["moves"].as_array().into_iter().flatten()).collect();
    let read = |d: &&Value| match d["path"].as_str().unwrap_or("").rsplit('/').next() {
        Some("package.json") => true,
        Some("package-lock.json" | "npm-shrinkwrap.json") => d["moves"].is_array(),
        _ => false,
    };
    let settled = |m: &&Value| ["routine", "removed", "internal"].iter().any(|a| m["age"] == *a);
    !moves.is_empty() && deps.iter().all(read) && moves.iter().all(settled) && deps.iter().all(|d| d["fields"].as_array().is_none_or(|f| f.is_empty()))
}
fn per_file(kind: &str, keys: Vec<String>, one: &str, many: &str) -> Vec<Value> {
    let mut files: Vec<(String, Vec<String>)> = Vec::new();
    for k in keys {
        let f = k.split_once(" > ").map_or(k.as_str(), |(f, _)| f).to_string();
        match files.iter_mut().find(|(x, _)| *x == f) { Some((_, v)) => v.push(k), None => files.push((f, vec![k])) }
    }
    files.into_iter().flat_map(|(f, ks)| {
        if ks.len() < 3 { return ks.iter().map(|k| item(kind, format!("{one}: {}", code(k, 200)))).collect::<Vec<_>>(); }
        let titles: Vec<String> = ks.iter().take(3).map(|k| code(k.split_once(" > ").map_or(k.as_str(), |(_, t)| t), 120)).collect();
        vec![item(kind, format!("{} {many} in {}: {}{}", ks.len(), code(&f, 160), titles.join(", "), if ks.len() > 3 { ", …" } else { "" }))]
    }).collect()
}

fn behaviour_items(report: &Value) -> (Vec<String>, Vec<String>) {
    if !report.is_object() { return (Vec::new(), Vec::new()); }
    let items = crate::drive::difference_items(report);
    let has_record = items.iter().any(|(_, rec)| *rec);
    let sorted = &report["sorted"];
    let (mut described, mut surprises) = (Vec::new(), Vec::new());
    for (i, (line, rec)) in items.iter().enumerate() {
        if has_record && !*rec { continue; }
        let says = sorted[format!("D{}", i + 1)]["says"].as_str();
        if says == Some("yes") { described.push(line.clone()) } else { surprises.push(line.clone()) }
    }
    (described, surprises)
}

const OUTPUT_FOLDERS: [&str; 12] = ["dist", "out", ".nuxt", ".next", ".svelte-kit", ".output", "coverage", ".turbo", ".vite", ".cache", ".parcel-cache", "storybook-static"];
fn generated(path: &str) -> bool { path.split('/').any(|s| OUTPUT_FOLDERS.contains(&s)) }
pub fn shown_power(p: &Value) -> bool {
    use crate::capability::{Reach, reach};
    if !crate::suite::is_test_path(p["path"].as_str().unwrap_or("")) { return true; }
    let (added, has) = (strings(&p["added"]), strings(&p["has"]));
    let after: Vec<String> = added.iter().chain(&has).cloned().collect();
    let any = |caps: &[String], want: &[Reach]| caps.iter().any(|c| want.contains(&reach(c)));
    any(&added, &[Reach::Anything])
        || (any(&added, &[Reach::Secrets, Reach::Network]) && any(&after, &[Reach::Secrets, Reach::Anything]) && any(&after, &[Reach::Network, Reach::Anything]))
}
fn title(key: &str) -> &str { key.split_once(" > ").map_or(key, |(_, t)| t) }
fn by_file<T>(list: Vec<(String, T)>) -> Vec<(String, Vec<(String, T)>)> {
    let mut files: Vec<(String, Vec<(String, T)>)> = Vec::new();
    for x in list {
        let f = file_of(&x.0).to_string();
        match files.iter_mut().find(|(n, _)| *n == f) { Some((_, v)) => v.push(x), None => files.push((f, vec![x])) }
    }
    files
}
fn moved_values(changed: Vec<(String, (String, String))>) -> Vec<Value> {
    by_file(changed).into_iter().flat_map(|(f, list)| {
        if list.len() < 3 {
            return list.iter().map(|(k, (was, now))| item("test-value", format!("{}: the change edits this test, and what it expects moves from {} to {}", code(k, 200), code(was, 160), code(now, 160)))).collect::<Vec<_>>();
        }
        let (k, (was, now)) = &list[0];
        vec![item("test-value", format!("{} tests in {} expect other values now, as the change edits them; for example {}: {} becomes {}", list.len(), code(&f, 160), code(title(k), 120), code(was, 160), code(now, 160)))]
    }).collect()
}
fn old_tests_unrun(harness: Vec<(String, String)>) -> Vec<Value> {
    by_file(harness).into_iter().flat_map(|(f, list)| {
        if list.len() < 3 {
            return list.iter().map(|(k, why)| item("edited-unchecked", format!("{}: Kern could not run the old version of this edited test against the new code ({}), so it cannot say what the change did to it", code(k, 200), plain(why, 300)))).collect::<Vec<_>>();
        }
        vec![item("edited-unchecked", format!("{} edited tests in {}: Kern could not run their old versions against the new code ({}), so it cannot say what the change did to them", list.len(), code(&f, 160), plain(&list[0].1, 300)))]
    }).collect()
}
fn rewritten_items(tests: &Value, rules: &Value) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let (mut look, mut harness, mut unread, mut changed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for k in strings(&tests["rewritten"]) {
        let r = &tests["polarity"][&k];
        let (was, now) = (r["was"].as_str().unwrap_or(""), r["now"].as_str().unwrap_or(""));
        let values = if was.is_empty() && now.is_empty() { String::new() } else { format!(" (it expected {}, and the new code gives {})", code(was, 160), code(now, 160)) };
        match r["kind"].as_str() {
            Some("opened") => look.push(item("test-rewritten", format!("The change edits this test, and its old version shows the new code accepting what the old code refused: {}{values}", code(&k, 200)))),
            Some("flipped") => look.push(item("test-rewritten", format!("The change edits this test, and its old version fails on the new code because a yes or no, a something or nothing, or the shape of the result changed: {}{values}", code(&k, 200)))),
            Some("changed") => changed.push((k.clone(), (was.to_string(), now.to_string()))),
            Some("harness") => harness.push((k.clone(), r["why"].as_str().unwrap_or("its setup no longer fits the new code").to_string())),
            _ => unread.push(k.clone()),
        }
    }
    look.extend(per_file("test-rewritten", unread, "The change edits this test, and its old version fails on the new code", "tests whose old versions fail on the new code (the change edits them)"));
    if let Some(words) = rules["says_no_change"].as_str().filter(|w| !w.trim().is_empty()) && let Some((k, (was, now))) = changed.first() {
        look.push(item("says-no-change", format!("The pull request says it does not change behaviour ({}), but the old versions of {} get other values on the new code; for example {}: {} becomes {}",
            code(words, 120), plural(changed.len(), "edited test", "edited tests"), code(k, 200), code(was, 160), code(now, 160))));
    }
    (look, old_tests_unrun(harness), moved_values(changed))
}

#[derive(Default)]
struct Found { blocked: Vec<Value>, look: Vec<Value>, unverified: Vec<Value>, notes: Vec<Value>, cannot_run: Vec<Value> }
fn removed_tests(rules: &Value) -> bool {
    match rules["deleted_tests"].as_array() {
        Some(d) => !d.is_empty(),
        None => rules["changed"].as_array().into_iter().flatten().any(|c| c["operation"] == "delete" && c["path"].as_str().is_some_and(crate::suite::is_test_path)),
    }
}
fn deleted_files(rules: &Value) -> std::collections::BTreeSet<String> {
    rules["deleted_tests"].as_array().into_iter().flatten().filter_map(|x| x["path"].as_str().map(str::to_string)).collect()
}
fn new_test_file(t: &str, tests: &Value, new_tests: &mut Vec<Value>, cannot_run: &mut Vec<Value>) {
    let why = tests["problem_why"][t].as_str().map(|w| format!(": {}", plain(w, 400))).unwrap_or_default();
    if tests["problem_why"][t].as_str().is_some_and(crate::suite::sandbox_error) { cannot_run.push(json!({ "what": format!("{}: a test file this change adds needs something Kern's sandbox does not have{why}", code(t, 200)) })); }
    else { new_tests.push(item("new-test-fails", format!("A test file this change adds does not run here: {}{why}", code(t, 200)))); }
}
fn test_file_items(f: &mut Found, rules: &Value, tests: &Value) -> (Vec<Value>, Vec<String>) {
    let changed = rules["changed"].as_array().cloned().unwrap_or_default();
    let tests_deleted = removed_tests(rules);
    let passing_lost = tests_deleted && tests["gone"].as_array().is_some_and(|g| !g.is_empty());
    let added = |p: &str| !passing_lost && changed.iter().any(|c| c["path"] == p && c["operation"] == "add");
    let mut new_tests = Vec::new();
    f.blocked.extend(per_file("test-broke", strings(&tests["broke"]), "A test that passed before now fails", "tests that passed before now fail"));
    for x in tests["run_broke"].as_array().into_iter().flatten() {
        let place = x["dir"].as_str().filter(|d| !d.is_empty()).map(|d| format!(" in {}", code(d, 160))).unwrap_or_default();
        f.blocked.push(item("test-run-broke", format!("The tests{place} run on the old code but give no results on the new code: {}", plain(x["why"].as_str().unwrap_or(""), 400))));
    }
    for t in strings(&tests["new_problems"]) {
        if added(file_of(&t)) { new_test_file(&t, tests, &mut new_tests, &mut f.cannot_run); continue; }
        let why = tests["problem_why"][&t].as_str().map(|w| format!(": {}", plain(w, 400))).unwrap_or_default();
        f.blocked.push(item("test-file", format!("A test file no longer runs, or a hook now fails: {}{why}", code(&t, 200))));
    }
    let (unproven_added, unproven): (Vec<String>, Vec<String>) = strings(&tests["unproven"]).into_iter().partition(|t| added(file_of(t)));
    for t in unproven_added { new_test_file(&t, tests, &mut new_tests, &mut f.cannot_run); }
    let mut needs: Vec<(String, usize)> = Vec::new();
    for t in strings(&tests["unrunnable"]) {
        match needs.iter_mut().find(|(n, _)| n == file_of(&t)) { Some((_, n)) => *n += 1, None => needs.push((file_of(&t).to_string(), 1)) }
    }
    f.cannot_run.extend(needs.iter().map(|(p, n)| json!({ "what": format!("{}: {} this change adds need{} something Kern's sandbox does not have (the network, a program or a native module), so Kern could not run {}",
        code(p, 160), plural(*n, "test", "tests"), if *n == 1 { "s" } else { "" }, if *n == 1 { "it" } else { "them" }) })));
    for x in rules["tricks"].as_array().into_iter().flatten() {
        let path = x["path"].as_str().unwrap_or("");
        let reach = x["why"].as_str().is_some_and(|w| w.starts_with("Code outside the tests"));
        let added_file = x["operation"] == "add" && !tests_deleted;
        if added_file && reach { new_tests.push(item("new-runner-reach", format!("{}: a file this change adds, outside the test folders, uses the test runner; if it is a test, Kern does not run it", code(path, 160)))); }
        else if reach { f.look.push(item("runner-reach", format!("{}: code outside the test folders uses the test runner; that is fine in test support code and a way to fake results in shipped code ({})", code(path, 160), plain(x["why"].as_str().unwrap_or(""), 400)))); }
        else if added_file { new_tests.push(item("new-test-skips", format!("{}: a test file this change adds skips, focuses or inverts some of its own tests", code(path, 160)))); }
        else { f.blocked.push(item("trick", format!("{}: {}", code(path, 160), plain(x["why"].as_str().unwrap_or(""), 400)))); }
    }
    (new_tests, unproven)
}
fn behaviour_looks(f: &mut Found, behaviour: &Value) -> (bool, Vec<String>, Vec<String>, bool) {
    let summary = !behaviour["report"].is_object() && behaviour["not_mentioned"].is_array();
    let (described, surprises) = if summary { (strings(&behaviour["described"]), strings(&behaviour["not_mentioned"])) } else { behaviour_items(&behaviour["report"]) };
    let sorted = behaviour["report"]["sorted"].is_object() || behaviour["sorted"] == true;
    for s in &surprises { f.look.push(item(if sorted { "not-mentioned" } else { "difference" }, if sorted { format!("Not mentioned by the pull request: {s}") } else { format!("Behaves differently: {s}") })); }
    for s in &described { f.look.push(item("described", format!("The pull request says it changes this, which only a person can confirm: {s}"))); }
    (summary, described, surprises, sorted)
}
fn change_items(f: &mut Found, rules: &Value, tests: &Value, new_tests: Vec<Value>) {
    for p in rules["powers"].as_array().into_iter().flatten().filter(|p| shown_power(p)) {
        let caps: Vec<String> = strings(&p["added"]).iter().map(|c| code(c, 80)).collect();
        f.look.push(item("power", format!("{} can now do something it could not before: {}", code(p["path"].as_str().unwrap_or(""), 160), caps.join(", "))));
    }
    let (look, unverified, notes) = rewritten_items(tests, rules);
    f.look.extend(look);
    f.unverified.extend(unverified);
    f.notes.extend(notes);
    for x in tests["edited_unchecked"].as_array().into_iter().flatten() {
        f.unverified.push(item("edited-unchecked", format!("{}: Kern could not run the old version of this edited test file on the new code, so it cannot tell whether the change rewrote what the test expects ({})",
            code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""), 400))));
    }
    for p in strings(&tests["test_data"]).into_iter().filter(|p| !crate::check::documentation(p)) {
        f.look.push(item("test-data", format!("{}: the change edits a file the tests use (data, a helper or a mock), so what they expect may have changed", code(&p, 160))));
    }
    f.look.extend(new_tests);
    let mut reach: Vec<(String, bool, bool)> = Vec::new();
    for x in tests["faults"].as_array().into_iter().flatten() {
        let path = x["path"].as_str().unwrap_or("").to_string();
        let (never, crashed) = (["unreached", "untested"].iter().any(|o| x["outcome"] == *o), x["outcome"] == "unreached");
        match reach.iter_mut().find(|(p, _, _)| *p == path) { Some((_, all, any)) => { *all &= never; *any |= crashed; } None => reach.push((path, never, crashed)) }
    }
    for (path, _, _) in reach.iter().filter(|(p, all, any)| *all && *any && !p.is_empty()) {
        f.unverified.push(item("untested-change", format!("{}: no test that runs here reaches the code this change edits in it (Kern put faults in the changed code, and no test loaded them), so the tests say nothing about it", code(path, 160))));
    }
}
fn rule_items(f: &mut Found, d: &Value, rules: &Value, tests: &Value) {
    let gone_all = strings(&tests["gone"]);
    for p in &deleted_files(rules) {
        let n = gone_all.iter().filter(|g| file_of(g) == p.as_str()).count();
        let lost = if n == 0 { String::new() } else { format!(" ({} that ran before no longer run{})", plural(n, "test", "tests"), if n == 1 { "s" } else { "" }) };
        f.look.push(item("test-deleted", format!("{}: a test file this change deletes{lost}", code(p, 160))));
    }
    for x in rules["inline_csp"].as_array().into_iter().flatten() {
        let what = match x["kind"].as_str().unwrap_or("") { "style" => "an inline <style>", "style-attr" => "inline style attributes", "script" => "an inline <script>", _ => "inline event handlers (such as onclick)" };
        f.look.push(item("inline-blocked", format!("{} adds {what}, but {} sends a Content-Security-Policy ({}) that does not allow it, so browsers will refuse it",
            code(x["path"].as_str().unwrap_or(""), 160), code(x["policy_path"].as_str().unwrap_or(""), 160), code(x["policy"].as_str().unwrap_or(""), 200))));
    }
    for x in rules["public_api"].as_array().into_iter().flatten() {
        let names = strings(&x["removed"]);
        let shown: Vec<String> = names.iter().take(SHOWN).map(|n| code(n, 80)).collect();
        let more = if names.len() > SHOWN { format!(" and {} more", names.len() - SHOWN) } else { String::new() };
        f.look.push(item("public-api", format!("{} no longer exports {}{more} ({}), so code that imports {} breaks",
            code(x["package"].as_str().unwrap_or(""), 120), shown.join(", "), code(x["entry"].as_str().unwrap_or(""), 160), if names.len() == 1 { "it" } else { "them" })));
    }
    for x in rules["node_versions"].as_array().into_iter().flatten() {
        let features: Vec<String> = x["features"].as_array().into_iter().flatten().take(SHOWN)
            .map(|n| format!("{} (Node {})", code(n["name"].as_str().unwrap_or(""), 80), plain(n["since"].as_str().unwrap_or(""), 400))).collect();
        f.look.push(item("node-version", format!("{} now uses {}, but its package says it runs on Node {} (engines.node), where that is missing or needs a flag",
            code(x["path"].as_str().unwrap_or(""), 160), features.join(", "), code(x["supports"].as_str().unwrap_or(""), 80))));
    }
    let gone: std::collections::BTreeSet<String> = gone_all.iter().map(|g| tidy(g)).collect();
    for x in rules["weakened"].as_array().into_iter().flatten() {
        let path = x["path"].as_str().unwrap_or("");
        let listed = |key: &str| -> String {
            let names = strings(&x[key]);
            let shown: Vec<String> = names.iter().take(SHOWN).map(|n| code(n, 160)).collect();
            format!("{}{}", shown.join(", "), if names.len() > SHOWN { format!(" and {} more", names.len() - SHOWN) } else { String::new() })
        };
        let removed = x["removed"].as_u64().unwrap_or(0);
        let covered = strings(&x["missing"]).iter().all(|m| gone.contains(&tidy(&format!("{path} > {m}"))));
        if removed > 0 && !covered {
            f.look.push(item("tests-removed", format!("{}: the change removes {removed} test{} from this existing test file, so {} no longer check{} anything: {}",
                code(path, 160), if removed == 1 { "" } else { "s" }, if removed == 1 { "it" } else { "they" }, if removed == 1 { "s" } else { "" }, listed("missing"))));
        }
        if !strings(&x["emptied"]).is_empty() { f.look.push(item("tests-emptied", format!("{}: tests that checked something now check nothing: {}", code(path, 160), listed("emptied")))); }
        if !strings(&x["loosened"]).is_empty() {
            f.look.push(item("tests-loosened", format!("{}: checks were loosened to ones that pass for almost any value (such as toBeDefined or toBeTruthy) in: {}", code(path, 160), listed("loosened"))));
        }
    }
    for x in d["unmeasured"].as_array().into_iter().flatten().filter(|x| x["why"] == "a submodule") {
        f.look.push(item("submodule", format!("{}: a submodule this change points at other code", code(x["path"].as_str().unwrap_or(""), 160))));
    }
    for x in rules["not_checked"].as_array().into_iter().flatten().filter(|x| x["code"] == "js" || x["path"].as_str().is_some_and(crate::capability::is_code)) {
        f.unverified.push(item("not-checked-code", format!("{}: Kern could not read this changed code, so its rules were not applied ({})", code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""), 400))));
    }
}
fn type_items(f: &mut Found, tests: &Value) {
    for p in tests["types"]["projects"].as_array().into_iter().flatten().filter(|p| p["result"] == "new-errors") {
        let shown: Vec<String> = p["added"].as_array().into_iter().flatten().take(3)
            .map(|d| code(&format!("{}:{} {} {}", d["file"].as_str().unwrap_or(""), d["line"], d["code"].as_str().unwrap_or(""), d["message"].as_str().unwrap_or("")), 200)).collect();
        let count = p["count"].as_u64().unwrap_or(0) as usize;
        let what = format!("The change brings {} to {}, which type-checked with none on the old code: {}{}", plural(count, "new TypeScript error", "new TypeScript errors"),
            code(p["config"].as_str().unwrap_or(""), 160), shown.join(", "), if count > shown.len() { ", …" } else { "" });
        match p["uneven"].as_str() {
            _ if p["generated"] == true && p["enforced"] == true => f.unverified.push(item("types-new", format!("{what} (some are about modules, exports or names that a build step may generate, and Kern did not run it, so it cannot say whether the change caused them)"))),
            _ if p["enforced"] != true => f.look.push(item("types-new", format!("{what} (the project's scripts do not run tsc, so Kern does not block on it)"))),
            Some(why) => f.unverified.push(item("types-new", format!("{what} (Kern cannot compare the two sides fairly: {})", plain(why, 400)))),
            None => f.blocked.push(item("types-broke", what)),
        }
    }
}
fn run_items(f: &mut Found, rules: &Value, tests: &Value, unproven: Vec<String>) {
    f.look.extend(per_file("new-test-fails", strings(&tests["new_failing"]), "A test this change adds fails on the new code", "tests this change adds fail on the new code"));
    let wrote: Vec<String> = strings(&tests["wrote_code"]).into_iter().filter(|p| !generated(p)).collect();
    if !wrote.is_empty() {
        let shown: Vec<String> = wrote.iter().take(SHOWN).map(|p| code(p, 160)).collect();
        let more = if wrote.len() > SHOWN { format!(" and {} more", wrote.len() - SHOWN) } else { String::new() };
        f.look.push(item("wrote-code", format!("Tests on the new code wrote code into the project while they ran: {}{more}", shown.join(", "))));
    }
    let (deleted, rewritten) = (deleted_files(rules), strings(&tests["rewritten"]).into_iter().collect::<std::collections::BTreeSet<String>>());
    let gone: Vec<String> = strings(&tests["gone"]).into_iter().filter(|g| !deleted.contains(file_of(g)) && !rewritten.contains(g)).collect();
    f.look.extend(per_file("test-gone", gone, "A test that ran before no longer runs", "tests that ran before no longer run"));
    for x in tests["run_unconfirmed"].as_array().into_iter().flatten() { f.unverified.push(item("test-unconfirmed", plain(x["why"].as_str().unwrap_or(""), 400))); }
    let reasons = strings(&tests["uneven_why"]);
    let because = if reasons.is_empty() { String::new() } else { format!(" ({})", reasons.iter().map(|r| plain(r, 400)).collect::<Vec<_>>().join("; ")) };
    f.unverified.extend(per_file("test-uneven", strings(&tests["uneven"]), &format!("This fails on the new code, but Kern did not set up both sides the same way{because}, so it cannot say whether the change broke it"),
        &format!("tests fail on the new code, but Kern did not set up both sides the same way{because}, so it cannot say whether the change broke them")));
    f.unverified.extend(per_file("test-unproven", unproven, "This test file fails on the new code, and Kern has no record of it passing on the old code, so it cannot say whether the change broke it",
        "test files fail on the new code, and Kern has no record of them passing on the old code, so it cannot say whether the change broke them"));
    type_items(f, tests);
    f.unverified.extend(per_file("test-sandbox", strings(&tests["sandbox"]), "This fails on the new code with an error from the sandbox, not the test (no network, a missing program or native module, or a crashed process), so Kern cannot tell whether the change broke it",
        "tests fail on the new code with an error from the sandbox, not the test (no network, a missing program or native module, or a crashed process), so Kern cannot tell whether the change broke them"));
    f.unverified.extend(per_file("test-unsteady", strings(&tests["unsteady"]), "This failed on the new code, but not in every run of it, or tests in the same file passed and failed by chance during this check, so Kern cannot say whether the change broke it",
        "tests failed on the new code, but not in every run of it, or tests in the same files passed and failed by chance during this check, so Kern cannot say whether the change broke them"));
    f.unverified.extend(per_file("test-together", strings(&tests["together"]), "This fails when the whole suite runs, but not when its file runs alone, so it may depend on other tests and Kern cannot say whether the change broke it",
        "tests fail when the whole suite runs, but not when their file runs alone, so they may depend on other tests and Kern cannot say whether the change broke them"));
    f.unverified.extend(per_file("test-unconfirmed", strings(&tests["unconfirmed"]), "This failed once, and Kern could not run it again to confirm it", "tests failed once, and Kern could not run them again to confirm"));
    f.look.extend(per_file("test-skipped", strings(&tests["newly_skipped"]), "A test that passed before is now skipped", "tests that passed before are now skipped"));
}
fn steering_items(f: &mut Found, rules: &Value) {
    let path_of = |x: &Value| code(x["path"].as_str().unwrap_or(""), 160);
    for (key, kind, says) in [("settings", "settings", "changes the test runner's settings, which decide which tests run and how their results are read"), ("protected", "protected", "is a CI, secret or configuration file")] {
        for x in rules[key].as_array().into_iter().flatten() { f.look.push(item(kind, format!("{} {says}", path_of(x)))); }
    }
    let deps: Vec<&Value> = rules["dependencies"].as_array().into_iter().flatten().collect();
    for m in deps.iter().flat_map(|d| d["moves"].as_array().into_iter().flatten()).filter(|m| m["age"] == "fresh") {
        let days = m["days"].as_u64().unwrap_or(0);
        f.look.push(item("fresh-release", format!("{} came out {} before this change; brand-new releases are where hijacked packages show up, so check it or wait until it is {} days old",
            code(&format!("{}@{}", m["name"].as_str().unwrap_or(""), m["version"].as_str().unwrap_or("")), 120),
            if days == 0 { "less than a day".to_string() } else if days == 1 { "1 day".to_string() } else { format!("{days} days") }, crate::deps::FRESH_DAYS)));
    }
    if !deps.is_empty() && !routine_deps(rules) {
        let moved: Vec<String> = deps.iter().flat_map(|d| strings(&d["packages"])).collect();
        let fields: std::collections::BTreeSet<String> = deps.iter().flat_map(|d| strings(&d["fields"])).collect();
        let only_fields = moved.is_empty() && !fields.is_empty() && deps.iter().all(|d| d["path"].as_str().is_some_and(|p| p.rsplit('/').next() == Some("package.json")));
        let mut what = String::from(if only_fields { "package.json changes" } else { "The project's dependencies change" });
        if !moved.is_empty() {
            what += &format!(": {}", moved.iter().take(SHOWN).map(|m| code(m, 120)).collect::<Vec<_>>().join(", "));
            if moved.len() > SHOWN { what += &format!(" and {} more", moved.len() - SHOWN); }
        }
        if !fields.is_empty() {
            let shown: Vec<String> = fields.iter().take(SHOWN).map(|n| code(n, 80)).collect();
            what += &format!("{}{}", if only_fields { " " } else { "; package.json also changes " }, shown.join(", "));
            if fields.len() > SHOWN { what += &format!(" and {} more", fields.len() - SHOWN); }
        }
        f.look.push(item("dependencies", format!("{what} ({})", deps.iter().map(|d| path_of(d)).collect::<Vec<_>>().join(", "))));
    }
    for x in rules["lockfile"].as_array().into_iter().flatten() { f.look.push(item("lockfile", format!("{}: {}", path_of(x), plain(x["why"].as_str().unwrap_or(""), 400)))); }
    for x in rules["agent_notes"].as_array().into_iter().flatten() { f.look.push(item("agent-notes", format!("{} changes what coding agents are told", path_of(x)))); }
}
fn not_run_items(f: &mut Found, tests: &Value) {
    let unchecked = |why: &str| -> String {
        if why.starts_with("Kern could not run the tests") { format!("Kern's environment had a problem, so it could not check the tests: {}", plain(why, 400)) }
        else { format!("Kern could not run the tests here, so it cannot say this change kept them passing: {}", plain(why, 400)) }
    };
    let said = tests["run_unconfirmed"].as_array().into_iter().flatten().any(|x| x["why"] == tests["why"]);
    if tests["status"] == "NOT_RUN" && tests["no_tests"] != true && !said { f.unverified.push(item("tests-not-run", unchecked(tests["why"].as_str().unwrap_or("no reason was given")))); }
    if tests["status"] == "RAN" {
        for n in tests["not_run"].as_array().into_iter().flatten() {
            f.unverified.push(item("tests-not-run", format!("{}: {}", code(n["dir"].as_str().filter(|d| !d.is_empty()).unwrap_or("."), 160), unchecked(n["why"].as_str().unwrap_or("no reason was given")))));
        }
    }
}

pub fn build(d: &Value, rules: &Value, tests: &Value, behaviour: &Value) -> Value {
    let mut f = Found::default();
    let (new_tests, unproven) = test_file_items(&mut f, rules, tests);
    let (summary, described, surprises, sorted) = behaviour_looks(&mut f, behaviour);
    change_items(&mut f, rules, tests, new_tests);
    rule_items(&mut f, d, rules, tests);
    run_items(&mut f, rules, tests, unproven);
    steering_items(&mut f, rules);
    not_run_items(&mut f, tests);
    let decision = if !f.blocked.is_empty() { "blocked" } else if !f.look.is_empty() { "look" } else if !f.unverified.is_empty() { "unverified" } else { "clear" };
    let report = &behaviour["report"];
    let lines = if summary { behaviour["lines"].clone() } else { json!({ "ran": report["coverage"]["new"]["ran"].as_u64().unwrap_or(0) + report["coverage"]["old"]["ran"].as_u64().unwrap_or(0),
        "total": report["coverage"]["new"]["total"].as_u64().unwrap_or(0) + report["coverage"]["old"]["total"].as_u64().unwrap_or(0) }) };
    let percent = if summary { behaviour["percent"].clone() } else { report["coverage"]["percent"].clone() };
    let said = |x: &Value| json!({ "what": format!("{}: {}", code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""), 400)) });
    let not_checked: Vec<Value> = rules["not_checked"].as_array().into_iter().flatten().map(said).chain(d["skipped"].as_array().into_iter().flatten().map(said))
        .chain(d["unmeasured"].as_array().into_iter().flatten().filter(|x| x["why"] == "other files")
            .map(|x| json!({ "what": format!("{}: Kern does not check this kind of file", code(x["path"].as_str().unwrap_or(""), 160)) }))).chain(f.cannot_run).collect();
    json!({ "version": 1, "kern": env!("CARGO_PKG_VERSION"), "sandbox": if crate::sandbox::throwaway() { "throwaway" } else { "gvisor" }, "base": d["base"], "head": d["head"],
        "drive_skipped": d["skipped"].as_array().is_some_and(|s| !s.is_empty()), "decision": decision, "blocked": f.blocked, "look": f.look, "unverified": f.unverified, "notes": f.notes, "tests": tests,
        "behaviour": { "status": behaviour["status"], "why": behaviour["why"], "percent": percent, "lines": lines, "described": described, "not_mentioned": surprises, "sorted": sorted },
        "rules": rules, "not_checked": not_checked, "drive": { "skipped": d["skipped"], "unmeasured": d["unmeasured"] } })
}
pub fn rebuild(r: &Value) -> R<Value> {
    let commit = |v: &Value| v.as_str().is_some_and(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()));
    ensure!(r["version"] == 1 && commit(&r["head"]) && commit(&r["base"]), "The receipt does not name the two commits it checked");
    ensure!(matches!(r["sandbox"].as_str(), Some("throwaway" | "gvisor")), "The receipt names no sandbox Kern knows");
    ensure!(r["kern"] == env!("CARGO_PKG_VERSION"), "The receipt was made by another version of Kern");
    ensure!(r["tests"].is_object() && r["rules"].is_object() && r["behaviour"].is_object(), "The receipt is missing the facts its answer comes from");
    let d = json!({ "base": r["base"], "head": r["head"], "skipped": r["drive"]["skipped"], "unmeasured": r["drive"]["unmeasured"] });
    let mut out = build(&d, &r["rules"], &r["tests"], &r["behaviour"]);
    out["sandbox"] = r["sandbox"].clone();
    Ok(out)
}
const KERN_TAGS: [&str; 6] = ["<details>", "</details>", "<summary>", "</summary>", "<sub>", "</sub>"];
fn kern_html_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        out.push(if c == '<' && !KERN_TAGS.iter().any(|t| text[i..].starts_with(t)) { '‹' } else { c });
    }
    out
}
const SIGNED_LINE: &str = "<sub>Signed by GitHub for Kern's own workflow.";
pub fn text_digest(md: &str) -> String { sha256(md.replace("\r\n", "\n").trim_end().as_bytes()) }
pub fn comment_matches(seal: &Value, body: &str) -> bool {
    let Some(want) = seal["markdown_sha256"].as_str() else { return true };
    body.rfind(SIGNED_LINE).is_some_and(|at| text_digest(&body[..at]) == want)
}

fn plural(n: usize, one: &str, many: &str) -> String { format!("{n} {}", if n == 1 { one } else { many }) }
fn list(md: &mut String, items: &[Value]) {
    for x in items.iter().take(SHOWN) { md.push_str(&format!("- {}\n", x["what"].as_str().unwrap_or(""))); }
    if items.len() > SHOWN { md.push_str(&format!("- … and {} more in Kern's receipt.json\n", items.len() - SHOWN)); }
}

pub fn markdown(r: &Value, drive_comment: Option<&str>) -> String {
    let blocked = r["blocked"].as_array().cloned().unwrap_or_default();
    let look = r["look"].as_array().cloned().unwrap_or_default();
    let unverified = r["unverified"].as_array().cloned().unwrap_or_default();
    let notes = r["notes"].as_array().cloned().unwrap_or_default();
    let tests = &r["tests"];
    let behaviour = &r["behaviour"];
    let tests_ran = tests["status"] == "RAN";
    let behaviour_ran = behaviour["status"] == "RAN";
    let nothing = tests["status"] == "NOTHING_TO_RUN";
    let mut md = String::from(MARK);
    md.push('\n');
    md += &match r["decision"].as_str() {
        Some("blocked") => format!("### ❌ Kern: Blocked — {}\n\n", plural(blocked.len(), "problem to fix", "problems to fix")),
        Some("look") => format!("### ⚠️ Kern: {} a look\n\n", plural(look.len(), "thing needs", "things need")),
        Some("unverified") => "### ⏭️ Kern: Not verified\n\nKern found nothing that needs a look, but it could not check all of this change (see below).\n\n".to_string(),
        _ if tests_ran && tests["not_run"].as_array().is_some_and(|n| !n.is_empty()) => "### ✅ Kern: Nothing unexpected in what it ran\n\nSome packages' tests did not run (see below).\n\n".to_string(),
        _ if tests_ran || nothing => "### ✅ Kern: Nothing unexpected\n\n".to_string(),
        _ if behaviour_ran => "### ✅ Kern: Nothing unexpected in what it ran\n\nThe project's tests did not run (see below).\n\n".to_string(),
        _ => "### ✅ Kern: Nothing unexpected in the rules it checked\n\nNot run: the project's tests and its changed code (see below), so this receipt covers the rules only.\n\n".to_string(),
    };
    if !blocked.is_empty() {
        md += "**Blocked: fix these.** An approval cannot clear them.\n\n";
        list(&mut md, &blocked);
        md += "\n";
    }
    if !look.is_empty() {
        md += "**Needs a look.** If each one is meant, a maintainer comments `/kern approve`.\n\n";
        list(&mut md, &look);
        md += "\n";
    }
    if !unverified.is_empty() {
        md += if r["decision"] == "unverified" { "**Not verified.** Kern could not check these, so it cannot say they are safe. If that is acceptable, a maintainer comments `/kern approve`.\n\n" }
            else { "**Not verified.** Kern could not check these either:\n\n" };
        list(&mut md, &unverified);
        md += "\n";
    }
    if !notes.is_empty() {
        md += "**Notes.** Changes the pull request makes on purpose, with the values before and after, so a person can scan them:\n\n";
        list(&mut md, &notes);
        md += "\n";
    }
    md += "**What Kern checked**\n\n";
    if tests_ran {
        let kept = tests["kept"].as_u64().unwrap_or(0) as usize;
        let broke = tests["broke"].as_array().map_or(0, Vec::len);
        let runner = plain(tests["runner"].as_str().unwrap_or("tests"), 400);
        let files = blocked.iter().filter(|x| x["kind"] == "test-file").count();
        let doubtful = unverified.iter().any(|x| ["test-unconfirmed", "test-together", "test-unsteady", "test-uneven", "test-unproven", "test-sandbox"].iter().any(|k| x["kind"] == *k));
        let mut line = if broke > 0 || files > 0 {
            let mut what = Vec::new();
            if broke > 0 { what.push(format!("{broke} that passed before now {}", if broke == 1 { "fails" } else { "fail" })); }
            if files > 0 { what.push(format!("{files} test {} no longer {}", if files == 1 { "file" } else { "files" }, if files == 1 { "loads" } else { "load" })); }
            format!("- ❌ Tests: {}; {kept} still pass ({runner})", what.join(", "))
        } else if doubtful {
            format!("- ⚠️ Tests: {kept} that passed before still pass, but Kern could not prove or rule out some failures (see Not verified above) ({runner})")
        } else if unverified.iter().any(|x| x["kind"] == "untested-change") {
            format!("- ⚠️ Tests: {kept} that passed before still pass, but no test that runs here reaches part of the changed code (see Not verified above) ({runner})")
        } else {
            format!("- ✅ Tests: {kept} that passed before still pass ({runner})")
        };
        let added = tests["added"].as_array().map_or(0, Vec::len);
        let failing = tests["already_failing"].as_array().map_or(0, Vec::len);
        let flaky = tests["flaky"].as_array().map_or(0, Vec::len);
        if added > 0 { line += &format!(" · {} new", added); }
        if flaky > 0 { line += &format!(" · {flaky} changed by chance between runs, so they were set aside (flaky)"); }
        let same = tests["same_failure"].as_array().map_or(0, Vec::len);
        if same > 0 { line += &format!(" · {same} new fail here the same way tests already failing on the old code do (set aside)"); }
        let aside = tests["environmental"].as_array().map_or(0, Vec::len);
        if aside > 0 { line += &format!(" · {aside} also fail on the old code when their files run alone (set aside)"); }
        if failing > 0 { line += &format!(" · {} already failing before this change (set aside)", failing); }
        md += &line;
        md += "\n";
        let planted: Vec<&Value> = tests["faults"].as_array().into_iter().flatten().filter(|f| f["outcome"] != "unknown").collect();
        if !planted.is_empty() {
            let caught = planted.iter().filter(|f| f["outcome"] == "caught").count();
            let missed: Vec<String> = planted.iter().filter(|f| f["outcome"] != "caught").take(SHOWN)
                .map(|f| code(&format!("{}:{} {}", f["path"].as_str().unwrap_or(""), f["line"], f["what"].as_str().unwrap_or("")), 200)).collect();
            md += &format!("- {} Planted faults: the tests caught {caught} of {} faults Kern put in the changed code{}\n", if missed.is_empty() { "✅" } else { "⚠️" }, planted.len(),
                if missed.is_empty() { String::new() } else { format!("; not caught: {}", missed.join(", ")) });
        }
        let mut no_fault: Vec<(String, Vec<String>)> = Vec::new();
        for x in tests["faults_missing"].as_array().into_iter().flatten() {
            let (path, why) = (code(x["path"].as_str().unwrap_or(""), 160), plain(x["why"].as_str().unwrap_or(""), 400));
            match no_fault.iter_mut().find(|(w, _)| *w == why) { Some((_, paths)) => paths.push(path), None => no_fault.push((why, vec![path])) }
        }
        for (why, paths) in &no_fault {
            let more = if paths.len() > SHOWN { format!(" and {} more", paths.len() - SHOWN) } else { String::new() };
            md += &format!("- ⏭️ No fault planted in {}{more}: {why}\n", paths.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", "));
        }
        if tests["dropped"].as_array().is_some_and(|d| d.iter().any(|x| x == "planted faults")) {
            md += "- ⏭️ Planted faults: not checked — Kern's time ran short, so it dropped them first (they rank below the tests and the type check)\n";
        }
        for n in tests["not_run"].as_array().into_iter().flatten() {
            md += &format!("- ⏭️ Tests in {}: Not run — {}\n", code(n["dir"].as_str().unwrap_or(""), 160), plain(n["why"].as_str().unwrap_or(""), 400));
        }
        for d in tests["no_tests_in"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            md += &format!("- ⏭️ Tests in {}: none — its test script runs no tests\n", code(if d.is_empty() { "." } else { d }, 160));
        }
        let count = |k: &str| tests[k].as_array().map_or(0, Vec::len);
        let (edited, rewritten, unchecked) = (tests["edited"].as_u64().unwrap_or(0) as usize, count("rewritten"), count("edited_unchecked"));
        let read_as = |kind: &str| strings(&tests["rewritten"]).iter().filter(|k| tests["polarity"][k.as_str()]["kind"] == kind).count();
        let (moved, unrun) = (read_as("changed"), read_as("harness"));
        let failing = rewritten.saturating_sub(moved + unrun);
        let also = if moved > 0 { format!("; the change moves what {} expect, on purpose (see Notes)", plural(moved, "other test", "other tests")) } else { String::new() };
        if failing > 0 { md += &format!("- ⚠️ Edited tests: the old versions of {} fail on the new code (see above)\n", plural(failing, "test", "tests")); }
        else if unrun > 0 { md += &format!("- ⏭️ Edited tests: Kern could not run the old versions of {} against the new code (see Not verified){also}\n", plural(unrun, "test", "tests")); }
        else if moved > 0 { md += &format!("- ✏️ Edited tests: the change moves what {} expect, on purpose (see Notes)\n", plural(moved, "test", "tests")); }
        else if unchecked > 0 {
            let passed = if edited > 0 { format!("; the old versions of {edited} others still pass") } else { String::new() };
            md += &format!("- ⚠️ Edited tests: Kern could not run the old versions of {} on the new code (see above){passed}\n", plural(unchecked, "edited test file", "edited test files"));
        }
        else if edited > 0 { md += &format!("- ✅ Edited tests: their old versions still pass on the new code ({})\n", plural(edited, "test file", "test files")); }
    } else if tests["status"] == "BROKE" {
        md += "- ❌ Tests: they run on the old code but give no results on the new code (see above)\n";
    } else if nothing {
        md += "- ✅ Tests: nothing to run — this change touches only documentation\n";
    } else {
        md += &format!("- ⏭️ Tests: Not run — {}\n", plain(tests["why"].as_str().unwrap_or("Kern found no tests it can run"), 400));
    }
    let (old_node, new_node) = (&tests["node"]["old"], &tests["node"]["new"]);
    if let (Some(a), Some(b)) = (old_node["major"].as_u64(), new_node["major"].as_u64()) {
        let from = new_node["from"].as_str().or(old_node["from"].as_str()).map(|f| format!(" ({})", plain(f, 400))).unwrap_or_default();
        if let Some(w) = new_node["wanted"].as_u64().or(old_node["wanted"].as_u64()) {
            md += &format!("- ⚠️ Node: the project asks for Node {w}{from}, but Kern ran Node {b}: {}\n", plain(new_node["why"].as_str().or(old_node["why"].as_str()).unwrap_or(""), 400));
        } else if a != b {
            md += &format!("- ⚠️ Node: the change moves the project from Node {a} to Node {b}{from}, and each side ran on its own\n");
        } else if !from.is_empty() {
            md += &format!("- ✅ Node: {b} on both sides, as the project asks{from}\n");
        }
    }
    if tests["types"]["status"] == "NOT_RUN" { md += &format!("- ⏭️ Types: not checked — {}\n", plain(tests["types"]["why"].as_str().unwrap_or("Kern could not run tsc"), 400)); }
    for p in tests["types"]["projects"].as_array().into_iter().flatten() {
        let config = code(p["config"].as_str().unwrap_or(""), 160);
        let version = p["version"].as_str().filter(|v| !v.is_empty()).map(|v| format!(", TypeScript {v}")).unwrap_or_default();
        match p["result"].as_str() {
            Some("same") => md += &format!("- ✅ Types: no new TypeScript errors in {config} (tsc on the old and the new code{version})\n"),
            Some("new-errors") => md += &format!("- ❌ Types: {} in {config}, which had none on the old code (see above)\n", plural(p["count"].as_u64().unwrap_or(0) as usize, "new TypeScript error", "new TypeScript errors")),
            _ => md += &format!("- ⏭️ Types: {config} not compared — {}\n", plain(p["why"].as_str().unwrap_or("Kern could not run tsc on it"), 400)),
        }
    }
    if routine_deps(&r["rules"]) {
        let moved: Vec<String> = r["rules"]["dependencies"].as_array().into_iter().flatten().flat_map(|d| strings(&d["packages"])).collect();
        md += &format!("- ✅ Dependencies: {}{} (each removed, from this repository, or updated within the same major version to a release out for at least {} days)\n",
            moved.iter().take(SHOWN).map(|m| code(m, 120)).collect::<Vec<_>>().join(", "), if moved.len() > SHOWN { format!(" and {} more", moved.len() - SHOWN) } else { String::new() }, crate::deps::FRESH_DAYS);
    }
    let all_powers: Vec<&Value> = r["rules"]["powers"].as_array().into_iter().flatten().collect();
    let powers = all_powers.iter().filter(|p| shown_power(p)).count();
    let quiet: Vec<String> = all_powers.iter().filter(|p| !shown_power(p)).take(SHOWN).map(|p| code(p["path"].as_str().unwrap_or(""), 160)).collect();
    let other_code = r["rules"]["changed"].as_array().into_iter().flatten().any(|c| c["operation"] != "delete" && c["path"].as_str().is_some_and(crate::capability::other_code));
    let unread = other_code || look.iter().any(|x| x["kind"] == "not-checked-code") || r["rules"]["not_checked"].as_array().into_iter().flatten().any(|x| x["code"].is_string());
    let skipped_code = r["drive_skipped"].as_bool() == Some(true);
    if powers == 0 && !unread && quiet.is_empty() { md += "- ✅ No new powers: no changed code newly starts programs, reaches the network or reads secret environment variables\n"; }
    if powers == 0 && !unread && !quiet.is_empty() {
        md += &format!("- ✅ No new powers outside the tests: tests gained some, but none can both read a secret and send it out, or start other programs ({})\n", quiet.join(", "));
    }
    let none = |k: &str| tests[k].as_array().is_none_or(Vec::is_empty);
    if (tests_ran || nothing) && !unread && !blocked.iter().any(|x| x["kind"] == "test-file") && r["rules"]["tricks"].as_array().is_none_or(Vec::is_empty) && r["rules"]["deleted_tests"].as_array().is_none_or(Vec::is_empty)
        && r["rules"]["weakened"].as_array().is_none_or(Vec::is_empty) && none("gone") && none("newly_skipped") {
        md += "- ✅ No skipped, focused or removed tests\n";
    }
    match behaviour["status"].as_str() {
        Some("RAN") => md += &format!("- {} {}% of the changed lines ran, on the old and the new code\n", if behaviour["percent"].as_u64().unwrap_or(0) >= 80 { "✅" } else { "⚠️" }, behaviour["percent"].as_u64().unwrap_or(0)),
        Some("NOTHING_TO_RUN") if unread || skipped_code => md += "- ⏭️ Changed lines: none Kern can run here (it runs JavaScript and TypeScript, and leaves out generated or minified code)\n",
        Some("NOTHING_TO_RUN") => md += "- ✅ No changed line of code to run: only tests, docs, configuration or types changed\n",
        Some("OFF") => md += "- ⏭️ Changed lines: Not run. Add an AI key (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`) and Kern runs every changed line on the old and the new code\n",
        _ => md += &format!("- ⏭️ Changed lines: Not run — {}\n", plain(behaviour["why"].as_str().unwrap_or("the run did not finish"), 400)),
    }
    md += "\n";
    let not_checked = r["not_checked"].as_array().cloned().unwrap_or_default();
    if !not_checked.is_empty() {
        md += "**Not checked**\n\n";
        list(&mut md, &not_checked);
        md += "\n";
    }
    let short = |k: &str| r[k].as_str().filter(|h| h.len() >= 7).map(|h| h[..7].to_string()).unwrap_or_default();
    let sandbox = match (tests_ran || behaviour_ran, r["sandbox"].as_str()) {
        (false, _) => "",
        (true, Some("throwaway")) => ", and ran the code only in locked containers on a throwaway machine that held no keys, with the network off while the tests ran",
        (true, _) => ", and ran the code only in a sealed sandbox with the network off",
    };
    let footer = format!("<sub>Kern {} checked `{}` against `{}`{sandbox}.</sub>\n", r["kern"].as_str().unwrap_or(""), short("head"), short("base"));
    if let Some(c) = drive_comment.filter(|c| !c.trim().is_empty()) {
        let mut body = c.lines().skip_while(|l| l.starts_with("### ") || l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        let room = RECEIPT_BYTES_MAX.saturating_sub(md.len() + footer.len() + DETAILS_FRAME_BYTES);
        if body.len() > room {
            let mut cut = room;
            while !body.is_char_boundary(cut) { cut -= 1; }
            body.truncate(body[..cut].rfind('\n').unwrap_or(cut));
            body += "\n\n… (cut short to fit the comment)";
        }
        md += &format!("<details><summary>What this change does when it runs, line by line</summary>\n\n{body}\n\n</details>\n\n");
    }
    md += &footer;
    if md.len() > RECEIPT_BYTES_MAX {
        let mut cut = RECEIPT_BYTES_MAX;
        while !md.is_char_boundary(cut) { cut -= 1; }
        md.truncate(md[..cut].rfind('\n').unwrap_or(cut));
        md += "\n… (cut short; the full receipt is in the job's receipt.json)\n";
    }
    md
}

pub fn write(out: &Path, r: &Value, drive_comment: Option<&str>) -> R<()> {
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("receipt.json"), format!("{}\n", serde_json::to_string_pretty(r)?))?;
    std::fs::write(out.join("receipt.md"), markdown(r, drive_comment))?;
    Ok(())
}

pub fn seal(r: &Value, md: &str, repository: &str, number: u64) -> R<Value> {
    Ok(json!({ "v": 1, "repository": repository, "pull": number, "base": r["base"], "head": r["head"], "decision": r["decision"], "receipt_sha256": digest(r)?, "markdown_sha256": text_digest(md) }))
}
pub fn seal_line(seal: &Value, token: &str) -> String { format!("{SEAL_MARK}{} -->", json!({ "seal": seal, "token": token })) }

pub fn render(out: &Path, head: &str) -> R<Value> {
    ensure!(head.len() == 40 && head.bytes().all(|b| b.is_ascii_hexdigit()), "Expected the pull request's head commit");
    let path = out.join("receipt.json");
    ensure!(std::fs::metadata(&path)?.len() <= 8 << 20, "The receipt is larger than 8 MiB");
    let given = read_json(&path)?;
    ensure!(given["version"] == 1 && given["head"].as_str() == Some(head), "The receipt is not for this pull request's head commit");
    let _ = std::fs::remove_file(out.join("seal.json"));
    let r = rebuild(&given)?;
    let comment = std::fs::read_to_string(out.join("comment.md")).ok().filter(|c| r["behaviour"]["status"] == "RAN" && c.len() <= 256 << 10).map(|c| kern_html_only(&c));
    write(out, &r, comment.as_deref())?;
    Ok(json!({ "status": "RENDERED", "decision": r["decision"], "head": head }))
}
pub fn sign(out: &Path, repository: &str, number: u64) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let s = seal(&read_json(out.join("receipt.json"))?, &std::fs::read_to_string(out.join("receipt.md"))?, repository, number)?;
    let token = crate::oidc::request(&crate::oidc::audience(&s)?)?;
    std::fs::write(out.join("seal.json"), format!("{}\n", serde_json::to_string_pretty(&json!({ "seal": s, "token": token }))?))?;
    Ok(json!({ "status": "SIGNED", "decision": s["decision"], "head": s["head"] }))
}

pub fn signed(seal: &Value, token: &str, jwks: &Value, repository: &str) -> R<Value> {
    let claims = crate::oidc::verify(token, jwks, &crate::oidc::audience(seal)?, crate::oidc::WORKFLOW, Some(repository))?;
    kern_workflow(&claims)?;
    Ok(claims)
}
const SIGNING_BRANCHES: [&str; 2] = ["refs/heads/main", "refs/heads/one"];
fn release_tag(at: &str) -> bool {
    at.strip_prefix("refs/tags/v").is_some_and(|v| v.split('.').count() == 3 && v.split('.').all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit())))
}
pub fn kern_workflow(claims: &Value) -> R<()> {
    let at = claims["job_workflow_ref"].as_str().and_then(|r| r.strip_prefix(crate::oidc::WORKFLOW)).unwrap_or("");
    ensure!(SIGNING_BRANCHES.contains(&at) || release_tag(at), "The receipt was not signed by Kern's workflow at a branch or tag Kern releases from ({})", code(at, 80));
    Ok(())
}

pub fn verify(api: &Api, repository: &str, number: u64, jwks: &Value, receipt: Option<&Value>) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let Some(c) = kern_comment(api, repository, number)? else { return Ok(json!({ "status": "NO_RECEIPT" })) };
    let Some((seal, token)) = read_seal_line(c["body"].as_str().unwrap_or("")) else { return Ok(json!({ "status": "UNSIGNED", "comment": c["html_url"] })) };
    if seal["repository"] != repository || seal["pull"] != number { return Ok(json!({ "status": "INVALID", "why": "the seal is for another pull request" })); }
    let claims = match signed(&seal, &token, jwks, repository) { Ok(c) => c, Err(e) => return Ok(json!({ "status": "INVALID", "why": e.0 })) };
    if let Some(r) = receipt && seal["receipt_sha256"] != digest(r)?.as_str() {
        return Ok(json!({ "status": "INVALID", "why": "receipt.json is not the receipt this seal names" }));
    }
    if !comment_matches(&seal, c["body"].as_str().unwrap_or("")) {
        return Ok(json!({ "status": "INVALID", "why": "the comment's text was changed after Kern signed it" }));
    }
    let head = head_of(api, repository, number)?;
    Ok(json!({ "status": "VERIFIED", "decision": seal["decision"], "head": seal["head"], "base": seal["base"], "current": seal["head"] == head.as_str(),
        "workflow": claims["job_workflow_ref"], "run": claims["run_id"], "signed_at": claims["iat"], "receipt_checked": receipt.is_some(), "comment": c["html_url"] }))
}
fn read_seal_line(body: &str) -> Option<(Value, String)> {
    let line = body.lines().rev().find(|l| l.starts_with(SEAL_MARK))?;
    let v: Value = serde_json::from_str(line.strip_prefix(SEAL_MARK)?.strip_suffix(" -->")?).ok()?;
    Some((v["seal"].clone(), v["token"].as_str()?.to_string()))
}

fn kern_comment(api: &Api, repository: &str, number: u64) -> R<Option<Value>> {
    let who = author();
    for page in 1..=1000 {
        let items = api.get(&format!("/repos/{repository}/issues/{number}/comments?per_page=100&page={page}"))?.as_array().cloned().unwrap_or_default();
        if let Some(c) = items.iter().find(|c| c["body"].as_str().is_some_and(|b| b.starts_with(MARK)) && c["user"]["login"] == who.as_str()) { return Ok(Some(c.clone())); }
        if items.len() < 100 { break; }
    }
    Ok(None)
}
fn head_of(api: &Api, repository: &str, number: u64) -> R<String> {
    Ok(api.get(&format!("/repos/{repository}/pulls/{number}"))?["head"]["sha"].as_str().unwrap_or("").to_string())
}
fn status(api: &Api, repository: &str, sha: &str, state: &str, description: &str, url: Option<&str>) -> R<()> {
    let description: String = description.chars().take(140).collect();
    let mut body = json!({ "state": state, "context": "Kern", "description": description });
    if let Some(u) = url { body["target_url"] = json!(u); }
    api.post(&format!("/repos/{repository}/statuses/{sha}"), &body)?;
    Ok(())
}
fn description(r: &Value) -> String {
    let n = |k: &str| r[k].as_array().map_or(0, Vec::len);
    match r["decision"].as_str() {
        Some("blocked") => format!("Blocked: {}", r["blocked"][0]["what"].as_str().unwrap_or("a problem to fix").replace('`', "")),
        Some("look") => format!("{} — approve with /kern approve", plural(n("look"), "thing needs a look", "things need a look")),
        Some("unverified") => format!("Not verified: Kern could not check {} — approve with /kern approve", plural(n("unverified"), "thing", "things")),
        _ if r["tests"]["status"] == "RAN" && r["tests"]["not_run"].as_array().is_some_and(|n| !n.is_empty()) => "Nothing unexpected in what ran; some packages' tests did not run".to_string(),
        _ if r["tests"]["status"] == "RAN" || r["tests"]["status"] == "NOTHING_TO_RUN" => "Nothing unexpected".to_string(),
        _ => "Nothing unexpected in the rules; the tests did not run".to_string(),
    }
}

pub fn post(api: &Api, out: &Path, repository: &str, number: u64) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    let r = read_json(out.join("receipt.json"))?;
    let head = r["head"].as_str().unwrap_or("").to_string();
    let now = head_of(api, repository, number)?;
    if now != head { return Ok(json!({ "status": "STALE", "checked": head, "head": now })); }
    let mut body = std::fs::read_to_string(out.join("receipt.md"))?;
    if let Ok(s) = read_json(out.join("seal.json")) {
        body += &format!("{SIGNED_LINE} Anyone can check this receipt: `kern receipt verify {repository} {number}`</sub>\n{}\n",
            seal_line(&s["seal"], s["token"].as_str().unwrap_or("")));
    }
    let edit = |c: &Value| -> R<(&str, Option<String>)> {
        api.patch(&format!("/repos/{repository}/issues/comments/{}", c["id"]), &json!({ "body": body }))?;
        Ok(("UPDATED", c["html_url"].as_str().map(str::to_string)))
    };
    let create = || api.create(&format!("/repos/{repository}/issues/{number}/comments"), &json!({ "body": body }));
    let (posted, url) = match kern_comment(api, repository, number)? {
        Some(c) => edit(&c)?,
        None => match create() {
            Ok(c) => ("POSTED", c["html_url"].as_str().map(str::to_string)),
            Err(_) => match kern_comment(api, repository, number)? {
                Some(c) => edit(&c)?,
                None => ("POSTED", create()?["html_url"].as_str().map(str::to_string)),
            },
        },
    };
    let state = match r["decision"].as_str() { Some("clear") => "success", Some("unverified") => "error", _ => "failure" };
    status(api, repository, &head, state, &description(&r), url.as_deref())?;
    Ok(json!({ "status": posted, "state": state, "decision": r["decision"] }))
}

pub fn approve(api: &Api, repository: &str, number: u64, login: &str, check: &dyn Fn(&Value, &str) -> R<()>) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    ensure!(!login.is_empty() && login.len() <= 100 && login.chars().all(|c| c.is_ascii_alphanumeric() || "-_[]".contains(c)), "Expected a GitHub login");
    let say = |text: &str| -> R<()> { api.create(&format!("/repos/{repository}/issues/{number}/comments"), &json!({ "body": text }))?; Ok(()) };
    let permission = api.get(&format!("/repos/{repository}/collaborators/{login}/permission"))?;
    if !["admin", "maintain", "write"].contains(&permission["permission"].as_str().unwrap_or("")) {
        say(&format!("Kern: only people who can merge into this repository can approve its receipt; @{login} cannot."))?;
        return Ok(json!({ "status": "NOT_ALLOWED" }));
    }
    let head = head_of(api, repository, number)?;
    let Some(c) = kern_comment(api, repository, number)? else { say("Kern: there is no receipt to approve yet.")?; return Ok(json!({ "status": "NO_RECEIPT" })) };
    let body = c["body"].as_str().unwrap_or("").to_string();
    let Some((seal, token)) = read_seal_line(&body) else { say("Kern: this receipt carries no signature, so it cannot be approved.")?; return Ok(json!({ "status": "UNVERIFIED" })) };
    if check(&seal, &token).is_err() || seal["repository"] != repository || seal["pull"] != number {
        say("Kern: this receipt's signature does not check out, so it is not approved.")?;
        return Ok(json!({ "status": "UNVERIFIED" }));
    }
    if seal["head"] != head.as_str() { say("Kern: the receipt is for an older commit. Wait for Kern to check the newest one, then approve.")?; return Ok(json!({ "status": "STALE" })); }
    match seal["decision"].as_str() {
        Some("blocked") => { say("Kern: blocked items need a fix; an approval cannot clear them.")?; Ok(json!({ "status": "BLOCKED" })) }
        Some(decision @ ("look" | "unverified")) => {
            status(api, repository, &head, "success", &format!("Approved by @{login}"), c["html_url"].as_str())?;
            let what = if decision == "look" { "every item above was looked at" } else { "Kern could not check every item above, and a maintainer accepts that" };
            let note = format!("\n\n**Approved by @{login}** on {}: {what}.\n", iso8601(now_ms()));
            let updated = match body.rfind(SEAL_MARK) { Some(at) => format!("{}{note}
{}", body[..at].trim_end(), &body[at..]), None => format!("{body}{note}") };
            api.patch(&format!("/repos/{repository}/issues/comments/{}", c["id"]), &json!({ "body": updated }))?;
            Ok(json!({ "status": "APPROVED", "by": login, "head": head }))
        }
        _ => Ok(json!({ "status": "NOTHING_TO_APPROVE" })),
    }
}
