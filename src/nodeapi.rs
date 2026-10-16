use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_syntax::symbol::SymbolId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub type Version = (u64, u64, u64);

const BUILTINS: [&str; 56] = ["assert", "assert/strict", "async_hooks", "buffer", "child_process", "cluster", "console", "constants", "crypto", "dgram", "diagnostics_channel",
    "dns", "dns/promises", "domain", "events", "fs", "fs/promises", "http", "http2", "https", "inspector", "inspector/promises", "module", "net", "os", "path", "path/posix",
    "path/win32", "perf_hooks", "process", "punycode", "querystring", "readline", "readline/promises", "repl", "stream", "stream/consumers", "stream/promises", "stream/web",
    "string_decoder", "sys", "timers", "timers/promises", "tls", "trace_events", "tty", "url", "util", "util/types", "v8", "vm", "wasi", "worker_threads", "zlib", "http_parser",
    "stream/iter"];

struct Entry { versions: Vec<Version>, unflagged: Option<Version> }

fn version(v: &str) -> Option<Version> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let number = |p: Option<&str>| -> Option<u64> { match p { None => Some(0), Some(x) if ["x", "X", "*"].contains(&x) => Some(0), Some(x) => x.parse().ok() } };
    let major = parts.next().filter(|m| !m.is_empty())?.parse().ok()?;
    Some((major, number(parts.next())?, number(parts.next())?))
}

fn table() -> &'static BTreeMap<String, Entry> {
    static TABLE: OnceLock<BTreeMap<String, Entry>> = OnceLock::new();
    TABLE.get_or_init(|| include_str!("node-api.tsv").lines().filter_map(|line| {
        let mut f = line.split('\t');
        let (kind, name, versions, unflagged) = (f.next()?, f.next()?, f.next()?, f.next().unwrap_or(""));
        (kind != "docs").then(|| (format!("{kind}:{name}"), Entry { versions: versions.split(',').filter_map(version).collect(), unflagged: version(unflagged) }))
    }).collect())
}

pub fn needs(key: &str, v: Version) -> Option<String> {
    let e = table().get(key)?;
    let show = |x: Version| format!("{}.{}.{}", x.0, x.1, x.2);
    if let Some(u) = e.unflagged { return (v < u).then(|| show(u)); }
    let last = *e.versions.iter().max()?;
    if v >= last { return None; }
    match e.versions.iter().find(|a| a.0 == v.0) {
        Some(a) if v >= *a => None,
        Some(a) => Some(show(*a)),
        None => Some(show(last)),
    }
}

fn bound(text: &str) -> Option<Version> {
    let (op, rest) = [">=", ">", "=", "^", "~"].iter().find_map(|o| text.strip_prefix(o).map(|r| (*o, r))).unwrap_or(("", text));
    if ["*", "x", "X", ""].contains(&rest) { return Some((0, 0, 0)); }
    let v = version(rest)?;
    let parts = rest.trim_start_matches('v').split('.').filter(|p| !["x", "X", "*"].contains(p)).count();
    Some(match (op, parts) { (">", 1) => (v.0 + 1, 0, 0), (">", 2) => (v.0, v.1 + 1, 0), (">", _) => (v.0, v.1, v.2 + 1), _ => v })
}

pub fn lowest(range: &str) -> Option<Version> {
    let mut low: Option<Version> = None;
    for alt in range.split("||") {
        let alt = alt.trim();
        let floor = match alt.split_once(" - ") {
            Some((a, _)) => bound(a.trim())?,
            None => {
                let mut floor = (0, 0, 0);
                for c in alt.split_whitespace().filter(|c| !c.starts_with('<')) { floor = floor.max(bound(c)?); }
                floor
            }
        };
        low = Some(low.map_or(floor, |l: Version| l.min(floor)));
    }
    low.filter(|l| *l != (0, 0, 0))
}

fn builtin(spec: &str) -> Option<(String, String)> {
    match spec.strip_prefix("node:") {
        Some(name) => {
            let prefixed = format!("module:node:{name}");
            Some((if table().contains_key(&prefixed) { prefixed } else { format!("module:{name}") }, name.to_string()))
        }
        None => BUILTINS.contains(&spec).then(|| (format!("module:{spec}"), spec.to_string())),
    }
}

fn strip<'b, 'a>(e: &'b Expression<'a>) -> &'b Expression<'a> {
    match e {
        Expression::ParenthesizedExpression(p) => strip(&p.expression),
        Expression::AwaitExpression(a) => strip(&a.argument),
        Expression::TSAsExpression(x) => strip(&x.expression),
        Expression::TSNonNullExpression(x) => strip(&x.expression),
        _ => e,
    }
}

