use serde_json::json;
use std::path::{Path, PathBuf};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-restart-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn write(dir: &Path, path: &str, text: &str) {
    let p = dir.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn comfy_desktop_1732_every_install_retry_starts_again_from_the_committed_tree() {
    let t = Temp::new();
    let (committed, tree) = (t.0.join("input"), t.0.join("workspace"));
    for (p, text) in [("package.json", "{}"), ("src/a.js", "export const a = 1;\n"), ("test/fixtures/node_modules/resolve/index.js", "module.exports = 1;\n"), ("tools/cli/package.json", "{}")] { write(&committed, p, text); }
    for (p, text) in [("package.json", "{}"), ("src/a.js", "rewritten by a failed script\n"), ("node_modules/electron/dist/half", "x"), ("dist/out.js", "built by a failed prepare script\n"),
        ("packages/app/node_modules/x/index.js", "x"), (".kern-install/npm/cache", "kept"), (".kern-pnpm-store/v3/files/00/x", "kept"), ("tools/cli/package.json", "{}"), ("tools/cli/node_modules/y/index.js", "installed before")] { write(&tree, p, text); }
    kern::sandbox::restore(&committed, &tree, &[tree.join(".kern-install"), tree.join(".kern-pnpm-store"), tree.join("tools/cli")]).unwrap();
    assert!(!tree.join("node_modules").exists() && !tree.join("packages/app/node_modules").exists(), "Comfy-Desktop#1732: a failed scripted install left Electron half-installed, and the next attempt reused it");
    assert!(!tree.join("dist").exists(), "files a failed script wrote outside node_modules are gone too");
    assert_eq!(std::fs::read_to_string(tree.join("src/a.js")).unwrap(), "export const a = 1;\n");
    assert!(tree.join("test/fixtures/node_modules/resolve/index.js").is_file(), "a node_modules folder the project commits is part of the committed tree");
    assert!(tree.join(".kern-install/npm/cache").is_file() && tree.join(".kern-pnpm-store/v3/files/00/x").is_file() && tree.join("tools/cli/node_modules/y/index.js").is_file(),
        "Kern's own caches and a package installed before are kept");
}

#[test]
fn a_package_counts_as_installed_with_the_workspace_only_when_the_workspace_install_worked() {
    let root = |result: &str| vec![json!({ "dir": ".", "manager": "npm", "result": result })];
    assert!(kern::sandbox::with_workspace("packages/app", true, &root("installed")));
    assert!(!kern::sandbox::with_workspace("packages/app", true, &root("failed: npm error code E404")), "the root install failed and left half a node_modules in the package, so its tests would run on a broken install");
    assert!(!kern::sandbox::with_workspace("packages/app", false, &root("installed")));
    assert!(!kern::sandbox::with_workspace(".", true, &root("installed")));
    assert!(!kern::sandbox::with_workspace("packages/app", true, &[]), "no workspace install ran, so a node_modules folder there is not one");
}
