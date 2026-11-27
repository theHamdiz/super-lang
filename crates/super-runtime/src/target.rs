//! Compile targets: OS/arch triples, including WebAssembly.
//!
//! Super accepts canonical target triples in `Pod.toml` / `Super.toml` and on the
//! CLI (`--target`). The C23 back end maps each triple onto a host compiler
//! invocation (`zig cc -target …` or `clang --target=…`). Host builds keep the
//! historical `cc` path and `target/<profile>/` layout; an explicit target
//! writes under `target/<triple>/<profile>/`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// CPU architecture named by a Super target triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Arch {
    X86_64,
    X86,
    Aarch64,
    Arm,
    Riscv64,
    Riscv32,
    PowerPc64,
    PowerPc64Le,
    LoongArch64,
    S390x,
    Mips64,
    Mips64El,
    Wasm32,
    Wasm64,
}

impl Arch {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "x86_64" | "amd64" => Self::X86_64,
            "i686" | "i586" | "i386" | "x86" => Self::X86,
            "aarch64" | "arm64" => Self::Aarch64,
            "arm" | "armv7" | "armv7l" | "thumbv7neon" => Self::Arm,
            "riscv64" | "riscv64gc" => Self::Riscv64,
            "riscv32" | "riscv32imac" | "riscv32imc" => Self::Riscv32,
            "powerpc64" | "ppc64" => Self::PowerPc64,
            "powerpc64le" | "ppc64le" => Self::PowerPc64Le,
            "loongarch64" => Self::LoongArch64,
            "s390x" => Self::S390x,
            "mips64" => Self::Mips64,
            "mips64el" => Self::Mips64El,
            "wasm32" => Self::Wasm32,
            "wasm64" => Self::Wasm64,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::X86 => "i686",
            Self::Aarch64 => "aarch64",
            Self::Arm => "arm",
            Self::Riscv64 => "riscv64",
            Self::Riscv32 => "riscv32",
            Self::PowerPc64 => "powerpc64",
            Self::PowerPc64Le => "powerpc64le",
            Self::LoongArch64 => "loongarch64",
            Self::S390x => "s390x",
            Self::Mips64 => "mips64",
            Self::Mips64El => "mips64el",
            Self::Wasm32 => "wasm32",
            Self::Wasm64 => "wasm64",
        }
    }

    pub const fn is_wasm(self) -> bool {
        matches!(self, Self::Wasm32 | Self::Wasm64)
    }
}

/// Operating system / runtime named by a Super target triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Linux,
    Windows,
    Macos,
    FreeBsd,
    NetBsd,
    OpenBsd,
    Android,
    Ios,
    Wasi,
    Unknown,
    None,
}

impl Os {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "linux" => Self::Linux,
            "windows" => Self::Windows,
            "darwin" | "macos" => Self::Macos,
            "freebsd" => Self::FreeBsd,
            "netbsd" => Self::NetBsd,
            "openbsd" => Self::OpenBsd,
            "android" => Self::Android,
            "ios" => Self::Ios,
            "wasi" | "wasip1" | "wasip2" => Self::Wasi,
            "unknown" => Self::Unknown,
            "none" | "freestanding" => Self::None,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Macos => "darwin",
            Self::FreeBsd => "freebsd",
            Self::NetBsd => "netbsd",
            Self::OpenBsd => "openbsd",
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Wasi => "wasi",
            Self::Unknown => "unknown",
            Self::None => "none",
        }
    }

    pub const fn is_wasm_runtime(self) -> bool {
        matches!(self, Self::Wasi | Self::Unknown | Self::None)
    }
}

/// ABI / environment suffix (`gnu`, `static CRT`, `msvc`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Env {
    Gnu,
    GnuEabi,
    GnuEabihf,
    Musl,
    MuslEabi,
    MuslEabihf,
    Msvc,
    Uclibc,
    Android,
    Macabi,
    None,
}

