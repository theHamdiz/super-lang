use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use super_runtime::{
    BuildProfile, CodegenOpt, LtoMode, ProfileOverrides, ResolvedProfile, resolve_profile,
};

/// Canonical Pod manifest filename.
pub const MANIFEST_POD: &str = "Pod.toml";
/// Lockfile written beside `Pod.toml`.
pub const LOCKFILE_POD: &str = "Pod.lock";
/// Default local registry directory under the user home.
pub const DEFAULT_REGISTRY_DIR: &str = ".super/registry";

/// What a Pod builds and how the toolchain treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PodKind {
    /// Native or bytecode executable (`src/main.sr`).
    Bin,
    /// Reusable library (`src/lib.sr`).
    Lib,
    /// Distributable comptime macros (`src/macros.sr`).
    Macro,
}

impl PodKind {
    #[allow(dead_code)]
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "bin" | "exe" | "executable" | "app" => Ok(Self::Bin),
            "lib" | "library" => Ok(Self::Lib),
            "macro" | "macros" | "comptime" => Ok(Self::Macro),
            other => Err(format!(
                "unknown pod kind `{other}` (expected bin, lib, or macro)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Bin => "bin",
            Self::Lib => "lib",
            Self::Macro => "macro",
        }
    }

    pub fn entry_candidates(self) -> &'static [&'static str] {
        match self {
            // Prefer src/ layout; flat `main.sr` next to Pod.toml is also valid.
            Self::Bin => &["src/main.sr", "main.sr"],
            Self::Lib => &["src/lib.sr", "lib.sr"],
            Self::Macro => &["src/macros.sr", "src/lib.sr", "macros.sr", "lib.sr"],
        }
    }

    pub fn is_installable(self) -> bool {
        matches!(self, Self::Bin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Override the filesystem registry root (default `~/.super/registry`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Identity table for this Pod (`[pod]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod: Option<Package>,
    /// Multi-member workspace at this manifest root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// Local registry path override for this tree.
    #[serde(default, skip_serializing_if = "RegistryConfig::is_empty")]
    pub registry: RegistryConfig,
    /// Package-wide build settings. Absent on older manifests.
    #[serde(default, skip_serializing_if = "BuildConfig::is_empty")]
    pub build: BuildConfig,
    /// Per-profile patches layered on the built-in presets.
    #[serde(default, skip_serializing_if = "Profiles::is_empty")]
    pub profile: Profiles,
    /// Per-triple overrides: `[target.<triple>]`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub target: BTreeMap<String, TargetOverride>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Dependency>,
    /// Named compile-time feature graph. `default` is enabled unless suppressed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub features: BTreeMap<String, Vec<String>>,
    /// Formal verification (`#law` / `#proof` / `super verify`).
    #[serde(default, skip_serializing_if = "VerifyConfig::is_empty")]
    pub verify: VerifyConfig,
    /// `super lint` policy (`[lint]`, `[[lint.exception]]`).
    #[serde(default, skip_serializing_if = "LintSection::is_empty")]
    pub lint: LintSection,
}

/// `[lint]` in `Pod.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LintSection {
    /// Rules switched off (`S003` or `unused`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Rules promoted to errors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    /// Listed exceptions, each with a reason.
    #[serde(default, rename = "exception", skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<LintExceptionEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LintExceptionEntry {
    pub rule: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default)]
    pub reason: String,
}

impl LintSection {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty() && self.exceptions.is_empty()
    }

    pub fn to_config(&self) -> super_frontend::lint::LintConfig {
        super_frontend::lint::LintConfig {
            allow: self.allow.clone(),
            deny: self.deny.clone(),
            exceptions: self
                .exceptions
                .iter()
                .map(|e| super_frontend::lint::Exception {
                    rule: e.rule.clone(),
                    path: e.path.clone(),
                    item: e.item.clone(),
                    reason: e.reason.clone(),
                })
                .collect(),
        }
    }
}

impl Manifest {
    pub fn resolve_features(
        &self,
        requested: &[String],
        all: bool,
        no_default: bool,
    ) -> Result<BTreeSet<String>, String> {
        let mut pending = Vec::new();
        if all {
            pending.extend(
                self.features
                    .keys()
                    .filter(|name| name.as_str() != "default")
                    .cloned(),
            );
        } else {
            if !no_default {
                pending.extend(self.features.get("default").cloned().unwrap_or_default());
            }
            pending.extend(requested.iter().cloned());
        }
        let mut enabled = BTreeSet::new();
        while let Some(feature) = pending.pop() {
            if feature == "default" {
                pending.extend(self.features.get("default").cloned().unwrap_or_default());
                continue;
            }
            if !self.features.contains_key(&feature) {
                return Err(format!("unknown Pod feature `{feature}`"));
            }
            if enabled.insert(feature.clone()) {
                pending.extend(self.features.get(&feature).cloned().unwrap_or_default());
            }
        }
        Ok(enabled)
    }
}

/// `[verify]` in `Pod.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyConfig {
    /// Explicit law source globs/paths (`*.law.sr`). Empty → auto-discover.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub laws: Vec<String>,
    /// Explicit proof source globs/paths (`*.proof.sr`). Empty → auto-discover.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<String>,
    /// When true, `super build --release` / `super install` run `super verify` first.
    #[serde(
        default,
        rename = "require-on-release",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub require_on_release: bool,
    /// Default solver for `super verify` when CLI omits `--solver`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver: Option<String>,
}

impl VerifyConfig {
    pub fn is_empty(&self) -> bool {
        self.laws.is_empty()
            && self.proofs.is_empty()
            && !self.require_on_release
            && self.solver.is_none()
    }
}

impl RegistryConfig {
    fn is_empty(&self) -> bool {
        self.path.is_none()
    }
}

impl Manifest {
    /// Requires a `[pod]` table.
    pub fn identity(&self) -> Result<&Package, String> {
        self.pod
            .as_ref()
            .ok_or_else(|| "manifest has no [pod] table".into())
    }

    /// Declared kind, or inferred from entry files next to the manifest.
    pub fn kind_or_infer(&self, manifest_path: &Path) -> PodKind {
        if let Ok(id) = self.identity()
            && let Some(kind) = id.kind
        {
            return kind;
        }
        infer_pod_kind(manifest_path.parent().unwrap_or_else(|| Path::new(".")))
    }
}

/// Infer kind from source layout when `kind` is omitted.
pub fn infer_pod_kind(root: &Path) -> PodKind {
    let src = root.join("src");
    if src.join("main.sr").is_file() || root.join("main.sr").is_file() {
        PodKind::Bin
    } else if src.join("macros.sr").is_file() || root.join("macros.sr").is_file() {
        PodKind::Macro
    } else if src.join("lib.sr").is_file() || root.join("lib.sr").is_file() {
        PodKind::Lib
    } else {
        PodKind::Bin
    }
}

/// `[build]` in `Pod.toml`. Omitted keys keep the built-in preset.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Code generator: `c23` (default), `jit`, or `vm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// `aot` (default native binary), `jit`, or `bytecode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Single compile destination (`native`, `wasm32-wasi`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Multi-target build list. When set, `super build` emits every triple.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lto: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip: Option<bool>,
    #[serde(
        default,
        rename = "debug-info",
        skip_serializing_if = "Option::is_none"
    )]
    pub debug_info: Option<bool>,
    #[serde(default, rename = "fast-math", skip_serializing_if = "Option::is_none")]
    pub fast_math: Option<bool>,
    #[serde(default, rename = "static", skip_serializing_if = "Option::is_none")]
    pub static_link: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
    #[serde(
        default,
        rename = "target-cpu",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_cpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c23: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
    #[serde(
        default,
        rename = "gc-sections",
        skip_serializing_if = "Option::is_none"
    )]
    pub gc_sections: Option<bool>,
    #[serde(
        default,
        rename = "link-arg",
        alias = "link-args",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub link_args: Vec<String>,
    /// Per-translation-unit C emission (`incremental`) or one `.c` file (`single`).
    #[serde(default, rename = "codegen-units")]
    pub codegen_units: CodegenUnits,
    /// Keep generated `.c` / `.o` artifacts after a successful link.
    #[serde(default, rename = "keep-c")]
    pub keep_c: bool,
    /// Max C statements per lowered function before automatic outlining (`0` disables).
    /// Defaults to 1500 — keeps GCC/Clang away from oversized-function cliffs.
    #[serde(
        default = "default_outline_stmt_threshold",
        rename = "outline-stmt-threshold",
        skip_serializing_if = "is_default_outline_stmt_threshold"
    )]
    pub outline_stmt_threshold: usize,
}

