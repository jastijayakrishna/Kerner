use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_syntax::module_record::ExportExportName;
use serde_json::Value;
use std::collections::BTreeSet;

const BUILD_DIRS: [&str; 9] = ["dist", "lib", "build", "out", "esm", "cjs", "es", "types", "module"];
const SOURCE_EXTENSIONS: [&str; 6] = ["ts", "tsx", "mts", "js", "jsx", "mjs"];

pub fn contracts(manifest: &Value) -> BTreeSet<String> {
    fn walk(v: &Value, path: String, depth: usize, out: &mut BTreeSet<String>) {
        if depth > 16 { return; }
        match v {
            Value::String(_) => { out.insert(path); }
            Value::Object(o) => { for (k, v) in o { walk(v, format!("{path}/{k}"), depth + 1, out); } }
            Value::Array(a) => { for v in a { walk(v, path.clone(), depth + 1, out); } }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(&manifest["exports"], "exports".into(), 0, &mut out);
    out
}

pub fn exported(path: &str, source: &str) -> Option<(BTreeSet<String>, Vec<String>)> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let m = &parsed.module_record;
        let mut names = BTreeSet::new();
        for e in m.local_export_entries.iter().chain(m.indirect_export_entries.iter()) {
            match &e.export_name {
                ExportExportName::Name(n) => { names.insert(n.name.to_string()); }
                ExportExportName::Default(_) => { names.insert("default".to_string()); }
                ExportExportName::Null => {}
            }
        }
        Some((names, m.star_export_entries.iter().filter_map(|e| e.module_request.as_ref().map(|r| r.name.to_string())).collect()))
    })
}

fn resolve(from: &str, spec: &str, read: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if !spec.starts_with("./") && !spec.starts_with("../") { return None; }
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in spec.split('/') {
        match seg { "" | "." => {} ".." => { parts.pop()?; } s => parts.push(s) }
    }
    let p = parts.join("/");
    let stem = [".js", ".mjs", ".cjs", ".jsx"].iter().find_map(|e| p.strip_suffix(e)).unwrap_or(&p).to_string();
    let mut candidates = vec![p.clone()];
    candidates.extend(SOURCE_EXTENSIONS.iter().chain(&["cts", "cjs"]).map(|e| format!("{stem}.{e}")));
    candidates.extend(SOURCE_EXTENSIONS.iter().map(|e| format!("{p}/index.{e}")));
    candidates.into_iter().find(|c| crate::capability::is_code(c) && read(c).is_some())
}

pub fn surface(read: &dyn Fn(&str) -> Option<String>, entry: &str) -> Option<BTreeSet<String>> {
    fn walk(read: &dyn Fn(&str) -> Option<String>, file: &str, depth: usize, seen: &mut BTreeSet<String>, out: &mut BTreeSet<String>) -> Option<()> {
        if depth > 4 || !seen.insert(file.to_string()) { return Some(()); }
        let (names, stars) = exported(file, &read(file)?)?;
        out.extend(names);
        for spec in stars {
            let Some(target) = resolve(file, &spec, read) else { continue };
            let mut inner = BTreeSet::new();
            walk(read, &target, depth + 1, seen, &mut inner)?;
            inner.remove("default");
            out.extend(inner);
        }
        Some(())
    }
    let mut out = BTreeSet::new();
    walk(read, entry, 0, &mut BTreeSet::new(), &mut out)?;
    Some(out)
}

pub fn entries(manifest: &Value) -> Vec<String> {
    fn targets(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Object(o) => { for (k, x) in o { if k != "./package.json" { targets(x, out); } } }
            Value::Array(a) => { for x in a { targets(x, out); } }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for key in ["main", "module", "types", "typings", "browser", "source"] { if let Some(s) = manifest[key].as_str() { found.push(s.to_string()); } }
    targets(&manifest["exports"], &mut found);
    let mut out: Vec<String> = Vec::new();
    let mut add = |p: String| { if !p.is_empty() && !out.contains(&p) { out.push(p); } };
    for t in found.iter().map(|t| t.trim_start_matches("./").to_string()).filter(|t| !t.contains('*') && !t.split('/').any(|s| s == "..")) {
        add(t.clone());
        let mut rest: Vec<&str> = t.split('/').collect();
        while rest.len() > 1 && BUILD_DIRS.contains(&rest[0]) { rest.remove(0); }
        let rest = rest.join("/");
        let stem = [".d.ts", ".d.mts", ".js", ".mjs", ".cjs", ".jsx", ".ts", ".mts", ".tsx"].iter().find_map(|e| rest.strip_suffix(e)).unwrap_or(&rest).to_string();
        for e in SOURCE_EXTENSIONS { add(format!("src/{stem}.{e}")); }
    }
    for e in SOURCE_EXTENSIONS { add(format!("src/index.{e}")); }
    for e in ["ts", "js", "mjs"] { add(format!("index.{e}")); }
    out
}
