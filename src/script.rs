use crate::policy::Runner;
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation { pub runner: Runner, pub command: Vec<String>, pub env: BTreeMap<String, String>, pub before: Vec<String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found { Run(Invocation), Packages, Nothing }

const NAMES: [&str; 4] = ["test", "test:unit", "test:ci", "unit"];
const DEPTH: u32 = 4;
const STEPS: u32 = 256;
const ORCHESTRATORS: [&str; 6] = ["turbo", "lerna", "nx", "ultra", "moon", "wireit"];
const RUN_ALL: [&str; 3] = ["npm-run-all", "run-s", "run-p"];
const RESERVED: [&str; 6] = ["PATH", "HOME", "LANG", "PWD", "SHELL", "USER"];
const PACKAGED: [&str; 11] = ["--workspaces", "-ws", "--workspace", "--recursive", "-r", "--filter", "-F", "-C", "--dir", "--prefix", "--cwd"];
const NODE_VALUED: [&str; 10] = ["-r", "--require", "--import", "--loader", "--experimental-loader", "-C", "--conditions", "--env-file", "--title", "--input-type"];
const FILES: [&str; 17] = ["--config", "-c", "--require", "-r", "--import", "--loader", "--experimental-loader", "--file", "--setupFiles", "--setupFilesAfterEnv",
    "--globalSetup", "--globalTeardown", "--testEnvironment", "--testRunner", "--testSequencer", "--resolver", "--env-file"];

#[derive(Debug, Clone)]
struct Word { text: String, expands: bool }

enum Takes { Nothing, Value, Number, Maybe }

struct Reader<'a> { scripts: &'a Value, steps: Cell<u32> }

pub fn found(package: &Value) -> Found {
    let r = Reader { scripts: &package["scripts"], steps: Cell::new(STEPS) };
    let mut packages = false;
    for name in NAMES {
        if r.scripts[name].as_str().is_none() { continue; }
        match r.script(name, &[], 0) {
            Found::Run(i) => return Found::Run(i),
            Found::Packages => packages = true,
            Found::Nothing => {}
        }
        if name == "test" && packages { break; }
    }
    if packages { Found::Packages } else { Found::Nothing }
}

pub fn files(i: &Invocation) -> Vec<String> {
    let mut words: Vec<&str> = i.command.iter().skip(1).map(String::as_str).collect();
    words.extend(i.env.get("NODE_OPTIONS").map(|o| o.split_whitespace().collect::<Vec<_>>()).unwrap_or_default());
    let mut out = Vec::new();
    for (k, w) in words.iter().enumerate() {
        match w.split_once('=') {
            Some((flag, value)) if FILES.contains(&flag) => out.push(value.to_string()),
            None if FILES.contains(w) => out.extend(words.get(k + 1).map(|v| v.to_string())),
            _ => {}
        }
    }
    out.into_iter().filter(|v| !v.is_empty() && !v.starts_with('-')).collect()
}

pub const ENV_FILES: &str = ".kern-env-files";
fn env_files(words: &[Word], mut i: usize, file_flags: &[&str], valued: &[&str], env: &mut BTreeMap<String, String>) -> usize {
    let mut files = Vec::new();
    while i < words.len() && words[i].text.starts_with('-') {
        let w = &words[i].text;
        if w == "--" { i += 1; break; }
        let (flag, inline) = match w.split_once('=') { Some((f, v)) if f.starts_with("--") || f.len() == 2 => (f, Some(v.to_string())), _ => (w.as_str(), None) };
        let takes = file_flags.contains(&flag) || valued.contains(&flag);
        let value = if takes { inline.or_else(|| words.get(i + 1).map(|n| n.text.clone())) } else { None };
        if takes && inline_missing(w) { i += 1; }
        if let Some(v) = value {
            if file_flags.contains(&flag) { files.push(v); }
            else if flag == "-v" && let Some((k, x)) = v.split_once('=') && env_name(k) && x.len() <= 4096 { env.insert(k.to_string(), x.to_string()); }
        }
        i += 1;
    }
    if files.is_empty() { files.push(".env".to_string()); }
    env.insert(ENV_FILES.to_string(), files.join("\n"));
    i
}
fn inline_missing(w: &str) -> bool { !w.contains('=') }
pub fn env_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        && !RESERVED.contains(&name) && !name.starts_with("KERN_") && !name.starts_with("LD_") && !name.starts_with("DYLD_")
}

