use crate::ensure;
use crate::util::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn last_line(file: &mut std::fs::File) -> R<Option<String>> {
    let len = file.seek(SeekFrom::End(0))?;
    if len == 0 { return Ok(None); }
    let start = len.saturating_sub(4 << 20);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let body = text.strip_suffix('\n').unwrap_or(&text);
    ensure!(start == 0 || body.contains('\n'), "Audit log entry too large");
    Ok(Some(body.rsplit('\n').next().unwrap_or(body).to_string()))
}
pub fn append(path: &Path, event: &Value) -> R<()> {
    let mut file = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    file.lock()?;
    let (seq, prev) = match last_line(&mut file)? {
        None => (1, GENESIS.to_string()),
        Some(line) => (serde_json::from_str::<Value>(&line)?["seq"].as_u64().unwrap_or(0) + 1, sha256(&line)),
    };
    let entry = json!({ "seq": seq, "at": iso8601(now_ms()), "kern": env!("CARGO_PKG_VERSION"), "prev": prev, "event": event });
    file.write_all(format!("{}\n", canonical(&entry)?).as_bytes())?;
    Ok(file.sync_all()?)
}
pub fn event(command: &str, outcome: &R<Value>) -> Value {
    match outcome {
        Ok(v) => json!({ "command": command, "status": v["status"], "task": v["task"], "commit": v.get("commit").or(v.get("head")), "reason": v.get("reason").or(v.get("summary")), "result": v }),
        Err(e) => json!({ "command": command, "status": "DENIED", "reason": e.0 }),
    }
}
pub fn verify(path: &Path) -> R<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    let (mut prev, mut entries) = (GENESIS.to_string(), 0u64);
    for line in text.lines() {
        entries += 1;
        let ok = serde_json::from_str::<Value>(line).ok().is_some_and(|v| v["seq"] == entries && v["prev"] == prev.as_str() && canonical(&v).ok().as_deref() == Some(line));
        ensure!(ok, "Audit log broken at line {entries}: an entry was altered, removed or reordered");
        prev = sha256(line);
    }
    Ok(json!({ "status": "INTACT", "entries": entries, "head_sha256": prev }))
}

fn esc(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;") }
fn field<'a>(e: &'a Value, key: &str) -> &'a str { e["event"][key].as_str().or(e["event"]["result"][key].as_str()).unwrap_or("") }
fn blocked(e: &Value) -> bool { matches!(field(e, "status"), "BLOCKED" | "DENIED") }
pub fn report(path: &Path) -> R<String> {
    let integrity = verify(path);
    let text = std::fs::read_to_string(path).map_err(|e| Denied(format!("{}: {e}", path.display())))?;
    let events: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let stopped = events.iter().filter(|e| blocked(e)).count();
    let tasks = events.iter().map(|e| field(e, "task")).filter(|t| !t.is_empty()).collect::<std::collections::BTreeSet<_>>().len();
    let mut kinds = BTreeMap::<String, usize>::new();
    for e in events.iter().filter(|e| blocked(e)) {
        let reason = field(e, "reason");
        *kinds.entry(reason.split([':', '.']).next().unwrap_or(reason).trim().to_string()).or_default() += 1;
    }
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let tile = |label: &str, value: String| format!("<div class=\"tile\"><b>{}</b>{}</div>", esc(&value), esc(label));
    let rate = if events.is_empty() { "0%".into() } else { format!("{}%", stopped * 100 / events.len()) };
    let reasons: String = kinds.iter().map(|(k, n)| format!("<tr><td>{}</td><td>{n}</td></tr>", esc(k))).collect();
    let rows: String = events.iter().rev().take(500).map(|e| {
        let (status, commit) = (field(e, "status"), field(e, "commit"));
        format!("<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td class=\"{}\">{}</td><td><code>{}</code></td><td class=\"reason\">{}</td></tr>",
            esc(e["at"].as_str().unwrap_or("")), esc(field(e, "command")), esc(field(e, "task")), if blocked(e) { "bad" } else { "ok" }, esc(status), esc(&commit[..commit.len().min(12)]), esc(field(e, "reason")))
    }).collect();
    let (chain, chain_class) = match &integrity {
        Ok(v) => (format!("Chain intact: {} entries, head {}", v["entries"], &v["head_sha256"].as_str().unwrap_or("")[..12]), "ok"),
        Err(e) => (e.0.clone(), "bad"),
    };
    Ok(format!(r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Kern audit report</title><style>
:root{{--bg:#fafaf9;--fg:#1c1917;--muted:#78716c;--card:#fff;--line:#e7e5e4;--ok:#15803d;--bad:#b91c1c}}
@media (prefers-color-scheme:dark){{:root{{--bg:#0c0a09;--fg:#e7e5e4;--muted:#a8a29e;--card:#1c1917;--line:#292524;--ok:#4ade80;--bad:#f87171}}}}
body{{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{max-width:1100px;margin:0 auto;padding:24px 16px}} h1{{font-size:20px;margin:0 0 4px}} h2{{font-size:15px;margin:28px 0 8px}}
.muted{{color:var(--muted)}} .ok{{color:var(--ok)}} .bad{{color:var(--bad)}} .wrap{{overflow-x:auto}} code{{font-size:12px}}
.tiles{{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:12px;margin:20px 0}}
.tile{{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px 14px;color:var(--muted)}} .tile b{{display:block;font-size:24px;color:var(--fg)}}
table{{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line)}}
th,td{{text-align:left;padding:8px 10px;border-bottom:1px solid var(--line);vertical-align:top}} th{{font-size:12px;color:var(--muted);font-weight:600}} td.reason{{word-break:break-word;min-width:280px}}
</style></head><body><main>
<h1>Kern audit report</h1><div class="muted">{log} &middot; generated {now}</div><div class="{chain_class}">{chain}</div>
<div class="tiles">{t1}{t2}{t3}{t4}</div>
<h2>Why changes were blocked</h2><div class="wrap"><table><tr><th>Reason</th><th>Count</th></tr>{reasons}</table></div>
<h2>Decisions, newest first</h2><div class="wrap"><table><tr><th>Time (UTC)</th><th>Command</th><th>Task</th><th>Status</th><th>Commit</th><th>Reason</th></tr>{rows}</table></div>
</main></body></html>
"#, log = esc(&path.display().to_string()), now = esc(&iso8601(now_ms())), chain = esc(&chain),
        t1 = tile("decisions", events.len().to_string()), t2 = tile("blocked", stopped.to_string()), t3 = tile("block rate", rate), t4 = tile("tasks", tasks.to_string())))
}
