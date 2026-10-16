use crate::policy::{Edits, Kind, Limits, Operator, Runner, Tests};
use crate::proof::{self, Report};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const FOLDERS: [&str; 11] = ["test", "tests", "__tests__", "spec", "specs", "e2e", "__e2e__", "cypress", "playwright", "__mocks__", "__fixtures__"];
const NAMES: [&str; 4] = [".test.", ".spec.", ".e2e.", ".cy."];
pub fn test_seconds() -> u64 {
    std::env::var("KERN_TEST_MINUTES").ok().and_then(|m| m.trim().parse::<u64>().ok()).filter(|m| (1..=60).contains(m)).unwrap_or(15) * 60
}
pub const REPORT: &str = "/tmp/.kern-report.json";

pub fn test_command(tests: &Tests) -> Vec<String> {
    let mut c = proof::command(tests);
    match tests.runner {
        Runner::Vitest => c.push(format!("--outputFile={REPORT}")),
        Runner::Mocha => c.extend(["--reporter-option".to_string(), format!("output={REPORT}")]),
        Runner::Node => { if let Some(at) = c.iter().position(|a| a == "--test-reporter=tap") { c.insert(at + 1, format!("--test-reporter-destination={REPORT}")); } }
        Runner::Jest => {}
    }
    c
}

pub fn detect(package: Option<&[u8]>) -> Option<Tests> {
    let (name, command) = crate::init::runner(package)?;
    Some(Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::parse(name)?, command, edits: Edits::Checked })
}

pub type Env = std::collections::BTreeMap<String, String>;
pub enum Plan { Run(Tests, Env), Packages, Nothing }

pub fn plan(package: Option<&[u8]>) -> Plan {
    let parsed = package.and_then(|b| serde_json::from_slice::<Value>(b).ok());
    match parsed.as_ref().map(crate::script::found) {
        Some(crate::script::Found::Run(i)) => Plan::Run(Tests { paths: Vec::new(), kind: Kind::Feature, runner: i.runner, command: i.command, edits: Edits::Checked }, i.env),
        Some(crate::script::Found::Packages) => Plan::Packages,
        _ => detect(package).map_or(Plan::Nothing, |t| Plan::Run(t, Env::new())),
    }
}
fn ready(p: Plan) -> Option<(Tests, Env)> { match p { Plan::Run(t, e) => Some((t, e)), _ => None } }

fn test_grant(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    if let Some(i) = lower[..lower.len().saturating_sub(1)].iter().position(|d| FOLDERS.contains(&d.as_str())) { return Some(format!("{}/", parts[..=i].join("/"))); }
    let name = lower.last()?;
    NAMES.iter().any(|m| name.contains(m)).then(|| path.to_string())
}

pub fn is_test_path(path: &str) -> bool { test_grant(path).is_some() }

pub fn test_paths(paths: &[&str]) -> Vec<String> { paths.iter().filter_map(|p| test_grant(p)).collect::<BTreeSet<_>>().into_iter().collect() }
fn sum(r: &Report) -> Value {
    let (p, f, s) = r.tests.values().fold((0, 0, 0), |(p, f, s), c| (p + c.passed, f + c.failed, s + c.skipped));
    json!({ "passed": p, "failed": f, "skipped": s })
}

fn count(r: &Report, k: &str) -> proof::Count { r.tests.get(k).copied().unwrap_or_default() }

fn masked(name: &str) -> String {
    let (folder, rest) = match name.split_once(": ") {
        Some((p, r)) if !p.is_empty() && !p.contains(char::is_whitespace) && !p.contains(" > ") => (format!("{p}: "), r),
        _ => (String::new(), name),
    };
    let (file, rest) = match rest.split_once(" > ") { Some((f, r)) if crate::capability::is_code(f) => (format!("{f} > "), r), _ => (String::new(), rest) };
    let (mut out, mut digit) = (folder + &file, false);
    for c in rest.chars() {
        if c.is_ascii_digit() { if !digit { out.push('#'); } digit = true; } else { out.push(c); digit = false; }
    }
    out
}

fn rank(c: proof::Count) -> u8 { if c.failed > 0 { 2 } else if c.passed > 0 { 0 } else if c.skipped > 0 { 1 } else { 3 } }

