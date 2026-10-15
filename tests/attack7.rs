use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-attack7-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}
fn repo(t: &Temp, files: &[(&str, &str)]) -> (PathBuf, String) {
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    for (p, text) in files { write(&r, p, text); }
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let base = git(&r, &["rev-parse", "HEAD"]);
    (r, base)
}
fn commit(r: &Path, files: &[(&str, &str)]) -> String {
    for (p, text) in files { write(r, p, text); }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "change"]);
    git(r, &["rev-parse", "HEAD"])
}
fn operator() -> kern::policy::Operator {
    kern::policy::operator(&json!({ "version": 3, "repository": "acme/shop", "deployment": "single-vm", "executors": ["external"], "image": format!("sha256:{}", "a".repeat(64)),
        "builders": { "executor": null, "verifier": "e".repeat(64) }, "checks": [{ "id": "addition", "file": std::path::absolute("checks.json").unwrap(), "sha256": "b".repeat(64) }],
        "registry": "registry.npmjs.org", "permissions": {} })).unwrap()
}

#[test]
fn a_code_file_named_like_a_doc_still_runs_the_tests() {
    let good = "exports.discount = (p) => p * 0.9;\n";
    let bad = "exports.discount = (p) => p * 0.5;\n";
    let mut wrong = Vec::new();
    for name in ["notice.js", "license.ts", "changelog.mjs", "authors.jsx"] {
        let t = Temp::new();
        let (r, b) = repo(&t, &[("package.json", r#"{"name":"shop","scripts":{"test":"jest"},"devDependencies":{"jest":"30.0.0"}}"#), (&format!("src/{name}"), good)]);
        let h = commit(&r, &[(&format!("src/{name}"), bad)]);
        let out = kern::check::check(&r, &b, &h, &t.0.join("out"), &operator(), None, None).unwrap();
        let md = std::fs::read_to_string(t.0.join("out").join("receipt.md")).unwrap();
        if out["tests"] == "NOTHING_TO_RUN" || md.contains("touches only documentation") {
            wrong.push(format!("src/{name}: tests={} decision={} says-documentation={}", out["tests"], out["decision"], md.contains("touches only documentation")));
        }
    }
    assert!(wrong.is_empty(), "these are real source files with logic a pull request breaks, yet Kern calls each one documentation, skips its tests and clears the change:\n{}", wrong.join("\n"));
}

#[test]
fn a_setup_file_a_package_names_is_a_settings_item_even_in_a_subpackage() {
    let before = "module.exports = { moduleNameMapper: { '^@/(.*)$': '<rootDir>/real/$1' } };\n";
    let after = "module.exports = { moduleNameMapper: { '^@/(.*)$': '<rootDir>/stub/$1' } };\n";
    let named = |pkgs: &[(&str, &str)]| -> bool {
        let t = Temp::new();
        let (r, b) = repo(&t, pkgs);
        let h = commit(&r, &[("tooling/map.js", after)]);
        let rules = kern::rules::rules(&r, &b, &h).unwrap();
        rules["settings"].as_array().into_iter().flatten().any(|s| s["path"] == "tooling/map.js")
    };
    let at_root = named(&[("package.json", r#"{"name":"mono","scripts":{"test":"jest --setupFiles ./tooling/map.js"},"devDependencies":{"jest":"30.0.0"}}"#), ("tooling/map.js", before)]);
    let in_package = named(&[("package.json", r#"{"name":"mono","private":true}"#), ("tooling/map.js", before),
        ("packages/app/package.json", r#"{"name":"app","scripts":{"test":"jest --setupFiles ../../tooling/map.js"},"devDependencies":{"jest":"30.0.0"}}"#)]);
    assert!(at_root, "a jest setup file the root test script names must be a settings item when it changes");
    assert!(in_package, "packages/app's test script names ../../tooling/map.js as its setup, but because packages/app/package.json is neither the root nor an ancestor of the changed file, Kern never reads it, so a change that rewrites the whole suite's module map raises no settings item and the change can clear");
}
