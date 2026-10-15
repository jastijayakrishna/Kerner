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
    let (mut out, mut digit) = (String::new(), false);
    for c in name.chars() {
        if c.is_ascii_digit() { if !digit { out.push('#'); } digit = true; } else { out.push(c); digit = false; }
    }
    out
}

fn paired(before: &Report, after: &Report) -> std::collections::BTreeMap<String, String> {
    let mut by_mask: std::collections::BTreeMap<String, Vec<&String>> = std::collections::BTreeMap::new();
    for k in before.tests.keys().filter(|k| !after.tests.contains_key(*k)) { by_mask.entry(masked(k)).or_default().push(k); }
    let (mut map, mut used) = (std::collections::BTreeMap::new(), BTreeSet::new());
    for k in after.tests.keys().filter(|k| !before.tests.contains_key(*k)) {
        if let Some(b) = by_mask.get(&masked(k)).and_then(|c| c.iter().find(|b| !used.contains(**b))) {
            used.insert(*b);
            map.insert(k.clone(), (*b).clone());
        }
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

pub fn compare(before: &Report, after: &Report, again: Option<&Report>) -> Value {
    let map = paired(before, after);
    let (after, again) = (renamed(after, &map), again.map(|g| renamed(g, &map)));
    let (after, again) = (&after, again.as_ref());
    let keys: BTreeSet<&String> = before.tests.keys().chain(after.tests.keys()).collect();
    let (mut kept, mut broke, mut flaky, mut gone, mut newly_skipped, mut added, mut already_failing) = (0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut new_failing = Vec::new();
    for k in keys {
        let (b, a) = (count(before, k), count(after, k));
        if b.total() == 0 {
            added.push(k.clone());
            if a.failed > 0 { new_failing.push(k.clone()); }
            continue;
        }
        if b.failed > 0 { already_failing.push(k.clone()); }
        kept += u64::from(b.passed.min(a.passed));
        if b.passed > 0 && a.failed > b.failed {
            match again.map(|g| count(g, k)) {
                Some(g) if g.failed <= b.failed && g.passed >= b.passed => { flaky.push(k.clone()); kept += u64::from(b.passed - b.passed.min(a.passed)); }
                _ => broke.push(k.clone()),
            }
        } else if b.passed > 0 && a.skipped > b.skipped {
            newly_skipped.push(k.clone());
        } else if b.passed > 0 && a.total() < b.total() && again.is_none_or(|g| count(g, k).total() < b.total()) {
            gone.push(k.clone());
        }
    }
    already_failing.extend(before.problems.iter().cloned());
    let total = |r: &Report| r.tests.values().map(|c| c.total()).sum::<u32>();
    let passed = |r: &Report| r.tests.values().map(|c| c.passed).sum::<u32>();
    if total(before) == 0 && total(after) == 0 && after.problems.iter().all(|p| before.problems.contains(p)) {
        return json!({ "status": "NOT_RUN", "why": "the test runner ran but found no tests on either side" });
    }
    if total(before) > 0 && passed(before) == 0 {
        return json!({ "status": "NOT_RUN", "why": "no test passed on the old code here (they may need the network, a database or a build first), so the tests showed nothing",
            "new_failing": new_failing });
    }
    let new_problems: Vec<&String> = after.problems.iter().filter(|p| !before.problems.contains(*p) && again.is_none_or(|a| a.problems.contains(*p))).collect();
    let problem_why: serde_json::Map<String, Value> = new_problems.iter().filter_map(|p| again.unwrap_or(after).why.get(*p).or_else(|| after.why.get(*p)).map(|w| ((*p).clone(), json!(w)))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(again.unwrap_or(after)), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
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

fn absorb(into: &mut Report, from: Report, dir: &str, runner: Runner) {
    let name = |k: String| if runner == Runner::Node && !dir.is_empty() { format!("{dir}: {k}") } else { k };
    for (k, c) in from.tests {
        let e = into.tests.entry(name(k)).or_default();
        e.passed += c.passed;
        e.failed += c.failed;
        e.skipped += c.skipped;
    }
    into.problems.extend(from.problems.into_iter().map(name));
    into.why.extend(from.why.into_iter().map(|(k, v)| (name(k), v)));
}

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
                let Some(new) = ready(plan(manifest(head, &dir).as_deref())) else { continue };
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
    let (mut before, mut after, mut again) = (Report::default(), Report::default(), Report::default());
    let (mut ran, mut rerun, mut all_new, mut wrote_code, mut run_broke) = (Vec::new(), false, Vec::new(), BTreeSet::new(), Vec::new());
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
                let again = run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new-again{label}"), until)?;
                timed(format!("new-again{label}"), at);
                match again {
                    Side::Ran(r, created) => { wrote_code.extend(created); r }
                    Side::Late => { late = true; not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
                    Side::Nothing(again) | Side::Slow(again) => { run_broke.push(json!({ "dir": suite.dir, "why": again })); continue; }
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
        let fails_new = new.tests.iter().any(|(k, c)| { let b = count(&old, k); b.passed > 0 && c.failed > b.failed }) || new.problems.iter().any(|p| !old.problems.contains(p))
            || old.tests.iter().any(|(k, b)| b.passed > 0 && count(&new, k).total() < b.total());
        let second = if fails_new {
            let at = now_ms();
            let r = run_side(out, op, &volume("new"), new_side, &suite.dir, &format!("new-again{label}"), until)?;
            timed(format!("new-again{label}"), at);
            match r {
                Side::Ran(r, created) => { wrote_code.extend(created); Some(r) }
                Side::Late => { late = true; not_run.push(json!({ "dir": suite.dir, "why": "Kern's time for this check ran out before it could run the failing tests again" })); continue; }
                Side::Nothing(_) | Side::Slow(_) => None,
            }
        } else { None };
        rerun |= second.is_some();
        let runner = suite.new.0.runner;
        absorb(&mut again, second.unwrap_or_else(|| new.clone()), &suite.dir, runner);
        absorb(&mut before, old, &suite.dir, runner);
        absorb(&mut after, new, &suite.dir, runner);
        ran.push(if suite.dir.is_empty() { runner.name().to_string() } else { format!("{} in {}", runner.name(), suite.dir) });
    }
    if ran.is_empty() && !run_broke.is_empty() {
        return Ok(json!({ "status": "BROKE", "why": "the tests run on the old code but give no results on the new code", "run_broke": run_broke, "not_run": not_run, "seconds": seconds }));
    }
    if ran.is_empty() {
        let why = not_run.first().and_then(|n| n["why"].as_str()).unwrap_or("no test run gave results").to_string();
        return Ok(json!({ "status": "NOT_RUN", "runner": suites[0].new.0.runner.name(), "why": why, "not_run": not_run, "seconds": seconds }));
    }
    let mut c = compare(&before, &after, rerun.then_some(&again));
    if c["status"] == "RAN" {
        c["runner"] = json!(ran.join(", "));
        c["wrote_code"] = json!(wrote_code);
    }
    c["not_run"] = json!(not_run);
    c["run_broke"] = json!(run_broke);
    c["seconds"] = json!(seconds);
    if !all_new.is_empty() { c["note"] = json!(format!("the old code had no test runner Kern can read in {}, so those tests are all new", all_new.join(", "))); }
    Ok(c)
}
