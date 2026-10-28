use serde_json::Value;
use std::collections::BTreeSet;

pub const LINES_COUNT_MAX: usize = 2000;
pub const SCRIPT_DEPTH_MAX: usize = 3;
pub const TEXT_CHARS_MAX: usize = 2000;
pub const VALUE_CHARS_MAX: usize = 300;
const CONTROL: &[&str] = &["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "select", "function", "[[", "]]", "[", "{", "}", "!", "test"];
const INFO: &[&str] = &["echo", "printf", "ls", "pwd", "printenv", "which", "type", "command", "df", "du", "free", "nproc", "uname", "date", "whoami", "id", "hostname", "lscpu", "ulimit", "sleep", "true", ":", "exit", "tree", "head", "tail", "wc", "cat", "ps", "grep", "jq", "stat", "file", "realpath", "dirname", "basename", "sort", "uniq", "diff", "cmp", "awk", "sed", "find", "xargs", "tr", "cut"];
const FILE_OPS: &[&str] = &["cp", "mv", "ln", "mkdir", "touch", "rm", "chmod", "chown", "tee", "install", "rsync", "tar", "unzip", "gunzip", "envsubst", "patch"];
const FILE_WRITERS: &[&str] = &["cp", "mv", "ln", "touch", "tee", "install", "envsubst"];
const SERVICE_PROGRAMS: &[&str] = &["docker", "docker-compose", "podman", "redis-server", "pg_ctl", "pg_ctlcluster", "mongod", "mysqld", "service", "systemctl", "localstack", "minio", "clickhouse", "clickhouse-server"];
const SYSTEM_PROGRAMS: &[&str] = &["apt-get", "apt", "yum", "dnf", "apk", "brew", "choco", "snap", "pacman", "zypper", "dpkg"];
const OTHER_LANGUAGES: &[&str] = &["python", "python3", "pip", "pip3", "uv", "uvx", "poetry", "cargo", "rustup", "rustc", "go", "java", "mvn", "gradle", "ruby", "bundle", "gem", "php", "composer", "dotnet", "zig", "mise"];
const CHECK_PROGRAMS: &[&str] = &["eslint", "eslint_d", "prettier", "biome", "oxlint", "oxfmt", "knip", "stylelint", "markdownlint", "markdownlint-cli2", "cspell", "publint", "attw", "syncpack", "manypkg", "sherif", "depcheck", "madge", "dpdm", "lint-staged", "commitlint", "svelte-check", "size-limit", "type-coverage", "jscpd", "dprint", "actionlint", "shellcheck", "yamllint", "ultracite", "ls-lint", "license-checker", "tsgo"];
const BUILD_PROGRAMS: &[&str] = &["webpack", "rollup", "tsup", "esbuild", "unbuild", "rspack", "tsdown", "bunchee", "microbundle", "babel", "swc", "gulp", "grunt", "graphql-codegen", "gql-gen", "napi", "node-gyp", "wasm-pack", "electron-builder", "lage", "ts-patch", "tsc-alias", "paraglide-js", "panda"];
const BUILD_SUBCOMMANDS: &[(&str, &[&str])] = &[("vite", &["build"]), ("next", &["build"]), ("nuxi", &["build", "prepare", "generate"]), ("nuxt", &["build", "prepare", "generate"]), ("astro", &["build", "sync"]), ("svelte-kit", &["sync"]), ("prisma", &["generate"]), ("drizzle-kit", &["generate"]), ("rsbuild", &["build"]), ("storybook", &["build"]), ("expo", &["prebuild", "export"]), ("react-router", &["typegen", "build"]), ("wxt", &["build", "prepare", "zip"]), ("plasmo", &["build"]), ("vp", &["build"])];
const RUNNERS: &[(&str, &str, Option<&[&str]>)] = &[("vitest", "vitest", None), ("jest", "jest", None), ("mocha", "mocha", None), ("_mocha", "mocha", None), ("ts-mocha", "mocha", None), ("electron-mocha", "mocha", None), ("ava", "ava", None), ("tap", "tap", None), ("tape", "tape", None), ("jasmine", "jasmine", None), ("karma", "karma", None), ("uvu", "uvu", None), ("wdio", "wdio", None), ("lab", "lab", None), ("qunit", "qunit", None), ("web-test-runner", "web-test-runner", None), ("wtr", "web-test-runner", None), ("testcafe", "testcafe", None), ("nightwatch", "nightwatch", None), ("test-storybook", "storybook", None), ("playwright", "playwright", Some(&["test"])), ("cypress", "cypress", Some(&["run"])), ("react-scripts", "jest", Some(&["test"])), ("craco", "jest", Some(&["test"])), ("ng", "karma", Some(&["test"])), ("vue-cli-service", "jest", Some(&["test:unit"])), ("detox", "detox", Some(&["test"])), ("vp", "vite-plus", Some(&["test"])), ("deno", "deno", Some(&["test"])), ("start-server-and-test", "start-server-and-test", None), ("vitest-browser", "vitest", None)];
const ORCHESTRATORS: &[&str] = &["turbo", "nx", "lerna", "ultra", "moon", "wireit", "rush", "lage"];
const RUN_ALL: &[&str] = &["npm-run-all", "npm-run-all2", "run-s", "run-p", "concurrently"];
const TEST_WORDS: &[&str] = &["test", "tests", "spec", "specs", "e2e", "vitest", "jest", "unit"];
const BUILD_WORDS: &[&str] = &["build", "compile", "bundle", "codegen", "generate", "prepare", "prisma"];
const CHECK_WORDS: &[&str] = &["lint", "format", "fmt", "prettier", "typecheck", "type-check", "types:check", "check", "tsc", "knip", "spell", "biome", "unused", "circular", "madge", "depcruise", "deadcode", "dead-code", "audit", "i18n"];
const PM_VALUED: &[&str] = &["--filter", "-F", "-C", "--dir", "--prefix", "--cwd", "--workspace", "--reporter", "--loglevel", "--workspace-concurrency", "--registry", "--cache", "--store-dir", "--network-concurrency", "--child-concurrency", "--mode"];
const PM_FAN: &[&str] = &["-r", "--recursive", "--workspaces", "-ws"];
pub const KERN_RUNNERS: &[&str] = &["vitest", "jest", "mocha", "node"];
pub const E2E_RUNNERS: [&str; 8] = ["playwright", "cypress", "wdio", "testcafe", "nightwatch", "detox", "start-server-and-test", "storybook"];
const KIND_ORDER: &[&str] = &["test", "build", "install", "services", "system", "tool-install", "env-file", "file", "sets-env", "browser-install", "git-network", "download", "other-language", "shell-script", "node-script", "other", "setup", "check", "info", "noop"];
const SHELL_FEATURES: &[&str] = &["pipe", "or", "command-substitution", "backtick", "subshell", "heredoc", "background", "function", "arithmetic", "special-variable", "parameter-expansion", "unclosed-quote", "tilde", "brace-expansion", "control-flow"];

pub trait Packages { fn scripts(&self, folder: &str) -> Option<Value>; }

#[derive(Clone, Debug, Default)]
pub struct Word { pub text: String, pub expands: bool, pub glob: bool, pub quoted: bool, pub start_quoted: bool }
#[derive(Clone, Debug, Default)]
pub struct Seg { pub words: Vec<Word>, pub redirects: Vec<(String, String)>, pub sep: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct Cmd { pub kind: String, pub shape: Option<String>, pub runner: Option<String>, pub script: Option<String>, pub body: Option<String>, pub text: String, pub wd: Option<String>, pub program: Option<String>, pub target: Option<String>, pub test: bool, pub names: Vec<String>, pub args: Vec<String>, pub manager: Option<String>, pub tasks: Vec<String>, pub fan: Option<String>, pub subcommand: Option<String>, pub inner: Vec<(Option<String>, Option<String>)>, pub sudo: bool }
impl Cmd {
    pub fn of(kind: &str) -> Cmd { Cmd { kind: kind.into(), ..Cmd::default() } }
    fn with_program(kind: &str, program: &str) -> Cmd { Cmd { program: Some(program.into()), ..Cmd::of(kind) } }
    fn test(runner: &str, shape: String) -> Cmd { Cmd { runner: Some(runner.into()), shape: Some(shape), ..Cmd::of("test") } }
}
pub struct Scripts<'a> { pub packages: &'a dyn Packages, pub folder: Option<String> }
impl Scripts<'_> {
    fn body(&self, name: &str, folder: Option<&str>) -> Option<String> {
        let folder = folder.map(str::to_string).or_else(|| self.folder.clone())?;
        self.packages.scripts(&folder)?.get(name)?.as_str().map(str::to_string)
    }
    fn at<'b>(&'b self, folder: Option<String>) -> Scripts<'b> { Scripts { packages: self.packages, folder } }
    fn keys(&self) -> Vec<String> {
        self.folder.as_deref().and_then(|f| self.packages.scripts(f)).and_then(|s| s.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default()
    }
}

pub fn lower(s: &str) -> String { s.to_ascii_lowercase() }
fn is_sep(c: char) -> bool { matches!(c, ':' | '_' | '.' | '/' | '-') }
pub fn test_name(name: &str) -> bool {
    let l = lower(name);
    l == "t" || l == "tst" || l.split(is_sep).any(|w| TEST_WORDS.contains(&w))
}
pub fn build_name(name: &str) -> bool {
    let l = lower(name);
    BUILD_WORDS.iter().any(|w| l.contains(w)) || (l.starts_with("gen") && l[3..].chars().next().is_none_or(|c| matches!(c, ':' | '_' | '-')))
}
pub fn check_name(name: &str) -> bool { let l = lower(name); CHECK_WORDS.iter().any(|w| l.contains(w)) || l.contains("size-limit") }
pub fn test_script_file(stem: &str) -> bool {
    let l = lower(stem);
    let mut rest = l.as_str();
    if let Some(r) = rest.strip_prefix("run") { rest = r.strip_prefix(['-', '_']).unwrap_or(r); }
    while let Some(w) = ["integration", "unit", "all", "e2e"].iter().find(|w| rest.starts_with(*w)) {
        let after = &rest[w.len()..];
        let after = after.strip_prefix(['-', '_']).unwrap_or(after);
        if ["tests", "test", "vitest", "jest", "mocha"].iter().any(|c| after.starts_with(c)) || ["integration", "unit", "all", "e2e"].iter().any(|c| after.starts_with(c)) { rest = after; } else { break; }
    }
    for core in ["tests", "test", "vitest", "jest", "mocha"] {
        let Some(after) = rest.strip_prefix(core) else { continue };
        if after.is_empty() { return true; }
        let a = after.strip_prefix(['-', '_']).unwrap_or(after);
        if ["all", "runner", "suite", "ci", "unit", "integration"].contains(&a) { return true; }
    }
    false
}
pub fn env_file(path: &str) -> bool {
    let l = lower(path);
    let last = l.rsplit('/').next().unwrap_or("");
    (last.starts_with(".env") && last[4..].chars().all(|c| c == '.' || c == '-' || c == '_' || c.is_ascii_alphanumeric())) || l.ends_with(".env") || last == ".dev.vars"
}
pub fn service_script(text: &str) -> bool {
    let l = lower(text);
    has_word(&l, "docker") || has_word(&l, "redis-server") || l.contains("pg_ctl") && boundary_before(&l, "pg_ctl")
        || l.split_whitespace().collect::<Vec<_>>().windows(3).any(|w| w[0] == "supabase" && (w[1] == "start" || (w[1] == "db" && w[2] == "start")))
        || l.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w[0] == "supabase" && w[1] == "start")
}
fn word_char(c: u8) -> bool { c.is_ascii_alphanumeric() || c == b'_' }
fn boundary_before(text: &str, word: &str) -> bool { text.match_indices(word).any(|(i, _)| i == 0 || !word_char(text.as_bytes()[i - 1])) }
pub fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, m)| (i == 0 || !word_char(text.as_bytes()[i - 1])) && text.as_bytes().get(i + m.len()).is_none_or(|&b| !word_char(b)))
}
pub fn base(program: &str) -> String {
    let mut name = program.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
    if let Some(at) = name.char_indices().skip(1).find(|(_, c)| *c == '@').map(|(i, _)| i) { name.truncate(at); }
    for suffix in [".js", ".mjs", ".cjs", ".cmd", ".exe"] { if let Some(s) = name.strip_suffix(suffix) { name = s.to_string(); } }
    name
}
pub fn strip_ext(name: &str) -> String {
    for ext in [".mjs", ".mts", ".cjs", ".tsx", ".js", ".ts"] { if let Some(s) = name.strip_suffix(ext) { return s.to_string(); } }
    name.to_string()
}
pub fn script_ext(name: &str) -> bool { [".mjs", ".mts", ".cjs", ".tsx", ".js", ".ts"].iter().any(|e| name.ends_with(e)) }
pub fn is_assign(w: &str) -> bool {
    let Some(eq) = w.find('=') else { return false };
    let name = &w[..eq];
    !name.is_empty() && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
}
pub fn ident_prefix(s: &str) -> &str {
    let n = s.bytes().take_while(|&b| word_char(b)).count();
    if n == 0 || s.as_bytes()[0].is_ascii_digit() { "" } else { &s[..n] }
}
pub fn join_dir(base: Option<&str>, sub: Option<&str>) -> Option<String> {
    let Some(sub) = sub else { return base.map(str::to_string) };
    if sub.contains('\0') || sub.contains('$') || sub.starts_with('/') || sub.starts_with('~') { return None; }
    let base = base?;
    let mut parts: Vec<&str> = if base.is_empty() { Vec::new() } else { base.split('/').collect() };
    for x in sub.split('/') {
        match x { "" | "." => {}, ".." => { parts.pop()?; }, _ => parts.push(x) }
    }
    Some(parts.join("/"))
}

