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
