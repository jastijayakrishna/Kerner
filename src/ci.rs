use crate::shell::{self, Cmd, Packages, Scripts};
use crate::yaml::text_of;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

pub const WORKFLOW_FILES_COUNT_MAX: usize = 200;
pub const ACTIONS_COUNT_MAX: usize = 64;
pub const ACTION_DEPTH_MAX: usize = 4;
pub const REUSABLE_DEPTH_MAX: usize = 2;
pub const PACKAGES_COUNT_MAX: usize = 150;
pub const JOBS_COUNT_MAX: usize = 300;
pub const STEPS_COUNT_MAX: usize = 400;
pub const CONDITION_TOKENS_COUNT_MAX: usize = 400;
pub const MATRIX_VARIANTS_COUNT_MAX: usize = 12;
pub const REASONS_COUNT_MAX: usize = 60;
pub const TIERS: [char; 7] = ['A', 'B', 'S', 'C', 'D', 'E', 'F'];
const ACTIONS: &[(&str, &str)] = &[
    ("actions/checkout", "checkout"), ("actions/setup-node", "setup-node"), ("useblacksmith/setup-node", "setup-node"), ("buildjet/setup-node", "setup-node"), ("volta-cli/action", "setup-node"),
    ("pnpm/action-setup", "setup-pnpm"), ("oven-sh/setup-bun", "setup-bun"),
    ("actions/cache", "cache"), ("actions/cache/restore", "cache"), ("actions/cache/save", "cache"), ("useblacksmith/cache", "cache"), ("buildjet/cache", "cache"), ("runs-on/cache", "cache"),
    ("namespacelabs/nscloud-cache-action", "cache"), ("rharkor/caching-for-turbo", "cache"), ("dtinth/setup-github-actions-caching-for-turbo", "cache"), ("swatinem/rust-cache", "cache"), ("runs-on/action", "noop"),
    ("actions/upload-artifact", "upload-artifact"), ("actions/download-artifact", "download-artifact"),
    ("codecov/codecov-action", "report"), ("codecov/test-results-action", "report"), ("coverallsapp/github-action", "report"), ("dorny/test-reporter", "report"), ("enricomi/publish-unit-test-result-action", "report"),
    ("mikepenz/action-junit-report", "report"), ("davelosert/vitest-coverage-report-action", "report"), ("actions/upload-pages-artifact", "report"), ("marocchino/sticky-pull-request-comment", "report"), ("thollander/actions-comment-pull-request", "report"),
    ("step-security/harden-runner", "noop"), ("actions/labeler", "noop"), ("actions/stale", "noop"), ("mxschmitt/action-tmate", "noop"),
    ("actions/create-github-app-token", "token"), ("tibdex/github-app-token", "token"), ("nrwl/nx-set-shas", "nx-shas"), ("tj-actions/changed-files", "changes"), ("dorny/paths-filter", "changes"),
    ("nick-fields/retry", "wrapper:command"), ("wandalen/wretry.action", "wrapper:command"), ("gabrielbb/xvfb-action", "wrapper:run"), ("coactions/setup-xvfb", "wrapper:run"),
    ("actions/setup-python", "toolchain"), ("actions/setup-go", "toolchain"), ("actions/setup-java", "toolchain"), ("actions/setup-dotnet", "toolchain"), ("dtolnay/rust-toolchain", "toolchain"), ("actions-rs/toolchain", "toolchain"),
    ("astral-sh/setup-uv", "toolchain"), ("denoland/setup-deno", "toolchain"), ("ruby/setup-ruby", "toolchain"), ("shivammathur/setup-php", "toolchain"), ("mlugg/setup-zig", "toolchain"), ("goto-bus-stop/setup-zig", "toolchain"), ("jdx/mise-action", "toolchain"),
    ("docker/setup-buildx-action", "docker"), ("docker/setup-qemu-action", "docker"), ("docker/login-action", "docker"), ("docker/build-push-action", "docker"), ("docker/metadata-action", "docker"),
    ("useblacksmith/setup-docker-builder", "docker"), ("useblacksmith/build-push-action", "docker"), ("depot/setup-action", "docker"), ("depot/build-push-action", "docker"),
    ("supercharge/redis-github-action", "service"), ("supercharge/mongodb-github-action", "service"), ("harmon758/postgresql-action", "service"), ("ankane/setup-postgres", "service"), ("ankane/setup-mysql", "service"),
    ("ikalnytskyi/action-setup-postgres", "service"), ("isbang/compose-action", "service"), ("hoverkraft-tech/compose-action", "service"), ("supabase/setup-cli", "service"), ("shogo82148/actions-setup-redis", "service"),
    ("shogo82148/actions-setup-mysql", "service"), ("getong/elasticsearch-action", "service"),
    ("browser-actions/setup-chrome", "browser"), ("browser-actions/setup-firefox", "browser"), ("nanasess/setup-chromedriver", "browser"),
    ("aws-actions/configure-aws-credentials", "cloud-auth"), ("google-github-actions/auth", "cloud-auth"), ("azure/login", "cloud-auth"), ("actions/github-script", "script"),
    ("cypress-io/github-action", "test-action"), ("artiomtr/jest-coverage-report-action", "test-action"), ("chromaui/action", "test-action"), ("mattallty/jest-github-action", "test-action"), ("reactivecircus/android-emulator-runner", "test-action"),
    ("useblacksmith/stickydisk", "cache"), ("relative-ci/agent-action", "report"), ("actions/attest-build-provenance", "report"), ("redhat-actions/podman-login", "docker"), ("socketdev/action", "security"),
    ("voidzero-dev/setup-vp", "setup-other-pm"), ("jsdaniell/create-json", "file-action"), ("android-actions/setup-android", "toolchain"), ("qwenlm/qwen-code-action", "ai-agent"), ("anthropics/claude-code-action", "ai-agent"),
];
const QUIET: &[&str] = &["checkout", "setup-node", "setup-pnpm", "setup-bun", "cache", "upload-artifact", "report", "noop", "token", "nx-shas", "changes", "security"];
const OPTIONAL_SECRETS: &[&str] = &["GITHUB_TOKEN", "GH_TOKEN", "TURBO_TOKEN", "TURBO_TEAM", "TURBO_REMOTE_CACHE_SIGNATURE_KEY", "NX_CLOUD_ACCESS_TOKEN", "NX_CLOUD_AUTH_TOKEN", "NX_CLOUD_ENCRYPTION_KEY", "CODECOV_TOKEN", "COVERALLS_REPO_TOKEN",
    "CURRENTS_RECORD_KEY", "CYPRESS_RECORD_KEY", "DD_API_KEY", "DATADOG_API_KEY", "BUILDKITE_ANALYTICS_TOKEN", "ARGOS_TOKEN", "PERCY_TOKEN", "CHROMATIC_PROJECT_TOKEN", "SENTRY_AUTH_TOKEN", "DEPOT_TOKEN", "CODSPEED_TOKEN"];
const RUNNER_VARS: &[&str] = &["GITHUB_WORKSPACE", "RUNNER_TEMP", "RUNNER_OS", "RUNNER_ARCH", "RUNNER_TOOL_CACHE", "HOME", "PWD", "CI", "GITHUB_SHA", "GITHUB_REF", "GITHUB_REF_NAME", "GITHUB_HEAD_REF", "GITHUB_BASE_REF", "GITHUB_EVENT_NAME", "GITHUB_EVENT_PATH",
    "GITHUB_REPOSITORY", "GITHUB_RUN_ID", "GITHUB_RUN_NUMBER", "GITHUB_ACTOR", "GITHUB_JOB", "GITHUB_ACTION_PATH", "GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_PATH", "GITHUB_STEP_SUMMARY", "GITHUB_SERVER_URL", "USER", "PATH"];
