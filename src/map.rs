use crate::util::R;
use protobuf::Message;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const INDEXER: &str = "@sourcegraph/scip-typescript@0.4.0";
pub const INDEXER_INTEGRITY: &str = "sha512-k+AtsrqmS41Sd5qjkZlHcmvoSQIvBOonRj4jpgp0KNFM6aqvMGpdSuPUqrUcg8ENTKjUbfaUVszgQwq3bCOvwA==";
pub const INDEX_BYTES_MAX: usize = 64 << 20;
pub const HOPS_MAX: usize = 64;
pub const NODES_COUNT_MAX: usize = 200_000;
pub const MAP_SECS_MIN: u64 = 180;
pub const MAP_SECS_MAX: u64 = 480;
const DEFINITION: i32 = 1;

#[derive(Clone, Debug)]
pub struct Def { pub symbol: String, pub start: usize, pub end: usize }
#[derive(Default, Clone, Debug)]
pub struct FileFacts { pub defs: Vec<Def>, pub refs: Vec<(String, usize)> }
pub struct Map { pub files: BTreeMap<String, FileFacts>, pub defined_in: BTreeMap<String, String>, pub refs: BTreeMap<String, Vec<(String, usize)>>, pub provenance: String }
pub struct Reach { pub hops: BTreeMap<String, (usize, String)>, pub files: BTreeMap<String, usize> }

fn line(range: &[i32]) -> usize { range.first().map_or(0, |l| (*l).max(0) as usize) }
fn span(o: &scip::types::Occurrence) -> (usize, usize) {
    let r = if o.enclosing_range.is_empty() { &o.range } else { &o.enclosing_range };
    let start = line(r);
    let end = match r.len() { 4 => r[2].max(0) as usize, _ => start };
    (start, end.max(start))
}

pub fn read(bytes: &[u8]) -> R<Map> {
    crate::ensure!(bytes.len() <= INDEX_BYTES_MAX, "The code index is larger than {INDEX_BYTES_MAX} bytes");
    let index = scip::types::Index::parse_from_bytes(bytes).map_err(|e| crate::util::Denied(format!("The code index could not be read: {e}")))?;
    let tool = index.metadata.as_ref().and_then(|m| m.tool_info.as_ref()).map(|t| format!("{} {}", t.name, t.version)).unwrap_or_else(|| "unknown indexer".into());
    let mut m = Map { files: BTreeMap::new(), defined_in: BTreeMap::new(), refs: BTreeMap::new(), provenance: tool };
    for d in &index.documents {
        let path = d.relative_path.replace('\\', "/").trim_start_matches("./").to_string();
        let facts = m.files.entry(path.clone()).or_default();
        for o in &d.occurrences {
            if o.symbol.is_empty() || scip::symbol::is_local_symbol(&o.symbol) { continue; }
            if o.symbol_roles & DEFINITION != 0 {
                let (start, end) = span(o);
                facts.defs.push(Def { symbol: o.symbol.clone(), start, end });
                m.defined_in.entry(o.symbol.clone()).or_insert_with(|| path.clone());
            } else {
                let at = line(&o.range);
                facts.refs.push((o.symbol.clone(), at));
                m.refs.entry(o.symbol.clone()).or_default().push((path.clone(), at));
            }
        }
        facts.defs.sort_by_key(|d| (d.start, std::cmp::Reverse(d.end)));
    }
    Ok(m)
}

fn enclosing(facts: &FileFacts, at: usize) -> Option<&Def> {
    facts.defs.iter().filter(|d| d.start <= at && at <= d.end).min_by_key(|d| d.end - d.start)
}