fn default_outline_stmt_threshold() -> usize {
    super_runtime::DEFAULT_OUTLINE_STMT_THRESHOLD
}

fn is_default_outline_stmt_threshold(v: &usize) -> bool {
    *v == default_outline_stmt_threshold()
}

/// How `super build` splits emitted C into translation units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum CodegenUnits {
    /// Shared runtime + per-function `.c` → `.o`, content-hash incremental compile.
    #[default]
    Incremental,
    /// Legacy single self-contained `.c` (unchanged behavior).
    Single,
}

impl CodegenUnits {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_lowercase().as_str() {
            "incremental" | "inc" => Ok(Self::Incremental),
            "single" | "one" | "1" => Ok(Self::Single),
            other => Err(format!(
                "unknown codegen-units `{other}` (expected incremental, single)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Single => "single",
        }
    }
}

/// `[target.<triple>]` overrides layered on `[build]` for one destination.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std: Option<String>,
    #[serde(
        default,
        rename = "target-cpu",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_cpu: Option<String>,
    #[serde(default, rename = "static", skip_serializing_if = "Option::is_none")]
    pub static_link: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
    #[serde(
        default,
        rename = "link-arg",
        alias = "link-args",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub link_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<String>,
}

impl BuildConfig {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Code generator named by `[build] backend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileBackend {
    /// Portable C23 ahead-of-time stack. The default.
    C23,
    /// Native image. A full program is still emitted through the C23 stack.
    Jit,
    /// Bytecode interpreter. Only valid with `mode = "bytecode"`.
    Vm,
}

impl CompileBackend {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "c23" | "c" | "aot" => Ok(Self::C23),
            "jit" => Ok(Self::Jit),
            "vm" | "bytecode" | "interp" => Ok(Self::Vm),
            other => Err(format!("unknown backend `{other}` (expected c23, jit, vm)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::C23 => "c23",
            Self::Jit => "jit",
            Self::Vm => "vm",
        }
    }
}

/// How `super build` produces the artifact. `[build] mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileMode {
    /// Link a native executable. The default.
    Aot,
    /// Link a native image and treat it as the JIT cache.
    Jit,
    /// Write a portable `.superb` module and do not link.
    Bytecode,
}

impl CompileMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "aot" => Ok(Self::Aot),
            "jit" => Ok(Self::Jit),
            "bytecode" | "vm" | "interp" => Ok(Self::Bytecode),
            other => Err(format!(
                "unknown mode `{other}` (expected aot, jit, bytecode)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Aot => "aot",
            Self::Jit => "jit",
            Self::Bytecode => "bytecode",
        }
    }

    /// `aot` and `jit` both leave an executable. `bytecode` does not.
    pub fn native(self) -> bool {
        !matches!(self, Self::Bytecode)
    }
}

/// One `[profile.<name>]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lto: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strip: Option<bool>,
    #[serde(
        default,
        rename = "debug-info",
        skip_serializing_if = "Option::is_none"
    )]
    pub debug_info: Option<bool>,
    #[serde(default, rename = "fast-math", skip_serializing_if = "Option::is_none")]
    pub fast_math: Option<bool>,
    #[serde(default, rename = "static", skip_serializing_if = "Option::is_none")]
    pub static_link: Option<bool>,
    /// Optional destination triple for this profile only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(
        default,
        rename = "target-cpu",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_cpu: Option<String>,
    #[serde(
        default,
        rename = "gc-sections",
        skip_serializing_if = "Option::is_none"
    )]
    pub gc_sections: Option<bool>,
    #[serde(
        default,
        rename = "link-arg",
        alias = "link-args",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub link_args: Vec<String>,
}

/// `[profile.dev]`, `[profile.test]`, `[profile.release]`, `[profile.size]`, `[profile.fast]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<ProfileTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test: Option<ProfileTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<ProfileTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<ProfileTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast: Option<ProfileTable>,
}

impl Profiles {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    fn for_profile(&self, profile: BuildProfile) -> Option<&ProfileTable> {
        match profile {
            BuildProfile::Dev => self.dev.as_ref(),
            BuildProfile::Test => self.test.as_ref(),
            BuildProfile::Release => self.release.as_ref(),
            BuildProfile::Size => self.size.as_ref(),
            BuildProfile::Fast => self.fast.as_ref(),
        }
    }
}

/// CLI knobs that were actually passed. `None` means "not on the command line",
/// so `Pod.toml` is allowed to fill that slot.
#[derive(Clone, Debug, Default)]
pub struct CliBuildChoices<'a> {
    pub release: bool,
    pub debug: bool,
    pub profile: Option<&'a str>,
    pub opt: Option<&'a str>,
    pub lto: Option<&'a str>,
    pub strip: Option<bool>,
    pub debug_info: Option<bool>,
    pub fast_math: Option<bool>,
    /// `None` → profile preset (GNU targets stay dynamic; static is explicit).
    pub static_link: Option<bool>,
    pub target_cpu: Option<&'a str>,
    pub link_args: &'a [String],
    pub shared: bool,
    pub c23: bool,
    pub std: Option<&'a str>,
    pub cc: Option<&'a str>,
    pub out: Option<&'a Path>,
    pub backend: Option<&'a str>,
    pub mode: Option<&'a str>,
    /// Explicit `--target` from the CLI. Overrides `[build] target` / `targets`.
    pub target: Option<&'a str>,
    /// `--c`, `--c23`, or `--cc`: link a native binary even when the manifest says bytecode.
    pub force_native: bool,
    pub codegen_units: Option<CodegenUnits>,
    pub keep_c: Option<bool>,
    pub outline_stmt_threshold: Option<usize>,
    pub features: &'a [String],
    pub all_features: bool,
    pub no_default_features: bool,
}

/// Profile plus the package-level compile settings after CLI and `Pod.toml` merge.
#[derive(Clone, Debug)]
pub struct AppliedBuild {
    pub profile: ResolvedProfile,
    pub shared: bool,
    pub c23: bool,
    pub std: String,
    pub cc: Option<String>,
    pub out: Option<PathBuf>,
    pub backend: CompileBackend,
    pub mode: CompileMode,
    /// Primary destination for this applied build.
    pub target: super_runtime::CompileTarget,
    /// Every destination this build should emit (CLI single-target → one entry).
    pub targets: Vec<super_runtime::CompileTarget>,
    pub codegen_units: CodegenUnits,
    pub keep_c: bool,
    pub outline_stmt_threshold: usize,
    pub features: BTreeSet<String>,
}

/// Manifest for a Pod of the given kind.
pub fn default_manifest_kind(name: &str, kind: PodKind) -> String {
    let kind_name = kind.name();
    format!(
        r#"[pod]
name = {name:?}
version = "0.1.0"
kind = {kind_name:?}

# Used by `super build` and `super run` when -d / -r / --profile is omitted.
# `super test` always uses [profile.test].
# backend is the code generator: c23 (default), jit, or vm.
# mode is aot (native binary, the default), jit, or bytecode.
# aot through the c23 stack is what `super build` writes.
# Pod kinds: bin (executable), lib (library), macro (distributable comptime).
[build]
profile = "dev"
backend = "c23"
mode = "aot"
# target = "native"                 # host (default)
# targets = ["native", "wasm32-wasi"]  # multi-target AOT
# See `super targets` for the full catalog (linux/windows/macos/android/wasi/…).

[profile.dev]
opt = "0"
lto = "off"
debug-info = true
fast-math = false

[profile.test]
opt = "2"
lto = "off"
debug-info = true
fast-math = false
target-cpu = "native"

[profile.release]
opt = "3"
lto = "fat"
strip = true
fast-math = true
target-cpu = "native"
# Opt-in static CRT (`--static`). Default release stays dynamic on glibc.
# Prefer `--target *-linux-musl --static` for a fully static binary.
# static = true

[profile.size]
opt = "s"
lto = "fat"
strip = true
gc-sections = true

[profile.fast]
opt = "3"
lto = "fat"
strip = true
fast-math = true
target-cpu = "native"

# Formal verification (`super verify` / `super v`).
# laws / proofs: optional explicit paths; empty → auto-discover `*.law.sr` / `*.proof.sr`.
# require-on-release: gate `super build --release` and `super install` on a green verify.
[verify]
# laws = ["src/*.law.sr", "laws/"]
# proofs = ["src/*.proof.sr"]
require-on-release = false
# solver = "auto"   # auto | smt | comptime | fuzz

[features]
default = []

[dependencies]
"#
    )
}

