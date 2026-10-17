use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bound { Function, Variable { start: usize, constant: bool }, Class }
const RUNTIME: &str = "/kern/trace.cjs";
const IGNORE: &str = "// @ts-ignore\n";

fn add_variables(out: &mut BTreeMap<String, Bound>, v: &VariableDeclaration, source: &str) {
    let start = v.span.start as usize;
    let constant = v.kind == VariableDeclarationKind::Const && source.get(start..start + 5) == Some("const");
    for d in &v.declarations {
        if let BindingPattern::BindingIdentifier(id) = &d.id { out.insert(id.name.to_string(), Bound::Variable { start, constant }); }
    }
}
fn bindings(program: &Program, source: &str) -> BTreeMap<String, Bound> {
    let mut out = BTreeMap::new();
    for s in &program.body {
        let declaration = match s {
            Statement::ExportDeclaration(e) => Some(&e.declaration),
            _ => s.as_declaration(),
        };
        match declaration {
            Some(Declaration::VariableDeclaration(v)) if !v.declare => add_variables(&mut out, v, source),
            Some(Declaration::FunctionDeclaration(f)) if !f.declare && f.body.is_some() => { if let Some(id) = &f.id { out.insert(id.name.to_string(), Bound::Function); } }
            Some(Declaration::ClassDeclaration(c)) if !c.declare => { if let Some(id) = &c.id { out.insert(id.name.to_string(), Bound::Class); } }
            _ => {}
        }
        if let Statement::ExportDefaultDeclaration(e) = s {
            match &e.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) => { if let Some(id) = &f.id { out.insert(id.name.to_string(), Bound::Function); } }
                ExportDefaultDeclarationKind::ClassDeclaration(c) => { if let Some(id) = &c.id { out.insert(id.name.to_string(), Bound::Class); } }
                _ => {}
            }
        }
    }
    out
}

fn exported(program: &Program) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for s in &program.body {
        match s {
            Statement::ExportDeclaration(e) => match &e.declaration {
                Declaration::VariableDeclaration(v) => {
                    for d in &v.declarations { if let BindingPattern::BindingIdentifier(id) = &d.id { out.push((id.name.to_string(), id.name.to_string())); } }
                }
                Declaration::FunctionDeclaration(f) => { if let Some(id) = &f.id { out.push((id.name.to_string(), id.name.to_string())); } }
                Declaration::ClassDeclaration(c) => { if let Some(id) = &c.id { out.push((id.name.to_string(), id.name.to_string())); } }
                _ => {}
            },
            Statement::ExportNamedDeclaration(e) if !e.export_kind.is_type() => {
                for spec in e.specifiers.iter().filter(|sp| !sp.export_kind.is_type()) {
                    if let ModuleExportName::IdentifierReference(local) = &spec.local { out.push((local.name.to_string(), spec.exported.name().to_string())); }
                }
            }
            Statement::ExportDefaultDeclaration(e) => match &e.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(f) => { if let Some(id) = &f.id { out.push((id.name.to_string(), id.name.to_string())); } }
                ExportDefaultDeclarationKind::ClassDeclaration(c) => { if let Some(id) = &c.id { out.push((id.name.to_string(), id.name.to_string())); } }
                ExportDefaultDeclarationKind::Identifier(id) => out.push((id.name.to_string(), "default".to_string())),
                _ => {}
            },
            _ => {}
        }
    }
    out
}

fn es_module(path: &str, source: &str, program: &Program) -> Option<String> {
    let bound = bindings(program, source);
    let wanted: Vec<(String, String, Bound)> = exported(program).into_iter().filter_map(|(local, name)| bound.get(&local).map(|b| (local, name, *b))).collect();
    if wanted.is_empty() { return None; }
    let mut lets: Vec<usize> = wanted.iter().filter_map(|(_, _, b)| match b { Bound::Variable { start, constant: true } => Some(*start), _ => None }).collect();
    lets.sort_unstable();
    lets.dedup();
    let mut out = String::with_capacity(source.len() + 512);
    let mut at = 0;
    for start in lets {
        out.push_str(&source[at..start]);
        out.push_str("let");
        at = start + 5;
    }
    out.push_str(&source[at..]);
    if !out.ends_with('\n') { out.push('\n'); }
    let file = serde_json::to_string(path).ok()?;
    out.push_str(&format!("{IGNORE}import * as __kern_trace_module from \"{RUNTIME}\";\n{IGNORE}const __kern_trace = __kern_trace_module.default ?? __kern_trace_module;\n"));
    let mut seen = std::collections::BTreeSet::new();
    for (local, name, b) in wanted {
        if !seen.insert(local.clone()) { continue; }
        let label = serde_json::to_string(&name).ok()?;
        out.push_str(IGNORE);
        out.push_str(&match b {
            Bound::Function => format!("{local} = __kern_trace.fn({file}, {label}, {local});\n"),
            Bound::Variable { .. } => format!("{local} = __kern_trace.value({file}, {label}, {local});\n"),
            Bound::Class => format!("__kern_trace.class({file}, {label}, {local});\n"),
        });
    }
    Some(out)
}

