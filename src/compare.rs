use crate::capability::{is_code, source_types};
use crate::ensure;
use crate::git::{Entry, Files, object_id};
use crate::policy::{App, Contract, Delta, Tests};
use crate::util::*;
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::GetSpan;
use oxc_syntax::operator::UnaryOperator;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub use crate::policy::COMPARE as CHECK;
pub const PROBE: &str = include_str!("probe.cjs");
const MARK: &[u8] = b"\n\x1ekern-probe\x1e\n";
const MAX_MODULES: usize = 40;
const MAX_TUPLES: usize = 240;
const MAX_HARVEST: usize = 120;
const MAX_CALLS: usize = 20000;
const KEEP: usize = 20;
const UNUSUAL: &[&str] = &[
    "undefined", "null", "0", "-0", "1", "-1", "2", "10", "0.5", "-0.5", "1e21", "NaN", "Infinity", "-Infinity", "9007199254740991", "true", "false",
    "\"\"", "\" \"", "\"a\"", "\"abc\"", "\"0\"", "\"-1\"", "\"1.5\"", "\"true\"", "\"null\"", "\"%s\"", "\"\\u0000\"", "\"\\u00e9\\ud83d\\ude00\"", "\"<script>\"",
    "\"a\".repeat(1000)", "[]", "[1, 2, 3]", "[null]", "{}", "{\"a\": 1}", "{\"length\": -1}", "new Date(0)",
];
const DELETES: &[&str] = &["rmSync", "unlinkSync", "rmdirSync", "rmdir", "truncateSync", "ftruncateSync", "emptyDir", "emptyDirSync", "removeSync", "rimraf", "rimrafSync", "deleteAsync", "deleteSync"];
const DELETES_VIA_FS: &[&str] = &["rm", "unlink", "truncate", "ftruncate", "remove"];
const PAID: &[&str] = &["chat.completions.create", "responses.create", "completions.create", "embeddings.create", "images.generate", "audio.speech.create", "audio.transcriptions.create",
    "messages.create", "messages.stream", "generateContentStream", "generateContent", "generateText", "streamText", "generateObject", "streamObject"];
const PAID_HOSTS: &[&str] = &["api.openai.com", "api.anthropic.com", "generativelanguage.googleapis.com", "aiplatform.googleapis.com", "api.cohere.ai", "api.cohere.com", "api.mistral.ai",
    "api.groq.com", "api.together.xyz", "api.fireworks.ai", "api.deepseek.com", "api.x.ai", "openrouter.ai", "api.perplexity.ai", "api.replicate.com", "api.elevenlabs.io",
    "api.assemblyai.com", "api.deepgram.com", "api.twilio.com", "api.sendgrid.com"];
fn paid_host(h: &str) -> bool { PAID_HOSTS.contains(&h) || h.ends_with(".openai.azure.com") || (h.starts_with("bedrock-runtime.") && h.ends_with(".amazonaws.com")) }

fn text_of<'f>(files: &'f Files, path: &str) -> Option<&'f str> { files.get(path).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()) }
fn clip(s: &str, n: usize) -> String { if s.chars().count() > n { format!("{}...", s.chars().take(n).collect::<String>()) } else { s.to_string() } }
fn is_test(path: &str, tests: Option<&Tests>) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    tests.is_some_and(|t| allowed(&t.paths, path)) || lower.split('/').any(|d| ["test", "tests", "__tests__", "spec", "__mocks__", "e2e", "fixtures", "__fixtures__"].contains(&d))
        || [".test.", ".spec."].iter().any(|m| name.contains(m))
}
pub fn probeable(path: &str, tests: Option<&Tests>) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    is_code(path) && !is_test(path, tests) && !lower.split('/').any(|d| d == "node_modules") && ![".config.", ".stories."].iter().any(|m| name.contains(m))
        && !["gulpfile", "gruntfile", "webpack.", "rollup.", "jest.", "babel.", "eslint", "prettier"].iter().any(|t| name.starts_with(t))
}

