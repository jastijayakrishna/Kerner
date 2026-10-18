use crate::suite::{Evidence, Observed};
use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};

pub const CONTRACTS_COUNT_MAX: usize = 4;
pub const INPUTS_COUNT_MAX: usize = 8;
pub const ARM_SECS_MAX: u64 = 1800;
pub const PROBE_SECS_MAX: u64 = 120;
pub const JSON_BYTES_MAX: usize = 1 << 20;
pub const PROBE_OUTPUT_BYTES_MAX: usize = 64 << 10;
const INPUT_BYTES_MAX: usize = 4096;

pub fn pack(v: &Value) -> R<()> {
    exact(v, &["version", "repository", "number", "base", "head", "approved_by", "contracts"], "validation pack")?;
    ensure!(v["version"] == 1 && crate::github::repository(text(v, "repository")?), "Invalid validation repository or version");
    ensure!(v["number"].as_u64().is_some_and(|n| n > 0) && !text(v, "approved_by")?.is_empty(), "The validation pack needs a PR and independent approval");
    for side in ["base", "head"] { ensure!(is_oid(text(v, side)?), "The validation pack needs exact revisions"); }
    let contracts = v["contracts"].as_array().ok_or_else(|| Denied("Missing contracts".into()))?;
    ensure!(!contracts.is_empty() && contracts.len() <= CONTRACTS_COUNT_MAX, "Expected 1..4 contracts");
    let mut ids = std::collections::BTreeSet::new();
    for c in contracts {
        exact(c, &["id", "kind", "source", "source_sha256", "input_domain", "command", "oracle", "baseline_inputs"], "validation contract")?;
        let id = text(c, "id")?;
        ensure!(crate::policy::task_id(id) && ids.insert(id), "Duplicate or invalid contract ID");
        ensure!(matches!(text(c, "kind")?, "build" | "api" | "security"), "Unsupported contract kind");
        file_path(text(c, "source")?)?;
        ensure!(is_hex64(text(c, "source_sha256")?), "A contract needs pinned pre-change evidence");
        let domain = &c["input_domain"];
        let kind = text(domain, "type")?;
        exact(domain, if kind == "string-prefix" { &["description", "type", "prefix"] } else { &["description", "type"] }, "input domain")?;
        ensure!(!text(domain, "description")?.is_empty() && matches!(kind, "null" | "string-prefix" | "json"), "Invalid input domain");
        if kind == "string-prefix" { ensure!(!text(domain, "prefix")?.is_empty(), "A prefix domain needs a nonempty prefix"); }
        let command = argv(&c["command"])?;
        ensure!(command.len() <= 16 && command.iter().all(|s| s.len() <= 16384), "The probe command exceeds its bounds");
        exact(&c["oracle"], &["mode", "expected"], "validation oracle")?;
        ensure!(matches!(text(&c["oracle"], "mode")?, "exit-zero" | "json-equal"), "Unsupported oracle");
        inputs(&c["baseline_inputs"])?;
        ensure!(c["baseline_inputs"].as_array().is_some_and(|a| a.iter().all(|i| in_domain(c, i))), "A baseline input is outside its approved domain");
    }
    Ok(())
}
fn inputs(v: &Value) -> R<()> {
    let a = v.as_array().ok_or_else(|| Denied("Inputs must be an array".into()))?;
    ensure!(!a.is_empty() && a.len() <= INPUTS_COUNT_MAX && a.iter().all(|v| v.to_string().len() <= INPUT_BYTES_MAX), "Inputs exceed the fixed budget");
    Ok(())
}
fn in_domain(c: &Value, input: &Value) -> bool {
    match c["input_domain"]["type"].as_str() {
        Some("null") => input.is_null(),
        Some("string-prefix") => input.as_str().zip(c["input_domain"]["prefix"].as_str()).is_some_and(|(i,p)| i.starts_with(p)),
        Some("json") => true,
        _ => false,
    }
}
pub fn proposals(p: &Value, v: &Value) -> R<()> {
    pack(p)?;
    exact(v, &["version", "pack_sha256", "model", "prompt_sha256", "input_tokens", "output_tokens", "cost_usd", "proposals"], "hunter response")?;
    ensure!(v["version"] == 1 && v["pack_sha256"] == digest(p)?.as_str(), "The hunter response names another contract pack");
    ensure!(!text(v, "model")?.is_empty() && is_hex64(text(v, "prompt_sha256")?), "The hunter response needs model and prompt provenance");
    for k in ["input_tokens", "output_tokens"] { ensure!(v[k].is_null() || v[k].is_u64(), "Invalid model usage"); }
    ensure!(v["cost_usd"].is_null() || v["cost_usd"].as_f64().is_some_and(|n| n.is_finite() && n >= 0.0), "Invalid model cost");
    let a = v["proposals"].as_array().ok_or_else(|| Denied("Missing proposals".into()))?;
    ensure!(a.len() <= CONTRACTS_COUNT_MAX, "Too many proposal groups");
    let mut seen = std::collections::BTreeSet::new();
    for x in a {
        exact(x, &["contract", "inputs"], "hunter proposal")?;
        let id = text(x, "contract")?;
        ensure!(seen.insert(id), "Duplicate contract");
        let c = p["contracts"].as_array().and_then(|a| a.iter().find(|c| c["id"] == id)).ok_or_else(|| Denied("Unknown contract".into()))?;
        inputs(&x["inputs"])?;
        ensure!(x["inputs"].as_array().is_some_and(|a| a.iter().all(|i| in_domain(c, i))), "A proposed input is outside its approved domain");
    }
    Ok(())
}
pub fn observed(oracle: &Value, ending: &str, output: &[u8], errors: &[u8]) -> Observed {
    if output.len() > PROBE_OUTPUT_BYTES_MAX || errors.len() > PROBE_OUTPUT_BYTES_MAX { return Observed::Missing; }
    let code = ending.strip_prefix("exit ").and_then(|s| s.parse::<u8>().ok());
    if code.is_none_or(|c| ending != format!("exit {c}") || c >= 128) || crate::suite::sandbox_error(&String::from_utf8_lossy(errors)) { return Observed::Missing; }
    match oracle["mode"].as_str() {
        Some("exit-zero") => if ending == "exit 0" { Observed::Passed } else { Observed::Failed },
        Some("json-equal") if ending == "exit 0" => match serde_json::from_slice::<Value>(output) {
            Ok(v) if v == oracle["expected"] => Observed::Passed,
            Ok(_) => Observed::Failed,
            Err(_) => Observed::Missing,
        },
        _ => Observed::Missing,
    }
}
pub fn judged(old: [Observed; 2], new: [Observed; 2], uneven: bool) -> Value {
    let result = crate::suite::blockable(Evidence { old: old[0], old_again: Some(old[1]), new: new[0], new_again: [Some(new[1]), None], shaky: false, sandbox: false, uneven });
    json!({"confirmed":result.is_ok(), "why":result.err().map(|u| u.list()), "old":old.map(|o| format!("{o:?}")), "new":new.map(|o| format!("{o:?}")), "uneven":uneven})
}