const TRACE_LINES_COUNT_MAX: usize = 200_000;
const DIFFERENCES_COUNT_MAX: usize = 20;
const EMPTY: [&str; 6] = ["null", "undefined", "[]", "{}", "\"\"", "NaN"];
type Calls = BTreeMap<(String, String), std::collections::BTreeSet<String>>;
fn calls(text: &str) -> Calls {
    let mut out = Calls::new();
    for line in text.lines().take(TRACE_LINES_COUNT_MAX) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let (Some(k), Some(a), Some(o)) = (v["k"].as_str(), v["a"].as_str(), v["o"].as_str()) else { continue };
        out.entry((k.to_string(), a.to_string())).or_default().insert(o.to_string());
    }
    out
}
fn failed(o: &str) -> bool { o.starts_with("throws ") || o.starts_with("rejects ") }
fn empty(o: &str) -> bool { o.split_once(' ').is_some_and(|(verb, v)| (verb == "returns" || verb == "resolves") && EMPTY.contains(&v)) }
fn kind(was: &std::collections::BTreeSet<String>, now: &std::collections::BTreeSet<String>) -> (u8, &'static str) {
    let (was_fails, now_fails) = (was.iter().any(|o| failed(o)), now.iter().any(|o| failed(o)));
    if was.iter().all(|o| failed(o)) && !now_fails { (0, "accepts") }
    else if !was_fails && now_fails { (1, "throws") }
    else if !was.iter().any(|o| empty(o)) && now.iter().all(|o| empty(o)) { (2, "empty") }
    else { (3, "changed") }
}
pub fn compare(old: &str, again: &str, new: &str) -> serde_json::Value {
    let (old, again, new) = (calls(old), calls(again), calls(new));
    let (mut compared, mut unstable, mut found) = (0usize, 0usize, Vec::new());
    for (key, now) in &new {
        let Some(was) = old.get(key) else { continue };
        compared += 1;
        if again.get(key) != Some(was) { unstable += 1; continue; }
        if was == now { continue; }
        let (rank, kind) = kind(was, now);
        found.push((rank, key, kind, was, now));
    }
    found.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let joined = |s: &std::collections::BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join(" | ");
    let differences: Vec<serde_json::Value> = found.iter().take(DIFFERENCES_COUNT_MAX)
        .map(|(_, (k, a), kind, was, now)| serde_json::json!({ "key": k, "input": a, "was": joined(was), "now": joined(now), "kind": kind })).collect();
    serde_json::json!({ "compared": compared, "unstable": unstable, "changed": found.len(), "differences": differences })
}

pub fn merged(a: &serde_json::Value, b: &serde_json::Value) -> serde_json::Value {
    if a["status"] != "RAN" { return if b["status"] == "RAN" || a.get("status").is_none() { b.clone() } else { a.clone() }; }
    if b["status"] != "RAN" { return a.clone(); }
    let sum = |k: &str| a[k].as_u64().unwrap_or(0) + b[k].as_u64().unwrap_or(0);
    let differences: Vec<serde_json::Value> = a["differences"].as_array().into_iter().flatten().chain(b["differences"].as_array().into_iter().flatten()).take(DIFFERENCES_COUNT_MAX).cloned().collect();
    serde_json::json!({ "status": "RAN", "compared": sum("compared"), "unstable": sum("unstable"), "changed": sum("changed"), "tests": sum("tests"), "differences": differences })
}

pub fn instrument(path: &str, source: &str) -> Option<String> {
    if !crate::capability::is_code(path) { return None; }
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let program = &parsed.program;
        if program.body.iter().any(Statement::is_module_declaration) { return Some(es_module(path, source, program)); }
        let commonjs = source.contains("module.exports") || source.contains("exports.");
        let file = serde_json::to_string(path).ok()?;
        Some(commonjs.then(|| format!("{source}{}module.exports = require(\"{RUNTIME}\").exports({file}, module.exports);\n", if source.ends_with('\n') { "" } else { "\n" })))
    }).flatten()
}
