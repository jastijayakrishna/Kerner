use serde_json::{Value, json};
use std::collections::BTreeMap;

fn manifests(list: &[(&str, &str)]) -> BTreeMap<String, Value> {
    list.iter().map(|(dir, build)| (dir.to_string(), if build.is_empty() { json!({ "name": "p" }) } else { json!({ "name": "p", "scripts": { "build": build } }) })).collect()
}
fn planned(changed: &[&str], head: &BTreeMap<String, Value>, base: &BTreeMap<String, Value>) -> Vec<String> {
    kern::build::planned(changed, &|d: &str| head.get(d).cloned(), &|d: &str| base.get(d).cloned())
}

#[test]
fn pivota_agent_ui_301_the_build_nearest_the_changed_code_is_the_one_kern_runs() {
    let both = manifests(&[("", "turbo run build"), ("apps/web", "next build"), ("packages/ui", "tsup src/index.ts"), ("packages/core", "")]);
    assert_eq!(planned(&["apps/web/src/app/page.tsx", "apps/web/README.md", "docs/guide.md"], &both, &both), ["apps/web"], "documentation never needs a build");
    assert_eq!(planned(&["packages/ui/src/button.test.tsx"], &both, &both), Vec::<String>::new(), "a change to tests alone is checked by the tests");
    assert_eq!(planned(&["packages/core/src/a.ts"], &both, &both), [""], "a package without its own build is built by the nearest folder that has one");
    let changed = ["apps/web/src/a.tsx", "apps/web/src/b.tsx", "packages/ui/src/c.ts", "packages/core/src/d.ts", "packages/core/src/e.ts", "packages/core/src/f.ts"];
    assert_eq!(planned(&changed, &both, &both), ["", "apps/web"], "at most two builds, the ones with the most changed files first");
    let added = manifests(&[("", ""), ("apps/web", "")]);
    let now = manifests(&[("", ""), ("apps/web", "next build")]);
    assert_eq!(planned(&["apps/web/src/app/page.tsx"], &now, &added), Vec::<String>::new(), "a build script the change adds has nothing on the old code to compare with");
    let placeholder = manifests(&[("", "echo \"no build step\"")]);
    assert_eq!(planned(&["src/index.js"], &placeholder, &placeholder), Vec::<String>::new(), "a build script that only prints builds nothing");
    let lib = manifests(&[("", "tsc -p tsconfig.build.json")]);
    assert_eq!(planned(&["src/index.ts", "src/styles.css"], &lib, &lib), [""]);
}

const OK: &str = "\n> web@1.0.0 build\n> next build\n\n   ▲ Next.js 15.5.4\n ✓ Compiled successfully in 6.2s\n ✓ Generating static pages (12/12)\n\nkern-build: exit 0 \n";
const BROKEN: &str = "\n> web@1.0.0 build\n> next build\n\nFailed to compile.\n\n./src/app/billing/page.tsx\nModule not found: Can't resolve '@/lib/stripe-server'\n\n> Build failed because of webpack errors\nnpm error Lifecycle script `build` failed with error:\nnpm error code 1\nnpm error path /workspace/apps/web\nnpm error workspace web@1.0.0\nnpm error location /workspace/apps/web\nnpm error command failed\nnpm error command sh -c next build\nkern-build: exit 1 \n";

