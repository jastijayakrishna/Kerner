use crate::leaf::Recorded;
use crate::plan::{AcceptedPlan, Kind, Outcome, Plan, Rejected, Run, Source, StepRecord};
use crate::proof::Report;
use crate::suite::Side;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub const PLAN_SECS_MAX: u64 = 1800;
pub const PLAN_SECS_MIN: u64 = 120;
pub const PLAN_STEP_SECS_MAX: u64 = crate::sandbox::PLAN_STEP_SECS_MAX;
pub const PLAN_FILE_BYTES_MAX: usize = 4 << 20;
pub const SETUP_FILE_BYTES_MAX: usize = 64 << 20;
pub const PLANS_ENV: &str = "KERN_PLANS";
pub const NOT_REACHED: &str = "it was not run: an earlier step did not finish in Kern's time";

pub fn unsupported_needs(needs: &[String]) -> Option<String> {
    (!needs.is_empty()).then(|| format!("not supported here: needs {} (the project's CI job runs its tests with it, and Kern's sandbox gives tests no services or secrets)", needs.join(", ")))
}
pub fn unsupported(plan: &Plan) -> Option<String> {
    let needs: Vec<String> = plan.needs.iter().map(|n| match n { crate::plan::Need::Service(s) => s.clone(), crate::plan::Need::Secret(s) => format!("the secret {s}") }).collect();
    unsupported_needs(&needs)
}
pub fn fits(plan: &Plan) -> Result<(), String> {
    let env: BTreeMap<String, String> = plan.env.iter().cloned().collect();
    if !crate::sandbox::box_env_fits(&env) { return Err(crate::leaf::box_env_why()); }
    let count = plan.steps.iter().filter(|s| s.kind != Kind::Install).count();
    if count > crate::sandbox::STEPS_COUNT_MAX { return Err(format!("the plan has {count} steps after its installs, and Kern's test box runs at most {}", crate::sandbox::STEPS_COUNT_MAX)); }
    Ok(())
}
pub fn listed(plan: &Plan) -> Vec<Value> {
    plan.steps.iter().filter(|s| s.kind != Kind::Install).map(|s| json!({ "id": s.id, "kind": s.kind.name(), "dir": s.dir, "line": s.line })).collect()
}
pub fn install_records(plan: &Plan, setup: &Value, side: &str) -> Vec<StepRecord> {
    plan.installs().map(|s| {
        let outcome = if s.line.starts_with("npx playwright install") {
            match &setup[side]["browser"] {
                Value::Null => Err("Kern did not set up a browser on this side".to_string()),
                b => b["error"].as_str().map_or(Ok(()), |e| Err(format!("Kern could not set up the browser: {e}"))),
            }
        } else {
            match crate::suite::install_result(setup, side, &s.dir) {
                None => Err(format!("Kern did not install in {}", if s.dir.is_empty() { "." } else { &s.dir })),
                Some(_) => crate::suite::install_failure(setup, side, &s.dir).map_or(Ok(()), Err),
            }
        };
        StepRecord { id: s.id.clone(), kind: Kind::Install, exit: outcome.as_ref().ok().map(|_| 0), error: outcome.err().unwrap_or_default() }
    }).collect()
}
pub type Leaves = Vec<(String, Result<Recorded, String>)>;
pub struct Ran { pub steps: Vec<StepRecord>, pub leaves: Leaves }
fn step_error(errors: &str) -> String {
    let said = crate::sandbox::first_error(errors).map_or_else(|| errors.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("").to_string(), str::to_string);
    plain(&said, crate::plan::TRIAL_ERROR_BYTES_MAX)
}
pub fn read_run(plan: &Plan, output: &[u8]) -> R<(Vec<StepRecord>, Leaves)> {
    let v: Value = serde_json::from_slice(output).map_err(|_| Denied("the plan's step results could not be read".into()))?;
    let results = v.as_array().ok_or_else(|| Denied("the plan's step results could not be read".into()))?;
    let (mut steps, mut leaves) = (Vec::new(), Vec::new());
    for s in plan.steps.iter().filter(|s| s.kind != Kind::Install) {
        let record = results.iter().find(|r| r["name"] == s.id);
        let finished = record.is_some_and(|r| r["finished"] == true);
        let exit = record.filter(|_| finished).and_then(|r| r["exit"].as_i64()).and_then(|e| i32::try_from(e).ok());
        crate::ensure!(record.is_none_or(|r| r.get("exit").is_some_and(|e| e.is_null() || e.is_i64())), "a plan step's result has no exit status");
        let error = match record {
            None => NOT_REACHED.to_string(),
            Some(_) if !finished => format!("it did not finish in the {PLAN_STEP_SECS_MAX} s Kern gives one step"),
            Some(r) => if exit == Some(0) { String::new() } else { step_error(r["errors"].as_str().unwrap_or("")) },
        };
        steps.push(StepRecord { id: s.id.clone(), kind: s.kind, exit, error });
        if s.kind == Kind::Test {
            leaves.push((s.id.clone(), match record {
                None => Err(NOT_REACHED.to_string()),
                Some(r) => crate::leaf::read(r["leaves"].as_str().unwrap_or("").as_bytes()),
            }));
        }
    }
    Ok((steps, leaves))
}
pub fn recorded(ran: &Ran) -> Result<Recorded, String> {
    let mut all = Recorded { leaves: Vec::new(), exit: Some(0), why: None };
    for (id, r) in &ran.leaves {
        let r = r.as_ref().map_err(|e| format!("its test step `{id}` could not be recorded: {e}"))?;
        all.leaves.extend(r.leaves.iter().cloned());
        if all.exit == Some(0) && all.why.is_none() { (all.exit, all.why) = (r.exit, r.why.clone()); }
    }
    if all.leaves.len() > crate::leaf::LEAVES_COUNT_MAX { return Err(format!("the plan's test steps started more than {} test runs", crate::leaf::LEAVES_COUNT_MAX)); }
    Ok(all)
}
pub fn uneven(old: &[StepRecord], new: &[StepRecord]) -> Option<String> {
    let steps = |records: &[StepRecord]| -> Vec<crate::suite::Step> {
        records.iter().filter(|s| s.error != NOT_REACHED).map(|s| crate::suite::Step { name: s.id.clone(), exit: s.exit.map(i64::from), errors: s.error.clone() }).collect()
    };
    crate::suite::uneven_steps(&steps(old), &steps(new)).map(|u| format!("the plan's steps did not go the same way on both sides: {u}"))
}
pub fn pair(out: &Path, plan: &Plan, setup: &Value, (old, new): (&str, &str), until: u64) -> R<(Ran, Ran)> {
    fits(plan).map_err(Denied)?;
    Ok((side(out, plan, setup, ("old", old), until)?, side(out, plan, setup, ("new", new), until)?))
}
fn side(out: &Path, plan: &Plan, setup: &Value, (name, volume): (&str, &str), until: u64) -> R<Ran> {
    let seconds = within(until, PLAN_SECS_MAX);
    crate::ensure!(seconds >= PLAN_SECS_MIN, "Kern's time for this check ran out before the plan could run on the {name} code");
    let env: BTreeMap<&str, &str> = plan.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let config = json!({ "version": 1, "command": ["/usr/local/bin/node"], "timeout_seconds": seconds, "cwd": "", "env": env, "files": {}, "early_secs": 0, "faults": [], "serve": {}, "steps": listed(plan) });
    let run = crate::suite::in_test_box(out, volume, &format!("plan-{name}"), ("drive-test.json", &config), ("__drive_test", false, seconds))?;
    let parts = crate::sandbox::parts(&run.stdout, &crate::sandbox::TEST_PARTS)?;
    crate::ensure!(parts["ending"] == b"steps", "the plan did not finish on the {name} code in the {seconds} s Kern gave it ({})", String::from_utf8_lossy(&parts["ending"]));
    let _ = std::fs::write(out.join(format!("plan-{name}.json")), &parts["output"]);
    let (steps, leaves) = read_run(plan, &parts["output"])?;
    let mut all = install_records(plan, setup, name);
    all.extend(steps);
    Ok(Ran { steps: all, leaves })
}