fn either(results: impl Iterator<Item = Found>) -> Found {
    let mut packages = false;
    for r in results {
        match r {
            Found::Run(i) => return Found::Run(i),
            Found::Packages => packages = true,
            Found::Nothing => {}
        }
    }
    if packages { Found::Packages } else { Found::Nothing }
}

impl Reader<'_> {
    fn spend(&self, depth: u32) -> bool {
        let left = self.steps.get();
        if left == 0 || depth > DEPTH { return false; }
        self.steps.set(left - 1);
        true
    }

    fn script(&self, name: &str, extra: &[Word], depth: u32) -> Found {
        let Some(text) = self.scripts[name].as_str() else { return Found::Nothing };
        if !self.spend(depth) { return Found::Nothing; }
        let mut commands = split(text);
        if let Some(last) = commands.last_mut() { last.extend(extra.iter().cloned()); }
        let mut before: Vec<String> = Some(format!("pre{name}")).filter(|p| self.scripts[p].is_string()).into_iter().collect();
        let mut packages = false;
        for c in &commands {
            match self.command(c, depth) {
                Found::Run(mut i) => { before.append(&mut i.before); i.before = before; return Found::Run(i); }
                Found::Packages => packages = true,
                Found::Nothing => before.extend(self.named(c)),
            }
        }
        if packages { Found::Packages } else { Found::Nothing }
    }

    fn named(&self, words: &[Word]) -> Option<String> {
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        let name = match texts.as_slice() {
            ["npm", "run" | "run-script", n, ..] | ["pnpm" | "yarn" | "bun", "run", n, ..] => *n,
            ["pnpm" | "yarn", n, ..] if !n.starts_with('-') => *n,
            _ => return None,
        };
        (self.scripts[name].is_string() && words.iter().all(|w| !w.expands)).then(|| name.to_string())
    }

    fn command(&self, words: &[Word], depth: u32) -> Found {
        if !self.spend(depth) { return Found::Nothing; }
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        let mut i = 0;
        loop {
            while let Some(w) = words.get(i) {
                let Some((name, value)) = assignment(w) else { break };
                if !w.expands && env_name(&name) && value.len() <= 4096 { env.insert(name, value); }
                i += 1;
            }
            let Some(w) = words.get(i) else { return Found::Nothing };
            let exec = words.get(i + 1).is_some_and(|n| n.text == "exec");
            i = match w.text.as_str() {
                "cross-env" | "time" => i + 1,
                "env" => skip_options(words, i + 1, &["-u", "--unset", "-C", "--chdir"]),
                "dotenv" => env_files(words, i + 1, &["-e"], &["-c", "-v", "-p"], &mut env),
                "env-cmd" => env_files(words, i + 1, &["-f", "--file"], &["-e", "--environments", "-r", "--rc-file"], &mut env),
                "npx" | "bunx" => skip_options(words, i + 1, &["-p", "--package", "-c", "--call"]),
                "c8" | "nyc" => skip_options(words, i + 1, &["-r", "--reporter", "--include", "--exclude", "-n", "-x", "-i", "--require", "-e", "--extension", "-o", "--temp-directory", "--report-dir", "--reports-dir", "--src"]),
                "npm" | "pnpm" | "yarn" | "bun" if exec => if words.get(i + 2).is_some_and(|n| n.text == "--") { i + 3 } else { i + 2 },
                _ => break,
            };
        }
        let rest = &words[i..];
        let program = rest[0].text.as_str();
        if let Some(runner) = runner_named(program) { return invocation(runner, None, &rest[1..], env); }
        match program {
            "node" => node(&rest[1..], env),
            "tsx" if rest[1..].iter().any(|w| w.text == "--test") => invocation(Runner::Node, Some("/workspace/node_modules/.bin/tsx"), &rest[1..], env),
            p if ORCHESTRATORS.contains(&p) => Found::Packages,
            p if RUN_ALL.contains(&p) => self.run_all(&rest[1..], depth),
            "npm" | "pnpm" | "yarn" | "bun" => self.manager(program, &rest[1..], depth, env),
            _ => Found::Nothing,
        }
    }

    fn run_all(&self, args: &[Word], depth: u32) -> Found {
        let valued = |k: usize| k > 0 && ["--npm-path", "--max-parallel"].contains(&args[k - 1].text.as_str());
        let names: Vec<String> = args.iter().enumerate().filter(|(k, w)| !(w.text.starts_with('-') || valued(*k))).flat_map(|(_, w)| match w.text.strip_suffix('*') {
            Some(prefix) => self.scripts.as_object().map(|o| o.keys().filter(|k| k.starts_with(prefix)).cloned().collect()).unwrap_or_default(),
            None => vec![w.text.clone()],
        }).collect();
        either(names.iter().map(|name| self.script(name, &[], depth + 1)))
    }

    fn manager(&self, program: &str, args: &[Word], depth: u32, env: BTreeMap<String, String>) -> Found {
        let packaged = |w: &Word| { let n = w.text.split_once('=').map_or(w.text.as_str(), |(n, _)| n); PACKAGED.contains(&n) || (program == "npm" && n == "-w") };
        if program == "yarn" && args.first().is_some_and(|w| w.text.starts_with("workspace")) { return Found::Packages; }
        let mut k = 0;
        while k < args.len() && args[k].text.starts_with('-') {
            if packaged(&args[k]) { return Found::Packages; }
            if !args[k].text.contains('=') && ["--loglevel", "--reporter"].contains(&args[k].text.as_str()) { k += 1; }
            k += 1;
        }
        let Some(first) = args.get(k) else { return Found::Nothing };
        let (name, rest) = match (program, first.text.as_str()) {
            ("npm", "test" | "t" | "tst") => ("test".to_string(), &args[k + 1..]),
            ("npm", "run" | "run-script") | ("pnpm" | "yarn" | "bun", "run") => {
                let mut j = k + 1;
                while j < args.len() && args[j].text.starts_with('-') {
                    if packaged(&args[j]) { return Found::Packages; }
                    j += 1;
                }
                let Some(n) = args.get(j) else { return Found::Nothing };
                (n.text.clone(), &args[j + 1..])
            }
            ("npm", _) | ("bun", "test") => return Found::Nothing,
            (_, n) => (n.to_string(), &args[k + 1..]),
        };
        let extra: Vec<Word> = if program == "npm" {
            let cut = rest.iter().position(|w| w.text == "--").unwrap_or(rest.len());
            if rest[..cut].iter().any(packaged) { return Found::Packages; }
            rest.get(cut + 1..).map(<[Word]>::to_vec).unwrap_or_default()
        } else {
            rest.iter().skip(usize::from(rest.first().is_some_and(|w| w.text == "--"))).cloned().collect()
        };
        let merged = |found: Found| match found {
            Found::Run(mut i) => { for (key, value) in env.clone() { i.env.entry(key).or_insert(value); } Found::Run(i) }
            other => other,
        };
        if self.scripts[&name].is_string() { return merged(self.script(&name, &extra, depth + 1)); }
        if program == "npm" { return Found::Nothing; }
        let mut words = vec![Word { text: name, expands: false }];
        words.extend(extra);
        merged(self.command(&words, depth + 1))
    }
}

