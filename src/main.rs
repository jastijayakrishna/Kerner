use kern::attest::{self, KeySource};
use kern::flow::{self, Source};
use kern::policy::{self, attenuate, contract, operator, require_role};
use kern::promote::{self, GitHub};
use kern::util::*;
use kern::{ensure, git, sandbox};
use serde_json::{Value, json};
use std::path::Path;

const HELP: &str = "kern 0.3 - admission control for AI-written code

Native box (developer-local, no Docker):
  box <box-job.json>              run one command inside the box
  run <run-job.json>              box an agent, gate its output, create a reversible branch
Executor (--control executor.json):
  doctor | plan <job> | task <job>
Any agent sandbox (OpenShell, Claude Code, Codex, ...):
  ingest <job> <agent-repo> <agent-commit>
Verifier (--control verifier.json):
  doctor | keygen <dir> | import <handoff-dir> <new-run-dir>
  verify <run-dir> <contract> <operator> <new-out-dir> <private.pem>
  verify-external <run-dir> <contract> <operator> <new-out-dir> <signer.json>
Promoter:
  authorize <run-dir> <attestation> <contract> <operator> <expected> <public.pem>
  promote-local <run-dir> <attestation> <contract> <operator> <expected> <public.pem> <target-git-dir>
  promote-github <run-dir> <attestation> <contract> <operator> <expected> <public.pem> <token-file> <base-branch> <ledger-dir> <git-pin.json>
GitHub required check:
  check-pr <event.json> <checkout-dir> [config-path=.github/kern]
Factory lock (deploy-time enforcement):
  gate-artifact <checkout-dir> <commit> <attestation.json> <trusted-config-dir>
Utilities:
  contract <contract> [narrower-contract] | pin-check <id> <file>";

