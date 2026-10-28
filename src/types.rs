use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const CHECK: &str = "const{spawnSync}=require('child_process'),path=require('path');const config=process.argv[1],dir=path.dirname(config);let tsc;try{tsc=require.resolve('typescript/bin/tsc',{paths:[dir]})}catch{process.stdout.write('kern-types: no typescript\\n');process.exit(0)}const heap=Number(process.argv[2])||1024;const run=(a)=>spawnSync(process.execPath,['--max-old-space-size='+heap,tsc,...a],{cwd:dir,encoding:'utf8',maxBuffer:1<<28});let o={};try{o=JSON.parse(run(['--showConfig','-p',config]).stdout).compilerOptions||{}}catch{}const a=['--noEmit','-p',config,'--pretty','false'];if(o.incremental||o.composite)a.push('--tsBuildInfoFile','/tmp/.kern-tsbuildinfo');const r=run(a);let v='';try{v=require(path.join(path.dirname(tsc),'..','package.json')).version}catch{}process.stdout.write('kern-types: typescript '+v+'\\n'+(r.stdout||'')+'\\nkern-types: exit '+r.status+' '+(r.signal||'')+'\\n')";
const MAX_PROJECTS: usize = 2;
const RESOLUTION: [&str; 15] = ["TS2307", "TS2305", "TS2724", "TS2614", "TS2694", "TS7016", "TS2792", "TS2306", "TS2688", "TS6053", "TS2503", "TS2709",
    "TS2304", "TS2552", "TS6305"];
const LOCKS: [&str; 6] = ["package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "bun.lock", "bun.lockb"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diag { pub file: String, pub line: usize, pub code: String, pub message: String }
impl Diag {
    pub fn json(&self) -> Value { json!({ "file": self.file, "line": self.line, "code": self.code, "message": self.message }) }
}

pub fn is_ts(path: &str) -> bool { [".ts", ".tsx", ".mts", ".cts"].iter().any(|e| path.ends_with(e)) && !path.split('/').any(|s| s == "node_modules") }

pub fn jsonc(text: &str) -> Option<Value> {
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut i, mut quoted) = (String::with_capacity(text.len()), 0usize, false);
    while i < chars.len() {
        let c = chars[i];
        if quoted {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() { out.push(chars[i + 1]); i += 1; } else if c == '"' { quoted = false; }
        } else if c == '"' {
            quoted = true;
            out.push(c);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' { i += 1; }
            continue;
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) { i += 1; }
            i += 2;
            continue;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|x| !x.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) { out.push(c); }
        } else {
            out.push(c);
        }
        i += 1;
    }
    serde_json::from_str(&out).ok()
}

fn parent(path: &str) -> &str { path.rsplit_once('/').map_or("", |(d, _)| d) }
fn joined(dir: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for p in rel.split('/') {
        match p { "" | "." => {} ".." => { parts.pop(); } p => parts.push(p) }
    }
    parts.join("/")
}

pub fn projects(changed: &[&str], head: &BTreeSet<String>, base: &BTreeSet<String>, read: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for f in changed.iter().filter(|f| is_ts(f)) {
        let mut dir = parent(f);
        loop {
            let candidate = joined(dir, "tsconfig.json");
            if head.contains(&candidate) { *count.entry(candidate).or_default() += 1; break; }
            if dir.is_empty() { break; }
            dir = parent(dir);
        }
    }
    let mut ranked: Vec<(String, usize)> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut out: Vec<String> = Vec::new();
    for (config, _) in ranked {
        let expanded = match read(&config).and_then(|t| jsonc(&t)) {
            Some(v) if v["files"].as_array().is_some_and(Vec::is_empty) => v["references"].as_array().into_iter().flatten().filter_map(|r| r["path"].as_str()).map(|p| {
                let p = joined(parent(&config), p);
                if p.ends_with(".json") { p } else { joined(&p, "tsconfig.json") }
            }).filter(|p| head.contains(p)).collect(),
            _ => vec![config],
        };
        for c in expanded { if base.contains(&c) && !out.contains(&c) && out.len() < MAX_PROJECTS { out.push(c); } }
    }
    out
}