fn split(text: &str) -> Vec<Vec<Word>> {
    let (mut commands, mut words, mut word): (Vec<Vec<Word>>, Vec<Word>, Option<Word>) = (Vec::new(), Vec::new(), None);
    let mut redirected = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let end_command = |words: &mut Vec<Word>, commands: &mut Vec<Vec<Word>>| { if !words.is_empty() { commands.push(std::mem::take(words)); } };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '#' if word.is_none() => { while i + 1 < chars.len() && chars[i + 1] != '\n' { i += 1; } }
            ' ' | '\t' => { if let Some(w) = word.take() && !redirected { words.push(w); } }
            '\n' | ';' | '&' | '|' | '(' | ')' => {
                if let Some(w) = word.take() && !redirected { words.push(w); }
                if c == '&' && i > 0 && chars[i - 1] == '>' { i += 1; continue; }
                end_command(&mut words, &mut commands);
                redirected = false;
            }
            '>' | '<' => {
                if word.as_ref().is_some_and(|w| w.text.bytes().all(|b| b.is_ascii_digit())) { word = None; }
                if let Some(w) = word.take() && !redirected { words.push(w); }
                redirected = true;
            }
            '\'' => {
                let w = word.get_or_insert(Word { text: String::new(), expands: false });
                i += 1;
                while i < chars.len() && chars[i] != '\'' { w.text.push(chars[i]); i += 1; }
            }
            '"' => {
                let w = word.get_or_insert(Word { text: String::new(), expands: false });
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() && matches!(chars[i + 1], '"' | '\\' | '$' | '`') { i += 1; w.text.push(chars[i]); }
                    else { if matches!(chars[i], '$' | '`') { w.expands = true; } w.text.push(chars[i]); }
                    i += 1;
                }
            }
            '\\' => {
                let w = word.get_or_insert(Word { text: String::new(), expands: false });
                if i + 1 < chars.len() { i += 1; if chars[i] != '\n' { w.text.push(chars[i]); } }
            }
            _ => {
                let w = word.get_or_insert(Word { text: String::new(), expands: false });
                if matches!(c, '$' | '`') { w.expands = true; }
                w.text.push(c);
            }
        }
        i += 1;
    }
    if let Some(w) = word.take() && !redirected { words.push(w); }
    end_command(&mut words, &mut commands);
    commands
}

