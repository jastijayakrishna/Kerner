use kern::receipt;
use kern::types::{added, diagnostics, enforced, is_ts, jsonc, judge, projects, side};
use serde_json::json;
use std::collections::BTreeSet;

fn set(items: &[&str]) -> BTreeSet<String> { items.iter().map(|s| s.to_string()).collect() }

#[test]
fn tsc_errors_are_read_with_their_file_line_code_and_message() {
    let out = "kern-types: typescript 5.6.3\nsrc/a.ts(12,5): error TS2339: Property 'foo' does not exist on type 'Bar'.\n  Did you mean 'for'?\n../shared/b.ts(3,1): error TS2304: Cannot find name 'x'.\nerror TS5083: Cannot read file '/workspace/tsconfig.base.json'.\nkern-types: exit 2 \n";
    let found = diagnostics(out, "packages/app");
    assert_eq!(found.len(), 3, "{found:?}");
    assert_eq!((found[0].file.as_str(), found[0].line, found[0].code.as_str(), found[0].message.as_str()), ("packages/app/src/a.ts", 12, "TS2339", "Property 'foo' does not exist on type 'Bar'."));
    assert_eq!(found[1].file, "packages/shared/b.ts", "paths are made relative to the repository");
    assert_eq!((found[2].file.as_str(), found[2].code.as_str()), ("", "TS5083"));
}

#[test]
fn a_side_is_clean_only_when_tsc_finished_with_no_error() {
    assert_eq!(side("kern-types: typescript 5.6.3\n\nkern-types: exit 0 \n", "")["status"], "clean");
    let errors = side("kern-types: typescript 5.6.3\nsrc/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\nkern-types: exit 2 \n", "");
    assert_eq!((errors["status"].as_str(), errors["errors"].as_u64()), (Some("errors"), Some(1)));
    assert_eq!(side("kern-types: typescript 5.6.3\n<--- Last few GCs --->\nkern-types: exit null SIGABRT\n", "")["status"], "unknown", "a tsc that ran out of memory proves nothing");
    assert_eq!(side("kern-types: typescript 5.6.3\nError: something odd\nkern-types: exit 1 \n", "")["status"], "unknown", "a failure with no error Kern can read proves nothing");
    assert_eq!(side("kern-types: typescript 5.6.3\nsrc/a.ts(1,7): error TS2322: x\n", "")["status"], "unknown", "a run that did not finish proves nothing");
    assert_eq!(side("kern-types: no typescript\n", "")["status"], "unknown");
}

#[test]
fn new_errors_are_counted_like_a_bag_so_moved_lines_do_not_count() {
    let old = diagnostics("a.ts(1,1): error TS2304: Cannot find name 'x'.\na.ts(9,1): error TS2304: Cannot find name 'x'.\n", "");
    let new = diagnostics("a.ts(3,1): error TS2304: Cannot find name 'x'.\na.ts(11,1): error TS2304: Cannot find name 'x'.\na.ts(12,1): error TS2304: Cannot find name 'x'.\nb.ts(1,1): error TS2322: y\n", "");
    let fresh = added(&old, &new);
    assert_eq!(fresh.iter().map(|d| (d.file.as_str(), d.code.as_str())).collect::<Vec<_>>(), [("a.ts", "TS2304"), ("b.ts", "TS2322")]);
}

#[test]
fn tsconfig_files_with_comments_and_trailing_commas_are_read() {
    let v = jsonc("{\n  // the app\n  \"compilerOptions\": { \"strict\": true, /* yes */ \"paths\": { \"@/*\": [\"./src/*\"], }, },\n  \"include\": [\"src\", \"a//b\"],\n}\n").unwrap();
    assert_eq!(v["compilerOptions"]["strict"], true);
    assert_eq!(v["include"][1], "a//b", "slashes inside strings are not comments");
}

#[test]
fn each_changed_typescript_file_is_checked_with_its_nearest_tsconfig_that_both_sides_have() {
    let head = set(&["tsconfig.json", "packages/app/tsconfig.json", "packages/new/tsconfig.json", "web/tsconfig.json", "web/tsconfig.app.json", "web/tsconfig.node.json"]);
    let base = set(&["tsconfig.json", "packages/app/tsconfig.json", "web/tsconfig.json", "web/tsconfig.app.json", "web/tsconfig.node.json"]);
    let read = |p: &str| match p {
        "web/tsconfig.json" => Some("{ \"files\": [], \"references\": [{ \"path\": \"./tsconfig.app.json\" }, { \"path\": \"./tsconfig.node.json\" }] }".to_string()),
        _ => Some("{}".to_string()),
    };
    assert_eq!(projects(&["packages/app/src/a.ts", "packages/app/src/b.tsx", "scripts/x.ts", "README.md"], &head, &base, read), ["packages/app/tsconfig.json", "tsconfig.json"]);
    assert!(projects(&["packages/new/src/a.ts"], &head, &base, read).is_empty(), "a project the old code does not have is not compared");
    assert_eq!(projects(&["web/src/main.ts"], &head, &base, read), ["web/tsconfig.app.json", "web/tsconfig.node.json"], "a solution tsconfig is checked through the projects it names");
    assert!(projects(&["src/a.js", "node_modules/x/index.ts"], &head, &base, read).is_empty());
    assert!(is_ts("a.d.ts") && is_ts("a.mts") && !is_ts("a.js"));
}

