use std::fs;
use std::path::PathBuf;
use zed_extension_api::{self as zed, LanguageServerId, Result, settings::LspSettings};

struct SuperExtension;

fn resolve_stdlib_root(worktree: &zed::Worktree) -> Option<String> {
    let root = worktree.root_path();
    for candidate in [
        format!("{root}/stdlib"),
        format!("{root}/../stdlib"),
    ] {
        if fs::metadata(&candidate)
            .map(|m| m.is_dir())
            .unwrap_or(false)
            && fs::metadata(format!("{candidate}/mod.sr"))
                .map(|m| m.is_file())
                .unwrap_or(false)
        {
            return PathBuf::from(candidate)
                .canonicalize()
                .map(|p| p.display().to_string())
                .ok();
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let installed = format!("{home}/.cargo/share/super/stdlib");
        if fs::metadata(&installed)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return PathBuf::from(installed)
                .canonicalize()
                .map(|p| p.display().to_string())
                .ok();
        }
    }

    for key in ["SUPER_STDLIB", "SUPER_STD"] {
        if let Ok(p) = std::env::var(key) {
            if fs::metadata(&p)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                return Some(p);
            }
        }
    }
    None
}

fn super_command(
    worktree: &zed::Worktree,
    binary: String,
    arguments: Vec<String>,
) -> zed::Command {
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(stdlib) = resolve_stdlib_root(worktree) {
        env.push(("SUPER_STDLIB".into(), stdlib.clone()));
        env.push(("SUPER_STD".into(), stdlib));
    }
    zed::Command {
        command: binary,
        args: arguments,
        env,
    }
}

impl zed::Extension for SuperExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let ls_id = language_server_id.as_ref();
        if ls_id != "super-lsp" {
            return Err(format!("unexpected language server id: {ls_id}").into());
        }

        // Prefer explicit user override from settings.
        if let Ok(settings) = LspSettings::for_worktree("super-lsp", worktree) {
            if let Some(binary) = settings.binary {
                if let Some(path) = binary.path {
                    return Ok(super_command(
                        worktree,
                        path,
                        binary
                            .arguments
                            .unwrap_or_else(|| vec!["lsp".into()]),
                    ));
                }
            }
        }

        // Prefer super on PATH (cargo install --path crates/super-cli).
        if let Some(path) = worktree.which("super") {
            return Ok(super_command(worktree, path, vec!["lsp".into()]));
        }

        // Fall back to common local build outputs relative to the worktree.
        let root = worktree.root_path();
        for rel in [
            "target/release/super",
            "target/debug/super",
            "crates/super-cli/../../target/release/super",
            "crates/super-cli/../../target/debug/super",
        ] {
            let candidate = format!("{root}/{rel}");
            if fs::metadata(&candidate).is_ok() {
                return Ok(super_command(worktree, candidate, vec!["lsp".into()]));
            }
        }

        Err(
            "super binary not found. Install with: cargo install --path crates/super-cli  (or set lsp.super-lsp.binary.path)"
                .into(),
        )
    }
}

zed::register_extension!(SuperExtension);