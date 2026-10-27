use crate::util::R;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, VecDeque};

pub const RUNNER: &str = include_str!("map.cjs");
pub const INDEXER: &str = "@sourcegraph/scip-typescript@0.4.0";
pub const INDEXER_INTEGRITY: &str = "sha512-k+AtsrqmS41Sd5qjkZlHcmvoSQIvBOonRj4jpgp0KNFM6aqvMGpdSuPUqrUcg8ENTKjUbfaUVszgQwq3bCOvwA==";
pub const INDEX_BYTES_MAX: usize = 64 << 20;
pub const HOPS_MAX: usize = 64;
pub const NODES_COUNT_MAX: usize = 200_000;
pub const MAP_SECS_MIN: u64 = 180;
pub const MAP_SECS_MAX: u64 = 480;
const FACTS_VERSION: u64 = 1;
const DEFINITION: u32 = 1;
const NONE: u32 = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Term, Type, Module, Other }
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Touch { Mentions, Tests, Declares, Loads, Runs, Changed }
#[derive(Clone, Copy, Debug)]
struct Def { symbol: u32, start: u32, end: u32 }
pub struct Map {
    pub symbols: Vec<String>, pub paths: Vec<String>, pub provenance: String,
    kinds: Vec<Kind>, canonical: Vec<u32>, defined_in: Vec<u32>, defs: Vec<Vec<Def>>, reach_end: Vec<Vec<u32>>, ref_start: Vec<u32>, ref_at: Vec<(u32, u32)>,
    file_ids: HashMap<String, u32>, symbol_ids: HashMap<String, u32>, bridged: usize, unbridged: usize, external: usize,
}
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hop { pub hops: usize, pub via: String }
pub struct Reach { hops: Vec<u32>, via: Vec<u32>, touched: Vec<(u32, Touch)>, pub complete: bool }
#[derive(Clone, Copy)]
pub struct Caps { pub hops_max: usize, pub nodes_count_max: usize }
pub struct Counts { pub changed_functions: usize, pub changed_types: usize, pub changed_modules: usize, pub running: usize, pub loading: usize, pub mentioning: usize, pub declaring: usize, pub tests: usize, pub farthest_hops: usize }
pub type Imported<'a> = &'a dyn Fn(&str, usize) -> Option<bool>;

pub fn kind(symbol: &str) -> Kind {
    if symbol.ends_with('.') { Kind::Term } else if symbol.ends_with('#') { Kind::Type } else if symbol.ends_with('/') { Kind::Module } else { Kind::Other }
}
fn parts(symbol: &str) -> Option<(&str, &str, &str)> {
    let descriptors = symbol.splitn(5, ' ').nth(4)?;
    let prefix = &symbol[..symbol.len() - descriptors.len()];
    let slash = descriptors.rfind("`/")?;
    let tick = descriptors[..slash].rfind('`')?;
    Some((prefix, &descriptors[tick + 1..slash], &descriptors[slash + 2..]))
}
fn built(file: &str) -> bool { file.ends_with(".d.ts") || file.ends_with(".d.mts") || file.ends_with(".d.cts") }
impl Touch {
    pub fn name(self) -> &'static str {
        match self { Touch::Mentions => "mentions", Touch::Tests => "tests", Touch::Declares => "declares", Touch::Loads => "loads", Touch::Runs => "runs", Touch::Changed => "changed" }
    }
}