fn paired(before: &Report, after: &Report) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    let lost: Vec<&String> = before.tests.keys().filter(|k| !after.tests.contains_key(*k)).collect();
    let found: Vec<&String> = after.tests.keys().filter(|k| !before.tests.contains_key(*k)).collect();
    pair_groups(before, after, &lost, &found, masked, &mut map);
    let used: BTreeSet<String> = map.values().cloned().collect();
    let (old_files, new_files): (BTreeSet<&str>, BTreeSet<&str>) = (before.tests.keys().map(|k| file_of(k)).collect(), after.tests.keys().map(|k| file_of(k)).collect());
    let moved = |k: &&String, files: &BTreeSet<&str>| crate::capability::is_code(file_of(k)) && !files.contains(file_of(k));
    let lost: Vec<&String> = lost.into_iter().filter(|k| !used.contains(*k) && moved(k, &new_files)).collect();
    let found: Vec<&String> = found.into_iter().filter(|k| !map.contains_key(*k) && moved(k, &old_files)).collect();
    pair_groups(before, after, &lost, &found, |k| masked(k.split_once(" > ").map_or(k, |(_, title)| title)), &mut map);
    map
}

fn pair_groups(before: &Report, after: &Report, lost: &[&String], found: &[&String], key: impl Fn(&str) -> String, map: &mut std::collections::BTreeMap<String, String>) {
    let mut groups: std::collections::BTreeMap<String, (Vec<&String>, Vec<&String>)> = std::collections::BTreeMap::new();
    for k in lost { groups.entry(key(k)).or_default().0.push(*k); }
    for k in found { groups.entry(key(k)).or_default().1.push(*k); }
    for (b, a) in groups.into_values().filter(|(b, a)| b.len() == a.len()) {
        let (mut rest_b, mut rest_a) = (Vec::new(), Vec::new());
        for r in 0..4u8 {
            let olds: Vec<&String> = b.iter().copied().filter(|k| rank(count(before, k)) == r).collect();
            let news: Vec<&String> = a.iter().copied().filter(|k| rank(count(after, k)) == r).collect();
            let n = olds.len().min(news.len());
            for (x, y) in news[..n].iter().zip(&olds[..n]) { map.insert((*x).clone(), (*y).clone()); }
            rest_b.extend_from_slice(&olds[n..]);
            rest_a.extend_from_slice(&news[n..]);
        }
        rest_b.sort();
        rest_a.sort();
        for (x, y) in rest_a.into_iter().zip(rest_b) { map.insert(x.clone(), y.clone()); }
    }
}

fn renamed(r: &Report, map: &std::collections::BTreeMap<String, String>) -> Report {
    let mut out = Report { tests: std::collections::BTreeMap::new(), problems: r.problems.clone(), why: r.why.clone() };
    for (k, c) in &r.tests {
        let e = out.tests.entry(map.get(k).cloned().unwrap_or_else(|| k.clone())).or_default();
        e.passed += c.passed;
        e.failed += c.failed;
        e.skipped += c.skipped;
    }
    out
}

fn file_of(key: &str) -> &str { key.split(" > ").next().unwrap_or(key) }

fn matched(from: &[String], to: &[String]) -> Vec<bool> {
    let mut done = vec![false; from.len()];
    let mut left: Vec<&String> = to.iter().collect();
    for (i, p) in from.iter().enumerate() {
        if let Some(j) = left.iter().position(|q| *q == p) { left.swap_remove(j); done[i] = true; }
    }
    let maskable = |p: &str| p.contains(" > ") || !crate::capability::is_code(p);
    let mut groups: std::collections::BTreeMap<String, (Vec<usize>, usize)> = std::collections::BTreeMap::new();
    for (i, p) in from.iter().enumerate().filter(|(i, p)| !done[*i] && maskable(p)) { groups.entry(masked(p)).or_default().0.push(i); }
    for q in left.iter().filter(|q| maskable(q)) { if let Some(g) = groups.get_mut(&masked(q)) { g.1 += 1; } }
    for (idx, n) in groups.into_values() { if idx.len() == n { for i in idx { done[i] = true; } } }
    done
}
fn fresh_problems(old: &Report, new: &Report) -> Vec<String> {
    new.problems.iter().zip(matched(&new.problems, &old.problems)).filter(|(_, seen)| !seen).map(|(p, _)| p.clone()).collect()
}

