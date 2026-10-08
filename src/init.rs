use crate::capability::{is_code, is_data};
use crate::ensure;
use crate::git;
use crate::policy::{Contract, contract, operator, task_id};
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

/// Build output and vendored code: not what a task changes.
const GENERATED: [&str; 6] = ["node_modules", "dist", "build", "out", "coverage", "vendor"];
/// Instructions coding agents read; an agent that rewrites them steers the next agent, so they are granted only by hand.
const AGENT_NOTES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md"];
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
pub(crate) fn origin(repo: &Path) -> R<String> {
    let url = git::trimmed(git::git(repo, &["config", "--get", "remote.origin.url"], None).map_err(|_| Denied("The repository has no origin remote; add one pointing at GitHub".into()))?);
    let path = url.strip_suffix(".git").unwrap_or(&url);
    let found = path.rsplit_once("github.com/").or_else(|| path.rsplit_once("github.com:")).map(|(_, p)| p.to_string());
    found.ok_or_else(|| Denied(format!("Origin {url} is not a GitHub repository")))
}
fn pretty(v: &Value) -> R<String> { Ok(format!("{}\n", serde_json::to_string_pretty(v)?)) }
fn runner(package: Option<&[u8]>) -> Option<(&'static str, Vec<String>)> {
    let pkg: Value = serde_json::from_slice(package?).ok()?;
    let has = |name: &str| ["devDependencies", "dependencies"].iter().any(|k| pkg[*k].get(name).is_some());
    let bin = |name: &str| format!("/workspace/node_modules/.bin/{name}");
    if has("vitest") { return Some(("vitest", vec![bin("vitest"), "run".into()])); }
    if let Some(name) = ["jest", "mocha"].into_iter().find(|n| has(n)) { return Some((name, vec![bin(name)])); }
    pkg["scripts"]["test"].as_str().filter(|t| t.contains("node --test")).map(|_| ("node", vec!["/usr/local/bin/node".into(), "--test".into()]))
}

