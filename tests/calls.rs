use serde_json::{Value, json};

type Imported = (String, Vec<(String, String)>, bool);
fn imported(path: &str, source: &str) -> Vec<Imported> {
    kern::calls::imports(path, source).into_iter().map(|i| (i.spec, i.bindings, i.esm)).collect()
}
fn pair(local: &str, name: &str) -> (String, String) { (local.to_string(), name.to_string()) }

#[test]
fn a_test_file_names_every_function_it_imports_in_either_module_style() {
    let esm = "import { removeComments, format as fmt } from '../src/sql';\nimport sql, * as all from '../src/sql';\nimport type { Query } from '../src/types';\nimport { type Row, count } from './rows';\nimport './setup';\n";
    assert_eq!(imported("test/sql.test.ts", esm), vec![
        ("../src/sql".to_string(), vec![pair("removeComments", "removeComments"), pair("fmt", "format")], true),
        ("../src/sql".to_string(), vec![pair("sql", "default"), pair("all", "*")], true),
        ("./rows".to_string(), vec![pair("count", "count")], true),
        ("./setup".to_string(), vec![], true),
    ], "type-only imports bring no function in");
    let cjs = "const { removeComments, format: fmt } = require('../src/sql');\nconst sqlLib = require('../src/sql');\nconst plain = require('../src/sql').default;\nasync function load() { const later = await import('../src/sql'); return later; }\n";
    assert_eq!(imported("test/sql.test.js", cjs), vec![
        ("../src/sql".to_string(), vec![pair("removeComments", "removeComments"), pair("fmt", "format")], false),
        ("../src/sql".to_string(), vec![pair("sqlLib", "*")], false),
        ("../src/sql".to_string(), vec![pair("plain", "default")], false),
        ("../src/sql".to_string(), vec![pair("later", "*")], true),
    ]);
    assert!(kern::calls::imports("test/broken.test.ts", "import { from").is_empty(), "a file Kern cannot parse imports nothing");
}

#[test]
fn only_calls_that_pass_plain_values_count_as_examples_of_how_a_function_is_used() {
    let files = ["src/sql.ts", "test/sql.test.ts"];
    let exists = |p: &str| files.contains(&p);
    let test = r#"import { removeComments as strip } from '../src/sql';
import * as sql from '../src/sql';
import { other } from '../src/other';
test('strips', async () => {
  expect(strip("SELECT 1 -- note")).toBe("SELECT 1 ");
  expect(sql.removeComments('a', { keep: true, depth: -2, list: [1, `x`, null], 'quoted key': 0.5 })).toBe('a');
  expect(await strip('awaited')).toBe('awaited');
  strip(query);
  strip(...parts);
  strip({ [name]: 1 });
  strip(`a ${b}`);
  other('not the target');
  sql.format('not the function');
});
"#;
    let found = kern::calls::literal_calls("test/sql.test.ts", test, ("src/sql.ts", "removeComments"), &exists);
    assert_eq!(found, vec![
        vec![json!("SELECT 1 -- note")],
        vec![json!("a"), json!({ "keep": true, "depth": -2, "list": [1, "x", null], "quoted key": 0.5 })],
        vec![json!("awaited")],
    ]);
    let commonjs = "const slug = require('../src/slug');\nconst { slugify } = require('../src/slug');\nslug('Hello World');\nslugify('a', 2);\n";
    let files = ["src/slug.js"];
    let exists = |p: &str| files.contains(&p);
    assert_eq!(kern::calls::literal_calls("test/slug.test.js", commonjs, ("src/slug.js", "default"), &exists), vec![vec![json!("Hello World")]],
        "a module whose whole export is a function is called through the name it was required as");
    assert_eq!(kern::calls::literal_calls("test/slug.test.js", commonjs, ("src/slug.js", "slugify"), &exists), vec![vec![json!("a"), json!(2)]]);
}

#[test]
fn an_alias_points_at_a_file_only_inside_the_package_that_owns_it() {
    let manifests = ["package.json", "apps/web/package.json", "apps/api/package.json", "apps/admin/package.json", "apps/shop/package.json"];
    let at = |importer: &str, spec: &str, target: &str| kern::calls::aliased(importer, spec, target, &|p: &str| manifests.contains(&p));
    assert!(at("apps/web/tests/page.test.ts", "@/lib/utils", "apps/web/src/lib/utils.ts"));
    assert!(at("src/app/page.test.tsx", "~/lib/format", "src/lib/format/index.ts"));
    assert!(at("test/a.test.ts", "src/lib/sql", "src/lib/sql.ts"), "a path from the project's base folder");
    assert!(!at("apps/api/tests/page.test.ts", "@/lib/utils", "apps/web/src/lib/utils.ts"), "another package's alias points into that package");
    assert!(!at("apps/shop/test/money.test.ts", "src/utils/money", "apps/admin/src/utils/money.ts"), "a base-folder path stays inside the package that owns the folder");
    assert!(at("apps/admin/test/money.test.ts", "src/utils/money", "apps/admin/src/utils/money.ts"));
    assert!(!at("test/a.test.ts", "lodash", "src/lodash.ts"), "a package name is not a path");
    assert!(!at("test/a.test.ts", "@scope/sql", "packages/sql/src/index.ts"), "a workspace package is loaded through its build, not its source");
    assert!(!at("test/a.test.ts", "./sql", "test/sql.ts"), "a relative path is resolved, not guessed");
    let files = ["apps/web/src/lib/utils.ts"];
    let exists = |p: &str| files.contains(&p);
    let test = "import { cn } from '@/lib/utils';\nexpect(cn('a', 'b')).toBe('a b');\n";
    assert_eq!(kern::calls::literal_calls("apps/web/tests/utils.test.ts", test, ("apps/web/src/lib/utils.ts", "cn"), &exists), vec![vec![json!("a"), json!("b")]]);
}

