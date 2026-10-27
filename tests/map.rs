use kern::map::{self, Caps, Kind, Touch};
use protobuf::Message;
use scip::types::{Document, Index, Metadata, Occurrence, SymbolInformation, ToolInfo};

fn occ(symbol: &str, range: &[i32], roles: i32, enclosing: &[i32]) -> Occurrence {
    let mut o = Occurrence::new();
    o.symbol = symbol.into();
    o.range = range.to_vec();
    o.symbol_roles = roles;
    o.enclosing_range = enclosing.to_vec();
    o
}
fn doc(path: &str, occurrences: Vec<Occurrence>) -> Document {
    let mut d = Document::new();
    d.relative_path = path.into();
    d.language = "typescript".into();
    d.occurrences = occurrences;
    d
}
const UI: &str = "scip-typescript npm @acme/ui 1.0.0 ";
const OPTIONS: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/helpers/`scheme.ts`/options().";
const LABEL: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/helpers/`scheme.ts`/label().";
const SHAPE: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/helpers/`scheme.ts`/Options#";
const VERSION: &str = "scip-typescript npm @acme/ui 1.0.0 src/`version.ts`/version.";
const SELECTOR: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/`AuthSelector.vue.ts`/setup().";
const BANNER: &str = "scip-typescript npm @acme/ui 1.0.0 src/`Banner.ts`/banner().";
const HANDLER: &str = "scip-typescript npm @acme/ui 1.0.0 src/api/`route.ts`/handler().";
const FORM: &str = "scip-typescript npm @acme/ui 1.0.0 src/types/`form.ts`/Form#";
const VALIDATE: &str = "scip-typescript npm @acme/ui 1.0.0 src/types/`form.ts`/validate().";
const DUP_A: &str = "scip-typescript npm @acme/ui 1.0.0 src/`a.ts`/dup().";
const DUP_B: &str = "scip-typescript npm @acme/ui 1.0.0 src/`b.ts`/dup().";
const MAIN: &str = "scip-typescript npm @acme/cli 1.0.0 src/`main.ts`/main().";
const BUILT_OPTIONS: &str = "scip-typescript npm @acme/ui 1.0.0 dist/`index.d.mts`/options().";
const BUILT_DUP: &str = "scip-typescript npm @acme/ui 1.0.0 dist/`index.d.ts`/dup().";
const TS_ARRAY: &str = "scip-typescript npm typescript 5.9.3 lib/`lib.es5.d.ts`/Array#";
const SCHEME: &str = "src/blocks/auth/helpers/scheme.ts";
const SELECTOR_FILE: &str = "src/blocks/auth/AuthSelector.vue.ts";
const MAIN_FILE: &str = "packages/cli/src/main.ts";
const IMPORT_LINES: &[(&str, usize)] = &[("src/index.ts", 0), ("src/index.ts", 1), (SELECTOR_FILE, 1), ("src/api/route.ts", 0), ("src/blocks/auth/helpers/scheme.test.ts", 0), (MAIN_FILE, 0), ("src/boot.ts", 0), ("src/Banner.ts", 0)];