impl Env {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "gnu" => Self::Gnu,
            "gnueabi" => Self::GnuEabi,
            "gnueabihf" => Self::GnuEabihf,
            "musl" => Self::Musl,
            "musleabi" => Self::MuslEabi,
            "musleabihf" => Self::MuslEabihf,
            "msvc" => Self::Msvc,
            "uclibc" => Self::Uclibc,
            "android" | "androideabi" => Self::Android,
            "macabi" => Self::Macabi,
            "none" | "" => Self::None,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gnu => "gnu",
            Self::GnuEabi => "gnueabi",
            Self::GnuEabihf => "gnueabihf",
            Self::Musl => "musl",
            Self::MuslEabi => "musleabi",
            Self::MuslEabihf => "musleabihf",
            Self::Msvc => "msvc",
            Self::Uclibc => "uclibc",
            Self::Android => "android",
            Self::Macabi => "macabi",
            Self::None => "none",
        }
    }
}

/// Vendor token in a four-part triple (`unknown`, `pc`, `apple`, …).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Vendor {
    Unknown,
    Pc,
    Apple,
    Ibm,
    SoftFloat,
    Uwp,
}

impl Vendor {
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "unknown" => Self::Unknown,
            "pc" => Self::Pc,
            "apple" => Self::Apple,
            "ibm" => Self::Ibm,
            "softfloat" => Self::SoftFloat,
            "uwp" => Self::Uwp,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Pc => "pc",
            Self::Apple => "apple",
            Self::Ibm => "ibm",
            Self::SoftFloat => "softfloat",
            Self::Uwp => "uwp",
        }
    }
}

/// Fully resolved compile destination.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CompileTarget {
    pub arch: Arch,
    pub vendor: Vendor,
    pub os: Os,
    pub env: Env,
    /// Canonical Super target-triple spelling.
    pub triple: String,
}

impl CompileTarget {
    /// Host this `super` binary was built for.
    pub fn host() -> Self {
        let arch = Arch::parse(std::env::consts::ARCH).unwrap_or(Arch::X86_64);
        let (vendor, os, env) = match std::env::consts::OS {
            "linux" => (Vendor::Unknown, Os::Linux, Env::Gnu),
            "windows" => (Vendor::Pc, Os::Windows, Env::Gnu),
            "macos" => (Vendor::Apple, Os::Macos, Env::None),
            "freebsd" => (Vendor::Unknown, Os::FreeBsd, Env::None),
            "netbsd" => (Vendor::Unknown, Os::NetBsd, Env::None),
            "openbsd" => (Vendor::Unknown, Os::OpenBsd, Env::None),
            "android" => (Vendor::Unknown, Os::Android, Env::Android),
            "ios" => (Vendor::Apple, Os::Ios, Env::None),
            _ => (Vendor::Unknown, Os::Unknown, Env::None),
        };
        Self::from_parts(arch, vendor, os, env)
    }

    pub fn from_parts(arch: Arch, vendor: Vendor, os: Os, env: Env) -> Self {
        let triple = format_triple(arch, vendor, os, env);
        Self {
            arch,
            vendor,
            os,
            env,
            triple,
        }
    }

