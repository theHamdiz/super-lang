//! `.pod` archives (D2–D4): tar + zstd, `pod.manifest` first, safe extract, content hashes.

use crate::pkg::{self, MANIFEST_POD, Manifest};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use tar::{Builder, EntryType, Header};
use zstd::stream::{decode_all, encode_all};

/// Archive format version stamped into `pod.manifest`.
pub const POD_FORMAT: u32 = 1;
/// Reject individual members larger than this (bomb defense).
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// Reject archives with more members than this.
pub const MAX_ENTRIES: usize = 100_000;
/// Reject uncompressed payloads larger than this.
pub const MAX_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;
/// Zstandard magic (little-endian `0xFD2FB528`) — no custom Super magic.
pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

#[derive(Debug, Clone)]
pub struct PackResult {
    pub archive: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct ManifestInfo {
    pub format: u32,
    pub name: String,
    pub version: String,
    #[allow(dead_code)]
    pub super_version: String,
}

/// SHA-256 hex digest of raw bytes, prefixed `sha256:`.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn normalize_rel(path: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!(
                    "refusing `..` path component in {}",
                    path.display()
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("refusing absolute path {}", path.display()));
            }
        }
    }
    if parts.is_empty() {
        return Err("empty archive member path".into());
    }
    Ok(parts.join("/"))
}

fn should_skip(rel: &str) -> bool {
    rel.starts_with("target/")
        || rel.contains("/target/")
        || rel.starts_with(".git/")
        || rel.contains("/.git/")
        || rel.ends_with(".pod")
        || rel == "Pod.lock"
        || rel.ends_with("/Pod.lock")
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .map_err(|_| format!("{} escapes {}", path.display(), root.display()))?;
            let rel_s = normalize_rel(rel)?;
            if should_skip(&rel_s) {
                continue;
            }
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_symlink() {
                return Err(format!("refusing symlink member {}", path.display()));
            }
            if ft.is_dir() {
                walk(&path, root, out)?;
            } else if ft.is_file() {
                out.push(path);
            }
        }
        Ok(())
    }
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn build_pod_manifest(manifest: &Manifest) -> Result<String, String> {
    let id = manifest.identity()?;
    Ok(format!(
        "format = {POD_FORMAT}\nname = {:?}\nversion = {:?}\nsuper = {:?}\n",
        id.name,
        id.version,
        env!("CARGO_PKG_VERSION")
    ))
}

fn append_bytes(
    builder: &mut Builder<Vec<u8>>,
    path: &str,
    data: &[u8],
    mtime: u64,
) -> Result<(), String> {
    let mut header = Header::new_gnu();
    header.set_path(path).map_err(|e| e.to_string())?;
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(mtime);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();
    builder
        .append(&header, data)
        .map_err(|e| format!("tar append {path}: {e}"))
}

/// Pack a Pod root into a deterministic `.pod` (tar+zstd). `pod.manifest` is always first.
pub fn pack_directory(root: &Path, out: &Path) -> Result<PackResult, String> {
    let manifest_path = root.join(MANIFEST_POD);
    if !manifest_path.is_file() {
        return Err(format!("{} missing {}", root.display(), MANIFEST_POD));
    }
    let manifest = pkg::parse_manifest(&manifest_path)?;
    let mtime = 0u64; // deterministic
    let mut builder = Builder::new(Vec::new());
    let pod_manifest = build_pod_manifest(&manifest)?;
    append_bytes(&mut builder, "pod.manifest", pod_manifest.as_bytes(), mtime)?;

    let files = collect_files(root)?;
    for path in files {
        let rel = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escapes pack root", path.display()))
            .and_then(normalize_rel)?;
        if rel == "pod.manifest" {
            continue;
        }
        let data = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        if data.len() as u64 > MAX_ENTRY_BYTES {
            return Err(format!(
                "{} exceeds MAX_ENTRY_BYTES ({MAX_ENTRY_BYTES})",
                path.display()
            ));
        }
        append_bytes(&mut builder, &rel, &data, mtime)?;
    }
    let tar_bytes = builder.into_inner().map_err(|e| e.to_string())?;
    if tar_bytes.len() as u64 > MAX_UNCOMPRESSED_BYTES {
        return Err("uncompressed tar exceeds MAX_UNCOMPRESSED_BYTES".into());
    }
    let compressed = encode_all(&tar_bytes[..], 3).map_err(|e| format!("zstd: {e}"))?;
    if compressed.len() < 4 || compressed[..4] != ZSTD_MAGIC {
        return Err("zstd encoder did not produce standard magic".into());
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(out, &compressed).map_err(|e| format!("write {}: {e}", out.display()))?;
    let hash = content_hash(&compressed);
    Ok(PackResult {
        archive: out.to_path_buf(),
        content_hash: hash,
        bytes: compressed.len() as u64,
    })
}

fn validate_member_path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.starts_with('/') || name.contains('\0') {
        return Err(format!("unsafe archive member `{name}`"));
    }
    let path = Path::new(name);
    for c in path.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => return Err(format!("refusing `..` in member `{name}`")),
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("refusing absolute member `{name}`"));
            }
        }
    }
    Ok(path.to_path_buf())
}

