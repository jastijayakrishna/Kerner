use kern::{ensure, validation as v};
use kern::suite::Observed;
use kern::util::*;
use serde_json::{Value, json};
use std::path::Path;

fn read(path: &Path) -> R<Value> {
    ensure!(std::fs::metadata(path)?.len() <= v::JSON_BYTES_MAX as u64, "Validation input exceeds one MiB");
    read_json(path)
}
fn probe(out: &Path, volume: &str, c: &Value, input: &Value, until: u64) -> R<(Observed, Value)> {
    let seconds = within(until, v::PROBE_SECS_MAX);
    ensure!(seconds > 0, "The arm exhausted its fixed budget");
    let mut command = argv(&c["command"])?;
    if c["oracle"]["mode"] == "json-equal" { command.push(input.to_string()); }
    let spec = json!({"version":1,"command":command,"timeout_seconds":seconds,"cwd":"","env":{},"files":{},"early":false,"faults":[],"serve":{}});
    let start = std::time::Instant::now();
    let run = kern::suite::in_test_box(out, volume, "probe", ("drive-test.json", &spec), ("__drive_test", false, seconds))?;
    let parts = kern::sandbox::parts(&run.stdout, &kern::sandbox::TEST_PARTS)?;
    let ending = String::from_utf8_lossy(&parts["ending"]);
    let observed = v::observed(&c["oracle"], &ending, &parts["output"], &parts["errors"]);
    let stem = format!("probe-{}", sha256(json!([volume,c["id"],input]).to_string()));
    for (key, suffix) in [("output", "out"), ("errors", "err")] {
        let bytes = &parts[key];
        std::fs::write(out.join(format!("{stem}.{suffix}")), &bytes[..bytes.len().min(v::PROBE_OUTPUT_BYTES_MAX)])?;
    }
    let preview = |key: &str| String::from_utf8_lossy(&parts[key][..parts[key].len().min(4096)]).to_string();
    Ok((observed, json!({"observed":format!("{observed:?}"),"ending":ending,"seconds":start.elapsed().as_secs_f64(),
        "output":preview("output"),"errors":preview("errors"),"artifact":stem,"output_sha256":sha256(&parts["output"]),"errors_sha256":sha256(&parts["errors"]),
        "output_bytes":parts["output"].len(),"errors_bytes":parts["errors"].len(),"truncated":parts["output"].len() > v::PROBE_OUTPUT_BYTES_MAX || parts["errors"].len() > v::PROBE_OUTPUT_BYTES_MAX})))
}
fn candidates(pack: &Value, proposals: &Value, hunter: bool) -> Vec<(Value, Value)> {
    pack["contracts"].as_array().into_iter().flatten().flat_map(|c| {
        let baseline = c["baseline_inputs"].as_array().into_iter().flatten();
        let extra: Vec<&Value> = if hunter { proposals["proposals"].as_array().into_iter().flatten().filter(|p| p["contract"] == c["id"]).flat_map(|p| p["inputs"].as_array().into_iter().flatten()).collect() } else { vec![] };
        let mut seen = std::collections::BTreeSet::new();
        baseline.chain(extra).filter(|i| seen.insert(i.to_string())).map(|i| (c.clone(), i.clone())).collect::<Vec<_>>()
    }).collect()
}
fn arm(repo: &Path, pack: &Value, proposals: &Value, op: &kern::policy::Operator, root: &Path, hunter: bool, core_seconds: u64) -> R<Value> {
    let start = std::time::Instant::now();
    let until = now_ms() + v::ARM_SECS_MAX.saturating_sub(core_seconds) * 1000;
    let cases = candidates(pack, proposals, hunter);
    let mut seen = vec![[Observed::Missing; 4]; cases.len()];
    let (mut observations, mut errors, mut uneven) = (Vec::new(), Vec::new(), false);
    for repeat in 0..2 {
        let out = root.join(format!("repeat-{repeat}"));
        let attempt = (|| -> R<()> {
            kern::drive::prepare(repo, text(pack, "base")?, text(pack, "head")?, &out, None)?;
            let setup = kern::drive::setup(&out, op, until)?;
            if let Some(why) = kern::suite::setup_differs(&setup, "").or_else(|| kern::suite::unlocked(&setup, "")) { uneven = true; errors.push(why); }
            for (side_index, side) in ["old", "new"].iter().enumerate() {
                for (i, (c, input)) in cases.iter().enumerate() {
                    let result = probe(&out, text(&setup[*side], "volume")?, c, input, until);
                    let mut row = json!({"case":i,"contract":c["id"],"input":input,"side":side,"repeat":repeat});
                    match result {
                        Ok((o, data)) => { seen[i][side_index * 2 + repeat] = o; row["run"] = data; }
                        Err(e) => { row["unsupported"] = json!(e.0); }
                    }
                    observations.push(row);
                }
            }
            Ok(())
        })();
        if let Err(e) = attempt { errors.push(e.0); }
        if out.join("volumes.txt").exists() && let Err(e) = kern::drive::clean(&out) { errors.push(format!("cleanup: {}", e.0)); }
    }
    let seconds = start.elapsed().as_secs_f64() + core_seconds as f64;
    let within_budget = seconds <= v::ARM_SECS_MAX as f64;
    if !within_budget { errors.push("The arm exceeded its budget; none of its findings count as confirmed".into()); }
    let judgments: Vec<Value> = cases.iter().zip(seen).enumerate().map(|(i, ((c, input), s))| {
        let mut j = v::judged([s[0], s[1]], [s[2], s[3]], uneven || !within_budget);
        j["case"] = json!(i); j["contract"] = c["id"].clone(); j["input"] = input.clone(); j
    }).collect();
    Ok(json!({"arm":if hunter {"hunter"} else {"baseline"},"seconds":seconds,"shared_core_seconds":core_seconds,"within_budget":within_budget,"budget_seconds":v::ARM_SECS_MAX,
        "judgments":judgments,"observations":observations,"errors":errors}))
}
fn run() -> R<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("--doctor") => {
            ensure!(a.len() == 2, "Expected the control JSON");
            kern::policy::configure(&read(Path::new(&a[1]))?)?;
            let doctor = kern::sandbox::doctor();
            say(&doctor.report.to_string());
            ensure!(doctor.ready, "The CI sandbox is not ready");
            return Ok(());
        }
        Some("__drive_setup") => return kern::sandbox::drive_setup(),
        Some("__drive_test") => return kern::sandbox::drive_test(),
        Some("__drive_seal") => return kern::sandbox::drive_seal(),
        Some("__reap") => {
            ensure!(a.len() == 3, "Expected a container controller and name");
            return kern::container::reap(Path::new(&a[1]), &a[2]);
        }
        _ => {}
    }
    ensure!(a.len() == 6, "Expected pack.json proposals.json operator.json control.json source-git output-directory");
    ensure!(cfg!(target_os = "linux") && std::env::var("CI").as_deref() == Ok("true"), "Run hostile repository code only in the Linux CI sandbox");
    let (pack, proposals) = (read(Path::new(&a[0]))?, read(Path::new(&a[1]))?);
    v::proposals(&pack, &proposals)?;
    let op = kern::policy::operator(&read(Path::new(&a[2]))?)?;
    ensure!(pack["repository"] == op.repository, "The operator names another repository");
    kern::policy::configure(&read(Path::new(&a[3]))?)?;
    let repo = Path::new(&a[4]);
    for c in pack["contracts"].as_array().into_iter().flatten() {
        let blob = kern::git::git(repo, &["show", &format!("{}:{}", text(&pack, "base")?, text(c, "source")?)], None)?;
        ensure!(sha256(blob) == text(c, "source_sha256")?, "Pre-change contract evidence does not match its pin");
    }
    let out = fresh_dir(&a[5])?;
    ensure!(kern::check::minutes() == 10, "Set KERN_CHECK_MINUTES=10 for the frozen core budget");
    let core_started = std::time::Instant::now();
    let core_dir = out.join("core");
    let core = match kern::check::check(repo, text(&pack, "base")?, text(&pack, "head")?, &core_dir, &op, None, None) {
        Ok(_) => read_json(core_dir.join("receipt.json"))?,
        Err(e) => json!({"unsupported":e.0}),
    };
    let core_seconds = core_started.elapsed().as_millis().div_ceil(1000) as u64;
    let mut results = Vec::new();
    let order = if text(&pack, "head")?.as_bytes()[0] % 2 == 0 { [false, true] } else { [true, false] };
    for hunter in order {
        let root = out.join(if hunter {"hunter"} else {"baseline"});
        std::fs::create_dir(&root)?;
        let result = arm(repo, &pack, &proposals, &op, &root, hunter, core_seconds)?;
        std::fs::write(root.join("result.json"), serde_json::to_vec_pretty(&result)?)?;
        results.push(result);
    }
    let report = json!({"version":1,"repository":pack["repository"],"number":pack["number"],"base":pack["base"],"head":pack["head"],
        "pack_sha256":digest(&pack)?,"proposals_sha256":sha256(serde_json::to_vec(&proposals)?),"model":proposals["model"],"cost_usd":proposals["cost_usd"],
        "input_tokens":proposals["input_tokens"],"output_tokens":proposals["output_tokens"],"core":core,"results":results});
    std::fs::write(out.join("result.json"), serde_json::to_vec_pretty(&report)?)?;
    say(&out.join("result.json").display().to_string());
    Ok(())
}
fn main() { if let Err(e) = run() { eprintln!("{e}"); std::process::exit(1); } }