    /// Parse a full `arch-vendor-os-abi` or short `arch-os-abi` triple. Aliases such as `wasm32-wasi`
    /// and `native` / `host` are accepted.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("empty target triple".into());
        }
        let lower = trimmed.to_ascii_lowercase();
        if matches!(lower.as_str(), "native" | "host" | "default") {
            return Ok(Self::host());
        }
        if let Some(known) = known_by_alias(&lower) {
            return Ok(known);
        }
        parse_triple(&lower)
    }

    pub fn is_host(&self) -> bool {
        self == &Self::host()
    }

    pub fn is_wasm(&self) -> bool {
        self.arch.is_wasm()
    }

    pub fn is_cross(&self) -> bool {
        !self.is_host()
    }

    /// OpenMP is a host-native optimization; wasm and freestanding drop it.
    pub fn supports_openmp(&self) -> bool {
        !self.is_wasm() && !matches!(self.os, Os::None | Os::Wasi | Os::Ios)
    }

    /// `-march=native` is only valid when compiling for this machine.
    pub fn allows_native_cpu(&self) -> bool {
        self.is_host()
    }

    /// Executable / module file name for a Pod named `name`.
    pub fn artifact_name(&self, name: &str, shared: bool) -> String {
        if shared {
            return match self.os {
                Os::Windows => format!("{name}.dll"),
                Os::Macos | Os::Ios => format!("lib{name}.dylib"),
                Os::Wasi | Os::Unknown | Os::None if self.is_wasm() => format!("{name}.wasm"),
                _ => format!("lib{name}.so"),
            };
        }
        if self.is_wasm() {
            return format!("{name}.wasm");
        }
        if self.os == Os::Windows {
            if name.to_ascii_lowercase().ends_with(".exe") {
                name.to_string()
            } else {
                format!("{name}.exe")
            }
        } else {
            name.to_string()
        }
    }

    /// Layout directory for this target under a Pod's `target/` root.
    ///
    /// Host keeps `target/<profile>/`. Every other triple nests as
    /// `target/<triple>/<profile>/`.
    pub fn artifact_dir(&self, root: &Path, profile: &str) -> PathBuf {
        if self.is_host() {
            root.join("target").join(profile)
        } else {
            root.join("target").join(&self.triple).join(profile)
        }
    }

    /// Clang `--target=` spelling.
    pub fn clang_triple(&self) -> String {
        match (self.arch, self.os, self.env) {
            (Arch::Wasm32, Os::Wasi, _) => "wasm32-wasip1".into(),
            (Arch::Wasm64, Os::Wasi, _) => "wasm64-wasip1".into(),
            (Arch::Wasm32, Os::Unknown | Os::None, _) => "wasm32-unknown-unknown".into(),
            (Arch::Wasm64, Os::Unknown | Os::None, _) => "wasm64-unknown-unknown".into(),
            (Arch::Aarch64, Os::Macos, _) => "arm64-apple-macos".into(),
            (Arch::X86_64, Os::Macos, _) => "x86_64-apple-macos".into(),
            _ => self.triple.clone(),
        }
    }

    /// peer `-target` spelling.
    pub fn zig_triple(&self) -> String {
        match (self.arch, self.os, self.env) {
            (Arch::Wasm32, Os::Wasi, _) => "wasm32-wasi".into(),
            (Arch::Wasm64, Os::Wasi, _) => "wasm64-wasi".into(),
            (Arch::Wasm32, Os::Unknown | Os::None, _) => "wasm32-freestanding".into(),
            (Arch::Wasm64, Os::Unknown | Os::None, _) => "wasm64-freestanding".into(),
            (arch, Os::Linux, Env::Gnu | Env::GnuEabi | Env::GnuEabihf) => {
                format!("{}-linux-gnu", arch.as_str())
            }
            (arch, Os::Linux, Env::Musl | Env::MuslEabi | Env::MuslEabihf) => {
                format!("{}-linux-musl", arch.as_str())
            }
            (arch, Os::Windows, Env::Gnu) => format!("{}-windows-gnu", arch.as_str()),
            (arch, Os::Windows, Env::Msvc) => format!("{}-windows-msvc", arch.as_str()),
            (Arch::Aarch64, Os::Macos, _) => "aarch64-macos".into(),
            (Arch::X86_64, Os::Macos, _) => "x86_64-macos".into(),
            (arch, Os::FreeBsd, _) => format!("{}-freebsd", arch.as_str()),
            (arch, Os::NetBsd, _) => format!("{}-netbsd", arch.as_str()),
            (arch, Os::OpenBsd, _) => format!("{}-openbsd", arch.as_str()),
            (arch, Os::Android, _) => format!("{}-linux-android", arch.as_str()),
            _ => self.triple.clone(),
        }
    }

    /// Short human label for progress lines.
    pub fn label(&self) -> &str {
        &self.triple
    }
}

