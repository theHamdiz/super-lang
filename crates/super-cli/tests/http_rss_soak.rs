//! Linux RSS soak CI wrapper → `scripts/http_rss_soak.sh` (server: Super AOT `new.sr`).
//! Full flat-RSS gate: `scripts/soak_http.sh` (Super soak driver).
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn passthrough(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[test]
fn http_rss_soak_aot_or_run() {
    let script = repo().join("scripts/http_rss_soak.sh");
    let mut cmd = Command::new("bash");
    cmd.arg(&script)
        .env("SUPER_BIN", env!("CARGO_BIN_EXE_super"))
        .env("SUPER_STD", repo().join("stdlib"));
    for key in [
        "SUPER_SOAK_REQUESTS",
        "SUPER_SOAK_FULL",
        "SUPER_SOAK_FALLBACK",
        "SUPER_SOAK_RSS_SPREAD_KIB",
    ] {
        if let Some(v) = passthrough(key) {
            cmd.env(key, v);
        }
    }
    let output = cmd.output().expect("run http_rss_soak.sh");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    assert!(
        output.status.success(),
        "scripts/http_rss_soak.sh failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}