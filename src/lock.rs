use crate::types::jsonc;
use crate::util::b64;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::iter::{Enumerate, Peekable};
use std::str::Split;

pub const LOCKFILES: [&str; 6] = ["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lock", "bun.lockb"];
pub const LOCKFILE_BYTES_MAX: u64 = 64 << 20;
pub const LOCKFILES_BYTES_MAX: u64 = 128 << 20;
const YAML_DEPTH_COUNT_MAX: usize = 16;
const JSON_DEPTH_COUNT_MAX: usize = 64;
const NPM_DEPTH_COUNT_MAX: usize = 64;
const SHOWN_CHARS_MAX: usize = 120;
const NAME_BYTES_MAX: usize = 214;
const VERSION_BYTES_MAX: usize = 256;
const URL_BYTES_MAX: usize = 4096;
const JS_SAFE_INTEGER_MAX: i64 = (1 << 53) - 1;
const SECTIONS: [&str; 5] = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies", "requires"];
const SNAPSHOT_FIELDS: [&str; 4] = ["dependencies", "optionalDependencies", "transitivePeerDependencies", "optional"];
const INDICATORS: [char; 19] =['-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`'];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source { Registry, Url(String) }

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package { pub name: String, pub version: String, pub source: Source, pub integrity: Option<String> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install { pub name: String, pub package: Package }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock { pub installs: BTreeMap<String, Install>, pub aliases: BTreeSet<(String, String)> }

impl Lock {
    pub fn packages(&self) -> BTreeSet<&Package> { self.installs.values().map(|i| &i.package).collect() }
    fn add(&mut self, place: &str, name: &str, package: Package) -> Result<(), String> {
        let install = Install { name: name.to_string(), package };
        if self.installs.get(place).is_some_and(|had| *had != install) { return Err(format!("it lists {} twice", cut(place))); }
        self.installs.insert(place.to_string(), install);
        Ok(())
    }
}

fn base(path: &str) -> &str { path.rsplit('/').next().unwrap_or(path) }
pub fn lockfile(path: &str) -> bool { LOCKFILES.contains(&base(path)) }
pub(crate) fn text_lockfile(path: &str) -> bool { lockfile(path) && base(path) != "bun.lockb" }

pub fn read(path: &str, bytes: Option<&[u8]>) -> Result<Lock, String> {
    if base(path) == "bun.lockb" { return Err("it is in Bun's binary format".into()); }
    let bytes = bytes.filter(|b| b.len() as u64 <= LOCKFILE_BYTES_MAX).ok_or_else(|| format!("it is larger than {} MiB, past the {} MiB of lockfiles Kern reads in one check, or not a regular file", LOCKFILE_BYTES_MAX >> 20, LOCKFILES_BYTES_MAX >> 20))?;
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match base(path) {
        "pnpm-lock.yaml" => pnpm(&yaml(text, Schema::Core).map_err(|n| form(n, "pnpm"))?),
        "yarn.lock" if classic(text) => yarn(text),
        "yarn.lock" => berry(&yaml(text, Schema::Failsafe).map_err(|n| form(n, "yarn"))?),
        "bun.lock" => bun(text),
        _ => npm(text),
    }
}
fn form(line: usize, tool: &str) -> String { format!("line {line} is not in the form {tool} writes") }
fn entry(key: &str, tool: &str) -> String { format!("its entry {} is not in the form {tool} writes", cut(key)) }
fn cut(s: &str) -> String { s.chars().take(SHOWN_CHARS_MAX).collect() }
fn stated(v: &Value) -> String { match v { Value::String(s) => cut(s), Value::Null => "missing".into(), v => cut(&v.to_string()) } }

fn split_name(s: &str) -> Option<(&str, &str)> {
    let at = s.get(1..)?.find('@')? + 1;
    Some((&s[..at], &s[at + 1..]))
}
pub(crate) fn named(s: &str) -> bool {
    let part = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~!'()*".contains(&b));
    s.len() <= NAME_BYTES_MAX && match s.strip_prefix('@') { Some(rest) => rest.split_once('/').is_some_and(|(scope, name)| part(scope) && part(name)), None => part(s) }
}
fn versioned(v: &str) -> bool { v.len() <= VERSION_BYTES_MAX && v.starts_with(|c: char| c.is_ascii_digit()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b".+-_".contains(&b)) }
fn tarball_file(spec: &str) -> bool { spec.strip_prefix("file:").is_some_and(|p| [".tgz", ".tar.gz", ".tar"].iter().any(|e| p.ends_with(e))) }
fn spotless(s: &str) -> bool { !s.is_empty() && s.len() <= URL_BYTES_MAX && !s.chars().any(|c| c.is_whitespace() || c.is_control()) }
fn sri(s: &str) -> Option<String> {
    let token = |t: &str| t.split_once('-').is_some_and(|(alg, digest)| !alg.is_empty() && alg.bytes().all(|b| b.is_ascii_alphanumeric())
        && !digest.is_empty() && digest.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=?._~-".contains(&b)));
    let tokens: BTreeSet<&str> = s.split_whitespace().collect();
    (!tokens.is_empty() && tokens.iter().all(|t| token(t))).then(|| tokens.into_iter().collect::<Vec<_>>().join(" "))
}
fn package(name: &str, version: &str, source: Source, integrity: Option<String>) -> Option<Package> {
    let found = match &source { Source::Url(url) => spotless(url) && (version.is_empty() || versioned(version)), Source::Registry => versioned(version) };
    (named(name) && found).then(|| Package { name: name.to_string(), version: version.to_string(), source, integrity })
}
fn npm_target(range: &str) -> Option<&str> {
    let rest = range.strip_prefix("npm:")?;
    let name = split_name(rest).map_or(rest, |(n, _)| n);
    (named(name) && !versioned(name)).then_some(name)
}
fn aliases(e: &Value) -> Vec<(String, String)> {
    SECTIONS.iter().flat_map(|s| e[*s].as_object().into_iter().flatten())
        .filter_map(|(dep, spec)| Some((dep.clone(), npm_target(spec.as_str()?)?.to_string()))).collect()
}
fn text_of<'a>(v: &'a Value, key: &str, bad: &dyn Fn() -> String) -> Result<Option<&'a str>, String> {
    match &v[key] { Value::Null => Ok(None), Value::String(s) => Ok(Some(s.as_str())), _ => Err(bad()) }
}
fn rows<'a>(v: &'a Value, bad: &dyn Fn() -> String) -> Result<Vec<(&'a String, &'a Value)>, String> {
    match v { Value::Null => Ok(Vec::new()), Value::Object(m) => Ok(m.iter().collect()), _ => Err(bad()) }
}