impl fmt::Display for CompileTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.triple)
    }
}

/// How the C23 driver invokes a system compiler for a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CcInvocation {
    pub program: String,
    pub leading_args: Vec<String>,
    pub kind: CcKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CcKind {
    /// Default host `cc` / `gcc` / system clang without an explicit `--target`.
    Host,
    /// `clang --target=<triple> …`
    Clang,
    /// `zig cc -target <triple> …`
    Zig,
}

impl CcInvocation {
    pub fn host(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            leading_args: Vec::new(),
            kind: CcKind::Host,
        }
    }

    pub fn apply(&self, cmd: &mut Command) {
        for arg in &self.leading_args {
            cmd.arg(arg);
        }
    }

    pub fn display(&self) -> String {
        if self.leading_args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.leading_args.join(" "))
        }
    }

    /// Name used for LTO / max-speed flag selection (`zig`/`clang` vs GCC).
    pub fn flag_driver_name(&self) -> &str {
        match self.kind {
            CcKind::Zig => "zig",
            CcKind::Clang => "clang",
            CcKind::Host => self
                .program
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(self.program.as_str()),
        }
    }
}

/// Pick a compiler for `target`. An explicit `preferred` binary wins.
///
/// Cross and wasm builds prefer `zig cc` when available, then `clang` with
/// `--target=`. Host builds keep `cc` (or the preferred binary as given).
pub fn select_compiler(
    target: &CompileTarget,
    preferred: Option<&str>,
) -> Result<CcInvocation, String> {
    if let Some(raw) = preferred {
        return Ok(parse_preferred_cc(raw, target));
    }
    if target.is_host() {
        return Ok(CcInvocation::host("cc"));
    }
    if which("zig") {
        return Ok(CcInvocation {
            program: "zig".into(),
            leading_args: vec!["cc".into(), "-target".into(), target.zig_triple()],
            kind: CcKind::Zig,
        });
    }
    if which("clang") {
        return Ok(CcInvocation {
            program: "clang".into(),
            leading_args: vec![format!("--target={}", target.clang_triple())],
            kind: CcKind::Clang,
        });
    }
    Err(format!(
        "no cross compiler for target `{}`: install `zig` or `clang`, or set `[build] cc`",
        target.triple
    ))
}

fn parse_preferred_cc(raw: &str, target: &CompileTarget) -> CcInvocation {
    let trimmed = raw.trim();
    if trimmed == "zig" || trimmed.starts_with("zig ") {
        let mut leading = vec!["cc".into(), "-target".into(), target.zig_triple()];
        if let Some(rest) = trimmed.strip_prefix("zig ") {
            for extra in rest.split_whitespace().filter(|part| *part != "cc") {
                if extra == "-target" {
                    continue;
                }
                leading.push(extra.to_string());
            }
        }
        return CcInvocation {
            program: "zig".into(),
            leading_args: leading,
            kind: CcKind::Zig,
        };
    }
    if trimmed == "clang" || trimmed.starts_with("clang") {
        let mut leading = vec![format!("--target={}", target.clang_triple())];
        if let Some(rest) = trimmed.strip_prefix("clang") {
            for extra in rest.split_whitespace() {
                if extra.starts_with("--target=") {
                    continue;
                }
                leading.push(extra.to_string());
            }
        }
        return CcInvocation {
            program: if trimmed.starts_with("clang++") {
                "clang++".into()
            } else {
                "clang".into()
            },
            leading_args: leading,
            kind: CcKind::Clang,
        };
    }
    // Named binary with no auto target flags — caller must pass them via link-arg.
    CcInvocation::host(trimmed)
}

