use crate::git::Files;
use crate::util::*;
use crate::ensure;
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{SemanticBuilder, Scoping};
use oxc_span::SourceType;
use oxc_syntax::scope::ScopeFlags;
use oxc_syntax::symbol::SymbolId;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const POLICY: &str = "capability-footprint-v3";
const CODE: [&str; 8] = ["js", "mjs", "cjs", "jsx", "ts", "tsx", "mts", "cts"];
const DATA: [&str; 5] = ["json", "md", "txt", "css", "html"];
const BUILTINS: [&str; 47] = [
    "assert", "async_hooks", "buffer", "child_process", "cluster", "console", "constants", "crypto", "dgram", "diagnostics_channel", "dns", "domain", "events", "fs", "http", "http2",
    "https", "inspector", "module", "net", "os", "path", "perf_hooks", "process", "punycode", "querystring", "readline", "repl", "stream", "string_decoder", "sys", "timers", "tls",
    "trace_events", "tty", "url", "util", "v8", "vm", "wasi", "worker_threads", "zlib", "test", "sqlite", "sea", "quic", "_http_agent",
];
const GLOBALS: [&str; 4] = ["globalThis", "global", "window", "self"];
const NETWORK: [&str; 9] = ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "importScripts", "Worker", "SharedWorker", "Bun", "Deno"];
const REACH: [(&str, &str); 8] = [("mainModule", "process"), ("getBuiltinModule", "process"), ("dlopen", "process"), ("_linkedBinding", "process"), ("_load", "module"), ("_compile", "module"), ("_resolveFilename", "module"), ("sendBeacon", "global")];
const PROCESS: [&str; 10] = ["binding", "_linkedBinding", "dlopen", "getBuiltinModule", "mainModule", "env", "kill", "chdir", "setuid", "setgid"];
const MODULE: [&str; 5] = ["require", "constructor", "_load", "_compile", "_resolveFilename"];
const CODE_GLOBALS: [&str; 3] = ["eval", "Function", "WebAssembly"];

