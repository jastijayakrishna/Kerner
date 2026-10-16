use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use std::collections::{BTreeMap, BTreeSet};

const GROUPS: [&str; 3] = ["describe", "context", "suite"];
const CASES: [&str; 3] = ["it", "test", "specify"];
const WEAK: [&str; 6] = ["toBeDefined", "toBeTruthy", "toBeFalsy", "toHaveBeenCalled", "toBeCalled", "toBeInstanceOf"];
const WEAK_WHEN_NEGATED: [&str; 5] = ["toThrow", "toBeNull", "toBeUndefined", "toBeNaN", "toBeFalsy"];
const WEAK_ASSERTS: [&str; 5] = ["ok", "isOk", "exists", "isDefined", "isNotNull"];
const HELPERS: [&str; 6] = ["expect", "assert", "check", "verify", "should", "ensure"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Case { pub checks: usize, pub weak: usize, pub runs: usize }

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Weakened { pub removed: usize, pub missing: Vec<String>, pub emptied: Vec<String>, pub loosened: Vec<String> }
impl Weakened {
    pub fn is_empty(&self) -> bool { self.removed == 0 && self.emptied.is_empty() && self.loosened.is_empty() }
}

fn strip<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> {
    match e {
        Expression::ParenthesizedExpression(p) => strip(&p.expression),
        Expression::TSAsExpression(x) => strip(&x.expression),
        Expression::TSNonNullExpression(x) => strip(&x.expression),
        Expression::AwaitExpression(x) => strip(&x.argument),
        _ => e,
    }
}
fn root(e: &Expression) -> Option<String> {
    match strip(e) {
        Expression::Identifier(id) => Some(id.name.to_string()),
        Expression::StaticMemberExpression(m) => root(&m.object),
        Expression::CallExpression(c) => root(&c.callee),
        Expression::TaggedTemplateExpression(t) => root(&t.tag),
        _ => None,
    }
}
fn called(e: &Expression) -> bool {
    match strip(e) {
        Expression::CallExpression(_) => true,
        Expression::StaticMemberExpression(m) => called(&m.object),
        _ => false,
    }
}
fn negated(e: &Expression) -> bool {
    match strip(e) {
        Expression::StaticMemberExpression(m) => m.property.name == "not" || negated(&m.object),
        _ => false,
    }
}
fn rows(callee: &Expression) -> Option<usize> {
    match strip(callee) {
        Expression::CallExpression(c) => match strip(&c.callee) {
            Expression::StaticMemberExpression(m) if m.property.name == "each" => match c.arguments.first().and_then(Argument::as_expression).map(strip) {
                Some(Expression::ArrayExpression(a)) => Some(a.elements.len()),
                _ => Some(0),
            },
            other => rows(other),
        },
        Expression::StaticMemberExpression(m) => rows(&m.object),
        Expression::TaggedTemplateExpression(t) => match strip(&t.tag) { Expression::StaticMemberExpression(m) if m.property.name == "each" => Some(0), _ => None },
        _ => None,
    }
}
fn title(args: &[Argument]) -> Option<String> {
    match strip(args.first()?.as_expression()?) {
        Expression::StringLiteral(s) => Some(s.value.to_string()),
        Expression::TemplateLiteral(t) => t.single_quasi().map(|q| q.to_string()),
        _ => None,
    }
}
fn check(call: &CallExpression) -> Option<bool> {
    match strip(&call.callee) {
        Expression::Identifier(id) => {
            let n = id.name.as_str();
            if n == "expect" { None } else if n == "assert" { Some(true) } else { HELPERS.iter().any(|h| n.starts_with(h)).then_some(false) }
        }
        Expression::StaticMemberExpression(m) => {
            let property = m.property.name.as_str();
            match root(&m.object)?.as_str() {
                "expect" if called(&m.object) => Some(WEAK.contains(&property) || (negated(&m.object) && WEAK_WHEN_NEGATED.contains(&property))),
                "assert" => Some(WEAK_ASSERTS.contains(&property)),
                _ => None,
            }
        }
        _ => None,
    }
}

struct Walk { path: Vec<String>, current: Option<String>, cases: BTreeMap<String, Case>, unknown: bool }
impl<'a> Visit<'a> for Walk {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.current.is_none() && let (Some(name), Some(t)) = (root(&it.callee), title(&it.arguments)) {
            if GROUPS.contains(&name.as_str()) {
                if rows(&it.callee).is_some() { self.unknown = true; }
                self.path.push(t);
                walk::walk_call_expression(self, it);
                self.path.pop();
                return;
            }
            if CASES.contains(&name.as_str()) {
                let key = self.path.iter().chain(std::iter::once(&t)).cloned().collect::<Vec<_>>().join(" > ");
                let runs = rows(&it.callee).unwrap_or(1);
                if runs == 0 { self.unknown = true; }
                self.cases.entry(key.clone()).or_default().runs += runs.max(1);
                self.current = Some(key);
                walk::walk_call_expression(self, it);
                self.current = None;
                return;
            }
        }
        if let (Some(key), Some(weak)) = (&self.current, check(it)) && let Some(c) = self.cases.get_mut(key) {
            c.checks += 1;
            if weak { c.weak += 1; }
        }
        walk::walk_call_expression(self, it);
    }
}