static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kern-map-{}-{name}-{}", std::process::id(), RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
fn node(args: &[&std::ffi::OsStr], env: &[(&str, &str)]) -> std::process::Output {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/map.cjs");
    std::process::Command::new("node").arg(&script).args(args).envs(env.iter().copied()).output().expect("node runs Kern's map script")
}
fn compacted(scip: &[u8], env: &[(&str, &str)]) -> Vec<u8> {
    let dir = scratch("compact");
    std::fs::write(dir.join("raw.scip"), scip).unwrap();
    let ran = node(&["--compact".as_ref(), dir.join("raw.scip").as_os_str(), dir.join("facts.jsonl").as_os_str()], env);
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let facts = std::fs::read(dir.join("facts.jsonl")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    facts
}

fn scalar_scip() -> Vec<u8> {
    let mut index = Index::new();
    let mut meta = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-typescript".into();
    tool.version = "0.4.0".into();
    meta.tool_info = protobuf::MessageField::some(tool);
    index.metadata = protobuf::MessageField::some(meta);
    index.documents = vec![
        doc(SCHEME, vec![occ(OPTIONS, &[0, 16, 0, 23], 1, &[0, 0, 2, 1]), occ(LABEL, &[3, 16, 3, 21], 1, &[3, 0, 3, 60]), occ(SHAPE, &[5, 17, 5, 24], 1, &[5, 0, 7, 1]), occ(OPTIONS, &[1, 9, 1, 16], 0, &[])]),
        doc("src/version.ts", vec![occ(VERSION, &[0, 13, 0, 20], 1, &[])]),
        doc("src/index.ts", vec![occ(OPTIONS, &[0, 9, 0, 16], 0, &[]), occ(LABEL, &[0, 18, 0, 23], 0, &[]), occ(VERSION, &[1, 9, 1, 16], 0, &[])]),
        doc(SELECTOR_FILE, vec![occ(OPTIONS, &[1, 9, 1, 16], 0, &[]), occ(SELECTOR, &[3, 9, 3, 14], 1, &[3, 0, 6, 1]), occ(OPTIONS, &[4, 12, 4, 19], 0, &[])]),
        doc("src/Banner.ts", vec![occ(VERSION, &[0, 9, 0, 16], 0, &[]), occ(BANNER, &[2, 16, 2, 22], 1, &[2, 0, 4, 1]), occ(VERSION, &[3, 20, 3, 27], 0, &[])]),
        doc("src/api/route.ts", vec![occ(SELECTOR, &[0, 9, 0, 14], 0, &[]), occ(HANDLER, &[2, 16, 2, 23], 1, &[2, 0, 5, 1]), occ(SELECTOR, &[3, 12, 3, 17], 0, &[])]),
        doc("src/types/form.ts", vec![occ(FORM, &[0, 17, 0, 21], 1, &[0, 0, 3, 1]), occ(SHAPE, &[1, 12, 1, 19], 0, &[]), occ(VALIDATE, &[5, 16, 5, 24], 1, &[5, 0, 8, 1]), occ(SHAPE, &[6, 20, 6, 27], 0, &[])]),
        doc("src/blocks/auth/helpers/scheme.test.ts", vec![occ(OPTIONS, &[0, 9, 0, 16], 0, &[]), occ(OPTIONS, &[2, 30, 2, 37], 0, &[])]),
        doc("src/boot.ts", vec![occ(OPTIONS, &[0, 9, 0, 16], 0, &[]), occ(OPTIONS, &[2, 0, 2, 7], 0, &[]), occ(TS_ARRAY, &[3, 0, 3, 5], 0, &[])]),
        doc("src/version.ts", vec![occ(VERSION, &[0, 13, 0, 20], 1, &[])]),
        doc("src/a.ts", vec![occ(DUP_A, &[0, 16, 0, 19], 1, &[0, 0, 0, 30])]),
        doc("src/b.ts", vec![occ(DUP_B, &[0, 16, 0, 19], 1, &[0, 0, 0, 30])]),
        doc(MAIN_FILE, vec![occ(BUILT_OPTIONS, &[0, 9, 0, 16], 0, &[]), occ(MAIN, &[1, 16, 1, 20], 1, &[1, 0, 3, 1]), occ(BUILT_OPTIONS, &[2, 2, 2, 9], 0, &[]), occ(BUILT_DUP, &[2, 12, 2, 15], 0, &[])]),
    ];
    index.write_to_bytes().unwrap()
}
fn scalar_like() -> Vec<u8> { compacted(&scalar_scip(), &[]) }
fn changed(lines: &[usize]) -> Vec<(String, Vec<usize>)> { vec![(SCHEME.to_string(), lines.to_vec())] }
fn known(file: &str, line: usize) -> Option<bool> { Some(IMPORT_LINES.contains(&(file, line))) }
fn unknown(_: &str, _: usize) -> Option<bool> { None }

#[test]
fn a_symbols_kind_is_read_from_its_descriptor() {
    assert_eq!((map::kind(OPTIONS), map::kind(VERSION), map::kind(SHAPE)), (Kind::Term, Kind::Term, Kind::Type));
    assert_eq!(map::kind(&format!("{UI}src/`x.ts`/")), Kind::Module, "a file's module, or a namespace, is neither a value nor a type: it runs when it loads");
    assert_eq!(map::kind(&format!("{UI}src/`x.ts`/f().(arg)")), Kind::Other, "a parameter is never followed");
}

#[test]
fn the_map_is_an_interned_graph_that_answers_by_name() {
    let m = map::read(&scalar_like()).unwrap();
    assert_eq!(m.provenance, "scip-typescript 0.4.0");
    assert_eq!((m.paths.len(), m.definitions(), m.references()), (12, 12, 20), "a file the indexer wrote twice, once per project, counts once");
    assert_eq!(m.defined_in(OPTIONS), Some(SCHEME));
    assert_eq!(m.defined_in(BUILT_OPTIONS), None, "built output is never a definition");
    assert_eq!(m.canonical(BUILT_OPTIONS), Some(OPTIONS), "a reference into a package's built output is linked to the one source definition with the same name");
    assert_eq!(m.canonical(BUILT_DUP), None, "two source definitions share that name, so the link would be a guess");
    assert_eq!(m.canonical(OPTIONS), None, "a source symbol is its own");
    let s = map::summary(&m);
    assert_eq!((s["documents"].as_u64(), s["symbols"].as_u64(), s["references"].as_u64(), s["bridged"].as_u64(), s["unbridged"].as_u64(), s["external"].as_u64()), (Some(12), Some(12), Some(20), Some(1), Some(1), Some(1)), "a library's declarations are external, never unbridged: {s}");
}

#[test]
fn a_change_inside_one_function_reaches_the_functions_that_run_it_and_names_how_each_file_touches_it() {
    let m = map::read(&scalar_like()).unwrap();
    let r = map::reach(&m, &changed(&[2]), &known);
    assert_eq!(r.hop(&m, OPTIONS).map(|h| h.hops), Some(0), "line 2 of the file is inside options()");
    assert!(r.hop(&m, LABEL).is_none() && r.hop(&m, SHAPE).is_none(), "label() on line 4 and the Options type on lines 6-8 are not changed");
    assert_eq!(r.hop(&m, SELECTOR).map(|h| (h.hops, h.via)), Some((1, OPTIONS.to_string())), "the Vue setup() calls options(), so it runs the change in one hop");
    assert_eq!(r.hop(&m, HANDLER).map(|h| (h.hops, h.via)), Some((2, SELECTOR.to_string())), "the route handler calls setup(), so it runs the change in two hops");
    assert_eq!(r.hop(&m, MAIN).map(|h| (h.hops, h.via)), Some((1, OPTIONS.to_string())), "the cli's main() calls options() through the ui package's built output, and the link to the source definition is followed");
    assert!(r.hop(&m, BANNER).is_none(), "Banner.ts imports version() from the same barrel and never uses options(): a file-level map reaches it, a symbol-level map does not");
    assert_eq!(r.files(&m), vec![
        (MAIN_FILE, 1, Touch::Runs),
        ("src/api/route.ts", 2, Touch::Runs),
        (SELECTOR_FILE, 1, Touch::Runs),
        ("src/blocks/auth/helpers/scheme.test.ts", 1, Touch::Tests),
        (SCHEME, 0, Touch::Changed),
        ("src/boot.ts", 1, Touch::Loads),
        ("src/index.ts", 1, Touch::Mentions),
    ], "the barrel only imports options() (a line inside an import or re-export statement), boot.ts calls it at module level so it runs when the module loads, the test file tests it, three files run it inside a function");
    assert!(r.complete);
    let c = map::counts(&m, &r);
    assert_eq!((c.changed_functions, c.changed_types, c.running, c.loading, c.mentioning, c.declaring, c.tests, c.farthest_hops), (1, 0, 3, 1, 1, 0, 1, 2));
    assert_eq!(map::neighbours(&m, &r, &[SCHEME.into()], 10), vec![(MAIN_FILE.to_string(), 1), (SELECTOR_FILE.to_string(), 1), ("src/boot.ts".to_string(), 1), ("src/api/route.ts".to_string(), 2)], "the files Kern watches: the ones that run the change, nearest first, never a mention, a test or the change itself");
    assert_eq!(map::neighbours(&m, &r, &[SCHEME.into()], 1), vec![(MAIN_FILE.to_string(), 1)]);
}

#[test]
fn without_import_positions_a_module_level_reference_is_only_a_mention_never_a_run() {
    let m = map::read(&scalar_like()).unwrap();
    let r = map::reach(&m, &changed(&[2]), &unknown);
    assert_eq!(r.touch(&m, "src/boot.ts"), Some((1, Touch::Mentions)), "Kern cannot tell an import from a top-level call, so it claims the weaker one");
    assert_eq!(r.touch(&m, "src/index.ts"), Some((1, Touch::Mentions)));
    assert_eq!(r.touch(&m, MAIN_FILE), Some((1, Touch::Runs)), "a reference inside a function needs no import positions");
    assert_eq!(map::counts(&m, &r).loading, 0);
}

#[test]
fn a_change_to_a_type_is_a_shape_change_the_files_that_declare_against_it_are_named_and_nothing_runs_it() {
    let m = map::read(&scalar_like()).unwrap();
    let r = map::reach(&m, &changed(&[7]), &known);
    assert_eq!(r.symbols(&m).iter().map(|(s, _)| *s).collect::<Vec<_>>(), vec![SHAPE], "a type reference never leads on to the functions around it");
    assert_eq!(r.touch(&m, "src/types/form.ts"), Some((1, Touch::Declares)));
    let c = map::counts(&m, &r);
    assert_eq!((c.changed_functions, c.changed_types, c.running, c.declaring), (0, 1, 0, 1));
    assert!(map::neighbours(&m, &r, &[SCHEME.into()], 10).is_empty(), "nothing to watch at run time");
    let r = map::reach(&m, &changed(&[]), &known);
    assert_eq!((map::counts(&m, &r).changed_functions, map::counts(&m, &r).changed_types), (2, 1), "with no line information every definition in the file counts as changed");
}

#[test]
fn reach_stops_at_its_caps_and_says_it_is_incomplete() {
    let m = map::read(&scalar_like()).unwrap();
    let r = map::reach_within(&m, &changed(&[2]), &known, Caps { hops_max: 1, nodes_count_max: map::NODES_COUNT_MAX });
    assert_eq!((r.touch(&m, "src/api/route.ts"), r.complete), (None, false), "the handler is two hops away");
    let r = map::reach_within(&m, &changed(&[2]), &known, Caps { hops_max: map::HOPS_MAX, nodes_count_max: 1 });
    assert_eq!((r.symbols(&m).len(), r.complete), (1, false));
    let r = map::reach_within(&m, &changed(&[2]), &known, Caps { hops_max: 2, nodes_count_max: 4 });
    assert!(r.complete, "every node fits exactly: {:?}", r.symbols(&m));
}

#[test]
fn a_changed_product_file_the_index_does_not_hold_means_the_map_cannot_speak_for_the_change() {
    let m = map::read(&scalar_like()).unwrap();
    assert_eq!(map::uncovered(&m, &[SCHEME.into(), "src/missing.ts".into(), "src/other.test.ts".into(), "README.md".into()]), vec!["src/missing.ts"], "tests and non-code files are not the map's business");
    let c = map::coverage(&m, &["src/index.ts".into(), "src/missing.ts".into()]);
    assert_eq!((c["indexed"].as_u64(), c["not_indexed"][0].as_str()), (Some(1), Some("src/missing.ts")), "a file the indexer did not cover is said, never assumed empty");
}

#[test]
fn the_report_and_the_map_file_carry_every_count_the_receipt_states() {
    let m = map::read(&scalar_like()).unwrap();
    let r = map::reach(&m, &changed(&[2]), &known);
    let rooms = vec![serde_json::json!({ "room": "src/api/route.ts", "kind": "route or entry", "hops": 2, "how": "runs" })];
    let s = map::report(&m, &r, rooms, 3);
    for (key, want) in [("documents", 12), ("symbols", 12), ("references", 20), ("bridged", 1), ("unbridged", 1), ("external", 1), ("changed_functions", 1), ("changed_types", 0), ("files_running", 3), ("files_loading", 1), ("files_mentioning", 1), ("files_declaring", 0), ("test_files", 1), ("farthest_hops", 2), ("rooms_reached", 1), ("not_indexed", 3)] {
        assert_eq!(s[key].as_u64(), Some(want), "{key}: {s}");
    }
    assert_eq!((s["status"].as_str(), s["complete"].as_bool(), s["nearest"].as_str(), s["provenance"].as_str()), (Some("RAN"), Some(true), Some(MAIN_FILE), Some("scip-typescript 0.4.0")));
    let j = map::reach_json(&m, &r, &changed(&[2]));
    assert_eq!(j["files"][0], serde_json::json!({ "file": MAIN_FILE, "hops": 1, "how": "runs" }));
    let selector = j["symbols"].as_array().unwrap().iter().find(|s| s["symbol"] == SELECTOR).cloned().unwrap();
    assert_eq!(selector, serde_json::json!({ "symbol": SELECTOR, "hops": 1, "via": OPTIONS, "file": SELECTOR_FILE }));
    assert_eq!((j["evidence"].as_str(), j["complete"].as_bool()), (Some("scip-typescript 0.4.0"), Some(true)), "every relationship names how it was found: {j}");
}

#[test]
fn an_index_too_large_or_unreadable_is_refused_with_a_reason() {
    let e = map::read(b"not an index").err().expect("bytes that are not an index are refused");
    assert!(e.0.starts_with("The code index could not be read"), "{}", e.0);
    let big = vec![0u8; map::INDEX_BYTES_MAX + 1];
    assert!(map::read(&big).err().is_some_and(|e| e.0.contains("larger than")));
    let footer = r#"{"version":1,"tool":"t","symbols":["s"]}"#;
    for bad in [
        format!("{{\"path\":\"a.ts\",\"occ\":[[9,1,0,0]]}}\n{footer}\n"),
        format!("{{\"path\":\"a.ts\",\"occ\":[[0,1,0]]}}\n{footer}\n"),
        format!("{{\"occ\":[]}}\n{footer}\n"),
        format!("{footer}\n{{\"path\":\"a.ts\",\"occ\":[]}}\n"),
        "{\"path\":\"a.ts\",\"occ\":[]}\n".to_string(),
        "{\"version\":1,\"symbols\":[]}\n".to_string(),
        "{\"version\":2,\"tool\":\"t\",\"symbols\":[]}\n".to_string(),
    ] {
        assert!(map::read(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn the_indexer_is_told_how_the_project_is_laid_out() {
    let has = |files: &[&str]| { let f: Vec<String> = files.iter().map(|s| s.to_string()).collect(); move |p: &str| f.iter().any(|x| x == p) };
    assert_eq!(map::flags(&has(&["pnpm-lock.yaml", "pnpm-workspace.yaml", "tsconfig.json"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--pnpm-workspaces"]);
    assert_eq!(map::flags(&has(&["yarn.lock", "package.json.workspaces"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--yarn-workspaces", "--infer-tsconfig"]);
    assert_eq!(map::flags(&has(&["package-lock.json"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--infer-tsconfig"]);
}

#[test]
fn a_project_without_a_root_tsconfig_gets_kerns_own_because_the_indexers_guess_reads_node_modules_and_calls_a_javascript_project_typescript() {
    let has = |files: &[&str]| { let f: Vec<String> = files.iter().map(|s| s.to_string()).collect(); move |p: &str| f.iter().any(|x| x == p) };
    assert_eq!(map::files(&has(&["package-lock.json"])), serde_json::json!({ "tsconfig.json": "{\"compilerOptions\":{\"allowJs\":true}}" }));
    assert_eq!(map::files(&has(&["package-lock.json", "tsconfig.json"])), serde_json::json!({}));
}

#[test]
fn the_reason_an_indexer_stopped_quotes_what_it_printed_on_stdout_when_stderr_is_empty() {
    let out = b"+ /workspace (47ms)\nerror: no files got indexed. To fix this problem, make sure that the TypeScript projects [\".\"] contain input files or reference other projects.\n";
    assert_eq!(map::stopped("exit 1", b"", out), "the indexer stopped (exit 1): error: no files got indexed. To fix this problem, make sure that the TypeScript projects [\".\"] contain input files or reference other projects.");
    assert_eq!(map::stopped("exit 1", b"Error: Cannot find module\n    at x\n", out), "the indexer stopped (exit 1): at x");
    assert_eq!(map::stopped("timeout", b"", b""), "the indexer stopped (timeout): ");
    assert_eq!(map::stopped("exit 1", b"", &[b'x'; 400]).len(), "the indexer stopped (exit 1): ".len() + 300);
}

fn padded_scip() -> Vec<u8> {
    let mut index = Index::parse_from_bytes(&scalar_scip()).unwrap();
    for d in &mut index.documents {
        d.text = "export const x = 1;\n".repeat(2000);
        d.occurrences.push(occ("local 3", &[5, 0, 5, 3], 1, &[]));
        d.occurrences.push(occ("local 4", &[6, 0, 6, 3], 0, &[]));
        let mut info = SymbolInformation::new();
        info.symbol = OPTIONS.into();
        info.documentation = vec!["```ts\nfunction options(): Options\n```".into(); 4];
        d.symbols.push(info);
    }
    index.write_to_bytes().unwrap()
}

#[test]
fn a_big_repos_index_is_kept_without_the_source_text_symbol_documentation_and_local_symbols_the_map_never_reads() {
    let raw = padded_scip();
    let dir = scratch("big");
    std::fs::write(dir.join("raw.scip"), &raw).unwrap();
    std::fs::write(dir.join("fake.js"), "const fs = require('fs'); const a = process.argv; fs.copyFileSync(process.env.KERN_FAKE_RAW, a[a.indexOf('--output') + 1]); console.log('done');\n").unwrap();
    std::fs::write(dir.join("broken.js"), "console.log('error: no files got indexed.'); process.exitCode = 1;\n").unwrap();
    let out = dir.join("out.jsonl");
    let raw_path = dir.join("raw.scip").to_string_lossy().to_string();
    let run = |indexer: &str| node(&[dir.join(indexer).as_os_str(), "index".as_ref(), "--no-global-caches".as_ref(), "--output".as_ref(), out.as_os_str(), "--infer-tsconfig".as_ref()], &[("KERN_FAKE_RAW", raw_path.as_str())]);
    let ran = run("fake.js");
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    let kept = std::fs::read(&out).unwrap();
    assert!(kept.len() * 10 < raw.len(), "the kept index is {} bytes of {}", kept.len(), raw.len());
    let m = map::read(&kept).expect("the kept file is Kern's fact file");
    let plain = map::read(&scalar_like()).unwrap();
    assert_eq!((&m.paths, &m.symbols, m.definitions(), m.references()), (&plain.paths, &plain.symbols, 12, 20));
    assert_eq!(m.provenance, "scip-typescript 0.4.0");
    assert!(!String::from_utf8_lossy(&kept).contains("local "), "local symbols are never kept");
    let r = map::reach(&m, &changed(&[2]), &known);
    assert_eq!(r.files(&m).len(), 7, "a definition keeps its enclosing lines, so reach is unchanged");
    assert!(String::from_utf8_lossy(&ran.stderr).contains("13 documents"), "{}", String::from_utf8_lossy(&ran.stderr));
    assert!(!dir.join("out.jsonl.raw").exists(), "the raw index is removed once compacted");
    std::fs::remove_file(&out).unwrap();
    let ran = run("broken.js");
    assert_eq!((ran.status.code(), String::from_utf8_lossy(&ran.stdout).contains("no files got indexed"), std::fs::exists(&out).unwrap()), (Some(1), true, false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_compactor_streams_the_index_so_a_document_may_straddle_its_read_window() {
    let raw = padded_scip();
    let whole = compacted(&raw, &[]);
    for chunk in ["100", "4097", "65536"] {
        assert_eq!(compacted(&raw, &[("KERN_MAP_CHUNK_BYTES", chunk)]), whole, "a read window of {chunk} bytes gives the same facts");
    }
}

const CJS_LABEL: &str = "scip-typescript npm watch 1.0.0 src/`label.js`/";
const CJS_REPORT: &str = "scip-typescript npm watch 1.0.0 src/`report.js`/";
fn commonjs_facts() -> Vec<u8> {
    [
        r#"{"path":"src/label.js","occ":[[0,1,0,1]]}"#,
        r#"{"path":"src/report.js","occ":[[1,1,0,2],[2,0,0,0],[0,0,0,0]]}"#,
        r#"{"path":"test/label.test.js","occ":[[3,1,0,3],[2,0,0,0],[0,0,0,0]]}"#,
        r#"{"path":"test/report.test.js","occ":[[4,1,0,2],[2,0,0,0],[1,0,0,0],[5,0,1,1]]}"#,
        concat!(r#"{"version":1,"tool":"scip-typescript 0.4.0","symbols":["scip-typescript npm watch 1.0.0 src/`label.js`/","scip-typescript npm watch 1.0.0 src/`report.js`/","#,
            r#""scip-typescript npm @types/node 18.19.130 `module.d.ts`/`\"module\"`/global/require.","scip-typescript npm watch 1.0.0 test/`label.test.js`/","#,
            r#""scip-typescript npm watch 1.0.0 test/`report.test.js`/","scip-typescript npm typescript 5.9.3 lib/`lib.es5.d.ts`/String#length."]}"#),
    ].join("\n").into_bytes()
}
fn commonjs_imports(file: &str, line: usize) -> Option<bool> { Some(line == 0 && file != "src/label.js") }

#[test]
fn a_commonjs_module_whose_exports_the_index_cannot_see_is_reached_through_the_files_that_require_it() {
    let m = map::read(&commonjs_facts()).unwrap();
    assert_eq!(map::kind(CJS_LABEL), Kind::Module, "scip-typescript indexes exports.label = ... as nothing but the module itself");
    let r = map::reach(&m, &[("src/label.js".to_string(), vec![1])], &commonjs_imports);
    assert_eq!(r.hop(&m, CJS_LABEL).map(|h| h.hops), Some(0), "the changed line is inside no function or type the index knows, so the change is to the module");
    assert_eq!(r.hop(&m, CJS_REPORT).map(|h| (h.hops, h.via)), Some((1, CJS_LABEL.to_string())), "report.js requires label.js, so loading report.js runs the changed module");
    assert_eq!(r.files(&m), vec![
        ("src/label.js", 0, Touch::Changed),
        ("src/report.js", 1, Touch::Loads),
        ("test/label.test.js", 1, Touch::Tests),
        ("test/report.test.js", 2, Touch::Tests),
    ], "a require of a changed module runs it when the requiring file loads, and the test that requires report.js is two hops away");
    let c = map::counts(&m, &r);
    assert_eq!((c.changed_functions, c.changed_modules, c.running, c.loading, c.tests, c.farthest_hops), (0, 1, 0, 1, 2, 2));
    assert_eq!(map::neighbours(&m, &r, &["src/label.js".into()], 10), vec![("src/report.js".to_string(), 1)], "the untouched-tests watch gets report.js, which the last green farm run also watched");
    let report = map::report(&m, &r, vec![], 0);
    assert_eq!((report["changed_modules"].as_u64(), report["changed_functions"].as_u64(), report["nearest"].as_str()), (Some(1), Some(0), Some("src/report.js")));
}

#[test]
fn a_top_level_call_of_a_changed_function_runs_when_the_module_loads_and_the_files_that_load_that_module_run_it_too() {
    let facts = [
        r#"{"path":"src/label.js","occ":[[0,1,0,3],[1,1,1,1]]}"#,
        r#"{"path":"src/boot.js","occ":[[2,1,0,2],[0,0,0,0],[1,0,0,0],[1,0,1,1]]}"#,
        r#"{"path":"src/app.js","occ":[[3,1,0,2],[2,0,0,0]]}"#,
        concat!(r#"{"version":1,"tool":"scip-typescript 0.4.0","symbols":["scip-typescript npm watch 1.0.0 src/`label.js`/","scip-typescript npm watch 1.0.0 src/`label.js`/label().","#,
            r#""scip-typescript npm watch 1.0.0 src/`boot.js`/","scip-typescript npm watch 1.0.0 src/`app.js`/"]}"#),
    ].join("\n").into_bytes();
    let m = map::read(&facts).unwrap();
    let imports = |file: &str, line: usize| Some(line == 0 && file != "src/label.js");
    let r = map::reach(&m, &[("src/label.js".to_string(), vec![2])], &imports);
    assert_eq!(map::counts(&m, &r).changed_modules, 0, "line 2 is inside label(), so the module itself is unchanged");
    assert_eq!(r.touch(&m, "src/boot.js"), Some((1, Touch::Loads)), "boot.js calls label() at its top level, inside no function: the module itself runs the change when it loads");
    assert_eq!(r.touch(&m, "src/app.js"), Some((2, Touch::Loads)), "app.js imports boot.js, so loading app.js runs the change too");
    let r = map::reach(&m, &[("src/label.js".to_string(), vec![2])], &|_, _| None);
    assert_eq!(r.touch(&m, "src/boot.js"), Some((1, Touch::Mentions)), "without import positions a module-level reference stays a mention");
    assert_eq!(r.touch(&m, "src/app.js"), None);
}
