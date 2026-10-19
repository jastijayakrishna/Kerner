use kern::house;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: cargo run --release --example house -- <repo> <commit> <out.json> [changed-files-list]\nBuilds the house of a repository at a commit: every product file, its exported functions, the import edges between them and the rooms (package entries, commands, routes); with a list of changed files, the reach of a change through the house.");
        std::process::exit(2);
    }
    let h = match house::build(Path::new(&args[0]), &args[1]) { Ok(x) => x, Err(e) => { eprintln!("{}", e.0); std::process::exit(1) } };
    let mut out = house::summary(&h);
    out["commit"] = json!(args[1]);
    out["graph"] = json!(h.exports.iter().map(|(p, e)| (p.clone(), json!({ "exports": e, "imports": h.imports.get(p), "room": h.rooms.get(p) }))).collect::<BTreeMap<_, _>>());
    println!("{} files, {} product files, {} exported functions, {} import edges, {} rooms, unresolved {:?}", h.files, out["product_files"], out["exported_functions"], out["import_edges"], h.rooms.len(), h.unresolved);
    if let Some(list) = args.get(3) {
        let changed: Vec<String> = std::fs::read_to_string(list).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
        let r = house::reach(&h, &changed).json(&changed, &h);
        println!("{}", serde_json::to_string_pretty(&json!({ "changed": r["changed"], "affected_files": r["affected_files"], "farthest_hops": r["farthest_hops"], "rooms_reached": r["rooms_reached"], "rooms": r["rooms"] })).unwrap_or_default());
        out["reach"] = r;
    }
    if let Err(e) = std::fs::write(&args[2], serde_json::to_string_pretty(&out).unwrap_or_default()) { eprintln!("{e}"); std::process::exit(1) }
}
