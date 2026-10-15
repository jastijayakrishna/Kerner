use crate::capability::source_types;
use crate::git::{git, git_with, trimmed};
use crate::policy::Limits;
use crate::util::*;
use crate::{deny, ensure};
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::GetSpan;
use oxc_syntax::scope::ScopeFlags;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub fn changed(old: &str, new: &str) -> (Vec<u32>, Vec<u32>) {
    fn split(s: &str) -> Vec<&str> { if s.is_empty() { Vec::new() } else { s.split('\n').collect() } }
    let (a, b) = (split(old), split(new));
    let number = |v: Vec<usize>| v.into_iter().map(|i| i as u32 + 1).collect();
    match crate::compare::changed_lines(&a, &b) {
        Some((x, y)) => (number(x), number(y)),
        None => (number((0..a.len()).collect()), number((0..b.len()).collect())),
    }
}

struct Inert { spans: Vec<(u32, u32)>, inner: Vec<(u32, u32)> }
impl<'a> Visit<'a> for Inert {
    fn visit_import_declaration(&mut self, it: &ImportDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_ts_import_equals_declaration(&mut self, it: &TSImportEqualsDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_export_named_declaration(&mut self, it: &ExportNamedDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_export_from_declaration(&mut self, it: &ExportFromDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_export_all_declaration(&mut self, it: &ExportAllDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_ts_interface_declaration(&mut self, it: &TSInterfaceDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_ts_type_alias_declaration(&mut self, it: &TSTypeAliasDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_ts_type(&mut self, it: &TSType<'a>) { self.inner.push((it.span().start, it.span().end)); }
    fn visit_variable_declaration(&mut self, it: &VariableDeclaration<'a>) {
        if it.declare { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_variable_declaration(self, it); }
    }
    fn visit_function(&mut self, it: &Function<'a>, flags: ScopeFlags) {
        if it.declare || it.body.is_none() { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_function(self, it, flags); }
    }
    fn visit_class(&mut self, it: &Class<'a>) {
        if it.declare { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_class(self, it); }
    }
    fn visit_property_definition(&mut self, it: &PropertyDefinition<'a>) {
        if it.declare || (it.value.is_none() && it.decorators.is_empty() && !it.computed) { self.spans.push((it.span.start, it.span.end)); }
        else { walk::walk_property_definition(self, it); }
    }
    fn visit_ts_enum_declaration(&mut self, it: &TSEnumDeclaration<'a>) {
        if it.declare { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_ts_enum_declaration(self, it); }
    }
    fn visit_ts_namespace_declaration(&mut self, it: &TSNamespaceDeclaration<'a>) {
        if it.declare { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_ts_namespace_declaration(self, it); }
    }
    fn visit_ts_external_module_declaration(&mut self, it: &TSExternalModuleDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_ts_global_declaration(&mut self, it: &TSGlobalDeclaration<'a>) { self.spans.push((it.span.start, it.span.end)); }
    fn visit_jsx_expression_container(&mut self, it: &JSXExpressionContainer<'a>) {
        if matches!(it.expression, JSXExpression::EmptyExpression(_)) { self.spans.push((it.span.start, it.span.end)); } else { walk::walk_jsx_expression_container(self, it); }
    }
}

fn clip(line: &str) -> String { if line.chars().count() > 200 { format!("{}...", line.chars().take(200).collect::<String>()) } else { line.to_string() } }
fn closing(t: &str) -> bool { !t.is_empty() && t.trim_start_matches([')', ']', '}']).len() < t.len() && t.trim_start_matches([')', ']', '}']).chars().all(|c| matches!(c, ';' | ',' | ')')) }

pub fn measured(path: &str, text: &str) -> BTreeMap<u32, String> {
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0u32;
    for l in text.split('\n') { starts.push(at); at += l.len() as u32 + 1; }
    let line_of = |pos: u32| -> usize { starts.partition_point(|s| *s <= pos).saturating_sub(1) };
    let mut quiet = vec![false; text.len() + 1];
    let mut skip = BTreeSet::new();
    let parsed = source_types(path).into_iter().flatten().find_map(|st| {
        let alloc = Allocator::default();
        let p = Parser::new(&alloc, text, st).parse();
        (!p.fatal_error && p.diagnostics.is_empty()).then(|| {
            let mut v = Inert { spans: Vec::new(), inner: Vec::new() };
            v.visit_program(&p.program);
            let comments: Vec<(u32, u32)> = p.program.comments.iter().map(|c| (c.span.start, c.span.end)).collect();
            (v, comments)
        })
    });
    if let Some((v, comments)) = &parsed {
        for (s, e) in comments.iter().chain(v.spans.iter().filter(|(s, e)| text.get(*s as usize..*e as usize).is_some_and(|x| x.starts_with('{')))) {
            for b in &mut quiet[*s as usize..(*e as usize).min(text.len())] { *b = true; }
        }
        for (s, e) in &v.spans { for l in line_of(*s)..=line_of(e.saturating_sub(1).max(*s)) { skip.insert(l); } }
        for (s, e) in &v.inner { for l in line_of(*s) + 1..line_of(e.saturating_sub(1).max(*s)) { skip.insert(l); } }
    }
    let mut out = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.is_empty() || skip.contains(&i) || closing(t) { continue; }
        let start = starts[i] as usize;
        let code = line.char_indices().any(|(o, c)| !c.is_whitespace() && !quiet.get(start + o).copied().unwrap_or(false));
        let comment_only = parsed.is_none() && (t.starts_with("//") || t.starts_with("/*") || t.starts_with('*'));
        if code && !comment_only { out.insert(i as u32 + 1, clip(line)); }
    }
    out
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Exports { pub functions: Vec<String>, pub cjs: bool, pub default: bool }
fn function_value(e: &Expression) -> bool {
    match e {
        Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_) => true,
        Expression::ParenthesizedExpression(p) => function_value(&p.expression),
        Expression::TSAsExpression(x) => function_value(&x.expression),
        Expression::TSSatisfiesExpression(x) => function_value(&x.expression),
        _ => false,
    }
}
fn module_exports(t: &AssignmentTarget) -> bool {
    let AssignmentTarget::StaticMemberExpression(m) = t else { return false };
    match &m.object {
        Expression::Identifier(o) => (o.name == "module" && m.property.name == "exports") || o.name == "exports",
        Expression::StaticMemberExpression(inner) => matches!(&inner.object, Expression::Identifier(o) if o.name == "module") && inner.property.name == "exports",
        _ => false,
    }
}
pub fn exported(path: &str, text: &str) -> Exports {
    let Some(types) = source_types(path) else { return Exports::default() };
    for st in types {
        let alloc = Allocator::default();
        let p = Parser::new(&alloc, text, st).parse();
        if p.fatal_error || !p.diagnostics.is_empty() { continue; }
        let body = &p.program.body;
        let mut local: BTreeSet<String> = BTreeSet::new();
        for s in body {
            let decl = match s { Statement::ExportDeclaration(d) => Some(&d.declaration), other => other.as_declaration() };
            match decl {
                Some(Declaration::FunctionDeclaration(f)) if f.body.is_some() => { if let Some(id) = &f.id { local.insert(id.name.to_string()); } }
                Some(Declaration::VariableDeclaration(v)) if !v.declare => {
                    for d in &v.declarations {
                        if let (BindingPattern::BindingIdentifier(b), Some(init)) = (&d.id, &d.init) && function_value(init) { local.insert(b.name.to_string()); }
                    }
                }
                _ => {}
            }
        }
        let (mut names, mut cjs, mut default) = (BTreeSet::new(), false, false);
        for s in body {
            match s {
                Statement::ExportDeclaration(d) => match &d.declaration {
                    Declaration::FunctionDeclaration(f) if f.body.is_some() => { if let Some(id) = &f.id { names.insert(id.name.to_string()); } }
                    Declaration::VariableDeclaration(v) => {
                        for d in &v.declarations { if let BindingPattern::BindingIdentifier(b) = &d.id && local.contains(b.name.as_str()) { names.insert(b.name.to_string()); } }
                    }
                    _ => {}
                },
                Statement::ExportDefaultDeclaration(d) => {
                    default = true;
                    match &d.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(f) if f.body.is_some() => { names.insert("default".to_string()); }
                        other => {
                            if let Some(e) = other.as_expression() && (function_value(e) || matches!(e, Expression::Identifier(id) if local.contains(id.name.as_str()))) { names.insert("default".to_string()); }
                        }
                    }
                }
                Statement::ExportNamedDeclaration(d) if d.export_kind != ImportOrExportKind::Type => {
                    for sp in d.specifiers.iter().filter(|sp| sp.export_kind != ImportOrExportKind::Type) {
                        if sp.exported.name() == "default" { default = true; }
                        if local.contains(sp.local.name().as_str()) { names.insert(sp.exported.name().to_string()); }
                    }
                }
                Statement::ExportFromDeclaration(d) if d.export_kind != ImportOrExportKind::Type => {
                    if d.specifiers.iter().any(|sp| sp.export_kind != ImportOrExportKind::Type && sp.exported.name() == "default") { default = true; }
                }
                Statement::ExpressionStatement(e) => {
                    if let Expression::AssignmentExpression(a) = &e.expression && module_exports(&a.left) { cjs = true; }
                }
                _ => {}
            }
        }
        return Exports { functions: names.into_iter().collect(), cjs, default };
    }
    Exports::default()
}

pub fn same_program(path: &str, old: &str, new: &str) -> bool {
    let Some(types) = source_types(path) else { return false };
    types.into_iter().any(|st| {
        let (a, b) = (Allocator::default(), Allocator::default());
        let options = oxc_parser::ParseOptions { preserve_parens: false, ..Default::default() };
        let (po, pn) = (Parser::new(&a, old, st).with_options(options).parse(), Parser::new(&b, new, st).with_options(options).parse());
        !po.fatal_error && !pn.fatal_error && po.diagnostics.is_empty() && pn.diagnostics.is_empty() && oxc_span::ContentEq::content_eq(&po.program, &pn.program)
    })
}
pub fn hinted(text: &str) -> BTreeSet<u32> {
    let (mut out, mut start) = (BTreeSet::new(), None::<u32>);
    let total = text.lines().count() as u32;
    for (i, line) in text.lines().enumerate() {
        let n = i as u32 + 1;
        let hint = ["c8 ignore ", "v8 ignore "].iter().find_map(|m| line.find(m).map(|at| &line[at + m.len()..]));
        let words: Vec<&str> = hint.map(|h| h.split(|c: char| c.is_whitespace() || c == '*' || c == '/').filter(|w| !w.is_empty()).take(2).collect()).unwrap_or_default();
        match words.first().copied() {
            Some("next") => { let k = words.get(1).and_then(|w| w.parse::<u32>().ok()).unwrap_or(1).min(total); out.extend((n + 1)..=(n + k).min(total)); }
            Some("start") => { start.get_or_insert(n); }
            Some("stop") => { if let Some(s) = start.take() { out.extend(s..=n); } }
            _ => {}
        }
    }
    if let Some(s) = start { out.extend(s..=total); }
    out
}

pub const TASK: &str = include_str!("drive-task.md");
const BINARY: [&str; 52] = ["png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp", "tif", "tiff", "psd", "heic", "mp3", "mp4", "m4a", "mov", "webm",
    "wav", "ogg", "flac", "avi", "mkv", "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "jar", "war", "woff", "woff2", "ttf", "otf", "eot", "glb", "gltf",
    "fbx", "blend", "wasm", "bin", "exe", "dll", "so", "dylib", "node", "db", "sqlite", "pyc", "class"];
const LOCKFILES: [&str; 6] = ["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lockb", "bun.lock"];
const OTHER_CODE: [&str; 30] = ["vue", "svelte", "astro", "mdx", "marko", "py", "rb", "go", "java", "kt", "kts", "rs", "php", "cs", "fs", "swift", "scala", "c", "cc", "cpp",
    "h", "hpp", "m", "mm", "dart", "ex", "exs", "lua", "pl", "sh"];
const LARGEST: u64 = 1 << 20;
const MOST_FILES: usize = 2000;
const MOST_TEXT: u64 = 128 << 20;
const MEASUREMENT: [&str; 6] = ["NODE_V8_COVERAGE", "__kernDriveRecord", "KERN_DRIVE_CALLS", "/tmp/.kern", "kern-wrap:", "sourceMappingURL"];
fn extension(path: &str) -> String { path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default() }
fn unmeasured_kind(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let dirs: Vec<String> = path.split('/').map(str::to_ascii_lowercase).collect();
    let ext = extension(path);
    if dirs.iter().any(|d| ["test", "tests", "__tests__", "spec", "specs", "e2e", "fixtures", "__fixtures__", "__mocks__", "cypress", "playwright"].contains(&d.as_str()))
        || [".test.", ".spec.", "-test.", "_test.", ".e2e."].iter().any(|m| name.contains(m)) { return "tests"; }
    if LOCKFILES.contains(&name.as_str()) || name == "package.json" { return "dependencies"; }
    if ["md", "mdx", "txt", "rst", "adoc"].contains(&ext.as_str()) || name.starts_with("license") || name.starts_with("changelog") { return "documentation"; }
    if name.contains(".config.") || name.starts_with('.') || name.starts_with("tsconfig") || name == "dockerfile" || dirs.first().is_some_and(|d| d == ".github")
        || ["toml", "ini", "cfg", "conf", "env"].contains(&ext.as_str()) { return "configuration"; }
    if ["json", "yaml", "yml", "sql", "prisma", "graphql", "gql", "csv", "xml", "proto"].contains(&ext.as_str()) { return "data and schemas"; }
    if ["css", "scss", "sass", "less", "styl", "pcss"].contains(&ext.as_str()) { return "styles"; }
    if binary(path) || ["svg", "html"].contains(&ext.as_str()) { return "assets and markup"; }
    "other files"
}
fn read_own(path: impl AsRef<Path>) -> R<Value> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    ensure!(bytes.len() <= 512 << 20, "{} is larger than 512 MiB", path.display());
    Ok(serde_json::from_slice(&bytes)?)
}
fn binary(path: &str) -> bool { BINARY.contains(&extension(path).as_str()) }
fn lockfile(path: &str) -> bool { LOCKFILES.contains(&path.rsplit('/').next().unwrap_or(path)) }
pub fn minified(text: &str) -> bool { let n = text.lines().count().max(1); text.lines().any(|l| l.len() > 5000) || text.len() / n > 300 }

#[derive(Clone)]
pub struct Item { pub path: String, pub mode: String, pub oid: String, pub size: u64 }
pub fn listing(repo: &Path, commit: &str) -> R<Vec<Item>> {
    let out = git_with(repo, &["ls-tree", "-rlz", "--full-tree", commit], None, 256 << 20)?;
    let mut items = Vec::new();
    for row in out.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let Some(tab) = row.iter().position(|b| *b == b'\t') else { deny!("Malformed tree listing") };
        let f: Vec<&str> = std::str::from_utf8(&row[..tab]).unwrap_or("").split_whitespace().collect();
        ensure!(f.len() == 4, "Malformed tree listing");
        let Ok(path) = std::str::from_utf8(&row[tab + 1..]) else { continue };
        if f[1] != "blob" { continue; }
        items.push(Item { path: path.to_string(), mode: f[0].to_string(), oid: f[2].to_string(), size: f[3].parse().unwrap_or(0) });
        ensure!(items.len() <= 500_000, "The repository has more than 500,000 files");
    }
    Ok(items)
}
pub fn blobs(repo: &Path, items: &[&Item], mut each: impl FnMut(&Item, &[u8]) -> R<()>) -> R<()> {
    let mut start = 0;
    while start < items.len() {
        let (mut end, mut bytes) = (start, 0u64);
        while end < items.len() && (end == start || (bytes + items[end].size <= 32 << 20 && end - start < 4000)) { bytes += items[end].size; end += 1; }
        let batch = &items[start..end];
        let request: String = batch.iter().map(|i| format!("{}\n", i.oid)).collect();
        let stream = git_with(repo, &["cat-file", "--batch"], Some(request.as_bytes()), bytes as usize + batch.len() * 64 + 4096)?;
        let mut at = 0usize;
        for item in batch {
            let nl = stream[at..].iter().position(|b| *b == b'\n').map(|p| p + at).ok_or_else(|| Denied("Malformed Git object stream".into()))?;
            let header = String::from_utf8_lossy(&stream[at..nl]).to_string();
            let size: usize = header.rsplit(' ').next().and_then(|s| s.parse().ok()).ok_or_else(|| Denied(format!("Git object missing: {}", item.path)))?;
            ensure!(header.starts_with(&item.oid) && stream.len() > nl + size, "Git object stream mismatch");
            each(item, &stream[nl + 1..nl + 1 + size])?;
            at = nl + 2 + size;
        }
        start = end;
    }
    Ok(())
}
pub(crate) fn commit_check(repo: &Path, c: &str) -> R<()> {
    crate::git::oid(c)?;
    ensure!(git(repo, &["cat-file", "-t", c], None).map(trimmed).ok().as_deref() == Some("commit"), "Not a commit in this repository: {c}");
    Ok(())
}
fn copy_for_author(repo: &Path, items: &[Item], changed: &[String], dest: &Path) -> R<&'static str> {
    let fits = |i: &&Item| matches!(i.mode.as_str(), "100644" | "100755") && i.size <= LARGEST && !binary(&i.path) && !lockfile(&i.path)
        && !i.path.split('/').any(|d| d == "node_modules") && file_path(&i.path).is_ok() && std::path::absolute(dest.join(&i.path)).is_ok_and(|p| p.as_os_str().len() < 250);
    let mut chosen: Vec<&Item> = items.iter().filter(fits).collect();
    let mut scope = "whole repository";
    if chosen.len() > 20_000 || chosen.iter().map(|i| i.size).sum::<u64>() > 150 << 20 {
        let dirs: BTreeSet<&str> = changed.iter().map(|p| p.rsplit_once('/').map_or("", |x| x.0)).collect();
        chosen.retain(|i| !i.path.contains('/') || dirs.contains(i.path.rsplit_once('/').map_or("", |x| x.0)));
        scope = "the changed folders and the top-level files (the repository is too large to copy whole)";
    }
    if chosen.len() > 20_000 || chosen.iter().map(|i| i.size).sum::<u64>() > 150 << 20 {
        chosen.retain(|i| changed.contains(&i.path));
        let mut room = 150u64 << 20;
        chosen.retain(|i| { let fits = room >= i.size; if fits { room -= i.size; } fits });
        chosen.truncate(20_000);
        scope = "the changed files only (even the changed folders are too large to copy)";
    }
    std::fs::create_dir_all(dest)?;
    blobs(repo, &chosen, |item, bytes| {
        let target = item.path.split('/').fold(dest.to_path_buf(), |p, part| p.join(part));
        if let Some(parent) = target.parent() { let _ = std::fs::create_dir_all(parent); }
        if !minified(&String::from_utf8_lossy(bytes)) { let _ = std::fs::write(&target, bytes); }
        Ok(())
    })?;
    Ok(scope)
}
fn lines_md(files: &[Value]) -> String {
    let total = |side: &str| files.iter().map(|f| f[side].as_array().map_or(0, Vec::len)).sum::<usize>();
    let mut md = format!("# Changed lines Kern measures\n\n{} lines of the new code and {} lines of the old code, numbered as in the file.\n", total("new"), total("old"));
    let mut shown = 0;
    for side in ["new", "old"] {
        for f in files.iter().filter(|f| f[side].as_array().is_some_and(|a| !a.is_empty())) {
            let lines = f[side].as_array().map_or(0, Vec::len);
            md += &format!("\n## {} ({side} code, {lines} lines)\n\n", f["path"].as_str().unwrap_or(""));
            for (num, text) in f[&format!("{side}_text")].as_array().into_iter().flatten().filter_map(|x| Some((x[0].as_u64()?, x[1].as_str()?))) {
                shown += 1;
                if shown > 1500 { md += "... more lines: see diff.patch\n"; break; }
                md += &format!("{num:>5}: {text}\n");
            }
        }
    }
    md
}

pub fn prepare(repo: &Path, base: &str, head: &str, out: &Path, pr: Option<&str>) -> R<Value> {
    commit_check(repo, base)?;
    commit_check(repo, head)?;
    let (old, new) = (listing(repo, base)?, listing(repo, head)?);
    let out = fresh_dir(out)?;
    let index = |items: &[Item]| -> BTreeMap<String, Item> { items.iter().map(|i| (i.path.clone(), i.clone())).collect() };
    let (old_at, new_at) = (index(&old), index(&new));
    let names = git_with(repo, &["diff", "--no-renames", "--name-only", "-z", base, head], None, 64 << 20)?;
    let paths: Vec<String> = names.split(|b| *b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).to_string()).collect();
    let code: Vec<&String> = paths.iter().filter(|p| crate::compare::probeable(p, None)).collect();
    let mut skipped: Vec<Value> = paths.iter().filter(|p| OTHER_CODE.contains(&extension(p).as_str()) && !p.split('/').any(|d| d == "node_modules"))
        .map(|p| json!({ "path": p, "why": format!("a .{} file; Kern does not run .{} code yet", extension(p), extension(p)) })).collect();
    let (mut room, mut kept) = (MOST_TEXT, BTreeSet::new());
    for p in &code {
        let size = old_at.get(*p).map_or(0, |i| i.size.min(LARGEST)) + new_at.get(*p).map_or(0, |i| i.size.min(LARGEST));
        if kept.len() < MOST_FILES && size <= room { room -= size; kept.insert(p.to_string()); }
        else { skipped.push(json!({ "path": p, "why": "not measured: the change is too large (Kern measures 2,000 files and 128 MiB of code)" })); }
    }
    let code: Vec<&String> = code.into_iter().filter(|p| kept.contains(*p)).collect();
    let mut texts: BTreeMap<(bool, String), Vec<u8>> = BTreeMap::new();
    for (side, at) in [(false, &old_at), (true, &new_at)] {
        let wanted: Vec<&Item> = code.iter().filter_map(|p| at.get(*p)).filter(|i| i.size <= LARGEST && matches!(i.mode.as_str(), "100644" | "100755")).collect();
        blobs(repo, &wanted, |item, bytes| { texts.insert((side, item.path.clone()), bytes.to_vec()); Ok(()) })?;
    }
    let (mut files, mut modules, mut measurement) = (Vec::new(), Vec::new(), Vec::<String>::new());
    for p in &code {
        let sides = [old_at.get(*p), new_at.get(*p)];
        if sides.iter().flatten().any(|i| i.size > LARGEST) { skipped.push(json!({ "path": p, "why": "larger than 1 MiB (generated code or data)" })); continue; }
        if sides.iter().flatten().any(|i| !matches!(i.mode.as_str(), "100644" | "100755")) { skipped.push(json!({ "path": p, "why": "a symlink" })); continue; }
        let text = |side: bool| -> Option<Result<&str, ()>> { texts.get(&(side, p.to_string())).map(|b| std::str::from_utf8(b).map_err(|_| ())) };
        let (o, n) = (text(false), text(true));
        if matches!(o, Some(Err(_))) || matches!(n, Some(Err(_))) { skipped.push(json!({ "path": p, "why": "not UTF-8 text" })); continue; }
        let (o, n) = (o.and_then(Result::ok).unwrap_or(""), n.and_then(Result::ok).unwrap_or(""));
        if minified(o) || minified(n) { skipped.push(json!({ "path": p, "why": "generated or minified code" })); continue; }
        if old_at.contains_key(*p) && new_at.contains_key(*p) && same_program(p, o, n) {
            skipped.push(json!({ "path": p, "why": "reformatted only: the old and the new code parse to the same program, so nothing in it can behave differently" }));
            continue;
        }
        let (co, cn) = changed(o, n);
        let (mo, mn) = (measured(p, o), measured(p, n));
        let pick = |c: &[u32], m: &BTreeMap<u32, String>| -> Vec<(u32, String)> { c.iter().filter_map(|l| m.get(l).map(|t| (*l, t.clone()))).collect() };
        let (po, pn) = (pick(&co, &mo), pick(&cn, &mn));
        if po.is_empty() && pn.is_empty() { skipped.push(json!({ "path": p, "why": "no line of code changed (comments, imports or types only)" })); continue; }
        let status = match (old_at.contains_key(*p), new_at.contains_key(*p)) { (false, _) => "added", (_, false) => "deleted", _ => "modified" };
        let under_hint = |text: &str, picked: &[(u32, String)]| -> Vec<u32> { let h = hinted(text); picked.iter().map(|x| x.0).filter(|l| h.contains(l)).collect() };
        files.push(json!({ "path": p, "status": status, "old": po.iter().map(|x| x.0).collect::<Vec<_>>(), "new": pn.iter().map(|x| x.0).collect::<Vec<_>>(),
            "old_text": po.iter().map(|(l, t)| json!([l, t])).collect::<Vec<_>>(), "new_text": pn.iter().map(|(l, t)| json!([l, t])).collect::<Vec<_>>(),
            "old_hinted": under_hint(o, &po), "new_hinted": under_hint(n, &pn) }));
        if MEASUREMENT.iter().any(|m| n.contains(m)) { measurement.push(p.to_string()); }
        let (eo, en) = (exported(p, o), exported(p, n));
        modules.push(json!({ "path": p, "old": { "functions": eo.functions, "cjs": eo.cjs, "default": eo.default }, "new": { "functions": en.functions, "cjs": en.cjs, "default": en.default } }));
    }
    let count = |side: &str| files.iter().map(|f| f[side].as_array().map_or(0, Vec::len)).sum::<usize>();
    let totals = json!({ "new": count("new"), "old": count("old") });
    let unmeasured: Vec<Value> = paths.iter().filter(|p| !files.iter().any(|f| f["path"] == p.as_str()) && !skipped.iter().any(|s| s["path"] == p.as_str()))
        .map(|p| json!({ "path": p, "why": unmeasured_kind(p) })).collect();
    let task = out.join("task");
    std::fs::create_dir(&task)?;
    create_new(task.join("task.md"), TASK.as_bytes(), 0o644)?;
    let pr = match pr { Some(t) => t.to_string(), None => format!("# {}\n", String::from_utf8_lossy(&git(repo, &["log", "-1", "--format=%B", head], None)?).trim()) };
    create_new(task.join("pr.md"), pr.as_bytes(), 0o644)?;
    let shown: Vec<&str> = paths.iter().map(String::as_str).filter(|p| !lockfile(p) && !binary(p) && !skipped.iter().any(|s| s["path"] == *p)).collect();
    let mut patch = Vec::new();
    let mut group: Vec<&str> = Vec::new();
    for (i, p) in shown.iter().enumerate() {
        group.push(p);
        let size: usize = group.iter().map(|g| g.len() + 3).sum();
        if size > 12_000 || i + 1 == shown.len() {
            let mut args = vec!["diff", "--no-renames", "-U3", base, head, "--"];
            args.append(&mut group);
            patch.extend(git_with(repo, &args, None, 64 << 20)?);
            if patch.len() > 250_000 { break; }
        }
    }
    if patch.len() > 250_000 {
        let cut = patch[..250_000].iter().rposition(|b| *b == b'\n').unwrap_or(250_000);
        patch.truncate(cut);
        patch.extend_from_slice(b"\n... the rest of the change is not shown (over 250 KB)\n");
    }
    create_new(task.join("diff.patch"), &patch, 0o644)?;
    create_new(task.join("lines.md"), lines_md(&files).as_bytes(), 0o644)?;
    let copy_old = copy_for_author(repo, &old, &paths, &task.join("old"))?;
    let copy_new = copy_for_author(repo, &new, &paths, &task.join("new"))?;
    let d = json!({ "version": 1, "repo": std::path::absolute(repo)?.display().to_string(), "base": base, "head": head, "files": files, "skipped": skipped,
        "unmeasured": unmeasured, "measurement": measurement, "modules": modules, "totals": totals, "copies": { "old": copy_old, "new": copy_new } });
    write_json(out.join("drive.json"), &d)?;
    Ok(json!({ "status": "PREPARED", "totals": totals, "files": files.len(), "skipped": skipped.len(), "task": task.display().to_string() }))
}

#[derive(Clone, Debug, Default)]
pub struct Run { pub output: String, pub errors: String, pub coverage: Value, pub calls: Vec<Value>, pub behavior: BTreeSet<String> }
impl Run {
    pub fn parse(stdout: &[u8]) -> R<Run> {
        let parts = crate::sandbox::drive_parts(stdout)?;
        let text = |k: &str| String::from_utf8_lossy(&parts[k]).to_string();
        Ok(Run {
            output: text("output"), errors: text("errors"),
            coverage: serde_json::from_slice(&parts["coverage"]).unwrap_or(Value::Null),
            calls: text("calls").lines().filter_map(|l| serde_json::from_str(l).ok()).collect(),
            behavior: text("behavior").lines().filter(|l| !l.is_empty() && !l.starts_with("count ")).map(str::to_string).collect(),
        })
    }
}
fn ran_lines(coverage: &Value) -> BTreeMap<String, BTreeSet<u32>> {
    let mut out: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (file, d) in coverage.as_object().into_iter().flatten() {
        let Some(path) = file.strip_prefix("/workspace/") else { continue };
        let lines = out.entry(path.to_string()).or_default();
        for (k, loc) in d["statementMap"].as_object().into_iter().flatten() {
            if d["s"][k].as_u64().unwrap_or(0) == 0 { continue; }
            let (a, b) = (loc["start"]["line"].as_u64().unwrap_or(0) as u32, loc["end"]["line"].as_u64().unwrap_or(0) as u32);
            lines.extend(a..=b.max(a));
        }
    }
    out
}
type Span = ((u32, u32), (u32, u32), bool);
const HELPERS: [&str; 22] = ["__name", "__export", "__copyProps", "__toCommonJS", "__toESM", "__reExport", "__commonJS", "__esm", "__require", "__defProp",
    "__defNormalProp", "__publicField", "__decorateClass", "__decorateParam", "__async", "__await", "__spreadValues", "__spreadProps", "__objRest", "__template", "__using", "get"];
fn left_out(coverage: &Value) -> BTreeMap<String, Vec<Span>> {
    let at = |p: &Value| (p["line"].as_u64().unwrap_or(0) as u32, p["column"].as_u64().unwrap_or(0) as u32);
    let mut out: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for (file, d) in coverage.as_object().into_iter().flatten() {
        let Some(path) = file.strip_prefix("/workspace/") else { continue };
        let spans = out.entry(path.to_string()).or_default();
        for (k, f) in d["fnMap"].as_object().into_iter().flatten() {
            if d["f"][k].as_u64() == Some(0) && !HELPERS.contains(&f["name"].as_str().unwrap_or("")) { spans.push((at(&f["loc"]["start"]), at(&f["loc"]["end"]), true)); }
        }
        for (k, b) in d["branchMap"].as_object().into_iter().flatten() {
            for (j, loc) in b["locations"].as_array().into_iter().flatten().enumerate() {
                if d["b"][k][j].as_u64() == Some(0) { spans.push((at(&loc["start"]), at(&loc["end"]), false)); }
            }
        }
    }
    out
}
fn plausible(text: &str, col: u32, function: bool) -> bool {
    let mut unit = 0u32;
    let at_byte = text.char_indices().find(|(_, c)| { let here = unit >= col; unit += c.len_utf16() as u32; here }).map_or(text.len(), |(i, _)| i);
    let (before, at) = (text[..at_byte].trim_end(), text[at_byte..].trim_start());
    if at.starts_with("export") || at.starts_with("import") { return false; }
    if function {
        let name: String = at.chars().take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$').collect();
        let rest = at[name.len()..].trim_start();
        return ["function", "async", "(", "get ", "set ", "static ", "*", "#"].iter().any(|s| at.starts_with(s))
            || (!name.is_empty() && !["const", "let", "var", "return", "if", "for", "while", "switch", "new", "await", "typeof"].contains(&name.as_str())
                && (rest.starts_with("=>") || rest.starts_with('(')));
    }
    before.is_empty() || at.starts_with('{') || ["?", ":", "&&", "||", "??", ")", "else", "=>", "{", ";"].iter().any(|t| before.ends_with(t))
        || [":", "&&", "||", "??"].iter().any(|t| at.starts_with(t)) || (at.starts_with('?') && !at.starts_with("?."))
}
fn line_left_out(spans: &[Span], line: u32, text: &str) -> bool {
    spans.iter().any(|&((sl, sc), (el, ec), function)| {
        if line < sl || line > el || (line == sl && !plausible(text, sc, function)) { return false; }
        let (from, to) = (if line == sl { sc } else { 0 }, if line == el { ec } else { u32::MAX });
        let mut unit = 0u32;
        text.chars().any(|c| {
            let inside = unit >= from && unit < to;
            unit += c.len_utf16() as u32;
            inside && !c.is_whitespace() && !"{}()[];,".contains(c)
        })
    })
}
fn left_out_text(spans: &[Span], line: u32, text: &str) -> Option<String> {
    let mut part = String::new();
    for &((sl, sc), (el, ec), function) in spans.iter().filter(|((sl, _), (el, _), _)| line >= *sl && line <= *el) {
        if line == sl && !plausible(text, sc, function) { continue; }
        let (from, to) = (if line == sl { sc } else { 0 }, if line == el { ec } else { u32::MAX });
        let mut unit = 0u32;
        let piece: String = text.chars().filter(|c| { let inside = unit >= from && unit < to; unit += c.len_utf16() as u32; inside }).collect();
        if piece.chars().any(|c| !c.is_whitespace() && !"{}()[];,".contains(c)) && !part.contains(piece.trim()) {
            if !part.is_empty() { part.push_str(" … "); }
            part.push_str(piece.trim());
        }
    }
    (!part.is_empty()).then(|| clip_str(&part, 120))
}
fn steps(output: &str) -> BTreeMap<String, String> {
    let (mut out, mut count) = (BTreeMap::new(), BTreeMap::<String, u32>::new());
    for line in output.lines() {
        let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(line) else { continue };
        let Some(step) = o.remove("step") else { continue };
        let name = step.as_str().map_or_else(|| step.to_string(), str::to_string);
        let n = count.entry(name.clone()).or_insert(0);
        *n += 1;
        out.insert(if *n > 1 { format!("{name} #{n}") } else { name }, Value::Object(o).to_string());
    }
    out
}
fn calls(run: &Run) -> BTreeMap<(String, String, u32), String> {
    let (mut seen, mut out) = (BTreeMap::<(String, String), u32>::new(), BTreeMap::new());
    for c in &run.calls {
        let (Some(k), Some(a)) = (c["k"].as_str(), c["a"].as_str()) else { continue };
        let n = seen.entry((k.to_string(), a.to_string())).or_insert(0);
        *n += 1;
        out.insert((k.to_string(), a.to_string(), *n), c["r"].as_str().or(c["o"].as_str()).unwrap_or("").to_string());
    }
    out
}
type Compared<K, V> = (u64, u64, u64, u64, u64, Vec<(K, Option<V>, Option<V>)>, Vec<K>);
fn compare<K: Ord + Clone, V: PartialEq + Clone>(o1: &BTreeMap<K, V>, o2: &BTreeMap<K, V>, n1: &BTreeMap<K, V>, n2: &BTreeMap<K, V>, one_sided: bool) -> Compared<K, V> {
    let keys: BTreeSet<&K> = o1.keys().chain(n1.keys()).collect();
    let (mut same, mut unstable, mut only_old, mut only_new, mut diffs, mut shaky) = (0, 0, 0, 0, Vec::new(), Vec::new());
    for k in keys {
        if o1.get(k) != o2.get(k) || n1.get(k) != n2.get(k) {
            unstable += 1;
            if shaky.len() < 200 { shaky.push(k.clone()); }
            continue;
        }
        match (o1.get(k), n1.get(k)) {
            (Some(a), Some(b)) if a == b => same += 1,
            (Some(a), None) => { only_old += 1; if one_sided { diffs.push((k.clone(), Some(a.clone()), None)); } }
            (None, Some(b)) => { only_new += 1; if one_sided { diffs.push((k.clone(), None, Some(b.clone()))); } }
            (a, b) => diffs.push((k.clone(), a.cloned(), b.cloned())),
        }
    }
    let different = diffs.iter().filter(|(_, a, b)| a.is_some() && b.is_some()).count() as u64;
    (same, different, unstable, only_old, only_new, diffs, shaky)
}
fn clip_str(s: &str, n: usize) -> String { if s.chars().count() > n { format!("{}...", s.chars().take(n).collect::<String>()) } else { s.to_string() } }

pub fn score(d: &Value, runs: &[Run; 4], round: u32) -> Value {
    let [o1, o2, n1, n2] = runs;
    let mut files = Vec::new();
    let mut totals = [(0u64, 0u64), (0u64, 0u64)];
    for (side, (a, b)) in [("new", (n1, n2)), ("old", (o1, o2))] {
        let runs = [(ran_lines(&a.coverage), left_out(&a.coverage)), (ran_lines(&b.coverage), left_out(&b.coverage))];
        for f in d["files"].as_array().into_iter().flatten() {
            let path = f["path"].as_str().unwrap_or("");
            let wanted: Vec<(u64, String)> = f[&format!("{side}_text")].as_array().into_iter().flatten().filter_map(|x| Some((x[0].as_u64()?, x[1].as_str()?.to_string()))).collect();
            let hinted: BTreeSet<u64> = f[&format!("{side}_hinted")].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
            let text_of = |l: u64| wanted.iter().find(|(n, _)| *n == l).map_or("", |(_, t)| t.as_str());
            let hit = |l: u64| !hinted.contains(&l) && runs.iter().any(|(ran, out)| ran.get(path).is_some_and(|s| s.contains(&(l as u32)))
                && !out.get(path).is_some_and(|spans| line_left_out(spans, l as u32, text_of(l))));
            let part = |l: u64| runs.iter().filter(|(ran, _)| ran.get(path).is_some_and(|s| s.contains(&(l as u32))))
                .find_map(|(_, out)| out.get(path).and_then(|spans| left_out_text(spans, l as u32, text_of(l))));
            let missed: Vec<Value> = wanted.iter().filter(|(l, _)| !hit(*l)).map(|(l, t)| match part(*l) { Some(p) => json!([l, t, p]), None => json!([l, t]) }).collect();
            let total = wanted.len() as u64;
            let i = usize::from(side == "old");
            totals[i].0 += total - missed.len() as u64;
            totals[i].1 += total;
            let entry = json!({ "ran": total - missed.len() as u64, "total": total, "loaded": runs.iter().any(|(ran, _)| ran.contains_key(path)), "missed": missed });
            match files.iter_mut().find(|x: &&mut Value| x["path"] == path) { Some(x) => { x[side] = entry; } None => files.push(json!({ "path": path, side: entry })) }
        }
    }
    let (ran, total) = (totals[0].0 + totals[1].0, totals[0].1 + totals[1].1);
    let percent = (ran * 100).checked_div(total).unwrap_or(100);
    let (s_same, s_diff, s_unstable, _, _, step_diffs, shaky_steps) = compare(&steps(&o1.output), &steps(&o2.output), &steps(&n1.output), &steps(&n2.output), true);
    let (c_same, c_diff, c_unstable, c_old, c_new, call_diffs, shaky_calls) = compare(&calls(o1), &calls(o2), &calls(n1), &calls(n2), false);
    let mut shaky_functions: BTreeMap<String, u64> = BTreeMap::new();
    for (k, _, _) in &shaky_calls { *shaky_functions.entry(k.clone()).or_insert(0) += 1; }
    let stable = |a: &Run, b: &Run| -> BTreeSet<String> { a.behavior.intersection(&b.behavior).cloned().collect() };
    let seen = |a: &Run, b: &Run| -> BTreeSet<String> { a.behavior.union(&b.behavior).cloned().collect() };
    let new_behavior: Vec<String> = stable(n1, n2).difference(&seen(o1, o2)).cloned().collect();
    let gone: Vec<String> = stable(o1, o2).difference(&seen(n1, n2)).cloned().collect();
    let (sn, so) = (steps(&n1.output), steps(&o1.output));
    let error_of = |v: &String| serde_json::from_str::<Value>(v).ok().and_then(|o| o.get("error").map(|e| e.as_str().map_or_else(|| e.to_string(), str::to_string)));
    let mut threw = Vec::new();
    for k in sn.keys().chain(so.keys()).collect::<BTreeSet<_>>() {
        let (n, o) = (sn.get(k).and_then(error_of), so.get(k).and_then(error_of));
        let side = match (&n, &o) { (Some(a), Some(b)) if a == b => "old and new", (Some(_), _) => "new", (None, Some(_)) => "old", _ => continue };
        threw.push(json!({ "step": clip_str(k, 300), "side": side, "error": clip_str(n.as_ref().or(o.as_ref()).map_or("", String::as_str), 600) }));
        if threw.len() >= 20 { break; }
    }
    json!({
        "round": round,
        "coverage": { "new": { "ran": totals[0].0, "total": totals[0].1 }, "old": { "ran": totals[1].0, "total": totals[1].1 }, "percent": percent },
        "files": files,
        "steps": { "old": steps(&o1.output).len(), "new": steps(&n1.output).len(), "same": s_same, "different": s_diff, "unstable": s_unstable },
        "differences": step_diffs.iter().take(50).map(|(k, a, b)| json!({ "step": clip_str(k, 300), "old": a.as_deref().map(|x| clip_str(x, 600)), "new": b.as_deref().map(|x| clip_str(x, 600)) })).collect::<Vec<_>>(),
        "calls": { "same": c_same, "different": c_diff, "unstable": c_unstable, "only_old": c_old, "only_new": c_new },
        "call_differences": call_diffs.iter().take(50).map(|((k, args, _), a, b)| json!({ "call": k, "args": clip_str(args, 300),
            "old": a.as_deref().map(|x| clip_str(x, 600)), "new": b.as_deref().map(|x| clip_str(x, 600)) })).collect::<Vec<_>>(),
        "behavior": { "new": new_behavior, "gone": gone },
        "threw": threw,
        "endings": { "old": endings(&[o1, o2]), "new": endings(&[n1, n2]) },
        "unstable": { "steps": shaky_steps.iter().take(30).map(|s| clip_str(s, 200)).collect::<Vec<_>>(),
            "calls": shaky_functions.iter().take(30).map(|(f, n)| json!({ "call": f, "count": n })).collect::<Vec<_>>() },
        "printed": first_steps(if n1.output.trim().is_empty() { &o1.output } else { &n1.output }, 40),
        "errors": { "old": clip_tail(&o1.errors, 4000), "new": clip_tail(&n1.errors, 4000) },
    })
}
fn endings(runs: &[&Run]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in runs {
        for l in r.errors.lines().filter(|l| l.starts_with("kern: ")) {
            let l = clip_str(l, 300);
            if !out.contains(&l) && out.len() < 4 { out.push(l); }
        }
    }
    out
}
fn first_steps(output: &str, n: usize) -> Vec<Value> {
    output.lines().filter_map(|l| match serde_json::from_str::<Value>(l) { Ok(Value::Object(mut o)) => o.remove("step").map(|s| (s, o)), _ => None }).take(n)
        .map(|(s, rest)| json!({ "step": clip_str(&s.as_str().map_or_else(|| s.to_string(), str::to_string), 200), "printed": clip_str(&Value::Object(rest).to_string(), 300) })).collect()
}
fn clip_tail(s: &str, n: usize) -> String { let c: Vec<char> = s.chars().collect(); if c.len() > n { format!("...{}", c[c.len() - n..].iter().collect::<String>()) } else { s.to_string() } }

pub fn feedback(_d: &Value, r: &Value) -> String {
    let c = &r["coverage"];
    let side = |s: &str| format!("{}/{}", c[s]["ran"], c[s]["total"]);
    let mut md = format!("# Kern ran your driver (round {})\n\nMeasured changed lines that ran: new code {}, old code {} ({}% in all).\n", r["round"], side("new"), side("old"), c["percent"]);
    let s = &r["steps"];
    md += &format!("Steps printed: old {}, new {}; the same on both: {}, different: {}, unstable between two runs of one side: {}.\n\n## Changed lines that did not run\n", s["old"], s["new"], s["same"], s["different"], s["unstable"]);
    let mut shown = 0;
    for which in ["new", "old"] {
        for f in r["files"].as_array().into_iter().flatten() {
            let e = &f[which];
            let missed = e["missed"].as_array().map_or(0, Vec::len);
            if missed == 0 { continue; }
            let note = if e["loaded"] == true { "" } else { "; the file was never loaded" };
            md += &format!("\n### {} ({which} code: {missed} of {} did not run{note})\n\n", f["path"].as_str().unwrap_or(""), e["total"]);
            for x in e["missed"].as_array().into_iter().flatten() {
                shown += 1;
                if shown > 600 { md += "...\n"; break; }
                let not_run = x[2].as_str().map(|p| format!("   (not run: `{}`)", p.replace('`', "'"))).unwrap_or_default();
                md += &format!("{:>5}: {}{not_run}\n", x[0].as_u64().unwrap_or(0), x[1].as_str().unwrap_or(""));
            }
        }
    }
    for which in ["new", "old"] {
        let e = r["errors"][which].as_str().unwrap_or("");
        if !e.trim().is_empty() { md += &format!("\n## Error output on the {which} code (last lines)\n\n```\n{e}\n```\n"); }
    }
    let threw: Vec<String> = r["threw"].as_array().into_iter().flatten().map(|t| format!("- {} ({} code): {}", t["step"].as_str().unwrap_or(""), t["side"].as_str().unwrap_or(""), t["error"].as_str().unwrap_or(""))).collect();
    if !threw.is_empty() { md += &format!("\n## Steps that threw\n\n{}\n", threw.join("\n")); }
    let shaky: Vec<String> = r["unstable"]["calls"].as_array().into_iter().flatten().map(|c| format!("- call {} ({} calls)", c["call"].as_str().unwrap_or(""), c["count"]))
        .chain(r["unstable"]["steps"].as_array().into_iter().flatten().map(|s| format!("- step {}", s.as_str().unwrap_or("")))).collect();
    if !shaky.is_empty() {
        md += &format!("\n## Not compared: these answered differently on two runs of the same code\n\nKern compares only what two runs of one side agree on. \
            Keep ports, timers, generated ids and the order of concurrent work out of what you print and pass in (listen on a fixed port, await \
            each request in turn).\n\n{}\n", shaky.join("\n"));
    }
    let printed: Vec<String> = r["printed"].as_array().into_iter().flatten().map(|p| format!("- {}: {}", p["step"].as_str().unwrap_or(""), p["printed"].as_str().unwrap_or(""))).collect();
    if !printed.is_empty() { md += &format!("\n## What your steps printed on the new code (the first {}, in order)\n\n{}\n", printed.len(), printed.join("\n")); }
    let diffs: Vec<String> = r["differences"].as_array().into_iter().flatten().take(20).map(|d| format!("- {}\n  old: {}\n  new: {}", d["step"].as_str().unwrap_or(""), d["old"].as_str().unwrap_or("(no such step)"), d["new"].as_str().unwrap_or("(no such step)"))).collect();
    if !diffs.is_empty() { md += &format!("\n## What differs between the old and the new code (first 20)\n\n{}\n", diffs.join("\n")); }
    let errors_text = format!("{}{}{}", r["errors"]["old"].as_str().unwrap_or(""), r["errors"]["new"].as_str().unwrap_or(""), r["threw"]);
    if package_failed(&errors_text) {
        md += "\n## A package failed to load\n\nThe errors above look like a package that cannot load here. If the changed code only needs it to exist (a UI kit, a \
               native add-on), write a stand-in for it: stubs/<package>.mjs next to driver.mjs, exporting what the code uses (task.md, Stand-ins).\n";
    }
    let used: Vec<&str> = r["stubs"]["used"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if !used.is_empty() { md += &format!("\n## Stand-ins in use\n\n{}\n", used.join(", ")); }
    let refused: Vec<String> = r["stubs"]["refused"].as_array().into_iter().flatten().take(20).map(|s| format!("- {}: {}", s["path"].as_str().unwrap_or(""), s["why"].as_str().unwrap_or(""))).collect();
    if !refused.is_empty() { md += &format!("\n## Stand-ins Kern refused\n\n{}\n", refused.join("\n")); }
    md
}

fn package_failed(text: &str) -> bool {
    const MARKS: [&str; 8] = ["node_modules", "bindings file", "NODE_MODULE_VERSION", ".node'", "invalid ELF header", "Transform failed", "ERR_REQUIRE_ESM", "ERR_UNKNOWN_FILE_EXTENSION"];
    let package_name = |s: &str| {
        let mut c = s.chars();
        match c.next() { Some('@') => c.next().is_some_and(|x| x.is_ascii_alphanumeric()), Some(x) => x.is_ascii_alphanumeric() && !s.starts_with("file:"), None => false }
    };
    let missing_package = ["Cannot find module '", "Cannot find package '"].iter().any(|m| text.match_indices(m).any(|(i, _)| package_name(&text[i + m.len()..])));
    missing_package || MARKS.iter().any(|m| text.contains(m))
}
fn span(s: &str, n: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '`' { '\'' } else if c.is_control() { ' ' } else { c }).collect();
    format!("`{}`", clip_str(flat.trim(), n))
}
fn windows(a: &str, b: &str, n: usize) -> (String, String) {
    let (ca, cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if ca.len() <= n && cb.len() <= n { return (a.to_string(), b.to_string()); }
    let first = ca.iter().zip(&cb).position(|(x, y)| x != y).unwrap_or(ca.len().min(cb.len()));
    let start = first.saturating_sub(n / 4);
    let cut = |c: &[char]| {
        let (from, to) = (start.min(c.len()), (start + n).min(c.len()));
        format!("{}{}{}", if from > 0 { "…" } else { "" }, c[from..to].iter().collect::<String>(), if to < c.len() { "…" } else { "" })
    };
    (cut(&ca), cut(&cb))
}
fn outcomes(old: Option<&str>, new: Option<&str>) -> (String, String) {
    match (old, new) {
        (Some(a), Some(b)) => match (a.split_once(' '), b.split_once(' ')) {
            (Some((ka, va)), Some((kb, vb))) if ka == kb && (ka == "r" || ka == "e") => {
                let (wa, wb) = windows(va, vb, 160);
                (outcome(&format!("{ka} {wa}")), outcome(&format!("{kb} {wb}")))
            }
            _ => (outcome(a), outcome(b)),
        },
        (a, b) => (a.map_or("was not called".to_string(), outcome), b.map_or("is not called".to_string(), outcome)),
    }
}
fn outcome(o: &str) -> String {
    match o.split_once(' ') {
        Some(("r", v)) => format!("returned {}", span(v, 160)),
        Some(("e", v)) => format!("threw {}", span(v, 160)),
        _ if o == "promise" => "returned a promise that had not settled".to_string(),
        _ => span(o, 160),
    }
}
fn ranges(lines: &[u64]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let mut j = i;
        while j + 1 < lines.len() && lines[j + 1] == lines[j] + 1 { j += 1; }
        out.push(if i == j { lines[i].to_string() } else { format!("{}-{}", lines[i], lines[j]) });
        i = j + 1;
    }
    out.join(", ")
}
pub fn difference_items(r: &Value) -> Vec<(String, bool)> {
    struct Group { call: String, now: Option<String>, inputs: Vec<String>, before: Vec<Option<String>>, calls: usize }
    let mut groups: Vec<Group> = Vec::new();
    for x in r["call_differences"].as_array().into_iter().flatten() {
        let (call, now) = (x["call"].as_str().unwrap_or("").to_string(), x["new"].as_str().map(str::to_string));
        let (input, before) = (span(x["args"].as_str().unwrap_or(""), 120), x["old"].as_str().map(str::to_string));
        let g = match groups.iter().position(|g| g.call == call && g.now == now) {
            Some(i) => &mut groups[i],
            None => { groups.push(Group { call, now, inputs: Vec::new(), before: Vec::new(), calls: 0 }); groups.last_mut().expect("just pushed") }
        };
        if !g.inputs.contains(&input) { g.inputs.push(input); }
        if !g.before.contains(&before) { g.before.push(before); }
        g.calls += 1;
    }
    let mut items: Vec<(String, bool)> = groups.iter().map(|g| {
        let inputs = match g.inputs.len() { 0..=2 => g.inputs.join(", "), n => format!("{}, {} and {} more input{}", g.inputs[0], g.inputs[1], n - 2, if n == 3 { "" } else { "s" }) };
        let now = outcomes(g.before.first().and_then(|b| b.as_deref()), g.now.as_deref()).1;
        let shown: Vec<String> = g.before.iter().map(|b| outcomes(b.as_deref(), g.now.as_deref()).0).collect();
        let before = if shown.len() <= 3 { shown.join(" or ") } else { format!("{} or {} other answers", shown[..2].join(" or "), shown.len() - 2) };
        let calls = if g.calls > 1 { format!(" ({} calls)", g.calls) } else { String::new() };
        (format!("{} with {inputs}: {before} before, {now} now{calls}", span(&g.call, 120)), true)
    }).collect();
    for (kind, list) in [("New behaviour", &r["behavior"]["new"]), ("No longer happens", &r["behavior"]["gone"])] {
        items.extend(list.as_array().into_iter().flatten().map(|b| (format!("{kind}: {}", span(b.as_str().unwrap_or(""), 200)), true)));
    }
    items.extend(r["differences"].as_array().into_iter().flatten().map(|x| {
        let (old, new) = match (x["old"].as_str(), x["new"].as_str()) {
            (Some(a), Some(b)) => { let (wa, wb) = windows(a, b, 160); (span(&wa, 200), span(&wb, 200)) }
            (a, b) => (a.map_or("(no such step)".to_string(), |s| span(s, 160)), b.map_or("(no such step)".to_string(), |s| span(s, 160))),
        };
        (format!("{}: {old} before, {new} now", span(x["step"].as_str().unwrap_or(""), 120)), false)
    }));
    items
}
const SORTED_MAX: usize = 60;
pub fn differences_md(r: &Value) -> String {
    let mut md = String::from("# What behaves differently between the old and the new code\n\nKern ran the driver on both and saw these differences. \
        \"printed\" ones are what the driver printed; the others are Kern's own record of calls and behaviour.\n\n");
    for (i, (line, recorded)) in difference_items(r).iter().take(SORTED_MAX).enumerate() { md += &format!("- D{}: {line}{}\n", i + 1, if *recorded { "" } else { " (printed)" }); }
    md
}
pub fn read_verdicts(path: &Path, r: &Value) -> Value {
    let count = difference_items(r).len().min(SORTED_MAX);
    let Ok(text) = std::fs::read_to_string(path) else { return Value::Null };
    if text.len() > 256 << 10 { return Value::Null; }
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&text) else { return Value::Null };
    let mut out = serde_json::Map::new();
    for (k, v) in m {
        let Some(i) = k.strip_prefix('D').and_then(|n| n.parse::<usize>().ok()).filter(|i| (1..=count).contains(i)) else { return Value::Null };
        let says = v["says"].as_str().unwrap_or("");
        if !["yes", "no", "unclear"].contains(&says) { return Value::Null; }
        out.insert(format!("D{i}"), json!({ "says": says, "why": clip_str(v["why"].as_str().unwrap_or(""), 300) }));
    }
    if out.is_empty() { Value::Null } else { Value::Object(out) }
}
pub fn comment(d: &Value, r: &Value) -> String {
    let c = &r["coverage"];
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let mut md = String::from("### Kern: what this change does when it runs\n\n");
    md += &format!("Kern ran **{}%** of the changed lines on both the old and the new code (new code: {} of {} lines, old code: {} of {}), twice each, in a sealed sandbox with the network off.\n\n",
        n(&c["percent"]), n(&c["new"]["ran"]), n(&c["new"]["total"]), n(&c["old"]["ran"]), n(&c["old"]["total"]));
    let ending = |side: &str| -> Vec<String> { r["endings"][side].as_array().into_iter().flatten().filter_map(Value::as_str).map(|l| span(l, 300)).collect() };
    let (eo, en) = (ending("old"), ending("new"));
    if eo != en {
        let say = |e: &[String]| if e.is_empty() { "ended normally".to_string() } else { e.join("; ") };
        md += &format!("**The old and the new code's runs ended differently.** Old code: {}. New code: {}.\n\n", say(&eo), say(&en));
    }
    let items = difference_items(r);
    let lines: Vec<String> = items.iter().filter(|(_, rec)| *rec).map(|(l, _)| format!("- {l}")).collect();
    let printed: Vec<String> = items.iter().filter(|(_, rec)| !*rec).map(|(l, _)| format!("- {l}")).collect();
    let list = |md: &mut String, items: &[String]| {
        for line in items.iter().take(25) { md.push_str(line); md.push('\n'); }
        if items.len() > 25 { md.push_str(&format!("- ... and {} more in Kern's report\n", items.len() - 25)); }
    };
    let shaky: Vec<String> = r["unstable"]["calls"].as_array().into_iter().flatten()
        .map(|c| format!("{} ({} call{})", span(c["call"].as_str().unwrap_or(""), 120), c["count"], if c["count"] == 1 { "" } else { "s" }))
        .chain(r["unstable"]["steps"].as_array().into_iter().flatten().map(|s| format!("step {}", span(s.as_str().unwrap_or(""), 120)))).collect();
    let sorted = &r["sorted"];
    match (lines.is_empty(), printed.is_empty()) {
        (true, true) if !shaky.is_empty() => md += "**No difference** in what Kern could compare.\n\n",
        (true, true) if n(&c["new"]["ran"]) + n(&c["old"]["ran"]) == 0 => md += "**None of the changed code ran**, so Kern saw none of its behaviour.\n\n",
        (true, true) => md += "**Nothing behaves differently** in what Kern ran.\n\n",
        _ if sorted.is_object() => {
            let has_record = items.iter().any(|(_, rec)| *rec);
            let (mut surprises, mut described, mut folded) = (Vec::new(), Vec::new(), Vec::new());
            for (i, (line, rec)) in items.iter().enumerate() {
                let v = &sorted[format!("D{}", i + 1)];
                let why = v["why"].as_str().filter(|w| !w.trim().is_empty()).map(|w| span(w, 200));
                let (verdict, yes) = match v["says"].as_str() {
                    Some("yes") => (format!(" — the pull request: {}", why.unwrap_or_default()), true),
                    Some("unclear") => (format!(" — unclear{}", why.map(|w| format!(": {w}")).unwrap_or_default()), false),
                    _ => (if has_record && !*rec { " — not mentioned by the pull request".to_string() } else { String::new() }, false),
                };
                let line = format!("- {line}{verdict}");
                if has_record && !*rec { folded.push(line); } else if yes { described.push(line); } else { surprises.push(line); }
            }
            md += &format!("**What behaves differently ({})**, sorted by whether the pull request describes it:\n\n", surprises.len() + described.len());
            if !surprises.is_empty() {
                md += &format!("**Not mentioned by the pull request ({}): check these first.**\n\n", surprises.len());
                list(&mut md, &surprises);
                md += "\n";
            }
            if !described.is_empty() {
                md += &format!("**Described by the pull request ({}):**\n\n", described.len());
                list(&mut md, &described);
                md += "\n";
            }
            if !folded.is_empty() {
                md += &format!("<details><summary>What the driver printed differently ({})</summary>\n\n", folded.len());
                list(&mut md, &folded);
                md += "\n</details>\n\n";
            }
            md += "<sub>Sorted by Kern's agent from the pull request's description alone. It can be wrong, so every difference is listed.</sub>\n\n";
        }
        (true, false) => {
            md += &format!("**What behaves differently ({})**, as the driver printed it — check that each one is intended:\n\n", printed.len());
            list(&mut md, &printed);
            md += "\n";
        }
        (false, _) => {
            md += &format!("**What behaves differently ({})** — check that each one is intended:\n\n", lines.len());
            list(&mut md, &lines);
            if !printed.is_empty() {
                md += &format!("\n<details><summary>What the driver printed differently ({})</summary>\n\n", printed.len());
                list(&mut md, &printed);
                md += "\n</details>\n";
            }
            md += "\n";
        }
    }
    if !shaky.is_empty() {
        md += "**Kern could not compare these**, because they answered differently on two runs of the same code (a random port, a clock or an order Kern \
               does not fix): read them yourself.\n\n";
        for line in shaky.iter().take(20) { md += &format!("- {line}\n"); }
        if shaky.len() > 20 { md += &format!("- ... and {} more in Kern's report\n", shaky.len() - 20); }
        md += "\n";
    }
    let failed: Vec<String> = ["new", "old"].iter().flat_map(|side| r["installs"][*side].as_array().into_iter().flatten().filter(|i| i["result"].as_str().is_some_and(|x| x.starts_with("failed") || x.starts_with("out of time")))
        .map(move |i| format!("- {} ({side} code): {}", span(i["dir"].as_str().unwrap_or("."), 120), span(i["result"].as_str().unwrap_or(""), 300)))).collect();
    if !failed.is_empty() { md += &format!("**Dependencies that could not be installed** — code that needs them could not run:\n\n{}\n\n", failed.join("\n")); }
    let loose: Vec<String> = ["new", "old"].iter().flat_map(|side| r["installs"][*side].as_array().into_iter().flatten().filter_map(move |i| {
        let why = i["result"].as_str()?.strip_prefix("installed from package.json (")?.strip_suffix(')')?;
        let why = if why.chars().all(|c| c.is_ascii_alphabetic() || c == ' ' || c == '\'') { why.to_string() } else { span(why, 120) };
        Some(format!("{} ({side} code: {why})", span(i["dir"].as_str().unwrap_or("."), 120)))
    })).collect();
    if !loose.is_empty() { md += &format!("**Installed without the project's lockfile**: {}. Package versions may differ from the project's, so a difference can come from them.\n\n", loose.join(", ")); }
    let touching: Vec<String> = d["measurement"].as_array().into_iter().flatten().take(10).map(|p| span(p.as_str().unwrap_or(""), 160)).collect();
    if !touching.is_empty() { md += &format!("**This change refers to Kern's own measurement** (its coverage folder, a source map or its call record), which code can use to fake what ran: {}. Read it closely.\n\n", touching.join(", ")); }
    let stand_ins: Vec<String> = r["stubs"]["used"].as_array().into_iter().flatten().take(30).map(|s| span(s.as_str().unwrap_or(""), 100)).collect();
    if !stand_ins.is_empty() { md += &format!("**Packages the driver replaced with stand-ins**, because they cannot load here (the real ones did not run): {}\n\n", stand_ins.join(", ")); }
    let skipped: Vec<String> = d["skipped"].as_array().into_iter().flatten().take(20).map(|s| format!("- {}: {}", span(s["path"].as_str().unwrap_or(""), 160), s["why"].as_str().unwrap_or(""))).collect();
    if !skipped.is_empty() { md += &format!("**Changed files Kern did not measure:**\n\n{}\n\n", skipped.join("\n")); }
    let others: Vec<(String, String)> = d["unmeasured"].as_array().into_iter().flatten().map(|u| (u["why"].as_str().unwrap_or("other").to_string(), u["path"].as_str().unwrap_or("").to_string())).collect();
    if !others.is_empty() {
        let mut kinds: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (k, p) in &others { kinds.entry(k.as_str()).or_default().push(p.as_str()); }
        let parts: Vec<String> = kinds.iter().map(|(k, ps)| format!("{k} ({}): {}{}", ps.len(), ps.iter().take(5).map(|p| span(p, 100)).collect::<Vec<_>>().join(", "), if ps.len() > 5 { ", ..." } else { "" })).collect();
        md += &format!("**Other changed files Kern does not run** ({}): {}.\n\n", others.len(), parts.join("; "));
    }
    let mut unread: Vec<String> = Vec::new();
    for f in r["files"].as_array().into_iter().flatten() {
        for which in ["new", "old"] {
            let lines: Vec<u64> = f[which]["missed"].as_array().into_iter().flatten().filter_map(|x| x[0].as_u64()).collect();
            if lines.is_empty() { continue; }
            let all = ranges(&lines);
            let shown: Vec<&str> = all.split(", ").take(40).collect();
            let more = all.split(", ").count().saturating_sub(40);
            unread.push(format!("- {} ({which} code) line{} {}{}", span(f["path"].as_str().unwrap_or(""), 160), if lines.len() == 1 { "" } else { "s" }, shown.join(", "),
                if more > 0 { format!(" and {more} more ranges") } else { String::new() }));
        }
    }
    let measured = n(&c["new"]["total"]) + n(&c["old"]["total"]);
    if unread.is_empty() && measured > 0 { md += "**Every changed line Kern measured ran.**\n\n"; }
    else if !unread.is_empty() {
        md += &format!("**Changed lines Kern could not run ({}):** these were not run, so read them yourself:\n\n", unread.len());
        for line in unread.iter().take(40) { md += line; md += "\n"; }
        if unread.len() > 40 { md += &format!("- ... and {} more files\n", unread.len() - 40); }
        md += "\n";
    }
    let commit = d["head"].as_str().filter(|h| h.len() >= 7 && h.bytes().all(|b| b.is_ascii_hexdigit())).map(|h| format!("Commit `{}`. ", &h[..7])).unwrap_or_default();
    md += &format!("<sub>{commit}Kern counts a line as run only when its own coverage shows that all of its code ran; it records every call into the changed modules itself and runs each side twice to set noise aside.</sub>\n");
    if md.len() > 60_000 {
        let mut cut = 60_000;
        while !md.is_char_boundary(cut) { cut -= 1; }
        md.truncate(md[..cut].rfind('\n').unwrap_or(cut));
        md += "\n... (cut short; the full report is in Kern's output)\n";
    }
    md
}

pub fn install_dirs(paths: &BTreeSet<String>, code: &[String], root_package: Option<&str>, pnpm_workspace: Option<&str>) -> Vec<String> {
    let mut patterns: Vec<String> = Vec::new();
    if let Some(p) = root_package.and_then(|t| serde_json::from_str::<Value>(t).ok()) {
        let w = if p["workspaces"].is_array() { &p["workspaces"] } else { &p["workspaces"]["packages"] };
        patterns.extend(w.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    }
    if let Some(y) = pnpm_workspace {
        patterns.extend(y.lines().filter_map(|l| l.trim().strip_prefix('-')).map(|p| p.trim().trim_matches(|c| c == '\'' || c == '"').to_string()));
    }
    let member = |dir: &str| patterns.iter().filter(|p| !p.starts_with('!')).any(|p| {
        let re: String = p.trim_end_matches('/').split('/').map(|part| match part { "**" => ".*".to_string(), _ => part.split('*').map(regex_quote).collect::<Vec<_>>().join("[^/]*") }).collect::<Vec<_>>().join("/");
        glob_match(&re, dir)
    });
    let mut dirs: Vec<String> = Vec::new();
    if paths.contains("package.json") { dirs.push(".".into()); }
    for p in code {
        let mut d = p.rsplit_once('/').map_or("", |x| x.0).to_string();
        let owner = loop {
            let pkg = if d.is_empty() { "package.json".to_string() } else { format!("{d}/package.json") };
            if paths.contains(&pkg) { break Some(d.clone()); }
            if d.is_empty() { break None; }
            d = d.rsplit_once('/').map_or("", |x| x.0).to_string();
        };
        if let Some(o) = owner && !o.is_empty() && !member(&o) && !dirs.contains(&o) && dirs.len() < 16 { dirs.push(o); }
    }
    dirs
}
fn regex_quote(s: &str) -> String { s.chars().map(|c| if "\\.+?()[]{}|^$".contains(c) { format!("\\{c}") } else { c.to_string() }).collect() }
fn glob_match(pattern: &str, dir: &str) -> bool {
    let mut tokens: Vec<(u8, char)> = Vec::new();
    let b: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' && i + 1 < b.len() { tokens.push((0, b[i + 1])); i += 2; }
        else if pattern[byte_at(&b, i)..].starts_with("[^/]*") { tokens.push((1, ' ')); i += 5; }
        else if pattern[byte_at(&b, i)..].starts_with(".*") { tokens.push((2, ' ')); i += 2; }
        else { tokens.push((0, b[i])); i += 1; }
    }
    fn go(t: &[(u8, char)], s: &[char]) -> bool {
        match t.first() {
            None => s.is_empty(),
            Some((0, c)) => s.first() == Some(c) && go(&t[1..], &s[1..]),
            Some((kind, _)) => (0..=s.len()).take_while(|k| *kind == 2 || !s[..*k].contains(&'/')).any(|k| go(&t[1..], &s[k..])),
        }
    }
    go(&tokens, &dir.chars().collect::<Vec<_>>())
}
fn byte_at(chars: &[char], i: usize) -> usize { chars[..i].iter().map(|c| c.len_utf8()).sum() }

pub fn tools() -> crate::git::Files {
    let entry = |text: &str| { let b = text.as_bytes().to_vec(); crate::git::Entry { mode: "100644".into(), oid: crate::git::object_id("blob", &b), size: b.len() as u64, bytes: Some(b) } };
    [("package.json", include_str!("drive-tools/package.json")), ("package-lock.json", include_str!("drive-tools/package-lock.json"))].iter().map(|(p, t)| (p.to_string(), entry(t))).collect()
}
pub fn tsconfig(project_has_one: bool) -> String {
    let extends = if project_has_one { "\"extends\":\"/workspace/tsconfig.json\"," } else { "" };
    format!("{{{extends}\"compilerOptions\":{{\"jsx\":\"react-jsx\"}},\"include\":[\"/workspace/**/*\"]}}\n")
}

pub const LOADER: &str = include_str!("drive-loader.mjs");
pub const HOOKS: &str = include_str!("drive-hooks.mjs");
pub const SUPPORT: &str = include_str!("drive-support.mjs");
pub const BROWSER: &str = include_str!("drive-browser.mjs");
pub const C8: &str = include_str!("drive-c8.mjs");
pub const RUNNER: &str = include_str!("drive-run.mjs");
fn run_limits() -> Limits { Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: 300, memory_mb: 2048, pids: 512 } }
fn setup_limits() -> Limits { Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 1 << 20, timeout_seconds: 1800, memory_mb: 4096, pids: 1024 } }

pub fn linux_path(p: &str) -> bool {
    !p.is_empty() && p.len() <= 4096 && !p.starts_with('/') && !p.contains('\0') && p.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && c != ".git")
}
fn materialize(repo: &Path, commit: &str, dest: &Path) -> R<BTreeSet<String>> {
    let items = listing(repo, commit)?;
    let files: Vec<&Item> = items.iter().filter(|i| matches!(i.mode.as_str(), "100644" | "100755") && linux_path(&i.path)).collect();
    ensure!(files.iter().map(|i| i.size).sum::<u64>() <= 4 << 30, "The repository holds more than 4 GiB of files");
    std::fs::create_dir_all(dest)?;
    blobs(repo, &files, |item, bytes| {
        let target = item.path.split('/').fold(dest.to_path_buf(), |p, part| p.join(part));
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&target, bytes)?;
        #[cfg(unix)]
        std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(if item.mode == "100755" { 0o755 } else { 0o644 }))?;
        Ok(())
    })?;
    Ok(files.iter().map(|i| i.path.clone()).collect())
}

