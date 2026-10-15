use crate::capability::is_code;
use crate::git::Files;
use crate::policy::{Edits, Kind, Runner, Tests};
use crate::util::*;
use crate::{deny, ensure};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::GetSpan;
use oxc_syntax::operator::{BinaryOperator, UnaryOperator};
use oxc_syntax::symbol::SymbolId;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub use crate::policy::HONEST_FIX as CHECK;

const HELPERS: [&str; 7] = ["fixtures", "__fixtures__", "helpers", "__helpers__", "support", "__mocks__", "mocks"];
const RUNNER_CONFIG: [&str; 16] = [".mocharc", "mocha.opts", "jest.config", "jest.setup", "jest-setup", "setuptests", "vitest.config", "vitest.workspace", "vitest.setup", "vite.config", "ava.config", ".taprc", "karma.conf", "playwright.config", ".babelrc", "babel.config"];
const SUITES: [&str; 6] = ["describe", "it", "test", "context", "suite", "specify"];
const SELECTORS: [&str; 7] = ["only", "skip", "todo", "skipIf", "runIf", "fails", "failing"];
const FRAMEWORKS: [&str; 9] = ["vitest", "@jest/globals", "node:test", "mocha", "bun:test", "@playwright/test", "uvu", "ava", "tap"];
const RENAMED: [&str; 8] = ["xit", "xdescribe", "xtest", "xcontext", "xspecify", "fit", "fdescribe", "ftest"];
const TEST_GLOBALS: [&str; 29] = ["describe", "it", "test", "expect", "context", "suite", "specify", "before", "after", "beforeEach", "afterEach", "beforeAll", "afterAll", "setup", "teardown",
    "suiteSetup", "suiteTeardown", "jest", "vi", "xit", "xdescribe", "xtest", "xcontext", "xspecify", "fit", "fdescribe", "ftest", "mocha", "chai"];
const ASSERTIONS: [&str; 13] = ["assert", "node:assert", "assert/strict", "node:assert/strict", "chai", "expect", "should", "power-assert", "uvu/assert", "node:test", "vitest", "@jest/globals", "mocha"];
const MUTATORS: [(&str, &str); 8] = [("Object", "defineProperty"), ("Object", "defineProperties"), ("Object", "assign"), ("Object", "setPrototypeOf"), ("Reflect", "set"), ("Reflect", "defineProperty"), ("Reflect", "deleteProperty"), ("Reflect", "setPrototypeOf")];
const SNIFFS: [&str; 6] = ["JEST_WORKER_ID", "VITEST", "VITEST_WORKER_ID", "VITEST_POOL_ID", "NODE_TEST_CONTEXT", "MOCHA_WORKER_ID"];
const GLOBALS: [&str; 4] = ["globalThis", "global", "window", "self"];

pub fn command(tests: &Tests) -> Vec<String> {
    let mut c = tests.command.clone();
    match tests.runner {
        Runner::Mocha => c.extend(["--reporter".to_string(), "json".to_string()]),
        Runner::Jest => c.extend(["--ci".to_string(), "--testResultsProcessor=/kern/jest-report.cjs".to_string()]),
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
        Expression::TemplateLiteral(t) => {
            let mut out = String::new();
            for (i, q) in t.quasis.iter().enumerate() {
                out.push_str(q.value.cooked.as_ref()?.as_str());
                if let Some(e) = t.expressions.get(i) { out.push_str(&literal(e)?); }
            }
            Some(out)
        }
        Expression::BinaryExpression(b) if b.operator == BinaryOperator::Addition => Some(literal(&b.left)? + &literal(&b.right)?),
        _ => None,
    }
}
fn chain_selector(e: &Expression) -> Option<(String, String)> {
    let (mut props, mut at) = (Vec::new(), unwrap(e));
    loop {
        match at {
            Expression::StaticMemberExpression(m) => { props.push(m.property.name.to_string()); at = unwrap(&m.object); }
            Expression::ComputedMemberExpression(m) => { props.push(literal(&m.expression).unwrap_or_default()); at = unwrap(&m.object); }
            Expression::CallExpression(c) => at = unwrap(&c.callee),
            Expression::Identifier(id) => return props.into_iter().rev().find(|p| SELECTORS.contains(&p.as_str())).map(|p| (id.name.to_string(), p)),
            _ => return None,
        }
    }
}
fn test_shaped(args: &[Argument]) -> bool {
    args.first().and_then(Argument::as_expression).and_then(literal).is_some() && matches!(args.get(1), Some(Argument::ArrowFunctionExpression(_) | Argument::FunctionExpression(_)))
}
fn truthy(e: &Expression) -> bool {
    match unwrap(e) {
        Expression::BooleanLiteral(b) => b.value,
        Expression::NumericLiteral(n) => n.value != 0.0,
        Expression::StringLiteral(_) | Expression::TemplateLiteral(_) => literal(e).is_some_and(|s| !s.is_empty()),
        _ => false,
    }
}

