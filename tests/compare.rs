use kern::git::{self, Entry, Files};
use kern::policy::{self, attenuate, contract};
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-compare-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
    fn join(&self, s: &str) -> PathBuf { self.0.join(s) }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn file(text: &str) -> Entry { let b = text.as_bytes().to_vec(); Entry { mode: "100644".into(), oid: git::object_id("blob", &b), size: b.len() as u64, bytes: Some(b) } }
fn files(items: &[(&str, &str)]) -> Files { items.iter().map(|(p, t)| (p.to_string(), file(t))).collect() }
fn denied<T>(r: R<T>, needle: &str) {
    match r { Ok(_) => panic!("expected denial containing {needle:?}"), Err(e) => assert!(e.0.contains(needle), "{:?} does not contain {needle:?}", e.0) }
}
fn v5(delta: Value) -> Value {
    json!({ "version": 5, "task": "fix-addition", "filesystem": { "read": ["README.md", "src/", "test/"], "write": ["src/", "test/"] }, "network": [], "secrets": [], "publish": false,
        "dependencies": null, "limits": { "files_changed": 8, "lines_changed": 400, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 60, "memory_mb": 256, "pids": 64 },
        "required_checks": ["addition"], "behaviors": [], "tests": null, "delta": delta, "app": null })
}
fn negatives() -> Value {
    json!({ "intent": "Negative amounts are refused with a RangeError instead of being added; nothing else changes",
        "changes": [{ "function": "src/money.js#add", "when": "(a, b) => a < 0 || b < 0", "examples": [{ "args": "[-1, 2]", "throws": "RangeError" }, { "args": "[2, 3]", "returns": "5" }] }] })
}

#[test]
fn version_5_contracts_state_the_change_before_the_code() {
    let c = contract(&v5(negatives())).unwrap();
    assert!(c.compare, "version 5 always compares the old and the new code");
    let d = c.delta.clone().expect("the stated change");
    assert_eq!((d.changes.len(), d.changes[0].function.as_str(), d.changes[0].when.as_deref(), d.changes[0].examples.len()), (1, "src/money.js#add", Some("(a, b) => a < 0 || b < 0"), 2));
    assert_eq!((d.changes[0].examples[0].throws.as_deref(), d.changes[0].examples[1].returns.as_deref()), (Some("RangeError"), Some("5")));
    assert!(d.removes.is_empty() && d.allow.is_empty());
    assert!(kern::flow::required_checks(&c).contains(&"compare".to_string()), "every version 5 proof carries the comparison");
    let report = contract(&v5(Value::Null)).unwrap();
    assert!(report.compare && report.delta.is_none());
    let refactor = contract(&v5(json!({ "intent": "Rename internals; nothing changes", "changes": [] }))).unwrap();
    assert!(refactor.delta.unwrap().changes.is_empty());
    let mut more = negatives();
    more["removes"] = json!(["src/legacy.js", "src/money.js#oldAdd", "docs/"]);
    more["allow"] = json!(["speed", "delete"]);
    let d = contract(&v5(more)).unwrap().delta.unwrap();
    assert_eq!((d.removes.len(), d.allow.clone()), (3, vec!["delete".to_string(), "speed".to_string()]));
    for (patch, needle) in [
        (json!({ "intent": "", "changes": [] }), "delta.intent"), (json!({ "intent": "x\u{7}", "changes": [] }), "delta.intent"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js" }] }), "path#name"), (json!({ "intent": "x", "changes": [{ "function": "src/money.md#add" }] }), "path#name"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js#a b" }] }), "path#name"), (json!({ "intent": "x", "changes": [{ "function": "../money.js#add" }] }), "Nonportable"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js#add", "examples": [{ "returns": "1" }] }] }), "missing or unknown fields"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js#add", "examples": [{ "args": "[1]" }] }] }), "returns or throws"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js#add", "examples": [{ "args": "[1]", "returns": "1", "throws": true }] }] }), "returns or throws"),
        (json!({ "intent": "x", "changes": [{ "function": "src/money.js#add", "when": "" }] }), "JavaScript"),
        (json!({ "intent": "x", "changes": [], "allow": ["everything"] }), "delta.allow"), (json!({ "intent": "x", "changes": [], "removes": ["a//b"] }), "Nonportable"),
        (json!({ "intent": "x", "changes": [], "extra": 1 }), "missing or unknown fields"),
    ] {
        denied(contract(&v5(patch)), needle);
    }
    let mut v4 = v5(Value::Null);
    v4["version"] = json!(4);
    denied(contract(&v4), "missing or unknown fields");
    let mut bare = v5(Value::Null);
    bare.as_object_mut().unwrap().remove("delta");
    denied(contract(&bare), "missing or unknown fields");
    attenuate(&v5(Value::Null), &v5(negatives())).unwrap();
    denied(attenuate(&v5(negatives()), &v5(Value::Null)), "stated change");
    let mut weaker = v5(negatives());
    weaker["delta"]["changes"][0]["when"] = json!("() => true");
    denied(attenuate(&v5(negatives()), &weaker), "stated change");
    let mut older = v5(Value::Null);
    older["version"] = json!(4);
    older.as_object_mut().unwrap().remove("delta");
    older.as_object_mut().unwrap().remove("app");
    denied(attenuate(&v5(Value::Null), &older), "comparison");
    let mut op = json!({ "version": 3, "repository": "acme/project", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "compare", "file": "/operator/checks/x.json", "sha256": "b".repeat(64) }], "registry": null, "permissions": {} });
    denied(policy::operator(&op), "reserved");
    op["checks"][0]["id"] = json!("addition");
    policy::operator(&op).unwrap();
    let mut allows = v5(Value::Null);
    allows["behaviors"] = json!(["delete:tmp/cache.json", "overwrite:data/state.json"]);
    assert_eq!(contract(&allows).unwrap().behaviors.len(), 2);
    let long = |n: usize| json!({ "args": format!("[\"{}\"]", "a".repeat(n)), "returns": "1" });
    let huge = json!({ "intent": "x", "changes": (0..32).map(|i| json!({ "function": format!("src/m{i}.js#f"), "examples": (0..16).map(|_| long(1990)).collect::<Vec<_>>() })).collect::<Vec<_>>() });
    denied(contract(&v5(huge)), "delta must be at most 32 KiB");
    contract(&v5(json!({ "intent": "x", "changes": [{ "function": "src/m.js#f", "examples": (0..8).map(|_| long(1000)).collect::<Vec<_>>() }] }))).unwrap();
}

#[test]
fn app_recipes_name_how_to_start_the_app_and_who_asks() {
    let recipe = json!({ "setup": null, "start": ["/usr/local/bin/node", "server.js"], "port": 3000, "ready": "/healthz", "routes": ["GET /", "GET /admin/export", "POST /orders"],
        "roles": { "anonymous": {}, "user": { "Authorization": "Bearer test-user" } }, "env": { "NODE_ENV": "test" } });
    let mut c = v5(Value::Null);
    c["app"] = recipe.clone();
    let app = contract(&c).unwrap().app.expect("the recipe");
    assert_eq!((app.port, app.routes.len(), app.roles.len(), app.start[0].as_str()), (3000, 3, 2, "/usr/local/bin/node"));
    for (key, value, needle) in [
        ("start", json!(["node", "server.js"]), "absolute"), ("port", json!(0), "port"), ("port", json!(70000), "port"), ("ready", json!("healthz"), "app.ready"),
        ("routes", json!(["GET"]), "app.routes"), ("routes", json!(["FETCH /x"]), "app.routes"), ("routes", json!(["GET /a b"]), "app.routes"),
        ("roles", json!({}), "app.roles"), ("roles", json!({ "user": { "Bad Header": "x" } }), "app.roles"), ("roles", json!({ "user": { "X": "a\r\nb" } }), "app.roles"),
        ("env", json!({ "lower": "x" }), "app.env"),
    ] {
        let mut bad = c.clone();
        bad["app"][key] = value;
        denied(contract(&bad), needle);
    }
    let mut other = c.clone();
    other["app"]["port"] = json!(3001);
    denied(attenuate(&c, &other), "app recipe");
}