pub fn setup(out: &Path, op: &crate::policy::Operator) -> R<Value> {
    let d = read_own(out.join("drive.json"))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let root = out.join("sandbox");
    if root.exists() { std::fs::remove_dir_all(&root)?; }
    std::fs::create_dir(&root)?;
    let code: Vec<String> = d["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(str::to_string)).collect();
    let mut sides = serde_json::Map::new();
    let mut volumes = Vec::new();
    for (side, commit) in [("old", d["base"].as_str().unwrap_or("")), ("new", d["head"].as_str().unwrap_or(""))] {
        let tree = root.join(side);
        let paths = materialize(repo, commit, &tree)?;
        let read = |p: &str| std::fs::read_to_string(tree.join(p)).ok();
        let dirs = install_dirs(&paths, &code, read("package.json").as_deref(), read("pnpm-workspace.yaml").as_deref());
        let control = root.join(format!("setup-{side}"));
        crate::sandbox::control_dir(&control, Some(("drive-setup.json", &json!({ "version": 1, "dirs": dirs, "timeout_seconds": 1500 }))))?;
        let volume = crate::sandbox::Volume::create()?;
        let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &op.image, snapshot: &tree, control: &control, writable: &[], limits: &setup_limits(), entry: vec!["__drive_setup".into()],
            network: "bridge", volumes: vec![], root: true, home_mb: 2048, scratch: false, workspace: Some((&volume.0, false)) })?;
        ensure!(run.exit_code == 0, "Setting up the {side} code failed in the sandbox (exit code {})", run.exit_code);
        let record: Value = serde_json::from_slice(&run.stdout).map_err(|_| Denied(format!("The {side} code's setup gave no record")))?;
        sides.insert(side.into(), json!({ "volume": volume.0, "installs": record["installs"], "tsconfig": paths.contains("tsconfig.json") }));
        volumes.push(volume);
        let _ = std::fs::remove_dir_all(&tree);
        let _ = std::fs::remove_dir_all(&control);
    }
    let mut public = op.clone();
    public.registry = Some("registry.npmjs.org".into());
    let tools_root = root.join("tools");
    std::fs::create_dir(&tools_root)?;
    let deps = crate::flow::install_dependencies(&tools(), &public, &tools_root, &op.image)?;
    let mut volume = deps.volume;
    let cached = volume.kept();
    volume.keep();
    for v in &mut volumes { v.keep(); }
    let s = json!({ "version": 1, "old": sides["old"], "new": sides["new"], "tools": { "volume": volume.0, "cached": cached } });
    write_json(out.join("setup.json"), &s)?;
    Ok(s)
}

