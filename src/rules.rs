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
const LARGEST: u64 = 1 << 20;
const LARGEST_LOCKFILE: u64 = 64 << 20;
const SIBLINGS: usize = 200;
const LARGEST_CODE: u64 = 16 << 20;
const MANIFESTS: usize = 5000;
fn npm_lockfile(p: &str) -> bool { matches!(base_name(p), "package-lock.json" | "npm-shrinkwrap.json") }
const RISKY_BUILTINS: [&str; 15] = ["child_process", "cluster", "worker_threads", "vm", "net", "tls", "dgram", "dns", "http", "https", "http2", "inspector", "wasi", "module", "repl"];
const RISKY_PACKAGES: [&str; 46] = ["axios", "node-fetch", "cross-fetch", "isomorphic-fetch", "got", "undici", "ky", "superagent", "request", "needle", "phin", "ws",
    "socket.io-client", "execa", "cross-spawn", "shelljs", "zx", "node-pty", "vm2", "isolated-vm", "dotenv", "puppeteer", "simple-git", "ssh2", "nodemailer", "basic-ftp",
    "ofetch", "wretch", "redaxios", "gaxios", "form-data", "make-fetch-happen", "request-promise", "http-proxy", "socket.io", "mqtt", "amqplib", "kafkajs", "openai",
    "stripe", "twilio", "resend", "firebase", "firebase-admin", "googleapis", "ssh2-sftp-client"];
const RISKY_SCOPES: [&str; 12] = ["@aws-sdk/", "@google-cloud/", "@azure/", "@slack/", "@octokit/", "@sendgrid/", "@anthropic-ai/", "@supabase/", "@grpc/",
    "@mailchimp/", "@twilio/", "@vercel/blob"];
pub fn risky(capability: &str) -> bool {
    match capability.split_once(':') {
        Some(("node", m)) => RISKY_BUILTINS.contains(&m),
        Some(("package", p)) => RISKY_PACKAGES.contains(&p) || RISKY_SCOPES.iter().any(|s| p.starts_with(s)),
        Some(("global", g)) => g != "dynamic",
        _ => true,
    }
}

fn base_name(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p) }
fn joined(dir: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\\') || path.contains('*') { return None; }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for c in path.split('/') {
        match c { "" | "." => {} ".." => { parts.pop()?; } c => parts.push(c) }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}
fn dependency_file(p: &str) -> bool {
    let b = base_name(p).to_ascii_lowercase();
    DEPENDENCY_FILES.contains(&b.as_str()) || (b.starts_with("requirements") && b.ends_with(".txt"))
}
fn agent_note(p: &str) -> bool { AGENT_NOTES.contains(&base_name(p)) || p.split('/').next().is_some_and(|d| AGENT_FOLDERS.contains(&d)) }

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