const SKIPPABLE: &[&str] = &["check", "info", "noop", "set", "control", "cd", "export"];
const RELEVANT: &[&str] = &["install", "tool-install", "build", "test", "env-file", "sets-env", "services", "system", "browser-install", "other-language", "git-network", "git-clone", "shell-script"];
const LEADING_CONTROL: &[&str] = &["then", "do", "else", "elif", "if", "while", "until", "!", "{", "time"];
const CLOSING_CONTROL: &[&str] = &["fi", "done", "esac", "}", ";;"];
const GITHUB_FILES: &[&str] = &["GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_PATH", "GITHUB_STEP_SUMMARY"];
const S_KINDS: &[(&str, &str)] = &[("system", "installs system packages"), ("git-network", "needs git submodules or LFS files"), ("git-clone", "clones another repository"), ("download", "downloads with curl, wget or gh"),
    ("other-language", "needs another language toolchain"), ("shell-script", "runs a shell script or Makefile Kern would have to read"), ("source", "sources a shell file")];

pub struct Tree<'a> { pub repo: String, pub workflows: Vec<(String, Vec<u8>)>, pub read: &'a dyn Fn(&str) -> Option<Vec<u8>> }

type Action = Result<(Value, String), String>;
pub struct Ctx<'a> { tree: &'a Tree<'a>, pub errors: RefCell<Vec<String>>, pub workflows: BTreeMap<String, Value>, actions: RefCell<BTreeMap<String, Action>>, packages: RefCell<BTreeMap<String, Option<Value>>> }

