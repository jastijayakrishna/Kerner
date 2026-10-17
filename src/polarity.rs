#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind { Changed, Unknown, Harness, Flipped, Opened }
impl Kind {
    pub fn name(self) -> &'static str {
        match self { Kind::Opened => "opened", Kind::Flipped => "flipped", Kind::Changed => "changed", Kind::Harness => "harness", Kind::Unknown => "unknown" }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading { pub kind: Kind, pub was: String, pub now: String }

const MESSAGE_CHARS_MAX: usize = 8000;
const DIFF_LINES_MAX: usize = 200;
const SHOWN_CHARS_MAX: usize = 160;
const HARNESS: [&str; 16] = ["is not a function", "is not a constructor", "Cannot read properties of undefined", "Cannot read properties of null", "Cannot destructure property",
    "is not defined", "is not iterable", "Cannot set properties of", "export is defined on the", "there is no such export", "does not provide an export named",
    "has no exported member", "Cannot find module", "Failed to resolve import", "ENOENT", "Seems like you have not used"];
const REFUSED_NOW_ACCEPTED: [&str; 6] = ["Received function did not throw", "Received promise resolved instead of rejected", "Missing expected exception", "Missing expected rejection",
    "expected promise to be rejected", "to not be called at all, but actually been called"];
const TRUNCATED: [&str; 7] = ["…", "...(", "[Array]", "[Object]", "Array(", "Object (", "[Circular"];
const ASYMMETRIC: [&str; 8] = ["Any<", "Anything", "StringContaining", "StringMatching", "ObjectContaining", "ArrayContaining", "CloseTo", "expect.any"];
const UNREADABLE: [&str; 5] = ["Test timed out in", "Exceeded timeout of", "to be an instance of", "mismatched", "Expected pattern:"];
const VITEST_VERBS: [&str; 7] = [" to deeply equal ", " to strictly equal ", " to match object ", " to equal ", " to be ", " to contain ", " to include "];
const ABSENT_WORDS: [&str; 5] = ["null", "undefined", "NaN", "true", "false"];

fn reading(kind: Kind, was: &str, now: &str) -> Reading { Reading { kind, was: shown(was), now: shown(now) } }
fn unknown() -> Reading { reading(Kind::Unknown, "", "") }
fn shown(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SHOWN_CHARS_MAX { flat } else { format!("{}…", flat.chars().take(SHOWN_CHARS_MAX - 1).collect::<String>()) }
}
fn clean(message: &str) -> String {
    let mut out = String::new();
    let mut chars = message.chars().take(MESSAGE_CHARS_MAX).peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for d in chars.by_ref() { if d.is_ascii_alphabetic() { break; } }
            continue;
        }
        out.push(c);
    }
    out.lines().filter(|l| !l.trim_start().starts_with("at ")).collect::<Vec<_>>().join("\n")
}

pub fn read(message: &str, expected: Option<&str>, actual: Option<&str>) -> Reading {
    let text = clean(message);
    let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if HARNESS.iter().any(|h| first.contains(h)) { return reading(Kind::Harness, "", ""); }
    if let Some(r) = opened(&text) { return r; }
    if let Some(r) = other_error(&text) { return r; }
    if UNREADABLE.iter().any(|u| text.contains(u)) && expected.is_none() { return unknown(); }
    if let (Some(e), Some(a)) = (expected, actual) && !calls(&text) { return values(e, a); }
    diff_block(&text).or_else(|| jest_pairs(&text)).or_else(|| vitest_sentence(&text)).or_else(|| node_short(&text)).unwrap_or_else(unknown)
}

