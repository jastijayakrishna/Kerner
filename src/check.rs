use crate::policy::Operator;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;

pub struct Ai<'a> { pub agent: &'a [String], pub rounds: usize }

const MINUTES: u64 = 55;
const REGISTRY_NAMES_COUNT_MAX: usize = 30;
const TESTS_MINUTES: u64 = 25;
pub fn minutes() -> u64 { std::env::var("KERN_CHECK_MINUTES").ok().and_then(|m| m.trim().parse::<u64>().ok()).filter(|m| (1..=1440).contains(m)).unwrap_or(MINUTES) }

const DOCS: [&str; 13] = ["md", "mdx", "markdown", "txt", "rst", "adoc", "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf"];
const DOC_NAMES: [&str; 7] = ["license", "licence", "changelog", "authors", "contributors", "codeowners", "notice"];
pub fn documentation(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let (stem, ext) = name.rsplit_once('.').unwrap_or((&name, ""));
    DOCS.contains(&ext) || (DOC_NAMES.contains(&stem) && ext.is_empty())
}
fn runnable(repo: &Path, head: &str, rules: &Value) -> bool {
    let paths: Vec<&str> = rules["changed"].as_array().into_iter().flatten().filter_map(|c| c["path"].as_str()).collect();
    if paths.iter().any(|p| !documentation(p)) { return true; }
    let mut needles: BTreeSet<String> = paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).filter(|n| !n.is_empty()).collect();
    for folder in paths.iter().filter_map(|p| p.rsplit_once('/').map(|(d, _)| d)).flat_map(|d| d.split('/')).filter(|f| !f.is_empty()) {
        needles.extend([format!("{folder}/"), format!("'{folder}'"), format!("\"{folder}\""), format!("`{folder}`")]);
    }
    if needles.is_empty() { return false; }
    crate::git::mentions(repo, head, &needles.iter().map(String::as_str).collect::<Vec<_>>()).map_or(true, |found| found.iter().any(|f| crate::capability::is_code(f)))
}

