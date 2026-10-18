use kern::investigate::{self, Prompt};
use kern::util::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const ANSWERER: &str = "claude-code-subagent";

fn reply(text: String) -> Value {
    json!({ "model": ANSWERER, "stop_reason": "end_turn", "content": [{ "type": "text", "text": text }], "usage": { "input_tokens": 0, "output_tokens": 0 } })
}
fn asked(answers: &Path, prompt: &Prompt) -> R<Value> {
    match std::fs::read_to_string(answers.join(format!("{}.json", prompt.name))) {
        Ok(text) => Ok(reply(text)),
        Err(_) => {
            std::fs::write(answers.join(format!("{}.request.json", prompt.name)), serde_json::to_vec_pretty(&investigate::request(ANSWERER, prompt))?)?;
            Err(Denied(format!("no answer yet for the {} prompt", prompt.name)))
        }
    }
}
fn run(args: &[String]) -> R<Value> {
    let answers = PathBuf::from(&args[5]);
    std::fs::create_dir_all(&answers)?;
    let pr = investigate::read_pr(Path::new(&args[3]))?;
    let ask = |p: &Prompt| asked(&answers, p);
    investigate::investigate(Path::new(&args[0]), (&args[1], &args[2]), &pr, Path::new(&args[4]), Some(&ask))
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 {
        eprintln!("usage: cargo run --example answered -- <repo> <base> <head> <pr.md> <out dir> <answers dir>\nRuns Kern's investigation with the model's answers read from <answers dir>/oracle.json and hunter.json; a missing answer writes <name>.request.json there, the exact request the API would get");
        std::process::exit(2);
    }
    match run(&args) {
        Ok(summary) => println!("{summary}"),
        Err(e) => { eprintln!("{}", e.0); std::process::exit(1); }
    }
}
