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
    let (x, y) = crate::diff::changed_lines(&a, &b);
    (number(x), number(y))
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

pub fn same_program(path: &str, old: &str, new: &str) -> bool {
    let Some(types) = source_types(path) else { return false };
    types.into_iter().any(|st| {
        let (a, b) = (Allocator::default(), Allocator::default());
        let options = oxc_parser::ParseOptions { preserve_parens: false, ..Default::default() };
        let (po, pn) = (Parser::new(&a, old, st).with_options(options).parse(), Parser::new(&b, new, st).with_options(options).parse());
        !po.fatal_error && !pn.fatal_error && po.diagnostics.is_empty() && pn.diagnostics.is_empty() && oxc_span::ContentEq::content_eq(&po.program, &pn.program)
    })
}

const BINARY: [&str; 52] = ["png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp", "tif", "tiff", "psd", "heic", "mp3", "mp4", "m4a", "mov", "webm",
    "wav", "ogg", "flac", "avi", "mkv", "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "jar", "war", "woff", "woff2", "ttf", "otf", "eot", "glb", "gltf",
    "fbx", "blend", "wasm", "bin", "exe", "dll", "so", "dylib", "node", "db", "sqlite", "pyc", "class"];
const LOCKFILES: [&str; 6] = ["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lockb", "bun.lock"];
const LARGEST: u64 = 1 << 20;
const MOST_FILES: usize = 2000;
const MOST_TEXT: u64 = 128 << 20;
fn extension(path: &str) -> String { path.rsplit('/').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default() }
fn unmeasured_kind(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let dirs: Vec<String> = path.split('/').map(str::to_ascii_lowercase).collect();
    let ext = extension(path);
    if crate::suite::is_test_path(path) { return "tests"; }
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
fn measurable(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    crate::capability::is_code(path) && !crate::suite::is_test_path(path) && !lower.split('/').any(|d| d == "node_modules") && ![".config.", ".stories."].iter().any(|m| name.contains(m))
        && !["gulpfile", "gruntfile", "webpack.", "rollup.", "jest.", "babel.", "eslint", "prettier"].iter().any(|t| name.starts_with(t))
}
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

pub fn prepare(repo: &Path, base: &str, head: &str, out: &Path) -> R<Value> {
    commit_check(repo, base)?;
    commit_check(repo, head)?;
    let (old, new) = (listing(repo, base)?, listing(repo, head)?);
    let out = fresh_dir(out)?;
    let index = |items: &[Item]| -> BTreeMap<String, Item> { items.iter().map(|i| (i.path.clone(), i.clone())).collect() };
    let (old_at, new_at) = (index(&old), index(&new));
    let names = git_with(repo, &["diff", "--no-renames", "--name-only", "-z", base, head], None, 64 << 20)?;
    let paths: Vec<String> = names.split(|b| *b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).to_string()).collect();
    let code: Vec<&String> = paths.iter().filter(|p| measurable(p)).collect();
    let mut skipped: Vec<Value> = paths.iter().filter(|p| crate::capability::other_code(p))
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
    let mut files = Vec::new();
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
        let pick = |c: &[u32], m: &BTreeMap<u32, String>| -> Vec<u32> { c.iter().filter(|l| m.contains_key(l)).copied().collect() };
        let (po, pn) = (pick(&co, &mo), pick(&cn, &mn));
        if po.is_empty() && pn.is_empty() { skipped.push(json!({ "path": p, "why": "no line of code changed (comments, imports or types only)" })); continue; }
        let status = match (old_at.contains_key(*p), new_at.contains_key(*p)) { (false, _) => "added", (_, false) => "deleted", _ => "modified" };
        files.push(json!({ "path": p, "status": status, "old": po, "new": pn }));
    }
    let gitlink = |p: &str| [&old_at, &new_at].iter().any(|at| at.get(p).is_some_and(|i| i.mode == "160000"));
    let unmeasured: Vec<Value> = paths.iter().filter(|p| !files.iter().any(|f| f["path"] == p.as_str()) && !skipped.iter().any(|s| s["path"] == p.as_str()))
        .map(|p| json!({ "path": p, "why": if gitlink(p) { "a submodule" } else { unmeasured_kind(p) } })).collect();
    let d = json!({ "version": 1, "repo": std::path::absolute(repo)?.display().to_string(), "base": base, "head": head, "files": files, "skipped": skipped, "unmeasured": unmeasured });
    write_json(out.join("drive.json"), &d)?;
    Ok(json!({ "status": "PREPARED", "files": files.len(), "skipped": skipped.len() }))
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
    let matches = |p: &str, dir: &str| {
        let re: String = p.trim_end_matches('/').split('/').map(|part| match part { "**" => ".*".to_string(), _ => part.split('*').map(regex_quote).collect::<Vec<_>>().join("[^/]*") }).collect::<Vec<_>>().join("/");
        glob_match(&re, dir)
    };
    let member = |dir: &str| patterns.iter().filter(|p| !p.starts_with('!')).any(|p| matches(p, dir))
        && !patterns.iter().filter_map(|p| p.strip_prefix('!')).any(|p| matches(p, dir));
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

