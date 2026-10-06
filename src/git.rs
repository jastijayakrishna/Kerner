use crate::policy::{Contract, git_exe};
use crate::util::*;
use crate::{deny, ensure};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Entry { pub mode: String, pub bytes: Vec<u8> }
pub type Files = BTreeMap<String, Entry>;
pub const MAX_FILE: u64 = 1048576;
pub const MAX_TOTAL: u64 = 16777216;
const BUNDLE_BYTES: usize = 64 * 1048576;

pub fn git_with(repo: &Path, args: &[&str], input: Option<&[u8]>, max_output: usize) -> R<Vec<u8>> {
    let mut full: Vec<String> = ["--no-replace-objects", "-c", "core.hooksPath=/dev/null", "-c", "protocol.allow=never", "-C"].map(String::from).to_vec();
    full.push(repo.display().to_string());
    full.extend(args.iter().map(|a| a.to_string()));
    run(&git_exe()?, &full, Run { cwd: Some(repo), env: safe_env(repo), input, timeout: Duration::from_secs(600), max_output })
}
pub fn git(repo: &Path, args: &[&str], input: Option<&[u8]>) -> R<Vec<u8>> { git_with(repo, args, input, BUNDLE_BYTES) }
pub fn trimmed(bytes: Vec<u8>) -> String { String::from_utf8_lossy(&bytes).trim().to_string() }
pub fn oid(s: &str) -> R<&str> {
    ensure!(is_oid(s), "Expected full SHA-1 commit ID");
    Ok(s)
}
pub fn object_id(kind: &str, body: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("{kind} {}\0", body.len()).as_bytes());
    h.update(body);
    hex::encode(h.finalize())
}
pub fn init_store(dir: &Path) -> R<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let args: Vec<String> = ["-c", "init.templateDir=", "init", "--bare", "--object-format=sha1", "-q"].iter().map(|s| s.to_string()).chain([dir.display().to_string()]).collect();
    run(&git_exe()?, &args, Run { cwd: Some(dir), env: safe_env(dir), ..Default::default() })?;
    Ok(std::path::absolute(dir)?)
}

pub fn read_commit(repo: &Path, commit: &str, file_bytes: u64, total_bytes: u64) -> R<Files> {
    oid(commit)?;
    ensure!(trimmed(git(repo, &["cat-file", "-t", commit], None)?) == "commit", "Artifact is not a commit");
    let listing = git(repo, &["ls-tree", "-rlz", "--full-tree", commit], None)?;
    let rows: Vec<&[u8]> = listing.split(|b| *b == 0).filter(|r| !r.is_empty()).collect();
    ensure!(rows.len() <= 10000, "Too many repository files");
    let (mut entries, mut seen, mut total) = (Vec::new(), std::collections::HashSet::new(), 0u64);
    for row in rows {
        let tab = row.iter().position(|b| *b == b'\t').ok_or_else(|| Denied("Malformed tree listing".into()))?;
        let header = std::str::from_utf8(&row[..tab]).map_err(|_| Denied("Malformed tree listing".into()))?;
        let name = std::str::from_utf8(&row[tab + 1..]).map_err(|_| Denied("Nonportable path: invalid UTF-8".into()))?.to_string();
        let f: Vec<&str> = header.split_whitespace().collect();
        ensure!(f.len() == 4 && (f[0] == "100644" || f[0] == "100755") && f[1] == "blob" && is_oid(f[2]), "Symlinks, submodules and special files are unsupported: {name}");
        let size: u64 = f[3].parse().map_err(|_| Denied("Malformed tree listing".into()))?;
        file_path(&name)?;
        ensure!(seen.insert(name.to_lowercase()), "Case-colliding filenames: {name}");
        total += size;
        ensure!(size <= file_bytes && total <= total_bytes, "Repository exceeds byte limits");
        entries.push((name, f[0].to_string(), f[2].to_string(), size as usize));
    }
    tree_paths(entries.iter().map(|e| e.0.as_str()), true)?;
    if entries.is_empty() { return Ok(Files::new()); }
    let request: String = entries.iter().map(|e| format!("{}\n", e.2)).collect();
    let stream = git_with(repo, &["cat-file", "--batch"], Some(request.as_bytes()), total as usize + entries.len() * 64 + 4096)?;
    let (mut files, mut at) = (Files::new(), 0usize);
    for (name, mode, blob, size) in entries {
        let end = stream[at..].iter().position(|b| *b == b'\n').map(|p| p + at).ok_or_else(|| Denied("Malformed Git object stream".into()))?;
        ensure!(stream[at..end] == *format!("{blob} blob {size}").as_bytes() && stream.get(end + 1 + size) == Some(&b'\n'), "Git object stream mismatch");
        files.insert(name, Entry { mode, bytes: stream[end + 1..end + 1 + size].to_vec() });
        at = end + 2 + size;
    }
    ensure!(at == stream.len(), "Unexpected Git object stream data");
    Ok(files)
}

