use kern::twin;
use kern::util::*;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-twin-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
    fn join(&self, s: &str) -> PathBuf { self.0.join(s) }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
fn read(path: &Path) -> Value { serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() }

const BASE: &str = include_str!("../twin/test/apps/three-prs/base/src/app.js");
const HEAD: &str = include_str!("../twin/test/apps/three-prs/head/src/app.js");

fn three_prs() -> (Temp, String, String) {
    let t = Temp::new();
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q"]);
    write(&r, "src/app.js", BASE);
    write(&r, "README.md", "# Shop\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Start the shop"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    let (b, h): (Vec<&str>, Vec<&str>) = (BASE.split('\n').collect(), HEAD.split('\n').collect());
    assert_eq!(b.len(), h.len(), "the fixture's versions differ only in changed lines");
    let mut now = b.clone();
    for (line, summary) in [(9, "Round totals to cents (#101)"), (13, "Rename the user field (#102)"), (17, "Tidy the limit check (#103)")] {
        now[line - 1] = h[line - 1];
        write(&r, "src/app.js", &now.join("\n"));
        git(&r, &["commit", "-qam", summary]);
    }
    write(&r, "README.md", "# Shop\n\nThree changes.\n");
    git(&r, &["commit", "-qam", "Describe the changes"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    (t, base, head)
}

#[test]
fn pull_request_numbers_come_from_squash_and_merge_summaries() {
    assert_eq!(twin::pull_number("Round totals to cents (#101)"), Some(101));
    assert_eq!(twin::pull_number("Merge pull request #104 from me/topic"), Some(104));
    assert_eq!(twin::pull_number("Fix (#12) the thing"), None, "only a trailing number counts");
    assert_eq!(twin::pull_number("Tidy up"), None);
}

#[test]
fn each_changed_line_names_the_pull_request_that_changed_it_on_both_sides() {
    let (t, base, head) = three_prs();
    let out = t.join("manifest.json");
    let summary = twin::plan(&t.join("repo"), &base, &head, &out).unwrap();
    assert_eq!(summary["status"], "PLANNED");
    assert_eq!(summary["lines"], serde_json::json!({ "old": 3, "new": 3 }));
    let m = read(&out);
    assert_eq!(m["files"].as_array().unwrap().len(), 1, "README.md is not code: {m}");
    let f = &m["files"][0];
    assert_eq!(f["path"], "src/app.js");
    assert_eq!(f["status"], "modified");
    assert_eq!(f["changed"], serde_json::json!({ "old": [9, 13, 17], "new": [9, 13, 17] }));
    let pull = |side: &str, line: &str| m["commits"][f[side][line]["commit"].as_str().unwrap_or_else(|| panic!("{side} {line} has no commit: {m}"))]["pull"].as_u64();
    for (line, number) in [("9", 101), ("13", 102), ("17", 103)] {
        assert_eq!(pull("new", line), Some(number), "the new line {line}");
        assert_eq!(pull("old", line), Some(number), "the old line {line}: the pull request that replaced it");
    }
    assert_eq!(f["new"]["17"]["text"], "  return policy.limits.max;");
    assert_eq!(f["old"]["17"]["text"], "  return policy.limits ? policy.limits.max : 100;");
}

#[test]
fn a_line_changed_on_a_merged_branch_names_the_merge_pull_request() {
    let t = Temp::new();
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q"]);
    write(&r, "src/a.js", "export function a() {\n  return 1;\n}\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Start"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    git(&r, &["checkout", "-qb", "topic"]);
    write(&r, "src/a.js", "export function a() {\n  return 2;\n}\n");
    git(&r, &["commit", "-qam", "Return two"]);
    git(&r, &["checkout", "-q", "-"]);
    git(&r, &["merge", "-q", "--no-ff", "topic", "-m", "Merge pull request #104 from me/topic"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("manifest.json");
    twin::plan(&r, &base, &head, &out).unwrap();
    let m = read(&out);
    let f = &m["files"][0];
    for side in ["old", "new"] {
        let commit = f[side]["2"]["commit"].as_str().unwrap_or_else(|| panic!("{side} line 2 has no commit: {m}"));
        assert_eq!(m["commits"][commit]["pull"], 104, "{side}: the merge's pull request");
    }
}

#[test]
fn a_shallow_clone_is_refused_because_its_blame_cannot_name_the_commits() {
    let t = Temp::new();
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q"]);
    write(&r, "src/a.js", "export function a() {\n  return 1;\n}\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Start"]);
    write(&r, "src/b.js", "export const b = 1;\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Add b"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    git(&r, &["checkout", "-qb", "topic"]);
    write(&r, "src/a.js", "export function a() {\n  return 2;\n}\n");
    git(&r, &["commit", "-qam", "Return two"]);
    write(&r, "notes.txt", "x\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "More on the topic"]);
    git(&r, &["checkout", "-q", "-"]);
    git(&r, &["merge", "-q", "--no-ff", "topic", "-m", "Merge pull request #104 from me/topic"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let url = format!("file://{}", r.display().to_string().replace('\\', "/"));
    git(&t.0, &["clone", "-q", "--depth", "2", &url, "shallow"]);
    let shallow = t.join("shallow");
    assert_eq!(git(&shallow, &["cat-file", "-t", &base]), "commit", "the shallow clone holds the base");
    let err = twin::plan(&shallow, &base, &head, &t.join("manifest.json")).unwrap_err().to_string();
    assert!(err.contains("shallow") && err.contains("git fetch --unshallow"), "{err}");
    assert!(!t.join("manifest.json").exists(), "no manifest is written");
}

#[test]
fn a_change_without_code_is_named_and_an_existing_manifest_is_never_replaced() {
    let t = Temp::new();
    let r = t.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q"]);
    write(&r, "src/a.js", "// One.\nexport const a = () => 1;\n");
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-qm", "Start"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    write(&r, "src/a.js", "// One, said better.\nexport const a = () => 1;\n");
    git(&r, &["commit", "-qam", "Say it better (#7)"]);
    let head = git(&r, &["rev-parse", "HEAD"]);
    let out = t.join("manifest.json");
    let summary = twin::plan(&r, &base, &head, &out).unwrap();
    assert_eq!(summary["files"], 0);
    let m = read(&out);
    assert_eq!(m["skipped"][0]["path"], "src/a.js");
    assert!(m["skipped"][0]["why"].as_str().unwrap().starts_with("reformatted only"), "{m}");
    assert!(twin::plan(&r, &base, &head, &out).is_err(), "an existing manifest is never replaced");
}

#[test]
fn kern_twin_plan_runs_from_the_command_line() {
    let (t, base, head) = three_prs();
    let out = Command::new(env!("CARGO_BIN_EXE_kern")).args(["twin", "plan"]).arg(t.join("repo")).args([&base, &head]).arg(t.join("m.json")).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(summary["lines"]["new"], 3);
    let bad = Command::new(env!("CARGO_BIN_EXE_kern")).args(["twin", "plan", "only-one-argument"]).output().unwrap();
    assert!(!bad.status.success());
}