fn setup_limits() -> Limits {
    let memory_mb = if crate::sandbox::throwaway() { total_mb().map_or(4096, |m| (m * 3 / 8).clamp(4096, 8192)) } else { 4096 };
    Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 1 << 20, timeout_seconds: 1800, memory_mb, pids: 1024 }
}

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

pub const BROWSER_DEPS: [&str; 11] = ["@vitest/browser", "@vitest/browser-playwright", "@vitest/browser-webdriverio", "playwright", "playwright-core", "puppeteer", "puppeteer-core",
    "jest-puppeteer", "jest-playwright-preset", "@web/test-runner-playwright", "@web/test-runner-puppeteer"];
const PLAYWRIGHT_DEFAULT: &str = "1.49.1";
const MAX_BROWSERS: usize = 4;
pub fn browser_need(manifests: &[String]) -> Option<String> {
    manifests.iter().filter_map(|m| serde_json::from_str::<Value>(m).ok()).find_map(|m| {
        ["dependencies", "devDependencies", "optionalDependencies"].iter().find_map(|s| BROWSER_DEPS.iter().find(|d| m[*s].get(**d).is_some()).map(|d| format!("the project uses {d}")))
    })
}
pub fn browser_dockerfile(base: &str, versions: &[String]) -> String {
    let install = if versions.is_empty() { format!("npx -y playwright@{PLAYWRIGHT_DEFAULT} install-deps chromium") }
        else { versions.iter().map(|v| format!("npx -y playwright@{v} install --with-deps chromium")).collect::<Vec<_>>().join(" && ") };
    format!("FROM {base}\nENV PLAYWRIGHT_BROWSERS_PATH=/ms-playwright\nRUN mkdir -p /ms-playwright && {install} && chmod -R a+rX /ms-playwright && rm -rf /var/lib/apt/lists/* /root/.npm /root/.cache\n")
}
pub fn browser_plan(sides: &[(String, Option<Vec<String>>)]) -> Vec<(String, Vec<String>)> {
    let mut plan: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (image, versions) in sides { if let Some(v) = versions { plan.entry(image.clone()).or_default().extend(v.iter().cloned()); } }
    plan.into_iter().map(|(image, set)| {
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort_by_key(|x| crate::sandbox::version_key(x));
        let newest = v.split_off(v.len().saturating_sub(MAX_BROWSERS));
        (image, newest)
    }).collect()
}
fn browsers(sides: &mut serde_json::Map<String, Value>) {
    let Some(why) = ["old", "new"].iter().find_map(|s| sides[*s]["browser"]["why"].as_str().map(str::to_string)) else { return };
    let installed = |s: &Value| -> Vec<String> { s["browser"]["installed"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|v| crate::sandbox::plain_version(v)).map(str::to_string).collect() };
    let wanted: Vec<(String, Option<Vec<String>>)> = ["old", "new"].iter().map(|s| (sides[*s]["image"].as_str().unwrap_or("").to_string(), Some(installed(&sides[*s])))).collect();
    if !crate::sandbox::throwaway() {
        for s in ["old", "new"] { sides[s]["browser"] = json!({ "why": why, "installed": installed(&sides[s]), "skipped": "browsers run only on a throwaway machine" }); }
        return;
    }
    for (base, versions) in browser_plan(&wanted) {
        let built = crate::sandbox::build_browser(&base, &browser_dockerfile, &versions);
        for s in ["old", "new"].into_iter().zip(&wanted).filter(|(_, (image, _))| *image == base).map(|(s, _)| s) {
            let mut record = json!({ "why": why, "installed": installed(&sides[s]), "playwright": versions });
            match &built { Ok(id) => sides[s]["image"] = json!(id), Err(e) => record["error"] = json!(e.0) }
            sides[s]["browser"] = record;
        }
    }
}
pub fn node_image(op: &crate::policy::Operator, wanted: Option<(u64, String)>) -> (String, Value) {
    static DEFAULT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let default = *DEFAULT.get_or_init(|| crate::sandbox::image_major(&op.image));
    match wanted {
        Some((m, from)) if default.is_some_and(|d| d != m) => match crate::sandbox::pull_node(m) {
            Ok(id) => (id, json!({ "major": m, "from": from })),
            Err(e) => (op.image.clone(), json!({ "major": default, "wanted": m, "from": from, "why": e.0 })),
        },
        Some((m, from)) => (op.image.clone(), json!({ "major": default.unwrap_or(m), "from": from })),
        None => (op.image.clone(), json!({ "major": default })),
    }
}
pub fn side_image(out: &Path, volume: &str) -> R<String> {
    let s = read_own(out.join("setup.json"))?;
    let side = ["old", "new"].into_iter().find(|side| s[*side]["volume"] == volume).ok_or_else(|| Denied(format!("Kern's setup has no record of the volume {volume}")))?;
    let image = s[side]["image"].as_str().filter(|i| crate::policy::image(i)).ok_or_else(|| Denied(format!("Kern's setup recorded no image pinned by digest for the {side} code")))?;
    Ok(image.to_string())
}
pub const SETUP_SECS_MAX: u64 = 1500;
const SETUP_SECS_MIN: u64 = 120;
const SETUP_ERROR_CHARS_MAX: usize = 300;
pub fn setup_failure(side: &str, exit_code: i64, errors: &[u8]) -> String {
    let text = String::from_utf8_lossy(errors);
    let why = crate::sandbox::first_error(&text).or_else(|| text.lines().map(str::trim).rev().find(|l| !l.is_empty()));
    let said = format!("Setting up the {side} code failed in the sandbox (exit code {exit_code})");
    why.map_or(said.clone(), |w| format!("{said}: {}", w.chars().take(SETUP_ERROR_CHARS_MAX).collect::<String>()))
}
pub fn setup_seconds(until: u64) -> R<u64> {
    let seconds = within(until, SETUP_SECS_MAX);
    ensure!(seconds >= SETUP_SECS_MIN, "Kern's time for this check ran out before it could set up the code");
    Ok(seconds)
}
fn install_side(out: &Path, op: &crate::policy::Operator, d: &Value, side: &str, label: &str, until: u64) -> R<(crate::sandbox::Volume, Value)> {
    setup_seconds(until).map_err(|_| Denied(format!("Kern's time for this check ran out before it could set up the {side} code")))?;
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let commit = d[if side == "old" { "base" } else { "head" }].as_str().unwrap_or("");
    let root = out.join("sandbox");
    std::fs::create_dir_all(&root)?;
    let code: Vec<String> = ["files", "skipped", "unmeasured"].iter().flat_map(|k| d[*k].as_array().cloned().unwrap_or_default())
        .filter(|f| f["why"] != "a submodule").filter_map(|f| f["path"].as_str().map(str::to_string)).collect();
    let tree = root.join(label);
    if tree.exists() { std::fs::remove_dir_all(&tree)?; }
    let paths = materialize(repo, commit, &tree)?;
    let read = |p: &str| std::fs::read_to_string(tree.join(p)).ok();
    let workflows: Vec<String> = paths.iter().filter(|p| (p.starts_with(".github/workflows/") || p.starts_with(".github/actions/")) && (p.ends_with(".yml") || p.ends_with(".yaml")))
        .take(100).filter_map(|p| read(p)).collect();
    let (image, node) = node_image(op, crate::nodeapi::wanted_major(&read, &workflows));
    let manifests: Vec<String> = paths.iter().filter(|p| p.rsplit('/').next() == Some("package.json") && !p.split('/').any(|s| s == "node_modules")).take(400).filter_map(|p| read(p)).collect();
    let need = browser_need(&manifests);
    let dirs = install_dirs(&paths, &code, read("package.json").as_deref(), read("pnpm-workspace.yaml").as_deref());
    let control = root.join(format!("setup-{label}"));
    let attempt = |scripts: bool| -> R<(crate::sandbox::Volume, crate::sandbox::Execution)> {
        let seconds = setup_seconds(until)?;
        let limits = Limits { timeout_seconds: seconds + 300, ..setup_limits() };
        if control.exists() { std::fs::remove_dir_all(&control)?; }
        crate::sandbox::control_dir(&control, Some(("drive-setup.json", &json!({ "version": 1, "dirs": dirs, "timeout_seconds": seconds, "scripts": scripts, "browsers": need.is_some() }))))?;
        let volume = crate::sandbox::Volume::create()?;
        let mut ledger = std::fs::OpenOptions::new().create(true).append(true).open(out.join("volumes.txt"))?;
        std::io::Write::write_all(&mut ledger, format!("{}\n", volume.0).as_bytes())?;
        let run = crate::sandbox::execute(&crate::sandbox::Spec { image: &image, snapshot: &tree, control: &control, writable: &[], limits: &limits, entry: vec!["__drive_setup".into()],
            network: "bridge", volumes: vec![], root: true, home_mb: 2048, scratch: false, workspace: Some((&volume.0, false)) })?;
        Ok((volume, run))
    };
    let scripts = crate::sandbox::throwaway();
    let (volume, run, scripts) = match attempt(scripts) {
        Err(e) if scripts && (e.0.contains("out of memory") || e.0.contains("exceeded limits")) => { let (v, r) = attempt(false)?; (v, r, false) }
        other => { let (v, r) = other?; (v, r, scripts) }
    };
    ensure!(run.exit_code == 0, "{}", setup_failure(side, run.exit_code, &run.errors));
    let record: Value = serde_json::from_slice(&run.stdout).map_err(|_| Denied(format!("The {side} code's setup gave no record")))?;
    let _ = std::fs::remove_dir_all(&tree);
    let _ = std::fs::remove_dir_all(&control);
    let browser = need.map_or(Value::Null, |why| json!({ "why": why, "installed": record["browsers"] }));
    let entry = json!({ "volume": volume.0, "installs": record["installs"], "tsconfig": paths.contains("tsconfig.json"), "image": image, "node": node, "browser": browser, "scripts": scripts });
    Ok((volume, entry))
}