enum Node { Dir(BTreeMap<String, Node>), File(String, String) }
pub fn write_commit(repo: &Path, files: &Files, parent: Option<&str>, task: &str) -> R<String> {
    if let Some(p) = parent { oid(p)?; }
    tree_paths(files.keys().map(String::as_str), true)?;
    let (mut root, mut objects) = (BTreeMap::new(), Vec::<(String, &'static str, Vec<u8>)>::new());
    for (name, e) in files {
        ensure!(e.mode == "100644" || e.mode == "100755", "Invalid file mode");
        let id = object_id("blob", &e.bytes);
        objects.push((id.clone(), "blob", e.bytes.clone()));
        let parts: Vec<&str> = name.split('/').collect();
        let mut node = &mut root;
        for part in &parts[..parts.len() - 1] {
            let next = node.entry(part.to_string()).or_insert_with(|| Node::Dir(BTreeMap::new()));
            let Node::Dir(children) = next else { deny!("File/directory collision") };
            node = children;
        }
        ensure!(node.insert(parts[parts.len() - 1].to_string(), Node::File(e.mode.clone(), id)).is_none(), "Duplicate file");
    }
    fn tree(node: &BTreeMap<String, Node>, objects: &mut Vec<(String, &'static str, Vec<u8>)>) -> String {
        let mut items: Vec<(Vec<u8>, String, &str, String)> = node.iter().map(|(name, n)| match n {
            Node::Dir(c) => (format!("{name}/").into_bytes(), name.clone(), "40000", tree(c, objects)),
            Node::File(mode, id) => (name.clone().into_bytes(), name.clone(), mode.as_str(), id.clone()),
        }).collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));
        let mut body = Vec::new();
        for (_, name, mode, id) in items {
            body.extend_from_slice(format!("{mode} {name}\0").as_bytes());
            body.extend_from_slice(&hex::decode(id).expect("hex object id"));
        }
        let id = object_id("tree", &body);
        objects.push((id.clone(), "tree", body));
        id
    }
    let tree_id = tree(&root, &mut objects);
    let request: String = objects.iter().map(|o| format!("{}\n", o.0)).collect();
    let status = String::from_utf8_lossy(&git(repo, &["cat-file", "--batch-check"], Some(request.as_bytes()))?).to_string();
    let missing: std::collections::HashSet<&str> = status.lines().filter(|l| l.ends_with(" missing")).map(|l| &l[..40]).collect();
    let mut written = std::collections::HashSet::new();
    for (id, kind, body) in &objects {
        if missing.contains(id.as_str()) && written.insert(id.clone()) {
            let args: &[&str] = if *kind == "blob" { &["hash-object", "-w", "--stdin", "--no-filters"] } else { &["hash-object", "-w", "-t", "tree", "--stdin"] };
            ensure!(trimmed(git(repo, args, Some(body))?) == *id, "Git object identity mismatch");
        }
    }
    let parent_line = parent.map(|p| format!("parent {p}\n")).unwrap_or_default();
    let commit = format!("tree {tree_id}\n{parent_line}author Kern <kern@invalid> 0 +0000\ncommitter Kern <kern@invalid> 0 +0000\n\nKern task {task}\n");
    Ok(trimmed(git(repo, &["hash-object", "-w", "-t", "commit", "--stdin"], Some(commit.as_bytes()))?))
}
pub fn manifest(repo: &Path, commit: &str, file_bytes: u64, total_bytes: u64) -> R<Value> {
    let files = read_commit(repo, commit, file_bytes, total_bytes)?;
    let entries: Vec<Value> = files.iter().map(|(p, e)| json!({ "path": p, "mode": e.mode, "size": e.bytes.len(), "sha256": sha256(&e.bytes) })).collect();
    Ok(json!({ "version": 1, "commit": commit, "commit_sha256": sha256(git(repo, &["cat-file", "commit", commit], None)?), "tree_sha256": digest(&json!(entries))?, "files": entries }))
}
pub fn parents(repo: &Path, commit: &str) -> R<Vec<String>> {
    let raw = String::from_utf8_lossy(&git(repo, &["cat-file", "commit", oid(commit)?], None)?).to_string();
    Ok(raw.split("\n\n").next().unwrap_or("").lines().filter_map(|l| l.strip_prefix("parent ").map(str::to_string)).collect())
}
pub fn materialize(files: &Files, dest: &Path, grants: Option<&[String]>) -> R<()> {
    std::fs::create_dir_all(dest)?;
    for (name, e) in files {
        if grants.is_some_and(|g| !allowed(g, name)) { continue; }
        let target = name.split('/').fold(dest.to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(target.parent().unwrap_or(dest))?;
        create_new(&target, &e.bytes, 0o444)?;
    }
    Ok(())
}
pub fn protected(name: &str) -> bool {
    let parts: Vec<String> = name.split('/').map(str::to_ascii_lowercase).collect();
    let base = parts.last().map(String::as_str).unwrap_or("");
    parts.iter().any(|p| p == ".github" || p == "node_modules")
        || ["package.json", "package-lock.json", "npm-shrinkwrap.json", "yarn.lock", "pnpm-lock.yaml", "cargo.toml", "cargo.lock", "go.mod", "go.sum", "pyproject.toml", ".gitmodules", ".gitattributes", ".npmrc", ".env"].contains(&base)
        || base.starts_with(".env.")
        || (base.starts_with("requirements") && base.ends_with(".txt"))
}
pub fn changes(base: &Files, candidate: &Files, policy: &Contract) -> R<Value> {
    let mut names: Vec<&String> = base.keys().chain(candidate.keys()).collect();
    names.sort();
    names.dedup();
    let (mut result, mut lines) = (Vec::new(), 0u64);
    for name in names {
        let (a, b) = (base.get(name), candidate.get(name));
        if let (Some(a), Some(b)) = (a, b) && a.mode == b.mode && a.bytes == b.bytes { continue; }
        ensure!(allowed(&policy.write, name), "Write outside contract: {name}");
        ensure!(!protected(name), "Protected dependency/configuration path: {name}");
        let mut texts = Vec::new();
        for e in [a, b] {
            let Some(e) = e else { texts.push(Vec::new()); continue; };
            ensure!(!e.bytes.contains(&0), "Binary changes unsupported: {name}");
            let s = std::str::from_utf8(&e.bytes).map_err(|_| Denied(format!("Invalid UTF-8 change: {name}")))?;
            texts.push(s.split('\n').collect::<Vec<&str>>());
        }
        let (x, y) = (&texts[0], &texts[1]);
        let (mut start, mut ea, mut eb) = (0, x.len(), y.len());
        while start < ea && start < eb && x[start] == y[start] { start += 1; }
        while ea > start && eb > start && x[ea - 1] == y[eb - 1] { ea -= 1; eb -= 1; }
        lines += (ea + eb - 2 * start) as u64;
        result.push(json!({ "path": name, "operation": if a.is_none() { "add" } else if b.is_none() { "delete" } else { "modify" } }));
    }
    ensure!(result.len() as u64 <= policy.limits.files_changed && lines <= policy.limits.lines_changed, "Change budget exceeded");
    Ok(json!({ "files": result, "lines_upper_bound": lines }))
}

pub fn import_base(source: &Path, base: &str, dest: &Path) -> R<PathBuf> {
    oid(base)?;
    let store = init_store(dest)?;
    let objects = git(source, &["rev-list", "--objects", "--no-walk", base], None)?;
    let pack = git(source, &["pack-objects", "--stdout", "-q"], Some(&objects))?;
    git(&store, &["index-pack", "--stdin", "--fsck-objects"], Some(&pack))?;
    std::fs::write(store.join("shallow"), format!("{base}\n"))?;
    ensure!(trimmed(git(&store, &["cat-file", "-t", base], None)?) == "commit", "Invalid base object");
    Ok(store)
}

pub const RUN_FIELDS: [&str; 9] = ["version", "run_id", "repository", "base", "commit", "contract_sha256", "manifest_sha256", "executor", "dependencies"];
pub fn validate_run(run: &Value) -> R<()> {
    exact(run, &RUN_FIELDS, "run")?;
    ensure!(run["version"] == 2 && is_uuid(text(run, "run_id")?), "Invalid run identity");
    oid(text(run, "base")?)?;
    oid(text(run, "commit")?)?;
    ensure!(text(run, "repository").is_ok() && is_hex64(text(run, "contract_sha256")?) && is_hex64(text(run, "manifest_sha256")?), "Invalid run digests");
    let e = &run["executor"];
    match e["kind"].as_str() {
        Some("external") => { exact(e, &["kind", "source_commit"], "external executor")?; oid(text(e, "source_commit")?)?; }
        Some("kern-gvisor") => { exact(e, &["kind", "builder_sha256", "builder", "runtime", "image", "exit_code", "output_sha256"], "Kern executor")?; }
        _ => deny!("Unknown executor kind"),
    }
    ensure!(run["dependencies"].is_null() || run["dependencies"]["sha256"].as_str().is_some_and(is_hex64), "Invalid dependency record");
    Ok(())
}
pub fn handoff(store: &Path, run: &Value, dir: &Path) -> R<Value> {
    validate_run(run)?;
    let root = fresh_dir(dir)?;
    let (base, commit) = (text(run, "base")?, text(run, "commit")?);
    let objects = git(store, &["rev-list", "--objects", "--no-walk", base, commit], None)?;
    let pack = git(store, &["pack-objects", "--stdout", "-q"], Some(&objects))?;
    let mut bundle = format!("# v2 git bundle\n{base} refs/kern/base\n{commit} refs/kern/candidate\n\n").into_bytes();
    bundle.extend_from_slice(&pack);
    ensure!(bundle.len() <= BUNDLE_BYTES, "Handoff bundle exceeds 64 MiB");
    create_new(root.join("artifact.bundle"), &bundle, 0o600)?;
    write_json(root.join("run.json"), run)?;
    Ok(json!({ "directory": root.display().to_string(), "bundle_sha256": sha256(&bundle), "bytes": bundle.len() }))
}
pub fn import_handoff(dir: &Path, output: &Path) -> R<Value> {
    let (run_file, bundle_file) = (dir.join("run.json"), dir.join("artifact.bundle"));
    for f in [&run_file, &bundle_file] { ensure!(std::fs::symlink_metadata(f).is_ok_and(|m| m.is_file()), "Handoff must contain regular run.json and artifact.bundle files"); }
    ensure!(std::fs::metadata(&bundle_file)?.len() <= BUNDLE_BYTES as u64, "Handoff bundle exceeds 64 MiB");
    let run = read_json(&run_file)?;
    validate_run(&run)?;
    let (base, commit) = (text(&run, "base")?, text(&run, "commit")?);
    let bytes = std::fs::read(&bundle_file)?;
    let header = format!("# v2 git bundle\n{base} refs/kern/base\n{commit} refs/kern/candidate\n\n");
    ensure!(bytes.starts_with(header.as_bytes()), "Bundle refs must be exactly the run base and candidate, with no prerequisites");
    ensure!(bytes[header.len()..].starts_with(b"PACK"), "Bundle does not contain a pack");
    let root = fresh_dir(output)?;
    let store = init_store(&root.join("objects.git"))?;
    git(&store, &["-c", "core.bigFileThreshold=1m", "index-pack", "--stdin", "--fsck-objects", &format!("--max-input-size={BUNDLE_BYTES}")], Some(&bytes[header.len()..]))?;
    std::fs::write(store.join("shallow"), format!("{base}\n"))?;
    for (r, id) in [("refs/kern/base", base), ("refs/kern/candidate", commit)] {
        ensure!(trimmed(git(&store, &["cat-file", "-t", id], None)?) == "commit", "Bundle lacks a declared commit");
        git(&store, &["update-ref", r, id, &"0".repeat(40)], None)?;
    }
    ensure!(parents(&store, commit)? == vec![base.to_string()], "Candidate must have the approved base as its only parent");
    read_commit(&store, base, MAX_FILE, MAX_TOTAL)?;
    ensure!(digest(&manifest(&store, commit, MAX_FILE, MAX_TOTAL)?)? == text(&run, "manifest_sha256")?, "Transferred manifest mismatch");
    write_json(root.join("run.json"), &run)?;
    Ok(json!({ "status": "IMPORTED_UNVERIFIED", "output": root.display().to_string(), "base": base, "commit": commit }))
}