#[derive(Default)]
struct Scan { tuples: Vec<String>, values: Vec<String>, deletes: BTreeMap<String, u64>, paid: BTreeMap<String, u64> }
struct Scanner<'s> { source: &'s str, found: Scan }
fn plain(e: &Expression) -> bool {
    match e {
        Expression::StringLiteral(_) | Expression::NumericLiteral(_) | Expression::BooleanLiteral(_) | Expression::NullLiteral(_) | Expression::BigIntLiteral(_) => true,
        Expression::TemplateLiteral(t) => t.expressions.is_empty(),
        Expression::Identifier(id) => matches!(id.name.as_str(), "undefined" | "NaN" | "Infinity"),
        Expression::UnaryExpression(u) => matches!(u.operator, UnaryOperator::UnaryNegation | UnaryOperator::UnaryPlus) && matches!(u.argument, Expression::NumericLiteral(_)),
        Expression::ArrayExpression(a) => a.elements.len() <= 16 && a.elements.iter().all(|el| el.as_expression().is_some_and(plain)),
        Expression::ObjectExpression(o) => o.properties.len() <= 16 && o.properties.iter().all(|p| matches!(p, ObjectPropertyKind::ObjectProperty(p) if !p.computed && !p.method && !p.shorthand && p.kind == PropertyKind::Init && plain(&p.value))),
        Expression::ParenthesizedExpression(p) => plain(&p.expression),
        _ => false,
    }
}
fn chain(e: &Expression) -> Option<String> {
    match e {
        Expression::Identifier(id) => Some(id.name.to_string()),
        Expression::ThisExpression(_) => Some("this".into()),
        Expression::StaticMemberExpression(m) => Some(format!("{}.{}", chain(&m.object).unwrap_or_else(|| "?".into()), m.property.name)),
        Expression::ParenthesizedExpression(p) => chain(&p.expression),
        _ => None,
    }
}
impl<'a> Visit<'a> for Scanner<'_> {
    fn visit_ts_type(&mut self, _: &TSType<'a>) {}
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let required = matches!(&it.callee, Expression::Identifier(id) if id.name == "require");
        if !required && !it.arguments.is_empty() && it.arguments.len() <= 4 && it.arguments.iter().all(|a| a.as_expression().is_some_and(plain)) && self.found.tuples.len() < 400 {
            let parts: Vec<&str> = it.arguments.iter().filter_map(|a| self.source.get(a.span().start as usize..a.span().end as usize)).collect();
            if parts.len() == it.arguments.len() { self.found.tuples.push(format!("[{}]", parts.join(", "))); }
        }
        if let Some(c) = chain(&it.callee) {
            let last = c.rsplit('.').next().unwrap_or(&c);
            let via_fs = c.contains('.') && (c.split('.').next().is_some_and(|r| r.to_ascii_lowercase().contains("fs")) || c.contains(".promises."));
            if DELETES.contains(&last) || (via_fs && DELETES_VIA_FS.contains(&last)) { *self.found.deletes.entry(c.clone()).or_default() += 1; }
            if let Some(p) = PAID.iter().find(|p| c == **p || c.ends_with(&format!(".{p}"))) { *self.found.paid.entry(p.to_string()).or_default() += 1; }
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if it.value.chars().count() <= 120 && self.found.values.len() < 200 { self.found.values.push(serde_json::to_string(it.value.as_str()).unwrap_or_default()); }
    }
    fn visit_numeric_literal(&mut self, it: &NumericLiteral<'a>) {
        if let Some(s) = self.source.get(it.span.start as usize..it.span.end as usize) && self.found.values.len() < 200 { self.found.values.push(s.to_string()); }
    }
}
fn scan(path: &str, source: &str) -> Option<Scan> {
    source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let p = Parser::new(&allocator, source, st).parse();
        (!p.fatal_error && p.diagnostics.is_empty()).then(|| {
            let mut v = Scanner { source, found: Scan::default() };
            v.visit_program(&p.program);
            v.found
        })
    })
}