fn npm(text: &str) -> Result<Lock, String> {
    let v: Value = serde_json::from_str(text).map_err(|_| "it is not valid JSON".to_string())?;
    let packaged = v["packages"].as_object().is_some_and(|p| !p.is_empty());
    let mut legacy = Vec::new();
    if !packaged { nested("", &v["dependencies"], &mut legacy, 0)?; }
    let found: Vec<(&str, &Value)> = if packaged { v["packages"].as_object().into_iter().flatten().map(|(k, e)| (k.as_str(), e)).collect() } else { legacy.iter().map(|(k, e)| (k.as_str(), e)).collect() };
    let mut lock = Lock::default();
    for (key, e) in found {
        lock.aliases.extend(aliases(e));
        if key.is_empty() || e["link"] == true || (packaged && !key.contains("node_modules/")) { continue; }
        let bad = || entry(key, "npm");
        let name = crate::deps::package_of(key);
        let integrity = text_of(e, "integrity", &bad)?.map(|i| sri(i).ok_or_else(bad)).transpose()?;
        let stated = text_of(e, "version", &bad)?.unwrap_or("");
        let (real, version) = match stated.strip_prefix("npm:").and_then(split_name) {
            Some((real, version)) => { lock.aliases.insert((name.to_string(), real.to_string())); (real, version) }
            None => (text_of(e, "name", &bad)?.unwrap_or(name), stated),
        };
        let source = match text_of(e, "resolved", &bad)? {
            Some(url) => Source::Url(url.to_string()),
            None if version.is_empty() || (version.starts_with("file:") && !tarball_file(version)) => continue,
            None if version.contains(':') => Source::Url(version.to_string()),
            None => Source::Registry,
        };
        let version = if version.contains(':') { "" } else { version };
        lock.add(key, name, package(real, version, source, integrity).ok_or_else(bad)?)?;
    }
    Ok(lock)
}
fn nested(prefix: &str, deps: &Value, found: &mut Vec<(String, Value)>, depth: usize) -> Result<(), String> {
    if depth > NPM_DEPTH_COUNT_MAX { return Err(format!("its packages nest more than {NPM_DEPTH_COUNT_MAX} levels deep")); }
    for (name, e) in deps.as_object().into_iter().flatten() {
        let key = format!("{prefix}node_modules/{name}");
        let mut own = e.clone();
        if let Some(o) = own.as_object_mut() { o.remove("dependencies"); }
        found.push((key.clone(), own));
        nested(&format!("{key}/"), &e["dependencies"], found, depth + 1)?;
    }
    Ok(())
}

