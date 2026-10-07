use crate::capability::is_code;
use crate::git::Files;
use crate::policy::{Kind, Runner, Tests};
use crate::util::*;
use crate::{deny, ensure};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::SourceType;
use oxc_syntax::operator::UnaryOperator;
use oxc_syntax::symbol::SymbolId;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub use crate::policy::HONEST_FIX as CHECK;

const HELPERS: [&str; 7] = ["fixtures", "__fixtures__", "helpers", "__helpers__", "support", "__mocks__", "mocks"];
const RUNNER_CONFIG: [&str; 16] = [".mocharc", "mocha.opts", "jest.config", "jest.setup", "jest-setup", "setuptests", "vitest.config", "vitest.workspace", "vitest.setup", "vite.config", "ava.config", ".taprc", "karma.conf", "playwright.config", ".babelrc", "babel.config"];
const SUITES: [&str; 6] = ["describe", "it", "test", "context", "suite", "specify"];
const SELECTORS: [&str; 3] = ["only", "skip", "todo"];
const RENAMED: [&str; 8] = ["xit", "xdescribe", "xtest", "xcontext", "xspecify", "fit", "fdescribe", "ftest"];
const TEST_GLOBALS: [&str; 29] = ["describe", "it", "test", "expect", "context", "suite", "specify", "before", "after", "beforeEach", "afterEach", "beforeAll", "afterAll", "setup", "teardown",
    "suiteSetup", "suiteTeardown", "jest", "vi", "xit", "xdescribe", "xtest", "xcontext", "xspecify", "fit", "fdescribe", "ftest", "mocha", "chai"];
const ASSERTIONS: [&str; 13] = ["assert", "node:assert", "assert/strict", "node:assert/strict", "chai", "expect", "should", "power-assert", "uvu/assert", "node:test", "vitest", "@jest/globals", "mocha"];
const MUTATORS: [(&str, &str); 8] = [("Object", "defineProperty"), ("Object", "defineProperties"), ("Object", "assign"), ("Object", "setPrototypeOf"), ("Reflect", "set"), ("Reflect", "defineProperty"), ("Reflect", "deleteProperty"), ("Reflect", "setPrototypeOf")];
const SNIFFS: [&str; 6] = ["JEST_WORKER_ID", "VITEST", "VITEST_WORKER_ID", "VITEST_POOL_ID", "NODE_TEST_CONTEXT", "MOCHA_WORKER_ID"];
const GLOBALS: [&str; 4] = ["globalThis", "global", "window", "self"];

/// The command the verifier runs: the contract's test command with the one reporter Kern reads.
pub fn command(tests: &Tests) -> Vec<String> {
    let mut c = tests.command.clone();
    match tests.runner {
        Runner::Mocha => c.extend(["--reporter".to_string(), "json".to_string()]),
        Runner::Jest => c.push("--json".into()),
        // The verifier's workspace is read-only, and vitest exits with failure when it cannot write its cache into node_modules.
        Runner::Vitest => c.extend(["--reporter=json".to_string(), "--no-cache".to_string()]),
        Runner::Node => { let at = c.iter().position(|a| a == "--test").map_or(c.len(), |i| i + 1); c.insert(at, "--test-reporter=tap".into()); }
    }
    c
}

fn unwrap<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> {
    match e {
        Expression::ParenthesizedExpression(x) => unwrap(&x.expression),
        Expression::TSAsExpression(x) => unwrap(&x.expression),
        Expression::TSSatisfiesExpression(x) => unwrap(&x.expression),
        Expression::TSNonNullExpression(x) => unwrap(&x.expression),
        Expression::TSTypeAssertion(x) => unwrap(&x.expression),
        Expression::AwaitExpression(x) => unwrap(&x.argument),
        _ => e,
    }
}
fn root<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> {
    match unwrap(e) {
        Expression::StaticMemberExpression(m) => root(&m.object),
        Expression::ComputedMemberExpression(m) => root(&m.object),
        other => other,
    }
}
fn literal(e: &Expression) -> Option<String> {
    match unwrap(e) {
        Expression::StringLiteral(s) => Some(s.value.to_string()),
        Expression::TemplateLiteral(t) => t.single_quasi().map(|q| q.to_string()),
        _ => None,
    }
}

