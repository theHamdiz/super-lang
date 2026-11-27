//! Remove Super package binaries under `target/`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ship::{self, PackageRoot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanMode {
    /// `target/dev` and `target/debug`.
    Debug,
    /// `target/release`.
    Release,
    /// The whole `target/` directory.
    All,
}

impl CleanMode {
    pub fn from_flags(debug: bool, release: bool, all: bool) -> Self {
        if debug {
            Self::Debug
        } else if release {
            Self::Release
        } else {
            let _ = all;
            Self::All
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub path: PathBuf,
    pub bytes: u64,
}

pub fn clean_packages(roots: &[PackageRoot], mode: CleanMode) -> Result<Vec<Removed>, String> {
    let mut removed = Vec::new();
    let mut seen = Vec::new();
    for root in roots {
        let target = root.root.join("target");
        if seen.iter().any(|prev: &PathBuf| prev == &target) {
            continue;
        }
        seen.push(target.clone());
        match mode {
            CleanMode::All => {
                if target.exists() {
                    removed.push(remove_path(&target)?);
                }
            }
            CleanMode::Debug => {
                for name in ["dev", "debug"] {
                    let path = target.join(name);
                    if path.exists() {
                        removed.push(remove_path(&path)?);
                    }
                }
            }
            CleanMode::Release => {
                let path = target.join("release");
                if path.exists() {
                    removed.push(remove_path(&path)?);
                }
            }
        }
    }
    Ok(removed)
}

pub fn discover_and_clean(start: &Path, mode: CleanMode) -> Result<Vec<Removed>, String> {
    let roots = ship::package_roots(start)?;
    clean_packages(&roots, mode)
}

pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn report(removed: &[Removed], mode: CleanMode) -> String {
    if removed.is_empty() {
        return format!("nothing to clean ({})", mode.label());
    }
    let mut lines = Vec::new();
    let mut total = 0u64;
    for item in removed {
        total += item.bytes;
        lines.push(format!(
            "removed {} ({})",
            item.path.display(),
            format_bytes(item.bytes)
        ));
    }
    lines.push(format!(
        "cleaned {} director{} ({}, {})",
        removed.len(),
        if removed.len() == 1 { "y" } else { "ies" },
        mode.label(),
        format_bytes(total)
    ));
    lines.join("\n")
}

fn remove_path(path: &Path) -> Result<Removed, String> {
    let bytes = path_bytes(path);
    if path.is_dir() && !path.is_symlink() {
        fs::remove_dir_all(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    } else {
        fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(Removed {
        path: path.to_path_buf(),
        bytes,
    })
}

fn path_bytes(path: &Path) -> u64 {
    if path.is_file() || path.is_symlink() {
        return fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        total += path_bytes(&entry.path());
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(dir: &Path, name: &str) -> PackageRoot {
        PackageRoot {
            manifest: dir.join(crate::pkg::MANIFEST_POD),
            root: dir.to_path_buf(),
            name: name.into(),
            version: "0.1.0".into(),
            kind: crate::pkg::PodKind::Bin,
            main: Some(dir.join("src/main.sr")),
        }
    }

    #[test]
    fn debug_release_and_all_remove_only_their_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        for name in ["dev", "debug", "release", "fast", "size", "test"] {
            let dir = app.join("target").join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("app"), b"bin").unwrap();
        }
        let package = root(&app, "app");
        let debug = clean_packages(std::slice::from_ref(&package), CleanMode::Debug).unwrap();
        assert!(debug.iter().any(|item| item.path.ends_with("dev")));
        assert!(debug.iter().any(|item| item.path.ends_with("debug")));
        assert!(app.join("target/release/app").is_file());
        assert!(!app.join("target/dev").exists());

        let release = clean_packages(std::slice::from_ref(&package), CleanMode::Release).unwrap();
        assert_eq!(release.len(), 1);
        assert!(release[0].path.ends_with("release"));
        assert!(app.join("target/fast/app").is_file());

        let all = clean_packages(&[package], CleanMode::All).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].path.ends_with("target"));
        assert!(!app.join("target").exists());
        let text = report(&all, CleanMode::All);
        assert!(text.contains("cleaned 1 directory"));
        assert!(text.contains("all"));
    }

    #[test]
    fn empty_clean_is_a_stable_report() {
        assert_eq!(
            report(&[], CleanMode::from_flags(false, false, false)),
            "nothing to clean (all)"
        );
        assert_eq!(CleanMode::from_flags(true, false, false), CleanMode::Debug);
        assert_eq!(format_bytes(1536), "1.5 KiB");
    }
}