fn pnpm_id(reference: &str) -> &str {
    let id = reference.strip_prefix('/').unwrap_or(reference);
    id.split('(').next().unwrap_or(id)
}
fn pnpm(docs: &[Value]) -> Result<Lock, String> {
    let mut lock = Lock::default();
    for doc in docs {
        if !matches!(doc["lockfileVersion"].as_str(), Some("6.0" | "6.1" | "7.0" | "9.0")) {
            return Err(format!("its format version is {}, which Kern does not read", stated(&doc["lockfileVersion"])));
        }
        for (key, e) in rows(&doc["packages"], &|| "its packages are not in the form pnpm writes".into())? {
            let Some(p) = pnpm_package(key, e)? else { continue };
            let id = pnpm_id(key);
            let name = split_name(id).map(|(n, _)| n).filter(|n| named(n)).unwrap_or(&p.name).to_string();
            lock.add(id, &name, p)?;
        }
        for (key, e) in rows(&doc["snapshots"], &|| "its snapshots are not in the form pnpm writes".into())? {
            if !e.as_object().is_some_and(|fields| fields.keys().all(|k| SNAPSHOT_FIELDS.contains(&k.as_str()))) { return Err(entry(key, "pnpm")); }
        }
        let importers = match &doc["importers"] { Value::Null => vec![(".", doc)], v => rows(v, &|| "its importers are not in the form pnpm writes".into())?.into_iter().map(|(k, v)| (k.as_str(), v)).collect() };
        for (dir, importer) in importers { pnpm_edges(&mut lock, dir, importer)?; }
    }
    Ok(lock)
}
fn pnpm_package(key: &str, e: &Value) -> Result<Option<Package>, String> {
    let bad = || entry(key, "pnpm");
    let id = pnpm_id(key);
    let (name, reference) = match text_of(e, "name", &bad)? { Some(n) => (n, ""), None => split_name(id).ok_or_else(bad)? };
    let version = text_of(e, "version", &bad)?.unwrap_or(reference);
    let r = &e["resolution"];
    if !r.is_object() { return Err(bad()); }
    let integrity = text_of(r, "integrity", &bad)?.map(|i| sri(i).ok_or_else(bad)).transpose()?;
    let source = match (text_of(r, "type", &bad)?, text_of(r, "tarball", &bad)?, text_of(r, "directory", &bad)?) {
        (Some("git"), _, _) => match (text_of(r, "repo", &bad)?, text_of(r, "commit", &bad)?) { (Some(repo), Some(commit)) => Source::Url(format!("{repo}#{commit}")), _ => return Err(bad()) },
        (Some("directory"), _, _) | (None, None, Some(_)) => return Ok(None),
        (None, Some(tarball), _) => Source::Url(tarball.to_string()),
        (None, None, None) if integrity.is_some() => Source::Registry,
        _ => return Err(bad()),
    };
    package(name, version, source, integrity).map(Some).ok_or_else(bad)
}
fn pnpm_edges(lock: &mut Lock, dir: &str, importer: &Value) -> Result<(), String> {
    for section in ["dependencies", "devDependencies", "optionalDependencies"] {
        for (dep, d) in rows(&importer[section], &|| format!("its importer {} is not in the form pnpm writes", cut(dir)))? {
            let bad = || entry(&format!("{dir} {dep}"), "pnpm");
            let (Some(spec), Some(reference)) = (text_of(d, "specifier", &bad)?, text_of(d, "version", &bad)?) else { return Err(bad()) };
            if let Some(target) = npm_target(spec) { lock.aliases.insert((dep.clone(), target.to_string())); }
            let id = pnpm_id(reference);
            if !split_name(id).is_some_and(|(t, _)| named(t) && t != dep.as_str()) { continue; }
            let Some(package) = lock.installs.get(id).map(|i| i.package.clone()) else { continue };
            let place = if dir == "." { format!("node_modules/{dep}") } else { format!("{dir}/node_modules/{dep}") };
            lock.add(&place, dep, package)?;
        }
    }
    Ok(())
}