struct Rng(u64);
impl Rng {
    fn new(seed: &str) -> Rng { Rng(u64::from_str_radix(&seed[..16.min(seed.len())], 16).unwrap_or(0x9e37_79b9_7f4a_7c15) | 1) }
    fn pick<'p>(&mut self, pool: &[&'p str]) -> &'p str {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        pool[(x % pool.len() as u64) as usize]
    }
}
fn add(list: &mut Vec<String>, p: &str) { if list.len() < MAX_MODULES && !list.iter().any(|x| x == p) { list.push(p.to_string()); } }
fn targets(base: &Files, candidate: &Files, changed: &Value, policy: &Contract) -> Vec<String> {
    let tests = policy.tests.as_ref();
    let mut list = Vec::new();
    for c in policy.delta.iter().flat_map(|d| &d.changes) {
        let p = c.function.split('#').next().unwrap_or("");
        if base.contains_key(p) || candidate.contains_key(p) { add(&mut list, p); }
    }
    let touched: Vec<String> = changed["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str()).filter(|p| probeable(p, tests)).map(str::to_string).collect();
    for p in &touched { add(&mut list, p); }
    let mut edges = crate::proof::import_edges(base);
    for (k, v) in crate::proof::import_edges(candidate) { edges.entry(k).or_default().extend(v); }
    let mut frontier: BTreeSet<String> = touched.into_iter().collect();
    for _ in 0..2 {
        let next: BTreeSet<String> = edges.iter().filter(|(from, to)| probeable(from, tests) && !frontier.contains(*from) && to.iter().any(|t| frontier.contains(t))).map(|(f, _)| f.clone()).collect();
        for p in &next { add(&mut list, p); }
        frontier = next;
    }
    let moved = crate::deps::moved(base, candidate);
    if !moved.is_empty() {
        let uses = |p: &String| text_of(candidate, p).and_then(|s| crate::capability::footprint(p, s).ok()).is_some_and(|caps| caps.iter().any(|c| c.strip_prefix("package:").is_some_and(|n| moved.contains(n))));
        let code: Vec<&String> = candidate.keys().filter(|p| probeable(p, tests)).collect();
        for p in code.iter().filter(|p| uses(p)) { add(&mut list, p); }
        for p in code { add(&mut list, p); }
    }
    list
}
fn harvest(base: &Files, candidate: &Files, tests: Option<&Tests>) -> Vec<String> {
    let (mut out, mut seen) = (Vec::new(), BTreeSet::new());
    for (files, cap) in [(base, MAX_HARVEST * 3 / 4), (candidate, MAX_HARVEST)] {
        for path in files.keys().filter(|p| is_code(p) && is_test(p, tests)) {
            for t in text_of(files, path).and_then(|s| scan(path, s)).map(|s| s.tuples).unwrap_or_default() {
                if out.len() < cap && seen.insert(t.clone()) { out.push(t); }
            }
        }
    }
    out
}
fn push(tuples: &mut Vec<String>, seen: &mut BTreeSet<String>, t: String) { if t.len() <= 600 && tuples.len() < MAX_TUPLES && seen.insert(t.clone()) { tuples.push(t); } }
fn app_json(a: &App) -> Value { json!({ "setup": a.setup, "start": a.start, "port": a.port, "ready": a.ready, "routes": a.routes, "roles": a.roles, "env": a.env }) }

pub fn plan(base: &Files, candidate: &Files, changed: &Value, policy: &Contract, seed: &str, timeout_seconds: u64) -> R<Value> {
    ensure!(is_hex64(seed), "Invalid comparison seed");
    let harvested = harvest(base, candidate, policy.tests.as_ref());
    let mut rng = Rng::new(seed);
    let targets = targets(base, candidate, changed, policy);
    let per_function = (MAX_CALLS / targets.len().max(1) / 20).max(8);
    let modules: Vec<Value> = targets.into_iter().map(|path| {
        let values = |f: &Files| text_of(f, &path).and_then(|s| scan(&path, s)).map(|x| x.values).unwrap_or_default();
        let (old, new) = (values(base), values(candidate));
        let mut literals: Vec<String> = Vec::new();
        for v in old.iter().take(30).chain(new.iter()) { if literals.len() < 40 && !literals.contains(v) { literals.push(v.clone()); } }
        let pool: Vec<&str> = UNUSUAL.iter().copied().chain(literals.iter().map(String::as_str)).collect();
        let (mut tuples, mut seen) = (Vec::new(), BTreeSet::new());
        for t in &harvested { push(&mut tuples, &mut seen, t.clone()); }
        push(&mut tuples, &mut seen, "[]".into());
        for v in &pool { push(&mut tuples, &mut seen, format!("[{v}]")); }
        for _ in 0..40 { let t = format!("[{}, {}]", rng.pick(&pool), rng.pick(&pool)); push(&mut tuples, &mut seen, t); }
        for _ in 0..30 { let t = format!("[{}, {}, {}]", rng.pick(&pool), rng.pick(&pool), rng.pick(&pool)); push(&mut tuples, &mut seen, t); }
        json!({ "path": path, "tuples": tuples })
    }).collect();
    let delta = policy.delta.as_ref();
    let changes: Vec<Value> = delta.map_or_else(Vec::new, |d| d.changes.iter().map(|c| json!({ "function": c.function, "when": c.when })).collect());
    let examples: Vec<Value> = delta.map_or_else(Vec::new, |d| d.changes.iter().flat_map(|c| c.examples.iter().map(move |e| json!({ "function": c.function, "args": e.args, "returns": e.returns,
        "throws": e.throws.as_ref().map_or(Value::Null, |t| if t.is_empty() { json!(true) } else { json!(t) }) }))).collect());
    Ok(json!({ "version": 1, "seed": seed, "budget_ms": timeout_seconds.saturating_sub(10).max(5) * 1000, "call_ms": 1000, "load_ms": 20000, "per_function": per_function, "modules": modules,
        "changes": changes, "examples": examples, "removals": removals(base, candidate, changed, policy), "app": policy.app.as_ref().map(app_json) }))
}

fn pure_removals(a: &[&str], b: &[&str]) -> Option<Vec<(usize, usize)>> {
    let (mut out, mut hunk) = (Vec::new(), Vec::new());
    for s in edits(a, b)?.into_iter().chain(std::iter::once(Step::Keep)) {
        if !matches!(s, Step::Keep) { hunk.push(s); continue; }
        let deleted: Vec<usize> = hunk.iter().filter_map(|h| if let Step::Delete(i) = h { Some(*i) } else { None }).collect();
        if !hunk.is_empty() && deleted.len() == hunk.len() { out.push((deleted.iter().min().copied().unwrap_or(0), deleted.len())); }
        hunk.clear();
    }
    Some(out)
}
pub fn changed_lines(a: &[&str], b: &[&str]) -> Option<(Vec<usize>, Vec<usize>)> {
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for s in edits(a, b)? {
        match s { Step::Delete(i) => old.push(i), Step::Insert(j) => new.push(j), Step::Keep => {} }
    }
    Some((old, new))
}
fn edits(a: &[&str], b: &[&str]) -> Option<Vec<Step>> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] { pre += 1; }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] { suf += 1; }
    let (x, y) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (x.len() as isize, y.len() as isize);
    let max = (n + m) as usize;
    let off = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=(max as isize).min(1000) {
        trace.push(v[(off - d - 1) as usize..=(off + d + 1) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let i = (k + off) as usize;
            let mut px = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut py = px - k;
            while px < n && py < m && x[px as usize] == y[py as usize] { px += 1; py += 1; }
            v[i] = px;
            if px >= n && py >= m { end = Some(d); break 'search; }
        }
    }
    let end = end?;
    let (mut px, mut py, mut steps) = (n, m, vec![Step::Keep; suf]);
    for d in (0..=end).rev() {
        let w = &trace[d as usize];
        let at = |k: isize| w[(k + d + 1) as usize];
        let k = px - py;
        let prev = if k == -d || (k != d && at(k - 1) < at(k + 1)) { k + 1 } else { k - 1 };
        let (qx, qy) = (at(prev), at(prev) - prev);
        while px > qx && py > qy { steps.push(Step::Keep); px -= 1; py -= 1; }
        if d > 0 { steps.push(if px == qx { Step::Insert(pre + (py - 1) as usize) } else { Step::Delete(pre + (px - 1) as usize) }); }
        px = qx;
        py = qy;
    }
    steps.extend(std::iter::repeat_n(Step::Keep, pre));
    steps.reverse();
    Some(steps)
}
#[derive(Clone, Copy)]
enum Step { Keep, Delete(usize), Insert(usize) }
pub fn removed_code(base: &Files, candidate: &Files, changed: &Value, policy: &Contract) -> Vec<Value> { removals(base, candidate, changed, policy) }
fn removals(base: &Files, candidate: &Files, changed: &Value, policy: &Contract) -> Vec<Value> {
    let tests = policy.tests.as_ref();
    let mut out = Vec::new();
    for f in changed["files"].as_array().into_iter().flatten() {
        let (path, op) = (f["path"].as_str().unwrap_or(""), f["operation"].as_str().unwrap_or(""));
        if !probeable(path, tests) { continue; }
        if op == "delete" { out.push(json!({ "path": path, "start": 1, "count": text_of(base, path).map_or(0, |s| s.lines().count()), "file": true })); }
        else if op == "modify" && let (Some(a), Some(b)) = (text_of(base, path), text_of(candidate, path)) {
            let (x, y): (Vec<&str>, Vec<&str>) = (a.split('\n').collect(), b.split('\n').collect());
            for (start, count) in pure_removals(&x, &y).unwrap_or_default() { out.push(json!({ "path": path, "start": start + 1, "count": count, "file": false })); }
        }
    }
    out.truncate(KEEP);
    out
}
pub fn without_removals(base: &Files, plan: &Value) -> Files {
    let mut files = base.clone();
    let mut cut: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for r in plan["removals"].as_array().into_iter().flatten() {
        let path = r["path"].as_str().unwrap_or("").to_string();
        if r["file"] == true { files.remove(&path); } else { cut.entry(path).or_default().push((r["start"].as_u64().unwrap_or(0), r["count"].as_u64().unwrap_or(0))); }
    }
    for (path, ranges) in cut {
        let (Some(text), Some(mode)) = (text_of(base, &path), base.get(&path).map(|e| e.mode.clone())) else { continue };
        let kept: Vec<&str> = text.split('\n').enumerate().filter(|(i, _)| !ranges.iter().any(|(s, c)| (*i as u64 + 1) >= *s && (*i as u64 + 1) < s + c)).map(|(_, l)| l).collect();
        let bytes = kept.join("\n").into_bytes();
        files.insert(path, Entry { mode, oid: object_id("blob", &bytes), size: bytes.len() as u64, bytes: Some(bytes) });
    }
    files
}
fn porcelain(text: &str) -> Vec<Value> {
    let (mut commits, mut current): (Vec<(String, String)>, Option<String>) = (Vec::new(), None);
    for line in text.lines().filter(|l| !l.starts_with('\t')) {
        let first = line.split(' ').next().unwrap_or("");
        if is_oid(first) {
            current = Some(first.to_string());
            if !commits.iter().any(|(c, _)| c == first) { commits.push((first.to_string(), String::new())); }
        } else if let (Some(c), Some(s)) = (&current, line.strip_prefix("summary ")) && let Some(e) = commits.iter_mut().find(|(x, _)| x == c) && e.1.is_empty() {
            e.1 = s.to_string();
        }
    }
    commits.into_iter().take(3).map(|(c, s)| json!({ "commit": c, "summary": clip(&s, 200) })).collect()
}
pub fn history(repo: &Path, base: &str, plan: &Value) -> Vec<Value> {
    plan["removals"].as_array().into_iter().flatten().take(10).filter_map(|r| {
        let path = r["path"].as_str()?;
        let (start, count) = (r["start"].as_u64()?, r["count"].as_u64()?);
        let file = r["file"] == true;
        let lines = if file { "file".to_string() } else if count <= 1 { start.to_string() } else { format!("{start}-{}", start + count - 1) };
        let commits = if file {
            let out = crate::git::git(repo, &["log", "-n", "3", "--format=%H%x1f%s", base, "--", path], None).ok()?;
            String::from_utf8_lossy(&out).lines().filter_map(|l| l.split_once('\x1f')).map(|(c, s)| json!({ "commit": c, "summary": clip(s, 200) })).collect()
        } else {
            porcelain(&String::from_utf8_lossy(&crate::git::git(repo, &["blame", "--porcelain", "-L", &format!("{start},+{count}"), base, "--", path], None).ok()?))
        };
        Some(json!({ "path": path, "lines": lines, "commits": commits }))
    }).collect()
}

pub fn read_probe(stdout: &[u8]) -> R<Value> {
    let found: Vec<usize> = stdout.windows(MARK.len()).enumerate().filter(|(_, w)| *w == MARK).map(|(i, _)| i).collect();
    ensure!(!found.is_empty(), "the output holds no probe report");
    ensure!(found.len() == 1, "the output holds more than one probe report; the code under test must not print one");
    let line = stdout[found[0] + MARK.len()..].split(|b| *b == b'\n').next().unwrap_or(&[]);
    let v: Value = serde_json::from_slice(line).map_err(|_| Denied("the probe report is not complete".into()))?;
    if let Some(e) = v["error"].as_str() { return Err(Denied(format!("the probe failed: {e}"))); }
    ensure!(v["version"] == 1 && v["modules"].is_array(), "the probe report is not complete");
    Ok(v)
}

pub struct Runs { pub old: [Value; 2], pub new: [Value; 2], pub removed: Option<Value>, pub counts: [BTreeMap<String, u64>; 2] }
static NOTHING: Value = Value::Null;
fn module<'v>(run: &'v Value, path: &str) -> &'v Value { run["modules"].as_array().and_then(|m| m.iter().find(|x| x["path"] == path)).unwrap_or(&NOTHING) }
fn word(out: &str) -> (&str, &str) { out.split_once(' ').unwrap_or((out, "")) }
fn did(out: &str) -> String {
    match word(out) {
        ("r", v) => format!("returned {v}"),
        ("e", v) => format!("threw {v}"),
        ("t", _) => "did not finish in time".into(),
        ("x", _) => "stopped its thread".into(),
        ("m", _) => "did not exist".into(),
        ("i", _) => "could not be called".into(),
        _ => out.to_string(),
    }
}
fn does(out: &str) -> String {
    match word(out) {
        ("r", v) => format!("returns {v}"),
        ("e", v) => format!("throws {v}"),
        ("t", _) => "did not finish in time".into(),
        ("x", _) => "stops its thread".into(),
        ("m", _) => "does not exist".into(),
        ("i", _) => "cannot be called".into(),
        _ => out.to_string(),
    }
}
fn error_name(out: &str) -> Option<&str> { out.strip_prefix("e ").map(|s| s.split(": ").next().unwrap_or(s)) }
fn kind(old: &str, new: &str) -> &'static str {
    let (o, n) = (old.as_bytes().first().copied().unwrap_or(b'?'), new.as_bytes().first().copied().unwrap_or(b'?'));
    match (o, n) {
        (b'r', b'r') => "changed",
        (b't', _) => "finishes now",
        (_, b't') => "new hang",
        (b'r', _) | (b'e' | b'm' | b'i', b'x') => "new crash",
        (b'e' | b'x', b'r') => "now accepted",
        (b'e', b'e') if error_name(old) == error_name(new) => "changed message",
        _ => "changed error",
    }
}
fn inside(delta: Option<&Delta>, name: &str, when: Option<bool>) -> bool {
    let (path, key) = name.split_once('#').unwrap_or((name, ""));
    delta.is_some_and(|d| d.changes.iter().any(|c| {
        let (p, k) = c.function.split_once('#').unwrap_or((&c.function, ""));
        p == path && (k == "*" || k == key) && (c.when.is_none() || when == Some(true))
    }))
}
fn stated_removal(delta: Option<&Delta>, item: &str) -> bool {
    let path = item.split('#').next().unwrap_or(item);
    delta.is_some_and(|d| d.removes.iter().any(|r| r == item || r == path || *r == format!("{path}#*") || (r.ends_with('/') && item.starts_with(r.as_str()))))
}
fn quote(items: &[String]) -> String {
    let shown: Vec<String> = items.iter().take(5).map(|k| serde_json::to_string(&clip(k, 240)).unwrap_or_default()).collect();
    if items.len() > 5 { format!("{} and {} more", shown.join(", "), items.len() - 5) } else { shown.join(", ") }
}
fn ms(us: u64) -> String { format!("{}.{:03}", us / 1000, us % 1000) }
fn first(items: &[String]) -> Value { json!(items.iter().take(KEEP).map(|s| clip(s, 240)).collect::<Vec<_>>()) }