/// Entry source for a Pod: respects `kind`, then falls back to layout heuristics.
pub fn package_entry(manifest_path: &Path) -> Option<PathBuf> {
    let root = manifest_path.parent()?;
    let kind = parse_manifest(manifest_path)
        .ok()
        .map(|m| m.kind_or_infer(manifest_path))
        .unwrap_or_else(|| infer_pod_kind(root));
    for rel in kind.entry_candidates() {
        let path = root.join(rel);
        if path.is_file() {
            return Some(path);
        }
    }
    for rel in [
        "src/main.sr",
        "src/macros.sr",
        "src/lib.sr",
        "main.sr",
        "macros.sr",
        "lib.sr",
    ] {
        let path = root.join(rel);
        if path.is_file() {
            return Some(path);
        }
    }
    // Prefer a single file under src/; else a single .sr beside Pod.toml.
    let mut sources = fs::read_dir(root.join("src"))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sr"))
        .collect::<Vec<_>>();
    if sources.is_empty() {
        sources = fs::read_dir(root)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("sr")
            })
            .collect();
    }
    sources.sort();
    if sources.len() == 1 {
        sources.pop()
    } else {
        None
    }
}

/// Executable file name for the host. Prefer [`CompileTarget::artifact_name`].
pub fn executable_name(name: &str) -> String {
    super_runtime::CompileTarget::host().artifact_name(name, false)
}

/// Default build output for a package: `target/<profile>/<name>` on the host,
/// or `target/<triple>/<profile>/<name>` when `target` is cross / wasm.
///
/// Native binaries are the executable (`name.exe` on Windows, `name.wasm` on
/// WebAssembly). Bytecode artifacts use `.superb` (tests stripped) or
/// `.superb-dev`. `None` when `file` is not in a package.
pub fn default_artifact(
    file: &Path,
    profile: &str,
    native: bool,
    strip_tests: bool,
) -> Option<PathBuf> {
    default_artifact_for(
        file,
        profile,
        native,
        strip_tests,
        &super_runtime::CompileTarget::host(),
    )
}

pub fn default_artifact_for(
    file: &Path,
    profile: &str,
    native: bool,
    strip_tests: bool,
    target: &super_runtime::CompileTarget,
) -> Option<PathBuf> {
    let manifest_path = find_manifest(file)?;
    let root = manifest_path.parent()?;
    let manifest = parse_manifest(&manifest_path).ok()?;
    let name = manifest.identity().ok()?.name.clone();
    let file_name = if native {
        target.artifact_name(&name, false)
    } else if strip_tests {
        format!("{name}.superb")
    } else {
        format!("{name}.superb-dev")
    };
    Some(target.artifact_dir(root, profile).join(file_name))
}

/// Resolve the destination list from CLI + manifest.
pub fn resolve_targets(
    cli_target: Option<&str>,
    build: &BuildConfig,
    profile_target: Option<&str>,
) -> Result<Vec<super_runtime::CompileTarget>, String> {
    if let Some(raw) = cli_target {
        return Ok(vec![super_runtime::CompileTarget::parse(raw)?]);
    }
    if !build.targets.is_empty() {
        let mut out = Vec::with_capacity(build.targets.len());
        for raw in &build.targets {
            out.push(super_runtime::CompileTarget::parse(raw)?);
        }
        return Ok(out);
    }
    if let Some(raw) = build.target.as_deref().or(profile_target) {
        return Ok(vec![super_runtime::CompileTarget::parse(raw)?]);
    }
    Ok(vec![super_runtime::CompileTarget::host()])
}

pub fn apply_cli(cli: CliBuildChoices<'_>) -> Result<AppliedBuild, String> {
    let (backend, mode) = resolve_codegen(&cli, &BuildConfig::default())?;
    let targets = resolve_targets(cli.target, &BuildConfig::default(), None)?;
    let target = targets[0].clone();
    let mut profile = resolve_cli(&cli)?;
    if cli.shared {
        profile.static_link = false;
    }
    Ok(AppliedBuild {
        profile,
        shared: cli.shared,
        c23: cli.c23 || backend == CompileBackend::C23,
        std: cli.std.unwrap_or("c23").to_string(),
        cc: cli.cc.map(str::to_string),
        out: cli.out.map(Path::to_path_buf),
        backend,
        mode,
        target,
        targets,
        codegen_units: cli.codegen_units.unwrap_or_default(),
        keep_c: cli.keep_c.unwrap_or(false),
        outline_stmt_threshold: cli
            .outline_stmt_threshold
            .unwrap_or_else(default_outline_stmt_threshold),
        features: cli.features.iter().cloned().collect(),
    })
}

/// Built-in preset, then `[profile.<name>]`, then `[build]`, then explicit CLI flags.
pub fn apply_manifest(
    manifest: &Manifest,
    manifest_dir: &Path,
    cli: CliBuildChoices<'_>,
) -> Result<AppliedBuild, String> {
    let profile_name = if cli.release || cli.debug || cli.profile.is_some() {
        cli.profile
    } else {
        manifest.build.profile.as_deref()
    };
    let mut profile = resolve_profile(ProfileOverrides {
        profile: profile_name,
        release: cli.release,
        debug: cli.debug,
        opt: cli.opt,
        lto: cli.lto,
        strip: cli.strip,
        debug_info: cli.debug_info,
        fast_math: cli.fast_math,
        static_link: cli.static_link,
        target_cpu: cli.target_cpu,
        link_args: cli.link_args,
    })?;
    let table = manifest.profile.for_profile(profile.profile);
    patch_profile(&mut profile, table, &manifest.build, &cli)?;
    let (backend, mode) = resolve_codegen(&cli, &manifest.build)?;
    let out = match cli.out {
        Some(path) => Some(path.to_path_buf()),
        None => manifest.build.out.as_ref().map(|raw| {
            let path = PathBuf::from(raw);
            if path.is_absolute() {
                path
            } else {
                manifest_dir.join(path)
            }
        }),
    };
    let kind = match manifest.identity() {
        Ok(id) => id.kind.unwrap_or_else(|| infer_pod_kind(manifest_dir)),
        Err(_) => infer_pod_kind(manifest_dir),
    };
    // Lib and macro Pods have no process entry; emit without a host `main`.
    let library_like = !kind.is_installable();
    let profile_target = table.and_then(|table| table.target.as_deref());
    let targets = resolve_targets(cli.target, &manifest.build, profile_target)?;
    let target = targets[0].clone();
    let override_table = lookup_target_override(manifest, &target);
    let mut shared = cli.shared || manifest.build.shared.unwrap_or(false) || library_like;
    let mut std = cli
        .std
        .or(manifest.build.std.as_deref())
        .unwrap_or("c23")
        .to_string();
    let mut cc = cli
        .cc
        .map(str::to_string)
        .or_else(|| manifest.build.cc.clone());
    let mut out = out;
    if let Some(over) = override_table {
        if let Some(value) = over.shared {
            shared = value || library_like;
        }
        if cli.std.is_none()
            && let Some(value) = over.std.as_deref()
        {
            std = value.to_string();
        }
        if cli.cc.is_none()
            && let Some(value) = over.cc.clone()
        {
            cc = Some(value);
        }
        if cli.out.is_none()
            && let Some(raw) = over.out.as_ref()
        {
            let path = PathBuf::from(raw);
            out = Some(if path.is_absolute() {
                path
            } else {
                manifest_dir.join(path)
            });
        }
        if !over.link_args.is_empty() {
            profile.link_args.extend(over.link_args.iter().cloned());
        }
        if cli.target_cpu.is_none()
            && let Some(cpu) = over.target_cpu.as_deref()
        {
            profile.target_cpu = if cpu.is_empty() || cpu == "generic" {
                None
            } else {
                Some(cpu.to_string())
            };
        }
        if cli.static_link.is_none()
            && let Some(value) = over.static_link
        {
            profile.static_link = value;
        }
    }
    if !target.allows_native_cpu() && profile.target_cpu.as_deref() == Some("native") {
        profile.target_cpu = None;
    }
    if shared {
        profile.static_link = false;
    }
    let codegen_units = cli.codegen_units.unwrap_or(manifest.build.codegen_units);
    let keep_c = cli.keep_c.unwrap_or(manifest.build.keep_c);
    let outline_stmt_threshold = cli
        .outline_stmt_threshold
        .unwrap_or(manifest.build.outline_stmt_threshold);
    let features =
        manifest.resolve_features(cli.features, cli.all_features, cli.no_default_features)?;
    Ok(AppliedBuild {
        profile,
        shared,
        c23: cli.c23 || manifest.build.c23.unwrap_or(false) || backend == CompileBackend::C23,
        std,
        cc,
        out,
        backend,
        mode,
        target,
        targets,
        codegen_units,
        keep_c,
        outline_stmt_threshold,
        features,
    })
}

