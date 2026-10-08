use crate::git::Files;
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MANIFEST: &str = "package.json";
pub const LOCKFILE: &str = "package-lock.json";
const SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];

/// An npm package name: lowercase, optionally under a scope.
pub fn valid_name(s: &str) -> bool {
    let part = |p: &str| !p.is_empty() && !p.starts_with('.') && !p.starts_with('_') && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~".contains(&b));
    s.len() <= 214 && match s.strip_prefix('@') { Some(rest) => rest.split_once('/').is_some_and(|(scope, name)| part(scope) && part(name)), None => part(s) }
}
/// What a task may approve: a package name, or a whole scope as `@scope/*`.
pub fn valid_pattern(s: &str) -> bool { s.strip_suffix("/*").map_or_else(|| valid_name(s), |scope| scope.starts_with('@') && valid_name(&format!("{scope}/x"))) }
pub fn approved(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| p == name || p.strip_suffix('*').is_some_and(|prefix| prefix.ends_with('/') && name.starts_with(prefix)))
}
fn read(files: &Files, path: &str) -> R<Option<Value>> {
    let Some(e) = files.get(path) else { return Ok(None) };
    let parsed = e.bytes.as_deref().and_then(|b| serde_json::from_slice::<Value>(b).ok()).filter(Value::is_object);
    parsed.map(Some).ok_or_else(|| Denied(format!("Dependency files must be valid JSON: {path}")))
}
fn package_of(key: &str) -> &str { key.rsplit_once("node_modules/").map_or(key, |(_, name)| name) }
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
    for name in &declared { ensure!(valid_name(name) && approved(patterns, name), "Dependency change not approved: {name}. The task approves changes only to: {list}"); }
    let (lock_old, lock_new) = (read(base, LOCKFILE)?.unwrap_or_else(|| json!({})), read(candidate, LOCKFILE)?.unwrap_or_else(|| json!({})));
    ensure!(lock_old["lockfileVersion"] == lock_new["lockfileVersion"], "The lockfile format changed: keep lockfileVersion as it was");
    let header = fields_changed(&lock_old, &lock_new, &["packages", "dependencies"]);
    ensure!(header.is_empty(), "The lockfile changes packages the task did not approve: its {}", header.join(", "));
    let (a, b) = (lock_old["packages"].as_object().cloned().unwrap_or_default(), lock_new["packages"].as_object().cloned().unwrap_or_default());
    // The root entry mirrors package.json: only its dependency sections may change, and only for approved packages.
    let (root_a, root_b) = (a.get("").cloned().unwrap_or_else(|| json!({})), b.get("").cloned().unwrap_or_else(|| json!({})));
    let root_fields = fields_changed(&root_a, &root_b, &SECTIONS);
    ensure!(root_fields.is_empty(), "The lockfile changes packages the task did not approve: its root {}", root_fields.join(", "));
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
    Ok(json!({ "approved": declared, "lockfile_packages_changed": packages }))
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
