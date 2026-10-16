use kern::nodeapi::{lowest, needs, uses};

#[test]
fn the_lowest_node_a_package_supports_is_read_from_its_engines_range() {
    assert_eq!(lowest(">=20.0.0"), Some((20, 0, 0)));
    assert_eq!(lowest("^18.18.0 || >=20"), Some((18, 18, 0)));
    assert_eq!(lowest("20.x"), Some((20, 0, 0)));
    assert_eq!(lowest(">=18 <23"), Some((18, 0, 0)));
    assert_eq!(lowest("18 || 20 || 22"), Some((18, 0, 0)));
    assert_eq!(lowest(">16"), Some((17, 0, 0)));
    assert_eq!(lowest("~20.11"), Some((20, 11, 0)));
    assert_eq!(lowest("v22.5.0"), Some((22, 5, 0)));
    assert_eq!(lowest("18 - 20"), Some((18, 0, 0)));
    assert_eq!(lowest("*"), None);
    assert_eq!(lowest("lts/*"), None);
}

#[test]
fn a_feature_is_usable_from_its_flag_free_version_and_on_every_line_it_was_backported_to() {
    assert_eq!(needs("module:node:sqlite", (20, 0, 0)).as_deref(), Some("22.13.0"), "workers-sdk#11805 used node:sqlite in a package that supports Node 20");
    assert_eq!(needs("module:node:sqlite", (22, 13, 0)), None);
    assert_eq!(needs("member:process.getBuiltinModule", (20, 16, 0)), None, "backported to 20.16");
    assert_eq!(needs("member:process.getBuiltinModule", (21, 5, 0)).as_deref(), Some("22.3.0"), "never in 21");
    assert_eq!(needs("member:process.getBuiltinModule", (20, 0, 0)).as_deref(), Some("20.16.0"));
    assert_eq!(needs("global:fetch", (18, 0, 0)), None);
    assert_eq!(needs("global:fetch", (16, 20, 0)).as_deref(), Some("18.0.0"), "only behind a flag before 18");
    assert_eq!(needs("member:fs.readFileSync", (14, 0, 0)), None, "features older than Node 14 are not in the table");
}

#[test]
fn every_way_code_reaches_a_node_feature_is_seen() {
    let src = "import { DatabaseSync } from 'node:sqlite';\nimport fs, { globSync } from 'node:fs';\nimport * as util from 'util';\nconst { parseArgs } = require('node:util');\n\
        const path = require('path');\nfs.promises.glob('x');\nutil.styleText('red', 'x');\nprocess.getBuiltinModule('fs');\nconst ws = new WebSocket('ws://x');\n\
        console.log(import.meta.dirname, AbortSignal.any([]), require('fs').globSync('y'), path.matchesGlob('a', 'b'));\nawait import('node:sea');\n";
    let found = uses("src/a.mjs", src).unwrap();
    for k in ["module:node:sqlite", "member:sqlite.DatabaseSync", "member:fs.globSync", "member:fs/promises.glob", "member:util.styleText", "member:util.parseArgs",
        "member:process.getBuiltinModule", "global:WebSocket", "global:import.meta.dirname", "global:AbortSignal.any", "module:node:sea", "member:path.matchesGlob"] {
        assert!(found.contains(k), "{k}: {found:?}");
    }
    let shadowed = uses("src/b.js", "const WebSocket = require('ws');\nnew WebSocket('x');\nimport test from 'test';\nconst fetch = () => 1;\nfetch();\n").unwrap();
    assert!(shadowed.is_empty(), "a local WebSocket or fetch and the npm package named test are not Node's: {shadowed:?}");
}