fn assignment(w: &Word) -> Option<(String, String)> {
    let (name, value) = w.text.split_once('=')?;
    let valid = !name.is_empty() && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()));
    valid.then(|| (name.to_string(), value.to_string()))
}

fn base(program: &str) -> &str {
    let name = program.rsplit('/').next().unwrap_or(program);
    name.strip_suffix(".js").or_else(|| name.strip_suffix(".mjs")).or_else(|| name.strip_suffix(".cjs")).unwrap_or(name)
}

fn runner_named(program: &str) -> Option<Runner> {
    if program.contains('/') && !program.contains("node_modules/") { return None; }
    match base(program) { "vitest" => Some(Runner::Vitest), "jest" => Some(Runner::Jest), "mocha" | "_mocha" => Some(Runner::Mocha), _ => None }
}

fn skip_options(words: &[Word], mut i: usize, valued: &[&str]) -> usize {
    while i < words.len() && words[i].text.starts_with('-') {
        if words[i].text == "--" { return i + 1; }
        if !words[i].text.contains('=') && valued.contains(&words[i].text.as_str()) { i += 1; }
        i += 1;
    }
    i
}

fn node(args: &[Word], mut env: BTreeMap<String, String>) -> Found {
    let (mut flags, mut i, mut test) = (Vec::new(), 0, false);
    while i < args.len() && args[i].text.starts_with('-') {
        if args[i].text == "--test" { test = true; }
        flags.push(args[i].clone());
        if !args[i].text.contains('=') && NODE_VALUED.contains(&args[i].text.as_str()) && i + 1 < args.len() { i += 1; flags.push(args[i].clone()); }
        i += 1;
    }
    if test { return invocation(Runner::Node, Some("/usr/local/bin/node"), args, env); }
    let Some(path) = args.get(i) else { return Found::Nothing };
    let Some(runner) = runner_named(&path.text).filter(|_| path.text.contains("node_modules/")) else { return Found::Nothing };
    if flags.iter().any(|f| f.expands) { return Found::Nothing; }
    if !flags.is_empty() {
        let mut options = flags.iter().map(|f| f.text.clone()).collect::<Vec<_>>().join(" ");
        if let Some(had) = env.remove("NODE_OPTIONS") { options = format!("{had} {options}"); }
        env.insert("NODE_OPTIONS".into(), options);
    }
    invocation(runner, None, &args[i + 1..], env)
}