fn dollar(s: &[char], j: usize, w: &mut Word, feats: &mut BTreeSet<String>, names: &mut Vec<String>) -> usize {
    let n = s.len();
    let at = |k: usize| s.get(k).copied();
    let find = |from: usize, p: &[char]| (from..n).find(|&k| s[k..].starts_with(p));
    w.expands = true;
    if at(j + 1) == Some('(') && at(j + 2) == Some('(') {
        feats.insert("arithmetic".into());
        let k = find(j, &[')', ')']).map_or(n, |k| k + 2);
        w.text.extend(&s[j..k]);
        return k;
    }
    if at(j + 1) == Some('(') {
        feats.insert("command-substitution".into());
        let (mut depth, mut k) = (0, j + 1);
        while k < n { if s[k] == '(' { depth += 1; } else if s[k] == ')' { depth -= 1; if depth == 0 { break; } } k += 1; }
        w.text.extend(&s[j..(k + 1).min(n)]);
        return k + 1;
    }
    if at(j + 1) == Some('{') {
        let k = find(j, &['}']).unwrap_or(n - 1);
        let inner: String = s[(j + 2).min(k)..k].iter().collect();
        let id = ident_prefix(&inner);
        if !id.is_empty() { names.push(id.to_string()); if inner != id { feats.insert("parameter-expansion".into()); } } else { feats.insert("special-variable".into()); }
        w.text.extend(&s[j..(k + 1).min(n)]);
        return k + 1;
    }
    let rest: String = s[j + 1..].iter().collect();
    let id = ident_prefix(&rest);
    if !id.is_empty() {
        names.push(id.to_string());
        w.text.push('$');
        w.text.push_str(id);
        return j + 1 + id.chars().count();
    }
    if let Some(c) = at(j + 1) && "0123456789@*#?$!-".contains(c) {
        feats.insert("special-variable".into());
        w.text.push('$');
        w.text.push(c);
        return j + 2;
    }
    w.text.push('$');
    w.expands = false;
    j + 1
}