pub fn outcomes(reports: &[Report]) -> BTreeMap<String, Outcome> {
    let mut out = BTreeMap::new();
    for r in reports {
        for (k, c) in &r.tests { out.insert(k.clone(), if c.failed > 0 { Outcome::Failed } else if c.passed > 0 { Outcome::Passed } else { Outcome::Skipped }); }
    }
    out
}
pub fn run_of(ran: &Ran, reports: &[Report]) -> Run { Run { steps: ran.steps.clone(), reports: reports.len(), outcomes: outcomes(reports) } }
fn outcome_name(o: Outcome) -> &'static str { match o { Outcome::Passed => "passed", Outcome::Failed => "failed", Outcome::Skipped => "skipped" } }
pub fn run_json(r: &Run) -> Value {
    json!({ "steps": r.steps.iter().map(|s| json!({ "id": s.id, "kind": s.kind.name(), "exit": s.exit, "error": s.error })).collect::<Vec<_>>(), "reports": r.reports,
        "outcomes": r.outcomes.iter().map(|(k, o)| (k.clone(), json!(outcome_name(*o)))).collect::<serde_json::Map<_, _>>() })
}
pub fn run_parse(v: &Value) -> R<Run> {
    exact(v, &["steps", "reports", "outcomes"], "a plan run")?;
    let steps = v["steps"].as_array().ok_or_else(|| Denied("a plan run's steps are not a list".into()))?.iter().map(|s| {
        exact(s, &["id", "kind", "exit", "error"], "a plan run's step")?;
        let kind = [Kind::Install, Kind::Run, Kind::Test, Kind::Check].into_iter().find(|k| s["kind"] == k.name()).ok_or_else(|| Denied("a plan run's step has no kind".into()))?;
        let exit = match &s["exit"] { Value::Null => None, e => Some(e.as_i64().and_then(|e| i32::try_from(e).ok()).ok_or_else(|| Denied("a plan run's step has no exit status".into()))?) };
        Ok(StepRecord { id: text(s, "id")?.to_string(), kind, exit, error: text(s, "error")?.to_string() })
    }).collect::<R<Vec<_>>>()?;
    let reports = v["reports"].as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| Denied("a plan run's report count is not a number".into()))?;
    let outcomes = v["outcomes"].as_object().ok_or_else(|| Denied("a plan run's outcomes are not an object".into()))?.iter().map(|(k, o)| {
        let outcome = [Outcome::Passed, Outcome::Failed, Outcome::Skipped].into_iter().find(|x| *o == outcome_name(*x)).ok_or_else(|| Denied(format!("a plan run's outcome for {k} is not passed, failed or skipped")))?;
        Ok((k.clone(), outcome))
    }).collect::<R<BTreeMap<_, _>>>()?;
    Ok(Run { steps, reports, outcomes })
}

