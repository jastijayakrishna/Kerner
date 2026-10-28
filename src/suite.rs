use crate::policy::{Edits, Kind, Limits, Operator, Runner, Tests};
use crate::proof::{self, Report};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

const FOLDERS: [&str; 11] = ["test", "tests", "__tests__", "spec", "specs", "e2e", "__e2e__", "cypress", "playwright", "__mocks__", "__fixtures__"];
const NAMES: [&str; 4] = [".test.", ".spec.", ".e2e.", ".cy."];
pub fn test_seconds() -> u64 {
    std::env::var("KERN_TEST_MINUTES").ok().and_then(|m| m.trim().parse::<u64>().ok()).filter(|m| (1..=crate::sandbox::TEST_SECS_MAX / 60).contains(m)).unwrap_or(15) * 60
}
const RUN_SECS_MIN: u64 = 60;
pub const RERUNS_SECS_RESERVE: u64 = 300;
const SLOWER_SECS_EXTRA: u64 = 60;
const HANG_SECS_EXTRA: u64 = 120;
const HANG_SECS_MIN: u64 = 300;
fn room(secs: u64, last: bool) -> u64 { if last { test_seconds().max(secs).min(crate::sandbox::TEST_SECS_MAX) } else { test_seconds() } }
pub fn later_secs(old_secs: u64, last: bool) -> u64 { room(old_secs.saturating_mul(3) / 2 + SLOWER_SECS_EXTRA, last) }
pub fn new_secs(old_secs: u64, last: bool) -> u64 { later_secs(old_secs, last).min(old_secs.saturating_mul(4).saturating_add(HANG_SECS_EXTRA).max(HANG_SECS_MIN)) }
pub fn related_secs(whole_left_secs: u64, last: bool) -> u64 { room(whole_left_secs.saturating_sub(test_seconds() + RERUNS_SECS_RESERVE) / 2, last) }
pub fn early_secs(limit: u64, seconds: u64) -> u64 { if limit >= test_seconds() { seconds.min(test_seconds()) } else { 0 } }
pub const REPORT: &str = "/tmp/.kern-report.json";

pub fn test_command(tests: &Tests) -> Vec<String> {
    let mut c = proof::command(tests);
    match tests.runner {
        Runner::Vitest => c.extend([format!("--outputFile={REPORT}"), "--bail=0".into(), "--sequence.shuffle=false".into(), crate::sandbox::PROGRESS_ARGS[0].to_string()]),
        Runner::Mocha => c.extend(["--reporter-options".to_string(), format!("output={REPORT}")]),
        Runner::Node => { if let Some(at) = c.iter().position(|a| a == crate::sandbox::NODE_REPORTER) { c.insert(at + 1, format!("--test-reporter-destination={REPORT}")); } }
        Runner::Jest => c.extend(std::iter::once("--bail=false").chain(crate::sandbox::PROGRESS_ARGS[1..].iter().copied()).map(str::to_string)),
    }
    c
}
pub fn worker_heap_mb(memory_mb: u64, cpus: u64) -> u64 { memory_mb / cpus.max(2) }
fn box_memory_mb() -> u64 { test_memory_mb(std::fs::read_to_string("/proc/meminfo").ok().as_deref()) }
pub fn sized_env(runner: Runner, env: &Env) -> Env {
    let mut out = env.clone();
    if matches!(runner, Runner::Jest | Runner::Vitest) {
        let memory = box_memory_mb();
        let heap = format!("--max-old-space-size={}", worker_heap_mb(memory, crate::sandbox::box_cpus(memory, host_cpus())));
        let options = out.get("NODE_OPTIONS").map_or(heap.clone(), |o| format!("{o} {heap}"));
        out.insert("NODE_OPTIONS".to_string(), options);
    }
    out
}
pub fn host_cpus() -> u64 { std::thread::available_parallelism().map_or(1, |n| n.get() as u64) }

fn runner(package: Option<&[u8]>) -> Option<(&'static str, Vec<String>)> {
    let pkg: Value = serde_json::from_slice(package?).ok()?;
    let has = |name: &str| ["devDependencies", "dependencies"].iter().any(|k| pkg[*k].get(name).is_some());
    let bin = |name: &str| format!("/workspace/node_modules/.bin/{name}");
    if has("vitest") { return Some(("vitest", vec![bin("vitest"), "run".into()])); }
    if let Some(name) = ["jest", "mocha"].into_iter().find(|n| has(n)) { return Some((name, vec![bin(name)])); }
    pkg["scripts"]["test"].as_str().filter(|t| t.contains("node --test")).map(|_| ("node", vec!["/usr/local/bin/node".into(), "--test".into()]))
}
pub fn detect(package: Option<&[u8]>) -> Option<Tests> {
    let (name, command) = runner(package)?;
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
pub fn by_config(has: impl Fn(&str) -> bool) -> Option<(Tests, Env)> {
    let any = |stem: &str| ["ts", "mts", "cts", "js", "mjs", "cjs", "json"].iter().any(|e| has(&format!("{stem}.{e}")));
    let (runner, command) = if any("vitest.config") { (Runner::Vitest, vec!["/workspace/node_modules/.bin/vitest".to_string(), "run".to_string()]) }
        else if any("jest.config") { (Runner::Jest, vec!["/workspace/node_modules/.bin/jest".to_string()]) }
        else { return None };
    Some((Tests { paths: Vec::new(), kind: Kind::Feature, runner, command, edits: Edits::Checked }, Env::new()))
}

fn test_grant(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let lower: Vec<String> = parts.iter().map(|p| p.to_ascii_lowercase()).collect();
    if let Some(i) = lower[..lower.len().saturating_sub(1)].iter().position(|d| FOLDERS.contains(&d.as_str())) { return Some(format!("{}/", parts[..=i].join("/"))); }
    let name = lower.last()?;
    let plain = name.split_once('.').is_some_and(|(stem, ext)| ["test", "spec"].contains(&stem) && !ext.contains('.') && crate::capability::is_code(name));
    (plain || NAMES.iter().any(|m| name.contains(m))).then(|| path.to_string())
}

pub fn is_test_path(path: &str) -> bool { test_grant(path).is_some() }
pub fn repo_has_tests(paths: &[String], dir: &str) -> bool {
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    paths.iter().any(|p| p.starts_with(&prefix) && !p.split('/').any(|s| s == "node_modules") && crate::capability::is_code(p) && is_test_path(p))
}

pub fn test_paths(paths: &[&str]) -> Vec<String> { paths.iter().filter_map(|p| test_grant(p)).collect::<BTreeSet<_>>().into_iter().collect() }
fn sum(r: &Report) -> Value {
    let (p, f, s) = r.tests.values().fold((0, 0, 0), |(p, f, s), c| (p + c.passed, f + c.failed, s + c.skipped));
    json!({ "passed": p, "failed": f, "skipped": s })
}

fn count(r: &Report, k: &str) -> proof::Count { r.tests.get(k).copied().unwrap_or_default() }
fn names_by_file(r: &Report) -> std::collections::BTreeMap<&str, BTreeSet<&str>> {
    let mut out: std::collections::BTreeMap<&str, BTreeSet<&str>> = std::collections::BTreeMap::new();
    for k in r.tests.keys() { out.entry(file_of(k)).or_default().insert(k.as_str()); }
    out
}

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
    let last = |k: &str| masked(k.rsplit(" > ").next().unwrap_or(k));
    let used: BTreeSet<String> = map.values().cloned().collect();
    let lost: Vec<&String> = before.tests.keys().filter(|k| !after.tests.contains_key(*k) && !used.contains(*k)).collect();
    let found: Vec<&String> = after.tests.keys().filter(|k| !before.tests.contains_key(*k) && !map.contains_key(*k)).collect();
    pair_groups(before, after, &lost, &found, |k| format!("{} > {}", file_of(k), last(k)), &mut map);
    let used: BTreeSet<String> = map.values().cloned().collect();
    let lost: Vec<&String> = lost.into_iter().filter(|k| !used.contains(*k)).collect();
    let found: Vec<&String> = found.into_iter().filter(|k| !map.contains_key(*k) && crate::capability::is_code(file_of(k)) && !old_files.contains(file_of(k)) && rank(count(after, k)) <= 1).collect();
    pair_groups(before, after, &lost, &found, last, &mut map);
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

pub(crate) fn file_of(key: &str) -> &str { key.split(" > ").next().unwrap_or(key) }
const RANDOM_RUN_CHARS_MIN: usize = 8;
pub fn path_shape(path: &str) -> String {
    let chars: Vec<char> = path.chars().collect();
    let (mut out, mut i) = (String::new(), 0);
    while i < chars.len() {
        let end = (i..chars.len()).find(|&j| !(chars[j].is_ascii_hexdigit() || chars[j] == '-')).unwrap_or(chars.len());
        if end - i >= RANDOM_RUN_CHARS_MIN && chars[i..end].iter().any(char::is_ascii_digit) { out.push('#'); i = end; } else { out.push(chars[i]); i += 1; }
    }
    out
}

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
const NODE_VALUED: [&str; 24] = ["--import", "--require", "-r", "--loader", "--experimental-loader", "--conditions", "-C", "--env-file", "--env-file-if-exists", "--test-name-pattern",
    "--test-skip-pattern", "--test-concurrency", "--test-timeout", "--test-reporter", "--test-reporter-destination", "--test-shard", "--test-isolation", "--test-coverage-include",
    "--test-coverage-exclude", "--test-global-setup", "--input-type", "--title", "--experimental-config-file", "--watch-path"];
pub fn node_files(tests: &Tests, files: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    if files.is_empty() || files.len() > FOCUS || tests.command.is_empty() { return None; }
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let (mut command, mut value) = (vec![tests.command[0].clone()], false);
    for a in &tests.command[1..] {
        if value { command.push(a.clone()); value = false; continue; }
        if a.starts_with('-') { value = NODE_VALUED.contains(&a.as_str()); command.push(a.clone()); }
    }
    for f in files { command.push(f.strip_prefix(&prefix)?.to_string()); }
    let mut t = tests.clone();
    t.command = command;
    Some(t)
}
pub fn focused(tests: &Tests, files: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    if tests.runner == Runner::Node { return node_files(tests, files, dir); }
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

pub fn verdict(old: &Report, new: &Report, alone: Option<&Report>, whole: Option<&Report>, old_alone: Option<&Report>, unconfirmed: bool, uneven: bool) -> Value {
    let c = steady(if unconfirmed { judged(old, new, None, None) } else { confirmed(old, new, alone, whole, old_alone) });
    if unconfirmed { prove(c, old, new, ([None, None], None), uneven) } else { prove(c, old, new, ([alone, whole], old_alone), uneven) }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observed { Passed, Failed, Skipped, Missing }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Evidence { pub old: Observed, pub old_again: Option<Observed>, pub new: Observed, pub new_again: [Option<Observed>; 2], pub shaky: bool, pub sandbox: bool, pub uneven: bool }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unproven { OldFailsToo, Sandbox, Unsteady, Unconfirmed, Uneven }
impl Unproven {
    pub fn list(self) -> &'static str {
        match self { Unproven::OldFailsToo => "environmental", Unproven::Sandbox => "sandbox", Unproven::Unsteady => "unsteady", Unproven::Unconfirmed => "unconfirmed", Unproven::Uneven => "uneven" }
    }
}
pub fn blockable(e: Evidence) -> Result<(), Unproven> {
    if e.old_again == Some(Observed::Failed) { return Err(Unproven::OldFailsToo); }
    if e.old != Observed::Passed { return Err(Unproven::Unconfirmed); }
    if e.sandbox { return Err(Unproven::Sandbox); }
    let reruns: Vec<Observed> = e.new_again.iter().flatten().copied().filter(|o| *o != Observed::Missing).collect();
    if e.new != Observed::Failed || e.shaky || reruns.iter().any(|o| *o != Observed::Failed) { return Err(Unproven::Unsteady); }
    if reruns.is_empty() || e.old_again != Some(Observed::Passed) { return Err(Unproven::Unconfirmed); }
    if e.uneven { return Err(Unproven::Uneven); }
    Ok(())
}
const SANDBOX_ERRORS: [&str; 37] = ["getaddrinfo", "EAI_AGAIN", "ENOTFOUND", "ECONNREFUSED", "ECONNRESET", "ETIMEDOUT", "ENETUNREACH", "EHOSTUNREACH", "fetch failed",
    "git: not found", "command not found", "Cannot find module 'bindings'", "Could not locate the bindings file", "invalid ELF header", "compiled against a different Node.js version",
    "NODE_MODULE_VERSION", "ERR_DLOPEN_FAILED", "cannot open shared object file", "SIGSEGV", "SIGKILL", "SIGABRT", "SIGBUS", "terminated by another process", "Worker exited unexpectedly",
    "JavaScript heap out of memory", "Reached heap limit", "ERR_WORKER_OUT_OF_MEMORY", "Allocation failed", "ENOSPC", "no space left on device", "ENOMEM", "Cannot allocate memory",
    "EMFILE", "ENFILE", "too many open files", "EAGAIN", "Resource temporarily unavailable"];
pub fn sandbox_error(why: &str) -> bool { SANDBOX_ERRORS.iter().any(|e| why.contains(e)) || (why.contains("spawn ") && why.contains("ENOENT")) }
fn outcome(b: proof::Count, c: proof::Count) -> Observed {
    if c.total() == 0 { return Observed::Missing; }
    match seen(b, c) { Seen::Same => Observed::Passed, Seen::Failed => Observed::Failed, Seen::Skipped => Observed::Skipped, Seen::Gone => Observed::Missing }
}
fn test_evidence(k: &str, old: &Report, new: &Report, again: &[Option<Report>; 2], old_again: Option<&Report>) -> Evidence {
    let b = count(old, k);
    let at = |r: &Report| outcome(b, count(r, k));
    Evidence { old: if b.passed > 0 && b.failed == 0 { Observed::Passed } else { Observed::Missing }, old_again: old_again.map(at), new: at(new),
        new_again: [again[0].as_ref().map(at), again[1].as_ref().map(at)], shaky: false, sandbox: false, uneven: false }
}
fn problem_evidence(p: &str, old: &Report, new: &Report, again: [Option<&Report>; 2], old_again: Option<&Report>) -> Evidence {
    let ran_clean = |r: &Report| r.tests.iter().any(|(k, n)| file_of(k) == file_of(p) && n.passed > 0);
    let has = |r: &Report| matched(&[p.to_string()], &r.problems).first().copied().unwrap_or(false);
    let at = |r: &Report| if has(r) { Observed::Failed } else if ran_clean(r) { Observed::Passed } else { Observed::Missing };
    Evidence { old: if !has(old) && ran_clean(old) { Observed::Passed } else { Observed::Missing }, old_again: old_again.map(at), new: at(new), new_again: again.map(|g| g.map(at)), shaky: false, sandbox: false,
        uneven: false }
}
fn prove(mut c: Value, old: &Report, new: &Report, (again, old_again): ([Option<&Report>; 2], Option<&Report>), uneven: bool) -> Value {
    if c["status"] != "RAN" { return c; }
    let (settled_new, settled_again, settled_old_again) = (settled(old, new), again.map(|g| g.map(|r| settled(old, r))), old_again.map(|r| settled(old, r)));
    let shaky: BTreeSet<String> = listed(&c, "flaky").iter().chain(&listed(&c, "unsteady")).map(|k| file_of(k).to_string()).collect();
    let signature = |w: &str| -> String { masked(&w.replace("/workspace/", "")).chars().take(160).collect() };
    let old_errors: BTreeSet<String> = old.tests.iter().filter(|(_, n)| n.failed > 0).map(|(k, _)| k).chain(&old.problems).filter_map(|k| old.why.get(k)).map(|w| signature(w)).collect();
    let why = |k: &str| new.why.get(k).or_else(|| new.why.iter().find(|(n, _)| masked(n) == masked(k)).map(|(_, w)| w));
    let sandboxed = |k: &str| why(k).is_some_and(|w| sandbox_error(w) || old_errors.contains(&signature(w)));
    for l in ["broke", "new_problems"] {
        let mut held = Vec::new();
        for k in listed(&c, l) {
            let e = if l == "broke" { test_evidence(&k, old, &settled_new, &settled_again, settled_old_again.as_ref()) } else { problem_evidence(&k, old, new, again, old_again) };
            match blockable(Evidence { shaky: shaky.contains(file_of(&k)), sandbox: sandboxed(&k), uneven, ..e }) {
                Ok(()) => held.push(k),
                Err(u) => match c[u.list()].as_array_mut() { Some(a) => a.push(json!(k)), None => c[u.list()] = json!([k]) },
            }
        }
        c[l] = json!(held);
    }
    c
}
fn listed(v: &Value, k: &str) -> Vec<String> { v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect() }
pub fn confirmed(before: &Report, after: &Report, alone: Option<&Report>, whole: Option<&Report>, alone_old: Option<&Report>) -> Value {
    let (Some(a), Some(w)) = (alone, whole) else { return judged(before, after, alone.or(whole), alone_old) };
    let mut c = judged(before, after, Some(a), alone_old);
    let wide = judged(before, after, Some(w), alone_old);
    if c["status"] != "RAN" || wide["status"] != "RAN" { return c; }
    let narrow: BTreeSet<String> = listed(&c, "broke").into_iter().chain(listed(&c, "new_problems")).collect();
    let together: Vec<String> = listed(&wide, "broke").into_iter().chain(listed(&wide, "new_problems")).filter(|k| !narrow.contains(k)).collect();
    let passed_again: BTreeSet<String> = listed(&wide, "flaky").into_iter().collect();
    let mut unsteady = Vec::new();
    for l in ["broke", "new_problems"] {
        let (moved, held): (Vec<String>, Vec<String>) = listed(&c, l).into_iter().partition(|k| passed_again.contains(k));
        c[l] = json!(held);
        unsteady.extend(moved);
    }
    let mut gone = listed(&c, "gone");
    for g in listed(&wide, "gone") { if !gone.contains(&g) { gone.push(g); } }
    let mut flaky = listed(&c, "flaky");
    for f in listed(&wide, "flaky") { if !flaky.contains(&f) { flaky.push(f); } }
    flaky.retain(|k| !together.contains(k) && !gone.contains(k) && !unsteady.contains(k));
    c["together"] = json!(together);
    c["gone"] = json!(gone);
    c["flaky"] = json!(flaky);
    c["unsteady"] = json!(unsteady);
    c
}
pub const LISTS: [&str; 20] = ["broke", "flaky", "gone", "newly_skipped", "added", "new_failing", "already_failing", "new_problems", "unconfirmed", "environmental", "together", "rewritten",
    "same_failure", "uneven", "unproven", "unsteady", "sandbox", "unrunnable", "old_sandbox", "edit_fails"];
pub fn in_suite(runner: Runner, dir: &str, k: &str) -> String { if runner == Runner::Node && !dir.is_empty() && !k.contains(" > ") { format!("{dir}: {k}") } else { k.to_string() } }
pub fn fold(merged: &mut serde_json::Map<String, Value>, c: &Value, named: &dyn Fn(&str) -> String) {
    for l in LISTS {
        let items: Vec<Value> = c[l].as_array().into_iter().flatten().filter_map(Value::as_str).map(|k| json!(named(k))).collect();
        if let Some(a) = merged.get_mut(l).and_then(Value::as_array_mut) { a.extend(items); }
    }
    if let Some(why) = merged.get_mut("problem_why") { for (k, w) in c["problem_why"].as_object().into_iter().flatten() { why[named(k)] = w.clone(); } }
}
pub fn steady(mut c: Value) -> Value {
    let shaky: BTreeSet<String> = listed(&c, "flaky").iter().chain(&listed(&c, "unsteady")).map(|k| file_of(k).to_string()).collect();
    if shaky.is_empty() || c["status"] != "RAN" { return c; }
    let mut unsteady = listed(&c, "unsteady");
    for l in ["broke", "new_problems"] {
        let (moved, held): (Vec<String>, Vec<String>) = listed(&c, l).into_iter().partition(|k| shaky.contains(file_of(k)));
        c[l] = json!(held);
        unsteady.extend(moved);
    }
    c["unsteady"] = json!(unsteady);
    c
}

fn repeats(old: &Report, new: &Report, again: &Report) -> bool {
    let (new, again) = (settled(old, new), settled(old, again));
    old.tests.iter().filter(|(_, b)| b.passed > 0).all(|(k, b)| seen(*b, count(&new, k)) == Seen::Same || seen(*b, count(&again, k)) != Seen::Same)
        && matched(&fresh_problems(old, &new), &again.problems).into_iter().all(|x| x)
}

fn suspects(old: &Report, new: &Report) -> BTreeSet<String> {
    let settled_new = settled(old, new);
    let back: std::collections::BTreeMap<String, String> = paired(old, new).into_iter().map(|(n, o)| (o, n)).collect();
    let tests: Vec<String> = old.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&settled_new, k)) != Seen::Same)
        .flat_map(|(k, _)| [Some(file_of(k).to_string()), back.get(k).map(|n| file_of(n).to_string())].into_iter().flatten()).collect();
    let problems = fresh_problems(old, new).into_iter().map(|p| file_of(&p).to_string()).collect::<Vec<_>>();
    tests.into_iter().chain(problems).collect()
}

