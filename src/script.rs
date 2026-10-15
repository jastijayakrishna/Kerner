use crate::policy::Runner;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation { pub runner: Runner, pub command: Vec<String>, pub env: BTreeMap<String, String> }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found { Run(Invocation), Packages, Nothing }

const NAMES: [&str; 4] = ["test", "test:unit", "test:ci", "unit"];
const DEPTH: u8 = 4;
const ORCHESTRATORS: [&str; 6] = ["turbo", "lerna", "nx", "ultra", "moon", "wireit"];
const RUN_ALL: [&str; 3] = ["npm-run-all", "run-s", "run-p"];
const RESERVED: [&str; 6] = ["PATH", "HOME", "LANG", "PWD", "SHELL", "USER"];

#[derive(Debug, Clone)]
struct Word { text: String, expands: bool }

pub fn found(package: &Value) -> Found {
    let scripts = &package["scripts"];
    let mut packages = false;
    for name in NAMES {
        if scripts[name].as_str().is_none() { continue; }
        match script(scripts, name, &[], 0) {
            Found::Run(i) => return Found::Run(i),
            Found::Packages => packages = true,
            Found::Nothing => {}
        }
        if name == "test" && packages { break; }
    }
    if packages { Found::Packages } else { Found::Nothing }
}

pub fn env_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().enumerate().all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
        && !RESERVED.contains(&name) && !name.starts_with("KERN_") && !name.starts_with("LD_") && !name.starts_with("DYLD_")
}

fn script(scripts: &Value, name: &str, extra: &[Word], depth: u8) -> Found {
    let Some(text) = scripts[name].as_str() else { return Found::Nothing };
    if depth >= DEPTH { return Found::Nothing; }
    let mut commands = split(text);
    if let Some(last) = commands.last_mut() { last.extend(extra.iter().cloned()); }
    let mut packages = false;
    for c in commands {
        match command(scripts, &c, depth) {
            Found::Run(i) => return Found::Run(i),
            Found::Packages => packages = true,
            Found::Nothing => {}
        }
    }
    if packages { Found::Packages } else { Found::Nothing }
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

fn command(scripts: &Value, words: &[Word], depth: u8) -> Found {
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
            "dotenv" => skip_options(words, i + 1, &["-e", "-c", "-v", "-p"]),
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
        p if RUN_ALL.contains(&p) => run_all(scripts, &rest[1..], depth),
        "npm" | "pnpm" | "yarn" | "bun" => manager(scripts, program, &rest[1..], depth, env),
        _ => Found::Nothing,
    }
}

fn run_all(scripts: &Value, args: &[Word], depth: u8) -> Found {
    let valued = |k: usize| k > 0 && ["--npm-path", "--max-parallel"].contains(&args[k - 1].text.as_str());
    let names = args.iter().enumerate().filter(|(k, w)| !(w.text.starts_with('-') || valued(*k)));
    let mut packages = false;
    for (_, w) in names {
        let matching: Vec<String> = match w.text.strip_suffix('*') {
            Some(prefix) => scripts.as_object().map(|o| o.keys().filter(|k| k.starts_with(prefix)).cloned().collect()).unwrap_or_default(),
            None => vec![w.text.clone()],
        };
        for name in matching {
            match script(scripts, &name, &[], depth + 1) {
                Found::Run(i) => return Found::Run(i),
                Found::Packages => packages = true,
                Found::Nothing => {}
            }
        }
    }
    if packages { Found::Packages } else { Found::Nothing }
}

