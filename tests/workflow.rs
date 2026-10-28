fn job(text: &str, name: &str) -> String {
    let mut lines = text.lines().skip_while(|l| *l != format!("  {name}:")).skip(1);
    let mut out = String::new();
    for l in lines.by_ref() {
        let next_job = l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':');
        if next_job { break; }
        out.push_str(l);
        out.push('\n');
    }
    assert!(!out.is_empty(), "no job {name}");
    out
}
fn permissions(job: &str) -> String { job.lines().skip_while(|l| l.trim() != "permissions:").skip(1).take_while(|l| l.starts_with("      ")).collect::<Vec<_>>().join("\n") }

#[test]
fn review_2_c_the_reusable_workflow_keeps_its_trust_boundaries() {
    let text = std::fs::read_to_string(".github/workflows/kern.yml").unwrap();
    let build = job(&text, "build");
    assert!(!build.contains("actions/cache") && !build.contains("pull_request.head"), "the job that builds the Kern the signer trusts never uses a cache or the pull request's code:\n{build}");
    let run = job(&text, "run");
    assert!(!permissions(&run).contains("id-token"), "the job that runs the pull request's code can never sign:\n{}", permissions(&run));
    assert!(!run.contains("secrets."), "the job that runs the pull request's code holds no keys: an AI agent there reads the pull request's text and could be told to copy a key into what Kern posts:\n{run}");
    let sign = job(&text, "sign");
    assert!(!sign.contains("actions/checkout") && !sign.contains("actions/cache"), "the signing job never loads anything the run job could have touched:\n{sign}");
    assert!(sign.contains("sha256sum") && sign.contains("needs.build.outputs.sha256"), "the signing job checks the Kern binary is the one the build job made");
    assert!(sign.contains("id: sign"), "the Sign step has an id so a failure can be seen");
    let say = sign.split("- name: Say so if Kern could not finish").nth(1).unwrap_or("");
    assert!(say.lines().next().is_some() && say.contains("steps.sign.outcome == 'failure'"),
        "review 2 A12: when signing fails the Post step is skipped, and the status stayed pending forever; it must say Kern could not finish");
}

#[test]
fn lightdash_12942_the_ai_step_holds_the_key_in_a_job_that_never_runs_the_pull_requests_code() {
    let text = std::fs::read_to_string(".github/workflows/kern.yml").unwrap();
    let investigate = job(&text, "investigate");
    assert_eq!(permissions(&investigate).trim(), "contents: read", "the AI step can only read the repository");
    assert!(investigate.contains("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}"), "the AI step is the one job that holds the key");
    assert!(investigate.contains("kern-trusted-") && investigate.contains("sha256sum") && investigate.contains("needs.build.outputs.sha256"),
        "the job holding the key runs only the Kern the build job made, checked by its hash:\n{investigate}");
    for runs in ["npm ", "npx ", "yarn ", "pnpm ", "node ", "docker", "--control", "actions/setup-node", "persist-credentials: true"] {
        assert!(!investigate.contains(runs), "the job holding the key reads the pull request and never runs it ({runs}):\n{investigate}");
    }
    assert!(!investigate.contains("actions/cache@") && investigate.contains("actions/cache/restore@") && investigate.contains("actions/cache/save@"),
        "only the AI's answers are cached, and Kern checks them again like a live answer; never a Kern build");
    let plan = job(&text, "plan");
    assert!(plan.contains("secrets.ANTHROPIC_API_KEY != ''") && !plan.contains("ANTHROPIC_API_KEY: ${{"), "the plan learns only whether a key exists, never the key");
    let run = job(&text, "run");
    assert!(run.contains("KERN_INVESTIGATION") && !run.contains("secrets."), "the run job reads the investigation as data and still holds no keys");
    let jobs = kern::yaml::read(text.as_bytes()).unwrap();
    let keyed: Vec<&String> = jobs["jobs"].as_object().unwrap().iter().filter(|(name, _)| job(&text, name).contains("secrets.ANTHROPIC_API_KEY }}")).map(|(name, _)| name).collect();
    assert_eq!(keyed, ["investigate", "learn"], "the key is held by the job that reads the pull request and the job that runs only the default branch's own code, never by one that runs a pull request");
    assert!(!permissions(&run).contains("write"), "the run job's token stays read-only:\n{}", permissions(&run));
}

#[test]
fn the_readme_install_line_uses_the_ref_examples_kern_yml_uses() {
    let uses = |text: &str| text.lines().map(str::trim).filter(|l| l.starts_with("uses: jastijayakrishna/Kerner/")).map(str::to_string).collect::<Vec<_>>();
    let example = uses(&std::fs::read_to_string("examples/kern.yml").unwrap());
    assert_eq!(example.len(), 1, "examples/kern.yml calls Kern's reusable workflow once");
    let readme = uses(&std::fs::read_to_string("README.md").unwrap());
    assert!(!readme.is_empty(), "the README shows the install workflow");
    for line in &readme { assert_eq!(line, &example[0], "a customer who copies the README must get the workflow examples/kern.yml points at, at a ref that exists"); }
}