fn count(args: &[String], n: usize) -> R<()> {
    ensure!(args.len() == n, "Expected {n} arguments; see kern --help");
    Ok(())
}
fn promotion(a: &[String]) -> R<(std::path::PathBuf, Value, kern::policy::Contract, kern::policy::Operator, Value, String)> {
    Ok((Path::new(&a[0]).join("objects.git"), read_json(&a[1])?, contract(&read_json(&a[2])?)?, operator(&read_json(&a[3])?)?, read_json(&a[4])?, std::fs::read_to_string(&a[5])?))
}
fn cli() -> R<(Option<Value>, i32)> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("__runner") => return sandbox::runner().map(|_| (None, 0)),
        Some("__setup") => return sandbox::setup().map(|_| (None, 0)),
        Some("__exec") => return sandbox::exec(&args[1..]).map(|_| (None, 0)),
        Some("--control") => {
            ensure!(args.len() >= 3, "Expected control JSON and a command");
            policy::configure(&read_json(&args[1])?)?;
            args.drain(0..2);
        }
        _ => {}
    }
    let (cmd, a) = args.split_first().map_or(("help", &[][..]), |(c, r)| (c.as_str(), r));
    let now = now_ms();
    Ok(match cmd {
        "help" | "--help" | "-h" => { println!("{HELP}"); (None, 0) }
        "--version" => (Some(json!({ "kern": env!("CARGO_PKG_VERSION"), "implementation_sha256": policy::implementation_digest()? })), 0),
        "box" => { count(a, 1)?; let r = kern::local::run_box_job(&kern::local::box_job(&read_json(&a[0])?)?)?; let code = r["exit_code"].as_i64().unwrap_or(1) as i32; (Some(r), code) }
        "run" => { count(a, 1)?; let r = kern::local::local_run(&kern::local::run_job(Path::new(&a[0]))?)?; if let Some(s) = r["summary"].as_str() { eprintln!("{s}"); } let code = i32::from(r["status"] != "GREEN"); (Some(r), code) }
        "doctor" => { count(a, 0)?; let d = sandbox::doctor(); let code = if d.ready { 0 } else { 2 }; (Some(d.report), code) }
        "plan" => { count(a, 1)?; let r = flow::preflight(&policy::job(Path::new(&a[0]))?, true); let code = i32::from(r["status"] != "READY"); (Some(r), code) }
        "task" => { count(a, 1)?; let r = flow::task(&policy::job(Path::new(&a[0]))?, Source::Agent)?; let code = i32::from(r["status"] != "CANDIDATE"); (Some(r), code) }
        "ingest" => { count(a, 3)?; let r = flow::task(&policy::job(Path::new(&a[0]))?, Source::Ingest(Path::new(&a[1]), &a[2]))?; let code = i32::from(r["status"] != "CANDIDATE"); (Some(r), code) }
        "contract" => {
            ensure!(a.len() == 1 || a.len() == 2, "Expected a contract and an optional narrower contract");
            let c = if a.len() == 2 { attenuate(&read_json(&a[0])?, &read_json(&a[1])?)? } else { contract(&read_json(&a[0])?)? };
            (Some(json!({ "contract": c.value, "sha256": policy::contract_digest(&c)? })), 0)
        }
        "keygen" => {
            count(a, 1)?;
            require_role("verifier")?;
            let dir = fresh_dir(&a[0])?;
            let (private, public) = attest::keypair()?;
            create_new(dir.join("private.pem"), private.as_bytes(), 0o600)?;
            create_new(dir.join("public.pem"), public.as_bytes(), 0o644)?;
            (Some(json!({ "public_key": dir.join("public.pem"), "private_key": dir.join("private.pem"), "key_id": attest::key_id(&attest::public_key(&public)?)? })), 0)
        }
        "pin-check" => { count(a, 2)?; (Some(json!({ "id": a[0], "file": std::path::absolute(&a[1])?, "sha256": sha256(std::fs::read(&a[1])?) })), 0) }
        "import" => { count(a, 2)?; require_role("verifier")?; (Some(git::import_handoff(Path::new(&a[0]), Path::new(&a[1]))?), 0) }
        "verify" | "verify-external" => {
            count(a, 5)?;
            let signer = if cmd == "verify" { KeySource::file(&std::fs::read_to_string(&a[4])?)? } else { KeySource::external(Path::new(&a[4]))? };
            let envelope = flow::verify(Path::new(&a[0]), &contract(&read_json(&a[1])?)?, &operator(&read_json(&a[2])?)?, Path::new(&a[3]), &signer)?;
            (Some(json!({ "status": "VERIFIED", "attestation": Path::new(&a[3]).join("attestation.json"), "attestation_sha256": digest(&envelope)? })), 0)
        }
        "authorize" | "promote-local" => {
            count(a, if cmd == "authorize" { 6 } else { 7 })?;
            let (store, envelope, c, op, expected, key) = promotion(a)?;
            let d = promote::authorize(&store, &envelope, &key, &c, &op, &expected, now)?;
            if cmd == "authorize" { (Some(json!({ "status": "AUTHORIZED", "repository": d.repository, "branch": d.branch, "base": d.base, "commit": d.commit })), 0) }
            else { (Some(promote::promote_local(&store, &d, &key, Path::new(&a[6]), None)?), 0) }
        }
        "promote-github" => {
            count(a, 10)?;
            let git_pin = promote::git_pin(Path::new(&a[9]))?;
            let (store, envelope, c, op, expected, key) = promotion(a)?;
            let d = promote::authorize(&store, &envelope, &key, &c, &op, &expected, now)?;
            let token = std::fs::read_to_string(&a[6])?;
            (Some(promote::promote_github(&store, &d, &c, &envelope, &key, GitHub { token: token.trim(), base_branch: &a[7], ledger: Path::new(&a[8]), git: git_pin })?), 0)
        }
        "gate-artifact" => { count(a, 4)?; (Some(promote::gate_artifact(Path::new(&a[0]), &a[1], &read_json(&a[2])?, Path::new(&a[3]), now)?), 0) }
        "check-pr" => {
            ensure!(a.len() == 2 || a.len() == 3, "Expected event JSON, checkout and optional config path");
            let result = promote::check_pr(&read_json(&a[0])?, Path::new(&a[1]), a.get(2).map_or(".github/kern", String::as_str), now);
            if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
                let line = match &result { Ok(r) => format!("### kern-verified: {}\n\n```json\n{}\n```\n", r["status"].as_str().unwrap_or(""), serde_json::to_string_pretty(r).unwrap_or_default()), Err(e) => format!("### kern-verified: DENIED\n\n{e}\n") };
                let _ = std::fs::OpenOptions::new().append(true).create(true).open(summary).and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
            }
            (Some(result?), 0)
        }
        _ => kern::deny!("Unknown command; see kern --help"),
    })
}
fn main() {
    let code = match cli() {
        Ok((value, code)) => {
            if let Some(v) = value { println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default()); }
            code
        }
        Err(e) => {
            eprintln!("{}", json!({ "status": "DENIED", "reason": e.0 }));
            1
        }
    };
    std::process::exit(code);
}