pub enum Stored { Accepted([Run; 2]), Rejected(String), Unsupported(String) }
pub enum Cached { Accepted(Box<AcceptedPlan>), Unsupported(String) }
pub fn plan_file(key: &str) -> String { format!("plan-{key}.json") }
pub fn rejection(r: &Rejected) -> String {
    match r {
        Rejected::StepFailed { run, step, exit, error } => format!("run {run}: step `{step}` ended with {}{}", exit.map_or("no exit status".to_string(), |e| format!("exit {e}")), if error.is_empty() { String::new() } else { format!(": {error}") }),
        Rejected::NoReport { run } => format!("run {run}: no test runner reported results"),
        Rejected::NoTestPassed { run } => format!("run {run}: no test passed"),
        Rejected::Unsteady { test, first, second } => format!("`{test}` was {} in the first run and {} in the second", first.map_or("absent", outcome_name), second.map_or("absent", outcome_name)),
    }
}
pub fn store(dir: &Path, key: &str, plan: Option<&Plan>, stored: &Stored) -> R<()> {
    let mut v = json!({ "version": 1, "key": key, "plan": plan.map(crate::plan::json) });
    match stored {
        Stored::Accepted(runs) => v["accepted"] = json!({ "runs": runs.iter().map(run_json).collect::<Vec<_>>() }),
        Stored::Rejected(why) => v["rejected"] = json!(why),
        Stored::Unsupported(why) => v["unsupported"] = json!(why),
    }
    let file = dir.join(plan_file(key));
    if file.exists() { std::fs::remove_file(&file)?; }
    write_json(file, &v)
}
pub fn cached(dir: &Path, key: &str) -> Option<Cached> {
    let file = dir.join(plan_file(key));
    if std::fs::metadata(&file).ok()?.len() > PLAN_FILE_BYTES_MAX as u64 { return None; }
    let v = read_json(&file).ok()?;
    if v["key"] != key { return None; }
    if let Some(why) = v["unsupported"].as_str() { return Some(Cached::Unsupported(why.to_string())); }
    let plan = crate::plan::parse(&v["plan"]).ok().filter(|p| p.key == key)?;
    let runs: Vec<Run> = v["accepted"]["runs"].as_array()?.iter().map(run_parse).collect::<R<Vec<_>>>().ok()?;
    let [first, second] = runs.as_slice() else { return None };
    crate::plan::accept(plan, [first, second], &BTreeSet::new()).ok().map(|a| Cached::Accepted(Box::new(a)))
}