fn manifest(deps: &[(&str, &str)], scripts: Value) -> String {
    serde_json::to_string_pretty(&json!({ "name": "app", "version": "1.0.0", "scripts": scripts, "dependencies": deps.iter().map(|(n, v)| (n.to_string(), json!(v))).collect::<serde_json::Map<_, _>>() })).unwrap()
}
fn lock(root: &[(&str, &str)], packages: &[(&str, &str)]) -> String {
    let mut map = serde_json::Map::new();
    map.insert(String::new(), json!({ "name": "app", "version": "1.0.0", "dependencies": root.iter().map(|(n, v)| (n.to_string(), json!(v))).collect::<serde_json::Map<_, _>>() }));
    for (name, version) in packages {
        map.insert(format!("node_modules/{name}"), json!({ "version": version, "resolved": format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz"), "integrity": "sha512-abc" }));
    }
    serde_json::to_string_pretty(&json!({ "name": "app", "version": "1.0.0", "lockfileVersion": 3, "requires": true, "packages": map })).unwrap()
}
fn upgrade_contract() -> Value {
    let mut c = v5(json!({ "intent": "Upgrade dependencies; the code behaves exactly as before", "changes": [] }));
    c["task"] = json!("upgrade");
    c["filesystem"] = json!({ "read": ["test/"], "write": [] });
    c["dependencies"] = json!({ "manager": "npm", "lockfile": "package-lock.json", "allow": ["*"] });
    c["limits"]["files_changed"] = json!(2);
    c
}

#[test]
fn upgrade_tasks_may_only_move_packages_that_are_already_there() {
    let c = contract(&upgrade_contract()).unwrap();
    assert_eq!(c.approved, vec!["*".to_string()]);
    let mut stated = upgrade_contract();
    stated["delta"] = negatives();
    denied(contract(&stated), "nothing changes");
    let mut silent = upgrade_contract();
    silent["delta"] = Value::Null;
    denied(contract(&silent), "nothing changes");
    let mut mixed = upgrade_contract();
    mixed["dependencies"]["allow"] = json!(["*", "zod"]);
    denied(contract(&mixed), "dependencies.allow");
    let mut v4 = upgrade_contract();
    v4["version"] = json!(4);
    v4.as_object_mut().unwrap().remove("delta");
    v4.as_object_mut().unwrap().remove("app");
    denied(contract(&v4), "dependencies.allow");
    let scripts = json!({ "test": "mocha" });
    let base = files(&[("package.json", &manifest(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0")], scripts.clone())), ("package-lock.json", &lock(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.20"), ("left-pad", "1.3.0")])),
        ("src/a.js", "module.exports = require('lodash');\n")]);
    let with = |m: &str, l: &str| { let mut f = base.clone(); f.insert("package.json".into(), file(m)); f.insert("package-lock.json".into(), file(l)); f };
    let bumped = with(&manifest(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], scripts.clone()), &lock(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.21"), ("left-pad", "1.3.0")]));
    let changed = git::changes(&base, &bumped, &c).unwrap();
    assert_eq!(changed["dependencies"]["upgraded"], json!(["lodash"]), "{changed}");
    assert!(kern::deps::imports(&changed).is_empty(), "an upgrade never lets code import something new");
    let transitive = with(&manifest(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0")], scripts.clone()), &lock(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.20"), ("left-pad", "1.3.1")]));
    git::changes(&base, &transitive, &c).unwrap();
    let added = with(&manifest(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0"), ("zod", "^3.0.0")], scripts.clone()), &lock(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0"), ("zod", "^3.0.0")], &[("lodash", "4.17.20"), ("left-pad", "1.3.0"), ("zod", "3.23.0")]));
    denied(git::changes(&base, &added, &c), "only upgrades packages the project already uses, but this change adds or removes: zod");
    let removed = with(&manifest(&[("lodash", "^4.17.20")], scripts.clone()), &lock(&[("lodash", "^4.17.20")], &[("lodash", "4.17.20")]));
    denied(git::changes(&base, &removed, &c), "adds or removes: left-pad");
    let hook = with(&manifest(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], json!({ "test": "mocha", "postinstall": "node steal.js" })), &lock(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.21"), ("left-pad", "1.3.0")]));
    denied(git::changes(&base, &hook, &c), "touches: scripts");
    let mut code = bumped.clone();
    code.insert("src/a.js".into(), file("module.exports = 1;\n"));
    denied(git::changes(&base, &code, &c), "Write outside contract: src/a.js");
    let swapped = |spec: &str, entry: Value| {
        let mut l: Value = serde_json::from_str(&lock(&[("lodash", spec), ("left-pad", "^1.0.0")], &[("left-pad", "1.3.0")])).unwrap();
        l["packages"]["node_modules/lodash"] = entry;
        with(&manifest(&[("lodash", spec), ("left-pad", "^1.0.0")], scripts.clone()), &l.to_string())
    };
    let evil = json!({ "name": "evil", "version": "1.0.0", "resolved": "https://registry.npmjs.org/evil/-/evil-1.0.0.tgz", "integrity": "sha512-abc" });
    denied(git::changes(&base, &swapped("npm:evil@1.0.0", evil.clone()), &c), "swaps lodash for another package: lodash becomes evil");
    let mut lock_only = base.clone();
    let mut l: Value = serde_json::from_str(&lock(&[("lodash", "^4.17.20"), ("left-pad", "^1.0.0")], &[("left-pad", "1.3.0")])).unwrap();
    l["packages"]["node_modules/lodash"] = evil;
    lock_only.insert("package-lock.json".into(), file(&l.to_string()));
    denied(git::changes(&base, &lock_only, &c), "swaps node_modules/lodash for another package: lodash becomes evil");
    let elsewhere = json!({ "version": "4.17.21", "resolved": "https://registry.npmjs.org/evil/-/evil-4.17.21.tgz", "integrity": "sha512-abc" });
    denied(git::changes(&base, &swapped("^4.17.21", elsewhere), &c), "resolves node_modules/lodash to a tarball that is not lodash 4.17.21");
    let mut l: Value = serde_json::from_str(&lock(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.21"), ("left-pad", "1.3.0")])).unwrap();
    l["packages"]["node_modules/string-width-cjs"] = json!({ "name": "string-width", "version": "4.2.3", "resolved": "https://registry.npmjs.org/string-width/-/string-width-4.2.3.tgz", "integrity": "sha512-abc" });
    l["packages"]["node_modules/@scope/pkg"] = json!({ "version": "2.0.0", "resolved": "https://registry.npmjs.org/@scope/pkg/-/pkg-2.0.0.tgz", "integrity": "sha512-abc" });
    git::changes(&base, &with(&manifest(&[("lodash", "^4.17.21"), ("left-pad", "^1.0.0")], scripts.clone()), &l.to_string()), &c).unwrap();
    let mut one = upgrade_contract();
    one["version"] = json!(4);
    one.as_object_mut().unwrap().remove("delta");
    one.as_object_mut().unwrap().remove("app");
    one["dependencies"]["allow"] = json!(["lodash"]);
    denied(git::changes(&base, &swapped("npm:evil@1.0.0", json!({ "name": "evil", "version": "1.0.0", "resolved": "https://registry.npmjs.org/evil/-/evil-1.0.0.tgz", "integrity": "sha512-abc" })), &contract(&one).unwrap()), "swaps lodash for another package");
}

fn node(args: &[&std::ffi::OsStr], dir: &Path, env: &[(&str, &Path)]) -> std::process::Output {
    use std::process::Stdio;
    let mut cmd = std::process::Command::new("node");
    cmd.args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env { cmd.env(k, v); }
    let mut child = cmd.spawn().unwrap();
    let read = |mut pipe: Box<dyn std::io::Read + Send>| std::thread::spawn(move || { let mut b = Vec::new(); let _ = pipe.read_to_end(&mut b); b });
    let (out, err) = (read(Box::new(child.stdout.take().unwrap())), read(Box::new(child.stderr.take().unwrap())));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() { break s; }
        if started.elapsed() > std::time::Duration::from_secs(90) { let _ = child.kill(); panic!("node did not finish in 90 s: the probe stalled"); }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    std::process::Output { status, stdout: out.join().unwrap(), stderr: err.join().unwrap() }
}
fn tree(root: &Path, items: &[(&str, &str)]) {
    for (p, c) in items {
        let at = root.join(p);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, c).unwrap();
    }
}

const MONEY: &str = "function add(a, b) { return a + b; }\nfunction scale(x, k) { return x * k; }\nmodule.exports = { add, scale };\n";
const MONEY_STATED: &str = "function add(a, b) {\n  if (a < 0 || b < 0) throw new RangeError('negative amount');\n  return a + b;\n}\nfunction scale(x, k) { return x * k; }\nmodule.exports = { add, scale };\n";
const MONEY_TEST: &str = "const assert = require('node:assert');\nconst { add, scale } = require('../src/money');\nassert.equal(add(2, 3), 5);\nassert.equal(scale(3, 7), 21);\n";

fn probe(dir: &Path, plan: &Value, t: &Temp) -> Value {
    let plan_file = t.join(&format!("plan-{}.json", uuid4().unwrap()));
    std::fs::write(&plan_file, plan.to_string()).unwrap();
    let harness = t.join("probe.cjs");
    if !harness.exists() { std::fs::write(&harness, kern::compare::PROBE).unwrap(); }
    let out = node(&[harness.as_os_str(), plan_file.as_os_str()], dir, &[("KERN_PROBE_ROOT", dir)]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    kern::compare::read_probe(&out.stdout).unwrap_or_else(|e| panic!("{}: {}", e.0, String::from_utf8_lossy(&out.stdout)))
}
struct Compared { evidence: Value, block: Option<String>, plan: Value }
fn compare_with(base: &Files, candidate: &Files, policy: &Value, tweak: impl Fn(&mut Value), counts: [std::collections::BTreeMap<String, u64>; 2]) -> Compared {
    let policy = contract(policy).unwrap();
    let changed = git::changes(base, candidate, &policy).unwrap();
    let mut plan = kern::compare::plan(base, candidate, &changed, &policy, &"5eed".repeat(16), 60).unwrap();
    plan["call_ms"] = json!(300);
    tweak(&mut plan);
    let t = Temp::new();
    let side = |name: &str, f: &Files| { let dir = t.join(name); git::materialize(None, f, &dir, None).unwrap(); dir };
    let (old, new) = (side("old", base), side("new", candidate));
    let removed = plan["removals"].as_array().is_some_and(|r| !r.is_empty()).then(|| probe(&side("removed", &kern::compare::without_removals(base, &plan)), &plan, &t));
    let runs = kern::compare::Runs { old: [probe(&old, &plan, &t), probe(&old, &plan, &t)], new: [probe(&new, &plan, &t), probe(&new, &plan, &t)], removed, counts };
    let (evidence, block) = kern::compare::evaluate(&plan, &runs, base, candidate, &changed, &policy);
    Compared { evidence, block, plan }
}
fn compared(base: &Files, candidate: &Files, policy: &Value) -> Compared { compare_with(base, candidate, policy, |_| {}, Default::default()) }
fn nothing_changes() -> Value { v5(json!({ "intent": "Tidy the code; nothing changes", "changes": [] })) }
fn blocked(c: &Compared, needle: &str) {
    let why = c.block.clone().unwrap_or_else(|| panic!("expected a block containing {needle:?}: {}", c.evidence));
    assert!(why.contains(needle), "{why:?} does not contain {needle:?}");
}
fn passed(c: &Compared) { assert!(c.block.is_none(), "{:?}\n{}", c.block, c.evidence); }
fn regex_like_ms(s: &str) -> bool {
    s.split(" ms ").take(2).all(|part| part.rsplit(' ').next().and_then(|n| n.split_once('.')).is_some_and(|(a, b)| a.parse::<u64>().is_ok() && b.len() == 3 && b.parse::<u64>().is_ok()))
}

#[test]
fn identical_code_passes_a_nothing_changes_delta() {
    let base = files(&[("src/money.js", MONEY), ("test/money.test.js", MONEY_TEST)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file("const add = (a, b) => [a, b].reduce((x, y) => x + y);\nfunction scale(x, k) { const r = x * k; return r; }\nmodule.exports = { add, scale };\n"));
    let c = compared(&base, &candidate, &nothing_changes());
    passed(&c);
    let e = &c.evidence;
    assert_eq!((e["mode"].as_str(), e["probed"]["modules"].as_u64(), e["probed"]["functions"].as_u64(), e["differences"].as_array().map(Vec::len)), (Some("delta"), Some(1), Some(2), Some(0)), "{e}");
    assert!(e["same"].as_u64() > Some(100), "hundreds of calls, from the tests' own arguments and from unusual inputs: {e}");
    assert!(c.plan["modules"][0]["tuples"].as_array().unwrap().iter().any(|t| t == "[3, 7]"), "the tests' own calls are inputs: {}", c.plan);
    let lines = kern::compare::summary(e);
    assert!(lines.iter().any(|l| l.contains("Tidy the code; nothing changes")) && lines.iter().any(|l| l.contains("identical")), "{lines:?}");
}

#[test]
fn the_stated_change_may_differ_and_nothing_else() {
    let base = files(&[("src/money.js", MONEY), ("test/money.test.js", MONEY_TEST)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(MONEY_STATED));
    let c = compared(&base, &candidate, &v5(negatives()));
    passed(&c);
    let e = &c.evidence;
    assert!(e["stated"].as_u64() > Some(0) && e["surprises"].as_array().unwrap().is_empty(), "{e}");
    assert_eq!(e["examples"].as_array().unwrap().iter().filter(|x| x["ok"] == true).count(), 2, "{e}");
    assert!(kern::compare::summary(e).iter().any(|l| l.contains("as the task states")), "{:?}", kern::compare::summary(e));
    let mut beyond = candidate.clone();
    beyond.insert("src/money.js".into(), file(&MONEY_STATED.replace("return x * k;", "return Math.round(x * k);")));
    blocked(&compared(&base, &beyond, &v5(negatives())), "Behaviour changed outside the task's stated change: \"src/money.js#scale(");
}

#[test]
fn the_code_under_test_cannot_widen_the_stated_change() {
    let delta = json!({ "intent": "Scaling a 7 adds one; nothing else changes", "changes": [{ "function": "src/money.js#scale", "when": "(x) => String(x).startsWith('7')" }] });
    let base = files(&[("src/money.js", MONEY)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(&MONEY.replace("return x * k;", "return x * k + 1;").replace("module.exports", "String.prototype.startsWith = function () { return true; };\nmodule.exports")));
    let c = compared(&base, &candidate, &v5(delta.clone()));
    blocked(&c, "Behaviour changed outside the task's stated change: \"src/money.js#scale(");
    let mut honest = base.clone();
    honest.insert("src/money.js".into(), file(&MONEY.replace("return x * k;", "return String(x).startsWith('7') ? x * k + 1 : x * k;")));
    let h = compared(&base, &honest, &v5(delta));
    passed(&h);
    assert!(h.evidence["stated"].as_u64() > Some(0), "{}", h.evidence);
}

#[test]
fn a_new_crash_is_refused_even_without_a_delta() {
    let base = files(&[("src/money.js", MONEY)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(&MONEY.replace("function scale(x, k) { return x * k; }", "function scale(x, k) {\n  if (typeof x !== 'number') throw new TypeError('x must be a number');\n  return x * k;\n}")));
    let c = compared(&base, &candidate, &v5(Value::Null));
    blocked(&c, "New crash where the old code worked: \"src/money.js#scale(");
    assert!(c.block.unwrap().contains("now throws TypeError: x must be a number"));
    let mut rounded = base.clone();
    rounded.insert("src/money.js".into(), file(&MONEY.replace("return x * k;", "return Math.round(x * k);")));
    let r = compared(&base, &rounded, &v5(Value::Null));
    passed(&r);
    assert!(r.evidence["differences"].as_array().unwrap().iter().any(|d| d["kind"] == "changed"), "{}", r.evidence);
    let lines = kern::compare::summary(&r.evidence);
    assert!(lines.iter().any(|l| l.starts_with("Changed (changed): src/money.js#scale(")) && !lines.iter().any(|l| l.contains("without being stated")), "{lines:?}");
}

#[test]
fn an_input_the_old_code_refused_must_still_be_refused() {
    let bank = "function withdraw(balance, amount) {\n  if (typeof amount !== 'number' || !(amount > 0)) throw new RangeError('invalid amount');\n  return balance - amount;\n}\nmodule.exports = { withdraw };\n";
    let base = files(&[("src/bank.js", bank)]);
    let mut open = base.clone();
    open.insert("src/bank.js".into(), file("const withdraw = (balance, amount) => balance - amount;\nmodule.exports = { withdraw };\n"));
    let c = compared(&base, &open, &nothing_changes());
    blocked(&c, "Behaviour changed outside the task's stated change: \"src/bank.js#withdraw(");
    assert!(c.block.as_deref().unwrap().contains("threw RangeError: invalid amount, now returns"), "{:?}", c.block);
    assert!(c.evidence["differences"].as_array().unwrap().iter().any(|d| d["kind"] == "now accepted"), "{}", c.evidence);
    let label = "function label(user) {\n  if (!user || typeof user.name !== 'string') throw new RangeError('a user needs a name');\n  return user.name.toUpperCase();\n}\nmodule.exports = { label };\n";
    let base = files(&[("src/label.js", label)]);
    let mut deeper = base.clone();
    deeper.insert("src/label.js".into(), file("function label(user) { return user.name.toUpperCase(); }\nmodule.exports = { label };\n"));
    let c = compared(&base, &deeper, &nothing_changes());
    blocked(&c, "threw RangeError: a user needs a name, now throws TypeError");
    let mut words = base.clone();
    words.insert("src/label.js".into(), file(&label.replace("a user needs a name", "label() needs a user with a name")));
    let c = compared(&base, &words, &nothing_changes());
    passed(&c);
    assert!(c.evidence["differences"].as_array().unwrap().iter().any(|d| d["kind"] == "changed message"), "{}", c.evidence);
    let mut exits = base.clone();
    exits.insert("src/label.js".into(), file(&label.replace("throw new RangeError('a user needs a name')", "process.exit(3)")));
    blocked(&compared(&base, &exits, &v5(Value::Null)), "threw RangeError: a user needs a name, now stops its thread");
}

#[test]
fn removed_files_and_exports_must_be_stated() {
    let base = files(&[("src/money.js", MONEY), ("src/legacy.js", "module.exports = { old: () => 1 };\n")]);
    let mut gone = base.clone();
    gone.remove("src/legacy.js");
    blocked(&compared(&base, &gone, &v5(Value::Null)), "Removed without the task stating it: \"src/legacy.js\"");
    let mut stated = nothing_changes();
    stated["delta"]["removes"] = json!(["src/legacy.js"]);
    passed(&compared(&base, &gone, &stated));
    let mut export = base.clone();
    export.insert("src/money.js".into(), file("function add(a, b) { return a + b; }\nmodule.exports = { add };\n"));
    blocked(&compared(&base, &export, &v5(Value::Null)), "Removed without the task stating it: \"src/money.js#scale\"");
}

#[test]
fn the_tasks_examples_must_hold() {
    let base = files(&[("src/money.js", MONEY)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(MONEY_STATED));
    let mut wrong = negatives();
    wrong["changes"][0]["examples"][1]["returns"] = json!("6");
    blocked(&compared(&base, &candidate, &v5(wrong)), "The task's examples do not hold: \"src/money.js#add([2, 3]): expected to return 6, but it returned 5\"");
}

#[test]
fn a_hanging_call_is_a_timeout_not_a_stalled_probe() {
    let base = files(&[("src/money.js", MONEY), ("test/money.test.js", MONEY_TEST)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(&MONEY.replace("function scale(x, k) { return x * k; }", "function scale(x, k) { while (k === 7) {} return x * k; }")));
    let started = std::time::Instant::now();
    let c = compared(&base, &candidate, &v5(Value::Null));
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    blocked(&c, "src/money.js#scale([3, 7]): returned 21, now did not finish in time");
}

#[test]
fn messages_from_the_code_under_test_are_never_trusted() {
    let base = files(&[("src/f.js", "exports.f = (x) => 'same';\n")]);
    let forged = "const { parentPort } = require('worker_threads');\nexports.f = (x) => {\n  if (parentPort) {\n    for (const k of [0, 1, 2]) parentPort.postMessage({ kind: 'call', name: 'f', t: 0, k, out: 'r \"same\"', us: 1 });\n    parentPort.postMessage({ kind: 'example', i: 0, out: 'r 5', ok: true });\n    parentPort.postMessage({ kind: 'done' });\n  }\n  return 'changed';\n};\n";
    let mut candidate = base.clone();
    candidate.insert("src/f.js".into(), file(forged));
    let c = compared(&base, &candidate, &nothing_changes());
    blocked(&c, "Behaviour changed outside the task's stated change: \"src/f.js#f(");
    assert!(c.evidence["skipped"] == 0 && c.evidence["different"].as_u64() > Some(50), "every call was made and compared: {}", c.evidence);
    let example = v5(json!({ "intent": "f returns 5 for 1", "changes": [{ "function": "src/f.js#f", "when": "(x) => x === 1", "examples": [{ "args": "[1]", "returns": "5" }] }] }));
    blocked(&compared(&base, &candidate, &example), "The task's examples do not hold: \"src/f.js#f([1]): expected to return 5, but it returned \\\"changed\\\"\"");
}

#[test]
fn calls_the_new_code_never_answered_are_refused() {
    let base = files(&[("src/slow.js", "exports.f = (x) => 1;\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/slow.js".into(), file("exports.f = (x) => { const end = process.hrtime.bigint() + 300000000n; while (process.hrtime.bigint() < end) {} return 1; };\n"));
    let c = compare_with(&base, &candidate, &v5(Value::Null), |p| p["budget_ms"] = json!(3000), Default::default());
    blocked(&c, "The new code did not answer calls the old code answered: \"src/slow.js#f(");
    assert!(c.block.as_deref().unwrap().contains("returned 1, now was not answered"), "{:?}", c.block);
}

#[test]
fn every_function_is_called_whatever_its_name() {
    let mut old = String::new();
    for i in 0..30 { old += &format!("exports.f{i:02} = (x) => x;\n"); }
    let new = format!("{old}exports.zvalidate = (x) => {{ if (x === null) throw new TypeError('null'); return x; }};\n");
    let old = format!("{old}exports.zvalidate = (x) => x;\n");
    let base = files(&[("src/many.js", &old)]);
    let mut candidate = base.clone();
    candidate.insert("src/many.js".into(), file(&new));
    let c = compare_with(&base, &candidate, &v5(Value::Null), |p| p["max_calls"] = json!(600), Default::default());
    blocked(&c, "New crash where the old code worked: \"src/many.js#zvalidate([null]): returned null, now throws TypeError: null\"");
    assert_eq!(c.evidence["probed"]["functions"], 31, "{}", c.evidence);
}

#[test]
fn differences_past_what_is_shown_still_count() {
    for (name, old, new) in [
        ("long", "() => 'a'.repeat(2000)", "() => 'a'.repeat(1000) + '<script>' + 'a'.repeat(992)"),
        ("list", "() => Array.from({ length: 300 }, (_, i) => i)", "() => Array.from({ length: 300 }, (_, i) => (i === 250 ? -1 : i))"),
        ("bytes", "() => Buffer.alloc(128)", "() => { const b = Buffer.alloc(128); b[99] = 1; return b; }"),
        ("deep", "() => ({ a: { b: { c: { d: { e: { f: { g: { h: 1 } } } } } } } })", "() => ({ a: { b: { c: { d: { e: { f: { g: { h: 2 } } } } } } } })"),
        ("keys", "() => Object.fromEntries(Array.from({ length: 300 }, (_, i) => ['k' + String(i).padStart(3, '0'), i]))", "() => Object.fromEntries(Array.from({ length: 300 }, (_, i) => ['k' + String(i).padStart(3, '0'), i === 299 ? 0 : i]))"),
        ("lines", "() => 'a\\nb'", "() => 'a b'"),
    ] {
        let base = files(&[("src/v.js", &format!("exports.{name} = {old};\n"))]);
        let mut candidate = base.clone();
        candidate.insert("src/v.js".into(), file(&format!("exports.{name} = {new};\n")));
        blocked(&compared(&base, &candidate, &nothing_changes()), &format!("Behaviour changed outside the task's stated change: \"src/v.js#{name}("));
    }
}

#[test]
fn exports_are_compared_by_name_aliases_included() {
    let base = files(&[("src/date.js", "function formatDate(d) { return 'day ' + d; }\nmodule.exports = { formatDate };\n")]);
    let mut alias = base.clone();
    alias.insert("src/date.js".into(), file("function formatDate(d) { return 'day ' + d; }\nmodule.exports = { format: formatDate, formatDate };\n"));
    passed(&compared(&base, &alias, &nothing_changes()));
    let both = files(&[("src/json.js", "function parse(s) { return JSON.parse(s); }\nmodule.exports = { parse, parseJSON: parse };\n")]);
    let mut one = both.clone();
    one.insert("src/json.js".into(), file("function parse(s) { return JSON.parse(s); }\nmodule.exports = { parse };\n"));
    blocked(&compared(&both, &one, &v5(Value::Null)), "Removed without the task stating it: \"src/json.js#parseJSON\"");
    let mut many = String::new();
    for i in 10..80 { many += &format!("exports.e{i} = (x) => {i};\n"); }
    let base = files(&[("src/many.js", &many)]);
    let mut added = base.clone();
    added.insert("src/many.js".into(), file(&format!("exports.a00 = (x) => 0;\n{many}")));
    passed(&compared(&base, &added, &nothing_changes()));
    let guard = files(&[("src/auth.js", "exports.isAdmin = (user) => user === 'root';\n")]);
    let mut gone = guard.clone();
    gone.insert("src/auth.js".into(), file("exports.isAdmin = undefined;\n"));
    blocked(&compared(&guard, &gone, &v5(Value::Null)), "Removed without the task stating it: \"src/auth.js#isAdmin\"");
}

#[test]
fn a_crash_on_one_of_the_two_new_runs_is_a_crash() {
    let base = files(&[("src/once.js", "exports.f = (x) => 1;\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/once.js".into(), file("const fs = require('fs'), path = require('path');\nconst mark = path.join(__dirname, '.ran');\nconst first = !fs.existsSync(mark);\nif (first) fs.writeFileSync(mark, '1');\nexports.f = (x) => { if (first) throw new TypeError('first run'); return 1; };\n"));
    blocked(&compared(&base, &candidate, &v5(Value::Null)), "New crash where the old code worked: \"src/once.js#f(");
}

#[test]
fn the_old_tests_and_the_old_code_choose_the_inputs_first() {
    let module = "const LIMIT = 4096;\nexports.over = (x, y) => x * y > LIMIT;\n";
    let base = files(&[("src/m.js", module), ("test/m.test.js", "const { over } = require('../src/m');\nover(3, 7);\n")]);
    let mut calls = String::from("const { over } = require('../src/m');\n");
    for i in 0..200 { calls += &format!("over(0, {i});\n"); }
    let words: Vec<String> = (0..60).map(|i| format!("'w{i:02}'")).collect();
    let mut candidate = base.clone();
    candidate.insert("test/aaa.test.js".into(), file(&calls));
    candidate.insert("src/m.js".into(), file(&format!("{module}exports.words = [{}];\n", words.join(", "))));
    let policy = contract(&nothing_changes()).unwrap();
    let plan = kern::compare::plan(&base, &candidate, &git::changes(&base, &candidate, &policy).unwrap(), &policy, &"5eed".repeat(16), 60).unwrap();
    let tuples = plan["modules"][0]["tuples"].as_array().unwrap();
    assert!(tuples.iter().any(|t| t == "[3, 7]") && tuples.iter().any(|t| t == "[4096]"), "{tuples:?}");
}

#[test]
fn a_module_that_no_longer_loads_is_excused_only_when_the_whole_module_is_stated() {
    let base = files(&[("src/m.js", "exports.f = (x) => 1;\n")]);
    let mut broken = base.clone();
    broken.insert("src/m.js".into(), file("throw new Error('boom');\n"));
    let narrow = v5(json!({ "intent": "f changes for 1", "changes": [{ "function": "src/m.js#*", "when": "(x) => x === 1" }] }));
    blocked(&compared(&base, &broken, &narrow), "New crash where the old code worked: \"src/m.js: loaded before, now fails to load");
    let whole = v5(json!({ "intent": "src/m.js is rewritten", "changes": [{ "function": "src/m.js#*" }] }));
    passed(&compared(&base, &broken, &whole));
}

#[test]
fn a_module_that_fails_to_load_again_keeps_what_it_answered() {
    let module = "const fs = require('fs'), path = require('path');\nconst mark = path.join(__dirname, '.loaded-' + process.pid);\nif (fs.existsSync(mark)) throw new Error('loaded twice');\nfs.writeFileSync(mark, '1');\nexports.f = (x) => { if (x === 10) for (;;) {} return typeof x; };\n";
    let base = files(&[("src/r.js", module)]);
    let mut candidate = base.clone();
    candidate.insert("src/r.js".into(), file(&format!("// the same\n{module}")));
    let c = compared(&base, &candidate, &nothing_changes());
    passed(&c);
    assert!(c.evidence["probed"]["modules"] == 1 && c.evidence["same"].as_u64() > Some(3) && c.evidence["probed"]["not_loadable"].as_array().unwrap().is_empty(), "{}", c.evidence);
}

#[test]
fn a_module_that_never_finishes_loading_is_not_checked_rather_than_stalling_the_probe() {
    let stuck = "for (;;) {}\nexports.add = (a, b) => a + b;\n";
    let base = files(&[("src/stuck.js", stuck), ("src/money.js", MONEY)]);
    let mut candidate = base.clone();
    candidate.insert("src/stuck.js".into(), file(&format!("// still stuck\n{stuck}")));
    candidate.insert("src/money.js".into(), file(&format!("// tidied\n{MONEY}")));
    let started = std::time::Instant::now();
    let c = compare_with(&base, &candidate, &nothing_changes(), |p| p["load_ms"] = json!(5000), Default::default());
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    passed(&c);
    assert!(c.evidence["probed"]["not_loadable"][0].as_str().is_some_and(|s| s.contains("src/stuck.js") && s.contains("did not finish loading in time")), "{}", c.evidence);
    assert_eq!(c.evidence["probed"]["modules"], 1, "the module that loads is still compared: {}", c.evidence);
    assert_eq!(kern::compare::plan(&base, &candidate, &git::changes(&base, &candidate, &contract(&nothing_changes()).unwrap()).unwrap(), &contract(&nothing_changes()).unwrap(), &"5eed".repeat(16), 60).unwrap()["load_ms"], 20000);
}

#[test]
fn randomness_and_time_do_not_count_as_changes() {
    let clock = "exports.now = () => Date.now();\nexports.today = () => new Date().toISOString();\nexports.roll = () => Math.random();\nexports.id = () => require('crypto').randomUUID();\nexports.tick = () => String(process.hrtime.bigint());\n";
    let base = files(&[("src/clock.js", clock)]);
    let mut candidate = base.clone();
    candidate.insert("src/clock.js".into(), file(&format!("// the same clock\n{clock}")));
    let c = compared(&base, &candidate, &nothing_changes());
    passed(&c);
    assert!(c.evidence["noisy"].as_u64() > Some(0), "a real clock is noise, never a change: {}", c.evidence);
}

#[test]
fn much_slower_code_is_shown_and_refused_only_when_the_task_says_nothing_changes() {
    let base = files(&[("src/sum.js", "exports.sum = (n) => 'done';\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/sum.js".into(), file("exports.sum = (n) => { let s = 0; for (let i = 0; i < 3e6; i++) s += i; return s >= 0 ? 'done' : 'never'; };\n"));
    let report = compared(&base, &candidate, &v5(Value::Null));
    passed(&report);
    assert!(report.evidence["speed"].as_array().unwrap().iter().any(|s| s["function"] == "src/sum.js#sum"), "{}", report.evidence);
    let slow = compared(&base, &candidate, &nothing_changes());
    blocked(&slow, "Slower than the old code: \"src/sum.js#sum:");
    assert!(slow.block.as_deref().is_some_and(|b| !b.contains(" 0.0 ms before") && regex_like_ms(b)), "{:?}", slow.block);
    let mut allowed = nothing_changes();
    allowed["delta"]["allow"] = json!(["speed"]);
    passed(&compared(&base, &candidate, &allowed));
}

#[test]
fn new_paid_api_calls_are_counted() {
    let base = files(&[("src/ai.js", "exports.ask = (client, q) => client.chat.completions.create({ messages: [q] });\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/ai.js".into(), file("exports.ask = (client, q) => {\n  client.chat.completions.create({ messages: [q] });\n  return client.chat.completions.create({ messages: [q, q] });\n};\n"));
    let counts = [std::collections::BTreeMap::from([("net:api.openai.com:443".to_string(), 1)]), std::collections::BTreeMap::from([("net:api.openai.com:443".to_string(), 3), ("net:example.com:443".to_string(), 5)])];
    let report = compare_with(&base, &candidate, &v5(Value::Null), |_| {}, counts.clone());
    passed(&report);
    let cost = &report.evidence["cost"];
    assert_eq!(cost["calls"], json!([{ "path": "src/ai.js", "call": "chat.completions.create", "old": 1, "new": 2 }]), "{cost}");
    assert_eq!(cost["hosts"], json!([{ "host": "api.openai.com", "old": 1, "new": 3 }]), "only paid APIs are listed: {cost}");
    blocked(&compare_with(&base, &candidate, &nothing_changes(), |_| {}, counts), "More paid API calls than the old code: \"src/ai.js: chat.completions.create, 1 before, 2 now\"");
}

#[test]
fn new_code_that_deletes_files_is_refused_unless_the_task_allows_it() {
    let base = files(&[("src/clean.js", "const fs = require('fs');\nexports.clean = (d) => fs.existsSync(d);\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/clean.js".into(), file("const fs = require('fs');\nexports.clean = (d) => { if (String(d).startsWith('cache-')) fs.rmSync(d, { recursive: true, force: true }); return fs.existsSync(d); };\n"));
    blocked(&compared(&base, &candidate, &v5(Value::Null)), "New code deletes files: \"src/clean.js: fs.rmSync\"");
    let mut allowed = v5(json!({ "intent": "Clean caches", "changes": [{ "function": "src/clean.js#clean" }], "allow": ["delete"] }));
    allowed["delta"]["changes"][0]["examples"] = json!([]);
    passed(&compared(&base, &candidate, &allowed));
}

#[test]
fn the_access_lens_finds_a_door_that_opened() {
    let server = |guard: bool| format!("const http = require('http');\nhttp.createServer((req, res) => {{\n  if (req.url === '/admin'{}) {{ res.statusCode = 401; return res.end('no'); }}\n  res.end('ok');\n}}).listen(Number(process.env.PORT), '127.0.0.1');\n",
        if guard { " && req.headers.authorization !== 'Bearer admin'" } else { " && false" });
    let base = files(&[("src/server.js", &server(true))]);
    let mut candidate = base.clone();
    candidate.insert("src/server.js".into(), file(&server(false)));
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let app = move |plan: &mut Value| {
        plan["modules"] = json!([]);
        plan["app"] = json!({ "setup": null, "start": ["node", "src/server.js"], "port": port, "ready": "/", "routes": ["GET /", "GET /admin"],
            "roles": { "admin": { "Authorization": "Bearer admin" }, "anonymous": {} }, "env": {} });
    };
    let c = compare_with(&base, &candidate, &v5(Value::Null), app, Default::default());
    blocked(&c, "Access widened: \"GET /admin as anonymous: 401 before, 200 now\"");
    passed(&compare_with(&base, &base.clone(), &v5(Value::Null), app, Default::default()));
    let export = |open: bool| format!("const http = require('http');\nhttp.createServer((req, res) => {{\n  if (req.url === '/admin/export' && {open}) return res.end('all the data');\n  if (req.url === '/admin/export') {{ res.statusCode = 404; return res.end(); }}\n  res.end('ok');\n}}).listen(Number(process.env.PORT), '127.0.0.1');\n");
    let base = files(&[("src/server.js", &export(false))]);
    let mut candidate = base.clone();
    candidate.insert("src/server.js".into(), file(&export(true)));
    let routes = move |plan: &mut Value| { app(plan); plan["app"]["routes"] = json!(["GET /", "GET /admin/export"]); };
    blocked(&compare_with(&base, &candidate, &v5(Value::Null), routes, Default::default()), "Access widened: \"GET /admin/export as admin: 404 before, 200 now\"");
    let flaky = "const http = require('http'), fs = require('fs'), path = require('path');\nconst mark = path.join(__dirname, '.ran');\nconst first = !fs.existsSync(mark);\nif (first) fs.writeFileSync(mark, '1');\nhttp.createServer((req, res) => {\n  if (req.url === '/admin' && !first && req.headers.authorization !== 'Bearer admin') { res.statusCode = 401; return res.end('no'); }\n  res.end('ok');\n}).listen(Number(process.env.PORT), '127.0.0.1');\n";
    let base = files(&[("src/server.js", &server(true))]);
    let mut candidate = base.clone();
    candidate.insert("src/server.js".into(), file(flaky));
    blocked(&compare_with(&base, &candidate, &v5(Value::Null), app, Default::default()), "Access widened: \"GET /admin as anonymous: 401 before, 200 now\"");
}

#[test]
fn removed_code_is_explained_by_what_it_did_and_who_added_it() {
    let guarded = "function add(a, b) {\n  if (a === undefined) return 0;\n  return a + b;\n}\nmodule.exports = { add };\n";
    let base = files(&[("src/money.js", guarded)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file("function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n"));
    let c = compared(&base, &candidate, &v5(Value::Null));
    assert_eq!(c.plan["removals"], json!([{ "path": "src/money.js", "start": 2, "count": 1, "file": false }]), "{}", c.plan);
    let removal = &c.evidence["removals"][0];
    let shown: Vec<&str> = removal["examples"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(removal["changed"].as_u64() > Some(0) && shown.contains(&"src/money.js#add([undefined]): returns 0 with it, returns NaN without it"), "{}", c.evidence);
    assert!(kern::compare::summary(&c.evidence).iter().any(|l| l.contains("src/money.js line 2")), "{:?}", kern::compare::summary(&c.evidence));
    let t = Temp::new();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let git = |args: &[&str]| { let o = std::process::Command::new("git").args(args).current_dir(&repo).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").output().unwrap(); assert!(o.status.success(), "{o:?}"); String::from_utf8_lossy(&o.stdout).trim().to_string() };
    git(&["init", "-q"]);
    git(&["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("src/money.js"), "function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "Add money"]);
    std::fs::write(repo.join("src/money.js"), guarded).unwrap();
    git(&["commit", "-qam", "Guard missing amounts (#12)"]);
    let head = git(&["rev-parse", "HEAD"]);
    let history = kern::compare::history(&repo, &head, &c.plan);
    assert_eq!((history[0]["path"].as_str(), history[0]["lines"].as_str(), history[0]["commits"][0]["summary"].as_str(), history[0]["commits"][0]["commit"].as_str()), (Some("src/money.js"), Some("2"), Some("Guard missing amounts (#12)"), Some(head.as_str())));
}

#[test]
fn removed_code_is_found_with_a_real_line_diff() {
    let policy = contract(&v5(Value::Null)).unwrap();
    let base = files(&[("src/x.js", "a\nb\nc\nd\ne\nf\ng\n"), ("src/gone.js", "1;\n2;\n")]);
    let mut candidate = base.clone();
    candidate.insert("src/x.js".into(), file("a\nc\nd\nE\nf\n"));
    candidate.remove("src/gone.js");
    let changed = git::changes(&base, &candidate, &policy).unwrap();
    assert_eq!(kern::compare::removed_code(&base, &candidate, &changed, &policy), vec![
        json!({ "path": "src/gone.js", "start": 1, "count": 2, "file": true }),
        json!({ "path": "src/x.js", "start": 2, "count": 1, "file": false }), json!({ "path": "src/x.js", "start": 7, "count": 1, "file": false }),
    ]);
    let plan = json!({ "removals": kern::compare::removed_code(&base, &candidate, &changed, &policy) });
    let without = kern::compare::without_removals(&base, &plan);
    assert_eq!((without["src/x.js"].bytes.as_deref(), without.contains_key("src/gone.js")), (Some(&b"a\nc\nd\ne\nf\n"[..]), false));
}

#[test]
fn probe_output_must_be_exactly_one_report() {
    let one = format!("noise\n\u{1e}kern-probe\u{1e}\n{}\n", json!({ "version": 1, "modules": [], "app": null }));
    kern::compare::read_probe(one.as_bytes()).unwrap();
    denied(kern::compare::read_probe(b"nothing"), "no probe report");
    denied(kern::compare::read_probe(format!("{one}{one}").as_bytes()), "more than one probe report");
}

#[test]
fn a_plan_is_the_same_for_the_same_change() {
    let base = files(&[("src/money.js", MONEY), ("test/money.test.js", MONEY_TEST)]);
    let mut candidate = base.clone();
    candidate.insert("src/money.js".into(), file(MONEY_STATED));
    let policy = contract(&v5(negatives())).unwrap();
    let changed = git::changes(&base, &candidate, &policy).unwrap();
    let plan = |seed: &str| kern::compare::plan(&base, &candidate, &changed, &policy, seed, 60).unwrap();
    assert_eq!(plan(&"a".repeat(64)), plan(&"a".repeat(64)));
    assert_ne!(plan(&"a".repeat(64))["modules"], plan(&"b".repeat(64))["modules"], "the seed picks the random inputs");
    let p = plan(&"a".repeat(64));
    assert_eq!((p["changes"][0]["function"].as_str(), p["examples"].as_array().map(Vec::len)), (Some("src/money.js#add"), Some(2)), "{p}");
    canonical(&p).expect("the plan is canonical JSON");
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}
fn operator_value() -> Value {
    json!({ "version": 3, "repository": "acme/project", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": "/operator/checks/addition.json", "sha256": "b".repeat(64) }], "registry": "registry.npmjs.org", "permissions": {} })
}
struct Repo { _t: Temp, dir: PathBuf, base: String, head: String, key: ed25519_dalek::SigningKey }
fn repo_with(task: &Value, base_files: &[(&str, &str)], head_files: &[(&str, &str)]) -> Repo {
    let t = Temp::new();
    let dir = t.join("repo");
    std::fs::create_dir_all(&dir).unwrap();
    sh(&dir, &["init", "-q"]);
    sh(&dir, &["config", "core.autocrlf", "false"]);
    let key = ed25519_dalek::SigningKey::from_bytes(&random_bytes::<32>().unwrap());
    let public = kern::attest::public_pem(&key.verifying_key()).unwrap();
    let config = [(".github/kern/operator.json", operator_value().to_string()), (".github/kern/verifier.pub", public), (".github/kern/tasks/fix-addition.json", task.to_string())];
    tree(&dir, &config.iter().map(|(p, c)| (*p, c.as_str())).collect::<Vec<_>>());
    tree(&dir, base_files);
    sh(&dir, &["add", "-A"]);
    sh(&dir, &["commit", "-qm", "base"]);
    let base = sh(&dir, &["rev-parse", "HEAD"]);
    let mut candidate = git::read_commit(&dir, &base, 1048576, 16777216).unwrap();
    for (p, c) in head_files { candidate.insert(p.to_string(), file(c)); }
    let head = git::write_commit(&dir, &candidate, Some(&base), "fix-addition").unwrap();
    Repo { _t: t, dir, base, head, key }
}
fn proof_for(r: &Repo, task: &Value, run_id: &str, evidence: &Value) -> Value {
    let (policy, op) = (contract(task).unwrap(), policy::operator(&operator_value()).unwrap());
    let manifest = digest(&git::manifest(&r.dir, &r.head, 65536, 1048576).unwrap()).unwrap();
    let checks: Vec<Value> = kern::flow::required_checks(&policy).iter().map(|id| json!({ "id": id, "status": "PASS", "evidence_sha256": if id == "compare" { digest(evidence).unwrap() } else { "c".repeat(64) } })).collect();
    let issued = now_ms() - 1000;
    let k = json!({ "run_id": run_id, "task": policy.task, "repository": op.repository, "base": r.base, "commit": r.head, "manifest_sha256": manifest, "contract_sha256": policy::contract_digest(&policy).unwrap(),
        "verifier_sha256": policy::verifier_digest(&op).unwrap(), "executor": { "kind": "external", "builder_sha256": null, "source_commit": r.head }, "verifier_builder_sha256": op.verifier_builder,
        "deployment": "single-vm", "image": op.image, "runtime": "runsc", "dependencies_sha256": null, "checks": checks, "issued_at": issued, "expires_at": issued + 60_000 });
    kern::attest::sign(&k, &kern::attest::KeySource::File(r.key.clone())).unwrap()
}
fn stated_evidence() -> Value {
    json!({ "policy": "kern-compare/v1", "mode": "delta", "intent": "Negative amounts are refused with a RangeError", "network": "off", "probed": { "modules": 1, "functions": 2, "calls": 480, "not_loadable": [] },
        "same": 443, "different": 37, "noisy": 0, "skipped": 0, "stated": 37, "differences": [], "crashes": [], "surprises": [], "removed": [], "deletes": [],
        "examples": [{ "call": "src/money.js#add([-1, 2])", "expected": "throw RangeError", "actual": "threw RangeError: negative amount", "ok": true }],
        "speed": [], "access": null, "removals": [], "cost": { "calls": [], "hosts": [] } })
}

#[test]
fn the_comparison_travels_with_the_proof_and_reviewers_read_it_on_the_pull_request() {
    let task = v5(negatives());
    let r = repo_with(&task, &[("src/money.js", MONEY)], &[("src/money.js", MONEY_STATED)]);
    let run_id = uuid4().unwrap();
    let evidence = stated_evidence();
    let envelope = proof_for(&r, &task, &run_id, &evidence);
    assert!(kern::compare::attested(&envelope, &evidence), "the proof's comparison check covers exactly this evidence");
    let d = kern::promote::Decision { repository: "acme/project".into(), branch: format!("kern/fix-addition/{run_id}"), base: r.base.clone(), commit: r.head.clone(), run_id: run_id.clone(), attestation: Value::Null };
    let body = kern::promote::pr_body("fix-addition", &d, &envelope, Some(&evidence)).unwrap();
    assert!(body.contains("### What changed") && body.contains("`Negative amounts are refused with a RangeError`") && body.contains("37 changed as the task states"), "{body}");
    assert_eq!((kern::promote::extract_evidence(&body), kern::promote::extract_attestation(&body).unwrap()), (Some(evidence.clone()), envelope.clone()));
    let event = |body: &str| json!({ "pull_request": { "head": { "sha": r.head, "ref": d.branch }, "base": { "sha": r.base, "repo": { "full_name": "acme/project" } }, "body": body } });
    let pass = kern::promote::check_pr(&event(&body), &r.dir, ".github/kern", now_ms()).unwrap();
    assert_eq!(pass["status"], "PASS", "{pass}");
    assert!(pass["what_changed"].as_str().unwrap().contains("37 changed as the task states"), "{pass}");
    let shown = kern::promote::step_summary(&Ok(pass));
    assert!(shown.contains("#### What changed") && shown.contains("- The task states: `Negative amounts are refused with a RangeError`"), "{shown}");
    let title = "<img src=x onerror=alert(1)> [click](https://evil.invalid)";
    let hostile = json!({ "status": "PASS", "what_changed": kern::compare::markdown(&evidence), "removed_code": [{ "path": "src/a.js", "lines": "3",
        "added_by": [{ "commit": "abc", "summary": "s", "pull_request": { "number": 7, "title": title, "url": "u" }, "closes": [{ "number": 8, "title": title }] }] }] });
    let shown = kern::promote::step_summary(&Ok(hostile));
    let details = shown.find("\n#### Details\n\n").unwrap_or_else(|| panic!("{shown}"));
    assert!(shown[details..].lines().skip(3).filter(|l| !l.is_empty()).all(|l| l.starts_with("    ")), "{shown}");
    assert!(shown[..details].matches(&format!("`{title}`")).count() == 2 && !shown[..details].replace(&format!("`{title}`"), "").contains("<img"), "{shown}");
    let mut forged = evidence.clone();
    forged["crashes"] = json!([]);
    forged["same"] = json!(480);
    let quiet = kern::promote::check_pr(&event(&kern::promote::pr_body("fix-addition", &d, &envelope, Some(&forged)).unwrap()), &r.dir, ".github/kern", now_ms()).unwrap();
    assert!(quiet["status"] == "PASS" && quiet["what_changed"].is_null(), "{quiet}");
    let edited = kern::promote::with_attestation(&format!("Notes from the reviewer\n\n{}", body.replacen("Kern-verified", "Verified", 1)), "fix-addition", &d, &envelope, Some(&evidence)).unwrap();
    assert!(edited.starts_with("Notes from the reviewer") && edited.matches(kern::promote::EVIDENCE).count() == 1, "{edited}");
    let largest = largest_evidence();
    let big = kern::promote::pr_body("fix-addition", &d, &envelope, Some(&largest)).unwrap();
    assert!(big.len() <= 30_000 && big.contains("### What changed") && big.contains("- More in the verifier's comparison evidence"), "{} characters", big.len());
    assert_eq!(kern::promote::extract_attestation(&big).unwrap(), envelope);
    assert!(kern::compare::markdown(&largest).len() <= 8_100, "{}", kern::compare::markdown(&largest).len());
}
fn largest_evidence() -> Value {
    let long = |c: char| c.to_string().repeat(240);
    let items = |c: char| json!((0..20).map(|_| long(c)).collect::<Vec<_>>());
    let mut e = stated_evidence();
    e["intent"] = json!("x".repeat(1000));
    for k in ["crashes", "surprises", "removed", "deletes"] { e[k] = items('a'); }
    e["mode"] = json!("report");
    e["differences"] = json!((0..20).map(|_| json!({ "function": long('f'), "args": "a".repeat(200), "old": long('o'), "new": long('n'), "kind": "changed", "stated": false })).collect::<Vec<_>>());
    e["examples"] = json!((0..512).map(|_| json!({ "call": long('c'), "expected": long('e'), "actual": long('a'), "ok": false })).collect::<Vec<_>>());
    e["speed"] = json!((0..20).map(|_| json!({ "function": long('s'), "old_us": 1, "new_us": 99999 })).collect::<Vec<_>>());
    e["removals"] = json!((0..20).map(|_| json!({ "path": long('p'), "lines": "1-2, 4-9", "file": false, "changed": 3, "examples": [long('r'), long('r'), long('r')] })).collect::<Vec<_>>());
    e["cost"] = json!({ "calls": (0..20).map(|_| json!({ "path": long('p'), "call": "chat.completions.create", "old": 1, "new": 2 })).collect::<Vec<_>>(),
        "hosts": (0..20).map(|i| json!({ "host": format!("h{i}.openai.azure.com"), "old": 0, "new": 1 })).collect::<Vec<_>>() });
    e["access"] = json!({ "checked": 99, "opened": items('d'), "changed": items('g') });
    e["probed"]["not_loadable"] = items('l');
    e
}

#[test]
fn every_new_block_becomes_an_instruction_the_agent_can_follow() {
    use kern::feedback::explain;
    for (reason, rule, fixable, words) in [
        ("New crash where the old code worked: \"src/money.js#scale([\\\"a\\\"]): returned NaN, now throws TypeError: x must be a number\". The new code must handle every input the old code handled", "never-worse", true, &["`", "work again"][..]),
        ("Behaviour changed outside the task's stated change: \"src/money.js#scale([0.5, 3]): returned 1.5, now returns 2\". Everything the task did not state must behave exactly as before", "unstated-change", true, &["exactly as on the old code"][..]),
        ("The new code did not answer calls the old code answered: \"src/slow.js#f([1]): returned 1, now was not answered\". The new code must answer every call the old code answered, in about the same time", "unanswered", true, &["same time", "src/slow.js#f"][..]),
        ("Removed without the task stating it: \"src/legacy.js\". Restore it; deleting files or exports needs the task's delta to list them under removes", "unstated-removal", true, &["Restore", "src/legacy.js"][..]),
        ("New code deletes files: \"src/clean.js: fs.rmSync\". Code may not newly delete or truncate files unless the task's delta allows delete", "deletes-files", true, &["fs.rmSync"][..]),
        ("Access widened: \"GET /admin as anonymous: 401 before, 200 now\". A route that refused a user before must still refuse them", "access-widened", true, &["GET /admin"][..]),
        ("The task's examples do not hold: \"src/money.js#add([2, 3]): expected to return 6, but it returned 5\". The new code must give the answers the task states", "examples", true, &["answers the task states"][..]),
        ("Access changed outside the task's stated change: \"GET / as user: 200 before, 500 now\". Routes must answer as before", "access-changed", true, &["answer as before"][..]),
        ("Slower than the old code: \"src/sum.js#sum: 0.0 ms before, 3.1 ms now\". The task did not allow slower code", "slower", true, &["fast"][..]),
        ("More paid API calls than the old code: \"src/ai.js: chat.completions.create, 1 before, 2 now\". The task did not allow more paid API calls", "paid-calls", true, &["paid API"][..]),
        ("This task only upgrades packages the project already uses, but this change adds or removes: zod", "dependency", true, &["`zod`"][..]),
        ("Calling the new code shows new runtime behavior: delete:., overwrite:data/state.json. If this is intended, allow it in the task contract: \"behaviors\": [\"delete:.\",\"overwrite:data/state.json\"]",
            "behavior", true, &["`delete:.`", "`overwrite:data/state.json`", "when Kern calls"][..]),
        ("Kern could not compare the old and the new code: the probe run failed (exit code 1)", "compare", false, &["human"][..]),
    ] {
        let f = explain(reason, None);
        assert_eq!((f["violations"][0]["rule"].as_str(), f["agent_fixable"].as_bool()), (Some(rule), Some(fixable)), "{reason}: {f}");
        let instruction = f["violations"][0]["instruction"].as_str().unwrap();
        for w in words { assert!(instruction.contains(w), "{reason}: {instruction:?} lacks {w:?}"); }
        assert!(!["delta", "operator.json", "task contract", "allow"].iter().any(|loosen| instruction.contains(loosen)), "agents are told how to fix code, never how to loosen the gate: {instruction}");
        let agent = kern::feedback::for_agent(&f);
        assert!(agent.get("reason").is_none() && agent["violations"][0].get("allow").is_none(), "{agent}");
    }
    assert_eq!(explain("Sandbox crashed. New crash where the old code worked: \"x\". The new code must handle every input the old code handled", None)["agent_fixable"], false);
}

#[test]
fn a_candidate_names_who_added_the_code_it_removes() {
    let t = Temp::new();
    let source = t.join("source");
    std::fs::create_dir_all(source.join("src")).unwrap();
    sh(&source, &["init", "-q"]);
    sh(&source, &["config", "core.autocrlf", "false"]);
    std::fs::write(source.join("src/money.js"), "function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "Add money"]);
    std::fs::write(source.join("src/money.js"), "function add(a, b) {\n  if (a === undefined) return 0;\n  return a + b;\n}\nmodule.exports = { add };\n").unwrap();
    sh(&source, &["commit", "-qam", "Guard missing amounts (#12)"]);
    let base = sh(&source, &["rev-parse", "HEAD"]);
    sh(&t.0, &["clone", "-q", "source", "agent"]);
    let agent = t.join("agent");
    sh(&agent, &["config", "core.autocrlf", "false"]);
    std::fs::write(agent.join("src/money.js"), "function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n").unwrap();
    sh(&agent, &["commit", "-qam", "agent: simplify add"]);
    let commit = sh(&agent, &["rev-parse", "HEAD"]);
    std::fs::write(t.join("contract.json"), v5(Value::Null).to_string()).unwrap();
    std::fs::write(t.join("operator.json"), operator_value().to_string()).unwrap();
    std::fs::create_dir(t.join("keys")).unwrap();
    std::fs::write(t.join("keys/public.pem"), kern::attest::keypair().unwrap().1).unwrap();
    std::fs::write(t.join("job.json"), json!({ "version": 3, "repo": "source", "base": base, "contract": "contract.json", "operator": "operator.json", "command": null, "verification_key": "keys/public.pem", "output": "tasks" }).to_string()).unwrap();
    let r = kern::flow::task(&policy::job(&t.join("job.json")).unwrap(), kern::flow::Source::Ingest(&agent, &commit)).unwrap();
    assert_eq!(r["status"], "CANDIDATE", "{r}");
    assert_eq!((r["removed_code"][0]["path"].as_str(), r["removed_code"][0]["lines"].as_str(), r["removed_code"][0]["commits"][0]["summary"].as_str()), (Some("src/money.js"), Some("2"), Some("Guard missing amounts (#12)")), "{r}");
}

fn stub(route: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut reader = std::io::BufReader::new(s.try_clone().unwrap());
            let mut request = String::new();
            if std::io::BufRead::read_line(&mut reader, &mut request).is_err() { continue; }
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") { length = v.trim().parse().unwrap_or(0); }
            }
            let mut body = vec![0u8; length];
            let _ = std::io::Read::read_exact(&mut reader, &mut body);
            let parts: Vec<&str> = request.split_whitespace().collect();
            let (status, reply) = route(parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""), &String::from_utf8_lossy(&body));
            let _ = std::io::Write::write_all(&mut s, format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes());
        }
    });
    url
}

#[test]
fn the_merge_check_names_the_pull_request_that_added_removed_code() {
    let guarded = "function add(a, b) {\n  if (a === undefined) return 0;\n  return a + b;\n}\nmodule.exports = { add };\n";
    let task = v5(Value::Null);
    let r = repo_with(&task, &[("src/money.js", guarded)], &[("src/money.js", "function add(a, b) {\n  return a + b;\n}\nmodule.exports = { add };\n")]);
    let run_id = uuid4().unwrap();
    let evidence = stated_evidence();
    let envelope = proof_for(&r, &task, &run_id, &evidence);
    let d = kern::promote::Decision { repository: "acme/project".into(), branch: format!("kern/fix-addition/{run_id}"), base: r.base.clone(), commit: r.head.clone(), run_id: run_id.clone(), attestation: Value::Null };
    let blame = json!({ "data": { "repository": { "object": { "blame": { "ranges": [
        { "startingLine": 1, "endingLine": 1, "commit": { "oid": "a".repeat(40), "messageHeadline": "Add money", "associatedPullRequests": { "nodes": [] } } },
        { "startingLine": 2, "endingLine": 2, "commit": { "oid": "b".repeat(40), "messageHeadline": "Guard missing amounts", "associatedPullRequests": { "nodes": [
            { "number": 12, "title": "Guard missing amounts", "url": "https://github.com/acme/project/pull/12", "closingIssuesReferences": { "nodes": [{ "number": 11, "title": "Double refund" }] } }] } } },
        { "startingLine": 3, "endingLine": 5, "commit": { "oid": "a".repeat(40), "messageHeadline": "Add money", "associatedPullRequests": { "nodes": [] } } }] } } } } });
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let api = stub(move |method, path, body| { log.lock().unwrap().push((method.to_string(), path.to_string(), body.to_string())); if (method, path) == ("POST", "/graphql") { (200, blame.to_string()) } else { (404, "{}".into()) } });
    let event = r.dir.parent().unwrap().join("event.json");
    std::fs::write(&event, json!({ "pull_request": { "number": 7, "head": { "sha": r.head, "ref": d.branch }, "base": { "sha": r.base, "repo": { "full_name": "acme/project" } },
        "body": kern::promote::pr_body("fix-addition", &d, &envelope, Some(&evidence)).unwrap() } }).to_string()).unwrap();
    let summary = r.dir.parent().unwrap().join("summary.md");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kern")).arg("check-pr").arg(&event).arg(&r.dir).env("GITHUB_API_URL", &api).env("GITHUB_TOKEN", "t0k3n").env("GITHUB_STEP_SUMMARY", &summary).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    let added = &result["removed_code"][0];
    assert_eq!((added["path"].as_str(), added["lines"].as_str(), added["added_by"][0]["pull_request"]["number"].as_u64(), added["added_by"][0]["closes"][0]["number"].as_u64()), (Some("src/money.js"), Some("2"), Some(12), Some(11)), "{result}");
    let sent = seen.lock().unwrap().iter().find(|(m, p, _)| m == "POST" && p == "/graphql").cloned().expect("a blame query");
    assert!(sent.2.contains(&r.base) && sent.2.contains("src/money.js"), "the blame is read at the base the proof covers: {}", sent.2);
    let text = std::fs::read_to_string(&summary).unwrap();
    assert!(text.contains("#12") && text.contains("Guard missing amounts") && text.contains("#11"), "{text}");
}

#[test]
fn the_monitor_records_files_the_code_deletes_or_overwrites_and_counts_calls() {
    let t = Temp::new();
    let (ws, input, log) = (t.join("ws"), t.join("input"), t.join("behavior.log"));
    let repo = [("a.txt", "a"), ("b.txt", "b"), ("keep.txt", "k"), ("trunc.txt", "t"), ("dir/c.txt", "c"), ("esm/d.txt", "d"), ("node_modules/x/index.js", "x")];
    tree(&ws, &repo);
    tree(&input, &repo);
    std::fs::write(t.join("monitor.cjs"), kern::sandbox::MONITOR).unwrap();
    let script = "const fs = require('fs'), path = require('path'), os = require('os');
        fs.unlinkSync('a.txt'); fs.writeFileSync('b.txt', 'changed'); fs.truncateSync('trunc.txt'); fs.rmSync('dir', { recursive: true });
        fs.writeFileSync('new.txt', 'x'); fs.unlinkSync('new.txt');
        fs.mkdirSync('made'); fs.writeFileSync('made/x', '1'); fs.rmSync('made', { recursive: true });
        const tmp = path.join(os.tmpdir(), 'kern-monitor-' + process.pid); fs.writeFileSync(tmp, '1'); fs.unlinkSync(tmp);
        fs.readFileSync('keep.txt'); fs.appendFileSync('keep.txt', '');
        try { fs.unlinkSync('node_modules/x/index.js'); } catch {}
        for (let i = 0; i < 3; i++) require('net').connect(443, 'paid.example.invalid').on('error', () => {});
        fs.promises.unlink('missing.txt').catch(() => {});";
    let env = [("KERN_MONITOR", log.as_path()), ("KERN_WORKSPACE", ws.as_path()), ("KERN_INPUT", input.as_path())];
    let out = node(&["--require".as_ref(), t.join("monitor.cjs").as_os_str(), "-e".as_ref(), script.as_ref()], &ws, &env);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let esm = "import { rmSync } from 'node:fs'; rmSync('esm', { recursive: true });";
    let out = node(&["--require".as_ref(), t.join("monitor.cjs").as_os_str(), "--input-type=module".as_ref(), "-e".as_ref(), esm.as_ref()], &ws, &env);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&log).unwrap();
    let recorded = format!("{}{text}", kern::sandbox::BEHAVIOR);
    let (_, behavior) = kern::sandbox::split_behavior(recorded.as_bytes());
    let behavior = behavior.unwrap();
    for want in ["delete:a.txt", "overwrite:b.txt", "overwrite:trunc.txt", "delete:dir", "delete:esm", "net:paid.example.invalid:443"] { assert!(behavior.contains(want), "missing {want} in {behavior:?}"); }
    for never in ["delete:new.txt", "delete:made", "delete:node_modules/x/index.js", "overwrite:keep.txt", "delete:missing.txt"] { assert!(!behavior.contains(never), "{never} is not destruction: {behavior:?}"); }
    assert!(!behavior.iter().any(|b| b.contains("kern-monitor-")), "temporary files never count: {behavior:?}");
    assert!(!behavior.iter().any(|b| b.starts_with("count")), "counts are not behaviours: {behavior:?}");
    assert_eq!(kern::sandbox::behavior_counts(recorded.as_bytes()).get("net:paid.example.invalid:443"), Some(&3));
}

#[test]
fn the_monitor_knows_a_workspace_it_is_told_about_through_a_link() {
    let t = Temp::new();
    let (real, link, input, log) = (t.join("real"), t.join("link"), t.join("input"), t.join("behavior.log"));
    tree(&real, &[("a.txt", "a")]);
    tree(&input, &[("a.txt", "a")]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(windows)]
    assert!(std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&real).output().unwrap().status.success());
    std::fs::write(t.join("monitor.cjs"), kern::sandbox::MONITOR).unwrap();
    let env = [("KERN_MONITOR", log.as_path()), ("KERN_WORKSPACE", link.as_path()), ("KERN_INPUT", input.as_path())];
    let out = node(&["--require".as_ref(), t.join("monitor.cjs").as_os_str(), "-e".as_ref(), "require('fs').unlinkSync('a.txt')".as_ref()], &real, &env);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let recorded = format!("{}{}", kern::sandbox::BEHAVIOR, std::fs::read_to_string(&log).unwrap_or_default());
    let (_, behavior) = kern::sandbox::split_behavior(recorded.as_bytes());
    let behavior = behavior.unwrap();
    assert!(behavior.contains("delete:a.txt"), "the delete was not recorded as delete:a.txt: {behavior:?}");
}
