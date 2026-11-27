//! Regression: analyzing `await spawn` (including duplicate `u =` rebinds) must not panic.
//! An LSP panic takes down Zed's Super extension entirely.

use std::path::PathBuf;

#[test]
fn analyze_await_spawn_variants_do_not_panic() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/02_task_api/src/main.sr");
    let base = std::fs::read_to_string(&root).unwrap();
    let needle = "        u = await spawn fetch_user(id)\n        c = await spawn fetch_cache(id)";
    assert!(
        base.contains(needle),
        "fixture pattern missing from main.sr — update test"
    );
    let variants: &[(&str, &str)] = &[
        (
            "both",
            "        u = await t_user\n        u = await spawn fetch_user(id)\n        c = await t_cache",
        ),
        (
            "only",
            "        u = await spawn fetch_user(id)\n        c = await spawn fetch_cache(id)",
        ),
        (
            "incomplete",
            "        u = await spawn\n        c = await t_cache",
        ),
        (
            "dup",
            "        u = await spawn fetch_user(id)\n        u = await spawn fetch_user(id)\n        c = await t_cache",
        ),
    ];
    for (name, new) in variants {
        let text = base.replace(needle, new);
        let path = root.clone();
        let result =
            std::panic::catch_unwind(|| super_frontend::analyze_project_text(&path, &text));
        assert!(
            result.is_ok(),
            "panicked on variant {name}: {:?}",
            result.err()
        );
        let a = result.unwrap();
        eprintln!("{name}: {} diagnostics", a.diagnostics.iter().count());
    }
}