pub fn enforced(scripts: &[Value]) -> bool {
    scripts.iter().flat_map(|s| s.as_object().into_iter().flatten()).filter_map(|(_, v)| v.as_str()).any(|s| {
        s.split(|c: char| c.is_whitespace() || "&|;()'\"".contains(c)).any(|t| {
            let name = t.rsplit('/').next().unwrap_or(t);
            matches!(name, "tsc" | "vue-tsc" | "tsgo")
        })
    })
}

fn parse(line: &str) -> Option<Diag> {
    let (head, rest) = match line.split_once(": error TS") { Some(x) => x, None => ("", line.strip_prefix("error TS")?) };
    let (code, message) = rest.split_once(": ")?;
    if code.is_empty() || !code.bytes().all(|b| b.is_ascii_digit()) { return None; }
    let (file, at) = if head.is_empty() { (String::new(), 0) } else {
        let open = head.rfind('(')?;
        let inner = head[open + 1..].strip_suffix(')')?;
        (head[..open].to_string(), inner.split(',').next()?.parse().ok()?)
    };
    Some(Diag { file, line: at, code: format!("TS{code}"), message: message.trim().to_string() })
}

pub fn diagnostics(output: &str, dir: &str) -> Vec<Diag> {
    output.lines().filter(|l| !l.starts_with(' ') && !l.starts_with('\t')).filter_map(|l| parse(l.trim_end()))
        .map(|d| Diag { file: if d.file.is_empty() { d.file } else { joined(dir, &d.file) }, ..d }).collect()
}

pub fn side(output: &str, dir: &str) -> Value {
    if output.lines().any(|l| l.trim() == "kern-types: no typescript") { return json!({ "status": "unknown", "why": "TypeScript is not installed here" }); }
    let version = output.lines().find_map(|l| l.strip_prefix("kern-types: typescript ")).map(str::trim).unwrap_or("").to_string();
    let Some(exit) = output.lines().rev().find_map(|l| l.strip_prefix("kern-types: exit ")) else {
        return json!({ "status": "unknown", "why": "tsc did not finish" });
    };
    let status = exit.split_whitespace().next().and_then(|s| s.parse::<i64>().ok());
    let found = diagnostics(output, dir);
    match status {
        Some(0) if found.is_empty() => json!({ "status": "clean", "version": version }),
        Some(_) if !found.is_empty() => json!({ "status": "errors", "version": version, "errors": found.len(), "list": found.iter().map(Diag::json).collect::<Vec<_>>() }),
        Some(code) => json!({ "status": "unknown", "why": format!("tsc ended with exit code {code} and reported no error Kern could read") }),
        None => json!({ "status": "unknown", "why": format!("tsc was stopped ({})", exit.split_whitespace().nth(1).unwrap_or("by a signal")) }),
    }
}

fn listed(v: &Value) -> Vec<Diag> {
    v["list"].as_array().into_iter().flatten().map(|d| Diag {
        file: d["file"].as_str().unwrap_or("").into(), line: d["line"].as_u64().unwrap_or(0) as usize, code: d["code"].as_str().unwrap_or("").into(), message: d["message"].as_str().unwrap_or("").into(),
    }).collect()
}