pub fn evaluate(plan: &Value, runs: &Runs, base: &Files, candidate: &Files, changed: &Value, policy: &Contract) -> (Value, Option<String>) {
    let delta = policy.delta.as_ref();
    let tests = policy.tests.as_ref();
    let allow = |lens: &str| delta.is_some_and(|d| d.allow.iter().any(|a| a == lens));
    let ([oa, ob], [na, nb]) = (&runs.old, &runs.new);
    let (mut same, mut noisy, mut skipped, mut calls, mut functions, mut modules, mut stated, mut different) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut differences, mut crashes, mut surprises, mut removed, mut unloadable, mut slow, mut speed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut unanswered: Vec<String> = Vec::new();
    let per_function = plan["per_function"].as_u64().map_or(usize::MAX, |n| n as usize);
    for m in plan["modules"].as_array().into_iter().flatten() {
        let path = m["path"].as_str().unwrap_or("");
        let tuples: Vec<&str> = m["tuples"].as_array().into_iter().flatten().filter_map(Value::as_str).take(per_function).collect();
        let (xa, xb, ya, yb) = (module(oa, path), module(ob, path), module(na, path), module(nb, path));
        let loaded = |x: &Value| x.is_object() && x["load"].is_null();
        if base.contains_key(path) && !candidate.contains_key(path) {
            if !stated_removal(delta, path) { removed.push(path.to_string()); }
            continue;
        }
        if !(loaded(xa) && loaded(xb)) {
            if base.contains_key(path) { unloadable.push(format!("{path}: {}", xa["load"].as_str().or(xb["load"].as_str()).unwrap_or("no result"))); }
            continue;
        }
        let answered: usize = xa["exports"].as_object().into_iter().flatten().filter(|(_, e)| e["type"] != "value")
            .map(|(key, e)| (0..tuples.len()).filter(|k| e["calls"][*k].is_string() && xb["exports"][key.as_str()]["calls"][*k].is_string()).count()).sum();
        let late = |y: &Value| y["ended"] == "time" && !loaded(y);
        if late(ya) || late(yb) {
            if answered > 5 { unanswered.push(format!("{path}: the old code answered {answered} calls here, the new code had no time left to load it")); } else { skipped += answered as u64; }
            continue;
        }
        if !(loaded(ya) && loaded(yb)) {
            let why = ya["load"].as_str().or(yb["load"].as_str()).unwrap_or("no result");
            if !inside(delta, &format!("{path}#*"), None) { crashes.push(format!("{path}: loaded before, now fails to load ({why})")); }
            continue;
        }
        modules += 1;
        let complete = ya["more"] != true && yb["more"] != true;
        let mut missing: Vec<String> = Vec::new();
        let keys: BTreeSet<String> = [xa, ya].iter().flat_map(|x| x["exports"].as_object().into_iter().flatten().map(|(k, _)| k.clone())).collect();
        for key in keys {
            let name = format!("{path}#{key}");
            let pick = |x: &Value| -> Value { x["exports"][key.as_str()].clone() };
            let (o, o2, n, n2) = (pick(xa), pick(xb), pick(ya), pick(yb));
            if o.is_null() { continue; }
            if n.is_null() || n2.is_null() || (o["type"] != "value" && (n["type"] == "value" || n2["type"] == "value")) {
                if (n.is_null() || n2.is_null()) && !complete { continue; }
                if !stated_removal(delta, &name) { removed.push(name); }
                continue;
            }
            if o["type"] == "value" || n["type"] == "value" {
                if o["value"] != o2["value"] || n["value"] != n2["value"] { noisy += 1; continue; }
                if o["value"] == n["value"] && o["type"] == n["type"] { same += 1; continue; }
                different += 1;
                let (was, now) = (o["value"].as_str().map_or_else(|| format!("a {}", o["type"].as_str().unwrap_or("value")), str::to_string), n["value"].as_str().map_or_else(|| format!("a {}", n["type"].as_str().unwrap_or("value")), str::to_string));
                let item = format!("{name}: was {was}, now {now}");
                let ok = inside(delta, &name, Some(true));
                if differences.len() < KEEP { differences.push(json!({ "function": name, "args": "", "old": clip(&was, 240), "new": clip(&now, 240), "kind": "changed", "stated": ok })); }
                if ok { stated += 1; } else { surprises.push(item); }
                continue;
            }
            let mut compared = false;
            for (k, tuple) in tuples.iter().enumerate() {
                let call = |x: &Value| x["calls"][k].as_str().map(str::to_string);
                let (Some(a), Some(b)) = (call(&o), call(&o2)) else { skipped += 1; continue };
                let (Some(c), Some(d)) = (call(&n), call(&n2)) else { missing.push(format!("{name}({tuple}): {}, now was not answered", did(&a))); continue };
                compared = true;
                calls += 1;
                if a != b { noisy += 1; continue; }
                let worse = [&c, &d].into_iter().map(|x| (x, kind(&a, x))).find(|(_, w)| matches!(*w, "new crash" | "new hang"));
                let (what, out) = match worse {
                    Some((x, w)) => (w, x.clone()),
                    None if c != d => ("unstable", c.clone()),
                    None if a == c => { same += 1; continue; }
                    None => (kind(&a, &c), c.clone()),
                };
                different += 1;
                let ok = inside(delta, &name, n["when"][k].as_bool());
                let now = if what == "unstable" { "gives a different answer on every run".to_string() } else { does(&out) };
                let item = format!("{name}({tuple}): {}, now {now}", did(&a));
                if differences.len() < KEEP { differences.push(json!({ "function": name, "args": clip(tuple, 200), "old": clip(&did(&a), 240), "new": clip(&now, 240), "kind": what, "stated": ok })); }
                if ok { stated += 1; }
                else if matches!(what, "new crash" | "new hang") { crashes.push(item); }
                else if !matches!(what, "changed message" | "finishes now") { surprises.push(item); }
            }
            if compared { functions += 1; }
            let us = |x: &Value| x["us"].as_u64();
            if let (Some(o1), Some(o2), Some(n1), Some(n2)) = (us(&o), us(&o2), us(&n), us(&n2)) {
                let (before, after) = (o1.max(o2), n1.min(n2));
                if after >= 3 * before.max(1) && after - before >= 1000 {
                    slow.push(format!("{name}: {} ms before, {} ms now", ms(before), ms(after)));
                    speed.push(json!({ "function": name, "old_us": before, "new_us": after }));
                }
            }
        }
        let timed_out = ya["ended"] == "time" || yb["ended"] == "time";
        if !missing.is_empty() && (!timed_out || missing.len() > (answered / 10).max(5)) { unanswered.append(&mut missing); } else { skipped += missing.len() as u64; }
    }
    let (mut examples, mut failing) = (Vec::new(), Vec::new());
    for (i, e) in plan["examples"].as_array().into_iter().flatten().enumerate() {
        let find = |run: &Value| run["modules"].as_array().into_iter().flatten().flat_map(|m| m["examples"].as_array().into_iter().flatten()).find(|x| x["i"] == i as u64).cloned();
        let (a, b) = (find(na), find(nb));
        let ok = [&a, &b].iter().all(|x| x.as_ref().is_some_and(|x| x["ok"] == true));
        let actual = a.as_ref().and_then(|x| x["out"].as_str()).map_or_else(|| "was not run".to_string(), did);
        let expected = match (&e["returns"], &e["throws"]) { (Value::String(r), _) => format!("return {r}"), (_, Value::String(t)) => format!("throw {t}"), _ => "throw an error".into() };
        let call = format!("{}({})", e["function"].as_str().unwrap_or(""), e["args"].as_str().unwrap_or(""));
        if !ok { failing.push(format!("{call}: expected to {expected}, but it {actual}")); }
        examples.push(json!({ "call": clip(&call, 240), "expected": clip(&expected, 240), "actual": clip(&actual, 240), "ok": ok }));
    }
    for f in changed["files"].as_array().into_iter().flatten().filter(|f| f["operation"] == "delete") {
        let path = f["path"].as_str().unwrap_or("");
        if !is_test(path, tests) && !stated_removal(delta, path) && !removed.iter().any(|r| r == path) { removed.push(path.to_string()); }
    }
    let (mut deletes, mut paid, mut more) = (Vec::new(), Vec::new(), Vec::new());
    for f in changed["files"].as_array().into_iter().flatten().filter(|f| f["operation"] != "delete") {
        let path = f["path"].as_str().unwrap_or("");
        if !is_code(path) || is_test(path, tests) { continue; }
        let scanned = |files: &Files| text_of(files, path).and_then(|s| scan(path, s)).unwrap_or_default();
        let (before, after) = (scanned(base), scanned(candidate));
        for (call, n) in &after.deletes { if *n > before.deletes.get(call).copied().unwrap_or(0) { deletes.push(format!("{path}: {call}")); } }
        for call in before.paid.keys().chain(after.paid.keys()).collect::<BTreeSet<_>>() {
            let (o, n) = (before.paid.get(call).copied().unwrap_or(0), after.paid.get(call).copied().unwrap_or(0));
            if o != n { paid.push(json!({ "path": path, "call": call, "old": o, "new": n })); }
            if n > o { more.push(format!("{path}: {call}, {o} before, {n} now")); }
        }
    }
    let mut hosts: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (side, counts) in runs.counts.iter().enumerate() {
        for (key, n) in counts {
            let Some(host) = key.strip_prefix("net:").and_then(|h| h.rsplit_once(':')).map(|(h, _)| h).filter(|h| paid_host(h)) else { continue };
            let e = hosts.entry(host.to_string()).or_default();
            if side == 0 { e.0 += n } else { e.1 += n }
        }
    }
    let hosts: Vec<(String, u64, u64)> = hosts.into_iter().filter(|(_, (o, n))| o != n).map(|(h, (o, n))| (h, o, n)).collect();
    for (h, o, n) in hosts.iter().filter(|(_, o, n)| n > o) { more.push(format!("{h}: {o} before, {n} now")); }
    let (mut opened, mut moved, mut access) = (Vec::new(), Vec::new(), Value::Null);
    if plan["app"].is_object() {
        let app = |run: &Value| run["app"].clone();
        let (pa, pb, qa, qb) = (app(oa), app(ob), app(na), app(nb));
        let ready = |x: &Value| x["ready"] == true;
        let mut checked = 0u64;
        if ready(&pa) && ready(&pb) && !(ready(&qa) && ready(&qb)) {
            crashes.push(format!("the app: answered before, now {}", qa["error"].as_str().or(qb["error"].as_str()).unwrap_or("does not answer")));
        } else if ready(&pa) && ready(&pb) {
            let keys: BTreeSet<String> = [&pa, &qa].iter().flat_map(|x| x["results"].as_object().into_iter().flatten().map(|(k, _)| k.clone())).collect();
            for key in keys {
                let get = |x: &Value| x["results"][key.as_str()].as_str().map(str::to_string);
                let (Some(a), Some(b), Some(c), Some(d)) = (get(&pa), get(&pb), get(&qa), get(&qb)) else { continue };
                checked += 1;
                if a != b { continue; }
                let now: Vec<String> = [c, d].into_iter().filter(|x| *x != a).collect();
                let Some(changed_to) = now.first() else { continue };
                let code = |s: &str| s.split(' ').next().and_then(|x| x.parse::<u16>().ok());
                let refused = code(&a).is_some_and(|x| matches!(x, 401 | 403 | 404 | 405) || (300..400).contains(&x));
                match now.iter().find(|x| code(x).is_some_and(|x| (200..300).contains(&x))) {
                    Some(open) if refused => opened.push(format!("{key}: {a} before, {open} now")),
                    _ => moved.push(format!("{key}: {a} before, {changed_to} now")),
                }
            }
        }
        access = json!({ "checked": checked, "opened": first(&opened), "changed": first(&moved) });
    }
    let mut grouped: BTreeMap<String, (Vec<String>, bool)> = BTreeMap::new();
    for r in plan["removals"].as_array().into_iter().flatten() {
        let (start, count) = (r["start"].as_u64().unwrap_or(0), r["count"].as_u64().unwrap_or(0));
        let e = grouped.entry(r["path"].as_str().unwrap_or("").to_string()).or_default();
        e.1 |= r["file"] == true;
        e.0.push(if count <= 1 { start.to_string() } else { format!("{start}-{}", start + count - 1) });
    }
    let removals: Vec<Value> = grouped.into_iter().map(|(path, (lines, file))| {
        let (mut changed_calls, mut seen) = (0u64, Vec::new());
        if let Some(gone) = &runs.removed {
            let tuples: Vec<&str> = plan["modules"].as_array().into_iter().flatten().find(|m| m["path"] == path.as_str()).and_then(|m| m["tuples"].as_array()).into_iter().flatten().filter_map(Value::as_str).collect();
            let (x, y, z) = (module(oa, &path), module(ob, &path), module(gone, &path));
            for (key, e) in x["exports"].as_object().into_iter().flatten() {
                for (k, tuple) in tuples.iter().enumerate() {
                    let at = |m: &Value| m["calls"][k].as_str().map(str::to_string);
                    let (Some(a), Some(b), Some(c)) = (at(e), at(&y["exports"][key.as_str()]), at(&z["exports"][key.as_str()])) else { continue };
                    if a == b && a != c {
                        changed_calls += 1;
                        if seen.len() < 3 { seen.push(clip(&format!("{path}#{key}({tuple}): {} with it, {} without it", does(&a), does(&c)), 240)); }
                    }
                }
            }
        }
        json!({ "path": path, "lines": if file { "file".to_string() } else { lines.join(", ") }, "file": file, "changed": changed_calls, "examples": seen })
    }).collect();
    let mode = if delta.is_some() { "delta" } else { "report" };
    let evidence = json!({
        "policy": "kern-compare/v1", "mode": mode, "intent": delta.map(|d| d.intent.clone()), "network": "off",
        "probed": { "modules": modules, "functions": functions, "calls": calls, "not_loadable": first(&unloadable) },
        "same": same, "different": different, "noisy": noisy, "skipped": skipped, "stated": stated,
        "differences": differences, "crashes": first(&crashes), "unanswered": first(&unanswered), "surprises": first(&surprises), "removed": first(&removed), "examples": examples,
        "speed": speed.into_iter().take(KEEP).collect::<Vec<_>>(), "deletes": first(&deletes), "access": access, "removals": removals,
        "cost": { "calls": paid.into_iter().take(KEEP).collect::<Vec<_>>(), "hosts": hosts.iter().take(KEEP).map(|(h, o, n)| json!({ "host": h, "old": o, "new": n })).collect::<Vec<_>>() },
    });
    let enforce = delta.is_some();
    let block = if !crashes.is_empty() { Some(format!("New crash where the old code worked: {}. The new code must handle every input the old code handled", quote(&crashes))) }
        else if !unanswered.is_empty() { Some(format!("The new code did not answer calls the old code answered: {}. The new code must answer every call the old code answered, in about the same time", quote(&unanswered))) }
        else if !removed.is_empty() { Some(format!("Removed without the task stating it: {}. Restore it; deleting files or exports needs the task's delta to list them under removes", quote(&removed))) }
        else if !deletes.is_empty() && !allow("delete") { Some(format!("New code deletes files: {}. Code may not newly delete or truncate files unless the task's delta allows delete", quote(&deletes))) }
        else if !opened.is_empty() && !allow("access") { Some(format!("Access widened: {}. A route that refused a user before must still refuse them", quote(&opened))) }
        else if enforce && !failing.is_empty() { Some(format!("The task's examples do not hold: {}. The new code must give the answers the task states", quote(&failing))) }
        else if enforce && !surprises.is_empty() { Some(format!("Behaviour changed outside the task's stated change: {}. Everything the task did not state must behave exactly as before", quote(&surprises))) }
        else if enforce && !moved.is_empty() && !allow("access") { Some(format!("Access changed outside the task's stated change: {}. Routes must answer as before", quote(&moved))) }
        else if enforce && !slow.is_empty() && !allow("speed") { Some(format!("Slower than the old code: {}. The task did not allow slower code", quote(&slow))) }
        else if enforce && !more.is_empty() && !allow("cost") { Some(format!("More paid API calls than the old code: {}. The task did not allow more paid API calls", quote(&more))) }
        else { None };
    (evidence, block)
}

