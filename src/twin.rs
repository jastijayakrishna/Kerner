use crate::drive::{Item, blobs, changed, commit_check, listing, measured, minified, same_program};
use crate::ensure;
use crate::git::{git, git_with, trimmed};
use crate::util::*;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const MOST_FILES: usize = 2000;
const MOST_TEXT: u64 = 128 << 20;
const LARGEST: u64 = 1 << 20;

pub fn pull_number(summary: &str) -> Option<u64> {
    let s = summary.trim();
    if let Some(rest) = s.strip_prefix("Merge pull request #") {
        return rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok());
    }
    let inner = s.strip_suffix(')')?;
    let at = inner.rfind("(#")?;
    inner[at + 2..].parse().ok()
}

fn blame(repo: &Path, args: &[&str]) -> R<BTreeMap<u32, (String, bool)>> {
    let out = git_with(repo, args, None, 256 << 20)?;
    let text = String::from_utf8_lossy(&out);
    let mut lines = Vec::new();
    let mut boundary = BTreeSet::new();
    let mut current: Option<(String, u32)> = None;
    for row in text.split('\n') {
        if row.starts_with('\t') {
            if let Some(entry) = current.take() { lines.push(entry); }
            continue;
        }
        let mut words = row.split(' ');
        let first = words.next().unwrap_or("");
        if first.len() == 40 && first.bytes().all(|b| b.is_ascii_hexdigit()) {
            if let Some(n) = words.nth(1).and_then(|w| w.parse::<u32>().ok()) { current = Some((first.to_string(), n)); }
        } else if first == "boundary" && let Some((sha, _)) = &current {
            boundary.insert(sha.clone());
        }
    }
    Ok(lines.into_iter().map(|(sha, n)| { let edge = boundary.contains(&sha); (n, (sha, edge)) }).collect())
}

fn landing(repo: &Path, head: &str, first: &[String], commit: &str, seen: &mut BTreeMap<String, Option<String>>) -> R<Option<String>> {
    if first.iter().any(|c| c == commit) { return Ok(Some(commit.to_string())); }
    if let Some(found) = seen.get(commit) { return Ok(found.clone()); }
    let out = git(repo, &["rev-list", "--first-parent", "--ancestry-path", &format!("{commit}..{head}")], None)?;
    let found = String::from_utf8_lossy(&out).lines().last().map(str::to_string).filter(|c| first.contains(c));
    seen.insert(commit.to_string(), found.clone());
    Ok(found)
}

