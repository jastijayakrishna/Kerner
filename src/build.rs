use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub const RUN: &str = "const{spawnSync}=require('child_process');const heap=Number(process.argv[1])||2048,secs=Number(process.argv[2])||300;const opts=((process.env.NODE_OPTIONS||'')+' --max-old-space-size='+heap).trim();const r=spawnSync('/usr/local/bin/npm',['run','build'],{env:Object.assign({},process.env,{NODE_OPTIONS:opts}),encoding:'utf8',maxBuffer:1<<28,timeout:secs*1000,killSignal:'SIGKILL'});const all=(r.stdout||'')+'\\n'+(r.stderr||'');const tail=all.length>65536?all.slice(-65536):all;process.stdout.write(tail+'\\nkern-build: exit '+r.status+' '+(r.signal||'')+'\\n')";
pub const BUILDS_COUNT_MAX: usize = 2;
pub const BUILD_SECS_MAX: u64 = 420;
pub const BUILD_SECS_MIN: u64 = 120;
const STOP_MARGIN_SECS: u64 = 15;
const ERRORS_COUNT_MAX: usize = 5;
const ERROR_CHARS_MAX: usize = 300;
const TAIL_LINES_COUNT_MAX: usize = 3;
const ERROR_WORDS: [&str; 8] = ["error", "failed to", "can't resolve", "cannot find", "cannot resolve", "is not exported", "not found", "unexpected token"];
const TOOL_PREFIXES: [&str; 6] = ["npm error", "npm err!", "elifecycle", "err_pnpm", "error command failed with exit code", "kern-build:"];

fn parent(path: &str) -> &str { path.rsplit_once('/').map_or("", |(d, _)| d) }
fn script(manifest: &Value) -> Option<&str> { manifest["scripts"]["build"].as_str().filter(|s| !s.trim().is_empty()) }
fn nearest(path: &str, head: &dyn Fn(&str) -> Option<Value>) -> Option<(String, String)> {
    let mut dir = parent(path);
    loop {
        if let Some(m) = head(dir) && let Some(s) = script(&m) { return Some((dir.to_string(), s.to_string())); }
        if dir.is_empty() { return None; }
        dir = parent(dir);
    }
}
pub fn planned(changed: &[&str], head: &dyn Fn(&str) -> Option<Value>, base: &dyn Fn(&str) -> Option<Value>) -> Vec<String> {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for path in changed.iter().filter(|p| !crate::check::documentation(p) && !crate::suite::is_test_path(p)) {
        let Some((dir, built)) = nearest(path, head) else { continue };
        if crate::script::placeholder(&built) || base(&dir).as_ref().and_then(script).is_none() { continue; }
        *count.entry(dir).or_default() += 1;
    }
    let mut ranked: Vec<(String, usize)> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.into_iter().take(BUILDS_COUNT_MAX).map(|(d, _)| d).collect()
}