fn settled(base: &Report, r: &Report) -> Report {
    let mut r = renamed(r, &paired(base, r));
    let loaded: BTreeSet<String> = r.tests.keys().map(|k| file_of(k).to_string()).collect();
    let broken: BTreeSet<String> = r.problems.iter().map(|p| file_of(p).to_string()).filter(|f| !loaded.contains(f)).collect();
    for (k, b) in &base.tests {
        if broken.contains(file_of(k)) {
            let e = r.tests.entry(k.clone()).or_default();
            e.failed += b.total().saturating_sub(e.total());
        }
    }
    r
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Seen { Same, Gone, Skipped, Failed }
fn seen(b: proof::Count, c: proof::Count) -> Seen {
    if c.failed > b.failed { Seen::Failed } else if c.skipped > b.skipped { Seen::Skipped } else if c.total() < b.total() { Seen::Gone } else { Seen::Same }
}

pub fn compare(before: &Report, after: &Report, again: Option<&Report>) -> Value { judged(before, after, again, None) }

const FOCUS: usize = 50;
pub fn focused(tests: &Tests, files: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    if files.is_empty() || files.len() > FOCUS { return None; }
    let mut t = tests.clone();
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    match tests.runner {
        Runner::Jest => {
            t.command.push("--runTestsByPath".into());
            t.command.extend(files.iter().map(|f| format!("/workspace/{f}")));
        }
        Runner::Vitest => {
            if positionals(tests.command.get(2..).unwrap_or_default()) { return None; }
            for f in files { t.command.push(f.strip_prefix(&prefix)?.to_string()); }
        }
        Runner::Mocha | Runner::Node => return None,
    }
    Some(t)
}

fn positionals(args: &[String]) -> bool {
    let mut value = false;
    for a in args {
        if a.starts_with('-') { value = !a.contains('='); continue; }
        if !value { return true; }
        value = false;
    }
    false
}

pub fn related(tests: &Tests, files: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    if files.is_empty() || files.len() > FOCUS { return None; }
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let mut t = tests.clone();
    match tests.runner {
        Runner::Jest => {
            t.command.push("--findRelatedTests".into());
            t.command.extend(files.iter().map(|f| format!("/workspace/{f}")));
        }
        Runner::Vitest => {
            let args = tests.command.get(2..).unwrap_or_default();
            if tests.command.get(1).map(String::as_str) != Some("run") || positionals(args) { return None; }
            let mut c = vec![tests.command[0].clone(), "related".to_string()];
            for f in files { c.push(f.strip_prefix(&prefix)?.to_string()); }
            c.push("--run".into());
            c.extend(args.iter().cloned());
            t.command = c;
        }
        Runner::Mocha | Runner::Node => return None,
    }
    Some(t)
}

pub fn confirmed(before: &Report, after: &Report, alone: Option<&Report>, whole: Option<&Report>, alone_old: Option<&Report>) -> Value {
    let (Some(a), Some(w)) = (alone, whole) else { return judged(before, after, alone.or(whole), alone_old) };
    let mut c = judged(before, after, Some(a), alone_old);
    let wide = judged(before, after, Some(w), alone_old);
    if c["status"] != "RAN" || wide["status"] != "RAN" { return c; }
    let list = |v: &Value, k: &str| -> Vec<String> { v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect() };
    let narrow: BTreeSet<String> = list(&c, "broke").into_iter().chain(list(&c, "new_problems")).collect();
    let together: Vec<String> = list(&wide, "broke").into_iter().chain(list(&wide, "new_problems")).filter(|k| !narrow.contains(k)).collect();
    let mut gone = list(&c, "gone");
    for g in list(&wide, "gone") { if !gone.contains(&g) { gone.push(g); } }
    let flaky: Vec<String> = list(&c, "flaky").into_iter().filter(|k| !together.contains(k) && !gone.contains(k)).collect();
    c["together"] = json!(together);
    c["gone"] = json!(gone);
    c["flaky"] = json!(flaky);
    c
}

fn repeats(old: &Report, new: &Report, again: &Report) -> bool {
    let (new, again) = (settled(old, new), settled(old, again));
    old.tests.iter().filter(|(_, b)| b.passed > 0).all(|(k, b)| seen(*b, count(&new, k)) == Seen::Same || seen(*b, count(&again, k)) != Seen::Same)
        && matched(&fresh_problems(old, &new), &again.problems).into_iter().all(|x| x)
}

fn suspects(old: &Report, new: &Report) -> BTreeSet<String> {
    let settled_new = settled(old, new);
    let tests = old.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&settled_new, k)) != Seen::Same).map(|(k, _)| file_of(k).to_string());
    let problems = fresh_problems(old, new).into_iter().map(|p| file_of(&p).to_string()).collect::<Vec<_>>();
    tests.chain(problems).collect()
}