pub fn reach(m: &Map, changed: &[(String, Vec<usize>)]) -> Reach {
    let mut hops: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut files: BTreeMap<String, usize> = BTreeMap::new();
    let mut queue = VecDeque::new();
    for (file, lines) in changed {
        let Some(facts) = m.files.get(file) else { continue };
        files.insert(file.clone(), 0);
        for d in facts.defs.iter().filter(|d| lines.is_empty() || lines.iter().any(|l| d.start < *l && *l <= d.end + 1)) {
            if hops.insert(d.symbol.clone(), (0, String::new())).is_none() { queue.push_back(d.symbol.clone()); }
        }
    }
    while let Some(sym) = queue.pop_front() {
        let d = hops[&sym].0;
        if d >= HOPS_MAX || hops.len() >= NODES_COUNT_MAX { break; }
        for (file, at) in m.refs.get(&sym).into_iter().flatten() {
            if m.defined_in.get(&sym).is_some_and(|f| f == file) && m.files[file].defs.iter().any(|x| x.symbol == sym && x.start <= *at && *at <= x.end) { continue; }
            let node = match m.files.get(file).and_then(|f| enclosing(f, *at)) { Some(e) => e.symbol.clone(), None => format!("file:{file}") };
            files.entry(file.clone()).and_modify(|h| *h = (*h).min(d + 1)).or_insert(d + 1);
            if !hops.contains_key(&node) { hops.insert(node.clone(), (d + 1, sym.clone())); if !node.starts_with("file:") { queue.push_back(node); } }
        }
    }
    Reach { hops, files }
}

pub fn neighbours(r: &Reach, changed: &[String], count_max: usize) -> Vec<(String, usize)> {
    let mut near: Vec<(String, usize)> = r.files.iter().filter(|(f, h)| **h > 0 && !changed.contains(f) && crate::house::product(f) && !crate::house::ui(f)).map(|(f, h)| (f.clone(), *h)).collect();
    near.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    near.truncate(count_max);
    near
}

pub fn flags(root_has: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let mut f = vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json"];
    if root_has("pnpm-lock.yaml") && root_has("pnpm-workspace.yaml") { f.push("--pnpm-workspaces"); }
    else if root_has("yarn.lock") && root_has("package.json.workspaces") { f.push("--yarn-workspaces"); }
    if !root_has("tsconfig.json") { f.push("--infer-tsconfig"); }
    f
}

pub fn files(root_has: &dyn Fn(&str) -> bool) -> Value {
    if root_has("tsconfig.json") { json!({}) } else { json!({ "tsconfig.json": "{\"compilerOptions\":{\"allowJs\":true}}" }) }
}

pub fn stopped(ending: &str, errors: &[u8], output: &[u8]) -> String {
    let last = |b: &[u8]| String::from_utf8_lossy(b).lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().chars().take(300).collect::<String>());
    format!("the indexer stopped ({ending}): {}", last(errors).or_else(|| last(output)).unwrap_or_default())
}

pub fn summary(m: &Map) -> Value {
    json!({ "provenance": m.provenance, "documents": m.files.len(), "symbols": m.defined_in.len(), "references": m.refs.values().map(Vec::len).sum::<usize>() })
}
pub fn reach_json(m: &Map, r: &Reach, changed: &[(String, Vec<usize>)]) -> Value {
    let mut symbols: Vec<(&String, &(usize, String))> = r.hops.iter().collect();
    symbols.sort_by_key(|(s, (h, _))| (*h, (*s).clone()));
    json!({ "changed": changed.iter().map(|(f, l)| json!({ "file": f, "lines": l.len() })).collect::<Vec<_>>(),
        "changed_symbols": r.hops.iter().filter(|(_, (h, _))| *h == 0).count(), "symbols_reached": r.hops.len(), "files_reached": r.files.len(),
        "farthest_hops": r.files.values().copied().max().unwrap_or(0),
        "files": r.files.iter().map(|(f, h)| json!({ "file": f, "hops": h })).collect::<Vec<_>>(),
        "symbols": symbols.iter().take(2000).map(|(s, (h, via))| json!({ "symbol": s, "hops": h, "via": via, "file": m.defined_in.get(*s) })).collect::<Vec<_>>(),
        "evidence": m.provenance, "resolution": "resolved" })
}
pub fn coverage(m: &Map, product: &[String]) -> Value {
    let indexed: BTreeSet<&String> = m.files.keys().collect();
    let missing: Vec<&String> = product.iter().filter(|p| !indexed.contains(p)).collect();
    json!({ "product_files": product.len(), "indexed": product.len() - missing.len(), "not_indexed": missing.iter().take(200).collect::<Vec<_>>(), "not_indexed_count": missing.len() })
}