struct Facts { tool: String, symbols: Vec<String>, files: Vec<(String, Vec<[u32; 4]>)> }
fn facts(bytes: &[u8]) -> R<Facts> {
    crate::ensure!(bytes.len() <= INDEX_BYTES_MAX, "The code index is larger than {INDEX_BYTES_MAX} bytes");
    let unreadable = |what: &str| crate::util::Denied(format!("The code index could not be read: {what}"));
    let text = std::str::from_utf8(bytes).map_err(|_| unreadable("not UTF-8"))?;
    let (mut files, mut footer) = (Vec::new(), None);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        crate::ensure!(footer.is_none(), "The code index could not be read: a line after the symbol table");
        let v: Value = serde_json::from_str(line).map_err(|e| unreadable(&e.to_string()))?;
        if let Some(symbols) = v["symbols"].as_array() {
            crate::ensure!(v["version"] == FACTS_VERSION, "The code index could not be read: it is not version {FACTS_VERSION}");
            let tool = v["tool"].as_str().ok_or_else(|| unreadable("no indexer named"))?.to_string();
            let symbols = symbols.iter().map(|s| s.as_str().map(str::to_string).ok_or_else(|| unreadable("a symbol that is not text"))).collect::<R<Vec<_>>>()?;
            footer = Some((tool, symbols));
            continue;
        }
        let path = v["path"].as_str().ok_or_else(|| unreadable("a file without a path"))?.replace('\\', "/").trim_start_matches("./").to_string();
        let occ = v["occ"].as_array().ok_or_else(|| unreadable("a file without occurrences"))?.iter().map(|o| {
            let n: Vec<u32> = o.as_array().into_iter().flatten().map(|x| x.as_u64().and_then(|n| u32::try_from(n).ok())).collect::<Option<_>>().ok_or_else(|| unreadable("an occurrence that is not numbers"))?;
            <[u32; 4]>::try_from(n).map_err(|_| unreadable("an occurrence that is not four numbers"))
        }).collect::<R<Vec<_>>>()?;
        files.push((path, occ));
    }
    let (tool, symbols) = footer.ok_or_else(|| unreadable("no symbol table"))?;
    let past = files.iter().flat_map(|(_, occ)| occ).any(|o| o[0] as usize >= symbols.len());
    crate::ensure!(!past, "The code index could not be read: a symbol number past the table");
    Ok(Facts { tool, symbols, files })
}

pub fn read(bytes: &[u8]) -> R<Map> {
    let f = facts(bytes)?;
    let count = f.symbols.len();
    let kinds: Vec<Kind> = f.symbols.iter().map(|s| kind(s)).collect();
    let mut m = Map { symbols: f.symbols, paths: Vec::new(), provenance: f.tool, kinds, canonical: (0..count as u32).collect(), defined_in: vec![NONE; count], defs: Vec::new(), reach_end: Vec::new(),
        ref_start: Vec::new(), ref_at: Vec::new(), file_ids: HashMap::new(), symbol_ids: HashMap::new(), bridged: 0, unbridged: 0, external: 0 };
    let mut refs: Vec<(u32, u32, u32)> = Vec::new();
    for (path, occ) in f.files {
        if m.file_ids.contains_key(&path) { continue; }
        let file = m.paths.len() as u32;
        m.paths.push(path.clone());
        m.defs.push(Vec::new());
        m.file_ids.insert(path, file);
        for [symbol, roles, start, end] in occ {
            if m.symbols[symbol as usize].is_empty() || m.symbols[symbol as usize].starts_with("local ") { continue; }
            if roles & DEFINITION != 0 {
                m.defs[file as usize].push(Def { symbol, start, end: end.max(start) });
                if m.defined_in[symbol as usize] == NONE { m.defined_in[symbol as usize] = file; }
            } else { refs.push((symbol, file, start)); }
        }
    }
    m.symbol_ids = m.symbols.iter().enumerate().map(|(i, s)| (s.clone(), i as u32)).collect();
    m.bridge();
    for defs in &mut m.defs {
        defs.sort_by_key(|d| (d.start, std::cmp::Reverse(d.end)));
        let mut ends: Vec<u32> = Vec::with_capacity(defs.len());
        for d in defs.iter() { ends.push(ends.last().map_or(d.end, |e| (*e).max(d.end))); }
        m.reach_end.push(ends);
    }
    refs.sort_by_key(|(s, file, at)| (m.canonical[*s as usize], *file, *at));
    m.ref_start = vec![0; count + 1];
    for (s, _, _) in &refs { m.ref_start[m.canonical[*s as usize] as usize + 1] += 1; }
    for i in 0..count { m.ref_start[i + 1] += m.ref_start[i]; }
    m.ref_at = refs.into_iter().map(|(_, file, at)| (file, at)).collect();
    Ok(m)
}

