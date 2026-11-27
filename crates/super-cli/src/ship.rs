//! Install Super binaries onto a directory that is on `PATH`, and remove them again.
//!
//! The destination is chosen so a new shell can run the program on every OS:
//! an explicit directory, a user bin that is already on `PATH`, or `~/.local/bin`
//! after that directory is added to the user environment.

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

#[cfg(any(not(windows), test))]
pub const MARK_BEGIN: &str = "# >>> super path >>>";
#[cfg(any(not(windows), test))]
pub const MARK_END: &str = "# <<< super path <<<";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageRoot {
    pub manifest: PathBuf,
    pub root: PathBuf,
    pub name: String,
    pub version: String,
    /// Pod kind: bin, lib, or macro.
    pub kind: crate::pkg::PodKind,
    /// `src/main.sr` when the Pod is an executable.
    pub main: Option<PathBuf>,
}

impl PackageRoot {
    pub fn is_binary(&self) -> bool {
        self.kind.is_installable() && self.main.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub bin: PathBuf,
    pub root: PathBuf,
    pub profile: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct InstallDb {
    #[serde(default)]
    installed: Vec<Installed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinChoice {
    pub dir: PathBuf,
    /// `true` when `dir` is the default location and is not already on `PATH`.
    pub persist: bool,
}

/// Packages under `start`: one package, a `[workspace]`, or a directory of packages.
pub fn package_roots(start: &Path) -> Result<Vec<PackageRoot>, String> {
    if !start.exists() {
        return Err(format!("{} does not exist", start.display()));
    }
    let mut roots = if let Some(manifest) = nearest_manifest(start) {
        roots_from_manifest(&manifest)?
    } else if start.is_dir() {
        implicit_workspace(start)?
    } else {
        return Err(format!(
            "could not find {} from {}",
            crate::pkg::MANIFEST_POD,
            start.display()
        ));
    };
    roots.sort_by(|a, b| a.name.cmp(&b.name).then(a.root.cmp(&b.root)));
    roots.dedup_by(|a, b| a.root == b.root);
    if roots.is_empty() {
        return Err(format!("no Super pods under {}", start.display()));
    }
    Ok(roots)
}

pub fn executable_file_name(name: &str) -> String {
    crate::pkg::executable_name(name)
}

pub fn valid_bin_name(name: &str) -> bool {
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    !stem.is_empty()
        && stem != "."
        && stem != ".."
        && !stem.contains(['/', '\\', '\0'])
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn choose_bin_dir(home: &Path, path_env: &str, explicit: Option<&Path>) -> BinChoice {
    if let Some(dir) = explicit {
        return BinChoice {
            dir: dir.to_path_buf(),
            persist: false,
        };
    }
    for candidate in user_bin_candidates(home) {
        if dir_on_path(&candidate, path_env) {
            return BinChoice {
                dir: candidate,
                persist: false,
            };
        }
    }
    BinChoice {
        dir: home.join(".local").join("bin"),
        persist: true,
    }
}

pub fn dir_on_path(dir: &Path, path_env: &str) -> bool {
    let want = normalize_dir(dir);
    env::split_paths(path_env).any(|entry| dirs_equal(&normalize_dir(&entry), &want))
}

/// Create the install directory. When the default directory is not on `PATH`,
/// add it to the user environment and return a note for the operator.
pub fn prepare_bin_dir(explicit: Option<&Path>) -> Result<(PathBuf, Option<String>), String> {
    let home = home_dir()?;
    let path_env = env::var("PATH").unwrap_or_default();
    let from_env = explicit
        .is_none()
        .then(|| env::var_os("SUPER_INSTALL_DIR"))
        .flatten();
    let explicit_buf = explicit
        .map(Path::to_path_buf)
        .or_else(|| from_env.map(PathBuf::from));
    let choice = choose_bin_dir(&home, &path_env, explicit_buf.as_deref());
    fs::create_dir_all(&choice.dir).map_err(|e| format!("mkdir {}: {e}", choice.dir.display()))?;
    if !choice.persist || dir_on_path(&choice.dir, &path_env) {
        let note = if explicit_buf.is_some() && !dir_on_path(&choice.dir, &path_env) {
            Some(format!("note: {} is not on PATH", choice.dir.display()))
        } else {
            None
        };
        return Ok((choice.dir, note));
    }
    let note = persist_default_path(&home, &choice.dir)?;
    Ok((choice.dir, Some(note)))
}

pub fn install_binary(
    package: &PackageRoot,
    source: &Path,
    dest_dir: &Path,
    force: bool,
) -> Result<PathBuf, String> {
    if !valid_bin_name(&package.name) {
        return Err(format!(
            "package name `{}` cannot be an executable name",
            package.name
        ));
    }
    if !source.is_file() {
        return Err(format!("built binary {} is missing", source.display()));
    }
    let dest = dest_dir.join(executable_file_name(&package.name));
    let db = load_db()?;
    let owned = db.installed.iter().any(|row| row.name == package.name);
    if dest.exists() && !owned && !force {
        return Err(format!(
            "{} exists and was not installed by super. Pass --force to replace it",
            dest.display()
        ));
    }
    if let Some(previous) = db.installed.iter().find(|row| row.name == package.name)
        && previous.bin != dest
        && previous.bin.is_file()
    {
        let _ = fs::remove_file(&previous.bin);
    }
    replace_file(source, &dest)?;
    let mut db = db;
    db.installed.retain(|row| row.name != package.name);
    db.installed.push(Installed {
        name: package.name.clone(),
        version: package.version.clone(),
        bin: dest.clone(),
        root: package.root.clone(),
        profile: "release".into(),
    });
    db.installed.sort_by(|a, b| a.name.cmp(&b.name));
    save_db(&db)?;
    Ok(dest)
}

pub fn uninstall_name(name: &str) -> Result<PathBuf, String> {
    let mut db = load_db()?;
    let Some(index) = db.installed.iter().position(|row| row.name == name) else {
        return Err(format!(
            "`{name}` is not installed by super. `super list` shows installed programs"
        ));
    };
    let row = db.installed.remove(index);
    if row.bin.is_file() {
        fs::remove_file(&row.bin).map_err(|e| format!("remove {}: {e}", row.bin.display()))?;
    }
    save_db(&db)?;
    Ok(row.bin)
}

pub fn list_installed() -> Result<Vec<Installed>, String> {
    let mut rows = load_db()?.installed;
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(rows)
}

pub fn which(name: &str) -> Result<PathBuf, String> {
    let rows = list_installed()?;
    let Some(row) = rows.into_iter().find(|row| row.name == name) else {
        return Err(format!("`{name}` is not installed"));
    };
    if !row.bin.is_file() {
        return Err(format!(
            "`{name}` is recorded at {} but the file is missing. Run `super install` again",
            row.bin.display()
        ));
    }
    Ok(row.bin)
}

pub fn data_dir() -> Result<PathBuf, String> {
    if let Some(dir) = env::var_os("SUPER_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Some(xdg) = env::var_os("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("super"));
    }
    #[cfg(windows)]
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        if !local.is_empty() {
            return Ok(PathBuf::from(local).join("super"));
        }
    }
    Ok(home_dir()?.join(".local").join("share").join("super"))
}

pub fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "HOME is not set; pass --bin-dir".to_string())
}

