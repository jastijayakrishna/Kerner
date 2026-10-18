use crate::proof::{parsed, resolve, unwrap};
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_syntax::operator::UnaryOperator;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

const CALLS_COUNT_MAX: usize = 64;
const VALUE_DEPTH_MAX: usize = 8;
const SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;
const ALIAS_MARKS: [&str; 4] = ["@", "~", "@@", "~~"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import { pub spec: String, pub bindings: Vec<(String, String)>, pub esm: bool }

fn loaded(e: &Expression) -> Option<(String, bool)> {
    match unwrap(e) {
        Expression::CallExpression(c) if c.arguments.len() == 1 && matches!(&c.callee, Expression::Identifier(id) if id.name == "require") => match &c.arguments[0] {
            Argument::StringLiteral(s) => Some((s.value.to_string(), false)),
            _ => None,
        },
        Expression::ImportExpression(i) => match unwrap(&i.source) { Expression::StringLiteral(s) => Some((s.value.to_string(), true)), _ => None },
        _ => None,
    }
}
struct Imports(Vec<Import>);
impl<'a> Visit<'a> for Imports {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if it.import_kind.is_type() { return; }
        let mut bindings = Vec::new();
        for s in it.specifiers.iter().flatten() {
            match s {
                ImportDeclarationSpecifier::ImportSpecifier(x) if !x.import_kind.is_type() => bindings.push((x.local.name.to_string(), x.imported.name().to_string())),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => bindings.push((x.local.name.to_string(), "default".to_string())),
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => bindings.push((x.local.name.to_string(), "*".to_string())),
                ImportDeclarationSpecifier::ImportSpecifier(_) => {}
            }
        }
        if bindings.is_empty() && it.specifiers.as_ref().is_some_and(|s| !s.is_empty()) { return; }
        self.0.push(Import { spec: it.source.value.to_string(), bindings, esm: true });
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init {
            let (from, picked) = match unwrap(init) {
                Expression::StaticMemberExpression(m) => (loaded(&m.object), Some(m.property.name.to_string())),
                e => (loaded(e), None),
            };
            if let Some((spec, esm)) = from {
                let bindings: Vec<(String, String)> = match (&it.id, picked) {
                    (BindingPattern::BindingIdentifier(id), Some(name)) => vec![(id.name.to_string(), name)],
                    (BindingPattern::BindingIdentifier(id), None) => vec![(id.name.to_string(), "*".to_string())],
                    (BindingPattern::ObjectPattern(o), None) => o.properties.iter().filter(|p| !p.computed).filter_map(|p| match &p.value {
                        BindingPattern::BindingIdentifier(v) => Some((v.name.to_string(), p.key.static_name()?.to_string())),
                        _ => None,
                    }).collect(),
                    _ => Vec::new(),
                };
                if !bindings.is_empty() { self.0.push(Import { spec, bindings, esm }); }
            }
        }
        walk::walk_variable_declarator(self, it);
    }
}
pub fn imports(path: &str, source: &str) -> Vec<Import> {
    parsed(path, source, |program, _| { let mut v = Imports(Vec::new()); v.visit_program(program); v.0 }).unwrap_or_default()
}

pub fn aliased(importer: &str, spec: &str, target: &str) -> bool {
    if spec.starts_with(['.', '/']) || !crate::capability::is_code(target) { return false; }
    let Some((first, rest)) = spec.split_once('/') else { return false };
    let stem = target.rsplit_once('.').map_or(target, |(s, _)| s);
    let stems = [stem, stem.strip_suffix("/index").unwrap_or(stem)];
    let marked = ALIAS_MARKS.contains(&first) || first.starts_with(['#', '$']);
    let clean = |c: &&str| !c.is_empty() && c.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    [Some(spec), marked.then_some(rest)].into_iter().flatten().filter(clean).any(|c| stems.iter().any(|s| {
        let root = if *s == c { Some("") } else { s.strip_suffix(c).filter(|r| r.ends_with('/')) };
        root.is_some_and(|r| {
            let scope = r.trim_end_matches('/').rsplit_once('/').map_or("", |(parent, _)| parent);
            scope.is_empty() || importer.starts_with(&format!("{scope}/"))
        })
    }))
}
pub fn points_to(importer: &str, spec: &str, target: &str, exists: &dyn Fn(&str) -> bool) -> bool {
    resolve(importer, spec, exists).as_deref() == Some(target) || aliased(importer, spec, target)
}

