use crate::git::Files;
use crate::lock::{Lock, Package, Source};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MANIFEST: &str = "package.json";
pub const LOCKFILE: &str = "package-lock.json";
pub const ANY: &str = "*";
const SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];

pub fn valid_name(s: &str) -> bool {
    let part = |p: &str| !p.is_empty() && !p.starts_with('.') && !p.starts_with('_') && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~".contains(&b));
    s.len() <= 214 && match s.strip_prefix('@') { Some(rest) => rest.split_once('/').is_some_and(|(scope, name)| part(scope) && part(name)), None => part(s) }
}
pub fn valid_pattern(s: &str) -> bool { s.strip_suffix("/*").map_or_else(|| valid_name(s), |scope| scope.starts_with('@') && valid_name(&format!("{scope}/x"))) }
pub fn approved(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| p == ANY || p == name || p.strip_suffix('*').is_some_and(|prefix| prefix.ends_with('/') && name.starts_with(prefix)))
}
fn added_or_removed(a: &Value, b: &Value) -> BTreeSet<String> {
    SECTIONS.iter().flat_map(|s| {
        let (x, y) = (a[*s].as_object().cloned().unwrap_or_default(), b[*s].as_object().cloned().unwrap_or_default());
        x.keys().chain(y.keys()).filter(|n| x.contains_key(*n) != y.contains_key(*n)).cloned().collect::<Vec<_>>()
    }).collect()
}
fn read(files: &Files, path: &str) -> R<Option<Value>> {
    let Some(e) = files.get(path) else { return Ok(None) };
    let parsed = e.bytes.as_deref().and_then(|b| serde_json::from_slice::<Value>(b).ok()).filter(Value::is_object);
    parsed.map(Some).ok_or_else(|| Denied(format!("Dependency files must be valid JSON: {path}")))
}
pub(crate) fn package_of(key: &str) -> &str { key.rsplit_once("node_modules/").map_or(key, |(_, name)| name) }
fn target(name: &str, spec: &Value) -> String {
    let Some(rest) = spec.as_str().and_then(|s| s.strip_prefix("npm:")) else { return name.to_string() };
    rest.get(1..).and_then(|r| r.find('@')).map_or(rest, |i| &rest[..=i]).to_string()
}
fn tarball_path(url: &str) -> Option<&str> { url.split_once("://").and_then(|(_, rest)| rest.split_once('/')).map(|(_, p)| p.split(['?', '#']).next().unwrap_or("")) }
fn tarball_of(url: &str, name: &str, version: &str) -> bool {
    let Some(path) = tarball_path(url) else { return false };
    let plain = !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c));
    let base = name.rsplit('/').next().unwrap_or(name);
    plain && crate::lock::named(name) && [name.to_string(), name.replacen('/', "%2f", 1), name.replacen('/', "%2F", 1)].iter()
        .any(|n| { let own = format!("{n}/-/{base}-{version}.tgz"); path == own || path.ends_with(&format!("/{own}")) })
}
pub(crate) fn registry_version(url: &str, name: &str) -> Option<String> {
    let file = tarball_path(url)?.rsplit('/').next()?;
    let version = file.strip_prefix(&format!("{}-", name.rsplit('/').next().unwrap_or(name)))?.strip_suffix(".tgz")?;
    tarball_of(url, name, version).then(|| version.to_string())
}
type Specs = BTreeMap<(String, &'static str), String>;
fn specs(manifest: &Value) -> Specs {
    let spec = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    SECTIONS.iter().flat_map(|s| manifest[*s].as_object().into_iter().flatten().map(move |(n, v)| ((n.clone(), *s), spec(v)))).collect()
}
fn in_any(specs: &Specs, name: &str) -> Option<String> { SECTIONS.iter().find_map(|s| specs.get(&(name.to_string(), *s)).cloned()) }
pub fn changed_versions(old: &Value, new: &Value) -> Vec<Value> {
    let (before, after) = (specs(old), specs(new));
    let mut out = BTreeMap::new();
    for ((name, s), to) in &after {
        let from = before.get(&(name.clone(), *s)).cloned().or_else(|| in_any(&before, name));
        if from.as_deref() != Some(to.as_str()) { out.insert((name.clone(), *s), json!({ "name": name, "from": from, "to": to })); }
    }
    for ((name, s), from) in &before {
        if !after.contains_key(&(name.clone(), *s)) && in_any(&after, name).is_none() { out.insert((name.clone(), *s), json!({ "name": name, "from": from, "to": null })); }
    }
    out.into_values().collect()
}
pub fn shown(m: &Value) -> String {
    let name = m["name"].as_str().unwrap_or("");
    match (m["from"].as_str(), m["to"].as_str()) {
        (Some(f), Some(t)) => format!("{name} {f} → {t}"),
        (None, Some(t)) => format!("{name} {t} (new)"),
        _ => format!("{name} (removed)"),
    }
}
const VERSIONS_SHOWN_COUNT_MAX: usize = 6;
fn version_order(v: &str) -> Vec<u64> { v.split(['.', '-', '+']).map(|p| p.parse().unwrap_or(0)).collect() }
fn listed_versions(versions: &BTreeSet<&str>) -> String {
    let mut sorted: Vec<&str> = versions.iter().copied().collect();
    sorted.sort_by_key(|v| version_order(v));
    let more = if sorted.len() > VERSIONS_SHOWN_COUNT_MAX { ", …" } else { "" };
    format!("{}{more}", sorted.into_iter().take(VERSIONS_SHOWN_COUNT_MAX).collect::<Vec<_>>().join(", "))
}
pub fn lock_moves(old: &Lock, new: &Lock) -> Vec<Value> {
    let versions = |lock: &Lock| {
        let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for p in lock.packages().into_iter().filter(|p| !p.version.is_empty()) { by_name.entry(p.name.clone()).or_default().insert(p.version.clone()); }
        by_name
    };
    let (before, after, none) = (versions(old), versions(new), BTreeSet::new());
    let names: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    names.into_iter().filter_map(|name| {
        let (b, a) = (before.get(name).unwrap_or(&none), after.get(name).unwrap_or(&none));
        let (gone, came): (BTreeSet<&str>, BTreeSet<&str>) = (b.difference(a).map(String::as_str).collect(), a.difference(b).map(String::as_str).collect());
        let all = |s: &BTreeSet<String>| listed_versions(&s.iter().map(String::as_str).collect());
        let (from, to) = match (gone.len(), came.len()) {
            (0, 0) => return None,
            (1, 1) => (Some(listed_versions(&gone)), Some(listed_versions(&came))),
            _ if b.is_empty() => (None, Some(all(a))),
            _ if a.is_empty() => (Some(all(b)), None),
            _ => (Some(all(b)), Some(all(a))),
        };
        Some(json!({ "name": name, "from": from, "to": to }))
    }).collect()
}
fn catalog(v: &Value, label: &str) -> Option<serde_json::Map<String, Value>> {
    match v {
        Value::Null => Some(serde_json::Map::new()),
        Value::Object(m) => m.iter().map(|(name, spec)| spec.as_str().map(|s| (format!("{name}{label}"), json!(s)))).collect(),
        _ => None,
    }
}
fn catalogs(v: &Value) -> Option<serde_json::Map<String, Value>> {
    match v {
        Value::Null => Some(serde_json::Map::new()),
        Value::Object(m) => m.iter().try_fold(serde_json::Map::new(), |mut all, (name, c)| { all.extend(catalog(c, &format!(" (catalog {name})"))?); Some(all) }),
        _ => None,
    }
}
fn globs(v: &Value) -> Option<BTreeSet<&str>> {
    match v { Value::Null => Some(BTreeSet::new()), Value::Array(items) => items.iter().map(Value::as_str).collect(), _ => None }
}
pub fn workspace_moves(key: &str, old: Option<&Value>, new: Option<&Value>) -> Option<Vec<String>> {
    let (a, b) = (old.unwrap_or(&Value::Null), new.unwrap_or(&Value::Null));
    let versions = |a: serde_json::Map<String, Value>, b: serde_json::Map<String, Value>| changed_versions(&json!({ "dependencies": a }), &json!({ "dependencies": b })).iter().map(shown).collect();
    match key {
        "packages" => {
            let (was, now) = (globs(a)?, globs(b)?);
            Some(now.difference(&was).map(|g| format!("workspace {g} (new)")).chain(was.difference(&now).map(|g| format!("workspace {g} (removed)"))).collect())
        }
        "catalog" => Some(versions(catalog(a, "")?, catalog(b, "")?)),
        "catalogs" => Some(versions(catalogs(a)?, catalogs(b)?)),
        _ => None,
    }
}
pub fn moves(old: &Value, new: &Value) -> Vec<String> { changed_versions(old, new).iter().map(shown).collect::<BTreeSet<_>>().into_iter().collect() }
const REGISTRIES: [&str; 2] = ["registry.npmjs.org", "registry.yarnpkg.com"];
fn host(url: &str) -> Option<String> {
    url.split_once("://").and_then(|(_, rest)| rest.split(['/', '?', '#']).next()).filter(|h| !h.is_empty()).map(str::to_ascii_lowercase)
}
const LOCK_FINDINGS_COUNT_MAX: usize = 20;
const STRONGEST: [&str; 4] = ["sha512", "sha384", "sha256", "sha1"];
fn url_of(p: &Package) -> Option<&str> { match &p.source { Source::Url(url) => Some(url), Source::Registry => None } }
fn own_tarball(p: &Package) -> bool { url_of(p).is_none_or(|url| tarball_of(url, &p.name, &p.version)) }
fn label(p: &Package) -> String { if p.version.is_empty() { p.name.clone() } else { format!("{} {}", p.name, p.version) } }
fn digests(integrity: &str) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut by_kind: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (kind, digest) in integrity.split(' ').filter_map(|t| t.split_once('-')) { by_kind.entry(kind).or_default().insert(digest.split('?').next().unwrap_or(digest)); }
    by_kind
}
fn same_file(was: &str, now: &str) -> Option<bool> {
    let (a, b) = (digests(was), digests(now));
    let kind = STRONGEST.iter().copied().find(|k| a.contains_key(k) && b.contains_key(k)).or_else(|| a.keys().copied().find(|k| b.contains_key(k)))?;
    Some(!a[kind].is_disjoint(&b[kind]))
}
type Earlier<'a> = BTreeMap<(&'a str, &'a str), Vec<&'a Package>>;
fn checksum_moved(p: &Package, before: &Earlier) -> Option<String> {
    let same_place = |o: &&&Package| own_tarball(o) == own_tarball(p) && (own_tarball(p) || url_of(o) == url_of(p));
    let earlier: Vec<&&Package> = before.get(&(p.name.as_str(), p.version.as_str())).into_iter().flatten().filter(same_place).collect();
    let kept = |o: &&&Package| match (&o.integrity, &p.integrity) { (Some(was), Some(now)) => same_file(was, now) != Some(false), (Some(_), None) => false, _ => true };
    if earlier.is_empty() || earlier.iter().any(kept) { return None; }
    Some(match p.integrity {
        Some(_) => format!("{} has a new checksum without a new version: the file it downloads changed", label(p)),
        None => format!("{} lost its checksum without a new version, so nothing pins the file it downloads", label(p)),
    })
}
pub fn lock_surprises(old: &Lock, new: &Lock) -> Vec<String> {
    let before = old.packages();
    let known: BTreeSet<String> = before.iter().filter_map(|p| url_of(p).and_then(host)).chain(REGISTRIES.iter().map(|r| r.to_string())).collect();
    let mut found: Vec<String> = new.installs.iter().filter_map(|(place, i)| {
        let was = old.installs.get(place).map_or(i.name.as_str(), |o| o.package.name.as_str());
        let swapped = own_tarball(&i.package) && i.package.name != was && !new.aliases.contains(&(i.name.clone(), i.package.name.clone()));
        swapped.then(|| format!("{place} installs another package: {} in place of {was}", i.package.name))
    }).collect();
    let mut earlier: Earlier = BTreeMap::new();
    for p in &before { earlier.entry((p.name.as_str(), p.version.as_str())).or_default().push(p); }
    let (mut foreign, mut hosts) = (Vec::new(), BTreeMap::<String, (usize, String)>::new());
    for p in new.packages().difference(&before) {
        found.extend(checksum_moved(p, &earlier));
        match url_of(p) {
            Some(url) if !own_tarball(p) => foreign.push(format!("{} downloads from {url}, which is not its own registry tarball", label(p))),
            Some(url) => { if let Some(h) = host(url).filter(|h| !known.contains(h)) { hosts.entry(h).or_insert((0, label(p))).0 += 1; } }
            None => {}
        }
    }
    found.extend(foreign);
    found.extend(hosts.into_iter().map(|(h, (n, first))| format!("{} now download from {h}, a host this lockfile did not use before (first: {first})", if n == 1 { "1 package".to_string() } else { format!("{n} packages") })));
    if found.len() > LOCK_FINDINGS_COUNT_MAX {
        let more = found.len() - LOCK_FINDINGS_COUNT_MAX;
        found.truncate(LOCK_FINDINGS_COUNT_MAX);
        found.push(format!("and {more} more lockfile entries like these"));
    }
    found
}
fn fields_changed(a: &Value, b: &Value, keep: &[&str]) -> Vec<String> {
    let keys: BTreeSet<&String> = a.as_object().into_iter().chain(b.as_object()).flat_map(|o| o.keys()).collect();
    keys.into_iter().filter(|k| !keep.contains(&k.as_str()) && a.get(k.as_str()) != b.get(k.as_str())).cloned().collect()
}
fn names_changed(a: &Value, b: &Value) -> BTreeSet<String> {
    SECTIONS.iter().flat_map(|s| {
        let (x, y) = (a[*s].as_object().cloned().unwrap_or_default(), b[*s].as_object().cloned().unwrap_or_default());
        x.keys().chain(y.keys()).filter(|n| x.get(*n) != y.get(*n)).cloned().collect::<Vec<_>>()
    }).collect()
}

