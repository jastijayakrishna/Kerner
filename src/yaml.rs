use crate::util::*;
use serde_json::{Map, Value};

pub const YAML_BYTES_MAX: usize = 524288;
pub const YAML_DEPTH_MAX: usize = 64;
pub const YAML_NODES_COUNT_MAX: usize = 200_000;
const BOOL_TRUE: [&str; 9] = ["true", "True", "TRUE", "yes", "Yes", "YES", "on", "On", "ON"];
const BOOL_FALSE: [&str; 9] = ["false", "False", "FALSE", "no", "No", "NO", "off", "Off", "OFF"];
const NULLS: [&str; 5] = ["", "~", "null", "Null", "NULL"];

struct Reader { lines: Vec<String>, at: usize, nodes: usize }

fn unreadable(line: usize, why: &str) -> Denied { Denied(format!("YAML line {}: {why}", line + 1)) }
fn indent_of(line: &str) -> usize { line.len() - line.trim_start_matches(' ').len() }
fn blank(line: &str) -> bool { let t = line.trim_start_matches(' '); t.is_empty() || t.starts_with('#') }

fn plain_value(text: &str) -> Value {
    if NULLS.contains(&text) { return Value::Null; }
    if BOOL_TRUE.contains(&text) { return Value::Bool(true); }
    if BOOL_FALSE.contains(&text) { return Value::Bool(false); }
    let t = text.replace('_', "");
    let t = t.strip_prefix('+').unwrap_or(&t);
    if let Some(h) = t.strip_prefix("0x") && let Ok(i) = i64::from_str_radix(h, 16) { return Value::from(i); }
    if let Some(o) = t.strip_prefix("0o") && let Ok(i) = i64::from_str_radix(o, 8) { return Value::from(i); }
    if let Some(b) = t.strip_prefix("0b") && let Ok(i) = i64::from_str_radix(b, 2) { return Value::from(i); }
    if let Ok(i) = t.parse::<i64>() && (t == "0" || !t.trim_start_matches('-').starts_with('0')) { return Value::from(i); }
    if t.contains('.') && t.bytes().all(|b| b.is_ascii_digit() || b".eE+-".contains(&b)) && t.bytes().any(|b| b.is_ascii_digit()) && !t.starts_with('.') && !t.contains("..")
        && let Ok(f) = t.parse::<f64>() && f.is_finite() { return serde_json::Number::from_f64(f).map_or(Value::String(text.into()), Value::Number); }
    Value::String(text.into())
}

fn comment_start(s: &str) -> usize {
    let b = s.as_bytes();
    (0..b.len()).find(|&i| b[i] == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t')).unwrap_or(b.len())
}
fn strip_comment(s: &str) -> &str { s[..comment_start(s)].trim_end() }

fn key_split(s: &str) -> Option<(String, &str, bool)> {
    let s = s.trim_start();
    if s.starts_with(['"', '\'']) {
        let (key, rest) = quoted_prefix(s)?;
        let rest = rest.trim_start();
        let rest = rest.strip_prefix(':')?;
        if !(rest.is_empty() || rest.starts_with(' ')) { return None; }
        return Some((key, rest.trim_start(), true));
    }
    let b = s.as_bytes();
    let limit = comment_start(s);
    let mut i = 0;
    while i < limit {
        if b[i] == b':' && (i + 1 == b.len() || b[i + 1] == b' ') {
            let key = s[..i].trim_end();
            if key.is_empty() || key.starts_with(['[', '{', '&', '*', '!', '|', '>', '%', '@', '`']) || key.contains(" #") { return None; }
            return Some((key.to_string(), s[i + 1..].trim_start(), false));
        }
        i += 1;
    }
    None
}

fn quoted_prefix(s: &str) -> Option<(String, &str)> {
    let quote = s.chars().next()?;
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            ('\'', '\'') => { if s[1 + i + 1..].starts_with('\'') { out.push('\''); chars.next(); } else { return Some((out, &s[1 + i + 1..])); } }
            ('"', '"') => return Some((out, &s[1 + i + 1..])),
            ('"', '\\') => {
                let (_, e) = chars.next()?;
                match e {
                    'n' => out.push('\n'), 't' => out.push('\t'), 'r' => out.push('\r'), '0' => out.push('\0'), ' ' => out.push(' '), '/' => out.push('/'),
                    '"' | '\\' | '\'' => out.push(e), 'a' => out.push('\u{7}'), 'b' => out.push('\u{8}'), 'e' => out.push('\u{1b}'), 'f' => out.push('\u{c}'), 'v' => out.push('\u{b}'),
                    'x' | 'u' | 'U' => {
                        let n = match e { 'x' => 2, 'u' => 4, _ => 8 };
                        let hex: String = (0..n).filter_map(|_| chars.next().map(|(_, c)| c)).collect();
                        out.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)?);
                    }
                    _ => return None,
                }
            }
            _ => out.push(c),
        }
    }
    None
}

