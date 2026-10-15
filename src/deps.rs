use crate::git::Files;
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MANIFEST: &str = "package.json";
pub const LOCKFILE: &str = "package-lock.json";
/// `dependencies.allow: ["*"]`: an upgrade task. Any package the project already uses may move to another version, none may be added or removed.
pub const ANY: &str = "*";
const SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];

/// An npm package name: lowercase, optionally under a scope.
pub fn valid_name(s: &str) -> bool {
    let part = |p: &str| !p.is_empty() && !p.starts_with('.') && !p.starts_with('_') && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~".contains(&b));
    s.len() <= 214 && match s.strip_prefix('@') { Some(rest) => rest.split_once('/').is_some_and(|(scope, name)| part(scope) && part(name)), None => part(s) }
}
/// What a task may approve: a package name, or a whole scope as `@scope/*`.
pub fn valid_pattern(s: &str) -> bool { s.strip_suffix("/*").map_or_else(|| valid_name(s), |scope| scope.starts_with('@') && valid_name(&format!("{scope}/x"))) }
pub fn approved(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| p == ANY || p == name || p.strip_suffix('*').is_some_and(|prefix| prefix.ends_with('/') && name.starts_with(prefix)))
}
/// Names that appear in a dependency section on one side only: added or removed, not moved.
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
/// The package a dependency spec installs: the target of an `npm:` alias, or the dependency's own name.
fn target(name: &str, spec: &Value) -> String {
    let Some(rest) = spec.as_str().and_then(|s| s.strip_prefix("npm:")) else { return name.to_string() };
    // `npm:pkg@1`, `npm:@scope/pkg@1` or `npm:pkg`: the name ends at the first `@` after its first character.
    rest.get(1..).and_then(|r| r.find('@')).map_or(rest, |i| &rest[..=i]).to_string()
}
/// Whether a registry URL is the package's own tarball of that version: `.../<name>/-/<base>-<version>.tgz`.
fn tarball_of(url: &str, name: &str, version: &str) -> bool {
    let Some(path) = url.split_once("://").and_then(|(_, rest)| rest.split_once('/')).map(|(_, p)| p.split(['?', '#']).next().unwrap_or("")) else { return false };
    let plain = !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c));
    let base = name.rsplit('/').next().unwrap_or(name);
    plain && valid_name(name) && [name.to_string(), name.replacen('/', "%2f", 1), name.replacen('/', "%2F", 1)].iter()
        .any(|n| { let own = format!("{n}/-/{base}-{version}.tgz"); path == own || path.ends_with(&format!("/{own}")) })
}
/// What a change does to one package.json's dependencies, for people, sorted: `name old → new`, `name spec (new)` and `name (removed)`.
pub fn moves(old: &Value, new: &Value) -> Vec<String> {
    let spec = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    let mut out = BTreeSet::new();
    for s in SECTIONS {
        let (x, y) = (old[s].as_object().cloned().unwrap_or_default(), new[s].as_object().cloned().unwrap_or_default());
        for name in x.keys().chain(y.keys()) {
            match (x.get(name), y.get(name)) {
                (Some(a), Some(b)) if a != b => { out.insert(format!("{name} {} → {}", spec(a), spec(b))); }
                (None, Some(b)) => { out.insert(format!("{name} {} (new)", spec(b))); }
                (Some(_), None) => { out.insert(format!("{name} (removed)")); }
                _ => {}
            }
        }
    }
    out.into_iter().collect()
}
/// The locked packages a lockfile change points somewhere else: another package under a package's name, or a download that is not that
/// package's own registry tarball of its version (someone's copy of it, a git or file source). One sentence each.
pub fn lock_surprises(old: &Value, new: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut tarball = |name: &str, entry: &Value| {
        if let Some(url) = entry["resolved"].as_str() {
            let version = entry["version"].as_str().unwrap_or("");
            if !tarball_of(url, name, version) { out.push(format!("{name} {version} downloads from {url}, which is not its own registry tarball")); }
        }
    };
    let (a, b) = (old["packages"].as_object().cloned().unwrap_or_default(), new["packages"].as_object().cloned().unwrap_or_default());
    let mut swaps = Vec::new();
    for (key, entry) in b.iter().filter(|(k, v)| !k.is_empty() && a.get(k.as_str()) != Some(*v) && v["link"] != true) {
        let real = entry["name"].as_str().unwrap_or_else(|| package_of(key));
        let was = a.get(key.as_str()).map_or_else(|| package_of(key), |o| o["name"].as_str().unwrap_or_else(|| package_of(key)));
        if was != real { swaps.push(format!("{key} installs another package: {real} in place of {was}")); continue; }
        tarball(real, entry);
    }
    // Lockfiles from npm 6 list packages under `dependencies`, by name.
    let (la, lb) = (old["dependencies"].as_object().cloned().unwrap_or_default(), new["dependencies"].as_object().cloned().unwrap_or_default());
    if b.is_empty() { for (name, entry) in lb.iter().filter(|(k, v)| la.get(k.as_str()) != Some(*v)) { tarball(name, entry); } }
    swaps.extend(out);
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

/// A change to package.json or package-lock.json under a task that approves packages: only the dependency entries of approved packages may
/// change, and the lockfile may only move approved packages and the packages they need. Everything else in both files stays as it was.
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
    // A dependency keeps its identity: it may move to another version, never become an `npm:` alias of another package.
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
    // So does every locked package that changes: never another package under its name (`"name": "evil"`), and what it resolves to is
    // that package's own tarball of that version. A package an upgrade newly needs may be an alias its own dependencies declare.
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
    // The root entry mirrors package.json: only its dependency sections may change, and only for approved packages.
    let (root_a, root_b) = (a.get("").cloned().unwrap_or_else(|| json!({})), b.get("").cloned().unwrap_or_else(|| json!({})));
    let root_fields = fields_changed(&root_a, &root_b, &SECTIONS);
    ensure!(root_fields.is_empty(), "The lockfile changes packages the task did not approve: its root {}", root_fields.join(", "));
    only_moves(&root_a, &root_b)?;
    for name in names_changed(&root_a, &root_b) { ensure!(approved(patterns, &name), "Dependency change not approved: {name}. The task approves changes only to: {list}"); }
    // The packages an approved package needs, followed by name through the new lockfile.
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
    // An upgrade names what moved, never what code may newly import (see `imports`).
    Ok(if upgrade { json!({ "upgraded": declared, "lockfile_packages_changed": packages }) } else { json!({ "approved": declared, "lockfile_packages_changed": packages }) })
}
/// The packages whose locked version differs between two trees: what an upgrade moved, directly or through another package.
pub fn moved(base: &Files, candidate: &Files) -> BTreeSet<String> {
    let lock = |f: &Files| f.get(LOCKFILE).and_then(|e| e.bytes.as_deref()).and_then(|b| serde_json::from_slice::<Value>(b).ok()).map(|v| v["packages"].as_object().cloned().unwrap_or_default()).unwrap_or_default();
    let (a, b) = (lock(base), lock(candidate));
    a.keys().chain(b.keys()).filter(|k| !k.is_empty() && a.get(*k) != b.get(*k)).map(|k| package_of(k).to_string()).collect()
}

/// Package names a change may newly import because the task approved adding them.
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