fn tool_line(line: &str) -> bool { let l = line.trim().to_ascii_lowercase(); TOOL_PREFIXES.iter().any(|p| l.starts_with(p)) }
fn clipped(line: &str) -> String { line.trim().chars().take(ERROR_CHARS_MAX).collect() }
pub fn errors(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines().filter(|l| !tool_line(l)) {
        let lower = line.to_ascii_lowercase();
        if !ERROR_WORDS.iter().any(|w| lower.contains(w)) { continue; }
        let c = clipped(line);
        if !c.is_empty() && !out.contains(&c) { out.push(c); }
        if out.len() >= ERRORS_COUNT_MAX { break; }
    }
    if out.is_empty() {
        let tail: Vec<String> = output.lines().filter(|l| !tool_line(l) && !l.trim().is_empty()).map(clipped).collect();
        out = tail[tail.len().saturating_sub(TAIL_LINES_COUNT_MAX)..].to_vec();
    }
    out
}
fn signature(side: &Value) -> String {
    side["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).take(2).collect::<Vec<_>>().join("\n").chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect()
}
pub fn side(output: &str) -> Value {
    let Some(ending) = output.lines().rev().find_map(|l| l.trim().strip_prefix("kern-build: exit ")) else {
        return json!({ "status": "unknown", "why": "the build did not finish" });
    };
    let mut words = ending.split_whitespace();
    let (code, signal) = (words.next().and_then(|c| c.parse::<i64>().ok()), words.next().unwrap_or(""));
    let body = output.lines().filter(|l| !l.trim().starts_with("kern-build: exit ")).collect::<Vec<_>>().join("\n");
    if code == Some(0) { return json!({ "status": "built" }); }
    if crate::suite::sandbox_error(&body) {
        let line = body.lines().find(|l| crate::suite::sandbox_error(l)).map(clipped).unwrap_or_default();
        return json!({ "status": "sandbox", "why": format!("the build needed something the sandbox does not have: {line}") });
    }
    match code {
        Some(c) if c < 128 => json!({ "status": "failed", "errors": errors(&body) }),
        Some(c) => json!({ "status": "unknown", "why": format!("the build ended with exit code {c}, which means it was stopped") }),
        None => json!({ "status": "unknown", "why": format!("the build was stopped ({})", if signal.is_empty() { "by a signal" } else { signal }) }),
    }
}

fn said(side: &Value, which: &str) -> String {
    match side["status"].as_str() {
        Some("failed") => format!("it fails on the {which} code here too: {}", side["errors"].as_array().into_iter().flatten().filter_map(Value::as_str).next().unwrap_or("no error was printed")),
        Some("skipped") => format!("Kern did not build the {which} code"),
        _ => format!("on the {which} code, {}", side["why"].as_str().unwrap_or("the build gave no answer")),
    }
}
pub fn judge(dir: &str, old: &Value, new: &Value, again: Option<&Value>, even: Option<String>) -> Value {
    let mut j = json!({ "dir": dir, "old": old["status"], "new": new["status"] });
    let status = |v: Option<&Value>| v.and_then(|s| s["status"].as_str()).unwrap_or("none").to_string();
    let (result, why) = match (status(Some(old)).as_str(), status(Some(new)).as_str(), status(again).as_str()) {
        ("built", "built", _) => ("same", None),
        ("built", "failed", "failed") if again.is_some_and(|a| signature(a) == signature(new)) => ("broke", None),
        ("built", "failed", "built" | "failed") => ("unsteady", Some("it failed once and then gave another result on the new code, so Kern sets it aside".to_string())),
        ("built", "failed", _) => ("not-compared", Some(again.map_or_else(|| "Kern could not build the new code a second time to confirm the failure".to_string(), |a| said(a, "new")))),
        ("built", _, _) => ("not-compared", Some(said(new, "new"))),
        _ => ("not-compared", Some(said(old, "old"))),
    };
    j["result"] = json!(result);
    if let Some(w) = why { j["why"] = json!(w); }
    if result == "broke" {
        j["errors"] = new["errors"].clone();
        if let Some(e) = even { j["uneven"] = json!(e); }
    }
    j
}

pub fn spec(dir: &str, seconds: u64, memory_mb: u64) -> Value {
    json!({ "version": 1, "command": ["/usr/local/bin/node", "-e", RUN, (memory_mb * 3 / 4).to_string(), seconds.saturating_sub(STOP_MARGIN_SECS).to_string()],
        "timeout_seconds": seconds, "cwd": dir, "env": {}, "files": {}, "early_secs": 0, "faults": [], "serve": {}, "steps": [] })
}
pub fn builds_fit(left_secs: u64) -> bool { left_secs >= BUILD_SECS_MIN }
fn build_side(out: &Path, volume: &str, dir: &str, until: u64, label: &str) -> Value {
    let seconds = within(until, BUILD_SECS_MAX);
    if !builds_fit(seconds) { return json!({ "status": "unknown", "why": "Kern's time ran out" }); }
    let memory_mb = crate::suite::test_limits(seconds).memory_mb;
    let run = match crate::suite::in_test_box(out, volume, &format!("build-{label}"), ("drive-test.json", &spec(dir, seconds, memory_mb)), ("__drive_test", false, seconds)) {
        Ok(r) => r,
        Err(e) => return json!({ "status": "unknown", "why": e.0 }),
    };
    let Ok(parts) = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS) else {
        return json!({ "status": "unknown", "why": crate::sandbox::refusal(&run.errors).map_or_else(|| "the build gave no report".to_string(), |r| format!("the sandbox refused the build: {r}")) });
    };
    let output = String::from_utf8_lossy(&parts["output"]).to_string();
    let _ = std::fs::write(out.join(format!("build-{label}.out")), &output);
    side(&output)
}
pub fn run(out: &Path, op: &Operator, until: u64) -> Value {
    let Ok(d) = read_json(out.join("drive.json")) else { return json!({ "status": "NOT_RUN", "why": "Kern could not read the change" }) };
    let repo = Path::new(d["repo"].as_str().unwrap_or(""));
    let (base, head) = (d["base"].as_str().unwrap_or(""), d["head"].as_str().unwrap_or(""));
    let Ok(changes) = crate::rules::changed(repo, base, head) else { return json!({ "status": "NOT_RUN", "why": "Kern could not list the changed files" }) };
    let changed: Vec<&str> = changes.iter().filter(|(_, op)| *op != "delete").map(|(p, _)| p.as_str()).collect();
    let manifest = |commit: &str, dir: &str| -> Option<Value> {
        let path = if dir.is_empty() { "package.json".to_string() } else { format!("{dir}/package.json") };
        crate::git::git(repo, &["show", &format!("{commit}:{path}")], None).ok().and_then(|b| serde_json::from_slice(&b).ok())
    };
    let dirs = planned(&changed, &|dir| manifest(head, dir), &|dir| manifest(base, dir));
    if dirs.is_empty() { return json!({ "status": "NONE" }); }
    if !builds_fit(within(until, BUILD_SECS_MAX)) { return json!({ "status": "NOT_RUN", "why": "Kern's time ran out before it could build the project" }); }
    if !out.join("setup.json").exists() && let Err(e) = crate::drive::setup(out, op, until) {
        return json!({ "status": "NOT_RUN", "why": format!("Kern could not install the project: {}", e.0) });
    }
    let Ok(s) = read_json(out.join("setup.json")) else { return json!({ "status": "NOT_RUN", "why": "Kern could not read its setup" }) };
    let volume = |side: &str| s[side]["volume"].as_str().unwrap_or("").to_string();
    let mut builds = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        if let Some(why) = crate::suite::install_failure(&s, "old", dir).or_else(|| crate::suite::install_failure(&s, "new", dir)) {
            builds.push(json!({ "dir": dir, "result": "not-compared", "why": why }));
            continue;
        }
        let old = build_side(out, &volume("old"), dir, until, &format!("old-{i}"));
        let new = if old["status"] == "built" { build_side(out, &volume("new"), dir, until, &format!("new-{i}")) } else { json!({ "status": "skipped" }) };
        let again = (new["status"] == "failed").then(|| build_side(out, &volume("new"), dir, until, &format!("new-again-{i}")));
        let even = crate::suite::setup_differs(&s, dir);
        builds.push(judge(dir, &old, &new, again.as_ref(), even));
    }
    json!({ "status": "CHECKED", "builds": builds })
}
pub fn rejudge(dir: &Path, stored: &Value, setup: Option<&Value>) -> Value {
    let Some(list) = stored["builds"].as_array() else { return stored.clone() };
    let read = |label: &str| std::fs::read_to_string(dir.join(format!("build-{label}.out"))).ok().map(|o| side(&o));
    let builds: Vec<Value> = list.iter().enumerate().map(|(i, b)| {
        let folder = b["dir"].as_str().unwrap_or("");
        match (read(&format!("old-{i}")), read(&format!("new-{i}"))) {
            (Some(old), Some(new)) => judge(folder, &old, &new, read(&format!("new-again-{i}")).as_ref(),
                setup.and_then(|s| crate::suite::setup_differs(s, folder))),
            _ => b.clone(),
        }
    }).collect();
    let mut out = stored.clone();
    out["builds"] = json!(builds);
    out
}
