//! The rules a receipt applies without running anything, in seconds. The merge gate refuses at the first problem; a receipt reports every
//! one, so these call the same checks (`capability::check`, `proof::screen`, `git::protected`) one changed file at a time and keep what
//! each says:
//! - `powers`: code that newly starts programs, reaches the network, reads environment variables or imports a new package;
//! - `tricks`: test files that newly skip or focus tests, and code that reaches into the test runner;
//! - `settings`: the test runner's settings, which decide which tests run and how they check results;
//! - `protected`: CI, secret and other configuration files;
//! - `dependencies`: dependency manifests and lockfiles, each package.json with the packages it moves;
//! - `lockfile`: npm lockfile entries that now install another package under a name, or download from somewhere other than the
//!   package's own registry tarball;
//! - `agent_notes`: the instructions coding agents read, which steer the next agent.
use crate::capability::{is_code, is_data};
use crate::git::{Entry, Files, git_with};
use crate::policy::{Edits, Kind, Runner, Tests};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const DEPENDENCY_FILES: [&str; 13] = ["package.json", "package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "pnpm-workspace.yaml", "bun.lock", "bun.lockb",
    "cargo.toml", "cargo.lock", "go.mod", "go.sum", "pyproject.toml"];
const AGENT_NOTES: [&str; 6] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md", ".cursorrules", ".windsurfrules", "copilot-instructions.md"];
const AGENT_FOLDERS: [&str; 3] = [".cursor", ".claude", ".windsurf"];
/// Files larger than this are named as not checked rather than read.
const LARGEST: u64 = 1 << 20;
/// npm lockfiles are read up to this size: a large project's runs to many megabytes.
const LARGEST_LOCKFILE: u64 = 64 << 20;
fn npm_lockfile(p: &str) -> bool { matches!(base_name(p), "package-lock.json" | "npm-shrinkwrap.json") }

fn base_name(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p) }
fn dependency_file(p: &str) -> bool {
    let b = base_name(p).to_ascii_lowercase();
    DEPENDENCY_FILES.contains(&b.as_str()) || (b.starts_with("requirements") && b.ends_with(".txt"))
}
fn agent_note(p: &str) -> bool { AGENT_NOTES.contains(&base_name(p)) || p.split('/').next().is_some_and(|d| AGENT_FOLDERS.contains(&d)) }

/// The changed files between two commits, each with its operation (add, delete or modify), sorted by path.
pub fn changed(repo: &Path, base: &str, head: &str) -> R<Vec<(String, &'static str)>> {
    let out = git_with(repo, &["diff", "--no-renames", "--name-status", "-z", base, head], None, 64 << 20)?;
    let fields: Vec<String> = out.split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).to_string()).collect();
    let mut list: Vec<(String, &'static str)> = fields.chunks(2).filter(|c| c.len() == 2).map(|c| {
        let op = match c[0].chars().next() { Some('A') => "add", Some('D') => "delete", _ => "modify" };
        (c[1].clone(), op)
    }).collect();
    list.sort();
    Ok(list)
}

/// A commit's files as the checks read them: every path, and the text of the ones asked for (up to 1 MiB each).
fn files_for(repo: &Path, commit: &str, wanted: &BTreeSet<String>) -> R<Files> {
    let items = crate::drive::listing(repo, commit)?;
    let mut files: Files = items.iter().map(|i| (i.path.clone(), Entry { mode: i.mode.clone(), oid: i.oid.clone(), size: i.size, bytes: None })).collect();
    let load: Vec<&crate::drive::Item> = items.iter().filter(|i| wanted.contains(&i.path) && (i.size <= LARGEST || (npm_lockfile(&i.path) && i.size <= LARGEST_LOCKFILE))
        && matches!(i.mode.as_str(), "100644" | "100755")).collect();
    crate::drive::blobs(repo, &load, |item, bytes| { if let Some(e) = files.get_mut(&item.path) { e.bytes = Some(bytes.to_vec()); } Ok(()) })?;
    Ok(files)
}

/// What the capability check refused, as `{ path, added }` items: it lists every file it found, as `name +[a, b]` joined by `; `.
fn widened(message: &str) -> Option<Vec<Value>> {
    let list = message.strip_prefix("Patch widens module capabilities: ")?;
    let list = list.split(". If this is intended").next().unwrap_or(list);
    Some(list.split("; ").filter_map(|item| {
        let (path, caps) = item.rsplit_once(" +[")?;
        Some(json!({ "path": path, "added": caps.trim_end_matches(']').split(", ").filter(|c| !c.is_empty()).collect::<Vec<_>>() }))
    }).collect())
}

