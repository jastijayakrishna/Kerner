use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};
use std::time::Duration;

pub fn env(key: &str) -> Option<String> { std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) }
pub fn repository(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && p.len() <= 100 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
}

fn again(r: &ureq::http::Response<ureq::Body>, repeatable: bool) -> Option<Option<Duration>> {
    let after = r.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok()).map(|s| Duration::from_secs(s.min(60)));
    let s = r.status().as_u16();
    let limited = s == 429 || (s == 403 && (after.is_some() || r.headers().get("x-ratelimit-remaining").is_some_and(|v| v == "0")));
    (limited || (repeatable && matches!(s, 500 | 502 | 503 | 504))).then_some(after)
}

pub struct Api { agent: ureq::Agent, base: String, token: String }
impl Api {
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
    fn send(&self, method: &str, path: &str, body: Option<&Value>, repeatable: bool) -> R<Value> {
        let (url, auth) = (format!("{}{path}", self.base), format!("Bearer {}", self.token));
        let json = |b: &Value| b.to_string();
        let mut tries = 0;
        loop {
            let sent = match (method, body) {
                ("POST", Some(b)) => self.agent.post(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(json(b)),
                ("PATCH", Some(b)) => self.agent.patch(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(json(b)),
                _ => self.agent.get(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").call(),
            };
            let wait = match &sent {
                Ok(r) => again(r, repeatable),
                Err(_) => repeatable.then_some(None),
            };
            if let Some(wait) = wait.filter(|_| tries < 3) {
                tries += 1;
                std::thread::sleep(wait.unwrap_or(Duration::from_secs(1 << (tries - 1))));
                continue;
            }
            let mut r = sent.map_err(|e| Denied(format!("GitHub unreachable: {e}")))?;
            ensure!(r.status().is_success(), "GitHub answered {} for {method} {}", r.status().as_u16(), path.split('?').next().unwrap_or(path));
            return Ok(serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?);
        }
    }
    pub fn get(&self, path: &str) -> R<Value> { self.send("GET", path, None, true) }
    pub fn post(&self, path: &str, body: &Value) -> R<Value> { self.send("POST", path, Some(body), true) }
    pub fn create(&self, path: &str, body: &Value) -> R<Value> { self.send("POST", path, Some(body), false) }
    pub fn patch(&self, path: &str, body: &Value) -> R<Value> { self.send("PATCH", path, Some(body), true) }
    pub fn token(&self) -> &str { &self.token }
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

pub fn server() -> R<String> {
    let s = env("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into()).trim_end_matches('/').to_string();
    ensure!(s.starts_with("https://") || s.starts_with("file://"), "GITHUB_SERVER_URL must use https");
    Ok(s)
}