pub fn attested(envelope: &Value, evidence: &Value) -> bool {
    let (Some(k), Ok(d)) = (crate::attest::claimed(envelope), digest(evidence)) else { return false };
    k["checks"].as_array().into_iter().flatten().any(|c| c["id"] == CHECK && c["evidence_sha256"] == d.as_str())
}
fn code(s: &str) -> String {
    let flat: String = s.replace('`', "'").replace(['\r', '\n'], " ").chars().take(240).collect();
    format!("`{flat}`")
}
pub fn summary(e: &Value) -> Vec<String> { lines(e, &|s: &str| s.to_string()) }
pub fn markdown(e: &Value) -> String {
    let mut out = String::new();
    for l in lines(e, &code) {
        let item = format!("- {l}\n");
        if out.len() + item.len() > 8000 { out.push_str("- More in the verifier's comparison evidence\n"); break; }
        out.push_str(&item);
    }
    out
}
fn lines(e: &Value, wrap: &dyn Fn(&str) -> String) -> Vec<String> {
    let n = |k: &str| e[k].as_u64().unwrap_or(0);
    let mut out = Vec::new();
    if let Some(intent) = e["intent"].as_str() { out.push(format!("The task states: {}", wrap(intent))); }
    let p = &e["probed"];
    let mut line = format!("Kern called {} functions in {} modules {} times on the old and on the new code, with the network off: {} identical",
        p["functions"].as_u64().unwrap_or(0), p["modules"].as_u64().unwrap_or(0), p["calls"].as_u64().unwrap_or(0), n("same"));
    if n("stated") > 0 { line += &format!(", {} changed as the task states", n("stated")); }
    if n("noisy") > 0 { line += &format!(", {} varied from run to run and were set aside", n("noisy")); }
    out.push(line);
    let list = |k: &str| e[k].as_array().into_iter().flatten().filter_map(Value::as_str).take(3).map(str::to_string).collect::<Vec<_>>();
    for (k, label) in [("crashes", "New crash"), ("unanswered", "Not answered by the new code"), ("surprises", "Changed without being stated"), ("removed", "Removed without being stated"), ("deletes", "New code deletes files")] {
        if k == "surprises" && e["mode"] == "report" { continue; }
        for item in list(k) { out.push(format!("{label}: {}", wrap(&item))); }
    }
    if e["mode"] == "report" {
        for d in e["differences"].as_array().into_iter().flatten().take(5) {
            out.push(format!("Changed ({}): {}", d["kind"].as_str().unwrap_or(""), wrap(&format!("{}({}): {}, now {}", d["function"].as_str().unwrap_or(""), d["args"].as_str().unwrap_or(""), d["old"].as_str().unwrap_or(""), d["new"].as_str().unwrap_or("")))));
        }
    }
    let examples = e["examples"].as_array().map_or(0, Vec::len);
    if examples > 0 { out.push(format!("{} of the task's {examples} examples hold", e["examples"].as_array().into_iter().flatten().filter(|x| x["ok"] == true).count())); }
    for r in e["removals"].as_array().into_iter().flatten() {
        let path = wrap(r["path"].as_str().unwrap_or(""));
        let lines = r["lines"].as_str().unwrap_or("");
        let at = if r["file"] == true { format!("Removed file {path}") } else if lines.contains([',', '-']) { format!("Removed code at {path} lines {lines}") } else { format!("Removed code at {path} line {lines}") };
        let changed = r["changed"].as_u64().unwrap_or(0);
        let effect = if changed == 0 { "no call Kern made behaves differently without it".to_string() } else { format!("{changed} calls behave differently without it") };
        let example = r["examples"][0].as_str().map_or(String::new(), |x| format!(", for example {}", wrap(x)));
        out.push(format!("{at}: {effect}{example}"));
    }
    for s in e["speed"].as_array().into_iter().flatten() {
        out.push(format!("Slower: {} {} ms before, {} ms now", wrap(s["function"].as_str().unwrap_or("")), ms(s["old_us"].as_u64().unwrap_or(0)), ms(s["new_us"].as_u64().unwrap_or(0))));
    }
    for c in e["cost"]["calls"].as_array().into_iter().flatten() {
        out.push(format!("Paid API calls in {}: {} {} before, {} now", wrap(c["path"].as_str().unwrap_or("")), wrap(c["call"].as_str().unwrap_or("")), c["old"], c["new"]));
    }
    for h in e["cost"]["hosts"].as_array().into_iter().flatten() { out.push(format!("Calls to {}: {} before, {} now", wrap(h["host"].as_str().unwrap_or("")), h["old"], h["new"])); }
    if e["access"].is_object() {
        let a = &e["access"];
        out.push(format!("The app answered {} route checks: {} opened, {} changed", a["checked"], a["opened"].as_array().map_or(0, Vec::len), a["changed"].as_array().map_or(0, Vec::len)));
    }
    if let Some(gap) = p["not_loadable"].as_array().filter(|g| !g.is_empty()) {
        out.push(format!("Not checked: {} modules could not be loaded on the old code, for example {}", gap.len(), wrap(gap[0].as_str().unwrap_or(""))));
    }
    out
}
