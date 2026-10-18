use kern::suite::Observed::{self, Failed, Missing, Passed};
use kern::util::digest;
use kern::validation as v;
use serde_json::{Value, json};

fn pack() -> Value {
    json!({"version":1,"repository":"example/project","number":1,"base":"1".repeat(40),"head":"2".repeat(40),"approved_by":"independent-contract-review",
        "contracts":[{"id":"denies-parent-paths","kind":"security","source":"SECURITY.md","source_sha256":"a".repeat(64),"input_domain":{"description":"Paths beginning with ../ must return false","type":"string-prefix","prefix":"../"},
            "command":["/usr/local/bin/node","-e","process.stdout.write('false')"],"oracle":{"mode":"json-equal","expected":false},"baseline_inputs":["../secret"]}]})
}
fn proposals(p: &Value) -> Value {
    json!({"version":1,"pack_sha256":digest(p).unwrap(),"model":"fixed-model-v1","prompt_sha256":"b".repeat(64),"input_tokens":null,"output_tokens":null,"cost_usd":null,
        "proposals":[{"contract":"denies-parent-paths","inputs":["../../secret"]}]})
}

#[test]
fn hunter_inputs_cannot_replace_oracles_commands_revisions_or_budgets() {
    let p = pack();
    assert!(v::proposals(&p, &proposals(&p)).is_ok());
    for key in ["oracle", "expected", "command", "head", "budget_seconds"] {
        let mut r = proposals(&p);
        r["proposals"][0][key] = json!("attacker-controlled");
        assert!(v::proposals(&p, &r).is_err(), "accepted {key}");
    }
    let mut r = proposals(&p);
    r["pack_sha256"] = json!("c".repeat(64));
    assert!(v::proposals(&p, &r).is_err());
    r = proposals(&p);
    r["proposals"][0]["contract"] = json!("invented-oracle");
    assert!(v::proposals(&p, &r).is_err());
}

#[test]
fn missing_approval_provenance_and_over_budget_proposals_are_rejected() {
    let p = pack();
    for key in ["approved_by", "contracts", "base", "head"] {
        let mut broken = p.clone();
        broken.as_object_mut().unwrap().remove(key);
        assert!(v::pack(&broken).is_err(), "accepted missing {key}");
    }
    let mut r = proposals(&p);
    r["proposals"][0]["inputs"] = json!(vec![json!("../secret"); 9]);
    assert!(v::proposals(&p, &r).is_err());
    r["proposals"][0]["inputs"] = json!(["x".repeat(4097)]);
    assert!(v::proposals(&p, &r).is_err());
    r = proposals(&p);
    r["cost_usd"] = json!(-1);
    assert!(v::proposals(&p, &r).is_err());
}

#[test]
fn execution_failures_and_unreadable_outputs_are_missing_evidence() {
    let oracle = json!({"mode":"json-equal","expected":false});
    for (ending, output, errors, want) in [
        ("exit 0", "false", "", Passed), ("exit 0", "true", "", Failed),
        ("timeout", "true", "", Missing), ("exit -1", "true", "", Missing),
        ("exit 0", "truncated", "", Missing), ("exit 1", "", "", Missing),
        ("exit 0", "true", "JavaScript heap out of memory", Missing),
    ] { assert_eq!(v::observed(&oracle, ending, output.as_bytes(), errors.as_bytes()), want); }
    let build = json!({"mode":"exit-zero","expected":null});
    for ending in ["exit 137", "exit 139", "exit 134", "exit 143", "timeout", "exit banana", "exit 1 truncated", "exit 256", "exit +1"] {
        assert_eq!(v::observed(&build, ending, b"", b""), Missing);
    }
    assert_eq!(v::observed(&build, "exit 1", b"", b"compiler error"), Failed);
    for code in 128..=255 {
        assert_eq!(v::observed(&build, &format!("exit {code}"), b"", b""), Missing, "Signal-style or ambiguous high exit codes are not build regressions");
    }
    assert_eq!(v::observed(&build, "exit 0", &vec![b'x'; (64 << 10) + 1], b""), Missing);
}

#[test]
fn experimental_confirmation_reuses_the_production_proof_boundary() {
    let all = [Passed, Failed, Missing, Observed::Skipped];
    for a in all { for b in all { for c in all { for d in all { for uneven in [false,true] {
        let r = v::judged([a,b], [c,d], uneven);
        assert_eq!(r["confirmed"], a == Passed && b == Passed && c == Failed && d == Failed && !uneven, "{r}");
    }}}}}
}

#[test]
fn no_proposals_and_unknown_cost_are_recorded_honestly() {
    let p = pack();
    let mut r = proposals(&p);
    r["proposals"] = json!([]);
    assert!(v::proposals(&p, &r).is_ok());
    assert!(r["cost_usd"].is_null());
}

#[test]
fn the_hunter_cannot_widen_the_approved_input_domain() {
    let p = pack();
    let mut r = proposals(&p);
    r["proposals"][0]["inputs"] = json!(["ordinary-public-file"]);
    assert!(v::proposals(&p, &r).is_err(), "An ordinary path is outside the approved parent-path policy");
}

#[test]
fn frozen_pilot_contracts_pass_the_workers_actual_schema() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("experiments/counterexample/packs");
    let mut count = 0;
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let p: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        v::pack(&p).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        count += 1;
    }
    assert_eq!(count, 5);
}
