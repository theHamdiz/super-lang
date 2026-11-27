//! VM request-scoped reclaim soak: repeated enter/leave must not grow pending stays.
#![cfg(not(target_os = "windows"))]

use std::process::Command;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn vm_req_pending_stays_flat_after_many_leaves() {
    let super_bin = PathBuf::from(env!("CARGO_BIN_EXE_super"));
    let script = repo().join("crates/super-cli/tests/fixtures/vm_req_soak.sr");
    assert!(script.exists(), "{}", script.display());

    let out = Command::new(super_bin)
        .args(["run", script.to_str().unwrap()])
        .env("SUPER_STD", repo().join("stdlib"))
        .output()
        .expect("super run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("vm_req_soak: PASS"),
        "unexpected output: {stdout}"
    );
}