// kern check: one receipt for a pull request. These tests cover the parts that run without a sandbox: the rules a test run's sandbox
// must follow, how the old and the new code's test results are compared, the static rules, the receipt and the keyless proof.
use kern::policy::Limits;
use kern::sandbox::{self, Spec};
use kern::util::*;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Temp {
        let p = std::env::temp_dir().join(format!("kern-check-{}", uuid4().unwrap()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}
fn denied<T>(r: R<T>, needle: &str) {
    match r { Ok(_) => panic!("expected denial containing {needle:?}"), Err(e) => assert!(e.0.contains(needle), "{:?} does not contain {needle:?}", e.0) }
}
fn limits() -> Limits { Limits { files_changed: 1, lines_changed: 1, file_bytes: 1 << 20, total_bytes: 64 << 20, timeout_seconds: 900, memory_mb: 4096, pids: 1024 } }

#[test]
fn a_test_run_reads_its_tree_read_only_without_network() {
    let t = Temp::new();
    let l = limits();
    let run = Spec { image: "sha256:x", snapshot: &t.0, control: &t.0, writable: &[], limits: &l, entry: vec!["__drive_test".into()], network: "none", volumes: vec![], root: false,
        home_mb: 0, scratch: false, workspace: Some(("kern-deps-tree", true)) };
    let a = sandbox::container_args(&run, "kern-x").unwrap().join(" ");
    for want in ["type=volume,source=kern-deps-tree,target=/workspace,readonly", "--network=none", "--user=1000:1000", "--runtime=runsc", "--read-only", "nofile=65536"] {
        assert!(a.contains(want), "{want}: {a}");
    }
    // Tools keep their caches in memory, as in a drive run: the installed tree itself stays read-only, so no test can change what later runs see.
    for dir in [".cache", ".vite", ".vite-temp"] { assert!(a.contains(&format!("--tmpfs /workspace/node_modules/{dir}:rw")), "{dir}: {a}"); }
    assert!(a.matches("type=bind").count() == 2, "only /input and /kern are bound: {a}");
    // A test run never gets a network, root, write grants, a writable tree, a scratch copy or another volume.
    let write = vec!["src/".to_string()];
    let tools = vec![("kern-deps-tools", "/kern-tools/node_modules", true)];
    denied(sandbox::container_args(&Spec { network: "bridge", ..run.clone() }, "kern-x"), "network");
    for bad in [Spec { root: true, ..run.clone() }, Spec { writable: &write, ..run.clone() }, Spec { workspace: Some(("kern-deps-tree", false)), ..run.clone() },
        Spec { workspace: None, ..run.clone() }, Spec { scratch: true, ..run.clone() }, Spec { volumes: tools.clone(), ..run.clone() }] {
        denied(sandbox::container_args(&bad, "kern-x"), "test run");
    }
}

#[test]
fn a_test_run_report_is_read_by_length() {
    let mut out = Vec::new();
    for (name, body) in [("output", &b"{\"stats\":{}}\n"[..]), ("errors", b""), ("behavior", b"exec:id\n"), ("ending", b"exit 1")] {
        out.extend(format!("{}{name} {}\n", sandbox::DRIVE_MARK, body.len()).as_bytes());
        out.extend(body);
    }
    let parts = sandbox::parts(&out, &sandbox::TEST_PARTS).unwrap();
    assert_eq!(parts["ending"], b"exit 1");
    assert_eq!(parts["behavior"], b"exec:id\n");
    // Output that pretends to end the report early, or extra bytes after it, is refused.
    let mut forged = out.clone();
    forged.extend(b"more");
    denied(sandbox::parts(&forged, &sandbox::TEST_PARTS), "unexpected data");
    denied(sandbox::parts(&out[..out.len() - 2], &sandbox::TEST_PARTS), "incomplete");
}