pub fn workflows(repo: &Path, commit: &str) -> R<Vec<(String, Vec<u8>)>> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for item in crate::drive::listing(repo, commit)? {
        let p = item.path.as_str();
        if !(p.starts_with(".github/workflows/") && p.matches('/').count() == 2 && (p.ends_with(".yml") || p.ends_with(".yaml"))) { continue; }
        if files.len() >= crate::ci::WORKFLOW_FILES_COUNT_MAX { break; }
        if let Ok(bytes) = crate::git::git_with(repo, &["show", &format!("{commit}:{p}")], None, crate::yaml::YAML_BYTES_MAX) { files.push((p.to_string(), bytes)); }
    }
    files.sort();
    Ok(files)
}
pub fn repo_name(repo: &Path) -> String {
    let url = crate::git::git(repo, &["config", "--get", "remote.origin.url"], None).ok().map(|b| String::from_utf8_lossy(&b).trim().trim_end_matches(".git").to_string()).unwrap_or_default();
    let parts: Vec<&str> = url.rsplit(['/', ':']).take(2).collect();
    if parts.len() == 2 { format!("{}/{}", parts[1], parts[0]) } else { "unknown/unknown".to_string() }
}
pub fn setup_key_at(repo: &Path, commit: &str) -> R<String> {
    let workflows = workflows(repo, commit)?;
    let read = |p: &str| crate::git::git_with(repo, &["show", &format!("{commit}:{p}")], None, SETUP_FILE_BYTES_MAX).ok();
    Ok(crate::plan::setup_key(&crate::ci::Tree { repo: repo_name(repo), workflows, read: &read }))
}
pub fn source_json(plan: &Plan) -> Value {
    match &plan.source {
        Source::Ci { file, job } => json!({ "kind": "ci", "file": file, "job": job }),
        Source::PackageJson => json!({ "kind": "package-json" }),
        Source::Ai { model, round } => json!({ "kind": "ai", "model": model, "round": round }),
    }
}
fn judged(out: &Path, ran: &Ran, (name, volume): (&str, &str), until: u64) -> Run {
    let Ok(all) = recorded(ran) else { return run_of(ran, &[]) };
    let mut reports = Vec::new();
    for (i, r) in all.leaves.iter().filter(|r| !crate::leaf::informational(r)).take(crate::leaf::LEAF_SUITES_COUNT_MAX).enumerate() {
        let Ok((tests, env)) = crate::leaf::side(r) else { continue };
        let dir = r.dir.clone().unwrap_or_default();
        if let Ok(Side::Ran(report, _)) = crate::suite::run_leaf(out, (volume, crate::suite::test_seconds()), (&tests, &env), &dir, &format!("plan-{name}-leaf{i}"), until) { reports.push(report); }
    }
    run_of(ran, &reports)
}
pub fn learn(repo: &Path, commit: &str, out: &Path, op: &crate::policy::Operator, until: u64) -> R<Value> {
    crate::drive::prepare(repo, commit, commit, out)?;
    let workflows = workflows(repo, commit)?;
    let read = |p: &str| crate::git::git_with(repo, &["show", &format!("{commit}:{p}")], None, SETUP_FILE_BYTES_MAX).ok();
    let tree = crate::ci::Tree { repo: repo_name(repo), workflows, read: &read };
    let key = crate::plan::setup_key(&tree);
    let file = out.join(plan_file(&key));
    let plan = match crate::plan::read(&tree) {
        Ok(p) => p,
        Err(u) => {
            let Some(why) = unsupported_needs(&u.services) else { return Ok(json!({ "status": "UNREADABLE", "tier": u.tier.to_string(), "why": u.why, "key": key })) };
            store(out, &key, None, &Stored::Unsupported(why.clone()))?;
            return Ok(json!({ "status": "UNSUPPORTED", "why": why, "key": key, "file": file }));
        }
    };
    if let Some(why) = unsupported(&plan) {
        store(out, &key, Some(&plan), &Stored::Unsupported(why.clone()))?;
        return Ok(json!({ "status": "UNSUPPORTED", "why": why, "key": key, "source": source_json(&plan), "file": file }));
    }
    let setup = crate::drive::setup(out, op, until)?;
    let volume = |side: &str| setup[side]["volume"].as_str().unwrap_or("").to_string();
    let (old, new) = (volume("old"), volume("new"));
    let ran = pair(out, &plan, &setup, (&old, &new), until);
    let result = ran.map(|(a, b)| [judged(out, &a, ("old", &old), until), judged(out, &b, ("new", &new), until)]);
    let _ = crate::drive::clean(out);
    let runs = result?;
    let (status, why, stored) = match crate::plan::accept(plan.clone(), [&runs[0], &runs[1]], &BTreeSet::new()) {
        Ok(a) => ("ACCEPTED", format!("reproduced the base's tests twice ({} tests)", a.tests_count()), Stored::Accepted(runs)),
        Err(r) => ("REJECTED", rejection(&r), Stored::Rejected(rejection(&r))),
    };
    store(out, &key, Some(&plan), &stored)?;
    Ok(json!({ "status": status, "why": why, "key": key, "source": source_json(&plan), "file": file }))
}
