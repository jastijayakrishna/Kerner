use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const PLAN_STEPS_COUNT_MAX: usize = 40;
pub const PLAN_LINE_BYTES_MAX: usize = 2048;
pub const PLAN_ENV_COUNT_MAX: usize = 64;
pub const PLAN_BYTES_MAX: usize = 65536;
pub const NEEDS_COUNT_MAX: usize = 16;
const NODE_MAJORS: [u32; 5] = [18, 20, 22, 23, 24];
const MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];
const BROWSERS: [&str; 3] = ["chromium", "firefox", "webkit"];
const INSTALLS: [&str; 7] = ["npm ci", "npm install", "pnpm install", "pnpm i", "yarn install", "yarn", "bun install"];
const INSTALL_FLAGS: [&str; 8] = ["--frozen-lockfile", "--immutable", "--ignore-scripts", "--no-audit", "--no-fund", "--prefer-offline", "--no-save", "--with-deps"];
const STEERING_ENV: [&str; 8] = ["PATH", "NODE_OPTIONS", "NODE_PATH", "LD_PRELOAD", "LD_LIBRARY_PATH", "BASH_ENV", "ENV", "HOME"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source { Ci { file: String, job: String }, PackageJson, Ai { model: String, round: u32 } }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Install, Run, Test, Check }
impl Kind {
    pub fn name(self) -> &'static str { match self { Kind::Install => "install", Kind::Run => "run", Kind::Test => "test", Kind::Check => "check" } }
    fn parse(s: &str) -> Option<Kind> { [Kind::Install, Kind::Run, Kind::Test, Kind::Check].into_iter().find(|k| k.name() == s) }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manager { pub name: String, pub version: String, pub integrity: Option<String> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toolchain { pub node: u32, pub manager: Manager, pub browsers: Vec<String> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step { pub id: String, pub kind: Kind, pub dir: String, pub line: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Need { Service(String), Secret(String) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan { pub key: String, pub source: Source, pub toolchain: Toolchain, pub env: Vec<(String, String)>, pub steps: Vec<Step>, pub needs: Vec<Need> }
impl Plan {
    pub fn tests(&self) -> impl Iterator<Item = &Step> { self.steps.iter().filter(|s| s.kind == Kind::Test) }
    pub fn installs(&self) -> impl Iterator<Item = &Step> { self.steps.iter().filter(|s| s.kind == Kind::Install) }
}

fn exact(v: &Value, fields: &[&str], what: &str) -> R<()> {
    let o = v.as_object().ok_or_else(|| Denied(format!("{what} is not an object")))?;
    for k in o.keys() { crate::ensure!(fields.contains(&k.as_str()), "{what} has a field Kern does not know: {k}"); }
    for f in fields { crate::ensure!(o.contains_key(*f), "{what} has no {f}"); }
    Ok(())
}
fn text<'a>(v: &'a Value, field: &str, max: usize) -> R<&'a str> {
    let s = v.as_str().ok_or_else(|| Denied(format!("{field} is not text")))?;
    crate::ensure!(!s.is_empty() && s.len() <= max && !s.contains(['\n', '\r', '\0']), "{field} is empty, over {max} bytes or holds a control character");
    Ok(s)
}
fn upper_name(s: &str) -> bool { !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') && !s.as_bytes()[0].is_ascii_digit() }
fn literal(s: &str) -> bool { !s.contains("${{") && !s.contains("$(") && !s.contains('`') && !s.contains('$') }
fn plain_dir(d: &str) -> bool { d.len() <= 512 && d.split('/').all(|c| !c.is_empty() && c != "." && c != ".." && !c.starts_with(".kern")) && !d.contains('\0') }
fn slug(s: &str) -> bool { !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') }

fn source(v: &Value) -> R<Source> {
    let o = v.as_object().filter(|o| o.len() == 1).ok_or_else(|| Denied("source is not one of ci, package-json or ai".into()))?;
    match o.iter().next().map(|(k, v)| (k.as_str(), v)) {
        Some(("package-json", Value::Null | Value::Bool(true))) => Ok(Source::PackageJson),
        Some(("ci", c)) => {
            exact(c, &["file", "job"], "source.ci")?;
            let file = text(&c["file"], "source.ci.file", 512)?;
            crate::ensure!(file.starts_with(".github/workflows/") && (file.ends_with(".yml") || file.ends_with(".yaml")) && plain_dir(file), "source.ci.file is not a workflow file");
            Ok(Source::Ci { file: file.into(), job: text(&c["job"], "source.ci.job", 128)?.into() })
        }
        Some(("ai", a)) => {
            exact(a, &["model", "round"], "source.ai")?;
            let round = a["round"].as_u64().filter(|r| (1..=8).contains(r)).ok_or_else(|| Denied("source.ai.round is not 1 to 8".into()))?;
            Ok(Source::Ai { model: text(&a["model"], "source.ai.model", 128)?.into(), round: round as u32 })
        }
        _ => crate::deny!("source is not one of ci, package-json or ai"),
    }
}
fn toolchain(v: &Value) -> R<Toolchain> {
    exact(v, &["node", "manager", "browsers"], "toolchain")?;
    let node = v["node"].as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| NODE_MAJORS.contains(n)).ok_or_else(|| Denied(format!("toolchain.node is not one of {NODE_MAJORS:?}")))?;
    let m = &v["manager"];
    exact(m, &["name", "version", "integrity"], "toolchain.manager")?;
    let name = text(&m["name"], "toolchain.manager.name", 8)?;
    crate::ensure!(MANAGERS.contains(&name), "toolchain.manager.name is not one of {MANAGERS:?}");
    let version = text(&m["version"], "toolchain.manager.version", 64)?;
    crate::ensure!(crate::sandbox::plain_version(version), "toolchain.manager.version is not a plain version");
    let integrity = match &m["integrity"] { Value::Null => None, i => Some(text(i, "toolchain.manager.integrity", 256)?.to_string()) };
    let browsers = v["browsers"].as_array().ok_or_else(|| Denied("toolchain.browsers is not a list".into()))?.iter()
        .map(|b| b.as_str().filter(|b| BROWSERS.contains(b)).map(str::to_string).ok_or_else(|| Denied(format!("toolchain.browsers holds one outside {BROWSERS:?}")))).collect::<R<Vec<_>>>()?;
    Ok(Toolchain { node, manager: Manager { name: name.into(), version: version.into(), integrity }, browsers })
}
fn env(v: &Value) -> R<Vec<(String, String)>> {
    let o = v.as_object().ok_or_else(|| Denied("env is not an object".into()))?;
    crate::ensure!(o.len() <= PLAN_ENV_COUNT_MAX, "env holds more than {PLAN_ENV_COUNT_MAX} variables");
    o.iter().map(|(k, val)| {
        crate::ensure!(upper_name(k) && !STEERING_ENV.contains(&k.as_str()) && !crate::capability::secret_name(k), "env name {k} is not a plain upper-case name, or steers programs, or is a secret");
        let s = text(val, &format!("env.{k}"), PLAN_LINE_BYTES_MAX)?;
        crate::ensure!(literal(s), "env.{k} is not a literal");
        Ok((k.clone(), s.to_string()))
    }).collect()
}
fn install_line(line: &str) -> bool {
    let words: Vec<&str> = line.split(' ').collect();
    if let ["npx", "playwright", "install", rest @ ..] = words.as_slice() {
        return !rest.is_empty() && rest.iter().all(|w| BROWSERS.contains(w) || *w == "--with-deps");
    }
    INSTALLS.iter().any(|i| line == *i || line.strip_prefix(i).is_some_and(|r| r.starts_with(' ') && r.split(' ').filter(|w| !w.is_empty()).all(|w| INSTALL_FLAGS.contains(&w))))
}
fn step(v: &Value, seen: &mut BTreeSet<String>) -> R<Step> {
    exact(v, &["id", "kind", "dir", "line"], "steps[]")?;
    let id = text(&v["id"], "steps[].id", 40)?;
    crate::ensure!(slug(id) && seen.insert(id.to_string()), "steps[].id {id} is not a unique slug");
    let kind = v["kind"].as_str().and_then(Kind::parse).ok_or_else(|| Denied("steps[].kind is not install, run, test or check".into()))?;
    let dir = v["dir"].as_str().ok_or_else(|| Denied("steps[].dir is not text".into()))?;
    crate::ensure!(dir.is_empty() || plain_dir(dir), "steps[].dir {dir:?} is not a plain relative path");
    let line = text(&v["line"], "steps[].line", PLAN_LINE_BYTES_MAX)?;
    crate::ensure!(!line.contains("${{"), "steps[].line holds a workflow expression");
    crate::ensure!(kind != Kind::Install || install_line(line), "an install step must be one of {INSTALLS:?} or npx playwright install <browser>, with plain flags: {line:?}");
    Ok(Step { id: id.into(), kind, dir: dir.into(), line: line.into() })
}
fn need(v: &Value) -> R<Need> {
    let o = v.as_object().filter(|o| o.len() == 1).ok_or_else(|| Denied("needs[] is not one of service or secret".into()))?;
    match o.iter().next().map(|(k, v)| (k.as_str(), v)) {
        Some(("service", s)) => {
            let image = text(s, "needs[].service", 128)?;
            crate::ensure!(image.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".:/_-@".contains(&b)) && !image.contains("..") && !image.starts_with(['.', '/', '-']), "needs[].service is not a plain image name");
            Ok(Need::Service(image.into()))
        }
        Some(("secret", s)) => { let name = text(s, "needs[].secret", 128)?; crate::ensure!(upper_name(name), "needs[].secret is not an upper-case name"); Ok(Need::Secret(name.into())) }
        _ => crate::deny!("needs[] is not one of service or secret"),
    }
}
pub fn parse(v: &Value) -> R<Plan> {
    exact(v, &["key", "source", "toolchain", "env", "steps", "needs"], "plan")?;
    crate::ensure!(v.to_string().len() <= PLAN_BYTES_MAX, "plan is over {PLAN_BYTES_MAX} bytes (env or steps)");
    let key = text(&v["key"], "key", 64)?;
    crate::ensure!(key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit()), "key is not a sha256");
    let steps = v["steps"].as_array().ok_or_else(|| Denied("steps is not a list".into()))?;
    crate::ensure!(!steps.is_empty() && steps.len() <= PLAN_STEPS_COUNT_MAX, "steps holds none or more than {PLAN_STEPS_COUNT_MAX}");
    let mut seen = BTreeSet::new();
    let steps = steps.iter().map(|s| step(s, &mut seen)).collect::<R<Vec<_>>>()?;
    crate::ensure!(steps.iter().any(|s| s.kind == Kind::Test), "steps holds no test step");
    let needs = v["needs"].as_array().ok_or_else(|| Denied("needs is not a list".into()))?;
    crate::ensure!(needs.len() <= NEEDS_COUNT_MAX, "needs holds more than {NEEDS_COUNT_MAX}");
    Ok(Plan { key: key.into(), source: source(&v["source"])?, toolchain: toolchain(&v["toolchain"])?, env: env(&v["env"])?, steps, needs: needs.iter().map(need).collect::<R<Vec<_>>>()? })
}
pub fn json(p: &Plan) -> Value {
    let source = match &p.source {
        Source::Ci { file, job } => json!({ "ci": { "file": file, "job": job } }),
        Source::PackageJson => json!({ "package-json": true }),
        Source::Ai { model, round } => json!({ "ai": { "model": model, "round": round } }),
    };
    let env: serde_json::Map<String, Value> = p.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({ "key": p.key, "source": source,
        "toolchain": { "node": p.toolchain.node, "manager": { "name": p.toolchain.manager.name, "version": p.toolchain.manager.version, "integrity": p.toolchain.manager.integrity }, "browsers": p.toolchain.browsers },
        "env": env, "steps": p.steps.iter().map(|s| json!({ "id": s.id, "kind": s.kind.name(), "dir": s.dir, "line": s.line })).collect::<Vec<_>>(),
        "needs": p.needs.iter().map(|n| match n { Need::Service(s) => json!({ "service": s }), Need::Secret(s) => json!({ "secret": s }) }).collect::<Vec<_>>() })
}
pub fn hash(p: &Plan) -> String { sha256(json(p).to_string()) }
pub fn key(files: &[(&str, &[u8])]) -> String {
    let mut sorted: Vec<&(&str, &[u8])> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut bytes = Vec::new();
    for (path, content) in sorted {
        bytes.extend_from_slice(&(path.len() as u64).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
        bytes.extend_from_slice(content);
    }
    sha256(bytes)
}