fn calls(text: &str) -> bool { ["[Function] to throw", "number of calls", "Number of calls", "to be called", "to not be called"].iter().any(|c| text.contains(c)) }
fn opened(text: &str) -> Option<Reading> {
    if text.contains("[Function] to throw") && !text.contains(" but got ") { return Some(reading(Kind::Opened, "throws", "does not throw")); }
    let from_zero = text.contains("Expected number of calls: 0") && after(text, "Received number of calls:").is_some_and(|n| n != "0");
    let refused = REFUSED_NOW_ACCEPTED.iter().any(|o| text.contains(o)) || (text.contains("promise resolved") && text.contains("instead of rejecting"));
    (refused || from_zero).then(|| reading(Kind::Opened, "refused", "accepted"))
}
fn other_error(text: &str) -> Option<Reading> {
    if text.contains("[Function] to throw error including ") && let Some((_, rest)) = text.split_once(" including ") && let Some((was, now)) = rest.split_once(" but got ") {
        return Some(reading(Kind::Changed, was.trim(), now.lines().next().unwrap_or("").trim()));
    }
    if let (Some(was), Some(now)) = (after(text, "Expected substring:"), after(text, "Received message:")) { return Some(reading(Kind::Changed, was, now)); }
    if text.contains("instead of resolving") || text.contains("Received promise rejected instead of resolved") { return Some(reading(Kind::Flipped, "accepted", "refused")); }
    None
}