pub fn run(out: &Path, op: &crate::policy::Operator, driver: &Path) -> R<Value> {
    let d = read_own(out.join("drive.json"))?;
    let driver_bytes = std::fs::read(driver)?;
    ensure!(driver_bytes.len() <= 1 << 20, "The driver is larger than 1 MiB");
    let (stand_ins, refused) = stubs(driver.parent().unwrap_or(Path::new(".")))?;
    let stub_names: Vec<&str> = stand_ins.iter().map(|(n, _)| n.as_str()).collect();
    if !out.join("setup.json").exists() { setup(out, op)?; }
    let s = read_own(out.join("setup.json"))?;
    let rounds = out.join("rounds");
    std::fs::create_dir_all(&rounds)?;
    let round = std::fs::read_dir(&rounds)?.count() as u32 + 1;
    let dir = rounds.join(round.to_string());
    std::fs::create_dir(&dir)?;
    create_new(dir.join("driver.mjs"), &driver_bytes, 0o644)?;
    let put_stubs = |root: &Path, mode: u32| -> R<()> {
        for (name, bytes) in &stand_ins {
            let p = root.join("stubs").join(format!("{name}.mjs"));
            if let Some(parent) = p.parent() { std::fs::create_dir_all(parent)?; }
            create_new(&p, bytes, mode)?;
        }
        Ok(())
    };
    put_stubs(&dir, 0o644)?;
    let input = dir.join("input");
    std::fs::create_dir(&input)?;
    let tools_volume = s["tools"]["volume"].as_str().unwrap_or("").to_string();
    let limits = run_limits();
    let one = |side: &str, k: u32| -> R<Run> {
        let control = dir.join(format!("control-{side}-{k}"));
        crate::sandbox::control_dir(&control, Some(("drive-run.json", &json!({ "version": 1, "timeout_seconds": 200 }))))?;
        let modules: Vec<Value> = d["modules"].as_array().into_iter().flatten().map(|m| json!({ "path": m["path"], "functions": m[side]["functions"], "cjs": m[side]["cjs"], "default": m[side]["default"] })).collect();
        create_new(control.join("drive.json"), json!({ "modules": modules, "stubs": stub_names }).to_string().as_bytes(), 0o444)?;
        put_stubs(&control, 0o444)?;
        #[cfg(unix)]
        if !stand_ins.is_empty() { std::os::unix::fs::symlink("/workspace/node_modules", control.join("node_modules"))?; }
        create_new(control.join("drive-loader.mjs"), LOADER.as_bytes(), 0o444)?;
        create_new(control.join("drive-hooks.mjs"), HOOKS.as_bytes(), 0o444)?;
        create_new(control.join("drive-support.mjs"), SUPPORT.as_bytes(), 0o444)?;
        create_new(control.join("drive-browser.mjs"), BROWSER.as_bytes(), 0o444)?;
        create_new(control.join("empty-marker.js"), b"", 0o444)?;
        create_new(control.join("drive-c8.mjs"), C8.as_bytes(), 0o444)?;
        create_new(control.join("drive-run.mjs"), RUNNER.as_bytes(), 0o444)?;
        create_new(control.join("driver.mjs"), &driver_bytes, 0o444)?;
        create_new(control.join("tsconfig.json"), tsconfig(s[side]["tsconfig"] == true).as_bytes(), 0o444)?;
        let volume = s[side]["volume"].as_str().unwrap_or("");
        let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &op.image, snapshot: &input, control: &control, writable: &[], limits: &limits, entry: vec!["__drive".into()],
            network: "none", volumes: vec![(&tools_volume, "/kern-tools/node_modules", true)], root: false, home_mb: 0, scratch: false, workspace: Some((volume, true)) });
        let _ = std::fs::remove_dir_all(&control);
        let run = match run {
            Err(e) if e.0.contains("exceeded limits") || e.0.contains("timed out") => return Ok(Run { errors: format!("kern: this run was stopped: {} (the sandbox allows 2 GiB of memory and 5 minutes)", e.0), ..Run::default() }),
            other => other?,
        };
        if run.exit_code != 0 { return Ok(Run { errors: format!("kern: this run failed in the sandbox (exit code {})", run.exit_code), ..Run::default() }); }
        Run::parse(&run.stdout)
    };
    let pair = |side: &str| -> R<(Run, Run)> {
        std::thread::scope(|sc| {
            let (a, b) = (sc.spawn(|| one(side, 1)), sc.spawn(|| one(side, 2)));
            let stopped = || Denied("A drive run stopped unexpectedly".into());
            Ok((a.join().map_err(|_| stopped())??, b.join().map_err(|_| stopped())??))
        })
    };
    let (o1, o2) = pair("old")?;
    let (n1, n2) = pair("new")?;
    let raw = dir.join("runs");
    std::fs::create_dir(&raw)?;
    for (name, r) in [("old-1", &o1), ("old-2", &o2), ("new-1", &n1), ("new-2", &n2)] {
        std::fs::write(raw.join(format!("{name}.out")), &r.output)?;
        std::fs::write(raw.join(format!("{name}.err")), &r.errors)?;
        std::fs::write(raw.join(format!("{name}.calls")), r.calls.iter().map(|c| format!("{c}\n")).collect::<String>())?;
        std::fs::write(raw.join(format!("{name}.behavior")), r.behavior.iter().map(|b| format!("{b}\n")).collect::<String>())?;
    }
    let mut report = score(&d, &[o1, o2, n1, n2], round);
    report["installs"] = json!({ "old": s["old"]["installs"], "new": s["new"]["installs"] });
    report["stubs"] = json!({ "used": stub_names, "refused": refused });
    write_json(dir.join("report.json"), &report)?;
    std::fs::write(out.join("task").join("feedback.md"), feedback(&d, &report))?;
    std::fs::write(out.join("comment.md"), comment(&d, &report))?;
    Ok(json!({ "status": "RAN", "round": round, "coverage": report["coverage"], "steps": report["steps"], "calls": report["calls"], "behavior": report["behavior"],
        "report": dir.join("report.json").display().to_string(), "comment": out.join("comment.md").display().to_string() }))
}

