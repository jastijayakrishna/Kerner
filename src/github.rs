use crate::ensure;
use crate::util::*;
use serde_json::Value;
use std::time::Duration;

pub fn env(key: &str) -> Option<String> { std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) }
pub fn repository(name: &str) -> bool {
    let parts: Vec<&str> = name.split('/').collect();
    parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.len() <= 100 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
}

pub struct Api { agent: ureq::Agent, base: String, token: String }
impl Api {
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
}
/// The GitHub web server the repository lives on (GITHUB_SERVER_URL in Actions); tests may name a local file URL.
pub fn server() -> R<String> {
    let s = env("GITHUB_SERVER_URL").unwrap_or_else(|| "https://github.com".into()).trim_end_matches('/').to_string();
    ensure!(s.starts_with("https://") || s.starts_with("file://"), "GITHUB_SERVER_URL must use https");
    Ok(s)
}
/// A branch name in an API path, escaped segment by segment.
pub fn ref_path(branch: &str) -> String { branch.split('/').map(|s| s.replace('%', "%25")).collect::<Vec<_>>().join("/") }