const SETUP_FAILED: &str = "setup-failed.txt";
pub fn setup(out: &Path, op: &crate::policy::Operator, until: u64) -> R<Value> {
    if let Ok(why) = std::fs::read_to_string(out.join(SETUP_FAILED)) { return Err(Denied(why)); }
    let done = set_up(out, op, until);
    if let Err(e) = &done { let _ = std::fs::write(out.join(SETUP_FAILED), &e.0); }
    done
}
fn set_up(out: &Path, op: &crate::policy::Operator, until: u64) -> R<Value> {
    let d = read_own(out.join("drive.json"))?;
    if out.join("volumes.txt").exists() { clean(out)?; }
    let root = out.join("sandbox");
    if root.exists() { std::fs::remove_dir_all(&root)?; }
    std::fs::create_dir(&root)?;
    let mut sides = serde_json::Map::new();
    let mut volumes = Vec::new();
    let installed: Vec<R<(crate::sandbox::Volume, Value)>> = if total_mb().is_some_and(|m| m >= TOGETHER_MB) {
        std::thread::scope(|s| {
            let handles: Vec<_> = ["old", "new"].iter().map(|side| { let d = &d; s.spawn(move || install_side(out, op, d, side, side, until)) }).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(Denied("Setting up one side stopped unexpectedly".into())))).collect()
        })
    } else {
        let mut v = Vec::new();
        for side in ["old", "new"] { v.push(Ok(install_side(out, op, &d, side, side, until)?)); }
        v
    };
    for (side, r) in ["old", "new"].iter().zip(installed) {
        let (volume, entry) = r?;
        sides.insert((*side).into(), entry);
        volumes.push(volume);
    }
    browsers(&mut sides);
    for v in &mut volumes { v.keep(); }
    let s = json!({ "version": 1, "old": sides["old"], "new": sides["new"] });
    write_json(out.join("setup.json"), &s)?;
    Ok(s)
}