struct Scan<'s> { scoping: &'s Scoping, found: BTreeSet<String>, aliases: BTreeMap<SymbolId, String> }
impl Scan<'_> {
    fn symbol(&self, id: &IdentifierReference) -> Option<SymbolId> { id.reference_id.get().and_then(|r| self.scoping.get_reference(r).symbol_id()) }
    fn global(&self, id: &IdentifierReference) -> bool { self.symbol(id).is_none() }
    fn alias(&self, id: &IdentifierReference) -> Option<String> { self.symbol(id).and_then(|s| self.aliases.get(&s).cloned()) }
    fn required(&self, e: &Expression) -> Option<(String, String)> {
        match strip(e) {
            Expression::CallExpression(c) => match (&c.callee, c.arguments.first()) {
                (Expression::Identifier(id), Some(Argument::StringLiteral(s))) if id.name == "require" && self.global(id) && c.arguments.len() == 1 => builtin(s.value.as_str()),
                _ => None,
            },
            Expression::ImportExpression(i) => match &i.source { Expression::StringLiteral(s) => builtin(s.value.as_str()), _ => None },
            _ => None,
        }
    }
}
impl<'a> Visit<'a> for Scan<'_> {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) {
        if it.import_kind.is_type() { return; }
        let Some((key, prefix)) = builtin(it.source.value.as_str()) else { return };
        self.found.insert(key);
        for s in it.specifiers.iter().flatten() {
            match s {
                ImportDeclarationSpecifier::ImportSpecifier(x) => { self.found.insert(format!("member:{prefix}.{}", x.imported.name())); }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(x) => { if let Some(sym) = x.local.symbol_id.get() { self.aliases.insert(sym, prefix.clone()); } }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(x) => { if let Some(sym) = x.local.symbol_id.get() { self.aliases.insert(sym, prefix.clone()); } }
            }
        }
    }
    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) { if let Some((k, _)) = builtin(it.source.value.as_str()) { self.found.insert(k); } }
    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) { if let Some((k, _)) = builtin(it.source.value.as_str()) { self.found.insert(k); } }
    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        if let Expression::StringLiteral(s) = &it.source && let Some((k, _)) = builtin(s.value.as_str()) { self.found.insert(k); }
        walk::walk_import_expression(self, it);
    }
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let (Expression::Identifier(id), Some(Argument::StringLiteral(s))) = (&it.callee, it.arguments.first())
            && id.name == "require" && it.arguments.len() == 1 && self.global(id) && let Some((k, _)) = builtin(s.value.as_str()) {
            self.found.insert(k);
        }
        walk::walk_call_expression(self, it);
    }
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(init) = &it.init && let Some((key, prefix)) = self.required(init) {
            self.found.insert(key);
            match &it.id {
                BindingPattern::BindingIdentifier(b) => { if let Some(s) = b.symbol_id.get() { self.aliases.insert(s, prefix); } }
                BindingPattern::ObjectPattern(o) => { for p in &o.properties { if let Some(k) = p.key.static_name() { self.found.insert(format!("member:{prefix}.{k}")); } } }
                _ => {}
            }
        }
        walk::walk_variable_declarator(self, it);
    }
    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        let prop = it.property.name.as_str();
        match strip(&it.object) {
            Expression::Identifier(id) => match self.alias(id) {
                Some(m) => { self.found.insert(format!("member:{m}.{prop}")); }
                None if self.global(id) => {
                    let o = id.name.as_str();
                    self.found.insert(if o == "process" || o == "Buffer" { format!("member:{o}.{prop}") } else { format!("global:{o}.{prop}") });
                }
                None => {}
            },
            Expression::StaticMemberExpression(inner) if inner.property.name == "promises" => {
                if let Expression::Identifier(id) = strip(&inner.object) && let Some(m) = self.alias(id) { self.found.insert(format!("member:{m}/promises.{prop}")); }
            }
            Expression::ImportMeta(_) => { self.found.insert(format!("global:import.meta.{prop}")); }
            other => { if let Some((_, m)) = self.required(other) { self.found.insert(format!("member:{m}.{prop}")); } }
        }
        walk::walk_static_member_expression(self, it);
    }
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) { if self.global(it) { self.found.insert(format!("global:{}", it.name)); } }
}

pub fn uses(path: &str, source: &str) -> Option<BTreeSet<String>> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        let mut scan = Scan { scoping: semantic.scoping(), found: BTreeSet::new(), aliases: BTreeMap::new() };
        scan.visit_program(&parsed.program);
        Some(scan.found.into_iter().filter(|k| table().contains_key(k)).collect())
    })
}