pub fn judged(before: &Report, after: &Report, again: Option<&Report>, alone_old: Option<&Report>) -> Value {
    let total = |r: &Report| r.tests.values().map(|c| c.total()).sum::<u32>();
    let passed = |r: &Report| r.tests.values().map(|c| c.passed).sum::<u32>();
    let raw_after = after;
    let after = settled(before, after);
    let again = again.map(|g| settled(before, g));
    let alone_old = alone_old.map(|o| settled(before, o));
    let (after, again, alone_old) = (&after, again.as_ref(), alone_old.as_ref());
    let empty_before = before.tests.is_empty() && before.problems.is_empty();
    let subject_is_new = |p: &str| empty_before || (!crate::capability::is_code(file_of(p)) && {
        let under = format!("{} > ", masked(p));
        !before.tests.keys().any(|k| masked(k).starts_with(&under))
    });
    let (fresh, checkable): (Vec<String>, Vec<String>) = fresh_problems(before, after).into_iter().partition(|p| subject_is_new(p));
    let repeated = again.map(|g| matched(&checkable, &g.problems));
    let excused = alone_old.map(|o| matched(&checkable, &o.problems));
    let (mut new_problems, mut unsteady, mut environmental) = (Vec::new(), Vec::new(), Vec::new());
    for (i, p) in checkable.iter().enumerate() {
        if excused.as_ref().is_some_and(|e| e[i]) { environmental.push(p.clone()); }
        else if repeated.as_ref().is_none_or(|r| r[i]) { new_problems.push(p.clone()); }
        else { unsteady.push(p.clone()); }
    }
    let reported: BTreeSet<String> = new_problems.iter().map(|p| file_of(p).to_string()).collect();
    let shaky: BTreeSet<String> = unsteady.iter().chain(&environmental).map(|p| file_of(p).to_string()).collect();
    let keys: BTreeSet<&String> = before.tests.keys().chain(after.tests.keys()).collect();
    let before_broken: BTreeSet<&str> = before.problems.iter().map(|p| file_of(p)).collect();
    let (mut kept, mut broke, mut flaky, mut gone, mut newly_skipped, mut added, mut already_failing) = (0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut new_failing = Vec::new();
    for k in keys {
        let (b, a) = (count(before, k), count(after, k));
        if b.passed == 0 && b.failed == 0 {
            if b.total() == 0 && before_broken.contains(file_of(k)) { already_failing.push(k.clone()); continue; }
            if b.total() == 0 || a.ran() > 0 { added.push(k.clone()); }
            if a.failed > 0 { new_failing.push(k.clone()); }
            continue;
        }
        if b.failed > 0 { already_failing.push(k.clone()); }
        kept += u64::from(b.passed.min(a.passed));
        if b.passed == 0 { continue; }
        let first = seen(b, a);
        if first == Seen::Same { continue; }
        let second = again.map_or(first, |g| seen(b, count(g, k)));
        let verdict = first.max(second);
        let old_alone = alone_old.map_or(Seen::Same, |o| seen(b, count(o, k)));
        let set_aside = if verdict == Seen::Gone { old_alone == Seen::Gone } else { old_alone != Seen::Same };
        if second == Seen::Same {
            kept += u64::from(b.passed - b.passed.min(a.passed));
            if !shaky.contains(file_of(k)) { flaky.push(k.clone()); }
            continue;
        }
        if set_aside {
            if !shaky.contains(file_of(k)) { environmental.push(k.clone()); }
            continue;
        }
        if reported.contains(file_of(k)) { continue; }
        match verdict {
            Seen::Failed => broke.push(k.clone()),
            Seen::Skipped => newly_skipped.push(k.clone()),
            Seen::Gone => gone.push(k.clone()),
            Seen::Same => {}
        }
    }
    flaky.extend(unsteady);
    new_failing.extend(fresh);
    already_failing.extend(before.problems.iter().cloned());
    if total(before) == 0 && total(raw_after) == 0 && before.problems.is_empty() && raw_after.problems.is_empty() {
        return json!({ "status": "NOT_RUN", "why": "the test runner ran but found no tests on either side" });
    }
    if passed(before) == 0 && (total(before) > 0 || !before.problems.is_empty()) {
        let why = match before.problems.iter().find(|p| before.why.contains_key(*p)).or(before.problems.first()).filter(|_| total(before) == 0) {
            Some(first) => format!("none of the {} test files could load on the old code (they may need a build or generated files first); {first}: {}",
                before.problems.len(), before.why.get(first).map_or("no error was given", String::as_str)),
            None => "no test passed on the old code here (they may need the network, a database or a build first), so the tests showed nothing".to_string(),
        };
        new_failing.extend(fresh_problems(before, raw_after).into_iter().filter(|p| !new_failing.contains(p)).collect::<Vec<_>>());
        return json!({ "status": "NOT_RUN", "why": why, "new_failing": new_failing });
    }
    let problem_why: serde_json::Map<String, Value> = new_problems.iter().filter_map(|p| again.unwrap_or(after).why.get(p).or_else(|| after.why.get(p)).map(|w| (p.clone(), json!(w)))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(raw_after), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
        "newly_skipped": newly_skipped, "added": added, "new_failing": new_failing, "already_failing": already_failing, "new_problems": new_problems, "problem_why": problem_why,
        "environmental": environmental })
}