struct Lex { segs: Vec<Seg>, words: Vec<Word>, redirects: Vec<(String, String)>, cur: Option<Word>, pending: Option<String> }
impl Lex {
    fn flush(&mut self) {
        if let Some(cur) = self.cur.take() {
            if let Some(p) = self.pending.take() { self.redirects.push((p, cur.text)); } else { self.words.push(cur); }
        }
    }
    fn end(&mut self, op: Option<&str>) {
        self.flush();
        if !self.words.is_empty() || !self.redirects.is_empty() {
            self.segs.push(Seg { words: std::mem::take(&mut self.words), redirects: std::mem::take(&mut self.redirects), sep: op.map(str::to_string) });
        }
        self.words.clear();
        self.redirects.clear();
    }
    fn word(&mut self, quoted: bool) -> &mut Word {
        if self.cur.is_none() { self.cur = Some(Word { quoted, start_quoted: quoted, ..Word::default() }); }
        self.cur.as_mut().unwrap()
    }
}

fn brace_expansion(s: &[char], i: usize) -> bool {
    let mut comma = false;
    for &c in &s[i + 1..] {
        match c { '}' => return comma, ',' => comma = true, ' ' | '\t' | '{' => return false, _ => {} }
    }
    false
}

fn backtick(s: &[char], i: usize, w: &mut Word, feats: &mut BTreeSet<String>) -> usize {
    feats.insert("backtick".into());
    w.expands = true;
    let k = s[i + 1..].iter().position(|&c| c == '`').map_or(s.len() - 1, |k| i + 1 + k);
    w.text.extend(&s[i..k + 1]);
    k + 1
}
fn double_quoted(s: &[char], i: usize, w: &mut Word, feats: &mut BTreeSet<String>, names: &mut Vec<String>) -> usize {
    let n = s.len();
    w.quoted = true;
    let mut j = i + 1;
    while j < n && s[j] != '"' {
        match s[j] {
            '\\' if j + 1 < n => { w.text.push(s[j + 1]); j += 2; }
            '$' => j = dollar(s, j, w, feats, names),
            '`' => j = backtick(s, j, w, feats),
            c => { w.text.push(c); j += 1; }
        }
    }
    if j >= n { feats.insert("unclosed-quote".into()); }
    j + 1
}
fn redirect(s: &[char], i: usize, l: &mut Lex) -> usize {
    let n = s.len();
    match &l.cur { Some(cur) if !cur.text.is_empty() && cur.text.bytes().all(|b| b.is_ascii_digit()) && !cur.quoted => l.cur = None, _ => l.flush() }
    let mut j = i + 1;
    while j < n && "<>&|-".contains(s[j]) { j += 1; }
    let op: String = s[i..j].iter().collect();
    if op.ends_with('&') && j < n && s[j].is_ascii_digit() { l.redirects.push((op, s[j].to_string())); return j + 1; }
    l.pending = Some(op);
    j
}
pub fn lex_line(line: &str) -> (Vec<Seg>, BTreeSet<String>, Vec<String>) {
    let s: Vec<char> = line.chars().collect();
    let n = s.len();
    let mut feats = BTreeSet::new();
    let mut names = Vec::new();
    let mut l = Lex { segs: Vec::new(), words: Vec::new(), redirects: Vec::new(), cur: None, pending: None };
    let starts = |i: usize, p: &str| s[i..].iter().take(p.len()).copied().eq(p.chars());
    let mut i = 0;
    while i < n {
        let c = s[i];
        if c == ' ' || c == '\t' { l.flush(); i += 1; continue; }
        if c == '#' && l.cur.is_none() { break; }
        if starts(i, "&&") { l.end(Some("&&")); i += 2; continue; }
        if starts(i, "||") { l.end(Some("||")); feats.insert("or".into()); i += 2; continue; }
        if c == ';' { l.end(Some(";")); if starts(i, ";;") { feats.insert("control-flow".into()); i += 1; } i += 1; continue; }
        if c == '|' { l.end(Some("|")); feats.insert("pipe".into()); i += 1; continue; }
        if c == '&' && !starts(i, "&>") { l.end(Some("&")); feats.insert("background".into()); i += 1; continue; }
        if c == '<' || c == '>' || starts(i, "&>") { i = redirect(&s, i, &mut l); continue; }
        if c == '(' || c == ')' {
            if c == '(' && l.cur.is_some() && starts(i, "()") { feats.insert("function".into()); }
            l.end(Some(&c.to_string()));
            feats.insert("subshell".into());
            i += 1;
            continue;
        }
        if c == '\'' {
            let w = l.word(true);
            w.quoted = true;
            let j = s[i + 1..].iter().position(|&c| c == '\'').map_or_else(|| { feats.insert("unclosed-quote".into()); n }, |k| i + 1 + k);
            w.text.extend(&s[i + 1..j]);
            i = j + 1;
            continue;
        }
        if c == '"' { let mut w = l.word(true).clone(); i = double_quoted(&s, i, &mut w, &mut feats, &mut names); l.cur = Some(w); continue; }
        if c == '\\' { let w = l.word(false); if i + 1 < n { w.text.push(s[i + 1]); } i += 2; continue; }
        if c == '$' { let mut w = l.word(false).clone(); i = dollar(&s, i, &mut w, &mut feats, &mut names); l.cur = Some(w); continue; }
        if c == '`' { let mut w = l.word(false).clone(); i = backtick(&s, i, &mut w, &mut feats); l.cur = Some(w); continue; }
        let brace = c == '{' && brace_expansion(&s, i);
        let w = l.word(false);
        if "*?[".contains(c) { w.glob = true; }
        if c == '~' && w.text.is_empty() { feats.insert("tilde".into()); }
        if brace { feats.insert("brace-expansion".into()); }
        w.text.push(c);
        i += 1;
    }
    l.end(None);
    (l.segs, feats, names)
}

