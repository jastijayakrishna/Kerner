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
const LOCK_MOVES_COUNT_MAX: usize = 200;
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
        Some(("bun" | "deno", m)) => !["argv", "args", "version", "revision", "main", "mainModule", "nanoseconds", "inspect", "deepEquals", "escapeHTML", "stringWidth", "sleep", "sleepSync", "build", "noColor"].contains(&m),
        _ => true,
    }
}

fn base_name(p: &str) -> &str { p.rsplit('/').next().unwrap_or(p) }
const DEPENDENCY_SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];
fn package_fields(a: &Value, b: &Value) -> Vec<String> {
    let empty = serde_json::Map::new();
    let (a, b) = (a.as_object().unwrap_or(&empty), b.as_object().unwrap_or(&empty));
    let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let mut out = Vec::new();
    for k in keys.into_iter().filter(|k| !DEPENDENCY_SECTIONS.contains(&k.as_str()) && a.get(*k) != b.get(*k)) {
        match (a.get(k).and_then(Value::as_object), b.get(k).and_then(Value::as_object)) {
            (x, y) if k == "scripts" && (x.is_some() || y.is_some()) => {
                let (x, y) = (x.unwrap_or(&empty), y.unwrap_or(&empty));
                let names: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                out.extend(names.into_iter().filter(|n| x.get(*n) != y.get(*n)).map(|n| format!("scripts.{n}")));
            }
            _ => out.push(k.clone()),
        }
    }
    out
}
fn joined(dir: &str, path: &str) -> Option<String> { if path.contains('*') { None } else { under(dir, path).filter(|p| !p.is_empty()) } }
fn dependency_file(p: &str) -> bool {
    let b = base_name(p).to_ascii_lowercase();
    DEPENDENCY_FILES.contains(&b.as_str()) || (b.starts_with("requirements") && b.ends_with(".txt"))
}
fn harmless_note(p: &str) -> bool {
    let (lower, base) = (p.to_ascii_lowercase(), base_name(p).to_ascii_lowercase());
    [".md", ".mdx", ".txt"].iter().any(|e| base.ends_with(e)) || [".env.example", ".env.sample", ".env.template", ".env.dist"].contains(&base.as_str())
        || lower.starts_with(".github/issue_template/")
}
const INSTALL_FIELDS: [&str; 14] = ["bin", "name", "overrides", "resolutions", "pnpm", "workspaces", "packageManager", "publishConfig", "bundleDependencies", "bundledDependencies",
    "dependenciesMeta", "peerDependenciesMeta", "trustedDependencies", "installConfig"];
const LIFECYCLE: [&str; 16] = ["preinstall", "install", "postinstall", "preprepare", "prepare", "postprepare", "prepublish", "prepublishOnly", "publish", "postpublish", "prepack", "postpack",
    "dependencies", "preuninstall", "uninstall", "postuninstall"];
fn steers_install(field: &str) -> bool { field.strip_prefix("scripts.").map_or_else(|| INSTALL_FIELDS.contains(&field), |s| LIFECYCLE.contains(&s)) }
const IMAGES: [&str; 8] = ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "avif"];
const GITHUB_NOTES: [&str; 3] = ["funding.yml", "funding.yaml", "release.yml"];
const RUNS_CODE_KEYS: [&str; 16] = ["NODE_OPTIONS", "NODE_PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "DYLD_INSERT_LIBRARIES", "PATH", "BASH_ENV", "ENV", "PYTHONSTARTUP", "PYTHONPATH",
    "PERL5OPT", "RUBYOPT", "JAVA_TOOL_OPTIONS", "GIT_SSH_COMMAND", "SHELL", "EDITOR"];
