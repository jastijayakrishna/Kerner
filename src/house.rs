use crate::util::R;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

pub const FILE_BYTES_MAX: u64 = 1 << 20;
pub const HOPS_MAX: usize = 64;
pub const HOUSE_FILES_COUNT_MAX: usize = 60_000;
const ROOM_FOLDERS: [&str; 6] = ["/routes/", "/route/", "/api/", "/pages/", "/controllers/", "/handlers/"];
const ROOM_NAMES: [&str; 8] = ["route", "page", "server", "main", "cli", "app", "handler", "controller"];
const ALIAS_MARKS: [&str; 6] = ["@@/", "~~/", "@/", "~/", "#/", "$/"];
const NOT_CODE: [&str; 8] = ["css", "scss", "svg", "png", "json", "md", "astro", "yaml"];

pub struct House { pub exports: BTreeMap<String, BTreeSet<String>>, pub imports: BTreeMap<String, BTreeSet<String>>, pub rooms: BTreeMap<String, String>, pub unresolved: BTreeMap<&'static str, usize>, pub files: usize }
pub struct Reach { pub hops: BTreeMap<String, (usize, String)>, pub rooms: Vec<(String, String, usize)> }

fn stem(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p).split('.').next().unwrap_or("") }
fn joined(from: &str, spec: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for s in spec.split('/') { match s { "." | "" => {}, ".." => { parts.pop()?; }, s => parts.push(s) } }
    Some(parts.join("/"))
}
fn ui_target(from: &str, spec: &str, exists: &dyn Fn(&str) -> bool) -> Option<String> {
    let base = joined(from, spec)?;
    [base.clone(), format!("{base}.vue"), format!("{base}.svelte"), format!("{base}/index.vue")].into_iter().find(|c| ui(c) && exists(c))
}
pub fn ui(p: &str) -> bool { p.ends_with(".vue") || p.ends_with(".svelte") || p.ends_with(".astro") }
pub fn product(p: &str) -> bool { (crate::capability::is_code(p) || ui(p)) && !crate::suite::is_test_path(p) && crate::suite::product_code(p) && !p.split('/').any(|c| c == "node_modules" || c == "dist" || c == "build") }
fn script(p: &str, s: &str) -> (String, String) {
    if !ui(p) { return (p.to_string(), s.to_string()); }
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("<script") {
        let Some(open) = rest[start..].find('>') else { break };
        let body = &rest[start + open + 1..];
        let end = body.find("</script>").unwrap_or(body.len());
        out.push_str(&body[..end]);
        out.push('\n');
        rest = &body[end..];
    }
    (format!("{p}.ts"), out)
}
pub fn specs(p: &str, s: &str) -> Vec<String> {
    let (name, text) = script(p, s);
    let mut out: Vec<String> = crate::calls::imports(&name, &text).into_iter().map(|i| i.spec).collect();
    out.extend(crate::proof::parsed(&name, &text, |program, _| program.body.iter().filter_map(|st| match st {
        oxc_ast::ast::Statement::ExportAllDeclaration(d) => Some(d.source.value.to_string()),
        oxc_ast::ast::Statement::ExportFromDeclaration(d) => Some(d.source.value.to_string()),
        _ => None,
    }).collect::<Vec<String>>()).unwrap_or_default());
    out
}
fn candidates(dir: &str, sub: &str) -> Vec<String> {
    let base = if sub.is_empty() { dir.to_string() } else { format!("{dir}/{sub}") };
    let mut out = Vec::new();
    for ext in ["", ".ts", ".tsx", ".mts", ".js", ".mjs", ".jsx", ".cjs"] { out.push(format!("{base}{ext}")); }
    for ext in ["ts", "tsx", "js", "mjs"] { out.push(format!("{base}/index.{ext}")); out.push(format!("{base}/src/index.{ext}")); }
    if !sub.is_empty() { for ext in ["ts", "tsx", "js"] { out.push(format!("{dir}/src/{sub}.{ext}")); out.push(format!("{dir}/src/{sub}/index.{ext}")); } }
    out
}
fn entry_files(dir: &str, manifest: &Value, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = crate::surface::entries(manifest).iter().filter_map(|e| {
        let e = e.trim_start_matches("./");
        let p = if dir.is_empty() { e.to_string() } else { format!("{dir}/{e}") };
        let src = p.replace("/dist/", "/src/").replace(".d.ts", ".ts");
        [p.clone(), src.clone(), src.replace(".js", ".ts"), src.replace(".mjs", ".ts"), src.replace(".cjs", ".ts")].into_iter().find(|c| exists(c) && product(c))
    }).collect();
    if out.is_empty() { out.extend(candidates(dir, "").into_iter().filter(|c| exists(c) && product(c)).take(1)); }
    out.sort();
    out.dedup();
    out
}
fn target_of(p: &str, spec: &str, packages: &BTreeMap<String, (String, Value)>, home: &str, exists: &dyn Fn(&str) -> bool) -> Result<Vec<String>, &'static str> {
    if let Some(t) = crate::proof::resolve(p, spec, exists).or_else(|| ui_target(p, spec, exists)) { return Ok(if product(&t) { vec![t] } else { Vec::new() }); }
    if spec.starts_with('.') { return if spec.rsplit('.').next().is_some_and(|e| NOT_CODE.contains(&e)) { Ok(Vec::new()) } else { Err("relative") }; }
    if let Some(rest) = ALIAS_MARKS.iter().find_map(|m| spec.strip_prefix(m)) {
        let found = candidates(&format!("{home}/src"), rest).into_iter().chain(candidates(home, rest)).chain([format!("{home}/src/{rest}.vue"), format!("{home}/{rest}.vue")]).find(|c| exists(c) && product(c));
        return found.map(|t| vec![t]).ok_or("alias");
    }
    let Some(pkg) = packages.keys().filter(|n| spec == n.as_str() || spec.starts_with(&format!("{n}/"))).max_by_key(|n| n.len()) else { return Err("npm") };
    let sub = spec[pkg.len()..].trim_start_matches('/');
    let (dir, m) = &packages[pkg];
    let found: Vec<String> = if sub.is_empty() { entry_files(dir, m, exists) } else { candidates(dir, sub).into_iter().filter(|c| exists(c) && product(c)).take(1).collect() };
    if found.is_empty() { Err("workspace") } else { Ok(found) }
}

