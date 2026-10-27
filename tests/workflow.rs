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
