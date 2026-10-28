use crate::policy::{Edits, Kind, Runner, Tests};
use crate::suite::Env;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

pub const RUNNERS: [&str; 3] = ["vitest", "jest", "mocha"];
pub const LEAVES_COUNT_MAX: usize = 64;
pub const LEAF_RECORD_BYTES_MAX: usize = 4 << 20;
pub const LEAF_SUITES_COUNT_MAX: usize = 8;
pub const DISCOVER_SECS_MAX: u64 = 300;
pub const DISCOVER_SECS_MIN: u64 = 60;
pub const RECORDINGS_SECS_MAX: u64 = 600;
const ARGS_COUNT_MAX: usize = 512;
const TEXT_CHARS_MAX: usize = 8192;
const DIR_CHARS_MAX: usize = 1024;
const NAME_CHARS_MAX: usize = 80;
const INFO: [&str; 8] = ["--version", "-v", "--help", "-h", "--listTests", "--showConfig", "--init", "--clearCache"];
const VITEST_INFO: [&str; 3] = ["list", "init", "bench"];
const MANAGER_PREFIXES: [&str; 10] = ["npm_", "KERN_", "TURBO_", "NX_", "PNPM_", "YARN_", "BERRY_", "BUN_", "COREPACK_", "NODE_PATH"];
const MANAGER_NAMES: [&str; 9] = ["INIT_CWD", "PROJECT_CWD", "NODE", "COLOR", "EDITOR", "OLDPWD", "SHLVL", "_", "TERM"];
pub const NO_LEAF: &str = "it started no vitest, jest or mocha run Kern could see (Kern sees a runner the script starts through its command; it does not see one started through its JavaScript API, nor node --test or bun test, nor the package tasks of turbo's strict environment mode)";