pub struct Block { pub segs: Vec<Seg>, pub feats: BTreeSet<String>, pub names: Vec<String>, pub exprs: Vec<String>, pub heredocs: usize }

pub fn expressions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("${{") {
        let Some(j) = rest[i..].find("}}") else { break };
        out.push(rest[i + 3..i + j].trim().to_string());
        rest = &rest[i + j + 2..];
        if out.len() >= 200 { break; }
    }
    out
}
fn keep_expressions(text: &str, exprs: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("${{") {
        let Some(j) = rest[i..].find("}}") else { break };
        out.push_str(&rest[..i]);
        exprs.push(rest[i..i + j + 2].to_string());
        out.push_str(&format!("\0{}\0", exprs.len() - 1));
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}
pub fn restore(text: &str, exprs: &[String]) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('\0') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i + 1..].find('\0') else { rest = &rest[i + 1..]; continue };
        if let Ok(k) = rest[i + 1..i + 1 + j].parse::<usize>() && let Some(e) = exprs.get(k) { out.push_str(e); }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}

pub fn run_block(text: &str) -> Block {
    let mut exprs = Vec::new();
    let t = keep_expressions(text, &mut exprs).replace("\r\n", "\n").replace("\\\n", " ");
    let lines: Vec<&str> = t.split('\n').collect();
    let mut block = Block { segs: Vec::new(), feats: BTreeSet::new(), names: Vec::new(), exprs: Vec::new(), heredocs: 0 };
    let mut i = 0;
    while i < lines.len() && i < LINES_COUNT_MAX {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() { continue; }
        let (segs, f, v) = lex_line(line);
        block.feats.extend(f);
        block.names.extend(v);
        for sg in &segs {
            for (op, target) in &sg.redirects {
                if op.starts_with("<<") && !op.starts_with("<<<") {
                    block.feats.insert("heredoc".into());
                    let delim = target.trim_matches(['\'', '"']);
                    while i < lines.len() && lines[i].trim() != delim { i += 1; }
                    i += 1;
                    block.heredocs += 1;
                }
            }
        }
        block.segs.extend(segs);
    }
    if i < lines.len() { block.feats.insert("cut-at-line-cap".into()); }
    block.exprs = exprs;
    block
}

fn skip_flags(ws: &[String], mut k: usize, valued: &[&str]) -> usize {
    while k < ws.len() && ws[k].starts_with('-') {
        let flag = ws[k].split('=').next().unwrap_or("");
        k += if valued.contains(&flag) && !ws[k].contains('=') { 2 } else { 1 };
    }
    k
}

