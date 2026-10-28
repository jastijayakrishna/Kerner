use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixture() -> Option<PathBuf> {
    let dir = PathBuf::from("C:/Users/krish/Downloads/kern-dev/data/wave1/ci-files");
    if dir.join("pairs.json").is_file() { Some(dir) } else { println!("skipped: the CI fixture {} is absent", dir.display()); None }
}

fn tree_of<'a>(root: &'a Path, repo: &str, read: &'a dyn Fn(&str) -> Option<Vec<u8>>) -> kern::ci::Tree<'a> {
    let mut workflows: Vec<(String, Vec<u8>)> = std::fs::read_dir(root.join(".github/workflows")).map(|d| d.flatten().filter(|e| e.path().is_file()).filter_map(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        (name.to_ascii_lowercase().ends_with(".yml") || name.to_ascii_lowercase().ends_with(".yaml")).then(|| (format!(".github/workflows/{name}"), std::fs::read(e.path()).unwrap()))
    }).collect()).unwrap_or_default();
    workflows.sort();
    kern::ci::Tree { repo: repo.to_string(), workflows, read }
}

#[test]
fn the_machine_reader_gives_the_tier_and_the_job_ci_jobs_json_recorded_for_every_stored_base_commit() {
    let Some(dir) = fixture() else { return };
    let pairs: Value = serde_json::from_slice(&std::fs::read(dir.join("pairs.json")).unwrap()).unwrap();
    let mut wrong = Vec::new();
    let mut agreed = 0;
    for pair in pairs["pairs"].as_array().unwrap() {
        let (repo, sha) = (pair["repo"].as_str().unwrap(), pair["base_sha"].as_str().unwrap());
        let root = dir.join(repo.replace('/', "__")).join(sha);
        let read = |path: &str| std::fs::read(root.join(path)).ok();
        let tree = tree_of(&root, repo, &read);
        let m = kern::ci::measure(&tree);
        let want_tier = pair["tier"].as_str().unwrap().chars().next().unwrap();
        let want_job = pair["best_job"].as_str().map(str::to_string);
        let got_job = m.best.as_ref().map(|(w, j)| format!("{w}:{j}"));
        if m.tier == want_tier && got_job == want_job { agreed += 1; } else { wrong.push(format!("{repo}@{} : want {want_tier} {want_job:?}, got {} {got_job:?} ({:?})", &sha[..8], m.tier, m.blocker.as_ref().map(|r| format!("{} {}", r.code, r.detail)))); }
    }
    println!("tier agreement: {agreed} of {}", pairs["pairs"].as_array().unwrap().len());
    assert!(wrong.is_empty(), "{} pairs disagree with ci-jobs.json:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn the_yaml_reader_reads_every_stored_workflow_file_as_pyyaml_does() {
    let Some(dir) = fixture() else { return };
    let Ok(oracle) = std::fs::read_to_string(dir.join("yaml-oracle.jsonl")) else { println!("skipped: no yaml-oracle.jsonl"); return };
    let (mut same, mut wrong) = (0, Vec::new());
    for line in oracle.lines().filter(|l| !l.trim().is_empty()) {
        let rec: Value = serde_json::from_str(line).unwrap();
        let file = rec["file"].as_str().unwrap();
        let bytes = std::fs::read(dir.join(file)).unwrap();
        let mine = kern::yaml::read(&bytes);
        match (&rec.get("doc"), mine) {
            (Some(want), Ok(got)) if **want == got => same += 1,
            (None, Err(_)) => same += 1,
            (Some(_), Err(e)) => wrong.push(format!("{file}: refused: {}", e.0)),
            (None, Ok(_)) => wrong.push(format!("{file}: read what PyYAML refused: {}", rec["error"])),
            (Some(want), Ok(got)) => wrong.push(format!("{file}: differs at {}", first_difference(want, &got, "$"))),
        }
    }
    println!("yaml agreement: {same} of {}", same + wrong.len());
    assert!(wrong.is_empty(), "{} files differ from PyYAML:\n{}", wrong.len(), wrong.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
}

fn first_difference(a: &Value, b: &Value, at: &str) -> String {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x { match y.get(k) { Some(w) if w == v => {}, Some(w) => return first_difference(v, w, &format!("{at}.{k}")), None => return format!("{at}.{k} missing") } }
            for k in y.keys() { if !x.contains_key(k) { return format!("{at}.{k} extra"); } }
            format!("{at} ?")
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (v, w)) in x.iter().zip(y).enumerate() { if v != w { return first_difference(v, w, &format!("{at}[{i}]")); } }
            format!("{at} length {} vs {}", x.len(), y.len())
        }
        _ => format!("{at}: want {} got {}", serde_json::to_string(a).unwrap().chars().take(200).collect::<String>(), serde_json::to_string(b).unwrap().chars().take(200).collect::<String>()),
    }
}

