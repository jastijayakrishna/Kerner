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
        Runner::Vitest => c.extend([format!("--outputFile={REPORT}"), "--bail=0".into(), "--sequence.shuffle=false".into(), crate::sandbox::PROGRESS_ARGS[0].to_string()]),
        Runner::Mocha => c.extend(["--reporter-options".to_string(), format!("output={REPORT}")]),
        Runner::Node => { if let Some(at) = c.iter().position(|a| a == crate::sandbox::NODE_REPORTER) { c.insert(at + 1, format!("--test-reporter-destination={REPORT}")); } }
        Runner::Jest => c.extend(std::iter::once("--bail=false").chain(crate::sandbox::PROGRESS_ARGS[1..].iter().copied()).map(str::to_string)),
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

pub fn verdict(old: &Report, new: &Report, alone: Option<&Report>, whole: Option<&Report>, old_alone: Option<&Report>, unconfirmed: bool) -> Value {
    let c = steady(if unconfirmed { judged(old, new, None, None) } else { confirmed(old, new, alone, whole, old_alone) });
    if unconfirmed { prove(c, old, new, [None, None], None) } else { prove(c, old, new, [alone, whole], old_alone) }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observed { Passed, Failed, Skipped, Missing }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Evidence { pub old: Observed, pub old_again: Option<Observed>, pub new: Observed, pub new_again: [Option<Observed>; 2], pub shaky: bool, pub sandbox: bool }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unproven { OldFailsToo, Sandbox, Unsteady, Unconfirmed }
impl Unproven {
    pub fn list(self) -> &'static str { match self { Unproven::OldFailsToo => "environmental", Unproven::Sandbox => "sandbox", Unproven::Unsteady => "unsteady", Unproven::Unconfirmed => "unconfirmed" } }
}
pub fn blockable(e: Evidence) -> Result<(), Unproven> {
    if e.old_again == Some(Observed::Failed) { return Err(Unproven::OldFailsToo); }
    if e.old != Observed::Passed { return Err(Unproven::Unconfirmed); }
    if e.sandbox { return Err(Unproven::Sandbox); }
    let reruns: Vec<Observed> = e.new_again.iter().flatten().copied().filter(|o| *o != Observed::Missing).collect();
    if e.new != Observed::Failed || e.shaky || reruns.iter().any(|o| *o != Observed::Failed) { return Err(Unproven::Unsteady); }
    if reruns.is_empty() || e.old_again != Some(Observed::Passed) { return Err(Unproven::Unconfirmed); }
    Ok(())
}
const SANDBOX_ERRORS: [&str; 35] = ["getaddrinfo", "EAI_AGAIN", "ENOTFOUND", "ECONNREFUSED", "ECONNRESET", "ETIMEDOUT", "ENETUNREACH", "EHOSTUNREACH", "fetch failed",
    "git: not found", "command not found", "Cannot find module 'bindings'", "Could not locate the bindings file", "invalid ELF header", "compiled against a different Node.js version",
    "NODE_MODULE_VERSION", "ERR_DLOPEN_FAILED", "cannot open shared object file", "SIGSEGV", "SIGKILL", "SIGABRT", "SIGBUS", "terminated by another process", "Worker exited unexpectedly",
    "JavaScript heap out of memory", "Reached heap limit", "ERR_WORKER_OUT_OF_MEMORY", "Allocation failed", "ENOSPC", "no space left on device", "ENOMEM", "Cannot allocate memory",
    "EMFILE", "ENFILE", "too many open files"];
pub fn sandbox_error(why: &str) -> bool { SANDBOX_ERRORS.iter().any(|e| why.contains(e)) || (why.contains("spawn ") && why.contains("ENOENT")) }
fn outcome(b: proof::Count, c: proof::Count) -> Observed {
    if c.total() == 0 { return Observed::Missing; }
    match seen(b, c) { Seen::Same => Observed::Passed, Seen::Failed => Observed::Failed, Seen::Skipped => Observed::Skipped, Seen::Gone => Observed::Missing }
}
fn test_evidence(k: &str, old: &Report, new: &Report, again: &[Option<Report>; 2], old_again: Option<&Report>) -> Evidence {
    let b = count(old, k);
    let at = |r: &Report| outcome(b, count(r, k));
    Evidence { old: if b.passed > 0 && b.failed == 0 { Observed::Passed } else { Observed::Missing }, old_again: old_again.map(at), new: at(new),
        new_again: [again[0].as_ref().map(at), again[1].as_ref().map(at)], shaky: false, sandbox: false }
}
fn problem_evidence(p: &str, old: &Report, new: &Report, again: [Option<&Report>; 2], old_again: Option<&Report>) -> Evidence {
    let ran_clean = |r: &Report| r.tests.iter().any(|(k, n)| file_of(k) == file_of(p) && n.passed > 0);
    let has = |r: &Report| matched(&[p.to_string()], &r.problems).first().copied().unwrap_or(false);
    let at = |r: &Report| if has(r) { Observed::Failed } else if ran_clean(r) { Observed::Passed } else { Observed::Missing };
    Evidence { old: if !has(old) && ran_clean(old) { Observed::Passed } else { Observed::Missing }, old_again: old_again.map(at), new: at(new), new_again: again.map(|g| g.map(at)), shaky: false, sandbox: false }
}
fn prove(mut c: Value, old: &Report, new: &Report, again: [Option<&Report>; 2], old_again: Option<&Report>) -> Value {
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
            match blockable(Evidence { shaky: shaky.contains(file_of(&k)), sandbox: sandboxed(&k), ..e }) {
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
pub const LISTS: [&str; 18] = ["broke", "flaky", "gone", "newly_skipped", "added", "new_failing", "already_failing", "new_problems", "unconfirmed", "environmental", "together", "rewritten",
    "same_failure", "uneven", "unproven", "unsteady", "sandbox", "unrunnable"];
pub fn in_suite(runner: Runner, dir: &str, k: &str) -> String { if runner == Runner::Node && !dir.is_empty() && !k.contains(" > ") { format!("{dir}: {k}") } else { k.to_string() } }
pub fn fold(merged: &mut serde_json::Map<String, Value>, c: &Value, named: &dyn Fn(&str) -> String, unconfirmed: bool, uneven: bool) {
    for l in LISTS {
        let target = if (l == "broke" || l == "new_problems") && unconfirmed { "unconfirmed" } else if (l == "broke" || l == "new_problems") && uneven { "uneven" } else { l };
        let items: Vec<Value> = c[l].as_array().into_iter().flatten().filter_map(Value::as_str).map(|k| json!(named(k))).collect();
        if let Some(a) = merged.get_mut(target).and_then(Value::as_array_mut) { a.extend(items); }
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
pub fn kept_name(path: &str, n: u32) -> Option<String> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let (stem, rest) = name.split_once('.').filter(|(s, _)| !s.is_empty())?;
    let tag = if n == 0 { "kern-kept".to_string() } else { format!("kern-kept{n}") };
    Some(if dir.is_empty() { format!("{stem}.{tag}.{rest}") } else { format!("{dir}/{stem}.{tag}.{rest}") })
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
    let base = Report { tests: old.tests.iter().filter(|(k, _)| loaded.contains(file_of(k))).map(|(k, c)| (k.clone(), *c)).collect(), ..Report::default() };
    let now = settled(&base, &first);
    let held = base.tests.iter().filter(|(k, b)| b.passed > 0 && seen(**b, count(&now, k)) == Seen::Same).map(|(k, _)| k.clone()).collect();
    Outcome { rewritten: rewritten(old, &first, second), checked: loaded.len(), unchecked, held }
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
    already_failing.extend(before.problems.iter().cloned());
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
        "environmental": environmental, "same_failure": same_failure, "unproven": unproven, "unrunnable": unrunnable })
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
pub fn whole(why: &str) -> String {
    why.replacen("the tests were", "the whole suite was", 1).replacen("the tests did", "the whole suite did", 1).replacen("no test file finished", "no test file of the whole suite finished", 1)
}
pub fn follows_script(package: &Value) -> bool { matches!(crate::script::found(package), crate::script::Found::Run(_)) }
pub fn guessed(package: &Value) -> Option<String> {
    let script = package["scripts"]["test"].as_str()?;
    (crate::script::found(package) == crate::script::Found::Nothing).then(|| script.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(120).collect())
}
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
pub fn unlocked(setup: &Value, dir: &str) -> Option<String> {
    ["old", "new"].iter().find_map(|side| {
        let result = install_record(setup, side, dir)?["result"].as_str()?;
        result.contains("from package.json").then(|| format!("the {side} code's dependencies were {}", result.chars().take(120).collect::<String>()))
    })
}
pub fn setup_differs(setup: &Value, dir: &str) -> Option<String> {
    let (old, new) = (install_record(setup, "old", dir)?, install_record(setup, "new", dir)?);
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
    (o["browser"]["error"].is_string() != n["browser"]["error"].is_string()).then(|| "Kern could set up the browser the tests need on one side only".to_string())
}
pub fn uneven_steps(old: &str, new: &str) -> Option<String> {
    let codes = |log: &str| -> std::collections::BTreeMap<String, String> {
        log.lines().filter_map(|l| l.trim().strip_prefix("kern-step ")).filter_map(|r| r.rsplit_once(" exit ")).map(|(s, c)| (s.to_string(), c.to_string())).collect()
    };
    let (a, b) = (codes(old), codes(new));
    let differ: Vec<&String> = a.iter().filter(|(s, c)| b.get(*s).is_some_and(|d| d != *c)).map(|(s, _)| s).collect();
    (!differ.is_empty()).then(|| format!("{} worked on one side only", differ.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")))
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
    if sandbox_error(&String::from_utf8_lossy(errors)) || sandbox_error(&String::from_utf8_lossy(output)) { Side::Cut(said) } else { Side::Nothing(said) }
}
pub fn unreadable(refusal: Option<String>, error: String) -> Side { refusal.map_or(Side::Cut(error), |r| Side::Nothing(format!("Kern could not run the tests in the sandbox: {r}"))) }
pub fn broke_run(first: &Side, again: &Side, uneven: bool) -> bool { matches!((first, again), (Side::Nothing(_), Side::Nothing(_))) && !uneven }

fn run_side(out: &Path, op: &Operator, volume: &str, side: (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    run_with(out, op, (volume, &std::collections::BTreeMap::new(), test_seconds()), side, dir, label, until)
}
fn run_for(out: &Path, op: &Operator, (volume, limit): (&str, u64), side: (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    run_with(out, op, (volume, &std::collections::BTreeMap::new(), limit), side, dir, label, until)
}
pub fn test_limits(seconds: u64) -> Limits {
    Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: seconds + 120, memory_mb: test_memory_mb(std::fs::read_to_string("/proc/meminfo").ok().as_deref()), pids: 4096 }
}
pub fn in_test_box(out: &Path, op: &Operator, volume: &str, name: &str, config: (&str, &Value), (entry, root, seconds): (&str, bool, u64)) -> R<crate::sandbox::Execution> {
    let control = out.join(name);
    if control.exists() { std::fs::remove_dir_all(&control)?; }
    crate::sandbox::control_dir(&control, Some(config))?;
    let input = out.join("tests-input");
    std::fs::create_dir_all(&input)?;
    let image = crate::drive::side_image(out, volume, op);
    let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &image, snapshot: &input, control: &control, writable: &[], limits: &test_limits(seconds),
        entry: vec![entry.into()], network: "none", volumes: vec![], root, home_mb: 0, scratch: false, workspace: Some((volume, false)) });
    let _ = std::fs::remove_dir_all(&control);
    run
}
fn run_with(out: &Path, op: &Operator, (volume, files, limit): (&str, &std::collections::BTreeMap<String, String>, u64), (tests, env): (&Tests, &Env), dir: &str, label: &str, until: u64) -> R<Side> {
    let seconds = within(until, limit);
    if seconds < 60 { return Ok(Side::Late); }
    let cut = seconds < limit;
    let config = json!({ "version": 1, "command": test_command(tests), "timeout_seconds": seconds, "cwd": dir, "env": env, "files": files, "early": limit >= test_seconds(), "faults": [] });
    let run = match in_test_box(out, op, volume, &format!("tests-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds)) {
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
    if parts["ending"] == b"timeout" { return Ok(if cut { Side::Late } else { Side::Slow(format!("the tests did not finish in {} minutes", limit / 60)) }); }
    if let Some(why) = stopped_why(&String::from_utf8_lossy(&parts["ending"]), limit) { return Ok(if cut { Side::Late } else { Side::Slow(why) }); }
    if parts["ending"] == b"exit -1" { return Ok(Side::Cut("the test runner was stopped by a signal, so Kern does not trust its report".into())); }
    let created: Vec<String> = String::from_utf8_lossy(&parts["created"]).lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
    Ok(match proof::read_report(tests.runner, &parts["output"]) {
        Ok(r) if r.tests.is_empty() && r.problems.is_empty() && stopped_early(&parts["ending"], &parts["errors"], &parts["output"]) =>
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

struct Suite { dir: String, old: Option<(Tests, Env)>, new: (Tests, Env) }

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
pub type Workspace = std::collections::BTreeMap<String, (String, Value)>;
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
pub fn repair_steps(packages: &Workspace, missing: &BTreeSet<String>, pretest: Option<&str>, (dir, before): (&str, &[String])) -> Vec<(String, String)> {
    fn visit(name: &str, packages: &Workspace, seen: &mut BTreeSet<String>, order: &mut Vec<(String, String)>) {
        if !seen.insert(name.to_string()) || order.len() >= 20 { return; }
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
const STEPS: &str = "const{spawnSync}=require('child_process');for(const[d,s]of JSON.parse(process.env.PREPARE_STEPS)){const r=spawnSync('/usr/local/bin/npm',['run',s],{cwd:'/workspace/'+d,stdio:'inherit'});process.stderr.write(`\\nkern-step ${d||'.'} ${s} exit ${r.status}\\n`)}";
fn prepare(out: &Path, op: &Operator, volume: &str, steps: &[(String, String)], label: &str, until: u64) -> R<String> {
    let seconds = within(until, 480);
    crate::ensure!(seconds >= 60, "Kern's time ran out before it could build what the tests need");
    let env: Env = [("PREPARE_STEPS".to_string(), serde_json::to_string(steps)?)].into_iter().collect();
    let config = json!({ "version": 1, "command": ["/usr/local/bin/node", "-e", STEPS], "timeout_seconds": seconds, "cwd": "", "env": env, "files": {}, "early": false, "faults": [] });
    let run = in_test_box(out, op, volume, &format!("prepare-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds));
    let parts = crate::sandbox::parts(&run?.stdout, &crate::sandbox::TEST_PARTS)?;
    let log = String::from_utf8_lossy(&parts["errors"]).to_string();
    std::fs::write(out.join(format!("prepare-{label}.err")), &log)?;
    let sealed = in_test_box(out, op, volume, &format!("seal-{label}"), ("drive-seal.json", &json!({ "version": 1 })), ("__drive_seal", true, seconds));
    crate::ensure!(sealed?.exit_code == 0, "Kern could not make what the build wrote read-only");
    Ok(log)
}
const NOT_PRODUCT: [&str; 19] = ["node_modules", ".github", "scripts", "script", "examples", "example", "docs", "doc", "demo", "demos", "benchmark", "benchmarks", "bench",
    "fixtures", "__fixtures__", "e2e", "playground", "playgrounds", ".storybook"];
pub fn product_code(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    !path.split('/').any(|s| NOT_PRODUCT.contains(&s)) && !name.contains(".config.") && !name.contains(".stories.") && !name.contains(".story.")
}
pub fn faults(files: &[(String, String, String)], limit: usize) -> Vec<crate::proof::Mutant> {
    let mut per_file: Vec<std::collections::VecDeque<crate::proof::Mutant>> = files.iter().map(|(path, old, new)| {
        let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
        let (removed, added) = crate::compare::changed_lines(&a, &b).unwrap_or_default();
        let lines: BTreeSet<usize> = added.into_iter().map(|i| i + 1).collect();
        let gone: BTreeSet<String> = removed.into_iter().filter_map(|i| a.get(i)).map(|l| l.trim().to_string()).collect();
        crate::proof::plant_lines(path, new, &lines, &gone).into_iter().collect()
    }).collect();
    let mut out = Vec::new();
    while out.len() < limit && per_file.iter().any(|f| !f.is_empty()) {
        for f in per_file.iter_mut() { if out.len() < limit && let Some(m) = f.pop_front() { out.push(m); } }
    }
    out
}
fn run_faults(out: &Path, op: &Operator, volume: &str, (tests, env): (&Tests, &Env), dir: &str, faults: Vec<Value>, until: u64) -> Vec<Value> {
    let seconds = within(until, 300);
    if seconds < 120 || faults.is_empty() { return Vec::new(); }
    let config = json!({ "version": 1, "command": test_command(tests), "timeout_seconds": seconds, "cwd": dir, "env": env, "files": {}, "early": false, "faults": faults });
    let Ok(run) = in_test_box(out, op, volume, "tests-faults", ("drive-test.json", &config), ("__drive_test", false, seconds)) else { return Vec::new() };
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
fn kept_runs(out: &Path, op: &Operator, volumes: (&str, &str), suite: &Suite, old: &Report, keep: &Kept, label: &str, until: u64, timed: &mut impl FnMut(String, u64)) -> KeptRuns {
    if keep.run.is_empty() { return (Ok(Report::default()), None, false); }
    let Some(t) = kept_focus(&suite.new.0, &keep.run, &suite.dir) else { return (Err(SINGLE_FILES.into()), None, false) };
    let at = now_ms();
    let first = run_with(out, op, (volumes.0, &keep.files, test_seconds()), (&t, &suite.new.1), &suite.dir, &format!("new-kept{label}"), until);
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
    let second = run_with(out, op, (volumes.1, &keep.old_files, test_seconds()), (&t, env), &suite.dir, &format!("old-kept{label}"), until);
    timed(format!("old-kept{label}"), at);
    let second = match second { Ok(Side::Ran(r, _)) => Some(unkept(&r, &keep.back)), _ => None };
    (Ok(first), second, false)
}

pub const LATE: &str = "Kern's time for this check ran out before these tests could run";
pub const TYPES_SECS_MIN: u64 = 240;
pub const TYPES_SECS_RESERVE: u64 = 480;
pub const FAULTS_SECS_MIN: u64 = 300;
pub fn types_fit(left_secs: u64) -> bool { left_secs >= TYPES_SECS_MIN }
pub fn faults_fit(left_secs: u64) -> bool { left_secs >= FAULTS_SECS_MIN + TYPES_SECS_RESERVE }

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
    let mut not_run = Vec::new();
    let suites: Vec<Suite> = match plan(manifest(head, "").as_deref()) {
        Plan::Run(t, env) => vec![Suite { dir: String::new(), old: ready(plan(manifest(base, "").as_deref())), new: (t, env) }],
        Plan::Packages | Plan::Nothing => {
            let changed = crate::rules::changed(repo, base, head)?;
            let paths: Vec<&str> = changed.iter().map(|(p, _)| p.as_str()).collect();
            let mut list = Vec::new();
            let project = |dir: &str| !dir.is_empty() && [head, base].iter().any(|c| has(c, format!("{dir}/project.json")));
            for dir in packages(&paths, |dir| manifest(head, dir).is_some() || manifest(base, dir).is_some() || project(dir)) {
                let Some(new) = planned(head, &dir) else {
                    let script = manifest(head, &dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).and_then(|v| v["scripts"]["test"].as_str().map(str::to_string));
                    if let Some(script) = script { not_run.push(json!({ "dir": dir, "why": format!("Kern cannot run this package's test script: {script}") })); }
                    continue;
                };
                if list.len() == MAX_PACKAGES { not_run.push(json!({ "dir": dir, "why": format!("Kern runs the tests of at most {MAX_PACKAGES} packages per change") })); continue; }
                list.push(Suite { old: planned(base, &dir), dir, new });
            }
            list
        }
    };
    if suites.is_empty() {
        let script = manifest(head, "").and_then(|m| serde_json::from_slice::<Value>(&m).ok()).and_then(|v| v["scripts"]["test"].as_str().map(str::to_string)).filter(|s| !s.contains("no test specified"));
        return Ok(match (not_run.first().map(|n| n["why"].clone()), script) {
            (Some(why), _) => json!({ "status": "NOT_RUN", "why": why, "not_run": not_run }),
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
    merged.insert("problem_why".into(), json!({}));
    let (mut sums, mut kept) = ([0u64; 6], 0u64);
    let (mut ran, mut all_new, mut wrote_code, mut run_broke, mut run_unconfirmed) = (Vec::new(), Vec::new(), BTreeSet::new(), Vec::new(), Vec::new());
    let (mut late, mut dropped, mut testless) = (false, BTreeSet::new(), Vec::new());
    let (mut old_created, mut edited_files, mut edited_unchecked, mut test_data) = (BTreeSet::new(), 0usize, Vec::new(), Vec::new());
    let changes = crate::rules::changed(repo, base, head)?;
    let followed = |dir: &str| manifest(base, dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref().is_some_and(follows_script);
    for suite in &suites {
        if late { not_run.push(json!({ "dir": suite.dir, "why": LATE })); continue; }
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
    let label = if suite.dir.is_empty() { String::new() } else { format!("-{}", suite.dir.replace('/', "_")) };
        let place = if suite.dir.is_empty() { String::new() } else { format!(" in {}", suite.dir) };
        let inside = |p: &str| suite.dir.is_empty() || p.starts_with(&format!("{}/", suite.dir));
        let changed_code = |skip: &str| -> BTreeSet<String> { changes.iter().filter(|(p, o)| *o != skip && inside(p) && crate::capability::is_code(p)).map(|(p, _)| p.clone()).collect() };
        let (mut narrowed, mut old_seconds, mut whole_why) = (None, 0u64, String::new());
        let mut uneven_why = setup_differs(&s, &suite.dir);
        let (mut built, mut tried, mut old_tests) = (Vec::new(), String::new(), None);
        let old_run = match &suite.old {
            None => None,
            Some((t, env)) => {
                let (mut tests, mut phase, mut repairable, mut narrowable, mut pending) = (t.clone(), format!("old{label}"), true, true, Vec::new());
                let run = loop {
                let at = now_ms();
                let run = run_side(out, op, &volume("old"), (&tests, env), &suite.dir, &phase, until).unwrap_or_else(|e| Side::Cut(e.0));
                timed(phase.clone(), at);
                old_seconds = secs_since(at);
                if !pending.is_empty() {
                    if cannot_load(Some(&run)).is_some() { tried = format!(" (even after Kern ran {} first)", pending.join(", ")); } else { built = std::mem::take(&mut pending); }
                    pending.clear();
                }
                let partial = match &run { Side::Ran(r, _) => partly_missing(r), _ => None };
                if repairable && let Some(why) = cannot_load(Some(&run)).or(partial) {
                    repairable = false;
                    let missing = missing_packages(&why);
                    let generated = ["Cannot find module '.", "Failed to load url .", "Failed to resolve import \".", "Cannot find module \"."].iter().any(|m| why.contains(m));
                    let compiled = no_test_files(&why);
                    let package = manifest(base, &suite.dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok());
                    let pretest = package.as_ref().filter(|v| generated && v["scripts"]["pretest"].is_string()).map(|_| suite.dir.clone());
                    let invocation = match package.as_ref().map(crate::script::found) { Some(crate::script::Found::Run(i)) => Some(i), _ => None };
                    let from_build = compiled && invocation.as_ref().is_some_and(|i| builds_tests(&i.command));
                    let mut before = invocation.filter(|_| generated || compiled).map(|i| i.before).unwrap_or_default();
                    if (own_build(&why) || from_build) && package.as_ref().is_some_and(|v| v["scripts"]["build"].is_string()) && !before.iter().any(|b| b == "build") { before.push("build".to_string()); }
                    let steps = repair_steps(&workspace(repo, base), &missing, pretest.as_deref(), (&suite.dir, &before));
                    if !steps.is_empty() {
                        let at = now_ms();
                        let names: Vec<String> = steps.iter().map(|(d, s)| format!("{} {s}", if d.is_empty() { "." } else { d })).collect();
                        let on_old = prepare(out, op, &volume("old"), &steps, &format!("old{label}"), until);
                        let on_new = prepare(out, op, &volume("new"), &repair_steps(&workspace(repo, head), &missing, pretest.as_deref(), (&suite.dir, &before)), &format!("new{label}"), until);
                        timed(format!("prepare{label}"), at);
                        match (on_old, on_new) {
                            (Err(e), _) => tried = format!(" (Kern tried running {} first, but could not: {})", names.join(", "), e.0),
                            (Ok(_), Err(e)) => tried = format!(" (Kern ran {} on the old code first, but could not on the new code: {})", names.join(", "), e.0),
                            (Ok(lo), Ok(ln)) => {
                                if let Some(u) = uneven_steps(&lo, &ln) { uneven_why = Some(u); }
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
                    (tests, phase, narrowed) = (o, format!("old-related{label}"), Some(n));
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
            Some(Side::Nothing(why)) if no_test_files(why) && tried.is_empty() && built.is_empty() && followed(&suite.dir) => { testless.push(suite.dir.clone()); continue; }
            Some(Side::Nothing(why) | Side::Cut(why)) => { not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the old code gave no results: {why}{tried}") })); continue; }
            Some(Side::Slow(why)) => {
                let more = if narrowed.is_some() { ", nor did the tests related to the change" } else { ", and Kern cannot pick the tests related to the change for this runner" };
                not_run.push(json!({ "dir": suite.dir, "why": format!("the tests{place} on the old code gave no results: {why}{more}") }));
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
        let budget = if suite.old.is_some() { test_seconds().min((4 * old_seconds + 120).max(300)) } else { test_seconds() };
        let new_tests = narrowed.clone().unwrap_or_else(|| suite.new.0.clone());
        let whole_new = (&new_tests, &suite.new.1);
        let name = if narrowed.is_some() { "new-related" } else { "new" };
        let at = now_ms();
        let first = match run_for(out, op, (&volume("new"), budget), whole_new, &suite.dir, &format!("{name}{label}"), until) {
            Ok(s) => s,
            Err(e) => { not_run.push(json!({ "dir": suite.dir, "why": format!("Kern could not run the tests{place}: {}", e.0) })); continue; }
        };
        timed(format!("{name}{label}"), at);
        let new = match first {
            Side::Ran(r, created) => { wrote_code.extend(created); r }
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
                let again = run_side(out, op, &volume("new"), whole_new, &suite.dir, &format!("new-again{label}"), until);
                timed(format!("new-again{label}"), at);
                let cut = |why: &str| json!({ "dir": suite.dir, "why": format!("the tests{place} gave no results on the new code once, where the old code's tests ran, and {why}") });
                match again {
                    Ok(Side::Ran(r, created)) => { wrote_code.extend(created); r }
                    Ok(Side::Late) => { late = true; run_unconfirmed.push(cut("Kern's time ran out before it could run them again")); continue; }
                    Err(e) => { run_unconfirmed.push(cut(&format!("running them again failed: {}", e.0))); continue; }
                    Ok(again) if broke_run(&first, &again, uneven_why.is_some()) => { run_broke.push(json!({ "dir": suite.dir, "why": again.ending() })); continue; }
                    Ok(again) => {
                        let why = match &uneven_why { Some(u) => format!("the two sides were not set up the same way: {u}"), None => format!("a run hit one of Kern's own limits, so it proves nothing about the change ({})", first.ending()) };
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
        let old_rerun = |alone: bool| -> Option<(&str, &str, Tests, &Env)> {
            let ((plain, env), first) = (suite.old.as_ref()?, old_tests.as_ref()?);
            if old_files.is_empty() { return None; }
            Some(match kept_focus(plain, &old_files, &suite.dir).filter(|_| alone) { Some(t) => ("old-alone", "old", t, env), None => ("old-again", "old", first.clone(), env) })
        };
        let plan: Vec<(&str, &str, Tests, &Env)> = if new_files.is_empty() {
            Vec::new()
        } else if let Some(new_alone) = kept_focus(&suite.new.0, &new_files, &suite.dir) {
            [Some(("new-alone", "new", new_alone, &suite.new.1)), old_rerun(true)].into_iter().flatten().collect()
        } else {
            [Some(("new-again", "new", whole_new.0.clone(), whole_new.1)), old_rerun(false)].into_iter().flatten().collect()
        };
        let (mut alone_new, mut whole, mut second_old, mut unconfirmed) = (None, None, None, false);
        for (name, side, tests, env) in &plan {
            let at = now_ms();
            let r = run_side(out, op, &volume(side), (tests, env), &suite.dir, &format!("{name}{label}"), until);
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
            let r = run_side(out, op, &volume("new"), whole_new, &suite.dir, &format!("new-again{label}"), until);
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
            let r = run_side(out, op, &volume("old"), (first, env), &suite.dir, &format!("old-again{label}"), until);
            timed(format!("old-again{label}"), at);
            match r {
                Ok(Side::Ran(r, created)) => { old_created.extend(created); second_old = Some(r); }
                Ok(Side::Late) => late = true,
                _ => {}
            }
        }
        if !plan.is_empty() && alone_new.is_none() && whole.is_none() { unconfirmed = true; }
        let mut c = verdict(&old, &new, alone_new.as_ref(), whole.as_ref(), second_old.as_ref(), unconfirmed);
        let runner = suite.new.0.runner;
        let named = |k: &str| in_suite(runner, &suite.dir, k);
        let list = |v: &Value| -> Vec<Value> { v.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(|x| json!(named(x))).collect() };
        if c["status"] != "RAN" && c["why"] == FOUND_NONE && narrowed.is_none() && followed(&suite.dir) { testless.push(suite.dir.clone()); continue; }
        if c["status"] != "RAN" {
            not_run.push(json!({ "dir": suite.dir, "why": c["why"] }));
            if let Some(a) = merged["new_failing"].as_array_mut() { a.extend(list(&c["new_failing"])); }
            continue;
        }
        let keep = kept_plan(repo, (base, head), &changes, &old, &suite.dir);
        let (first, second, cut) = kept_runs(out, op, (&volume("new"), &volume("old")), suite, &old, &keep, &label, until, &mut timed);
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
        edited_unchecked.extend(outcome.unchecked.iter().map(|(p, why)| json!({ "path": p, "why": why })));
        test_data.extend(keep.data.iter().map(|p| json!(p)));
        if let Some(a) = merged["rewritten"].as_array_mut() { a.extend(outcome.rewritten.iter().filter(|k| !failing.contains(k.as_str())).map(|k| json!(named(k)))); }
        if let Some(u) = &uneven_why && ["broke", "new_problems"].iter().any(|l| c[*l].as_array().is_some_and(|a| !a.is_empty())) && let Some(a) = merged["uneven_why"].as_array_mut() {
            a.push(json!(u));
        }
        fold(&mut merged, &c, &named, unconfirmed, uneven_why.is_some());
        for (i, (side, field)) in [("before", "passed"), ("before", "failed"), ("before", "skipped"), ("after", "passed"), ("after", "failed"), ("after", "skipped")].iter().enumerate() {
            sums[i] += c[*side][*field].as_u64().unwrap_or(0);
        }
        kept += c["kept"].as_u64().unwrap_or(0);
        let faults_wanted = matches!(suite.new.0.runner, Runner::Jest | Runner::Vitest) && c["status"] == "RAN" && std::env::var("KERN_FAULTS").as_deref() != Ok("0");
        if faults_wanted && !faults_fit(within(until, 900)) { dropped.insert("planted faults"); }
        if faults_wanted && faults_fit(within(until, 900)) {
            let shown = |commit: &str, p: &str| crate::git::git(repo, &["show", &format!("{commit}:{p}")], None).ok().map(|b| String::from_utf8_lossy(&b).to_string());
            let files: Vec<(String, String, String)> = changes.iter()
                .filter(|(p, op)| *op != "delete" && inside(p) && crate::capability::is_code(p) && !is_test_path(p) && product_code(p))
                .take(20).filter_map(|(p, _)| Some((p.clone(), shown(base, p).unwrap_or_default(), shown(head, p)?))).collect();
            let planted = faults(&files, 6);
            let mut kept_faults = Vec::new();
            let specs: Vec<Value> = planted.iter().filter_map(|m| {
                let t = related(&suite.new.0, &[m.path.clone()].into_iter().collect(), &suite.dir)?;
                let command: Vec<String> = test_command(&t).into_iter().filter(|a| !crate::sandbox::PROGRESS_ARGS.contains(&a.as_str())).collect();
                kept_faults.push(m);
                Some(json!({ "path": m.path, "start": m.start, "end": m.end, "original": m.original, "replacement": m.replacement, "command": command }))
            }).collect();
            let at = now_ms();
            let results = run_faults(out, op, &volume("new"), (&suite.new.0, &suite.new.1), &suite.dir, specs, until);
            if !results.is_empty() { timed(format!("faults{label}"), at); }
            let _ = std::fs::write(out.join(format!("faults{label}.json")), serde_json::to_vec(&results).unwrap_or_default());
            if let Some(a) = merged["faults"].as_array_mut() {
                a.extend(kept_faults.iter().zip(&results).map(|(m, r)| {
                    let text = |k: &str| r[k].as_str().unwrap_or("").as_bytes().to_vec();
                    let outcome = proof::fault_verdict(runner, &new, (r["finished"] == true, r["served"] == true), &text("output"), &text("errors"));
                    json!({ "path": m.path, "line": m.line, "what": m.what(), "outcome": outcome })
                }));
            }
        }
        let only = if narrowed.is_some() { format!(", only the tests related to the change: {whole_why}") } else { String::new() };
        let only = if built.is_empty() { only } else { format!("{only}, after Kern ran {}", built.join(", ")) };
        let only = match manifest(head, &suite.dir).and_then(|m| serde_json::from_slice::<Value>(&m).ok()).as_ref().and_then(guessed) {
            Some(script) => format!("{only}, run directly: the project's test script, `{script}`, is not one Kern can follow"),
            None => only,
        };
        ran.push(if suite.dir.is_empty() { format!("{}{only}", runner.name()) } else { format!("{} in {}{only}", runner.name(), suite.dir) });
    }
    wrote_code.retain(|f| !old_created.contains(f));
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