fn classic(text: &str) -> bool {
    text.lines().take_while(|l| l.starts_with('#')).map(str::to_ascii_lowercase)
        .any(|l| l.ends_with("v1") && l[1..].starts_with(char::is_whitespace) && l[1..].split_whitespace().eq(["yarn", "lockfile", "v1"]))
}
fn closing(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() { match b[i] { b'\\' => i += 2, b'"' => return Some(i), _ => i += 1 } }
    None
}
fn token(s: &str) -> Option<(String, &str)> {
    if s.starts_with('"') { let end = closing(s)?; return Some((serde_json::from_str(&s[..=end]).ok()?, &s[end + 1..])); }
    if !s.starts_with(|c: char| c.is_ascii_alphabetic() || "/.-".contains(c)) { return None; }
    let end = s.find([':', ' ', ',']).unwrap_or(s.len());
    Some((s[..end].to_string(), &s[end..]))
}
fn keys(body: &str) -> Option<Vec<String>> {
    let mut rest = body.strip_suffix(':')?;
    let mut out = Vec::new();
    loop {
        let (key, tail) = token(rest)?;
        out.push(key);
        if tail.is_empty() { return Some(out); }
        rest = tail.strip_prefix(", ")?;
    }
}
fn pair(body: &str) -> Option<(String, Option<String>)> {
    let (key, rest) = token(body)?;
    if rest == ":" { return Some((key, None)); }
    let (value, tail) = token(rest.strip_prefix(' ')?)?;
    tail.is_empty().then_some((key, Some(value)))
}
type Fields = BTreeMap<String, Option<String>>;
fn yarn(text: &str) -> Result<Lock, String> {
    let mut entries: Vec<(Vec<String>, Fields)> = Vec::new();
    let mut nested = false;
    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let bad = || form(n + 1, "yarn");
        if line.chars().any(char::is_control) { return Err(bad()); }
        let body = line.trim_start_matches(' ');
        if body.is_empty() || body.starts_with('#') { continue; }
        match line.len() - body.len() {
            0 => { entries.push((keys(body).ok_or_else(bad)?, BTreeMap::new())); nested = false; }
            2 => {
                let (key, value) = pair(body).ok_or_else(bad)?;
                nested = value.is_none();
                let (_, fields) = entries.last_mut().ok_or_else(bad)?;
                if fields.insert(key, value).is_some() { return Err(bad()); }
            }
            4 if nested => { pair(body).filter(|(_, v)| v.is_some()).ok_or_else(bad)?; }
            _ => return Err(bad()),
        }
    }
    let mut lock = Lock::default();
    for (keys, fields) in &entries { yarn_entry(&mut lock, keys, fields)?; }
    Ok(lock)
}
fn fragment_sha1(resolved: &str) -> Option<String> {
    let (_, hash) = resolved.rsplit_once('#')?;
    (hash.len() == 40).then(|| hex::decode(hash).ok()).flatten().map(|bytes| format!("sha1-{}", b64(&bytes)))
}
fn yarn_entry(lock: &mut Lock, keys: &[String], fields: &Fields) -> Result<(), String> {
    let bad = || entry(&keys.join(", "), "yarn");
    let field = |k: &str| match fields.get(k) { None => Ok(None), Some(Some(v)) => Ok(Some(v.as_str())), Some(None) => Err(bad()) };
    let version = field("version")?.ok_or_else(bad)?;
    let resolved = field("resolved")?;
    let stated = field("integrity")?.map(|i| sri(i).ok_or_else(bad)).transpose()?;
    let hashes: Vec<String> = stated.into_iter().chain(resolved.and_then(fragment_sha1)).collect();
    let integrity = sri(&hashes.join(" "));
    for key in keys {
        let (dep, range) = split_name(key).ok_or_else(bad)?;
        if resolved.is_none() && ["file:", "link:", "portal:"].iter().any(|p| range.starts_with(p)) && !tarball_file(range) { continue; }
        let real = npm_target(range).unwrap_or(dep);
        if real != dep { lock.aliases.insert((dep.to_string(), real.to_string())); }
        let source = match resolved { Some(r) => Source::Url(r.to_string()), None if tarball_file(range) => Source::Url(range.to_string()), None => return Err(bad()) };
        lock.add(key, dep, package(real, version, source, integrity.clone()).ok_or_else(bad)?)?;
    }
    Ok(())
}

