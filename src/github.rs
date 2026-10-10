use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};
use std::time::Duration;

pub fn env(key: &str) -> Option<String> { std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) }
pub fn repository(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.len() <= 100 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
}

pub struct Api { agent: ureq::Agent, base: String, token: String }
impl Api {
    /// A client for an explicit API root and token (GitHub Enterprise, tests); `from_env` reads both from the environment.
    pub fn new(base: &str, token: &str) -> R<Api> {
        let base = base.trim_end_matches('/').to_string();
        ensure!(base.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| base.starts_with(p)), "The GitHub API URL must use https");
        let agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into();
        Ok(Api { agent, base, token: token.to_string() })
    }
    pub fn from_env(purpose: &str) -> R<Api> {
        let base = env("GITHUB_API_URL").unwrap_or_else(|| "https://api.github.com".into()).trim_end_matches('/').to_string();
        ensure!(base.starts_with("https://") || ["http://127.0.0.1:", "http://localhost:"].iter().any(|p| base.starts_with(p)), "GITHUB_API_URL must use https");
        let token = env("GITHUB_TOKEN").or_else(|| env("GH_TOKEN")).ok_or_else(|| Denied(format!("Set GITHUB_TOKEN to {purpose}")))?;
        let agent = ureq::Agent::config_builder().max_redirects(0).http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into();
        Ok(Api { agent, base, token })
    }
    fn send(&self, method: &str, path: &str, body: Option<&Value>) -> R<Value> {
        let (url, auth) = (format!("{}{path}", self.base), format!("Bearer {}", self.token));
        let json = |b: &Value| b.to_string();
        let sent = match (method, body) {
            ("POST", Some(b)) => self.agent.post(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(json(b)),
            ("PATCH", Some(b)) => self.agent.patch(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(json(b)),
            _ => self.agent.get(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").call(),
        };
        let mut r = sent.map_err(|e| Denied(format!("GitHub unreachable: {e}")))?;
        ensure!(r.status().is_success(), "GitHub answered {} for {method} {}", r.status().as_u16(), path.split('?').next().unwrap_or(path));
        Ok(serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?)
    }
    pub fn get(&self, path: &str) -> R<Value> { self.send("GET", path, None) }
    pub fn post(&self, path: &str, body: &Value) -> R<Value> { self.send("POST", path, Some(body)) }
    pub fn patch(&self, path: &str, body: &Value) -> R<Value> { self.send("PATCH", path, Some(body)) }
    /// The token, for git commands that talk to the same GitHub (see git::remote).
    pub fn token(&self) -> &str { &self.token }
    /// A GraphQL query: api.github.com/graphql, or /api/graphql on GitHub Enterprise Server (whose REST root ends in /api/v3).
    pub fn graphql(&self, query: &str, variables: &Value) -> R<Value> {
        let url = match self.base.strip_suffix("/api/v3") { Some(host) => format!("{host}/api/graphql"), None => format!("{}/graphql", self.base) };
        let mut r = self.agent.post(&url).header("Authorization", &format!("Bearer {}", self.token)).header("Accept", "application/vnd.github+json").header("Content-Type", "application/json")
            .send(json!({ "query": query, "variables": variables }).to_string()).map_err(|e| Denied(format!("GitHub unreachable: {e}")))?;
        ensure!(r.status().is_success(), "GitHub answered {} for its GraphQL API", r.status().as_u16());
        let v: Value = serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?;
        ensure!(v["data"].is_object(), "GitHub's GraphQL API answered without data");
        Ok(v)
    }
}

const BLAME: &str = "query($owner: String!, $name: String!, $oid: GitObjectID!, $path: String!) { repository(owner: $owner, name: $name) { object(oid: $oid) { ... on Commit { blame(path: $path) { ranges { startingLine endingLine commit { oid messageHeadline associatedPullRequests(first: 1) { nodes { number title url closingIssuesReferences(first: 3) { nodes { number title } } } } } } } } } } }";
fn clip(s: &str, n: usize) -> String { s.chars().take(n).collect() }
/// Who added the code a change removes: for each removed range, the commits that last touched those lines at `base`, with the pull request
/// that brought each one and the issues it closed. Informational only: when GitHub cannot be asked, there is simply nothing to show.
pub fn who_added(repository: &str, base: &str, removals: &[Value]) -> Value {
    let (Ok(api), Some((owner, name))) = (Api::from_env("read who added the code a change removes"), repository.split_once('/')) else { return Value::Null };
    let mut blamed: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    let mut out = Vec::new();
    for r in removals.iter().take(10) {
        let Some(path) = r["path"].as_str() else { continue };
        let (start, count, file) = (r["start"].as_u64().unwrap_or(1), r["count"].as_u64().unwrap_or(1).max(1), r["file"] == true);
        let ranges = blamed.entry(path.to_string()).or_insert_with(|| api.graphql(BLAME, &json!({ "owner": owner, "name": name, "oid": base, "path": path })).map(|v| v["data"]["repository"]["object"]["blame"]["ranges"].clone()).unwrap_or(Value::Null));
        let (lo, hi) = if file { (1, u64::MAX) } else { (start, start + count - 1) };
        let mut added_by: Vec<Value> = Vec::new();
        for range in ranges.as_array().into_iter().flatten() {
            let (a, b) = (range["startingLine"].as_u64().unwrap_or(0), range["endingLine"].as_u64().unwrap_or(0));
            let c = &range["commit"];
            if a > hi || b < lo || added_by.iter().any(|x| x["commit"] == c["oid"]) { continue; }
            let pr = &c["associatedPullRequests"]["nodes"][0];
            let closes: Vec<Value> = pr["closingIssuesReferences"]["nodes"].as_array().into_iter().flatten().map(|i| json!({ "number": i["number"], "title": clip(i["title"].as_str().unwrap_or(""), 200) })).collect();
            added_by.push(json!({ "commit": c["oid"], "summary": clip(c["messageHeadline"].as_str().unwrap_or(""), 200),
                "pull_request": if pr.is_object() { json!({ "number": pr["number"], "title": clip(pr["title"].as_str().unwrap_or(""), 200), "url": pr["url"] }) } else { Value::Null }, "closes": closes }));
            if added_by.len() >= 3 { break; }
        }
        let lines = if file { "file".to_string() } else if count == 1 { start.to_string() } else { format!("{start}-{}", start + count - 1) };
        out.push(json!({ "path": path, "lines": lines, "added_by": added_by }));
    }
    json!(out)
}
/// The same as Markdown bullets; titles and paths sit inside code formatting, numbers are plain.
pub fn who_markdown(list: &[Value]) -> String {
    let code = |s: &str| format!("`{}`", s.replace('`', "'").replace(['\r', '\n'], " "));
    list.iter().map(|r| {
        let at = if r["lines"] == "file" { format!("The removed file {}", code(r["path"].as_str().unwrap_or(""))) } else { format!("Removed code at {} line {}", code(r["path"].as_str().unwrap_or("")), r["lines"].as_str().unwrap_or("")) };
        let by: Vec<String> = r["added_by"].as_array().into_iter().flatten().map(|a| {
            let pr = &a["pull_request"];
            let from = match pr["number"].as_u64() { Some(n) => format!("#{n} {}", code(pr["title"].as_str().unwrap_or(""))), None => format!("commit {}", code(&a["commit"].as_str().unwrap_or("").chars().take(12).collect::<String>())) };
            let closes: Vec<String> = a["closes"].as_array().into_iter().flatten().filter_map(|i| i["number"].as_u64().map(|n| format!("#{n} {}", code(i["title"].as_str().unwrap_or(""))))).collect();
            if closes.is_empty() { from } else { format!("{from}, which closed {}", closes.join(", ")) }
        }).collect();
        if by.is_empty() { format!("- {at}: GitHub did not say who added it\n") } else { format!("- {at} was added in {}\n", by.join("; ")) }
    }).collect()
}
/// The GitHub web server the repository lives on (GITHUB_SERVER_URL in Actions); tests may name a local file URL.
pub fn server() -> R<String> {
    let s = env("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into()).trim_end_matches('/').to_string();
    ensure!(s.starts_with("https://") || s.starts_with("file://"), "GITHUB_SERVER_URL must use https");
    Ok(s)
}
/// A branch name in an API path, escaped segment by segment.
pub fn ref_path(branch: &str) -> String { branch.split('/').map(|s| s.replace('%', "%25")).collect::<Vec<_>>().join("/") }