enum Write { Module(String), Symbol(SymbolId) }
struct Scan<'s> {
    scoping: &'s Scoping, test: bool, found: Vec<String>, bound: BTreeMap<SymbolId, String>, writes: Vec<Write>,
    suites: BTreeMap<SymbolId, String>, namespaces: BTreeSet<SymbolId>,
}
impl Scan<'_> {
    fn global(&self, id: &IdentifierReference) -> bool { id.reference_id.get().is_none_or(|r| self.scoping.get_reference(r).symbol_id().is_none()) }
    fn symbol(&self, id: &IdentifierReference) -> Option<SymbolId> { id.reference_id.get().and_then(|r| self.scoping.get_reference(r).symbol_id()) }
    fn suite(&self, e: &Expression) -> Option<String> {
        match unwrap(e) {
            Expression::Identifier(id) => match self.symbol(id).and_then(|s| self.suites.get(&s)) {
                Some(name) => Some(name.clone()),
                None => SUITES.contains(&id.name.as_str()).then(|| id.name.to_string()),
            },
            Expression::StaticMemberExpression(m) => self.suite(&m.object).or_else(|| {
                let namespace = matches!(unwrap(&m.object), Expression::Identifier(id) if self.symbol(id).is_some_and(|s| self.namespaces.contains(&s)));
                (namespace && SUITES.contains(&m.property.name.as_str())).then(|| m.property.name.to_string())
            }),
            Expression::ComputedMemberExpression(m) => self.suite(&m.object),
            _ => None,
        }
    }
    fn selector(&self, callee: &Expression) -> Option<(String, String)> {
        match unwrap(callee) {
            Expression::StaticMemberExpression(m) => {
                let prop = m.property.name.as_str();
                match self.suite(&m.object) {
                    Some(suite) if SELECTORS.contains(&prop) => Some((suite, prop.to_string())),
                    _ => self.selector(&m.object),
                }
            }
            Expression::ComputedMemberExpression(m) => match (literal(&m.expression), self.suite(&m.object)) {
                (Some(prop), Some(suite)) if SELECTORS.contains(&prop.as_str()) => Some((suite, prop)),
                _ => self.selector(&m.object),
            },
            Expression::CallExpression(c) => self.selector(&c.callee),
            _ => None,
        }
    }
    fn unconditional_skip<'b>(&self, arg: &'b Argument<'b>) -> Option<String> {
        let top = |statements: &'b oxc_allocator::Vec<'b, Statement<'b>>| -> Vec<&'b Expression<'b>> {
            statements.iter().filter_map(|s| match s {
                Statement::ExpressionStatement(e) => Some(&e.expression),
                Statement::ReturnStatement(r) => r.argument.as_ref(),
                _ => None,
            }).collect()
        };
        let (params, calls) = match arg {
            Argument::ArrowFunctionExpression(f) => (&f.params, match &f.body {
                ArrowFunctionBody::FunctionBody(b) => top(&b.statements),
                other => other.as_expression().into_iter().collect(),
            }),
            Argument::FunctionExpression(f) => (&f.params, top(&f.body.as_ref()?.statements)),
            _ => return None,
        };
        let first = params.items.first().map(|p| &p.pattern);
        let context = match first { Some(BindingPattern::BindingIdentifier(b)) => b.symbol_id.get(), _ => None };
        let destructured: Vec<SymbolId> = match first {
            Some(BindingPattern::ObjectPattern(o)) => o.properties.iter().filter(|p| p.key.static_name().is_some_and(|k| k == "skip" || k == "todo"))
                .filter_map(|p| match &p.value { BindingPattern::BindingIdentifier(b) => b.symbol_id.get(), _ => None }).collect(),
            _ => Vec::new(),
        };
        for e in calls {
            let Expression::CallExpression(c) = unwrap(e) else { continue };
            match unwrap(&c.callee) {
                Expression::StaticMemberExpression(m) if ["skip", "todo"].contains(&m.property.name.as_str()) => match unwrap(&m.object) {
                    Expression::ThisExpression(_) => return Some(format!("this.{}()", m.property.name)),
                    Expression::Identifier(id) if context.is_some() && self.symbol(id) == context => return Some(format!("the test context's {}()", m.property.name)),
                    _ => {}
                },
                Expression::Identifier(id) if self.symbol(id).is_some_and(|s| destructured.contains(&s)) => return Some(format!("the test context's {}()", id.name)),
                _ => {}
            }
        }
        None
    }
    fn runner(&self, e: &Expression) -> Option<String> {
        match unwrap(e) {
            Expression::Identifier(id) => match self.symbol(id) {
                Some(s) => self.suites.get(&s).cloned(),
                None => SUITES.contains(&id.name.as_str()).then(|| id.name.to_string()),
            },
            Expression::StaticMemberExpression(m) => self.runner(&m.object).or_else(|| self.suite(e)),
            _ => None,
        }
    }
    fn framework(&self, e: &Expression) -> bool {
        matches!(unwrap(e), Expression::CallExpression(c) if matches!(&c.callee, Expression::Identifier(id) if id.name == "require" && self.global(id))
            && matches!(c.arguments.first(), Some(Argument::StringLiteral(s)) if FRAMEWORKS.contains(&s.value.as_str())))
    }
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
    fn caller(&mut self, object: &Expression, prop: &str) {
        let made = |e: &Expression| match unwrap(e) {
            Expression::NewExpression(n) => matches!(unwrap(&n.callee), Expression::Identifier(id) if id.name == "Error" && self.global(id)),
            Expression::CallExpression(c) => matches!(unwrap(&c.callee), Expression::Identifier(id) if id.name == "Error" && self.global(id)),
            _ => false,
        };
        let error = matches!(unwrap(object), Expression::Identifier(id) if id.name == "Error" && self.global(id));
        let found = match prop {
            "stack" if made(object) => Some("new Error().stack".to_string()),
            "caller" | "callee" => Some(format!(".{prop}")),
            "prepareStackTrace" | "captureStackTrace" if error => Some(format!("Error.{prop}")),
            _ => None,
        };
        if let Some(f) = found { self.found.push(f); }
    }
    fn member(&mut self, object: &Expression, prop: &str) {
        if self.test {
            if SELECTORS.contains(&prop) && let Some(suite) = self.suite(object) { self.found.push(format!("{suite}.{prop}")); }
            return;
        }
        self.caller(object, prop);
        if SNIFFS.contains(&prop) { self.found.push(prop.to_string()); }
        let Expression::Identifier(id) = unwrap(object) else { return };
        let name = id.name.as_str();
        if self.global(id) && GLOBALS.contains(&name) && TEST_GLOBALS.contains(&prop) { self.found.push(format!("{name}.{prop}")); }
    }
}
impl<'a> Visit<'a> for Scan<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if self.test && !it.import_kind.is_type() && FRAMEWORKS.contains(&it.source.value.as_str()) {
            for spec in it.specifiers.iter().flatten() {
                match spec {
                    ImportDeclarationSpecifier::ImportSpecifier(x) => {
                        let imported = x.imported.name();
                        if SUITES.contains(&imported.as_str()) && let Some(id) = x.local.symbol_id.get() { self.suites.insert(id, imported.to_string()); }
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => { if let Some(id) = x.local.symbol_id.get() { self.suites.insert(id, "test".into()); } }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => { if let Some(id) = x.local.symbol_id.get() { self.namespaces.insert(id); } }
                }
            }
        }
        if !self.test && !it.import_kind.is_type() && ASSERTIONS.contains(&it.source.value.as_str()) {
            for s in it.specifiers.iter().flatten() {
                let local = match s { ImportDeclarationSpecifier::ImportSpecifier(x) => &x.local, ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => &x.local, ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => &x.local };
                if let Some(id) = local.symbol_id.get() { self.bound.insert(id, it.source.value.to_string()); }
            }
        }
        walk::walk_import_declaration(self, it);
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if self.test && let Some(init) = &it.init {
            match &it.id {
                BindingPattern::BindingIdentifier(b) => if let Some(id) = b.symbol_id.get() {
                    if self.framework(init) { self.namespaces.insert(id); }
                    else if self.selector(init).is_none() && let Some(suite) = self.suite(init) { self.suites.insert(id, suite); }
                },
                BindingPattern::ObjectPattern(o) => {
                    let (suite, framework) = (self.suite(init), self.framework(init));
                    for prop in &o.properties {
                        let Some(key) = prop.key.static_name() else { continue };
                        if let Some(suite) = &suite && SELECTORS.contains(&key.as_ref()) { self.found.push(format!("{suite}.{key}")); }
                        if framework && SUITES.contains(&key.as_ref()) && let BindingPattern::BindingIdentifier(b) = &prop.value && let Some(id) = b.symbol_id.get() {
                            self.suites.insert(id, key.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
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
            let title = it.arguments.first().and_then(Argument::as_expression).and_then(literal);
            let renamed = match unwrap(&it.callee) { Expression::Identifier(id) if RENAMED.contains(&id.name.as_str()) => Some(id.name.to_string()), _ => None };
            if let Some(name) = &renamed {
                self.found.push(name.clone());
                if let Some(t) = &title { self.found.push(format!("{name}('{t}')")); }
            }
            let mut selected = self.selector(&it.callee);
            if selected.is_none() && test_shaped(&it.arguments) && let Some((name, prop)) = chain_selector(&it.callee) {
                self.found.push(format!("{name}.{prop}"));
                selected = Some((name, prop));
            }
            if let (Some((suite, prop)), Some(t)) = (&selected, &title) { self.found.push(format!("{suite}.{prop}('{t}')")); }
            if let Expression::ComputedMemberExpression(m) = unwrap(&it.callee) && literal(&m.expression).is_none() && let Some(r) = self.runner(&m.object) {
                self.found.push(format!("{r}[a computed name]"));
            }
            let suite = renamed.is_some() || selected.is_some() || self.suite(&it.callee).is_some();
            for arg in it.arguments.iter().filter(|_| suite) {
                if let Some(skip) = self.unconditional_skip(arg) {
                    if let Some(t) = &title { self.found.push(format!("{skip} in '{t}'")); }
                    self.found.push(skip);
                }
                let Argument::ObjectExpression(options) = arg else { continue };
                for property in &options.properties {
                    let ObjectPropertyKind::ObjectProperty(p) = property else { continue };
                    let Some(key) = p.key.static_name() else { continue };
                    let marked = match key.as_ref() {
                        "skip" | "todo" => truthy(&p.value),
                        k => SELECTORS.contains(&k) && !matches!(unwrap(&p.value), Expression::BooleanLiteral(b) if !b.value),
                    };
                    if marked { self.found.push(format!("the {key} option")); }
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
        if !self.test && TEST_GLOBALS.contains(&it.name.as_str()) && self.global(it) { self.found.push(format!("the test global {}", it.name)); }
    }
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) {
        if !self.test && SNIFFS.contains(&it.value.as_str()) { self.found.push(it.value.to_string()); }
    }
    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        if !self.test && let Some(q) = it.single_quasi() && SNIFFS.contains(&q.as_str()) { self.found.push(q.to_string()); }
        walk::walk_template_literal(self, it);
    }
}
fn markers(path: &str, source: &str, test: bool) -> R<BTreeMap<String, usize>> {
    let attempts = crate::capability::source_types(path).ok_or_else(|| Denied(format!("Unsupported code file: {path}")))?;
    for st in attempts {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { continue; }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let mut scan = Scan { scoping: semantic.scoping(), test, found: Vec::new(), bound: BTreeMap::new(), writes: Vec::new(), suites: BTreeMap::new(),
            namespaces: BTreeSet::new() };
        scan.visit_program(&parsed.program);
        for w in std::mem::take(&mut scan.writes) {
            let module = match w { Write::Module(m) => Some(m), Write::Symbol(s) => scan.bound.get(&s).cloned() };
            if let Some(m) = module { scan.found.push(format!("a change to {m}")); }
        }
        let mut counts = BTreeMap::new();
        for f in scan.found { *counts.entry(f).or_insert(0) += 1; }
        return Ok(counts);
    }
    deny!("Changed code cannot be parsed for the test proof: {path}. Fix its syntax")
}
fn text_of<'f>(files: &'f Files, path: &str) -> Option<&'f str> { files.get(path).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()) }
fn helper(path: &str) -> bool { path.split('/').map(str::to_ascii_lowercase).any(|d| HELPERS.contains(&d.as_str())) }

pub fn screen(base: &Files, candidate: &Files, changed: &Value, tests: &Tests) -> R<Value> {
    let (mut new_tests, mut changed_tests, mut deleted_tests, mut regression) = (Vec::new(), Vec::new(), Vec::new(), false);
    for f in changed["files"].as_array().into_iter().flatten() {
        let (path, op) = (f["path"].as_str().unwrap_or(""), f["operation"].as_str().unwrap_or(""));
        let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
        ensure!(!RUNNER_CONFIG.iter().any(|c| name.starts_with(c)), "Test runner configuration cannot change under the test proof: {path}. It decides which tests run and how they check results; leave it unchanged");
        let inside = allowed(&tests.paths, path);
        if inside && op != "add" {
            if tests.edits == Edits::Locked {
                ensure!(op != "delete", "Existing test file deleted: {path}. The test proof locks existing tests so they keep checking the old behaviour; leave existing tests in place");
                deny!("Existing test file changed: {path}. The test proof locks existing tests so they keep checking the old behaviour; put new tests in a new file and leave existing ones unchanged");
            }
            if op == "delete" { deleted_tests.push(path.to_string()) } else { changed_tests.push(path.to_string()) }
        }
        if op == "delete" || !is_code(path) { continue; }
        let source = text_of(candidate, path).ok_or_else(|| Denied(format!("Changed file cannot be read as text: {path}")))?;
        let found = markers(path, source, inside)?;
        let before = text_of(base, path).and_then(|s| markers(path, s, inside).ok()).unwrap_or_default();
        if inside {
            let grown: Vec<&String> = found.iter().filter(|(k, n)| before.get(*k).copied().unwrap_or(0) < **n).map(|(k, _)| k).collect();
            if let Some(m) = grown.iter().find(|k| k.contains("('")).or(grown.first()) {
                deny!("A test file skips, focuses or inverts tests: {path} uses {m}. Every test must run and check what it says, so remove it")
            }
            if !helper(path) {
                regression = true;
                if op == "add" { new_tests.push(path.to_string()); }
            }
            continue;
        }
        if let Some(m) = found.keys().find(|k| !before.contains_key(*k)) {
            deny!("Code outside the tests reaches into the test runner: {path} uses {m}. Code may not change how tests run or what they check, or check whether tests are running; remove it")
        }
    }
    ensure!(tests.kind != Kind::Bugfix || regression, "A bug fix must add a test that fails before the fix and passes after it: add or change a test file under {}", tests.paths.join(", "));
    Ok(json!({ "kind": tests.kind.name(), "runner": tests.runner.name(), "edits": tests.edits.name(), "new_test_files": new_tests, "changed_test_files": changed_tests,
        "deleted_test_files": deleted_tests, "existing_test_files": base.keys().filter(|k| allowed(&tests.paths, k)).count() }))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Count { pub passed: u32, pub failed: u32, pub skipped: u32 }
impl Count {
    pub fn total(self) -> u32 { self.passed + self.failed + self.skipped }
    pub fn ran(self) -> u32 { self.passed + self.failed }
}
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Run { Base, New, Kept, Old }
impl Run {
    pub fn name(self) -> &'static str { match self { Run::Base => "base", Run::New => "new", Run::Kept => "kept", Run::Old => "old" } }
    fn code(self) -> &'static str { match self { Run::Base => "the old code", Run::New => "the new code", Run::Kept => "the new code with the old tests", Run::Old => "the old code with the new tests added" } }
}
pub struct Output { pub stdout: Vec<u8>, pub exit_code: i64 }
pub type Exec<'a> = dyn Fn(Run, &[String]) -> R<Output> + Sync + 'a;
fn batch(run: &Exec, jobs: &[(Run, Vec<String>)]) -> Vec<R<Output>> {
    std::thread::scope(|s| {
        let handles: Vec<_> = jobs.iter().map(|(which, files)| s.spawn(move || run(*which, files))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(Denied("A test run stopped unexpectedly".into())))).collect()
    })
}
fn failing(r: &Report) -> Vec<&String> { r.tests.iter().filter(|(_, c)| c.failed > 0).map(|(k, _)| k).chain(&r.problems).collect() }
fn fails_on_old(old: &Report, before: &Report) -> BTreeSet<String> { old.tests.iter().filter(|(k, c)| c.failed > before.count(k).failed).map(|(k, _)| k.clone()).collect() }
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

pub fn prove(tests: &Tests, base: &Files, candidate: &Files, changed: &Value, run: &Exec, mutate: &mut dyn FnMut(&Plan) -> R<Value>) -> R<Value> {
    let screened = match changed.get("tests") { Some(t) if t.is_object() => t.clone(), _ => screen(base, candidate, changed, tests)? };
    let names = |k: &str| -> Vec<String> { screened[k].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    let edited = !names("changed_test_files").is_empty() || !names("deleted_test_files").is_empty();
    let (bugfix, kept_run) = (tests.kind == Kind::Bugfix, old_tests_alone(&changed["files"], &screened, tests));
    let focused: Vec<String> = names("new_test_files").into_iter().chain(names("changed_test_files").into_iter().filter(|p| is_code(p) && !helper(p))).collect();
    let read = |which: Run, out: R<Output>| -> R<(Report, i64)> {
        let out = out.map_err(|e| Denied(format!("Kern could not run the test suite on {}: {}", which.code(), e.0)))?;
        let report = read_report(tests.runner, &out.stdout).map_err(|why| Denied(match which {
            Run::Base => format!("Kern could not read the test results of the old code: {why}. Check tests.runner and tests.command in the task contract"),
            Run::New => format!("The test run on the new code gave no usable results: {why}. The test command must run to completion; often a test file cannot load or the run crashed"),
            Run::Kept => format!("The old tests gave no usable results on the new code: {why}. Tests may be rewritten or moved, but the old tests must still load and pass on the new code"),
            Run::Old => format!("The test run on the old code with the new tests added gave no usable results: {why}. A bug-fix test must load on the old code and fail inside a test, so it cannot depend on files the fix adds"),
        }))?;
        Ok((report, out.exit_code))
    };
    let missing = || Err(Denied("A test run returned no result".into()));
    let mut jobs = vec![(Run::Base, Vec::new()), (Run::New, Vec::new())];
    if kept_run { jobs.push((Run::Kept, Vec::new())); }
    if bugfix { jobs.push((Run::Old, focused.clone())); }
    let mut outs = batch(run, &jobs).into_iter();
    let (before, _) = read(Run::Base, outs.next().unwrap_or_else(missing))?;
    let known: BTreeSet<String> = failing(&before).into_iter().cloned().collect();
    ensure!(known.is_empty() || before.sum().passed > 0, "The test suite already fails before the change: {}. No test passes before the change, so the test proof has nothing to compare against; fix the suite on the base branch first", quote(&known.iter().collect::<Vec<_>>()));
    let healthy = |r: &R<(Report, i64)>| r.as_ref().is_ok_and(|(report, exit)| { let f = failing(report); f.iter().all(|k| known.contains(*k)) && (*exit == 0 || !f.is_empty()) });
    let new1 = read(Run::New, outs.next().unwrap_or_else(missing));
    let kept1 = kept_run.then(|| read(Run::Kept, outs.next().unwrap_or_else(missing)));
    let old1 = bugfix.then(|| read(Run::Old, outs.next().unwrap_or_else(missing)));
    let shown1 = old1.as_ref().and_then(|r| r.as_ref().ok()).map(|(r, _)| fails_on_old(r, &before)).unwrap_or_default();
    let old2_files = old1.as_ref().and_then(|_| if !shown1.is_empty() { Some(focused.clone()) } else if !focused.is_empty() { Some(Vec::new()) } else { None });
    let mut again = Vec::new();
    if !healthy(&new1) { again.push((Run::New, Vec::new())); }
    if kept1.as_ref().is_some_and(|k| !healthy(k)) { again.push((Run::Kept, Vec::new())); }
    if let Some(files) = &old2_files { again.push((Run::Old, files.clone())); }
    let mut outs2 = batch(run, &again).into_iter();
    let mut reruns = Vec::new();
    let new_final = if healthy(&new1) { new1.clone_report() } else { reruns.push("new"); read(Run::New, outs2.next().unwrap_or_else(missing)) };
    let kept_final = match &kept1 { Some(k) if !healthy(k) => { reruns.push("kept"); Some(read(Run::Kept, outs2.next().unwrap_or_else(missing))) } other => other.as_ref().map(|k| k.clone_report()) };
    let old2 = old2_files.as_ref().map(|_| read(Run::Old, outs2.next().unwrap_or_else(missing)));
    let flaky: Vec<String> = match (&new1, &new_final) {
        (first, Ok(_)) if !healthy(first) => match first { Ok((r, _)) if !failing(r).is_empty() => failing(r).into_iter().cloned().collect(), Ok(_) => vec!["(the test command failed once without a failing test)".into()], Err(_) => vec!["(the run gave no usable results once)".into()] },
        _ => Vec::new(),
    };
    let (after, exit) = new_final?;
    let failed: Vec<&String> = failing(&after).into_iter().filter(|k| !known.contains(*k)).collect();
    ensure!(failed.is_empty(), "The test suite fails on the new code: {}. Every test must pass after the change; fix the code, not the tests", quote(&failed));
    let stopped = before.tests.iter().filter(|_| !edited).filter(|(k, c)| { let n = after.count(k); n.total() < c.total() || if known.contains(*k) { n.passed + n.failed < c.ran() } else { n.passed < c.ran() } }).map(|(k, _)| k);
    let skipped = after.tests.iter().filter(|(k, c)| c.skipped > before.count(k).skipped).map(|(k, _)| k);
    let not_run: Vec<&String> = stopped.chain(skipped).collect::<BTreeSet<_>>().into_iter().collect();
    ensure!(not_run.is_empty(), "Tests did not run on the new code: {}. Every test that ran before must still run, and every new test must run; do not skip, remove or rename tests", quote(&not_run));
    ensure!(exit == 0 || !failing(&after).is_empty(), "The test command failed on the new code although no test failed. Something outside the tests failed, such as a crash after the run, an unhandled error or a coverage threshold; fix it so the command succeeds");
    ensure!(after.sum().passed > 0, "The test suite ran no tests on the new code, so it proves nothing. Check tests.runner and tests.command in the task contract");
    let new_tests: Vec<&String> = after.tests.iter().filter(|(k, c)| c.total() > before.count(k).total()).map(|(k, _)| k).collect();
    let mut evidence = json!({
        "kind": tests.kind.name(), "runner": tests.runner.name(), "command": command(tests), "runs": { "base": summary(&before), "new": summary(&after) },
        "pass_to_pass": before.tests.values().filter(|c| c.passed > 0).count(), "new_tests_total": new_tests.len(),
        "new_test_files": screened["new_test_files"], "existing_test_files": screened["existing_test_files"], "reruns": reruns, "flaky": flaky,
    });
    if !known.is_empty() {
        let passing_now = known.iter().filter(|k| { let n = after.count(k); n.passed > 0 && n.failed == 0 }).count();
        evidence["already_failing"] = json!({ "total": known.len(), "tests": known.iter().take(100).collect::<Vec<_>>(), "passing_now": passing_now });
    }
    if !edited && let Some(alone) = kept_final.as_ref() {
        let (alone, _) = alone.clone_report()?;
        let failed: Vec<&String> = failing(&alone).into_iter().filter(|k| !known.contains(*k)).collect();
        ensure!(failed.is_empty(), "Existing tests fail on the new code when they run without the new tests: {}. The code has to behave the same however the tests are run, so it must not keep state that changes its answers", quote(&failed));
        let missing: Vec<&String> = before.tests.iter().filter(|(k, c)| { let n = alone.count(k); n.total() < c.total() || if known.contains(*k) { n.passed + n.failed < c.ran() } else { n.passed < c.ran() } }).map(|(k, _)| k).collect();
        ensure!(missing.is_empty(), "Tests did not run on the new code: {}. Every test that ran before must still run, and every new test must run; do not skip, remove or rename tests", quote(&missing));
        evidence["old_tests_alone"] = summary(&alone);
    }
    if edited {
        let kept = kept_final.unwrap_or_else(|| Ok((before.clone(), 0)));
        let record = json!({ "mode": tests.edits.name(), "changed_files": screened["changed_test_files"], "deleted_files": screened["deleted_test_files"] });
        evidence["test_edits"] = edits(tests, base, candidate, &screened, &before, kept, record, mutate)?;
    }
    if tests.kind != Kind::Bugfix {
        evidence["new_tests"] = json!(new_tests.iter().take(100).map(|k| json!({ "test": k, "old": "not run", "new": "passed" })).collect::<Vec<_>>());
        return Ok(evidence);
    }
    ensure!(edited || !new_tests.is_empty(), "No new test ran in the test suite. A bug fix must add a test under {} that the test command runs, and that fails before the fix and passes after it", tests.paths.join(", "));
    let (decider, confirm) = match (old1, old2) {
        (Some(first), Some(second)) if !shown1.is_empty() => (first?, Some(second?)),
        (Some(_), Some(full)) => {
            let full = full?;
            let shown = fails_on_old(&full.0, &before);
            let again = if shown.is_empty() { None } else { Some(read(Run::Old, batch(run, &[(Run::Old, Vec::new())]).into_iter().next().unwrap_or_else(missing))?) };
            (full, again)
        }
        (Some(first), None) => (first?, None),
        _ => deny!("A test run returned no result"),
    };
    let first: BTreeSet<String> = fails_on_old(&decider.0, &before).into_iter().filter(|k| after.tests.contains_key(k)).collect();
    let shown = if new_tests.is_empty() { "the changed tests".to_string() } else { quote(&new_tests) };
    ensure!(!first.is_empty(), "None of the new tests fails on the old code, so they do not show the bug: {shown}. Write a test that fails before the fix and passes after it; a test that never reaches the changed code (for example because it mocks it) proves nothing");
    let repeated: Vec<&String> = confirm.as_ref().map(|(r, _)| fails_on_old(r, &before)).map(|again| first.iter().filter(|k| again.contains(*k)).collect()).unwrap_or_default();
    let unsure: Vec<&String> = first.iter().collect();
    ensure!(!repeated.is_empty(), "The new tests fail on the old code only sometimes: {}. A test that shows the bug must fail on the old code every time and pass on the new code", quote(&unsure));
    let old = &decider.0;
    let outcome = |k: &str| {
        let (o, b) = (old.count(k), before.count(k));
        if repeated.iter().any(|r| *r == k) { "failed" } else if o.failed > b.failed { "failed once" } else if o.passed > b.passed { "passed" } else if o.skipped > b.skipped { "skipped" } else { "not run" }
    };
    evidence["runs"]["old"] = summary(old);
    evidence["new_tests"] = json!(new_tests.iter().take(100).map(|k| json!({ "test": k, "old": outcome(k), "new": "passed" })).collect::<Vec<_>>());
    evidence["fail_to_pass"] = json!(repeated.len());
    evidence["fail_to_pass_tests"] = json!(repeated.iter().take(100).collect::<Vec<_>>());
    Ok(evidence)
}

pub fn old_tests_alone(files: &Value, screened: &Value, tests: &Tests) -> bool {
    let any = |k: &str| screened[k].as_array().is_some_and(|a| !a.is_empty());
    let code_changed = files.as_array().into_iter().flatten().filter_map(|f| f["path"].as_str()).any(|p| !allowed(&tests.paths, p));
    code_changed && (any("changed_test_files") || any("deleted_test_files") || any("new_test_files"))
}
trait CloneReport { fn clone_report(&self) -> R<(Report, i64)>; }
impl CloneReport for R<(Report, i64)> {
    fn clone_report(&self) -> R<(Report, i64)> { match self { Ok((r, e)) => Ok((r.clone(), *e)), Err(e) => Err(Denied(e.0.clone())) } }
}

#[allow(clippy::too_many_arguments)]
fn edits(tests: &Tests, base: &Files, candidate: &Files, screened: &Value, before: &Report, kept: R<(Report, i64)>, mut record: Value, mutate: &mut dyn FnMut(&Plan) -> R<Value>) -> R<Value> {
    let change = tests.kind == Kind::Change;
    let (kept, exit) = match kept {
        Ok(k) => k,
        Err(why) if change => { record["old_tests_on_new_code"] = json!({ "usable": false, "why": why.0 }); record["safety_net"] = json!({ "checked": 0, "reason": "the old tests no longer run on the new code" }); return Ok(record); }
        Err(why) => return Err(why),
    };
    record["old_tests_on_new_code"] = summary(&kept);
    let command_failed = "the test command itself".to_string();
    let (known, explained): (Vec<String>, bool) = (failing(before).into_iter().cloned().collect(), !failing(&kept).is_empty());
    let mut failing: Vec<&String> = kept.tests.iter().filter(|(k, c)| c.failed > before.count(k).failed).map(|(k, _)| k).chain(kept.problems.iter().filter(|p| !before.problems.contains(p))).collect();
    if failing.is_empty() && exit != 0 && !explained { failing.push(&command_failed); }
    let missing: Vec<&String> = before.tests.iter().filter(|(k, c)| { let n = kept.count(k); n.total() < c.total() || n.passed + n.failed < c.ran() }).map(|(k, _)| k).collect();
    if change {
        record["changed_expectations"] = json!(failing.iter().chain(&missing).take(100).collect::<Vec<_>>());
        if !failing.is_empty() || !missing.is_empty() {
            record["safety_net"] = json!({ "checked": 0, "reason": "the task changes behaviour, and the old tests no longer all pass on the new code" });
            return Ok(record);
        }
    }
    ensure!(failing.is_empty(), "Existing tests fail on the new code: {}. Tests may be rewritten or removed, but the old tests must still pass on the new code; fix the code", quote(&failing));
    ensure!(missing.is_empty(), "Tests did not run on the new code: {}. Every test that ran before must still run, and every new test must run; do not skip, remove or rename tests", quote(&missing));
    let mut plan = plan(tests, base, candidate, screened)?;
    if !known.is_empty() { (plan.command, plan.known) = (command(tests), known); }
    let mut net = json!({ "targets": plan.targets, "planted": plan.mutants.len() });
    if !plan.mutants.is_empty() {
        let outcome = mutate(&plan)?;
        let unusable = |why: &str| Denied(format!("Kern could not check the test changes: {why}"));
        ensure!(outcome["version"] == 1, "{}", unusable("the planting run returned no result").0);
        for side in ["old", "new"] {
            let error: String = outcome["baseline"][format!("{side}_error")].as_str().unwrap_or("").chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            let error: String = error.split_whitespace().collect::<Vec<_>>().join(" ").chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
            let detail = if error.is_empty() { String::new() } else { format!(" ({error})") };
            ensure!(outcome["baseline"][side] == true, "{}", unusable(&format!("the {side} tests did not pass on the new code when bugs were planted{detail}")).0);
        }
        let results = outcome["mutants"].as_array().ok_or_else(|| unusable("the planting run returned no result"))?;
        let ids: Vec<u64> = results.iter().filter_map(|r| r["id"].as_u64()).collect();
        ensure!(ids.len() == plan.mutants.len() && ids.iter().enumerate().all(|(i, id)| *id == i as u64 + 1), "{}", unusable("the planting run reported bugs Kern did not plant").0);
        let of = |state: &str| -> Vec<String> { results.iter().zip(&plan.mutants).filter(|(r, _)| r["result"] == state).map(|(_, m)| m.describe()).collect() };
        ensure!(results.iter().all(|r| r["result"].as_str().is_some_and(|x| ["caught", "missed by old", "lost", "flaky", "not run"].contains(&x))), "{}", unusable("the planting run reported an unknown result").0);
        let (lost, flaky, not_run) = (of("lost"), of("flaky"), of("not run").len());
        let checked = plan.mutants.len() - not_run;
        ensure!(checked >= plan.mutants.len().min(5), "Kern could not check the test changes in time: {checked} of {} planted bugs were checked before the time limit. Leave the existing tests unchanged, or raise limits.timeout_seconds", plan.mutants.len());
        ensure!(lost.is_empty(), "Your test changes stop catching bugs the old tests caught: {}. Keep a test that catches each of them or write a better one; tests may be rewritten or removed only while every bug the old tests caught is still caught", quote(&lost));
        let caught = of("caught").len();
        net["checked"] = json!(checked);
        net["caught_by_old"] = json!(caught);
        net["still_caught"] = json!(caught);
        net["lost"] = json!(lost);
        net["flaky"] = json!(flaky);
        net["not_run"] = json!(not_run);
        let ms = |k: &str| outcome["milliseconds"][k].as_u64().unwrap_or(0);
        net["milliseconds"] = json!({ "old": ms("old"), "new": ms("new"), "total": ms("total") });
    } else {
        net["checked"] = json!(0);
        net["reason"] = json!("the old tests reach no code Kern can plant a bug in");
    }
    record["safety_net"] = net;
    Ok(record)
}

pub fn caught(runner: Runner, known: &BTreeSet<String>, stdout: &[u8], passed: bool) -> bool {
    if known.is_empty() { return !passed; }
    match read_report(runner, stdout) {
        Ok(r) => { let f = failing(&r); f.iter().any(|k| !known.contains(*k)) || (!passed && f.is_empty()) }
        Err(_) => !passed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suite { Old, NewFocused, NewFull }
pub fn decide(run: &mut dyn FnMut(Suite) -> R<Option<bool>>, focused: bool) -> R<&'static str> {
    let Some(old) = run(Suite::Old)? else { return Ok("not run") };
    if !old { return Ok("missed by old"); }
    if focused {
        match run(Suite::NewFocused)? { None => return Ok("not run"), Some(true) => return Ok("caught"), Some(false) => {} }
    }
    Ok(match run(Suite::NewFull)? {
        None => "not run",
        Some(true) => "caught",
        Some(false) => match run(Suite::Old)? {
            None => "not run",
            Some(false) => "flaky",
            Some(true) => match run(Suite::NewFull)? { None => "not run", Some(true) => "flaky", Some(false) => "lost" },
        },
    })
}

pub fn run_limit(left: std::time::Duration, limit: Option<std::time::Duration>) -> Option<std::time::Duration> {
    match limit { None => (!left.is_zero()).then_some(left), Some(l) => (left >= l).then_some(l) }
}

const MAX_PLANTED: usize = 40;
const MAX_TARGETS: usize = 40;
const EXTENSIONS: [&str; 8] = [".js", ".mjs", ".cjs", ".ts", ".tsx", ".jsx", ".mts", ".cts"];
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutant { pub id: usize, pub path: String, pub line: usize, pub start: usize, pub end: usize, pub original: String, pub replacement: String, what: String,
    pub tests: Vec<String> }
impl Mutant {
    pub fn describe(&self) -> String { format!("{}:{} {}", self.path, self.line, self.what) }
}
#[derive(Clone, Debug)]
pub struct Plan { pub targets: Vec<String>, pub mutants: Vec<Mutant>, pub command: Vec<String>, pub tests_paths: Vec<String>, pub old_files: Vec<String>, pub runner: Runner, pub known: Vec<String> }
pub fn split_planting(plan: &Plan, n: usize) -> Vec<Plan> {
    let total = plan.mutants.len();
    let parts = n.min(total.div_ceil(4)).max(1);
    let (size, extra) = (total / parts, total % parts);
    let mut at = 0;
    (0..parts).map(|i| {
        let take = size + usize::from(i < extra);
        let part = Plan { mutants: plan.mutants[at..at + take].to_vec(), ..plan.clone() };
        at += take;
        part
    }).collect()
}
pub fn merge_planted(parts: Vec<Value>) -> R<Value> {
    let mut merged = json!({ "version": 1, "baseline": { "old": true, "new": true, "old_error": "", "new_error": "" }, "milliseconds": { "old": 0, "new": 0, "total": 0 } });
    let mut mutants = Vec::new();
    for part in parts {
        if part["version"] != 1 { return Ok(part); }
        for side in ["old", "new"] {
            if part["baseline"][side] != true && merged["baseline"][side] == true {
                merged["baseline"][side] = json!(false);
                merged["baseline"][format!("{side}_error")] = part["baseline"][format!("{side}_error")].clone();
            }
        }
        for k in ["old", "new", "total"] {
            let ms = part["milliseconds"][k].as_u64().unwrap_or(0);
            if ms > merged["milliseconds"][k].as_u64().unwrap_or(0) { merged["milliseconds"][k] = json!(ms); }
        }
        mutants.extend(part["mutants"].as_array().cloned().unwrap_or_default());
    }
    mutants.sort_by_key(|m| m["id"].as_u64().unwrap_or(u64::MAX));
    merged["mutants"] = json!(mutants);
    Ok(merged)
}
impl Plan {
    pub fn json(&self, budget_seconds: u64) -> Value {
        json!({ "version": 1, "command": self.command, "tests_paths": self.tests_paths, "budget_seconds": budget_seconds, "old_files": self.old_files, "runner": self.runner.name(), "known": self.known,
            "mutants": self.mutants.iter().map(|m| json!({ "id": m.id, "path": m.path, "start": m.start, "end": m.end, "original": m.original, "replacement": m.replacement, "tests": m.tests })).collect::<Vec<_>>() })
    }
}

struct Declares(bool);
impl<'a> Visit<'a> for Declares {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let name = match unwrap(&it.callee) {
            Expression::Identifier(id) => Some(id.name.as_str()),
            Expression::StaticMemberExpression(m) => if let Expression::Identifier(id) = unwrap(&m.object) { Some(id.name.as_str()) } else { None },
            _ => None,
        };
        if name.is_some_and(|n| SUITES.contains(&n)) { self.0 = true; }
        walk::walk_call_expression(self, it);
    }
}
fn declares_tests(path: &str, source: &str) -> bool { parsed(path, source, |program, _| { let mut v = Declares(false); v.visit_program(program); v.0 }).unwrap_or(false) }
pub fn import_edges(files: &Files) -> BTreeMap<String, Vec<String>> { edges(files) }
fn edges(files: &Files) -> BTreeMap<String, Vec<String>> {
    files.iter().filter(|(k, _)| is_code(k)).filter_map(|(k, _)| {
        let source = text_of(files, k)?;
        let specs = parsed(k, source, |program, _| { let mut v = Imports(Vec::new()); v.visit_program(program); v.0 }).unwrap_or_default();
        Some((k.clone(), specs.iter().filter_map(|s| resolve(k, s, &|p| files.contains_key(p))).collect()))
    }).collect()
}
fn closure(edges: &BTreeMap<String, Vec<String>>, start: &str) -> BTreeSet<String> {
    let (mut seen, mut queue) = (BTreeSet::new(), VecDeque::from([start.to_string()]));
    while let Some(f) = queue.pop_front() {
        if seen.len() >= 2000 || !seen.insert(f.clone()) { continue; }
        queue.extend(edges.get(&f).into_iter().flatten().cloned());
    }
    seen
}

struct Imports(Vec<String>);
impl<'a> Visit<'a> for Imports {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) { if !it.import_kind.is_type() { self.0.push(it.source.value.to_string()); } }
    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) { self.0.push(it.source.value.to_string()); }
    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) { self.0.push(it.source.value.to_string()); }
    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if let Some(s) = literal(&it.source) { self.0.push(s); }
        walk::walk_import_expression(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if matches!(&it.callee, Expression::Identifier(id) if id.name == "require") && let Some(Argument::StringLiteral(s)) = it.arguments.first() { self.0.push(s.value.to_string()); }
        walk::walk_call_expression(self, it);
    }
}
fn parsed<T>(path: &str, source: &str, f: impl Fn(&Program, &str) -> T) -> Option<T> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let p = Parser::new(&allocator, source, st).parse();
        (!p.fatal_error && p.diagnostics.is_empty()).then(|| f(&p.program, source))
    })
}
fn resolve(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    if !(spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../")) { return None; }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in spec.split(['?', '#']).next().unwrap_or("").split('/') {
        match seg { "" | "." => {} ".." => { parts.pop()?; } s => parts.push(s) }
    }
    let p = parts.join("/");
    let swapped = [(".js", ".ts"), (".js", ".tsx"), (".mjs", ".mts"), (".cjs", ".cts"), (".jsx", ".tsx")].iter().filter_map(|(js, ts)| p.strip_suffix(js).map(|stem| format!("{stem}{ts}")));
    std::iter::once(p.clone()).chain(EXTENSIONS.iter().map(|e| format!("{p}{e}"))).chain(swapped).chain(EXTENSIONS.iter().map(|e| format!("{p}/index{e}")))
        .find(|c| is_code(c) && exists(c))
}
fn reach(tests: &Tests, base: &Files, candidate: &Files, starts: &[String]) -> Vec<String> {
    let side = |p: &str| allowed(&tests.paths, p);
    let exists = |p: &str| if side(p) { base.contains_key(p) } else { candidate.contains_key(p) };
    let (mut queue, mut seen, mut targets): (VecDeque<String>, BTreeSet<String>, BTreeSet<String>) = (starts.iter().cloned().collect(), BTreeSet::new(), BTreeSet::new());
    while let Some(path) = queue.pop_front() {
        if seen.len() >= 400 || !seen.insert(path.clone()) || !is_code(&path) { continue; }
        let Some(source) = text_of(if side(&path) { base } else { candidate }, &path) else { continue };
        let specs = parsed(&path, source, |program, _| { let mut v = Imports(Vec::new()); v.visit_program(program); v.0 }).unwrap_or_default();
        for found in specs.iter().filter_map(|s| resolve(&path, s, &exists)) {
            if !side(&found) && targets.len() < MAX_TARGETS { targets.insert(found.clone()); }
            queue.push_back(found);
        }
    }
    targets.into_iter().collect()
}