/// Shell startup files that should grow a `PATH` entry for `dir`.
#[cfg(any(not(windows), test))]
pub fn profile_files(home: &Path, shell: &str) -> Vec<PathBuf> {
    match shell {
        "zsh" => vec![home.join(".zshenv")],
        "bash" => vec![home.join(".bashrc"), home.join(".profile")],
        "fish" => vec![home.join(".config").join("fish").join("config.fish")],
        "nu" | "nushell" => vec![home.join(".config").join("nushell").join("env.nu")],
        _ => vec![home.join(".profile")],
    }
}

#[cfg(any(not(windows), test))]
pub fn path_snippet(dir: &Path, home: &Path, shell: &str) -> String {
    let rendered = path_for_shell(dir, home, shell);
    match shell {
        "fish" => format!("{MARK_BEGIN}\nfish_add_path -g {rendered}\n{MARK_END}\n"),
        "nu" | "nushell" => {
            format!("{MARK_BEGIN}\n$env.PATH = ($env.PATH | prepend {rendered})\n{MARK_END}\n")
        }
        _ => format!(
            "{MARK_BEGIN}\ncase \":$PATH:\" in\n  *\":{rendered}:\"*) ;;\n  *) export PATH=\"{rendered}:$PATH\" ;;\nesac\n{MARK_END}\n"
        ),
    }
}

