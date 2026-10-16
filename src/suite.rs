use crate::policy::{Edits, Kind, Limits, Operator, Runner, Tests};
use crate::proof::{self, Report};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const FOLDERS: [&str; 11] = ["test", "tests", "__tests__", "spec", "specs", "e2e", "__e2e__", "cypress", "playwright", "__mocks__", "__fixtures__"];
const NAMES: [&str; 4] = [".test.", ".spec.", ".e2e.", ".cy."];
const TIMEOUT_SECONDS: u64 = 900;
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
    let (file, rest) = match rest.split_once(" > ") { Some((f, r)) if f.contains('/') || f.contains('.') => (format!("{f} > "), r), _ => (String::new(), rest) };
    let (mut out, mut digit) = (folder + &file, false);
    for c in rest.chars() {
        if c.is_ascii_digit() { if !digit { out.push('#'); } digit = true; } else { out.push(c); digit = false; }
    }
    out
}

fn rank(c: proof::Count) -> u8 { if c.failed > 0 { 2 } else if c.passed > 0 { 0 } else if c.skipped > 0 { 1 } else { 3 } }

fn paired(before: &Report, after: &Report) -> std::collections::BTreeMap<String, String> {
    let mut groups: std::collections::BTreeMap<String, (Vec<&String>, Vec<&String>)> = std::collections::BTreeMap::new();
    for k in before.tests.keys().filter(|k| !after.tests.contains_key(*k)) { groups.entry(masked(k)).or_default().0.push(k); }
    for k in after.tests.keys().filter(|k| !before.tests.contains_key(*k)) { groups.entry(masked(k)).or_default().1.push(k); }
    let mut map = std::collections::BTreeMap::new();
    for (mut b, mut a) in groups.into_values().filter(|(b, a)| b.len() == a.len()) {
        for r in 0..4u8 {
            let olds: Vec<&String> = b.iter().copied().filter(|k| rank(count(before, k)) == r).collect();
            let news: Vec<&String> = a.iter().copied().filter(|k| rank(count(after, k)) == r).collect();
            for (x, y) in news.into_iter().zip(olds) {
                map.insert(x.clone(), y.clone());
                a.retain(|k| *k != x);
                b.retain(|k| *k != y);
            }
        }
        for (x, y) in a.into_iter().zip(b) { map.insert(x.clone(), y.clone()); }
    }
    map
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

fn has(list: &[String], p: &str) -> bool { list.iter().any(|q| q == p || (q.contains(" > ") && masked(q) == masked(p))) }

fn settled(base: &Report, r: &Report) -> Report {
    let mut r = renamed(r, &paired(base, r));
    let broken: BTreeSet<String> = r.problems.iter().map(|p| file_of(p).to_string()).collect();
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
            if tests.command.iter().skip(2).any(|a| !a.starts_with('-')) { return None; }
            for f in files { t.command.push(f.strip_prefix(&prefix)?.to_string()); }
        }
        Runner::Mocha | Runner::Node => return None,
    }
    Some(t)
}

pub fn worse(alone: &Report, whole: &Report) -> Report {
    let alone = renamed(alone, &paired(whole, alone));
    let broken: BTreeSet<String> = alone.problems.iter().map(|p| file_of(p).to_string()).collect();
    let mut out = whole.clone();
    for (k, c) in out.tests.iter_mut() {
        match alone.tests.get(k) {
            Some(a) => {
                c.passed = c.passed.min(a.passed);
                c.failed = c.failed.max(a.failed);
                c.skipped = c.skipped.max(a.skipped);
            }
            None if broken.contains(file_of(k)) => { c.failed = c.failed.max(c.total()); c.passed = 0; }
            None => {}
        }
    }
    for p in &alone.problems { if !has(&out.problems, p) { out.problems.push(p.clone()); } }
    for (k, w) in &alone.why { out.why.entry(k.clone()).or_insert_with(|| w.clone()); }
    out
}

