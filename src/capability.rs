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
const NETWORK: [&str; 7] = ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "importScripts", "Worker", "SharedWorker"];
const REACH: [(&str, &str); 8] = [("mainModule", "process"), ("getBuiltinModule", "process"), ("dlopen", "process"), ("_linkedBinding", "process"), ("_load", "module"), ("_compile", "module"), ("_resolveFilename", "module"), ("sendBeacon", "global")];
const PROCESS: [&str; 10] = ["binding", "_linkedBinding", "dlopen", "getBuiltinModule", "mainModule", "env", "kill", "chdir", "setuid", "setgid"];
const MODULE: [&str; 5] = ["require", "constructor", "_load", "_compile", "_resolveFilename"];
const CODE_GLOBALS: [&str; 3] = ["eval", "Function", "WebAssembly"];

fn extension(name: &str) -> &str { name.rsplit_once('.').map_or("", |(_, e)| e) }
fn declaration(name: &str) -> bool { [".d.ts", ".d.mts", ".d.cts"].iter().any(|s| name.ends_with(s)) }
pub fn is_code(name: &str) -> bool { CODE.contains(&extension(name)) && !declaration(name) }
/// Contributor lists and change logs that often have no extension: text, inspected like the other data files.
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
        Some(("require" | "code" | "process" | "module" | "global", w)) => word(w),
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

struct Footprint<'s> { file: &'s str, caps: BTreeSet<String>, scoping: &'s Scoping, strict: bool, global_this: bool, method_next: bool }
impl Footprint<'_> {
    fn add(&mut self, c: String) { self.caps.insert(c); }
    fn global(&self, id: &IdentifierReference) -> bool { id.reference_id.get().is_none_or(|r| self.scoping.get_reference(r).symbol_id().is_none()) }
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
            return self.add(format!("url:{}", scheme.to_ascii_lowercase()));
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
        self.visit_assignment_target(&it.left);
        self.method_next = matches!(it.left, AssignmentTarget::StaticMemberExpression(_) | AssignmentTarget::ComputedMemberExpression(_)) && matches!(it.right, Expression::FunctionExpression(_));
        self.visit_expression(&it.right);
        self.method_next = false;
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
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
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        let prop = it.property.name.as_str();
        if let Some((_, kind)) = REACH.iter().find(|(p, _)| *p == prop) { self.add(format!("{kind}:{prop}")); }
        let this_global = self.global_this && matches!(strip(&it.object), Expression::ThisExpression(_));
        if this_global || matches!(&it.object, Expression::Identifier(obj) if self.global(obj)) {
            let o = if let Expression::Identifier(obj) = &it.object { obj.name.as_str() } else { "globalThis" };
            if o == "process" { if PROCESS.contains(&prop) { self.add(format!("process:{prop}")); } return; }
            if o == "module" { if MODULE.contains(&prop) { self.add(format!("module:{prop}")); } return; }
            if o == "require" { return self.add(format!("require:{}", if word(prop) { prop } else { "computed" })); }
            if GLOBALS.contains(&o) {
                if CODE_GLOBALS.contains(&prop) { self.add(format!("code:{prop}")); }
                if NETWORK.contains(&prop) || prop == "require" || prop == "process" { self.add(format!("global:{prop}")); }
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
        if this_global || matches!(&it.object, Expression::Identifier(obj) if self.global(obj)) {
            let o = if let Expression::Identifier(obj) = &it.object { obj.name.as_str() } else { "globalThis" };
            let handled = match o {
                "process" => { self.add(format!("process:{}", prop.filter(|p| word(p)).unwrap_or("computed"))); true }
                "module" => { self.add(format!("module:{}", prop.filter(|p| word(p)).unwrap_or("computed"))); true }
                "require" => { self.add("require:computed".into()); true }
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
    let mut v = Footprint { file: name, caps: BTreeSet::new(), scoping: semantic.scoping(), strict: esm, global_this: !esm, method_next: false };
    v.visit_program(&parsed.program);
    Some(v.caps)
}
/// The ways Kern reads a code file, in order: as its extension says, then for .js and .jsx as a script, and only then, for .js, with JSX,
/// which React projects write in plain .js files. Plain JavaScript that parses keeps its meaning.
pub fn source_types(name: &str) -> Option<Vec<SourceType>> {
    let st = SourceType::from_path(name).ok()?;
    Some(match extension(name) {
        "js" => vec![st, st.with_module(false), st.with_jsx(true), st.with_module(false).with_jsx(true)],
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
        let added: Vec<&String> = after.iter().filter(|c| !before.contains(*c) && !granted.contains(c)).collect();
        if !added.is_empty() {
            widened.push(format!("{name} +[{}]", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            grant.entry(name.rsplit_once('/').map_or(name.to_string(), |(d, _)| format!("{d}/"))).or_default().extend(added.iter().map(|s| s.to_string()));
        }
        report.push(json!({ "path": name, "operation": op, "baseline": if base.contains_key(name) { "file" } else { "directory" }, "capabilities": after, "added": added }));
    }
    ensure!(widened.is_empty(), "Patch widens module capabilities: {}. If this is intended, allow it in operator.json: \"permissions\": {}", widened.join("; "), serde_json::to_string(&grant).unwrap_or_default());
    Ok(json!({ "policy": POLICY, "files": report }))
}