impl Packages for Ctx<'_> {
    fn scripts(&self, folder: &str) -> Option<Value> { self.package(folder)?.get("scripts").filter(|s| s.is_object()).cloned() }
}
impl<'a> Ctx<'a> {
    pub fn new(tree: &'a Tree<'a>) -> Ctx<'a> {
        let mut ctx = Ctx { tree, errors: RefCell::new(Vec::new()), workflows: BTreeMap::new(), actions: RefCell::new(BTreeMap::new()), packages: RefCell::new(BTreeMap::new()) };
        for (path, bytes) in tree.workflows.iter().take(WORKFLOW_FILES_COUNT_MAX) {
            match crate::yaml::read(bytes) {
                Ok(doc) if doc.is_object() => { ctx.workflows.insert(path.clone(), doc); }
                Ok(_) => ctx.errors.get_mut().push(format!("{path}: not a YAML mapping")),
                Err(e) => ctx.errors.get_mut().push(format!("{path}: does not parse: {}", e.0)),
            }
        }
        ctx
    }
    fn note(&self, text: String) { self.errors.borrow_mut().push(text); }
    pub fn package(&self, folder: &str) -> Option<Value> {
        let key = if folder.is_empty() { "." } else { folder };
        if let Some(v) = self.packages.borrow().get(key) { return v.clone(); }
        if self.packages.borrow().len() >= PACKAGES_COUNT_MAX { self.note(format!("package.json cap of {PACKAGES_COUNT_MAX} reached; {key} not read")); return None; }
        let path = if folder.is_empty() { "package.json".to_string() } else { format!("{folder}/package.json") };
        let value = (self.tree.read)(&path).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(|_| self.note(format!("{path} is not valid JSON"))).ok()).filter(Value::is_object);
        self.packages.borrow_mut().insert(key.to_string(), value.clone());
        value
    }
    fn action(&self, path: &str) -> Action {
        if let Some(a) = self.actions.borrow().get(path) { return a.clone(); }
        let result = if self.actions.borrow().len() >= ACTIONS_COUNT_MAX { Err("local action cap reached".to_string()) } else { self.read_action(path) };
        self.actions.borrow_mut().insert(path.to_string(), result.clone());
        result
    }
    fn read_action(&self, path: &str) -> Action {
        for name in ["action.yml", "action.yaml"] {
            let file = format!("{path}/{name}");
            let Some(bytes) = (self.tree.read)(&file) else { continue };
            return match crate::yaml::read(&bytes) {
                Ok(doc) if doc.is_object() => Ok((doc, file)),
                Ok(_) => Err(format!("{file} is not a mapping")),
                Err(e) => Err(format!("{file} does not parse: {}", e.0)),
            };
        }
        Err(format!("no action.yml or action.yaml at {path}"))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Analysis { pub commands: Vec<Cmd>, pub b_features: BTreeSet<String>, pub s_features: BTreeSet<String>, pub needed: bool, pub plumbing: Option<String>, pub kern_replaces: bool, pub files: Vec<(String, String)>, pub sets_env: Vec<String>, pub is_test: bool }
#[derive(Clone, Debug, Default)]
pub struct Step { pub origin: String, pub name: Option<String>, pub if_: Option<String>, pub env: Map<String, Value>, pub wd: Option<String>, pub shell: Option<String>, pub continue_on_error: Option<Value>, pub timeout_minutes: Option<Value>, pub with: Map<String, Value>,
    pub inputs: BTreeMap<String, String>, pub uses: Option<String>, pub category: Option<String>, pub problem: Option<String>, pub action: Option<String>, pub run: Option<String>, pub skipped: Option<String>, pub if_value: Option<bool>, pub is_test: bool, pub maybe_test: bool, pub wrapped: Option<String>, pub analysis: Option<Analysis> }
#[derive(Clone, Debug, Default)]
pub struct View { pub workflow: String, pub job: String, pub name: Option<String>, pub triggers: Vec<String>, pub via: Option<String>, pub runs_on: Value, pub container: Value, pub services: Vec<(String, String)>, pub matrix: Value, pub matrix_dynamic: bool, pub needs: Vec<String>, pub if_: Option<String>,
    pub steps: Vec<Step>, pub workflow_env: Map<String, Value>, pub job_env: Map<String, Value>, pub inputs: BTreeMap<String, String>, pub default_wd: Option<String>, pub default_shell: Option<String>, pub caller_needs: Vec<String>, pub caller_workflow: Option<String>, pub caller_if: Option<String>,
    pub unreadable_call: bool, pub reusable_uses: Option<String>, pub is_test: bool, pub pr_gate: bool, pub if_value: Option<bool>, pub caller_if_value: Option<bool> }

pub fn rank(t: char) -> usize { TIERS.iter().position(|x| *x == t).unwrap_or(0) }
fn str_list(v: &Value) -> Vec<String> {
    match v { Value::String(s) => vec![s.clone()], Value::Array(a) => a.iter().map(text_of).collect(), _ => Vec::new() }
}
fn opt_text(v: &Value) -> Option<String> { let t: String = text_of(v).chars().take(shell::VALUE_CHARS_MAX).collect(); (!t.is_empty()).then_some(t) }
fn object(v: &Value) -> Map<String, Value> { v.as_object().cloned().unwrap_or_default() }
pub fn safe_rel(path: &str) -> Option<String> {
    let p = path.trim();
    let p = p.strip_prefix("./").unwrap_or(p).trim_end_matches('/');
    let parts: Vec<&str> = p.split('/').filter(|x| !x.is_empty() && *x != ".").collect();
    if parts.is_empty() || parts.contains(&"..") || p.len() > 200 || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/@+-".contains(&b)) { return None; }
    Some(parts.join("/"))
}
fn substitute_inputs(text: &str, inputs: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${{") {
        let Some(j) = rest[i..].find("}}") else { break };
        let inner = rest[i + 3..i + j].trim();
        out.push_str(&rest[..i]);
        match inner.strip_prefix("inputs.").filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')).and_then(|n| inputs.get(n)) {
            Some(v) => out.push_str(v),
            None => out.push_str(&rest[i..i + j + 2]),
        }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}
pub fn triggers(doc: &Value) -> Vec<String> {
    match doc.get("on") { Some(Value::Object(o)) => o.keys().cloned().collect(), Some(v) => str_list(v), None => Vec::new() }
}
fn category_of(uses: &str) -> (Option<&'static str>, String) {
    let name = uses.trim().split('@').next().unwrap_or("");
    let parts: Vec<&str> = name.split('/').collect();
    let owner_repo = parts.iter().take(2).copied().collect::<Vec<_>>().join("/").to_ascii_lowercase();
    let full = name.to_ascii_lowercase();
    let cat = ACTIONS.iter().find(|(n, _)| *n == full).or_else(|| ACTIONS.iter().find(|(n, _)| *n == owner_repo)).map(|(_, c)| *c);
    (cat, full)
}

fn expand_steps(ctx: &Ctx, steps: &[Value], origin: &str, depth: usize, inputs: &BTreeMap<String, String>, out: &mut Vec<Step>) {
    for raw in steps.iter().take(STEPS_COUNT_MAX) {
        if out.len() >= STEPS_COUNT_MAX { ctx.note(format!("step cap of {STEPS_COUNT_MAX} reached in {origin}")); break; }
        let Some(o) = raw.as_object() else { out.push(Step { origin: origin.into(), ..Step::default() }); continue };
        let get = |k: &str| o.get(k).cloned().unwrap_or(Value::Null);
        let mut step = Step { origin: origin.into(), name: opt_text(&get("name")), if_: opt_text(&get("if")), env: object(&get("env")), wd: get("working-directory").as_str().map(str::to_string), shell: get("shell").as_str().map(str::to_string),
            continue_on_error: o.get("continue-on-error").cloned(), timeout_minutes: o.get("timeout-minutes").cloned(), with: object(&get("with")), ..Step::default() };
        if !inputs.is_empty() && origin != "job" {
            step.inputs = inputs.clone();
            step.with = step.with.iter().map(|(k, v)| (k.clone(), match v { Value::String(s) => Value::String(substitute_inputs(s, inputs)), v => v.clone() })).collect();
        }
        if let Some(uses) = get("uses").as_str() { expand_uses(ctx, uses.trim(), depth, &mut step, out); if step.category.as_deref() == Some("local-composite") { continue; } }
        if let Some(run) = o.get("run").filter(|r| r.is_string() || r.is_number()) {
            let text = text_of(run);
            step.run = Some(if inputs.is_empty() { text } else { substitute_inputs(&text, inputs) });
        }
        out.push(step);
    }
}
fn expand_uses(ctx: &Ctx, uses: &str, depth: usize, step: &mut Step, out: &mut Vec<Step>) {
    step.uses = Some(uses.to_string());
    if let Some(local) = uses.strip_prefix("./") {
        step.category = Some("local".into());
        let Some(path) = safe_rel(local) else { step.category = Some("local-unreadable".into()); step.problem = Some("local action path is not a plain repository path".into()); return };
        if depth >= ACTION_DEPTH_MAX { step.category = Some("local-unreadable".into()); step.problem = Some("local action nesting cap reached".into()); return; }
        let (doc, _file) = match ctx.action(&path) { Ok(a) => a, Err(e) => { step.category = Some("local-unreadable".into()); step.problem = Some(e); return } };
        let runs = object(&doc.get("runs").cloned().unwrap_or(Value::Null));
        let using = text_of(runs.get("using").unwrap_or(&Value::Null));
        if using == "composite" {
            step.category = Some("local-composite".into());
            let mut merged: BTreeMap<String, String> = object(&doc.get("inputs").cloned().unwrap_or(Value::Null)).iter().filter_map(|(k, v)| v.get("default").map(|d| (k.clone(), text_of(d)))).collect();
            for (k, v) in &step.with { merged.insert(k.clone(), text_of(v)); }
            out.push(step.clone());
            let steps = runs.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
            expand_steps(ctx, &steps, &format!("action:{path}"), depth + 1, &merged, out);
            return;
        }
        let kind = if using == "docker" { "docker" } else { "javascript" };
        step.category = Some(format!("local-{kind}"));
        step.problem = Some(format!("local {} action ({using}): Kern cannot follow its code", if using == "docker" { "Docker" } else { "JavaScript" }));
        return;
    }
    if uses.starts_with("docker://") { step.category = Some("docker-image-action".into()); return; }
    let (cat, full) = category_of(uses);
    step.action = Some(full);
    step.category = Some(cat.unwrap_or("unknown").into());
}

fn job_view(ctx: &Ctx, wf: &str, doc: &Value, (jid, job): (&str, &Map<String, Value>), (trig, inputs, via): (&[String], &BTreeMap<String, String>, Option<&str>)) -> View {
    let get = |k: &str| job.get(k).cloned().unwrap_or(Value::Null);
    let mut view = View { workflow: wf.into(), job: jid.into(), name: opt_text(&get("name")), triggers: trig.to_vec(), via: via.map(str::to_string), runs_on: get("runs-on"), container: get("container"), if_: opt_text(&get("if")), needs: str_list(&get("needs")), inputs: inputs.clone(), ..View::default() };
    match get("services") {
        Value::Object(o) => view.services = o.iter().take(20).map(|(n, s)| (n.clone(), s.get("image").map_or_else(|| text_of(s), text_of))).collect(),
        Value::Null => {}
        s if text_of(&s).is_empty() || s == Value::Bool(false) => {}
        s => view.services.push(("(expression)".into(), text_of(&s))),
    }
    let matrix = get("strategy").get("matrix").cloned().unwrap_or(Value::Null);
    view.matrix_dynamic = matrix.is_string() || matrix.as_object().is_some_and(|m| m.values().any(|v| v.as_str().is_some_and(|s| s.contains("${{"))));
    view.matrix = matrix;
    view.workflow_env = object(&doc.get("env").cloned().unwrap_or(Value::Null));
    view.job_env = object(&get("env"));
    let run_default = |v: &Value| v.get("defaults").and_then(|d| d.get("run")).cloned().unwrap_or(Value::Null);
    let (wd, jd) = (run_default(doc), run_default(&Value::Object(job.clone())));
    view.default_wd = jd.get("working-directory").and_then(Value::as_str).or_else(|| wd.get("working-directory").and_then(Value::as_str)).map(str::to_string);
    view.default_shell = jd.get("shell").and_then(Value::as_str).or_else(|| wd.get("shell").and_then(Value::as_str)).map(str::to_string);
    if let Some(steps) = get("steps").as_array() { expand_steps(ctx, steps, "job", 0, inputs, &mut view.steps); }
    view
}

fn call_target(ctx: &Ctx, uses: &str) -> Option<String> {
    if let Some(local) = uses.strip_prefix("./") { return safe_rel(local.split('@').next().unwrap_or("")); }
    let prefix = format!("{}/", ctx.tree.repo);
    if uses.len() > prefix.len() && uses[..prefix.len()].eq_ignore_ascii_case(&prefix) && uses[prefix.len()..].starts_with(".github/workflows/") && let Some((path, r)) = uses[prefix.len()..].split_once('@') && !r.is_empty() { return safe_rel(path); }
    None
}
pub fn workflow_jobs(ctx: &Ctx, wf: &str, doc: &Value, inputs: &BTreeMap<String, String>, via: Option<&str>, depth: usize, called: &mut BTreeSet<String>) -> Vec<View> {
    let trig = triggers(doc);
    let mut views = Vec::new();
    let Some(jobs) = doc.get("jobs").and_then(Value::as_object) else { ctx.note(format!("{wf} has no jobs mapping")); return views };
    for (jid, job) in jobs.iter().take(JOBS_COUNT_MAX) {
        let Some(job) = job.as_object() else { ctx.note(format!("{wf} job {jid} is not a mapping")); continue };
        if let Some(uses) = job.get("uses").and_then(Value::as_str) {
            let u = uses.trim();
            let target = call_target(ctx, u);
            if let Some(target) = target.filter(|t| ctx.workflows.contains_key(t) && depth < REUSABLE_DEPTH_MAX) {
                called.insert(target.clone());
                let sub_doc = ctx.workflows[&target].clone();
                let mut given: BTreeMap<String, String> = sub_doc.get("on").and_then(|on| on.get("workflow_call")).and_then(|w| w.get("inputs")).and_then(Value::as_object).map(|i| i.iter().filter_map(|(k, v)| v.get("default").map(|d| (k.clone(), text_of(d)))).collect()).unwrap_or_default();
                for (k, v) in object(job.get("with").unwrap_or(&Value::Null)) { given.insert(k, text_of(&v)); }
                let mut sub = workflow_jobs(ctx, &target, &sub_doc, &given, Some(&format!("{wf}:{jid}")), depth + 1, called);
                for v in &mut sub {
                    v.triggers = trig.clone();
                    v.caller_workflow = Some(wf.into());
                    v.caller_needs = str_list(job.get("needs").unwrap_or(&Value::Null));
                    v.caller_if = opt_text(job.get("if").unwrap_or(&Value::Null));
                }
                views.extend(sub);
            } else {
                views.push(View { workflow: wf.into(), job: jid.clone(), triggers: trig.clone(), unreadable_call: true, reusable_uses: Some(u.into()), ..View::default() });
            }
            continue;
        }
        views.push(job_view(ctx, wf, doc, (jid, job), (&trig, inputs, via)));
    }
    views
}
pub fn only_called(doc: &Value) -> bool { triggers(doc) == ["workflow_call"] }

#[derive(Clone, Debug, PartialEq)]
enum Cv { Unknown, Null, Bool(bool), Num(f64), Str(String) }
fn truth(v: &Cv) -> Option<bool> {
    match v { Cv::Unknown => None, Cv::Null | Cv::Bool(false) => Some(false), Cv::Str(s) => Some(!s.is_empty()), Cv::Num(n) => Some(*n != 0.0), Cv::Bool(true) => Some(true) }
}
fn tokens(t: &str) -> Vec<String> {
    let s: Vec<char> = t.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let two: String = s[i..(i + 2).min(s.len())].iter().collect();
        if c == '\'' {
            let mut j = i + 1;
            let mut tok = String::from("'");
            loop {
                if j >= s.len() { tok.clear(); break; }
                if s[j] == '\'' { if s.get(j + 1) == Some(&'\'') { tok.push_str("''"); j += 2; continue; } tok.push('\''); j += 1; break; }
                tok.push(s[j]); j += 1;
            }
            if tok.is_empty() { i += 1; continue; }
            out.push(tok); i = j; continue;
        }
        if ["&&", "||", "==", "!=", "<=", ">="].contains(&two.as_str()) { out.push(two); i += 2; continue; }
        if "()!<>,".contains(c) { out.push(c.to_string()); i += 1; continue; }
        if c.is_ascii_alphabetic() || c == '_' {
            let j = (i..s.len()).find(|&k| !(s[k].is_ascii_alphanumeric() || "_.-*[]".contains(s[k]))).unwrap_or(s.len());
            out.push(s[i..j].iter().collect()); i = j; continue;
        }
        if c.is_ascii_digit() || (c == '-' && s.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let mut j = i + 1;
            while j < s.len() && s[j].is_ascii_digit() { j += 1; }
            if s.get(j) == Some(&'.') && s.get(j + 1).is_some_and(char::is_ascii_digit) { j += 1; while j < s.len() && s[j].is_ascii_digit() { j += 1; } }
            out.push(s[i..j].iter().collect()); i = j; continue;
        }
        i += 1;
    }
    out
}
struct Cond<'a> { t: &'a [String], v: &'a BTreeMap<String, String> }
impl Cond<'_> {
    fn tok(&self, k: usize) -> Result<&str, ()> { self.t.get(k).map(String::as_str).ok_or(()) }
    fn or(&self, k: usize) -> Result<(Cv, usize), ()> {
        let (left, mut k) = self.and(k)?;
        let mut parts = vec![left];
        while self.tok(k) == Ok("||") { let (r, k2) = self.and(k + 1)?; parts.push(r); k = k2; }
        if parts.len() == 1 { return Ok((parts.remove(0), k)); }
        let ts: Vec<Option<bool>> = parts.iter().map(truth).collect();
        Ok((if ts.contains(&Some(true)) { Cv::Bool(true) } else if ts.iter().all(|x| *x == Some(false)) { Cv::Bool(false) } else { Cv::Unknown }, k))
    }
    fn and(&self, k: usize) -> Result<(Cv, usize), ()> {
        let (left, mut k) = self.not(k)?;
        let mut parts = vec![left];
        while self.tok(k) == Ok("&&") { let (r, k2) = self.not(k + 1)?; parts.push(r); k = k2; }
        if parts.len() == 1 { return Ok((parts.remove(0), k)); }
        let ts: Vec<Option<bool>> = parts.iter().map(truth).collect();
        Ok((if ts.contains(&Some(false)) { Cv::Bool(false) } else if ts.iter().all(|x| *x == Some(true)) { Cv::Bool(true) } else { Cv::Unknown }, k))
    }
    fn not(&self, k: usize) -> Result<(Cv, usize), ()> {
        if self.tok(k)? == "!" { let (v, k) = self.not(k + 1)?; return Ok((truth(&v).map_or(Cv::Unknown, |b| Cv::Bool(!b)), k)); }
        self.cmp(k)
    }
    fn cmp(&self, k: usize) -> Result<(Cv, usize), ()> {
        let (left, k) = self.primary(k)?;
        let Ok(op) = self.tok(k) else { return Ok((left, k)) };
        if !["==", "!=", "<", ">", "<=", ">="].contains(&op) { return Ok((left, k)); }
        let (right, k) = self.primary(k + 1)?;
        if left == Cv::Unknown || right == Cv::Unknown { return Ok((Cv::Unknown, k)); }
        let low = |v: Cv| match v { Cv::Str(s) => Cv::Str(s.to_lowercase()), v => v };
        let (a, b) = (low(left), low(right));
        let value = match op {
            "==" => Cv::Bool(a == b),
            "!=" => Cv::Bool(a != b),
            _ => match (&a, &b) {
                (Cv::Num(x), Cv::Num(y)) => Cv::Bool(match op { "<" => x < y, ">" => x > y, "<=" => x <= y, _ => x >= y }),
                (Cv::Str(x), Cv::Str(y)) => Cv::Bool(match op { "<" => x < y, ">" => x > y, "<=" => x <= y, _ => x >= y }),
                _ => Cv::Unknown,
            },
        };
        Ok((value, k))
    }
    fn primary(&self, k: usize) -> Result<(Cv, usize), ()> {
        let tok = self.tok(k)?;
        if tok == "(" { let (v, k) = self.or(k + 1)?; if self.tok(k)? != ")" { return Err(()); } return Ok((v, k + 1)); }
        if let Some(inner) = tok.strip_prefix('\'') { return Ok((Cv::Str(inner.strip_suffix('\'').unwrap_or(inner).replace("''", "'")), k + 1)); }
        if let Ok(n) = tok.parse::<f64>() && tok.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-') { return Ok((Cv::Num(n), k + 1)); }
        if tok == "true" || tok == "false" { return Ok((Cv::Bool(tok == "true"), k + 1)); }
        if tok == "null" { return Ok((Cv::Null, k + 1)); }
        if self.tok(k + 1) == Ok("(") {
            let mut args = Vec::new();
            let mut j = k + 2;
            while self.tok(j)? != ")" { let (v, j2) = self.or(j)?; args.push(v); j = j2; if self.tok(j)? == "," { j += 1; } }
            let name = tok.to_ascii_lowercase();
            let value = match (name.as_str(), args.as_slice()) {
                ("always" | "success", _) => Cv::Bool(true),
                ("failure" | "cancelled", _) => Cv::Bool(false),
                ("contains" | "startswith" | "endswith", [Cv::Str(a), Cv::Str(b)]) => { let (a, b) = (a.to_lowercase(), b.to_lowercase()); Cv::Bool(match name.as_str() { "contains" => a.contains(&b), "startswith" => a.starts_with(&b), _ => a.ends_with(&b) }) }
                _ => Cv::Unknown,
            };
            return Ok((value, j + 1));
        }
        Ok((self.v.get(tok).map_or(Cv::Unknown, |s| Cv::Str(s.clone())), k + 1))
    }
}
pub fn condition(text: Option<&str>, values: &BTreeMap<String, String>) -> Option<bool> {
    let Some(text) = text.filter(|t| !t.is_empty()) else { return Some(true) };
    let t = text.trim();
    let t = if t.starts_with("${{") && t.ends_with("}}") && t.matches("${{").count() == 1 { t[3..t.len() - 2].trim() } else if t.contains("${{") { return None } else { t };
    let toks = tokens(t);
    if toks.is_empty() || toks.len() > CONDITION_TOKENS_COUNT_MAX { return None; }
    let (value, k) = Cond { t: &toks, v: values }.or(0).ok()?;
    if k != toks.len() { return None; }
    truth(&value)
}
fn condition_values(ctx: &Ctx, view: &View) -> BTreeMap<String, String> {
    let owner = ctx.tree.repo.split('/').next().unwrap_or("").to_string();
    let mut values: BTreeMap<String, String> = [("github.event_name", "pull_request".to_string()), ("runner.os", "Linux".into()), ("runner.environment", "github-hosted".into()), ("runner.arch", "X64".into()),
        ("github.repository", ctx.tree.repo.clone()), ("github.repository_owner", owner), ("github.server_url", "https://github.com".into())].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    for scope in [&view.workflow_env, &view.job_env] {
        for (k, v) in scope { if (v.is_string() || v.is_number() || v.is_boolean()) && !text_of(v).contains("${{") { values.insert(format!("env.{k}"), text_of(v)); } }
    }
    for (k, v) in &view.inputs { if !v.contains("${{") { values.insert(format!("inputs.{k}"), v.clone()); } }
    if let Some(m) = view.matrix.as_object() && !m.contains_key("include") {
        for (k, v) in m { if let Some(a) = v.as_array() && a.len() == 1 && (a[0].is_string() || a[0].is_number()) { values.insert(format!("matrix.{k}"), text_of(&a[0])); } }
    }
    values
}