fn number(n: f64) -> Option<Value> {
    if !n.is_finite() { return None; }
    if n.fract() == 0.0 && n.abs() < SAFE_INTEGER { return Some(json!(n as i64)); }
    serde_json::Number::from_f64(n).map(Value::Number)
}
fn value(e: &Expression, depth: usize) -> Option<Value> {
    if depth > VALUE_DEPTH_MAX { return None; }
    Some(match unwrap(e) {
        Expression::StringLiteral(s) => json!(s.value.as_str()),
        Expression::TemplateLiteral(t) if t.expressions.is_empty() => json!(t.quasis.first()?.value.cooked.as_ref()?.as_str()),
        Expression::NumericLiteral(n) => number(n.value)?,
        Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => match unwrap(&u.argument) { Expression::NumericLiteral(n) => number(-n.value)?, _ => return None },
        Expression::BooleanLiteral(b) => json!(b.value),
        Expression::NullLiteral(_) => Value::Null,
        Expression::ArrayExpression(a) => Value::Array(a.elements.iter().map(|x| x.as_expression().and_then(|x| value(x, depth + 1))).collect::<Option<Vec<_>>>()?),
        Expression::ObjectExpression(o) => {
            let mut m = Map::new();
            for p in &o.properties {
                let ObjectPropertyKind::ObjectProperty(p) = p else { return None };
                if p.kind != PropertyKind::Init || p.method || p.shorthand || (p.computed && !matches!(p.key, PropertyKey::StringLiteral(_))) { return None; }
                m.insert(p.key.static_name()?.to_string(), value(&p.value, depth + 1)?);
            }
            Value::Object(m)
        }
        _ => return None,
    })
}
pub fn literal(e: &Expression) -> Option<Value> { value(e, 0) }

struct Locals<'e> { direct: BTreeSet<String>, owners: BTreeSet<String>, export: &'e str }
impl<'e> Locals<'e> {
    fn of(path: &str, source: &str, (target, export): (&str, &'e str), exists: &dyn Fn(&str) -> bool) -> Locals<'e> {
        let (mut direct, mut owners) = (BTreeSet::new(), BTreeSet::new());
        for i in imports(path, source).into_iter().filter(|i| points_to(path, &i.spec, target, exists)) {
            for (local, name) in i.bindings {
                if name == export || (name == "*" && export == "default" && !i.esm) { direct.insert(local.clone()); }
                if name == "*" || name == "default" { owners.insert(local); }
            }
        }
        Locals { direct, owners, export }
    }
    fn none(&self) -> bool { self.direct.is_empty() && self.owners.is_empty() }
    fn called(&self, callee: &Expression) -> bool {
        match unwrap(callee) {
            Expression::Identifier(id) => self.direct.contains(id.name.as_str()),
            Expression::StaticMemberExpression(m) => m.property.name == self.export && matches!(unwrap(&m.object), Expression::Identifier(o) if self.owners.contains(o.name.as_str())),
            _ => false,
        }
    }
}
struct Calls<'s> { locals: &'s Locals<'s>, found: Vec<Vec<Value>> }
impl<'a> Visit<'a> for Calls<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.locals.called(&it.callee) && self.found.len() < CALLS_COUNT_MAX && let Some(args) = it.arguments.iter().map(|a| a.as_expression().and_then(literal)).collect::<Option<Vec<_>>>()
            && !self.found.contains(&args) {
            self.found.push(args);
        }
        walk::walk_call_expression(self, it);
    }
}
pub fn literal_calls(path: &str, source: &str, target: (&str, &str), exists: &dyn Fn(&str) -> bool) -> Vec<Vec<Value>> {
    let locals = Locals::of(path, source, target, exists);
    if locals.none() { return Vec::new(); }
    parsed(path, source, |program, _| { let mut v = Calls { locals: &locals, found: Vec::new() }; v.visit_program(program); v.found }).unwrap_or_default()
}

const TEST_CALLS: [&str; 3] = ["it", "test", "specify"];
fn root<'b>(e: &'b Expression) -> Option<&'b str> {
    match unwrap(e) {
        Expression::Identifier(id) => Some(id.name.as_str()),
        Expression::StaticMemberExpression(m) => root(&m.object),
        Expression::CallExpression(c) => root(&c.callee),
        _ => None,
    }
}
struct Cases<'s> { locals: &'s Locals<'s>, open: Vec<(usize, usize, bool)>, found: Vec<(usize, usize)> }
impl<'a> Visit<'a> for Cases<'_> {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.locals.called(&it.callee) { for case in self.open.iter_mut() { case.2 = true; } }
        let case = root(&it.callee).is_some_and(|r| TEST_CALLS.contains(&r));
        if case { self.open.push((it.span.start as usize, it.span.end as usize, false)); }
        walk::walk_call_expression(self, it);
        if case && let Some((start, end, true)) = self.open.pop() && self.found.len() < CALLS_COUNT_MAX { self.found.push((start, end)); }
    }
}
pub fn test_spans(path: &str, source: &str, target: (&str, &str), exists: &dyn Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let locals = Locals::of(path, source, target, exists);
    if locals.none() { return Vec::new(); }
    parsed(path, source, |program, _| { let mut v = Cases { locals: &locals, open: Vec::new(), found: Vec::new() }; v.visit_program(program); v.found }).unwrap_or_default()
}