#[cfg(any(not(windows), test))]
pub fn upsert_marked_block(existing: &str, block: &str) -> String {
    let block = if block.ends_with('\n') {
        block.to_string()
    } else {
        format!("{block}\n")
    };
    if let Some(start) = existing.find(MARK_BEGIN)
        && let Some(rel) = existing[start..].find(MARK_END)
    {
        let after = start + rel + MARK_END.len();
        let mut out = String::new();
        out.push_str(&existing[..start]);
        out.push_str(block.trim_end_matches('\n'));
        out.push('\n');
        let rest = existing[after..].trim_start_matches(['\n', '\r']);
        if !rest.is_empty() {
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(rest);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
        return out;
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
    out.push_str(&block);
    out
}

fn persist_default_path(_home: &Path, dir: &Path) -> Result<String, String> {
    #[cfg(windows)]
    {
        persist_windows_path(dir)?;
        return Ok(format!(
            "added {} to the user Path. Open a new terminal to run installed programs",
            dir.display()
        ));
    }
    #[cfg(not(windows))]
    {
        let shell = env::var("SHELL")
            .ok()
            .and_then(|value| {
                Path::new(&value)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "sh".into());
        let files = profile_files(_home, &shell);
        for file in &files {
            let existing = fs::read_to_string(file).unwrap_or_default();
            let updated = upsert_marked_block(&existing, &path_snippet(dir, _home, &shell));
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
            }
            if updated != existing {
                fs::write(file, &updated).map_err(|e| format!("write {}: {e}", file.display()))?;
            }
        }
        let listed = files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(format!(
            "added {} to PATH via {listed}. Open a new terminal to run installed programs",
            dir.display()
        ))
    }
}

#[cfg(windows)]
fn persist_windows_path(dir: &Path) -> Result<(), String> {
    let dir = dir.display().to_string().replace('\'', "''");
    let script = format!(
        "$dir = '{dir}'; \
         $user = [Environment]::GetEnvironmentVariable('Path', 'User'); \
         if (-not $user) {{ $user = '' }}; \
         $parts = $user -split ';' | Where-Object {{ $_ -and ($_.TrimEnd('\\') -ine $dir.TrimEnd('\\')) }}; \
         $next = (@($dir) + $parts) -join ';'; \
         [Environment]::SetEnvironmentVariable('Path', $next, 'User')"
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| format!("powershell: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "could not add {} to the user Path (powershell status {status})",
            dir
        ))
    }
}

fn replace_file(source: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_file_name(format!(
        ".{}.super-tmp",
        dest.file_name().and_then(|s| s.to_str()).unwrap_or("bin")
    ));
    fs::copy(source, &tmp).map_err(|e| format!("copy {}: {e}", dest.display()))?;
    set_executable(&tmp)?;
    if dest.exists() {
        fs::remove_file(dest).map_err(|e| format!("replace {}: {e}", dest.display()))?;
    }
    fs::rename(&tmp, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename {}: {e}", dest.display())
    })
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|e| format!("stat {}: {e}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).map_err(|e| format!("chmod {}: {e}", path.display()))?;
    }
    let _ = path;
    Ok(())
}

fn user_bin_candidates(home: &Path) -> [PathBuf; 3] {
    [
        home.join(".local").join("bin"),
        home.join("bin"),
        home.join(".cargo").join("bin"),
    ]
}