fn manager(scripts: &Value, program: &str, args: &[Word], depth: u8, env: BTreeMap<String, String>) -> Found {
    let flag = |names: &[&str]| args.iter().any(|w| names.iter().any(|n| w.text == *n || w.text.starts_with(&format!("{n}="))));
    if flag(&["--workspaces", "-ws", "--workspace", "--recursive", "-r", "--filter", "-F"]) || (program == "yarn" && args.first().is_some_and(|w| w.text.starts_with("workspace"))) {
        return Found::Packages;
    }
    if program == "npm" && args.iter().any(|w| w.text == "-w") { return Found::Packages; }
    let mut k = skip_options(args, 0, &["--prefix", "-C", "--dir", "--cwd"]);
    let Some(first) = args.get(k) else { return Found::Nothing };
    let named = match (program, first.text.as_str()) {
        ("npm", "test" | "t" | "tst") => Some("test".to_string()),
        ("npm", "run" | "run-script") | ("pnpm" | "yarn" | "bun", "run") => { k += 1; args.get(k).map(|w| w.text.clone()) }
        ("npm", _) | ("bun", "test") => None,
        (_, name) => Some(name.to_string()),
    };
    let Some(name) = named else { return Found::Nothing };
    let after = &args[(k + 1).min(args.len())..];
    let extra: Vec<Word> = match program {
        "npm" => after.iter().skip_while(|w| w.text != "--").skip(1).cloned().collect(),
        _ => after.iter().skip(usize::from(after.first().is_some_and(|w| w.text == "--"))).cloned().collect(),
    };
    if scripts[&name].is_string() {
        return match script(scripts, &name, &extra, depth + 1) {
            Found::Run(mut i) => { for (key, value) in env { i.env.entry(key).or_insert(value); } Found::Run(i) }
            other => other,
        };
    }
    if program == "npm" { return Found::Nothing; }
    let mut words = vec![Word { text: name, expands: false }];
    words.extend(extra);
    match command(scripts, &words, depth + 1) {
        Found::Run(mut i) => { for (key, value) in env { i.env.entry(key).or_insert(value); } Found::Run(i) }
        other => other,
    }
}

const NODE_VALUED: [&str; 10] = ["-r", "--require", "--import", "--loader", "--experimental-loader", "-C", "--conditions", "--env-file", "--title", "--input-type"];

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

fn dropped(runner: Runner, flag: &str) -> (bool, bool) {
    let name = flag.split_once('=').map_or(flag, |(n, _)| n);
    let valued = !flag.contains('=');
    let has = |list: &[&str]| list.contains(&name);
    match runner {
        Runner::Vitest => {
            if has(&["--reporter", "--outputFile"]) { return (true, valued); }
            (has(&["--watch", "-w", "--ui", "--open", "--run", "-u", "--update", "--bail", "--coverage"]) || name.starts_with("--coverage."), false)
        }
        Runner::Jest => {
            if has(&["--reporters", "--outputFile", "--testResultsProcessor", "--changedSince", "--coverageReporters", "--coverageDirectory", "--json-output"]) { return (true, valued); }
            (has(&["--watch", "--watchAll", "--coverage", "--collectCoverage", "--json", "--notify", "-u", "--updateSnapshot", "--bail", "-b", "-o", "--onlyChanged", "--lastCommit", "--listTests"]), false)
        }
        Runner::Mocha => {
            if has(&["--reporter", "-R", "--reporter-option", "--reporter-options", "-O"]) { return (true, valued); }
            (has(&["--watch", "-w", "--bail", "-b"]), false)
        }
        Runner::Node => {
            if has(&["--test-reporter", "--test-reporter-destination"]) { return (true, valued); }
            (has(&["--experimental-test-coverage", "--watch", "--test-only"]), false)
        }
    }
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
        if w.expands {
            if kept.last().is_some_and(|p| p.text.starts_with('-') && !p.text.contains('=')) { kept.pop(); }
            k += 1;
            continue;
        }
        if w.text.starts_with('-') && w.text != "--" {
            let (drop, value) = dropped(runner, &w.text);
            if drop { k += if value { 2 } else { 1 }; continue; }
        }
        kept.push(w.clone());
        k += 1;
    }
    let program = program.map(str::to_string).unwrap_or_else(|| format!("/workspace/node_modules/.bin/{}", match runner { Runner::Vitest => "vitest", Runner::Jest => "jest", Runner::Mocha => "mocha", Runner::Node => "node" }));
    let mut command = vec![program];
    if runner == Runner::Vitest { command.push("run".into()); }
    command.extend(kept.into_iter().map(|w| w.text));
    if command.len() > 56 || command.iter().any(|a| a.len() > 8192 || a.contains('\0')) || env.values().any(|v| v.contains('\0')) { return Found::Nothing; }
    Found::Run(Invocation { runner, command, env })
}