#[derive(Clone, Debug, PartialEq)]
enum Token { Text(String), Number(f64), Word(String), Mark(char) }
fn quoted(chars: &[char], i: usize) -> (Token, usize) {
    let mut j = i + 1;
    while j < chars.len() && chars[j] != chars[i] { if chars[j] == '\\' { j += 1; } j += 1; }
    (Token::Text(chars[i + 1..j.min(chars.len())].iter().collect()), j + 1)
}
fn number(chars: &[char], i: usize) -> (Token, usize) {
    let (mut j, mut dot) = (i + 1, false);
    while j < chars.len() && (chars[j].is_ascii_digit() || (!dot && chars[j] == '.' && chars.get(j + 1).is_some_and(char::is_ascii_digit))) { dot |= chars[j] == '.'; j += 1; }
    let raw: String = chars[i..j].iter().collect();
    (raw.parse().map_or_else(|_| Token::Word(raw.clone()), Token::Number), j)
}
fn tokens(line: &str) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let (mut out, mut i) = (Vec::new(), 0usize);
    while i < chars.len() {
        let c = chars[i];
        let after_value = out.last().is_some_and(|t| matches!(t, Token::Number(_) | Token::Word(_) | Token::Text(_)));
        let (token, next) = if c.is_whitespace() { i += 1; continue }
            else if c == '"' || c == '\'' || c == '`' { quoted(&chars, i) }
            else if c.is_ascii_digit() || ((c == '+' || c == '-') && !after_value && chars.get(i + 1).is_some_and(char::is_ascii_digit)) { number(&chars, i) }
            else if c.is_alphabetic() || c == '_' || c == '$' {
                let j = (i + 1..chars.len()).find(|&j| !(chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$')).unwrap_or(chars.len());
                (Token::Word(chars[i..j].iter().collect()), j)
            } else { (Token::Mark(c), i + 1) };
        out.push(token);
        i = next;
    }
    out
}

fn absent_text(s: &str) -> bool { s.is_empty() || s == "undefined" || s == "null" }
fn broken_text(s: &str) -> bool { s.contains("NaN") || s.contains("[object Object]") }
fn status(n: f64) -> Option<bool> { (n.fract() == 0.0 && (100.0..=599.0).contains(&n)).then_some(n >= 400.0) }
fn literal(was: &Token, now: &Token) -> Kind {
    match (was, now) {
        (Token::Text(a), Token::Text(b)) if absent_text(a) || absent_text(b) || broken_text(a) || broken_text(b) => Kind::Flipped,
        (Token::Text(_), Token::Text(_)) => Kind::Changed,
        (Token::Number(a), Token::Number(b)) if *a == 0.0 || *b == 0.0 => Kind::Flipped,
        (Token::Number(a), Token::Number(b)) => match (status(*a), status(*b)) { (Some(true), Some(false)) => Kind::Opened, (Some(false), Some(true)) => Kind::Flipped, _ => Kind::Changed },
        _ => Kind::Flipped,
    }
}
fn compare(was: &[Token], now: &[Token]) -> Kind {
    if was.len() != now.len() { return Kind::Flipped; }
    let worst = was.iter().zip(now).filter(|(a, b)| a != b).map(|(a, b)| literal(a, b)).max();
    worst.unwrap_or(Kind::Unknown)
}
fn compare_text(was: &str, now: &str) -> Kind {
    let absent = |s: &str| { let t = tokens(s); t.len() == 1 && matches!(&t[0], Token::Word(w) if ABSENT_WORDS.contains(&w.as_str())) };
    if [was, now].iter().any(|s| ASYMMETRIC.iter().any(|m| s.contains(m))) { return if absent(was) || absent(now) { Kind::Flipped } else { Kind::Unknown }; }
    if [was, now].iter().any(|s| TRUNCATED.iter().any(|m| s.contains(m))) { return Kind::Unknown; }
    compare(&tokens(was), &tokens(now))
}

fn scalar(s: &str) -> Token {
    let t = s.trim();
    t.parse::<f64>().map_or_else(|_| if ABSENT_WORDS.contains(&t) { Token::Word(t.to_string()) } else { Token::Text(t.to_string()) }, Token::Number)
}
fn values(expected: &str, actual: &str) -> Reading {
    if !expected.contains('\n') && !actual.contains('\n') {
        let (was, now) = (scalar(expected), scalar(actual));
        let kind = if was == now { Kind::Unknown } else { compare(&[was], &[now]) };
        return reading(kind, expected, actual);
    }
    let (old, new): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    if old.len() > DIFF_LINES_MAX || new.len() > DIFF_LINES_MAX { return unknown(); }
    pairs(&line_diff(&old, &new))
}
type Hunk<'a> = (Vec<&'a str>, Vec<&'a str>);
fn line_diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Hunk<'a>> {
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() { for j in (0..m).rev() { lcs[i][j] = if old[i].trim() == new[j].trim() { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) }; } }
    let (mut i, mut j, mut hunks) = (0, 0, vec![(Vec::new(), Vec::new())]);
    while i < n || j < m {
        if i < n && j < m && old[i].trim() == new[j].trim() { hunks.push((Vec::new(), Vec::new())); i += 1; j += 1; }
        else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) { if let Some(h) = hunks.last_mut() { h.1.push(new[j]); } j += 1; }
        else { if let Some(h) = hunks.last_mut() { h.0.push(old[i]); } i += 1; }
    }
    hunks
}
fn pairs(hunks: &[Hunk]) -> Reading {
    let (mut worst, mut was, mut now) = (None, Vec::new(), Vec::new());
    for (removed, added) in hunks.iter().filter(|(r, a)| !r.is_empty() || !a.is_empty()) {
        was.extend(removed.iter().map(|l| l.trim()));
        now.extend(added.iter().map(|l| l.trim()));
        let k = if removed.len() != added.len() { Kind::Flipped } else { removed.iter().zip(added).map(|(x, y)| compare_text(x, y)).max().unwrap_or(Kind::Unknown) };
        worst = worst.max(Some(k));
    }
    worst.map_or_else(unknown, |k| reading(k, &was.join("; "), &now.join("; ")))
}

fn diff_header(line: &str) -> bool {
    let t = line.trim();
    ["- Expected", "+ Received", "- Snapshot", "+ actual - expected"].iter().any(|h| t.starts_with(h)) || (t.starts_with("@@") && t.ends_with("@@"))
}
fn diff_block(text: &str) -> Option<Reading> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| diff_header(l)).or_else(|| lines.iter().position(|l| l.trim_end().ends_with("call:")))?;
    let mut hunks: Vec<Hunk> = vec![(Vec::new(), Vec::new())];
    for line in lines[start..].iter().filter(|l| !diff_header(l)).take(DIFF_LINES_MAX * 2) {
        match line.chars().next() {
            Some('-') => if let Some(h) = hunks.last_mut() { h.0.push(&line[1..]) },
            Some('+') => if let Some(h) = hunks.last_mut() { h.1.push(&line[1..]) },
            _ => if hunks.last().is_some_and(|(r, a)| !r.is_empty() || !a.is_empty()) { hunks.push((Vec::new(), Vec::new())) },
        }
    }
    hunks.iter().any(|(r, a)| !r.is_empty() || !a.is_empty()).then(|| pairs(&hunks))
}