fn strip_wrappers(mut ws: Vec<String>) -> (Vec<String>, BTreeSet<String>) {
    let mut flags = BTreeSet::new();
    for _ in 0..16 {
        if ws.is_empty() { break; }
        let p = base(&ws[0]);
        let cut = match p.as_str() {
            "sudo" => { flags.insert("sudo"); skip_flags(&ws, 1, &["-u", "-g"]) }
            "time" | "nice" | "nohup" | "stdbuf" | "sfw" => { if p == "sfw" { flags.insert("socket-firewall"); } 1 }
            "timeout" => skip_flags(&ws, 1, &["-s", "-k", "--signal", "--kill-after"]) + 1,
            "xvfb-run" => { flags.insert("xvfb"); skip_flags(&ws, 1, &["-s", "-n", "-e", "-f", "-p", "--server-args", "--server-num", "--error-file", "--auth-file"]) }
            "cross-env" | "env" => {
                flags.insert("env");
                let mut k = 1;
                while k < ws.len() && (is_assign(&ws[k]) || ws[k].starts_with('-')) { k += if ["-u", "--unset", "-C", "--chdir"].contains(&ws[k].as_str()) { 2 } else { 1 }; }
                k
            }
            "dotenv" | "env-cmd" => {
                flags.insert("dotenv");
                let k = skip_flags(&ws, 1, &["-e", "-c", "-v", "-p", "-f", "--file", "-r", "--rc-file", "--environments"]);
                if k < ws.len() && ws[k] == "--" { k + 1 } else { k }
            }
            "npx" | "bunx" | "pnpx" => { flags.insert("npx"); skip_flags(&ws, 1, &["-p", "--package", "-c", "--call"]) }
            "pnpm" | "yarn" if ws.len() > 1 && (ws[1] == "exec" || ws[1] == "dlx") => skip_flags(&ws, 2, &[]),
            "npm" if ws.len() > 1 && (ws[1] == "exec" || ws[1] == "x") => {
                let k = skip_flags(&ws, 2, &["-w", "--workspace", "-c", "--call", "-p", "--package"]);
                if k < ws.len() && ws[k] == "--" { k + 1 } else { k }
            }
            "bun" if ws.len() > 1 && ws[1] == "x" => 2,
            "c8" | "nyc" => { flags.insert("coverage"); skip_flags(&ws, 1, &["-r", "--reporter", "--include", "--exclude", "-n", "-x", "-i", "--require", "-e", "--extension", "-o", "--temp-directory", "--report-dir", "--reports-dir", "--src", "--all"]) }
            _ => break,
        };
        ws = ws.get(cut.min(ws.len())..).map(|s| s.to_vec()).unwrap_or_default();
    }
    (ws, flags.into_iter().map(str::to_string).collect())
}

pub fn merge_kind<'a>(kinds: impl Iterator<Item = &'a str>) -> String {
    kinds.filter_map(|k| KIND_ORDER.iter().position(|o| *o == k)).min().map_or("noop".to_string(), |i| KIND_ORDER[i].to_string())
}

pub fn classify_text(text: &str, scripts: &Scripts, depth: usize) -> (Vec<Cmd>, BTreeSet<String>) {
    let block = run_block(text);
    let mut found = Vec::new();
    for sg in &block.segs {
        let ws: Vec<String> = sg.words.iter().map(|w| restore(&w.text, &block.exprs)).collect();
        let k = ws.iter().take_while(|w| is_assign(w)).count();
        found.push(classify(&ws[k..], scripts, depth + 1));
    }
    (found, block.feats)
}

fn name_kind(name: &str) -> &'static str {
    if test_name(name) { "test" } else if build_name(name) { "build" } else if check_name(name) { "check" } else { "other" }
}

pub fn script_kind(name: &str, scripts: &Scripts, depth: usize, fan: Option<&str>, folder: Option<&str>) -> Cmd {
    if let Some(fan) = fan {
        let kind = name_kind(name);
        let mut out = Cmd { script: Some(name.into()), fan: Some(fan.into()), ..Cmd::of(kind) };
        if kind == "test" { out.shape = Some(format!("fan-out:{}", fan.split(':').next().unwrap_or(""))); }
        return out;
    }
    let body = if depth <= SCRIPT_DEPTH_MAX { scripts.body(name, folder) } else { None };
    let Some(body) = body else {
        let kind = name_kind(name);
        let mut out = Cmd { script: Some(name.into()), ..Cmd::of(kind) };
        if kind == "test" { out.shape = Some("script-unresolved".into()); }
        return out;
    };
    let inner_scripts = scripts.at(folder.map(str::to_string).or_else(|| scripts.folder.clone()));
    let (mut inner, _) = classify_text(&body, &inner_scripts, depth);
    let mut kind = merge_kind(inner.iter().map(|c| c.kind.as_str()));
    let custom = ["node-script", "other", "noop", "shell-script"].contains(&kind.as_str());
    if custom && test_name(name) {
        kind = "test".into();
        inner.push(Cmd::test("custom", format!("custom:{}", body.chars().take(60).collect::<String>())));
    } else if custom && build_name(name) { kind = "build".into(); } else if custom && check_name(name) { kind = "check".into(); }
    let runners: BTreeSet<String> = inner.iter().filter_map(|c| c.runner.clone()).collect();
    let shapes: BTreeSet<String> = inner.iter().filter_map(|c| c.shape.clone()).collect();
    if kind != "test" && test_name(name) && inner.is_empty() { kind = "test".into(); }
    let mut out = Cmd { script: Some(name.into()), body: Some(body.chars().take(VALUE_CHARS_MAX).collect()), ..Cmd::of(&kind) };
    if kind == "test" {
        let shape = if shapes.len() == 1 { shapes.iter().next().unwrap().clone() } else { let j = shapes.iter().cloned().collect::<Vec<_>>().join("|"); if j.is_empty() { "unknown".into() } else { j } };
        out.shape = Some(format!("script>{shape}"));
        out.runner = if runners.len() == 1 { runners.iter().next().cloned() } else { let j = runners.iter().cloned().collect::<Vec<_>>().join("|"); (!j.is_empty()).then_some(j) };
    }
    out
}

fn task_kind(tasks: &[String]) -> String {
    if tasks.is_empty() { return "other".into(); }
    merge_kind(tasks.iter().map(|t| name_kind(t.rsplit(':').next().unwrap_or(t))))
}

fn orchestrator(p: &str, args: &[String]) -> Vec<String> {
    let flag = |a: &String| a.starts_with('-');
    match p {
        "turbo" => {
            let k = usize::from(args.first().is_some_and(|a| a == "run"));
            args[k..].iter().take_while(|a| !flag(a)).cloned().collect()
        }
        "nx" => {
            if matches!(args.first().map(String::as_str), Some("run-many" | "affected")) {
                let mut tasks = Vec::new();
                for (k, a) in args.iter().enumerate() {
                    if ["-t", "--target", "--targets"].contains(&a.as_str()) {
                        tasks.extend(args[k + 1..].iter().take_while(|x| !flag(x)).flat_map(|x| x.split(',').map(str::to_string)));
                    } else if let Some(v) = ["--target=", "--targets=", "-t="].iter().find_map(|f| a.strip_prefix(f)) {
                        tasks.extend(v.split(',').map(str::to_string));
                    }
                }
                return tasks;
            }
            if args.first().is_some_and(|a| a == "run") && args.len() > 1 { return vec![args[1].rsplit(':').next().unwrap_or("").to_string()]; }
            if let Some(a) = args.first() && !flag(a) { return vec![a.split_once(':').map_or(a.clone(), |(_, t)| t.to_string())]; }
            Vec::new()
        }
        "lerna" | "ultra" | "lage" => {
            let k = usize::from(args.first().is_some_and(|a| ["run", "exec", "-r"].contains(&a.as_str())));
            args.get(k).filter(|a| !flag(a)).cloned().into_iter().collect()
        }
        "moon" => args.get(1).map(|a| a.rsplit(':').next().unwrap_or("").to_string()).into_iter().collect(),
        _ => args.first().filter(|a| !flag(a)).cloned().into_iter().collect(),
    }
}