/// Safely extract a `.pod` into `dest` (must be empty or creatable).
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<ManifestInfo, String> {
    let compressed = fs::read(archive).map_err(|e| format!("read {}: {e}", archive.display()))?;
    if compressed.len() < 4 || compressed[..4] != ZSTD_MAGIC {
        return Err(format!(
            "{} is not a zstd archive (missing magic 0xFD2FB528)",
            archive.display()
        ));
    }
    let tar_bytes = decode_all(&compressed[..]).map_err(|e| format!("zstd decode: {e}"))?;
    if tar_bytes.len() as u64 > MAX_UNCOMPRESSED_BYTES {
        return Err("decoded tar exceeds MAX_UNCOMPRESSED_BYTES".into());
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(&tar_bytes[..]);
    let mut entries = 0usize;
    let mut first = true;
    let mut info: Option<ManifestInfo> = None;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        entries += 1;
        if entries > MAX_ENTRIES {
            return Err("archive exceeds MAX_ENTRIES".into());
        }
        let size = entry.size();
        if size > MAX_ENTRY_BYTES {
            return Err(format!("member size {size} exceeds MAX_ENTRY_BYTES"));
        }
        let path = entry.path().map_err(|e| e.to_string())?;
        let name = path.to_string_lossy().into_owned();
        if entry.header().entry_type().is_symlink() || entry.header().entry_type().is_hard_link() {
            return Err(format!("refusing link member `{name}`"));
        }
        let rel = validate_member_path(&name)?;
        if first {
            if name != "pod.manifest" {
                return Err(format!(
                    "first tar entry must be `pod.manifest`, got `{name}`"
                ));
            }
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| format!("read pod.manifest: {e}"))?;
            info = Some(parse_pod_manifest(&buf)?);
            let out = dest.join("pod.manifest");
            fs::write(&out, buf).map_err(|e| e.to_string())?;
            first = false;
            continue;
        }
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        let mut file = File::create(&out).map_err(|e| format!("create {}: {e}", out.display()))?;
        io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
    }
    if first {
        return Err("empty archive".into());
    }
    info.ok_or_else(|| "missing pod.manifest".into())
}

fn parse_pod_manifest(text: &str) -> Result<ManifestInfo, String> {
    let mut format = 0u32;
    let mut name = String::new();
    let mut version = String::new();
    let mut super_version = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        match k {
            "format" => format = v.parse().map_err(|_| "bad format")?,
            "name" => name = v.to_owned(),
            "version" => version = v.to_owned(),
            "super" => super_version = v.to_owned(),
            _ => {}
        }
    }
    if name.is_empty() || version.is_empty() {
        return Err("pod.manifest missing name/version".into());
    }
    Ok(ManifestInfo {
        format,
        name,
        version,
        super_version,
    })
}

/// Verify archive bytes match an expected `sha256:…` content hash.
pub fn verify_hash(archive: &Path, expected: &str) -> Result<(), String> {
    let bytes = fs::read(archive).map_err(|e| e.to_string())?;
    let got = content_hash(&bytes);
    if got != expected {
        return Err(format!(
            "content hash mismatch for {}: expected {expected}, got {got}",
            archive.display()
        ));
    }
    Ok(())
}