fn lookup_target_override<'a>(
    manifest: &'a Manifest,
    target: &super_runtime::CompileTarget,
) -> Option<&'a TargetOverride> {
    manifest.target.get(&target.triple).or_else(|| {
        manifest.target.iter().find_map(|(key, value)| {
            match super_runtime::CompileTarget::parse(key) {
                Ok(parsed) if parsed.triple == target.triple => Some(value),
                _ => None,
            }
        })
    })
}

/// Re-bind an applied build onto a different destination (multi-target loop).
pub fn apply_for_target(
    mut applied: AppliedBuild,
    manifest: &Manifest,
    manifest_dir: &Path,
    target: super_runtime::CompileTarget,
) -> Result<AppliedBuild, String> {
    applied.target = target.clone();
    applied.targets = vec![target.clone()];
    if let Some(over) = lookup_target_override(manifest, &target) {
        if let Some(value) = over.cc.clone() {
            applied.cc = Some(value);
        }
        if let Some(value) = over.std.clone() {
            applied.std = value;
        }
        if let Some(value) = over.shared {
            applied.shared = value;
        }
        if let Some(cpu) = over.target_cpu.as_deref() {
            applied.profile.target_cpu = if cpu.is_empty() || cpu == "generic" {
                None
            } else {
                Some(cpu.to_string())
            };
        }
        if let Some(value) = over.static_link {
            applied.profile.static_link = value;
        }
        applied
            .profile
            .link_args
            .extend(over.link_args.iter().cloned());
        if let Some(raw) = over.out.as_ref() {
            let path = PathBuf::from(raw);
            applied.out = Some(if path.is_absolute() {
                path
            } else {
                manifest_dir.join(path)
            });
        }
    }
    if !target.allows_native_cpu() && applied.profile.target_cpu.as_deref() == Some("native") {
        applied.profile.target_cpu = None;
    }
    Ok(applied)
}

/// Manifest keys, then CLI overrides. Missing keys are `c23` and `aot`.
fn resolve_codegen(
    cli: &CliBuildChoices<'_>,
    build: &BuildConfig,
) -> Result<(CompileBackend, CompileMode), String> {
    if cli.force_native
        && cli
            .mode
            .is_some_and(|mode| CompileMode::parse(mode).ok() == Some(CompileMode::Bytecode))
    {
        return Err("`--c` / `--c23` builds a native binary; drop `--mode bytecode`".into());
    }
    let mut backend =
        CompileBackend::parse(cli.backend.or(build.backend.as_deref()).unwrap_or("c23"))?;
    let mut mode = CompileMode::parse(cli.mode.or(build.mode.as_deref()).unwrap_or("aot"))?;
    if cli.force_native && cli.mode.is_none() {
        mode = CompileMode::Aot;
    }
    if cli.force_native && cli.backend.is_none() {
        backend = CompileBackend::C23;
    }
    if mode.native() && backend == CompileBackend::Vm {
        return Err(format!(
            "mode `{}` compiles through the c23 stack; set backend = \"c23\" or mode = \"bytecode\"",
            mode.name()
        ));
    }
    Ok((backend, mode))
}

fn resolve_cli(cli: &CliBuildChoices<'_>) -> Result<ResolvedProfile, String> {
    resolve_profile(ProfileOverrides {
        profile: cli.profile,
        release: cli.release,
        debug: cli.debug,
        opt: cli.opt,
        lto: cli.lto,
        strip: cli.strip,
        debug_info: cli.debug_info,
        fast_math: cli.fast_math,
        static_link: cli.static_link,
        target_cpu: cli.target_cpu,
        link_args: cli.link_args,
    })
}

fn patch_profile(
    resolved: &mut ResolvedProfile,
    table: Option<&ProfileTable>,
    build: &BuildConfig,
    cli: &CliBuildChoices<'_>,
) -> Result<(), String> {
    if cli.opt.is_none()
        && let Some(raw) = table
            .and_then(|table| table.opt.as_deref())
            .or(build.opt.as_deref())
    {
        resolved.opt = CodegenOpt::parse(raw)?;
    }
    if cli.lto.is_none()
        && let Some(raw) = table
            .and_then(|table| table.lto.as_deref())
            .or(build.lto.as_deref())
    {
        resolved.lto = LtoMode::parse(raw)?;
    }
    if cli.strip.is_none()
        && let Some(value) = table.and_then(|table| table.strip).or(build.strip)
    {
        resolved.strip = value;
    }
    if cli.debug_info.is_none()
        && let Some(value) = table
            .and_then(|table| table.debug_info)
            .or(build.debug_info)
    {
        resolved.debug_info = value;
    }
    if cli.fast_math.is_none()
        && let Some(value) = table.and_then(|table| table.fast_math).or(build.fast_math)
    {
        resolved.fast_math = value;
    }
    if cli.static_link.is_none()
        && let Some(value) = table
            .and_then(|table| table.static_link)
            .or(build.static_link)
    {
        resolved.static_link = value;
    }
    if cli.target_cpu.is_none()
        && let Some(cpu) = table
            .and_then(|table| table.target_cpu.as_deref())
            .or(build.target_cpu.as_deref())
    {
        resolved.target_cpu = if cpu.is_empty() || cpu == "generic" {
            None
        } else {
            Some(cpu.to_string())
        };
    }
    if let Some(value) = table
        .and_then(|table| table.gc_sections)
        .or(build.gc_sections)
    {
        resolved.gc_sections = value;
    }
    let mut link_args = Vec::new();
    link_args.extend(build.link_args.iter().cloned());
    if let Some(table) = table {
        link_args.extend(table.link_args.iter().cloned());
    }
    link_args.extend(cli.link_args.iter().cloned());
    resolved.link_args = link_args;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// `bin`, `lib`, or `macro`. Omitted manifests infer from `src/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PodKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Path {
        path: String,
    },
    Git {
        git: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rev: Option<String>,
    },
    Registry {
        version: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
    },
    Version(String),
}