const TOOL_PREFIXES: [&str; 4] = ["NPM_CONFIG_", "YARN_", "PNPM_", "COREPACK_"];
fn env_keys(text: &str) -> Option<Vec<String>> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| {
        let (key, _) = l.strip_prefix("export ").unwrap_or(l).split_once('=')?;
        let key = key.trim();
        (!key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) && !key.starts_with(|c: char| c.is_ascii_digit())).then(|| key.to_ascii_uppercase())
    }).collect()
}
fn quiet(p: &str, text: Option<&str>) -> bool {
    let lower = p.to_ascii_lowercase();
    let base = base_name(&lower);
    if lower.split('/').any(|d| d == ".github") && (GITHUB_NOTES.contains(&base) || base.rsplit_once('.').is_some_and(|(_, e)| IMAGES.contains(&e))) { return true; }
    let harmless = |k: &String| !crate::capability::secret_name(k) && !RUNS_CODE_KEYS.contains(&k.as_str()) && !TOOL_PREFIXES.iter().any(|t| k.starts_with(t));
    (base == ".env" || base.starts_with(".env.")) && text.and_then(env_keys).is_some_and(|keys| keys.iter().all(harmless))
}
fn agent_note(p: &str) -> bool {
    AGENT_NOTES.contains(&base_name(p)) || p.split('/').next().is_some_and(|d| AGENT_FOLDERS.contains(&d)) || [".github/instructions/", ".github/prompts/", ".github/chatmodes/"].iter().any(|d| p.starts_with(d))
}

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

pub fn renames(repo: &Path, base: &str, head: &str) -> R<Vec<(String, String, bool)>> {
    let out = git_with(repo, &["diff", "-M", "--diff-filter=R", "--name-status", "-z", base, head], None, 64 << 20)?;
    let fields: Vec<String> = out.split(|b| *b == 0).filter(|f| !f.is_empty()).map(|f| String::from_utf8_lossy(f).to_string()).collect();
    Ok(fields.chunks(3).filter(|c| c.len() == 3 && c[0].starts_with('R')).map(|c| (c[1].clone(), c[2].clone(), c[0] == "R100")).collect())
}
const RUNNER_KEYS: [&str; 15] = ["setupFilesAfterEnv", "setupFiles", "globalSetup", "globalTeardown", "testEnvironment", "environment", "snapshotResolver", "testRunner",
    "testSequencer", "sequencer", "resolver", "transform", "reporters", "snapshotSerializers", "moduleNameMapper"];
fn values_of(text: &str, key: &str) -> Vec<String> {
    let (mut out, mut from) = (Vec::new(), 0);
    while let Some(i) = text[from..].find(key) {
        let at = from + i + key.len();
        from = at;
        let rest = text[at..].trim_start_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let rest = rest.trim_start();
        let end = match rest.chars().next() { Some('[') => rest.find(']').map(|e| e + 1), Some('{') => rest.find('}').map(|e| e + 1), _ => rest.find([',', '}', '\n']) };
        out.extend(quoted(&rest[..end.unwrap_or(rest.len())]));
    }
    out
}
fn leaves(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| leaves(x, out)),
        Value::Object(o) => o.values().for_each(|x| leaves(x, out)),
        _ => {}
    }
}
fn under(dir: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\\') { return None; }
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for c in path.split('/') {
        match c { "" | "." => {} ".." => { parts.pop()?; } c => parts.push(c) }
    }
    Some(parts.join("/"))
}
fn runner_path(root: &str, f: &str) -> Option<String> { joined(root, f.strip_prefix("<rootDir>").map_or(f, |r| r.trim_start_matches('/'))) }
fn runner_config(p: &str) -> bool {
    let n = base_name(p).to_ascii_lowercase();
    ["vitest.", "vite.", "jest."].iter().any(|s| n.starts_with(s)) && [".config.", ".workspace."].iter().any(|m| n.contains(m)) && !p.split('/').any(|c| c == "node_modules")
}
fn mentioned(named: &BTreeSet<String>, p: &str) -> bool {
    if named.contains(p) { return true; }
    let Some((stem, _)) = p.rsplit_once('.').filter(|_| is_code(p)) else { return false };
    named.contains(stem) || stem.strip_suffix("/index").is_some_and(|d| named.contains(d))
}
const SCRIPT_RUNNERS: [&str; 7] = ["node", "nodejs", "zx", "bun", "deno", "tsx", "ts-node"];
fn quoted(text: &str) -> Vec<String> {
    let (chars, mut out, mut i) = (text.chars().collect::<Vec<_>>(), Vec::new(), 0);
    while i < chars.len() {
        let q = chars[i];
        if q == '\'' || q == '"' || q == '`' {
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != q { i += 1; }
            out.push(chars[start..i.min(chars.len())].iter().collect());
        }
        i += 1;
    }
    out
}