impl Reader {
    fn peek(&mut self) -> Option<usize> {
        while self.at < self.lines.len() && blank(&self.lines[self.at]) { self.at += 1; }
        (self.at < self.lines.len()).then_some(self.at)
    }
    fn count(&mut self) -> R<()> {
        self.nodes += 1;
        crate::ensure!(self.nodes <= YAML_NODES_COUNT_MAX, "YAML holds more than {YAML_NODES_COUNT_MAX} nodes");
        Ok(())
    }
    fn block(&mut self, indent: usize, depth: usize) -> R<Value> {
        crate::ensure!(depth <= YAML_DEPTH_MAX, "YAML nests deeper than {YAML_DEPTH_MAX}");
        let Some(i) = self.peek() else { return Ok(Value::Null) };
        let line = self.lines[i].clone();
        let col = indent_of(&line);
        if col < indent { return Ok(Value::Null); }
        let body = &line[col..];
        crate::ensure!(!body.starts_with("? "), "YAML line {}: complex keys are not read", i + 1);
        if body == "-" || body.starts_with("- ") { return self.sequence(col, depth); }
        if key_split(body).is_some() { return self.mapping(col, depth); }
        self.scalar(col, depth)
    }
    fn sequence(&mut self, col: usize, depth: usize) -> R<Value> {
        let mut items = Vec::new();
        while let Some(i) = self.peek() {
            let line = self.lines[i].clone();
            if indent_of(&line) != col || !(line[col..] == *"-" || line[col..].starts_with("- ")) { break; }
            self.count()?;
            let rest = line[col + 1..].to_string();
            if strip_comment(&rest).trim().is_empty() { self.at += 1; items.push(self.block(col + 1, depth + 1)?); continue; }
            let pad = rest.len() - rest.trim_start().len();
            self.lines[i] = format!("{}{}", " ".repeat(col + 1 + pad), rest.trim_start());
            items.push(self.block(col + 1, depth + 1)?);
        }
        Ok(Value::Array(items))
    }
    fn mapping(&mut self, col: usize, depth: usize) -> R<Value> {
        let mut map = Map::new();
        while let Some(i) = self.peek() {
            let line = self.lines[i].clone();
            if indent_of(&line) != col { break; }
            let Some((key, rest, _)) = key_split(&line[col..]) else { return Err(unreadable(i, "expected a key: value entry")) };
            self.count()?;
            let value = if strip_comment(rest).is_empty() {
                self.at += 1;
                self.child_block(col, depth)?
            } else {
                self.inline(rest, col, depth + 1)?
            };
            crate::ensure!(map.insert(key.clone(), value).is_none(), "YAML line {}: duplicate key {key}", i + 1);
        }
        Ok(Value::Object(map))
    }
    fn child_block(&mut self, col: usize, depth: usize) -> R<Value> {
        let Some(i) = self.peek() else { return Ok(Value::Null) };
        let line = &self.lines[i];
        let c = indent_of(line);
        if c == col && (line[c..] == *"-" || line[c..].starts_with("- ")) { return self.sequence(c, depth + 1); }
        if c <= col { return Ok(Value::Null); }
        self.block(c, depth + 1)
    }
    fn inline(&mut self, rest: &str, col: usize, depth: usize) -> R<Value> {
        let text = strip_comment(rest);
        let first = text.chars().next().unwrap_or(' ');
        crate::ensure!(!matches!(first, '&' | '*' | '!' | '%' | '@' | '`'), "YAML line {}: anchors, aliases, tags and directives are not read", self.at + 1);
        self.count()?;
        if first == '|' || first == '>' { return self.block_scalar(text, col); }
        if first == '[' || first == '{' { return self.flow(rest, depth); }
        if first == '"' || first == '\'' {
            let line = self.at;
            let joined = self.joined_quoted(rest, col)?;
            let (value, after) = quoted_prefix(&joined).ok_or_else(|| unreadable(line, "unclosed quoted string"))?;
            crate::ensure!(strip_comment(after).trim().is_empty(), "YAML line {}: text after a quoted string", line + 1);
            return Ok(Value::String(value));
        }
        self.at += 1;
        let mut parts = vec![text.to_string()];
        self.continuation(col, &mut parts);
        let folded = fold_plain(&parts);
        Ok(if parts.len() == 1 { plain_value(&folded) } else { Value::String(folded) })
    }
    fn joined_quoted(&mut self, rest: &str, col: usize) -> R<String> {
        let quote = rest.trim_start().chars().next().unwrap_or('"');
        let mut joined = rest.trim_start().to_string();
        let start = self.at;
        self.at += 1;
        while closes(&joined, quote).is_none() {
            crate::ensure!(self.at < self.lines.len() && self.at - start < 400, "YAML line {}: unclosed quoted string", start + 1);
            let line = self.lines[self.at].clone();
            crate::ensure!(line.trim().is_empty() || indent_of(&line) > col, "YAML line {}: unclosed quoted string", start + 1);
            let t = line.trim();
            if t.is_empty() { joined.push('\n'); } else if !joined.ends_with('\n') { joined.push(' '); }
            joined.push_str(t);
            self.at += 1;
        }
        Ok(joined)
    }
    fn continuation(&mut self, col: usize, parts: &mut Vec<String>) {
        while self.at < self.lines.len() {
            let line = &self.lines[self.at];
            let t = line.trim();
            if t.is_empty() { parts.push(String::new()); self.at += 1; continue; }
            if indent_of(line) <= col || t.starts_with('#') || t.starts_with("- ") || key_split(t).is_some() { break; }
            parts.push(strip_comment(t).to_string());
            self.at += 1;
        }
        while parts.last().is_some_and(String::is_empty) { parts.pop(); }
    }
    fn block_scalar(&mut self, header: &str, col: usize) -> R<Value> {
        let h = header.trim();
        let folded = h.starts_with('>');
        let chomp = if h.contains('-') { '-' } else if h.contains('+') { '+' } else { ' ' };
        let fixed: Option<usize> = h.bytes().find(u8::is_ascii_digit).map(|d| (d - b'0') as usize);
        crate::ensure!(h.len() <= 3 && h[1..].bytes().all(|b| b == b'-' || b == b'+' || b.is_ascii_digit()), "YAML line {}: unknown block scalar header {h}", self.at + 1);
        self.at += 1;
        let mut raw: Vec<String> = Vec::new();
        let mut content = fixed.map(|d| col + d);
        while self.at < self.lines.len() {
            let line = &self.lines[self.at];
            if line.trim().is_empty() { raw.push(match content { Some(c) if line.len() > c && line[..c].bytes().all(|b| b == b' ') => line[c..].to_string(), _ => String::new() }); self.at += 1; continue; }
            let ind = indent_of(line);
            if content.is_none() { if ind > col { content = Some(ind); } else { break; } }
            if ind < content.unwrap_or(0) { break; }
            raw.push(line[content.unwrap_or(0)..].to_string());
            self.at += 1;
        }
        let trailing = raw.iter().rev().take_while(|l| l.trim().is_empty()).count();
        raw.truncate(raw.len() - trailing);
        let mut text = if folded { fold_block(&raw) } else { raw.join("\n") };
        if !raw.is_empty() { match chomp { '-' => {}, '+' => { text.push('\n'); text.push_str(&"\n".repeat(trailing)); }, _ => text.push('\n') } }
        Ok(Value::String(text))
    }
    fn flow(&mut self, rest: &str, depth: usize) -> R<Value> {
        let start = self.at;
        let mut text = rest.trim_start().to_string();
        self.at += 1;
        let mut flow = Flow { s: text.as_bytes().to_vec(), i: 0, nodes: 0, depth };
        while flow.parse().is_err() || !flow.only_comment_left() {
            crate::ensure!(self.at < self.lines.len() && self.at - start < 2000, "YAML line {}: unclosed flow collection", start + 1);
            text.push('\n');
            text.push_str(&self.lines[self.at]);
            self.at += 1;
            flow = Flow { s: text.as_bytes().to_vec(), i: 0, nodes: 0, depth };
        }
        self.nodes += flow.nodes;
        crate::ensure!(self.nodes <= YAML_NODES_COUNT_MAX, "YAML holds more than {YAML_NODES_COUNT_MAX} nodes");
        flow.i = 0;
        flow.parse().map_err(|e| unreadable(start, &e.0))
    }
    fn scalar(&mut self, col: usize, depth: usize) -> R<Value> {
        let line = self.lines[self.at].clone();
        let rest = line[col..].to_string();
        self.inline(&rest, col.saturating_sub(1), depth)
    }
}