fn extension(name: &str) -> &str { name.rsplit_once('.').map_or("", |(_, e)| e) }
fn declaration(name: &str) -> bool { [".d.ts", ".d.mts", ".d.cts"].iter().any(|s| name.ends_with(s)) }
pub fn is_code(name: &str) -> bool { CODE.contains(&extension(name)) && !declaration(name) }
const TEXT_NAMES: [&str; 8] = ["AUTHORS", "CONTRIBUTORS", "MAINTAINERS", "CHANGELOG", "CHANGES", "HISTORY", "THANKS", "CREDITS"];
pub fn text_name(name: &str) -> bool { TEXT_NAMES.contains(&name.rsplit('/').next().unwrap_or(name)) }
pub fn is_data(name: &str) -> bool { DATA.contains(&extension(name)) || declaration(name) || text_name(name) }
fn word(s: &str) -> bool { !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') }
fn package(s: &str) -> bool {
    let name_ok = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    match s.strip_prefix('@').and_then(|r| r.split_once('/')) { Some((scope, name)) => name_ok(scope) && name_ok(name), None => name_ok(s) }
}
pub fn valid(c: &str) -> bool {
    if c.len() > 200 { return false; }
    match c.split_once(':') {
        Some(("node", m)) => !m.is_empty() && m.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'/'),
        Some(("package", p)) => package(p),
        Some(("escape", p)) => !p.is_empty(),
        Some(("url", s)) => !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+.-".contains(&b)),
        Some(("import", "computed")) | Some(("timer", "string")) => true,
        Some(("require" | "code" | "process" | "module" | "global" | "bun" | "deno", w)) => word(w),
        _ => false,
    }
}
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) { out.push((h * 16 + l) as u8); i += 3; continue; }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Footprint<'s> {
    file: &'s str, caps: BTreeSet<String>, scoping: &'s Scoping, strict: bool, global_this: bool, method_next: bool, aliases: BTreeSet<SymbolId>,
    saved: Vec<(SymbolId, String)>, restored: BTreeMap<SymbolId, usize>,
}
fn class_of<'b, 'a>(e: &'b Expression<'a>) -> Option<&'b Expression<'a>> {
    match strip(e) { Expression::StaticMemberExpression(m) if m.property.name == "constructor" => Some(&m.object), _ => None }
}
impl Footprint<'_> {
    fn add(&mut self, c: String) { self.caps.insert(c); }
    fn global(&self, id: &IdentifierReference) -> bool { id.reference_id.get().is_none_or(|r| self.scoping.get_reference(r).symbol_id().is_none()) }
    fn symbol(&self, id: &IdentifierReference) -> Option<SymbolId> { id.reference_id.get().and_then(|r| self.scoping.get_reference(r).symbol_id()) }
    fn alias(&self, id: &IdentifierReference) -> bool { self.symbol(id).is_some_and(|s| self.aliases.contains(&s)) }
    fn network_member(&self, e: &Expression) -> Option<String> {
        match strip(e) { Expression::StaticMemberExpression(m) if NETWORK.contains(&m.property.name.as_str()) && self.global_object(&m.object) => Some(format!("global:{}", m.property.name)), _ => None }
    }
    fn beside_class<'a>(&mut self, e: &Expression<'a>) { match class_of(e) { Some(inner) => self.visit_expression(inner), None => self.visit_expression(e) } }
    fn type_matcher(&self, call: &CallExpression) -> bool {
        crate::suite::is_test_path(self.file) && call.arguments.len() == 1 && matches!(&call.arguments[0], Argument::Identifier(id) if id.name == "Function" && self.global(id))
            && matches!(strip(&call.callee), Expression::StaticMemberExpression(m) if ["any", "toBeInstanceOf"].contains(&m.property.name.as_str()))
    }
    fn global_object(&self, e: &Expression) -> bool {
        match strip(e) {
            Expression::Identifier(id) => (self.global(id) && GLOBALS.contains(&id.name.as_str())) || self.alias(id),
            Expression::LogicalExpression(l) => self.global_object(&l.left) || self.global_object(&l.right),
            Expression::ConditionalExpression(c) => self.global_object(&c.consequent) || self.global_object(&c.alternate),
            Expression::SequenceExpression(s) => s.expressions.last().is_some_and(|x| self.global_object(x)),
            Expression::AssignmentExpression(a) => self.global_object(&a.right),
            Expression::StaticMemberExpression(m) => GLOBALS.contains(&m.property.name.as_str()) && self.global_object(&m.object),
            Expression::TSAsExpression(x) => self.global_object(&x.expression),
            Expression::TSSatisfiesExpression(x) => self.global_object(&x.expression),
            Expression::TSNonNullExpression(x) => self.global_object(&x.expression),
            Expression::AwaitExpression(x) => self.global_object(&x.argument),
            Expression::CallExpression(c) => match strip(&c.callee) {
                Expression::ArrowFunctionExpression(f) => match &f.body {
                    ArrowFunctionBody::FunctionBody(b) => self.returns_global(&b.statements),
                    other => other.as_expression().is_some_and(|x| self.global_object(x)),
                },
                Expression::FunctionExpression(f) => f.body.as_ref().is_some_and(|b| self.returns_global(&b.statements)),
                _ => false,
            },
            _ => false,
        }
    }
    fn returns_global(&self, statements: &[Statement]) -> bool {
        statements.iter().any(|s| match s {
            Statement::ReturnStatement(r) => r.argument.as_ref().is_some_and(|a| self.global_object(a)),
            Statement::ExpressionStatement(e) => self.global_object(&e.expression),
            _ => false,
        })
    }
    fn global_member(&mut self, prop: &str) {
        if CODE_GLOBALS.contains(&prop) { self.add(format!("code:{prop}")); }
        if NETWORK.contains(&prop) || prop == "require" || prop == "process" { self.add(format!("global:{prop}")); }
    }
    fn module(&mut self, spec: &str) {
        if spec.starts_with('/') || spec.contains('\\') { return self.add(format!("escape:{spec}")); }
        if spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../") {
            let mut parts: Vec<String> = self.file.split('/').map(str::to_string).collect();
            parts.pop();
            for seg in percent_decode(spec).split('/') {
                match seg {
                    "" | "." => {}
                    ".." => { if parts.pop().is_none() { return self.add(format!("escape:{spec}")); } }
                    s => parts.push(s.to_string()),
                }
            }
            let last = parts.last().map(|p| p.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase()).unwrap_or_default();
            if declaration(&last) || last.ends_with(".d") { return self.add(format!("escape:{spec}")); }
            if let Some(i) = parts.iter().rposition(|p| p.eq_ignore_ascii_case("node_modules")) {
                return match &parts[i + 1..] {
                    [scope, name, ..] if scope.starts_with('@') => self.add(format!("package:{}/{}", scope.to_ascii_lowercase(), name.to_ascii_lowercase())),
                    [name, ..] => self.add(format!("package:{}", name.to_ascii_lowercase())),
                    [] => self.add(format!("escape:{spec}")),
                };
            }
            return;
        }
        if let Some((scheme, _)) = spec.split_once(':') && scheme != "node" && !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b)) {
            return match scheme.to_ascii_lowercase().as_str() {
                "virtual" => {}
                "astro" => { if spec.starts_with("astro:env/server") { self.add("process:env".into()) } }
                s => self.add(format!("url:{s}")),
            };
        }
        let bare = spec.strip_prefix("node:").unwrap_or(spec);
        let mut segs = bare.split('/');
        let first = segs.next().unwrap_or("");
        let root = if first.starts_with('@') { format!("{first}/{}", segs.next().unwrap_or("")) } else { first.to_string() };
        if spec.starts_with("node:") || BUILTINS.contains(&root.as_str()) { self.add(format!("node:{root}")) } else { self.add(format!("package:{}", root.to_ascii_lowercase())) }
    }
}
fn literal<'b>(e: &'b Expression) -> Option<&'b str> { if let Expression::StringLiteral(s) = e { Some(s.value.as_str()) } else { None } }
fn strip<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> { if let Expression::ParenthesizedExpression(p) = e { strip(&p.expression) } else { e } }
fn strict(directives: &[Directive]) -> bool { directives.iter().any(|d| d.directive.as_str() == "use strict") }
fn capitalized(name: Option<&str>) -> bool { name.and_then(|n| n.chars().next()).is_some_and(char::is_uppercase) }
impl<'a> Visit<'a> for Footprint<'_> {
    fn visit_program(&mut self, it: &Program<'a>) {
        self.strict = self.strict || strict(&it.directives) || it.body.iter().any(Statement::is_module_declaration);
        self.global_this = !self.strict;
        walk::walk_program(self, it);
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        let (saved, method) = ((self.strict, self.global_this), std::mem::take(&mut self.method_next));
        self.strict = self.strict || it.body.as_ref().is_some_and(|b| strict(&b.directives));
        self.global_this = !self.strict && !method && !capitalized(it.id.as_ref().map(|i| i.name.as_str()));
        walk::walk_function(self, it, flags);
        (self.strict, self.global_this) = saved;
    }
    fn visit_class(&mut self, it: &Class<'a>) {
        let saved = (self.strict, self.global_this);
        (self.strict, self.global_this) = (true, false);
        walk::walk_class(self, it);
        (self.strict, self.global_this) = saved;
    }
    fn visit_object_property(&mut self, it: &ObjectProperty<'a>) {
        self.visit_property_key(&it.key);
        self.method_next = matches!(it.value, Expression::FunctionExpression(_));
        self.visit_expression(&it.value);
        self.method_next = false;
    }
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let AssignmentTarget::AssignmentTargetIdentifier(id) = &it.left && self.global_object(&it.right)
            && let Some(s) = id.reference_id.get().and_then(|r| self.scoping.get_reference(r).symbol_id()) {
            self.aliases.insert(s);
        }
        let replaced = it.operator == AssignmentOperator::Assign
            && matches!(&it.left, AssignmentTarget::StaticMemberExpression(m) if NETWORK.contains(&m.property.name.as_str()) && self.global_object(&m.object));
        if !replaced { self.visit_assignment_target(&it.left); }
        else if let Expression::Identifier(id) = strip(&it.right) && let Some(s) = self.symbol(id) { *self.restored.entry(s).or_default() += 1; }
        self.method_next = matches!(it.left, AssignmentTarget::StaticMemberExpression(_) | AssignmentTarget::ComputedMemberExpression(_)) && matches!(it.right, Expression::FunctionExpression(_));
        self.visit_expression(&it.right);
        self.method_next = false;
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let (Some(init), BindingPattern::BindingIdentifier(b)) = (&it.init, &it.id) && let Some(cap) = self.network_member(init) && let Some(s) = b.symbol_id.get() {
            self.saved.push((s, cap));
            return;
        }
        if let Some(init) = &it.init && self.global_object(init) {
            match &it.id {
                BindingPattern::BindingIdentifier(b) => { if let Some(s) = b.symbol_id.get() { self.aliases.insert(s); } }
                BindingPattern::ObjectPattern(o) => {
                    for p in &o.properties {
                        match p.key.static_name() { Some(k) => self.global_member(&k), None => self.add("global:computed".into()) }
                    }
                }
                _ => self.add("global:computed".into()),
            }
        }
        self.visit_binding_pattern(&it.id);
        if let Some(init) = &it.init {
            let name = if let BindingPattern::BindingIdentifier(b) = &it.id { Some(b.name.as_str()) } else { None };
            self.method_next = matches!(init, Expression::FunctionExpression(_)) && capitalized(name);
            self.visit_expression(init);
            self.method_next = false;
        }
    }
    fn visit_this_expression(&mut self, _: &ThisExpression) { if self.global_this { self.add("global:dynamic".into()); } }
    fn visit_ts_type(&mut self, _: &TSType<'a>) {}
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) { if !it.import_kind.is_type() { self.module(it.source.value.as_str()); } }
    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) { if !it.export_kind.is_type() { self.module(it.source.value.as_str()); } }
    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) { if !it.export_kind.is_type() { self.module(it.source.value.as_str()); } }
    fn visit_ts_external_module_reference(&mut self, it: &TSExternalModuleReference<'a>) { self.module(it.expression.value.as_str()); }
    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        match literal(&it.source) { Some(s) => self.module(s), None => self.add("import:computed".into()) }
        walk::walk_import_expression(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if self.type_matcher(it) { return self.visit_expression(&it.callee); }
        if let Expression::StaticMemberExpression(m) = strip(&it.callee) && m.property.name == "captureStackTrace"
            && matches!(strip(&m.object), Expression::Identifier(e) if e.name == "Error" && self.global(e)) {
            self.visit_expression(&it.callee);
            for arg in &it.arguments { match arg.as_expression() { Some(e) => self.beside_class(e), None => self.visit_argument(arg) } }
            return;
        }
        if let Expression::Identifier(id) = &it.callee {
            let name = id.name.as_str();
            if name == "require" && self.global(id) {
                match (it.arguments.len(), it.arguments.first()) { (1, Some(Argument::StringLiteral(s))) => self.module(s.value.as_str()), _ => self.add("require:computed".into()) }
                for arg in &it.arguments { self.visit_argument(arg); }
                return;
            }
            if ["setTimeout", "setInterval", "setImmediate"].contains(&name) && matches!(it.arguments.first(), Some(Argument::StringLiteral(_) | Argument::TemplateLiteral(_) | Argument::BinaryExpression(_))) {
                self.add("timer:string".into());
            }
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_binary_expression(&mut self, it: &BinaryExpression<'a>) {
        if it.operator.is_equality() { self.beside_class(&it.left); return self.beside_class(&it.right); }
        if it.operator != BinaryOperator::Instanceof { return walk::walk_binary_expression(self, it); }
        self.visit_expression(&it.left);
        match strip(&it.right) { Expression::Identifier(id) if id.name == "Function" && self.global(id) => {}, right => self.beside_class(right) }
    }
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        let prop = it.property.name.as_str();
        if prop == "name" && let Some(inner) = class_of(&it.object) { return self.visit_expression(inner); }
        if let Some((_, kind)) = REACH.iter().find(|(p, _)| *p == prop) { self.add(format!("{kind}:{prop}")); }
        let this_global = self.global_this && matches!(strip(&it.object), Expression::ThisExpression(_));
        let aliased = matches!(&it.object, Expression::Identifier(obj) if self.alias(obj));
        if this_global || aliased || matches!(&it.object, Expression::Identifier(obj) if self.global(obj)) {
            let o = match &it.object { Expression::Identifier(obj) if !aliased => obj.name.as_str(), _ => "globalThis" };
            if o == "process" { if PROCESS.contains(&prop) { self.add(format!("process:{prop}")); } return; }
            if o == "module" { if MODULE.contains(&prop) { self.add(format!("module:{prop}")); } return; }
            if o == "require" { return self.add(format!("require:{}", if word(prop) { prop } else { "computed" })); }
            if o == "Bun" || o == "Deno" { return self.add(format!("{}:{}", o.to_ascii_lowercase(), if prop == "$" { "shell" } else if word(prop) { prop } else { "computed" })); }
            if GLOBALS.contains(&o) {
                self.global_member(prop);
                return;
            }
        }
        if prop == "constructor" || prop == "__proto__" { self.add(format!("code:{prop}")); }
        walk::walk_static_member_expression(self, it);
    }
    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        let prop = literal(&it.expression);
        if let Some(p) = prop && let Some((_, kind)) = REACH.iter().find(|(r, _)| *r == p) { self.add(format!("{kind}:{p}")); }
        let this_global = self.global_this && matches!(strip(&it.object), Expression::ThisExpression(_));
        let aliased = matches!(&it.object, Expression::Identifier(obj) if self.alias(obj));
        if this_global || aliased || matches!(&it.object, Expression::Identifier(obj) if self.global(obj)) {
            let o = match &it.object { Expression::Identifier(obj) if !aliased => obj.name.as_str(), _ => "globalThis" };
            let handled = match o {
                "process" => { self.add(format!("process:{}", prop.filter(|p| word(p)).unwrap_or("computed"))); true }
                "module" => { self.add(format!("module:{}", prop.filter(|p| word(p)).unwrap_or("computed"))); true }
                "require" => { self.add("require:computed".into()); true }
                "Bun" | "Deno" => { self.add(format!("{}:{}", o.to_ascii_lowercase(), prop.filter(|p| word(p)).unwrap_or("computed"))); true }
                g if GLOBALS.contains(&g) => { self.add("global:computed".into()); true }
                _ => false,
            };
            if handled { return self.visit_expression(&it.expression); }
        }
        if matches!(prop, Some("constructor" | "__proto__")) { self.add(format!("code:{}", prop.unwrap_or_default())); }
        walk::walk_computed_member_expression(self, it);
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        let n = it.name.as_str();
        if !self.global(it) { return; }
        if CODE_GLOBALS.contains(&n) { self.add(format!("code:{n}")); }
        else if n == "require" { self.add("require:dynamic".into()); }
        else if NETWORK.contains(&n) { self.add(format!("global:{n}")); }
        else if n == "process" || n == "module" { self.add(format!("{n}:dynamic")); }
        else if GLOBALS.contains(&n) { self.add("global:dynamic".into()); }
    }
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator.is_typeof() && matches!(it.argument, Expression::Identifier(_)) { return; }
        walk::walk_unary_expression(self, it);
    }
}