enum Write { Module(String), Symbol(SymbolId) }
/// Finds what a changed file does to the test run. In a new test file: markers that skip or focus tests. Elsewhere: use of test-runner
/// globals, changes to an assertion library, and checks of whether tests are running.
struct Scan<'s> { scoping: &'s Scoping, test: bool, found: BTreeSet<String>, bound: BTreeMap<SymbolId, String>, writes: Vec<Write> }
impl Scan<'_> {
    fn global(&self, id: &IdentifierReference) -> bool { id.reference_id.get().is_none_or(|r| self.scoping.get_reference(r).symbol_id().is_none()) }
    fn module(&self, e: &Expression) -> Option<String> {
        let source = match unwrap(e) {
            Expression::CallExpression(c) if matches!(&c.callee, Expression::Identifier(id) if id.name == "require" && self.global(id)) && c.arguments.len() == 1 => match &c.arguments[0] {
                Argument::StringLiteral(s) => s.value.to_string(),
                _ => return None,
            },
            Expression::ImportExpression(i) => literal(&i.source)?,
            _ => return None,
        };
        ASSERTIONS.contains(&source.as_str()).then_some(source)
    }
    fn write(&mut self, target: &Expression) {
        let r = root(target);
        if let Some(m) = self.module(r) { self.writes.push(Write::Module(m)); }
        else if let Expression::Identifier(id) = r && let Some(s) = id.reference_id.get().and_then(|r| self.scoping.get_reference(r).symbol_id()) { self.writes.push(Write::Symbol(s)); }
    }
    fn member(&mut self, object: &Expression, prop: &str) {
        if !self.test && SNIFFS.contains(&prop) { self.found.insert(prop.to_string()); }
        let Expression::Identifier(id) = unwrap(object) else { return };
        let name = id.name.as_str();
        if self.test { if SUITES.contains(&name) && SELECTORS.contains(&prop) { self.found.insert(format!("{name}.{prop}")); } return; }
        if self.global(id) && GLOBALS.contains(&name) && TEST_GLOBALS.contains(&prop) { self.found.insert(format!("{name}.{prop}")); }
    }
}
impl<'a> Visit<'a> for Scan<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if !self.test && !it.import_kind.is_type() && ASSERTIONS.contains(&it.source.value.as_str()) {
            for s in it.specifiers.iter().flatten() {
                let local = match s { ImportDeclarationSpecifier::ImportSpecifier(x) => &x.local, ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => &x.local, ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => &x.local };
                if let Some(id) = local.symbol_id.get() { self.bound.insert(id, it.source.value.to_string()); }
            }
        }
        walk::walk_import_declaration(self, it);
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if !self.test && let (BindingPattern::BindingIdentifier(b), Some(init)) = (&it.id, &it.init) && let (Some(m), Some(id)) = (self.module(root(init)), b.symbol_id.get()) {
            self.bound.insert(id, m);
        }
        walk::walk_variable_declarator(self, it);
    }
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if !self.test {
            match &it.left {
                AssignmentTarget::StaticMemberExpression(m) => self.write(&m.object),
                AssignmentTarget::ComputedMemberExpression(m) => self.write(&m.object),
                _ => {}
            }
        }
        walk::walk_assignment_expression(self, it);
    }
    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        if !self.test {
            match &it.argument {
                SimpleAssignmentTarget::StaticMemberExpression(m) => self.write(&m.object),
                SimpleAssignmentTarget::ComputedMemberExpression(m) => self.write(&m.object),
                _ => {}
            }
        }
        walk::walk_update_expression(self, it);
    }
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if !self.test && it.operator == UnaryOperator::Delete {
            match unwrap(&it.argument) {
                Expression::StaticMemberExpression(m) => self.write(&m.object),
                Expression::ComputedMemberExpression(m) => self.write(&m.object),
                _ => {}
            }
        }
        walk::walk_unary_expression(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.test {
            let suite = match unwrap(&it.callee) {
                Expression::Identifier(id) => {
                    if RENAMED.contains(&id.name.as_str()) { self.found.insert(id.name.to_string()); }
                    SUITES.contains(&id.name.as_str())
                }
                Expression::StaticMemberExpression(m) => matches!(unwrap(&m.object), Expression::Identifier(id) if SUITES.contains(&id.name.as_str())),
                _ => false,
            };
            for arg in it.arguments.iter().filter(|_| suite) {
                let Argument::ObjectExpression(options) = arg else { continue };
                for property in &options.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = property else { continue };
                    let Some(key) = p.key.static_name() else { continue };
                    if SELECTORS.contains(&key.as_ref()) && !matches!(&p.value, Expression::BooleanLiteral(b) if !b.value) { self.found.insert(format!("the {key} option")); }
                }
            }
        } else if let Expression::StaticMemberExpression(m) = unwrap(&it.callee)
            && let Expression::Identifier(o) = unwrap(&m.object)
            && self.global(o) && MUTATORS.contains(&(o.name.as_str(), m.property.name.as_str()))
            && let Some(first) = it.arguments.first().and_then(Argument::as_expression) {
            self.write(first);
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        self.member(&it.object, it.property.name.as_str());
        walk::walk_static_member_expression(self, it);
    }
    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Some(key) = literal(&it.expression) { self.member(&it.object, &key); }
        walk::walk_computed_member_expression(self, it);
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        if !self.test && TEST_GLOBALS.contains(&it.name.as_str()) && self.global(it) { self.found.insert(format!("the test global {}", it.name)); }
    }
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if !self.test && SNIFFS.contains(&it.value.as_str()) { self.found.insert(it.value.to_string()); }
    }
    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        if !self.test && let Some(q) = it.single_quasi() && SNIFFS.contains(&q.as_str()) { self.found.insert(q.to_string()); }
        walk::walk_template_literal(self, it);
    }
}
fn markers(path: &str, source: &str, test: bool) -> R<BTreeSet<String>> {
    let st = SourceType::from_path(path).map_err(|_| Denied(format!("Unsupported code file: {path}")))?;
    let attempts = if path.ends_with(".js") || path.ends_with(".jsx") { vec![st, st.with_module(false)] } else { vec![st] };
    for st in attempts {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { continue; }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let mut scan = Scan { scoping: semantic.scoping(), test, found: BTreeSet::new(), bound: BTreeMap::new(), writes: Vec::new() };
        scan.visit_program(&parsed.program);
        for w in std::mem::take(&mut scan.writes) {
            let module = match w { Write::Module(m) => Some(m), Write::Symbol(s) => scan.bound.get(&s).cloned() };
            if let Some(m) = module { scan.found.insert(format!("a change to {m}")); }
        }
        return Ok(scan.found);
    }
    deny!("Changed code cannot be parsed for the test proof: {path}. Fix its syntax")
}
fn text_of<'f>(files: &'f Files, path: &str) -> Option<&'f str> { files.get(path).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()) }
fn helper(path: &str) -> bool { path.split('/').map(str::to_ascii_lowercase).any(|d| HELPERS.contains(&d.as_str())) }