/// The task contract `kern init` proposes for a commit with these files, and the operator permissions its tests need.
pub fn draft(files: &git::Files, package: Option<&[u8]>, task: &str, checks: Vec<Value>) -> R<(Contract, Value)> {
    let inspectable: u64 = files.values().map(|e| e.size).filter(|s| *s <= git::MAX_FILE).sum();
    ensure!(inspectable <= git::MAX_TOTAL, "The repository holds {inspectable} bytes of inspectable files; Kern supports up to {}", git::MAX_TOTAL);
    let top: BTreeSet<&str> = files.keys().filter_map(|k| k.split_once('/').map(|(d, _)| d)).collect();
    let tests: Vec<String> = TESTS.iter().filter(|d| top.contains(*d)).map(|d| format!("{d}/")).collect();
    // Every top-level folder with code, docs or styles in it, except tool settings (dot folders), generated output and vendored code. The
    // dependency, CI and secret files stay protected wherever they are (git::protected), so wider grants never reach them.
    let dirs = files.keys().filter_map(|k| k.split_once('/').filter(|(d, _)| !d.starts_with('.') && !GENERATED.contains(d) && (is_code(k) || is_data(k))).map(|(d, _)| format!("{d}/")));
    // Root files: code, docs, pages and styles, but not build and test configuration, and not the instructions agents read.
    let roots = files.keys().filter(|k| !k.contains('/') && !k.starts_with('.') && !git::protected(k) && !AGENT_NOTES.contains(&k.as_str())
        && !k.contains(".config.") && !k.starts_with("tsconfig") && !k.starts_with("jsconfig") && !["gulpfile.js", "Gruntfile.js"].contains(&k.as_str())
        && (is_code(k) || crate::capability::text_name(k) || [".md", ".html", ".css"].iter().any(|e| k.ends_with(e)))).cloned();
    let write: Vec<String> = dirs.chain(roots).chain(tests.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    ensure!(!write.is_empty(), "No folder or root file with code, docs or styles found; write the task contract by hand");
    let read = write.clone();
    let deps = files.contains_key("package-lock.json");
    let found = runner(package).filter(|(_, c)| deps || !c[0].starts_with("/workspace/node_modules/"));
    let proof = match (&found, tests.is_empty()) { (Some((name, cmd)), false) => json!({ "paths": tests, "kind": "feature", "runner": name, "command": cmd }), _ => Value::Null };
    let c = contract(&json!({ "version": 4, "task": task, "behaviors": [], "tests": proof, "filesystem": { "read": read, "write": write }, "network": [], "secrets": [], "publish": false,
        "dependencies": if deps { json!({ "manager": "npm", "lockfile": "package-lock.json" }) } else { Value::Null },
        "limits": { "files_changed": 40, "lines_changed": 4000, "file_bytes": git::MAX_FILE, "total_bytes": (inspectable * 2).clamp(git::MAX_FILE, git::MAX_TOTAL), "timeout_seconds": 900, "memory_mb": 2048, "pids": 512 },
        "required_checks": checks }))?;
    Ok((c, tests.iter().map(|t| (t.clone(), json!(["node:assert", "node:test"]))).collect::<serde_json::Map<_, _>>().into()))
}

pub fn init(repo: &Path, profile: &Value, task: &str, action: &str) -> R<Value> {
    ensure!(task_id(task), "Invalid task ID: lowercase letters, digits, '-' and '_' only");
    let valid_action = action.split_once('@').is_some_and(|(r, v)| r.split('/').count() == 2 && !v.is_empty()) && action.bytes().all(|b| b.is_ascii_alphanumeric() || b"/@._-".contains(&b));
    ensure!(valid_action, "Pass the Kern action as owner/repo@commit-sha (no published default yet)");
    let repository = origin(repo)?;
    let head = git::trimmed(git::git(repo, &["rev-parse", "HEAD"], None)?);
    let files = git::read_commit(repo, &head, 0, git::MAX_TOTAL)?;
    let checks: Vec<Value> = profile["operator"]["checks"].as_array().into_iter().flatten().map(|c| c["id"].clone()).collect();
    let package = files.contains_key("package.json").then(|| git::git(repo, &["cat-file", "blob", &format!("{head}:package.json")], None).ok()).flatten();
    let (c, permissions) = draft(&files, package.as_deref(), task, checks)?;
    let mut op = profile["operator"].clone();
    op["repository"] = json!(repository);
    op["permissions"] = permissions;
    operator(&op)?;
    let ruleset = json!({ "name": "kern-verified", "target": "branch", "enforcement": "active", "bypass_actors": [],
        "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
        "rules": [{ "type": "required_status_checks", "parameters": { "strict_required_status_checks_policy": true, "required_status_checks": [{ "context": "kern-verified", "integration_id": GITHUB_ACTIONS_APP }] } }] });
    let workflow = format!("name: kern-verified\non:\n  pull_request_target:\n    types: [opened, synchronize, reopened, edited]\npermissions:\n  contents: read\n  pull-requests: write\njobs:\n  kern-verified:\n    runs-on: ubuntu-24.04\n    steps:\n      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n        if: startsWith(github.event.pull_request.head.ref, 'kern/') && github.event.pull_request.head.repo.full_name == github.repository\n        with:\n          ref: ${{{{ github.event.pull_request.head.sha }}}}\n          fetch-depth: 2\n          persist-credentials: false\n      - uses: {action}\n        with:\n          comment: \"true\"\n          agent-mention: \"\"\n          max-attempts: \"3\"\n");
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
            if c.tests.is_some() { "The task contract locks existing tests and requires every test that ran before to still run and pass; for a bug-fix task set tests.kind to bugfix, so every fix must come with a test that fails before it".to_string() }
            else { "Kern found no test runner it can read (mocha, jest, vitest or node --test); add a \"tests\" section to the task contract to require proof of honest fixes".to_string() },
            "The ruleset requires pull requests to be up to date with their base, so the merged result is the commit Kern verified".to_string(),
            "Kern comments its verdict on agent pull requests; set agent-mention in the workflow (for example @claude) so the agent is asked to fix what it can".to_string(),
            format!("Each quarter, run kern audit-merges on a full clone of {repository} to re-verify every merged change and list any gaps"),
        ] }))
}