fn parse(name: &str, source: &str, st: SourceType) -> Option<BTreeSet<String>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, st).parse();
    if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let esm = matches!(extension(name), "mjs" | "mts");
    let mut v = Footprint { file: name, caps: BTreeSet::new(), scoping: semantic.scoping(), strict: esm, global_this: !esm, method_next: false, aliases: BTreeSet::new(), saved: Vec::new(), restored: BTreeMap::new() };
    v.visit_program(&parsed.program);
    for (s, cap) in std::mem::take(&mut v.saved) {
        if v.restored.get(&s).copied().unwrap_or(0) < v.scoping.get_resolved_reference_ids(s).len() { v.caps.insert(cap); }
    }
    Some(v.caps)
}
#[derive(Default)]
struct EnvReads { names: BTreeSet<String>, seen: usize, read: usize, bulk: bool, process: usize, members: usize }
fn bare<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> {
    match e {
        Expression::ParenthesizedExpression(x) => bare(&x.expression),
        Expression::TSNonNullExpression(x) => bare(&x.expression),
        Expression::TSAsExpression(x) => bare(&x.expression),
        Expression::TSSatisfiesExpression(x) => bare(&x.expression),
        _ => e,
    }
}
fn env_object(e: &Expression) -> bool {
    match bare(e) {
        Expression::StaticMemberExpression(m) if m.property.name == "env" => match bare(&m.object) {
            Expression::Identifier(o) => o.name == "process",
            Expression::StaticMemberExpression(p) => p.property.name == "process",
            _ => false,
        },
        _ => false,
    }
}
impl<'a> Visit<'a> for EnvReads {
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if env_object(&it.object) { self.names.insert(it.property.name.to_string()); self.read += 1; }
        if matches!(bare(&it.object), Expression::Identifier(o) if o.name == "process") { self.members += 1; }
        walk::walk_static_member_expression(self, it);
    }
    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if matches!(bare(&it.object), Expression::Identifier(o) if o.name == "process") { self.bulk = true; }
        if env_object(&it.object) {
            match bare(&it.expression) {
                Expression::StringLiteral(s) => { self.names.insert(s.value.to_string()); self.read += 1; }
                _ => self.bulk = true,
            }
        }
        walk::walk_computed_member_expression(self, it);
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init && env_object(init) {
            match &it.id {
                BindingPattern::ObjectPattern(o) if o.rest.is_none() => {
                    for p in &o.properties {
                        match p.key.static_name() { Some(n) if !p.computed => { self.names.insert(n.to_string()); } _ => self.bulk = true }
                    }
                    self.read += 1;
                }
                _ => self.bulk = true,
            }
        }
        walk::walk_variable_declarator(self, it);
    }
    fn visit_expression(&mut self, it: &Expression<'a>) {
        if matches!(it, Expression::StaticMemberExpression(_)) && env_object(it) { self.seen += 1; }
        if matches!(it, Expression::Identifier(o) if o.name == "process") { self.process += 1; }
        walk::walk_expression(self, it);
    }
    fn visit_unary_expression(&mut self, it: &UnaryExpression<'a>) {
        if it.operator.is_typeof() && matches!(bare(&it.argument), Expression::Identifier(_)) { return; }
        walk::walk_unary_expression(self, it);
    }
}
fn words(name: &str) -> Vec<String> {
    let (mut words, mut word, mut prev_lower) = (Vec::new(), String::new(), false);
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() { if !word.is_empty() { words.push(std::mem::take(&mut word)); } prev_lower = false; continue; }
        if c.is_ascii_uppercase() && prev_lower && !word.is_empty() { words.push(std::mem::take(&mut word)); }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        word.push(c.to_ascii_uppercase());
    }
    if !word.is_empty() { words.push(word); }
    words
}
const PUBLIC_PREFIXES: [&str; 9] = ["NEXT_PUBLIC_", "VITE_", "EXPO_PUBLIC_", "PUBLIC_", "REACT_APP_", "GATSBY_", "NUXT_PUBLIC_", "VUE_APP_", "STORYBOOK_"];
const CREDENTIALS: [&str; 24] = ["KEY", "KEYS", "APIKEY", "TOKEN", "TOKENS", "SECRET", "SECRETS", "PASSWORD", "PASSWORDS", "PASSWD", "PASS", "PASSPHRASE", "PWD", "CREDENTIAL",
    "CREDENTIALS", "CREDS", "CRED", "AUTH", "DSN", "SALT", "SK", "JWT", "OTP", "BEARER"];