/// Write / update `content_hash` for the root package in `Pod.lock`.
pub fn write_lock_content_hash(manifest_path: &Path, hash: &str) -> Result<(), String> {
    let lock_path = pkg::lockfile_for(manifest_path);
    let manifest = pkg::parse_manifest(manifest_path)?;
    let id = manifest.identity()?;
    let mut lock = if lock_path.is_file() {
        let text = fs::read_to_string(&lock_path).map_err(|e| e.to_string())?;
        toml::from_str::<LockfileV1>(&text).unwrap_or_default()
    } else {
        LockfileV1::default()
    };
    if lock.version == 0 {
        lock.version = 1;
    }
    if let Some(pkg) = lock.packages.iter_mut().find(|p| p.name == id.name) {
        pkg.content_hash = Some(hash.to_owned());
        pkg.version = Some(id.version.clone());
    } else {
        lock.packages.push(LockedPackageV1 {
            name: id.name.clone(),
            source: "path".into(),
            commit: None,
            version: Some(id.version.clone()),
            features: Vec::new(),
            content_hash: Some(hash.to_owned()),
        });
    }
    let text = toml::to_string_pretty(&lock).map_err(|e| e.to_string())?;
    fs::write(&lock_path, text).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read expected content hash for `name` from Pod.lock, if present.
pub fn lock_content_hash(manifest_path: &Path, name: &str) -> Result<Option<String>, String> {
    let lock_path = pkg::lockfile_for(manifest_path);
    if !lock_path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&lock_path).map_err(|e| e.to_string())?;
    let lock: LockfileV1 = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(lock
        .packages
        .into_iter()
        .find(|p| p.name == name)
        .and_then(|p| p.content_hash))
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LockfileV1 {
    version: u32,
    #[serde(rename = "package", default)]
    packages: Vec<LockedPackageV1>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LockedPackageV1 {
    name: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn tiny_pod(dir: &Path) -> PathBuf {
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(
            dir.join("Pod.toml"),
            r#"[pod]
name = "tiny"
version = "0.1.0"
kind = "lib"
"#,
        )
        .unwrap();
        fs::write(
            dir.join("src/lib.sr"),
            r#"
+answer() : i64 => 42

test "answer" {
    assert_eq(answer(), 42)
}
"#,
        )
        .unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn pack_extract_round_trip_and_hash() {
        let dir = tempdir().unwrap();
        let root = tiny_pod(dir.path());
        let pod = dir.path().join("tiny.pod");
        let packed = pack_directory(&root, &pod).unwrap();
        assert!(packed.content_hash.starts_with("sha256:"));
        assert_eq!(packed.bytes, fs::metadata(&pod).unwrap().len());

        // First entry must be pod.manifest
        let compressed = fs::read(&pod).unwrap();
        assert_eq!(&compressed[..4], &ZSTD_MAGIC);
        let tar_bytes = decode_all(&compressed[..]).unwrap();
        let mut ar = tar::Archive::new(&tar_bytes[..]);
        let first = ar.entries().unwrap().next().unwrap().unwrap();
        assert_eq!(first.path().unwrap().to_string_lossy(), "pod.manifest");

        let dest = dir.path().join("out");
        let info = extract_archive(&pod, &dest).unwrap();
        assert_eq!(info.name, "tiny");
        assert_eq!(info.version, "0.1.0");
        assert!(dest.join("Pod.toml").is_file());
        assert!(dest.join("src/lib.sr").is_file());

        verify_hash(&pod, &packed.content_hash).unwrap();
        write_lock_content_hash(&root.join("Pod.toml"), &packed.content_hash).unwrap();
        let got = lock_content_hash(&root.join("Pod.toml"), "tiny")
            .unwrap()
            .unwrap();
        assert_eq!(got, packed.content_hash);
    }

    #[test]
    fn validate_member_path_rejects_traversal() {
        assert!(validate_member_path("../evil").is_err());
        assert!(validate_member_path("/abs").is_err());
        assert!(validate_member_path("ok/path.sr").is_ok());
    }

    #[test]
    fn pack_rejects_symlink_members() {
        let dir = tempdir().unwrap();
        let root = tiny_pod(dir.path());
        let link = root.join("sneaky.sr");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("src/lib.sr", &link).unwrap();
            let err = pack_directory(&root, &dir.path().join("x.pod")).unwrap_err();
            assert!(
                err.contains("symlink"),
                "expected symlink refusal, got {err}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = (root, link);
        }
    }

    #[test]
    fn rejects_non_zstd_magic() {
        let dir = tempdir().unwrap();
        let pod = dir.path().join("fake.pod");
        fs::write(&pod, b"NOTZ").unwrap();
        let err = extract_archive(&pod, &dir.path().join("x")).unwrap_err();
        assert!(err.contains("zstd"));
    }
}