pub fn build(repo: &Path, commit: &str) -> R<House> {
    let items = crate::drive::listing(repo, commit)?;
    let all: BTreeSet<String> = items.iter().map(|i| i.path.clone()).collect();
    let exists = |p: &str| all.contains(p);
    let wanted: Vec<&crate::drive::Item> = items.iter().filter(|i| i.size <= FILE_BYTES_MAX && (product(&i.path) || i.path.rsplit('/').next() == Some("package.json"))).collect();
    crate::ensure!(wanted.len() <= HOUSE_FILES_COUNT_MAX, "The repository has more than {HOUSE_FILES_COUNT_MAX} product files, more than Kern maps in one check");
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    crate::drive::blobs(repo, &wanted, |i, b| { sources.insert(i.path.clone(), String::from_utf8_lossy(b).to_string()); Ok(()) })?;
    let mut packages: BTreeMap<String, (String, Value)> = BTreeMap::new();
    for (p, s) in &sources {
        if p.rsplit('/').next() != Some("package.json") || p.contains("node_modules") { continue; }
        let Ok(m) = serde_json::from_str::<Value>(s) else { continue };
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d).to_string();
        if let Some(name) = m["name"].as_str() { packages.insert(name.to_string(), (dir, m)); }
    }
    let mut h = House { exports: BTreeMap::new(), imports: BTreeMap::new(), rooms: BTreeMap::new(), unresolved: BTreeMap::new(), files: all.len() };
    for (name, (dir, m)) in &packages {
        for e in entry_files(dir, m, &exists) { h.rooms.insert(e, format!("package {name}")); }
        for (b, v) in m["bin"].as_object().into_iter().flatten() {
            let Some(v) = v.as_str() else { continue };
            for c in [format!("{dir}/{}", v.trim_start_matches("./")), format!("{dir}/src/{}", v.trim_start_matches("./").trim_start_matches("dist/")).replace(".js", ".ts")] { if exists(&c) { h.rooms.insert(c, format!("command {b}")); } }
        }
    }
    let package_dirs: BTreeSet<String> = packages.values().map(|(d, _)| d.clone()).collect();
    let home = |p: &str| package_dirs.iter().filter(|d| d.is_empty() || p.starts_with(&format!("{d}/"))).max_by_key(|d| d.len()).cloned().unwrap_or_default();
    for (p, s) in &sources {
        if !product(p) { continue; }
        h.exports.insert(p.clone(), if ui(p) { BTreeSet::new() } else { crate::trace::callable(p, s).unwrap_or_default() });
        let lower = p.to_ascii_lowercase();
        if ROOM_FOLDERS.iter().any(|f| lower.contains(f)) || ROOM_NAMES.contains(&stem(&lower)) || (ui(p) && lower.contains("/pages/")) { h.rooms.entry(p.clone()).or_insert_with(|| "route or entry".to_string()); }
        let mut targets = BTreeSet::new();
        let at = home(p);
        for spec in specs(p, s) {
            match target_of(p, &spec, &packages, &at, &exists) { Ok(t) => targets.extend(t), Err(kind) => *h.unresolved.entry(kind).or_default() += 1 }
        }
        h.imports.insert(p.clone(), targets);
    }
    Ok(h)
}

