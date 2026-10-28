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
fn posthog_4869_the_pnpm_store_stays_for_the_tests_and_only_its_links_back_into_the_project_go() {
    let inside = |p: &str| p.starts_with("/workspace/") && !p.starts_with("/workspace/node_modules/");
    let scratch = |p: &str| kern::sandbox::INSTALL_SCRATCH.iter().any(|s| p == *s || p.starts_with(&format!("{s}/")));
    for (key, value) in kern::sandbox::INSTALL_ENV.iter().filter(|(_, v)| inside(v)) {
        assert!(scratch(value), "{key}={value} sits inside the project, where its test runner can find it, and is not removed after install");
    }
    assert!(!scratch(kern::sandbox::PNPM_STORE), "PostHog/posthog-js#4869: pnpm's global virtual store keeps vitest itself in the store, and node_modules links into it, so the store must outlive the install");
    let t = Temp::new();
    write(&t.0, "package.json", "{\"name\":\"p\",\"packageManager\":\"pnpm@10.18.0\"}");
    write(&t.0, "pnpm-lock.yaml", "lockfileVersion: '9.0'\n");
    let (_, attempts) = kern::sandbox::install_plan(&t.0);
    let stores: Vec<&str> = attempts.iter().flat_map(|(argv, _)| argv.iter()).filter_map(|a| a.strip_prefix("--store-dir=")).collect();
    assert!(!stores.is_empty() && stores.iter().all(|s| *s == kern::sandbox::PNPM_STORE), "{stores:?}");
    let store = Temp::new();
    for p in ["v10/projects/c52ddf65/packages/components/src/__tests__/cli.test.ts", "v10/files/00/ab", "v10/index/aa",
        "v11/links/@/vitest/1.6.1/aeab4f17/node_modules/vitest/vitest.mjs", "v11/projects/9f/a.test.ts"] { write(&store.0, p, "x"); }
    let mut gone = kern::sandbox::store_links_back(&store.0);
    gone.sort();
    assert_eq!(gone, [store.0.join("v10").join("projects"), store.0.join("v11").join("projects")],
        "langchain-ai/macaw-design-system#32: pnpm keeps a link from its store back to the project, so vitest ran every test a second time through it; the packages in links/ stay");
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

#[test]
fn review_stage1_r5_kerns_tool_folders_are_made_afresh_so_nothing_left_in_them_is_reused() {
    let t = Temp::new();
    let tools = t.0.join(".kern-tools");
    write(&tools, "bin/node", "#!/bin/sh\nexit 0\n");
    write(&tools, "bun/node_modules/@oven/bun-linux-x64-baseline/bin/bun", "#!/bin/sh\nexit 0\n");
    write(&tools, "scip/.npmrc", "registry=https://evil.example\n");
    std::fs::write(tools.join("corepack-home"), "a file where Kern expects its folder").unwrap();
    for dir in ["bin", "bun", "scip", "corepack-home", "corepack"] {
        kern::sandbox::remake_dir(&tools.join(dir)).unwrap();
        assert!(tools.join(dir).is_dir() && std::fs::read_dir(tools.join(dir)).unwrap().next().is_none(), "{dir}: a planted node, Bun or .npmrc would run as Kern's own");
    }
    kern::sandbox::remake_dir(&t.0.join("made/inside/a/new/folder")).unwrap();
    assert!(t.0.join("made/inside/a/new/folder").is_dir());
    #[cfg(unix)]
    {
        std::fs::create_dir_all(t.0.join("elsewhere")).unwrap();
        write(&t.0, "elsewhere/keep", "x");
        std::os::unix::fs::symlink(t.0.join("elsewhere"), tools.join("linked")).unwrap();
        kern::sandbox::remake_dir(&tools.join("linked")).unwrap();
        assert!(tools.join("linked").symlink_metadata().unwrap().is_dir() && t.0.join("elsewhere/keep").is_file(), "a link is replaced, never followed");
    }
}

#[test]
fn review_stage1_r6_kern_reads_at_most_what_it_keeps_of_a_file_the_tests_wrote_and_never_follows_a_link_or_waits_on_a_pipe() {
    let t = Temp::new();
    let report = t.0.join("report.json");
    std::fs::write(&report, b"0123456789").unwrap();
    assert_eq!(kern::sandbox::some_of(&report, 4, false), b"0123");
    assert_eq!(kern::sandbox::some_of(&report, 4, true), b"6789", "the end of a long output is what Kern keeps of it");
    assert_eq!(kern::sandbox::some_of(&report, 64, true), b"0123456789");
    assert!(kern::sandbox::some_of(&t.0, 4, false).is_empty() && kern::sandbox::some_of(t.0.join("missing"), 4, false).is_empty(), "a folder or a missing file gives nothing");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/dev/zero", t.0.join("zero")).unwrap();
        assert!(kern::sandbox::some_of(t.0.join("zero"), 4, false).is_empty(), "a test that links Kern's report file to /dev/zero gets nothing read, not an endless read");
        std::os::unix::fs::symlink(&report, t.0.join("linked")).unwrap();
        assert!(kern::sandbox::some_of(t.0.join("linked"), 4, false).is_empty(), "a link is never followed, even to a real file");
        let made = std::process::Command::new("mkfifo").arg(t.0.join("pipe")).status().is_ok_and(|s| s.success());
        assert!(!made || kern::sandbox::some_of(t.0.join("pipe"), 4, false).is_empty(), "a pipe in its place is not waited on");
    }
}