impl Dependency {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Dependency::Git {
                tag, branch, rev, ..
            } => validate_git_pin(tag.as_deref(), branch.as_deref(), rev.as_deref()),
            _ => Ok(()),
        }
    }

    pub fn registry_features(&self) -> &[String] {
        match self {
            Dependency::Registry { features, .. } => features,
            _ => &[],
        }
    }

    pub fn is_optional(&self) -> bool {
        matches!(self, Dependency::Registry { optional: true, .. })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Lockfile {
    version: u32,
    #[serde(rename = "package", default)]
    packages: Vec<LockedPackage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LockedPackage {
    name: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    features: Vec<String>,
    /// D3: SHA-256 of the published `.pod` archive (`sha256:…`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryIndex {
    #[serde(default)]
    versions: BTreeMap<String, RegistryVersionMeta>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RegistryVersionMeta {
    published: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSpec {
    pub url: String,
    pub reference: Option<String>,
}

pub fn parse_manifest(path: &Path) -> Result<Manifest, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// Walk upward until a directory contains `Pod.toml`.
pub fn find_manifest(from: &Path) -> Option<PathBuf> {
    let mut dir = if from.is_dir() {
        from.to_path_buf()
    } else {
        from.parent()?.to_path_buf()
    };
    loop {
        let pod = dir.join(MANIFEST_POD);
        if pod.is_file() {
            return Some(pod);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Lockfile path beside a manifest (`Pod.lock`).
pub fn lockfile_for(manifest_path: &Path) -> PathBuf {
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    root.join(LOCKFILE_POD)
}

/// Child Pod manifest under a dependency root.
pub fn child_manifest(dep_root: &Path) -> Option<PathBuf> {
    let pod = dep_root.join(MANIFEST_POD);
    pod.is_file().then_some(pod)
}

/// Every manifest to fetch or resolve: the root Pod and each `[workspace]` member.
pub fn workspace_manifests(from: &Path) -> Result<Vec<PathBuf>, String> {
    let manifest_path = find_manifest(from)
        .ok_or_else(|| format!("could not find {MANIFEST_POD} from {}", from.display()))?;
    let manifest = parse_manifest(&manifest_path)?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| format!("{} has no parent", manifest_path.display()))?;
    let mut paths = Vec::new();
    if manifest.identity().is_ok() {
        paths.push(manifest_path.clone());
    }
    if let Some(workspace) = &manifest.workspace {
        for member in &workspace.members {
            let member_root = root.join(member);
            let member_manifest = child_manifest(&member_root)
                .ok_or_else(|| format!("workspace member `{member}` has no {MANIFEST_POD}"))?;
            let member_file = parse_manifest(&member_manifest)?;
            member_file
                .identity()
                .map_err(|_| format!("workspace member `{member}` has no [pod] table"))?;
            paths.push(member_manifest);
        }
    }
    if paths.is_empty() {
        return Err(format!(
            "{} has neither [pod] nor [workspace] members",
            manifest_path.display()
        ));
    }
    Ok(paths)
}

/// Resolve dependencies for every manifest in the workspace.
pub fn resolve_workspace(from: &Path, update: bool) -> Result<Vec<(String, PathBuf)>, String> {
    let manifests = workspace_manifests(from)?;
    let mut merged = Vec::new();
    let mut seen = HashSet::new();
    for manifest in manifests {
        for row in resolve(&manifest, update)? {
            if seen.insert(row.0.clone()) {
                merged.push(row);
            }
        }
    }
    Ok(merged)
}

fn validate_git_pin(
    tag: Option<&str>,
    branch: Option<&str>,
    rev: Option<&str>,
) -> Result<(), String> {
    let count = [tag, branch, rev]
        .into_iter()
        .filter(|slot| slot.is_some_and(|s| !s.is_empty()))
        .count();
    if count > 1 {
        return Err("git dependency must set exactly one of tag, branch, or rev".into());
    }
    Ok(())
}

fn looks_like_git_url(value: &str) -> bool {
    let value = value.trim();
    value.contains("://")
        || value.starts_with("git@")
        || value.ends_with(".git")
        || value.contains('/')
}

fn normalize_version_req(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with(['^', '~', '=', '*'])
        || trimmed.contains(',')
        || trimmed.split('.').count() > 3
    {
        trimmed.to_owned()
    } else if trimmed.split('.').count() == 2 {
        format!("^{trimmed}")
    } else {
        format!("={trimmed}")
    }
}

fn version_matches(requirement: &str, candidate: &str) -> Result<bool, String> {
    let req = normalize_version_req(requirement);
    if req.is_empty() {
        return Err("empty version requirement".into());
    }
    let parsed_req = semver::VersionReq::parse(&req)
        .map_err(|error| format!("invalid version requirement `{requirement}`: {error}"))?;
    let parsed_ver = semver::Version::parse(candidate)
        .map_err(|error| format!("invalid published version `{candidate}`: {error}"))?;
    Ok(parsed_req.matches(&parsed_ver))
}

pub fn registry_dir(manifest: Option<&Manifest>) -> Result<PathBuf, String> {
    if let Some(manifest) = manifest
        && let Some(path) = manifest.registry.path.as_deref()
    {
        let path = PathBuf::from(path);
        return Ok(if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join(path)
        });
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set; cannot locate registry".to_owned())?;
    Ok(home.join(DEFAULT_REGISTRY_DIR))
}

fn registry_index_path(registry: &Path, name: &str) -> PathBuf {
    registry.join(name).join("index.toml")
}

fn read_registry_index(registry: &Path, name: &str) -> Result<RegistryIndex, String> {
    let path = registry_index_path(registry, name);
    if !path.is_file() {
        return Ok(RegistryIndex::default());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn write_registry_index(registry: &Path, name: &str, index: &RegistryIndex) -> Result<(), String> {
    let path = registry_index_path(registry, name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = toml::to_string_pretty(index).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn select_registry_version(index: &RegistryIndex, requirement: &str) -> Result<String, String> {
    if index.versions.is_empty() {
        return Err("no published versions".into());
    }
    let mut matches: Vec<_> = index
        .versions
        .keys()
        .filter(|version| version_matches(requirement, version).unwrap_or(false))
        .cloned()
        .collect();
    if matches.is_empty() {
        return Err(format!("no version satisfies `{requirement}`"));
    }
    matches.sort_by(|left, right| {
        semver::Version::parse(right)
            .ok()
            .cmp(&semver::Version::parse(left).ok())
    });
    Ok(matches[0].clone())
}

fn registry_version_root(registry: &Path, name: &str, version: &str) -> PathBuf {
    registry.join(name).join(version)
}

/// Copy the current Pod into the local filesystem registry.
pub fn publish(manifest_path: &Path) -> Result<String, String> {
    let manifest = parse_manifest(manifest_path)?;
    let id = manifest.identity()?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| format!("{} has no parent", manifest_path.display()))?;
    let registry = registry_dir(Some(&manifest))?;
    fs::create_dir_all(&registry).map_err(|e| e.to_string())?;
    let dest = registry_version_root(&registry, &id.name, &id.version);
    if dest.exists() {
        return Err(format!(
            "{} {} is already published at {}",
            id.name,
            id.version,
            dest.display()
        ));
    }
    copy_pod_tree(root, &dest)?;
    let mut index = read_registry_index(&registry, &id.name)?;
    index.versions.insert(
        id.version.clone(),
        RegistryVersionMeta {
            published: chrono_lite_timestamp(),
        },
    );
    write_registry_index(&registry, &id.name, &index)?;
    Ok(format!(
        "published {} {} → {}",
        id.name,
        id.version,
        dest.display()
    ))
}

fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

pub fn parse_git_shorthand(value: &str) -> Option<GitSpec> {
    let (host, rest) = value.split_once(':')?;
    let base = match host {
        "github" => "https://github.com",
        "gitlab" => "https://gitlab.com",
        _ => return None,
    };
    let (repo, reference) = rest
        .rsplit_once('@')
        .map_or((rest, None), |(repo, tag)| (repo, Some(tag.to_owned())));
    if repo.split('/').count() != 2 || repo.is_empty() {
        return None;
    }
    Some(GitSpec {
        url: format!("{base}/{repo}.git"),
        reference,
    })
}

pub fn add(manifest_path: &Path, package: &str) -> Result<String, String> {
    let mut manifest = parse_manifest(manifest_path)?;
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let candidate = root.join(package);
    let (name, dependency) = if candidate.exists() || Path::new(package).exists() {
        let path = if candidate.exists() {
            candidate
        } else {
            PathBuf::from(package)
        };
        let dep_manifest_path = child_manifest(&path)
            .ok_or_else(|| format!("{} has no {MANIFEST_POD}", path.display()))?;
        let dep_manifest = parse_manifest(&dep_manifest_path)?;
        (
            dep_manifest.identity()?.name.clone(),
            Dependency::Path {
                path: package.to_owned(),
            },
        )
    } else {
        let spec = parse_git_shorthand(package).unwrap_or_else(|| GitSpec {
            url: package.to_owned(),
            reference: None,
        });
        let name = repository_name(&spec.url)?;
        (
            name,
            Dependency::Git {
                git: spec.url,
                tag: spec.reference,
                branch: None,
                rev: None,
            },
        )
    };
    dependency.validate()?;
    manifest.dependencies.insert(name.clone(), dependency);
    write_manifest(manifest_path, &manifest)?;
    Ok(name)
}

pub fn remove(manifest_path: &Path, package: &str) -> Result<(), String> {
    let mut manifest = parse_manifest(manifest_path)?;
    if manifest.dependencies.remove(package).is_none() {
        return Err(format!("dependency `{package}` is not present"));
    }
    write_manifest(manifest_path, &manifest)
}

fn write_manifest(path: &Path, manifest: &Manifest) -> Result<(), String> {
    manifest.identity()?;
    for dependency in manifest.dependencies.values() {
        dependency.validate()?;
    }
    let text = toml::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))
}

fn repository_name(url: &str) -> Result<String, String> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|s| s.trim_end_matches(".git").to_owned())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("cannot derive package name from `{url}`"))
}

pub fn resolve(manifest_path: &Path, update: bool) -> Result<Vec<(String, PathBuf)>, String> {
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let manifest = parse_manifest(manifest_path)?;
    let registry = registry_dir(Some(&manifest))?;
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut paths = Vec::new();
    let mut locked = BTreeMap::new();
    let lock_path = lockfile_for(manifest_path);
    let lock_rows = read_lock_packages(&lock_path);
    resolve_manifest(
        manifest_path,
        root,
        &registry,
        update,
        &lock_rows,
        &mut visiting,
        &mut visited,
        &mut paths,
        &mut locked,
    )?;
    let lockfile = Lockfile {
        version: 1,
        packages: locked.into_values().collect(),
    };
    let text = toml::to_string_pretty(&lockfile).map_err(|e| e.to_string())?;
    fs::write(&lock_path, text).map_err(|e| e.to_string())?;
    Ok(paths)
}

fn read_lock_packages(path: &Path) -> HashMap<String, LockedPackage> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<Lockfile>(&text).ok())
        .map(|lock| {
            lock.packages
                .into_iter()
                .map(|package| (package.name.clone(), package))
                .collect()
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn resolve_manifest(
    manifest_path: &Path,
    project_root: &Path,
    registry: &Path,
    update: bool,
    lock_rows: &HashMap<String, LockedPackage>,
    visiting: &mut HashSet<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    paths: &mut Vec<(String, PathBuf)>,
    locked: &mut BTreeMap<String, LockedPackage>,
) -> Result<(), String> {
    let identity = manifest_path
        .canonicalize()
        .unwrap_or_else(|_| manifest_path.to_path_buf());
    if !visiting.insert(identity.clone()) {
        return Err(format!(
            "dependency cycle detected at {}",
            manifest_path.display()
        ));
    }
    if visited.contains(&identity) {
        visiting.remove(&identity);
        return Ok(());
    }
    let manifest = parse_manifest(manifest_path)?;
    let base = manifest_path.parent().unwrap_or(project_root);
    for (name, dependency) in manifest.dependencies {
        dependency.validate()?;
        if dependency.is_optional() {
            continue;
        }
        let features = dependency.registry_features().to_vec();
        let (dep_root, source, commit, version) = match dependency {
            Dependency::Path { path } => {
                let root = base.join(&path);
                let canonical = root.canonicalize().map_err(|e| {
                    format!(
                        "resolve path dependency `{name}` at {}: {e}",
                        root.display()
                    )
                })?;
                (canonical, format!("path+{path}"), None, None)
            }
            Dependency::Git {
                git,
                tag,
                branch,
                rev,
            } => {
                let locked_rev = (!update)
                    .then(|| lock_rows.get(&name))
                    .flatten()
                    .and_then(|row| row.commit.as_deref());
                let root = checkout_git(
                    &git,
                    tag.as_deref(),
                    branch.as_deref(),
                    rev.as_deref(),
                    locked_rev,
                    update,
                )?;
                let sha = git_output(&root, &["rev-parse", "HEAD"])?;
                (root, format!("git+{git}"), Some(sha), None)
            }
            Dependency::Version(ref requirement) => {
                if looks_like_git_url(requirement) {
                    let root = checkout_git(requirement, None, None, None, None, update)?;
                    let sha = git_output(&root, &["rev-parse", "HEAD"])?;
                    (root, format!("git+{requirement}"), Some(sha), None)
                } else {
                    let index = read_registry_index(registry, &name)?;
                    let picked = if !update {
                        match lock_rows
                            .get(&name)
                            .and_then(|row| row.version.clone())
                            .filter(|version| {
                                version_matches(requirement, version).unwrap_or(false)
                            }) {
                            Some(version) => version,
                            None => select_registry_version(&index, requirement)?,
                        }
                    } else {
                        select_registry_version(&index, requirement)?
                    };
                    let root = registry_version_root(registry, &name, &picked);
                    if !root.is_dir() {
                        return Err(format!(
                            "registry package `{name}` {picked} is missing at {}",
                            root.display()
                        ));
                    }
                    (
                        root,
                        format!("registry+{name}@{picked}"),
                        None,
                        Some(picked),
                    )
                }
            }
            Dependency::Registry {
                version: requirement,
                ..
            } => {
                let index = read_registry_index(registry, &name)?;
                let picked = if !update {
                    match lock_rows
                        .get(&name)
                        .and_then(|row| row.version.clone())
                        .filter(|version| version_matches(&requirement, version).unwrap_or(false))
                    {
                        Some(version) => version,
                        None => select_registry_version(&index, &requirement)?,
                    }
                } else {
                    select_registry_version(&index, &requirement)?
                };
                let root = registry_version_root(registry, &name, &picked);
                if !root.is_dir() {
                    return Err(format!(
                        "registry package `{name}` {picked} is missing at {}",
                        root.display()
                    ));
                }
                (
                    root,
                    format!("registry+{name}@{picked}"),
                    None,
                    Some(picked),
                )
            }
        };
        let source_root = dep_root.join("src");
        let search_root = if source_root.is_dir() {
            source_root
        } else {
            dep_root.clone()
        };
        paths.push((name.clone(), search_root));
        locked.insert(
            name.clone(),
            LockedPackage {
                name,
                source,
                commit,
                version,
                features,
                content_hash: None,
            },
        );
        if let Some(child) = child_manifest(&dep_root) {
            resolve_manifest(
                &child,
                project_root,
                registry,
                update,
                lock_rows,
                visiting,
                visited,
                paths,
                locked,
            )?;
        }
    }
    visiting.remove(&identity);
    visited.insert(identity);
    Ok(())
}

fn checkout_git(
    url: &str,
    tag: Option<&str>,
    branch: Option<&str>,
    rev: Option<&str>,
    locked_rev: Option<&str>,
    update: bool,
) -> Result<PathBuf, String> {
    let cache = cache_root()?;
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    let root = cache.join(format!("{:016x}", hasher.finish()));
    if !root.join(".git").is_dir() {
        let mut args = vec!["clone", "--depth", "1"];
        if let Some(reference) = tag.or(branch) {
            args.extend(["--branch", reference]);
        }
        args.push(url);
        let target = root.to_string_lossy().into_owned();
        args.push(&target);
        run_git(None, &args)?;
    } else if update {
        run_git(Some(&root), &["fetch", "--depth", "1", "origin"])?;
    }
    if let Some(locked_rev) = locked_rev {
        if run_git(Some(&root), &["checkout", "--detach", locked_rev]).is_err() {
            run_git(
                Some(&root),
                &["fetch", "--depth", "1", "origin", locked_rev],
            )?;
            run_git(Some(&root), &["checkout", "--detach", "FETCH_HEAD"])?;
        }
    } else if let Some(branch) = branch {
        run_git(Some(&root), &["fetch", "--depth", "1", "origin", branch])?;
        run_git(Some(&root), &["checkout", "--detach", "FETCH_HEAD"])?;
    } else if let Some(reference) = rev.or(tag) {
        run_git(Some(&root), &["fetch", "--depth", "1", "origin", reference])?;
        run_git(Some(&root), &["checkout", "--detach", "FETCH_HEAD"])?;
    } else if update {
        run_git(Some(&root), &["checkout", "--detach", "origin/HEAD"])?;
    }
    Ok(root)
}

fn cache_root() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|p| p.join(".super/cache/git"))
        .ok_or_else(|| "HOME is not set; cannot locate package cache".to_owned())
}

fn run_git(cwd: Option<&Path>, args: &[&str]) -> Result<(), String> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|e| format!("run git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}

pub fn vendor(manifest_path: &Path) -> Result<usize, String> {
    let paths = resolve(manifest_path, false)?;
    let vendor = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("vendor");
    fs::create_dir_all(&vendor).map_err(|e| e.to_string())?;
    for (package_name, source) in &paths {
        let root = if source.file_name().and_then(|name| name.to_str()) == Some("src") {
            source.parent().unwrap_or(source)
        } else {
            source
        };
        let name = child_manifest(root)
            .and_then(|path| parse_manifest(&path).ok())
            .and_then(|m| m.identity().ok().map(|id| id.name.clone()))
            .unwrap_or_else(|| package_name.clone());
        copy_tree(root, &vendor.join(name))?;
    }
    Ok(paths.len())
}

fn skip_pod_artifact(name: &str) -> bool {
    matches!(name, ".git" | "target" | "vendor")
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    copy_tree_inner(source, target, false)
}

fn copy_pod_tree(source: &Path, target: &Path) -> Result<(), String> {
    copy_tree_inner(source, target, true)
}

fn copy_tree_inner(source: &Path, target: &Path, pod_publish: bool) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if pod_publish && skip_pod_artifact(&name) {
            continue;
        }
        if !pod_publish && name == ".git" {
            continue;
        }
        let destination = target.join(entry.file_name());
        if path.is_dir() {
            copy_tree_inner(&path, &destination, pod_publish)?;
        } else {
            fs::copy(path, destination).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest() {
        let manifest: Manifest = toml::from_str(
            "[pod]\nname='app'\nversion='0.1.0'\n[dependencies]\nutils={path='../utils'}\nrouter={git='https://example/router',tag='v1'}",
        )
        .unwrap();
        assert_eq!(manifest.identity().unwrap().name, "app");
        assert_eq!(manifest.dependencies.len(), 2);
    }

    #[test]
    fn parses_pod_manifest_with_kind() {
        let manifest: Manifest =
            toml::from_str("[pod]\nname='macros'\nversion='0.1.0'\nkind='macro'\n[dependencies]\n")
                .unwrap();
        assert_eq!(manifest.identity().unwrap().name, "macros");
        assert_eq!(manifest.identity().unwrap().kind, Some(PodKind::Macro));
        assert_eq!(PodKind::parse("macros").unwrap(), PodKind::Macro);
    }

    #[test]
    fn parses_workspace_and_registry_config() {
        let manifest: Manifest = toml::from_str(
            "[workspace]\nmembers = [\"app\", \"lib\"]\n[registry]\npath = \"/tmp/registry\"\n",
        )
        .unwrap();
        assert_eq!(
            manifest.workspace.as_ref().unwrap().members,
            vec!["app", "lib"]
        );
        assert_eq!(manifest.registry.path.as_deref(), Some("/tmp/registry"));
    }

    #[test]
    fn parses_version_and_feature_dependencies() {
        let manifest: Manifest = toml::from_str(
            "[pod]\nname='app'\nversion='0.1.0'\n[dependencies]\nutils = \"^1.2\"\nfeat = { version = \"1.0.0\", features = [\"serde\"], optional = true }\n",
        )
        .unwrap();
        assert!(matches!(
            manifest.dependencies.get("utils"),
            Some(Dependency::Version(version)) if version == "^1.2"
        ));
        assert!(matches!(
            manifest.dependencies.get("feat"),
            Some(Dependency::Registry { optional: true, .. })
        ));
    }

    #[test]
    fn git_dependency_rejects_multiple_pins() {
        let dependency = Dependency::Git {
            git: "https://example.com/r.git".into(),
            tag: Some("v1".into()),
            branch: Some("main".into()),
            rev: None,
        };
        assert!(dependency.validate().is_err());
    }

    #[test]
    fn parses_github_shorthand() {
        assert_eq!(
            parse_git_shorthand("github:user/repo@v1"),
            Some(GitSpec {
                url: "https://github.com/user/repo.git".into(),
                reference: Some("v1".into())
            })
        );
    }

    #[test]
    fn identity_requires_pod_table() {
        let manifest: Manifest = toml::from_str("[workspace]\nmembers = [\"app\"]\n").unwrap();
        assert!(manifest.identity().is_err());
    }

    #[test]
    fn codegen_units_default_incremental_and_single_override() {
        let manifest: Manifest = toml::from_str("[pod]\nname='app'\nversion='0.1.0'\n").unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.codegen_units, CodegenUnits::Incremental);
        assert!(!applied.keep_c);

        let mut single = CliBuildChoices::default();
        single.codegen_units = Some(CodegenUnits::Single);
        single.keep_c = Some(true);
        let applied = apply_manifest(&manifest, Path::new("."), single).unwrap();
        assert_eq!(applied.codegen_units, CodegenUnits::Single);
        assert!(applied.keep_c);
    }

    #[test]
    fn default_manifest_selects_dev_and_release_is_peak() {
        let manifest: Manifest =
            toml::from_str(&default_manifest_kind("hello", PodKind::Bin)).unwrap();
        assert_eq!(manifest.identity().unwrap().kind, Some(PodKind::Bin));
        let dev = apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(dev.profile.profile, BuildProfile::Dev);
        assert_eq!(dev.profile.opt, CodegenOpt::O0);
        assert!(dev.profile.debug_info);
        assert!(!dev.profile.strip_tests);
        assert_eq!(dev.backend, CompileBackend::C23);
        assert_eq!(dev.mode, CompileMode::Aot);
        assert!(dev.mode.native());

        let mut release_cli = CliBuildChoices::default();
        release_cli.release = true;
        let release = apply_manifest(&manifest, Path::new("."), release_cli).unwrap();
        assert_eq!(release.profile.profile, BuildProfile::Release);
        assert_eq!(release.profile.lto, LtoMode::Fat);
        assert!(release.profile.max_speed);
        assert!(release.profile.fast_math);
        assert!(release.profile.strip_tests);
        assert!(!release.profile.static_link);
        assert_eq!(release.profile.target_cpu.as_deref(), Some("native"));

        let mut test_cli = CliBuildChoices::default();
        test_cli.profile = Some("test");
        let test_profile = apply_manifest(&manifest, Path::new("."), test_cli).unwrap();
        assert_eq!(test_profile.profile.profile, BuildProfile::Test);
        assert_eq!(test_profile.profile.opt, CodegenOpt::O2);
        assert!(test_profile.profile.debug_info);
        assert!(!test_profile.profile.fast_math);
        assert!(!test_profile.profile.strip_tests);
        assert!(!test_profile.profile.max_speed);

        let text = toml::to_string_pretty(&manifest).unwrap();
        let again: Manifest = toml::from_str(&text).unwrap();
        assert_eq!(again.build.profile.as_deref(), Some("dev"));
        assert_eq!(again.build.backend.as_deref(), Some("c23"));
        assert_eq!(again.build.mode.as_deref(), Some("aot"));
        assert!(again.profile.test.is_some());
        assert_eq!(
            again
                .profile
                .release
                .as_ref()
                .and_then(|table| table.lto.as_deref()),
            Some("fat")
        );
    }

    #[test]
    fn cli_flags_override_super_toml() {
        let manifest: Manifest = toml::from_str(
            "[pod]\nname='app'\nversion='0.1.0'\n[build]\nprofile='release'\nopt='z'\n[profile.release]\nopt='s'\nlto='thin'\n",
        )
        .unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.profile.profile, BuildProfile::Release);
        assert_eq!(applied.profile.opt, CodegenOpt::Os);
        assert_eq!(applied.profile.lto, LtoMode::Thin);
        assert!(applied.profile.max_speed);

        let mut debug = CliBuildChoices::default();
        debug.debug = true;
        debug.opt = Some("1");
        let applied = apply_manifest(&manifest, Path::new("."), debug).unwrap();
        assert_eq!(applied.profile.profile, BuildProfile::Dev);
        assert_eq!(applied.profile.opt, CodegenOpt::O1);
        assert!(!applied.profile.strip_tests);
    }

    #[test]
    fn backend_and_mode_default_to_c23_aot() {
        let manifest: Manifest = toml::from_str("[pod]\nname='app'\nversion='0.1.0'\n").unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.backend, CompileBackend::C23);
        assert_eq!(applied.mode, CompileMode::Aot);

        let manifest: Manifest = toml::from_str(
            "[pod]\nname='app'\nversion='0.1.0'\n[build]\nbackend='vm'\nmode='aot'\n",
        )
        .unwrap();
        let err =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap_err();
        assert!(err.contains("c23"), "{err}");

        let manifest: Manifest = toml::from_str(
            "[pod]\nname='app'\nversion='0.1.0'\n[build]\nbackend='c23'\nmode='bytecode'\n",
        )
        .unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.mode, CompileMode::Bytecode);
        assert!(!applied.mode.native());

        let mut cli = CliBuildChoices::default();
        cli.mode = Some("jit");
        let applied = apply_manifest(&manifest, Path::new("."), cli).unwrap();
        assert_eq!(applied.mode, CompileMode::Jit);
        assert_eq!(applied.backend, CompileBackend::C23);
        assert!(applied.mode.native());
    }

    #[test]
    fn build_target_and_targets_resolve_from_manifest_and_cli() {
        let manifest: Manifest = toml::from_str(
            r#"[pod]
name = "app"
version = "0.1.0"
[build]
target = "wasm32-wasi"
"#,
        )
        .unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.target.triple, "wasm32-wasi");
        assert!(applied.target.is_wasm());
        assert_eq!(applied.targets.len(), 1);

        let multi: Manifest = toml::from_str(
            r#"[pod]
name = "app"
version = "0.1.0"
[build]
targets = ["native", "wasm32-wasi", "aarch64-unknown-linux-gnu"]
"#,
        )
        .unwrap();
        let applied = apply_manifest(&multi, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.targets.len(), 3);
        assert!(applied.targets[1].is_wasm());
        assert_eq!(applied.targets[2].triple, "aarch64-unknown-linux-gnu");

        let mut cli = CliBuildChoices::default();
        cli.target = Some("x86_64-pc-windows-gnu");
        let applied = apply_manifest(&multi, Path::new("."), cli).unwrap();
        assert_eq!(applied.targets.len(), 1);
        assert_eq!(applied.target.triple, "x86_64-pc-windows-gnu");
        assert!(applied.profile.target_cpu.is_none());
    }

    #[test]
    fn target_override_table_applies_cc_and_link_args() {
        let manifest: Manifest = toml::from_str(
            r#"[pod]
name = "app"
version = "0.1.0"
[build]
target = "wasm32-wasi"
[target.wasm32-wasi]
cc = "zig"
link-arg = ["--export-all"]
"#,
        )
        .unwrap();
        let applied =
            apply_manifest(&manifest, Path::new("."), CliBuildChoices::default()).unwrap();
        assert_eq!(applied.cc.as_deref(), Some("zig"));
        assert!(
            applied
                .profile
                .link_args
                .iter()
                .any(|arg| arg == "--export-all")
        );
    }

    #[test]
    fn package_entry_is_next_to_the_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        let manifest = root.join(MANIFEST_POD);
        fs::write(&manifest, default_manifest_kind("app", PodKind::Bin)).unwrap();
        assert!(package_entry(&manifest).is_none());
        fs::write(root.join("src/lib.sr"), "+answer() => 1\n").unwrap();
        assert_eq!(package_entry(&manifest).unwrap(), root.join("src/lib.sr"));
        fs::write(root.join("src/main.sr"), "+main() {}\n").unwrap();
        assert_eq!(package_entry(&manifest).unwrap(), root.join("src/main.sr"));
    }

    #[test]
    fn package_entry_accepts_the_only_source_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        let manifest = root.join(MANIFEST_POD);
        fs::write(&manifest, default_manifest_kind("physics", PodKind::Bin)).unwrap();
        fs::write(root.join("src/physics.sr"), "+step(dt: f64) {}\n").unwrap();
        assert_eq!(
            package_entry(&manifest).unwrap(),
            root.join("src/physics.sr")
        );
    }

    #[test]
    fn package_entry_accepts_flat_main_beside_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let manifest = root.join(MANIFEST_POD);
        fs::write(&manifest, default_manifest_kind("doc_pod", PodKind::Bin)).unwrap();
        fs::write(root.join("main.sr"), "+main() { print(\"ok\") }\n").unwrap();
        assert_eq!(package_entry(&manifest).unwrap(), root.join("main.sr"));
        assert_eq!(infer_pod_kind(root), PodKind::Bin);
    }

    #[test]
    fn package_entry_prefers_macros_for_macro_kind() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        let manifest = root.join(MANIFEST_POD);
        fs::write(&manifest, default_manifest_kind("m", PodKind::Macro)).unwrap();
        fs::write(root.join("src/lib.sr"), "+answer() => 1\n").unwrap();
        fs::write(root.join("src/macros.sr"), "+#WIDTH = 8\n").unwrap();
        assert_eq!(
            package_entry(&manifest).unwrap(),
            root.join("src/macros.sr")
        );
    }

    #[test]
    fn detects_cycles() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(
            a.join(MANIFEST_POD),
            "[pod]\nname='a'\nversion='0.1.0'\n[dependencies]\nb={path='../b'}\n",
        )
        .unwrap();
        fs::write(
            b.join(MANIFEST_POD),
            "[pod]\nname='b'\nversion='0.1.0'\n[dependencies]\na={path='../a'}\n",
        )
        .unwrap();
        assert!(resolve(&a.join(MANIFEST_POD), false).is_err());
    }

    #[test]
    fn workspace_manifests_lists_members() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("app")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        fs::write(
            root.join(MANIFEST_POD),
            "[workspace]\nmembers = [\"app\", \"lib\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("app").join(MANIFEST_POD),
            "[pod]\nname='app'\nversion='0.1.0'\n[dependencies]\n",
        )
        .unwrap();
        fs::write(
            root.join("lib").join(MANIFEST_POD),
            "[pod]\nname='lib'\nversion='0.1.0'\n[dependencies]\n",
        )
        .unwrap();
        let paths = workspace_manifests(root).unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn publish_then_fetch_registry_dependency() {
        let temp = tempfile::tempdir().unwrap();
        let registry = temp.path().join("registry");
        let registry_path = registry.display().to_string();
        let lib = temp.path().join("lib");
        let app = temp.path().join("app");
        fs::create_dir_all(lib.join("src")).unwrap();
        fs::create_dir_all(app.join("src")).unwrap();
        fs::write(
            lib.join(MANIFEST_POD),
            format!(
                "[pod]\nname='utils'\nversion='1.2.3'\nkind='lib'\n[registry]\npath = {:?}\n[dependencies]\n",
                registry_path
            ),
        )
        .unwrap();
        fs::write(lib.join("src/lib.sr"), "+answer() : i64 => 42\n").unwrap();
        publish(&lib.join(MANIFEST_POD)).unwrap();
        fs::write(
            app.join(MANIFEST_POD),
            format!(
                "[pod]\nname='app'\nversion='0.1.0'\nkind='bin'\n[registry]\npath = {:?}\n[dependencies]\nutils = \"^1.2\"\n",
                registry_path
            ),
        )
        .unwrap();
        fs::write(
            app.join("src/main.sr"),
            "use utils\n+main() {\n    print(answer())\n}\n",
        )
        .unwrap();
        let rows = resolve(&app.join(MANIFEST_POD), false).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "utils");
        let lock_text = fs::read_to_string(lockfile_for(&app.join(MANIFEST_POD))).unwrap();
        assert!(lock_text.contains("1.2.3"), "{lock_text}");
        assert!(lock_text.contains("registry+utils@1.2.3"), "{lock_text}");
    }

    #[test]
    fn semver_short_requirement_matches_patch_releases() {
        assert!(version_matches("1.2", "1.2.9").unwrap());
        assert!(version_matches("1.2", "1.3.0").unwrap());
        assert!(!version_matches("=1.2.0", "1.3.0").unwrap());
        assert!(version_matches("^1", "1.9.0").unwrap());
    }

    #[test]
    fn serialized_manifest_preserves_workspace() {
        let manifest: Manifest = toml::from_str(
            "[workspace]\nmembers = [\"app\"]\n[pod]\nname='root'\nversion='0.0.1'\n",
        )
        .unwrap();
        let text = toml::to_string_pretty(&manifest).unwrap();
        let again: Manifest = toml::from_str(&text).unwrap();
        assert_eq!(again.workspace.as_ref().unwrap().members, vec!["app"]);
    }

    #[test]
    fn resolves_default_transitive_all_and_no_default_features() {
        let manifest: Manifest = toml::from_str(
            r#"
[pod]
name = "features"
version = "0.1.0"
[features]
default = ["http"]
http = ["tls"]
tls = []
uring = []
"#,
        )
        .unwrap();
        let defaults = manifest.resolve_features(&[], false, false).unwrap();
        assert_eq!(
            defaults,
            BTreeSet::from(["http".to_string(), "tls".to_string()])
        );
        assert!(
            manifest
                .resolve_features(&[], false, true)
                .unwrap()
                .is_empty()
        );
        let all = manifest.resolve_features(&[], true, true).unwrap();
        assert_eq!(
            all,
            BTreeSet::from(["http".to_string(), "tls".to_string(), "uring".to_string()])
        );
        assert!(
            manifest
                .resolve_features(&["missing".into()], false, true)
                .is_err()
        );
    }
}
