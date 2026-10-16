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

fn parts(text: &str) -> (Option<Version>, usize) {
    let rest = text.trim().trim_start_matches(['v', '=']);
    let n = rest.split('.').take_while(|p| !p.is_empty() && !["x", "X", "*"].contains(p)).count();
    (version(rest), n)
}
fn next(v: Version, n: usize) -> Version { match n { 1 => (v.0 + 1, 0, 0), 2 => (v.0, v.1 + 1, 0), _ => (v.0, v.1, v.2 + 1) } }
fn comparator(c: &str, v: Version) -> Option<bool> {
    if ["*", "x", "X", ""].contains(&c) { return Some(true); }
    let (op, rest) = [">=", "<=", ">", "<", "=", "^", "~"].iter().find_map(|o| c.strip_prefix(o).map(|r| (*o, r))).unwrap_or(("", c));
    if ["*", "x", "X", ""].contains(&rest.trim()) { return Some(op != "<" && op != ">"); }
    let (b, n) = parts(rest);
    let b = b?;
    if n == 0 { return None; }
    Some(match op {
        "" | "=" => if n == 3 { v == b } else { v >= b && v < next(b, n) },
        ">=" => v >= b,
        ">" => if n == 3 { v > b } else { v >= next(b, n) },
        "<" => v < b,
        "<=" => if n == 3 { v <= b } else { v < next(b, n) },
        "~" => v >= b && v < if n == 1 { (b.0 + 1, 0, 0) } else { (b.0, b.1 + 1, 0) },
        _ => v >= b && v < if b.0 > 0 || n == 1 { (b.0 + 1, 0, 0) } else if b.1 > 0 || n == 2 { (0, b.1 + 1, 0) } else { (0, 0, b.2 + 1) },
    })
}
pub fn satisfies(range: &str, v: Version) -> Option<bool> {
    let mut any = false;
    for alt in range.split("||") {
        let alt = alt.trim();
        any |= match alt.split_once(" - ") {
            Some((a, b)) => {
                let (lo, (hi, n)) = (parts(a).0?, parts(b));
                v >= lo && if n == 3 { v <= hi? } else { v < next(hi?, n.max(1)) }
            }
            None => {
                let mut ok = true;
                let mut words = alt.split_whitespace().peekable();
                while let Some(w) = words.next() {
                    let c = if [">=", "<=", ">", "<", "=", "^", "~"].contains(&w) { format!("{w}{}", words.next()?) } else { w.to_string() };
                    ok &= comparator(&c, v)?;
                }
                ok
            }
        };
    }
    Some(any)
}
pub const NODE_MAJORS: [u64; 5] = [18, 20, 22, 24, 26];
fn spec_major(spec: &str) -> Option<u64> {
    let s = spec.trim().trim_matches(['"', '\'']).trim_start_matches('v').to_ascii_lowercase();
    let named = |n: &str| -> Option<u64> {
        Some(match n { "argon" => 4, "boron" => 6, "carbon" => 8, "dubnium" => 10, "erbium" => 12, "fermium" => 14, "gallium" => 16, "hydrogen" => 18, "iron" => 20, "jod" => 22, "krypton" => 24, _ => return None })
    };
    match s.as_str() {
        "node" | "latest" | "current" | "stable" => Some(26),
        "lts" | "lts/*" | "lts/latest" => Some(24),
        x if x.starts_with("lts/") => named(&x[4..]),
        x => x.split(['.', ' ', '-']).next()?.parse().ok(),
    }
}
pub fn supported_major(m: u64) -> u64 { NODE_MAJORS.iter().copied().find(|s| *s >= m).unwrap_or(NODE_MAJORS[NODE_MAJORS.len() - 1]) }
pub fn wanted_major(read: &dyn Fn(&str) -> Option<String>, workflows: &[String]) -> Option<(u64, String)> {
    let package = read("package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let range = package.as_ref().and_then(|p| p["engines"]["node"].as_str()).map(str::to_string);
    let allowed = |m: u64| range.as_deref().is_none_or(|r| satisfies(r, (m, 999, 999)) != Some(false));
    let chosen = declared(read, workflows, package.as_ref());
    match (chosen, &range) {
        (Some((m, from)), _) if allowed(m) => Some((m, from)),
        (_, Some(r)) => {
            let m = if allowed(22) { 22 } else { NODE_MAJORS.iter().copied().find(|m| allowed(*m))? };
            Some((m, format!("package.json's engines.node is {r}")))
        }
        (chosen, None) => chosen,
    }
}
fn declared(read: &dyn Fn(&str) -> Option<String>, workflows: &[String], package: Option<&serde_json::Value>) -> Option<(u64, String)> {
    let mut ci: Vec<u64> = Vec::new();
    for w in workflows {
        for l in w.lines().map(str::trim) {
            let Some(value) = l.strip_prefix("node-version:").or_else(|| l.strip_prefix("node_version:")).map(|v| v.split(" #").next().unwrap_or(v).trim()) else { continue };
            if value.contains("${{") { continue; }
            ci.extend(value.trim_matches(['[', ']']).split(',').filter_map(spec_major));
        }
    }
    if !ci.is_empty() {
        let m = if ci.contains(&22) { 22 } else { *ci.iter().max()? };
        return Some((supported_major(m), format!("the CI workflow sets up Node {m}")));
    }
    let first = |t: &str| t.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string);
    for f in [".nvmrc", ".node-version"] {
        if let Some(line) = read(f).as_deref().and_then(first) && let Some(m) = spec_major(&line) { return Some((supported_major(m), format!("{f} says {line}"))); }
    }
    if let Some(t) = read(".tool-versions") {
        for l in t.lines().map(str::trim) {
            let mut w = l.split_whitespace();
            if matches!(w.next(), Some("nodejs" | "node")) && let Some(v) = w.next() && let Some(m) = spec_major(v) { return Some((supported_major(m), format!(".tool-versions says {v}"))); }
        }
    }
    if let Some(v) = package.and_then(|p| p["volta"]["node"].as_str()) && let Some(m) = spec_major(v) { return Some((supported_major(m), format!("package.json's volta.node is {v}"))); }
    None
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
        Some(scan.found.into_iter().filter(|k| table().contains_key(k) && (!k.starts_with("global:") || k.starts_with("global:import.meta."))).collect())
    })
}