fn which(bin: &str) -> bool {
    // `zig` answers `zig version`, not `zig --version`.
    let version_args: &[&[&str]] = if bin == "zig" {
        &[&["version"], &["--help"]]
    } else {
        &[&["--version"], &["-v"], &["--help"]]
    };
    for args in version_args {
        if Command::new(bin)
            .args(*args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn format_triple(arch: Arch, vendor: Vendor, os: Os, env: Env) -> String {
    match (arch, os, env) {
        (Arch::Wasm32, Os::Wasi, _) => "wasm32-wasi".into(),
        (Arch::Wasm64, Os::Wasi, _) => "wasm64-wasi".into(),
        (Arch::Wasm32, Os::Unknown | Os::None, _) => "wasm32-unknown-unknown".into(),
        (Arch::Wasm64, Os::Unknown | Os::None, _) => "wasm64-unknown-unknown".into(),
        (arch, Os::Macos, Env::None) => format!("{}-apple-darwin", arch.as_str()),
        (arch, Os::Windows, env) => format!("{}-pc-windows-{}", arch.as_str(), env.as_str()),
        (Arch::Arm, Os::Android, _) => "armv7-linux-androideabi".into(),
        (arch, Os::Android, _) => format!("{}-linux-android", arch.as_str()),
        (arch, os, Env::None) => {
            let vendor = match os {
                Os::Macos | Os::Ios => Vendor::Apple,
                Os::Windows => Vendor::Pc,
                _ => vendor,
            };
            format!("{}-{}-{}", arch.as_str(), vendor.as_str(), os.as_str())
        }
        (arch, os, env) => {
            let vendor = match os {
                Os::Windows => Vendor::Pc,
                Os::Macos | Os::Ios => Vendor::Apple,
                _ => vendor,
            };
            format!(
                "{}-{}-{}-{}",
                arch.as_str(),
                vendor.as_str(),
                os.as_str(),
                env.as_str()
            )
        }
    }
}

fn parse_triple(raw: &str) -> Result<CompileTarget, String> {
    let mut target = parse_triple_parts(raw)?;
    // Prefer a catalog spelling when the free-form parse lands on the same shape.
    if let Some(entry) = KNOWN_TARGETS.iter().find(|entry| {
        parse_triple_parts(entry.triple)
            .map(|canonical| {
                canonical.arch == target.arch
                    && canonical.os == target.os
                    && canonical.env == target.env
                    && canonical.vendor == target.vendor
            })
            .unwrap_or(false)
    }) {
        target.triple = entry.triple.to_string();
    }
    Ok(target)
}

fn parse_triple_parts(raw: &str) -> Result<CompileTarget, String> {
    let parts: Vec<&str> = raw.split('-').collect();
    match parts.as_slice() {
        [arch_s, "wasi"] | [arch_s, "wasip1"] | [arch_s, "wasip2"] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            if !arch.is_wasm() {
                return Err(format!("`{arch_s}-wasi` requires a wasm arch"));
            }
            Ok(CompileTarget::from_parts(
                arch,
                Vendor::Unknown,
                Os::Wasi,
                Env::None,
            ))
        }
        [arch_s, "unknown", "unknown"] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            Ok(CompileTarget::from_parts(
                arch,
                Vendor::Unknown,
                Os::Unknown,
                Env::None,
            ))
        }
        [arch_s, "freestanding"] | [arch_s, "none"] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            Ok(CompileTarget::from_parts(
                arch,
                Vendor::Unknown,
                Os::None,
                Env::None,
            ))
        }
        // peer / Android style: arch-os-abi without a vendor token.
        [arch_s, os_s, env_s]
            if Os::parse(os_s).is_some()
                && Env::parse(env_s).is_some()
                && Vendor::parse(os_s).is_none() =>
        {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            let os = Os::parse(os_s).unwrap();
            let env = Env::parse(env_s).unwrap();
            let vendor = match os {
                Os::Windows => Vendor::Pc,
                Os::Macos | Os::Ios => Vendor::Apple,
                _ => Vendor::Unknown,
            };
            Ok(CompileTarget::from_parts(arch, vendor, os, env))
        }
        [arch_s, "linux", "androideabi"] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            Ok(CompileTarget::from_parts(
                arch,
                Vendor::Unknown,
                Os::Android,
                Env::Android,
            ))
        }
        [arch_s, "linux", "android"] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            Ok(CompileTarget::from_parts(
                arch,
                Vendor::Unknown,
                Os::Android,
                Env::Android,
            ))
        }
        [arch_s, vendor_s, os_s] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            let vendor =
                Vendor::parse(vendor_s).ok_or_else(|| format!("unknown vendor `{vendor_s}`"))?;
            let os = Os::parse(os_s).ok_or_else(|| format!("unknown os `{os_s}`"))?;
            Ok(CompileTarget::from_parts(arch, vendor, os, Env::None))
        }
        [arch_s, vendor_s, os_s, env_s] => {
            let arch = Arch::parse(arch_s).ok_or_else(|| format!("unknown arch `{arch_s}`"))?;
            let vendor =
                Vendor::parse(vendor_s).ok_or_else(|| format!("unknown vendor `{vendor_s}`"))?;
            let os = Os::parse(os_s).ok_or_else(|| format!("unknown os `{os_s}`"))?;
            let env = Env::parse(env_s).ok_or_else(|| format!("unknown env `{env_s}`"))?;
            Ok(CompileTarget::from_parts(arch, vendor, os, env))
        }
        _ => Err(format!(
            "unrecognized target `{raw}` (expected arch-vendor-os[-env], wasm32-wasi, or wasm32-unknown-unknown)"
        )),
    }
}