fn repeats(old: &Report, new: &Report, again: &Report) -> bool {
    let (new, again) = (settled(old, new), settled(old, again));
    old.tests.iter().filter(|(_, b)| b.passed > 0).all(|(k, b)| seen(*b, count(&new, k)) == Seen::Same || seen(*b, count(&again, k)) != Seen::Same)
        && new.problems.iter().filter(|p| !has(&old.problems, p)).all(|p| has(&again.problems, p))
}

fn suspects(old: &Report, new: &Report) -> BTreeSet<String> {
    let settled_new = settled(old, new);
    let tests = old.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&settled_new, k)) != Seen::Same).map(|(k, _)| file_of(k).to_string());
    let problems = new.problems.iter().filter(|p| !has(&old.problems, p)).map(|p| file_of(p).to_string());
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
    let candidates: Vec<&String> = after.problems.iter().filter(|p| !has(&before.problems, p)).collect();
    let (new_problems, unsteady): (Vec<&String>, Vec<&String>) = candidates.into_iter()
        .partition(|p| again.is_none_or(|g| has(&g.problems, p)) && alone_old.is_none_or(|o| !has(&o.problems, p)));
    let reported: BTreeSet<&str> = new_problems.iter().map(|p| file_of(p)).collect();
    let shaky: BTreeSet<&str> = unsteady.iter().map(|p| file_of(p)).collect();
    let keys: BTreeSet<&String> = before.tests.keys().chain(after.tests.keys()).collect();
    let (mut kept, mut broke, mut flaky, mut gone, mut newly_skipped, mut added, mut already_failing) = (0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut new_failing = Vec::new();
    for k in keys {
        let (b, a) = (count(before, k), count(after, k));
        if b.passed == 0 && b.failed == 0 {
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
        let environmental = if verdict == Seen::Gone { old_alone == Seen::Gone } else { old_alone != Seen::Same };
        if second == Seen::Same || environmental {
            kept += u64::from(b.passed - b.passed.min(a.passed));
            if !shaky.contains(file_of(k)) { flaky.push(k.clone()); }
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
    flaky.extend(unsteady.iter().map(|p| (*p).clone()));
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
        new_failing.extend(raw_after.problems.iter().filter(|p| !has(&before.problems, p)).cloned());
        return json!({ "status": "NOT_RUN", "why": why, "new_failing": new_failing });
    }
    let problem_why: serde_json::Map<String, Value> = new_problems.iter().filter_map(|p| again.unwrap_or(after).why.get(*p).or_else(|| after.why.get(*p)).map(|w| ((*p).clone(), json!(w)))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(raw_after), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
        "newly_skipped": newly_skipped, "added": added, "new_failing": new_failing, "already_failing": already_failing, "new_problems": new_problems, "problem_why": problem_why })
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
    let seconds = within(until, TIMEOUT_SECONDS);
    if seconds < 60 { return Ok(Side::Late); }
    let cut = seconds < TIMEOUT_SECONDS;
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
    if parts["ending"] == b"timeout" { return Ok(if cut { Side::Late } else { Side::Slow(format!("the tests did not finish in {} minutes", TIMEOUT_SECONDS / 60)) }); }
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
    const LISTS: [&str; 9] = ["broke", "flaky", "gone", "newly_skipped", "added", "new_failing", "already_failing", "new_problems", "unconfirmed"];
    let mut merged = serde_json::Map::new();
    for l in LISTS { merged.insert(l.into(), json!([])); }
    merged.insert("problem_why".into(), json!({}));
    let (mut sums, mut kept) = ([0u64; 6], 0u64);
    let (mut ran, mut all_new, mut wrote_code, mut run_broke, mut run_unconfirmed) = (Vec::new(), Vec::new(), BTreeSet::new(), Vec::new(), Vec::new());
    let mut late = false;
    for suite in &suites {
        if late { not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
    let label = if suite.dir.is_empty() { String::new() } else { format!("-{}", suite.dir.replace('/', "_")) };
        let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
        let at = now_ms();
        let new_side = (&suite.new.0, &suite.new.1);
        let first = run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new{label}"), until)?;
        timed(format!("new{label}"), at);
        let at = now_ms();
        let old_run = match &suite.old {
            None => None,
            Some((t, env)) if !matches!(first, Side::Late) => {
                let run = run_side(out, op, &volume("old"), (t, env), &suite.dir, &format!("old{label}"), until)?;
                timed(format!("old{label}"), at);
                Some(run)
            }
            Some(_) => Some(Side::Late),
        };
        let old_seconds = (now_ms() - at) / 1000;
        if matches!(first, Side::Late) || matches!(old_run, Some(Side::Late)) { late = true; not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
        let slow = matches!(first, Side::Slow(_)) && old_seconds * 3 > TIMEOUT_SECONDS;
        let new = match first {
            Side::Ran(r, created) => { wrote_code.extend(created); r }
            Side::Nothing(why) | Side::Slow(why) => {
                if !matches!(old_run, Some(Side::Ran(..))) || slow {
                    not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the new code gave no results: {why}") }));
                    continue;
                }
                let at = now_ms();
                let again = run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new-again{label}"), until);
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
            vec![("new-again", "new", suite.new.0.clone(), &suite.new.1)]
        };
        let (mut second, mut second_old, mut unconfirmed) = (None, None, false);
        for (name, side, tests, env) in &plan {
            let at = now_ms();
            let r = run_side(out, op, &volume(side), (tests, env), &suite.dir, &format!("{name}{label}"), until);
            timed(format!("{name}{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) if *side == "new" => { wrote_code.extend(created); second = Some(r); }
                Ok(Side::Ran(r, _)) => second_old = Some(r),
                Ok(Side::Late) => { late = true; unconfirmed = true; break; }
                Err(_) => { unconfirmed = true; break; }
                Ok(Side::Nothing(_) | Side::Slow(_)) => {}
            }
        }
        if !unconfirmed && plan.first().is_some_and(|p| p.0 == "new-alone") && second.as_ref().is_none_or(|g| !repeats(&old, &new, g)) {
            let at = now_ms();
            let r = run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new-again{label}"), until);
            timed(format!("new-again{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) => { wrote_code.extend(created); second = Some(match second { Some(g) => worse(&g, &r), None => r }); }
                Ok(Side::Late) => { late = true; unconfirmed = true; }
                Err(_) => unconfirmed = true,
                Ok(Side::Nothing(_) | Side::Slow(_)) => second = None,
            }
        }
        if unconfirmed { second = None; second_old = None; }
        let c = judged(&old, &new, second.as_ref(), second_old.as_ref());
        let runner = suite.new.0.runner;
        let named = |k: &str| if runner == Runner::Node && !suite.dir.is_empty() { format!("{}: {k}", suite.dir) } else { k.to_string() };
        let list = |v: &Value| -> Vec<Value> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|x| json!(named(x))).collect() };
        if c["status"] != "RAN" {
            not_run.push(json!({ "dir": suite.dir, "why": c["why"] }));
            if let Some(a) = merged["new_failing"].as_array_mut() { a.extend(list(&c["new_failing"])); }
            continue;
        }
        for l in LISTS.iter().filter(|l| **l != "unconfirmed") {
            let target: &str = if unconfirmed && (*l == "broke" || *l == "new_problems") { "unconfirmed" } else { l };
            let items = list(&c[*l]);
            if let Some(a) = merged[target].as_array_mut() { a.extend(items); }
        }
        for (k, w) in c["problem_why"].as_object().into_iter().flatten() { merged["problem_why"][named(k)] = w.clone(); }
        for (i, (side, field)) in [("before", "passed"), ("before", "failed"), ("before", "skipped"), ("after", "passed"), ("after", "failed"), ("after", "skipped")].iter().enumerate() {
            sums[i] += c[*side][*field].as_u64().unwrap_or(0);
        }
        kept += c["kept"].as_u64().unwrap_or(0);
        ran.push(if suite.dir.is_empty() { runner.name().to_string() } else { format!("{} in {}", runner.name(), suite.dir) });
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