fn files_for(repo: &Path, commit: &str, wanted: &BTreeSet<String>) -> R<Files> {
    let items = crate::drive::listing(repo, commit)?;
    let mut files: Files = items.iter().map(|i| (i.path.clone(), Entry { mode: i.mode.clone(), oid: i.oid.clone(), size: i.size, bytes: None })).collect();
    let load: Vec<&crate::drive::Item> = items.iter().filter(|i| wanted.contains(&i.path) && (i.size <= LARGEST || (npm_lockfile(&i.path) && i.size <= LARGEST_LOCKFILE)
        || (is_code(&i.path) && i.size <= LARGEST_CODE)) && matches!(i.mode.as_str(), "100644" | "100755")).collect();
    crate::drive::blobs(repo, &load, |item, bytes| {
        let text = if is_code(&item.path) && std::str::from_utf8(bytes).is_err() { String::from_utf8_lossy(bytes).into_owned().into_bytes() } else { bytes.to_vec() };
        if let Some(e) = files.get_mut(&item.path) { e.bytes = Some(text); }
        Ok(())
    })?;
    Ok(files)
}

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
    let inspected: Vec<&(String, &str)> = changes.iter().filter(|(p, op)| *op != "delete" && (is_code(p) || is_data(p))).collect();
    let mut wanted: BTreeSet<String> = changes.iter().map(|(p, _)| p.clone()).collect();
    let base_listing = crate::drive::listing(repo, base)?;
    let mut dirs = BTreeSet::new();
    for (p, op) in &inspected {
        if *op != "add" { continue; }
        let dir = p.rsplit_once('/').map_or("", |(d, _)| d);
        if !dirs.insert(dir.to_string()) { continue; }
        wanted.extend(base_listing.iter().filter(|i| is_code(&i.path) && i.size <= LARGEST && i.path.rsplit_once('/').map_or("", |(d, _)| d) == dir)
            .take(SIBLINGS).map(|i| i.path.clone()));
    }
    let head_listing = crate::drive::listing(repo, head)?;
    let manifests: BTreeSet<String> = base_listing.iter().chain(&head_listing).map(|i| i.path.as_str())
        .filter(|p| base_name(p) == "package.json" && !p.split('/').any(|c| c == "node_modules")).take(MANIFESTS).map(str::to_string).collect();
    wanted.extend(manifests.iter().cloned());
    let (old, new) = (files_for(repo, base, &wanted)?, files_for(repo, head, &wanted)?);
    let (mut powers, mut tricks, mut settings, mut not_checked) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let none = BTreeMap::new();
    let test_files = crate::suite::test_paths(&paths);
    let is_test = |p: &str| test_files.iter().any(|t| if t.ends_with('/') { p.starts_with(t.as_str()) } else { p == t });
    for (p, op) in &inspected {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        let test = is_test(p);
        match crate::capability::check(&old, &new, &one, &none) {
            Ok(_) => {}
            Err(e) => match widened(&e.0) {
                Some(list) => powers.extend(list.into_iter().filter_map(|mut x| {
                    let added: Vec<Value> = x["added"].as_array().into_iter().flatten().filter(|c| c.as_str().is_some_and(|c| risky(c) && !(test && c == "process:env"))).cloned().collect();
                    if added.is_empty() { return None; }
                    x["added"] = json!(added);
                    Some(x)
                })),
                None => not_checked.push(json!({ "path": p, "why": e.0 })),
            },
        }
    }
    let package = new.get("package.json").and_then(|e| e.bytes.clone());
    let mut tests = match crate::suite::plan(package.as_deref()) {
        crate::suite::Plan::Run(t, _) => t,
        _ => Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::Node, command: Vec::new(), edits: Edits::Checked },
    };
    tests.paths = crate::suite::test_paths(&paths);
    for (p, op) in &changes {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        if let Err(e) = crate::proof::screen(&old, &new, &one, &tests) {
            let item = json!({ "path": p, "operation": op, "why": e.0 });
            if e.0.starts_with("Test runner configuration cannot change") { settings.push(item); }
            else if e.0.starts_with("A test file skips, focuses or inverts tests") || e.0.starts_with("Code outside the tests reaches into the test runner") { tricks.push(item); }
            else { not_checked.push(item); }
        }
    }
    let deleted_tests: Vec<Value> = changes.iter().filter(|(p, op)| *op == "delete" && is_test(p)).map(|(p, _)| json!({ "path": p })).collect();
    let json_of = |files: &Files, p: &str| -> Option<Value> { files.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| serde_json::from_slice::<Value>(b).ok()) };
    for (p, op) in changes.iter().filter(|(p, op)| base_name(p) == "package.json" && *op != "delete") {
        if let (Some(a), Some(b)) = (json_of(&old, p), json_of(&new, p))
            && (crate::script::found(&a) != crate::script::found(&b) || ["jest", "mocha", "ava", "c8", "nyc"].iter().any(|k| a[*k] != b[*k])) {
            settings.push(json!({ "path": p, "operation": op, "why": "the test script or the test settings in package.json change" }));
        }
    }
    let mut named = BTreeSet::new();
    for m in &manifests {
        let dir = m.strip_suffix("package.json").unwrap_or("");
        for files in [&old, &new] {
            if let Some(v) = json_of(files, m) && let crate::script::Found::Run(i) = crate::script::found(&v) {
                named.extend(crate::script::files(&i).iter().filter_map(|f| joined(dir, f)));
            }
        }
    }
    for (p, op) in &changes {
        if named.contains(p) && !settings.iter().any(|s| s["path"] == p.as_str()) {
            settings.push(json!({ "path": p, "operation": op, "why": "the test script names this file as the test runner's config or setup" }));
        }
    }
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
        "lockfile": lockfile, "agent_notes": agent_notes, "deleted_tests": deleted_tests, "not_checked": not_checked }))
}
