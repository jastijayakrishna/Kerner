use kern::policy::Runner;
use kern::probe::{self, Answers, Authority, Candidate, Check, Data, Evidence, HUNG, Missing, Parsed, Requirement, Style, Unchecked, Verdict};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const SQL: &str = "src/sql.ts";
const DOLLARS: &str = "SELECT $$https://example.com/a//b$$ AS address";
const KEPT: &str = "returns \"SELECT $$https://example.com/a//b$$ AS address\"";
const CUT: &str = "returns \"SELECT $$https:\"";
const BASE: &str = "1111111111111111111111111111111111111111";
const HEAD: &str = "2222222222222222222222222222222222222222";
const STATEMENT: &str = "Removing comments keeps the text inside string literals unchanged.";
const QUOTE: &str = "it('keeps -- inside quoted strings'";
const CITED: &str = "The requirement: Removing comments keeps the text inside string literals unchanged (`src/sql.test.ts`: `it('keeps -- inside quoted strings'`)";

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-probe-{}", kern::util::uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

fn failed(a: &str) -> bool { a.starts_with("throws ") || a.starts_with("rejects ") }
fn succeeded(a: &str) -> bool { a.starts_with("returns ") || a.starts_with("resolves ") }

#[test]
fn lightdash_12942_every_combination_of_probe_evidence_keeps_the_kernels_promises() {
    let answers = [None, Some(CUT), Some(KEPT), Some(HUNG), Some("throws TypeError: x is not iterable"), Some("rejects Error: no rows"), Some("rejects Error: getaddrinfo ENOTFOUND db.internal")];
    let mut requirements = vec![None];
    for check in [Check::Same, Check::Returns, Check::Throws] {
        for authority in [Authority::Verified, Authority::Contested, Authority::Authorized, Authority::Unverified] { requirements.push(Some(Requirement { check, authority })); }
    }
    let n = answers.len();
    let (mut blocked, mut combinations) = (0, 0);
    for requirement in requirements {
        for bits in 0..128u8 {
            let (shape_seen, reached_old, reached_new, uneven) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let (chance, network, retested) = (bits & 16 != 0, bits & 32 != 0, bits & 64 != 0);
            for data in [Data::Same, Data::Extended, Data::Differs, Data::Missing] {
                for code in 0..n.pow(4) {
                    let pick = |i: u32| answers[code / n.pow(i) % n];
                    let (old, old_again, new, new_again) = (pick(0), pick(1), pick(2), pick(3));
                    let e = Evidence { requirement, shape_seen, reached_old, reached_new, uneven, data, chance, network, retested,
                        answers: Answers { old: old.map(str::to_string), old_again: old_again.map(str::to_string), new: new.map(str::to_string), new_again: new_again.map(str::to_string) } };
                    let v = probe::verdict(&e);
                    combinations += 1;
                    let four = [old, old_again, new, new_again];
                    let sandbox = four.into_iter().flatten().any(|a| failed(a) && a.contains("getaddrinfo"));
                    let hung = four.into_iter().flatten().any(|a| a == HUNG);
                    let authority = requirement.map(|r| r.authority);
                    let usable = authority.is_some_and(|a| a != Authority::Unverified);
                    let crash = |a: &str| failed(a) && kern::proof::crashed(a);
                    let broken: Option<bool> = match (requirement, old, new) {
                        (Some(r), Some(o), Some(w)) if o != w => match r.check {
                            Check::Same if succeeded(o) && succeeded(w) => match data { Data::Differs => Some(true), Data::Missing => None, _ => Some(false) },
                            Check::Same if crash(o) && succeeded(w) => Some(false),
                            Check::Same => Some(succeeded(o) != succeeded(w)),
                            Check::Returns => Some(succeeded(o) && failed(w)),
                            Check::Throws => Some(failed(o) && succeeded(w)),
                        },
                        _ => Some(false),
                    };
                    let said = || format!("{e:?} gave {v:?}");
                    if sandbox { assert!(matches!(v, Verdict::Unchecked(_)), "a sandbox error never gives anything but not checked: {}", said()); }
                    if !usable { assert!(!matches!(v, Verdict::Blocked | Verdict::Look(_) | Verdict::Uneven), "no verified requirement, no block and no look: {}", said()); }
                    if !(reached_old && reached_new) { assert_eq!(v, Verdict::Unchecked(Unchecked::Unreached), "{}", said()); }
                    match v {
                        Verdict::Blocked => {
                            blocked += 1;
                            assert!(authority == Some(Authority::Verified) && shape_seen && reached_old && reached_new && !uneven && !sandbox && !hung && !chance && !network && !retested && broken == Some(true), "{}", said());
                            assert!(old.is_some() && new.is_some() && old_again == old && new_again == new && old != new, "a block needs four answers, both reruns agreeing: {}", said());
                            assert!(requirement.is_some_and(|r| r.check != Check::Same), "a requirement that the answer must not change never blocks: which inputs it covers is the AI's reading, not evidence: {}", said());
                        }
                        Verdict::Look(m) => {
                            assert!(usable && broken != Some(false) && !uneven && !sandbox && !hung && !chance && !network && m != Missing::default(), "{}", said());
                            assert_eq!(m, Missing { contested: authority == Some(Authority::Contested), authorized: authority == Some(Authority::Authorized), shape: !shape_seen,
                                rerun: old_again.is_none() || new_again.is_none(), values: broken.is_none(), retested, scope: requirement.is_some_and(|r| r.check == Check::Same) }, "a look names exactly what is missing for a block: {}", said());
                        }
                        Verdict::Uneven => assert!(usable && broken != Some(false) && uneven && !sandbox && !hung && !chance && !network, "{}", said()),
                        Verdict::Same => assert!(reached_old && reached_new && old.is_some() && old == new && !sandbox && !hung, "{}", said()),
                        Verdict::Note => assert!(old.is_some() && new.is_some() && old != new && !sandbox && !hung && !chance && !network && (!usable || broken == Some(false)), "{}", said()),
                        Verdict::Unchecked(Unchecked::Unanswered) => assert!(old.is_none() || new.is_none(), "{}", said()),
                        Verdict::Unchecked(Unchecked::Sandbox) => assert!(sandbox || (network && old != new && !hung), "{}", said()),
                        Verdict::Unchecked(Unchecked::Hung) => assert!(hung && !sandbox, "{}", said()),
                        Verdict::Unchecked(Unchecked::Chance) => assert!(old != new && (chance || (old_again.is_some() && old_again != old) || (new_again.is_some() && new_again != new)), "{}", said()),
                        Verdict::Unchecked(Unchecked::Unreached) => assert!(!(reached_old && reached_new), "{}", said()),
                    }
                }
            }
        }
    }
    assert_eq!(combinations, 13 * 128 * 4 * 7usize.pow(4));
    assert_eq!(blocked, 32, "four plain answers give 12 changed pairs: `returns` blocks when a value became an error and `throws` when an error became a value, 4 pairs each, whatever the data (16 each); \
        `same` never blocks, and nothing else may");
}