pub type StandIns = (Vec<(String, Vec<u8>)>, Vec<Value>);
pub fn stubs(driver_dir: &Path) -> R<StandIns> {
    let root = driver_dir.join("stubs");
    let (mut used, mut refused) = (Vec::new(), Vec::new());
    if !std::fs::symlink_metadata(&root).is_ok_and(|m| m.is_dir()) { return Ok((used, refused)); }
    let (mut total, mut seen) = (0usize, 0usize);
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir)? {
            let path = e?.path();
            seen += 1;
            ensure!(seen <= 1000, "The stubs folder holds more than 1000 entries");
            let rel = path.strip_prefix(&root).map_err(|_| Denied("A stand-in lies outside the stubs folder".into()))?.to_string_lossy().replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_dir() { stack.push(path); continue; }
            let why = if !meta.is_file() { Some("not a regular file") }
                else if !rel.ends_with(".mjs") { Some("a stand-in is an .mjs file") }
                else if !package_specifier(&rel[..rel.len() - 4]) { Some("not a package name: stand-ins replace packages, never the project's own code") }
                else if meta.len() > 256 * 1024 { Some("larger than 256 KiB") }
                else if used.len() >= 64 { Some("more than 64 stand-ins") }
                else if total + meta.len() as usize > 2 << 20 { Some("the stand-ins pass 2 MiB in all") }
                else { None };
            if let Some(why) = why { refused.push(json!({ "path": format!("stubs/{rel}"), "why": why })); continue; }
            let bytes = std::fs::read(&path)?;
            total += bytes.len();
            used.push((rel[..rel.len() - 4].to_string(), bytes));
        }
    }
    used.sort();
    Ok((used, refused))
}
pub fn package_specifier(s: &str) -> bool {
    let parts: Vec<&str> = s.split('/').collect();
    let part = |p: &str| !p.is_empty() && p != "." && p != ".." && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let name = if parts[0].starts_with('@') { if parts.len() < 2 || !part(&parts[0][1..]) { return false; } 1 } else { 0 };
    s.len() <= 214 && !parts[name].starts_with(['.', '_']) && parts[name..].iter().all(|p| part(p))
}

