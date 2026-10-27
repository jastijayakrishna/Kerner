use kern::git::{self, Entry, Files};
use kern::house;
use kern::util::*;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-house-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn entry(text: &str) -> Entry { let b = text.as_bytes().to_vec(); Entry { mode: "100644".into(), oid: git::object_id("blob", &b), size: b.len() as u64, bytes: Some(b) } }
fn tree(list: &[(&str, &str)]) -> Files { list.iter().map(|(p, t)| (p.to_string(), entry(t))).collect() }

const SCALAR_LIKE: &[(&str, &str)] = &[
    ("package.json", r#"{ "name": "root", "private": true, "workspaces": ["packages/*"] }"#),
    ("packages/ui/package.json", r#"{ "name": "@acme/ui", "exports": { ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" } } }"#),
    ("packages/ui/src/index.ts", "export * from './blocks/auth'\nexport { version } from './version'\n"),
    ("packages/ui/src/version.ts", "export const version = '1'\n"),
    ("packages/ui/src/blocks/auth/index.ts", "export { default as AuthSelector } from './AuthSelector.vue'\n"),
    ("packages/ui/src/blocks/auth/AuthSelector.vue", "<template><div /></template>\n<script setup lang=\"ts\">\nimport { options } from '@/blocks/auth/helpers/scheme'\nimport { ref } from 'vue'\nimport './style.css'\nconst o = ref(options([]))\n</script>\n"),
    ("packages/ui/src/blocks/auth/helpers/scheme.ts", "export function options(schemes: string[]): string[] { return schemes.filter(Boolean) }\nexport function label(s: string) { return s.toUpperCase() }\n"),
    ("packages/ui/src/blocks/auth/helpers/scheme.test.ts", "import { options } from './scheme'\nit('filters', () => expect(options([''])).toEqual([]))\n"),
    ("packages/ui/src/blocks/auth/style.css", ".a{}\n"),
    ("packages/cli/package.json", r#"{ "name": "@acme/cli", "bin": { "acme": "./dist/main.js" } }"#),
    ("packages/cli/src/main.ts", "import { AuthSelector } from '@acme/ui'\nconsole.log(AuthSelector)\n"),
];

#[test]
fn scalar_9592_the_house_reaches_the_auth_selector_room_from_its_helper_through_a_vue_file_an_alias_and_a_barrel() {
    let t = Temp::new();
    let dir = git::init_store(&t.0.join("repo.git")).unwrap();
    let base = git::write_commit(&dir, &tree(SCALAR_LIKE), None, "base").unwrap();
    let h = house::build(&dir, &base).unwrap();
    assert_eq!(h.exports.keys().cloned().collect::<Vec<_>>(), vec!["packages/cli/src/main.ts", "packages/ui/src/blocks/auth/AuthSelector.vue", "packages/ui/src/blocks/auth/helpers/scheme.ts", "packages/ui/src/blocks/auth/index.ts", "packages/ui/src/index.ts", "packages/ui/src/version.ts"], "product code only: no tests, no css, no manifests");
    assert_eq!(h.exports["packages/ui/src/blocks/auth/helpers/scheme.ts"].iter().cloned().collect::<Vec<_>>(), vec!["label", "options"]);
    let edge = |from: &str, to: &str| h.imports[from].contains(to);
    assert!(edge("packages/ui/src/index.ts", "packages/ui/src/blocks/auth/index.ts"), "a barrel's `export * from` is an edge: {:?}", h.imports);
    assert!(edge("packages/ui/src/index.ts", "packages/ui/src/version.ts"), "a named re-export is an edge");
    assert!(edge("packages/ui/src/blocks/auth/index.ts", "packages/ui/src/blocks/auth/AuthSelector.vue"), "a .vue default re-export is an edge");
    assert!(edge("packages/ui/src/blocks/auth/AuthSelector.vue", "packages/ui/src/blocks/auth/helpers/scheme.ts"), "an `@/` alias inside a Vue script block resolves inside its package");
    assert!(edge("packages/cli/src/main.ts", "packages/ui/src/index.ts"), "a workspace package name resolves to the package's source entry");
    assert_eq!(h.rooms.get("packages/ui/src/index.ts").map(String::as_str), Some("package @acme/ui"));
    assert_eq!(h.rooms.get("packages/cli/src/main.ts").map(String::as_str), Some("command acme"));
    assert_eq!(h.unresolved.get("npm"), Some(&1), "only `vue` is outside the house; the css import is not code: {:?}", h.unresolved);
    assert_eq!(h.unresolved.get("relative"), None);
    let changed = vec!["packages/ui/src/blocks/auth/helpers/scheme.ts".to_string()];
    let r = house::reach(&h, &changed);
    assert_eq!(r.hops.len(), 5, "the helper, the Vue file, the block barrel, the package entry and the command: {:?}", r.hops.keys().collect::<Vec<_>>());
    assert_eq!(r.path("packages/ui/src/index.ts"), vec!["packages/ui/src/index.ts", "packages/ui/src/blocks/auth/index.ts", "packages/ui/src/blocks/auth/AuthSelector.vue", "packages/ui/src/blocks/auth/helpers/scheme.ts"]);
    assert_eq!(r.rooms.iter().map(|(f, k, d)| (f.as_str(), k.as_str(), *d)).collect::<Vec<_>>(), vec![("packages/cli/src/main.ts", "command acme", 4), ("packages/ui/src/index.ts", "package @acme/ui", 3)]);
    assert_eq!(house::neighbours(&h, &changed, 2), vec![("packages/ui/src/blocks/auth/index.ts".to_string(), 2), ("packages/ui/src/index.ts".to_string(), 3)], "the files Kern watches next to a change: nearest first, never a Vue file (the recorder cannot rewrite it), never the change itself, capped");
    let r = house::reach(&h, &["packages/ui/src/version.ts".to_string()]);
    assert_eq!((r.hops.len(), r.rooms.len()), (3, 2), "a change to version.ts reaches the package entry and the command only");
    assert_eq!((h.imported("packages/ui/src/index.ts", 0), h.imported("packages/ui/src/index.ts", 1), h.imported("packages/ui/src/blocks/auth/helpers/scheme.ts", 0)), (Some(true), Some(true), Some(false)), "the house knows which lines are import or re-export statements");
    assert_eq!((h.imported("packages/ui/src/blocks/auth/AuthSelector.vue", 2), h.imported("packages/ui/src/missing.ts", 0)), (None, None), "a Vue file or a file outside the house gives no answer, never a guess");
}

#[test]
fn import_statements_are_known_by_line_so_a_top_level_call_is_told_apart_from_an_import() {
    let source = "import {\n  a,\n  b\n} from './x'\nexport * from './y'\nexport { c } from './z'\nconst q = require('./w')\nexport { a }\nconst r = a()\nexport function f() { return require('./lazy') }\nrequire('./side-effect')\nconst { d } = require('./v').inner\n";
    assert_eq!(house::import_lines("src/m.ts", source), vec![(0, 3), (4, 4), (5, 5), (6, 6), (10, 10), (11, 11)], "a multi-line import, both re-export forms and require() at the top level; a local export list, a top-level call and a require() inside a function are not imports");
    assert_eq!(house::import_lines("src/m.ts", "const = ;"), Vec::<(u32, u32)>::new(), "a file that does not parse has no known import lines");
}