fn known_by_alias(raw: &str) -> Option<CompileTarget> {
    let entry = KNOWN_TARGETS
        .iter()
        .find(|entry| entry.triple == raw || entry.aliases.contains(&raw))?;
    // Build from the canonical catalog spelling without re-entering alias lookup.
    parse_triple_parts(entry.triple).ok().map(|mut target| {
        target.triple = entry.triple.to_string();
        target
    })
}

/// One row in the public target catalog.
#[derive(Clone, Copy, Debug)]
pub struct TargetCatalogEntry {
    pub triple: &'static str,
    pub summary: &'static str,
    pub aliases: &'static [&'static str],
}

/// First-class targets Super documents and accepts in manifests.
pub const KNOWN_TARGETS: &[TargetCatalogEntry] = &[
    TargetCatalogEntry {
        triple: "x86_64-unknown-linux-gnu",
        summary: "64-bit Linux (glibc)",
        aliases: &["x86_64-linux-gnu", "linux-x86_64"],
    },
    TargetCatalogEntry {
        triple: "x86_64-unknown-linux-musl",
        summary: "64-bit Linux (musl, static-friendly)",
        aliases: &["x86_64-linux-musl"],
    },
    TargetCatalogEntry {
        triple: "aarch64-unknown-linux-gnu",
        summary: "64-bit ARM Linux (glibc)",
        aliases: &["aarch64-linux-gnu", "arm64-linux-gnu"],
    },
    TargetCatalogEntry {
        triple: "aarch64-unknown-linux-musl",
        summary: "64-bit ARM Linux (musl)",
        aliases: &["aarch64-linux-musl"],
    },
    TargetCatalogEntry {
        triple: "armv7-unknown-linux-gnueabihf",
        summary: "32-bit ARMv7 Linux hard-float",
        aliases: &["arm-linux-gnueabihf", "armv7-linux-gnueabihf"],
    },
    TargetCatalogEntry {
        triple: "riscv64gc-unknown-linux-gnu",
        summary: "64-bit RISC-V Linux (glibc)",
        aliases: &["riscv64-unknown-linux-gnu", "riscv64-linux-gnu"],
    },
    TargetCatalogEntry {
        triple: "loongarch64-unknown-linux-gnu",
        summary: "LoongArch64 Linux (glibc)",
        aliases: &["loongarch64-linux-gnu"],
    },
    TargetCatalogEntry {
        triple: "powerpc64le-unknown-linux-gnu",
        summary: "PowerPC64 LE Linux (glibc)",
        aliases: &["powerpc64le-linux-gnu", "ppc64le-linux-gnu"],
    },
    TargetCatalogEntry {
        triple: "s390x-unknown-linux-gnu",
        summary: "IBM Z Linux (glibc)",
        aliases: &["s390x-linux-gnu"],
    },
    TargetCatalogEntry {
        triple: "x86_64-pc-windows-gnu",
        summary: "64-bit Windows (MinGW)",
        aliases: &["x86_64-windows-gnu", "windows-gnu"],
    },
    TargetCatalogEntry {
        triple: "x86_64-pc-windows-msvc",
        summary: "64-bit Windows (MSVC ABI)",
        aliases: &["x86_64-windows-msvc", "windows-msvc"],
    },
    TargetCatalogEntry {
        triple: "aarch64-pc-windows-gnu",
        summary: "64-bit ARM Windows (MinGW)",
        aliases: &["aarch64-windows-gnu"],
    },
    TargetCatalogEntry {
        triple: "i686-pc-windows-gnu",
        summary: "32-bit Windows (MinGW)",
        aliases: &["i686-windows-gnu"],
    },
    TargetCatalogEntry {
        triple: "x86_64-apple-darwin",
        summary: "64-bit macOS",
        aliases: &["x86_64-macos", "macos-x86_64"],
    },
    TargetCatalogEntry {
        triple: "aarch64-apple-darwin",
        summary: "Apple Silicon macOS",
        aliases: &["aarch64-macos", "macos-arm64", "macos-aarch64"],
    },
    TargetCatalogEntry {
        triple: "aarch64-linux-android",
        summary: "64-bit Android",
        aliases: &["arm64-linux-android"],
    },
    TargetCatalogEntry {
        triple: "armv7-linux-androideabi",
        summary: "32-bit Android",
        aliases: &["arm-linux-androideabi"],
    },
    TargetCatalogEntry {
        triple: "x86_64-unknown-freebsd",
        summary: "64-bit FreeBSD",
        aliases: &["x86_64-freebsd"],
    },
    TargetCatalogEntry {
        triple: "aarch64-unknown-freebsd",
        summary: "64-bit ARM FreeBSD",
        aliases: &["aarch64-freebsd"],
    },
    TargetCatalogEntry {
        triple: "wasm32-wasi",
        summary: "WebAssembly with WASI (cli / servers)",
        aliases: &["wasm32-wasip1", "wasm32-wasi-preview1", "wasi"],
    },
    TargetCatalogEntry {
        triple: "wasm32-unknown-unknown",
        summary: "WebAssembly freestanding (no WASI)",
        aliases: &["wasm32", "wasm", "wasm32-freestanding"],
    },
    TargetCatalogEntry {
        triple: "wasm64-wasi",
        summary: "64-bit WebAssembly with WASI",
        aliases: &["wasm64-wasip1"],
    },
    TargetCatalogEntry {
        triple: "wasm64-unknown-unknown",
        summary: "64-bit WebAssembly freestanding",
        aliases: &["wasm64", "wasm64-freestanding"],
    },
];