pub fn clean(out: &Path) -> R<Value> {
    let s = read_own(out.join("setup.json"))?;
    let mut volumes: Vec<&str> = ["old", "new"].iter().filter_map(|side| s[side]["volume"].as_str()).collect();
    if s["tools"]["cached"] != true { volumes.extend(s["tools"]["volume"].as_str()); }
    let mut removed = Vec::new();
    for v in volumes.into_iter().filter(|v| v.starts_with("kern-deps-")) {
        let filter = format!("volume={v}");
        if let Ok(ids) = crate::sandbox::docker(&["ps", "--all", "--quiet", "--filter", &filter, "--filter", "name=kern-"], std::time::Duration::from_secs(60), 1 << 16) {
            for id in String::from_utf8_lossy(&ids).split_whitespace() { let _ = crate::sandbox::docker(&["rm", "--force", id], std::time::Duration::from_secs(60), 4096); }
        }
        crate::sandbox::docker(&["volume", "rm", "--force", v], std::time::Duration::from_secs(120), 4096)?;
        removed.push(v.to_string());
    }
    let _ = std::fs::remove_dir_all(out.join("sandbox"));
    std::fs::remove_file(out.join("setup.json"))?;
    Ok(json!({ "status": "CLEANED", "removed": removed }))
}

pub const FIRST: &str = "Read task.md in this folder and do what it says: write driver.mjs here. Read pr.md, diff.patch and lines.md first, then the code in new/ and old/.";
pub const AGAIN: &str = "Kern ran your driver.mjs. Read feedback.md in this folder: it lists the measured changed lines that did not run, the error output, the steps that threw and what differs between the old and the new code. Revise driver.mjs so the lines that did not run do run, keeping the steps that already work. If a line can never run (dead code), say which and why in notes.md. task.md still applies.";
pub fn merge(reports: &[Value]) -> Value {
    let Some(last) = reports.last() else { return Value::Null };
    let mut out = last.clone();
    let (mut files, mut totals) = (Vec::new(), [(0u64, 0u64), (0u64, 0u64)]);
    for f in last["files"].as_array().into_iter().flatten() {
        let mut entry = json!({ "path": f["path"] });
        for (i, side) in ["new", "old"].iter().enumerate() {
            let total = f[*side]["total"].as_u64().unwrap_or(0);
            let (mut missed, mut loaded): (Option<Vec<Value>>, bool) = (None, false);
            for r in reports {
                let rf = r["files"].as_array().into_iter().flatten().find(|x| x["path"] == f["path"]);
                let m = rf.and_then(|x| x[*side]["missed"].as_array().cloned()).unwrap_or_default();
                loaded |= rf.is_some_and(|x| x[*side]["loaded"] == true);
                missed = Some(match missed { None => m, Some(before) => before.into_iter().filter(|x| m.iter().any(|y| y[0] == x[0])).collect() });
            }
            let missed = missed.unwrap_or_default();
            let ran = total.saturating_sub(missed.len() as u64);
            totals[i] = (totals[i].0 + ran, totals[i].1 + total);
            entry[*side] = json!({ "ran": ran, "total": total, "loaded": loaded, "missed": missed });
        }
        files.push(entry);
    }
    let (ran, total) = (totals[0].0 + totals[1].0, totals[0].1 + totals[1].1);
    out["files"] = json!(files);
    out["coverage"] = json!({ "new": { "ran": totals[0].0, "total": totals[0].1 }, "old": { "ran": totals[1].0, "total": totals[1].1 },
        "percent": (ran * 100).checked_div(total).unwrap_or(100) });
    for path in [&["differences"][..], &["call_differences"], &["behavior", "new"], &["behavior", "gone"], &["unstable", "steps"], &["unstable", "calls"],
        &["endings", "old"], &["endings", "new"], &["stubs", "used"], &["threw"]] {
        let mut all: Vec<Value> = Vec::new();
        for r in reports {
            let list = path.iter().fold(r, |v, k| &v[*k]);
            for x in list.as_array().into_iter().flatten() { if !all.contains(x) { all.push(x.clone()); } }
        }
        let slot = path.iter().fold(&mut out, |v, k| &mut v[*k]);
        *slot = json!(all);
    }
    out["rounds"] = json!(reports.len());
    out
}
fn round_reports(out: &Path) -> Vec<Value> {
    (1..=64u32).filter_map(|n| read_own(out.join("rounds").join(n.to_string()).join("report.json")).ok()).collect()
}
pub const SORT: &str = "Read pr.md and differences.md in this folder. differences.md lists, numbered D1, D2 and so on, what behaves differently between the old and the new code. For each one, decide from pr.md alone whether the pull request says that this should change. Write verdicts.json here with one entry per difference, like {\"D1\": {\"says\": \"yes\", \"why\": \"...\"}}: says is yes (pr.md describes this change; quote its words in why), no (pr.md does not mention it) or unclear. Change no other file.";
fn conclude(out: &Path, d: &Value, sorted: Option<Value>) -> R<Value> {
    let mut report = merge(&round_reports(out));
    ensure!(!report.is_null(), "No round has run yet");
    if let Some(s) = sorted { report["sorted"] = s; }
    let path = out.join("report.json");
    let fresh = out.join("report.json.new");
    let _ = std::fs::remove_file(&fresh);
    write_json(&fresh, &report)?;
    std::fs::rename(&fresh, &path)?;
    std::fs::write(out.join("comment.md"), comment(d, &report))?;
    Ok(report)
}
pub fn sort(out: &Path, verdicts: Option<&Path>) -> R<Value> {
    let d = read_own(out.join("drive.json"))?;
    let Some(verdicts) = verdicts else {
        let report = conclude(out, &d, None)?;
        let count = difference_items(&report).len();
        if count > 0 { std::fs::write(out.join("task").join("differences.md"), differences_md(&report))?; }
        return Ok(json!({ "status": if count > 0 { "ASK" } else { "NOTHING_TO_SORT" }, "differences": count, "percent": report["coverage"]["percent"] }));
    };
    let merged = merge(&round_reports(out));
    let report = conclude(out, &d, Some(read_verdicts(verdicts, &merged)))?;
    Ok(json!({ "status": if report["sorted"].is_object() { "SORTED" } else { "NOT_SORTED" }, "percent": report["coverage"]["percent"] }))
}
pub fn default_agent(model: &str) -> R<Vec<String>> {
    ensure!(!model.is_empty() && model.len() <= 200 && !model.starts_with('-') && model.chars().all(|c| c.is_ascii_alphanumeric() || "._:/@[]-".contains(c)),
        "KERN_DRIVE_MODEL must be a model name such as sonnet");
    Ok(["claude", "-p", "{prompt}", "--restricted", "--tools", "Read,Write,Edit,Glob,Grep", "--permission-mode", "acceptEdits", "--model", model,
        "--output-format", "json", "--no-session-persistence", "--strict-mcp-config"].iter().map(|s| s.to_string()).collect())
}
fn agent_may_see(name: &str, extra: &[String]) -> bool {
    const NAMES: [&str; 35] = ["PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "TERM", "TZ", "TMPDIR", "TEMP", "TMP", "HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY",
        "ALL_PROXY", "NODE_EXTRA_CA_CERTS", "SSL_CERT_FILE", "SSL_CERT_DIR", "SYSTEMROOT", "WINDIR", "SYSTEMDRIVE", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
        "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "COMMONPROGRAMFILES", "PATHEXT", "COMSPEC", "HOMEDRIVE", "HOMEPATH", "USERNAME", "NUMBER_OF_PROCESSORS", "OS"];
    const PREFIXES: [&str; 9] = ["ANTHROPIC_", "CLAUDE_", "LC_", "XDG_", "AWS_", "GOOGLE_", "CLOUD_ML_", "VERTEX_", "DISABLE_"];
    let n = name.to_ascii_uppercase();
    NAMES.contains(&n.as_str()) || PREFIXES.iter().any(|p| n.starts_with(p)) || extra.iter().any(|e| e.eq_ignore_ascii_case(name))
}
const TASK_FILES: [&str; 12] = ["task.md", "pr.md", "diff.patch", "lines.md", "old", "new", "feedback.md", "differences.md", "driver.mjs", "stubs", "notes.md", "verdicts.json"];
pub fn tidy_task(task: &Path) -> R<Vec<String>> {
    let mut removed = Vec::new();
    for e in std::fs::read_dir(task)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if TASK_FILES.contains(&name.as_str()) { continue; }
        if std::fs::symlink_metadata(e.path())?.is_dir() { std::fs::remove_dir_all(e.path())?; } else { std::fs::remove_file(e.path())?; }
        removed.push(name);
    }
    removed.sort();
    Ok(removed)
}
pub fn another_round(percents: &[u64], max: usize) -> bool {
    match percents {
        [] => max > 0,
        _ if percents.len() >= max => false,
        [.., last] if *last >= 95 => false,
        [_] => true,
        [.., before, last] => last.saturating_sub(*before) >= 5,
    }
}
pub fn agent_round(task: &Path, argv: &[String], prompt: &str, timeout: std::time::Duration) -> R<Value> {
    ensure!(!argv.is_empty(), "The agent command is empty");
    let args: Vec<String> = argv.iter().map(|a| a.replace("{prompt}", prompt)).collect();
    let mut command = std::process::Command::new(&args[0]);
    let extra: Vec<String> = std::env::var("KERN_DRIVE_AGENT_ENV").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    command.env_clear().envs(std::env::vars_os().filter(|(name, _)| agent_may_see(&name.to_string_lossy(), &extra)));
    tidy_task(task)?;
    let mut child = command.args(&args[1..]).current_dir(task).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|e| Denied(format!("The agent command could not start: {e}")))?;
    let mut out = child.stdout.take().ok_or_else(|| Denied("The agent command has no output".into()))?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut std::io::Read::take(&mut out, 1 << 20), &mut b);
        let _ = std::io::copy(&mut out, &mut std::io::sink());
        b
    });
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? { break Some(s); }
        if started.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); break None; }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let stdout = if status.is_some() { reader.join().unwrap_or_default() } else { Vec::new() };
    let said = serde_json::from_slice::<Value>(&stdout).ok().and_then(|v| v["result"].as_str().map(str::to_string)).unwrap_or_else(|| String::from_utf8_lossy(&stdout).to_string());
    Ok(json!({ "exit": status.and_then(|s| s.code()), "timed_out": status.is_none(), "seconds": started.elapsed().as_secs(), "said": clip_str(said.trim(), 600) }))
}
#[allow(clippy::too_many_arguments)]
pub fn auto(repo: &Path, base: &str, head: &str, out: &Path, op: &crate::policy::Operator, pr: Option<&str>, agent: &[String], max: usize) -> R<Value> {
    if !out.join("drive.json").exists() { prepare(repo, base, head, out, pr)?; }
    let d = read_own(out.join("drive.json"))?;
    if d["totals"]["new"] == 0 && d["totals"]["old"] == 0 {
        let mut md = String::from("### Kern: what this change does when it runs\n\nThis change touches no line of code Kern can run: only tests, documentation, configuration, types or files it does not measure.\n");
        for s in d["skipped"].as_array().into_iter().flatten().take(20) { md += &format!("- {}: {}\n", span(s["path"].as_str().unwrap_or(""), 160), s["why"].as_str().unwrap_or("")); }
        std::fs::write(out.join("comment.md"), md)?;
        return Ok(json!({ "status": "NOTHING_TO_RUN", "comment": out.join("comment.md").display().to_string() }));
    }
    let task = out.join("task");
    let (mut percents, mut rounds) = (Vec::new(), Vec::new());
    while another_round(&percents, max) {
        let a = agent_round(&task, agent, if percents.is_empty() { FIRST } else { AGAIN }, std::time::Duration::from_secs(900))?;
        let driver = task.join("driver.mjs");
        if !driver.exists() { rounds.push(json!({ "agent": a, "error": "the agent wrote no driver.mjs" })); break; }
        let r = run(out, op, &driver)?;
        percents.push(r["coverage"]["percent"].as_u64().unwrap_or(0));
        rounds.push(json!({ "agent": a, "run": r }));
    }
    let (mut sorting, mut percent) = (Value::Null, Value::Null);
    if percents.is_empty() {
        let said = rounds.last().and_then(|r| r["agent"]["said"].as_str()).unwrap_or("");
        std::fs::write(out.join("comment.md"), format!("### Kern: what this change does when it runs\n\nKern asked its agent for a driver, but it wrote none, so nothing ran: read this change yourself.\n\nThe agent said: {}\n", span(said, 400)))?;
    } else {
        let report = conclude(out, &d, None)?;
        percent = report["coverage"]["percent"].clone();
        if pr.is_some_and(|p| !p.trim().is_empty()) && !difference_items(&report).is_empty() {
            std::fs::write(task.join("differences.md"), differences_md(&report))?;
            let _ = std::fs::remove_file(task.join("verdicts.json"));
            let a = agent_round(&task, agent, SORT, std::time::Duration::from_secs(600)).unwrap_or_else(|e| json!({ "error": e.0 }));
            sorting = json!({ "agent": a, "result": sort(out, Some(&task.join("verdicts.json")))? });
        }
    }
    Ok(json!({ "status": if percents.is_empty() { "NO_DRIVER" } else { "RAN" }, "percent": percent, "percents": percents, "rounds": rounds, "sorting": sorting,
        "comment": out.join("comment.md").display().to_string() }))
}

