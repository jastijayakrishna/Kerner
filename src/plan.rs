use crate::util::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PLAN_STEPS_COUNT_MAX: usize = 40;
pub const PLAN_LINE_BYTES_MAX: usize = 2048;
pub const PLAN_ENV_COUNT_MAX: usize = 64;
pub const PLAN_BYTES_MAX: usize = 65536;
pub const NEEDS_COUNT_MAX: usize = 16;
const NODE_MAJORS: [u32; 5] = [18, 20, 22, 23, 24];
const MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];
const BROWSERS: [&str; 3] = ["chromium", "firefox", "webkit"];
const INSTALLS: [&str; 7] = ["npm ci", "npm install", "pnpm install", "pnpm i", "yarn install", "yarn", "bun install"];
const INSTALL_FLAGS: [&str; 16] = ["--frozen-lockfile", "--immutable", "--ignore-scripts", "--no-audit", "--no-fund", "--prefer-offline", "--no-save", "--with-deps", "--legacy-peer-deps", "--force", "--progress=false", "--no-progress", "--silent", "--prefer-frozen-lockfile", "--frozen", "--strict-peer-dependencies=false"];
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
pub struct Manager { pub name: String, pub version: Option<String>, pub integrity: Option<String> }
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
    let version = match &m["version"] { Value::Null => None, v => Some(text(v, "toolchain.manager.version", 64)?.to_string()) };
    crate::ensure!(version.as_deref().is_none_or(crate::sandbox::plain_version), "toolchain.manager.version is not a plain version");
    let integrity = match &m["integrity"] { Value::Null => None, i => Some(text(i, "toolchain.manager.integrity", 256)?.to_string()) };
    crate::ensure!(version.is_some() || integrity.is_none(), "toolchain.manager.integrity without a version");
    let browsers = v["browsers"].as_array().ok_or_else(|| Denied("toolchain.browsers is not a list".into()))?.iter()
        .map(|b| b.as_str().filter(|b| BROWSERS.contains(b)).map(str::to_string).ok_or_else(|| Denied(format!("toolchain.browsers holds one outside {BROWSERS:?}")))).collect::<R<Vec<_>>>()?;
    Ok(Toolchain { node, manager: Manager { name: name.into(), version, integrity }, browsers })
}
const MEMORY_FLAGS: [&str; 4] = ["--max-old-space-size=", "--max_old_space_size=", "--max-semi-space-size=", "--max_semi_space_size="];
const STEERING_CONFIG_WORDS: [&str; 9] = ["registry", "proxy", "cafile", "strict_ssl", "ca", "script_shell", "node_options", "prefix", "cache"];
fn memory_only(value: &str) -> bool {
    !value.trim().is_empty() && value.split(' ').filter(|w| !w.is_empty()).all(|w| MEMORY_FLAGS.iter().any(|f| w.strip_prefix(f).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))))
}
fn manager_config_name(k: &str) -> bool {
    let Some(rest) = ["npm_config_", "pnpm_config_", "yarn_config_"].iter().find_map(|p| k.strip_prefix(p)) else { return false };
    let framed = format!("_{rest}_");
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') && !STEERING_CONFIG_WORDS.iter().any(|w| framed.contains(&format!("_{w}_")))
}
fn env_name_allowed(k: &str, value: &str) -> bool {
    if k == "NODE_OPTIONS" { return memory_only(value); }
    if manager_config_name(k) { return true; }
    upper_name(k) && !STEERING_ENV.contains(&k) && !crate::capability::secret_name(k)
}
fn env(v: &Value) -> R<Vec<(String, String)>> {
    let o = v.as_object().ok_or_else(|| Denied("env is not an object".into()))?;
    crate::ensure!(o.len() <= PLAN_ENV_COUNT_MAX, "env holds more than {PLAN_ENV_COUNT_MAX} variables");
    o.iter().map(|(k, val)| {
        let s = val.as_str().ok_or_else(|| Denied(format!("env.{k} is not text")))?;
        crate::ensure!(s.len() <= PLAN_LINE_BYTES_MAX && !s.contains(['\n', '\r', '\0']), "env.{k} is over {PLAN_LINE_BYTES_MAX} bytes or holds a control character");
        crate::ensure!(env_name_allowed(k, s), "env name {k} is not a plain upper-case name, or steers programs, or is a secret");
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unreadable { pub tier: char, pub why: String }
const SETUP_FILES: [&str; 10] = ["package.json", "package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb", ".nvmrc", ".node-version", "pnpm-workspace.yaml"];
const COSMETIC_INSTALL_WORDS: [&str; 3] = ["--prefer-offline", "--progress=false", "--silent"];

pub fn setup_key(tree: &crate::ci::Tree) -> String {
    let mut files: Vec<(String, Vec<u8>)> = tree.workflows.clone();
    files.extend(SETUP_FILES.iter().filter_map(|f| (tree.read)(f).map(|b| (f.to_string(), b))));
    key(&files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect::<Vec<_>>())
}
fn node_major(view: &crate::ci::View, tree: &crate::ci::Tree, env: &[(String, String)]) -> Option<u32> {
    let setup = view.steps.iter().filter(|s| s.skipped.is_none()).find(|s| s.category.as_deref() == Some("setup-node"))?;
    let version = setup.with.get("node-version").map(crate::yaml::text_of).filter(|v| !v.is_empty());
    let text = match version {
        Some(v) if v.starts_with("${{") => {
            let inner = v.trim_start_matches("${{").trim_end_matches("}}").trim();
            if let Some(k) = inner.strip_prefix("env.") { env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())? }
            else { nearest_node(view.matrix.get(inner.strip_prefix("matrix.")?)?)? }
        }
        Some(v) => v,
        None => { let file = setup.with.get("node-version-file").map(crate::yaml::text_of).filter(|f| plain_dir(f))?; String::from_utf8((tree.read)(&file)?).ok()?.lines().find(|l| !l.trim().is_empty())?.trim().to_string() }
    };
    major_of(&text)
}
fn major_of(text: &str) -> Option<u32> { text.trim().trim_start_matches('v').split(['.', 'x']).next()?.parse().ok() }
fn nearest_node(values: &Value) -> Option<String> {
    values.as_array()?.iter().map(crate::yaml::text_of).filter(|v| major_of(v).is_some()).min_by_key(|v| (major_of(v).unwrap().abs_diff(22), major_of(v).unwrap())).or_else(|| Some(crate::yaml::text_of(values)))
}
fn manager_of(view: &crate::ci::View, tree: &crate::ci::Tree, installs: &[String]) -> Manager {
    let package: Option<Value> = (tree.read)("package.json").and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(pm) = package.as_ref().and_then(|p| p["packageManager"].as_str()) && let Some((name, rest)) = pm.split_once('@') {
        let (version, integrity) = rest.split_once('+').map_or((rest.to_string(), None), |(v, i)| (v.to_string(), Some(i.to_string())));
        return Manager { name: name.into(), version: Some(version), integrity };
    }
    let step = |cat: &str| view.steps.iter().filter(|s| s.skipped.is_none()).find(|s| s.category.as_deref() == Some(cat));
    let literal = |v: Option<&Value>| v.map(crate::yaml::text_of).filter(|t| crate::sandbox::plain_version(t));
    if let Some(s) = step("setup-pnpm") { return Manager { name: "pnpm".into(), version: literal(s.with.get("version")), integrity: None }; }
    if let Some(s) = step("setup-bun") { return Manager { name: "bun".into(), version: literal(s.with.get("bun-version")).or(Some(crate::sandbox::BUN.into())), integrity: None }; }
    let from_install = installs.iter().find_map(|l| MANAGERS.iter().find(|m| l.starts_with(*m)).map(|m| m.to_string()));
    let from_lock = [("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("bun.lock", "bun"), ("bun.lockb", "bun")].iter().find(|(f, _)| (tree.read)(f).is_some()).map(|(_, m)| m.to_string());
    Manager { name: from_install.or(from_lock).unwrap_or("npm".into()), version: None, integrity: None }
}
fn install_words(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split(' ').filter(|w| !w.is_empty()).collect();
    let words: Vec<&str> = match words.as_slice() { ["sfw", rest @ ..] => rest.to_vec(), w => w.to_vec() };
    if let [tool, pw, "install", rest @ ..] = words.as_slice() && ["npx", "bunx", "pnpx"].contains(tool) && pw.starts_with("playwright") {
        let mut out = vec!["npx", "playwright", "install"];
        out.extend(rest.iter().filter(|w| BROWSERS.contains(w) || **w == "--with-deps"));
        if out[3..].iter().all(|w| *w == "--with-deps") { out.insert(3, "chromium"); }
        return Some(out.join(" "));
    }
    let kept: Vec<&str> = words.iter().filter(|w| !COSMETIC_INSTALL_WORDS.contains(w) || INSTALL_FLAGS.contains(w)).copied().collect();
    let line = kept.join(" ");
    install_line(&line).then_some(line)
}
fn env_value(v: &Value) -> Option<String> { let t = crate::yaml::text_of(v); (!t.contains("${{") && literal(&t) && !t.contains(['\n', '\r'])).then_some(t) }
fn plan_env(view: &crate::ci::View, test_steps: &[&crate::ci::Step], needs: &mut Vec<Need>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let scopes = [&view.workflow_env, &view.job_env].into_iter().chain(test_steps.iter().map(|s| &s.env));
    for scope in scopes {
        for (k, v) in scope {
            let Some(value) = env_value(v) else { continue };
            if crate::capability::secret_name(k) { if !value.is_empty() { needs.push(Need::Secret(k.clone())); } continue; }
            env.retain(|(n, _)| n != k);
            env.push((k.clone(), value));
        }
    }
    env
}
fn resolve_expressions(text: &str, env: &[(String, String)], matrix: &Value) -> Vec<String> {
    let mut out = vec![text.to_string()];
    for e in crate::shell::expressions(text) {
        if let Some(k) = e.strip_prefix("env.") && let Some((_, v)) = env.iter().find(|(n, _)| n == k) { out = out.iter().map(|t| swap(t, &e, v)).collect(); }
        else if let Some(k) = e.strip_prefix("matrix.") && let Some(vals) = matrix.get(k).and_then(Value::as_array) {
            let vals: Vec<String> = vals.iter().map(crate::yaml::text_of).collect();
            let picks: Vec<String> = if vals.iter().all(|v| major_of(v).is_some()) { nearest_node(&matrix[k]).into_iter().collect() } else { vals };
            out = out.iter().flat_map(|t| picks.iter().map(|v| swap(t, &e, v))).take(PLAN_STEPS_COUNT_MAX).collect();
        }
    }
    out
}
fn swap(text: &str, expr: &str, value: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${{") {
        let Some(j) = rest[i..].find("}}") else { break };
        out.push_str(&rest[..i]);
        if rest[i + 3..i + j].trim() == expr { out.push_str(value); } else { out.push_str(&rest[i..i + j + 2]); }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}
fn step_json(id: &str, kind: &str, dir: &str, line: &str) -> Value { json!({ "id": id, "kind": kind, "dir": dir, "line": line }) }
fn plan_steps(view: &crate::ci::View, plan_count: usize, env: &mut Vec<(String, String)>) -> R<Vec<Value>> {
    let mut steps = Vec::new();
    for s in view.steps.iter().take(plan_count).filter(|s| s.skipped.is_none()) {
        let Some(a) = &s.analysis else { continue };
        if !a.needed && !s.is_test { continue; }
        for c in &a.commands {
            let dir = c.wd.clone().ok_or_else(|| Denied(format!("a step runs in a folder given by an expression: {}", c.text)))?;
            let kind = match c.kind.as_str() {
                "install" | "browser-install" => "install",
                "test" => "test",
                "check" => "check",
                "sets-env" => { if let Some((k, v)) = github_env(&c.text) { env.retain(|(n, _)| *n != k); env.push((k, v)); } continue }
                k if ["cd", "export", "set", "control", "noop", "info", "file", "env-file"].contains(&k) => continue,
                _ => "run",
            };
            let lines = if kind == "install" { vec![install_words(&c.text).ok_or_else(|| Denied(format!("an install line outside Kern's vocabulary: {}", c.text)))?] } else { resolve_expressions(&c.text, env, &view.matrix) };
            for line in lines {
                let n = steps.len() + 1;
                steps.push(step_json(&format!("{kind}-{n}"), kind, &dir, &line));
            }
        }
    }
    Ok(steps)
}
fn github_env(text: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("echo ")?.trim().trim_matches(['"', '\'']);
    let (k, v) = rest.split_once('=')?;
    (upper_name(k) && literal(v)).then(|| (k.to_string(), v.to_string()))
}
pub fn read(tree: &crate::ci::Tree) -> Result<Plan, Unreadable> {
    let m = crate::ci::measure(tree);
    let Some(job) = m.jobs.first().filter(|_| m.tier == 'A' || m.tier == 'B') else {
        return Err(Unreadable { tier: m.tier, why: m.blocker.map_or("no test job".into(), |r| format!("{}: {}", r.code, r.detail)) });
    };
    let unreadable = |why: String| Unreadable { tier: m.tier, why };
    let view = &job.view;
    let mut needs: Vec<Need> = view.services.iter().map(|(_, image)| Need::Service(image.clone())).collect();
    let test_steps: Vec<&crate::ci::Step> = view.steps.iter().filter(|s| s.is_test).collect();
    let mut env = plan_env(view, &test_steps, &mut needs);
    let steps = plan_steps(view, job.verdict.plan_steps, &mut env).map_err(|e| unreadable(e.0))?;
    let installs: Vec<String> = steps.iter().filter(|s| s["kind"] == "install").map(|s| s["line"].as_str().unwrap_or("").to_string()).collect();
    let manager = manager_of(view, tree, &installs);
    let browsers: Vec<&str> = installs.iter().filter(|l| l.starts_with("npx playwright install")).flat_map(|l| l.split(' ').filter(|w| BROWSERS.contains(w))).collect();
    let node = node_major(view, tree, &env).unwrap_or(22);
    let env: serde_json::Map<String, Value> = env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let needs: Vec<Value> = needs.iter().map(|n| match n { Need::Service(s) => json!({ "service": s }), Need::Secret(s) => json!({ "secret": s }) }).collect();
    let value = json!({ "key": setup_key(tree), "source": { "ci": { "file": view.workflow, "job": view.job } },
        "toolchain": { "node": node, "manager": { "name": manager.name, "version": manager.version, "integrity": manager.integrity }, "browsers": browsers },
        "env": env, "steps": steps, "needs": needs });
    parse(&value).map_err(|e| unreadable(e.0))
}

pub const AI_ROUNDS_COUNT_MAX: u32 = 2;
pub const AI_CONTEXT_BYTES_MAX: usize = crate::investigate::CONTEXT_BYTES_MAX;
pub const TRIAL_ERROR_BYTES_MAX: usize = 8192;
const CONTEXT_FILE_BYTES_MAX: usize = 12288;
const README_SECTION_LINES_MAX: usize = 80;
const CONTEXT_FILES: [&str; 26] = ["package.json", "pnpm-workspace.yaml", ".npmrc", ".nvmrc", ".node-version", ".tool-versions", "turbo.json", "nx.json", "lerna.json", "Makefile", "justfile", "Dockerfile", "docker-compose.yml",
    "docker-compose.yaml", "compose.yml", ".gitlab-ci.yml", ".circleci/config.yml", ".travis.yml", "azure-pipelines.yml", "Jenkinsfile", ".buildkite/pipeline.yml", "bitbucket-pipelines.yml", "vitest.config.ts", "vitest.workspace.ts", "jest.config.js", "jest.config.ts"];
const LOCKFILES: [&str; 6] = ["package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb"];
const SETUP_SYSTEM: &str = "You are the setup engineer of Kern, a tool that checks pull requests by running a project's own tests on the old and the new code. You read a repository's files at one commit of its main branch and write the one recipe that installs it and runs its unit tests on Linux with the network off during the tests. Kern will run your recipe twice on that same commit and keep it only if every non-test step exits 0 both times, the test runner reports results, at least one test passes, and both runs give the same per-test outcomes. You never see a pull request. Use only the closed vocabulary described in the task; anything outside it is refused by the schema checker and the recipe is lost.";
const SETUP_TASK: &str = "Write this repository's setup recipe as JSON.\n\nVocabulary: `toolchain.node` is one of 18, 20, 22, 23, 24. `toolchain.manager.name` is npm, pnpm, yarn or bun; `version` is a plain version or null (the image's). `toolchain.browsers` lists chromium, firefox or webkit. `env` holds upper-case names with literal values only (never a secret, never PATH or NODE_OPTIONS beyond memory flags). Each step has `id` (a short slug), `kind` (install, run, test or check), `dir` (a plain relative folder or \"\") and `line` (one command, no newline, no `${{ }}`). An install step is one of: npm ci, npm install, pnpm install, pnpm i, yarn install, yarn, bun install, with plain flags such as --frozen-lockfile, or `npx playwright install <browser> [--with-deps]`; install steps are the only ones with the network on. A test step runs vitest, jest or mocha, directly or through a package script. `needs` lists services (an image name) or secrets (an upper-case name) the tests cannot do without; Kern cannot provide them, so name them only when the tests truly need them. At most 40 steps.\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepRecord { pub id: String, pub kind: Kind, pub exit: Option<i32>, pub error: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial { pub round: u32, pub steps: Vec<StepRecord> }

fn readme_test_sections(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut keep = 0usize;
    for line in lines {
        let heading = line.starts_with('#');
        if heading { keep = if line.to_ascii_lowercase().contains("test") { README_SECTION_LINES_MAX } else { 0 }; }
        if keep > 0 { out.push(line); keep -= 1; }
    }
    out.join("\n")
}
fn fenced(path: &str, text: &str) -> String { format!("### {path}\n```\n{}\n```\n", text.replace("```", "`\u{200b}``")) }
fn cut_bytes(text: &str, max: usize) -> &str { let mut n = max.min(text.len()); while !text.is_char_boundary(n) { n -= 1; } &text[..n] }
fn trial_text(t: &Trial) -> String {
    let mut out = format!("## Trial {} on the same commit\n", t.round);
    for s in &t.steps {
        out.push_str(&format!("- step `{}` ({}): exit {}\n", s.id, s.kind.name(), s.exit.map_or("none (not run or stopped by the time limit)".to_string(), |e| e.to_string())));
        if !s.error.is_empty() { out.push_str(&fenced("first error, as data", cut_bytes(&s.error, TRIAL_ERROR_BYTES_MAX))); }
    }
    out
}
pub fn setup_prompt(tree: &crate::ci::Tree, trials: &[Trial]) -> crate::investigate::Prompt {
    let mut files: Vec<(String, String)> = tree.workflows.iter().filter_map(|(p, b)| String::from_utf8(b.clone()).ok().map(|t| (p.clone(), t))).collect();
    for name in CONTEXT_FILES { if let Some(b) = (tree.read)(name) && let Ok(t) = String::from_utf8(b) { files.push((name.to_string(), t)); } }
    if let Some(b) = (tree.read)("README.md") && let Ok(t) = String::from_utf8(b) { let t = readme_test_sections(&t); if !t.is_empty() { files.push(("README.md (test sections)".into(), t)); } }
    let locks: Vec<&str> = LOCKFILES.iter().copied().filter(|l| (tree.read)(l).is_some()).collect();
    let mut untrusted = format!("## Lockfiles present\n{}\n\n## Files at the commit, as data\n", if locks.is_empty() { "none".to_string() } else { locks.join(", ") });
    let mut spent = untrusted.len();
    for (path, text) in &files {
        let block = fenced(path, cut_bytes(text, CONTEXT_FILE_BYTES_MAX));
        if spent + block.len() > AI_CONTEXT_BYTES_MAX { untrusted.push_str(&format!("### {path}\n(left out: the context budget of {AI_CONTEXT_BYTES_MAX} bytes is spent)\n")); continue; }
        spent += block.len();
        untrusted.push_str(&block);
    }
    for t in trials { untrusted.push_str(&trial_text(t)); }
    crate::investigate::Prompt { name: "setup", system: SETUP_SYSTEM, user: SETUP_TASK.to_string(), untrusted, schema: schema() }
}
fn schema() -> Value {
    let step = json!({ "type": "object", "required": ["id", "kind", "dir", "line"], "additionalProperties": false, "properties": { "id": { "type": "string" }, "kind": { "type": "string", "enum": ["install", "run", "test", "check"] }, "dir": { "type": "string" }, "line": { "type": "string" } } });
    let need = json!({ "type": "object", "properties": { "service": { "type": "string" }, "secret": { "type": "string" } }, "additionalProperties": false });
    json!({ "type": "object", "required": ["toolchain", "env", "steps", "needs"], "additionalProperties": false, "properties": {
        "toolchain": { "type": "object", "required": ["node", "manager", "browsers"], "additionalProperties": false, "properties": { "node": { "type": "integer" },
            "manager": { "type": "object", "required": ["name", "version", "integrity"], "additionalProperties": false, "properties": { "name": { "type": "string" }, "version": { "type": ["string", "null"] }, "integrity": { "type": ["string", "null"] } } },
            "browsers": { "type": "array", "items": { "type": "string" } } } },
        "env": { "type": "object", "additionalProperties": { "type": "string" } },
        "steps": { "type": "array", "items": step },
        "needs": { "type": "array", "items": need } } })
}
pub fn proposal_name(model: &str, setup_key: &str, round: u32) -> String { format!("propose-{}.json", sha256(format!("{}\n{model}\n{setup_key}\n{round}", env!("CARGO_PKG_VERSION")))) }
pub fn propose(tree: &crate::ci::Tree, trials: &[Trial], model: &str, out: &Path, ask: crate::investigate::Ask) -> R<Plan> {
    let round = u32::try_from(trials.len()).unwrap_or(u32::MAX).saturating_add(1);
    crate::ensure!(round <= AI_ROUNDS_COUNT_MAX, "the setup engineer has had its {AI_ROUNDS_COUNT_MAX} rounds for this setup key");
    let key = setup_key(tree);
    let cache = out.join(proposal_name(model, &key, round));
    let answer = match read_json(&cache) {
        Ok(cached) => cached,
        Err(_) => {
            let prompt = setup_prompt(tree, trials);
            let response = ask(&prompt)?;
            let answer = crate::investigate::answer_of(&response, &prompt.schema)?;
            write_json(&cache, &answer)?;
            answer
        }
    };
    let mut value = answer.as_object().cloned().ok_or_else(|| Denied("the setup engineer's answer is not an object".into()))?;
    value.insert("key".into(), json!(key));
    value.insert("source".into(), json!({ "ai": { "model": model, "round": round } }));
    parse(&Value::Object(value))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome { Passed, Failed, Skipped }
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Run { pub steps: Vec<StepRecord>, pub reports: usize, pub outcomes: BTreeMap<String, Outcome> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejected { StepFailed { run: usize, step: String, exit: Option<i32>, error: String }, NoReport { run: usize }, NoTestPassed { run: usize }, Unsteady { test: String, first: Option<Outcome>, second: Option<Outcome> } }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedPlan { plan: Plan, tests_count: usize }
impl AcceptedPlan {
    pub fn plan(&self) -> &Plan { &self.plan }
    pub fn tests_count(&self) -> usize { self.tests_count }
}
pub fn accept(plan: Plan, runs: [&Run; 2], flaky: &BTreeSet<String>) -> Result<AcceptedPlan, Rejected> {
    for (i, run) in runs.iter().enumerate() {
        if let Some(s) = run.steps.iter().find(|s| s.kind != Kind::Test && s.exit != Some(0)) {
            return Err(Rejected::StepFailed { run: i + 1, step: s.id.clone(), exit: s.exit, error: cut_bytes(&s.error, TRIAL_ERROR_BYTES_MAX).to_string() });
        }
        if run.reports == 0 { return Err(Rejected::NoReport { run: i + 1 }); }
        if !run.outcomes.values().any(|o| *o == Outcome::Passed) { return Err(Rejected::NoTestPassed { run: i + 1 }); }
    }
    let tests: BTreeSet<&String> = runs[0].outcomes.keys().chain(runs[1].outcomes.keys()).collect();
    for t in tests {
        if flaky.contains(t) { continue; }
        let (a, b) = (runs[0].outcomes.get(t).copied(), runs[1].outcomes.get(t).copied());
        if a != b { return Err(Rejected::Unsteady { test: t.clone(), first: a, second: b }); }
    }
    Ok(AcceptedPlan { tests_count: runs[0].outcomes.len(), plan })
}
pub fn trial_of(round: u32, run: &Run) -> Trial { Trial { round, steps: run.steps.clone() } }