fn after<'a>(text: &'a str, label: &str) -> Option<&'a str> { text.lines().find_map(|l| l.trim_start().strip_prefix(label)).map(str::trim) }
fn below<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().position(|l| l.trim_end().ends_with(label)).and_then(|at| lines.get(at + 1)).map(|l| l.trim())
}
fn jest_pairs(text: &str) -> Option<Reading> {
    for matcher in ["toBeNull()", "toBeUndefined()", "toBeDefined()", "toBeTruthy()", "toBeFalsy()"] {
        if text.contains(matcher) { return Some(reading(Kind::Flipped, matcher.trim_start_matches("toBe").trim_end_matches("()"), after(text, "Received:").unwrap_or(""))); }
    }
    if let (Some(e), Some(r)) = (after(text, "Expected number of calls:"), after(text, "Received number of calls:")) { return Some(reading(compare(&[scalar(e)], &[scalar(r)]), e, r)); }
    if text.contains("Number of calls: 0") && text.contains("Expected:") { return Some(reading(Kind::Flipped, "called", "not called")); }
    if let (Some(e), Some(r)) = (after(text, "Expected substring:"), after(text, "Received string:")) { return Some(reading(compare_text(e, r), e, r)); }
    if let (Some(e), Some(r)) = (below(text, "Expected the element to have class:"), below(text, "Received:")) {
        return Some(reading(compare(&[Token::Text(e.to_string())], &[Token::Text(r.to_string())]), e, r));
    }
    let (e, r) = (after(text, "Expected:")?, after(text, "Received:")?);
    if e.is_empty() || r.is_empty() { return Some(unknown()); }
    Some(reading(compare_text(e, r), e, r))
}
fn vitest_sentence(text: &str) -> Option<Reading> {
    let line = text.lines().find(|l| l.contains("expected "))?;
    let rest = line[line.find("expected ")? + "expected ".len()..].split(" // Object.is equality").next().unwrap_or("").trim_end();
    if let Some((_, y)) = rest.split_once(" to have a length of ") {
        let (want, got) = y.split_once(" but got ").map_or((y.trim(), ""), |(w, g)| (w.trim(), g.trim()));
        let zero = |s: &str| s.trim_start_matches('+') == "0";
        return Some(reading(if zero(want) || zero(got) || got.is_empty() { Kind::Flipped } else { Kind::Changed }, &format!("length {want}"), &format!("length {got}")));
    }
    if let Some(t) = ["truthy", "falsy", "null", "undefined", "defined", "NaN"].iter().find(|t| rest.ends_with(&format!(" to be {t}"))) {
        return Some(reading(Kind::Flipped, t, rest.split(" to be ").next().unwrap_or("")));
    }
    if let Some((_, y)) = rest.split_once(" to be called ") {
        let counts = y.split_once(" times, but got ").map(|(w, g)| (w.trim(), g.trim_end_matches(" times").trim()));
        return Some(counts.map_or_else(unknown, |(w, g)| reading(compare(&[scalar(w)], &[scalar(g)]), w, g)));
    }
    let verb = VITEST_VERBS.iter().find(|v| rest.contains(*v))?;
    let (now, was) = rest.split_once(verb)?;
    Some(reading(compare_text(was.trim(), now.trim()), was.trim(), now.trim()))
}
fn node_short(text: &str) -> Option<Reading> {
    text.lines().map(str::trim).find_map(|l| {
        [" !== ", " === ", " != ", " == "].iter().find_map(|op| l.split_once(op)).filter(|(a, e)| tokens(a).len() == 1 && tokens(e).len() == 1)
            .map(|(a, e)| reading(compare(&tokens(e), &tokens(a)), e, a))
    })
}
