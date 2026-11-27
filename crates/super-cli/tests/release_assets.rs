//! Release tarball naming must match install.sh `detect_target()` triples.
use std::process::Command;

#[test]
fn verify_release_assets_script_ok_on_host_bundle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root");
    let script = root.join("scripts/verify_release_assets.sh");
    assert!(script.is_file(), "missing {}", script.display());
    let status = Command::new("bash")
        .arg(&script)
        .current_dir(root)
        .env("SUPER_VERSION", "0.1.0")
        .status()
        .expect("run verify_release_assets.sh");
    if !status.success() {
        // Bootstrap bundle when dist/ is empty (CI smoke builds first).
        let pkg = root.join("scripts/package_release.sh");
        let built = Command::new("bash")
            .arg(&pkg)
            .current_dir(root)
            .status()
            .expect("package");
        assert!(built.success(), "package_release.sh failed");
        let retry = Command::new("bash")
            .arg(&script)
            .current_dir(root)
            .env("SUPER_VERSION", "0.1.0")
            .status()
            .expect("retry verify");
        assert!(
            retry.success(),
            "verify_release_assets.sh failed after package"
        );
    }
}