fn berry(docs: &[Value]) -> Result<Lock, String> {
    let [doc] = docs else { return Err("it holds more than one YAML document".into()) };
    let meta = &doc["__metadata"];
    let stated = meta["version"].as_str().unwrap_or("missing");
    if !stated.parse::<u32>().is_ok_and(|v| (4..=10).contains(&v)) { return Err(format!("its format version is {stated}, which Kern does not read")); }
    let key = meta["cacheKey"].as_str().unwrap_or("");
    let mut lock = Lock::default();
    for (descriptors, e) in rows(doc, &|| "it is not in the form yarn writes".into())?.into_iter().filter(|(k, _)| *k != "__metadata") {
        let bad = || entry(descriptors, "yarn");
        let (name, reference) = text_of(e, "resolution", &bad)?.and_then(split_name).ok_or_else(bad)?;
        let Some(source) = berry_source(reference).ok_or_else(bad)? else { continue };
        let integrity = text_of(e, "checksum", &bad)?.map(|c| checksum(c, key).ok_or_else(bad)).transpose()?.flatten();
        let version = text_of(e, "version", &bad)?.ok_or_else(bad)?;
        let p = package(name, version, source, integrity).ok_or_else(bad)?;
        for d in descriptors.split(',').map(|d| d.trim_matches(' ')) {
            let (dep, range) = split_name(d).filter(|(dep, _)| named(dep)).ok_or_else(bad)?;
            if let Some(target) = npm_target(range) { lock.aliases.insert((dep.to_string(), target.to_string())); }
            lock.add(d, dep, p.clone())?;
        }
    }
    Ok(lock)
}
fn berry_source(reference: &str) -> Option<Option<Source>> {
    let (protocol, rest) = reference.split_once(':')?;
    let (selector, params) = rest.split_once("::").unwrap_or((rest, ""));
    let shown = format!("{protocol}:{selector}");
    let archives: Vec<String> = params.split('&').filter_map(|p| { let (k, v) = p.split_once('=').unwrap_or((p, "")); (query(k) == "__archiveUrl").then(|| query(v)) }).collect();
    Some(match protocol {
        "npm" => match archives.as_slice() { [] => Some(Source::Registry), [url] => Some(Source::Url(url.clone())), _ => return None },
        "workspace" | "portal" | "link" | "patch" => None,
        "file" if !tarball_file(&shown) => None,
        _ => Some(Source::Url(shown)),
    })
}
fn query(s: &str) -> String { unescape(&s.replace('+', " ")) }
fn unescape(s: &str) -> String {
    let (b, mut out, mut i) = (s.as_bytes(), Vec::new(), 0);
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => { out.push(byte); i += 3; }
            (c, _) => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn checksum(c: &str, key: &str) -> Option<Option<String>> {
    let (own, hex) = c.split_once('/').unwrap_or((key, c));
    let found = !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && own.bytes().all(|b| b.is_ascii_alphanumeric());
    found.then(|| (own == key).then(|| format!("berry{key}-{hex}")))
}

fn bun(text: &str) -> Result<Lock, String> {
    unique_keys(text)?;
    let v = jsonc(text).ok_or_else(|| "it is not valid JSON".to_string())?;
    if !matches!(v["lockfileVersion"].as_u64(), Some(0..=3)) { return Err(format!("its format version is {}, which Kern does not read", stated(&v["lockfileVersion"]))); }
    let mut lock = Lock::default();
    for (_, w) in v["workspaces"].as_object().into_iter().flatten() { lock.aliases.extend(aliases(w)); }
    for (key, row) in v["packages"].as_object().ok_or_else(|| "it lists no packages".to_string())? {
        let bad = || entry(key, "bun");
        let items = row.as_array().filter(|a| !a.is_empty()).ok_or_else(bad)?;
        let text = |i: usize| match items.get(i) { None => Ok(None), Some(Value::String(s)) => Ok(Some(s.as_str())), Some(_) => Err(bad()) };
        let (name, reference) = text(0)?.and_then(split_name).ok_or_else(bad)?;
        if ["root:", "workspace:", "link:", "file:"].iter().any(|p| reference.starts_with(p)) { continue; }
        let (version, source, info, sum) = if reference.starts_with(|c: char| c.is_ascii_digit()) {
            (reference.to_string(), text(1)?.filter(|r| !r.is_empty()).map_or(Source::Registry, |r| Source::Url(r.to_string())), 2, text(3)?)
        } else if reference.starts_with("git+") || reference.starts_with("github:") {
            (String::new(), Source::Url(reference.to_string()), 1, text(3)?)
        } else {
            (crate::deps::registry_version(reference, name).unwrap_or_default(), Source::Url(reference.to_string()), 1, text(2)?)
        };
        lock.aliases.extend(aliases(&items.get(info).cloned().unwrap_or_default()));
        let integrity = sum.filter(|s| !s.is_empty()).map(|s| sri(s).ok_or_else(bad)).transpose()?;
        lock.add(key, bun_name(key).ok_or_else(bad)?, package(name, &version, source, integrity).ok_or_else(bad)?)?;
    }
    Ok(lock)
}
fn bun_name(key: &str) -> Option<&str> {
    let (mut parts, mut at, mut last) = (key.split('/'), 0, None);
    while let Some(part) = parts.next() {
        let len = if part.starts_with('@') { part.len() + 1 + parts.next()?.len() } else { part.len() };
        let name = key.get(at..at + len).filter(|n| named(n))?;
        (last, at) = (Some(name), at + len + 1);
    }
    last
}
fn unique_keys(text: &str) -> Result<(), String> {
    let (b, mut open, mut key_next, mut i) = (text.as_bytes(), Vec::<Option<BTreeSet<String>>>::new(), false, 0);
    let bad = || "it is not valid JSON".to_string();
    while i < b.len() {
        match b[i] {
            b'"' => {
                let end = i + closing(&text[i..]).ok_or_else(bad)?;
                if let (true, Some(Some(seen))) = (key_next, open.last_mut()) {
                    let key: String = serde_json::from_str(&text[i..=end]).map_err(|_| bad())?;
                    if !seen.insert(key) { return Err(format!("it names the key {} twice in one object", cut(&text[i..=end]))); }
                }
                key_next = false;
                i = end;
            }
            b'{' | b'[' => {
                open.push((b[i] == b'{').then(BTreeSet::new));
                key_next = b[i] == b'{';
                if open.len() > JSON_DEPTH_COUNT_MAX { return Err(format!("it nests more than {JSON_DEPTH_COUNT_MAX} levels deep")); }
            }
            b'}' | b']' => { open.pop(); key_next = false; }
            b',' => key_next = matches!(open.last(), Some(Some(_))),
            b'/' if b.get(i + 1) == Some(&b'/') => i = text[i..].find('\n').map_or(b.len(), |n| i + n),
            b'/' if b.get(i + 1) == Some(&b'*') => i = text[i + 2..].find("*/").map_or(b.len(), |n| i + n + 3),
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

type Lines<'a> = Peekable<Enumerate<Split<'a, char>>>;
enum Block { Map(Map<String, Value>), Seq(Vec<Value>) }
struct Open { indent: usize, key: String, block: Block }
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Schema { #[default] Failsafe, Core }
#[derive(Default)]
struct Doc { open: Vec<Open>, pending: Option<(usize, String)>, schema: Schema }

pub fn yaml(text: &str, schema: Schema) -> Result<Vec<Value>, usize> {
    let mut lines: Lines = text.split('\n').enumerate().peekable();
    let fresh = || Doc { schema, ..Doc::default() };
    let (mut docs, mut doc, mut last) = (Vec::new(), fresh(), 0);
    while let Some((n, raw)) = lines.next() {
        last = n + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if odd(line) { return Err(n + 1); }
        let line = uncommented(line);
        let body = line.trim_start_matches(' ');
        if body.is_empty() { continue; }
        if line == "---" || line == "..." { docs.extend(std::mem::replace(&mut doc, fresh()).end().map_err(|_| n + 1)?); continue; }
        doc.line(line.len() - body.len(), body, &mut lines).map_err(|_| n + 1)?;
    }
    docs.extend(doc.end().map_err(|_| last)?);
    if docs.is_empty() { return Err(1); }
    Ok(docs)
}
pub fn workspace(bytes: &[u8]) -> Result<Map<String, Value>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.split('\n').all(|l| uncommented(l.strip_suffix('\r').unwrap_or(l)).trim_start_matches(' ').is_empty()) { return Ok(Map::new()); }
    match yaml(text, Schema::Core).map_err(|n| format!("line {n} is not in a form Kern reads"))?.as_slice() {
        [Value::Object(settings)] => Ok(settings.clone()),
        _ => Err("it holds more than one YAML document".into()),
    }
}
fn uncommented(line: &str) -> &str {
    let b = line.as_bytes();
    let (mut i, mut open) = (0, true);
    while i < b.len() {
        match b[i] {
            b'#' if i == 0 || b[i - 1] == b' ' => return line[..i].trim_end_matches(' '),
            b'\'' | b'"' if open => {
                let Some(n) = quoted_len(&line[i..]) else { return line };
                (i, open) = (i + n, false);
                continue;
            }
            b' ' => {}
            b'[' | b'{' | b',' => open = true,
            b'-' | b':' | b'?' => open = b.get(i + 1).is_none_or(|c| *c == b' '),
            _ => open = false,
        }
        i += 1;
    }
    line
}
fn quoted_len(s: &str) -> Option<usize> {
    if s.starts_with('"') { return closing(s).map(|end| end + 1); }
    let (b, mut i) = (s.as_bytes(), 1);
    while i < b.len() {
        match (b[i], b.get(i + 1)) { (b'\'', Some(b'\'')) => i += 2, (b'\'', _) => return Some(i + 1), _ => i += 1 }
    }
    None
}
fn odd(line: &str) -> bool { line.chars().any(|c| c.is_control() || matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}' | '\u{feff}')) }
impl Doc {
    fn line(&mut self, indent: usize, body: &str, lines: &mut Lines) -> Result<(), ()> {
        if self.open.is_empty() { self.open.push(Open { indent: 0, key: String::new(), block: Block::Map(Map::new()) }); }
        let item = body == "-" || body.starts_with("- ");
        if let Some((at, key)) = self.pending.take() {
            if indent < at || (indent == at && !item) { self.put(key, Value::Null)?; }
            else if self.open.len() >= YAML_DEPTH_COUNT_MAX { return Err(()); }
            else { self.open.push(Open { indent, key, block: if item { Block::Seq(Vec::new()) } else { Block::Map(Map::new()) } }); }
        }
        while self.open.last().is_some_and(|o| indent < o.indent || (indent == o.indent && !item && matches!(o.block, Block::Seq(_)))) { self.close()?; }
        let top = self.open.last_mut().ok_or(())?;
        if indent != top.indent { return Err(()); }
        if let Block::Seq(items) = &mut top.block { items.push(scalar(body.strip_prefix("- ").ok_or(())?, self.schema)?); return Ok(()); }
        let (key, rest) = key_of(body)?;
        match rest.trim_start() {
            "" => { self.pending = Some((indent, key)); Ok(()) }
            r if r.starts_with(['|', '>']) => { let v = folded(r, indent, lines)?; self.put(key, Value::String(v)) }
            r => { let v = value(r, self.schema)?; self.put(key, v) }
        }
    }
    fn put(&mut self, key: String, v: Value) -> Result<(), ()> {
        match self.open.last_mut().map(|o| &mut o.block) { Some(Block::Map(m)) if !m.contains_key(&key) => { m.insert(key, v); Ok(()) } _ => Err(()) }
    }
    fn close(&mut self) -> Result<(), ()> {
        let open = self.open.pop().ok_or(())?;
        let v = match open.block { Block::Map(m) => Value::Object(m), Block::Seq(s) => Value::Array(s) };
        self.put(open.key, v)
    }
    fn end(mut self) -> Result<Option<Value>, ()> {
        if let Some((_, key)) = self.pending.take() { self.put(key, Value::Null)?; }
        while self.open.len() > 1 { self.close()?; }
        Ok(self.open.pop().and_then(|o| match o.block { Block::Map(m) if !m.is_empty() => Some(Value::Object(m)), _ => None }))
    }
}
fn key_of(body: &str) -> Result<(String, &str), ()> {
    if body.starts_with(['\'', '"']) {
        let (key, rest) = quoted(body)?;
        let rest = rest.strip_prefix(':').ok_or(())?;
        return if rest.is_empty() || rest.starts_with(' ') { Ok((key, rest)) } else { Err(()) };
    }
    let (key, rest) = match body.find(": ") { Some(i) => (&body[..i], &body[i + 1..]), None => (body.strip_suffix(':').ok_or(())?, "") };
    let plain = !key.is_empty() && !key.starts_with(INDICATORS) && !key.contains(" #") && !key.ends_with(' ') && key != "<<";
    if plain { Ok((key.to_string(), rest)) } else { Err(()) }
}
fn quoted(s: &str) -> Result<(String, &str), ()> {
    if s.starts_with('"') { let end = closing(s).ok_or(())?; return Ok((serde_json::from_str(&s[..=end]).map_err(|_| ())?, &s[end + 1..])); }
    let (mut out, mut rest) = (String::new(), s.get(1..).ok_or(())?);
    loop {
        let i = rest.find('\'').ok_or(())?;
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        match rest.strip_prefix('\'') { Some(more) => { out.push('\''); rest = more; } None => return Ok((out, rest)) }
    }
}
fn scalar(s: &str, schema: Schema) -> Result<Value, ()> {
    let s = s.trim_end();
    if s.starts_with(['\'', '"']) { let (v, rest) = quoted(s)?; return if rest.trim().is_empty() { Ok(Value::String(v)) } else { Err(()) }; }
    let plain = !s.is_empty() && !s.starts_with(INDICATORS) && !s.contains(" #") && !s.contains(": ") && !s.ends_with(':');
    match (plain, schema) { (false, _) => Err(()), (true, Schema::Failsafe) => Ok(Value::String(s.to_string())), (true, Schema::Core) => core(s) }
}
fn core(s: &str) -> Result<Value, ()> {
    match s {
        "~" | "null" | "Null" | "NULL" => Ok(Value::Null),
        "true" | "True" | "TRUE" => Ok(Value::Bool(true)),
        "false" | "False" | "FALSE" => Ok(Value::Bool(false)),
        _ if dated(s) => Err(()),
        _ if number_like(s) => number(s).ok_or(()),
        _ => Ok(Value::String(s.to_string())),
    }
}
fn dated(s: &str) -> bool { let b = s.as_bytes(); b.len() >= 6 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[5].is_ascii_digit() }
fn exponent(e: &str) -> bool { let d = e.strip_prefix(['-', '+']).unwrap_or(e); !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()) }
fn number_like(s: &str) -> bool {
    let r = s.strip_prefix(['-', '+']).unwrap_or(s);
    if [".inf", ".nan"].contains(&r.to_ascii_lowercase().as_str()) { return true; }
    if let Some(x) = ["0x", "0o", "0b"].iter().find_map(|p| r.strip_prefix(p)) { return !x.is_empty() && x.chars().all(|c| c.is_ascii_hexdigit() || c == '_'); }
    let (mantissa, exp) = r.split_once(['e', 'E']).map_or((r, None), |(m, e)| (m, Some(e)));
    let (whole, part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    mantissa.bytes().any(|b| b.is_ascii_digit()) && exp.is_none_or(exponent) && whole.chars().chain(part.chars()).all(|c| c.is_ascii_digit() || c == '_')
}
fn number(s: &str) -> Option<Value> {
    let r = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exp) = r.split_once(['e', 'E']).map_or((r, None), |(m, e)| (m, Some(e)));
    let (whole, part) = mantissa.split_once('.').map_or((mantissa, None), |(w, p)| (w, Some(p)));
    let digits = |d: &str| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (whole.len() > 1 && whole.starts_with('0')) || !part.is_none_or(digits) || !exp.is_none_or(exponent) { return None; }
    let f = s.parse::<f64>().ok().filter(|f| f.is_finite())?;
    if f.fract() == 0.0 && f.abs() <= JS_SAFE_INTEGER_MAX as f64 { return Some(Value::from(f as i64)); }
    if part.is_none() && exp.is_none() { return None; }
    serde_json::Number::from_f64(f).map(Value::Number)
}
fn value(r: &str, schema: Schema) -> Result<Value, ()> {
    let r = r.trim_end();
    let close = match r.as_bytes().first() { Some(b'{') => '}', Some(b'[') => ']', _ => return scalar(r, schema) };
    let inner = r.strip_suffix(close).ok_or(())?[1..].trim();
    let items: Vec<&str> = if inner.is_empty() { Vec::new() } else { split_items(inner) };
    let flow = |s: &str| if s.contains(['[', ']', '{', '}', ',']) && !s.starts_with(['\'', '"']) { Err(()) } else { scalar(s, schema) };
    if close == ']' { return items.into_iter().map(flow).collect::<Result<Vec<_>, _>>().map(Value::Array); }
    let mut m = Map::new();
    for item in items {
        let (k, v) = key_of(item)?;
        if m.insert(k, flow(v.trim())?).is_some() { return Err(()); }
    }
    Ok(Value::Object(m))
}
fn split_items(s: &str) -> Vec<&str> {
    let (mut out, mut start, mut quote, mut escaped) = (Vec::new(), 0, None::<char>, false);
    for (i, c) in s.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if c == '\\' => escaped = true,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == ',' => { out.push(s[start..i].trim()); start = i + 1; }
            None => {}
        }
    }
    out.push(s[start..].trim());
    out
}
fn folded(header: &str, indent: usize, lines: &mut Lines) -> Result<String, ()> {
    let header = header.trim_end();
    if !["|", "|-", "|+", ">", ">-", ">+"].contains(&header) { return Err(()); }
    let (mut parts, mut at) = (Vec::new(), None);
    while let Some(&(_, raw)) = lines.peek() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let body = line.trim_start_matches(' ');
        let own = line.len() - body.len();
        if !body.is_empty() && own <= indent { break; }
        if odd(line) || (!body.is_empty() && own < *at.get_or_insert(own)) { return Err(()); }
        parts.push(at.and_then(|a| line.get(a..)).unwrap_or("").to_string());
        lines.next();
    }
    at.ok_or(())?;
    while parts.last().is_some_and(|p| p.is_empty()) { parts.pop(); }
    let joined = parts.join(if header.starts_with('|') { "\n" } else { " " });
    Ok(if header.ends_with('-') { joined } else { joined + "\n" })
}