fn files_from(items: &[crate::drive::Item]) -> Files {
    items.iter().map(|i| (i.path.clone(), Entry { mode: i.mode.clone(), oid: i.oid.clone(), size: i.size, bytes: None })).collect()
}
fn fill(repo: &Path, files: &mut Files, items: &[crate::drive::Item], wanted: &BTreeSet<String>) -> R<()> {
    let load: Vec<&crate::drive::Item> = items.iter().filter(|i| wanted.contains(&i.path) && files.get(&i.path).is_some_and(|e| e.bytes.is_none())
        && (i.size <= LARGEST || (npm_lockfile(&i.path) && i.size <= LARGEST_LOCKFILE) || (is_code(&i.path) && i.size <= LARGEST_CODE)) && matches!(i.mode.as_str(), "100644" | "100755")).collect();
    crate::drive::blobs(repo, &load, |item, bytes| {
        let text = if is_code(&item.path) && std::str::from_utf8(bytes).is_err() { String::from_utf8_lossy(bytes).into_owned().into_bytes() } else { bytes.to_vec() };
        if let Some(e) = files.get_mut(&item.path) { e.bytes = Some(text); }
        Ok(())
    })
}

fn runner_at(dir: &str, old: &Files, new: &Files, head: &BTreeSet<&str>) -> Option<(String, Runner)> {
    let parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for n in (0..=parts.len()).rev() {
        let d = parts[..n].join("/");
        let at = |f: &str| if d.is_empty() { f.to_string() } else { format!("{d}/{f}") };
        let Some(bytes) = new.get(&at("package.json")).or_else(|| old.get(&at("package.json"))).and_then(|e| e.bytes.as_deref()) else { continue };
        match crate::suite::plan(Some(bytes)) {
            crate::suite::Plan::Run(t, _) => return Some((d, t.runner)),
            crate::suite::Plan::Packages => return None,
            crate::suite::Plan::Nothing => { if let Some((t, _)) = crate::suite::by_config(|f| head.contains(at(f).as_str())) { return Some((d, t.runner)); } }
        }
    }
    None
}
fn runner_reads(p: &str, old: &Files, new: &Files, head: &BTreeSet<&str>) -> bool {
    let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
    let Some((root, runner)) = runner_at(dir, old, new, head) else { return true };
    if !crate::proof::read_by(name, runner) { return false; }
    if !name.to_ascii_lowercase().starts_with("vite.") { return true; }
    let beside: Vec<&&str> = head.iter().filter(|q| q.rsplit_once('/').map_or("", |(d, _)| d) == dir && base_name(q).to_ascii_lowercase().starts_with("vitest.config.")).collect();
    root == dir && (beside.is_empty() || beside.iter().any(|q| new.get(**q).and_then(|e| e.bytes.as_deref()).is_some_and(|b| String::from_utf8_lossy(b).contains("vite.config"))))
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
    let protected = pick(|p| crate::git::protected(p) && !dependency_file(p) && !agent_note(p) && !harmless_note(p));
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
    let mut configs: BTreeSet<String> = base_listing.iter().chain(&head_listing).map(|i| i.path.clone()).filter(|p| runner_config(p)).take(MANIFESTS).collect();
    wanted.extend(configs.iter().cloned());
    let moved: Vec<(String, String, bool)> = renames(repo, base, head)?.into_iter().filter(|(from, to, _)| is_code(from) == is_code(to)).collect();
    let (mut old, mut new) = (files_from(&base_listing), files_from(&head_listing));
    fill(repo, &mut old, &base_listing, &wanted)?;
    fill(repo, &mut new, &head_listing, &wanted)?;
    let json_of = |files: &Files, p: &str| -> Option<Value> { files.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| serde_json::from_slice::<Value>(b).ok()) };
    let mut named = BTreeSet::new();
    for m in &manifests {
        let dir = m.strip_suffix("package.json").unwrap_or("");
        for files in [&old, &new] {
            if let Some(v) = json_of(files, m) && let crate::script::Found::Run(i) = crate::script::found(&v) { named.extend(crate::script::files(&i).iter().filter_map(|f| joined(dir, f))); }
        }
    }
    let scripted: BTreeSet<String> = named.iter().filter(|p| !wanted.contains(*p) && (is_code(p) || p.ends_with(".json"))).cloned().collect();
    fill(repo, &mut old, &base_listing, &scripted)?;
    fill(repo, &mut new, &head_listing, &scripted)?;
    configs.extend(named.iter().filter(|p| is_code(p) || p.ends_with(".json")).cloned());
    let (mut moved_to, mut based) = (BTreeSet::new(), BTreeSet::new());
    for (from, to, _) in &moved {
        if crate::suite::is_test_path(from) == crate::suite::is_test_path(to) && let Some(e) = old.get(from).cloned() && !old.contains_key(to) {
            old.insert(to.clone(), e);
            moved_to.insert(to.clone());
            based.insert(from.clone());
        }
    }
    let moved_from: BTreeSet<&String> = moved.iter().map(|(from, _, _)| from).collect();
    for m in &manifests {
        let dir = m.strip_suffix("package.json").unwrap_or("");
        for files in [&old, &new] {
            let Some(v) = json_of(files, m) else { continue };
            let root = v["jest"]["rootDir"].as_str().and_then(|r| under(dir, r)).unwrap_or_else(|| dir.trim_end_matches('/').to_string());
            let mut found = Vec::new();
            for key in RUNNER_KEYS { leaves(&v["jest"][key], &mut found); }
            named.extend(found.iter().filter_map(|f| runner_path(&root, f)));
        }
    }
    for c in &configs {
        let dir = c.rsplit_once('/').map_or("", |(d, _)| d);
        for files in [&old, &new] {
            let Some(text) = files.get(c).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()) else { continue };
            let root = values_of(text, "rootDir").first().and_then(|r| under(dir, r)).unwrap_or_else(|| dir.to_string());
            let found: Vec<String> = RUNNER_KEYS.iter().flat_map(|k| values_of(text, k)).collect();
            named.extend(found.iter().filter_map(|f| runner_path(&root, f)));
        }
    }
    let (mut powers, mut tricks, mut settings, mut not_checked) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut twins: Vec<(String, &str, String)> = Vec::new();
    for (p, op) in changes.iter().filter(|(p, op)| *op != "delete" && !is_code(p) && !is_data(p) && !p.split('/').any(|c| c == "node_modules")) {
        let Some(line) = new.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| b.split(|c| *c == b'\n').next()).and_then(|l| std::str::from_utf8(l).ok()).and_then(|l| l.strip_prefix("#!")) else { continue };
        match line.split_whitespace().map(|w| w.rsplit('/').next().unwrap_or(w)).find(|w| SCRIPT_RUNNERS.contains(w)) {
            Some(r) => {
                let twin = format!("{p}.kern.{}", if ["node", "nodejs", "zx"].contains(&r) { "js" } else { "ts" });
                for files in [&mut old, &mut new] { if let Some(e) = files.get(p).cloned() { files.insert(twin.clone(), e); } }
                twins.push((twin, op, p.clone()));
            }
            None => not_checked.push(json!({ "path": p, "why": format!("a script run by {}, which Kern does not read", line.trim()), "code": "other" })),
        }
    }
    let none = BTreeMap::new();
    let test_files = crate::suite::test_paths(&paths);
    let is_test = |p: &str| test_files.iter().any(|t| if t.ends_with('/') { p.starts_with(t.as_str()) } else { p == t });
    let checks: Vec<(String, &str, String)> = inspected.iter().map(|(p, op)| (p.clone(), *op, p.clone())).chain(twins).collect();
    let source = |files: &Files, p: &str| files.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_string);
    for (p, op, real) in &checks {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        let test = is_test(real);
        let readable = |s: &String| is_code(p) && crate::capability::footprint(p, s).is_ok();
        let after = source(&new, p).filter(readable).map(|s| crate::capability::env_names(p, &s));
        let settings_only = after.as_ref().is_some_and(|a| a.as_ref().is_some_and(|names| !names.is_empty() && names.iter().all(|n| !crate::capability::secret_name(n))));
        match crate::capability::check(&old, &new, &one, &none) {
            Ok(_) => {}
            Err(e) => match widened(&e.0) {
                Some(list) => powers.extend(list.into_iter().filter_map(|mut x| {
                    let added: Vec<Value> = x["added"].as_array().into_iter().flatten()
                        .filter(|c| c.as_str().is_some_and(|c| risky(c) && !(c == "process:env" && (test || settings_only)))).cloned().collect();
                    if added.is_empty() { return None; }
                    x["added"] = json!(added);
                    x["path"] = json!(real);
                    Some(x)
                })),
                None => not_checked.push(if p == real { json!({ "path": real, "why": e.0 }) } else { json!({ "path": real, "why": e.0, "code": "js" }) }),
            },
        }
        if *op == "delete" { continue; }
        let before = match source(&old, p) { Some(s) if readable(&s) => crate::capability::env_names(p, &s), Some(_) => None, None => Some(BTreeSet::new()) };
        let fresh: Vec<String> = match (before, after) {
            (Some(_), Some(None)) => vec!["env:*".to_string()],
            (Some(b), Some(Some(a))) => a.iter().filter(|n| crate::capability::secret_name(n) && !b.contains(*n)).map(|n| format!("env:{n}")).collect(),
            _ => Vec::new(),
        };
        if fresh.is_empty() { continue; }
        match powers.iter_mut().find(|x| x["path"] == real.as_str()) {
            Some(x) if x["added"].as_array().is_some_and(|a| a.iter().any(|c| c == "process:env")) => {}
            Some(x) => { if let Some(a) = x["added"].as_array_mut() { a.extend(fresh.into_iter().map(Value::from)); } }
            None => powers.push(json!({ "path": real, "added": fresh })),
        }
    }
    for x in powers.iter_mut().filter(|x| x["path"].as_str().is_some_and(is_test)) {
        let path = x["path"].as_str().unwrap_or("").to_string();
        let Some(text) = source(&new, &path) else { continue };
        let mut has: BTreeSet<String> = crate::capability::footprint(&path, &text).map(|caps| caps.into_iter().filter(|c| risky(c) && c != "process:env").collect()).unwrap_or_default();
        match crate::capability::env_names(&path, &text) {
            Some(names) => has.extend(names.into_iter().filter(|n| crate::capability::secret_name(n)).map(|n| format!("env:{n}"))),
            None => { has.insert("env:*".to_string()); }
        }
        x["has"] = json!(has);
    }
    let package = new.get("package.json").and_then(|e| e.bytes.clone());
    let mut tests = match crate::suite::plan(package.as_deref()) {
        crate::suite::Plan::Run(t, _) => t,
        _ => Tests { paths: Vec::new(), kind: Kind::Feature, runner: Runner::Node, command: Vec::new(), edits: Edits::Checked },
    };
    tests.paths = crate::suite::test_paths(&paths);
    let mut pool = BTreeMap::new();
    for (p, _) in changes.iter().filter(|(p, op)| *op == "delete" && is_code(p) && crate::suite::is_test_path(p) && !based.contains(p)) {
        if let Some(text) = old.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()) {
            for (m, n) in crate::proof::test_markers(p, text) { *pool.entry(m).or_insert(0) += n; }
        }
    }
    let head_paths: BTreeSet<&str> = head_listing.iter().map(|i| i.path.as_str()).collect();
    let reads = |p: &str| runner_reads(p, &old, &new, &head_paths);
    for (p, op) in &changes {
        let one = json!({ "files": [{ "path": p, "operation": op }] });
        if let Err(e) = crate::proof::screen_with(&old, &new, &one, &tests, &pool, true) {
            let item = json!({ "path": p, "operation": if moved_to.contains(p) { "modify" } else { op }, "why": e.0 });
            let config = e.0.starts_with("Test runner configuration cannot change");
            if config && !mentioned(&named, p) && !reads(p) { continue; }
            if config || (mentioned(&named, p) && e.0.starts_with("Code outside the tests reaches into the test runner")) { settings.push(item); }
            else if e.0.starts_with("A test file skips, focuses or inverts tests") || e.0.starts_with("Code outside the tests reaches into the test runner") { tricks.push(item); }
            else { not_checked.push(item); }
        }
    }
    let text = |files: &Files, p: &str| -> Option<String> { files.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok()).map(str::to_string) };
    let defined_tests = |p: &str| text(&old, p).and_then(|s| crate::weaken::cases(p, &s)).is_none_or(|c| !c.is_empty());
    let deleted_tests: Vec<Value> = changes.iter().filter(|(p, op)| *op == "delete" && is_test(p) && !moved_from.contains(p) && defined_tests(p)).map(|(p, _)| json!({ "path": p })).collect();
    let now: BTreeMap<&String, BTreeMap<String, crate::weaken::Case>> = changes.iter().filter(|(p, op)| *op != "delete" && is_test(p) && is_code(p))
        .filter_map(|(p, _)| text(&new, p).and_then(|t| crate::weaken::cases(p, &t)).map(|c| (p, c))).collect();
    let mut weakened = Vec::new();
    for (p, after) in &now {
        let Some(before) = text(&old, p).and_then(|t| crate::weaken::cases(p, &t)) else { continue };
        let elsewhere: BTreeSet<String> = now.iter().filter(|(q, _)| q != &p).flat_map(|(_, c)| c.keys().map(|k| crate::weaken::leaf(k).to_string())).collect();
        let w = crate::weaken::compare(&before, after, &elsewhere);
        if !w.is_empty() { weakened.push(json!({ "path": p, "removed": w.removed, "missing": w.missing, "emptied": w.emptied, "loosened": w.loosened })); }
    }
    let engines = |p: &str| -> Option<(String, crate::nodeapi::Version)> {
        let parts: Vec<&str> = p.split('/').collect();
        (0..parts.len()).rev().map(|n| parts[..n].join("/")).find_map(|d| {
            let manifest = if d.is_empty() { "package.json".to_string() } else { format!("{d}/package.json") };
            json_of(&new, &manifest).and_then(|v| v["engines"]["node"].as_str().map(str::to_string))
        }).and_then(|range| crate::nodeapi::lowest(&range).map(|low| (range, low)))
    };
    let mut node_versions = Vec::new();
    for (p, _) in changes.iter().filter(|(p, op)| *op != "delete" && is_code(p)) {
        let Some(after) = text(&new, p).and_then(|t| crate::nodeapi::uses(p, &t)) else { continue };
        let before = text(&old, p).and_then(|t| crate::nodeapi::uses(p, &t)).unwrap_or_default();
        let fresh: Vec<&String> = after.difference(&before).collect();
        if fresh.is_empty() { continue; }
        let Some((range, low)) = engines(p) else { continue };
        let features: Vec<Value> = fresh.iter().filter_map(|k| crate::nodeapi::needs(k, low).map(|since| json!({ "name": k.split_once(':').map_or(k.as_str(), |(_, f)| f), "since": since }))).collect();
        if !features.is_empty() { node_versions.push(json!({ "path": p, "supports": range, "features": features })); }
    }
    let live: Vec<&String> = changes.iter().filter(|(p, op)| *op != "delete" && is_code(p) && !is_test(p)).map(|(p, _)| p).collect();
    let policies: Vec<(String, String)> = live.iter().flat_map(|p| text(&new, p).map(|t| crate::csp::policies(p, &t)).unwrap_or_default().into_iter().map(move |x| (p.to_string(), x))).collect();
    let mut inline_csp = Vec::new();
    for p in live.iter().filter(|_| !policies.is_empty()) {
        let Some(after) = text(&new, p).map(|t| crate::csp::inline(p, &t)) else { continue };
        let before = text(&old, p).map(|t| crate::csp::inline(p, &t)).unwrap_or_default();
        for (kind, n) in &after {
            if *n <= before.get(kind).copied().unwrap_or(0) { continue; }
            if let Some((q, policy)) = policies.iter().find(|(_, x)| crate::csp::blocks(x, kind)) {
                inline_csp.push(json!({ "path": p, "kind": kind, "policy": policy, "policy_path": q }));
            }
        }
    }
    let shown = |commit: &str, p: &str| -> Option<String> { crate::git::git(repo, &["show", &format!("{commit}:{p}")], None).ok().and_then(|b| String::from_utf8(b).ok()) };
    let mut public_api = Vec::new();
    let mut packages_touched = BTreeSet::new();
    for (p, _) in changes.iter().filter(|(p, _)| is_code(p)) {
        let parts: Vec<&str> = p.split('/').collect();
        if let Some(d) = (0..parts.len()).rev().map(|n| parts[..n].join("/")).find(|d| manifests.contains(&if d.is_empty() { "package.json".to_string() } else { format!("{d}/package.json") })) {
            packages_touched.insert(d);
        }
    }
    for dir in packages_touched.iter().take(10) {
        let manifest = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
        let (Some(before), Some(_)) = (json_of(&old, &manifest), json_of(&new, &manifest)) else { continue };
        let Some(name) = before["name"].as_str().filter(|_| before["private"] != json!(true)) else { continue };
        let joined = |e: &str| if dir.is_empty() { e.to_string() } else { format!("{dir}/{e}") };
        let read_old = |f: &str| shown(base, f);
        let read_new = |f: &str| shown(head, f);
        let mut seen = BTreeSet::new();
        for entry in crate::surface::entries(&before).iter().map(|e| joined(e)).filter(|e| is_code(e)) {
            let Some(was) = crate::surface::surface(&read_old, &entry) else { continue };
            if !seen.insert(was.clone()) { continue; }
            let Some(now) = crate::surface::surface(&read_new, &entry) else { continue };
            let removed: Vec<&String> = was.difference(&now).collect();
            if !removed.is_empty() { public_api.push(json!({ "package": name, "entry": entry, "removed": removed })); }
        }
    }
    for (p, op) in changes.iter().filter(|(p, op)| base_name(p) == "package.json" && *op != "delete") {
        if let (Some(a), Some(b)) = (json_of(&old, p), json_of(&new, p))
            && (!crate::script::same_tests(&a, &b) || ["jest", "mocha", "ava", "c8", "nyc"].iter().any(|k| a[*k] != b[*k])) {
            settings.push(json!({ "path": p, "operation": op, "why": "the test script or the test settings in package.json change" }));
        }
    }
    for (p, op) in &changes {
        if mentioned(&named, p) && !settings.iter().any(|s| s["path"] == p.as_str()) {
            settings.push(json!({ "path": p, "operation": op, "why": "the test script names this file as the test runner's config or setup" }));
        }
    }
    let mut lockfile = Vec::new();
    for d in dependencies.iter_mut() {
        let p = d["path"].as_str().unwrap_or("").to_string();
        let (a, b) = (json_of(&old, &p).unwrap_or_else(|| json!({})), json_of(&new, &p));
        if base_name(&p) == "package.json" && let Some(b) = &b {
            let moved = crate::deps::moves(&a, b);
            if !moved.is_empty() { d["packages"] = json!(moved); d["moves"] = json!(crate::deps::changed_versions(&a, b)); }
            let fields: Vec<String> = package_fields(&a, b).into_iter().filter(|f| steers_install(f)).collect();
            if !fields.is_empty() { d["fields"] = json!(fields); }
            if moved.is_empty() && fields.is_empty() && a.is_object() && d["operation"] == "modify" { d["harmless"] = json!(true); }
        }
        if base_name(&p) == "package.json" && d["operation"] == "delete" {
            d["harmless"] = json!(true);
        }
        if npm_lockfile(&p) && d["operation"] != "delete" {
            match b {
                Some(b) => {
                    lockfile.extend(crate::deps::lock_surprises(&a, &b).into_iter().map(|why| json!({ "path": p, "why": why })));
                    let moves = crate::deps::lock_moves(&a, &b);
                    if moves.len() > LOCK_MOVES_COUNT_MAX {
                        not_checked.push(json!({ "path": p, "why": format!("{} versions move in this lockfile, more than the {LOCK_MOVES_COUNT_MAX} whose release dates Kern checks", moves.len()) }));
                    }
                    if moves.is_empty() { d["harmless"] = json!(true); }
                    if !moves.is_empty() && moves.len() <= LOCK_MOVES_COUNT_MAX {
                        d["packages"] = json!(moves.iter().map(crate::deps::shown).collect::<Vec<_>>());
                        d["moves"] = json!(moves);
                    }
                }
                None => not_checked.push(json!({ "path": p, "why": "Kern could not read this lockfile (not JSON, or larger than 64 MiB)" })),
            }
        }
    }
    dependencies.retain(|d| d.get("harmless").is_none());
    if dependencies.iter().any(|d| base_name(d["path"].as_str().unwrap_or("")) == "package.json" && d["packages"].as_array().is_some_and(|p| !p.is_empty())) {
        for d in dependencies.iter_mut().filter(|d| npm_lockfile(d["path"].as_str().unwrap_or(""))) { if let Some(o) = d.as_object_mut() { o.remove("packages"); } }
    }
    let mut listed = BTreeSet::new();
    not_checked.retain(|x| listed.insert(x["path"].as_str().unwrap_or("").to_string()));
    let text_now = |p: &str| new.get(p).and_then(|e| e.bytes.as_deref()).and_then(|b| std::str::from_utf8(b).ok());
    let unchanged = |x: &Value| x["path"].as_str().is_some_and(|p| (p.ends_with(".json") && json_of(&old, p).is_some_and(|a| json_of(&new, p).is_some_and(|b| a == b)))
        || (is_code(p) && text(&old, p).zip(text(&new, p)).is_some_and(|(a, b)| crate::capability::same_program(p, &a, &b))));
    let protected: Vec<Value> = protected.into_iter().filter(|x| { let p = x["path"].as_str().unwrap_or(""); !quiet(p, text_now(p)) && !unchanged(x) }).collect();
    settings.retain(|x| !unchanged(x));
    let changed: Vec<Value> = changes.iter().map(|(p, op)| json!({ "path": p, "operation": op })).collect();
    Ok(json!({ "changed": changed, "powers": powers, "tricks": tricks, "settings": settings, "protected": protected, "dependencies": dependencies,
        "lockfile": lockfile, "agent_notes": agent_notes, "deleted_tests": deleted_tests, "weakened": weakened, "node_versions": node_versions, "public_api": public_api, "inline_csp": inline_csp, "not_checked": not_checked }))
}
