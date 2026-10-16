use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use std::collections::BTreeMap;

const KINDS: [&str; 4] = ["style", "style-attr", "script", "handler"];

struct Texts { strings: Vec<String>, objects: Vec<String> }
fn camel_to_kebab(k: &str) -> String {
    let mut out = String::new();
    for c in k.chars() { if c.is_ascii_uppercase() { out.push('-'); out.push(c.to_ascii_lowercase()); } else { out.push(c); } }
    out
}
fn sources(v: &Expression) -> Option<Vec<String>> {
    match v {
        Expression::StringLiteral(s) => Some(vec![s.value.to_string()]),
        Expression::ArrayExpression(a) => Some(a.elements.iter().filter_map(|e| match e { ArrayExpressionElement::StringLiteral(s) => Some(s.value.to_string()), _ => None }).collect()),
        _ => None,
    }
}
impl<'a> Visit<'a> for Texts {
    fn visit_string_literal(&mut self, it: &StringLiteral<'a>) { self.strings.push(it.value.to_string()); }
    fn visit_template_literal(&mut self, it: &TemplateLiteral<'a>) {
        self.strings.push(it.quasis.iter().map(|q| q.value.cooked.as_ref().map_or_else(|| q.value.raw.to_string(), |c| c.to_string())).collect::<Vec<_>>().join("X"));
        walk::walk_template_literal(self, it);
    }
    fn visit_object_expression(&mut self, it: &ObjectExpression<'a>) {
        let mut directives = Vec::new();
        for p in &it.properties {
            let ObjectPropertyKind::ObjectProperty(p) = p else { continue };
            let Some(key) = p.key.static_name() else { continue };
            if (key.ends_with("Src") || key.ends_with("SrcElem") || key.ends_with("SrcAttr")) && let Some(list) = sources(&p.value) {
                directives.push(format!("{} {}", camel_to_kebab(&key), list.join(" ")));
            }
        }
        if !directives.is_empty() { self.objects.push(directives.join("; ")); }
        walk::walk_object_expression(self, it);
    }
}
fn texts(path: &str, source: &str) -> Option<Texts> {
    crate::capability::source_types(path)?.into_iter().find_map(|st| {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, st).parse();
        if parsed.fatal_error || !parsed.diagnostics.is_empty() { return None; }
        let mut t = Texts { strings: Vec::new(), objects: Vec::new() };
        t.visit_program(&parsed.program);
        Some(t)
    })
}

pub fn policies(path: &str, source: &str) -> Vec<String> {
    let Some(t) = texts(path, source) else { return Vec::new() };
    let looks = |s: &str| ["default-src ", "script-src ", "style-src "].iter().any(|d| s.contains(d));
    let mut out: Vec<String> = t.strings.into_iter().filter(|s| looks(s)).collect();
    out.extend(t.objects);
    out
}

fn tags(text: &str, name: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let open = format!("<{name}");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find(&open) {
        let start = at + i;
        let after = lower[start + open.len()..].chars().next();
        if after.is_some_and(|c| c == '>' || c.is_whitespace()) {
            let end = lower[start..].find('>').map_or(lower.len(), |e| start + e);
            out.push(lower[start..end].to_string());
        }
        at = start + open.len();
    }
    out
}

pub fn inline(path: &str, source: &str) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = KINDS.iter().map(|k| (*k, 0)).collect();
    let Some(t) = texts(path, source) else { return counts };
    for s in t.strings.iter().filter(|s| s.contains('<')) {
        let lower = s.to_ascii_lowercase();
        *counts.get_mut("style").unwrap_or(&mut 0) += tags(s, "style").iter().filter(|tag| !tag.contains("nonce=")).count();
        *counts.get_mut("script").unwrap_or(&mut 0) += tags(s, "script").iter().filter(|tag| !tag.contains("src=") && !tag.contains("nonce=")).count();
        *counts.get_mut("style-attr").unwrap_or(&mut 0) += lower.matches(" style=\"").count() + lower.matches(" style='").count();
        let handlers = lower.match_indices(" on").filter(|(i, _)| {
            let rest = &lower[i + 3..];
            let name: String = rest.chars().take_while(char::is_ascii_lowercase).collect();
            !name.is_empty() && rest[name.len()..].starts_with("=\"")
        }).count();
        *counts.get_mut("handler").unwrap_or(&mut 0) += handlers;
    }
    counts
}

pub fn blocks(policy: &str, kind: &str) -> bool {
    let directives: BTreeMap<String, Vec<String>> = policy.split(';').filter_map(|d| {
        let mut words = d.split_whitespace();
        Some((words.next()?.to_ascii_lowercase(), words.map(str::to_string).collect()))
    }).collect();
    let order: &[&str] = match kind {
        "style" => &["style-src-elem", "style-src", "default-src"],
        "style-attr" => &["style-src-attr", "style-src", "default-src"],
        "script" => &["script-src-elem", "script-src", "default-src"],
        _ => &["script-src-attr", "script-src", "default-src"],
    };
    let Some(list) = order.iter().find_map(|d| directives.get(*d)) else { return false };
    let has = |w: &str| list.iter().any(|s| s.trim_matches('"').eq_ignore_ascii_case(w));
    let keyed = list.iter().any(|s| { let s = s.trim_matches(['\'', '"']).to_ascii_lowercase(); s.starts_with("nonce-") || s.starts_with("sha256-") || s.starts_with("sha384-") || s.starts_with("sha512-") });
    !(has("'unsafe-inline'") && !keyed)
}
