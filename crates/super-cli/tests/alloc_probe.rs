//! Linux gate: delegates to `scripts/alloc_probe.sh` (LD_PRELOAD pin 0/0).
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn app_serve_allocations_per_request_are_pinned() {
    let script = repo().join("scripts/alloc_probe.sh");
    let status = Command::new("bash")
        .arg(&script)
        .env("SUPER_BIN", env!("CARGO_BIN_EXE_super"))
        .env("SUPER_STD", repo().join("stdlib"))
        .status()
        .expect("run alloc_probe.sh");
    assert!(status.success(), "scripts/alloc_probe.sh failed (see stderr)");
}