fn pm_flags(p: &str, args: &[String], mut k: usize, mut folder: Option<String>, mut fan: Option<String>) -> (usize, Option<String>, Option<String>) {
    while k < args.len() && args[k].starts_with('-') {
        let a = &args[k];
        let (name, inline) = a.split_once('=').map_or((a.as_str(), None), |(n, v)| (n, Some(v.to_string())));
        let value = inline.clone().or_else(|| args.get(k + 1).cloned());
        if PM_FAN.contains(&name) { fan = fan.or(Some("recursive".into())); }
        if ["--filter", "-F", "--workspace"].contains(&name) || (p == "npm" && name == "-w") { fan = Some(format!("filter:{}", value.clone().unwrap_or_default())); }
        if (name == "-w" || name == "--workspace-root") && p == "pnpm" { folder = Some(String::new()); }
        if ["-C", "--dir", "--prefix", "--cwd"].contains(&name) && let Some(v) = &value { folder = Some(v.clone()); }
        k += if (PM_VALUED.contains(&name) || name == "--config" || (p == "npm" && name == "-w")) && inline.is_none() { 2 } else { 1 };
    }
    (k, folder, fan)
}

fn install_sub(p: &str, sub: &str) -> bool {
    match p { "npm" => ["ci", "install", "i", "in", "isnt", "instal", "add"].contains(&sub), "pnpm" => ["install", "i", "add", "fetch"].contains(&sub), "yarn" => ["install", "add"].contains(&sub), "bun" => ["install", "i", "add", "a"].contains(&sub), _ => false }
}
fn is_runner(name: &str) -> bool { RUNNERS.iter().any(|(p, _, _)| *p == name) }
fn build_sub(p: &str) -> Option<&'static [&'static str]> { BUILD_SUBCOMMANDS.iter().find(|(n, _)| *n == p).map(|(_, s)| *s) }
fn known_program(name: &str) -> bool {
    is_runner(name) || ORCHESTRATORS.contains(&name) || BUILD_PROGRAMS.contains(&name) || build_sub(name).is_some() || CHECK_PROGRAMS.contains(&name) || ["tsc", "vue-tsc", "node", "tsx", "playwright", "prisma"].contains(&name)
}

fn npm_run(rest: &[String], sub_scripts: &Scripts, depth: usize, mut fan: Option<String>) -> Cmd {
    let mut j = 0;
    while j < rest.len() && rest[j].starts_with('-') {
        let (n, v) = rest[j].split_once('=').map_or((rest[j].as_str(), None), |(n, v)| (n, Some(v.to_string())));
        if n == "--workspaces" || n == "-ws" { fan = fan.or(Some("workspaces".into())); }
        if n == "-w" || n == "--workspace" { fan = Some(format!("filter:{}", v.clone().or_else(|| rest.get(j + 1).cloned()).unwrap_or_default())); }
        j += if (n == "-w" || n == "--workspace") && v.is_none() { 2 } else { 1 };
    }
    if j >= rest.len() { return Cmd::of("info"); }
    let tail = &rest[j + 1..];
    let cut = tail.iter().position(|a| a == "--").unwrap_or(tail.len());
    if tail[..cut].iter().any(|a| ["--workspaces", "-ws", "-w", "--workspace"].contains(&a.split('=').next().unwrap_or(""))) { fan = fan.or(Some("workspaces".into())); }
    script_kind(&rest[j], sub_scripts, depth, fan.as_deref(), None)
}

