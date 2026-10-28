use kern::policy::{self, operator};
use kern::util::*;
use kern::{ensure, sandbox};
use serde_json::{Value, json};
use std::path::Path;

const HELP: &str = "kern 0.3 - one receipt on every pull request

Kern's GitHub workflow, .github/workflows/kern.yml, runs these:
  check <repo> <base> <head> <out-dir> <operator.json> [description.md]
                                  verifier side (--control verifier.json): the rules, the project's tests on the old and the
                                  new code, the type check, the build, planted faults and the inputs the AI step proposed;
                                  writes receipt.json and receipt.md (KERN_INVESTIGATION names the investigation to wait for)
  investigate <repo> <base> <head> <pr.md> <out-dir>
                                  in Kern's investigate job, which holds ANTHROPIC_API_KEY (KERN_MODEL or claude-opus-5-5): the
                                  model states what the changed functions must keep doing from sources older than the change, and
                                  proposes inputs; Kern checks every answer and writes investigation.json. It reads the code only
                                  through git, runs none of it, and reuses oracle.answer.json and hunter.answer.json in out-dir
  receipt render <out-dir> <head-sha>
                                  in Kern's signing job: rebuild the answer from receipt.json's facts and write receipt.md
  receipt sign <out-dir> <owner/repo> <pr-number>
                                  in Kern's workflow: GitHub signs the receipt's seal (OIDC); writes seal.json
  receipt post <out-dir> <owner/repo> <pr-number>
                                  post the receipt or edit Kern's earlier one, and set the Kern status (GITHUB_TOKEN)
  receipt approve <owner/repo> <pr-number> <login>
                                  /kern approve: someone who can merge approves a signed \"needs a look\" receipt
  receipt verify <owner/repo> <pr-number> [receipt.json]
                                  check a receipt's signature with GitHub's keys, and receipt.json against its seal
  learn <repo> <commit> <out-dir> <operator.json>
                                  on main only, never in a pull request check: read the commit's CI job into one typed
                                  plan, run it twice on that commit and keep it only when both runs agree test by test;
                                  writes plan-<setup key>.json, which a check reads from the folder KERN_PLANS names
  drive clean <out-dir>           remove the check's sandbox volumes
  replay <dir>...                 judge every stored check under the folders again with today's rules: one JSON line per
                                  receipt with the answer before and after, then a summary (KERN_REPLAY_WORKERS: 1 to 4)
Verifier side (--control verifier.json):
  doctor                          check that this machine can run the sandbox
  pin-check <id> <file>           the hash of a pinned file, for operator.json";

