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
    fn send(&self, path: &str, body: Option<&Value>) -> R<Value> {
        let (url, auth) = (format!("{}{path}", self.base), format!("Bearer {}", self.token));
        let sent = match body {
            Some(b) => self.agent.post(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").header("Content-Type", "application/json").send(b.to_string()),
            None => self.agent.get(&url).header("Authorization", &auth).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28").call(),
        };
        let mut r = sent.map_err(|e| Denied(format!("GitHub unreachable: {e}")))?;
        ensure!(r.status().is_success(), "GitHub answered {} for {}", r.status().as_u16(), path.split('?').next().unwrap_or(path));
        Ok(serde_json::from_str(&r.body_mut().read_to_string().map_err(|e| Denied(e.to_string()))?)?)
    }
    pub fn get(&self, path: &str) -> R<Value> { self.send(path, None) }
    pub fn post(&self, path: &str, body: &Value) -> R<Value> { self.send(path, Some(body)) }
}