impl Map {
    fn bridge(&mut self) {
        let mut sources: HashMap<(&str, &str), Vec<u32>> = HashMap::new();
        let mut packages: BTreeSet<&str> = BTreeSet::new();
        for (i, s) in self.symbols.iter().enumerate().filter(|(i, _)| self.defined_in[*i] != NONE) {
            if let Some((prefix, _, tail)) = parts(s).filter(|(_, file, _)| !built(file)) { sources.entry((prefix, tail)).or_default().push(i as u32); packages.insert(prefix); }
        }
        let mut links = Vec::new();
        for (i, s) in self.symbols.iter().enumerate().filter(|(i, _)| self.defined_in[*i] == NONE) {
            let Some((prefix, _, tail)) = parts(s).filter(|(_, file, _)| built(file)) else { continue };
            if !packages.contains(prefix) { self.external += 1; continue; }
            match sources.get(&(prefix, tail)).map(Vec::as_slice) { Some([one]) => links.push((i as u32, *one)), _ => self.unbridged += 1 }
        }
        self.bridged = links.len();
        for (alias, target) in links { self.canonical[alias as usize] = target; }
    }
    pub fn file(&self, path: &str) -> Option<u32> { self.file_ids.get(path).copied() }
    pub fn symbol(&self, name: &str) -> Option<u32> { self.symbol_ids.get(name).copied() }
    pub fn defined_in(&self, name: &str) -> Option<&str> { self.symbol(name).map(|s| self.defined_in[s as usize]).filter(|f| *f != NONE).map(|f| self.paths[f as usize].as_str()) }
    pub fn canonical(&self, name: &str) -> Option<&str> { let s = self.symbol(name)?; let c = self.canonical[s as usize]; (c != s).then(|| self.symbols[c as usize].as_str()) }
    pub fn definitions(&self) -> usize { self.defined_in.iter().filter(|f| **f != NONE).count() }
    pub fn references(&self) -> usize { self.ref_at.len() }
    fn uses(&self, symbol: u32) -> &[(u32, u32)] { &self.ref_at[self.ref_start[symbol as usize] as usize..self.ref_start[symbol as usize + 1] as usize] }
    fn enclosing(&self, file: u32, at: u32) -> Option<&Def> {
        let (defs, ends) = (&self.defs[file as usize], &self.reach_end[file as usize]);
        let mut i = defs.partition_point(|d| d.start <= at);
        while i > 0 {
            i -= 1;
            if ends[i] < at { return None; }
            if defs[i].end >= at { return Some(&defs[i]); }
        }
        None
    }
    fn touch(&self, edge: Kind, file: u32, at: u32, imported: Imported) -> (Touch, Option<u32>) {
        let path = &self.paths[file as usize];
        if crate::suite::is_test_path(path) { return (Touch::Tests, None); }
        if edge == Kind::Type { return (Touch::Declares, None); }
        let enclosing = self.enclosing(file, at);
        if let Some(d) = enclosing.filter(|d| self.kinds[d.symbol as usize] != Kind::Module) { return (Touch::Runs, Some(d.symbol)); }
        let module = enclosing.map(|d| d.symbol);
        match imported(path, at as usize) {
            Some(false) => (Touch::Loads, module),
            Some(true) if edge == Kind::Module => (Touch::Loads, module),
            _ => (Touch::Mentions, None),
        }
    }
    fn seeds(&self, file: u32, lines: &[usize]) -> Vec<u32> {
        let defs = &self.defs[file as usize];
        let named = |d: &Def| self.kinds[d.symbol as usize] != Kind::Module;
        let inside = |d: &Def, l: &usize| (d.start as usize) < *l && *l <= d.end as usize + 1;
        let module_level = lines.is_empty() || lines.iter().any(|l| !defs.iter().any(|d| named(d) && inside(d, l)));
        defs.iter().filter(|d| if named(d) { lines.is_empty() || lines.iter().any(|l| inside(d, l)) } else { module_level }).map(|d| d.symbol).collect()
    }
    fn own_definition(&self, symbol: u32, file: u32, at: u32) -> bool {
        self.defined_in[symbol as usize] == file && self.defs[file as usize].iter().any(|x| x.symbol == symbol && x.start <= at && at <= x.end)
    }
}