fn step_folder(wd: Option<&str>) -> Option<String> {
    match wd { None | Some("") => Some(String::new()), Some(w) if w.contains("${{") => None, Some(w) => shell::join_dir(Some(""), Some(w)) }
}
fn name_has(name: Option<&str>, words: &[&str]) -> bool { name.is_some_and(|n| { let l = n.to_ascii_lowercase(); words.iter().any(|w| l.contains(w)) }) }
pub fn analyze_view(ctx: &Ctx, view: &mut View) {
    let values = condition_values(ctx, view);
    view.if_value = condition(view.if_.as_deref(), &values);
    let matrix = view.matrix.clone();
    let (default_wd, workflow_env, job_env) = (view.default_wd.clone(), view.workflow_env.clone(), view.job_env.clone());
    let names_of = |step: &Step| -> BTreeSet<String> { workflow_env.keys().chain(job_env.keys()).chain(step.env.keys()).cloned().collect() };
    for step in &mut view.steps {
        let mut step_values = values.clone();
        for (k, v) in &step.inputs { if !v.contains("${{") { step_values.insert(format!("inputs.{k}"), v.clone()); } }
        step.if_value = condition(step.if_.as_deref(), &step_values);
        if step.if_value == Some(false) { step.skipped = Some("its if: is false for a pull request on a GitHub-hosted Linux runner".into()); step.is_test = false; continue; }
        if let Some(run) = step.run.clone() {
            let wd = step.wd.clone().or_else(|| default_wd.clone()).unwrap_or_default();
            let mut a = analyze_run(ctx, &run, step_folder(Some(&wd)), &matrix, &names_of(step));
            if a.needed && !a.commands.iter().any(|c| RELEVANT.contains(&c.kind.as_str())) && name_has(step.name.as_deref(), &["cache", "store"]) { a.needed = false; a.plumbing = Some("cache-plumbing".into()); }
            step.is_test = a.is_test;
            step.analysis = Some(a);
        } else if matches!(step.category.as_deref(), Some("wrapper:command" | "wrapper:run")) {
            let key = if step.category.as_deref() == Some("wrapper:command") { "command" } else { "run" };
            let text = text_of(step.with.get(key).unwrap_or(&Value::Null));
            step.wrapped = Some(text.chars().take(shell::TEXT_CHARS_MAX).collect());
            let wd = step.with.get("working-directory").map(text_of).filter(|w| !w.is_empty()).or_else(|| default_wd.clone()).unwrap_or_default();
            let folder = if step.with.get("working-directory").map(text_of).unwrap_or_default().contains("${{") { None } else { shell::join_dir(Some(""), Some(&wd)) };
            let a = analyze_run(ctx, &text, folder, &matrix, &names_of(step));
            step.is_test = a.is_test;
            step.analysis = Some(a);
        } else if step.category.as_deref() == Some("test-action") {
            step.is_test = true;
        } else if step.uses.is_some() && step.category.as_deref() == Some("unknown") && name_has(step.uses.as_deref(), &["test", "jest", "vitest", "playwright", "cypress"]) {
            step.maybe_test = true;
        }
    }
    view.is_test = view.steps.iter().any(|s| s.is_test);
    let pr = view.triggers.iter().any(|t| ["pull_request", "pull_request_target", "merge_group"].contains(&t.as_str()));
    view.caller_if_value = if view.caller_if.is_some() { condition(view.caller_if.as_deref(), &values) } else { Some(true) };
    view.pr_gate = pr && view.if_value != Some(false) && view.caller_if_value != Some(false);
}

