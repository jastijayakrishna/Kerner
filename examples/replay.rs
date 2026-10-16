use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: cargo run --release --example replay -- <out dir> <farm results dir>...\nJudges every stored check again from its own test runs with this build's rules and writes <out dir>/<farm>/receipt-*/receipt.json");
        std::process::exit(2);
    }
    let out = PathBuf::from(&args[0]);
    let mut jobs: Vec<(PathBuf, PathBuf)> = Vec::new();
    for farm in &args[1..] {
        let name = Path::new(farm).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(farm).into_iter().flatten().flatten().map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("receipt-")) && p.join("receipt.json").is_file()).collect();
        dirs.sort();
        jobs.extend(dirs.into_iter().map(|d| { let dest = out.join(&name).join(d.file_name().unwrap_or_default()); (d, dest) }));
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
    let counts: Vec<(u64, u64, Vec<String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs.chunks(jobs.len().div_ceil(threads).max(1)).map(|chunk| s.spawn(move || {
            let (mut done, mut replayed, mut failed) = (0u64, 0u64, Vec::new());
            for (from, to) in chunk {
                match kern::replay::rejudge(from) {
                    Ok(v) => {
                        let written = std::fs::create_dir_all(to).and_then(|_| std::fs::write(to.join("receipt.json"), serde_json::to_vec_pretty(&v["receipt"]).unwrap_or_default()))
                            .and_then(|_| std::fs::write(to.join("replay.json"), serde_json::json!({ "replayed": v["replayed"], "suites": v["suites"], "from": from }).to_string()));
                        if let Err(e) = written { failed.push(format!("{}: {e}", from.display())); continue; }
                        done += 1;
                        if v["replayed"].as_u64().unwrap_or(0) > 0 { replayed += 1; }
                    }
                    Err(e) => failed.push(format!("{}: {}", from.display(), e.0)),
                }
            }
            (done, replayed, failed)
        })).collect();
        handles.into_iter().map(|h| h.join().unwrap_or((0, 0, vec!["a replay thread stopped".into()]))).collect()
    });
    let (done, replayed): (u64, u64) = counts.iter().fold((0, 0), |(a, b), (d, r, _)| (a + d, b + r));
    for (_, _, failed) in &counts { for f in failed { eprintln!("not replayed: {f}"); } }
    println!("{} stored checks: {done} rebuilt, {replayed} of them judged again from their own test runs", jobs.len());
}