pub fn rules(repo: &Path, base: &str, head: &str) -> R<Value> {
    let changes = changed(repo, base, head)?;
    let paths: Vec<&str> = changes.iter().map(|(p, _)| p.as_str()).collect();
    let pick = |f: fn(&str) -> bool| -> Vec<Value> { changes.iter().filter(|(p, _)| f(p)).map(|(p, op)| json!({ "path": p, "operation": op })).collect() };
    let protected = pick(|p| crate::git::protected(p) && !dependency_file(p) && !agent_note(p));
    let mut dependencies = pick(dependency_file);
    let agent_notes = pick(agent_note);
    // The files the checks read: every changed code or data file on both sides, and on the old side the code next to each new file, since a
    // new file is held to the powers its folder already had.
    let inspected: Vec<&(String, &str)> = changes.iter().filter(|(p, op)| *op != "delete" && (is_code(p) || is_data(p))).collect();
    let mut wanted: BTreeSet<String> = changes.iter().map(|(p, _)| p.clone()).collect();
    let base_listing = crate::drive::listing(repo, base)?;
    for (p, op) in &inspected {
        if *op != "add" { continue; }
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        wanted.extend(base_listing.iter().filter(|i| is_code(&i.path) && i.path.rsplit_once('/').map_or("", |(d, _)| d) == dir).map(|i| i.path.clone()));
    }
    wanted.insert("package.json".into());
    let (old, new) = (files_for(repo, base, &wanted)?, files_for(repo, head, &wanted)?);
    let (mut powers, mut tricks, mut settings, mut not_checked) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let none = BTreeMap::new();
    for (p, op) in &inspected {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        match crate::capability::check(&old, &new, &one, &none) {
            Ok(_) => {}
            Err(e) => match widened(&e.0) { Some(list) => powers.extend(list), None => not_checked.push(json!({ "path": p, "why": e.0 })) },
        }
    }
    // Test tricks: the test proof's own screen, one file at a time, with the test files known by folder and by name.
    let package = new.get("package.json").and_then(|e| e.bytes.clone());
    let mut tests = crate::suite::detect(package.as_deref()).unwrap_or(Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::Node, command: Vec::new(), edits: Edits::Checked });
    tests.paths = crate::suite::test_paths(&paths);
    for (p, op) in &changes {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        if let Err(e) = crate::proof::screen(&old, &new, &one, &tests) {
            let item = json!({ "path": p, "why": e.0 });
            if e.0.starts_with("Test runner configuration cannot change") { settings.push(item); }
            else if e.0.starts_with("A test file skips or focuses tests") || e.0.starts_with("Code outside the tests reaches into the test runner") { tricks.push(item); }
            else { not_checked.push(item); }
        }
    }
    // Dependencies: which packages each package.json moves, and lockfile entries that now point somewhere else.
    let json_of = |files: &Files, p: &str| -> Option<Value> { files.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| serde_json::from_slice::<Value>(b).ok()) };
    let mut lockfile = Vec::new();
    for d in dependencies.iter_mut() {
        let p = d["path"].as_str().unwrap_or("").to_string();
        let (a, b) = (json_of(&old, &p).unwrap_or_else(|| json!({})), json_of(&new, &p));
        if base_name(&p) == "package.json" && let Some(b) = &b {
            let moved = crate::deps::moves(&a, b);
            if !moved.is_empty() { d["packages"] = json!(moved); }
        }
        if npm_lockfile(&p) && d["operation"] != "delete" {
            match b {
                Some(b) => lockfile.extend(crate::deps::lock_surprises(&a, &b).into_iter().map(|why| json!({ "path": p, "why": why }))),
                None => not_checked.push(json!({ "path": p, "why": "Kern could not read this lockfile (not JSON, or larger than 64 MiB)" })),
            }
        }
    }
    let changed: Vec<Value> = changes.iter().map(|(p, op)| json!({ "path": p, "operation": op })).collect();
    Ok(json!({ "changed": changed, "powers": powers, "tricks": tricks, "settings": settings, "protected": protected, "dependencies": dependencies,
        "lockfile": lockfile, "agent_notes": agent_notes, "not_checked": not_checked }))
}