#[derive(Default)]
struct Planter { found: Vec<(usize, usize, String, String, String, u8)>, arid: u32 }
impl Planter {
    fn add(&mut self, start: usize, end: usize, original: &str, replacement: &str, what: String, rank: u8) {
        if self.arid == 0 && start < end { self.found.push((start, end, original.to_string(), replacement.to_string(), what, rank)); }
    }
}
fn swap(op: &str) -> Option<(&'static str, u8)> {
    Some(match op {
        "<" => ("<=", 1), "<=" => ("<", 1), ">" => (">=", 1), ">=" => (">", 1), "===" => ("!==", 1), "!==" => ("===", 1), "==" => ("!=", 1), "!=" => ("==", 1),
        "&&" => ("||", 2), "||" => ("&&", 2), "??" => ("&&", 2),
        "+" => ("-", 4), "-" => ("+", 4), "*" => ("/", 4), "/" => ("*", 4), "%" => ("*", 4),
        _ => return None,
    })
}
fn operator_at(source: &str, from: usize, to: usize, op: &str) -> Option<usize> {
    let gap = source.get(from..to)?;
    let mut i = 0;
    while i < gap.len() {
        let rest = &gap[i..];
        if rest.starts_with("//") { i += rest.find('\n').unwrap_or(rest.len()); }
        else if rest.starts_with("/*") { i += rest.find("*/").map_or(rest.len(), |j| j + 2); }
        else if rest.starts_with(op) { return Some(from + i); }
        else { i += rest.chars().next().map_or(1, char::len_utf8); }
    }
    None
}
struct Plant<'s> { source: &'s str, p: Planter }
impl Plant<'_> {
    fn binary(&mut self, left: oxc_span::Span, right: oxc_span::Span, op: &str) {
        let Some((to, rank)) = swap(op) else { return };
        if let Some(at) = operator_at(self.source, left.end as usize, right.start as usize, op) { self.p.add(at, at + op.len(), op, to, format!("changes {op} to {to}"), rank); }
    }
    fn negate(&mut self, test: oxc_span::Span) {
        let (start, end) = (test.start as usize, test.end as usize);
        if let Some(text) = self.source.get(start..end) { self.p.add(start, end, text, &format!("!({text})"), "negates the condition".into(), 3); }
    }
}
impl<'a> Visit<'a> for Plant<'_> {
    fn visit_ts_type(&mut self, _: &TSType<'a>) {}
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        let logging = matches!(&it.callee, Expression::StaticMemberExpression(m) if matches!(&m.object, Expression::Identifier(o) if o.name == "console"));
        self.p.arid += u32::from(logging);
        walk::walk_call_expression(self, it);
        self.p.arid -= u32::from(logging);
    }
    fn visit_expression_statement(&mut self, it: &ExpressionStatement<'a>) {
        if let Expression::CallExpression(call) = &it.expression && !matches!(&call.callee, Expression::StaticMemberExpression(m) if matches!(&m.object, Expression::Identifier(o) if o.name == "console")) {
            let (start, end) = (call.span.start as usize, call.span.end as usize);
            if let Some(text) = self.source.get(start..end) { self.p.add(start, end, text, "void 0", "removes a call".into(), 7); }
        }
        walk::walk_expression_statement(self, it);
    }
    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        self.binary(it.left.span(), it.right.span(), it.operator.as_str());
        walk::walk_binary_expression(self, it);
    }
    fn visit_logical_expression(&mut self, it: &LogicalExpression<'a>) {
        self.binary(it.left.span(), it.right.span(), it.operator.as_str());
        walk::walk_logical_expression(self, it);
    }
    fn visit_if_statement(&mut self, it: &IfStatement<'a>) {
        self.negate(it.test.span());
        walk::walk_if_statement(self, it);
    }
    fn visit_conditional_expression(&mut self, it: &ConditionalExpression<'a>) {
        self.negate(it.test.span());
        walk::walk_conditional_expression(self, it);
    }
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        let at = it.span.start as usize;
        if it.operator == UnaryOperator::LogicalNot && self.source.get(at..at + 1) == Some("!") { self.p.add(at, at + 1, "!", "", "removes !".into(), 5); }
        walk::walk_unary_expression(self, it);
    }
    fn visit_update_expression(&mut self, it: &UpdateExpression<'a>) {
        let (start, end) = (it.span.start as usize, it.span.end as usize);
        let at = if it.prefix { start } else { end.saturating_sub(2) };
        if let Some(op) = self.source.get(at..at + 2).filter(|o| *o == "++" || *o == "--") {
            let to = if op == "++" { "--" } else { "++" };
            self.p.add(at, at + 2, op, to, format!("changes {op} to {to}"), 6);
        }
        walk::walk_update_expression(self, it);
    }
    fn visit_boolean_literal(&mut self, it: &BooleanLiteral) {
        let (o, r) = if it.value { ("true", "false") } else { ("false", "true") };
        self.p.add(it.span.start as usize, it.span.end as usize, o, r, format!("changes {o} to {r}"), 5);
    }
}
fn plant(path: &str, source: &str) -> Vec<Mutant> {
    let found = parsed(path, source, |program, src| { let mut v = Plant { source: src, p: Planter::default() }; v.visit_program(program); v.p.found }).unwrap_or_default();
    let mut best: BTreeMap<usize, (usize, usize, String, String, String, u8)> = BTreeMap::new();
    for f in found {
        let line = source[..f.0].matches('\n').count() + 1;
        if best.get(&line).is_none_or(|b| (f.5, f.0) < (b.5, b.0)) { best.insert(line, f); }
    }
    best.into_iter().map(|(line, (start, end, original, replacement, what, _))| Mutant { id: 0, path: path.to_string(), line, start, end, original, replacement, what, tests: Vec::new() }).collect()
}
pub fn plan(tests: &Tests, base: &Files, candidate: &Files, screened: &Value) -> R<Plan> {
    let starts: Vec<String> = ["changed_test_files", "deleted_test_files"].iter().flat_map(|k| screened[*k].as_array().cloned().unwrap_or_default()).filter_map(|v| v.as_str().map(str::to_string)).collect();
    let mut targets = reach(tests, base, candidate, &starts);
    if targets.is_empty() {
        targets = candidate.keys().filter(|k| is_code(k) && !allowed(&tests.paths, k) && !k.split('/').any(|d| d == "node_modules") && !helper(k)).take(MAX_TARGETS).cloned().collect();
    }
    let per_file: Vec<Vec<Mutant>> = targets.iter().filter_map(|t| text_of(candidate, t).map(|s| plant(t, s))).collect();
    let (mut quota, mut left) = (vec![0usize; per_file.len()], MAX_PLANTED);
    while left > 0 && quota.iter().zip(&per_file).any(|(q, f)| *q < f.len()) {
        for (q, f) in quota.iter_mut().zip(&per_file) { if left > 0 && *q < f.len() { *q += 1; left -= 1; } }
    }
    let mut picked: Vec<VecDeque<Mutant>> = per_file.iter().zip(&quota).map(|(f, &q)| (0..q).map(|i| f[if q > 1 { i * (f.len() - 1) / (q - 1) } else { 0 }].clone()).collect()).collect();
    let mut mutants = Vec::new();
    while picked.iter().any(|f| !f.is_empty()) {
        for f in picked.iter_mut() { if let Some(m) = f.pop_front() { mutants.push(m); } }
    }
    let new_edges = edges(candidate);
    let new_tests: Vec<&String> = candidate.keys().filter(|k| allowed(&tests.paths, k) && is_code(k) && !helper(k) && text_of(candidate, k).is_some_and(|s| declares_tests(k, s))).collect();
    let mut reachers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in &new_tests { for f in closure(&new_edges, t) { reachers.entry(f).or_default().push((*t).clone()); } }
    for (i, m) in mutants.iter_mut().enumerate() {
        m.id = i + 1;
        m.tests = reachers.get(&m.path).filter(|r| r.len() * 2 <= new_tests.len().max(1)).cloned().unwrap_or_default();
    }
    let old_edges = edges(&base.iter().filter(|(k, _)| allowed(&tests.paths, k)).map(|(k, v)| (k.clone(), v.clone())).collect());
    let imported = |p: &String| old_edges.iter().any(|(from, to)| from != p && to.contains(p));
    let focus = !starts.is_empty() && starts.iter().all(|p| is_code(p) && text_of(base, p).is_some_and(|s| declares_tests(p, s)) && !imported(p));
    let old_files = if focus { starts.clone() } else { Vec::new() };
    let mut command = tests.command.clone();
    match tests.runner { Runner::Vitest => command.push("--no-cache".into()), Runner::Jest => command.push("--ci".into()), _ => {} }
    Ok(Plan { targets, mutants, command, tests_paths: tests.paths.clone(), old_files, runner: tests.runner, known: Vec::new() })
}
