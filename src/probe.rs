use crate::calls::Import;
use crate::policy::{Runner, Tests};
use crate::suite::{Env, Suite, Texts};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const INVESTIGATION_WAIT_SECS_MAX: u64 = 600;
const INVESTIGATION_POLL_SECS: u64 = 2;
pub const PROBE_SECS_MAX: u64 = 480;
pub const PROBE_SECS_MIN: u64 = 240;
const PROBE_RUN_SECS_MAX: u64 = 240;
const PROBE_RUN_SECS_MIN: u64 = 60;
pub const TARGETS_COUNT_MAX: usize = 12;
pub const OBLIGATIONS_COUNT_MAX: usize = 24;
pub const EXPERIMENTS_PER_TARGET_COUNT_MAX: usize = 8;
pub const EXPERIMENTS_COUNT_MAX: usize = 64;
pub const ARGS_COUNT_MAX: usize = 8;
pub const ARGS_BYTES_MAX: usize = 2048;
pub const EXPERIMENTS_BYTES_MAX: usize = 65536;
pub const SHAPES_PER_TARGET_COUNT_MAX: usize = 16;
pub const CALLS_PER_TARGET_COUNT_MAX: usize = 16;
pub const STATEMENT_CHARS_MAX: usize = 300;
pub const WHY_CHARS_MAX: usize = 200;
pub const QUOTE_CHARS_MAX: usize = 600;
const ID_CHARS_MAX: usize = 64;
const NAME_CHARS_MAX: usize = 128;
const ANSWER_BYTES_MAX: usize = 2048;
const VALUE_BYTES_MAX: usize = 65_536;
const TEST_TIMEOUT_MS: u64 = 10_000;
const PROBE_TESTS: &str = "kern probe";
const PROBE_TAG: &str = "kern-probe";
const HOSTS_COUNT_MAX: usize = 200;
const HOST_BYTES_MAX: u64 = 1 << 20;
const SOURCE_BYTES_MAX: usize = 4 << 20;
const REPORT_BYTES_MAX: u64 = 64 << 20;
const TRACE_BYTES_MAX: u64 = 16 << 20;
const TRACE_LINES_COUNT_MAX: usize = 200_000;
const ANSWERS_PER_INPUT_COUNT_MAX: usize = 2;
const NAMES_COUNT_MAX: u32 = 10;
const RUNTIME: &str = "/kern/trace.cjs";
const SIDES: [&str; 4] = ["old", "old-again", "new", "new-again"];
const SHAPE_TAGS: [&str; 6] = ["string", "number", "boolean", "null", "array", "object"];
const SOURCE_KINDS: [&str; 4] = ["test", "doc", "types", "comment"];
const VERBS: [&str; 4] = ["returns ", "resolves ", "throws ", "rejects "];
pub const HUNG: &str = "hangs: gave no answer before the run ended";
const OFF: &str = "the AI step is off";
const UNTESTED: &str = "Kern tries the AI step's inputs through the project's own tests, and they did not run here";
const LEAVES: &str = "Kern does not yet try the AI step's inputs through the test runs it learned from the project's test script";
const LATE: &str = "Kern's time ran out before it could try the AI step's inputs";
const NONE_TRIED: &str = "Kern could not try any input the AI step chose";
const NO_FOLDER: &str = "Kern ran no tests in its folder";
const NO_OLD: &str = "the old code has no test runner Kern can read";
const RUNNERS: &str = "Kern tries inputs only through jest and vitest";
const UNWATCHED: &str = "Kern could not rewrite the file to watch its answers";
const UNREAD: &str = "Kern could not read both versions of the file";
const NO_HOST: &str = "no test file Kern ran on the new code imports it";
const NO_NAME: &str = "Kern found no free name for its test file next to the test it copies";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check { Same, Returns, Throws }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Authority { Verified, Contested, Authorized, Unverified }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Requirement { pub check: Check, pub authority: Authority }
fn check_named(s: &str) -> Option<Check> {
    match s { "same" => Some(Check::Same), "returns" => Some(Check::Returns), "throws" => Some(Check::Throws), _ => None }
}
fn authority_named(s: &str) -> Option<Authority> {
    match s { "verified" => Some(Authority::Verified), "contested" => Some(Authority::Contested), "authorized" => Some(Authority::Authorized), "unverified" => Some(Authority::Unverified), _ => None }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target { pub path: String, pub export: String, pub shapes: Vec<Vec<String>>, pub calls: Vec<Vec<Value>>, pub retested: bool }
#[derive(Clone, Debug, PartialEq)]
pub struct Experiment { pub id: String, pub path: String, pub export: String, pub args: Vec<Value>, pub why: String, pub obligation: Value }
#[derive(Clone, Debug, PartialEq)]
pub struct Investigation { pub targets: Vec<Target>, pub experiments: Vec<Experiment> }
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed { Investigated(Investigation), Nothing(String), Unusable(String) }

type Parse<T> = Result<T, String>;
type Place = (String, String);
fn field<'a>(v: &'a Value, key: &str, chars_max: usize, what: &str) -> Parse<&'a str> {
    match v[key].as_str() {
        Some(s) if s.chars().count() <= chars_max => Ok(s),
        Some(_) => Err(format!("{what} has a {key} longer than {chars_max} characters")),
        None => Err(format!("{what} has no {key}")),
    }
}
fn listed<'a>(v: &'a Value, key: &str, count_max: usize, what: &str) -> Parse<&'a Vec<Value>> {
    let list = v[key].as_array().ok_or_else(|| format!("{what} has no {key} list"))?;
    if list.len() > count_max { return Err(format!("{what} has {} {key} (at most {count_max})", list.len())); }
    Ok(list)
}
fn ident(v: &Value, what: &str) -> Parse<String> {
    let id = field(v, "id", ID_CHARS_MAX, what)?;
    if id.is_empty() || id.chars().any(char::is_control) { return Err(format!("{what} has no usable id")); }
    Ok(id.to_string())
}
fn function_name(s: &str) -> bool {
    s == "default" || (s.len() <= NAME_CHARS_MAX && s.starts_with(|c: char| c.is_alphabetic() || c == '_' || c == '$') && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$'))
}
fn place(v: &Value, what: &str) -> Parse<Place> {
    let path = v["path"].as_str().filter(|p| file_path(p).is_ok() && crate::capability::is_code(p)).ok_or_else(|| format!("{what} names no code file Kern can read"))?;
    let export = v["export"].as_str().filter(|e| function_name(e)).ok_or_else(|| format!("{what} names no function"))?;
    Ok((path.to_string(), export.to_string()))
}
fn target(v: &Value, what: &str) -> Parse<Target> {
    let (path, export) = place(v, what)?;
    let shape = |s: &Value| s.as_array().filter(|tags| tags.len() <= ARGS_COUNT_MAX)?.iter().map(|t| t.as_str().filter(|t| SHAPE_TAGS.contains(t)).map(str::to_string)).collect::<Option<Vec<_>>>();
    let shapes = listed(v, "shapes", SHAPES_PER_TARGET_COUNT_MAX, what)?.iter().map(shape).collect::<Option<Vec<_>>>().ok_or_else(|| format!("{what} has an argument shape Kern does not know"))?;
    let calls = listed(v, "calls", CALLS_PER_TARGET_COUNT_MAX, what)?.iter().map(|c| arguments(c, what)).collect::<Parse<Vec<_>>>()?;
    let retested = match &v["retested"] { Value::Null | Value::Bool(false) => false, Value::Bool(true) => true, _ => return Err(format!("{what} says in a way Kern does not read whether the change tests it")) };
    Ok(Target { path, export, shapes, calls, retested })
}
fn arguments(v: &Value, what: &str) -> Parse<Vec<Value>> {
    let args = v.as_array().filter(|a| a.len() <= ARGS_COUNT_MAX).ok_or_else(|| format!("{what} has a call with more than {ARGS_COUNT_MAX} arguments or none Kern reads"))?;
    if serde_json::to_string(args).map_or(usize::MAX, |s| s.len()) > ARGS_BYTES_MAX { return Err(format!("{what} has a call with arguments longer than {ARGS_BYTES_MAX} bytes")); }
    Ok(args.clone())
}
fn obligation(v: &Value, at: usize) -> Parse<(String, BTreeSet<Place>)> {
    let id = ident(v, &format!("obligation {}", at + 1))?;
    let what = format!("obligation {id}");
    let covers = listed(v, "targets", TARGETS_COUNT_MAX, &what)?.iter().map(|t| place(t, &what)).collect::<Parse<BTreeSet<_>>>()?;
    if covers.is_empty() { return Err(format!("{what} names no function")); }
    let authority = v["authority"].as_str().and_then(authority_named).ok_or_else(|| format!("{what} has no authority Kern knows"))?;
    if authority == Authority::Unverified { return Ok((id, covers)); }
    if field(v, "statement", STATEMENT_CHARS_MAX, &what)?.trim().is_empty() { return Err(format!("{what} states nothing")); }
    field(v, "why", WHY_CHARS_MAX, &what)?;
    v["check"].as_str().and_then(check_named).ok_or_else(|| format!("{what} has no check Kern knows"))?;
    if field(v, "authorized_by", QUOTE_CHARS_MAX, &what)?.trim().is_empty() && authority == Authority::Authorized {
        return Err(format!("{what} says the pull request changes it without quoting it"));
    }
    let source = &v["source"];
    source["path"].as_str().filter(|p| file_path(p).is_ok()).ok_or_else(|| format!("{what} cites no file"))?;
    if field(source, "quote", QUOTE_CHARS_MAX, &what)?.trim().is_empty() { return Err(format!("{what} quotes nothing")); }
    source["kind"].as_str().filter(|k| SOURCE_KINDS.contains(k)).ok_or_else(|| format!("{what} cites a kind of source Kern does not know"))?;
    Ok((id, covers))
}
fn experiment(v: &Value, at: usize) -> Parse<(Experiment, Option<String>)> {
    let id = ident(v, &format!("experiment {}", at + 1))?;
    let what = format!("experiment {id}");
    let (path, export) = place(v, &what)?;
    let args = listed(v, "args", ARGS_COUNT_MAX, &what)?;
    if serde_json::to_string(args).map_or(usize::MAX, |s| s.len()) > ARGS_BYTES_MAX { return Err(format!("{what} has arguments longer than {ARGS_BYTES_MAX} bytes")); }
    let why = field(v, "why", WHY_CHARS_MAX, &what)?.to_string();
    let wanted = match &v["obligation"] { Value::Null => None, Value::String(o) => Some(o.clone()), _ => return Err(format!("{what} names its obligation in a way Kern does not read")) };
    Ok((Experiment { id, path, export, args: args.clone(), why, obligation: Value::Null }, wanted))
}
fn investigation(v: &Value) -> Parse<Investigation> {
    let targets = listed(v, "targets", TARGETS_COUNT_MAX, "it")?.iter().enumerate().map(|(i, t)| target(t, &format!("target {}", i + 1))).collect::<Parse<Vec<_>>>()?;
    let known: BTreeSet<Place> = targets.iter().map(|t| (t.path.clone(), t.export.clone())).collect();
    if known.len() < targets.len() { return Err("it names a target twice".into()); }
    let mut obligations: BTreeMap<String, (BTreeSet<Place>, &Value)> = BTreeMap::new();
    for (i, o) in listed(v, "obligations", OBLIGATIONS_COUNT_MAX, "it")?.iter().enumerate() {
        let (id, covers) = obligation(o, i)?;
        if obligations.insert(id.clone(), (covers, o)).is_some() { return Err(format!("it names obligation {id} twice")); }
    }
    let raw = listed(v, "experiments", EXPERIMENTS_COUNT_MAX, "it")?;
    if serde_json::to_string(raw).map_or(usize::MAX, |s| s.len()) > EXPERIMENTS_BYTES_MAX { return Err(format!("its experiments are longer than {EXPERIMENTS_BYTES_MAX} bytes")); }
    let (mut experiments, mut per_target): (Vec<Experiment>, BTreeMap<Place, usize>) = (Vec::new(), BTreeMap::new());
    for (i, x) in raw.iter().enumerate() {
        let (mut e, wanted) = experiment(x, i)?;
        let key = (e.path.clone(), e.export.clone());
        if !known.contains(&key) { return Err(format!("experiment {} tries a function no target names", e.id)); }
        if experiments.iter().any(|o| o.id == e.id) { return Err(format!("it names experiment {} twice", e.id)); }
        let tried = per_target.entry(key.clone()).or_default();
        *tried += 1;
        if *tried > EXPERIMENTS_PER_TARGET_COUNT_MAX { return Err(format!("it has more than {EXPERIMENTS_PER_TARGET_COUNT_MAX} experiments for {}", key.0)); }
        if let Some(o) = wanted {
            let (covers, object) = obligations.get(&o).ok_or_else(|| format!("experiment {} names obligation {o}, which it does not list", e.id))?;
            if !covers.contains(&key) { return Err(format!("experiment {} names obligation {o}, which is not about {}", e.id, key.1)); }
            e.obligation = (*object).clone();
        }
        experiments.push(e);
    }
    Ok(Investigation { targets, experiments })
}
pub fn parse(v: &Value, (base, head): (&str, &str)) -> Parsed {
    if v["version"] != 1 { return Parsed::Unusable("the AI step's findings are not in a version Kern reads".into()); }
    let why: String = v["why"].as_str().unwrap_or("").chars().take(WHY_CHARS_MAX).collect();
    if v["status"] == "FAILED" { return Parsed::Unusable(if why.is_empty() { "the AI step failed".into() } else { format!("the AI step failed: {why}") }); }
    if v["base"] != base || v["head"] != head { return Parsed::Unusable("the AI step's findings are for other commits than the ones Kern checked".into()); }
    match v["status"].as_str() {
        Some("NONE") => Parsed::Nothing(if why.is_empty() { "the AI step found nothing to investigate".into() } else { why }),
        Some("INVESTIGATED") => match investigation(v) {
            Ok(i) if i.experiments.is_empty() => Parsed::Nothing("the AI step proposed no input to try".into()),
            Ok(i) => Parsed::Investigated(i),
            Err(why) => Parsed::Unusable(format!("the AI step's findings are malformed: {why}")),
        },
        _ => Parsed::Unusable("the AI step's findings have a status Kern does not know".into()),
    }
}

pub struct Candidate { pub path: String, pub source: String, pub both: bool, pub unchanged: bool }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style { pub esm: bool, pub typescript: bool, pub jest_globals: bool }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Host { pub path: String, pub spec: String, pub style: Style, pub text: Option<String> }
fn style(path: &str, source: &str, imports: &[Import]) -> Style {
    let extension = path.rsplit_once('.').map_or("", |(_, e)| e);
    Style { esm: crate::proof::parsed(path, source, |program, _| program.body.iter().any(|s| s.is_module_declaration())).unwrap_or(false),
        typescript: ["ts", "tsx", "mts", "cts"].contains(&extension), jest_globals: imports.iter().any(|i| i.spec == "@jest/globals") }
}
pub fn hosts(targets: &[&str], candidates: &[Candidate], exists: &dyn Fn(&str) -> bool) -> BTreeMap<String, Host> {
    let mut ranked: Vec<&Candidate> = candidates.iter().collect();
    ranked.sort_by(|a, b| (!a.unchanged, !a.both, &a.path).cmp(&(!b.unchanged, !b.both, &b.path)));
    let read: Vec<(&Candidate, Vec<Import>)> = ranked.into_iter().map(|c| (c, crate::calls::imports(&c.path, &c.source))).collect();
    targets.iter().filter_map(|t| read.iter().find_map(|(c, imports)| {
        let spec = &imports.iter().find(|i| crate::calls::points_to(&c.path, &i.spec, t, exists))?.spec;
        Some((t.to_string(), Host { path: c.path.clone(), spec: spec.clone(), style: style(&c.path, &c.source, imports), text: c.unchanged.then(|| c.source.clone()) }))
    })).collect()
}
fn literal(s: &str) -> String { serde_json::to_string(s).unwrap_or_default().replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029") }
fn probe_tests(style: Style, runner: Runner, (specs, tries): (&[String], &[(usize, &Experiment)]), p: &str) -> String {
    let load = |name: String, spec: &str| if style.esm { format!("import * as {name} from {};\n", literal(spec)) } else { format!("const {name} = require({});\n", literal(spec)) };
    let api = if runner == Runner::Vitest { Some("vitest") } else { style.jest_globals.then_some("@jest/globals") };
    let test_api = match (api, p.is_empty(), style.esm) {
        (Some(a), true, true) => format!("import {{ test }} from {};\n", literal(a)),
        (Some(a), true, false) => format!("const {{ test }} = require({});\n", literal(a)),
        (Some(a), false, true) => format!("import {{ test as {p}test }} from {};\n", literal(a)),
        (Some(a), false, false) => format!("const {{ test: {p}test }} = require({});\n", literal(a)),
        (None, true, _) => String::new(),
        (None, false, _) => format!("const {p}test = test;\n"),
    };
    let mut out = if p.is_empty() { test_api.clone() } else { String::new() };
    out += &load(format!("{p}m"), RUNTIME);
    for (i, spec) in specs.iter().enumerate() { out += &load(format!("{p}target{i}"), spec); }
    if !p.is_empty() { out += &test_api; }
    out += &format!("const {p}t = {p}m.default ?? {p}m;\n");
    for (i, e) in tries {
        let args = literal(&serde_json::to_string(&e.args).unwrap_or_default());
        out += &format!("{p}test({}, async () => {{ await {p}t.probe({}, {p}target{i}, {}, JSON.parse({args})); }}, {TEST_TIMEOUT_MS});\n", literal(&format!("{PROBE_TESTS} {}", e.id)), literal(&e.id), literal(&e.export));
    }
    out
}
fn checked_off(style: Style) -> &'static str { if style.typescript { "// @ts-nocheck\n" } else { "" } }
pub fn probe_source(style: Style, runner: Runner, specs: &[String], tries: &[(usize, &Experiment)]) -> String {
    format!("{}{}", checked_off(style), probe_tests(style, runner, (specs, tries), ""))
}
fn docblock_end(host: &str) -> usize {
    let Some(rest) = host.trim_start().strip_prefix("/*") else { return 0 };
    let lead = host.len() - host.trim_start().len();
    match rest.find("*/") {
        Some(i) => { let end = lead + 2 + i + 2; host[end..].find('\n').map_or(host.len(), |n| end + n + 1) }
        None => 0,
    }
}
pub fn hosted_source(host: &str, style: Style, runner: Runner, specs: &[String], tries: &[(usize, &Experiment)]) -> String {
    let at = docblock_end(host);
    format!("{}{}{}{}{}", &host[..at], checked_off(style), &host[at..], if host.ends_with('\n') { "" } else { "\n" }, probe_tests(style, runner, (specs, tries), "__kern_"))
}
pub fn probe_name(host: &str, n: u32) -> Option<String> {
    let (dir, name) = host.rsplit_once('/').unwrap_or(("", host));
    if name.is_empty() { return None; }
    let tag = if n == 0 { PROBE_TAG.to_string() } else { format!("{PROBE_TAG}{n}") };
    Some(if dir.is_empty() { format!("{tag}.{name}") } else { format!("{dir}/{tag}.{name}") })
}
pub fn focus(tests: &Tests, names: &BTreeSet<String>, dir: &str) -> Option<Tests> {
    let mut t = crate::suite::kept_focus(tests, names, dir)?;
    t.command.extend(["-t".to_string(), PROBE_TESTS.to_string()]);
    Some(t)
}

struct Change<'a> { repo: &'a Path, base: &'a str, head: &'a str, base_oids: BTreeMap<String, String>, head_oids: BTreeMap<String, String>, head_items: Vec<crate::drive::Item> }
impl Change<'_> {
    fn show(&self, commit: &str, path: &str) -> Option<String> { String::from_utf8(crate::git::git_with(self.repo, &["show", &format!("{commit}:{path}")], None, SOURCE_BYTES_MAX).ok()?).ok() }
    fn free(&self, host: &str, taken: &mut BTreeSet<String>) -> Option<String> {
        let name = (0..NAMES_COUNT_MAX).filter_map(|n| probe_name(host, n)).find(|n| !self.base_oids.contains_key(n) && !self.head_oids.contains_key(n) && !taken.contains(n))?;
        taken.insert(name.clone());
        Some(name)
    }
    fn same_both_sides(&self, path: &str) -> bool { self.base_oids.get(path).is_some_and(|oid| self.head_oids.get(path) == Some(oid)) }
    fn leans_on_the_change(&self, path: &str, source: &str, targets: &BTreeSet<&str>) -> bool {
        let exists = |p: &str| self.head_oids.contains_key(p);
        crate::calls::imports(path, source).iter().filter(|i| i.spec.starts_with('.')).filter_map(|i| crate::proof::resolve(path, &i.spec, &exists))
            .any(|p| !targets.contains(p.as_str()) && !self.same_both_sides(&p))
    }
}
pub struct Plan { pub dir: String, pub sides: [(Tests, Env); 2], pub files: [Texts; 2], pub serve: [Texts; 2], pub records: Vec<Value>, pub uneven: Option<String> }
fn skipped(path: &str, why: &str) -> Value { json!({ "path": path, "why": why }) }
fn needle(path: &str) -> String {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    let stem = name.split('.').next().unwrap_or(name);
    if stem == "index" { dir.rsplit('/').next().unwrap_or("").to_string() } else { stem.to_string() }
}
fn ran_files(out: &Path, label: &str, runner: Runner) -> BTreeSet<String> {
    let report = ["new-related", "new"].iter().find_map(|r| {
        let path = out.join(format!("tests-{r}{label}.out"));
        std::fs::metadata(&path).ok().filter(|m| m.len() <= REPORT_BYTES_MAX)?;
        crate::proof::read_report(runner, &std::fs::read(&path).ok()?).ok()
    });
    report.map(|r| r.tests.iter().filter(|(_, c)| c.ran() > 0).map(|(k, _)| k.split(" > ").next().unwrap_or(k).to_string()).collect()).unwrap_or_default()
}
fn candidates(c: &Change, ran: &BTreeSet<String>, targets: &BTreeSet<&str>) -> Vec<Candidate> {
    let needles: BTreeSet<String> = targets.iter().map(|p| needle(p)).collect();
    let words: Vec<&str> = needles.iter().map(String::as_str).filter(|n| !n.is_empty()).collect();
    if words.is_empty() { return Vec::new(); }
    let mentioned: BTreeSet<String> = crate::git::mentions(c.repo, c.head, &words).unwrap_or_default().into_iter().collect();
    let items: Vec<&crate::drive::Item> = c.head_items.iter().filter(|i| ran.contains(&i.path) && mentioned.contains(&i.path) && i.mode.starts_with("100") && i.size <= HOST_BYTES_MAX && crate::capability::is_code(&i.path))
        .take(HOSTS_COUNT_MAX).collect();
    let mut found = Vec::new();
    let _ = crate::drive::blobs(c.repo, &items, |item, bytes| {
        if let Ok(source) = std::str::from_utf8(bytes) {
            let unchanged = c.same_both_sides(&item.path) && !c.leans_on_the_change(&item.path, source, targets);
            found.push(Candidate { path: item.path.clone(), source: source.to_string(), both: c.base_oids.contains_key(&item.path), unchanged });
        }
        Ok(())
    });
    found
}
fn ready<'a>(c: &Change, targets: &[&'a Target], not_run: &mut Vec<Value>) -> (Vec<&'a Target>, [Texts; 2]) {
    let mut files: Vec<(String, String, String)> = Vec::new();
    let mut kept = Vec::new();
    for t in targets {
        let sources = files.iter().find(|f| f.0 == t.path).map(|f| (f.1.clone(), f.2.clone())).or_else(|| Some((c.show(c.base, &t.path)?, c.show(c.head, &t.path)?)));
        let callable = |text: &str| crate::trace::callable(&t.path, text).is_some_and(|names| names.contains(&t.export));
        match sources {
            Some((old, new)) if callable(&old) && callable(&new) => {
                if !files.iter().any(|f| f.0 == t.path) { files.push((t.path.clone(), old, new)); }
                kept.push(*t);
            }
            Some(_) => not_run.push(skipped(&t.path, &format!("{} is not a function both versions of the file export", t.export))),
            None => not_run.push(skipped(&t.path, UNREAD)),
        }
    }
    let serve = [crate::suite::watched(&files, true), crate::suite::watched(&files, false)];
    let (watched, lost): (Vec<&Target>, Vec<&Target>) = kept.into_iter().partition(|t| serve.iter().all(|s| s.contains_key(&t.path)));
    not_run.extend(lost.iter().map(|t| skipped(&t.path, UNWATCHED)));
    (watched, serve)
}
fn record(e: &Experiment, t: &Target, host: &str, dir: &str) -> Value {
    json!({ "id": e.id, "path": e.path, "export": e.export, "args": e.args, "why": e.why, "obligation": e.obligation, "shapes": t.shapes, "calls": t.calls, "retested": t.retested, "host": host, "dir": dir,
        "reached": { "old": false, "new": false }, "answers": { "old": null, "old_again": null, "new": null, "new_again": null }, "data": "missing", "chance": false, "network": false, "uneven": null })
}
fn hostable(runner: Runner) -> bool { matches!(runner, Runner::Jest | Runner::Vitest) }
fn suite_plan(out: &Path, c: &Change, suite: &Suite, targets: &[&Target], inv: &Investigation, taken: &mut BTreeSet<String>) -> (Option<Plan>, Vec<Value>) {
    let all = |why: &str| (None, targets.iter().map(|t| skipped(&t.path, why)).collect());
    let Some(old) = suite.old.clone() else { return all(NO_OLD) };
    if !hostable(old.0.runner) || !hostable(suite.new.0.runner) { return all(RUNNERS); }
    if old.0.runner != suite.new.0.runner {
        return all(&format!("the change moves the tests from {} to {}, so Kern cannot run its inputs through one runner on both sides", old.0.runner.name(), suite.new.0.runner.name()));
    }
    let mut not_run = Vec::new();
    let (watched, serve) = ready(c, targets, &mut not_run);
    let paths: BTreeSet<&str> = watched.iter().map(|t| t.path.as_str()).collect();
    let ran = ran_files(out, &crate::suite::label(&suite.dir), suite.new.0.runner);
    let found = hosts(&paths.iter().copied().collect::<Vec<_>>(), &candidates(c, &ran, &paths), &|p| c.head_oids.contains_key(p));
    not_run.extend(paths.iter().filter(|p| !found.contains_key(**p)).map(|p| skipped(p, NO_HOST)));
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (path, host) in &found { groups.entry(host.path.as_str()).or_default().push(path.as_str()); }
    let (mut files, mut records): ([Texts; 2], Vec<Value>) = Default::default();
    for (host, served) in groups {
        let Some(name) = c.free(host, taken) else { not_run.extend(served.iter().map(|p| skipped(p, NO_NAME))); continue };
        let specs: Vec<String> = served.iter().map(|p| found[*p].spec.clone()).collect();
        let tries: Vec<(usize, &Experiment, &Target)> = inv.experiments.iter().filter_map(|e| Some((served.iter().position(|p| *p == e.path)?, e, *watched.iter().find(|t| t.path == e.path && t.export == e.export)?))).collect();
        let placed: Vec<(usize, &Experiment)> = tries.iter().map(|(i, e, _)| (*i, *e)).collect();
        let chosen = &found[served[0]];
        for (side, runner) in [old.0.runner, suite.new.0.runner].into_iter().enumerate() {
            let source = match &chosen.text { Some(text) => hosted_source(text, chosen.style, runner, &specs, &placed), None => probe_source(chosen.style, runner, &specs, &placed) };
            files[side].insert(name.clone(), json!(source));
        }
        records.extend(tries.iter().map(|(_, e, t)| record(e, t, host, &suite.dir)));
    }
    if records.is_empty() { return (None, not_run); }
    let names: BTreeSet<String> = files[1].keys().cloned().collect();
    let (Some(old_tests), Some(new_tests)) = (focus(&old.0, &names, &suite.dir), focus(&suite.new.0, &names, &suite.dir)) else {
        let paths: BTreeSet<&str> = records.iter().filter_map(|x| x["path"].as_str()).collect();
        not_run.extend(paths.into_iter().map(|p| skipped(p, crate::suite::SINGLE_FILES)));
        return (None, not_run);
    };
    (Some(Plan { dir: suite.dir.clone(), sides: [(old_tests, old.1), (new_tests, suite.new.1.clone())], files, serve, records, uneven: suite.uneven.clone() }), not_run)
}
fn suite_of(path: &str, suites: &[Suite]) -> Option<usize> {
    suites.iter().enumerate().filter(|(_, s)| s.dir.is_empty() || path.starts_with(&format!("{}/", s.dir))).max_by_key(|(_, s)| s.dir.len()).map(|(i, _)| i)
}
pub fn plan(out: &Path, repo: &Path, (base, head): (&str, &str), inv: &Investigation, suites: &[Suite]) -> (Vec<Plan>, Vec<Value>) {
    let head_items = crate::drive::listing(repo, head).unwrap_or_default();
    let base_oids = crate::drive::listing(repo, base).unwrap_or_default().into_iter().map(|i| (i.path, i.oid)).collect();
    let c = Change { repo, base, head, base_oids, head_oids: head_items.iter().map(|i| (i.path.clone(), i.oid.clone())).collect(), head_items };
    let tried: Vec<&Target> = inv.targets.iter().filter(|t| inv.experiments.iter().any(|e| e.path == t.path && e.export == t.export)).collect();
    let (mut plans, mut not_run, mut taken) = (Vec::new(), Vec::new(), BTreeSet::new());
    not_run.extend(tried.iter().filter(|t| suite_of(&t.path, suites).is_none()).map(|t| skipped(&t.path, NO_FOLDER)));
    for (i, suite) in suites.iter().enumerate() {
        let mine: Vec<&Target> = tried.iter().copied().filter(|t| suite_of(&t.path, suites) == Some(i)).collect();
        if mine.is_empty() { continue; }
        let (p, lost) = suite_plan(out, &c, suite, &mine, inv, &mut taken);
        plans.extend(p);
        not_run.extend(lost);
    }
    (plans, not_run)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen { pub answers: BTreeMap<String, Vec<String>>, pub values: BTreeMap<String, Value>, pub reached: BTreeMap<String, String>, pub started: BTreeSet<String>, pub network: bool }
fn outcome(o: &str) -> bool { o.len() <= ANSWER_BYTES_MAX && (o == HUNG || VERBS.iter().any(|v| o.starts_with(v))) }
pub fn seen(trace: &str) -> Seen {
    let mut s = Seen::default();
    for line in trace.lines().take(TRACE_LINES_COUNT_MAX) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(id) = v["p"].as_str() else { continue };
        if v["s"] == 1 {
            s.started.insert(id.to_string());
            if let Some(k) = v["k"].as_str() { s.reached.entry(id.to_string()).or_insert_with(|| k.to_string()); }
            continue;
        }
        let Some(o) = v["o"].as_str().filter(|o| outcome(o) && *o != HUNG) else { continue };
        let answers = s.answers.entry(id.to_string()).or_default();
        if answers.is_empty() {
            if let Some(value) = v["v"].as_str().filter(|t| t.len() <= VALUE_BYTES_MAX).and_then(|t| serde_json::from_str::<Value>(t).ok()) { s.values.insert(id.to_string(), value); }
            if let Some(k) = v["k"].as_str() { s.reached.entry(id.to_string()).or_insert_with(|| k.to_string()); }
        }
        if answers.len() < ANSWERS_PER_INPUT_COUNT_MAX && !answers.iter().any(|a| a == o) { answers.push(o.to_string()); }
    }
    s
}
fn kept(old: &Value, new: &Value) -> bool {
    match (old, new) {
        (Value::Object(was), Value::Object(now)) => was.iter().all(|(k, v)| now.get(k).is_some_and(|w| kept(v, w))),
        _ => old == new,
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Data { Same, Extended, Differs, Missing }
impl Data {
    fn name(self) -> &'static str { match self { Data::Same => "same", Data::Extended => "extended", Data::Differs => "differs", Data::Missing => "missing" } }
    fn named(s: &str) -> Data { match s { "same" => Data::Same, "extended" => Data::Extended, "differs" => Data::Differs, _ => Data::Missing } }
}
fn compared(old: Option<&Value>, new: Option<&Value>) -> Data {
    match (old, new) {
        (Some(old), Some(new)) if old == new => Data::Same,
        (Some(old), Some(new)) if kept(old, new) => Data::Extended,
        (Some(_), Some(_)) => Data::Differs,
        _ => Data::Missing,
    }
}
pub fn recorded(x: &mut Value, traces: &[Option<Seen>; 4]) {
    let id = x["id"].as_str().unwrap_or("").to_string();
    let key = format!("{}#{}", x["path"].as_str().unwrap_or(""), x["export"].as_str().unwrap_or(""));
    let answer = |t: &Option<Seen>| match t { Some(s) => s.answers.get(&id).and_then(|a| a.first()).map_or_else(|| if s.started.contains(&id) { json!(HUNG) } else { Value::Null }, |a| json!(a)), None => Value::Null };
    let reached = |t: &Option<Seen>| t.as_ref().is_some_and(|s| s.reached.get(&id) == Some(&key));
    let (old, new) = (traces[0].as_ref().and_then(|s| s.values.get(&id)), traces[2].as_ref().and_then(|s| s.values.get(&id)));
    x["reached"] = json!({ "old": reached(&traces[0]), "new": reached(&traces[2]) });
    x["answers"] = json!({ "old": answer(&traces[0]), "old_again": answer(&traces[1]), "new": answer(&traces[2]), "new_again": answer(&traces[3]) });
    x["data"] = json!(compared(old, new).name());
    x["chance"] = json!(traces.iter().flatten().any(|s| s.answers.get(&id).is_some_and(|a| a.len() > 1)));
    x["network"] = json!(traces.iter().flatten().any(|s| s.network));
}
fn uneven(setup: &Value, dir: &str, suite: Option<&str>) -> Value { json!(crate::suite::setup_differs(setup, dir).or_else(|| suite.map(str::to_string))) }
fn trace(out: &Path, volume: &str, (tests, env): (&Tests, &Env), dir: &str, (files, serve): (&Texts, &Texts), name: &str, seconds: u64) -> Option<Seen> {
    if seconds < PROBE_RUN_SECS_MIN { return None; }
    let traced = crate::suite::traced_run(out, volume, (tests, env), dir, (files, serve), name, seconds)?;
    let _ = std::fs::write(out.join(format!("{name}.jsonl")), &traced.trace);
    let _ = std::fs::write(out.join(format!("{name}.behavior")), if traced.network { "network\n" } else { "" });
    let mut s = seen(&traced.trace);
    s.network = traced.network;
    Some(s)
}
fn run_plan(out: &Path, volumes: [&str; 2], p: &mut Plan, phase_until: u64) -> Result<(), &'static str> {
    if within(phase_until, PROBE_SECS_MAX) < 2 * PROBE_RUN_SECS_MIN { return Err(LATE); }
    let label = crate::suite::label(&p.dir);
    let mut traces: [Option<Seen>; 4] = Default::default();
    for (round, slots) in [[0, 2], [1, 3]].into_iter().enumerate() {
        if round == 1 && !p.records.iter().any(|x| verdict(&evidence(x)).differs()) { break; }
        for slot in slots {
            let side = slot / 2;
            let name = format!("probe-{}{label}", SIDES[slot]);
            traces[slot] = trace(out, volumes[side], (&p.sides[side].0, &p.sides[side].1), &p.dir, (&p.files[side], &p.serve[side]), &name, within(phase_until, PROBE_RUN_SECS_MAX));
        }
        for x in p.records.iter_mut() { recorded(x, &traces); }
    }
    Ok(())
}
fn tried(out: &Path, repo: &Path, commits: (&str, &str), inv: &Investigation, suites: &[Suite], until: u64) -> Value {
    let phase_until = until.min(now_ms().saturating_add(PROBE_SECS_MAX * 1000));
    let setup = read_json(out.join("setup.json")).unwrap_or(Value::Null);
    let volumes = ["old", "new"].map(|side| setup[side]["volume"].as_str().unwrap_or("").to_string());
    let (plans, mut not_run) = plan(out, repo, commits, inv, suites);
    let mut experiments = Vec::new();
    for mut p in plans {
        let differs = uneven(&setup, &p.dir, p.uneven.as_deref());
        for x in p.records.iter_mut() { x["uneven"] = differs.clone(); }
        match run_plan(out, [volumes[0].as_str(), volumes[1].as_str()], &mut p, phase_until) {
            Ok(()) => experiments.append(&mut p.records),
            Err(why) => not_run.extend(p.records.iter().filter_map(|x| x["path"].as_str()).collect::<BTreeSet<_>>().into_iter().map(|path| skipped(path, why))),
        }
    }
    let (said, why) = if experiments.is_empty() { ("NOT_RUN", NONE_TRIED) } else { ("RAN", "") };
    json!({ "status": said, "why": why, "experiments": experiments, "not_run": not_run })
}
fn status(status: &str, why: &str) -> Value { json!({ "status": status, "why": why, "experiments": [], "not_run": [] }) }
fn arrived(file: &Path, wait_secs: u64) -> bool {
    let started = now_ms();
    for _ in 0..=wait_secs / INVESTIGATION_POLL_SECS {
        if file.is_file() || secs_since(started) >= wait_secs { break; }
        std::thread::sleep(std::time::Duration::from_secs(INVESTIGATION_POLL_SECS));
    }
    file.is_file()
}
pub fn run(out: &Path, investigation: Option<&Path>, until: u64) -> Value {
    let Some(file) = investigation else { return status("NONE", OFF) };
    let suites = crate::suite::ran_suites(out).unwrap_or_default();
    if suites.is_empty() { return status("NOT_RUN", if out.join("leaves.json").is_file() { LEAVES } else { UNTESTED }); }
    if !suites.iter().any(|s| s.old.as_ref().is_some_and(|o| hostable(o.0.runner)) && hostable(s.new.0.runner)) { return status("NOT_RUN", RUNNERS); }
    let wait_secs = within(until, INVESTIGATION_WAIT_SECS_MAX + PROBE_SECS_MIN).saturating_sub(PROBE_SECS_MIN);
    if !arrived(file, wait_secs) { return status("NOT_RUN", &format!("the AI step's findings did not arrive within {wait_secs} seconds")); }
    let d = read_json(out.join("drive.json")).unwrap_or(Value::Null);
    let commits = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let found = read_json(file).map_or_else(|e| Parsed::Unusable(format!("Kern could not read the AI step's findings: {}", e.0)), |v| parse(&v, commits));
    match found {
        Parsed::Nothing(why) => status("NONE", &why),
        Parsed::Unusable(why) => status("NOT_RUN", &why),
        Parsed::Investigated(_) if within(until, PROBE_SECS_MAX) < PROBE_SECS_MIN => status("NOT_RUN", LATE),
        Parsed::Investigated(i) => tried(out, Path::new(d["repo"].as_str().unwrap_or("")), commits, &i, &suites, until),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers { pub old: Option<String>, pub old_again: Option<String>, pub new: Option<String>, pub new_again: Option<String> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub requirement: Option<Requirement>, pub shape_seen: bool, pub call_seen: bool, pub reached_old: bool, pub reached_new: bool, pub answers: Answers, pub uneven: bool, pub data: Data,
    pub chance: bool, pub network: bool, pub retested: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unchecked { Unreached, Unanswered, Sandbox, Chance, Hung }
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Missing { pub contested: bool, pub authorized: bool, pub shape: bool, pub rerun: bool, pub values: bool, pub retested: bool, pub scope: bool }
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict { Unchecked(Unchecked), Same, Note, Uneven, Look(Missing), Blocked }
impl Verdict {
    pub fn differs(self) -> bool { matches!(self, Verdict::Note | Verdict::Uneven | Verdict::Look(_) | Verdict::Blocked) }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Broken { No, Yes, Unread }
fn succeeded(a: &str) -> bool { a.starts_with("returns ") || a.starts_with("resolves ") }
fn crashed(a: &str) -> bool { crate::trace::failed(a) && crate::proof::crashed(a) }
fn broken(check: Check, (old, new): (&str, &str), data: Data) -> Broken {
    let yes = |b: bool| if b { Broken::Yes } else { Broken::No };
    if crashed(old) && succeeded(new) { return Broken::No; }
    match check {
        Check::Same if succeeded(old) && succeeded(new) => match data { Data::Differs => Broken::Yes, Data::Same | Data::Extended => Broken::No, Data::Missing => Broken::Unread },
        Check::Same => yes(succeeded(old) != succeeded(new)),
        Check::Returns => yes(succeeded(old) && crate::trace::failed(new)),
        Check::Throws => yes(crate::trace::failed(old) && succeeded(new)),
    }
}
pub fn verdict(e: &Evidence) -> Verdict {
    let a = &e.answers;
    if !(e.reached_old && e.reached_new) { return Verdict::Unchecked(Unchecked::Unreached); }
    let (Some(old), Some(new)) = (a.old.as_deref(), a.new.as_deref()) else { return Verdict::Unchecked(Unchecked::Unanswered) };
    if [&a.old, &a.old_again, &a.new, &a.new_again].into_iter().flatten().any(|x| crate::trace::failed(x) && crate::suite::sandbox_error(x)) { return Verdict::Unchecked(Unchecked::Sandbox); }
    if [&a.old, &a.old_again, &a.new, &a.new_again].into_iter().flatten().any(|x| x == HUNG) { return Verdict::Unchecked(Unchecked::Hung); }
    if old == new { return Verdict::Same; }
    if e.chance || a.old_again.as_deref().is_some_and(|x| x != old) || a.new_again.as_deref().is_some_and(|x| x != new) { return Verdict::Unchecked(Unchecked::Chance); }
    if e.network { return Verdict::Unchecked(Unchecked::Sandbox); }
    let Some((r, found)) = e.requirement.filter(|r| r.authority != Authority::Unverified).map(|r| (r, broken(r.check, (old, new), e.data))).filter(|(_, b)| *b != Broken::No) else { return Verdict::Note };
    if e.uneven { return Verdict::Uneven; }
    let missing = Missing { contested: r.authority == Authority::Contested, authorized: r.authority == Authority::Authorized, shape: !e.shape_seen,
        rerun: a.old_again.is_none() || a.new_again.is_none(), values: found == Broken::Unread, retested: e.retested, scope: r.check == Check::Same || !e.call_seen };
    if missing == Missing::default() { Verdict::Blocked } else { Verdict::Look(missing) }
}
pub fn evidence(x: &Value) -> Evidence {
    let answer = |k: &str| x["answers"][k].as_str().filter(|a| outcome(a)).map(str::to_string);
    let args = x["args"].as_array().cloned().unwrap_or_default();
    let shape = crate::calls::shape(&args);
    let shape_seen = x["shapes"].as_array().into_iter().flatten().any(|s| s.as_array().is_some_and(|tags| tags.iter().map(Value::as_str).eq(shape.iter().map(|t| Some(*t)))));
    let call_seen = x["calls"].as_array().into_iter().flatten().any(|c| c.as_array() == Some(&args));
    let o = &x["obligation"];
    let requirement = o["check"].as_str().and_then(check_named).map(|check| Requirement { check, authority: o["authority"].as_str().and_then(authority_named).unwrap_or(Authority::Unverified) });
    Evidence { requirement, shape_seen, call_seen, reached_old: x["reached"]["old"] == true, reached_new: x["reached"]["new"] == true,
        answers: Answers { old: answer("old"), old_again: answer("old_again"), new: answer("new"), new_again: answer("new_again") }, uneven: x["uneven"].as_str().is_some_and(|u| !u.is_empty()),
        data: Data::named(x["data"].as_str().unwrap_or("")), chance: x["chance"] == true, network: x["network"] == true, retested: x["retested"] == true }
}
pub fn replayed(dir: &Path, stored: &Value, setup: Option<&Value>, suites: &[Suite]) -> Value {
    let read = |name: String| -> Option<Seen> {
        std::fs::metadata(dir.join(&name)).ok().filter(|m| m.len() <= TRACE_BYTES_MAX)?;
        let mut s = seen(&std::fs::read_to_string(dir.join(&name)).ok()?);
        s.network = std::fs::read_to_string(dir.join(name.replace(".jsonl", ".behavior"))).is_ok_and(|b| b.trim() == "network");
        Some(s)
    };
    let mut out = stored.clone();
    let mut traces: BTreeMap<String, [Option<Seen>; 4]> = BTreeMap::new();
    for x in out["experiments"].as_array_mut().into_iter().flatten() {
        let folder = x["dir"].as_str().unwrap_or("").to_string();
        let label = crate::suite::label(&folder);
        let found = traces.entry(folder.clone()).or_insert_with(|| SIDES.map(|side| read(format!("probe-{side}{label}.jsonl"))));
        if found.iter().any(Option::is_some) { recorded(x, found); }
        if let Some(s) = setup { x["uneven"] = uneven(s, &folder, suites.iter().find(|s| s.dir == folder).and_then(|s| s.uneven.as_deref())); }
    }
    out
}