pub fn box_env_why() -> String {
    format!("its environment, with the heap size Kern sets, passes the test box's limits of {} variables of at most {} characters", crate::sandbox::BOX_ENV_COUNT_MAX, crate::sandbox::BOX_ENV_VALUE_BYTES_MAX)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record { pub runner: Runner, pub dir: Option<String>, pub argv: Vec<String>, pub node: Vec<String>, pub env: Env, pub event: String, pub cut: bool }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Recorded { pub leaves: Vec<Record>, pub exit: Option<i64>, pub why: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf { pub name: String, pub label: String, pub prefix: Option<String> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned { pub dir: String, pub old: Option<(Tests, Env)>, pub new: (Tests, Env), pub name: String }

fn strings(v: &Value, max: usize) -> Option<Vec<String>> {
    let a = v.as_array().filter(|a| a.len() <= max)?;
    a.iter().map(|x| x.as_str().filter(|s| s.len() <= TEXT_CHARS_MAX && !s.contains('\0')).map(str::to_string)).collect()
}
pub fn workspace_dir(cwd: &str) -> Option<String> {
    let rest = if cwd == "/workspace" { "" } else { cwd.strip_prefix("/workspace/")? };
    let fine = rest.len() <= DIR_CHARS_MAX && !rest.chars().any(char::is_control)
        && (rest.is_empty() || rest.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.contains('\\')));
    fine.then(|| rest.to_string())
}
fn record(v: &Value) -> Option<Record> {
    let runner = match v["runner"].as_str()? { "jest" => Runner::Jest, "vitest" => Runner::Vitest, "mocha" => Runner::Mocha, _ => return None };
    let dir = v["cwd"].as_str().filter(|c| c.len() <= TEXT_CHARS_MAX).and_then(workspace_dir);
    if v["cut"] == true { return Some(Record { runner, dir, argv: Vec::new(), node: Vec::new(), env: Env::new(), event: String::new(), cut: true }); }
    v["cwd"].as_str()?;
    let env = v["env"].as_object()?.iter().map(|(k, x)| Some((k.clone(), x.as_str().filter(|s| s.len() <= TEXT_CHARS_MAX && !s.contains('\0'))?.to_string()))).collect::<Option<Env>>()?;
    let event = v["event"].as_str().filter(|e| e.len() <= TEXT_CHARS_MAX)?.chars().filter(|c| !c.is_control() && *c != '#').take(NAME_CHARS_MAX).collect();
    Some(Record { runner, dir, argv: strings(&v["argv"], ARGS_COUNT_MAX)?, node: strings(&v["node"], ARGS_COUNT_MAX)?, env, event, cut: false })
}
pub fn read(bytes: &[u8]) -> Result<Recorded, String> {
    if bytes.len() > LEAF_RECORD_BYTES_MAX { return Err(format!("Kern's record of its test runs is larger than its limit of {} MiB", LEAF_RECORD_BYTES_MAX >> 20)); }
    let text = std::str::from_utf8(bytes).map_err(|_| "Kern could not read its record of the test runs".to_string())?;
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let Some((last, runs)) = lines.split_last() else { return Err("the project's test script stopped before Kern's recorder could say how it ended".into()) };
    let end: Value = serde_json::from_str(last).map_err(|_| "Kern could not read its record of the test runs".to_string())?;
    if end["end"] != true { return Err("the project's test script stopped before Kern's recorder could say how it ended".into()); }
    if runs.len() > LEAVES_COUNT_MAX { return Err(format!("the project's test script started more than {LEAVES_COUNT_MAX} test runs")); }
    let leaves = runs.iter().map(|l| serde_json::from_str::<Value>(l).ok().as_ref().and_then(record)).collect::<Option<Vec<_>>>()
        .ok_or_else(|| "Kern could not read its record of the test runs".to_string())?;
    let why = end["error"].as_str().map(|e| plain(e, 300)).or_else(|| end["signal"].as_str().map(|s| format!("it was stopped by {}", plain(s, 40))));
    Ok(Recorded { leaves, exit: end["exit"].as_i64(), why })
}

pub fn informational(r: &Record) -> bool {
    r.argv.iter().any(|a| INFO.contains(&a.split_once('=').map_or(a.as_str(), |(f, _)| f))) || (r.runner == Runner::Vitest && r.argv.first().is_some_and(|a| VITEST_INFO.contains(&a.as_str())))
}
fn kept(name: &str, value: &str) -> bool {
    crate::script::env_name(name) && !MANAGER_PREFIXES.iter().any(|p| name.starts_with(p)) && !MANAGER_NAMES.contains(&name)
        && !crate::sandbox::TEST_ENV.iter().chain(crate::sandbox::LEAVES_ENV.iter()).any(|(k, _)| *k == name) && name != "TZ" && value.len() <= crate::sandbox::BOX_ENV_VALUE_BYTES_MAX
}
pub fn side(r: &Record) -> Result<(Tests, Env), String> {
    if r.cut { return Err("Kern could not record this run whole: its arguments or its environment pass Kern's limits".into()); }
    if r.dir.is_none() { return Err("it ran outside the project's folder".into()); }
    let mut env: Env = r.env.iter().filter(|(k, v)| kept(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    if let Some(tz) = r.env.get("TZ").filter(|z| z.as_str() != "UTC" && z.len() <= 64) { env.insert("TZ".into(), tz.clone()); }
    if !r.node.is_empty() {
        let flags = r.node.join(" ");
        let options = env.get("NODE_OPTIONS").map_or(flags.clone(), |o| format!("{o} {flags}"));
        env.insert("NODE_OPTIONS".into(), options);
    }
    let i = crate::script::leaf(r.runner, &r.argv, env).ok_or_else(|| format!("Kern cannot run it the way it was started ({})", shown(r)))?;
    if !crate::sandbox::box_env_fits(&crate::suite::sized_env(r.runner, &i.env)) { return Err(box_env_why()); }
    Ok((Tests { paths: Vec::new(), kind: Kind::Feature, runner: r.runner, command: i.command, edits: Edits::Checked }, i.env))
}
pub fn shown(r: &Record) -> String {
    let words: Vec<&str> = std::iter::once(r.runner.name()).chain(r.argv.iter().map(String::as_str)).collect();
    plain(&words.join(" "), 160)
}

type Run = (Tests, Env);
fn assignments(env: &Env) -> impl Iterator<Item = String> + '_ { env.iter().map(|(k, v)| format!("{k}={v}")) }
fn run_shown((t, env): &Run) -> String {
    plain(&assignments(env).chain(std::iter::once(t.runner.name().to_string())).chain(t.command.iter().skip(1).cloned()).collect::<Vec<_>>().join(" "), 160)
}
pub fn owns(dir: Option<&str>, path: &str) -> bool { dir.is_none_or(|d| d.is_empty() || path.starts_with(&format!("{d}/"))) }
fn identity<'a>(dir: &'a str, (t, env): &'a Run) -> (&'a str, &'static str, &'a [String], &'a Env) { (dir, t.runner.name(), &t.command, env) }
fn first_named(a: &str, b: &str) -> bool { (a.is_empty(), a) < (b.is_empty(), b) }
fn place(dir: &str) -> String { if dir.is_empty() { String::new() } else { format!(" in {dir}") } }

struct Side { runs: Vec<(String, Run, String, bool)>, unrunnable: Vec<Value>, others: BTreeSet<String>, seen: bool, owning: bool }
fn sorted(recorded: &Recorded, changed: &[&str], chain_dir: &str, side_name: &str) -> Side {
    let mut s = Side { runs: Vec::new(), unrunnable: Vec::new(), others: BTreeSet::new(), seen: false, owning: false };
    for r in recorded.leaves.iter().filter(|r| !informational(r)) {
        let owning = changed.iter().any(|p| owns(r.dir.as_deref(), p));
        (s.seen, s.owning) = (true, s.owning || owning);
        if !owning { s.others.insert(r.dir.clone().unwrap_or_default()); }
        let dir = r.dir.clone().unwrap_or_else(|| chain_dir.to_string());
        match side(r) {
            Ok(t) => match s.runs.iter_mut().find(|(d, x, _, _)| identity(d, x) == identity(&dir, &t)) {
                Some((_, _, event, _)) => if first_named(&r.event, event) { event.clone_from(&r.event); },
                None => s.runs.push((dir, t, r.event.clone(), owning)),
            },
            Err(why) => s.unrunnable.push(json!({ "dir": dir, "why": format!("Kern could not repeat the {} run{} that {} starts on the {side_name} code: {why}", r.runner.name(), place(&dir), whose(chain_dir)) })),
        }
    }
    s.unrunnable.sort_by_key(Value::to_string);
    s.unrunnable.dedup();
    s
}
fn nearness(dir: &str, changed: &[&str]) -> usize {
    let parts = |p: &str| p.split('/').filter(|c| !c.is_empty()).map(str::to_string).collect::<Vec<_>>();
    let mine = parts(dir);
    changed.iter().map(|p| parts(p.rsplit_once('/').map_or("", |(d, _)| d)).iter().zip(&mine).take_while(|(a, b)| a == b).count()).max().unwrap_or(0)
}
fn ended(recorded: &Recorded, side_name: &str, chain_dir: &str) -> Option<String> {
    let how = match (&recorded.why, recorded.exit) {
        (Some(why), _) => why.clone(),
        (None, Some(0)) => return None,
        (None, Some(code)) => format!("it stopped with exit {code}"),
        (None, None) => "it stopped without an exit code".into(),
    };
    Some(format!("on the {side_name} code, Kern ran {} with each test runner replaced by a recorder, and {how}, so it may not have reached every test run", whose(chain_dir)))
}
fn numbered(base: Vec<String>) -> Vec<String> {
    base.iter().enumerate().map(|(i, b)| {
        let n = base[..i].iter().filter(|x| *x == b).count();
        if n == 0 && base.iter().filter(|x| *x == b).count() == 1 { b.clone() } else { format!("{b} #{}", n + 1) }
    }).collect()
}
pub fn names(runs: &[(String, &Run)]) -> Vec<String> {
    let base: Vec<String> = runs.iter().map(|(e, (t, _))| if e.is_empty() { t.runner.name().to_string() } else { e.chars().take(NAME_CHARS_MAX).collect() }).collect();
    let args = |(t, _): &Run| t.command.iter().skip(1).filter(|a| a.as_str() != "run").cloned().collect::<Vec<_>>();
    let what = |i: usize| -> String {
        let group: Vec<&Run> = (0..runs.len()).filter(|j| base[*j] == base[i]).map(|j| runs[j].1).collect();
        let mine = args(runs[i].1);
        let words: Vec<String> = if group.iter().filter(|r| args(r) == mine).count() == 1 { mine } else {
            assignments(&runs[i].1.1).filter(|a| !group.iter().all(|r| assignments(&r.1).any(|b| b == *a))).chain(mine).collect()
        };
        plain(&words.join(" "), NAME_CHARS_MAX)
    };
    let shared = |b: &String| base.iter().filter(|x| *x == b).count() > 1;
    numbered(base.iter().enumerate().map(|(i, b)| if shared(b) && !what(i).is_empty() { format!("{b} ({})", what(i)) } else { b.clone() }).collect())
}
fn whose(chain_dir: &str) -> String { if chain_dir.is_empty() { "the project's test script".into() } else { format!("the test script of {chain_dir}") } }

#[derive(Default)]
pub struct Expansion { pub leaves: Vec<Planned>, pub not_run: Vec<Value>, pub unheld: Vec<String>, pub uneven: Option<String> }
pub fn unheld_why(chain_dir: &str) -> String { format!("no test run of {} starts in its folder", whose(chain_dir)) }
pub fn recording_secs(spent_secs: u64) -> Option<u64> { Some((RECORDINGS_SECS_MAX.saturating_sub(spent_secs) / 2).min(DISCOVER_SECS_MAX)).filter(|s| *s >= DISCOVER_SECS_MIN) }
pub fn out_of_time(chain_dir: &str) -> Value {
    json!({ "dir": chain_dir, "why": format!("Kern spends at most {} minutes per check learning which test runs the project's scripts start, and that time ran out before it could learn those of {}", RECORDINGS_SECS_MAX / 60, whose(chain_dir)) })
}
pub fn expand(chain_dir: &str, old: Result<&Recorded, String>, new: Result<&Recorded, String>, changed: &[&str]) -> Expansion {
    let gave = |side_name: &str, why: String| Expansion { not_run: vec![json!({ "dir": chain_dir, "why": format!("Kern could not learn which test runs {} starts on the {side_name} code: {why}", whose(chain_dir)) })], ..Expansion::default() };
    let (old, new) = match (old, new) { (Err(why), _) => return gave("old", why), (_, Err(why)) => return gave("new", why), (Ok(o), Ok(n)) => (o, n) };
    let (o, n) = (sorted(old, changed, chain_dir, "old"), sorted(new, changed, chain_dir, "new"));
    if !o.owning && !n.owning {
        let started = n.others.iter().chain(&o.others).map(|d| if d.is_empty() { ".".to_string() } else { d.clone() }).collect::<BTreeSet<_>>();
        let why = if started.is_empty() { format!("Kern ran {} with each test runner replaced by a recorder, and {NO_LEAF}", whose(chain_dir)) }
            else { format!("{} runs its tests only in {}, and this change is outside them", whose(chain_dir), started.into_iter().take(8).collect::<Vec<_>>().join(", ")) };
        let tail = ended(new, "new", chain_dir).or_else(|| ended(old, "old", chain_dir)).map(|e| format!("; {e}")).unwrap_or_default();
        return Expansion { not_run: vec![json!({ "dir": chain_dir, "why": format!("{why}{tail}") })], ..Expansion::default() };
    }
    let mut not_run: Vec<Value> = o.unrunnable.into_iter().chain(n.unrunnable).collect();
    if !o.seen {
        not_run.push(json!({ "dir": chain_dir, "why": format!("on the old code, Kern ran {} with each test runner replaced by a recorder, and {NO_LEAF}, so none of the old code's tests can be compared with the new code", whose(chain_dir)) }));
    }
    not_run.extend([ended(old, "old", chain_dir), ended(new, "new", chain_dir)].into_iter().flatten().map(|why| json!({ "dir": chain_dir, "why": why })));
    let mut pairs: Vec<(Planned, String, bool)> = n.runs.iter().map(|(d, t, e, owning)| {
        let old = o.runs.iter().find(|(od, ot, _, _)| identity(od, ot) == identity(d, t)).map(|(_, ot, _, _)| ot.clone());
        (Planned { dir: d.clone(), old, new: t.clone(), name: String::new() }, e.clone(), *owning)
    }).collect();
    for (d, t, _, _) in o.runs.iter().filter(|(od, ot, _, _)| !n.runs.iter().any(|(d, t, _, _)| identity(d, t) == identity(od, ot))) {
        not_run.push(json!({ "dir": d, "why": format!("{} starts `{}`{} on the old code but not on the new code", whose(chain_dir), run_shown(t), place(d)) }));
    }
    let order = |(p, _, owning): &(Planned, String, bool)| (!owning, std::cmp::Reverse(nearness(&p.dir, changed)));
    pairs.sort_by(|a, b| order(a).cmp(&order(b)).then_with(|| identity(&a.0.dir, &a.0.new).cmp(&identity(&b.0.dir, &b.0.new))));
    for (p, _, _) in pairs.iter().skip(LEAF_SUITES_COUNT_MAX) {
        not_run.push(json!({ "dir": p.dir, "why": format!("Kern runs at most {LEAF_SUITES_COUNT_MAX} of the test runs one script starts, and this one (`{}`) was over that", run_shown(&p.new)) }));
    }
    pairs.truncate(LEAF_SUITES_COUNT_MAX);
    let named = names(&pairs.iter().map(|(p, e, _)| (e.clone(), &p.new)).collect::<Vec<_>>());
    let leaves = pairs.into_iter().zip(named).map(|((p, _, _), name)| Planned { name, ..p }).collect();
    let held = |p: &str| [old, new].iter().flat_map(|r| &r.leaves).any(|r| !informational(r) && owns(r.dir.as_deref(), p));
    let unheld = changed.iter().filter(|p| owns(Some(chain_dir), p) && !held(p)).map(|p| p.to_string()).collect();
    Expansion { leaves, not_run, unheld, uneven: None }
}
pub fn uneven_recording(chain_dir: &str, script: &str, old: Option<&Result<Recorded, String>>, new: &Result<Recorded, String>) -> Option<String> {
    let ran = format!("Kern ran `npm run {}`{}", plain(script, 80), place(chain_dir));
    let tail = "so the two trees may hold different build output";
    let ending = |r: &Result<Recorded, String>| r.as_ref().ok().map(|r| (r.exit, r.why.clone()));
    match old.map(|o| (ending(o), ending(new))) {
        None => Some(format!("{ran} on the new code only to learn its test runs, {tail}")),
        Some((Some(a), Some(b))) if a == b => None,
        Some((Some(_), Some(_))) => Some(format!("{ran} on both trees to learn its test runs, and it ended differently on the two sides, {tail}")),
        Some(_) => Some(format!("{ran} on both trees to learn its test runs, and it did not finish on at least one side, {tail}")),
    }
}

pub fn discover(out: &Path, volume: &str, (dir, command): (&str, &[String]), label: &str, (until, limit): (u64, u64)) -> Result<Recorded, String> {
    let seconds = within(until, limit.min(DISCOVER_SECS_MAX));
    if seconds < DISCOVER_SECS_MIN { return Err(crate::suite::LATE.to_string()); }
    let env: Env = crate::sandbox::LEAVES_ENV.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let argv: Vec<String> = ["/usr/local/bin/node", "/kern/leaf.cjs"].iter().map(|s| s.to_string()).chain(command.iter().cloned()).collect();
    let config = json!({ "version": 1, "command": argv, "timeout_seconds": seconds, "cwd": dir, "env": env, "files": {}, "early": false, "faults": [], "serve": {} });
    let run = crate::suite::in_test_box(out, volume, &format!("leaves-{label}"), ("drive-test.json", &config), ("__drive_test", false, seconds)).map_err(|e| e.0)?;
    let parts = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS).map_err(|e| e.0)?;
    let _ = std::fs::write(out.join(format!("leaves-{label}.out")), &parts["output"]);
    let _ = std::fs::write(out.join(format!("leaves-{label}.err")), &parts["errors"]);
    let ending = String::from_utf8_lossy(&parts["ending"]).to_string();
    if ending == "timeout" { return Err(format!("it did not reach the end in {} minutes", seconds / 60)); }
    if let Some(why) = crate::suite::capped(&ending) { return Err(why); }
    read(&parts["output"])
}

pub fn named(prefix: Option<&str>, runner: Runner, dir: &str, key: &str) -> String {
    match prefix { Some(p) => format!("{p}: {key}"), None => crate::suite::in_suite(runner, dir, key) }
}
pub fn label(dir: &str, taken: &BTreeSet<String>) -> String {
    let base = crate::suite::label(dir);
    (1..=taken.len() + 1).map(|n| format!("{base}-leaf{n}")).find(|l| !taken.contains(l)).unwrap_or(base)
}
pub fn assigned(paths: &[String], leaves: &[(usize, &str, &crate::proof::Report)]) -> std::collections::BTreeMap<usize, Vec<String>> {
    let shared = |a: &str, b: &str| a.split('/').zip(b.split('/')).take_while(|(x, y)| x == y).count();
    let mut out: std::collections::BTreeMap<usize, Vec<String>> = std::collections::BTreeMap::new();
    for p in paths {
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        let near = |r: &crate::proof::Report| r.tests.keys().map(|k| { let f = k.split(" > ").next().unwrap_or(k); shared(dir, f.rsplit_once('/').map_or("", |(d, _)| d)) }).max().unwrap_or(0);
        let owners = leaves.iter().filter(|(_, d, _)| owns(Some(d), p)).map(|(i, _, r)| (*i, near(r)));
        let Some((best, _)) = owners.fold(None::<(usize, usize)>, |b, (i, n)| match b { Some((_, m)) if m >= n => b, _ => Some((i, n)) }) else { continue };
        out.entry(best).or_default().push(p.clone());
    }
    out
}