fn closes(joined: &str, quote: char) -> Option<usize> { quoted_prefix(joined).map(|(v, _)| v.len()).filter(|_| joined.starts_with(quote)) }

fn fold_plain(parts: &[String]) -> String {
    let mut out = String::new();
    for (k, p) in parts.iter().enumerate() {
        if k == 0 { out.push_str(p); continue; }
        if p.is_empty() { out.push('\n'); } else { if !out.ends_with('\n') { out.push(' '); } out.push_str(p); }
    }
    out
}

fn fold_block(raw: &[String]) -> String {
    let mut out = String::new();
    let mut prev_more = false;
    let mut pending_blanks = 0usize;
    for (k, line) in raw.iter().enumerate() {
        if line.is_empty() { pending_blanks += 1; continue; }
        let more = line.starts_with([' ', '\t']);
        if k > 0 {
            if pending_blanks > 0 { out.push_str(&"\n".repeat(pending_blanks + usize::from(more || prev_more))); }
            else if more || prev_more { out.push('\n'); }
            else { out.push(' '); }
        }
        pending_blanks = 0;
        out.push_str(line);
        prev_more = more;
    }
    out
}

struct Flow { s: Vec<u8>, i: usize, nodes: usize, depth: usize }
impl Flow {
    fn skip(&mut self) {
        while self.i < self.s.len() {
            match self.s[self.i] {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                b'#' if self.i == 0 || matches!(self.s[self.i - 1], b' ' | b'\n') => { while self.i < self.s.len() && self.s[self.i] != b'\n' { self.i += 1; } }
                _ => return,
            }
        }
    }
    fn only_comment_left(&mut self) -> bool { self.skip(); self.i >= self.s.len() }
    fn parse(&mut self) -> R<Value> {
        self.nodes += 1;
        crate::ensure!(self.nodes <= YAML_NODES_COUNT_MAX && self.depth <= YAML_DEPTH_MAX, "flow collection over the node or depth cap");
        self.skip();
        match self.s.get(self.i) {
            Some(b'[') => { self.i += 1; self.depth += 1; let v = self.seq()?; self.depth -= 1; Ok(v) }
            Some(b'{') => { self.i += 1; self.depth += 1; let v = self.map()?; self.depth -= 1; Ok(v) }
            Some(b'"' | b'\'') => {
                let text = std::str::from_utf8(&self.s[self.i..]).map_err(|_| Denied("not UTF-8".into()))?;
                let (v, after) = quoted_prefix(text).ok_or_else(|| Denied("unclosed quoted string".into()))?;
                self.i = self.s.len() - after.len();
                Ok(Value::String(v))
            }
            Some(_) => self.plain(),
            None => crate::deny!("unclosed flow collection"),
        }
    }
    fn plain(&mut self) -> R<Value> {
        let start = self.i;
        while self.i < self.s.len() {
            let b = self.s[self.i];
            if matches!(b, b',' | b']' | b'}' | b'\n') || (b == b':' && matches!(self.s.get(self.i + 1), Some(b' ' | b',' | b']' | b'}') | None)) || (b == b'#' && self.s[self.i - 1] == b' ') { break; }
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| Denied("not UTF-8".into()))?.trim();
        crate::ensure!(!text.starts_with(['&', '*', '!']), "anchors, aliases and tags are not read");
        Ok(plain_value(text))
    }
    fn seq(&mut self) -> R<Value> {
        let mut items = Vec::new();
        loop {
            self.skip();
            match self.s.get(self.i) {
                Some(b']') => { self.i += 1; return Ok(Value::Array(items)); }
                Some(b',') => { self.i += 1; }
                None => crate::deny!("unclosed flow sequence"),
                _ => items.push(self.parse()?),
            }
        }
    }
    fn map(&mut self) -> R<Value> {
        let mut map = Map::new();
        loop {
            self.skip();
            match self.s.get(self.i) {
                Some(b'}') => { self.i += 1; return Ok(Value::Object(map)); }
                Some(b',') => { self.i += 1; }
                None => crate::deny!("unclosed flow mapping"),
                _ => {
                    let key = match self.parse()? { Value::String(s) => s, other => text_of(&other) };
                    self.skip();
                    let value = if self.s.get(self.i) == Some(&b':') { self.i += 1; self.parse()? } else { Value::Null };
                    map.insert(key, value);
                }
            }
        }
    }
}

pub fn text_of(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

pub fn read(bytes: &[u8]) -> R<Value> {
    crate::ensure!(bytes.len() <= YAML_BYTES_MAX, "YAML is over {YAML_BYTES_MAX} bytes");
    let text = std::str::from_utf8(bytes).map_err(|_| Denied("YAML is not UTF-8".into()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines: Vec<String> = Vec::new();
    for (n, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if n == 0 && line.starts_with("%YAML") || line.starts_with("%TAG") { lines.push(String::new()); continue; }
        if line.starts_with("---") && (line.len() == 3 || line.as_bytes()[3] == b' ') { if lines.iter().any(|l| !blank(l)) { break; } lines.push(String::new()); continue; }
        if line == "..." { break; }
        lines.push(line.to_string());
    }
    let mut reader = Reader { lines, at: 0, nodes: 0 };
    let value = reader.block(0, 0)?;
    if let Some(i) = reader.peek() { return Err(unreadable(i, "text after the document")); }
    Ok(value)
}