pub fn reach(h: &House, changed: &[String]) -> Reach {
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, tos) in &h.imports { for to in tos { dependents.entry(to).or_default().push(from); } }
    let mut hops: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for c in changed { if h.exports.contains_key(c) { hops.insert(c.clone(), (0, String::new())); queue.push_back(c.as_str()); } }
    while let Some(f) = queue.pop_front() {
        let d = hops[f].0;
        if d >= HOPS_MAX { continue; }
        for &dep in dependents.get(f).into_iter().flatten() { if !hops.contains_key(dep) { hops.insert(dep.to_string(), (d + 1, f.to_string())); queue.push_back(dep); } }
    }
    let rooms = hops.iter().filter_map(|(f, (d, _))| h.rooms.get(f).map(|k| (f.clone(), k.clone(), *d))).collect();
    Reach { hops, rooms }
}
impl Reach {
    pub fn path(&self, room: &str) -> Vec<String> {
        let mut p = vec![room.to_string()];
        let mut f = room.to_string();
        while let Some((d, via)) = self.hops.get(&f) { if *d == 0 { break; } p.push(via.clone()); f = via.clone(); }
        p
    }
    pub fn json(&self, changed: &[String], h: &House) -> Value {
        json!({ "changed": changed, "affected_files": self.hops.len(), "farthest_hops": self.hops.values().map(|x| x.0).max().unwrap_or(0), "rooms_reached": self.rooms.len(),
            "rooms": self.rooms.iter().map(|(f, k, d)| json!({ "room": f, "kind": k, "hops": d, "path": self.path(f) })).collect::<Vec<_>>(),
            "affected": self.hops.iter().map(|(f, (d, _))| json!({ "file": f, "hops": d, "exports": h.exports.get(f).map_or(0, BTreeSet::len) })).collect::<Vec<_>>() })
    }
}
pub const NEIGHBOURS_COUNT_MAX: usize = 40;
pub fn neighbours(h: &House, changed: &[String], count_max: usize) -> Vec<(String, usize)> {
    let r = reach(h, changed);
    let mut near: Vec<(String, usize)> = r.hops.iter().filter(|(f, (d, _))| *d > 0 && !ui(f) && !changed.contains(f)).map(|(f, (d, _))| (f.clone(), *d)).collect();
    near.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    near.truncate(count_max);
    near
}
pub fn summary(h: &House) -> Value {
    json!({ "files_in_repo": h.files, "product_files": h.exports.len(), "exported_functions": h.exports.values().map(BTreeSet::len).sum::<usize>(),
        "import_edges": h.imports.values().map(BTreeSet::len).sum::<usize>(), "unresolved_imports": h.unresolved, "rooms": h.rooms.len() })
}