const JOINED: [&str; 9] = ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "APIKEY", "PRIVATEKEY", "ACCESSKEY", "SECRETKEY", "PASSPHRASE"];
const LINKS: [&str; 5] = ["URL", "URI", "STRING", "CONNECTION", "CONN"];
const SERVICES: [&str; 22] = ["DATABASE", "DB", "MONGO", "MONGODB", "POSTGRES", "POSTGRESQL", "PG", "MYSQL", "MARIADB", "MSSQL", "REDIS", "VALKEY", "AMQP", "RABBITMQ", "SMTP",
    "KAFKA", "NATS", "ELASTICSEARCH", "OPENSEARCH", "WEBHOOK", "CLICKHOUSE", "CONNECTION"];
const QUALIFIERS: [&str; 18] = ["LIVE", "TEST", "PROD", "PRODUCTION", "DEV", "DEVELOPMENT", "STAGING", "STAGE", "LOCAL", "PREVIEW", "OLD", "NEW", "PRIMARY", "SECONDARY", "BASE64",
    "B64", "ENCODED", "RAW"];
const NAMED_KEYS: [&str; 8] = ["PUBLIC", "PUBLISHABLE", "CACHE", "SORT", "PRIMARY", "PARTITION", "STORAGE", "QUERY"];
pub fn secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper == "PWD" || upper == "OLDPWD" || PUBLIC_PREFIXES.iter().any(|p| upper.starts_with(p)) { return false; }
    let words = words(name);
    let qualifier = |w: &str| QUALIFIERS.contains(&w) || w.bytes().all(|b| b.is_ascii_digit()) || w.strip_prefix('V').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let mut end = words.len();
    while end > 1 && qualifier(&words[end - 1]) { end -= 1; }
    let Some((last, before)) = words[..end].split_last() else { return false };
    if last == "KEY" && before.last().is_some_and(|w| NAMED_KEYS.contains(&w.as_str())) { return false; }
    CREDENTIALS.contains(&last.as_str()) || JOINED.iter().any(|j| last.len() > j.len() && last.ends_with(j))
        || (LINKS.contains(&last.as_str()) && before.iter().any(|w| SERVICES.contains(&w.as_str())))
}
pub fn env_names(name: &str, source: &str) -> Option<BTreeSet<String>> {
    for st in source_types(name)? {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { continue; }
        let mut v = EnvReads::default();
        v.visit_program(&parsed.program);
        return (!v.bulk && v.read >= v.seen && v.process <= v.members).then_some(v.names);
    }
    None
}
pub fn source_types(name: &str) -> Option<Vec<SourceType>> {
    let st = SourceType::from_path(name).ok()?;
    Some(match extension(name) {
        "js" => vec![st, st.with_module(false), st.with_commonjs(true), st.with_jsx(true), st.with_module(false).with_jsx(true)],
        "jsx" => vec![st, st.with_module(false)],
        _ => vec![st],
    })
}
pub fn footprint(name: &str, source: &str) -> R<BTreeSet<String>> {
    let attempts = source_types(name).ok_or_else(|| Denied(format!("Unsupported code file: {name}")))?;
    attempts.into_iter().find_map(|st| parse(name, source, st)).ok_or_else(|| Denied(format!("Changed code cannot be parsed for capability analysis: {name}. Fix its syntax, or make this change yourself")))
}
fn analyze(name: &str, source: &str) -> R<Option<BTreeSet<String>>> {
    if is_code(name) { return footprint(name, source).map(Some); }
    Ok(if extension(name) == "json" { None } else { parse(name, source, SourceType::cjs()) })
}
fn inspectable(files: &Files, name: &str) -> Option<BTreeSet<String>> {
    std::str::from_utf8(files.get(name)?.bytes.as_deref()?).ok().and_then(|s| analyze(name, s).ok().flatten())
}
pub fn check(base: &Files, candidate: &Files, changed: &Value, allowances: &BTreeMap<String, Vec<String>>) -> R<Value> {
    let (mut report, mut siblings) = (Vec::new(), BTreeMap::<String, BTreeSet<String>>::new());
    let (mut widened, mut grant) = (Vec::new(), BTreeMap::<String, BTreeSet<String>>::new());
    let approved = crate::deps::imports(changed);
    for f in changed["files"].as_array().into_iter().flatten() {
        let (name, op) = (f["path"].as_str().unwrap_or(""), f["operation"].as_str().unwrap_or(""));
        if op == "delete" { continue; }
        ensure!(is_code(name) || is_data(name), "Unsupported changed file type: {name}. Kern can inspect JS/TS, JSON, Markdown, text, CSS and HTML; make this change yourself");
        let source = std::str::from_utf8(candidate.get(name).ok_or_else(|| Denied(format!("Missing changed file {name}")))?.bytes.as_deref().ok_or_else(|| Denied(format!("Changed file too large to inspect: {name}")))?).map_err(|_| Denied(format!("Invalid UTF-8: {name}")))?;
        let Some(after) = analyze(name, source)? else { continue };
        let before = if base.contains_key(name) { inspectable(base, name).unwrap_or_default() } else {
            let dir = name.rsplit_once('/').map_or("", |(d, _)| d).to_string();
            siblings.entry(dir.clone()).or_insert_with(|| base.keys().filter(|f| is_code(f) && f.rsplit_once('/').map_or("", |(d, _)| d) == dir).filter_map(|f| inspectable(base, f)).flatten().collect()).clone()
        };
        let granted: BTreeSet<&String> = allowances.iter().filter(|(p, _)| covers(p, name)).flat_map(|(_, caps)| caps).collect();
        let added: Vec<&String> = after.iter().filter(|c| !before.contains(*c) && !granted.contains(c) && !approved.contains(*c)).collect();
        if !added.is_empty() {
            widened.push(format!("{name} +[{}]", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            grant.entry(name.rsplit_once('/').map_or(name.to_string(), |(d, _)| format!("{d}/"))).or_default().extend(added.iter().map(|s| s.to_string()));
        }
        report.push(json!({ "path": name, "operation": op, "baseline": if base.contains_key(name) { "file" } else { "directory" }, "capabilities": after, "added": added }));
    }
    ensure!(widened.is_empty(), "Patch widens module capabilities: {}. If this is intended, allow it in operator.json: \"permissions\": {}", widened.join("; "), serde_json::to_string(&grant).unwrap_or_default());
    Ok(json!({ "policy": POLICY, "files": report }))
}