pub fn clock(repo: &Path, head: &str) -> u64 {
    let wall = now_ms() / 1000;
    let commit = crate::git::git(repo, &["show", "-s", "--format=%ct", head], None).ok().and_then(|b| String::from_utf8_lossy(&b).trim().parse::<u64>().ok()).filter(|c| *c > 0);
    commit.map_or(wall, |c| c.min(wall))
}
pub fn own_packages(repo: &Path, base: &str, head: &str) -> BTreeSet<String> {
    [base, head].iter().flat_map(|c| crate::suite::workspace(repo, c).into_keys()).collect()
}
const SELF_TEST_PACKAGE: &str = r#"{"name":"kern-self-test","version":"1.0.0","private":true,"type":"module","scripts":{"test":"node --test"}}"#;
const SELF_TEST_CODE: &str = "export const add = (a, b) => a + b;\nexport const hello = (name) => `hello ${name}`;\n";
const SELF_TEST_TESTS: &str = "import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add, hello } from '../math.js';\n\ntest('adds', () => assert.equal(add(2, 3), 5));\ntest('says hello', () => assert.equal(hello('kern'), 'hello kern'));\n";
const SELF_TEST_REUSE_SECS_MAX: u64 = 3600;
pub fn canary_held(r: &Value) -> Result<(), String> {
    let blocked = r["blocked"].as_array().cloned().unwrap_or_default();
    if r["decision"] == "blocked" && blocked.len() == 1 && blocked[0]["kind"] == "test-broke" && r["tests"]["status"] == "RAN" && r["tests"]["kept"].as_u64().unwrap_or(0) >= 1 { return Ok(()); }
    Err(format!("Kern's self-test on this machine answered {} ({} blocked, tests {}), where the known answer is one broken test", r["decision"].as_str().unwrap_or("nothing"), blocked.len(), r["tests"]["status"].as_str().unwrap_or("missing")))
}
fn self_test(op: &Operator) -> Result<&'static str, String> {
    let doctor = crate::sandbox::doctor();
    let Some((builder, _)) = doctor.builder.filter(|_| doctor.ready) else { return Ok("skipped: no sandbox on this machine") };
    let marker = std::env::temp_dir().join(format!("kern-self-test-{}.json", builder.chars().take(16).collect::<String>()));
    if read_json(&marker).ok().and_then(|v| v["at"].as_u64()).is_some_and(|at| now_ms().saturating_sub(at) < SELF_TEST_REUSE_SECS_MAX * 1000) { return Ok("passed earlier on this machine"); }
    let dir = std::env::temp_dir().join(format!("kern-self-test-{}", uuid4().map_err(|e| e.0)?));
    let file = |text: &str| { let b = text.as_bytes().to_vec(); crate::git::Entry { mode: "100644".into(), oid: crate::git::object_id("blob", &b), size: b.len() as u64, bytes: Some(b) } };
    let tree = |code: &str| -> crate::git::Files { [("package.json", SELF_TEST_PACKAGE), ("math.js", code), ("test/math.test.js", SELF_TEST_TESTS)].iter().map(|(p, t)| (p.to_string(), file(t))).collect() };
    let ran = (|| -> R<Value> {
        let store = crate::git::init_store(&dir.join("repo.git"))?;
        let base = crate::git::write_commit(&store, &tree(SELF_TEST_CODE), None, "self-test")?;
        let head = crate::git::write_commit(&store, &tree(&SELF_TEST_CODE.replace("a + b", "a - b")), Some(&base), "self-test break")?;
        judge(&store, (&base, &head), &dir.join("out"), op, None, None, &|| Ok("this is the self-test".to_string()))?;
        read_json(dir.join("out").join("receipt.json"))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    ran.map_err(|e| format!("Kern's self-test could not run on this machine: {}", e.0)).and_then(|r| canary_held(&r))?;
    let _ = write_json(&marker, &json!({ "at": now_ms() }));
    Ok("passed")
}
pub fn check(repo: &Path, base: &str, head: &str, out: &Path, op: &Operator, pr: Option<&str>, ai: Option<&Ai>) -> R<Value> {
    judge(repo, (base, head), out, op, pr, ai, &|| self_test(op).map(str::to_string))
}
pub fn judge(repo: &Path, (base, head): (&str, &str), out: &Path, op: &Operator, pr: Option<&str>, ai: Option<&Ai>, self_test: &dyn Fn() -> Result<String, String>) -> R<Value> {
    let started = now_ms();
    let until = started + minutes() * 60_000;
    crate::drive::prepare(repo, base, head, out, pr)?;
    let d = read_json(out.join("drive.json"))?;
    let rules = crate::rules::rules(repo, base, head)?;
    let now = clock(repo, head);
    let times: std::cell::RefCell<std::collections::BTreeMap<String, Option<Value>>> = Default::default();
    let meta = |name: &str| -> Option<Value> {
        let mut cache = times.borrow_mut();
        if !cache.contains_key(name) && cache.len() >= REGISTRY_NAMES_COUNT_MAX { return None; }
        cache.entry(name.to_string()).or_insert_with(|| crate::deps::registry_meta(name)).clone()
    };
    let published = |name: &str, version: &str| -> Option<u64> { meta(name).and_then(|m| m["time"][version].as_str().and_then(crate::deps::rfc3339)) };
    let ours = own_packages(repo, base, head);
    let rules = crate::deps::aged(rules, now, &published, &|name: &str| ours.contains(name));
    let run = runnable(repo, head, &rules);
    let machine = if run { self_test() } else { Ok("not needed: nothing to run".to_string()) };
    let trusted = machine.is_ok();
    let checked = now_ms();
    let behaviour = match ai {
        _ if !run => json!({ "status": "NOTHING_TO_RUN" }),
        Some(_) if !trusted => json!({ "status": "OFF", "why": "Kern's self-test failed on this machine" }),
        None => json!({ "status": "OFF", "why": "no AI key" }),
        Some(a) => match crate::drive::auto(repo, base, head, out, op, pr, a.agent, a.rounds, until.saturating_sub(TESTS_MINUTES * 60_000).max(started)) {
            Ok(v) => json!({ "status": v["status"], "why": v["rounds"].as_array().and_then(|r| r.last()).and_then(|r| r["error"].as_str()).unwrap_or(""),
                "report": read_json(out.join("report.json")).unwrap_or(Value::Null) }),
            Err(e) => json!({ "status": "FAILED", "why": e.0 }),
        },
    };
    let behaved = now_ms();
    let mut tests = match &machine {
        _ if !run => json!({ "status": "NOTHING_TO_RUN", "why": "this change touches only documentation" }),
        Err(why) => json!({ "status": "NOT_RUN", "why": format!("Kern could not run the tests: {why}") }),
        Ok(_) => crate::suite::run(out, op, until).unwrap_or_else(|e| json!({ "status": "NOT_RUN", "why": format!("Kern could not run the tests: {}", e.0) })),
    };
    let tested = now_ms();
    if run && trusted && std::env::var("KERN_TYPES").as_deref() != Ok("0") { tests["types"] = crate::types::run(out, op, until); }
    let typed = now_ms();
    let comment = std::fs::read_to_string(out.join("comment.md")).ok().filter(|_| behaviour["status"] == "RAN");
    let receipt = crate::receipt::build(&d, &rules, &tests, &behaviour);
    crate::receipt::write(out, &receipt, comment.as_deref())?;
    let keep = std::env::var("KERN_KEEP").is_ok_and(|v| v.trim() == "1");
    let cleaned = !keep && (out.join("setup.json").exists() || out.join("volumes.txt").exists()) && crate::drive::clean(out).is_ok();
    let canary = match &machine { Ok(said) => said.to_string(), Err(why) => format!("failed: {why}") };
    Ok(json!({ "status": "CHECKED", "cleaned": cleaned, "canary": canary, "decision": receipt["decision"], "blocked": receipt["blocked"].as_array().map_or(0, Vec::len),
        "look": receipt["look"].as_array().map_or(0, Vec::len), "tests": tests["status"], "behaviour": behaviour["status"], "seconds": (now_ms() - started) / 1000,
        "phases": { "rules": (checked - started) / 1000, "behaviour": (behaved - checked) / 1000, "tests": (tested - behaved) / 1000, "types": (typed - tested) / 1000, "tests_detail": tests["seconds"] },
        "receipt": out.join("receipt.md").display().to_string() }))
}