pub fn added(old: &[Diag], new: &[Diag]) -> Vec<Diag> {
    let mut seen: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    for d in old { *seen.entry((d.file.as_str(), d.code.as_str(), d.message.as_str())).or_default() += 1; }
    new.iter().filter(|d| match seen.get_mut(&(d.file.as_str(), d.code.as_str(), d.message.as_str())) {
        Some(n) if *n > 0 => { *n -= 1; false }
        _ => true,
    }).cloned().collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict { Blocked, Look(String), Unverified(String) }
pub fn verdict(p: &Value) -> Verdict {
    if let Some(why) = p["uneven"].as_str() { return Verdict::Unverified(format!("Kern cannot compare the two sides fairly: {}", plain(why, 400))); }
    if p["generated"] == true && p["enforced"] == true {
        return Verdict::Unverified("some are about modules, exports or names that a build step may generate, and Kern did not run it, so it cannot say whether the change caused them".into());
    }
    if p["enforced"] != true { return Verdict::Look("the project's scripts do not run tsc, so Kern does not block on it".into()); }
    if p["confirmed"] != true { return Verdict::Look("Kern could not run tsc a second time on the new code and see the same errors, so it does not block on one run".into()); }
    Verdict::Blocked
}
pub fn confirm(p: &mut Value, new: &Value, again: &Value) {
    let bag = |v: &Value| { let mut b: Vec<(String, String, String)> = listed(v).into_iter().map(|d| (d.file, d.code, d.message)).collect(); b.sort(); b };
    p["confirmed"] = json!(again["status"] == "errors" && bag(new) == bag(again));
}
pub fn judge(config: &str, old: &Value, new: &Value, enforced: bool, even: Option<String>) -> Value {
    let mut p = json!({ "config": config, "old": old["status"], "new": new["status"], "version": new["version"].as_str().or(old["version"].as_str()).unwrap_or("") });
    match (old["status"].as_str(), new["status"].as_str()) {
        (Some("clean"), Some("clean")) => { p["result"] = "same".into(); }
        (Some("clean"), Some("errors")) => {
            let fresh = added(&[], &listed(new));
            p["result"] = "new-errors".into();
            p["added"] = json!(fresh.iter().take(20).map(Diag::json).collect::<Vec<_>>());
            p["count"] = json!(fresh.len());
            p["enforced"] = json!(enforced);
            if fresh.iter().any(|d| RESOLUTION.contains(&d.code.as_str())) { p["generated"] = json!(true); }
            if let Some(why) = even { p["uneven"] = json!(why); }
        }
        (Some("errors"), _) => { p["result"] = "not-compared".into(); p["why"] = json!(format!("it already had {} TypeScript errors here on the old code", old["errors"].as_u64().unwrap_or(0))); }
        (Some("unknown"), _) => { p["result"] = "not-compared".into(); p["why"] = old["why"].clone(); }
        _ => { p["result"] = "not-compared".into(); p["why"] = new["why"].clone(); }
    }
    p
}

const TSC_HEAP_MB_MIN: u64 = 1024;
pub fn spec(config: &str, seconds: u64, memory_mb: u64) -> Value {
    let heap = (memory_mb * 7 / 10).max(TSC_HEAP_MB_MIN);
    json!({ "version": 1, "command": ["/usr/local/bin/node", "-e", CHECK, format!("/workspace/{config}"), heap.to_string()], "timeout_seconds": seconds, "cwd": parent(config), "env": {},
        "files": {}, "early_secs": 0, "faults": [], "serve": {}, "steps": [] })
}
fn check_side(out: &Path, volume: &str, config: &str, until: u64, label: &str) -> Value {
    let seconds = within(until, 240);
    if seconds < 90 { return json!({ "status": "unknown", "why": "Kern's time ran out" }); }
    let dir = parent(config);
    let memory_mb = crate::suite::test_limits(seconds).memory_mb;
    let run = match crate::suite::in_test_box(out, volume, &format!("types-{label}"), ("drive-test.json", &spec(config, seconds, memory_mb)), ("__drive_test", false, seconds)) { Ok(r) => r, Err(e) => return json!({ "status": "unknown", "why": e.0 }) };
    let Ok(parts) = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) else {
        return json!({ "status": "unknown", "why": crate::sandbox::refusal(&run.errors).map_or_else(|| "the run gave no report".to_string(), |r| format!("the sandbox refused the run: {r}")) });
    };
    let output = String::from_utf8_lossy(&parts["output"]).to_string();
    let _ = std::fs::write(out.join(format!("types-{label}.out")), &output);
    side(&output, dir)
}