fn package_manager(p: &str, args: &[String], scripts: &Scripts, depth: usize) -> Cmd {
    let (k, folder, mut fan) = pm_flags(p, args, 0, None, None);
    let sub = args.get(k).cloned();
    let rest: Vec<String> = args.get(k + 1..).map(|r| r.to_vec()).unwrap_or_default();
    let owned;
    let mut sub_scripts: &Scripts = if let Some(f) = &folder { owned = scripts.at(join_dir(scripts.folder.as_deref(), Some(f))); &owned } else { scripts };
    let Some(sub) = sub else { return if p == "yarn" { Cmd { manager: Some(p.into()), ..Cmd::of("install") } } else { Cmd::of("info") } };
    let sub = sub.as_str();
    if ["exec", "dlx", "x"].contains(&sub) {
        let j = skip_flags(&rest, 0, &["-c", "--call", "-p", "--package", "-w", "--workspace"]);
        let inner = if j < rest.len() && rest[j] == "--" { &rest[j + 1..] } else { &rest[j.min(rest.len())..] };
        let mut c = classify(inner, sub_scripts, depth + 1);
        if let Some(f) = &fan { c.fan = Some(f.clone()); if c.kind == "test" { c.shape = Some(format!("fan-out:{}>{}", f.split(':').next().unwrap_or(""), c.shape.clone().unwrap_or_default())); } }
        return c;
    }
    if install_sub(p, sub) {
        if rest.iter().any(|a| a == "-g" || a == "--global") { return Cmd { manager: Some(p.into()), ..Cmd::of("tool-install") }; }
        return Cmd { manager: Some(p.into()), ..Cmd::of("install") };
    }
    if p == "yarn" && sub == "global" { return Cmd { manager: Some(p.into()), ..Cmd::of("tool-install") }; }
    if ["-v", "--version", "version", "ls", "list", "why", "doctor", "ping", "whoami", "view", "info", "outdated", "licenses", "store", "help", "bin", "root", "prefix", "cache", "dedupe", "audit", "pm", "explain"].contains(&sub) {
        return Cmd::of(if ["audit", "dedupe", "outdated", "licenses"].contains(&sub) { "check" } else { "info" });
    }
    if ["config", "set", "policies", "plugin", "env", "setup", "self-update", "upgrade"].contains(&sub) {
        return Cmd { manager: Some(p.into()), ..Cmd::of(if sub == "config" { "setup" } else { "tool-install" }) };
    }
    if ["rebuild", "rb", "prune", "link", "pack", "publish", "patch", "deploy"].contains(&sub) {
        return Cmd { manager: Some(p.into()), ..Cmd::of(if ["rebuild", "rb", "pack"].contains(&sub) { "build" } else { "other" }) };
    }
    if p == "npm" {
        if ["test", "t", "tst"].contains(&sub) {
            let cut = rest.iter().position(|a| a == "--").unwrap_or(rest.len());
            if rest[..cut].iter().any(|a| ["--workspaces", "-ws", "-w", "--workspace"].contains(&a.split('=').next().unwrap_or(""))) { fan = fan.or(Some("workspaces".into())); }
            return script_kind("test", sub_scripts, depth, fan.as_deref(), None);
        }
        if ["run", "run-script", "rum", "urn"].contains(&sub) { return npm_run(&rest, sub_scripts, depth, fan); }
        return Cmd { manager: Some(p.into()), subcommand: Some(sub.into()), ..Cmd::of("other") };
    }
    if p == "yarn" && sub == "workspace" && rest.len() >= 2 {
        let fan = format!("workspace:{}", rest[0]);
        return if rest[1] != "run" { script_kind(&rest[1], sub_scripts, depth, Some(&fan), None) } else { script_kind(rest.get(2).map_or("", String::as_str), sub_scripts, depth, Some(&fan), None) };
    }
    if p == "yarn" && sub == "workspaces" {
        let name = rest.iter().find(|a| !a.starts_with('-') && a != &"foreach" && a != &"run").cloned().unwrap_or_default();
        return script_kind(&name, sub_scripts, depth, Some("workspaces"), None);
    }
    if p == "bun" && sub == "test" { return Cmd::test("bun", "runner:bun".into()); }
    let owned2;
    let (name, tail): (String, Vec<String>) = if sub == "run" || sub == "run-script" {
        let (j, folder2, fan2) = pm_flags(p, &rest, 0, folder.clone(), fan.clone());
        fan = fan2;
        if folder2 != folder && let Some(f2) = &folder2 { owned2 = scripts.at(join_dir(scripts.folder.as_deref(), Some(f2))); sub_scripts = &owned2; }
        if j >= rest.len() { return Cmd::of("info"); }
        (rest[j].clone(), rest[j + 1..].to_vec())
    } else if ["test", "t", "tst"].contains(&sub) { ("test".into(), rest.clone()) } else { (sub.into(), rest.clone()) };
    if p == "bun" && (name.starts_with("./") || script_ext(&name)) { return Cmd { target: Some(name), ..Cmd::of("node-script") }; }
    let body = sub_scripts.body(&name, None);
    let b = base(&name);
    if body.is_some() || fan.is_some() || (test_name(&name) && !is_runner(&b)) {
        if body.is_none() && fan.is_none() && is_runner(&b) {
            let mut ws = vec![name.clone()]; ws.extend(tail);
            return classify(&ws, sub_scripts, depth + 1);
        }
        return script_kind(&name, sub_scripts, depth, fan.as_deref(), None);
    }
    if known_program(&b) { let mut ws = vec![name.clone()]; ws.extend(tail); return classify(&ws, sub_scripts, depth + 1); }
    script_kind(&name, sub_scripts, depth, fan.as_deref(), None)
}

pub fn classify(ws: &[String], scripts: &Scripts, depth: usize) -> Cmd {
    if depth > SCRIPT_DEPTH_MAX + 3 { return Cmd::of("other"); }
    let (ws, flags) = strip_wrappers(ws.to_vec());
    if ws.is_empty() { return Cmd::of("noop"); }
    let p = base(&ws[0]);
    let mut out = classify_program(&p, &ws, &ws[1..], scripts, depth);
    if flags.contains("sudo") && out.kind != "system" && out.kind != "services" { out.sudo = true; }
    out
}

fn node_like(p: &str, args: &[String]) -> Cmd {
    if args.iter().any(|a| a == "--test" || a.starts_with("--test-") || a.starts_with("--test=")) { return Cmd::test("node", "runner:node".into()); }
    let target = args.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default();
    if p == "node" && (target.is_empty() || args.iter().any(|a| a == "-v" || a == "--version")) {
        return if args.is_empty() || args.iter().any(|a| ["-v", "--version", "-p", "--print"].contains(&a.as_str())) { Cmd::of("info") } else { Cmd::with_program("other", "node -e") };
    }
    let stem = strip_ext(&base(&target));
    if test_script_file(&stem) || tests_dir_runner(&target) { return Cmd::test("custom", format!("custom:{p} {target}")); }
    if build_name(&stem) { return Cmd::with_program("build", &format!("{p} {target}")); }
    if check_name(&stem) { return Cmd::with_program("check", &format!("{p} {target}")); }
    Cmd { target: Some(target), ..Cmd::of("node-script") }
}
fn tests_dir_runner(target: &str) -> bool {
    target.match_indices("test").any(|(i, _)| {
        let after = &target[i + 4..];
        let after = after.strip_prefix('s').unwrap_or(after);
        (i == 0 || target.as_bytes()[i - 1] == b'/') && after.starts_with('/') && { let seg = after[1..].split('/').next().unwrap_or(""); seg.contains("run") || seg.contains("all") || seg.contains("index") }
    })
}

fn run_all(p: &str, args: &[String], scripts: &Scripts, depth: usize) -> Cmd {
    if p == "concurrently" {
        let mut inner = Vec::new();
        for a in args.iter().filter(|a| !a.starts_with('-')) { inner.extend(classify_text(a, scripts, depth).0); }
        let kind = merge_kind(inner.iter().map(|c| c.kind.as_str()));
        return Cmd { shape: (kind == "test").then(|| "run-all:concurrently".into()), ..Cmd::with_program(&kind, p) };
    }
    let mut names = Vec::new();
    for (k, a) in args.iter().enumerate() {
        if a.starts_with('-') || (k > 0 && ["--npm-path", "--max-parallel"].contains(&args[k - 1].as_str())) { continue; }
        if let Some(prefix) = a.strip_suffix('*') { names.extend(scripts.keys().into_iter().filter(|x| x.starts_with(prefix))); } else { names.push(a.clone()); }
    }
    names.truncate(20);
    let inner: Vec<Cmd> = names.iter().map(|n| script_kind(n, scripts, depth + 1, None, None)).collect();
    let kind = merge_kind(inner.iter().map(|c| c.kind.as_str()));
    let tests: Vec<&Cmd> = inner.iter().filter(|c| c.kind == "test").collect();
    let mut out = Cmd { names: names.clone(), ..Cmd::with_program(&kind, p) };
    if kind == "test" {
        out.shape = Some(format!("run-all:{} test scripts", tests.len()));
        out.runner = Some(tests.iter().map(|c| c.runner.clone().unwrap_or("?".into())).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join("|"));
        out.inner = tests.iter().take(10).map(|c| (c.script.clone(), c.body.clone())).collect();
    }
    out
}