pub fn screen(base: &Files, candidate: &Files, patterns: &[String]) -> R<Value> {
    let (old, new) = (read(base, MANIFEST)?.unwrap_or_else(|| json!({})), read(candidate, MANIFEST)?.ok_or_else(|| Denied(format!("Dependency files must be valid JSON: {MANIFEST}")))?);
    let touched = fields_changed(&old, &new, &SECTIONS);
    ensure!(touched.is_empty(), "package.json may only change dependency entries, but this change touches: {}", touched.join(", "));
    ensure!(SECTIONS.iter().all(|s| new.get(*s).is_none_or(Value::is_object)), "package.json may only change dependency entries, but this change touches: the shape of a dependency section");
    let declared = names_changed(&old, &new);
    let list = patterns.join(", ");
    let upgrade = patterns.iter().any(|p| p == ANY);
    let only_moves = |a: &Value, b: &Value| -> R<()> {
        let names: Vec<String> = added_or_removed(a, b).into_iter().collect();
        ensure!(!upgrade || names.is_empty(), "This task only upgrades packages the project already uses, but this change adds or removes: {}", names.join(", "));
        Ok(())
    };
    only_moves(&old, &new)?;
    for name in &declared { ensure!(valid_name(name) && approved(patterns, name), "Dependency change not approved: {name}. The task approves changes only to: {list}"); }
    for s in SECTIONS {
        for (name, spec) in new[s].as_object().into_iter().flatten() {
            let before = old[s].get(name.as_str());
            if before == Some(spec) { continue; }
            let (was, now) = (before.map_or_else(|| name.clone(), |b| target(name, b)), target(name, spec));
            ensure!(was == now, "package.json swaps {name} for another package: {was} becomes {now}");
        }
    }
    let (lock_old, lock_new) = (read(base, LOCKFILE)?.unwrap_or_else(|| json!({})), read(candidate, LOCKFILE)?.unwrap_or_else(|| json!({})));
    ensure!(lock_old["lockfileVersion"] == lock_new["lockfileVersion"], "The lockfile format changed: keep lockfileVersion as it was");
    let header = fields_changed(&lock_old, &lock_new, &["packages", "dependencies"]);
    ensure!(header.is_empty(), "The lockfile changes packages the task did not approve: its {}", header.join(", "));
    let (a, b) = (lock_old["packages"].as_object().cloned().unwrap_or_default(), lock_new["packages"].as_object().cloned().unwrap_or_default());
    for (key, entry) in b.iter().filter(|(k, v)| !k.is_empty() && a.get(k.as_str()) != Some(*v)) {
        let real = entry["name"].as_str().unwrap_or_else(|| package_of(key));
        if let Some(old) = a.get(key.as_str()) {
            let was = old["name"].as_str().unwrap_or_else(|| package_of(key));
            ensure!(was == real, "The lockfile swaps {key} for another package: {was} becomes {real}");
        }
        if let Some(url) = entry["resolved"].as_str() {
            let version = entry["version"].as_str().unwrap_or("");
            ensure!(tarball_of(url, real, version), "The lockfile resolves {key} to a tarball that is not {real} {version}: {url}");
        }
    }
    let (root_a, root_b) = (a.get("").cloned().unwrap_or_else(|| json!({})), b.get("").cloned().unwrap_or_else(|| json!({})));
    let root_fields = fields_changed(&root_a, &root_b, &SECTIONS);
    ensure!(root_fields.is_empty(), "The lockfile changes packages the task did not approve: its root {}", root_fields.join(", "));
    only_moves(&root_a, &root_b)?;
    for name in names_changed(&root_a, &root_b) { ensure!(approved(patterns, &name), "Dependency change not approved: {name}. The task approves changes only to: {list}"); }
    let mut need: BTreeSet<String> = declared.iter().cloned().chain(a.keys().chain(b.keys()).map(|k| package_of(k).to_string()).filter(|n| approved(patterns, n))).collect();
    loop {
        let more: Vec<String> = b.iter().filter(|(k, _)| !k.is_empty() && need.contains(package_of(k)))
            .flat_map(|(_, entry)| ["dependencies", "optionalDependencies", "peerDependencies"].into_iter().flat_map(move |s| entry[s].as_object().into_iter().flat_map(|d| d.keys().cloned())))
            .filter(|n| !need.contains(n)).collect();
        if more.is_empty() { break; }
        need.extend(more);
    }
    let legacy = |v: &Value| v["dependencies"].as_object().cloned().unwrap_or_default();
    let (la, lb) = (legacy(&lock_old), legacy(&lock_new));
    let moved: BTreeSet<&str> = a.keys().chain(b.keys()).filter(|k| !k.is_empty() && a.get(*k) != b.get(*k)).map(|k| package_of(k))
        .chain(la.keys().chain(lb.keys()).filter(|k| la.get(*k) != lb.get(*k)).map(String::as_str))
        .filter(|n| !need.contains(*n)).collect();
    ensure!(moved.is_empty(), "The lockfile changes packages the task did not approve: {}", moved.into_iter().collect::<Vec<_>>().join(", "));
    let packages = a.keys().chain(b.keys()).filter(|k| !k.is_empty() && a.get(*k) != b.get(*k)).collect::<BTreeSet<_>>().len();
    Ok(if upgrade { json!({ "upgraded": declared, "lockfile_packages_changed": packages }) } else { json!({ "approved": declared, "lockfile_packages_changed": packages }) })
}
pub fn moved(base: &Files, candidate: &Files) -> BTreeSet<String> {
    let lock = |f: &Files| f.get(LOCKFILE).and_then(|e| e.bytes.as_deref()).and_then(|b| serde_json::from_slice::<Value>(b).ok()).map(|v| v["packages"].as_object().cloned().unwrap_or_default()).unwrap_or_default();
    let (a, b) = (lock(base), lock(candidate));
    a.keys().chain(b.keys()).filter(|k| !k.is_empty() && a.get(*k) != b.get(*k)).map(|k| package_of(k).to_string()).collect()
}

pub fn imports(changed: &Value) -> BTreeSet<String> {
    changed["dependencies"]["approved"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|n| valid_name(n)).map(|n| format!("package:{n}")).collect()
}

pub fn validate(patterns: &[String]) -> R<()> {
    ensure!(patterns.len() <= 64, "dependencies.allow names at most 64 packages");
    ensure!(patterns.iter().all(|p| valid_pattern(p)), "dependencies.allow must list npm package names or scopes such as @types/*");
    ensure!(patterns.windows(2).all(|w| w[0] != w[1]), "dependencies.allow must not repeat a package");
    if patterns.is_empty() { deny!("dependencies.allow must name at least one package when present"); }
    Ok(())
}