pub fn no_results(why: &str, ending: &[u8], errors: &[u8], output: &[u8]) -> String {
    let text = |b: &[u8]| -> String {
        let s = String::from_utf8_lossy(b);
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' { if chars.peek() == Some(&'[') { chars.next(); for d in chars.by_ref() { if d.is_ascii_alphabetic() { break; } } } continue; }
            out.push(if c.is_control() && c != '\n' { ' ' } else { c });
        }
        out
    };
    let source = if errors.iter().any(|b| !b.is_ascii_whitespace()) { text(errors) } else { text(output) };
    let last: Vec<&str> = source.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let from = last.iter().rposition(|l| !l.starts_with("at ")).unwrap_or(0).saturating_sub(1);
    let tail = last[from..].iter().take(3).copied().collect::<Vec<_>>().join(" / ");
    let mut s = format!("{why} ({})", String::from_utf8_lossy(ending).trim());
    if !tail.is_empty() { s += &format!("; the last lines it printed: {tail}"); }
    if s.chars().count() > 600 { s = format!("{}…", s.chars().take(599).collect::<String>()); }
    s
}

pub enum Side { Ran(Report, Vec<String>), Nothing(String), Slow(String), Late }

fn run_side(out: &Path, op: &Operator, volume: &str, (tests, env): (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    let limit = test_seconds();
    let seconds = within(until, limit);
    if seconds < 60 { return Ok(Side::Late); }
    let cut = seconds < limit;
    let control = out.join(format!("tests-{label}"));
    if control.exists() { std::fs::remove_dir_all(&control)?; }
    crate::sandbox::control_dir(&control, Some(("drive-test.json", &json!({ "version": 1, "command": test_command(tests), "timeout_seconds": seconds, "cwd": dir, "env": env }))))?;
    let input = out.join("tests-input");
    std::fs::create_dir_all(&input)?;
    let limits = Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: seconds + 120, memory_mb: test_memory_mb(std::fs::read_to_string("/proc/meminfo").ok().as_deref()), pids: 4096 };
    let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &op.image, snapshot: &input, control: &control, writable: &[], limits: &limits, entry: vec!["__drive_test".into()],
        network: "none", volumes: vec![], root: false, home_mb: 0, scratch: false, workspace: Some((volume, false)) });
    let _ = std::fs::remove_dir_all(&control);
    let run = match run {
        Ok(r) => r,
        Err(e) if e.0.contains("timed out") => return Ok(if cut { Side::Late } else { Side::Slow(format!("the run was stopped: {}", e.0)) }),
        Err(e) if e.0.contains("exceeded limits") => return Ok(Side::Nothing(format!("the run was stopped: {}", e.0))),
        Err(e) => return Err(e),
    };
    let parts = match crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) {
        Ok(p) => p,
        Err(e) => return Ok(Side::Nothing(crate::sandbox::refusal(&run.errors).map_or(e.0, |r| format!("Kern could not run the tests in the sandbox: {r}")))),
    };
    std::fs::write(out.join(format!("tests-{label}.out")), &parts["output"])?;
    std::fs::write(out.join(format!("tests-{label}.err")), &parts["errors"])?;
    if parts["ending"] == b"timeout" { return Ok(if cut { Side::Late } else { Side::Slow(format!("the tests did not finish in {} minutes", limit / 60)) }); }
    if parts["ending"] == b"exit -1" { return Ok(Side::Nothing("the test runner was stopped by a signal, so Kern does not trust its report".into())); }
    let created: Vec<String> = String::from_utf8_lossy(&parts["created"]).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    Ok(match proof::read_report(tests.runner, &parts["output"]) {
        Ok(r) => Side::Ran(r, created),
        Err(why) => Side::Nothing(no_results(&why, &parts["ending"], &parts["errors"], &parts["output"])),
    })
}

const MAX_PACKAGES: usize = 5;

pub fn test_memory_mb(meminfo: Option<&str>) -> u64 {
    let total_kb = meminfo.and_then(|m| m.lines().find_map(|l| l.strip_prefix("MemTotal:"))).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok());
    total_kb.map_or(4096, |kb| (kb / 1024 * 3 / 4).clamp(4096, 12288))
}