fn matrix_variants(text: &str, matrix: &Value) -> Vec<String> {
    let keys: BTreeSet<String> = shell::expressions(text).iter().filter_map(|e| e.strip_prefix("matrix.")).filter(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')).map(str::to_string).collect();
    let Some(m) = matrix.as_object().filter(|_| !keys.is_empty()) else { return vec![text.to_string()] };
    let mut out = vec![text.to_string()];
    for key in &keys {
        let mut vals: Vec<Value> = m.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        for inc in m.get("include").and_then(Value::as_array).into_iter().flatten() { if let Some(v) = inc.get(key) && !vals.contains(v) { vals.push(v.clone()); } }
        let vals: Vec<String> = vals.iter().filter(|v| v.is_string() || v.is_number()).map(text_of).take(MATRIX_VARIANTS_COUNT_MAX).collect();
        let mut next = Vec::new();
        for t in &out {
            if vals.is_empty() { next.push(t.clone()); }
            for v in &vals { next.push(replace_matrix(t, key, v)); }
        }
        next.truncate(MATRIX_VARIANTS_COUNT_MAX);
        out = next;
    }
    let mut result = vec![text.to_string()];
    result.extend(out.into_iter().filter(|t| t != text).take(MATRIX_VARIANTS_COUNT_MAX - 1));
    result
}
fn replace_matrix(text: &str, key: &str, value: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${{") {
        let Some(j) = rest[i..].find("}}") else { break };
        out.push_str(&rest[..i]);
        if rest[i + 3..i + j].trim() == format!("matrix.{key}") { out.push_str(value); } else { out.push_str(&rest[i..i + j + 2]); }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}
fn github_file(t: &str, names: &[&str]) -> bool {
    let t = t.strip_prefix('$').unwrap_or(t);
    let t = t.strip_prefix('{').unwrap_or(t);
    let t = t.strip_suffix('}').unwrap_or(t);
    names.contains(&t)
}
struct Seen { b: BTreeSet<String>, s: BTreeSet<String>, defined: BTreeSet<String>, files: Vec<(String, String)>, sets_env: Vec<String> }
fn segment_command(ctx: &Ctx, sg: &shell::Seg, exprs: &[String], folder: Option<String>, seen: &mut Seen) -> (Cmd, Option<String>) {
    let words: Vec<String> = sg.words.iter().map(|w| shell::restore(&w.text, exprs)).collect();
    let mut k = 0;
    let mut assigns = Vec::new();
    while k < sg.words.len() && shell::is_assign(&sg.words[k].text) && !sg.words[k].start_quoted { assigns.push(words[k].split('=').next().unwrap_or("").to_string()); k += 1; }
    while k < sg.words.len() && LEADING_CONTROL.contains(&sg.words[k].text.as_str()) && !sg.words[k].quoted { if sg.words[k].text != "time" { seen.s.insert("control-flow".into()); } k += 1; }
    let (rest_raw, rest) = (&sg.words[k..], &words[k..]);
    if !assigns.is_empty() && rest.is_empty() { seen.defined.extend(assigns.iter().cloned()); seen.b.insert("shell-variable".into()); } else if !assigns.is_empty() { seen.b.insert("inline-env".into()); }
    let redirects: Vec<(String, String)> = sg.redirects.iter().map(|(op, t)| (op.clone(), shell::restore(t, exprs))).collect();
    let mut c = if !rest.is_empty() && CLOSING_CONTROL.contains(&rest_raw[0].text.as_str()) { seen.s.insert("control-flow".into()); Cmd::of("control") }
        else if rest.is_empty() { Cmd::of("noop") } else { shell::classify(rest, &Scripts { packages: ctx, folder: folder.clone() }, 0) };
    let mut folder = folder;
    match c.kind.as_str() {
        "cd" => { seen.b.insert("cd".into()); folder = shell::join_dir(folder.as_deref(), c.target.as_deref()); }
        "export" => { seen.b.insert("export".into()); seen.defined.extend(c.names.iter().cloned()); }
        "set" => { seen.b.insert("set-option".into()); }
        "control" => { seen.s.insert("control-flow".into()); }
        _ => if rest_raw.iter().any(|w| w.glob && !w.text.starts_with('-') && !w.quoted && !["[", "[[", "]", "]]"].contains(&w.text.as_str())) { seen.b.insert("glob".into()); },
    }
    let literal = rest_raw.iter().all(|w| !w.expands);
    let joined: String = rest.join(" ").chars().take(shell::VALUE_CHARS_MAX).collect();
    for (op, target) in &redirects {
        let t = target.trim_matches(['"', '\'']);
        if op.starts_with("<<") && !op.starts_with("<<<") { continue; }
        if ["/dev/null", "1", "2", "&1", "&2"].contains(&t) || op.ends_with('&') { seen.b.insert("output-redirect".into()); }
        else if github_file(t, &["GITHUB_ENV", "GITHUB_PATH"]) {
            seen.sets_env.push(joined.clone());
            if literal { seen.b.insert("sets-env".into()); } else { seen.s.insert("sets-env-at-run-time".into()); }
            c.kind = "sets-env".into();
        } else if github_file(t, &["GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY"]) { seen.b.insert("github-output".into()); }
        else if op.starts_with('<') { seen.s.insert("input-redirect".into()); }
        else {
            seen.files.push((t.to_string(), joined.clone()));
            let env = shell::env_file(t);
            if literal && rest.first().is_some_and(|r| ["echo", "printf"].contains(&shell::base(r).as_str())) && rest.len() > 1 { seen.b.insert("write-literal-file".into()); c.kind = if env { "env-file".into() } else { "file".into() }; }
            else { seen.s.insert("redirect-output-to-file".into()); if env { c.kind = "env-file".into(); } else if SKIPPABLE.contains(&c.kind.as_str()) { c.kind = "file".into(); } }
        }
    }
    match sg.sep.as_deref() { Some("&&") => { seen.b.insert("&&".into()); }, Some(";") => { seen.b.insert(";".into()); }, _ => {} }
    if (c.kind == "file" || c.kind == "env-file") && c.program.as_deref().is_some_and(|p| ["cp", "mv", "ln", "touch", "tee", "install", "envsubst"].contains(&p)) {
        seen.files.push((c.args.last().cloned().unwrap_or_default(), joined.clone()));
    }
    c.text = words[..assigns.len()].iter().chain(rest.iter()).cloned().collect::<Vec<_>>().join(" ").chars().take(shell::TEXT_CHARS_MAX).collect();
    c.wd = folder.clone();
    (c, folder)
}
fn expr_kind(e: &str) -> &'static str {
    let e = e.trim();
    if boundary_word(e, "secrets.") || e.contains("github.token") { return "secret"; }
    if boundary_word(e, "vars.") { return "var"; }
    if ["steps.", "needs.", "jobs."].iter().any(|w| boundary_word(e, w)) || ["hashFiles", "fromJSON", "toJSON", "format", "join", "contains", "startsWith", "endsWith"].iter().any(|f| e.match_indices(f).any(|(i, _)| (i == 0 || !is_word_byte(e.as_bytes()[i - 1])) && e[i + f.len()..].trim_start().starts_with('('))) { return "dynamic"; }
    "static"
}
fn is_word_byte(b: u8) -> bool { b.is_ascii_alphanumeric() || b == b'_' }
fn boundary_word(text: &str, word: &str) -> bool { text.match_indices(word).any(|(i, _)| i == 0 || !is_word_byte(text.as_bytes()[i - 1])) }
pub fn secret_names(text: &str) -> BTreeSet<String> {
    let mut names: BTreeSet<String> = text.match_indices("secrets.").filter_map(|(i, m)| { let id = shell::ident_prefix(&text[i + m.len()..]); (!id.is_empty()).then(|| id.to_string()) }).collect();
    if text.contains("github.token") { names.insert("GITHUB_TOKEN".into()); }
    names
}
fn var_names(text: &str) -> BTreeSet<String> {
    text.match_indices("vars.").filter(|(i, _)| *i == 0 || !is_word_byte(text.as_bytes()[i - 1])).filter_map(|(i, m)| { let id = shell::ident_prefix(&text[i + m.len()..]); (!id.is_empty()).then(|| id.to_string()) }).collect()
}
fn optional_name(name: &str) -> bool { OPTIONAL_SECRETS.contains(&name) || name.starts_with("TURBO_") || name.starts_with("NX_CLOUD") || name.starts_with("NX_") }

pub fn analyze_run(ctx: &Ctx, text: &str, wd: Option<String>, matrix: &Value, env_names: &BTreeSet<String>) -> Analysis {
    let block = shell::run_block(text);
    let mut seen = Seen { b: BTreeSet::new(), s: BTreeSet::new(), defined: env_names.iter().cloned().chain(RUNNER_VARS.iter().map(|s| s.to_string())).collect(), files: Vec::new(), sets_env: Vec::new() };
    let mut commands = Vec::new();
    let mut folder = wd.clone();
    for sg in &block.segs { let (c, f) = segment_command(ctx, sg, &block.exprs, folder, &mut seen); commands.push(c); folder = f; }
    for variant in matrix_variants(text, matrix).iter().skip(1) {
        let vb = shell::run_block(variant);
        if vb.segs.len() != block.segs.len() { continue; }
        let mut vfolder = wd.clone();
        let mut scratch = Seen { b: BTreeSet::new(), s: BTreeSet::new(), defined: seen.defined.clone(), files: Vec::new(), sets_env: Vec::new() };
        for (k, sg) in vb.segs.iter().enumerate() {
            let (c, f) = segment_command(ctx, sg, &vb.exprs, vfolder, &mut scratch);
            vfolder = f;
            let b = &mut commands[k];
            if c.kind == "test" && b.kind != "test" { b.kind = "test".into(); b.shape = c.shape.clone(); b.runner = c.runner.clone(); }
        }
    }
    for n in &block.names {
        if GITHUB_FILES.contains(&n.as_str()) { continue; }
        if seen.defined.contains(n) { seen.b.insert("env-variable".into()); } else { seen.s.insert(format!("runtime-variable:{n}")); }
    }
    seen.s.extend(shell::needs_shell(&block.feats));
    for e in &block.exprs { if expr_kind(&e[3..e.len() - 2]) == "dynamic" { seen.s.insert("dynamic-expression".into()); } }
    let kinds: Vec<String> = commands.iter().map(|c| c.kind.clone()).collect();
    let kinds: Vec<&str> = kinds.iter().map(String::as_str).collect();
    let mut needed = kinds.iter().any(|k| !SKIPPABLE.contains(k));
    let relevant: BTreeSet<&str> = kinds.iter().copied().filter(|k| RELEVANT.contains(k)).collect();
    let workspace_writes = seen.files.iter().any(|(t, _)| !(t.starts_with('$') || t.starts_with("/tmp/") || t.starts_with("/dev/") || t.starts_with('~')));
    let mut plumbing = None;
    if needed && relevant.is_empty() && seen.b.contains("github-output") && !workspace_writes { plumbing = Some("output-plumbing".to_string()); }
    else if needed && relevant.is_empty() && !kinds.is_empty() && kinds.iter().all(|k| SKIPPABLE.contains(k) || *k == "git-history" || *k == "git-checkout") && kinds.iter().any(|k| *k == "git-history" || *k == "git-checkout") { plumbing = Some("checkout-plumbing".to_string()); }
    if plumbing.is_some() { needed = false; }
    for c in &mut commands { if c.kind == "shell-script" && c.test { c.kind = "test".into(); c.shape = Some(format!("custom:{}", c.target.clone().or_else(|| c.program.clone()).unwrap_or_default())); c.runner = Some("custom".into()); } }
    let kern_replaces = !relevant.is_empty() && relevant.iter().all(|k| ["install", "tool-install", "browser-install", "file"].contains(k));
    let is_test = commands.iter().any(|c| c.kind == "test");
    Analysis { commands, b_features: seen.b, s_features: seen.s, needed, plumbing, kern_replaces, files: seen.files.into_iter().take(20).collect(), sets_env: seen.sets_env.into_iter().take(20).collect(), is_test }
}

pub fn linux(view: &View) -> (&'static str, Vec<String>) {
    let labels: Vec<String> = match &view.runs_on { Value::String(s) => vec![s.clone()], Value::Array(a) => a.iter().map(text_of).collect(), Value::Object(o) => vec![o.get("labels").map(text_of).unwrap_or_default(), o.get("group").map(text_of).unwrap_or_default()], _ => Vec::new() };
    let joined = labels.join(" ").to_ascii_lowercase();
    let any = |t: &str, words: &[&str]| words.iter().any(|w| t.contains(w));
    if joined.contains("${{") && !joined.contains("matrix.") {
        if any(&joined, &["ubuntu", "linux", "blacksmith", "depot-ubuntu", "buildjet", "namespace-profile"]) { return ("linux", labels); }
        if any(&joined, &["windows", "macos"]) { return ("not-linux", labels); }
        return ("unknown", labels);
    }
    if joined.contains("${{") {
        let key = joined.find("matrix.").map(|i| joined[i + 7..].bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-').count()).map(|n| joined[joined.find("matrix.").unwrap() + 7..][..n].to_string());
        if let (Some(key), Some(m)) = (key, view.matrix.as_object()) {
            let mut vals: Vec<String> = m.get(&key).and_then(Value::as_array).map(|a| a.iter().map(text_of).collect()).unwrap_or_default();
            vals.extend(m.get("include").and_then(Value::as_array).into_iter().flatten().filter_map(|inc| inc.get(&key)).map(text_of).filter(|v| !v.is_empty() && v != "false"));
            let text = vals.join(" ").to_ascii_lowercase();
            if text.is_empty() { return ("unknown", labels); }
            return (if any(&text, &["ubuntu", "linux"]) { "linux" } else { "not-linux" }, labels);
        }
        return ("unknown", labels);
    }
    if any(&joined, &["windows", "macos", "mac-", "osx"]) && !any(&joined, &["ubuntu", "linux"]) { return ("not-linux", labels); }
    if any(&joined, &["ubuntu", "linux", "blacksmith", "depot-ubuntu", "buildjet", "namespace-profile", "warp-ubuntu", "runs-on"]) { return ("linux", labels); }
    if joined.contains("self-hosted") { return ("self-hosted", labels); }
    ("unknown", labels)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reason { pub tier: char, pub code: String, pub detail: String }
#[derive(Clone, Debug, Default)]
pub struct Verdict { pub tier: char, pub reasons: Vec<Reason>, pub runners: Vec<String>, pub linux: String, pub plan_steps: usize, pub secrets_setup: Vec<String> }
fn reason(tier: char, code: &str, detail: String) -> Reason { Reason { tier, code: code.into(), detail: detail.chars().take(shell::VALUE_CHARS_MAX).collect() } }
fn clip(s: &str, n: usize) -> String { s.chars().take(n).collect() }
fn with_text(step: &Step) -> String { text_of(&Value::Object(step.with.clone())) }

fn action_reason(ctx: &Ctx, view: &View, siblings: &[View], s: &Step, depth: usize, reasons: &mut Vec<Reason>) {
    let cat = s.category.as_deref().unwrap_or("unknown");
    let action = s.action.clone().unwrap_or_default();
    let uses = s.uses.clone().unwrap_or_default();
    match cat {
        c if QUIET.contains(&c) || c == "local-composite" => {}
        "download-artifact" => {
            let found: Vec<&View> = siblings.iter().filter(|sib| !std::ptr::eq(*sib, view) && sib.steps.iter().any(|x| x.category.as_deref() == Some("upload-artifact")) && (
                (view.needs.contains(&sib.job) && sib.workflow == view.workflow && sib.via == view.via) || (view.caller_needs.contains(&sib.job) && Some(&sib.workflow) == view.caller_workflow.as_ref() && sib.via.is_none()))).collect();
            if !found.is_empty() && depth < 2 {
                for up in found {
                    let sub = judge(ctx, up, siblings, depth + 1);
                    reasons.push(reason('B', "upstream-job", format!("downloads artifacts from job {} (its own tier: {})", up.job, sub.tier)));
                    for r in sub.reasons { if rank(r.tier) > rank('B') { reasons.push(Reason { code: format!("upstream:{}", r.code), ..r }); } }
                }
            } else {
                let ups: Vec<String> = view.needs.iter().chain(view.caller_needs.iter()).cloned().collect();
                reasons.push(reason('S', "artifact-from-unknown-job", format!("downloads artifacts from a job Kern cannot trace ({})", if ups.is_empty() { "none".to_string() } else { ups.join(", ") })));
            }
        }
        "wrapper:command" | "wrapper:run" => reasons.push(reason('B', "wrapper-action", format!("{action} runs: {}", clip(s.wrapped.as_deref().unwrap_or(""), 120)))),
        "toolchain" => reasons.push(reason('S', "other-toolchain", format!("sets up {action}"))),
        "setup-other-pm" => reasons.push(reason('S', "unpinned-tool", format!("{action} installs a tool Kern does not pin yet (as Bun was before)"))),
        "file-action" => reasons.push(reason('B', "file-action", format!("{action} writes a file from its inputs: {}", clip(&with_text(s), 120)))),
        "ai-agent" => reasons.push(reason('E', "ai-agent", format!("{action} runs an AI agent"))),
        "browser" => reasons.push(reason('S', "browser-action", format!("sets up a browser with {action}"))),
        "docker" | "docker-image-action" => reasons.push(reason('C', "docker-action", format!("{} uses Docker (registry login, image build or a container step)", if action.is_empty() { &uses } else { &action }))),
        "service" => reasons.push(reason('C', "service-action", format!("{action} starts a service"))),
        "cloud-auth" => reasons.push(reason('D', "cloud-credentials", format!("{action} sets up cloud credentials"))),
        "script" => reasons.push(reason('E', "github-script", "actions/github-script runs inline JavaScript against the GitHub API".into())),
        "test-action" => reasons.push(reason('E', "test-action", format!("the tests run inside {action}, which Kern cannot follow"))),
        "local-unreadable" | "local-javascript" | "local-docker" => reasons.push(reason('E', cat, format!("{uses}: {}", s.problem.clone().unwrap_or_default()))),
        _ => reasons.push(reason('E', "unknown-action", format!("{uses} is not an action Kern knows"))),
    }
    if cat == "setup-pnpm" && !["false", "", "none"].contains(&text_of(s.with.get("run_install").unwrap_or(&Value::Bool(false))).to_ascii_lowercase().as_str()) {
        reasons.push(reason('B', "action-installs", format!("pnpm/action-setup run_install: {}", clip(&text_of(s.with.get("run_install").unwrap_or(&Value::Null)), 120))));
    }
}
fn run_reasons(view: &View, s: &Step, a: &Analysis, reasons: &mut Vec<Reason>) {
    let shell = s.shell.clone().or_else(|| view.default_shell.clone()).unwrap_or_default().to_ascii_lowercase();
    if !(a.needed || s.is_test) { return; }
    if ["pwsh", "powershell", "cmd", "python"].iter().any(|p| shell.starts_with(p)) { reasons.push(reason('S', "non-posix-shell", format!("the step runs in {shell}"))); }
    for f in &a.b_features { reasons.push(reason('B', &format!("shell:{f}"), f.clone())); }
    for f in &a.s_features {
        if a.kern_replaces && !s.is_test { reasons.push(reason('B', &format!("kern-replaces:{f}"), format!("{f} in a step Kern replaces with its own install or browser setup"))); }
        else { reasons.push(reason('S', &format!("shell:{f}"), f.clone())); }
    }
    for c in &a.commands { if c.kind == "git-history" { reasons.push(reason('B', "git-history", format!("git {}: Kern has the tree; with no history it would run every test, not only the affected ones", c.subcommand.clone().unwrap_or_default()))); } }
    for c in &a.commands {
        if let Some((_, what)) = S_KINDS.iter().find(|(k, _)| *k == c.kind) { reasons.push(reason('S', &c.kind, format!("{what}: {}", clip(&c.text, 160)))); }
        if c.kind == "services" { reasons.push(reason('C', "services-command", clip(&c.text, 160))); }
        if c.kind != "services" && c.body.as_deref().is_some_and(shell::service_script) { reasons.push(reason('C', "services-in-script", format!("{} runs {}", c.script.clone().unwrap_or_default(), clip(c.body.as_deref().unwrap_or(""), 160)))); }
    }
}
pub fn judge(ctx: &Ctx, view: &View, siblings: &[View], depth: usize) -> Verdict {
    let mut reasons = Vec::new();
    let last = view.steps.iter().rposition(|s| s.is_test).unwrap_or(view.steps.len().saturating_sub(1));
    let plan_steps = &view.steps[..(last + 1).min(view.steps.len())];
    for s in plan_steps {
        if s.skipped.is_some() { continue; }
        if s.uses.is_some() { action_reason(ctx, view, siblings, s, depth, &mut reasons); }
        if let Some(a) = &s.analysis { run_reasons(view, s, a, &mut reasons); }
    }
    for (name, image) in &view.services { reasons.push(reason('C', "job-service", format!("{name}: {image}"))); }
    if !view.container.is_null() && !(view.container.as_str().is_some_and(str::is_empty)) {
        let image = view.container.get("image").map_or_else(|| text_of(&view.container), text_of);
        let i = image.strip_prefix("docker.io/").unwrap_or(&image);
        let i = i.strip_prefix("library/").unwrap_or(i);
        if i == "node" || i.starts_with("node:") || image.starts_with("mcr.microsoft.com/playwright") { reasons.push(reason('B', "container", format!("runs in {image}"))); } else { reasons.push(reason('S', "custom-container", format!("runs in {image}"))); }
        if let Some(c) = view.container.get("credentials") && !secret_names(&text_of(c)).is_empty() { reasons.push(reason('D', "container-credentials", format!("pulls its container with secrets {:?}", secret_names(&text_of(c))))); }
    }
    let (os_kind, labels) = linux(view);
    if os_kind == "not-linux" { reasons.push(reason('S', "not-linux", format!("runs on {}", labels.join(", ")))); }
    if view.matrix_dynamic { reasons.push(reason('S', "dynamic-matrix", format!("the matrix comes from an expression: {}", clip(&text_of(&view.matrix), 160)))); }
    let (mut secrets_needed, mut vars_needed, mut secrets_setup) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for s in plan_steps {
        if s.skipped.is_some() { continue; }
        let env_text = [text_of(&Value::Object(view.workflow_env.clone())), text_of(&Value::Object(view.job_env.clone())), text_of(&Value::Object(s.env.clone()))].join(" ");
        let inputs = Value::Object(s.inputs.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect());
        let own = [s.run.clone().unwrap_or_default(), with_text(s), if s.inputs.is_empty() { String::new() } else { text_of(&inputs) }, s.wrapped.clone().unwrap_or_default()].join(" ");
        let all = format!("{env_text} {own}");
        if s.is_test {
            secrets_needed.extend(secret_names(&all).into_iter().filter(|n| !optional_name(n)));
            vars_needed.extend(var_names(&all).into_iter().filter(|n| !optional_name(n)));
        } else { secrets_setup.extend(secret_names(&all)); }
    }
    for n in &secrets_needed { reasons.push(reason('D', "secret", format!("secrets.{n} reaches a test step"))); }
    for n in &vars_needed { reasons.push(reason('D', "repository-variable", format!("vars.{n} reaches a test step (repository variables are not in the repository)"))); }
    let tier = reasons.iter().map(|r| r.tier).max_by_key(|t| rank(*t)).unwrap_or('A');
    let runners: BTreeSet<String> = plan_steps.iter().filter_map(|s| s.analysis.as_ref()).flat_map(|a| a.commands.iter().filter(|c| c.kind == "test").filter_map(|c| c.runner.clone())).collect();
    let mut deduped: Vec<Reason> = Vec::new();
    for r in reasons { if !deduped.contains(&r) { deduped.push(r); } }
    deduped.truncate(REASONS_COUNT_MAX);
    Verdict { tier, reasons: deduped, runners: runners.into_iter().collect(), linux: os_kind.into(), plan_steps: plan_steps.len(), secrets_setup: secrets_setup.into_iter().collect() }
}

#[derive(Clone, Debug)]
pub struct Job { pub view: View, pub verdict: Verdict }
#[derive(Clone, Debug)]
pub struct Measure { pub tier: char, pub best: Option<(String, String)>, pub jobs: Vec<Job>, pub blocker: Option<Reason>, pub errors: Vec<String> }

fn job_order(j: &Job) -> (bool, usize, bool, bool, usize, String, String) {
    let unit = j.verdict.runners.iter().any(|r| !shell::E2E_RUNNERS.contains(&r.as_str()));
    let kern = j.verdict.runners.iter().any(|r| shell::KERN_RUNNERS.contains(&r.as_str()));
    (!j.view.pr_gate, rank(j.verdict.tier), !unit, !kern, j.verdict.reasons.len(), j.view.workflow.clone(), j.view.job.clone())
}
fn testish(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    ["test", "check", "e2e", "unit", "jest", "vitest", "spec", "verify"].iter().any(|w| l.contains(w)) || l.match_indices("ci").any(|(i, _)| l.as_bytes().get(i + 2).is_none_or(|b| !is_word_byte(*b)))
}
pub fn measure(tree: &Tree) -> Measure {
    let ctx = Ctx::new(tree);
    let mut called = BTreeSet::new();
    let mut views = Vec::new();
    for (path, doc) in &ctx.workflows { views.extend(workflow_jobs(&ctx, path, doc, &BTreeMap::new(), None, 0, &mut called)); }
    views.retain(|v| !(called.contains(&v.workflow) && v.via.is_none() && ctx.workflows.get(&v.workflow).is_some_and(only_called)));
    for v in &mut views { if !v.unreadable_call { analyze_view(&ctx, v); } }
    let mut jobs: Vec<Job> = views.iter().filter(|v| v.is_test).map(|v| Job { view: v.clone(), verdict: judge(&ctx, v, &views, 0) }).collect();
    jobs.sort_by_key(job_order);
    let errors = ctx.errors.borrow().clone();
    if let Some(best) = jobs.first() {
        let blocker = best.verdict.reasons.iter().max_by_key(|r| rank(r.tier)).cloned();
        return Measure { tier: best.verdict.tier, best: Some((best.view.workflow.clone(), best.view.job.clone())), jobs, blocker, errors };
    }
    let unreadable: Vec<String> = views.iter().filter(|v| !v.is_test).filter_map(|v| {
        if v.unreadable_call && let Some(u) = &v.reusable_uses && testish(u.split('@').next().unwrap_or("").rsplit('/').next().unwrap_or("")) { return Some(u.clone()); }
        let maybe: Vec<String> = v.steps.iter().filter(|s| s.maybe_test).filter_map(|s| s.uses.clone()).collect();
        (!maybe.is_empty()).then(|| maybe.join(", "))
    }).collect();
    if !unreadable.is_empty() {
        let blocker = reason('E', "unreadable-call", format!("no readable test job; the tests may run inside {}", unreadable.join(", ")));
        return Measure { tier: 'E', best: None, jobs, blocker: Some(blocker), errors };
    }
    Measure { tier: 'F', best: None, jobs, blocker: Some(reason('F', "no-test-job", "no job in .github/workflows runs the tests".into())), errors }
}
