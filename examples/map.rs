use kern::map;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --release --example map -- <facts.jsonl> <out.json> [changed-files-list]\nReads the fact file `node src/map.cjs --compact <index.scip> <facts.jsonl>` makes from scip-typescript's index, prints what it holds, and with a list of changed files the symbol-level reach of a change: the functions defined in those files, every function that runs them, and so on, with the files that run the change nearest first.");
        std::process::exit(2);
    }
    let bytes = match std::fs::read(&args[0]) { Ok(b) => b, Err(e) => { eprintln!("{e}"); std::process::exit(1) } };
    let m = match map::read(&bytes) { Ok(m) => m, Err(e) => { eprintln!("{}", e.0); std::process::exit(1) } };
    let mut out = json!({ "summary": map::summary(&m) });
    println!("{}", out["summary"]);
    if let Some(list) = args.get(2) {
        let changed: Vec<(String, Vec<usize>)> = std::fs::read_to_string(list).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| (l.to_string(), Vec::new())).collect();
        let r = map::reach(&m, &changed, &|_, _| None);
        let paths: Vec<String> = changed.iter().map(|c| c.0.clone()).collect();
        let near = map::neighbours(&m, &r, &paths, 40);
        let reach = map::reach_json(&m, &r, &changed);
        println!("{}", serde_json::to_string_pretty(&json!({ "changed_functions": reach["changed_functions"], "changed_types": reach["changed_types"], "symbols_reached": reach["symbols_reached"], "files_running": reach["files_running"], "files_mentioning": reach["files_mentioning"], "test_files": reach["test_files"], "farthest_hops": reach["farthest_hops"], "complete": reach["complete"], "nearest": near.iter().take(15).collect::<Vec<_>>() })).unwrap_or_default());
        out["reach"] = reach;
        out["neighbours"] = json!(near);
    }
    if let Err(e) = std::fs::write(&args[1], serde_json::to_string_pretty(&out).unwrap_or_default()) { eprintln!("{e}"); std::process::exit(1) }
}