#[cfg(any(not(windows), test))]
fn path_for_shell(dir: &Path, home: &Path, shell: &str) -> String {
    let relative = dir
        .strip_prefix(home)
        .ok()
        .map(|rest| rest.to_string_lossy().replace('\\', "/"));
    match (shell, relative) {
        ("nu" | "nushell", Some(rest)) if !rest.is_empty() => {
            format!("$\"($env.HOME)/{rest}\"")
        }
        ("nu" | "nushell", _) => format!("\"{}\"", dir.display()),
        (_, Some(rest)) if !rest.is_empty() => format!("$HOME/{rest}"),
        _ => dir.display().to_string(),
    }
}

fn dirs_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn normalize_dir(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn nearest_manifest(start: &Path) -> Option<PathBuf> {
    crate::pkg::find_manifest(start)
}

fn roots_from_manifest(manifest: &Path) -> Result<Vec<PackageRoot>, String> {
    let file = crate::pkg::parse_manifest(manifest)?;
    let root = manifest
        .parent()
        .ok_or_else(|| format!("{} has no parent", manifest.display()))?
        .to_path_buf();
    let members = file
        .workspace
        .as_ref()
        .map(|workspace| workspace.members.clone())
        .unwrap_or_default();
    let mut roots = Vec::new();
    if file.identity().is_ok() {
        roots.push(root_from_manifest(manifest, &file)?);
    }
    for member in members {
        let member_root = root.join(&member);
        let member_manifest = crate::pkg::child_manifest(&member_root).ok_or_else(|| {
            format!(
                "workspace member `{member}` has no {}",
                crate::pkg::MANIFEST_POD
            )
        })?;
        let member_file = crate::pkg::parse_manifest(&member_manifest)?;
        member_file
            .identity()
            .map_err(|_| format!("workspace member `{member}` has no [pod] table"))?;
        roots.push(root_from_manifest(&member_manifest, &member_file)?);
    }
    if roots.is_empty() {
        return Err(format!(
            "{} has neither [pod] nor [workspace] members",
            manifest.display()
        ));
    }
    Ok(roots)
}

fn implicit_workspace(start: &Path) -> Result<Vec<PackageRoot>, String> {
    let mut roots = Vec::new();
    let entries = fs::read_dir(start).map_err(|e| format!("read {}: {e}", start.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | "target" | "vendor" | "src" | "assets")
        ) {
            continue;
        }
        if let Some(manifest) = crate::pkg::child_manifest(&path) {
            let file = crate::pkg::parse_manifest(&manifest)?;
            if file.identity().is_ok() {
                roots.push(root_from_manifest(&manifest, &file)?);
            }
        }
    }
    Ok(roots)
}

fn root_from_manifest(manifest: &Path, file: &crate::pkg::Manifest) -> Result<PackageRoot, String> {
    let package = file
        .identity()
        .map_err(|_| format!("{} has no [pod] table", manifest.display()))?;
    let root = manifest
        .parent()
        .ok_or_else(|| format!("{} has no parent", manifest.display()))?
        .to_path_buf();
    let kind = package
        .kind
        .unwrap_or_else(|| crate::pkg::infer_pod_kind(&root));
    let main = root.join("src").join("main.sr");
    Ok(PackageRoot {
        manifest: manifest.to_path_buf(),
        root,
        name: package.name.clone(),
        version: if package.version.is_empty() {
            "0.0.0".into()
        } else {
            package.version.clone()
        },
        kind,
        main: main.is_file().then_some(main),
    })
}

fn db_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("installed.toml"))
}