fn count(args: &[String], n: usize) -> R<()> {
    ensure!(args.len() == n, "Expected {n} arguments; see kern --help");
    Ok(())
}
fn cli() -> R<(Option<Value>, i32)> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("__drive_setup") => return sandbox::drive_setup().map(|_| (None, 0)),
        Some("__drive_test") => return sandbox::drive_test().map(|_| (None, 0)),
        Some("__drive_seal") => return sandbox::drive_seal().map(|_| (None, 0)),
        Some("--control") => {
            ensure!(args.len() >= 3, "Expected control JSON and a command");
            policy::configure(&read_json(&args[1])?)?;
            args.drain(0..2);
        }
        _ => {}
    }
    let (cmd, a) = args.split_first().map_or(("help", &[][..]), |(c, r)| (c.as_str(), r));
    Ok(match cmd {
        "help" | "--help" | "-h" => { say(HELP); (None, 0) }
        "--version" => (Some(json!({ "kern": env!("CARGO_PKG_VERSION"), "implementation_sha256": policy::implementation_digest()? })), 0),
        "doctor" => {
            count(a, 0)?;
            policy::control()?;
            let d = sandbox::doctor();
            (Some(d.report), if d.ready { 0 } else { 2 })
        }
        "pin-check" => { count(a, 2)?; (Some(json!({ "id": a[0], "file": std::path::absolute(&a[1])?, "sha256": sha256(std::fs::read(&a[1])?) })), 0) }
        "replay" => {
            ensure!(!a.is_empty(), "Expected one or more folders of stored checks; see kern --help");
            let workers = std::env::var("KERN_REPLAY_WORKERS").ok().and_then(|v| v.parse().ok()).unwrap_or(kern::replay::WORKERS_COUNT_MAX);
            let roots: Vec<std::path::PathBuf> = a.iter().map(std::path::PathBuf::from).collect();
            (Some(kern::replay::run(&roots, workers, &mut std::io::stdout().lock())?), 0)
        }
        "check" => {
            ensure!(a.len() == 5 || a.len() == 6, "Expected a repository, the base and the head commit, an output directory, the operator and an optional description file");
            let pr = a.get(5).map(std::fs::read_to_string).transpose()?;
            (Some(kern::check::check(Path::new(&a[0]), &a[1], &a[2], Path::new(&a[3]), &operator(&read_json(&a[4])?)?, pr.as_deref())?), 0)
        }
        "investigate" => {
            count(a, 5)?;
            let pr = kern::investigate::read_pr(Path::new(&a[3]))?;
            let (key, model) = (kern::github::env("ANTHROPIC_API_KEY"), kern::investigate::model());
            let call = |p: &kern::investigate::Prompt| kern::investigate::call(key.as_deref().unwrap_or_default(), &kern::investigate::request(&model, p));
            let ask: Option<kern::investigate::Ask> = if key.is_some() { Some(&call) } else { None };
            (Some(kern::investigate::investigate(Path::new(&a[0]), (&a[1], &a[2]), &pr, Path::new(&a[4]), ask)?), 0)
        }
        "receipt" => {
            let (sub, rest) = a.split_first().ok_or_else(|| Denied("Expected receipt render, sign, post, approve or verify; see kern --help".into()))?;
            let number = |s: &String| s.parse::<u64>().map_err(|_| Denied("Expected a pull request number".into()));
            match sub.as_str() {
                "sign" => {
                    count(rest, 3)?;
                    (Some(kern::receipt::sign(Path::new(&rest[0]), &rest[1], number(&rest[2])?)?), 0)
                }
                "render" => {
                    count(rest, 2)?;
                    (Some(kern::receipt::render(Path::new(&rest[0]), &rest[1])?), 0)
                }
                "post" => {
                    count(rest, 3)?;
                    (Some(kern::receipt::post(&kern::github::Api::from_env("post Kern's receipt on the pull request")?, Path::new(&rest[0]), &rest[1], number(&rest[2])?)?), 0)
                }
                "approve" => {
                    count(rest, 3)?;
                    let api = kern::github::Api::from_env("approve Kern's receipt")?;
                    let jwks = kern::oidc::jwks()?;
                    let check = |seal: &Value, token: &str| kern::receipt::signed(seal, token, &jwks, &rest[0]).map(|_| ());
                    (Some(kern::receipt::approve(&api, &rest[0], number(&rest[1])?, &rest[2], &check)?), 0)
                }
                "verify" => {
                    ensure!(rest.len() == 2 || rest.len() == 3, "Expected the repository, the pull request number and an optional receipt.json");
                    let api = kern::github::Api::from_env("read Kern's receipt on the pull request")?;
                    let receipt = rest.get(2).map(read_json).transpose()?;
                    let v = kern::receipt::verify(&api, &rest[0], number(&rest[1])?, &kern::oidc::jwks()?, receipt.as_ref())?;
                    let code = if v["status"] == "VERIFIED" { 0 } else { 1 };
                    (Some(v), code)
                }
                other => kern::deny!("Unknown receipt command {other}; see kern --help"),
            }
        }
        "learn" => {
            count(a, 4)?;
            let until = now_ms() + kern::check::minutes() * 60_000;
            (Some(kern::exec::learn(Path::new(&a[0]), &a[1], Path::new(&a[2]), &operator(&read_json(&a[3])?)?, until)?), 0)
        }
        "drive" => {
            ensure!(a.len() == 2 && a[0] == "clean", "Expected drive clean <out-dir>; see kern --help");
            (Some(kern::drive::clean(Path::new(&a[1]))?), 0)
        }
        _ => kern::deny!("Unknown command; see kern --help"),
    })
}
fn main() {
    let code = match cli() {
        Ok((value, code)) => {
            if let Some(v) = value { say(&serde_json::to_string_pretty(&v).unwrap_or_default()); }
            code
        }
        Err(e) => {
            eprintln!("{}", json!({ "status": "DENIED", "reason": e.0 }));
            1
        }
    };
    std::process::exit(code);
}