#[test]
fn lightdash_12942_a_test_case_that_calls_the_function_is_the_only_part_of_a_test_file_that_can_vouch_for_it() {
    let files = ["src/sql.ts"];
    let exists = |p: &str| files.contains(&p);
    let test = r#"import { removeComments, normalize } from '../src/sql';
import * as sql from '../src/sql';
describe('removeComments', () => {
  it('keeps -- inside quoted strings', () => {
    expect(removeComments("SELECT '--' AS a")).toBe("SELECT '--' AS a");
  });
  it.each([['a', 'a']])('keeps %s', (input, out) => { expect(sql.removeComments(input)).toBe(out); });
  test('trims', () => { expect(normalize(' a ')).toBe('a'); });
  test.concurrent('drops a comment', async () => { expect(removeComments('a -- b')).toBe('a '); });
});
"#;
    let spans = kern::calls::test_spans("test/sql.test.ts", test, ("src/sql.ts", "removeComments"), &exists);
    let quoted: Vec<&str> = spans.iter().map(|(s, e)| test[*s..*e].lines().next().unwrap()).collect();
    assert_eq!(quoted, ["it('keeps -- inside quoted strings', () => {", "it.each([['a', 'a']])('keeps %s', (input, out) => { expect(sql.removeComments(input)).toBe(out); })",
        "test.concurrent('drops a comment', async () => { expect(removeComments('a -- b')).toBe('a '); })"],
        "the import line, the describe title and a test of another function state nothing about removeComments");
    assert!(kern::calls::test_spans("test/sql.test.ts", test, ("src/other.ts", "removeComments"), &exists).is_empty(), "a name is the target's only when it is imported from the target");
}

#[test]
fn the_old_codes_plain_parameter_types_say_which_inputs_a_type_correct_caller_can_pass() {
    let declared = |source: &str, export: &str| kern::calls::declared_shapes("src/money.ts", source, export);
    let money = "export function formatPrice(cents: number, currency?: 'USD' | 'EUR', fill: string | null = null): string { return ''; }\n\
                 export const parse = (text: string, strict: boolean | undefined) => text;\n\
                 export function render(sql: string, options: { keepHints?: boolean } = {}): string { return sql; }\n\
                 export function lookup(user: User, id: string) { return id; }\n\
                 export function join(...parts: string[]) { return parts.join(''); }\n\
                 export function plain(a, b) { return a + b; }\n\
                 export function pick(a: string): string;\nexport function pick(a: number): number;\nexport function pick(a: any) { return a; }\n\
                 function format(x: any) { return x; }\nexport function formatAmount(cents: number): string { return String(format(cents)); }\nexport { formatAmount as format };\n\
                 export function tag(kind: 'a' | 'b', n: number) { return kind + n; }\n";
    assert_eq!(declared(money, "formatPrice"), vec![vec!["number"]], "a parameter typed with literal values admits only those values, which the shape cannot say, so the shapes stop before it");
    assert_eq!(declared(money, "parse"), vec![vec!["string"], vec!["string", "boolean"]], "a parameter that may be undefined may be left out");
    assert!(declared(money, "pick").is_empty(), "an overloaded function's implementation signature is not what callers see");
    assert_eq!(declared(money, "format"), vec![vec!["number"]], "an export is read under its local name, not an unrelated local of the exported name");
    assert!(declared(money, "tag").is_empty());
    assert_eq!(declared(money, "render"), vec![vec!["string"]], "an object type says nothing Kern can check about the fields, so the shapes stop before it");
    assert!(declared(money, "lookup").is_empty(), "a first parameter of a type Kern cannot read leaves no shape a caller is known to pass");
    assert!(declared(money, "join").is_empty() && declared(money, "plain").is_empty() && declared(money, "missing").is_empty());
    assert!(kern::calls::declared_shapes("src/money.ts", "export function {", "formatPrice").is_empty());
}

#[test]
fn the_shape_of_a_call_is_the_kind_of_each_value_it_passes() {
    let shape = |args: Value| kern::calls::shape(args.as_array().unwrap());
    assert_eq!(shape(json!(["a", 1, true, null, [], {}])), ["string", "number", "boolean", "null", "array", "object"]);
    assert_eq!(shape(json!([])), Vec::<&str>::new());
}