pub fn plan(repo: &Path, base: &str, head: &str, out: &Path) -> R<Value> {
    commit_check(repo, base)?;
    commit_check(repo, head)?;
    ensure!(trimmed(git(repo, &["rev-parse", "--is-shallow-repository"], None)?) == "false",
        "The repository is a shallow clone, so git blame cannot name the commit behind each changed line. Fetch the full history first: git fetch --unshallow");
    let (old, new) = (listing(repo, base)?, listing(repo, head)?);
    let index = |items: &[Item]| -> BTreeMap<String, Item> { items.iter().map(|i| (i.path.clone(), i.clone())).collect() };
    let (old_at, new_at) = (index(&old), index(&new));
    let names = git_with(repo, &["diff", "--no-renames", "--name-only", "-z", base, head], None, 64 << 20)?;
    let paths: Vec<String> = names.split(|b| *b == 0).filter(|n| !n.is_empty()).map(|n| String::from_utf8_lossy(n).to_string()).collect();
    let mut skipped = Vec::new();
    let (mut room, mut code) = (MOST_TEXT, Vec::new());
    for p in paths.iter().filter(|p| crate::compare::probeable(p, None)) {
        let size = old_at.get(p).map_or(0, |i| i.size.min(LARGEST)) + new_at.get(p).map_or(0, |i| i.size.min(LARGEST));
        if code.len() < MOST_FILES && size <= room {
            room -= size;
            code.push(p.clone());
        } else {
            skipped.push(json!({ "path": p, "why": "not planned: the change is too large (Kern plans 2,000 files and 128 MiB of code)" }));
        }
    }
    let mut texts: BTreeMap<(bool, String), Vec<u8>> = BTreeMap::new();
    for (side, at) in [(false, &old_at), (true, &new_at)] {
        let wanted: Vec<&Item> = code.iter().filter_map(|p| at.get(p)).filter(|i| i.size <= LARGEST && matches!(i.mode.as_str(), "100644" | "100755")).collect();
        blobs(repo, &wanted, |item, bytes| {
            texts.insert((side, item.path.clone()), bytes.to_vec());
            Ok(())
        })?;
    }
    let range = format!("{base}..{head}");
    let first: Vec<String> = String::from_utf8_lossy(&git(repo, &["rev-list", "--first-parent", "--reverse", &range], None)?).lines().map(str::to_string).collect();
    let mut seen = BTreeMap::new();
    let mut files = Vec::new();
    for p in &code {
        let sides = [old_at.get(p), new_at.get(p)];
        if sides.iter().flatten().any(|i| i.size > LARGEST) {
            skipped.push(json!({ "path": p, "why": "larger than 1 MiB (generated code or data)" }));
            continue;
        }
        if sides.iter().flatten().any(|i| !matches!(i.mode.as_str(), "100644" | "100755")) {
            skipped.push(json!({ "path": p, "why": "a symlink" }));
            continue;
        }
        let text = |side: bool| texts.get(&(side, p.clone())).map(|b| std::str::from_utf8(b));
        let (o, n) = (text(false), text(true));
        if matches!(o, Some(Err(_))) || matches!(n, Some(Err(_))) {
            skipped.push(json!({ "path": p, "why": "not UTF-8 text" }));
            continue;
        }
        let (o, n) = (o.and_then(Result::ok).unwrap_or(""), n.and_then(Result::ok).unwrap_or(""));
        if minified(o) || minified(n) {
            skipped.push(json!({ "path": p, "why": "generated or minified code" }));
            continue;
        }
        if old_at.contains_key(p) && new_at.contains_key(p) && same_program(p, o, n) {
            skipped.push(json!({ "path": p, "why": "reformatted only: the old and the new code parse to the same program" }));
            continue;
        }
        let (co, cn) = changed(o, n);
        let (mo, mn) = (measured(p, o), measured(p, n));
        let pick = |c: &[u32], m: &BTreeMap<u32, String>| -> Vec<(u32, String)> { c.iter().filter_map(|l| m.get(l).map(|t| (*l, t.clone()))).collect() };
        let (po, pn) = (pick(&co, &mo), pick(&cn, &mn));
        if po.is_empty() && pn.is_empty() {
            skipped.push(json!({ "path": p, "why": "no line of code changed (comments, imports or types only)" }));
            continue;
        }
        let blamed_new = if pn.is_empty() { BTreeMap::new() } else { blame(repo, &["blame", "--porcelain", &range, "--", p])? };
        let blamed_old = if po.is_empty() { BTreeMap::new() } else { blame(repo, &["blame", "--reverse", "--porcelain", &range, "--", p])? };
        let mut new_side = Map::new();
        for (line, text) in &pn {
            let commit = match blamed_new.get(line) {
                Some((sha, edge)) if !edge && sha != base => landing(repo, head, &first, sha, &mut seen)?,
                _ => None,
            };
            new_side.insert(line.to_string(), json!({ "text": text, "commit": commit }));
        }
        let mut old_side = Map::new();
        for (line, text) in &po {
            let commit = match blamed_old.get(line) {
                Some((sha, _)) if sha == base => first.first().cloned(),
                Some((sha, _)) => match first.iter().position(|c| c == sha) {
                    Some(i) => first.get(i + 1).cloned(),
                    None => landing(repo, head, &first, sha, &mut seen)?,
                },
                None => None,
            };
            old_side.insert(line.to_string(), json!({ "text": text, "commit": commit }));
        }
        let status = match (old_at.contains_key(p), new_at.contains_key(p)) { (false, _) => "added", (_, false) => "deleted", _ => "modified" };
        files.push(json!({ "path": p, "status": status, "changed": { "old": co, "new": cn }, "old": old_side, "new": new_side }));
    }
    let mut named = BTreeSet::new();
    for f in &files {
        for side in ["old", "new"] {
            for entry in f[side].as_object().into_iter().flat_map(|m| m.values()) {
                if let Some(c) = entry["commit"].as_str() { named.insert(c.to_string()); }
            }
        }
    }
    let named: Vec<String> = named.into_iter().collect();
    let mut commits = Map::new();
    for group in named.chunks(200) {
        let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x09%s"];
        args.extend(group.iter().map(String::as_str));
        for row in String::from_utf8_lossy(&git(repo, &args, None)?).lines() {
            if let Some((sha, summary)) = row.split_once('\t') { commits.insert(sha.to_string(), json!({ "summary": summary, "pull": pull_number(summary) })); }
        }
    }
    let count = |side: &str| files.iter().map(|f| f[side].as_object().map_or(0, Map::len)).sum::<usize>();
    let summary = json!({ "status": "PLANNED", "manifest": std::path::absolute(out)?.display().to_string(), "files": files.len(),
        "lines": { "old": count("old"), "new": count("new") }, "commits": commits.len(), "skipped": skipped.len() });
    write_json(out, &json!({ "version": 1, "base": base, "head": head, "files": files, "commits": commits, "skipped": skipped }))?;
    Ok(summary)
}