/// Pretty-print the catalog for `super targets`.
pub fn format_catalog() -> String {
    let host = CompileTarget::host();
    let mut out = String::from("Super compile targets\n\n");
    out.push_str(&format!("host: {}\n\n", host.triple));
    out.push_str(&format!("{:<36} {}\n", "TRIPLE", "DESCRIPTION"));
    out.push_str(&format!("{:-<36} {}\n", "", "-----------"));
    for entry in KNOWN_TARGETS {
        out.push_str(&format!("{:<36} {}\n", entry.triple, entry.summary));
    }
    out.push_str(
        "\nAlso accepted: `native` / `host`, short `arch-os-abi` (e.g. `x86_64-linux-musl`),\n",
    );
    out.push_str("and any well-formed `arch-vendor-os[-env]` triple the C driver can reach.\n");
    out.push_str("Set `[build] target` or `[build] targets = […]` in Pod.toml / Super.toml,\n");
    out.push_str("or pass `--target <TRIPLE>`. Cross and wasm builds prefer `zig cc`.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_gnu_and_aliases() {
        let t = CompileTarget::parse("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(t.arch, Arch::X86_64);
        assert_eq!(t.os, Os::Linux);
        assert_eq!(t.env, Env::Gnu);
        assert_eq!(
            CompileTarget::parse("x86_64-linux-gnu").unwrap().triple,
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn parses_wasm_variants() {
        let wasi = CompileTarget::parse("wasm32-wasi").unwrap();
        assert!(wasi.is_wasm());
        assert_eq!(wasi.os, Os::Wasi);
        assert_eq!(wasi.artifact_name("app", false), "app.wasm");
        assert_eq!(wasi.zig_triple(), "wasm32-wasi");
        assert_eq!(wasi.clang_triple(), "wasm32-wasip1");

        let free = CompileTarget::parse("wasm32-unknown-unknown").unwrap();
        assert_eq!(free.zig_triple(), "wasm32-freestanding");
        assert!(!free.supports_openmp());

        let alias = CompileTarget::parse("wasi").unwrap();
        assert_eq!(alias.triple, "wasm32-wasi");
    }

    #[test]
    fn host_alias_and_native_cpu_gate() {
        let host = CompileTarget::parse("native").unwrap();
        assert!(host.is_host());
        assert!(host.allows_native_cpu());
        let cross = CompileTarget::parse("aarch64-unknown-linux-gnu").unwrap();
        assert!(!cross.allows_native_cpu() || cross.is_host());
    }

    #[test]
    fn artifact_layout_nests_cross_triples() {
        let host = CompileTarget::host();
        let root = Path::new("/pod");
        assert_eq!(
            host.artifact_dir(root, "release"),
            PathBuf::from("/pod/target/release")
        );
        let wasm = CompileTarget::parse("wasm32-wasi").unwrap();
        assert_eq!(
            wasm.artifact_dir(root, "release"),
            PathBuf::from("/pod/target/wasm32-wasi/release")
        );
    }

    #[test]
    fn windows_and_shared_names() {
        let win = CompileTarget::parse("x86_64-pc-windows-gnu").unwrap();
        assert_eq!(win.artifact_name("app", false), "app.exe");
        assert_eq!(win.artifact_name("app", true), "app.dll");
        let linux = CompileTarget::parse("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(linux.artifact_name("app", true), "libapp.so");
    }

    #[test]
    fn catalog_entries_all_parse() {
        for entry in KNOWN_TARGETS {
            let parsed = CompileTarget::parse(entry.triple)
                .unwrap_or_else(|e| panic!("{}: {e}", entry.triple));
            assert_eq!(parsed.triple, entry.triple, "{}", entry.triple);
            for alias in entry.aliases {
                let via =
                    CompileTarget::parse(alias).unwrap_or_else(|e| panic!("alias {alias}: {e}"));
                assert_eq!(via.triple, entry.triple, "alias {alias}");
            }
        }
    }

    #[test]
    fn format_catalog_mentions_wasm() {
        let text = format_catalog();
        assert!(text.contains("wasm32-wasi"));
        assert!(text.contains("host:"));
    }

    #[test]
    fn preferred_zig_and_clang_get_target_flags() {
        let t = CompileTarget::parse("wasm32-wasi").unwrap();
        let zig = select_compiler(&t, Some("zig")).unwrap();
        assert_eq!(zig.kind, CcKind::Zig);
        assert!(zig.leading_args.iter().any(|a| a == "wasm32-wasi"));
        let clang = select_compiler(&t, Some("clang")).unwrap();
        assert_eq!(clang.kind, CcKind::Clang);
        assert!(
            clang
                .leading_args
                .iter()
                .any(|a| a == "--target=wasm32-wasip1")
        );
    }
}