/// Checks a change before anything runs: existing tests are locked, the runner's settings stay as they are, new test files do not skip or
/// focus tests, and code outside the tests does not reach into the test runner.
pub fn screen(base: &Files, candidate: &Files, changed: &Value, tests: &Tests) -> R<Value> {
    let mut new_tests = Vec::new();
    for f in changed["files"].as_array().into_iter().flatten() {
        let (path, op) = (f["path"].as_str().unwrap_or(""), f["operation"].as_str().unwrap_or(""));
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        ensure!(!RUNNER_CONFIG.iter().any(|c| name.starts_with(c)), "Test runner configuration cannot change under the test proof: {path}. It decides which tests run and how they check results; leave it unchanged");
        let inside = allowed(&tests.paths, path);
        if inside {
            ensure!(op != "delete", "Existing test file deleted: {path}. The test proof locks existing tests so they keep checking the old behaviour; leave existing tests in place");
            ensure!(op == "add", "Existing test file changed: {path}. The test proof locks existing tests so they keep checking the old behaviour; put new tests in a new file and leave existing ones unchanged");
        }
        if op == "delete" || !is_code(path) { continue; }
        let source = text_of(candidate, path).ok_or_else(|| Denied(format!("Changed file cannot be read as text: {path}")))?;
        let found = markers(path, source, inside)?;
        if inside {
            if let Some(m) = found.first() { deny!("New test file skips or focuses tests: {path} uses {m}. Every test must run, so remove it") }
            if !helper(path) { new_tests.push(path.to_string()); }
            continue;
        }
        let before = text_of(base, path).and_then(|s| markers(path, s, false).ok()).unwrap_or_default();
        if let Some(m) = found.difference(&before).next() {
            deny!("Code outside the tests reaches into the test runner: {path} uses {m}. Code may not change how tests run or what they check, or check whether tests are running; remove it")
        }
    }
    ensure!(tests.kind != Kind::Bugfix || !new_tests.is_empty(), "A bug fix must add a test that fails before the fix and passes after it: add a new test file under {}", tests.paths.join(", "));
    Ok(json!({ "kind": tests.kind.name(), "runner": tests.runner.name(), "new_test_files": new_tests, "locked_test_files": base.keys().filter(|k| allowed(&tests.paths, k)).count() }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count { pub passed: u32, pub failed: u32, pub skipped: u32 }
impl Count {
    fn total(self) -> u32 { self.passed + self.failed + self.skipped }
    fn ran(self) -> u32 { self.passed + self.failed }
}
/// One run's results as the test runner reported them: how often each test passed, failed or was skipped, and failures outside any test
/// (a test file that could not load, a failing hook).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report { pub tests: BTreeMap<String, Count>, pub problems: Vec<String> }
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status { Passed, Failed, Skipped }
impl Report {
    fn add(&mut self, key: String, status: Status) {
        let c = self.tests.entry(key).or_default();
        match status { Status::Passed => c.passed += 1, Status::Failed => c.failed += 1, Status::Skipped => c.skipped += 1 }
    }
    fn count(&self, key: &str) -> Count { self.tests.get(key).copied().unwrap_or_default() }
    fn sum(&self) -> Count { self.tests.values().fold(Count::default(), |a, c| Count { passed: a.passed + c.passed, failed: a.failed + c.failed, skipped: a.skipped + c.skipped }) }
}
fn rel(path: &str) -> String { path.strip_prefix("/workspace/").unwrap_or(path).to_string() }

/// Reads the one report the runner printed. Anything else on the output, such as text or JSON that a test prints, is ignored, and a
/// second report is refused, so a test cannot pass off results of its own.
pub fn read_report(runner: Runner, out: &[u8]) -> Result<Report, String> {
    if runner == Runner::Node { return tap(out); }
    let (mut found, mut at, mut attempts) = (Vec::new(), 0usize, 0u32);
    while at < out.len() {
        if out[at] == b'{' && (at == 0 || out[at - 1] == b'\n') {
            attempts += 1;
            if attempts > 10_000 { return Err("the output holds too many lines that look like JSON".into()); }
            let mut values = serde_json::Deserializer::from_slice(&out[at..]).into_iter::<Value>();
            if let Some(Ok(v)) = values.next() {
                if let Some(r) = if runner == Runner::Mocha { mocha(&v) } else { jest(&v) } { found.push(r); }
                at += values.byte_offset();
                continue;
            }
        }
        match out[at..].iter().position(|&b| b == b'\n') { Some(i) => at += i + 1, None => break }
    }
    match found.len() {
        0 => Err(format!("the output contains no {} report", runner.name())),
        1 => Ok(found.remove(0)),
        _ => Err(format!("the output contains more than one {} report; tests must not print test reports", runner.name())),
    }
}
fn mocha(v: &Value) -> Option<Report> {
    v.get("stats")?.as_object()?;
    let keys = |field: &str| -> Option<Vec<String>> {
        v.get(field)?.as_array()?.iter().map(|t| {
            let title = t.get("fullTitle")?.as_str()?;
            Some(match t.get("file").and_then(Value::as_str) { Some(f) => format!("{} > {title}", rel(f)), None => title.to_string() })
        }).collect()
    };
    let (tests, passes, failures, pending) = (keys("tests")?, keys("passes")?, keys("failures")?, keys("pending")?);
    let known: BTreeSet<&String> = tests.iter().collect();
    let mut r = Report::default();
    for k in passes { r.add(k, Status::Passed); }
    for k in pending { r.add(k, Status::Skipped); }
    for k in failures { if known.contains(&k) { r.add(k, Status::Failed) } else { r.problems.push(k) } }
    Some(r)
}
fn jest(v: &Value) -> Option<Report> {
    v.get("numTotalTests")?.as_u64()?;
    let mut r = Report::default();
    for file in v.get("testResults")?.as_array()? {
        let name = rel(file.get("name")?.as_str()?);
        let mut failed = false;
        for a in file.get("assertionResults")?.as_array()? {
            let mut path: Vec<&str> = a.get("ancestorTitles")?.as_array()?.iter().map(Value::as_str).collect::<Option<_>>()?;
            path.push(a.get("title")?.as_str()?);
            let status = match a.get("status")?.as_str()? { "passed" => Status::Passed, "pending" | "skipped" | "todo" | "disabled" => Status::Skipped, _ => Status::Failed };
            failed |= status == Status::Failed;
            r.add(format!("{name} > {}", path.join(" > ")), status);
        }
        if file.get("status").and_then(Value::as_str) == Some("failed") && !failed { r.problems.push(name); }
    }
    Some(r)
}
struct Point { depth: usize, ok: bool, directive: Option<Status>, name: String, suite: bool }
fn directive(rest: &str) -> (String, Option<Status>) {
    let mut escaped = false;
    for (i, c) in rest.char_indices() {
        if escaped { escaped = false; continue; }
        if c == '\\' { escaped = true; }
        if c == '#' {
            let d = rest[i + 1..].trim_start().to_ascii_uppercase();
            return (rest[..i].trim_end().to_string(), (d.starts_with("SKIP") || d.starts_with("TODO")).then_some(Status::Skipped));
        }
    }
    (rest.trim_end().to_string(), None)
}
fn tap(out: &[u8]) -> Result<Report, String> {
    let text = String::from_utf8_lossy(out);
    let lines: Vec<&str> = text.lines().collect();
    let headers: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.starts_with("TAP version ")).map(|(i, _)| i).collect();
    let start = match headers.as_slice() {
        [] => return Err("the output contains no TAP report".into()),
        [one] => *one,
        _ => return Err("the output contains more than one TAP report; tests must not print test reports".into()),
    };
    let incomplete = || "the TAP report is incomplete: the run did not finish".to_string();
    let (mut points, mut plan, mut i) = (Vec::new(), None, start + 1);
    while i < lines.len() {
        let line = lines[i];
        let body = line.trim_start_matches(' ');
        let indent = line.len() - body.len();
        i += 1;
        if !indent.is_multiple_of(4) { continue; }
        if indent == 0 && body.starts_with("Bail out!") { return Err("the test run bailed out".into()); }
        if indent == 0 && let Some(n) = body.strip_prefix("1..") { plan = n.split_whitespace().next().and_then(|n| n.parse::<usize>().ok()); continue; }
        let (ok, rest) = match (body.strip_prefix("not ok"), body.strip_prefix("ok")) { (Some(r), _) => (false, r), (None, Some(r)) => (true, r), _ => continue };
        if !rest.is_empty() && !rest.starts_with(' ') { continue; }
        let rest = rest.trim_start().trim_start_matches(|c: char| c.is_ascii_digit());
        let (name, directive) = directive(rest.strip_prefix(" - ").or_else(|| rest.strip_prefix(' ')).unwrap_or(rest));
        let pad = " ".repeat(indent + 2);
        let mut suite = false;
        if lines.get(i).is_some_and(|l| *l == format!("{pad}---")) {
            let (end, kind) = (format!("{pad}..."), format!("{pad}type: 'suite'"));
            i += 1;
            while i < lines.len() && lines[i] != end { suite |= lines[i] == kind; i += 1; }
            i += 1;
        }
        points.push(Point { depth: indent / 4, ok, directive, name, suite });
    }
    if plan != Some(points.iter().filter(|p| p.depth == 0).count()) { return Err(incomplete()); }
    // Subtests come before the test or suite that holds them, one level deeper.
    let mut levels: Vec<Vec<(String, Option<Status>)>> = vec![Vec::new()];
    for p in points {
        while levels.len() < p.depth + 2 { levels.push(Vec::new()); }
        let children = std::mem::take(&mut levels[p.depth + 1]);
        let status = p.directive.unwrap_or(if p.ok { Status::Passed } else { Status::Failed });
        if children.is_empty() && !p.suite {
            levels[p.depth].push((p.name, Some(status)));
            continue;
        }
        let child_failed = children.iter().any(|(_, s)| !matches!(s, Some(Status::Passed | Status::Skipped)));
        levels[p.depth].extend(children.into_iter().map(|(k, s)| (format!("{} > {k}", p.name), s)));
        if status == Status::Failed && !child_failed { levels[p.depth].push((p.name, None)); }
    }
    if levels.iter().skip(1).any(|l| !l.is_empty()) { return Err(incomplete()); }
    let mut report = Report::default();
    for (k, s) in levels.swap_remove(0) { match s { Some(s) => report.add(k, s), None => report.problems.push(k) } }
    Ok(report)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run { Base, New, Old }
impl Run {
    pub fn name(self) -> &'static str { match self { Run::Base => "base", Run::New => "new", Run::Old => "old" } }
    fn code(self) -> &'static str { match self { Run::Base => "the old code", Run::New => "the new code", Run::Old => "the old code with the new tests added" } }
}
pub struct Output { pub stdout: Vec<u8>, pub exit_code: i64 }
fn quote<S: AsRef<str>>(keys: &[S]) -> String {
    let shown: Vec<String> = keys.iter().take(5).map(|k| {
        let k = k.as_ref();
        let short: String = k.chars().take(120).collect();
        serde_json::to_string(&if short.len() < k.len() { format!("{short}...") } else { short }).unwrap_or_default()
    }).collect();
    if keys.len() > 5 { format!("{} and {} more", shown.join(", "), keys.len() - 5) } else { shown.join(", ") }
}
fn summary(r: &Report) -> Value {
    let s = r.sum();
    json!({ "tests": s.total(), "passed": s.passed, "failed": s.failed, "skipped": s.skipped, "failed_outside_tests": r.problems.len() })
}

/// Proves the change honest from the runner's own reports, at most three runs: the existing suite on the old code, everything on the new
/// code, and for a bug fix everything on the old code with the new tests added. Every test that ran before must still run and pass, nothing
/// may fail or be newly skipped, and a bug fix needs a new test that the runner reports failing on the old code.
pub fn prove(tests: &Tests, base: &Files, candidate: &Files, changed: &Value, run: &mut dyn FnMut(Run) -> R<Output>) -> R<Value> {
    let screened = match changed.get("tests") { Some(t) if t.is_object() => t.clone(), _ => screen(base, candidate, changed, tests)? };
    let mut results = |which: Run| -> R<(Report, i64)> {
        let out = run(which).map_err(|e| Denied(format!("Kern could not run the test suite on {}: {}", which.code(), e.0)))?;
        let report = read_report(tests.runner, &out.stdout).map_err(|why| Denied(match which {
            Run::Base => format!("Kern could not read the test results of the old code: {why}. Check tests.runner and tests.command in the task contract"),
            Run::New => format!("The test run on the new code gave no usable results: {why}. The test command must run to completion; often a test file cannot load or the run crashed"),
            Run::Old => format!("The test run on the old code with the new tests added gave no usable results: {why}. A bug-fix test must load on the old code and fail inside a test, so it cannot depend on files the fix adds"),
        }))?;
        Ok((report, out.exit_code))
    };
    let (before, _) = results(Run::Base)?;
    let (after, exit) = results(Run::New)?;
    let failing: Vec<&String> = after.tests.iter().filter(|(_, c)| c.failed > 0).map(|(k, _)| k).chain(&after.problems).collect();
    if !failing.is_empty() {
        if failing.iter().all(|k| before.count(k).failed > 0 || before.problems.contains(k)) {
            deny!("The test suite already fails before the change: {}. The test proof needs a suite that passes; fix it on the base branch first", quote(&failing))
        }
        deny!("The test suite fails on the new code: {}. Every test must pass after the change; fix the code, not the tests", quote(&failing))
    }
    let stopped = before.tests.iter().filter(|(k, c)| { let n = after.count(k); n.total() < c.total() || n.passed < c.ran() }).map(|(k, _)| k);
    let skipped = after.tests.iter().filter(|(k, c)| c.skipped > before.count(k).skipped).map(|(k, _)| k);
    let missing: Vec<&String> = stopped.chain(skipped).collect::<BTreeSet<_>>().into_iter().collect();
    ensure!(missing.is_empty(), "Tests did not run on the new code: {}. Every test that ran before must still run, and every new test must run; do not skip, remove or rename tests", quote(&missing));
    ensure!(exit == 0, "The test command failed on the new code although no test failed. Something outside the tests failed, such as a crash after the run, an unhandled error or a coverage threshold; fix it so the command succeeds");
    ensure!(after.sum().passed > 0, "The test suite ran no tests on the new code, so it proves nothing. Check tests.runner and tests.command in the task contract");
    let new_tests: Vec<&String> = after.tests.iter().filter(|(k, c)| c.total() > before.count(k).total()).map(|(k, _)| k).collect();
    let mut evidence = json!({
        "kind": tests.kind.name(), "runner": tests.runner.name(), "command": command(tests), "runs": { "base": summary(&before), "new": summary(&after) },
        "pass_to_pass": before.tests.values().filter(|c| c.passed > 0).count(), "new_tests_total": new_tests.len(),
        "new_test_files": screened["new_test_files"], "locked_test_files": screened["locked_test_files"],
    });
    if tests.kind != Kind::Bugfix {
        evidence["new_tests"] = json!(new_tests.iter().take(100).map(|k| json!({ "test": k, "old": "not run", "new": "passed" })).collect::<Vec<_>>());
        return Ok(evidence);
    }
    ensure!(!new_tests.is_empty(), "No new test ran in the test suite. A bug fix must add a test under {} that the test command runs, and that fails before the fix and passes after it", tests.paths.join(", "));
    let (old, _) = results(Run::Old)?;
    let outcome = |k: &str| {
        let (o, b) = (old.count(k), before.count(k));
        if o.failed > b.failed { "failed" } else if o.passed > b.passed { "passed" } else if o.skipped > b.skipped { "skipped" } else { "not run" }
    };
    let proven = new_tests.iter().filter(|k| outcome(k) == "failed").count();
    ensure!(proven > 0, "None of the new tests fails on the old code, so they do not show the bug: {}. Write a test that fails before the fix and passes after it; a test that never reaches the changed code (for example because it mocks it) proves nothing", quote(&new_tests));
    evidence["runs"]["old"] = summary(&old);
    evidence["new_tests"] = json!(new_tests.iter().take(100).map(|k| json!({ "test": k, "old": outcome(k), "new": "passed" })).collect::<Vec<_>>());
    evidence["fail_to_pass"] = json!(proven);
    Ok(evidence)
}