fn load_db() -> Result<InstallDb, String> {
    let path = db_path()?;
    if !path.is_file() {
        return Ok(InstallDb::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(InstallDb::default());
    }
    toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn save_db(db: &InstallDb) -> Result<(), String> {
    let path = db_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let text = toml::to_string_pretty(db).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(dir: &Path, name: &str, binary: bool) {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join(crate::pkg::MANIFEST_POD),
            format!("[pod]\nname = \"{name}\"\nversion = \"1.2.3\"\n"),
        )
        .unwrap();
        let file = if binary { "main.sr" } else { "lib.sr" };
        fs::write(dir.join("src").join(file), "+main() {{}}\n").unwrap();
    }

    #[test]
    fn package_from_a_subdirectory_and_an_implicit_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        pkg(&app, "app", true);
        let nested = app.join("src");
        let found = package_roots(&nested).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "app");
        assert!(found[0].is_binary());

        let workspace = temp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();
        pkg(&workspace.join("one"), "one", true);
        pkg(&workspace.join("lib"), "lib", false);
        let found = package_roots(&workspace).unwrap();
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .any(|root| root.name == "one" && root.is_binary())
        );
        assert!(
            found
                .iter()
                .any(|root| root.name == "lib" && !root.is_binary())
        );
    }

    #[test]
    fn workspace_members_are_loaded_from_the_root_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        pkg(&root.join("app"), "app", true);
        pkg(&root.join("tool"), "tool", true);
        fs::write(
            root.join(crate::pkg::MANIFEST_POD),
            "[workspace]\nmembers = [\"app\", \"tool\"]\n",
        )
        .unwrap();
        let found = package_roots(root).unwrap();
        let names: Vec<_> = found.iter().map(|root| root.name.as_str()).collect();
        assert_eq!(names, ["app", "tool"]);
    }

    #[test]
    fn bin_dir_prefers_a_directory_already_on_path() {
        let home = Path::new("/home/ada");
        let cargo = home.join(".cargo").join("bin");
        let path = env::join_paths([Path::new("/usr/bin"), &cargo]).unwrap();
        let path = path.to_string_lossy();
        let choice = choose_bin_dir(home, &path, None);
        assert_eq!(choice.dir, cargo);
        assert!(!choice.persist);

        let fallback = choose_bin_dir(home, "/usr/bin", None);
        assert_eq!(fallback.dir, home.join(".local").join("bin"));
        assert!(fallback.persist);

        let explicit = Path::new("/opt/bin");
        let forced = choose_bin_dir(home, &path, Some(explicit));
        assert_eq!(forced.dir, explicit);
        assert!(!forced.persist);
    }

    #[test]
    fn path_block_is_idempotent_and_shell_specific() {
        let home = Path::new("/home/ada");
        let dir = home.join(".local").join("bin");
        let zsh = path_snippet(&dir, home, "zsh");
        assert!(zsh.contains("export PATH=\"$HOME/.local/bin:$PATH\""));
        assert!(profile_files(home, "zsh") == vec![home.join(".zshenv")]);
        assert!(profile_files(home, "bash").len() == 2);
        let fish = path_snippet(&dir, home, "fish");
        assert!(fish.contains("fish_add_path -g $HOME/.local/bin"));
        let once = upsert_marked_block("export A=1\n", &zsh);
        let twice = upsert_marked_block(&once, &zsh);
        assert_eq!(once.matches(MARK_BEGIN).count(), 1);
        assert_eq!(twice.matches(MARK_BEGIN).count(), 1);
        assert!(twice.contains("export A=1"));
    }

    #[test]
    fn install_records_a_binary_and_refuses_a_foreign_file() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        unsafe {
            env::set_var("SUPER_DATA_DIR", &data);
        }
        let package = PackageRoot {
            manifest: temp.path().join(crate::pkg::MANIFEST_POD),
            root: temp.path().to_path_buf(),
            name: "hello".into(),
            version: "0.1.0".into(),
            kind: crate::pkg::PodKind::Bin,
            main: Some(temp.path().join("src/main.sr")),
        };
        let source = temp.path().join("built");
        fs::write(&source, b"#!/bin/sh\n").unwrap();
        let foreign = bin.join(executable_file_name("hello"));
        fs::write(&foreign, b"nope").unwrap();
        let err = install_binary(&package, &source, &bin, false).unwrap_err();
        assert!(err.contains("--force"), "{err}");
        let dest = install_binary(&package, &source, &bin, true).unwrap();
        assert_eq!(dest, foreign);
        assert_eq!(fs::read(&dest).unwrap(), b"#!/bin/sh\n");
        assert_eq!(which("hello").unwrap(), dest);
        assert_eq!(list_installed().unwrap().len(), 1);
        let removed = uninstall_name("hello").unwrap();
        assert_eq!(removed, dest);
        assert!(!dest.exists());
        assert!(list_installed().unwrap().is_empty());
        unsafe {
            env::remove_var("SUPER_DATA_DIR");
        }
    }
}