pub fn reach(m: &Map, changed: &[(String, Vec<usize>)], imported: Imported) -> Reach { reach_within(m, changed, imported, Caps { hops_max: HOPS_MAX, nodes_count_max: NODES_COUNT_MAX }) }
pub fn reach_within(m: &Map, changed: &[(String, Vec<usize>)], imported: Imported, caps: Caps) -> Reach {
    let mut r = Reach { hops: vec![NONE; m.symbols.len()], via: vec![NONE; m.symbols.len()], touched: vec![(NONE, Touch::Mentions); m.paths.len()], complete: true };
    let (mut queue, mut nodes) = (VecDeque::new(), 0usize);
    for (path, lines) in changed {
        let Some(file) = m.file(path) else { continue };
        r.touched[file as usize] = (0, Touch::Changed);
        for symbol in m.seeds(file, lines) {
            if r.hops[symbol as usize] == NONE { r.hops[symbol as usize] = 0; nodes += 1; queue.push_back(symbol); }
        }
    }
    'walk: while let Some(symbol) = queue.pop_front() {
        let (d, edge) = (r.hops[symbol as usize], m.kinds[symbol as usize]);
        if edge == Kind::Other { continue; }
        if d as usize >= caps.hops_max { if !m.uses(symbol).is_empty() { r.complete = false; } continue; }
        for &(file, at) in m.uses(symbol) {
            if m.own_definition(symbol, file, at) { continue; }
            let (how, node) = m.touch(edge, file, at, imported);
            let t = &mut r.touched[file as usize];
            *t = (t.0.min(d + 1), t.1.max(how));
            let Some(node) = node.filter(|n| r.hops[*n as usize] == NONE) else { continue };
            if nodes >= caps.nodes_count_max { r.complete = false; break 'walk; }
            r.hops[node as usize] = d + 1;
            r.via[node as usize] = symbol;
            nodes += 1;
            queue.push_back(node);
        }
    }
    r
}

impl Reach {
    fn hop_of(&self, m: &Map, s: usize) -> Option<Hop> {
        (self.hops[s] != NONE).then(|| Hop { hops: self.hops[s] as usize, via: if self.via[s] == NONE { String::new() } else { m.symbols[self.via[s] as usize].clone() } })
    }
    pub fn hop(&self, m: &Map, name: &str) -> Option<Hop> { self.hop_of(m, m.symbol(name)? as usize) }
    pub fn touch(&self, m: &Map, path: &str) -> Option<(usize, Touch)> {
        let (h, t) = self.touched[m.file(path)? as usize];
        (h != NONE).then_some((h as usize, t))
    }
    pub fn files<'a>(&self, m: &'a Map) -> Vec<(&'a str, usize, Touch)> {
        let mut out: Vec<(&str, usize, Touch)> = self.touched.iter().enumerate().filter(|(_, (h, _))| *h != NONE).map(|(f, (h, t))| (m.paths[f].as_str(), *h as usize, *t)).collect();
        out.sort();
        out
    }
    pub fn symbols<'a>(&self, m: &'a Map) -> Vec<(&'a str, Hop)> {
        let mut out: Vec<(&str, Hop)> = (0..self.hops.len()).filter_map(|s| self.hop_of(m, s).map(|h| (m.symbols[s].as_str(), h))).collect();
        out.sort_by(|a, b| a.1.hops.cmp(&b.1.hops).then_with(|| a.0.cmp(b.0)));
        out
    }
}