#[derive(Default)]
pub struct Kept {
    pub files: std::collections::BTreeMap<String, String>, pub old_files: std::collections::BTreeMap<String, String>, pub run: BTreeSet<String>,
    pub back: std::collections::BTreeMap<String, String>, pub skipped: Vec<(String, String)>, pub data: Vec<String>,
}
const KEPT_BYTES: usize = 3 << 20;
pub const NODE_FILES: &str = "node's test runner does not say which file each test is in, so Kern cannot run the old version of an edited test file";
const DATA_FOLDERS: [&str; 10] = ["fixtures", "__fixtures__", "__testfixtures__", "mocks", "__mocks__", "testdata", "test-data", "test_data", "golden", "goldens"];
const SHARED: [&str; 5] = ["spec", "specs", "e2e", "cypress", "playwright"];
const EXTENSIONS: [&str; 9] = ["js", "ts", "mjs", "cjs", "jsx", "tsx", "mts", "cts", "json"];
pub fn tagged(path: &str, tag: &str, n: u32) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let (stem, rest) = name.split_once('.').filter(|(s, _)| !s.is_empty())?;
    let tag = if n == 0 { tag.to_string() } else { format!("{tag}{n}") };
    Some(if dir.is_empty() { format!("{stem}.{tag}.{rest}") } else { format!("{dir}/{stem}.{tag}.{rest}") })
}
pub fn kept_name(path: &str, n: u32) -> Option<String> { tagged(path, "kern-kept", n) }
pub fn kept_back(path: &str) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let (stem, rest) = name.split_once('.')?;
    let rest = rest.strip_prefix("kern-kept")?.trim_start_matches(|c: char| c.is_ascii_digit()).strip_prefix('.')?;
    Some(if dir.is_empty() { format!("{stem}.{rest}") } else { format!("{dir}/{stem}.{rest}") })
}
pub fn polarity(found: &std::collections::BTreeMap<String, proof::Failure>, back: &dyn Fn(&str) -> Option<String>, wanted: &BTreeSet<String>) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for (k, f) in found {
        let file = file_of(k);
        let Some(key) = back(file).map(|o| format!("{o}{}", &k[file.len()..])).filter(|key| wanted.contains(key)) else { continue };
        let r = crate::polarity::read(&f.message, f.expected.as_deref(), f.actual.as_deref());
        out.insert(key, json!({ "kind": r.kind.name(), "was": r.was, "now": r.now, "why": proof::first_lines(&f.message) }));
    }
    out
}
fn snapshot_of(path: &str) -> String { path.rsplit_once('/').map_or_else(|| format!("__snapshots__/{path}.snap"), |(d, n)| format!("{d}/__snapshots__/{n}.snap")) }
fn snapshot_owner(path: &str) -> Option<String> {
    let (dir, file) = path.strip_suffix(".snap")?.rsplit_once('/')?;
    let parent = if dir == "__snapshots__" { "" } else { dir.strip_suffix("/__snapshots__")? };
    Some(if parent.is_empty() { file.to_string() } else { format!("{parent}/{file}") })
}
fn parent(p: &str) -> &str { p.rsplit_once('/').map_or("", |(d, _)| d) }
fn infra(p: &str, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    let rel = if dir.is_empty() { p } else { p.strip_prefix(prefix.as_str()).unwrap_or(p) };
    let parts: Vec<String> = rel.split('/').map(str::to_ascii_lowercase).collect();
    let (folders, name) = parts.split_at(parts.len() - 1);
    if NAMES.iter().any(|m| name[0].contains(m)) { return true; }
    match folders.iter().position(|d| FOLDERS.contains(&d.as_str())) {
        Some(i) => i == 0 || !SHARED.contains(&folders[i].as_str()),
        None => folders.iter().any(|d| DATA_FOLDERS.contains(&d.as_str())),
    }
}
fn normal(dir: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for c in spec.split('/') {
        match c { "" | "." => {} ".." => { parts.pop()?; } c => parts.push(c) }
    }
    Some(parts.join("/"))
}
#[derive(Clone, Copy)]
enum Style { Exact, Swap, Bare, Dir }
fn target(tree: &BTreeSet<String>, from_dir: &str, spec: &str) -> Option<(String, Style)> {
    let p = normal(from_dir, spec)?;
    if tree.contains(&p) { return Some((p, Style::Exact)); }
    for (js, ts) in [("js", "ts"), ("jsx", "tsx"), ("mjs", "mts"), ("cjs", "cts")] {
        if let Some(stem) = p.strip_suffix(&format!(".{js}")) && tree.contains(&format!("{stem}.{ts}")) { return Some((format!("{stem}.{ts}"), Style::Swap)); }
    }
    if let Some(f) = EXTENSIONS.iter().map(|e| format!("{p}.{e}")).find(|f| tree.contains(f)) { return Some((f, Style::Bare)); }
    EXTENSIONS.iter().map(|e| format!("{p}/index.{e}")).find(|f| tree.contains(f)).map(|f| (f, Style::Dir))
}
fn relative(from_dir: &str, to: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (from_dir.split('/').filter(|s| !s.is_empty()).collect(), to.split('/').filter(|s| !s.is_empty()).collect());
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let rest = b[common..].join("/");
    if a.len() == common { format!("./{rest}") } else { format!("{}{rest}", "../".repeat(a.len() - common)) }
}
fn bare(f: &str) -> String {
    let (dir, name) = f.rsplit_once('/').unwrap_or(("", f));
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => if dir.is_empty() { stem.to_string() } else { format!("{dir}/{stem}") },
        _ => f.to_string(),
    }
}
fn spec_for(dir: &str, file: &str, style: Style) -> String {
    match style {
        Style::Exact => relative(dir, file),
        Style::Swap => {
            let r = relative(dir, file);
            [(".ts", ".js"), (".tsx", ".jsx"), (".mts", ".mjs"), (".cts", ".cjs")].iter().find_map(|(t, j)| r.strip_suffix(t).map(|s| format!("{s}{j}"))).unwrap_or(r)
        }
        Style::Bare => bare(&relative(dir, file)),
        Style::Dir => match file.rsplit_once('/') { Some((d, n)) if n.starts_with("index.") => relative(dir, d), _ => bare(&relative(dir, file)) },
    }
}
fn retarget(text: &str, from: &str, to: &str, tree: &BTreeSet<String>, moves: &std::collections::BTreeMap<String, String>) -> (String, BTreeSet<String>) {
    let (old_dir, new_dir) = (parent(from), parent(to));
    let (mut out, mut at, mut hits) = (String::with_capacity(text.len()), 0usize, BTreeSet::new());
    for (start, end, value) in proof::literals(from, text) {
        if start < at || text.get(start..end) != Some(value.as_str()) || !(value.starts_with("./") || value.starts_with("../")) || value.contains(char::is_whitespace) { continue; }
        let Some((file, style)) = target(tree, old_dir, &value) else { continue };
        let moved = moves.get(&file);
        if moved.is_some() { hits.insert(file.clone()); }
        if moved.is_none() && old_dir == new_dir { continue; }
        let spec = spec_for(new_dir, moved.unwrap_or(&file), style);
        if spec == value { continue; }
        out.push_str(&text[at..start]);
        out.push_str(&spec);
        at = end;
    }
    out.push_str(&text[at..]);
    (out, hits)
}
struct Copy { name: String, new_text: String, old_text: String, snap: Option<String>, hits: BTreeSet<String>, size: usize }
fn put(k: &mut Kept, f: &str, c: Copy) {
    if let Some(s) = &c.snap {
        k.files.insert(snapshot_of(&c.name), s.clone());
        k.old_files.insert(snapshot_of(&c.name), s.clone());
    }
    k.files.insert(c.name.clone(), c.new_text);
    k.old_files.insert(c.name.clone(), c.old_text);
    k.run.insert(c.name.clone());
    k.back.insert(c.name, f.to_string());
}
pub fn unloaded_edits(changes: &[(String, &'static str)], old: &Report, new: &Report, dir: &str) -> Vec<(String, String)> {
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let loaded = |r: &Report, f: &str| r.tests.keys().any(|t| file_of(t) == f);
    changes.iter().filter(|(p, op)| *op == "modify" && p.starts_with(&prefix) && is_test_path(p) && crate::capability::is_code(p) && !loaded(old, p) && !loaded(new, p))
        .filter_map(|(p, _)| {
            let why = new.why.get(p).or_else(|| old.why.get(p))?;
            Some((p.clone(), format!("it did not load on the old code or the new code: {}", crate::util::plain(why.lines().next().unwrap_or(""), 300))))
        }).collect()
}
pub fn kept_plan(repo: &Path, (base, head): (&str, &str), changes: &[(String, &'static str)], old: &Report, dir: &str) -> Kept {
    let mut k = Kept::default();
    let code = crate::capability::is_code;
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    if old.tests.values().all(|c| c.passed == 0) || changes.iter().all(|(p, _)| crate::check::documentation(p) || infra(p, dir)) { return k; }
    let known = old.tests.keys().any(|t| code(file_of(t)));
    let ran: BTreeSet<&str> = old.tests.iter().filter(|(_, c)| known && c.passed > 0).map(|(t, _)| file_of(t)).collect();
    let renames: Vec<(String, String, bool)> = crate::rules::renames(repo, base, head).unwrap_or_default().into_iter().filter(|(f, t, _)| code(f) == code(t)).collect();
    let moved: std::collections::BTreeMap<String, String> = renames.iter().map(|(f, t, _)| (f.clone(), t.clone())).collect();
    let same: BTreeSet<&str> = renames.iter().filter(|r| r.2).map(|r| r.0.as_str()).collect();
    let gone: BTreeSet<&str> = changes.iter().filter(|(p, op)| *op == "delete" && !moved.contains_key(p)).map(|(p, _)| p.as_str()).collect();
    let named = |p: &str| { let n = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase(); NAMES.iter().any(|m| n.contains(m)) };
    let (mut owners, mut data) = (BTreeSet::new(), BTreeSet::new());
    for (p, _) in changes.iter().filter(|(p, op)| *op != "add" && p.starts_with(&prefix) && !same.contains(p.as_str())) {
        if !known && code(p) && is_test_path(p) { if !gone.contains(p.as_str()) { k.skipped.push((p.clone(), NODE_FILES.into())); } }
        else if ran.contains(p.as_str()) { if !gone.contains(p.as_str()) { owners.insert(p.clone()); } }
        else if let Some(o) = snapshot_owner(p) { if ran.contains(o.as_str()) && !gone.contains(o.as_str()) { owners.insert(o); } }
        else if infra(p, dir) && !(code(p) && named(p)) && test_grant(p).is_none_or(|g| !g.ends_with('/') || ran.iter().any(|r| r.starts_with(g.as_str()))) { data.insert(p.clone()); }
    }
    if owners.is_empty() && data.is_empty() { return k; }
    let base_items = crate::drive::listing(repo, base).unwrap_or_default();
    let head_items = crate::drive::listing(repo, head).unwrap_or_default();
    let tree: BTreeSet<String> = base_items.iter().map(|i| i.path.clone()).collect();
    let mut present = tree.clone();
    present.extend(head_items.iter().map(|i| i.path.clone()));
    let mut free = |p: &str| -> Option<String> {
        let n = (0..10).filter_map(|n| kept_name(p, n)).find(|n| !present.contains(n) && !present.contains(&snapshot_of(n)))?;
        present.insert(n.clone());
        Some(n)
    };
    let data_kept: std::collections::BTreeMap<String, String> = data.iter().filter_map(|d| Some((d.clone(), free(d)?))).collect();
    let needles: BTreeSet<String> = data_kept.keys().flat_map(|d| {
        let name = d.rsplit('/').next().unwrap_or(d);
        let stem = name.split_once('.').map(|(s, _)| s).filter(|s| code(d) && s.len() > 2);
        [Some(name.to_string()), stem.map(str::to_string)].into_iter().flatten()
    }).collect();
    let mut mentioned: BTreeSet<String> = if needles.is_empty() { BTreeSet::new() } else {
        crate::git::mentions(repo, base, &needles.iter().map(String::as_str).collect::<Vec<_>>()).unwrap_or_default().into_iter()
            .filter(|f| ran.contains(f.as_str()) && !gone.contains(f.as_str()) && !owners.contains(f)).collect()
    };
    if mentioned.len() > 500 { mentioned.clear(); }
    let wanted: BTreeSet<String> = owners.iter().chain(&mentioned).flat_map(|f| [f.clone(), snapshot_of(f)]).chain(data_kept.keys().cloned()).collect();
    let mut texts = std::collections::BTreeMap::new();
    let items: Vec<&crate::drive::Item> = base_items.iter().filter(|i| wanted.contains(&i.path) && i.size <= KEPT_BYTES as u64).collect();
    let _ = crate::drive::blobs(repo, &items, |item, bytes| { if let Ok(t) = std::str::from_utf8(bytes) { texts.insert(item.path.clone(), t.to_string()); } Ok(()) });
    let snaps: BTreeSet<&str> = changes.iter().map(|(p, _)| p.as_str()).filter(|p| p.ends_with(".snap")).collect();
    let edited: Vec<&crate::drive::Item> = head_items.iter().filter(|i| owners.contains(&i.path) && i.size <= KEPT_BYTES as u64 && !snaps.contains(snapshot_of(&i.path).as_str())).collect();
    let mut grown = BTreeSet::new();
    let _ = crate::drive::blobs(repo, &edited, |item, bytes| {
        if let (Ok(now), Some(was)) = (std::str::from_utf8(bytes), texts.get(&item.path)) && crate::weaken::only_adds_tests(&item.path, was, now) { grown.insert(item.path.clone()); }
        Ok(())
    });
    owners.retain(|f| !grown.contains(f));
    let mut moves = moved.clone();
    moves.extend(data_kept.clone());
    let escaped = |s: &str| serde_json::to_string(s).map_or(KEPT_BYTES + 1, |j| j.len());
    let mut make = |f: &str| -> Result<Copy, &'static str> {
        let name = free(moved.get(f).map_or(f, String::as_str)).ok_or("Kern found no free name to put its old version back under")?;
        let text = texts.get(f).ok_or("Kern could not read its old version as text")?;
        let (new_text, hits) = retarget(text, f, &name, &tree, &moves);
        let (old_text, _) = retarget(text, f, &name, &tree, &data_kept);
        let snap = texts.get(&snapshot_of(f)).cloned();
        let size = escaped(&new_text).max(escaped(&old_text)) + snap.as_deref().map_or(0, escaped);
        Ok(Copy { name, new_text, old_text, snap, hits, size })
    };
    let (mut bytes, mut used) = (0usize, BTreeSet::new());
    for (i, f) in owners.iter().enumerate() {
        if i >= FOCUS { k.skipped.push((f.clone(), format!("Kern puts back at most {FOCUS} edited test files per check"))); continue; }
        match make(f) {
            Err(why) => k.skipped.push((f.clone(), why.into())),
            Ok(c) if bytes + c.size > KEPT_BYTES => k.skipped.push((f.clone(), "its old version is too large for Kern to put back (3 MiB in all)".into())),
            Ok(c) => { bytes += c.size; used.extend(c.hits.iter().cloned()); put(&mut k, f, c); }
        }
    }
    let mut extra: std::collections::BTreeMap<String, Copy> = mentioned.iter().filter_map(|f| make(f).ok().map(|c| (f.clone(), c))).filter(|(_, c)| c.hits.iter().any(|h| data_kept.contains_key(h))).collect();
    let mut covered = BTreeSet::new();
    for (d, kept) in &data_kept {
        let Some(text) = texts.get(d) else { continue };
        let refs: Vec<String> = extra.iter().filter(|(_, c)| c.hits.contains(d)).map(|(f, _)| f.clone()).collect();
        if refs.is_empty() && !used.contains(d) { continue; }
        let need = refs.iter().map(|f| extra[f].size).sum::<usize>() + 2 * escaped(text);
        if k.run.len() + refs.len() > FOCUS || bytes + need > KEPT_BYTES {
            if used.contains(d) && bytes + 2 * escaped(text) <= KEPT_BYTES {
                bytes += 2 * escaped(text);
                k.files.insert(kept.clone(), text.clone());
                k.old_files.insert(kept.clone(), text.clone());
            }
            continue;
        }
        bytes += need;
        for f in refs { if let Some(c) = extra.remove(&f) { used.extend(c.hits.iter().cloned()); put(&mut k, &f, c); } }
        k.files.insert(kept.clone(), text.clone());
        k.old_files.insert(kept.clone(), text.clone());
        covered.insert(d.clone());
    }
    k.data = data.into_iter().filter(|d| !covered.contains(d)).collect();
    k
}
pub fn unkept(r: &Report, back: &std::collections::BTreeMap<String, String>) -> Report {
    let swap = |k: &str| -> Option<String> { let f = file_of(k); back.get(f).map(|o| format!("{o}{}", &k[f.len()..])) };
    Report { tests: r.tests.iter().filter_map(|(k, c)| Some((swap(k)?, *c))).collect(), problems: r.problems.iter().filter_map(|p| swap(p)).collect(),
        why: r.why.iter().filter_map(|(k, w)| Some((swap(k)?, w.clone()))).collect() }
}
pub struct Outcome { pub rewritten: Vec<String>, pub checked: usize, pub unchecked: Vec<(String, String)>, pub held: Vec<String> }
pub fn kept_outcome(old: &Report, keep: &Kept, first: Result<Report, String>, second: Option<&Report>) -> Outcome {
    let mut unchecked = keep.skipped.clone();
    let first = match first {
        Ok(r) => r,
        Err(why) => {
            unchecked.extend(keep.back.values().map(|o| (o.clone(), why.clone())));
            return Outcome { rewritten: Vec::new(), checked: 0, unchecked, held: Vec::new() };
        }
    };
    let loaded: BTreeSet<&str> = first.tests.keys().map(|k| file_of(k)).collect();
    for o in keep.back.values().filter(|o| !loaded.contains(o.as_str())) {
        unchecked.push((o.clone(), first.why.get(o).cloned().unwrap_or_else(|| "its old version gave no test results on the new code".into())));
    }
    Outcome { rewritten: rewritten(old, &first, second), checked: loaded.len(), unchecked, held: held(old, &first) }
}
pub fn held(old: &Report, kept_new: &Report) -> Vec<String> {
    let loaded: BTreeSet<&str> = kept_new.tests.keys().map(|k| file_of(k)).collect();
    let base = Report { tests: old.tests.iter().filter(|(k, _)| loaded.contains(file_of(k))).map(|(k, c)| (k.clone(), *c)).collect(), ..Report::default() };
    let now = settled(&base, kept_new);
    base.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&now, k)) == Seen::Same).map(|(k, _)| k.clone()).collect()
}
pub fn tests_only<'a>(changes: impl IntoIterator<Item = (&'a str, &'a str)>, old: &Report) -> Vec<String> {
    let mut edited = BTreeSet::new();
    for (p, op) in changes {
        if crate::check::documentation(p) { continue; }
        let name = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase();
        if !crate::capability::is_code(p) || !NAMES.iter().any(|m| name.contains(m)) { return Vec::new(); }
        if op == "modify" { edited.insert(p); }
    }
    old.tests.iter().filter(|(k, c)| c.passed > 0 && c.failed == 0 && edited.contains(file_of(k))).map(|(k, _)| k.clone()).collect()
}
pub fn own_edits(c: &mut Value, held: &[String]) {
    if c["status"] != "RAN" || held.is_empty() { return; }
    let tests: BTreeSet<&str> = held.iter().map(String::as_str).collect();
    let files: BTreeSet<&str> = held.iter().map(|k| file_of(k)).collect();
    let (failing, broke): (Vec<String>, Vec<String>) = listed(c, "broke").into_iter().partition(|k| tests.contains(k.as_str()));
    let (unloaded, problems): (Vec<String>, Vec<String>) = listed(c, "new_problems").into_iter().partition(|p| files.contains(file_of(p)));
    c["edit_fails"] = json!(failing.into_iter().chain(unloaded).collect::<Vec<_>>());
    c["broke"] = json!(broke);
    c["new_problems"] = json!(problems);
}
pub fn rewritten(old: &Report, kept_new: &Report, kept_old: Option<&Report>) -> Vec<String> {
    let loaded: BTreeSet<&str> = kept_new.tests.keys().map(|k| file_of(k)).collect();
    let base = Report { tests: old.tests.iter().filter(|(k, _)| loaded.contains(file_of(k))).map(|(k, c)| (k.clone(), *c)).collect(), ..Report::default() };
    let now = settled(&base, kept_new);
    let then = kept_old.map(|o| settled(&base, o));
    base.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&now, k)) == Seen::Failed && then.as_ref().is_none_or(|t| seen(**b, count(t, k)) == Seen::Same))
        .map(|(k, _)| k.clone()).collect()
}
pub fn kept_focus(tests: &Tests, files: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    match tests.runner {
        Runner::Jest => focused(tests, files, dir),
        Runner::Vitest => {
            let mut t = tests.clone();
            let (mut args, mut value) = (Vec::new(), false);
            for a in t.command.get(2..).unwrap_or_default() {
                if a.starts_with('-') { value = !a.contains('='); args.push(a.clone()); continue; }
                if value { args.push(a.clone()); }
                value = false;
            }
            t.command.truncate(2);
            t.command.extend(args);
            focused(&t, files, dir)
        }
        Runner::Mocha => {
            if files.is_empty() || files.len() > FOCUS { return None; }
            let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
            let mut t = tests.clone();
            for f in files { t.command.push(f.strip_prefix(&prefix)?.to_string()); }
            Some(t)
        }
        Runner::Node => node_files(tests, files, dir),
    }
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
    let files_named = before.tests.keys().any(|k| crate::capability::is_code(file_of(k)));
    let passed_before = |p: &str| !files_named || before.tests.iter().chain(alone_old.into_iter().flat_map(|o| o.tests.iter()))
        .any(|(k, c)| c.passed > 0 && masked(file_of(k)) == masked(file_of(p)));
    let (mut new_problems, mut unsteady, mut environmental, mut unproven) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, p) in checkable.iter().enumerate() {
        if excused.as_ref().is_some_and(|e| e[i]) { environmental.push(p.clone()); }
        else if repeated.as_ref().is_some_and(|r| !r[i]) { unsteady.push(p.clone()); }
        else if !passed_before(p) { unproven.push(p.clone()); }
        else { new_problems.push(p.clone()); }
    }
    let reported: BTreeSet<String> = new_problems.iter().map(|p| file_of(p).to_string()).collect();
    let shaky: BTreeSet<String> = unsteady.iter().chain(&environmental).map(|p| file_of(p).to_string()).collect();
    let keys: BTreeSet<&String> = before.tests.keys().chain(after.tests.keys()).collect();
    let before_broken: BTreeSet<&str> = before.problems.iter().map(|p| file_of(p)).collect();
    let (mut kept, mut broke, mut flaky, mut gone, mut newly_skipped, mut added, mut already_failing) = (0u64, Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut new_failing, mut same_failure, mut unrunnable) = (Vec::new(), Vec::new(), Vec::new());
    let signature = |w: &str| -> String { masked(&w.replace("/workspace/", "")).chars().take(160).collect() };
    let old_errors: BTreeSet<String> = before.tests.iter().filter(|(_, c)| c.failed > 0).map(|(k, _)| k).chain(&before.problems).filter_map(|k| before.why.get(k)).map(|w| signature(w)).collect();
    let known_error = |k: &str| raw_after.why.get(k).is_some_and(|w| old_errors.contains(&signature(w)));
    let (named_before, named_after, named_alone_old, named_again) = (names_by_file(before), names_by_file(after), alone_old.map(names_by_file), again.map(names_by_file));
    let by_chance: BTreeSet<&str> = named_before.keys().copied().filter(|f| [(&named_before, named_alone_old.as_ref()), (&named_after, named_again.as_ref())].into_iter()
        .any(|(a, b)| b.and_then(|b| b.get(f)).is_some_and(|n| a.get(f) != Some(n)))).collect();
    let lacking = |k: &str| raw_after.why.get(k).is_some_and(|w| sandbox_error(w));
    for k in keys {
        let (b, a) = (count(before, k), count(after, k));
        if b.passed == 0 && b.failed == 0 {
            if b.total() == 0 && before_broken.contains(file_of(k)) { already_failing.push(k.clone()); continue; }
            if b.total() == 0 || a.ran() > 0 { added.push(k.clone()); }
            if a.failed > 0 { if known_error(k) { same_failure.push(k.clone()); } else if lacking(k) { unrunnable.push(k.clone()); } else { new_failing.push(k.clone()); } }
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
        if verdict == Seen::Gone && by_chance.contains(file_of(k)) { flaky.push(k.clone()); continue; }
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
    for p in fresh { if known_error(&p) { same_failure.push(p); } else if lacking(&p) { unrunnable.push(p); } else { new_failing.push(p); } }
    let (old_sandbox, failing_before): (Vec<String>, Vec<String>) = before.problems.iter().cloned().partition(|p| before.why.get(p).is_some_and(|w| sandbox_error(w)));
    already_failing.extend(failing_before);
    if total(before) == 0 && total(raw_after) == 0 && before.problems.is_empty() && raw_after.problems.is_empty() {
        return json!({ "status": "NOT_RUN", "why": FOUND_NONE });
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
    let problem_why: serde_json::Map<String, Value> = new_problems.iter().chain(&unproven).filter_map(|p| again.unwrap_or(after).why.get(p).or_else(|| after.why.get(p)).map(|w| (p.clone(), json!(w)))).collect();
    json!({ "status": "RAN", "before": sum(before), "after": sum(raw_after), "kept": kept, "broke": broke, "flaky": flaky, "gone": gone,
        "newly_skipped": newly_skipped, "added": added, "new_failing": new_failing, "already_failing": already_failing, "new_problems": new_problems, "problem_why": problem_why,
        "environmental": environmental, "same_failure": same_failure, "unproven": unproven, "unrunnable": unrunnable, "old_sandbox": old_sandbox })
}

fn lasting(s: u64) -> String { if s < 120 { format!("{s} seconds") } else { format!("{} minutes", s / 60) } }
pub fn stopped_why(ending: &str, limit: u64) -> Option<String> {
    let words: Vec<u64> = ending.split(' ').skip(1).filter_map(|w| w.parse().ok()).collect();
    match (ending.split(' ').next(), words.as_slice()) {
        (Some("projected"), [projected, after]) => Some(format!("the tests were on course to take about {} (the limit is {} minutes), so Kern stopped the run after {}",
            lasting(*projected), limit / 60, lasting(*after))),
        (Some("stalled"), [after]) => Some(format!("no test file finished in {} (the limit is {} minutes), so Kern stopped the run", lasting(*after), limit / 60)),
        _ => None,
    }
}
pub fn stopped(ending: &str, limit: u64, early_secs: u64) -> Option<Side> {
    stopped_why(ending, limit).map(|why| if early_secs < test_seconds() { Side::Late } else { Side::Slow(why) })
}
pub fn whole(why: &str) -> String {
    why.replacen("the tests were", "the whole suite was", 1).replacen("the tests did", "the whole suite did", 1).replacen("no test file finished", "no test file of the whole suite finished", 1)
}
pub fn related_none(place: &str, whole_why: &str, why: &str) -> String {
    let whole_why = whole_why.split(" (the limit is").next().unwrap_or(whole_why);
    format!("the tests{place} on the old code gave no results: {whole_why}, and the tests related to the change gave none either: {why}")
}
pub fn ran_out(place: &str, old_secs: u64, cut_secs: u64) -> String {
    format!("the tests{place} finished on the old code in {}, but on the new code they had not finished after {}, when Kern's time for this check ran out", lasting(old_secs), lasting(cut_secs))
}
pub fn follows_script(package: &Value) -> bool { matches!(crate::script::found(package), crate::script::Found::Run(_)) }
pub fn guessed(package: &Value) -> Option<String> {
    let script = package["scripts"]["test"].as_str()?;
    (crate::script::found(package) == crate::script::Found::Nothing).then(|| script.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect())
}
pub fn failed_exit(ending: &[u8]) -> bool { String::from_utf8_lossy(ending).strip_prefix("exit ").is_some_and(|code| code.trim() != "0") }
pub fn stopped_early(ending: &[u8], errors: &[u8], output: &[u8]) -> bool {
    let (errors, output) = (String::from_utf8_lossy(errors), String::from_utf8_lossy(output));
    let empty = [&errors, &output].iter().any(|t| t.contains("No tests found") || t.contains("No test files found"));
    let _ = ending;
    !empty && crate::sandbox::first_error(&errors).is_some()
}
fn install_record<'v>(setup: &'v Value, side: &str, dir: &str) -> Option<&'v Value> {
    let applies = |d: &str| d == "." || d == dir || dir.starts_with(&format!("{d}/"));
    setup[side]["installs"].as_array()?.iter().filter(|i| i["dir"].as_str().is_some_and(applies)).max_by_key(|i| i["dir"].as_str().map_or(0, |d| if d == "." { 0 } else { d.len() }))
}
fn setup_record<'v>(setup: &'v Value, side: &str, dir: &str) -> Option<&'v Value> {
    install_record(setup, side, dir).and_then(|r| if r["result"] == crate::sandbox::WITH_WORKSPACE { install_record(setup, side, ".") } else { Some(r) })
}
fn unlocked(setup: &Value, dir: &str) -> Option<String> {
    ["old", "new"].iter().find_map(|side| {
        let result = setup_record(setup, side, dir)?["result"].as_str()?;
        result.contains("from package.json").then(|| format!("the {side} code's dependencies were {}, so each side resolved its own versions", result.chars().take(120).collect::<String>()))
    })
}
pub fn setup_differs(setup: &Value, dir: &str) -> Option<String> {
    let (old, new) = (setup_record(setup, "old", dir)?, setup_record(setup, "new", dir)?);
    let said = |r: &Value| r["result"].as_str().unwrap_or("").chars().take(120).collect::<String>();
    if old["result"] != new["result"] { return Some(format!("the old code's dependencies were {} and the new code's were {}", said(old), said(new))); }
    let parts = |r: &Value| -> BTreeSet<String> { r["parts"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    if parts(old) != parts(new) { return Some("Kern fetched a different set of Linux parts for the dependencies on each side".to_string()); }
    let (o, n) = (&setup["old"], &setup["new"]);
    if o["scripts"] != n["scripts"] { return Some("Kern ran the packages' install scripts on one side only, because the other ran out of memory and was installed again without them".to_string()); }
    let fell_back = |s: &Value| s["node"]["why"].is_string();
    if (fell_back(o) || fell_back(n)) && o["node"]["major"] != n["node"]["major"] {
        return Some(format!("Kern could not get the Node version one side asks for, so the old code ran on Node {} and the new code on Node {}", o["node"]["major"], n["node"]["major"]));
    }
    if o["node"]["major"] == n["node"]["major"] && o["image"].is_string() && n["image"].is_string() && o["image"] != n["image"] && o["browser"] == n["browser"] {
        return Some("the two sides ran different builds of the same Node image".to_string());
    }
    if o["browser"]["error"].is_string() != n["browser"]["error"].is_string() { return Some("Kern could set up the browser the tests need on one side only".to_string()); }
    unlocked(setup, dir)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step { pub name: String, pub exit: Option<i64>, pub errors: String }
pub fn steps_run(output: &[u8]) -> R<Vec<Step>> {
    let v: Value = serde_json::from_slice(output).map_err(|_| crate::util::Denied("the build steps' results could not be read".into()))?;
    let list = v.as_array().ok_or_else(|| crate::util::Denied("the build steps' results could not be read".into()))?;
    list.iter().map(|r| {
        let name = r["name"].as_str().filter(|n| !n.is_empty()).ok_or_else(|| crate::util::Denied("a build step's result names no step".into()))?;
        crate::ensure!(r["finished"] == true, "{name} did not finish in the time Kern gave it");
        crate::ensure!(r.get("exit").is_some_and(|e| e.is_null() || e.is_i64()), "a build step's result has no exit status");
        Ok(Step { name: name.to_string(), exit: r["exit"].as_i64(), errors: r["errors"].as_str().unwrap_or("").to_string() })
    }).collect()
}
pub fn uneven_steps(old: &[Step], new: &[Step]) -> Option<String> {
    let names: BTreeSet<&str> = old.iter().chain(new).map(|s| s.name.as_str()).collect();
    let (mut unrecorded, mut differ) = (Vec::new(), Vec::new());
    for name in names {
        match (old.iter().find(|s| s.name == name), new.iter().find(|s| s.name == name)) {
            (Some(a), Some(b)) if (a.exit == Some(0)) != (b.exit == Some(0)) => differ.push(name),
            (Some(_), Some(_)) => {}
            _ => unrecorded.push(name),
        }
    }
    let mut said = Vec::new();
    if !differ.is_empty() { said.push(format!("{} worked on one side only", differ.join(", "))); }
    if !unrecorded.is_empty() { said.push(format!("{} has no recorded result on one side", unrecorded.join(", "))); }
    (!said.is_empty()).then(|| said.join("; "))
}
pub fn install_failure(setup: &Value, side: &str, dir: &str) -> Option<String> {
    let record = install_record(setup, side, dir)?;
    let result = record["result"].as_str()?;
    if result.starts_with("installed") { return None; }
    let with = record["manager"].as_str().map(|m| format!(" with {m}")).unwrap_or_default();
    let detail = result.strip_prefix("failed:").map(str::trim).unwrap_or("");
    let line = crate::sandbox::first_error(detail).or_else(|| detail.lines().map(str::trim).rfind(|l| !l.is_empty())).unwrap_or("").chars().take(240).collect::<String>();
    Some(if result == "out of time" { format!("installing its dependencies{with} ran out of time") }
        else if line.is_empty() { format!("installing its dependencies{with} failed") }
        else { format!("installing its dependencies{with} failed: {line}") })
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
    let last: Vec<&str> = source.lines().map(str::trim).filter(|l| !l.is_empty() && !l.contains('\u{23af}')).collect();
    let frame = |l: &str| l.starts_with("at ") || l.starts_with('\u{276f}');
    let from = last.iter().rposition(|l| !frame(l)).unwrap_or(0).saturating_sub(1);
    let tail = last[from..].iter().take(3).copied().collect::<Vec<_>>().join(" / ");
    let first = last.iter().find(|l| !frame(l) && sandbox_error(l))
        .or_else(|| last.iter().find(|l| { let x = l.to_ascii_lowercase(); !frame(l) && (x.contains("error") || x.contains("cannot find") || x.contains("failed to")) }))
        .map(|l| l.chars().take(240).collect::<String>()).filter(|f| !tail.contains(f.as_str()));
    let mut s = format!("{why} ({})", String::from_utf8_lossy(ending).trim());
    if let Some(f) = first { s += &format!("; its first error: {f}"); }
    if !tail.is_empty() { s += &format!("; the last lines it printed: {tail}"); }
    if s.chars().count() > 600 { s = format!("{}…", s.chars().take(599).collect::<String>()); }
    s
}

#[derive(Debug)]
pub enum Side { Ran(Report, Vec<String>), Nothing(String), Slow(String), Cut(String), Late }
impl Side {
    fn ending(&self) -> &str { match self { Side::Nothing(w) | Side::Slow(w) | Side::Cut(w) => w, Side::Ran(..) => "it gave results", Side::Late => LATE } }
}
pub fn without_results(why: &str, ending: &[u8], errors: &[u8], output: &[u8]) -> Side {
    let said = no_results(why, ending, errors, output);
    let kerns = |b: &[u8]| { let text = String::from_utf8_lossy(b); sandbox_error(&text) || crate::sandbox::kern_failed(&text) };
    if kerns(errors) || kerns(output) { Side::Cut(said) } else { Side::Nothing(said) }
}
const RUNNER_MISSING: &str = "The test runner is not installed in the project";
pub fn unreadable(refusal: Option<String>, error: String) -> Side {
    match refusal {
        Some(r) if r.contains(RUNNER_MISSING) => Side::Nothing(format!("Kern could not run the tests in the sandbox: {r}")),
        Some(r) => Side::Cut(format!("Kern's own sandbox entry stopped before it could read the tests' results: {r}")),
        None => Side::Cut(error),
    }
}
pub fn runner_declared(manifest: Option<&Value>, runner: Runner) -> bool {
    runner == Runner::Node || manifest.is_some_and(|m| ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"].iter().any(|s| m[*s][runner.name()].is_string()))
}
pub fn kerns_install(side: Side, declared: bool) -> Side {
    match side {
        Side::Nothing(why) if declared && why.contains(RUNNER_MISSING) => Side::Cut(format!("{why}; the project declares the runner, so Kern's install, not the change, left it out")),
        side => side,
    }
}
pub fn capped(ending: &str) -> Option<String> { ending.strip_prefix("cut: ").map(str::to_string) }
pub fn broke_run(first: &Side, again: &Side, old_again: Option<&Side>, uneven: bool) -> bool {
    matches!((first, again), (Side::Nothing(_), Side::Nothing(_))) && !uneven && matches!(old_again, Some(Side::Ran(r, _)) if r.sum().passed > 0)
}
fn no_results_twice(first: &Side, again: &Side) -> bool { matches!((first, again), (Side::Nothing(_), Side::Nothing(_))) }
fn unconfirmed_why(first: &Side, again: &Side, old_again: Option<&Result<Side, String>>, uneven: Option<&str>) -> String {
    match (uneven, old_again) {
        (Some(u), _) => format!("the two sides were not set up the same way: {u}"),
        (None, Some(Ok(o))) => format!("the old code's tests gave no results when run again either ({}), so the cause is in Kern's environment, not the change", o.ending()),
        (None, Some(Err(e))) => format!("running the old code's tests again to confirm failed: {e}"),
        (None, None) if no_results_twice(first, again) => "Kern could not run the old code's tests again to confirm".to_string(),
        (None, None) => format!("a run hit one of Kern's own limits, so it proves nothing about the change ({})", first.ending()),
    }
}

fn run_for(out: &Path, (volume, limit): (&str, u64), side: (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    run_with(out, (volume, &std::collections::BTreeMap::new(), limit), side, dir, label, until)
}
pub fn test_limits(seconds: u64) -> Limits {
    Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: seconds + 120, memory_mb: test_memory_mb(std::fs::read_to_string("/proc/meminfo").ok().as_deref()), pids: 4096 }
}
pub fn in_test_box(out: &Path, volume: &str, name: &str, config: (&str, &Value), (entry, root, seconds): (&str, bool, u64)) -> R<crate::sandbox::Execution> {
    let control = out.join(name);
    if control.exists() { std::fs::remove_dir_all(&control)?; }
    crate::sandbox::control_dir(&control, Some(config))?;
    let input = out.join("tests-input");
    std::fs::create_dir_all(&input)?;
    let image = crate::drive::side_image(out, volume)?;
    let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &image, snapshot: &input, control: &control, writable: &[], limits: &test_limits(seconds),
        entry: vec![entry.into()], network: "none", volumes: vec![], root, home_mb: 0, scratch: false, workspace: Some((volume, false)) });
    let _ = std::fs::remove_dir_all(&control);
    run
}
fn run_with(out: &Path, (volume, files, limit): (&str, &std::collections::BTreeMap<String, String>, u64), (tests, env): (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    let seconds = within(until, limit);
    if seconds < RUN_SECS_MIN { return Ok(Side::Late); }
    let (cut, early) = (seconds < limit, early_secs(limit, seconds));
    let config = json!({ "version": 1, "command": test_command(tests), "timeout_seconds": seconds, "cwd": dir, "env": sized_env(tests.runner, env), "files": files, "early_secs": early, "faults": [], "serve": {}, "steps": [] });
    let run = match in_test_box(out, volume, &format!("tests-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds)) {
        Ok(r) => r,
        Err(e) if e.0.contains("timed out") => return Ok(if cut { Side::Late } else { Side::Slow(format!("the run was stopped: {}", e.0)) }),
        Err(e) if e.0.contains("exceeded limits") => return Ok(Side::Cut(format!("the run was stopped: {}", e.0))),
        Err(e) => return Err(e),
    };
    let parts = match crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) {
        Ok(p) => p,
        Err(e) => return Ok(unreadable(crate::sandbox::refusal(&run.errors), e.0)),
    };
    std::fs::write(out.join(format!("tests-{label}.out")), &parts["output"])?;
    std::fs::write(out.join(format!("tests-{label}.err")), &parts["errors"])?;
    if !parts["values"].is_empty() { std::fs::write(out.join(format!("tests-{label}.values")), &parts["values"])?; }
    if let Some(why) = capped(&String::from_utf8_lossy(&parts["ending"])) { return Ok(Side::Cut(why)); }
    if parts["ending"] == b"timeout" { return Ok(if cut { Side::Late } else { Side::Slow(format!("the tests did not finish in {} minutes", limit / 60)) }); }
    if let Some(side) = stopped(&String::from_utf8_lossy(&parts["ending"]), limit, early) { return Ok(side); }
    if parts["ending"] == b"exit -1" { return Ok(Side::Cut("the test runner was stopped by a signal, so Kern does not trust its report".into())); }
    let created: Vec<String> = String::from_utf8_lossy(&parts["created"]).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    Ok(match proof::read_report(tests.runner, &parts["output"]) {
        Ok(r) if r.tests.is_empty() && r.problems.is_empty() && (failed_exit(&parts["ending"]) || stopped_early(&parts["ending"], &parts["errors"], &parts["output"])) =>
            without_results("the runner stopped before running any test", &parts["ending"], &parts["errors"], &parts["output"]),
        Ok(r) => Side::Ran(r, created),
        Err(why) => without_results(&why, &parts["ending"], &parts["errors"], &parts["output"]),
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

pub struct Suite { pub dir: String, pub old: Option<(Tests, Env)>, pub new: (Tests, Env), pub uneven: Option<String> }
const SUITES_COUNT_MAX: usize = 16;
fn side_json((tests, env): &(Tests, Env)) -> Value { json!({ "runner": tests.runner.name(), "command": tests.command, "env": env }) }
fn side_of(v: &Value) -> Option<(Tests, Env)> {
    let command = v["command"].as_array()?.iter().map(|a| a.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
    let env = v["env"].as_object()?.iter().map(|(k, x)| Some((k.clone(), x.as_str()?.to_string()))).collect::<Option<Env>>()?;
    Some((Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::parse(v["runner"].as_str()?)?, command, edits: Edits::Checked }, env))
}
impl Suite {
    pub fn json(&self) -> Value { json!({ "dir": self.dir, "old": self.old.as_ref().map(side_json), "new": side_json(&self.new), "uneven": self.uneven }) }
}
pub fn ran_suites(out: &Path) -> Option<Vec<Suite>> {
    let v = read_json(out.join("suites.json")).ok()?;
    v.as_array()?.iter().take(SUITES_COUNT_MAX).map(|s| Some(Suite { dir: s["dir"].as_str()?.to_string(), old: if s["old"].is_null() { None } else { Some(side_of(&s["old"])?) }, new: side_of(&s["new"])?,
        uneven: match &s["uneven"] { Value::Null => None, Value::String(u) => Some(u.clone()), _ => return None } })).collect()
}
pub fn label(dir: &str) -> String { if dir.is_empty() { String::new() } else { format!("-{}", dir.replace('/', "_")) } }

const REPAIR_ROUNDS_COUNT_MAX: usize = 3;
pub const SINGLE_FILES: &str = "Kern runs single test files only with jest, vitest and mocha";
pub fn missing_packages(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (start, end) in [("Cannot find module '", "'"), ("Cannot find package '", "'"), ("Failed to load url ", " "), ("Failed to resolve entry for package \"", "\""),
        ("Could not resolve \"", "\""), ("Failed to resolve import \"", "\"")] {
        let mut rest = text;
        while let Some(i) = rest.find(start) {
            rest = &rest[i + start.len()..];
            let spec = rest.split(end).next().unwrap_or("").trim_end_matches(['.', ',', ')']);
            let spec = spec.rsplit_once("node_modules/").map_or(spec, |(_, after)| after);
            if spec.is_empty() || spec.starts_with(['.', '/', '#']) || spec.starts_with("node:") || spec.contains(char::is_whitespace) { continue; }
            let mut parts = spec.split('/');
            let first = parts.next().unwrap_or("");
            let name = if first.starts_with('@') { format!("{first}/{}", parts.next().unwrap_or("")) } else { first.to_string() };
            if !name.ends_with('/') { out.insert(name); }
        }
    }
    out
}
pub fn missing_files(text: &str) -> BTreeSet<String> {
    let (mut out, lines) = (BTreeSet::new(), text.lines().collect::<Vec<_>>());
    for (n, line) in lines.iter().enumerate() {
        let Some(i) = line.find("Cannot find module ") else { continue };
        let rest = &line[i + "Cannot find module ".len()..];
        let Some(quote) = rest.chars().next().filter(|c| *c == '\'' || *c == '"') else { continue };
        let spec = rest[1..].split(quote).next().unwrap_or("");
        let requirer = line.split_once(" imported from ").map(|(_, f)| f.trim().to_string())
            .or_else(|| lines[n + 1..].iter().skip_while(|l| !l.trim_start().starts_with("Require stack:")).nth(1).and_then(|l| l.trim().strip_prefix("- ").map(str::to_string)));
        let file = if spec.starts_with('.') { requirer } else { Some(spec.to_string()) };
        if let Some(rel) = file.as_deref().and_then(|f| f.strip_prefix("/workspace/")).filter(|r| !r.split('/').any(|s| s == "node_modules")) { out.insert(rel.to_string()); }
    }
    out
}
pub type Workspace = std::collections::BTreeMap<String, (String, Value)>;
pub fn owners(packages: &Workspace, files: &BTreeSet<String>) -> BTreeSet<String> {
    files.iter().filter_map(|f| packages.iter().filter(|(_, (dir, _))| !dir.is_empty() && f.starts_with(&format!("{dir}/"))).max_by_key(|(_, (dir, _))| dir.len()).map(|(name, _)| name.clone())).collect()
}
pub fn workspace(repo: &Path, commit: &str) -> Workspace {
    let items = crate::drive::listing(repo, commit).unwrap_or_default();
    let manifests: Vec<&crate::drive::Item> = items.iter().filter(|i| i.path.rsplit('/').next() == Some("package.json") && !i.path.split('/').any(|c| c == "node_modules") && i.size <= 1 << 20).take(3000).collect();
    let mut out = Workspace::new();
    let _ = crate::drive::blobs(repo, &manifests, |item, bytes| {
        if let Ok(v) = serde_json::from_slice::<Value>(bytes) && let Some(name) = v["name"].as_str() {
            out.insert(name.to_string(), (item.path.trim_end_matches("package.json").trim_end_matches('/').to_string(), v));
        }
        Ok(())
    });
    out
}
pub const REPAIR_BUILDS_COUNT_MAX: usize = 20;
pub fn repair_steps(packages: &Workspace, missing: &BTreeSet<String>, pretest: Option<&str>, (dir, before): (&str, &[String])) -> Vec<(String, String)> {
    fn visit(name: &str, packages: &Workspace, seen: &mut BTreeSet<String>, order: &mut Vec<(String, String)>) {
        if !seen.insert(name.to_string()) || order.len() >= REPAIR_BUILDS_COUNT_MAX { return; }
        let Some((dir, v)) = packages.get(name) else { return };
        for section in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
            for dep in v[section].as_object().into_iter().flatten().map(|(k, _)| k) { if packages.contains_key(dep) { visit(dep, packages, seen, order); } }
        }
        if v["scripts"]["build"].is_string() && crate::util::file_path(dir).is_ok() { order.push((dir.clone(), "build".into())); }
    }
    let (mut seen, mut order) = (BTreeSet::new(), Vec::new());
    for m in missing { visit(m, packages, &mut seen, &mut order); }
    if let Some(dir) = pretest { order.push((dir.to_string(), "pretest".into())); }
    for step in before { if !order.contains(&(dir.to_string(), step.clone())) { order.push((dir.to_string(), step.clone())); } }
    order
}
pub const PREPARE_SECS_MAX: u64 = 480;
const PREPARE_SECS_MIN: u64 = 60;
pub fn repair_fits(left_secs: u64, last_old_secs: u64, build_secs: u64) -> bool { left_secs >= build_secs + 2 * last_old_secs }
pub fn failed_steps(steps: &[Step]) -> Vec<(String, String)> {
    steps.iter().filter(|s| s.exit != Some(0)).map(|s| {
        let code = s.exit.map_or("exit null".to_string(), |c| format!("exit {c}"));
        (s.name.clone(), crate::sandbox::first_error(&s.errors).map_or(code, str::to_string))
    }).collect()
}
pub fn ran_first(steps: &[String], failed: &[(String, String)]) -> String {
    if failed.is_empty() { return format!(" (even after Kern ran {} first)", steps.join(", ")); }
    let why: Vec<String> = failed.iter().map(|(step, error)| format!("{step} failed: {}", crate::util::plain(error, 300))).collect();
    format!(" (Kern ran {} first, but {})", steps.join(", "), why.join("; "))
}
fn prepare(out: &Path, volume: &str, steps: &[(String, String)], label: &str, until: u64) -> R<Vec<Step>> {
    let seconds = within(until, PREPARE_SECS_MAX);
    crate::ensure!(seconds >= PREPARE_SECS_MIN, "Kern's time ran out before it could build what the tests need");
    let listed: Vec<Value> = steps.iter().map(|(dir, script)| json!({ "dir": dir, "script": script })).collect();
    let config = json!({ "version": 1, "command": ["/usr/local/bin/node"], "timeout_seconds": seconds, "cwd": "", "env": {}, "files": {}, "early_secs": 0, "faults": [], "serve": {}, "steps": listed });
    let run = in_test_box(out, volume, &format!("prepare-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds));
    let parts = crate::sandbox::parts(&run?.stdout, &crate::sandbox::TEST_PARTS)?;
    std::fs::write(out.join(format!("prepare-{label}.json")), &parts["output"])?;
    let sealed = in_test_box(out, volume, &format!("seal-{label}"), ("drive-seal.json", &json!({ "version": 1 })), ("__drive_seal", true, seconds));
    crate::ensure!(sealed?.exit_code == 0, "Kern could not make what the build wrote read-only");
    crate::ensure!(parts["ending"] == b"steps", "the build did not finish in the {seconds} s Kern gave it ({})", String::from_utf8_lossy(&parts["ending"]));
    let ran = steps_run(&parts["output"])?;
    crate::ensure!(ran.len() == steps.len(), "the build steps' results do not cover every step");
    Ok(ran)
}
const NOT_PRODUCT: [&str; 19] = ["node_modules", ".github", "scripts", "script", "examples", "example", "docs", "doc", "demo", "demos", "benchmark", "benchmarks", "bench",
    "fixtures", "__fixtures__", "e2e", "playground", "playgrounds", ".storybook"];
pub fn product_code(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    !path.split('/').any(|s| NOT_PRODUCT.contains(&s)) && !name.contains(".config.") && !name.contains(".stories.") && !name.contains(".story.")
}
const OPERATOR_FAULTS_PER_FILE_COUNT_MAX: usize = 3;
fn spread<T: Clone>(all: Vec<T>, n: usize) -> Vec<T> {
    if all.len() <= n || n < 2 { return all.into_iter().take(n).collect(); }
    (0..n).map(|i| all[i * (all.len() - 1) / (n - 1)].clone()).collect()
}
pub fn faults(files: &[(String, String, String)], limit: usize) -> Vec<crate::proof::Mutant> {
    let mut per_file: Vec<std::collections::VecDeque<crate::proof::Mutant>> = files.iter().map(|(path, old, new)| {
        let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        let (removed, added) = crate::diff::changed_lines(&a, &b);
        let lines: BTreeSet<usize> = added.into_iter().map(|i| i + 1).collect();
        let gone: BTreeSet<String> = removed.into_iter().filter_map(|i| a.get(i)).map(|l| l.trim().to_string()).collect();
        spread(crate::proof::plant_lines(path, new, &lines, &gone), OPERATOR_FAULTS_PER_FILE_COUNT_MAX).into_iter().collect()
    }).collect();
    let mut undone: Vec<std::collections::VecDeque<crate::proof::Mutant>> = files.iter().map(|(path, old, new)| {
        let elsewhere: Vec<&str> = files.iter().filter(|f| f.0 != *path).map(|f| f.2.as_str()).collect();
        crate::proof::undos(path, old, new, &elsewhere).into_iter().collect()
    }).collect();
    let mut out: Vec<crate::proof::Mutant> = Vec::new();
    while out.len() < limit && undone.iter().any(|f| !f.is_empty()) {
        for f in undone.iter_mut() { if out.len() < limit && let Some(m) = f.pop_front() { out.push(m); } }
    }
    while out.len() < limit && per_file.iter().any(|f| !f.is_empty()) {
        for f in per_file.iter_mut() { if out.len() < limit && let Some(m) = f.pop_front() { out.push(m); } }
    }
    out
}
fn controlled(runner: Runner, new: &Report, placebo: Option<&Value>) -> (Report, bool) {
    let control = placebo.map(|p| if p["finished"] == true { proof::read_report(runner, p["output"].as_str().unwrap_or("").as_bytes()).ok() } else { None });
    let mut baseline = new.clone();
    if let Some(Some(r)) = &control {
        for (k, c) in r.tests.iter().filter(|(_, c)| c.failed > 0) { baseline.tests.entry(k.clone()).or_default().failed += c.failed; }
        baseline.problems.extend(r.problems.iter().filter(|p| !new.problems.contains(p)).cloned());
    }
    (baseline, matches!(control, Some(None)))
}
pub fn fault_outcomes(runner: Runner, new: &Report, placebo: Option<&Value>, results: &[Value]) -> Vec<&'static str> {
    let (baseline, uncontrolled) = controlled(runner, new, placebo);
    results.iter().map(|r| {
        let text = |k: &str| r[k].as_str().unwrap_or("").as_bytes().to_vec();
        let outcome = proof::fault_verdict(runner, &baseline, (r["finished"] == true, r["served"] == true), &text("output"), &text("errors"));
        if outcome == "caught" && uncontrolled { "unknown" } else { outcome }
    }).collect()
}
const CLAIMS_PER_FAULT_COUNT_MAX: usize = 3;
const RAN_FILES_COUNT_MAX: usize = 50;
pub fn fault_claims(runner: Runner, new: &Report, placebo: Option<&Value>, results: &[Value]) -> Vec<Value> {
    let (baseline, _) = controlled(runner, new, placebo);
    let known: BTreeSet<String> = baseline.tests.iter().filter(|(_, c)| c.failed > 0).map(|(k, _)| k.clone()).chain(baseline.problems.iter().cloned()).collect();
    fault_outcomes(runner, new, placebo, results).into_iter().zip(results).map(|(outcome, r)| {
        let output = r["output"].as_str().unwrap_or("").as_bytes();
        match outcome {
            "caught" => {
                let claims: Vec<Value> = proof::failures(runner, output, r["values"].as_str().unwrap_or("").as_bytes()).into_iter().filter(|(k, _)| !known.contains(k)).take(CLAIMS_PER_FAULT_COUNT_MAX).map(|(test, f)| {
                    let read = crate::polarity::read(&f.message, f.expected.as_deref(), f.actual.as_deref());
                    if read.was.is_empty() && read.now.is_empty() { json!({ "test": test, "why": proof::first_lines(&f.message) }) } else { json!({ "test": test, "now": read.was, "before": read.now }) }
                }).collect();
                json!({ "caught_by": claims })
            }
            "missed" => {
                let ran: BTreeSet<String> = proof::read_report(runner, output).map(|rep| rep.tests.keys().map(|k| file_of(k).to_string()).collect()).unwrap_or_default();
                json!({ "ran": ran.into_iter().take(RAN_FILES_COUNT_MAX).collect::<Vec<_>>() })
            }
            _ => json!({}),
        }
    }).collect()
}
const NO_FAULT_RELATED: &str = "Kern could not pick the tests related to this file";
const NO_FAULT_CHANGE: &str = "Kern found nothing in its change to put a fault in (only comments, formatting, logging or TypeScript types changed, or no line it knows how to change)";
const NO_FAULT_RESULTS: &str = "Kern's fault run gave no results";
const NO_FAULT_RUNNER: &str = "Kern puts faults in code only where the tests run under jest or vitest";
type FaultRun = (Vec<Value>, Vec<Value>, BTreeSet<String>);
pub const CONTROL_SECS_MAX: u64 = 240;
const INDEXER_MAIN: &str = "/workspace/.kern-tools/scip/node_modules/@sourcegraph/scip-typescript/dist/src/main.js";
fn indexed(out: &Path, volume: &str, (repo, head): (&Path, &str), head_paths: Option<&[String]>, seconds: u64) -> Result<crate::map::Map, String> {
    let has = |p: &str| match p {
        "package.json.workspaces" => crate::git::git(repo, &["show", &format!("{head}:package.json")], None).ok().and_then(|m| serde_json::from_slice::<Value>(&m).ok()).is_some_and(|m| !m["workspaces"].is_null()),
        _ => head_paths.is_some_and(|paths| paths.iter().any(|x| x == p)),
    };
    let mut command = vec!["/usr/local/bin/node".to_string(), format!("--max-old-space-size={}", test_limits(seconds).memory_mb * 3 / 4), "/kern/map.cjs".to_string(), INDEXER_MAIN.to_string()];
    command.extend(crate::map::flags(&has).into_iter().map(str::to_string));
    let config = json!({ "version": 1, "command": command, "timeout_seconds": seconds, "cwd": "", "env": {}, "files": crate::map::files(&has), "early_secs": 0, "faults": [], "serve": {}, "steps": [] });
    let run = in_test_box(out, volume, "tests-map", ("drive-test.json", &config), ("__drive_test", false, seconds)).map_err(|e| format!("the indexer could not run: {}", e.0))?;
    let parts = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS).map_err(|_| format!("the indexer gave no readable result: {}", crate::sandbox::refusal(&run.errors).unwrap_or_default()))?;
    if parts["ending"] != b"exit 0" {
        let _ = std::fs::write(out.join("map.err"), [parts["errors"].as_slice(), b"\n", &parts["output"]].concat());
        return Err(crate::map::stopped(&String::from_utf8_lossy(&parts["ending"]), &parts["errors"], &parts["output"]));
    }
    let _ = std::fs::write(out.join("map.err"), &parts["errors"]);
    let m = crate::map::read(&parts["output"]).map_err(|e| e.0)?;
    let _ = std::fs::write(out.join("map.facts.jsonl"), &parts["output"]);
    Ok(m)
}
fn mapped(out: &Path, volume: &str, (repo, head): (&Path, &str), head_paths: Option<&[String]>, files: &[(String, String, String)], until: u64) -> (Vec<(String, usize)>, Value) {
    let changed: Vec<String> = files.iter().map(|f| f.0.clone()).collect();
    let house = crate::house::build(repo, head).ok();
    let fallback = |why: String| {
        let near = house.as_ref().map(|h| crate::house::neighbours(h, &changed, crate::house::NEIGHBOURS_COUNT_MAX)).unwrap_or_default();
        (near, json!({ "status": "NOT_RUN", "why": why }))
    };
    let seconds = within(until, crate::map::MAP_SECS_MAX);
    if seconds < crate::map::MAP_SECS_MIN { return fallback("Kern's time for this check ran short before the code could be mapped".into()); }
    let m = match indexed(out, volume, (repo, head), head_paths, seconds) { Ok(m) => m, Err(why) => return fallback(why) };
    let missing = crate::map::uncovered(&m, &changed);
    if !missing.is_empty() { return fallback(format!("the index does not hold {}", missing.join(", "))); }
    let lines: Vec<(String, Vec<usize>)> = files.iter().map(|(p, old, new)| {
        let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        (p.clone(), crate::diff::changed_lines(&a, &b).1.into_iter().map(|l| l + 1).collect())
    }).collect();
    let r = crate::map::reach(&m, &lines, &|file, line| house.as_ref().and_then(|h| h.imported(file, line)));
    let rooms: Vec<Value> = house.as_ref().map(|h| {
        let mut v: Vec<(usize, &str, &String, crate::map::Touch)> = r.files(&m).into_iter().filter(|(_, _, t)| *t != crate::map::Touch::Tests).filter_map(|(f, hops, t)| h.rooms.get(f).map(|k| (hops, f, k, t))).collect();
        v.sort();
        v.into_iter().map(|(hops, f, k, t)| json!({ "room": f, "kind": k, "hops": hops, "how": t.name() })).collect()
    }).unwrap_or_default();
    let product: Vec<String> = house.as_ref().map(|h| h.exports.keys().cloned().collect()).unwrap_or_default();
    let coverage = crate::map::coverage(&m, &product);
    let _ = std::fs::write(out.join("map.json"), serde_json::to_vec_pretty(&json!({ "summary": crate::map::summary(&m), "coverage": coverage, "reach": crate::map::reach_json(&m, &r, &lines), "rooms": rooms, "house": house.as_ref().map(crate::house::summary) })).unwrap_or_default());
    let not_indexed = coverage["not_indexed_count"].as_u64().unwrap_or(0) as usize;
    (crate::map::neighbours(&m, &r, &changed, crate::house::NEIGHBOURS_COUNT_MAX), crate::map::report(&m, &r, rooms, not_indexed))
}
pub fn after_tests(left_secs: u64) -> (bool, bool) {
    let plant = faults_fit(left_secs);
    let spent = if plant { fault_seconds(left_secs) } else { CONTROL_SECS_MAX };
    (plant, bystanders_fit(left_secs.saturating_sub(spent)))
}
fn planted_faults(out: &Path, volume: &str, suite: &Suite, new: &Report, files: &[(String, String, String)], label: &str, (seconds, plant): (u64, bool)) -> FaultRun {
    let (tests, dir) = (&suite.new.0, suite.dir.as_str());
    let command = |paths: BTreeSet<String>| related(tests, &paths, dir).map(|t| test_command(&t).into_iter().filter(|a| !crate::sandbox::PROGRESS_ARGS[1..].contains(&a.as_str())).collect::<Vec<String>>());
    let limit = crate::sandbox::FAULTS_COUNT_MAX - 1;
    let planted = if plant { faults(files, limit) } else { Vec::new() };
    let kept: Vec<(&crate::proof::Mutant, Vec<String>)> = planted.iter().filter_map(|m| Some((m, command([m.path.clone()].into())?))).collect();
    let mut missing: Vec<Value> = files.iter().filter(|(p, _, _)| plant && !kept.iter().any(|(m, _)| m.path == *p)).map(|f| {
        let why = if planted.iter().any(|m| m.path == f.0) { NO_FAULT_RELATED.to_string() }
            else if !faults(std::slice::from_ref(f), limit).is_empty() { format!("Kern puts at most {limit} faults in one check, and the other changed files took them") }
            else { NO_FAULT_CHANGE.to_string() };
        json!({ "path": f.0, "why": why })
    }).collect();
    let paths: BTreeSet<String> = if kept.is_empty() { files.iter().map(|(p, _, _)| p.clone()).collect() } else { kept.iter().map(|(m, _)| m.path.clone()).collect() };
    let Some(control) = command(paths.clone()).or_else(|| kept.first().map(|(_, c)| c.clone())) else { return (Vec::new(), missing, BTreeSet::new()) };
    let Some(first) = paths.iter().next() else { return (Vec::new(), missing, BTreeSet::new()) };
    let placebo = json!({ "path": first, "start": 0, "end": 0, "original": "", "replacement": "", "command": control });
    let specs: Vec<Value> = std::iter::once(placebo).chain(kept.iter().map(|(m, c)| json!({ "path": m.path, "start": m.start, "end": m.end, "original": m.original, "replacement": m.replacement, "command": c }))).collect();
    let results = run_faults(out, volume, (tests, &suite.new.1), dir, specs, seconds);
    let Some((placebo, runs)) = results.split_first().filter(|(_, runs)| runs.len() == kept.len()) else {
        if plant { missing.extend(paths.iter().map(|p| json!({ "path": p, "why": NO_FAULT_RESULTS }))); }
        return (Vec::new(), missing, BTreeSet::new());
    };
    let _ = std::fs::write(out.join(format!("faults{label}.json")), serde_json::to_vec(&json!({ "placebo": placebo, "faults": runs })).unwrap_or_default());
    let outcomes = fault_outcomes(tests.runner, new, Some(placebo), runs);
    let claims = fault_claims(tests.runner, new, Some(placebo), runs);
    let related: BTreeSet<String> = proof::read_report(tests.runner, placebo["output"].as_str().unwrap_or("").as_bytes()).map(|r| r.tests.keys().map(|k| file_of(k).to_string()).collect()).unwrap_or_default();
    let entries = kept.iter().zip(outcomes).zip(claims).map(|(((m, _), o), claim)| {
        let mut entry = json!({ "path": m.path, "line": m.line, "span": [m.start, m.end], "what": m.what(), "kind": m.kind.name(), "target": m.target, "outcome": o });
        if m.kind == crate::proof::FaultKind::Undo { for (k, v) in claim.as_object().into_iter().flatten() { entry[k] = v.clone(); } }
        entry
    }).collect();
    (entries, missing, related)
}
pub const BYSTANDER_SECS_MIN: u64 = 240;
pub const BYSTANDER_SECS_MAX: u64 = 480;
const BYSTANDER_FILES_COUNT_MAX: usize = 40;
const SERVE_BYTES_MAX: usize = 2 << 20;
pub fn bystanders_fit(left_secs: u64) -> bool { left_secs >= BYSTANDER_SECS_MIN + TYPES_SECS_RESERVE }
pub type Texts = serde_json::Map<String, Value>;
pub struct Traced { pub trace: String, pub finished: bool, pub network: bool }
pub fn reached_out(behavior: &str) -> bool {
    behavior.lines().any(|l| l.starts_with("dns:") || l.strip_prefix("net:").is_some_and(|t| t != "ipc" && t != "loopback"))
}
pub fn traced_run(out: &Path, volume: &str, (tests, env): (&Tests, &Env), dir: &str, (files, serve): (&Texts, &Texts), label: &str, seconds: u64) -> Option<Traced> {
    let command: Vec<String> = test_command(tests).into_iter().filter(|a| !crate::sandbox::PROGRESS_ARGS.contains(&a.as_str())).collect();
    let config = json!({ "version": 1, "command": command, "timeout_seconds": seconds, "cwd": dir, "env": sized_env(tests.runner, env), "files": files, "early_secs": 0, "faults": [], "serve": serve, "steps": [] });
    let run = in_test_box(out, volume, &format!("trace-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds)).ok()?;
    let parts = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS).ok()?;
    let _ = std::fs::write(out.join(format!("trace-{label}.err")), &parts["errors"]);
    Some(Traced { trace: String::from_utf8_lossy(&parts["trace"]).to_string(), finished: parts["ending"].starts_with(b"exit"), network: reached_out(&String::from_utf8_lossy(&parts["behavior"])) })
}
pub fn watched(files: &[(String, String, String)], old_side: bool) -> Texts {
    let mut out = serde_json::Map::new();
    let mut bytes = 0usize;
    for (path, old, new) in files {
        let text = if old_side { old } else { new };
        let Some(t) = (!text.is_empty()).then(|| crate::trace::instrument(path, text)).flatten() else { continue };
        if bytes + t.len() > SERVE_BYTES_MAX || out.len() >= crate::sandbox::SERVE_FILES_COUNT_MAX { break; }
        bytes += t.len();
        out.insert(path.clone(), json!(t));
    }
    out
}
#[allow(clippy::too_many_arguments)]
fn bystanders(out: &Path, (old_volume, new_volume): (&str, &str), suite: &Suite, files: &[(String, String, String)], related: &BTreeSet<String>, touched: &BTreeSet<String>, label: &str, seconds: u64) -> Value {
    let none = |why: String| json!({ "status": "NONE", "why": why });
    let Some((old_tests, old_env)) = suite.old.as_ref() else { return none("the old code has no test runner Kern can read".into()) };
    let chosen: BTreeSet<String> = related.iter().filter(|f| !touched.contains(*f) && kept_back(f).is_none()).take(BYSTANDER_FILES_COUNT_MAX).cloned().collect();
    if chosen.is_empty() { return none("no test this change leaves untouched loads the changed code".into()); }
    let (old_serve, new_serve) = (watched(files, true), watched(files, false));
    if old_serve.is_empty() || new_serve.is_empty() { return none("Kern could not rewrite the changed code to watch it".into()); }
    let (Some(old_t), Some(new_t)) = (kept_focus(old_tests, &chosen, &suite.dir), kept_focus(&suite.new.0, &chosen, &suite.dir)) else { return none(SINGLE_FILES.into()) };
    let each = (seconds / 3).max(60);
    let mut traces = Vec::new();
    for (side, volume, tests, serve) in [("old", old_volume, (&old_t, old_env), &old_serve), ("old-again", old_volume, (&old_t, old_env), &old_serve), ("new", new_volume, (&new_t, &suite.new.1), &new_serve)] {
        let Some(traced) = traced_run(out, volume, tests, &suite.dir, (&Texts::new(), serve), &format!("{side}{label}"), each).filter(|t| t.finished) else { return none(format!("the {side} run of the untouched tests did not finish")) };
        let _ = std::fs::write(out.join(format!("trace-{side}{label}.jsonl")), &traced.trace);
        traces.push(traced.trace);
    }
    let mut c = crate::trace::compare(&traces[0], &traces[1], &traces[2]);
    c["status"] = json!("RAN");
    c["tests"] = json!(chosen.len());
    c
}
fn run_faults(out: &Path, volume: &str, (tests, env): (&Tests, &Env), dir: &str, faults: Vec<Value>, seconds: u64) -> Vec<Value> {
    if seconds < 120 || faults.is_empty() { return Vec::new(); }
    let config = json!({ "version": 1, "command": test_command(tests), "timeout_seconds": seconds, "cwd": dir, "env": sized_env(tests.runner, env), "files": {}, "early_secs": 0, "faults": faults, "serve": {}, "steps": [] });
    let Ok(run) = in_test_box(out, volume, "tests-faults", ("drive-test.json", &config), ("__drive_test", false, seconds)) else { return Vec::new() };
    let Ok(parts) = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) else { return Vec::new() };
    serde_json::from_slice::<Vec<Value>>(&parts["output"]).unwrap_or_default()
}
pub fn dotenv(text: &str) -> Vec<(String, String)> {
    text.lines().filter_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') { return None; }
        let (k, v) = l.strip_prefix("export ").unwrap_or(l).split_once('=')?;
        let k = k.trim();
        let v = v.trim();
        let v = if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2) || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2) { &v[1..v.len() - 1] }
            else { v.split(" #").next().unwrap_or(v).trim() };
        (crate::script::env_name(k) && v.len() <= 4096 && !v.contains('\0')).then(|| (k.to_string(), v.to_string()))
    }).collect()
}
pub fn partly_missing(r: &Report) -> Option<String> {
    let markers = ["Cannot find module", "Cannot find package", "Failed to resolve import", "Failed to resolve entry for package", "Failed to load url", "Could not resolve"];
    let text: Vec<&String> = r.why.values().filter(|w| markers.iter().any(|m| w.contains(m))).collect();
    (!text.is_empty()).then(|| text.iter().map(|w| w.as_str()).collect::<Vec<_>>().join("\n"))
}
const BUILD_DIRS: [&str; 7] = ["dist", "build", "lib", "out", "es", "esm", "cjs"];
pub fn own_build(why: &str) -> bool {
    why.split(['\'', '"', ' ', '(', ')', ',', '`']).any(|spec| (spec.starts_with("./") || spec.starts_with("../")) && spec.split('/').any(|seg| BUILD_DIRS.contains(&seg)))
}
pub fn builds_tests(command: &[String]) -> bool {
    let built = |a: &str| { let p = a.trim_start_matches("./"); p.contains('/') && p.split('/').next().is_some_and(|d| BUILD_DIRS.contains(&d)) };
    command.iter().skip(1).any(|a| !a.starts_with('-') && built(a)) || command.windows(2).any(|w| w[0] == "--rootDir" && BUILD_DIRS.contains(&w[1].trim_start_matches("./").trim_end_matches('/')))
}
const NO_TEST_FILES: [&str; 3] = ["No test files found", "No tests found", "Could not find any test files"];
pub fn no_test_files(why: &str) -> bool { NO_TEST_FILES.iter().any(|m| why.contains(m)) }
const FOUND_NONE: &str = "the test runner ran but found no tests on either side";
const NO_TESTS: &str = "the project's test script finds no test files, so no test checks this change";
fn cannot_load(old: Option<&Side>) -> Option<String> {
    match old {
        Some(Side::Nothing(why)) => Some(why.clone()),
        Some(Side::Ran(r, _)) if r.tests.values().all(|c| c.passed == 0) && !r.problems.is_empty() => Some(r.why.values().cloned().collect::<Vec<_>>().join("\n")),
        _ => None,
    }
}
type KeptRuns = (Result<Report, String>, Option<Report>, bool);
#[allow(clippy::too_many_arguments)]
fn kept_runs(out: &Path, volumes: (&str, &str), suite: &Suite, old: &Report, keep: &Kept, label: &str, until: u64, timed: &mut impl FnMut(String, u64)) -> KeptRuns {
    if keep.run.is_empty() { return (Ok(Report::default()), None, false); }
    let Some(t) = kept_focus(&suite.new.0, &keep.run, &suite.dir) else { return (Err(SINGLE_FILES.into()), None, false) };
    let at = now_ms();
    let first = run_with(out, (volumes.0, &keep.files, test_seconds()), (&t, &suite.new.1), &suite.dir, &format!("new-kept{label}"), until);
    timed(format!("new-kept{label}"), at);
    let first = match first {
        Ok(Side::Ran(r, _)) => unkept(&r, &keep.back),
        Ok(Side::Late) => return (Err(LATE.into()), None, true),
        Ok(Side::Nothing(why) | Side::Slow(why) | Side::Cut(why)) => return (Err(why), None, false),
        Err(e) => return (Err(e.0), None, false),
    };
    let candidates = rewritten(old, &first, None);
    let files: BTreeSet<String> = keep.back.iter().filter(|(_, o)| candidates.iter().any(|c| file_of(c) == o.as_str())).map(|(k, _)| k.clone()).collect();
    let Some((t, env)) = suite.old.as_ref().filter(|_| !files.is_empty()).and_then(|(t, e)| kept_focus(t, &files, &suite.dir).map(|t| (t, e))) else { return (Ok(first), None, false) };
    let at = now_ms();
    let second = run_with(out, (volumes.1, &keep.old_files, test_seconds()), (&t, env), &suite.dir, &format!("old-kept{label}"), until);
    timed(format!("old-kept{label}"), at);
    let second = match second { Ok(Side::Ran(r, _)) => Some(unkept(&r, &keep.back)), _ => None };
    (Ok(first), second, false)
}

pub const LATE: &str = "Kern's time for this check ran out before these tests could finish";
pub const TYPES_SECS_MIN: u64 = 240;
pub const TYPES_SECS_RESERVE: u64 = 480;
pub const FAULTS_SECS_MIN: u64 = 300;
pub const FAULTS_SECS_MAX: u64 = 480;
pub const FAULT_FILES_COUNT_MAX: usize = 20;
pub fn types_fit(left_secs: u64) -> bool { left_secs >= TYPES_SECS_MIN }
pub fn fault_seconds(left_secs: u64) -> u64 { left_secs.saturating_sub(TYPES_SECS_RESERVE).min(FAULTS_SECS_MAX) }
pub fn faults_fit(left_secs: u64) -> bool { fault_seconds(left_secs) >= FAULTS_SECS_MIN }

pub fn run(out: &Path, op: &Operator, until: u64) -> R<Value> {
    let d = read_json(out.join("drive.json"))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let (base, head) = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let manifest = |commit: &str, dir: &str| -> Option<Vec<u8>> {
        let path = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
        crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok()
    };
    let has = |commit: &str, path: String| crate::git::git(repo, &["cat-file", "-e", &format!("{commit}:{path}")], None).is_ok();
    let planned = |commit: &str, dir: &str| ready(plan(manifest(commit, dir).as_deref())).or_else(|| {
        let scripted = manifest(commit, dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).is_some_and(|v| v["scripts"]["test"].is_string());
        if dir.is_empty() || scripted { None } else { by_config(|f| has(commit, format!("{dir}/{f}"))) }
    });
    let (mut not_run, mut testless) = (Vec::new(), Vec::new());
    let suites: Vec<Suite> = match plan(manifest(head, "").as_deref()) {
        Plan::Run(t, env) => vec![Suite { dir: String::new(), old: ready(plan(manifest(base, "").as_deref())), new: (t, env), uneven: None }],
        Plan::Packages | Plan::Nothing => {
            let changed = crate::rules::changed(repo, base, head)?;
            let paths: Vec<&str> = changed.iter().map(|(p, _)| p.as_str()).collect();
            let mut list = Vec::new();
            let project = |dir: &str| !dir.is_empty() && [head, base].iter().any(|c| has(c, format!("{dir}/project.json")));
            for dir in packages(&paths, |dir| manifest(head, dir).is_some() || manifest(base, dir).is_some() || project(dir)) {
                let Some(new) = planned(head, &dir) else {
                    let script = manifest(head, &dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).and_then(|v| v["scripts"]["test"].as_str().map(str::to_string));
                    match script {
                        Some(s) if crate::script::placeholder(&s) => testless.push(dir.clone()),
                        Some(s) => not_run.push(json!({ "dir": dir, "why": format!("Kern cannot run this package's test script: {s}") })),
                        None => {}
                    }
                    continue;
                };
                if list.len() == MAX_PACKAGES { not_run.push(json!({ "dir": dir, "why": format!("Kern runs the tests of at most {MAX_PACKAGES} packages per change") })); continue; }
                list.push(Suite { old: planned(base, &dir), dir, new, uneven: None });
            }
            list
        }
    };
    if suites.is_empty() {
        let script = manifest(head, "").and_then(|m| serde_json::from_slice::<Value>(&m).ok()).and_then(|v| v["scripts"]["test"].as_str().map(str::to_string)).filter(|s| !crate::script::placeholder(s));
        return Ok(match (not_run.first().map(|n| n["why"].clone()), script) {
            (Some(why), _) => json!({ "status": "NOT_RUN", "why": why, "not_run": not_run, "no_tests_in": testless }),
            (None, _) if !testless.is_empty() => json!({ "status": "NOT_RUN", "why": "the test scripts of the changed packages run no tests, so no test checks this change", "no_tests": true, "no_tests_in": testless }),
            (None, Some(s)) => json!({ "status": "NOT_RUN", "why": format!("Kern cannot run this project's test script, `{}` (Kern runs vitest, jest, mocha and node --test)", s.chars().take(200).collect::<String>()) }),
            (None, None) => json!({ "status": "NOT_RUN", "why": "the project has no test script, so no test checks this change", "no_tests": true }),
        });
    }
    let load = |commit: &str, dir: &str, env: &mut Env| {
        let Some(files) = env.remove(crate::script::ENV_FILES) else { return };
        for f in files.lines().filter(|f| crate::util::file_path(f).is_ok()) {
            let path = if dir.is_empty() { f.to_string() } else { format!("{dir}/{f}") };
            let Some(text) = crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok().map(|b| String::from_utf8_lossy(&b).to_string()) else { continue };
            for (k, v) in dotenv(&text) { if env.len() < 32 { env.entry(k).or_insert(v); } }
        }
    };
    let mut suites = suites;
    for suite in suites.iter_mut() {
        let dir = suite.dir.clone();
        load(head, &dir, &mut suite.new.1);
        if let Some((_, env)) = suite.old.as_mut() { load(base, &dir, env); }
    }
    let mut seconds = serde_json::Map::new();
    let mut timed = |name: String, from: u64| { seconds.insert(name, json!(secs_since(from))); };
    let at = now_ms();
    if !out.join("setup.json").exists() { crate::drive::setup(out, op, until)?; timed("setup".into(), at); }
    let s = read_json(out.join("setup.json"))?;
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    let mut merged = serde_json::Map::new();
    for l in LISTS { merged.insert(l.into(), json!([])); }
    merged.insert("uneven_why".into(), json!([]));
    merged.insert("faults".into(), json!([]));
    merged.insert("faults_missing".into(), json!([]));
    merged.insert("bystanders".into(), json!({}));
    merged.insert("problem_why".into(), json!({}));
    merged.insert("polarity".into(), json!({}));
    let (mut sums, mut kept) = ([0u64; 6], 0u64);
    let (mut ran, mut all_new, mut wrote_code, mut run_broke, mut run_unconfirmed) = (Vec::new(), Vec::new(), BTreeSet::new(), Vec::new(), Vec::new());
    let (mut late, mut dropped, mut tested) = (false, BTreeSet::new(), Vec::new());
    let (mut old_created, mut edited_files, mut edited_unchecked, mut test_data) = (BTreeSet::new(), 0usize, Vec::new(), Vec::new());
    let changes = crate::rules::changed(repo, base, head)?;
    let followed = |dir: &str| manifest(base, dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref().is_some_and(follows_script);
    let head_paths: Option<Vec<String>> = crate::drive::listing(repo, head).ok().map(|items| items.into_iter().map(|i| i.path).collect());
    let testless_here = |dir: &str| followed(dir) && head_paths.as_ref().is_some_and(|p| !repo_has_tests(p, dir));
    let mut near_once: Option<(Vec<(String, usize)>, Value)> = None;
    for (nth, suite) in suites.iter().enumerate() {
        if late { not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
        let last = nth + 1 == suites.len();
        if let Some(why) = install_failure(&s, "old", &suite.dir).filter(|_| suite.old.is_some()) {
            let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
            not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} could not run on the old code: {why}") }));
            continue;
        }
        if let Some(why) = install_failure(&s, "new", &suite.dir) {
            let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
            run_unconfirmed.push(json!({ "dir": suite.dir, "why": format!("the tests{place} could not run on the new code, where {why} (it worked on the old code)") }));
            continue;
        }
        let label = label(&suite.dir);
        let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
        let inside = |p: &str| suite.dir.is_empty() || p.starts_with(&format!("{}/", suite.dir));
        let changed_code = |skip: &str| -> BTreeSet<String> { changes.iter().filter(|(p, o)| *o != skip && inside(p) && crate::capability::is_code(p)).map(|(p, _)| p.clone()).collect() };
        let (mut narrowed, mut old_seconds, mut whole_why) = (None, 0u64, String::new());
        let mut uneven_why = setup_differs(&s, &suite.dir);
        let (mut built, mut tried, mut old_tests, mut unbuilt) = (Vec::new(), String::new(), None, Vec::new());
        let old_run = match &suite.old {
            None => None,
            Some((t, env)) => {
                let (mut tests, mut phase, mut limit, mut repairs_left, mut narrowable, mut pending) = (t.clone(), format!("old{label}"), test_seconds(), REPAIR_ROUNDS_COUNT_MAX, true, Vec::new());
                let (mut attempted, mut pending_failed, mut build_secs): (BTreeSet<String>, Vec<(String, String)>, u64) = (BTreeSet::new(), Vec::new(), 2 * PREPARE_SECS_MAX);
                let run = loop {
                let (at, left_before) = (now_ms(), within(until, u64::MAX));
                let run = run_for(out, (&volume("old"), limit), (&tests, env), &suite.dir, &phase, until).unwrap_or_else(|e| Side::Cut(e.0));
                timed(phase.clone(), at);
                old_seconds = secs_since(at);
                if !pending.is_empty() {
                    if cannot_load(Some(&run)).is_some() { tried = ran_first(&pending, &pending_failed); }
                    else { built.extend(pending.iter().filter(|p| !pending_failed.iter().any(|(f, _)| f == *p)).cloned()); }
                    pending.clear();
                    pending_failed.clear();
                }
                let partial = match &run { Side::Ran(r, _) => partly_missing(r), _ => None };
                if repairs_left > 0 && let Some(why) = cannot_load(Some(&run)).or(partial) {
                    let first = repairs_left == REPAIR_ROUNDS_COUNT_MAX;
                    repairs_left -= 1;
                    let (old_packages, new_packages) = (workspace(repo, base), workspace(repo, head));
                    let mut missing = missing_packages(&why);
                    missing.extend(owners(&old_packages, &missing_files(&why)));
                    missing.retain(|m| attempted.insert(m.clone()));
                    let generated = ["Cannot find module '.", "Failed to load url .", "Failed to resolve import \".", "Cannot find module \"."].iter().any(|m| why.contains(m));
                    let compiled = no_test_files(&why);
                    let package = manifest(base, &suite.dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok());
                    let pretest = package.as_ref().filter(|v| generated && v["scripts"]["pretest"].is_string()).map(|_| suite.dir.clone());
                    let invocation = match package.as_ref().map(crate::script::found) { Some(crate::script::Found::Run(i)) => Some(i), _ => None };
                    let from_build = compiled && invocation.as_ref().is_some_and(|i| builds_tests(&i.command));
                    let mut before = invocation.filter(|_| first && (generated || compiled)).map(|i| i.before).unwrap_or_default();
                    if first && (own_build(&why) || from_build) && package.as_ref().is_some_and(|v| v["scripts"]["build"].is_string()) && !before.iter().any(|b| b == "build") { before.push("build".to_string()); }
                    let pretest = pretest.filter(|_| first);
                    let steps = repair_steps(&old_packages, &missing, pretest.as_deref(), (&suite.dir, &before));
                    let names: Vec<String> = steps.iter().map(|(d, s)| format!("{} {s}", if d.is_empty() { "." } else { d })).collect();
                    let left = within(until, u64::MAX);
                    if !steps.is_empty() && (!repair_fits(left, old_seconds, build_secs) || left.min(PREPARE_SECS_MAX) < PREPARE_SECS_MIN) { unbuilt = names; }
                    else if !steps.is_empty() {
                        let at = now_ms();
                        let on_old = prepare(out, &volume("old"), &steps, &format!("old{label}"), until);
                        let on_new = prepare(out, &volume("new"), &repair_steps(&new_packages, &missing, pretest.as_deref(), (&suite.dir, &before)), &format!("new{label}"), until);
                        timed(format!("prepare-{phase}"), at);
                        build_secs = secs_since(at);
                        match (on_old, on_new) {
                            (Err(e), _) => {
                                tried = format!(" (Kern tried running {} first, but could not: {})", names.join(", "), e.0);
                                uneven_why = Some(format!("Kern's build of {} did not finish on the old code ({}), so the two sides may hold different builds", names.join(", "), e.0));
                            }
                            (Ok(_), Err(e)) => {
                                tried = format!(" (Kern ran {} on the old code first, but could not on the new code: {})", names.join(", "), e.0);
                                uneven_why = Some(format!("Kern built {} on the old code but not on the new code ({})", names.join(", "), e.0));
                            }
                            (Ok(lo), Ok(ln)) => {
                                if let Some(u) = uneven_steps(&lo, &ln) { uneven_why = Some(u); }
                                pending_failed = failed_steps(&lo);
                                pending = names;
                                phase = phase.replacen("old", "old-built", 1);
                                continue;
                            }
                        }
                    }
                }
                if narrowable && let Side::Slow(why) = &run && tried.is_empty() && let Some((n, o)) = related(&suite.new.0, &changed_code("delete"), &suite.dir).zip(related(t, &changed_code("add"), &suite.dir)) {
                    narrowable = false;
                    whole_why = whole(why);
                    (tests, phase, narrowed, limit) = (o, format!("old-related{label}"), Some(n), related_secs(left_before, last));
                    continue;
                }
                old_tests = Some(tests.clone());
                break run;
                };
                Some(run)
            }
        };
        match &old_run {
            Some(Side::Late) => { late = true; not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
            Some(Side::Nothing(why)) if no_test_files(why) && tried.is_empty() && built.is_empty() && testless_here(&suite.dir) => { testless.push(suite.dir.clone()); continue; }
            Some(Side::Nothing(why) | Side::Cut(why)) => {
                let skipped = if unbuilt.is_empty() { String::new() } else { format!(" (Kern did not run {} first: its time ran short)", unbuilt.join(", ")) };
                not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the old code gave no results: {why}{tried}{skipped}") }));
                continue;
            }
            Some(Side::Slow(why)) => {
                let why = if narrowed.is_some() { related_none(&place, &whole_why, why) } else { format!("the tests{place} on the old code gave no results: {why}, and Kern cannot pick the tests related to the change for this runner") };
                not_run.push(json!({ "dir": suite.dir, "why": why }));
                continue;
            }
            Some(Side::Ran(r, _)) if narrowed.is_some() && r.tests.is_empty() && r.problems.is_empty() => {
                not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} were not run: {whole_why}, and no test is related to the changed files") }));
                continue;
            }
            Some(Side::Ran(r, _)) if r.tests.values().all(|c| c.passed == 0) && (!r.tests.is_empty() || !r.problems.is_empty()) => {
                not_run.push(json!({ "dir": suite.dir, "why": format!("{}{tried}", judged(r, &Report::default(), None, None)["why"].as_str().unwrap_or("")) }));
                continue;
            }
            _ => {}
        }
        let (budget, again_secs) = (if suite.old.is_some() { new_secs(old_seconds, last) } else { test_seconds() }, later_secs(old_seconds, last));
        let new_tests = narrowed.clone().unwrap_or_else(|| suite.new.0.clone());
        let whole_new = (&new_tests, &suite.new.1);
        let name = if narrowed.is_some() { "new-related" } else { "new" };
        let at = now_ms();
        let declared = std::iter::successors(Some(suite.dir.as_str()), |d| (!d.is_empty()).then(|| d.rsplit_once('/').map_or("", |(p, _)| p)))
            .any(|d| runner_declared(manifest(head, d).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref(), suite.new.0.runner));
        let first = match run_for(out, (&volume("new"), budget), whole_new, &suite.dir, &format!("{name}{label}"), until) {
            Ok(s) => kerns_install(s, declared),
            Err(e) => { not_run.push(json!({ "dir": suite.dir, "why": format!("Kern could not run the tests{place}: {}", e.0) })); continue; }
        };
        timed(format!("{name}{label}"), at);
        let ran_secs = secs_since(at);
        let new = match first {
            Side::Ran(r, created) => { wrote_code.extend(created); r }
            Side::Late if ran_secs >= RUN_SECS_MIN && matches!(old_run, Some(Side::Ran(..))) => {
                late = true;
                run_unconfirmed.push(json!({ "dir": suite.dir, "why": ran_out(&place, old_seconds, ran_secs) }));
                continue;
            }
            Side::Late => { late = true; not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
            Side::Nothing(why) | Side::Slow(why) | Side::Cut(why) if !matches!(old_run, Some(Side::Ran(..))) => {
                not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the new code gave no results: {why}") }));
                continue;
            }
            Side::Slow(why) | Side::Cut(why) if budget >= test_seconds() => {
                run_unconfirmed.push(json!({ "dir": suite.dir, "why": format!("the tests{place} finished on the old code but not on the new code: {why}") }));
                continue;
            }
            first => {
                let at = now_ms();
                let again = run_for(out, (&volume("new"), again_secs), whole_new, &suite.dir, &format!("new-again{label}"), until).map(|s| kerns_install(s, declared));
                timed(format!("new-again{label}"), at);
                let cut = |why: &str| json!({ "dir": suite.dir, "why": format!("the tests{place} gave no results on the new code once, where the old code's tests ran, and {why}") });
                match again {
                    Ok(Side::Ran(r, created)) => { wrote_code.extend(created); r }
                    Ok(Side::Late) => { late = true; run_unconfirmed.push(cut("Kern's time for this check ran out before a second run could finish")); continue; }
                    Err(e) => { run_unconfirmed.push(cut(&format!("running them again failed: {}", e.0))); continue; }
                    Ok(again) => {
                        let old_again = (no_results_twice(&first, &again) && uneven_why.is_none()).then(|| old_tests.as_ref().zip(suite.old.as_ref()).map(|(t, (_, env))| {
                            let at = now_ms();
                            let r = run_for(out, (&volume("old"), again_secs), (t, env), &suite.dir, &format!("old-again{label}"), until);
                            timed(format!("old-again{label}"), at);
                            r.map_err(|e| e.0)
                        })).flatten();
                        if let Some(Ok(Side::Late)) = old_again { late = true; run_unconfirmed.push(cut("Kern's time ran out before it could run the old code's tests again to confirm")); continue; }
                        if broke_run(&first, &again, old_again.as_ref().and_then(|r| r.as_ref().ok()), uneven_why.is_some()) {
                            let passed = match &old_again { Some(Ok(Side::Ran(r, _))) => r.sum().passed, _ => 0 };
                            run_broke.push(json!({ "dir": suite.dir, "why": again.ending(), "old_rerun_passed": passed }));
                            continue;
                        }
                        let why = unconfirmed_why(&first, &again, old_again.as_ref(), uneven_why.as_deref());
                        run_unconfirmed.push(json!({ "dir": suite.dir, "why": format!("the tests{place} give no results on the new code ({}), but {why}", again.ending()) }));
                        continue;
                    }
                }
            }
        };
        let old = match old_run {
            Some(Side::Ran(r, created)) => { old_created.extend(created); r }
            _ => { all_new.push(if suite.dir.is_empty() { ".".to_string() } else { suite.dir.clone() }); Report::default() }
        };
        let suspect = suspects(&old, &new);
        let present = |r: &Report, f: &String| r.problems.iter().any(|p| file_of(p) == f.as_str()) || r.tests.keys().any(|k| file_of(k) == f.as_str());
        let new_files: BTreeSet<String> = suspect.iter().filter(|f| present(&new, f)).cloned().collect();
        let old_files: BTreeSet<String> = suspect.iter().filter(|f| present(&old, f)).cloned().collect();
        let old_rerun = |alone: bool| -> Option<(&str, &str, Tests, &Env, u64)> {
            let ((plain, env), first) = (suite.old.as_ref()?, old_tests.as_ref()?);
            if old_files.is_empty() { return None; }
            Some(match kept_focus(plain, &old_files, &suite.dir).filter(|_| alone) { Some(t) => ("old-alone", "old", t, env, test_seconds()), None => ("old-again", "old", first.clone(), env, again_secs) })
        };
        let plan: Vec<(&str, &str, Tests, &Env, u64)> = if new_files.is_empty() {
            Vec::new()
        } else if let Some(new_alone) = kept_focus(&suite.new.0, &new_files, &suite.dir) {
            [Some(("new-alone", "new", new_alone, &suite.new.1, test_seconds())), old_rerun(true)].into_iter().flatten().collect()
        } else {
            [Some(("new-again", "new", whole_new.0.clone(), whole_new.1, again_secs)), old_rerun(false)].into_iter().flatten().collect()
        };
        let (mut alone_new, mut whole, mut second_old, mut unconfirmed) = (None, None, None, false);
        for (name, side, tests, env, limit) in &plan {
            let at = now_ms();
            let r = run_for(out, (&volume(side), *limit), (tests, env), &suite.dir, &format!("{name}{label}"), until);
            timed(format!("{name}{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) if *name == "new-alone" => { wrote_code.extend(created); alone_new = Some(r); }
                Ok(Side::Ran(r, created)) if *side == "new" => { wrote_code.extend(created); whole = Some(r); }
                Ok(Side::Ran(r, created)) => { old_created.extend(created); second_old = Some(r); }
                Ok(Side::Late) => { late = true; unconfirmed = true; break; }
                Err(_) => { unconfirmed = true; break; }
                Ok(Side::Nothing(_) | Side::Slow(_) | Side::Cut(_)) => {}
            }
        }
        if !unconfirmed && plan.first().is_some_and(|p| p.0 == "new-alone") && alone_new.as_ref().is_none_or(|g| !repeats(&old, &new, g)) {
            let at = now_ms();
            let r = run_for(out, (&volume("new"), again_secs), whole_new, &suite.dir, &format!("new-again{label}"), until);
            timed(format!("new-again{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) => { wrote_code.extend(created); whole = Some(r); }
                Ok(Side::Late) => { late = true; unconfirmed = true; }
                Err(_) => unconfirmed = true,
                Ok(Side::Nothing(_) | Side::Slow(_) | Side::Cut(_)) => {}
            }
        }
        let empty = |r: &Option<Report>| r.as_ref().is_none_or(|r| r.tests.is_empty() && r.problems.is_empty());
        if !unconfirmed && empty(&second_old) && plan.iter().any(|p| p.0 == "old-alone") && let (Some(first), Some((_, env))) = (old_tests.as_ref(), &suite.old) {
            let at = now_ms();
            let r = run_for(out, (&volume("old"), again_secs), (first, env), &suite.dir, &format!("old-again{label}"), until);
            timed(format!("old-again{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) => { old_created.extend(created); second_old = Some(r); }
                Ok(Side::Late) => late = true,
                _ => {}
            }
        }
        if !plan.is_empty() && alone_new.is_none() && whole.is_none() { unconfirmed = true; }
        let mut c = verdict(&old, &new, alone_new.as_ref(), whole.as_ref(), second_old.as_ref(), unconfirmed, uneven_why.is_some());
        let runner = suite.new.0.runner;
        let named = |k: &str| in_suite(runner, &suite.dir, k);
        let list = |v: &Value| -> Vec<Value> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|x| json!(named(x))).collect() };
        if c["status"] != "RAN" && c["why"] == FOUND_NONE && narrowed.is_none() && testless_here(&suite.dir) { testless.push(suite.dir.clone()); continue; }
        if c["status"] != "RAN" {
            not_run.push(json!({ "dir": suite.dir, "why": c["why"] }));
            if let Some(a) = merged["new_failing"].as_array_mut() { a.extend(list(&c["new_failing"])); }
            continue;
        }
        let keep = kept_plan(repo, (base, head), &changes, &old, &suite.dir);
        let (first, second, cut) = kept_runs(out, (&volume("new"), &volume("old")), suite, &old, &keep, &label, until, &mut timed);
        late |= cut;
        let outcome = kept_outcome(&old, &keep, first, second.as_ref());
        let failing: BTreeSet<String> = ["broke", "new_problems"].iter().flat_map(|l| c[*l].as_array().into_iter().flatten()).filter_map(Value::as_str).map(str::to_string).collect();
        edited_files += outcome.checked;
        let held: BTreeSet<&str> = outcome.held.iter().map(String::as_str).collect();
        if let Some(g) = c["gone"].as_array_mut() {
            let was = g.len();
            g.retain(|x| x.as_str().is_none_or(|k| !held.contains(k)));
            kept += (was - g.len()) as u64;
        }
        own_edits(&mut c, &outcome.held.iter().cloned().chain(tests_only(changes.iter().map(|(p, op)| (p.as_str(), *op)), &old)).collect::<Vec<_>>());
        edited_unchecked.extend(outcome.unchecked.iter().map(|(p, why)| json!({ "path": p, "why": why })));
        edited_unchecked.extend(unloaded_edits(&changes, &old, &new, &suite.dir).into_iter().filter(|(p, _)| !outcome.unchecked.iter().any(|(u, _)| u == p)).map(|(p, why)| json!({ "path": p, "why": why })));
        test_data.extend(keep.data.iter().map(|p| json!(p)));
        let wanted: BTreeSet<String> = outcome.rewritten.iter().filter(|k| !failing.contains(k.as_str())).cloned().collect();
        if let Some(a) = merged["rewritten"].as_array_mut() { a.extend(wanted.iter().map(|k| json!(named(k)))); }
        if !wanted.is_empty() {
            let kept = |kind: &str| std::fs::read(out.join(format!("tests-new-kept{label}.{kind}"))).unwrap_or_default();
            let readings = polarity(&proof::failures(runner, &kept("out"), &kept("values")), &|f| keep.back.get(f).cloned(), &wanted);
            if let Some(p) = merged.get_mut("polarity").and_then(Value::as_object_mut) { for (k, v) in readings { p.insert(named(&k), v); } }
        }
        if let Some(u) = &uneven_why && c["uneven"].as_array().is_some_and(|a| !a.is_empty()) && let Some(a) = merged["uneven_why"].as_array_mut() {
            a.push(json!(u));
        }
        fold(&mut merged, &c, &named);
        for (i, (side, field)) in [("before", "passed"), ("before", "failed"), ("before", "skipped"), ("after", "passed"), ("after", "failed"), ("after", "skipped")].iter().enumerate() {
            sums[i] += c[*side][*field].as_u64().unwrap_or(0);
        }
        kept += c["kept"].as_u64().unwrap_or(0);
        if std::env::var("KERN_FAULTS").as_deref() != Ok("0") {
            let shown = |commit: &str, p: &str| crate::git::git(repo, &["show", &format!("{commit}:{p}")], None).ok().map(|b| String::from_utf8_lossy(&b).to_string());
            let product: Vec<&String> = changes.iter().filter(|(p, op)| *op != "delete" && inside(p) && crate::capability::is_code(p) && !is_test_path(p) && product_code(p)).map(|(p, _)| p).collect();
            let files: Vec<(String, String, String)> = product.iter().take(FAULT_FILES_COUNT_MAX).filter_map(|p| Some((p.to_string(), shown(base, p).unwrap_or_default(), shown(head, p)?))).collect();
            let (near, map_status) = near_once.get_or_insert_with(|| if files.is_empty() { (Vec::new(), json!({ "status": "NONE" })) } else { mapped(out, &volume("new"), (repo, head), head_paths.as_deref(), &files, until) });
            merged.insert("map".into(), map_status.clone());
            let neighbours: Vec<(String, String, String)> = near.iter().filter_map(|(p, _)| { let s = shown(head, p)?; Some((p.clone(), s.clone(), s)) }).collect();
            let mut missing: Vec<Value> = product.iter().skip(FAULT_FILES_COUNT_MAX).map(|p| json!({ "path": p, "why": format!("Kern puts faults in at most {FAULT_FILES_COUNT_MAX} changed files per check") })).collect();
            let mut found = Vec::new();
            let left = within(until, FAULTS_SECS_MAX + TYPES_SECS_RESERVE);
            let (plant, watch) = after_tests(left);
            if files.is_empty() {}
            else if !matches!(runner, Runner::Jest | Runner::Vitest) { missing.extend(files.iter().map(|(p, _, _)| json!({ "path": p, "why": NO_FAULT_RUNNER }))); }
            else if !plant && !watch { dropped.insert("planted faults"); dropped.insert("untouched tests"); }
            else {
                if !plant { dropped.insert("planted faults"); }
                let at = now_ms();
                let (planted, lacking, related) = planted_faults(out, &volume("new"), suite, &new, &files, &label, (if plant { fault_seconds(left) } else { CONTROL_SECS_MAX.min(left.saturating_sub(TYPES_SECS_RESERVE)) }, plant));
                timed(format!("faults{label}"), at);
                found = planted;
                missing.extend(lacking);
                let left = within(until, BYSTANDER_SECS_MAX + TYPES_SECS_RESERVE);
                if !related.is_empty() && !bystanders_fit(left) { dropped.insert("untouched tests"); }
                if !related.is_empty() && bystanders_fit(left) {
                    let at = now_ms();
                    let touched: BTreeSet<String> = changes.iter().filter(|(p, _)| is_test_path(p)).map(|(p, _)| p.clone()).collect();
                    let seen = bystanders(out, (&volume("old"), &volume("new")), suite, &[files.clone(), neighbours.clone()].concat(), &related, &touched, &label, left.saturating_sub(TYPES_SECS_RESERVE));
                    timed(format!("trace{label}"), at);
                    merged["bystanders"] = crate::trace::merged(&merged["bystanders"], &seen);
                }
            }
            if let Some(a) = merged["faults"].as_array_mut() { a.extend(found); }
            if let Some(a) = merged["faults_missing"].as_array_mut() { a.extend(missing); }
        }
        let only = if narrowed.is_some() { format!(", only the tests related to the change: {whole_why}") } else { String::new() };
        let only = if built.is_empty() { only } else { format!("{only}, after Kern ran {}", built.join(", ")) };
        let only = if unbuilt.is_empty() { only } else { format!("{only}, without first running {}: Kern kept its time for the new code's run", unbuilt.join(", ")) };
        let only = match manifest(head, &suite.dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref().and_then(guessed) {
            Some(script) => format!("{only}, run directly: the project's test script, `{script}`, is not one Kern can follow"),
            None => only,
        };
        ran.push(if suite.dir.is_empty() { format!("{}{only}", runner.name()) } else { format!("{} in {}{only}", runner.name(), suite.dir) });
        tested.push(Suite { dir: suite.dir.clone(), old: suite.old.clone(), new: suite.new.clone(), uneven: uneven_why.clone() }.json());
    }
    let _ = std::fs::write(out.join("suites.json"), serde_json::to_vec(&tested).unwrap_or_default());
    let old_shapes: BTreeSet<String> = old_created.iter().map(|f: &String| path_shape(f)).collect();
    wrote_code.retain(|f| !old_shapes.contains(&path_shape(f)));
    for n in not_run.iter_mut() {
        let dir = n["dir"].as_str().unwrap_or("").to_string();
        let Some(suite) = suites.iter().find(|s| s.dir == dir) else { continue };
        if let Some(script) = manifest(head, &dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref().and_then(guessed) {
            let why = format!("{} (the project's test script, `{script}`, is not one Kern can follow, so Kern ran {} directly)", n["why"].as_str().unwrap_or(""), suite.new.0.runner.name());
            n["why"] = json!(why);
        }
    }
    let mut extra = json!({ "edited": edited_files, "edited_unchecked": edited_unchecked, "test_data": test_data, "not_run": not_run, "run_broke": run_broke, "run_unconfirmed": run_unconfirmed, "wrote_code": wrote_code, "new_failing": merged["new_failing"], "seconds": seconds, "dropped": dropped,
        "no_tests_in": testless, "node": { "old": s["old"]["node"], "new": s["new"]["node"] } });
    if !s["old"]["browser"].is_null() || !s["new"]["browser"].is_null() { extra["browser"] = json!({ "old": s["old"]["browser"], "new": s["new"]["browser"] }); }
    let with = |mut c: Value| -> Value { for (k, v) in extra.as_object().into_iter().flatten() { if c.get(k).is_none() || k != "new_failing" { c[k] = v.clone(); } } c };
    if ran.is_empty() && !run_broke.is_empty() {
        return Ok(with(json!({ "status": "BROKE", "why": "the tests run on the old code but give no results on the new code" })));
    }
    if ran.is_empty() && not_run.is_empty() && run_unconfirmed.is_empty() && !testless.is_empty() {
        return Ok(with(json!({ "status": "NOT_RUN", "runner": suites[0].new.0.runner.name(), "why": NO_TESTS, "no_tests": true })));
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

