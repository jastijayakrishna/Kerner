use kern::map;
use protobuf::Message;
use scip::types::{Document, Index, Metadata, Occurrence, ToolInfo};

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
const OPTIONS: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/helpers/`scheme.ts`/options().";
const LABEL: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/helpers/`scheme.ts`/label().";
const VERSION: &str = "scip-typescript npm @acme/ui 1.0.0 src/`version.ts`/version.";
const SELECTOR: &str = "scip-typescript npm @acme/ui 1.0.0 src/blocks/auth/`AuthSelector.vue.ts`/setup().";
const BANNER: &str = "scip-typescript npm @acme/ui 1.0.0 src/`Banner.ts`/banner().";

fn scalar_like() -> Vec<u8> {
    let mut index = Index::new();
    let mut meta = Metadata::new();
    let mut tool = ToolInfo::new();
    tool.name = "scip-typescript".into();
    tool.version = "0.4.0".into();
    meta.tool_info = protobuf::MessageField::some(tool);
    index.metadata = protobuf::MessageField::some(meta);
    index.documents = vec![
        doc("src/blocks/auth/helpers/scheme.ts", vec![occ(OPTIONS, &[0, 16, 0, 23], 1, &[0, 0, 2, 1]), occ(LABEL, &[3, 16, 3, 21], 1, &[3, 0, 3, 60])]),
        doc("src/version.ts", vec![occ(VERSION, &[0, 13, 0, 20], 1, &[])]),
        doc("src/index.ts", vec![occ(OPTIONS, &[0, 9, 0, 16], 2, &[]), occ(LABEL, &[0, 18, 0, 23], 2, &[]), occ(VERSION, &[1, 9, 1, 16], 2, &[])]),
        doc("src/blocks/auth/AuthSelector.vue.ts", vec![occ(OPTIONS, &[1, 9, 1, 16], 2, &[]), occ(SELECTOR, &[3, 9, 3, 14], 1, &[3, 0, 6, 1]), occ(OPTIONS, &[4, 12, 4, 19], 0, &[])]),
        doc("src/Banner.ts", vec![occ(VERSION, &[0, 9, 0, 16], 2, &[]), occ(BANNER, &[2, 16, 2, 22], 1, &[2, 0, 4, 1]), occ(VERSION, &[3, 20, 3, 27], 0, &[])]),
    ];
    index.write_to_bytes().unwrap()
}

#[test]
fn a_change_to_one_function_reaches_the_files_that_use_that_symbol_and_not_everything_that_imports_the_barrel() {
    let m = map::read(&scalar_like()).unwrap();
    assert_eq!(m.provenance, "scip-typescript 0.4.0");
    assert_eq!((m.files.len(), m.defined_in.len()), (5, 5));
    assert_eq!(m.defined_in[OPTIONS], "src/blocks/auth/helpers/scheme.ts");
    let r = map::reach(&m, &[("src/blocks/auth/helpers/scheme.ts".into(), vec![2])]);
    assert_eq!(r.hops.get(OPTIONS).map(|x| x.0), Some(0), "line 2 of the file is inside options(): {:?}", r.hops);
    assert_eq!(r.hops.get(LABEL), None, "label() on line 4 is not changed");
    assert_eq!(r.hops.get(SELECTOR), Some(&(1, OPTIONS.to_string())), "the Vue setup() calls options(), so it is reached in one hop");
    assert_eq!(r.hops.get("file:src/index.ts"), Some(&(1, OPTIONS.to_string())), "the barrel re-exports options(), so the barrel is reached as a file");
    assert_eq!(r.hops.get(BANNER), None, "Banner.ts imports version() from the same barrel and never uses options(): a file-level map reaches it, a symbol-level map does not");
    assert_eq!(r.files, [("src/blocks/auth/helpers/scheme.ts".to_string(), 0), ("src/blocks/auth/AuthSelector.vue.ts".to_string(), 1), ("src/index.ts".to_string(), 1)].into_iter().collect());
    let r = map::reach(&m, &[("src/blocks/auth/helpers/scheme.ts".into(), vec![])]);
    assert_eq!(r.hops.get(LABEL).map(|x| x.0), Some(0), "with no line information every definition in the file counts as changed");
    assert_eq!(map::neighbours(&r, &["src/blocks/auth/helpers/scheme.ts".into()], 10), vec![("src/blocks/auth/AuthSelector.vue.ts".to_string(), 1), ("src/index.ts".to_string(), 1)]);
    let j = map::reach_json(&m, &r, &[("src/blocks/auth/helpers/scheme.ts".into(), vec![])]);
    assert_eq!((j["files_reached"].as_u64(), j["resolution"].as_str(), j["evidence"].as_str()), (Some(3), Some("resolved"), Some("scip-typescript 0.4.0")), "every relationship names how it was found: {j}");
    let c = map::coverage(&m, &["src/index.ts".into(), "src/missing.ts".into()]);
    assert_eq!((c["indexed"].as_u64(), c["not_indexed"][0].as_str()), (Some(1), Some("src/missing.ts")), "a file the indexer did not cover is said, never assumed empty");
}

#[test]
fn an_index_too_large_or_unreadable_is_refused_with_a_reason() {
    let e = map::read(b"not an index").err().expect("bytes that are not an index are refused");
    assert!(e.0.starts_with("The code index could not be read"), "{}", e.0);
    let big = vec![0u8; map::INDEX_BYTES_MAX + 1];
    assert!(map::read(&big).err().is_some_and(|e| e.0.contains("larger than")));
}

#[test]
fn the_indexer_is_told_how_the_project_is_laid_out() {
    let has = |files: &[&str]| { let f: Vec<String> = files.iter().map(|s| s.to_string()).collect(); move |p: &str| f.iter().any(|x| x == p) };
    assert_eq!(map::flags(&has(&["pnpm-lock.yaml", "pnpm-workspace.yaml", "tsconfig.json"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--pnpm-workspaces"]);
    assert_eq!(map::flags(&has(&["yarn.lock", "package.json.workspaces"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--yarn-workspaces", "--infer-tsconfig"]);
    assert_eq!(map::flags(&has(&["package-lock.json"])), vec!["index", "--no-global-caches", "--output", "/tmp/.kern-report.json", "--infer-tsconfig"]);
}