const DECLARED_SHAPES_COUNT_MAX: usize = 16;
fn kinds(t: &TSType) -> Option<(Vec<&'static str>, bool)> {
    Some(match t {
        TSType::TSStringKeyword(_) | TSType::TSTemplateLiteralType(_) => (vec!["string"], false),
        TSType::TSNumberKeyword(_) => (vec!["number"], false),
        TSType::TSBooleanKeyword(_) => (vec!["boolean"], false),
        TSType::TSNullKeyword(_) => (vec!["null"], false),
        TSType::TSUndefinedKeyword(_) => (Vec::new(), true),
        TSType::TSLiteralType(l) => (vec![match &l.literal {
            TSLiteral::StringLiteral(_) | TSLiteral::TemplateLiteral(_) => "string",
            TSLiteral::NumericLiteral(_) => "number",
            TSLiteral::BooleanLiteral(_) => "boolean",
            _ => return None,
        }], false),
        TSType::TSParenthesizedType(p) => kinds(&p.type_annotation)?,
        TSType::TSUnionType(u) => {
            let (mut all, mut optional) = (Vec::new(), false);
            for t in &u.types {
                let (more, maybe) = kinds(t)?;
                optional |= maybe;
                for k in more { if !all.contains(&k) { all.push(k); } }
            }
            (all, optional)
        }
        _ => return None,
    })
}
fn parameters<'p>(program: &'p Program, export: &str) -> Option<&'p FormalParameters<'p>> {
    program.body.iter().find_map(|s| {
        let declaration = match s { Statement::ExportDeclaration(e) => Some(&e.declaration), _ => s.as_declaration() }?;
        match declaration {
            Declaration::FunctionDeclaration(f) if f.body.is_some() && f.id.as_ref().is_some_and(|id| id.name == export) => Some(&*f.params),
            Declaration::VariableDeclaration(v) => v.declarations.iter().find_map(|d| match (&d.id, d.init.as_ref().map(unwrap)) {
                (BindingPattern::BindingIdentifier(id), Some(Expression::ArrowFunctionExpression(f))) if id.name == export => Some(&*f.params),
                (BindingPattern::BindingIdentifier(id), Some(Expression::FunctionExpression(f))) if id.name == export => Some(&*f.params),
                _ => None,
            }),
            _ => None,
        }
    })
}
pub fn declared_shapes(path: &str, source: &str, export: &str) -> Vec<Vec<&'static str>> {
    parsed(path, source, |program, _| {
        let Some(params) = parameters(program, export).filter(|p| p.rest.is_none()) else { return Vec::new() };
        let mut known: Vec<(Vec<&'static str>, bool)> = Vec::new();
        for p in &params.items {
            let Some((all, maybe)) = p.type_annotation.as_ref().and_then(|t| kinds(&t.type_annotation)).filter(|(all, _)| !all.is_empty()) else { break };
            known.push((all, maybe || p.optional || p.initializer.is_some()));
        }
        let typed = known.len();
        let required = params.items.iter().enumerate().take_while(|(i, p)| !(p.optional || p.initializer.is_some() || known.get(*i).is_some_and(|(_, maybe)| *maybe))).count();
        if required > typed { return Vec::new(); }
        let mut shapes: Vec<Vec<&'static str>> = Vec::new();
        let mut row: Vec<Vec<&'static str>> = vec![Vec::new()];
        for (n, (all, _)) in known.iter().enumerate() {
            if n >= required { shapes.extend(row.iter().cloned()); }
            row = row.iter().flat_map(|r| all.iter().map(move |k| { let mut r = r.clone(); r.push(*k); r })).take(DECLARED_SHAPES_COUNT_MAX).collect();
        }
        shapes.extend(row);
        shapes.retain(|s| s.len() >= required);
        shapes.truncate(DECLARED_SHAPES_COUNT_MAX);
        shapes
    }).unwrap_or_default()
}

pub fn shape(args: &[Value]) -> Vec<&'static str> {
    args.iter().map(|a| match a {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }).collect()
}