#[test]
#[ignore]
fn show_one_pair() {
    let Some(dir) = fixture() else { return };
    let want = std::env::var("KERN_CI_PAIR").unwrap_or_default();
    let (repo, sha) = want.split_once('@').unwrap();
    let root = dir.join(repo.replace('/', "__")).join(sha);
    let read = |path: &str| std::fs::read(root.join(path)).ok();
    let tree = tree_of(&root, repo, &read);
    let m = kern::ci::measure(&tree);
    println!("tier {} best {:?} errors {:?}", m.tier, m.best, m.errors);
    for j in &m.jobs {
        println!("{} {} {} pr_gate={} via={:?} triggers={:?} if={:?}={:?} caller_if={:?}={:?} runners={:?} reasons={}", j.view.workflow, j.view.job, j.verdict.tier, j.view.pr_gate, j.view.via, j.view.triggers, j.view.if_, j.view.if_value, j.view.caller_if, j.view.caller_if_value, j.verdict.runners, j.verdict.reasons.len());
        for r in &j.verdict.reasons { println!("    {} {} | {}", r.tier, r.code, r.detail); }
        for s in &j.view.steps { if let Some(a) = &s.analysis { for c in &a.commands { println!("      step {:?}: {} {:?} {:?} {:?} | {}", s.name, c.kind, c.runner, c.shape, c.script, c.text); } } }
    }
}

#[test]
#[ignore]
fn show_yaml() {
    let file = std::env::var("KERN_YAML_FILE").unwrap_or_default();
    match kern::yaml::read(&std::fs::read(&file).unwrap()) { Ok(v) => println!("{}", serde_json::to_string(&v).unwrap()), Err(e) => println!("ERR {}", e.0) }
}

#[test]
fn every_a_or_b_base_commit_gives_a_typed_plan_from_its_ci_job_and_every_other_tier_says_why_not() {
    let Some(dir) = fixture() else { return };
    let pairs: Value = serde_json::from_slice(&std::fs::read(dir.join("pairs.json")).unwrap()).unwrap();
    let (mut plans, mut unreadable, mut wrong) = (0, Vec::new(), Vec::new());
    for pair in pairs["pairs"].as_array().unwrap() {
        let (repo, sha) = (pair["repo"].as_str().unwrap(), pair["base_sha"].as_str().unwrap());
        let root = dir.join(repo.replace('/', "__")).join(sha);
        let read = |path: &str| std::fs::read(root.join(path)).ok();
        let tree = tree_of(&root, repo, &read);
        let tier = pair["tier"].as_str().unwrap().chars().next().unwrap();
        match kern::plan::read(&tree) {
            Ok(p) => {
                plans += 1;
                let want = pair["best_job"].as_str().unwrap().to_string();
                let got = match &p.source { kern::plan::Source::Ci { file, job } => format!("{file}:{job}"), _ => "?".into() };
                if got != want || !"AB".contains(tier) { wrong.push(format!("{repo}@{}: plan from {got}, oracle {want} tier {tier}", &sha[..8])); }
                assert!(p.tests().count() >= 1, "{repo}: a plan has a test");
                let again = kern::plan::parse(&kern::plan::json(&p)).unwrap();
                assert_eq!(kern::plan::hash(&again), kern::plan::hash(&p));
                println!("{repo}@{}: node {} {}@{:?} {} steps: {}", &sha[..8], p.toolchain.node, p.toolchain.manager.name, p.toolchain.manager.version, p.steps.len(), p.steps.iter().map(|s| format!("[{} {}]", s.kind.name(), s.line)).collect::<Vec<_>>().join(" "));
            }
            Err(u) => {
                if u.tier != tier { wrong.push(format!("{repo}@{}: unreadable with tier {} but the oracle says {tier}: {}", &sha[..8], u.tier, u.why)); }
                if "AB".contains(tier) { unreadable.push(format!("{repo}@{} ({tier}): {}", &sha[..8], u.why)); }
            }
        }
    }
    println!("plans: {plans} of 70; A/B commits without a plan: {}\n{}", unreadable.len(), unreadable.join("\n"));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(plans >= 30, "at least 30 of the 42 A/B commits give a plan: {plans}");
}
