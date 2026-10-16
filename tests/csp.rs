use kern::csp::{blocks, inline, policies};

#[test]
fn policies_are_read_from_header_strings_and_from_helmet_style_objects() {
    let header = "res.setHeader('Content-Security-Policy', \"default-src 'self'; script-src 'self' 'unsafe-inline'\");\n";
    assert_eq!(policies("src/controller.ts", header), ["default-src 'self'; script-src 'self' 'unsafe-inline'"]);
    let helmet = "app.use(helmet({ contentSecurityPolicy: { directives: { defaultSrc: [\"'self'\"], styleSrc: [\"'self'\", \"'unsafe-inline'\"] } } }));\n";
    assert_eq!(policies("src/server.js", helmet), ["default-src 'self'; style-src 'self' 'unsafe-inline'"]);
    assert!(policies("src/a.ts", "const x = 'nothing about policies here';\n").is_empty());
}

#[test]
fn inline_styles_and_scripts_in_html_strings_are_counted_but_framework_style_props_are_not() {
    let page = "export const page = (x) => `<html><head><style>body { color: red }</style></head><body><p style=\"margin: 0\">${x}</p><script>go()</script>\
        <script src=\"/a.js\"></script><button onclick=\"go()\">Go</button><style nonce=\"${n}\">p {}</style></body></html>`;\n";
    let found = inline("src/page.ts", page);
    assert_eq!((found["style"], found["style-attr"], found["script"], found["handler"]), (1, 1, 1, 1), "{found:?}");
    let jsx = "export const P = () => <p style={{ margin: 0 }}>hi</p>;\n";
    assert!(inline("src/P.tsx", jsx).values().all(|n| *n == 0), "React sets style through the DOM, which a policy allows");
}

#[test]
fn a_policy_blocks_inline_code_unless_it_allows_unsafe_inline_without_a_nonce_or_hash() {
    let novu = "default-src 'self'; script-src 'self' 'unsafe-inline'";
    assert!(blocks(novu, "style"), "novu#11372: no style-src, so default-src 'self' refuses the new page's inline <style>");
    assert!(!blocks(novu, "script"));
    assert!(!blocks("default-src 'self'; style-src 'self' 'unsafe-inline'", "style"));
    assert!(blocks("style-src 'self' 'unsafe-inline' 'nonce-abc'", "style"), "with a nonce, browsers ignore 'unsafe-inline'");
    assert!(!blocks("script-src 'self'", "style"), "nothing restricts styles");
    assert!(blocks("default-src 'none'", "handler"));
}