#[test]
fn kern_blocks_on_types_only_where_the_projects_own_scripts_run_tsc() {
    assert!(enforced(&[json!({ "build": "tsc -p tsconfig.build.json && tsc-alias" })]));
    assert!(enforced(&[json!({ "typecheck": "vue-tsc --noEmit" })]));
    assert!(enforced(&[json!({ "check": "node ./node_modules/.bin/tsc --noEmit" })]));
    assert!(!enforced(&[json!({ "build": "tsup src/index.ts", "fix": "tsc-alias" }), json!(null)]));
}

#[test]
fn errors_on_a_project_that_was_clean_block_when_the_project_runs_tsc_and_need_a_look_otherwise() {
    let clean = json!({ "status": "clean", "version": "5.6.3" });
    let broken = side("kern-types: typescript 5.6.3\nsrc/total.ts(4,3): error TS2322: Type 'string' is not assignable to type 'number'.\nkern-types: exit 2 \n", "");
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = |projects: serde_json::Value| json!({ "status": "NOT_RUN", "why": "no test runner", "types": { "status": "CHECKED", "projects": projects } });
    let off = json!({ "status": "OFF", "why": "no AI key" });

    let enforced_run = judge("tsconfig.json", &clean, &broken, true, None);
    let r = receipt::build(&d, &rules, &tests(json!([enforced_run])), &off);
    assert_eq!(r["decision"], "blocked", "{r}");
    assert!(r["blocked"][0]["what"].as_str().unwrap().contains("src/total.ts:4 TS2322"), "{r}");
    assert!(receipt::markdown(&r, None).contains("1 new TypeScript error in `tsconfig.json`, which had none on the old code"));

    let loose = receipt::build(&d, &rules, &tests(json!([judge("tsconfig.json", &clean, &broken, false, None)])), &off);
    assert_eq!(loose["decision"], "look");
    assert!(loose["look"].as_array().into_iter().flatten().any(|x| x["kind"] == "types-new"));
    let uneven = receipt::build(&d, &rules, &tests(json!([judge("tsconfig.json", &clean, &broken, true, Some("the project has no lockfile".into()))])), &off);
    assert_eq!(uneven["decision"], "look", "{uneven}");

    let dirty = json!({ "status": "errors", "errors": 7, "list": [] });
    let not = judge("tsconfig.json", &dirty, &broken, true, None);
    assert_eq!(not["result"], "not-compared");
    let r = receipt::build(&d, &rules, &tests(json!([not])), &off);
    assert_ne!(r["decision"], "blocked");
    assert!(receipt::markdown(&r, None).contains("not compared — it already had 7 TypeScript errors here on the old code"));
    let same = receipt::build(&d, &rules, &tests(json!([judge("tsconfig.json", &clean, &clean, true, None)])), &off);
    assert!(receipt::markdown(&same, None).contains("no new TypeScript errors in `tsconfig.json` (tsc on the old and the new code, TypeScript 5.6.3)"));
}

#[test]
fn the_type_check_asks_the_sandbox_only_for_what_a_test_run_may_have() {
    let spec = kern::types::spec("packages/app/tsconfig.json", 420, 4096);
    let command = kern::util::argv(&spec["command"]).expect("the sandbox accepts the command");
    assert_eq!(command[3..], ["/workspace/packages/app/tsconfig.json".to_string(), "2867".to_string()], "tsc gets 70% of the memory the container has, not of the machine");
    assert!(kern::types::CHECK.contains("process.argv[2]") && !kern::types::CHECK.contains("totalmem"));
    assert!(spec["env"].as_object().unwrap().keys().all(|k| kern::script::env_name(k)), "the sandbox refuses variables it reserves, such as KERN_*: {spec}");
    assert_eq!(kern::sandbox::test_dir(spec["cwd"].as_str().unwrap()).unwrap(), "/workspace/packages/app");
    assert_eq!(kern::sandbox::test_dir(kern::types::spec("tsconfig.json", 420, 4096)["cwd"].as_str().unwrap()).unwrap(), "/workspace");
}

#[test]
fn a_new_error_about_a_module_or_an_export_needs_a_look_because_a_build_step_may_generate_it() {
    let clean = json!({ "status": "clean", "version": "5.6.3" });
    let generated = side("kern-types: typescript 5.6.3\napps/web/jest.integration.setup.ts(1,10): error TS2305: Module '\"@repo/database\"' has no exported member 'PrismaClient'.\nkern-types: exit 2 \n", "");
    let p = judge("apps/web/tsconfig.json", &clean, &generated, true, None);
    assert_eq!(p["generated"], true);
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let r = receipt::build(&d, &rules, &json!({ "status": "NOT_RUN", "why": "x", "types": { "status": "CHECKED", "projects": [p] } }), &json!({ "status": "OFF", "why": "no AI key" }));
    assert_eq!(r["decision"], "look", "tuliren/turbo-template#29 imported PrismaClient, which prisma generate makes in the project's CI and Kern never ran: blocking it was wrong: {r}");
    assert!(r["look"][0]["what"].as_str().unwrap().contains("a build step may generate"));
}