fn step(job: &str, name: &str) -> String {
    let mut out = String::new();
    for l in job.lines().skip_while(|l| l.trim() != format!("- name: {name}")).skip(1) {
        if l.trim_start().starts_with("- name: ") { break; }
        out.push_str(l);
        out.push('\n');
    }
    assert!(!out.is_empty(), "no step {name}");
    out
}
fn condition<'a>(jobs: &'a serde_json::Value, name: &str) -> &'a str { jobs[name]["if"].as_str().unwrap_or_else(|| panic!("job {name} has no if")) }

#[test]
fn kern_check_demo_the_learn_job_writes_the_plan_on_main_and_the_run_job_restores_it_for_the_pull_requests_base() {
    let text = std::fs::read_to_string(".github/workflows/kern.yml").unwrap();
    let read = |path: &str| kern::yaml::read(&std::fs::read(path).unwrap()).unwrap_or_else(|e| panic!("{path} is not YAML Kern's reader follows: {}", e.0));
    let (workflow, example, action) = (read(".github/workflows/kern.yml"), read("examples/kern.yml"), read(".github/actions/sandbox/action.yml"));
    let jobs = &workflow["jobs"];
    let learn_if = condition(jobs, "learn");
    assert!(learn_if.contains("github.event_name == 'push'") && learn_if.contains("github.event.repository.default_branch") && !learn_if.contains("pull_request"), "learn runs only on a push to the default branch: {learn_if}");
    assert!(condition(jobs, "plan").contains("github.event_name == 'push'"), "the plan job names which Kern on a push too");
    for name in ["run", "investigate"] { assert!(condition(jobs, name).contains("github.event_name == 'pull_request'"), "{name} never runs on a push"); }
    assert!(condition(jobs, "sign").contains("github.event_name == 'pull_request'") && condition(jobs, "sign").starts_with("always()"), "sign never runs on a push");
    assert_eq!(jobs["learn"]["continue-on-error"], true, "a plan that cannot be learned never fails the customer's push");
    assert_eq!(jobs["learn"]["permissions"], serde_json::json!({ "contents": "read" }));
    assert_eq!(jobs["learn"]["needs"], serde_json::json!(["plan", "build"]));
    let learn = job(&text, "learn");
    assert!(learn.contains("kern-trusted-") && learn.contains("sha256sum") && learn.contains("needs.build.outputs.sha256"), "learn runs only the Kern the build job made, checked by its hash:\n{learn}");
    assert!(!learn.contains("pull_request") && !learn.contains("actions/cache@"), "learn reads no pull request and uses no plain cache");
    let keyed: Vec<&str> = learn.split("- name: ").filter(|s| s.contains("secrets.")).collect();
    assert_eq!(keyed.len(), 1, "exactly one step of learn holds the key, the one that runs kern learn:\n{learn}");
    assert!(keyed[0].contains("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}") && keyed[0].contains("\" learn \"") && keyed[0].contains("--control"), "{}", keyed[0]);
    assert!(keyed[0].contains("KERN_PLANS: ${{ runner.temp }}/plans"), "learn keeps a plan the setup key still matches instead of learning it again");
    let save = step(&learn, "Keep the plan, so every pull request on this commit is checked through it");
    assert!(save.contains("actions/cache/save@") && save.contains("path: ${{ runner.temp }}/plans") && save.contains("key: kern-plan-v1-${{ needs.plan.outputs.kern }}-${{ github.sha }}"), "{save}");
    assert!(save.contains("if: steps.learn.outputs.learned == 'true'"), "nothing is saved when learn wrote no plan file");
    let run = job(&text, "run");
    let restore = step(&run, "The plan Kern learned for this base, if its branch was learned");
    assert!(restore.contains("actions/cache/restore@") && restore.contains("path: ${{ runner.temp }}/plans") && restore.contains("key: kern-plan-v1-${{ needs.plan.outputs.kern }}-${{ github.event.pull_request.base.sha }}"), "{restore}");
    assert!(!restore.contains("restore-keys"), "a plan learned for another base is not this base's");
    let check = step(&run, "Check");
    assert!(check.contains("KERN_PLANS: ${{ steps.plans.outputs.cache-hit == 'true' && format('{0}/plans', runner.temp) || '' }}"), "the run job reads the plan only when the exact key hit:\n{check}");
    for name in ["run", "learn"] { assert!(job(&text, name).contains("uses: ./.kern/.github/actions/sandbox"), "{name} sets its sandbox up through the one composite action"); }
    assert_eq!(action["runs"]["using"], "composite");
    assert!(action["runs"]["steps"].as_array().unwrap().iter().all(|s| s["uses"].is_null() && s["shell"] == "bash"), "the sandbox action runs plain bash steps only");
    assert!(example["jobs"]["kern"]["if"].as_str().unwrap().contains("github.event_name == 'push'"), "the customer's file passes the push through");
    let on = example.as_object().unwrap().values().find(|v| v["push"].is_object()).unwrap_or_else(|| panic!("the customer's file has no push trigger: {example}"));
    assert_eq!(on["push"], serde_json::json!({ "branches": ["main"] }));
    let readme = std::fs::read_to_string("README.md").unwrap();
    assert!(readme.contains("  push:\n    branches: [main]\n") && readme.contains("github.event_name == 'push'"), "the README's copy of the file carries the push trigger");
    assert!(readme.contains("kern learn"), "the README says what learn does on main");
}