fn git_command(args: &[String]) -> Cmd {
    let sub = args.iter().find(|a| !a.starts_with('-')).map_or("", String::as_str);
    let (kind, program) = match sub {
        "fetch" | "pull" | "remote" => ("git-history", None),
        "submodule" | "lfs" => ("git-network", None),
        "clone" => ("git-clone", None),
        "init" | "checkout" | "switch" | "reset" | "clean" | "sparse-checkout" | "worktree" => ("git-checkout", None),
        "diff" | "status" => ("check", None),
        "config" | "log" | "rev-parse" | "show" | "branch" | "describe" | "ls-files" | "merge-base" => ("info", None),
        _ => ("other", Some(format!("git {sub}"))),
    };
    Cmd { subcommand: Some(sub.into()), program, ..Cmd::of(kind) }
}

fn classify_program(p: &str, ws: &[String], args: &[String], scripts: &Scripts, depth: usize) -> Cmd {
    fn first(a: &[String]) -> &str { a.first().map_or("", String::as_str) }
    if CONTROL.contains(&ws[0].as_str()) || ["then", "do", "done", "fi", "esac"].contains(&p) { return Cmd::of("control"); }
    match p {
        "cd" | "pushd" | "popd" => return Cmd { target: Some(first(args).into()), ..Cmd::of("cd") },
        "export" => return Cmd { names: args.iter().take(10).map(|a| a.split('=').next().unwrap_or("").to_string()).collect(), ..Cmd::of("export") },
        "set" | "shopt" | "unset" | "trap" | "alias" | "local" | "declare" | "readonly" => return Cmd::of("set"),
        "source" | "." => return Cmd { target: Some(first(args).into()), ..Cmd::of("source") },
        "npm" | "pnpm" | "yarn" | "bun" => return package_manager(p, args, scripts, depth),
        "corepack" => return Cmd { manager: Some("corepack".into()), subcommand: Some(first(args).into()), ..Cmd::of("tool-install") },
        _ => {}
    }
    if let Some((_, runner, needs)) = RUNNERS.iter().find(|(n, _, _)| *n == p) {
        if needs.is_none_or(|n| n.contains(&first(args))) { return Cmd::test(runner, format!("runner:{runner}")); }
        if p == "playwright" && first(args) == "install" { return Cmd::of("browser-install"); }
        if let Some(subs) = build_sub(p) && subs.contains(&first(args)) { return Cmd::with_program("build", p); }
        if p == "cypress" && ["install", "verify"].contains(&first(args)) { return Cmd::of("browser-install"); }
        return Cmd::with_program("other", p);
    }
    if ["node", "tsx", "ts-node", "bun"].contains(&p) { return node_like(p, args); }
    if ORCHESTRATORS.contains(&p) {
        let tasks = orchestrator(p, args);
        let kind = task_kind(&tasks);
        return Cmd { shape: (kind == "test").then(|| format!("orchestrator:{p}")), tasks: tasks.into_iter().take(10).collect(), ..Cmd::with_program(&kind, p) };
    }
    if RUN_ALL.contains(&p) { return run_all(p, args, scripts, depth); }
    if p == "tsc" || p == "vue-tsc" { return Cmd::with_program(if args.iter().any(|a| lower(a) == "--noemit") { "check" } else { "build" }, p); }
    if BUILD_PROGRAMS.contains(&p) { return Cmd::with_program("build", p); }
    if let Some(subs) = build_sub(p) {
        if subs.contains(&first(args)) { return Cmd::with_program("build", p); }
        if p == "astro" && first(args) == "check" { return Cmd::of("check"); }
        return Cmd::with_program("other", p);
    }
    if CHECK_PROGRAMS.contains(&p) { return Cmd::with_program("check", p); }
    if p == "git" { return git_command(args); }
    if SYSTEM_PROGRAMS.contains(&p) { return Cmd::with_program("system", p); }
    if SERVICE_PROGRAMS.contains(&p) || (p == "supabase" && ["start", "db"].contains(&first(args))) { return Cmd { args: args.iter().take(4).cloned().collect(), ..Cmd::with_program("services", p) }; }
    if p == "curl" || p == "wget" || p == "gh" { return Cmd::with_program("download", p); }
    if p == "wait-on" || p == "wait-port" { return Cmd::with_program("services", p); }
    if OTHER_LANGUAGES.contains(&p) { return Cmd::with_program("other-language", p); }
    if ["bash", "sh", "zsh", "make", "just"].contains(&p) || ws[0].ends_with(".sh") {
        let named = ["bash", "sh", "zsh", "make", "just"].contains(&p);
        let target = if named { args.iter().find(|a| !a.starts_with('-')).cloned().unwrap_or_default() } else { ws[0].clone() };
        let mut out = Cmd { target: Some(target.clone()), ..Cmd::with_program("shell-script", p) };
        if ["bash", "sh", "zsh"].contains(&p) && args.iter().any(|a| a == "-c") { return out; }
        let stem = base(&target);
        let stem = stem.strip_suffix(".sh").or_else(|| stem.strip_suffix(".bash")).unwrap_or(&stem);
        if test_script_file(stem) || ((p == "make" || p == "just") && test_name(&target)) { out.test = true; }
        return out;
    }
    if FILE_OPS.contains(&p) {
        let targets: Vec<String> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
        let kind = if FILE_WRITERS.contains(&p) && targets.iter().any(|t| env_file(t)) { "env-file" } else { "file" };
        return Cmd { args: targets.into_iter().take(4).collect(), ..Cmd::with_program(kind, p) };
    }
    if INFO.contains(&p) || p == "env" { return Cmd::with_program("info", p); }
    if ["wait", "kill", "pkill", "jobs", "disown"].contains(&p) { return Cmd::with_program("control", p); }
    Cmd::with_program("other", p)
}

pub fn needs_shell(feats: &BTreeSet<String>) -> BTreeSet<String> {
    feats.iter().filter(|f| SHELL_FEATURES.contains(&f.as_str()) || f.as_str() == "cut-at-line-cap").cloned().collect()
}