pub fn cases(path: &str, source: &str) -> Option<BTreeMap<String, Case>> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let mut w = Walk { path: Vec::new(), current: None, cases: BTreeMap::new(), unknown: false };
        w.visit_program(&parsed.program);
        if w.unknown { for c in w.cases.values_mut() { c.runs = 0; } }
        Some(w.cases)
    })
}

struct Blocks { path: Vec<String>, found: Vec<(String, usize, usize)> }
impl<'a> Visit<'a> for Blocks {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let (Some(name), Some(t)) = (root(&it.callee), title(&it.arguments)) && (GROUPS.contains(&name.as_str()) || CASES.contains(&name.as_str())) {
            let key = self.path.iter().chain(std::iter::once(&t)).cloned().collect::<Vec<_>>().join(" > ");
            self.found.push((key, it.span.start as usize, it.span.end as usize));
            if GROUPS.contains(&name.as_str()) {
                self.path.push(t);
                walk::walk_call_expression(self, it);
                self.path.pop();
            }
            return;
        }
        walk::walk_call_expression(self, it);
    }
}
fn blocks(path: &str, source: &str) -> Option<Vec<(String, usize, usize)>> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let mut b = Blocks { path: Vec::new(), found: Vec::new() };
        b.visit_program(&parsed.program);
        Some(b.found)
    })
}
pub fn only_adds_tests(path: &str, old: &str, new: &str) -> bool {
    let (Some(before), Some(after)) = (blocks(path, old), blocks(path, new)) else { return false };
    let known: BTreeSet<&str> = before.iter().map(|(k, _, _)| k.as_str()).collect();
    let mut cut: Vec<(usize, usize)> = Vec::new();
    for (key, start, end) in &after {
        if known.contains(key.as_str()) || cut.last().is_some_and(|(_, e)| start < e) { continue; }
        cut.push((*start, *end));
    }
    let mut rest = String::with_capacity(new.len());
    let mut at = 0;
    for (start, end) in &cut { rest.push_str(&new[at..*start]); at = *end; }
    rest.push_str(&new[at..]);
    let squeeze = |t: &str| t.chars().filter(|c| !c.is_whitespace() && *c != ';').collect::<String>();
    !cut.is_empty() && squeeze(&rest) == squeeze(old)
}

pub fn leaf(key: &str) -> &str { key.rsplit(" > ").next().unwrap_or(key) }

pub fn compare(old: &BTreeMap<String, Case>, new: &BTreeMap<String, Case>, elsewhere: &BTreeSet<String>) -> Weakened {
    let missing: Vec<String> = old.keys().filter(|k| !new.contains_key(*k) && !elsewhere.contains(leaf(k))).cloned().collect();
    let countable = old.values().chain(new.values()).all(|c| c.runs > 0);
    let lost: usize = missing.iter().map(|k| old[k].runs).sum();
    let gained: usize = new.iter().filter(|(k, _)| !old.contains_key(*k)).map(|(_, c)| c.runs).sum::<usize>()
        + old.iter().filter_map(|(k, o)| new.get(k).map(|n| n.runs.saturating_sub(o.runs))).sum::<usize>();
    let shrunk: usize = old.iter().filter_map(|(k, o)| new.get(k).map(|n| o.runs.saturating_sub(n.runs))).sum();
    let removed = if countable { (lost + shrunk).saturating_sub(gained) } else { 0 };
    let kept = |f: &dyn Fn(&Case, &Case) -> bool| -> Vec<String> { old.iter().filter_map(|(k, o)| new.get(k).filter(|n| f(o, n)).map(|_| k.clone())).collect() };
    Weakened {
        removed,
        missing: if removed > 0 { missing } else { Vec::new() },
        emptied: kept(&|o, n| o.checks > 0 && n.checks == 0),
        loosened: kept(&|o, n| n.checks > 0 && n.weak > o.weak && n.checks - n.weak < o.checks - o.weak),
    }
}