pub fn run(out: &Path, op: &Operator, until: u64) -> Value {
    let Ok(d) = read_json(out.join("drive.json")) else { return json!({ "status": "NOT_RUN", "why": "Kern could not read the change" }) };
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let (base, head) = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let Ok(changes) = crate::rules::changed(repo, base, head) else { return json!({ "status": "NOT_RUN", "why": "Kern could not list the changed files" }) };
    let changed: Vec<&str> = changes.iter().filter(|(_, op)| *op != "delete").map(|(p, _)| p.as_str()).collect();
    if !changed.iter().any(|p| is_ts(p)) { return json!({ "status": "NONE" }); }
    let files = |commit: &str| -> BTreeSet<String> {
        crate::git::git(repo, &["ls-tree", "-r", "--name-only", commit], None).map(|b| String::from_utf8_lossy(&b).lines().filter(|l| !l.split('/').any(|s| s == "node_modules"))
            .map(str::to_string).collect()).unwrap_or_default()
    };
    let (head_files, base_files) = (files(head), files(base));
    let configs = |all: &BTreeSet<String>| -> BTreeSet<String> {
        all.iter().filter(|l| l.rsplit('/').next().is_some_and(|n| n == "tsconfig.json" || (n.starts_with("tsconfig.") && n.ends_with(".json")))).cloned().collect()
    };
    let show = |commit: &str, path: &str| crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok().map(|b| String::from_utf8_lossy(&b).to_string());
    let list = projects(&changed, &configs(&head_files), &configs(&base_files), |p| show(head, p));
    if list.is_empty() { return json!({ "status": "NONE" }); }
    let manifests = |commit: &str, config: &str| -> Vec<Value> {
        let mut dir = parent(config).to_string();
        let mut found = Vec::new();
        loop {
            let path = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
            if let Some(v) = show(commit, &path).and_then(|t| serde_json::from_str::<Value>(&t).ok()) { found.push(v); }
            if dir.is_empty() { break; }
            dir = parent(&dir).to_string();
        }
        found
    };
    let uses_typescript = |commit: &str, config: &str| manifests(commit, config).iter().any(|m| {
        ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"].iter().any(|s| m[*s]["typescript"].is_string())
    });
    let list: Vec<String> = list.into_iter().filter(|c| uses_typescript(head, c) && uses_typescript(base, c)).collect();
    if list.is_empty() { return json!({ "status": "NONE" }); }
    if !crate::suite::types_fit(within(until, 900)) { return json!({ "status": "NOT_RUN", "why": "Kern's time ran out before it could check the TypeScript types" }); }
    if !out.join("setup.json").exists() && let Err(e) = crate::drive::setup(out, op, until) {
        return json!({ "status": "NOT_RUN", "why": format!("Kern could not install the project: {}", e.0) });
    }
    let Ok(s) = read_json(out.join("setup.json")) else { return json!({ "status": "NOT_RUN", "why": "Kern could not read its setup" }) };
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    let mut checked = Vec::new();
    for (i, config) in list.iter().enumerate() {
        let dir = parent(config);
        if let Some(why) = crate::suite::install_failure(&s, "old", dir).or_else(|| crate::suite::install_failure(&s, "new", dir)) {
            checked.push(json!({ "config": config, "result": "not-compared", "why": why }));
            continue;
        }
        let old = check_side(out, &volume("old"), config, until, &format!("old-{i}"));
        let new = if old["status"] == "clean" { check_side(out, &volume("new"), config, until, &format!("new-{i}")) } else { json!({ "status": "skipped" }) };
        let scripts: Vec<Value> = manifests(head, config).iter().chain(manifests(base, config).iter()).map(|m| m["scripts"].clone()).collect();
        let locked = |all: &BTreeSet<String>| {
            let mut at = dir.to_string();
            loop {
                if LOCKS.iter().any(|l| all.contains(&if at.is_empty() { l.to_string() } else { format!("{at}/{l}") })) { return true; }
                if at.is_empty() { return false; }
                at = parent(&at).to_string();
            }
        };
        let even = crate::suite::setup_differs(&s, dir).or_else(|| crate::suite::unlocked(&s, dir))
            .or_else(|| (!locked(&head_files) || !locked(&base_files)).then(|| "the project has no lockfile, so each side's dependencies were resolved on their own".to_string()));
        let mut p = judge(config, &old, &new, enforced(&scripts), even);
        if p["result"] == "new-errors" && p["enforced"] == true && p["uneven"].is_null() && p["generated"] != true {
            confirm(&mut p, &new, &check_side(out, &volume("new"), config, until, &format!("new-again-{i}")));
        }
        checked.push(p);
    }
    json!({ "status": "CHECKED", "projects": checked })
}