#[test]
fn meshcore_ts_15_one_function_breaking_one_requirement_is_one_item_however_many_inputs_show_it() {
    let shapeless = |id: &str, arg: &str| { let mut x = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified")); x["id"] = json!(id); x["args"] = json!([arg]); x["shapes"] = json!([]); x };
    let unlinked = |id: &str| { let mut x = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], Value::Null); x["id"] = json!(id); x };
    let r = receipt(ran(vec![shapeless("e1", "a"), shapeless("e2", "b"), shapeless("e3", "c"), unlinked("e4"), unlinked("e5")], json!([])));
    let of = |level: &str, kind: &str| -> Vec<String> { r[level].as_array().unwrap().iter().filter(|x| x["kind"] == kind).map(|x| x["what"].as_str().unwrap().to_string()).collect() };
    let looks = of("look", "probe-concern");
    assert_eq!(looks.len(), 1, "five looks for one decoder and one requirement teach people to skim: {looks:?}");
    assert!(looks[0].contains("removeComments(\"a\")") && looks[0].contains("2 more inputs show the same"), "{}", looks[0]);
    let notes = of("notes", "probe-difference");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("1 more input shows the same"), "{}", notes[0]);
}

#[test]
fn lightdash_12942_an_answer_that_keeps_every_old_value_and_only_adds_fields_breaks_no_same_requirement() {
    let line = |o: &str, v: Option<&str>| { let mut r = json!({ "p": "e1", "o": o, "k": "src/sql.ts#removeComments" }); if let Some(v) = v { r["v"] = json!(v); } format!("{}\n{}\n", json!({ "p": "e1", "s": 1 }), r) };
    let judged = |old: (&str, Option<&str>), new: (&str, Option<&str>)| {
        let traces = [line(old.0, old.1), line(old.0, old.1), line(new.0, new.1), line(new.0, new.1)].map(|t| Some(probe::seen(&t)));
        let mut x = tried([None; 4], obligation("verified"));
        probe::recorded(&mut x, &traces);
        (probe::verdict(&probe::evidence(&x)), x["data"].clone())
    };
    let was = ("returns {\"rows\":[1,2],\"meta\":{\"total\":2}}", Some(r#"{"rows":[1,2],"meta":{"total":2}}"#));
    let scope = Verdict::Look(Missing { scope: true, ..Missing::default() });
    assert_eq!(judged(was, ("returns {\"cursor\":null,\"meta\":{\"page\":1,\"total\":2},\"rows\":[1,2]}", Some(r#"{"rows":[1,2],"meta":{"total":2,"page":1},"cursor":null}"#))), (Verdict::Note, json!("extended")),
        "a new field, even inside a nested object, adds to the answer and keeps everything a caller relied on");
    assert_eq!(judged(was, ("returns {\"meta\":{\"total\":3},\"rows\":[1,2]}", Some(r#"{"rows":[1,2],"meta":{"total":3}}"#))), (scope, json!("differs")), "an old value that changed breaks the requirement, and a `same` requirement is a look at most");
    assert_eq!(judged(was, ("returns {\"rows\":[1,2]}", Some(r#"{"rows":[1,2]}"#))), (scope, json!("differs")), "a field that disappeared is a break");
    assert_eq!(judged(was, ("returns {\"meta\":{\"total\":2},\"rows\":[1,2,3]}", Some(r#"{"rows":[1,2,3],"meta":{"total":2}}"#))), (scope, json!("differs")), "a list with another item is a different answer");
    assert_eq!(judged(was, ("returns Map{}", None)), (Verdict::Look(Missing { values: true, scope: true, ..Missing::default() }), json!("missing")),
        "without a value Kern can read on both sides, a changed answer is a look that says so, never a block");
    let renamed = ("returns {\"get\":[function request]}", Some(r#"{}"#));
    assert_eq!(judged(("returns {\"get\":[function get]}", Some(r#"{}"#)), renamed), (Verdict::Note, json!("same")), "a function renamed inside the answer prints differently but is the same data: a note, never a break");
    assert_eq!(judged(("throws TypeError: Cannot read properties of undefined (reading 'kind')", None), ("returns []", Some("[]"))), (Verdict::Note, json!("missing")),
        "KerfDesk#566: the old code crashed on an entity kind it did not know and the new code handles it; a crash is no answer a requirement can keep");
}

fn obligation(authority: &str) -> Value {
    json!({ "id": "o1", "statement": STATEMENT, "check": "same", "targets": [{ "path": SQL, "export": "removeComments" }],
        "source": { "path": "src/sql.test.ts", "quote": QUOTE, "kind": "test" }, "authority": authority, "why": "",
        "authorized_by": if authority == "authorized" { "Strip // comments too, as MySQL does" } else { "" } })
}
fn tried(answers: [Option<&str>; 4], obligation: Value) -> Value {
    json!({ "id": "e1", "path": SQL, "export": "removeComments", "args": [DOLLARS], "why": "a dollar-quoted string that holds //", "obligation": obligation,
        "shapes": [["string"]], "retested": false, "host": "src/sql.test.ts", "dir": "", "reached": { "old": true, "new": true },
        "answers": { "old": answers[0], "old_again": answers[1], "new": answers[2], "new_again": answers[3] }, "data": "differs", "chance": false, "network": false, "uneven": null })
}
fn ran(experiments: Vec<Value>, not_run: Value) -> Value { json!({ "status": "RAN", "why": "", "experiments": experiments, "not_run": not_run }) }
fn receipt(probes: Value) -> Value {
    let d = json!({ "base": BASE, "head": HEAD, "skipped": [], "unmeasured": [] });
    let rules = json!({ "changed": [{ "path": SQL, "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "lockfile": [], "agent_notes": [], "not_checked": [] });
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": { "passed": 4, "failed": 0, "skipped": 0 }, "after": { "passed": 4, "failed": 0, "skipped": 0 },
        "kept": 4, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "already_failing": [], "new_problems": [], "not_run": [], "probes": probes });
    kern::receipt::build(&d, &rules, &tests)
}
fn only(r: &Value, list: &str, kind: &str) -> String {
    let items: Vec<&Value> = r[list].as_array().unwrap().iter().filter(|x| x["kind"] == kind).collect();
    assert_eq!(items.len(), 1, "one {kind} in {list}: {r}");
    items[0]["what"].as_str().unwrap().to_string()
}
fn probes_lines(r: &Value) -> Vec<String> { kern::receipt::markdown(r).lines().filter(|l| l.contains("Probes:")).map(str::to_string).collect() }

#[test]
fn lightdash_12942_a_stripper_that_now_throws_on_a_dollar_quoted_string_breaks_a_verified_requirement_and_is_blocked() {
    let mut returns = obligation("verified");
    returns["check"] = json!("returns");
    returns["statement"] = json!("Removing comments never throws for a query string.");
    let r = receipt(ran(vec![tried([Some(KEPT), Some(KEPT), Some("throws TypeError: unterminated quote"), Some("throws TypeError: unterminated quote")], returns)], json!([])));
    assert_eq!(r["decision"], "blocked", "{r}");
    let what = only(&r, "blocked", "probe-regression");
    for part in ["`removeComments` in `src/sql.ts`", "`removeComments(\"SELECT $$https://example.com/a//b$$ AS address\")`", "`TypeError: unterminated quote`",
        "`\"SELECT $$https://example.com/a//b$$ AS address\"`", "The requirement: Removing comments never throws for a query string"] {
        assert!(what.contains(part), "the block names the function, the input as one call, both answers and the requirement with its quote ({part}): {what}");
    }
    assert_eq!(probes_lines(&r), ["- ⚠️ Probes: 1 of 1 input the AI step chose, for 1 function, got another answer on the new code (see above)"]);
    let r = receipt(ran(vec![tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"))], json!([])));
    assert_eq!(r["decision"], "look", "civitai#3102 blocked a label the change added on purpose: which inputs a `same` requirement covers is the AI's reading, so it is a look at most: {r}");
    let what = only(&r, "look", "probe-concern");
    assert!(what.contains(CITED) && what.contains("`\"SELECT $$https:\"`") && what.contains("covers the inputs its source names"), "{what}");
}

#[test]
fn lightdash_12942_a_broken_requirement_short_of_proof_is_a_look_that_says_what_a_block_would_need() {
    let mut shapeless = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"));
    shapeless["shapes"] = json!([["string", "object"]]);
    let mut unread = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"));
    unread["data"] = json!("missing");
    let mut retested = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"));
    retested["retested"] = json!(true);
    let cases = [
        (tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("contested")), "the change edits the quoted text in `src/sql.test.ts`"),
        (tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("authorized")), "the pull request says it changes this: `Strip // comments too, as MySQL does`"),
        (shapeless, "neither the old code's tests and callers nor its declared parameter types pass inputs of this shape"),
        (tried([Some(KEPT), None, Some(CUT), Some(CUT)], obligation("verified")), "Kern could not run it a second time on each side to confirm"),
        (unread, "Kern could not read both answers as plain data, so it compared only how they print"),
        (retested, "the change also adds or edits a test that calls `removeComments`, which may state the new behaviour"),
    ];
    for (x, missing) in cases {
        let r = receipt(ran(vec![x], json!([])));
        assert_eq!(r["decision"], "look", "{r}");
        let what = only(&r, "look", "probe-concern");
        assert!(what.contains(missing) && what.contains(CITED) && what.contains("`removeComments(\"SELECT $$https://example.com/a//b$$ AS address\")`") && what.contains("`\"SELECT $$https:\"`"), "{what}");
        assert!(what.contains("Kern does not block it because"), "{what}");
    }
}

#[test]
fn lightdash_12942_an_answer_that_changes_with_no_requirement_kern_can_verify_is_a_note_that_never_decides() {
    let mut kept = tried([Some(CUT), Some(CUT), Some(KEPT), Some(KEPT)], obligation("verified"));
    kept["obligation"]["check"] = json!("returns");
    for x in [tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], Value::Null), tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("unverified")), kept] {
        let r = receipt(ran(vec![x], json!([])));
        assert_eq!(r["decision"], "clear", "a note never decides: {r}");
        let what = only(&r, "notes", "probe-difference");
        assert!(what.contains("`removeComments` in `src/sql.ts`") && what.contains("`removeComments(\"SELECT $$https://example.com/a//b$$ AS address\")`") && what.contains("nothing Kern can verify says which answer is right"), "{what}");
    }
}

#[test]
fn lightdash_12942_a_broken_requirement_on_sides_set_up_differently_is_not_verified() {
    let mut x = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"));
    x["uneven"] = json!("the old code's dependencies were installed and the new code's were installed from package.json (the lockfile is out of date)");
    let r = receipt(ran(vec![x], json!([])));
    assert_eq!(r["decision"], "unverified", "{r}");
    let what = only(&r, "unverified", "probe-uneven");
    assert!(what.contains("did not set up both sides the same way") && what.contains("the lockfile is out of date") && what.contains(CITED), "{what}");
}

#[test]
fn lightdash_12942_inputs_kern_could_not_compare_are_not_checked_and_never_earn_a_check_mark() {
    let mut unreached = tried([Some(KEPT), Some(KEPT), Some(CUT), Some(CUT)], obligation("verified"));
    unreached["reached"]["new"] = json!(false);
    let sandbox = tried([Some(KEPT), Some(KEPT), Some("throws Error: connect ECONNREFUSED 127.0.0.1:5432"), Some("throws Error: connect ECONNREFUSED 127.0.0.1:5432")], obligation("verified"));
    let chance = tried([Some("returns 1760000000000"), Some("returns 1760000000004"), Some("returns 1760000000009"), Some("returns 1760000000012")], obligation("verified"));
    let r = receipt(ran(vec![unreached, sandbox, chance], json!([])));
    assert_eq!(r["decision"], "clear", "what Kern could not compare never decides: {r}");
    let lines = probes_lines(&r);
    assert_eq!(lines.len(), 1, "{lines:?}");
    for part in ["⏭️ Probes: 3 of 3 inputs not compared", "1 never reached the real function", "1 needed something the sandbox does not have", "1 answered differently between two runs of the same code"] {
        assert!(lines[0].contains(part), "{part}: {lines:?}");
    }
    let late = receipt(json!({ "status": "NOT_RUN", "why": "the AI step's findings did not arrive within 600 seconds", "experiments": [], "not_run": [] }));
    assert_eq!(probes_lines(&late), ["- ⏭️ Probes: not checked — the AI step's findings did not arrive within 600 seconds"]);
    assert_eq!(late["decision"], "clear");
    assert!(probes_lines(&receipt(json!({ "status": "NONE", "why": "the AI step is off" }))).is_empty(), "no investigation, no line");
    let partly = receipt(ran(vec![tried([Some(KEPT), Some(KEPT), Some(KEPT), Some(KEPT)], obligation("verified"))], json!([{ "path": "src/a.ts", "why": "no test file Kern ran on the new code imports it" }])));
    let lines = probes_lines(&partly);
    assert!(!lines.iter().any(|l| l.contains("✅")), "not every input ran, so no check mark: {lines:?}");
    assert!(lines.contains(&"- ⏭️ Probes: not tried on `src/a.ts` — no test file Kern ran on the new code imports it".to_string()), "{lines:?}");
}

#[test]
fn lightdash_12942_inputs_that_answer_the_same_on_both_sides_earn_a_check_mark_that_counts_them() {
    let mut second = tried([Some("returns \"SELECT 1 \""), None, Some("returns \"SELECT 1 \""), None], obligation("verified"));
    second["id"] = json!("e2");
    second["args"] = json!(["SELECT 1 -- note"]);
    let r = receipt(ran(vec![tried([Some(KEPT), None, Some(KEPT), None], obligation("verified")), second.clone()], json!([])));
    assert_eq!(r["decision"], "clear", "{r}");
    assert_eq!(probes_lines(&r), ["- ✅ Probes: 2 inputs the AI step chose, for 1 function, got the same answers on the old and the new code"]);
    assert!(r["notes"].as_array().unwrap().is_empty());
    let mut thrown = tried([Some("throws TypeError: db is not set"), None, Some("throws TypeError: db is not set"), None], obligation("verified"));
    thrown["id"] = json!("e3");
    let r = receipt(ran(vec![tried([Some(KEPT), None, Some(KEPT), None], obligation("verified")), second, thrown], json!([])));
    assert_eq!(probes_lines(&r), ["- ✅ Probes: 3 inputs the AI step chose, for 1 function, got the same answers on the old and the new code (1 of them an error on both)"],
        "an input that only shows the same error on both sides says less, and the line says how many did");
}

fn investigation() -> Value {
    json!({ "version": 1, "base": BASE, "head": HEAD, "status": "INVESTIGATED", "why": "", "model": "claude-opus-5-5", "usage": { "input_tokens": 0, "output_tokens": 0 },
        "targets": [{ "path": SQL, "export": "removeComments", "shapes": [["string"], ["string", "object"]] }], "obligations": [obligation("verified")],
        "experiments": [{ "id": "e1", "obligation": "o1", "path": SQL, "export": "removeComments", "args": [DOLLARS], "why": "a dollar-quoted string that holds //" }], "dropped": [] })
}
fn unusable(v: &Value) -> String {
    match probe::parse(v, (BASE, HEAD)) { Parsed::Unusable(why) => why, other => panic!("{v} was accepted: {other:?}") }
}
fn with(change: impl Fn(&mut Value)) -> Value { let mut v = investigation(); change(&mut v); v }

#[test]
fn lightdash_12942_kern_reads_the_ai_steps_findings_only_when_every_part_is_within_the_contract() {
    let Parsed::Investigated(i) = probe::parse(&investigation(), (BASE, HEAD)) else { panic!("the contract's own example is read") };
    assert_eq!(i.experiments.len(), 1);
    assert_eq!(i.experiments[0].obligation, obligation("verified"), "an experiment carries the whole obligation it tries");
    assert_eq!(i.experiments[0].args, vec![json!(DOLLARS)]);
    assert_eq!(i.targets[0].shapes, vec![vec!["string".to_string()], vec!["string".to_string(), "object".to_string()]]);
    assert!(unusable(&with(|v| v["head"] = json!(BASE))).contains("other commits"));
    assert!(unusable(&with(|v| v["version"] = json!(2))).contains("version"));
    assert!(unusable(&json!([])).contains("version"), "a file that is not an object is refused, never read in part");
    assert!(unusable(&with(|v| v["status"] = json!("DONE"))).contains("status"));
    assert_eq!(probe::parse(&with(|v| { v["status"] = json!("FAILED"); v["why"] = json!("the model timed out"); }), (BASE, HEAD)), Parsed::Unusable("the AI step failed: the model timed out".into()));
    assert!(matches!(probe::parse(&with(|v| { v["status"] = json!("NONE"); v["why"] = json!("no changed exported function"); }), (BASE, HEAD)), Parsed::Nothing(w) if w == "no changed exported function"));
    assert!(matches!(probe::parse(&with(|v| v["experiments"] = json!([])), (BASE, HEAD)), Parsed::Nothing(_)), "nothing to try is nothing to run");
    let target = |n: usize| json!({ "path": format!("src/f{n}.ts"), "export": "f", "shapes": [] });
    let experiment = |n: usize, path: &str| json!({ "id": format!("e{n}"), "obligation": null, "path": path, "export": "removeComments", "args": [n], "why": "" });
    let refused = [
        (with(|v| v["targets"] = json!((0..13).map(target).collect::<Vec<_>>())), "13 targets"),
        (with(|v| v["experiments"][0]["args"] = json!([1, 2, 3, 4, 5, 6, 7, 8, 9])), "9 args"),
        (with(|v| v["experiments"][0]["args"] = json!(["x".repeat(2100)])), "2048 bytes"),
        (with(|v| v["obligations"][0]["statement"] = json!("s".repeat(301))), "300 characters"),
        (with(|v| v["obligations"][0]["statement"] = json!("  ")), "states nothing"),
        (with(|v| v["obligations"][0]["source"]["quote"] = json!("q".repeat(601))), "600 characters"),
        (with(|v| v["experiments"][0]["why"] = json!("w".repeat(201))), "200 characters"),
        (with(|v| v["targets"][0]["shapes"] = json!((0..17).map(|_| json!(["string"])).collect::<Vec<_>>())), "17 shapes"),
        (with(|v| v["targets"][0]["shapes"] = json!([["string", "function"]])), "argument shape"),
        (with(|v| v["experiments"] = json!((0..9).map(|n| experiment(n, SQL)).collect::<Vec<_>>())), "more than 8 experiments"),
        (with(|v| { v["targets"] = json!((0..9).map(|n| json!({ "path": format!("src/f{n}.ts"), "export": "removeComments", "shapes": [] })).collect::<Vec<_>>());
            v["experiments"] = json!((0..65).map(|n| experiment(n, &format!("src/f{}.ts", n % 9))).collect::<Vec<_>>()); }), "65 experiments"),
        (with(|v| { let e = v["experiments"][0].clone(); v["experiments"] = json!([e.clone(), e]); }), "twice"),
        (with(|v| v["experiments"][0]["path"] = json!("src/other.ts")), "no target names"),
        (with(|v| v["experiments"][0]["obligation"] = json!("o9")), "does not list"),
        (with(|v| v["obligations"][0]["targets"] = json!([{ "path": SQL, "export": "format" }])), "not about"),
        (with(|v| v["obligations"][0]["authority"] = json!("certain")), "authority"),
        (with(|v| v["obligations"][0]["check"] = json!("faster")), "check"),
        (with(|v| v["obligations"][0]["source"]["kind"] = json!("code")), "kind of source"),
        (with(|v| { v["obligations"][0]["authority"] = json!("authorized"); v["obligations"][0]["authorized_by"] = json!(""); }), "without quoting it"),
        (with(|v| { v["targets"][0]["path"] = json!("../outside.ts"); v["experiments"][0]["path"] = json!("../outside.ts"); }), "code file"),
        (with(|v| v["experiments"][0]["export"] = json!("a b")), "names no function"),
        (with(|v| v["experiments"] = json!({ "e1": 1 })), "no experiments list"),
    ];
    for (v, why) in refused {
        let said = unusable(&v);
        assert!(said.starts_with("the AI step's findings are malformed: ") && said.contains(why), "{why}: {said}");
    }
}

fn candidate(path: &str, source: &str, both: bool) -> Candidate { Candidate { path: path.into(), source: source.into(), both, unchanged: both } }
fn experiment(id: &str, path: &str, export: &str, args: Value) -> probe::Experiment {
    probe::Experiment { id: id.into(), path: path.into(), export: export.into(), args: args.as_array().unwrap().clone(), why: String::new(), obligation: Value::Null }
}

#[test]
fn lightdash_12942_a_test_that_imports_the_target_only_as_a_type_never_hosts_its_probe() {
    let files = ["src/sql.ts", "src/sql.test.ts", "src/scheduler.test.ts", "src/project.ts"];
    let exists = |p: &str| files.contains(&p);
    let candidates = [
        candidate("src/scheduler.test.ts", "import type { Sql } from './sql';\nimport { it } from 'vitest';\nit('plans', () => {});\n", true),
        candidate("src/sql.test.ts", "import { describe, it, expect } from 'vitest';\nimport { removeComments } from './sql';\n\nit(\"keeps -- inside quoted strings\", () => {});\n", true),
    ];
    let hosts = probe::hosts(&[SQL, "src/project.ts"], &candidates, &exists);
    assert_eq!(hosts.len(), 1, "src/project.ts has no test that loads it: {hosts:?}");
    let host = &hosts[SQL];
    assert_eq!((host.path.as_str(), host.spec.as_str()), ("src/sql.test.ts", "./sql"), "SchedulerTask.test.ts only named the type, so it never loaded the module");
    assert_eq!(host.style, Style { esm: true, typescript: true, jest_globals: false });
    let e = experiment("e1", SQL, "removeComments", json!([DOLLARS]));
    assert_eq!(probe::probe_source(host.style, Runner::Vitest, &["./sql".to_string()], &[(0, &e)]),
        "// @ts-nocheck\nimport { test } from \"vitest\";\nimport * as m from \"/kern/trace.cjs\";\nimport * as target0 from \"./sql\";\nconst t = m.default ?? m;\n\
         test(\"kern probe e1\", async () => { await t.probe(\"e1\", target0, \"removeComments\", JSON.parse(\"[\\\"SELECT $$https://example.com/a//b$$ AS address\\\"]\")); }, 10000);\n");
    assert_eq!(probe::probe_name("src/sql.test.ts", 0).as_deref(), Some("src/kern-probe.sql.test.ts"), "the probe keeps the host's whole name after a prefix, so every pattern that picked the host (*.test.ts, *_test.ts, __tests__/) picks it too");
    assert_eq!(probe::probe_name("src/__tests__/sql.ts", 2).as_deref(), Some("src/__tests__/kern-probe2.sql.ts"));
    assert_eq!(probe::probe_name("test/money_test.js", 0).as_deref(), Some("test/kern-probe.money_test.js"));
    assert_eq!(kern::suite::kept_name("src/sql.test.ts", 0).as_deref(), Some("src/sql.kern-kept.test.ts"));
}

#[test]
fn lightdash_12942_the_probe_file_copies_its_hosts_module_style_and_test_api() {
    let files = ["src/sql.js", "src/slug.js", "test/sql.test.js", "test/slug.test.ts", "a/first.test.js", "z/last.test.js"];
    let exists = |p: &str| files.contains(&p);
    let cjs = probe::hosts(&["src/sql.js"], &[candidate("test/sql.test.js", "const { removeComments } = require('../src/sql');\n\ntest('strips', () => {});\n", true)], &exists);
    let host = &cjs["src/sql.js"];
    assert_eq!(host.style, Style { esm: false, typescript: false, jest_globals: false });
    let e = experiment("e1", "src/sql.js", "removeComments", json!(["a // b"]));
    assert_eq!(probe::probe_source(host.style, Runner::Jest, std::slice::from_ref(&host.spec), &[(0, &e)]),
        "const m = require(\"/kern/trace.cjs\");\nconst target0 = require(\"../src/sql\");\nconst t = m.default ?? m;\n\
         test(\"kern probe e1\", async () => { await t.probe(\"e1\", target0, \"removeComments\", JSON.parse(\"[\\\"a // b\\\"]\")); }, 10000);\n", "jest's own global test, and require like its host");
    let globals = probe::hosts(&["src/slug.js"], &[candidate("test/slug.test.ts", "import { describe, expect, it } from '@jest/globals';\nimport slug from '../src/slug.js';\n", true)], &exists);
    let host = &globals["src/slug.js"];
    assert_eq!(host.style, Style { esm: true, typescript: true, jest_globals: true });
    let d = experiment("e2", "src/slug.js", "default", json!(["A b", { "lower": true }]));
    let source = probe::probe_source(host.style, Runner::Jest, std::slice::from_ref(&host.spec), &[(0, &d)]);
    assert!(source.starts_with("// @ts-nocheck\nimport { test } from \"@jest/globals\";\n") && source.contains("import * as target0 from \"../src/slug.js\";"), "{source}");
    assert!(source.contains("t.probe(\"e2\", target0, \"default\", JSON.parse(\"[\\\"A b\\\",{\\\"lower\\\":true}]\"))"), "a default export goes in whole, and probe picks it: {source}");
    let both = probe::hosts(&["src/sql.js"], &[candidate("a/first.test.js", "import { removeComments } from '../src/sql.js';\n", false), candidate("z/last.test.js", "import { removeComments } from '../src/sql.js';\n", true)], &exists);
    assert_eq!(both["src/sql.js"].path, "z/last.test.js", "a test file both sides have is preferred, so the old side loads the probe the same way");
    let separators = experiment("e3", "src/sql.js", "removeComments", json!(["a\u{2028}b\u{2029}"]));
    let source = probe::probe_source(host.style, Runner::Vitest, &["../src/sql.js".to_string()], &[(0, &separators)]);
    assert!(source.contains("a\\u2028b\\u2029") && !source.contains('\u{2028}') && !source.contains('\u{2029}'), "line separators are escaped, so no parser reads them as the end of a line: {source}");
}

#[test]
fn insighta_1137_a_probe_runs_inside_a_copy_of_its_host_test_so_the_hosts_own_mocks_and_setup_load_the_module() {
    let files = ["src/modules/book.ts", "tests/unit/book.test.js", "tests/unit/book-old.test.js"];
    let exists = |p: &str| files.contains(&p);
    let host_text = "jest.mock('../../src/config', () => ({ config: { secret: 'test' } }));\nconst { buildBook } = require('../../src/modules/book');\n\ntest('builds', () => { expect(buildBook('a')).toBeTruthy(); });\n";
    let unchanged = Candidate { path: "tests/unit/book.test.js".into(), source: host_text.into(), both: true, unchanged: true };
    let edited = Candidate { path: "tests/unit/book-old.test.js".into(), source: host_text.into(), both: true, unchanged: false };
    let hosts = probe::hosts(&["src/modules/book.ts"], &[edited, unchanged], &exists);
    let host = &hosts["src/modules/book.ts"];
    assert_eq!((host.path.as_str(), host.text.as_deref()), ("tests/unit/book.test.js", Some(host_text)), "a host the change leaves alone is preferred, and only it lends its text: its mocks are the same on both sides");
    let e = experiment("e1", "src/modules/book.ts", "buildBook", json!(["a"]));
    let source = probe::hosted_source(host_text, host.style, Runner::Jest, std::slice::from_ref(&host.spec), &[(0, &e)]);
    assert!(source.starts_with(host_text), "the host's jest.mock and setup come first, exactly as the host has them:\n{source}");
    assert!(source.ends_with("const __kern_m = require(\"/kern/trace.cjs\");\nconst __kern_target0 = require(\"../../src/modules/book\");\nconst __kern_test = test;\nconst __kern_t = __kern_m.default ?? __kern_m;\n\
        __kern_test(\"kern probe e1\", async () => { await __kern_t.probe(\"e1\", __kern_target0, \"buildBook\", JSON.parse(\"[\\\"a\\\"]\")); }, 10000);\n"), "Kern's own names cannot clash with the host's:\n{source}");
    let esm = "import { vi, test, expect } from 'vitest';\nvi.mock('../src/config');\nimport { removeComments } from '../src/sql';\n";
    let source = probe::hosted_source(esm, Style { esm: true, typescript: true, jest_globals: false }, Runner::Vitest, &["../src/sql".to_string()], &[(0, &e)]);
    assert!(source.starts_with(&format!("// @ts-nocheck\n{esm}")) && source.contains("\nimport { test as __kern_test } from \"vitest\";\n") && source.contains("import * as __kern_target0 from \"../src/sql\";"), "{source}");
    let docblock = "/**\n * @jest-environment jsdom\n */\nimport { render } from '../src/render';\n";
    let source = probe::hosted_source(docblock, Style { esm: true, typescript: true, jest_globals: false }, Runner::Jest, &["../src/render".to_string()], &[(0, &e)]);
    assert!(source.starts_with("/**\n * @jest-environment jsdom\n */\n// @ts-nocheck\nimport { render } from '../src/render';\n"), "jest reads its environment only from a docblock that opens the file, so Kern's pragma comes after it:\n{source}");
    let focused = probe::focus(&kern::policy::Tests { paths: vec![], kind: kern::policy::Kind::Feature, runner: Runner::Jest, command: vec!["node_modules/.bin/jest".into()], edits: kern::policy::Edits::Checked },
        &["tests/unit/book.kern-probe.test.js".to_string()].into_iter().collect(), "").unwrap();
    assert!(focused.command.ends_with(&["-t".to_string(), "kern probe".to_string()]), "only Kern's tests run, not the host's own: {:?}", focused.command);
}

fn line(v: Value) -> String { format!("{v}\n") }

#[test]
fn lightdash_12942_each_input_is_answered_once_per_run_by_the_real_function_or_it_is_not_compared() {
    let key = format!("{SQL}#removeComments");
    let old = [line(json!({ "k": key, "a": "[\"x\"]", "o": "returns \"x\"" })), line(json!({ "p": "e1", "s": 1 })), line(json!({ "p": "e1", "o": KEPT, "k": key })), line(json!({ "p": "e1", "o": KEPT, "k": key })),
        "not json\n".to_string(), line(json!({ "p": "e2", "o": "maybe 1" })), line(json!({ "p": "e3", "o": format!("returns {}", "x".repeat(5000)) })), line(json!({ "p": "e4", "o": HUNG })),
        line(json!({ "p": "e5", "s": 1, "k": key }))].concat();
    let new = [line(json!({ "p": "e1", "s": 1 })), line(json!({ "p": "e1", "o": CUT, "k": "src/other.ts#removeComments" })), line(json!({ "p": "e5", "s": 1 })), line(json!({ "p": "e5", "o": CUT, "k": key }))].concat();
    let mut x = tried([None; 4], obligation("verified"));
    probe::recorded(&mut x, &[Some(probe::seen(&old)), None, Some(probe::seen(&new)), None]);
    assert_eq!(x["answers"], json!({ "old": KEPT, "old_again": null, "new": CUT, "new_again": null }), "a run that did not happen has no answer");
    assert_eq!(x["reached"], json!({ "old": true, "new": false }), "on the new side a mock or another file's function answered, so the call never went through the real function");
    assert_eq!(x["chance"], json!(false), "two records with the same answer are one answer");
    assert_eq!(probe::verdict(&probe::evidence(&x)), Verdict::Unchecked(Unchecked::Unreached));
    let seen = probe::seen(&old);
    assert!(!seen.answers.contains_key("e2") && !seen.answers.contains_key("e3") && !seen.answers.contains_key("e4"), "an answer Kern's recorder could not have written is ignored: {seen:?}");
    let mut hung = tried([None; 4], obligation("verified"));
    hung["id"] = json!("e5");
    probe::recorded(&mut hung, &[Some(probe::seen(&old)), None, Some(probe::seen(&new)), None]);
    assert_eq!(hung["answers"]["old"], HUNG, "an input that started and never answered before the run ended is a hang, not a missing run");
    assert_eq!(hung["reached"]["new"], json!(true));
    assert_eq!(probe::verdict(&probe::evidence(&hung)), Verdict::Unchecked(Unchecked::Hung));
    let twice = [line(json!({ "p": "e1", "o": KEPT, "k": key })), line(json!({ "p": "e1", "o": CUT, "k": key }))].concat();
    let mut x = tried([None; 4], obligation("verified"));
    probe::recorded(&mut x, &[Some(probe::seen(&twice)), None, Some(probe::seen(&new.replace("src/other.ts", SQL))), None]);
    assert_eq!(x["chance"], json!(true), "two test projects ran the same probe and answered differently, so which one Kern read first is luck");
    assert_eq!(probe::verdict(&probe::evidence(&x)), Verdict::Unchecked(Unchecked::Chance));
}

fn stored(dir: &Path, probes: Value) {
    let tests = json!({ "status": "RAN", "runner": "vitest", "before": {}, "after": {}, "kept": 4, "broke": [], "flaky": [], "gone": [], "newly_skipped": [], "added": [], "new_failing": [],
        "already_failing": [], "new_problems": [], "not_run": [], "run_broke": [], "run_unconfirmed": [], "rewritten": [], "problem_why": {}, "probes": probes });
    let receipt = json!({ "version": 1, "base": BASE, "head": HEAD, "decision": "clear", "blocked": [], "look": [], "tests": tests,
        "rules": { "changed": [{ "path": SQL, "operation": "modify" }], "powers": [], "tricks": [], "settings": [], "protected": [], "dependencies": [], "agent_notes": [], "not_checked": [] },
        "behaviour": { "status": "OFF", "why": "no AI key" } });
    std::fs::write(dir.join("receipt.json"), receipt.to_string()).unwrap();
}

#[test]
fn lightdash_12942_the_replay_judges_stored_probe_traces_with_the_same_code() {
    let t = Temp::new();
    let key = format!("{SQL}#removeComments");
    let mut returns = obligation("verified");
    returns["check"] = json!("returns");
    stored(&t.0, ran(vec![tried([Some(KEPT), Some(KEPT), Some(KEPT), Some(KEPT)], returns)], json!([])));
    for (side, answer) in [("old", KEPT), ("old-again", KEPT), ("new", "throws TypeError: unterminated quote"), ("new-again", "throws TypeError: unterminated quote")] {
        let mut record = json!({ "p": "e1", "o": answer, "k": key });
        if let Some(v) = answer.strip_prefix("returns ") { record["v"] = json!(serde_json::from_str::<Value>(v).unwrap().to_string()); }
        std::fs::write(t.0.join(format!("probe-{side}.jsonl")), [line(json!({ "p": "e1", "s": 1, "k": key })), line(record)].concat()).unwrap();
    }
    let r = kern::replay::rejudge(&t.0).unwrap();
    assert_eq!(r["receipt"]["decision"], "blocked", "the stored receipt said the answers were the same, but its own traces show the break: {r}");
    assert_eq!(r["receipt"]["tests"]["probes"]["experiments"][0]["answers"]["new"], "throws TypeError: unterminated quote");
    let setup = json!({ "old": { "installs": [{ "dir": ".", "result": "installed" }] }, "new": { "installs": [{ "dir": ".", "result": "installed from package.json (the lockfile is out of date)" }] } });
    std::fs::write(t.0.join("setup.json"), setup.to_string()).unwrap();
    let r = kern::replay::rejudge(&t.0).unwrap();
    assert_eq!(r["receipt"]["decision"], "unverified", "the replay reads the sides' setup again: {r}");
    assert!(r["receipt"]["unverified"].as_array().unwrap().iter().any(|x| x["kind"] == "probe-uneven"), "{r}");
}

#[test]
fn lightdash_12942_the_probe_phase_reads_back_each_suite_kern_ran_with_its_own_command_and_variables() {
    use kern::policy::{Edits, Kind, Tests};
    let t = Temp::new();
    let tests = |runner: Runner, command: &[&str]| Tests { paths: Vec::new(), kind: Kind::Feature, runner, command: command.iter().map(|c| c.to_string()).collect(), edits: Edits::Checked };
    let env: kern::suite::Env = [("TZ_OFFSET".to_string(), "0".to_string())].into_iter().collect();
    let suites = [kern::suite::Suite { dir: "packages/backend".into(), old: Some((tests(Runner::Jest, &["/workspace/node_modules/.bin/jest", "--config", "jest.config.js"]), env.clone())),
            new: (tests(Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run"]), kern::suite::Env::new()), uneven: Some("packages/shared build worked on one side only".into()) },
        kern::suite::Suite { dir: String::new(), old: None, new: (tests(Runner::Vitest, &["/workspace/node_modules/.bin/vitest", "run"]), env), uneven: None }];
    std::fs::write(t.0.join("suites.json"), json!(suites.iter().map(kern::suite::Suite::json).collect::<Vec<_>>()).to_string()).unwrap();
    let back = kern::suite::ran_suites(&t.0).unwrap();
    assert_eq!(back.len(), 2);
    for (a, b) in suites.iter().zip(&back) { assert!(a.dir == b.dir && a.old == b.old && a.new == b.new && a.uneven == b.uneven, "{:?} came back as {:?}", a.json(), b.json()); }
    assert_eq!(kern::suite::label("packages/backend"), "-packages_backend");
    std::fs::write(t.0.join("suites.json"), "[{\"dir\": \"\", \"new\": {\"runner\": \"ava\", \"command\": [], \"env\": {}}}]").unwrap();
    assert!(kern::suite::ran_suites(&t.0).is_none(), "a record Kern cannot read whole is not read in part");
}

fn suites_json(out: &Path) {
    let side = json!({ "runner": "vitest", "command": ["/workspace/node_modules/.bin/vitest", "run"], "env": {} });
    std::fs::write(out.join("suites.json"), json!([{ "dir": "", "old": side, "new": side }]).to_string()).unwrap();
}

#[test]
fn lightdash_12942_the_probe_phase_stays_honest_when_its_inputs_are_missing_late_or_wrong() {
    let t = Temp::new();
    let out = t.0.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("drive.json"), json!({ "repo": t.0.join("repo"), "base": BASE, "head": HEAD }).to_string()).unwrap();
    let file = t.0.join("investigation.json");
    let far = kern::util::now_ms() + 3_600_000;
    let status = |v: &Value| (v["status"].as_str().unwrap_or("").to_string(), v["why"].as_str().unwrap_or("").to_string());
    assert_eq!(status(&probe::run(&out, None, far)), ("NONE".into(), "the AI step is off".into()));
    let (s, why) = status(&probe::run(&out, Some(&file), far));
    assert!(s == "NOT_RUN" && why.contains("did not run"), "no suite ran, so there is no test file to run the inputs in, and Kern does not wait: {why}");
    suites_json(&out);
    let (s, why) = status(&probe::run(&out, Some(&file), kern::util::now_ms()));
    assert!(s == "NOT_RUN" && why == "the AI step's findings did not arrive within 0 seconds", "with no time left Kern does not wait at all: {why}");
    for (v, want) in [(with(|v| v["head"] = json!(BASE)), "other commits"), (with(|v| v["status"] = json!("FAILED")), "the AI step failed"),
        (with(|v| v["experiments"][0]["args"] = json!("x")), "malformed"), (json!("not an object"), "version")] {
        std::fs::write(&file, v.to_string()).unwrap();
        let (s, why) = status(&probe::run(&out, Some(&file), far));
        assert!(s == "NOT_RUN" && why.contains(want), "{want}: {s} {why}");
    }
    std::fs::write(&file, "{ cut off").unwrap();
    let (s, why) = status(&probe::run(&out, Some(&file), far));
    assert!(s == "NOT_RUN" && why.starts_with("Kern could not read the AI step's findings"), "a cut-off file: {why}");
    std::fs::write(&file, with(|v| { v["status"] = json!("NONE"); v["why"] = json!("no changed exported function"); }).to_string()).unwrap();
    assert_eq!(status(&probe::run(&out, Some(&file), far)), ("NONE".into(), "no changed exported function".into()));
    std::fs::write(&file, investigation().to_string()).unwrap();
    let (s, why) = status(&probe::run(&out, Some(&file), kern::util::now_ms() + 100_000));
    assert!(s == "NOT_RUN" && why.contains("time ran out"), "too little time left to run four probe runs: {why}");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}
fn commit(r: &Path, files: &[(&str, &str)]) -> String {
    for (p, text) in files {
        let path = r.join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "change"]);
    git(r, &["rev-parse", "HEAD"])
}
const STRIP_OLD: &str = "export function removeComments(sql) {\n  return sql.split('$$').map((part, i) => (i % 2 ? part : part.replace(/(--|\\/\\/).*$/gm, ''))).join('$$');\n}\n";
const STRIP_NEW: &str = "export function removeComments(sql) {\n  return sql.replace(/(--|\\/\\/).*$/gm, '');\n}\n";
const SQL_TEST: &str = "import { test, expect } from 'vitest';\nimport { removeComments } from '../src/sql.js';\n\ntest('keeps -- inside quoted strings', () => expect(removeComments('SELECT 1')).toBe('SELECT 1'));\n";

#[test]
fn lightdash_12942_kern_plans_one_probe_file_beside_a_test_that_loads_the_target_and_a_sandbox_that_never_starts_leaves_it_not_checked() {
    let t = Temp::new();
    let r = t.0.join("repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    git(&r, &["config", "core.autocrlf", "false"]);
    let base = commit(&r, &[("package.json", "{\"name\":\"shop\",\"type\":\"module\",\"scripts\":{\"test\":\"vitest run\"}}\n"), ("src/sql.js", STRIP_OLD),
        ("src/other.js", "export function helper(x) { return x; }\n"), ("test/sql.test.js", SQL_TEST)]);
    let head = commit(&r, &[("src/sql.js", STRIP_NEW), ("src/other.js", "export function helper(x) { return [x]; }\n"), ("src/fresh.js", "export function fresh() { return 1; }\n")]);
    let out = t.0.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("drive.json"), json!({ "repo": r, "base": base, "head": head }).to_string()).unwrap();
    std::fs::write(out.join("setup.json"), json!({ "old": { "volume": "kern-vol-old" }, "new": { "volume": "kern-vol-new" } }).to_string()).unwrap();
    std::fs::write(out.join("tests-new.out"), json!({ "numTotalTests": 1, "success": true, "testResults": [{ "name": "/workspace/test/sql.test.js", "status": "passed", "message": "",
        "assertionResults": [{ "ancestorTitles": [], "fullName": "keeps -- inside quoted strings", "status": "passed", "title": "keeps -- inside quoted strings", "failureMessages": [] }] }] }).to_string()).unwrap();
    suites_json(&out);
    let mut v = investigation();
    v["base"] = json!(base);
    v["head"] = json!(head);
    v["targets"] = json!([{ "path": "src/sql.js", "export": "removeComments", "shapes": [["string"]] }, { "path": "src/sql.js", "export": "ghost", "shapes": [] },
        { "path": "src/other.js", "export": "helper", "shapes": [] }, { "path": "src/fresh.js", "export": "fresh", "shapes": [] }]);
    v["obligations"][0]["targets"] = json!([{ "path": "src/sql.js", "export": "removeComments" }]);
    v["experiments"] = json!([
        { "id": "e1", "obligation": "o1", "path": "src/sql.js", "export": "removeComments", "args": [DOLLARS], "why": "" },
        { "id": "e2", "obligation": null, "path": "src/sql.js", "export": "removeComments", "args": ["SELECT 1 -- note"], "why": "" },
        { "id": "e3", "obligation": null, "path": "src/sql.js", "export": "ghost", "args": [], "why": "" },
        { "id": "e4", "obligation": null, "path": "src/other.js", "export": "helper", "args": [1], "why": "" },
        { "id": "e5", "obligation": null, "path": "src/fresh.js", "export": "fresh", "args": [], "why": "" }]);
    let Parsed::Investigated(i) = probe::parse(&v, (&base, &head)) else { panic!("{v}") };
    let suites = kern::suite::ran_suites(&out).expect("the suites Kern ran are read back");
    let (plans, not_run) = probe::plan(&out, &r, (&base, &head), &i, &suites);
    assert_eq!(plans.len(), 1);
    let p = &plans[0];
    let name = "test/kern-probe.sql.test.js";
    for side in 0..2 {
        let source = p.files[side][name].as_str().unwrap();
        assert!(source.starts_with("import { test, expect } from 'vitest';\nimport { removeComments } from '../src/sql.js';\n") && source.contains("import * as __kern_target0 from \"../src/sql.js\";"),
            "the host test is the same on both sides, so the probe runs inside a copy of it: {source}");
        assert!(source.contains("__kern_t.probe(\"e1\", __kern_target0, \"removeComments\"") && source.contains("__kern_t.probe(\"e2\", __kern_target0, \"removeComments\"") && !source.contains("\"e3\""), "{source}");
        assert!(p.sides[side].0.command.ends_with(&["-t".to_string(), "kern probe".to_string()]), "only Kern's tests run: {:?}", p.sides[side].0.command);
        assert!(p.serve[side]["src/sql.js"].as_str().unwrap().contains("__kern_trace.fn(\"src/sql.js\", \"removeComments\", removeComments)"), "the real module is served, watched");
    }
    assert!(p.serve[0]["src/sql.js"].as_str().unwrap().contains("split('$$')") && !p.serve[1]["src/sql.js"].as_str().unwrap().contains("split('$$')"), "each side serves its own code");
    let whys: Vec<(String, String)> = not_run.iter().map(|n| (n["path"].as_str().unwrap().to_string(), n["why"].as_str().unwrap().to_string())).collect();
    assert_eq!(whys, vec![("src/sql.js".to_string(), "ghost is not a function both versions of the file export".to_string()),
        ("src/fresh.js".to_string(), "Kern could not read both versions of the file".to_string()), ("src/other.js".to_string(), "no test file Kern ran on the new code imports it".to_string())]);
    let file = t.0.join("investigation.json");
    std::fs::write(&file, v.to_string()).unwrap();
    let probes = probe::run(&out, Some(&file), kern::util::now_ms() + 3_600_000);
    assert_eq!(probes["status"], "RAN", "{probes}");
    assert_eq!(probes["experiments"].as_array().unwrap().len(), 2);
    assert!(probes["experiments"].as_array().unwrap().iter().all(|x| x["answers"]["old"].is_null() && x["reached"]["old"] == false && x["host"] == "test/sql.test.js"), "{probes}");
    assert!(!out.join("probe-old.jsonl").exists(), "a run that never started leaves no trace to replay");
    let r = receipt(probes);
    assert_eq!(r["decision"], "clear", "Kern's sandbox failing is never the change's fault: {r}");
    let lines = probes_lines(&r);
    assert!(lines.iter().any(|l| l.starts_with("- ⏭️ Probes: 2 of 2 inputs not compared")) && !lines.iter().any(|l| l.contains("✅")), "{lines:?}");
}

#[test]
fn lightdash_12942_the_probe_runtime_records_one_answer_per_input_and_never_throws() {
    let t = Temp::new();
    let runtime = t.0.join("trace.cjs");
    std::fs::write(&runtime, kern::sandbox::TRACE).unwrap();
    let script = format!("const t = require({:?});\n\
        const mod = {{ strip(s) {{ return s.split('//')[0]; }}, async later(x) {{ return x * 2; }}, boom() {{ throw new TypeError('no'); }}, async refuse() {{ throw new Error('nope'); }}, self() {{ return this === mod; }},\n\
          bytes(b) {{ return Buffer.isBuffer(b) ? b.readUInt16BE(0) : 'not a buffer'; }}, own(o) {{ return Object.getOwnPropertyNames(o).join(','); }} }};\n\
        function slug(s) {{ return s.toLowerCase(); }}\n\
        (async () => {{\n\
          const said = await Promise.all([t.probe('e1', mod, 'strip', ['a//b']), t.probe('e2', mod, 'later', [21]), t.probe('e3', mod, 'boom', []), t.probe('e4', mod, 'refuse', []),\n\
            t.probe('e5', mod, 'self', []), t.probe('e6', slug, 'default', ['A B']), t.probe('e7', {{ default: slug }}, 'default', ['C']), t.probe('e8', null, 'gone', []), t.probe('e9', mod, 'missing', []),\n\
            t.probe('e10', mod, 'bytes', [{{ type: 'Buffer', data: [1, 2] }}]), t.probe('e11', mod, 'own', [JSON.parse('{{\"__proto__\": {{\"polluted\": 1}}, \"a\": 1}}')])]);\n\
          if (said.some((x) => x !== undefined) || ({{}}).polluted !== undefined) process.exit(4);\n\
        }})().then(() => process.exit(0), () => process.exit(3));\n", runtime.to_string_lossy());
    let trace = t.0.join("trace.jsonl");
    let Ok(ran) = Command::new("node").args(["-e", &script]).env("KERN_TRACE", &trace).output() else { eprintln!("node is not installed here; the runtime is checked where it is"); return };
    assert!(ran.status.success(), "probe never throws: {} {}", ran.status, String::from_utf8_lossy(&ran.stderr));
    let seen = probe::seen(&std::fs::read_to_string(&trace).unwrap());
    let answer = |id: &str| seen.answers.get(id).and_then(|a| a.first()).cloned().unwrap_or_default();
    assert!((1..=11).all(|n| seen.started.contains(&format!("e{n}"))), "every input marks its start before the call, so a hang leaves a trace: {seen:?}");
    assert_eq!(answer("e1"), "returns \"a\"");
    assert_eq!(answer("e2"), "resolves 42", "a promise is awaited");
    assert_eq!(answer("e3"), "throws TypeError: no");
    assert_eq!(answer("e4"), "rejects Error: nope");
    assert_eq!(answer("e5"), "returns true", "a named export is called on its module, as `import * as` callers do");
    assert_eq!(answer("e6"), "returns \"a b\"", "a module that is itself the function is its default export");
    assert_eq!(answer("e7"), "returns \"c\"");
    assert!(answer("e8").starts_with("throws TypeError") && answer("e9").starts_with("throws TypeError"), "a missing function is an answer, never a crash: {seen:?}");
    assert_eq!(answer("e10"), "returns 258", "a Buffer in Node's own JSON form arrives as a Buffer, as the code under test expects");
    assert_eq!(answer("e11"), "returns \"__proto__,a\"", "a __proto__ key in an input stays a plain key and changes no prototype");
    assert_eq!((seen.values.get("e1"), seen.values.get("e2"), seen.values.get("e3")), (Some(&json!("a")), Some(&json!(42)), None), "a plain answer keeps its value for the kernel to compare");
}