pub fn counts(m: &Map, r: &Reach) -> Counts {
    let how = |t: Touch| r.touched.iter().filter(|(h, x)| *h != NONE && *x == t).count();
    let seeds = |k: Kind| r.hops.iter().enumerate().filter(|(s, h)| **h == 0 && m.kinds[*s] == k).count();
    Counts { changed_functions: seeds(Kind::Term), changed_types: seeds(Kind::Type), changed_modules: seeds(Kind::Module), running: how(Touch::Runs), loading: how(Touch::Loads), mentioning: how(Touch::Mentions), declaring: how(Touch::Declares), tests: how(Touch::Tests),
        farthest_hops: r.touched.iter().filter(|(h, _)| *h != NONE).map(|(h, _)| *h as usize).max().unwrap_or(0) }
}
pub fn neighbours(m: &Map, r: &Reach, changed: &[String], count_max: usize) -> Vec<(String, usize)> {
    let mut near: Vec<(String, usize)> = r.files(m).into_iter().filter(|(f, _, t)| matches!(t, Touch::Runs | Touch::Loads) && !changed.iter().any(|c| c == f) && crate::house::product(f) && !crate::house::ui(f)).map(|(f, h, _)| (f.to_string(), h)).collect();
    near.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    near.truncate(count_max);
    near
}
pub fn uncovered(m: &Map, changed: &[String]) -> Vec<String> {
    changed.iter().filter(|f| crate::house::product(f) && m.file(f).is_none()).cloned().collect()
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
    json!({ "provenance": m.provenance, "documents": m.paths.len(), "symbols": m.definitions(), "references": m.references(), "bridged": m.bridged, "unbridged": m.unbridged, "external": m.external })
}
pub fn report(m: &Map, r: &Reach, rooms: Vec<Value>, not_indexed: usize) -> Value {
    let c = counts(m, r);
    let mut out = summary(m);
    for (k, v) in [("changed_functions", c.changed_functions), ("changed_types", c.changed_types), ("changed_modules", c.changed_modules), ("files_running", c.running), ("files_loading", c.loading), ("files_mentioning", c.mentioning),
        ("files_declaring", c.declaring), ("test_files", c.tests), ("farthest_hops", c.farthest_hops), ("rooms_reached", rooms.len()), ("not_indexed", not_indexed)] { out[k] = json!(v); }
    out["status"] = json!("RAN");
    out["complete"] = json!(r.complete);
    out["nearest"] = json!(neighbours(m, r, &[], 1).first().map(|(f, _)| f.clone()));
    out["rooms"] = json!(rooms.into_iter().take(12).collect::<Vec<_>>());
    out
}
pub fn reach_json(m: &Map, r: &Reach, changed: &[(String, Vec<usize>)]) -> Value {
    let c = counts(m, r);
    json!({ "changed": changed.iter().map(|(f, l)| json!({ "file": f, "lines": l.len() })).collect::<Vec<_>>(),
        "changed_functions": c.changed_functions, "changed_types": c.changed_types, "changed_modules": c.changed_modules, "symbols_reached": r.symbols(m).len(), "files_running": c.running, "files_loading": c.loading, "files_mentioning": c.mentioning,
        "files_declaring": c.declaring, "test_files": c.tests, "farthest_hops": c.farthest_hops, "complete": r.complete,
        "files": r.files(m).iter().map(|(f, h, t)| json!({ "file": f, "hops": h, "how": t.name() })).collect::<Vec<_>>(),
        "symbols": r.symbols(m).iter().take(2000).map(|(s, h)| json!({ "symbol": s, "hops": h.hops, "via": h.via, "file": m.defined_in(s) })).collect::<Vec<_>>(),
        "evidence": m.provenance })
}
pub fn coverage(m: &Map, product: &[String]) -> Value {
    let indexed: BTreeSet<&String> = m.paths.iter().collect();
    let missing: Vec<&String> = product.iter().filter(|p| !indexed.contains(p)).collect();
    json!({ "product_files": product.len(), "indexed": product.len() - missing.len(), "not_indexed": missing.iter().take(200).collect::<Vec<_>>(), "not_indexed_count": missing.len() })
}
