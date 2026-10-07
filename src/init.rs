use crate::capability::is_code;
use crate::ensure;
use crate::git;
use crate::policy::{contract, operator, task_id};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

const SOURCE: [&str; 5] = ["src", "lib", "app", "source", "packages"];
const TESTS: [&str; 4] = ["test", "tests", "__tests__", "spec"];
const GITHUB_ACTIONS_APP: u64 = 15368;

pub fn profile(template: &Value, public_pem: &str) -> R<Value> {
    operator(template)?;
    crate::attest::public_key(public_pem)?;
    Ok(json!({ "version": 1, "operator": template, "public_key": public_pem }))
}
pub fn load_profile(source: &str) -> R<Value> {
    let local = ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| source.starts_with(p));
    let v: Value = if source.starts_with("https://") || local {
        let agent: ureq::Agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(20))).build().into();
        let mut r = agent.get(&format!("{}/v1/profile", source.trim_end_matches('/'))).call().map_err(|e| Denied(format!("Verifier profile unavailable: {e}")))?;
        ensure!(r.status().is_success(), "Verifier profile unavailable ({})", r.status().as_u16());
        serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?
    } else {
        ensure!(!source.starts_with("http://"), "Fetch verifier profiles over https");
        read_json(source)?
    };
    exact(&v, &["version", "operator", "public_key"], "verifier profile")?;
    ensure!(v["version"] == 1, "Unsupported verifier profile");
    profile(&v["operator"], text(&v, "public_key")?)
}
fn origin(repo: &Path) -> R<String> {
    let url = git::trimmed(git::git(repo, &["config", "--get", "remote.origin.url"], None).map_err(|_| Denied("The repository has no origin remote; add one pointing at GitHub".into()))?);
    let path = url.strip_suffix(".git").unwrap_or(&url);
    let found = path.rsplit_once("github.com/").or_else(|| path.rsplit_once("github.com:")).map(|(_, p)| p.to_string());
    found.ok_or_else(|| Denied(format!("Origin {url} is not a GitHub repository")))
}
fn pretty(v: &Value) -> R<String> { Ok(format!("{}\n", serde_json::to_string_pretty(v)?)) }

pub fn init(repo: &Path, profile: &Value, task: &str, action: &str) -> R<Value> {
    ensure!(task_id(task), "Invalid task ID: lowercase letters, digits, '-' and '_' only");
    let valid_action = action.split_once('@').is_some_and(|(r, v)| r.split('/').count() == 2 && !v.is_empty()) && action.bytes().all(|b| b.is_ascii_alphanumeric() || b"/@._-".contains(&b));
    ensure!(valid_action, "Pass the Kern action as owner/repo@commit-sha (no published default yet)");
    let repository = origin(repo)?;
    let head = git::trimmed(git::git(repo, &["rev-parse", "HEAD"], None)?);
    let files = git::read_commit(repo, &head, 0, git::MAX_TOTAL)?;
    let inspectable: u64 = files.values().map(|e| e.size).filter(|s| *s <= git::MAX_FILE).sum();
    ensure!(inspectable <= git::MAX_TOTAL, "The repository holds {inspectable} bytes of inspectable files; Kern supports up to {}", git::MAX_TOTAL);
    let top: BTreeSet<&str> = files.keys().filter_map(|k| k.split_once('/').map(|(d, _)| d)).collect();
    let dirs = |names: &[&str]| names.iter().filter(|d| top.contains(*d)).map(|d| format!("{d}/")).collect::<Vec<_>>();
    let (sources, tests) = (dirs(&SOURCE), dirs(&TESTS));
    let roots: Vec<String> = files.keys().filter(|k| !k.contains('/') && is_code(k) && !k.starts_with('.') && !k.contains(".config.") && !["gulpfile.js", "Gruntfile.js"].contains(&k.as_str())).cloned().collect();
    ensure!(!sources.is_empty() || !roots.is_empty(), "No source directory ({}) or root-level code file found; write the task contract by hand", SOURCE.join(", "));
    let mut write: Vec<String> = sources.iter().chain(&tests).chain(&roots).cloned().collect();
    if top.contains("docs") { write.push("docs/".into()); }
    let mut read = write.clone();
    if files.contains_key("README.md") { read.push("README.md".into()); }
    let deps = files.contains_key("package-lock.json");
    let checks: Vec<Value> = profile["operator"]["checks"].as_array().into_iter().flatten().map(|c| c["id"].clone()).collect();
    let c = contract(&json!({ "version": 2, "task": task, "filesystem": { "read": read, "write": write }, "network": [], "secrets": [], "publish": false,
        "dependencies": if deps { json!({ "manager": "npm", "lockfile": "package-lock.json" }) } else { Value::Null },
        "limits": { "files_changed": 25, "lines_changed": 2000, "file_bytes": git::MAX_FILE, "total_bytes": (inspectable * 2).clamp(git::MAX_FILE, git::MAX_TOTAL), "timeout_seconds": 900, "memory_mb": 2048, "pids": 512 },
        "required_checks": checks }))?;
    let mut op = profile["operator"].clone();
    op["repository"] = json!(repository);
    op["permissions"] = tests.iter().map(|t| (t.clone(), json!(["node:assert", "node:test"]))).collect::<serde_json::Map<_, _>>().into();
    operator(&op)?;
    let ruleset = json!({ "name": "kern-verified", "target": "branch", "enforcement": "active", "bypass_actors": [],
        "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
        "rules": [{ "type": "required_status_checks", "parameters": { "strict_required_status_checks_policy": false, "required_status_checks": [{ "context": "kern-verified", "integration_id": GITHUB_ACTIONS_APP }] } }] });
    let workflow = format!("name: kern-verified\non:\n  pull_request_target:\n    types: [opened, synchronize, reopened, edited]\npermissions:\n  contents: read\njobs:\n  kern-verified:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        with:\n          ref: ${{{{ github.event.pull_request.head.sha }}}}\n          fetch-depth: 2\n          persist-credentials: false\n      - uses: {action}\n");
    let kern = repo.join(".github").join("kern");
    let outputs = [(kern.join("operator.json"), pretty(&op)?), (kern.join("verifier.pub"), text(profile, "public_key")?.to_string()), (kern.join("tasks").join(format!("{task}.json")), pretty(&c.value)?),
        (kern.join("ruleset.json"), pretty(&ruleset)?), (repo.join(".github").join("workflows").join("kern-verified.yml"), workflow)];
    for (p, _) in &outputs { ensure!(!p.exists(), "{} already exists; Kern is already set up here", p.display()); }
    for (p, body) in &outputs {
        std::fs::create_dir_all(p.parent().unwrap_or(repo))?;
        create_new(p, body.as_bytes(), 0o644)?;
    }
    Ok(json!({ "status": "INITIALIZED", "repository": repository, "task": task, "contract": c.value, "files": outputs.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>(),
        "next_steps": [
            "Review .github/kern/tasks and commit .github/kern and .github/workflows/kern-verified.yml to the default branch",
            format!("Require the check from GitHub Actions: gh api -X POST repos/{repository}/rulesets --input .github/kern/ruleset.json"),
            format!("Agents open pull requests from kern/{task}/<run_id> branches with a verifier-signed attestation; other branches are reported as SKIPPED"),
        ] }))
}