pub fn packages(changed: &[&str], has_manifest: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = BTreeSet::new();
    for p in changed.iter().filter(|p| !p.split('/').any(|c| c == "node_modules") && !p.chars().any(char::is_control)) {
        let mut dir = p.rsplit_once('/').map(|(d, _)| d);
        while let Some(d) = dir {
            if has_manifest(d) { out.insert(d.to_string()); break; }
            dir = d.rsplit_once('/').map(|(x, _)| x);
        }
    }
    out.into_iter().collect()
}

struct Suite { dir: String, old: Option<(Tests, Env)>, new: (Tests, Env) }

pub const LATE: &str = "Kern's time for this check ran out before these tests could run";

pub fn run(out: &Path, op: &Operator, until: u64) -> R<Value> {
    let d = read_json(out.join("drive.json"))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let (base, head) = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let manifest = |commit: &str, dir: &str| -> Option<Vec<u8>> {
        let path = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
        crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok()
    };
    let mut not_run = Vec::new();
    let suites: Vec<Suite> = match plan(manifest(head, "").as_deref()) {
        Plan::Run(t, env) => vec![Suite { dir: String::new(), old: ready(plan(manifest(base, "").as_deref())), new: (t, env) }],
        Plan::Packages | Plan::Nothing => {
            let changed = crate::rules::changed(repo, base, head)?;
            let paths: Vec<&str> = changed.iter().map(|(p, _)| p.as_str()).collect();
            let mut list = Vec::new();
            for dir in packages(&paths, |dir| manifest(head, dir).is_some() || manifest(base, dir).is_some()) {
                let Some(new) = ready(plan(manifest(head, &dir).as_deref())) else {
                    let script = manifest(head, &dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).and_then(|v| v["scripts"]["test"].as_str().map(str::to_string));
                    if let Some(script) = script { not_run.push(json!({ "dir": dir, "why": format!("Kern cannot run this package's test script: {script}") })); }
                    continue;
                };
                if list.len() == MAX_PACKAGES { not_run.push(json!({ "dir": dir, "why": format!("Kern runs the tests of at most {MAX_PACKAGES} packages per change") })); continue; }
                list.push(Suite { old: ready(plan(manifest(base, &dir).as_deref())), dir, new });
            }
            list
        }
    };
    if suites.is_empty() {
        return Ok(json!({ "status": "NOT_RUN", "why": "no test runner Kern can read was found in package.json, at the root or in a package this change touches (Kern runs vitest, jest, mocha and node --test)" }));
    }
    let mut seconds = serde_json::Map::new();
    let mut timed = |name: String, from: u64| { seconds.insert(name, json!((now_ms() - from) / 1000)); };
    let at = now_ms();
    if !out.join("setup.json").exists() { crate::drive::setup(out, op, until)?; timed("setup".into(), at); }
    let s = read_json(out.join("setup.json"))?;
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    const LISTS: [&str; 11] = ["broke", "flaky", "gone", "newly_skipped", "added", "new_failing", "already_failing", "new_problems", "unconfirmed", "environmental", "together"];
    let mut merged = serde_json::Map::new();
    for l in LISTS { merged.insert(l.into(), json!([])); }
    merged.insert("problem_why".into(), json!({}));
    let (mut sums, mut kept) = ([0u64; 6], 0u64);
    let (mut ran, mut all_new, mut wrote_code, mut run_broke, mut run_unconfirmed) = (Vec::new(), Vec::new(), BTreeSet::new(), Vec::new(), Vec::new());
    let mut late = false;
    let changes = crate::rules::changed(repo, base, head)?;
    for suite in &suites {
        if late { not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
    let label = if suite.dir.is_empty() { String::new() } else { format!("-{}", suite.dir.replace('/', "_")) };
        let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
        let at = now_ms();
        let new_side = (&suite.new.0, &suite.new.1);
        let first = match run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new{label}"), until) {
            Ok(s) => s,
            Err(e) => { not_run.push(json!({ "dir": suite.dir, "why": format!("Kern could not run the tests{place}: {}", e.0) })); continue; }
        };
        timed(format!("new{label}"), at);
        let inside = |p: &str| suite.dir.is_empty() || p.starts_with(&format!("{}/", suite.dir));
        let changed_code = |skip: &str| -> BTreeSet<String> { changes.iter().filter(|(p, o)| *o != skip && inside(p) && crate::capability::is_code(p)).map(|(p, _)| p.clone()).collect() };
        let narrowed = match (&first, &suite.old) {
            (Side::Slow(_), Some((t, e))) => related(&suite.new.0, &changed_code("delete"), &suite.dir).zip(related(t, &changed_code("add"), &suite.dir)).map(|(n, o)| (n, o, e)),
            _ => None,
        };
        let (first, whole_new) = match &narrowed {
            Some((n, _, _)) => {
                let at = now_ms();
                let r = match run_side(out, op, &volume("new"), (n, &suite.new.1), &suite.dir, &format!("new-related{label}"), until) {
                    Ok(s) => s,
                    Err(e) => { not_run.push(json!({ "dir": suite.dir, "why": format!("Kern could not run the tests{place}: {}", e.0) })); continue; }
                };
                timed(format!("new-related{label}"), at);
                (r, (n, &suite.new.1))
            }
            None => (first, new_side),
        };
        let at = now_ms();
        let old_run = match (&suite.old, &narrowed) {
            (None, _) => None,
            (Some(_), Some((_, o, e))) if !matches!(first, Side::Late) => {
                let run = run_side(out, op, &volume("old"), (o, e), &suite.dir, &format!("old-related{label}"), until).unwrap_or_else(|e| Side::Nothing(e.0));
                timed(format!("old-related{label}"), at);
                Some(run)
            }
            (Some((t, env)), None) if !matches!(first, Side::Late) => {
                let run = run_side(out, op, &volume("old"), (t, env), &suite.dir, &format!("old{label}"), until).unwrap_or_else(|e| Side::Nothing(e.0));
                timed(format!("old{label}"), at);
                Some(run)
            }
            (Some(_), _) => Some(Side::Late),
        };
        let old_seconds = (now_ms() - at) / 1000;
        if matches!(first, Side::Late) || matches!(old_run, Some(Side::Late)) { late = true; not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
        let slow = matches!(first, Side::Slow(_)) && old_seconds * 3 > test_seconds();
        let new = match first {
            Side::Ran(r, created) => { wrote_code.extend(created); r }
            Side::Nothing(why) | Side::Slow(why) => {
                if !matches!(old_run, Some(Side::Ran(..))) {
                    not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the new code gave no results: {why}") }));
                    continue;
                }
                if slow {
                    run_unconfirmed.push(json!({ "dir": suite.dir, "why": format!("the tests{place} finished on the old code but not on the new code: {why}") }));
                    continue;
                }
                let at = now_ms();
                let again = run_side(out, op, &volume("new"), whole_new, &suite.dir, &format!("new-again{label}"), until);
                timed(format!("new-again{label}"), at);
                let cut = |why: &str| json!({ "dir": suite.dir, "why": format!("the tests{place} gave no results on the new code once, where the old code's tests ran, and {why}") });
                match again {
                    Ok(Side::Ran(r, created)) => { wrote_code.extend(created); r }
                    Ok(Side::Late) => { late = true; run_unconfirmed.push(cut("Kern's time ran out before it could run them again")); continue; }
                    Err(e) => { run_unconfirmed.push(cut(&format!("running them again failed: {}", e.0))); continue; }
                    Ok(Side::Nothing(again) | Side::Slow(again)) => { run_broke.push(json!({ "dir": suite.dir, "why": again })); continue; }
                }
            }
            Side::Late => continue,
        };
        let old = match old_run {
            None => { all_new.push(if suite.dir.is_empty() { ".".to_string() } else { suite.dir.clone() }); Report::default() }
            Some(Side::Ran(r, _)) => r,
            Some(Side::Nothing(why) | Side::Slow(why)) => { not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the old code gave no results: {why}") })); continue; }
            Some(Side::Late) => continue,
        };
        let suspect = suspects(&old, &new);
        let present = |r: &Report, f: &String| r.problems.iter().any(|p| file_of(p) == f.as_str()) || r.tests.keys().any(|k| file_of(k) == f.as_str());
        let new_files: BTreeSet<String> = suspect.iter().filter(|f| present(&new, f)).cloned().collect();
        let old_files: BTreeSet<String> = suspect.iter().filter(|f| present(&old, f)).cloned().collect();
        let plan: Vec<(&str, &str, Tests, &Env)> = if new_files.is_empty() {
            Vec::new()
        } else if let Some(new_alone) = focused(&suite.new.0, &new_files, &suite.dir) {
            let mut v = vec![("new-alone", "new", new_alone, &suite.new.1)];
            if let Some((t, e)) = &suite.old && let Some(old_alone) = focused(t, &old_files, &suite.dir) { v.push(("old-alone", "old", old_alone, e)); }
            v
        } else {
            vec![("new-again", "new", whole_new.0.clone(), whole_new.1)]
        };
        let (mut alone_new, mut whole, mut second_old, mut unconfirmed) = (None, None, None, false);
        for (name, side, tests, env) in &plan {
            let at = now_ms();
            let r = run_side(out, op, &volume(side), (tests, env), &suite.dir, &format!("{name}{label}"), until);
            timed(format!("{name}{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) if *name == "new-alone" => { wrote_code.extend(created); alone_new = Some(r); }
                Ok(Side::Ran(r, created)) if *side == "new" => { wrote_code.extend(created); whole = Some(r); }
                Ok(Side::Ran(r, _)) => second_old = Some(r),
                Ok(Side::Late) => { late = true; unconfirmed = true; break; }
                Err(_) => { unconfirmed = true; break; }
                Ok(Side::Nothing(_) | Side::Slow(_)) => {}
            }
        }
        if !unconfirmed && plan.first().is_some_and(|p| p.0 == "new-alone") && alone_new.as_ref().is_none_or(|g| !repeats(&old, &new, g)) {
            let at = now_ms();
            let r = run_side(out, op, &volume("new"), whole_new, &suite.dir, &format!("new-again{label}"), until);
            timed(format!("new-again{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) => { wrote_code.extend(created); whole = Some(r); }
                Ok(Side::Late) => { late = true; unconfirmed = true; }
                Err(_) => unconfirmed = true,
                Ok(Side::Nothing(_) | Side::Slow(_)) => {}
            }
        }
        if !plan.is_empty() && alone_new.is_none() && whole.is_none() { unconfirmed = true; }
        let c = if unconfirmed { judged(&old, &new, None, None) } else { confirmed(&old, &new, alone_new.as_ref(), whole.as_ref(), second_old.as_ref()) };
        let runner = suite.new.0.runner;
        let named = |k: &str| if runner == Runner::Node && !suite.dir.is_empty() { format!("{}: {k}", suite.dir) } else { k.to_string() };
        let list = |v: &Value| -> Vec<Value> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|x| json!(named(x))).collect() };
        if c["status"] != "RAN" {
            not_run.push(json!({ "dir": suite.dir, "why": c["why"] }));
            if let Some(a) = merged["new_failing"].as_array_mut() { a.extend(list(&c["new_failing"])); }
            continue;
        }
        for l in LISTS.iter() {
            let target: &str = if unconfirmed && (*l == "broke" || *l == "new_problems") { "unconfirmed" } else { l };
            let items = list(&c[*l]);
            if let Some(a) = merged[target].as_array_mut() { a.extend(items); }
        }
        for (k, w) in c["problem_why"].as_object().into_iter().flatten() { merged["problem_why"][named(k)] = w.clone(); }
        for (i, (side, field)) in [("before", "passed"), ("before", "failed"), ("before", "skipped"), ("after", "passed"), ("after", "failed"), ("after", "skipped")].iter().enumerate() {
            sums[i] += c[*side][*field].as_u64().unwrap_or(0);
        }
        kept += c["kept"].as_u64().unwrap_or(0);
        let only = if narrowed.is_some() { format!(", only the tests related to the change: the whole suite takes over {} minutes", test_seconds() / 60) } else { String::new() };
        ran.push(if suite.dir.is_empty() { format!("{}{only}", runner.name()) } else { format!("{} in {}{only}", runner.name(), suite.dir) });
    }
    let extra = json!({ "not_run": not_run, "run_broke": run_broke, "run_unconfirmed": run_unconfirmed, "wrote_code": wrote_code, "new_failing": merged["new_failing"], "seconds": seconds });
    let with = |mut c: Value| -> Value { for (k, v) in extra.as_object().into_iter().flatten() { if c.get(k).is_none() || k != "new_failing" { c[k] = v.clone(); } } c };
    if ran.is_empty() && !run_broke.is_empty() {
        return Ok(with(json!({ "status": "BROKE", "why": "the tests run on the old code but give no results on the new code" })));
    }
    if ran.is_empty() {
        let why = not_run.first().and_then(|n| n["why"].as_str()).or_else(|| run_unconfirmed.first().and_then(|n| n["why"].as_str())).unwrap_or("no test run gave results").to_string();
        return Ok(with(json!({ "status": "NOT_RUN", "runner": suites[0].new.0.runner.name(), "why": why })));
    }
    let mut c = Value::Object(merged);
    c["status"] = json!("RAN");
    c["before"] = json!({ "passed": sums[0], "failed": sums[1], "skipped": sums[2] });
    c["after"] = json!({ "passed": sums[3], "failed": sums[4], "skipped": sums[5] });
    c["kept"] = json!(kept);
    c["runner"] = json!(ran.join(", "));
    let mut c = with(c);
    if !all_new.is_empty() { c["note"] = json!(format!("the old code had no test runner Kern can read in {}, so those tests are all new", all_new.join(", "))); }
    Ok(c)
}