#[test]
fn planyr_1262_a_build_that_breaks_twice_on_the_new_code_is_a_look_that_quotes_its_error() {
    let (old, new) = (kern::build::side(OK), kern::build::side(BROKEN));
    assert_eq!(old["status"], "built", "{old}");
    assert_eq!(new["status"], "failed", "{new}");
    let errors: Vec<&str> = new["errors"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(errors.contains(&"Module not found: Can't resolve '@/lib/stripe-server'") && errors.contains(&"Failed to compile."), "{errors:?}");
    assert!(!errors.iter().any(|e| e.starts_with("npm error")), "npm's own lines say nothing about the change: {errors:?}");
    let j = kern::build::judge("apps/web", &old, &new, Some(&kern::build::side(BROKEN)), None);
    assert_eq!(j["result"], "broke", "{j}");
    let r = receipt(json!({ "status": "CHECKED", "builds": [j] }));
    assert_eq!(r["decision"], "look", "a build that broke is something to look at; whether it must stop the merge is the project's call: {r}");
    let look = r["look"].as_array().unwrap().iter().find(|x| x["kind"] == "build-broke").expect("a build-broke look");
    let what = look["what"].as_str().unwrap();
    assert!(what.contains("apps/web") && what.contains("stripe-server") && what.contains("old code"), "{what}");
    let md = kern::receipt::markdown(&r);
    assert!(md.contains("⚠️ Build:") && md.contains("apps/web"), "{md}");
}

fn side_of(kind: &str) -> Option<Value> {
    let failed = |line: &str| format!("> x build\n{line}\nkern-build: exit 1 \n");
    let output = match kind {
        "built" => OK.to_string(),
        "failed-a" => failed("Error: Cannot find module './gone'"),
        "failed-b" => failed("SyntaxError: Unexpected token '}'"),
        "network" => failed("Error: getaddrinfo ENOTFOUND fonts.googleapis.com"),
        "memory" => "> x build\nFATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\nkern-build: exit null SIGABRT\n".to_string(),
        "stopped" => "> x build\ncompiling...".to_string(),
        _ => return None,
    };
    Some(kern::build::side(&output))
}

#[test]
fn inflect_compliance_1893_every_combination_of_build_runs_breaks_only_with_two_matching_failures_after_a_clean_old_build() {
    let kinds = ["built", "failed-a", "failed-b", "network", "memory", "stopped", "none"];
    let mut broke = 0;
    for old in kinds.iter().filter(|k| **k != "none") {
        for new in kinds.iter().filter(|k| **k != "none") {
            for again in kinds {
                for even in [None, Some("the old code's dependencies were installed and the new code's were installed from package.json".to_string())] {
                    let (o, n, a) = (side_of(old).unwrap(), side_of(new).unwrap(), side_of(again));
                    let j = kern::build::judge(".", &o, &n, a.as_ref(), even.clone());
                    let same_failure = *old == "built" && new.starts_with("failed") && again == *new;
                    let expected = match (*old, *new) {
                        ("built", "built") => "same",
                        _ if same_failure => "broke",
                        ("built", n) if n.starts_with("failed") && (again == "built" || (again.starts_with("failed") && again != *new)) => "unsteady",
                        _ => "not-compared",
                    };
                    assert_eq!(j["result"], expected, "old {old}, new {new}, again {again}, even {even:?}: {j}");
                    assert_eq!(j["uneven"].is_string(), expected == "broke" && even.is_some(), "old {old}, new {new}, again {again}: {j}");
                    if expected == "broke" { broke += 1; }
                    if expected == "not-compared" { assert!(j["why"].as_str().is_some_and(|w| !w.is_empty()), "every build Kern did not compare says why: {j}"); }
                }
            }
        }
    }
    assert_eq!(broke, 4, "two kinds of real failure, each with and without even setup");
}

#[test]
fn smart_cart_432_a_build_that_needs_the_network_or_more_memory_is_kerns_limit_not_the_changes_fault() {
    assert_eq!(side_of("network").unwrap()["status"], "sandbox");
    assert_eq!(side_of("memory").unwrap()["status"], "sandbox");
    assert_eq!(side_of("stopped").unwrap()["status"], "unknown", "a build with no ending did not finish");
    let killed = kern::build::side("> x build\nkern-build: exit null SIGKILL\n");
    assert_eq!(killed["status"], "unknown", "{killed}");
    let j = kern::build::judge(".", &side_of("built").unwrap(), &side_of("network").unwrap(), None, None);
    assert_eq!(j["result"], "not-compared");
    assert!(j["why"].as_str().unwrap().contains("ENOTFOUND"), "{j}");
    let r = receipt(json!({ "status": "CHECKED", "builds": [j] }));
    assert_eq!(r["decision"], "clear", "Kern's own limit is never the change's fault: {r}");
    assert!(kern::receipt::markdown(&r).contains("⏭️ Build:"));
}

#[test]
fn gov_bb_1970_each_build_result_has_its_own_receipt_line() {
    let same = kern::build::judge(".", &side_of("built").unwrap(), &side_of("built").unwrap(), None, None);
    let md = kern::receipt::markdown(&receipt(json!({ "status": "CHECKED", "builds": [same] })));
    assert!(md.contains("✅ Build: `npm run build` works on the old and the new code"), "{md}");
    let old_broken = kern::build::judge("packages/api", &side_of("failed-a").unwrap(), &json!({ "status": "skipped" }), None, None);
    let md = kern::receipt::markdown(&receipt(json!({ "status": "CHECKED", "builds": [old_broken] })));
    assert!(md.contains("⏭️ Build: `npm run build` in `packages/api` not compared") && md.contains("old code"), "{md}");
    let late = receipt(json!({ "status": "NOT_RUN", "why": "Kern's time ran out before it could build the project" }));
    assert!(kern::receipt::markdown(&late).contains("⏭️ Build: not checked — Kern's time ran out"));
    let uneven = kern::build::judge(".", &side_of("built").unwrap(), &side_of("failed-a").unwrap(), side_of("failed-a").as_ref(), Some("the project has no lockfile".into()));
    let r = receipt(json!({ "status": "CHECKED", "builds": [uneven] }));
    assert_eq!(r["decision"], "unverified", "{r}");
    assert!(r["unverified"].as_array().unwrap().iter().any(|x| x["kind"] == "build-uneven"), "{r}");
    assert!(kern::receipt::markdown(&receipt(json!({ "status": "NONE" }))).lines().all(|l| !l.contains("Build:")), "no build, no line");
}

#[test]
fn a_build_runs_the_projects_own_script_with_a_heap_that_fits_and_reports_its_exit() {
    let spec = kern::build::spec("apps/web", 300, 8192);
    assert_eq!(spec["cwd"], "apps/web");
    let command: Vec<&str> = spec["command"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(command[0], "/usr/local/bin/node");
    assert!(command[2].contains("'run','build'") && command[2].contains("kern-build: exit"), "{}", command[2]);
    assert_eq!(&command[3..], ["6144", "285"], "three quarters of the box's memory for the heap, and the build stops before the sandbox does");
}

fn receipt(builds: Value) -> Value {
    let d = json!({ "base": "1".repeat(40), "head": "2".repeat(40), "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 4, "failed": 0, "skipped": 0 }, "after": { "passed": 4, "failed": 0, "skipped": 0 },
        "kept": 4, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "builds": builds });
    kern::receipt::build(&d, &rules, &tests)
}