pub const COMMENT_MARK: &str = "<!-- kern-drive -->";
pub fn post(out: &Path, repository: &str, number: u64, update_only: bool) -> R<Value> {
    post_to(&crate::github::Api::from_env("post Kern's comment on the pull request")?, out, repository, number, update_only)
}
pub fn post_with(out: &Path, repository: &str, number: u64, api: &str, token: &str, update_only: bool) -> R<Value> {
    post_to(&crate::github::Api::new(api, token)?, out, repository, number, update_only)
}
fn post_to(api: &crate::github::Api, out: &Path, repository: &str, number: u64, update_only: bool) -> R<Value> {
    ensure!(crate::github::repository(repository), "Expected the repository as owner/name");
    ensure!(number > 0, "Expected a pull request number");
    if let Some(head) = read_own(out.join("drive.json")).ok().and_then(|d| d["head"].as_str().map(str::to_string))
        && let Ok(pull) = api.get(&format!("/repos/{repository}/pulls/{number}"))
        && let Some(now) = pull["head"]["sha"].as_str()
        && now != head {
        return Ok(json!({ "status": "STALE", "checked": head, "head": now }));
    }
    let body = format!("{COMMENT_MARK}\n{}", std::fs::read_to_string(out.join("comment.md"))?);
    let author = crate::github::env("KERN_COMMENT_AUTHOR").unwrap_or_else(|| "github-actions[bot]".into());
    let mut existing = None;
    for page in 1..=10 {
        let list = api.get(&format!("/repos/{repository}/issues/{number}/comments?per_page=100&page={page}"))?;
        let items = list.as_array().cloned().unwrap_or_default();
        existing = items.iter().find(|c| c["body"].as_str().is_some_and(|b| b.starts_with(COMMENT_MARK)) && c["user"]["login"] == author.as_str()).and_then(|c| c["id"].as_u64());
        if existing.is_some() || items.len() < 100 { break; }
    }
    match existing {
        Some(id) => { api.patch(&format!("/repos/{repository}/issues/comments/{id}"), &json!({ "body": body }))?; Ok(json!({ "status": "UPDATED", "comment": id })) }
        None if update_only => Ok(json!({ "status": "SKIPPED", "reason": "no earlier comment of Kern's to update" })),
        None => { let c = api.post(&format!("/repos/{repository}/issues/{number}/comments"), &json!({ "body": body }))?; Ok(json!({ "status": "POSTED", "comment": c["id"] })) }
    }
}
