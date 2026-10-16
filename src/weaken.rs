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
pub struct Case { pub checks: usize, pub weak: usize }

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

struct Walk { path: Vec<String>, current: Option<String>, cases: BTreeMap<String, Case> }
impl<'a> Visit<'a> for Walk {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.current.is_none() && let (Some(name), Some(t)) = (root(&it.callee), title(&it.arguments)) {
            if GROUPS.contains(&name.as_str()) {
                self.path.push(t);
                walk::walk_call_expression(self, it);
                self.path.pop();
                return;
            }
            if CASES.contains(&name.as_str()) {
                let key = self.path.iter().chain(std::iter::once(&t)).cloned().collect::<Vec<_>>().join(" > ");
                self.cases.entry(key.clone()).or_default();
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
        let mut w = Walk { path: Vec::new(), current: None, cases: BTreeMap::new() };
        w.visit_program(&parsed.program);
        Some(w.cases)
    })
}

pub fn leaf(key: &str) -> &str { key.rsplit(" > ").next().unwrap_or(key) }

pub fn compare(old: &BTreeMap<String, Case>, new: &BTreeMap<String, Case>, elsewhere: &BTreeSet<String>) -> Weakened {
    let missing: Vec<String> = old.keys().filter(|k| !new.contains_key(*k) && !elsewhere.contains(leaf(k))).cloned().collect();
    let added = new.keys().filter(|k| !old.contains_key(*k)).count();
    let removed = missing.len().saturating_sub(added);
    let kept = |f: &dyn Fn(&Case, &Case) -> bool| -> Vec<String> { old.iter().filter_map(|(k, o)| new.get(k).filter(|n| f(o, n)).map(|_| k.clone())).collect() };
    Weakened {
        removed,
        missing: if removed > 0 { missing } else { Vec::new() },
        emptied: kept(&|o, n| o.checks > 0 && n.checks == 0),
        loosened: kept(&|o, n| n.checks > 0 && n.weak > o.weak && n.checks - n.weak < o.checks - o.weak),
    }
}
