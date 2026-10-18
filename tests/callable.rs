fn names(path: &str, source: &str) -> Option<Vec<String>> { kern::trace::callable(path, source).map(|s| s.into_iter().collect()) }

#[test]
fn the_functions_kern_can_call_are_the_exports_its_recorder_watches() {
    let esm = "export function a() {}\nexport const b = (x: number) => x;\nconst c = function () {};\nexport { c as d };\nexport const e = 5;\nexport class F { run() {} }\nexport async function* g() {}\nexport declare function h(): void;\nexport function i(x: string): void;\nexport function i(x: unknown) {}\n";
    assert_eq!(names("src/m.ts", esm).unwrap(), ["a", "b", "d", "g", "i"], "values and classes are not functions to call; a declaration has no body");
    assert_eq!(names("src/n.js", "function slug() {}\nexport default slug;\n").unwrap(), ["default"]);
    assert_eq!(names("src/o.js", "export default function () {}\n").unwrap(), Vec::<String>::new(), "an unnamed default function is not one the recorder can wrap");
    assert_eq!(names("src/slug.ts", "export default function slug(title: string) { return title; }\nexport function other() {}\n").unwrap(), ["default", "other"],
        "importers call a named default function as default, so Kern calls and records it under that name");
    let cjs = "function a() {}\nconst five = 5;\nmodule.exports = { a, b: () => 1, c() {}, d: five, get e() { return 1; } };\nexports.x = function () {};\nmodule.exports.y = async () => 0;\n";
    assert_eq!(names("src/p.js", cjs).unwrap(), ["a", "b", "c", "x", "y"]);
    assert_eq!(names("src/q.cjs", "module.exports = function slug() {};\n").unwrap(), ["default"]);
    assert_eq!(names("src/r.js", "function slug() {}\nmodule.exports = slug;\n").unwrap(), ["default"]);
    assert_eq!(names("src/s.ts", "export function {").map(|n| n.len()), None, "a file Kern cannot parse has no answer");
    assert_eq!(names("README.md", "# a").map(|n| n.len()), None);
}