const TOGETHER_MB: u64 = 12 * 1024;
fn total_mb() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    info.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok()).map(|kb| kb / 1024)
}

pub fn clean(out: &Path) -> R<Value> {
    let s = if out.join("setup.json").exists() { read_own(out.join("setup.json"))? } else { Value::Null };
    let mut volumes: BTreeSet<String> = ["old", "new"].iter().filter_map(|side| s[side]["volume"].as_str().map(str::to_string)).collect();
    volumes.extend(std::fs::read_to_string(out.join("volumes.txt")).unwrap_or_default().lines().map(str::to_string));
    let mut removed = Vec::new();
    for v in volumes.iter().map(String::as_str).filter(|v| v.starts_with("kern-deps-") && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')) {
        let filter = format!("volume={v}");
        if let Ok(ids) = crate::sandbox::docker(&["ps", "--all", "--quiet", "--filter", &filter, "--filter", "name=kern-"], std::time::Duration::from_secs(60), 1 << 16) {
            for id in String::from_utf8_lossy(&ids).split_whitespace() { let _ = crate::sandbox::docker(&["rm", "--force", "--volumes", id], std::time::Duration::from_secs(60), 4096); }
        }
        crate::sandbox::docker(&["volume", "rm", "--force", v], std::time::Duration::from_secs(120), 4096)?;
        removed.push(v.to_string());
    }
    let _ = std::fs::remove_dir_all(out.join("sandbox"));
    for f in ["setup.json", "volumes.txt"] { if out.join(f).exists() { std::fs::remove_file(out.join(f))?; } }
    Ok(json!({ "status": "CLEANED", "removed": removed }))
}