fn dropped(runner: Runner, flag: &str) -> Option<Takes> {
    let name = flag.split_once('=').map_or(flag, |(n, _)| n);
    let has = |list: &[&str]| list.contains(&name);
    let (valued, numbered, plain): (&[&str], &[&str], &[&str]) = match runner {
        Runner::Vitest => (&["--reporter", "--outputFile"], &["--bail"], &["--watch", "-w", "--ui", "--open", "--run", "-u", "--update", "--coverage"]),
        Runner::Jest => (&["--reporters", "--outputFile", "--testResultsProcessor", "--changedSince", "--coverageReporters", "--coverageDirectory", "--json-output"], &["--bail", "-b"],
            &["--watch", "--watchAll", "--coverage", "--collectCoverage", "--json", "--notify", "-u", "--updateSnapshot", "-o", "--onlyChanged", "--lastCommit", "--listTests"]),
        Runner::Mocha => (&["--reporter", "-R", "--reporter-option", "--reporter-options", "-O"], &[], &["--watch", "-w", "--bail", "-b"]),
        Runner::Node => (&["--test-reporter", "--test-reporter-destination"], &[], &["--experimental-test-coverage", "--watch", "--test-only"]),
    };
    let inline = flag.contains('=');
    if has(valued) { return Some(if inline { Takes::Nothing } else { Takes::Value }); }
    if has(numbered) { return Some(if inline { Takes::Nothing } else { Takes::Number }); }
    if runner == Runner::Vitest && name.starts_with("--coverage.") { return Some(if inline { Takes::Nothing } else { Takes::Maybe }); }
    has(plain).then_some(Takes::Nothing)
}

fn invocation(runner: Runner, program: Option<&str>, args: &[Word], env: BTreeMap<String, String>) -> Found {
    let mut args: Vec<Word> = args.to_vec();
    if runner == Runner::Vitest {
        match args.first().map(|w| w.text.as_str()) {
            Some("run" | "watch" | "dev") => { args.remove(0); }
            Some("bench" | "typecheck" | "list" | "init" | "related") => return Found::Nothing,
            _ => {}
        }
    }
    let mut kept: Vec<Word> = Vec::new();
    let mut k = 0;
    while k < args.len() {
        let w = &args[k];
        k += 1;
        if w.expands {
            if kept.last().is_some_and(|p| p.text.starts_with('-') && !p.text.contains('=')) { kept.pop(); }
            continue;
        }
        if w.text.starts_with('-') && w.text != "--" && let Some(takes) = dropped(runner, &w.text) {
            let next = args.get(k);
            let value = match takes {
                Takes::Nothing => false,
                Takes::Value | Takes::Maybe => next.is_some_and(|n| !n.text.starts_with('-')),
                Takes::Number => next.is_some_and(|n| !n.text.is_empty() && n.text.bytes().all(|b| b.is_ascii_digit())),
            };
            if value { k += 1; }
            continue;
        }
        kept.push(w.clone());
    }
    if let Some(at) = kept.iter().position(|w| w.text == "--") {
        if kept[at + 1..].iter().any(|w| w.text.starts_with('-')) { return Found::Nothing; }
        kept.remove(at);
    }
    let program = program.map(str::to_string).unwrap_or_else(|| format!("/workspace/node_modules/.bin/{}", match runner { Runner::Vitest => "vitest", Runner::Jest => "jest", Runner::Mocha => "mocha", Runner::Node => "node" }));
    let mut command = vec![program];
    if runner == Runner::Vitest { command.push("run".into()); }
    command.extend(kept.into_iter().map(|w| w.text));
    if command.len() > 56 || command.iter().any(|a| a.len() > 8192 || a.contains('\0')) || env.values().any(|v| v.contains('\0')) { return Found::Nothing; }
    Found::Run(Invocation { runner, command, env, before: Vec::new() })
}
