use crate::git::Files;
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
fn package_of(key: &str) -> &str { key.rsplit_once("node_modules/").map_or(key, |(_, name)| name) }
fn target(name: &str, spec: &Value) -> String {
    let Some(rest) = spec.as_str().and_then(|s| s.strip_prefix("npm:")) else { return name.to_string() };
    rest.get(1..).and_then(|r| r.find('@')).map_or(rest, |i| &rest[..=i]).to_string()
}
fn tarball_of(url: &str, name: &str, version: &str) -> bool {
    let Some(path) = url.split_once("://").and_then(|(_, rest)| rest.split_once('/')).map(|(_, p)| p.split(['?', '#']).next().unwrap_or("")) else { return false };
    let plain = !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c));
    let base = name.rsplit('/').next().unwrap_or(name);
    plain && valid_name(name) && [name.to_string(), name.replacen('/', "%2f", 1), name.replacen('/', "%2F", 1)].iter()
        .any(|n| { let own = format!("{n}/-/{base}-{version}.tgz"); path == own || path.ends_with(&format!("/{own}")) })
}
pub const FRESH_DAYS: u64 = 7;
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
fn local(spec: &str) -> bool { ["workspace:", "link:", "file:", "portal:"].iter().any(|p| spec.starts_with(p)) }
pub fn top(spec: &str) -> Option<(u64, u64, u64)> {
    let floors: Vec<Option<(u64, u64, u64)>> = spec.split("||").map(floor).collect();
    if floors.iter().any(Option::is_none) { return None; }
    floors.into_iter().flatten().max()
}
pub fn floor(spec: &str) -> Option<(u64, u64, u64)> {
    let s = spec.trim();
    if s.contains(':') || s.contains('/') { return None; }
    let first = s.split("||").next()?.split_whitespace().next()?;
    let v = first.trim_start_matches(['^', '~', '>', '=', 'v']);
    let mut parts = v.split('.');
    let number = |p: Option<&str>| -> Option<u64> { match p { None => Some(0), Some(x) if ["x", "X", "*"].contains(&x) => Some(0), Some(x) => x.split(['-', '+']).next()?.parse().ok() } };
    let major = parts.next().filter(|m| !m.is_empty() && *m != "*" && *m != "x")?.parse().ok()?;
    Some((major, number(parts.next())?, number(parts.next())?))
}
pub fn rfc3339(s: &str) -> Option<u64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let clock: Vec<i64> = time.trim_end_matches('Z').split(['.', '+']).next()?.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let (hh, mm, ss) = (*clock.first()?, *clock.get(1)?, clock.get(2).copied().unwrap_or(0));
    let yy = if m <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from(days * 86_400 + hh * 3_600 + mm * 60 + ss).ok()
}
pub fn age(from: Option<&str>, to: Option<&str>, published: &dyn Fn(&str) -> Option<u64>, now: u64) -> Value {
    let (Some(f), Some(t)) = (from, to) else { return serde_json::json!({ "age": if from.is_none() { "new" } else { "removed" } }) };
    let (Some(a), Some(b)) = (top(f), top(t)) else { return serde_json::json!({ "age": "unknown" }) };
    if a.0 != b.0 || (a.0 == 0 && a.1 != b.1) { return serde_json::json!({ "age": "major" }); }
    released(&format!("{}.{}.{}", b.0, b.1, b.2), published, now)
}
pub fn lock_age(to: Option<&str>, published: &dyn Fn(&str) -> Option<u64>, now: u64) -> Value {
    let Some(version) = to else { return serde_json::json!({ "age": "removed" }) };
    released(version, published, now)
}
fn released(version: &str, published: &dyn Fn(&str) -> Option<u64>, now: u64) -> Value {
    match published(version) {
        None => serde_json::json!({ "age": "unknown", "version": version }),
        Some(at) if now.saturating_sub(at) < FRESH_DAYS * 86_400 => serde_json::json!({ "age": "fresh", "version": version, "days": now.saturating_sub(at) / 86_400 }),
        Some(_) => serde_json::json!({ "age": "routine", "version": version }),
    }
}
pub fn aged(mut rules: Value, now: u64, published: &dyn Fn(&str, &str) -> Option<u64>, own: &dyn Fn(&str) -> bool) -> Value {
    for pass in [false, true] {
        for d in rules["dependencies"].as_array_mut().into_iter().flatten() {
            for m in d["moves"].as_array_mut().into_iter().flatten().filter(|m| (m["lock"] == true) == pass) {
                let name = m["name"].as_str().unwrap_or("").to_string();
                let verdict = if pass { lock_age(m["to"].as_str(), &|v| published(&name, v), now) }
                    else if own(&name) || m["to"].as_str().is_some_and(local) { json!({ "age": "internal" }) }
                    else { age(m["from"].as_str(), m["to"].as_str(), &|v| published(&name, v), now) };
                for (k, v) in verdict.as_object().into_iter().flatten() { m[k] = v.clone(); }
            }
        }
    }
    rules
}
pub fn lock_moves(old: &Value, new: &Value) -> Vec<Value> {
    let deps = |lock: &Value| -> BTreeMap<String, Value> {
        let packaged = lock["packages"].as_object().is_some_and(|p| !p.is_empty());
        entries(lock).into_iter().filter(|(k, e)| e["link"] != true && (!packaged || k.contains("node_modules/")) && e["version"].is_string()).collect()
    };
    let name_of = |key: &str, e: &Value| e["name"].as_str().map_or_else(|| package_of(key).to_string(), str::to_string);
    let (before, after) = (deps(old), deps(new));
    let mut out = BTreeMap::new();
    for (key, e) in &after {
        let (version, was) = (e["version"].as_str().unwrap_or(""), before.get(key).and_then(|o| o["version"].as_str()));
        if was == Some(version) { continue; }
        out.insert((name_of(key, e), version.to_string()), serde_json::json!({ "name": name_of(key, e), "from": was, "to": version, "lock": true }));
    }
    for (key, e) in before.iter().filter(|(k, _)| !after.contains_key(*k)) {
        out.entry((name_of(key, e), String::new())).or_insert_with(|| serde_json::json!({ "name": name_of(key, e), "from": e["version"], "to": null, "lock": true }));
    }
    out.into_values().collect()
}
pub fn registry_meta(name: &str) -> Option<Value> {
    if !valid_name(name) { return None; }
    let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(2).http_status_as_error(false).timeout_global(Some(std::time::Duration::from_secs(20))).build().into();
    let mut response = agent.get(&format!("https://registry.npmjs.org/{}", name.replace('/', "%2F"))).header("Accept", "application/json").call().ok()?;
    if response.status() != 200 { return None; }
    let body = response.body_mut().with_config().limit(64 << 20).read_to_vec().ok()?;
    serde_json::from_slice::<Value>(&body).ok().map(|v| serde_json::json!({ "time": v["time"], "repository": v["repository"] }))
}
pub fn moves(old: &Value, new: &Value) -> Vec<String> { changed_versions(old, new).iter().map(shown).collect::<BTreeSet<_>>().into_iter().collect() }
const REGISTRIES: [&str; 2] = ["registry.npmjs.org", "registry.yarnpkg.com"];
fn host(url: &str) -> Option<String> {
    url.split_once("://").and_then(|(_, rest)| rest.split(['/', '?', '#']).next()).filter(|h| !h.is_empty()).map(str::to_ascii_lowercase)
}
fn legacy(prefix: &str, deps: &Value, out: &mut Vec<(String, Value)>) {
    for (name, entry) in deps.as_object().into_iter().flatten() {
        let key = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let mut own = entry.clone();
        if let Some(o) = own.as_object_mut() { o.remove("dependencies"); o.insert("name".into(), Value::String(name.clone())); }
        out.push((key.clone(), own));
        legacy(&key, &entry["dependencies"], out);
    }
}
fn entries(lock: &Value) -> Vec<(String, Value)> {
    let packages = lock["packages"].as_object().cloned().unwrap_or_default();
    if !packages.is_empty() { return packages.into_iter().filter(|(k, _)| !k.is_empty()).collect(); }
    let mut out = Vec::new();
    legacy("", &lock["dependencies"], &mut out);
    out
}
pub fn lock_surprises(old: &Value, new: &Value) -> Vec<String> {
    let before: BTreeMap<String, Value> = entries(old).into_iter().collect();
    let known: BTreeSet<String> = before.values().filter_map(|e| e["resolved"].as_str().and_then(host)).chain(REGISTRIES.iter().map(|r| r.to_string())).collect();
    let (mut swaps, mut foreign, mut hosts) = (Vec::new(), Vec::new(), BTreeMap::<String, (usize, String)>::new());
    let aliases: BTreeSet<(String, String)> = entries(new).iter().flat_map(|(_, e)| {
        ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"].iter().flat_map(|k| e[*k].as_object().cloned().unwrap_or_default()).collect::<Vec<_>>()
    }).filter_map(|(name, spec)| {
        let target = spec.as_str()?.strip_prefix("npm:")?;
        let at = target.rfind('@').filter(|i| *i > 0).unwrap_or(target.len());
        Some((name, target[..at].to_string()))
    }).collect();
    let packaged = new["packages"].as_object().is_some_and(|p| !p.is_empty());
    for (key, entry) in entries(new).into_iter().filter(|(k, v)| before.get(k) != Some(v) && v["link"] != true && (!packaged || k.contains("node_modules/"))) {
        let real = entry["name"].as_str().map_or_else(|| package_of(&key).to_string(), str::to_string);
        let was = before.get(&key).map_or_else(|| package_of(&key).to_string(), |o| o["name"].as_str().map_or_else(|| package_of(&key).to_string(), str::to_string));
        if was != real && !aliases.contains(&(package_of(&key).to_string(), real.clone())) { swaps.push(format!("{key} installs another package: {real} in place of {was}")); continue; }
        let version = entry["version"].as_str().unwrap_or("");
        if let Some(o) = before.get(&key) && o["version"] == entry["version"] && o["integrity"].is_string() && o["integrity"] != entry["integrity"] {
            swaps.push(format!("{real} {version} has a new checksum without a new version: the file it downloads changed"));
        }
        let Some(url) = entry["resolved"].as_str() else { continue };
        if !tarball_of(url, &real, version) { foreign.push(format!("{real} {version} downloads from {url}, which is not its own registry tarball")); continue; }
        if let Some(h) = host(url).filter(|h| !known.contains(h)) {
            let e = hosts.entry(h).or_insert((0, format!("{real} {version}")));
            e.0 += 1;
        }
    }
    swaps.extend(foreign);
    for (h, (n, first)) in hosts {
        swaps.push(format!("{} now download from {h}, a host this lockfile did not use before (first: {first})", if n == 1 { "1 package".to_string() } else { format!("{n} packages") }));
    }
    swaps
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
