//! Transpile Super bytecode to optimized standalone C.
//!
//! Arithmetic / control-flow ops lower to native C; host calls fall back to a
//! thin runtime shim so the output remains linkable with `cc -O3`.

use crate::bytecode::{Chunk, FunctionChunk, Op};
use crate::value::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use super_core::{Interner, SymId, TypeId};
use super_frontend::ast::{Item, Module, StructDef, SumDef, TypeExpr};
use super_frontend::sema::{TypeKind, TypedModule};

mod incremental;
mod outline;
mod unboxed;

pub use incremental::{
    IncrementalEmitOptions, emit_incremental_sources, write_and_compile_incremental,
};
pub use outline::DEFAULT_OUTLINE_STMT_THRESHOLD;

/// Options controlling C source emission (ABI layouts, entry symbol, test stripping).
#[derive(Clone, Debug)]
pub struct EmitCOptions<'a> {
    pub typed: Option<&'a TypedModule>,
    pub module: Option<&'a Module>,
    /// When true, omit `int main` (shared library / host-loaded module).
    pub omit_main: bool,
    /// Recorded in banner comment (`release` strips `test` blocks at compile time).
    pub release: bool,
    /// Max C statements per lowered function before automatic outlining (`0` disables).
    /// Defaults to [`DEFAULT_OUTLINE_STMT_THRESHOLD`] (~1500).
    pub outline_stmt_threshold: usize,
}

impl Default for EmitCOptions<'_> {
    fn default() -> Self {
        Self {
            typed: None,
            module: None,
            omit_main: false,
            release: false,
            outline_stmt_threshold: DEFAULT_OUTLINE_STMT_THRESHOLD,
        }
    }
}

/// Host-compiler optimization level. Spellings match `gcc`/`clang` (`-O3`, `-Os`, `-Oz`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodegenOpt {
    O0,
    O1,
    O2,
    O3,
    /// Optimize for size (`-Os`).
    Os,
    /// Optimize harder for size (`-Oz`).
    Oz,
    /// Optimize while keeping a debugger usable (`-Og`).
    Og,
    /// `-Ofast` (relaxes IEEE where the host compiler does).
    Ofast,
}

impl CodegenOpt {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().trim_start_matches("-O").trim_start_matches('O') {
            "0" => Ok(Self::O0),
            "1" => Ok(Self::O1),
            "2" => Ok(Self::O2),
            "3" => Ok(Self::O3),
            "s" | "size" => Ok(Self::Os),
            "z" => Ok(Self::Oz),
            "g" => Ok(Self::Og),
            "fast" => Ok(Self::Ofast),
            other => Err(format!(
                "unknown optimization level `{other}` (expected 0, 1, 2, 3, s, z, g, fast)"
            )),
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            Self::O0 => "-O0",
            Self::O1 => "-O1",
            Self::O2 => "-O2",
            Self::O3 => "-O3",
            Self::Os => "-Os",
            Self::Oz => "-Oz",
            Self::Og => "-Og",
            Self::Ofast => "-Ofast",
        }
    }
}

/// Link-time optimization, matching cargo's `lto` and gcc's `-flto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LtoMode {
    Off,
    Thin,
    Fat,
}

impl LtoMode {
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim() {
            "off" | "none" | "false" | "0" => Ok(Self::Off),
            "thin" => Ok(Self::Thin),
            "fat" | "full" | "on" | "true" | "1" => Ok(Self::Fat),
            other => Err(format!(
                "unknown lto mode `{other}` (expected off, thin, fat)"
            )),
        }
    }

    /// Host flag. GCC has no `-flto=thin`; that spelling is Clang's.
    /// `zig cc` is Clang-compatible (`-flto=auto` is rejected by peer).
    pub fn flag(self, compiler: &str) -> Option<&'static str> {
        let name = compiler.rsplit(['/', '\\']).next().unwrap_or(compiler);
        let clang_like = name.contains("clang") || name == "zig" || name.starts_with("zig");
        match (self, clang_like) {
            (Self::Off, _) => None,
            (Self::Thin, true) => Some("-flto=thin"),
            (Self::Thin, false) => Some("-flto=auto"),
            (Self::Fat, true) => Some("-flto=full"),
            // GCC `-flto` alone serializes LTRANS and warns; `-flto=auto` keeps fat LTO parallel.
            (Self::Fat, false) => Some("-flto=auto"),
        }
    }
}

/// Named build profile. `dev` is the default; `--release` selects [`Release`](Self::Release).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildProfile {
    Dev,
    /// Optimized tests: faster than `dev`, still keeps `test` blocks and debug info.
    Test,
    Release,
    Size,
    Fast,
}

/// Extra host flags that only the peak profiles ask for (GCC).
const MAX_SPEED_FLAGS: &[&str] = &[
    "-fomit-frame-pointer",
    "-funroll-loops",
    "-fno-plt",
    "-fno-semantic-interposition",
    // Physics / solver leaves are large once `#inline` expands; raise GCC's
    // heuristic so `always_inline` callees actually flatten into the grid loop.
    "-finline-limit=100000",
    "-fpartial-inlining",
];
/// Clang / `zig cc` subset — omit GCC-only `-fno-semantic-interposition`.
const MAX_SPEED_FLAGS_CLANG: &[&str] = &[
    "-fomit-frame-pointer",
    "-funroll-loops",
    "-fno-plt",
    "-finline-limit=100000",
];

impl BuildProfile {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name.trim() {
            "dev" | "debug" => Ok(Self::Dev),
            "test" => Ok(Self::Test),
            "release" | "rel" => Ok(Self::Release),
            "size" | "release-size" | "minsize" => Ok(Self::Size),
            "fast" | "release-fast" => Ok(Self::Fast),
            other => Err(format!(
                "unknown profile `{other}` (expected dev, test, release, size, fast)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Test => "test",
            Self::Release => "release",
            Self::Size => "size",
            Self::Fast => "fast",
        }
    }

    fn preset(self) -> ProfilePreset {
        match self {
            Self::Dev => ProfilePreset {
                opt: CodegenOpt::O0,
                lto: LtoMode::Off,
                strip: false,
                debug_info: true,
                target_cpu: None,
                fast_math: false,
                gc_sections: false,
                strip_tests: false,
                max_speed: false,
                // Dynamic link keeps `super test` / debug rebuilds light.
                static_link: false,
            },
            // Tests stay correct and debuggable, but -O2 keeps large suites from
            // running at the unoptimized dev speed.
            Self::Test => ProfilePreset {
                opt: CodegenOpt::O2,
                lto: LtoMode::Off,
                strip: false,
                debug_info: true,
                target_cpu: Some("native"),
                fast_math: false,
                gc_sections: false,
                strip_tests: false,
                max_speed: false,
                static_link: false,
            },
            // Peak speed. Fat LTO, native CPU, fast-math, and the extra flags in
            // `MAX_SPEED_FLAGS`. `fast` is the same peak so older commands do not
            // get a slower binary than `--release`.
            // Dynamic CRT by default: glibc `-static` + getaddrinfo/dlopen (HTTP /
            // optional GUI stubs) triggers ld NSS warnings. Opt in with `--static`
            // / `static = true` (prefer a musl target for a true static binary).
            Self::Release | Self::Fast => ProfilePreset {
                opt: CodegenOpt::O3,
                lto: LtoMode::Fat,
                strip: true,
                debug_info: false,
                target_cpu: Some("native"),
                fast_math: true,
                // Drop unused runtime helpers from the fat C prelude — cuts
                // text RSS on small kernels without changing semantics.
                gc_sections: true,
                strip_tests: true,
                max_speed: true,
                static_link: false,
            },
            Self::Size => ProfilePreset {
                opt: CodegenOpt::Os,
                lto: LtoMode::Fat,
                strip: true,
                debug_info: false,
                target_cpu: None,
                fast_math: false,
                gc_sections: true,
                strip_tests: true,
                max_speed: false,
                static_link: false,
            },
        }
    }
}

struct ProfilePreset {
    opt: CodegenOpt,
    lto: LtoMode,
    strip: bool,
    debug_info: bool,
    target_cpu: Option<&'static str>,
    fast_math: bool,
    gc_sections: bool,
    strip_tests: bool,
    max_speed: bool,
    /// When true, emit `-static` on Linux AOT (opt-in; musl is the honest static path).
    static_link: bool,
}

/// CLI knobs layered on top of a [`BuildProfile`]. `None` keeps the profile's value.
#[derive(Clone, Debug, Default)]
pub struct ProfileOverrides<'a> {
    pub profile: Option<&'a str>,
    /// `-r` / `--release`.
    pub release: bool,
    /// `-d` / `--debug`. Selects the dev profile.
    pub debug: bool,
    pub opt: Option<&'a str>,
    pub lto: Option<&'a str>,
    pub strip: Option<bool>,
    pub debug_info: Option<bool>,
    pub fast_math: Option<bool>,
    /// `None` keeps the profile preset (`true` for release/size/fast).
    pub static_link: Option<bool>,
    pub target_cpu: Option<&'a str>,
    pub link_args: &'a [String],
}

/// Fully resolved native build settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub profile: BuildProfile,
    pub opt: CodegenOpt,
    pub lto: LtoMode,
    pub strip: bool,
    pub debug_info: bool,
    pub fast_math: bool,
    pub static_link: bool,
    pub target_cpu: Option<String>,
    pub gc_sections: bool,
    pub strip_tests: bool,
    /// Peak-speed host flags (`-funroll-loops`, `-fno-plt`, …). On for `release` and `fast`.
    pub max_speed: bool,
    pub link_args: Vec<String>,
}

impl ResolvedProfile {
    pub fn summary(&self) -> String {
        let mut parts = vec![self.opt.flag().to_string()];
        if let Some(flag) = self.lto.flag("cc") {
            parts.push(flag.to_string());
        }
        if let Some(cpu) = &self.target_cpu {
            parts.push(format!("-march={cpu}"));
        }
        if self.fast_math {
            parts.push("-ffast-math".into());
        }
        if self.max_speed {
            parts.extend(MAX_SPEED_FLAGS.iter().map(|flag| (*flag).to_string()));
        }
        if self.gc_sections {
            parts.push("-Wl,--gc-sections".into());
        }
        if self.strip {
            parts.push("-s".into());
        }
        if self.debug_info {
            parts.push("-g".into());
        }
        if self.static_link {
            parts.push("-static".into());
        }
        parts.join(" ")
    }

    pub fn compile_options<'a>(
        &'a self,
        cc: Option<&'a str>,
        out_bin: Option<&'a Path>,
        std: &'a str,
        shared: bool,
    ) -> CCompileOptions<'a> {
        self.compile_options_for(cc, out_bin, std, shared, None)
    }

    pub fn compile_options_for<'a>(
        &'a self,
        cc: Option<&'a str>,
        out_bin: Option<&'a Path>,
        std: &'a str,
        shared: bool,
        target: Option<&'a crate::target::CompileTarget>,
    ) -> CCompileOptions<'a> {
        let mut target_cpu = self.target_cpu.as_deref();
        if let Some(target) = target
            && !target.allows_native_cpu()
            && target_cpu == Some("native")
        {
            // Cross builds cannot ask the host CPU; fall back to generic.
            target_cpu = None;
        }
        CCompileOptions {
            cc,
            out_bin,
            opt: self.opt,
            shared,
            std,
            lto: self.lto,
            strip: self.strip,
            debug_info: self.debug_info,
            static_link: self.static_link,
            target_cpu,
            fast_math: self.fast_math,
            gc_sections: self.gc_sections,
            max_speed: self.max_speed,
            link_args: &self.link_args,
            target,
            // Caller may tighten this after inspecting emitted C for `#pragma omp`.
            needs_openmp: true,
        }
    }
}

/// True when emitted C contains OpenMP directives that require `-fopenmp`.
pub fn c_source_needs_openmp(c_source: &str) -> bool {
    c_source.contains("#pragma omp") || c_source.contains("_OPENMP")
}

pub fn resolve_profile(overrides: ProfileOverrides<'_>) -> Result<ResolvedProfile, String> {
    if overrides.release && overrides.debug {
        return Err("`-r`/`--release` and `-d`/`--debug` cannot be combined".into());
    }
    let profile = match (overrides.release, overrides.debug, overrides.profile) {
        (true, _, Some(name)) if BuildProfile::parse(name)? != BuildProfile::Release => {
            return Err(
                "`-r`/`--release` selects the release profile; drop it or pass `--profile release`"
                    .into(),
            );
        }
        (true, _, _) => BuildProfile::Release,
        (_, true, Some(name)) if BuildProfile::parse(name)? != BuildProfile::Dev => {
            return Err(
                "`-d`/`--debug` selects the dev profile; drop it or pass `--profile dev`".into(),
            );
        }
        (_, true, _) => BuildProfile::Dev,
        (false, false, Some(name)) => BuildProfile::parse(name)?,
        (false, false, None) => BuildProfile::Dev,
    };
    let preset = profile.preset();
    let opt = match overrides.opt {
        Some(raw) => CodegenOpt::parse(raw)?,
        None => preset.opt,
    };
    let lto = match overrides.lto {
        Some(raw) => LtoMode::parse(raw)?,
        None => preset.lto,
    };
    let target_cpu = match overrides.target_cpu {
        Some(cpu) if cpu == "generic" || cpu.is_empty() => None,
        Some(cpu) => Some(cpu.to_string()),
        None => preset.target_cpu.map(str::to_string),
    };
    Ok(ResolvedProfile {
        profile,
        opt,
        lto,
        strip: overrides.strip.unwrap_or(preset.strip),
        debug_info: overrides.debug_info.unwrap_or(preset.debug_info),
        fast_math: overrides.fast_math.unwrap_or(preset.fast_math),
        static_link: overrides.static_link.unwrap_or(preset.static_link),
        target_cpu,
        gc_sections: preset.gc_sections,
        strip_tests: preset.strip_tests,
        max_speed: preset.max_speed,
        link_args: overrides.link_args.to_vec(),
    })
}

/// Options passed to the system C compiler when linking emitted sources.
#[derive(Clone, Debug)]
pub struct CCompileOptions<'a> {
    pub cc: Option<&'a str>,
    pub out_bin: Option<&'a Path>,
    pub opt: CodegenOpt,
    pub shared: bool,
    pub std: &'a str,
    pub lto: LtoMode,
    pub strip: bool,
    pub debug_info: bool,
    pub static_link: bool,
    /// `None` omits `-march` (generic). `"native"` is `-march=native`.
    pub target_cpu: Option<&'a str>,
    pub fast_math: bool,
    pub gc_sections: bool,
    pub max_speed: bool,
    pub link_args: &'a [String],
    /// Destination architecture / OS. Defaults to the host.
    pub target: Option<&'a crate::target::CompileTarget>,
    /// When false, omit `-fopenmp` so sequential programs do not pull in libgomp.
    pub needs_openmp: bool,
}

impl<'a> Default for CCompileOptions<'a> {
    fn default() -> Self {
        Self {
            cc: None,
            out_bin: None,
            opt: CodegenOpt::O3,
            shared: false,
            std: "c23",
            lto: LtoMode::Off,
            strip: false,
            debug_info: false,
            static_link: false,
            target_cpu: Some("native"),
            fast_math: true,
            gc_sections: false,
            max_speed: false,
            link_args: &[],
            target: None,
            needs_openmp: true,
        }
    }
}

impl CCompileOptions<'_> {
    /// Effective compile target (host when unset).
    pub fn resolved_target(&self) -> crate::target::CompileTarget {
        self.target
            .cloned()
            .unwrap_or_else(crate::target::CompileTarget::host)
    }

    /// True when LTO / max-speed flags must use Clang/`zig cc` spellings.
    /// Cross and static CRT destinations prefer `zig cc`, which rejects GCC's `-flto=auto`.
    fn clang_like_driver(&self) -> bool {
        let name = self.cc.unwrap_or("cc");
        let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
        if base.contains("clang") || base == "zig" || base.starts_with("zig") {
            return true;
        }
        let t = self.resolved_target();
        t.is_cross()
            || matches!(
                t.env,
                crate::target::Env::Musl
                    | crate::target::Env::MuslEabi
                    | crate::target::Env::MuslEabihf
            )
    }

    /// System libraries / frameworks. Must follow input `.c`/`.o` so static
    /// links actually pull `libm` (`sin`/`cos`/`sincos` under `-ffast-math`).
    fn sys_link_libs(&self, target: &crate::target::CompileTarget) -> Vec<String> {
        match target.os {
            crate::target::Os::Windows => {
                vec![
                    "-lws2_32".into(),
                    // mingw-w64 clang needs winpthread for clock_gettime64 /
                    // nanosleep64 pulled in via pthread_time.h wrappers.
                    "-lwinpthread".into(),
                    "-luser32".into(),
                    "-lgdi32".into(),
                    "-lcomctl32".into(),
                ]
            }
            crate::target::Os::Macos | crate::target::Os::Ios => vec![
                "-framework".into(),
                "AppKit".into(),
                "-framework".into(),
                "Foundation".into(),
                "-framework".into(),
                "CoreGraphics".into(),
            ],
            crate::target::Os::Wasi | crate::target::Os::Unknown | crate::target::Os::None
                if target.is_wasm() =>
            {
                Vec::new()
            }
            _ => vec!["-lm".into()],
        }
    }

    /// Argument vector for the C driver, not including the compiler binary or
    /// its leading target flags (`zig cc -target …` / `clang --target=…`).
    pub fn cc_args(&self, out_c: &Path, bin: &Path) -> Vec<String> {
        let target = self.resolved_target();
        let mut args = self.cc_common_args();
        if self.gc_sections && !target.is_wasm() {
            args.push("-ffunction-sections".into());
            args.push("-fdata-sections".into());
            args.push("-Wl,--gc-sections".into());
        }
        for link in self.link_args {
            if link.starts_with("-Wl,") || link.starts_with('-') {
                args.push(link.clone());
            } else {
                args.push(format!("-Wl,{link}"));
            }
        }
        if target.is_wasm()
            && matches!(
                target.os,
                crate::target::Os::Unknown | crate::target::Os::None
            )
        {
            args.push("-nostdlib".into());
            args.push("-Wl,--no-entry".into());
        }
        if self.strip && !target.is_wasm() {
            args.push("-s".into());
        }
        args.push("-o".into());
        args.push(bin.display().to_string());
        args.push(out_c.display().to_string());
        args.extend(self.sys_link_libs(&target));
        args
    }

    /// Host flags shared by single-file compile, per-object `-c`, and link steps.
    pub fn cc_common_args(&self) -> Vec<String> {
        let target = self.resolved_target();
        let mut args = Vec::with_capacity(24);
        args.push(self.opt.flag().to_string());
        args.push(format!("-std={}", self.std));
        if self.shared {
            args.push("-shared".into());
            args.push("-fPIC".into());
        }
        // `-static` is opt-in (`--static` / Pod `static = true`). Default release
        // builds stay dynamically linked on glibc so getaddrinfo/dlopen (HTTP,
        // optional GUI stubs) do not trip ld NSS warnings. Prefer a musl target
        // when a fully static binary is required. Parallel (`par` / OpenMP) needs
        // libgomp (almost never a `.a`) — keep the dynamic CRT when OpenMP is on.
        let want_static = self.static_link
            && !self.shared
            && !self.needs_openmp
            && !target.is_wasm()
            && target.os == crate::target::Os::Linux;
        if want_static {
            args.push("-static".into());
        }
        let driver = if self.clang_like_driver() {
            "clang"
        } else {
            self.cc.unwrap_or("cc")
        };
        if let Some(flag) = self.lto.flag(driver) {
            args.push(flag.to_string());
        }
        if self.debug_info {
            args.push("-g".into());
        }
        if let Some(cpu) = self
            .target_cpu
            .filter(|cpu| *cpu != "generic" && !cpu.is_empty())
            && (cpu != "native" || target.allows_native_cpu())
            && !target.is_wasm()
        {
            args.push(format!("-march={cpu}"));
        }
        args.push("-fwrapv".into());
        args.push("-ffp-contract=off".into());
        // Only link libgomp when the TU emits OpenMP (avoids RSS inflation on
        // sequential kernels). Dynamic CRT only — see `want_static`. Musl peers
        // have no libgomp and stay sequential.
        if target.supports_openmp()
            && self.needs_openmp
            && !matches!(
                target.env,
                crate::target::Env::Musl
                    | crate::target::Env::MuslEabi
                    | crate::target::Env::MuslEabihf
            )
        {
            args.push("-fopenmp".into());
        }
        if self.fast_math && !target.is_wasm() {
            args.push("-ffast-math".into());
            args.push("-fno-math-errno".into());
        }
        if self.max_speed && !target.is_wasm() {
            let flags = if self.clang_like_driver() {
                MAX_SPEED_FLAGS_CLANG
            } else {
                MAX_SPEED_FLAGS
            };
            args.extend(flags.iter().map(|flag| (*flag).to_string()));
        }
        args.extend(
            [
                "-Wall",
                "-Wextra",
                "-Wno-unused-parameter",
                "-Wno-unused-variable",
                "-Wno-unused-but-set-variable",
                "-Wno-unused-label",
                "-Wno-unused-function",
                "-Wno-unused-const-variable",
            ]
            .into_iter()
            .map(str::to_string),
        );
        args
    }

    /// Compile one translation unit to an object file (`-c`).
    pub fn cc_object_args(&self, out_c: &Path, out_o: &Path) -> Vec<String> {
        let mut args = self.cc_common_args();
        args.push("-c".into());
        args.push("-o".into());
        args.push(out_o.display().to_string());
        args.push(out_c.display().to_string());
        args
    }

    /// Link object files into the final binary or shared library.
    pub fn cc_link_args(&self, objects: &[PathBuf], bin: &Path) -> Vec<String> {
        let target = self.resolved_target();
        let mut args = self.cc_common_args();
        if self.gc_sections && !target.is_wasm() {
            args.push("-ffunction-sections".into());
            args.push("-fdata-sections".into());
            args.push("-Wl,--gc-sections".into());
        }
        for link in self.link_args {
            if link.starts_with("-Wl,") || link.starts_with('-') {
                args.push(link.clone());
            } else {
                args.push(format!("-Wl,{link}"));
            }
        }
        if target.is_wasm()
            && matches!(
                target.os,
                crate::target::Os::Unknown | crate::target::Os::None
            )
        {
            args.push("-nostdlib".into());
            args.push("-Wl,--no-entry".into());
        }
        if self.strip && !target.is_wasm() {
            args.push("-s".into());
        }
        args.push("-o".into());
        args.push(bin.display().to_string());
        for object in objects {
            args.push(object.display().to_string());
        }
        // After objects: required for `-static` to resolve libm (sincos/sin/cos).
        args.extend(self.sys_link_libs(&target));
        args
    }
}

/// Emit a self-contained `.c` translation of `chunk` (backward-compatible defaults).
pub fn emit_c(chunk: &Chunk, interner: &Interner, module_name: &str) -> String {
    emit_c_module(chunk, interner, module_name, EmitCOptions::default())
}

/// Emit optimized standalone C with optional typed C-ABI layouts and LUT tables.
pub fn emit_c_module(
    chunk: &Chunk,
    interner: &Interner,
    module_name: &str,
    opts: EmitCOptions<'_>,
) -> String {
    let mut out = String::with_capacity(32 * 1024);
    let guard = sanitize_ident(module_name).to_uppercase();
    let profile = if opts.release { "release" } else { "dev" };
    writeln!(
        out,
        "/* Generated by Super AOT ({profile}) — do not edit by hand. */"
    )
    .unwrap();
    // POSIX hosts (`setenv`, `nanosleep`) are not in the ISO C headers unless a
    // feature macro is defined before the first include.
    writeln!(out, "#define _DEFAULT_SOURCE").unwrap();
    writeln!(out, "#include <stdint.h>").unwrap();
    writeln!(out, "#include <stdbool.h>").unwrap();
    writeln!(out, "#include <stddef.h>").unwrap();
    // C23 `<stdbit.h>` is optional on freestanding / WASI toolchains.
    writeln!(out, "#if defined(__has_include)").unwrap();
    writeln!(out, "#  if __has_include(<stdbit.h>)").unwrap();
    writeln!(out, "#    include <stdbit.h>").unwrap();
    writeln!(out, "#  endif").unwrap();
    writeln!(out, "#elif !defined(__wasm__) && !defined(__wasi__)").unwrap();
    writeln!(out, "#  include <stdbit.h>").unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(out, "#include <stdio.h>").unwrap();
    writeln!(out, "#include <stdlib.h>").unwrap();
    writeln!(out, "#include <string.h>").unwrap();
    writeln!(out, "#include <math.h>").unwrap();
    writeln!(out, "#include <time.h>").unwrap();
    writeln!(out, "#include <errno.h>").unwrap();
    // Platform host surfaces. Keep POSIX-only headers out of Windows AOT units.
    writeln!(out, "#if defined(_WIN32)").unwrap();
    writeln!(out, "#define WIN32_LEAN_AND_MEAN").unwrap();
    writeln!(out, "#include <windows.h>").unwrap();
    writeln!(out, "#include <io.h>").unwrap();
    writeln!(out, "#include <winsock2.h>").unwrap();
    writeln!(out, "#include <ws2tcpip.h>").unwrap();
    writeln!(
        out,
        "static inline int super_setenv_win(const char *k, const char *v, int overwrite) {{ (void)overwrite; return _putenv_s(k, v); }}"
    )
    .unwrap();
    writeln!(out, "#define setenv super_setenv_win").unwrap();
    writeln!(out, "#elif !defined(__wasm__) && !defined(__wasi__)").unwrap();
    writeln!(out, "#include <dlfcn.h>").unwrap();
    writeln!(out, "#include <unistd.h>").unwrap();
    writeln!(out, "#include <termios.h>").unwrap();
    writeln!(out, "#include <poll.h>").unwrap();
    writeln!(out, "#include <signal.h>").unwrap();
    writeln!(out, "#include <sys/ioctl.h>").unwrap();
    writeln!(out, "#include <sys/socket.h>").unwrap();
    writeln!(out, "#include <sys/types.h>").unwrap();
    writeln!(out, "#include <netinet/in.h>").unwrap();
    writeln!(out, "#include <arpa/inet.h>").unwrap();
    writeln!(out, "#include <netdb.h>").unwrap();
    writeln!(out, "#include <fcntl.h>").unwrap();
    writeln!(out, "#include <sys/mman.h>").unwrap();
    writeln!(out, "#if defined(__linux__)").unwrap();
    writeln!(out, "#include <sys/epoll.h>").unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(out, "#elif defined(__wasi__)").unwrap();
    writeln!(out, "#include <unistd.h>").unwrap();
    writeln!(out, "#include <fcntl.h>").unwrap();
    writeln!(out, "#endif").unwrap();
    // Freestanding / WASI builds keep the host GUI/net stubs linkable without dlfcn.
    writeln!(
        out,
        "#if defined(_WIN32) || defined(__wasm__) || defined(__wasi__)"
    )
    .unwrap();
    writeln!(out, "#ifndef RTLD_LAZY").unwrap();
    writeln!(out, "#define RTLD_LAZY 1").unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(out, "#ifndef RTLD_LOCAL").unwrap();
    writeln!(out, "#define RTLD_LOCAL 2").unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(out, "#ifndef RTLD_NOW").unwrap();
    writeln!(out, "#define RTLD_NOW 4").unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(
        out,
        "static inline void *dlopen(const char *n, int f) {{ (void)n; (void)f; return 0; }}"
    )
    .unwrap();
    writeln!(
        out,
        "static inline void *dlsym(void *h, const char *n) {{ (void)h; (void)n; return 0; }}"
    )
    .unwrap();
    writeln!(out, "#endif").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "#ifndef SUPER_C_RUNTIME_{guard}").unwrap();
    writeln!(out, "#define SUPER_C_RUNTIME_{guard}").unwrap();
    writeln!(out).unwrap();
    out.push_str(&emit_font8());
    out.push_str("#define SUPER_GUI_SPLIT 1\n");
    out.push_str("#define SUPER_PROVIDER_API static\n");
    out.push_str("#define SUPER_GUI_PASTED 1\n");
    out.push_str(include_str!("gui_provider.h"));
    out.push('\n');
    out.push_str(include_str!("gui_doc.h"));
    out.push('\n');
    out.push_str(include_str!("gtk_provider.c"));
    out.push('\n');
    out.push_str(include_str!("win32_provider.c"));
    out.push('\n');
    out.push_str(include_str!("cocoa_provider.c"));
    out.push('\n');
    out.push_str(include_str!("gui_dispatch.c"));
    out.push('\n');
    out.push_str(RUNTIME_PRELUDE);
    // Escape-analysis → codegen: record how many Make* births were classified.
    let stay = chunk
        .escape_birth
        .iter()
        .filter(|(_, r)| *r == 1)
        .count();
    let durable = chunk
        .escape_birth
        .iter()
        .filter(|(_, r)| *r == 2)
        .count();
    writeln!(
        out,
        "/* escape_analysis: tagged_births={} stay_arena={} promote_or_conservative_durable={} */",
        chunk.escape_birth.len(),
        stay,
        durable
    )
    .unwrap();
    writeln!(out).unwrap();
    // TCP / signal / stdio / color host — after SuperVal helpers from RUNTIME_PRELUDE.
    // Round 9: owned by `aot_host` (Rust); no standalone `net_host.c`.
    out.push_str(crate::aot_host::NET_HOST_C);
    out.push('\n');

    if let (Some(typed), Some(module)) = (opts.typed, opts.module) {
        emit_c_abi_types(&mut out, typed, interner, module);
        writeln!(out).unwrap();
    }

    // String / constant pool
    writeln!(out, "/* --- constant pool --- */").unwrap();
    for (i, c) in chunk.consts.iter().enumerate() {
        match c {
            Value::Str(s) => {
                writeln!(
                    out,
                    "static const char *const SUPER_STR_{i} = \"{}\";",
                    escape_c_string(s)
                )
                .unwrap();
            }
            Value::Int(n) => {
                writeln!(out, "static const int64_t SUPER_I64_{i} = {n}LL;").unwrap();
            }
            Value::Float(f) => {
                writeln!(out, "static const double SUPER_F64_{i} = {f:?};").unwrap();
            }
            Value::Bool(b) => {
                writeln!(
                    out,
                    "static const bool SUPER_BOOL_{i} = {};",
                    if *b { "true" } else { "false" }
                )
                .unwrap();
            }
            Value::Array(elems) if elems.iter().all(|v| matches!(v, Value::Int(_))) => {
                let vals: Vec<String> = elems.iter().map(|v| format!("{}LL", v.as_int())).collect();
                writeln!(
                    out,
                    "static constexpr int64_t SUPER_LUT_{i}[] = {{ {} }};",
                    vals.join(", ")
                )
                .unwrap();
                writeln!(
                    out,
                    "static constexpr size_t SUPER_LUT_{i}_LEN = {};",
                    elems.len()
                )
                .unwrap();
            }
            Value::Bytes(b) => {
                let hex: Vec<String> = b.iter().map(|x| format!("0x{x:02x}")).collect();
                writeln!(
                    out,
                    "/* Comptime byte table; large file-backed assets may use C23 #embed. */"
                )
                .unwrap();
                writeln!(
                    out,
                    "static constexpr uint8_t SUPER_BYTES_{i}[] = {{ {} }};",
                    hex.join(", ")
                )
                .unwrap();
            }
            _ => {
                writeln!(out, "/* const[{i}] = complex — materialized at runtime */").unwrap();
            }
        }
    }
    writeln!(out).unwrap();

    // Module globals (LoadGlobal / StoreGlobal) — top-level `+N = …` bindings.
    let mut global_slots: u32 = 0;
    for op in &chunk.ops {
        if let Op::LoadGlobal(s) | Op::StoreGlobal(s) = op {
            global_slots = global_slots.max(s.index().saturating_add(1));
        }
    }
    if global_slots > 0 {
        writeln!(out, "/* --- module globals (SymId.index → SuperVal) --- */").unwrap();
        writeln!(
            out,
            "static SuperVal super_globals[{global_slots}] = {{0}};"
        )
        .unwrap();
        writeln!(out).unwrap();
    }

    // Whole-program monomorphization: typed functions become unboxed C23; only truly dynamic
    // code keeps the boxed `SuperVal` lowering. Oversized bodies are outlined so GCC/Clang
    // keep a sane cost model instead of one multi-thousand-line C function.
    let planned = unboxed::plan(chunk, interner, opts.outline_stmt_threshold);
    out.push_str(&unboxed::unboxed_prelude());
    writeln!(out).unwrap();
    for note in &planned.notes {
        writeln!(out, "{note}").unwrap();
    }
    writeln!(out, "/* --- boxed (dynamic) forward decls --- */").unwrap();
    for &fi in &planned.dyn_fns {
        let f = &chunk.functions[fi];
        let fname = function_c_name(chunk, interner, fi);
        let inline_kw = if f.always_inline {
            "static inline __attribute__((always_inline)) "
        } else {
            "static "
        };
        writeln!(
            out,
            "{inline_kw}SuperVal {fname}(SuperVal *restrict args, int nargs);"
        )
        .unwrap();
    }
    writeln!(out, "static SuperVal super_module_main(void);").unwrap();
    out.push_str(&planned.types);
    out.push_str(&planned.decls);
    writeln!(out).unwrap();
    out.push_str(&planned.defs);

    // Boxed per-function C bodies (dynamic fallback only).
    for &fi in &planned.dyn_fns {
        emit_function(&mut out, chunk, interner, fi, &chunk.functions[fi], false);
        writeln!(out).unwrap();
    }

    match &planned.module_wrapper {
        Some(wrapper) => out.push_str(wrapper),
        None => emit_boxed_module_main(&mut out, chunk, interner),
    }
    writeln!(out).unwrap();

    if !opts.omit_main {
        writeln!(out, "int main(int argc, char **argv) {{").unwrap();
        writeln!(out, "  super_argc = argc;").unwrap();
        writeln!(out, "  super_argv = argv;").unwrap();
        writeln!(out, "  SuperVal r = super_module_main();").unwrap();
        writeln!(out, "  if (r.tag == SUPER_TAG_INT) return (int)r.as.i;").unwrap();
        writeln!(out, "  return 0;").unwrap();
        writeln!(out, "}}").unwrap();
        writeln!(out).unwrap();
    } else {
        writeln!(
            out,
            "/* Shared library: call super_module_main() from the host loader. */"
        )
        .unwrap();
        writeln!(out).unwrap();
    }

    writeln!(out, "#endif /* SUPER_C_RUNTIME_{guard} */").unwrap();
    out
}

/// Boxed `super_module_main` for modules whose top-level code is not monomorphic.
pub(super) fn emit_boxed_module_main(out: &mut String, chunk: &Chunk, interner: &Interner) {
    // Skip the leading `Jump` over function bodies (see Compiler::compile).
    let module_start = match chunk.ops.first() {
        Some(Op::Jump(t)) => (*t as usize).min(chunk.ops.len()),
        _ => 0,
    };
    writeln!(out, "static SuperVal super_module_main(void) {{").unwrap();
    writeln!(out, "  SuperVal locals[256];").unwrap();
    writeln!(out, "  SuperVal stack[512];").unwrap();
    writeln!(out, "  int sp = 0;").unwrap();
    if span_uses_iter(&chunk.ops[module_start..]) {
        writeln!(out, "  SuperIter iters[16];").unwrap();
        writeln!(out, "  memset(iters, 0, sizeof(iters));").unwrap();
        writeln!(out, "  int itp = 0;").unwrap();
    }
    writeln!(out, "  memset(locals, 0, sizeof(locals));").unwrap();
    writeln!(out, "  memset(stack, 0, sizeof(stack));").unwrap();
    emit_op_stream(
        out,
        chunk,
        interner,
        module_start,
        chunk.ops.len(),
        "  ",
        false,
        None,
    );
    writeln!(out, "  return sp > 0 ? stack[sp-1] : SUPER_NULL;").unwrap();
    writeln!(out, "}}").unwrap();
}

fn emit_c_abi_types(out: &mut String, typed: &TypedModule, interner: &Interner, module: &Module) {
    writeln!(out, "/* --- C-ABI struct layouts (from TypedModule) --- */").unwrap();
    let mut items = Vec::new();
    for item in &module.items {
        match item {
            Item::Struct(def) => items.push(AbiItem::Struct(def)),
            Item::Sum(def) => items.push(AbiItem::Sum(def)),
            _ => {}
        }
    }
    // A field of struct or sum type is a complete C type, so the dependency
    // has to be defined first. Source order is not that order: an imported
    // `SqlDb` can be mentioned by `SqliteConn` before its own typedef.
    for index in abi_emit_order(&items, typed, interner) {
        match items[index] {
            AbiItem::Struct(def) => emit_struct_typedef(out, typed, interner, def),
            AbiItem::Sum(def) => emit_sum_typedef(out, typed, interner, def),
        }
    }
}

enum AbiItem<'a> {
    Struct(&'a StructDef),
    Sum(&'a SumDef),
}

fn abi_item_name(item: &AbiItem<'_>) -> SymId {
    match item {
        AbiItem::Struct(def) => def.name,
        AbiItem::Sum(def) => def.name,
    }
}

/// Indices in an order where every struct or sum field type is already defined.
fn abi_emit_order(items: &[AbiItem<'_>], typed: &TypedModule, interner: &Interner) -> Vec<usize> {
    let mut by_name: HashMap<SymId, Vec<usize>> = HashMap::new();
    for (index, item) in items.iter().enumerate() {
        by_name.entry(abi_item_name(item)).or_default().push(index);
    }
    let mut succ = vec![Vec::new(); items.len()];
    let mut indegree = vec![0u32; items.len()];
    for (index, item) in items.iter().enumerate() {
        let mut seen = HashSet::new();
        for dep in abi_item_deps(item, typed, interner) {
            if !seen.insert(dep) {
                continue;
            }
            let Some(targets) = by_name.get(&dep) else {
                continue;
            };
            for &target in targets {
                if target == index {
                    continue;
                }
                succ[target].push(index);
                indegree[index] += 1;
            }
        }
    }
    let mut ready: Vec<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, degree)| **degree == 0)
        .map(|(index, _)| index)
        .collect();
    let mut emitted = vec![false; items.len()];
    let mut order = Vec::with_capacity(items.len());
    while !ready.is_empty() {
        let pick = ready
            .iter()
            .enumerate()
            .min_by_key(|(_, index)| *index)
            .map(|(pos, _)| pos)
            .expect("ready is non-empty");
        let index = ready.swap_remove(pick);
        if emitted[index] {
            continue;
        }
        emitted[index] = true;
        order.push(index);
        for &dependent in &succ[index] {
            indegree[dependent] = indegree[dependent].saturating_sub(1);
            if indegree[dependent] == 0 {
                ready.push(dependent);
            }
        }
    }
    for index in 0..items.len() {
        if !emitted[index] {
            order.push(index);
        }
    }
    order
}

fn abi_item_deps(item: &AbiItem<'_>, typed: &TypedModule, interner: &Interner) -> Vec<SymId> {
    match item {
        AbiItem::Struct(def) => {
            if let Some(tid) = typed.structs.get(&def.name)
                && let TypeKind::Struct { fields, .. } = typed.types.get_type(*tid)
            {
                return fields
                    .iter()
                    .filter(|field| !field.private)
                    .filter_map(|field| type_id_dep(typed, field.ty))
                    .collect();
            }
            def.fields
                .iter()
                .filter(|field| !field.private)
                .filter_map(|field| type_expr_dep(&field.ty, interner))
                .collect()
        }
        AbiItem::Sum(def) => {
            if let Some(tid) = typed.sums.get(&def.name)
                && let TypeKind::Sum { variants, .. } = typed.types.get_type(*tid)
            {
                return variants
                    .iter()
                    .flat_map(|variant| {
                        variant
                            .fields
                            .iter()
                            .filter_map(|(_, ty)| type_id_dep(typed, *ty))
                    })
                    .collect();
            }
            def.variants
                .iter()
                .flat_map(|variant| {
                    variant
                        .fields
                        .iter()
                        .filter_map(|(_, ty)| type_expr_dep(ty, interner))
                })
                .collect()
        }
    }
}

fn type_id_dep(typed: &TypedModule, tid: TypeId) -> Option<SymId> {
    match typed.types.get_type(tid) {
        TypeKind::Struct { name, .. } | TypeKind::Sum { name, .. } => Some(*name),
        TypeKind::Optional(inner) | TypeKind::Error(inner) => type_id_dep(typed, *inner),
        TypeKind::Named(name)
            if typed.structs.contains_key(name) || typed.sums.contains_key(name) =>
        {
            Some(*name)
        }
        _ => None,
    }
}

fn type_expr_dep(ty: &TypeExpr, interner: &Interner) -> Option<SymId> {
    match ty {
        TypeExpr::Named(name, _) => {
            if is_c_primitive(interner.resolve(*name)) {
                None
            } else {
                Some(*name)
            }
        }
        TypeExpr::Optional(inner, _) | TypeExpr::Error(inner, _) => type_expr_dep(inner, interner),
        TypeExpr::Path(parts, _) => parts
            .last()
            .copied()
            .filter(|name| !is_c_primitive(interner.resolve(*name))),
        _ => None,
    }
}

fn is_c_primitive(name: &str) -> bool {
    matches!(
        name,
        "void"
            | "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "int"
            | "f32"
            | "f64"
            | "str"
    )
}

fn emit_struct_typedef(
    out: &mut String,
    typed: &TypedModule,
    interner: &Interner,
    s: &super_frontend::ast::StructDef,
) {
    let name = interner.resolve(s.name);
    let cname = format!("Super{}", sanitize_ident(name));
    writeln!(out, "typedef struct {cname} {{").unwrap();
    if let Some(tid) = typed.structs.get(&s.name)
        && let TypeKind::Struct { fields, size, .. } = typed.types.get_type(*tid)
    {
        for f in fields {
            if f.private {
                continue;
            }
            let fname = sanitize_ident(interner.resolve(f.name));
            let cty = c_type_name(typed, interner, f.ty);
            writeln!(out, "  {cty} {fname};").unwrap();
        }
        writeln!(out, "}} {cname};").unwrap();
        // Document Super's arena layout size. Exact `sizeof` equality is not
        // required on every host ABI (bool packing / SuperStr fat pointers
        // still match field order and C-ABI intent).
        writeln!(
            out,
            "/* Super ABI size={size}; host sizeof may differ by padding */"
        )
        .unwrap();
        writeln!(
            out,
            "static_assert(sizeof({cname}) >= sizeof(int64_t), \"{cname} layout\");"
        )
        .unwrap();
        return;
    }
    for f in &s.fields {
        if f.private {
            continue;
        }
        let fname = sanitize_ident(interner.resolve(f.name));
        let cty = type_expr_c(&f.ty, interner);
        writeln!(out, "  {cty} {fname};").unwrap();
    }
    writeln!(out, "}} {cname};").unwrap();
}

fn emit_sum_typedef(out: &mut String, typed: &TypedModule, interner: &Interner, s: &SumDef) {
    let name = interner.resolve(s.name);
    let cname = format!("Super{}", sanitize_ident(name));
    writeln!(out, "typedef struct {cname} {{").unwrap();
    writeln!(out, "  uint32_t tag;").unwrap();
    writeln!(out, "  union {{").unwrap();
    for v in &s.variants {
        let vname = sanitize_ident(interner.resolve(v.name));
        if v.fields.is_empty() {
            writeln!(out, "    uint8_t {vname}_pad;").unwrap();
        } else {
            writeln!(out, "    struct {{").unwrap();
            for (fname, fty) in &v.fields {
                let fnm = sanitize_ident(interner.resolve(*fname));
                let cty = type_expr_c(fty, interner);
                writeln!(out, "      {cty} {fnm};").unwrap();
            }
            writeln!(out, "    }} {vname};").unwrap();
        }
    }
    writeln!(out, "  }} as;").unwrap();
    writeln!(out, "}} {cname};").unwrap();
    if let Some(tid) = typed.sums.get(&s.name)
        && let TypeKind::Sum { size, .. } = typed.types.get_type(*tid)
    {
        writeln!(
            out,
            "static_assert(sizeof({cname}) >= {size}, \"{cname} tagged union\");"
        )
        .unwrap();
    }
}

fn c_type_name(typed: &TypedModule, interner: &Interner, tid: TypeId) -> String {
    match typed.types.get_type(tid) {
        TypeKind::Void | TypeKind::Never => "void".into(),
        TypeKind::Bool => "bool".into(),
        TypeKind::U8 => "uint8_t".into(),
        TypeKind::I8 => "int8_t".into(),
        TypeKind::U16 => "uint16_t".into(),
        TypeKind::I16 => "int16_t".into(),
        TypeKind::U32 => "uint32_t".into(),
        TypeKind::I32 => "int32_t".into(),
        TypeKind::U64 => "uint64_t".into(),
        TypeKind::I64 => "int64_t".into(),
        TypeKind::I128 => "__int128".into(),
        TypeKind::F32 => "float".into(),
        TypeKind::F64 => "double".into(),
        // Super `str` is a fat pointer {ptr,len} (16 bytes) to match C-ABI layouts.
        TypeKind::Str => "SuperStr".into(),
        TypeKind::Struct { name, .. } => {
            format!("Super{}", sanitize_ident(interner.resolve(*name)))
        }
        TypeKind::Sum { name, .. } => format!("Super{}", sanitize_ident(interner.resolve(*name))),
        _ => "int64_t".into(),
    }
}

fn type_expr_c(ty: &TypeExpr, interner: &Interner) -> String {
    match ty {
        TypeExpr::Named(n, _) => match interner.resolve(*n) {
            "bool" => "bool".into(),
            "u8" => "uint8_t".into(),
            "i8" => "int8_t".into(),
            "u16" => "uint16_t".into(),
            "i16" => "int16_t".into(),
            "u32" => "uint32_t".into(),
            "i32" => "int32_t".into(),
            "u64" => "uint64_t".into(),
            "i64" | "int" => "int64_t".into(),
            "i128" => "__int128".into(),
            "f32" => "float".into(),
            "f64" => "double".into(),
            "str" => "SuperStr".into(),
            other => format!("Super{}", sanitize_ident(other)),
        },
        TypeExpr::Optional(inner, _) | TypeExpr::Error(inner, _) => type_expr_c(inner, interner),
        _ => "int64_t".into(),
    }
}

/// Write C source to `out_c`, optionally invoke `cc` (defaults: `-O3 -std=c23`).
/// Returns the binary path when `compile` is true.
pub fn write_and_maybe_compile(
    source: &str,
    out_c: &Path,
    compile: bool,
    cc: Option<&str>,
    out_bin: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let opts = CCompileOptions {
        cc,
        out_bin,
        needs_openmp: c_source_needs_openmp(source),
        ..CCompileOptions::default()
    };
    if !compile {
        return write_and_maybe_compile_with(source, out_c, false, &opts);
    }
    write_and_maybe_compile_with(source, out_c, true, &opts)
}

/// Extended compile driver: shared libs (`-shared -fPIC`), custom `-O` level, and `-std=c23`.
pub fn write_and_maybe_compile_with(
    source: &str,
    out_c: &Path,
    compile: bool,
    opts: &CCompileOptions<'_>,
) -> Result<Option<PathBuf>, String> {
    std::fs::write(out_c, source).map_err(|e| format!("write {}: {e}", out_c.display()))?;
    // Tighten OpenMP linking from the actual TU: callers often leave
    // `needs_openmp: true` as a safe default from the profile helper.
    let mut opts_owned;
    let opts = if opts.needs_openmp && !c_source_needs_openmp(source) {
        opts_owned = opts.clone();
        opts_owned.needs_openmp = false;
        &opts_owned
    } else {
        opts
    };
    if !compile {
        return Ok(None);
    }
    let target = opts.resolved_target();
    let invocation = crate::target::select_compiler(&target, opts.cc)?;
    let bin = opts.out_bin.map(|p| p.to_path_buf()).unwrap_or_else(|| {
        if opts.shared {
            default_shared_name(out_c)
        } else if target.is_wasm() {
            out_c.with_extension("wasm")
        } else {
            let candidate = out_c.with_extension("");
            if candidate == *out_c {
                out_c.with_extension("out")
            } else {
                candidate
            }
        }
    });
    if let Some(parent) = bin.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut cmd = std::process::Command::new(&invocation.program);
    invocation.apply(&mut cmd);
    cmd.args(opts.cc_args(out_c, &bin));
    apply_cc_diagnostic_colors(&mut cmd);
    let display = invocation.display();
    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn {display}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{display} failed with status {status} (source: {}, target: {})",
            out_c.display(),
            target.triple
        ));
    }
    Ok(Some(bin))
}

/// Neon-yellow warnings / cyan-red errors for GCC/Clang diagnostics (not magenta).
fn apply_cc_diagnostic_colors(cmd: &mut std::process::Command) {
    if !super_core::wants_color() {
        return;
    }
    // GCC_COLORS keys: error, warning, note, caret, locus, quote, path, …
    cmd.env(
        "GCC_COLORS",
        "error=01;38;2;255;84;112:warning=01;38;2;255;255;0:note=01;38;2;64;196;255:caret=01;38;2;0;214;170:locus=01:quote=01:path=01;38;2;120;144;156",
    );
    cmd.env("CLICOLOR_FORCE", "1");
}

/// Parse emitted C with the host compiler. Does not link or optimize.
///
/// `super check` uses this so a program that type-checks but cannot be compiled
/// (an incomplete struct, a renamed keyword that was missed) fails the check.
pub fn syntax_check_c(
    source: &str,
    out_c: &Path,
    cc: Option<&str>,
    std: &str,
) -> Result<(), String> {
    if let Some(parent) = out_c.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    std::fs::write(out_c, source).map_err(|e| format!("write {}: {e}", out_c.display()))?;
    let compiler = cc.unwrap_or("cc");
    let mut cmd = std::process::Command::new(compiler);
    cmd.arg("-fsyntax-only").arg(format!("-std={std}"));
    if c_source_needs_openmp(source) {
        cmd.arg("-fopenmp");
    }
    cmd.args([
        "-Wall",
        "-Wextra",
        "-Wno-unused-parameter",
        "-Wno-unused-variable",
        "-Wno-unused-but-set-variable",
        "-Wno-unused-label",
        "-Wno-unused-function",
        "-Wno-unused-const-variable",
    ])
    .arg(out_c);
    apply_cc_diagnostic_colors(&mut cmd);
    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn {compiler}: {e}"))?;
    if !status.success() {
        return Err(format!(
            "{compiler} failed with status {status} (source: {})",
            out_c.display()
        ));
    }
    Ok(())
}

fn default_shared_name(out_c: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    let ext = "dylib";
    #[cfg(target_os = "windows")]
    let ext = "dll";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let ext = "so";
    out_c.with_extension(ext)
}

pub(super) fn emit_function(
    out: &mut String,
    chunk: &Chunk,
    interner: &Interner,
    fi: usize,
    f: &FunctionChunk,
    export_linkage: bool,
) {
    let fname = function_c_name(chunk, interner, fi);
    let locals = f.local_count.max(f.arity as u16).max(8) as usize;
    let restrict_args = typed_param_restrict(chunk, interner, fi);
    let inline_kw = if export_linkage {
        if f.always_inline {
            "__attribute__((always_inline)) "
        } else {
            ""
        }
    } else if f.always_inline {
        "static inline __attribute__((always_inline)) "
    } else {
        "static "
    };
    writeln!(
        out,
        "{inline_kw}SuperVal {fname}(SuperVal *{restrict_args}args, int nargs) {{"
    )
    .unwrap();

    // Benchmark fingerprint peeps removed — all functions lower through the
    // general unboxed / SuperVal pipeline below (typed restrict views + op stream).

    writeln!(out, "  SuperVal locals[{locals}];").unwrap();
    // Tight operand stack: prefer a depth derived from the op stream over a fixed 512
    // so SROA / regalloc see a smaller aggregate when the body is shallow.
    let start = f.entry as usize;
    let end = chunk
        .functions
        .iter()
        .map(|of| of.entry as usize)
        .filter(|&e| e > start)
        .min()
        .unwrap_or_else(|| match chunk.ops.first() {
            Some(Op::Jump(t)) if (*t as usize) > start => *t as usize,
            _ => chunk.ops.len(),
        });
    let stack_depth = boxed_stack_depth(&chunk.ops[start..end.min(chunk.ops.len())]).clamp(8, 512);
    writeln!(out, "  SuperVal stack[{stack_depth}];").unwrap();
    writeln!(out, "  int sp = 0;").unwrap();
    writeln!(out, "  memset(locals, 0, sizeof(locals));").unwrap();
    // Zero the operand stack so LTO cannot warn about maybe-uninitialized union arms.
    writeln!(out, "  memset(stack, 0, sizeof(stack));").unwrap();
    writeln!(
        out,
        "  for (int i = 0; i < nargs && i < {arity}; ++i) locals[i] = args[i];",
        arity = f.arity
    )
    .unwrap();

    // Scalar SROA views: known-tag locals become typed C scalars that mirror `locals[i]`.
    // Clang/GCC promote these into registers; Dyn slots stay on the SuperVal array.
    emit_boxed_scalar_views(out, f, locals);

    if span_uses_iter(&chunk.ops[start..end.min(chunk.ops.len())]) {
        writeln!(out, "  SuperIter iters[16];").unwrap();
        writeln!(out, "  memset(iters, 0, sizeof(iters));").unwrap();
        writeln!(out, "  int itp = 0;").unwrap();
    }
    emit_op_stream(
        out,
        chunk,
        interner,
        start,
        end,
        "  ",
        true,
        Some(f.local_tags.as_slice()),
    );
    writeln!(out, "  return SUPER_NULL;").unwrap();
    writeln!(out, "}}").unwrap();
}

/// Conservative max operand-stack depth for a boxed op span (push − pop balance).
///
/// Must count every op that **pushes** — under-counting produces `SuperVal stack[N]`
/// too small for the C emitter and trips `__stack_chk_fail` (seen on `Matches.new`
/// / multi-field struct literals with `MakeArray(0)` empty-array values).
fn boxed_stack_depth(ops: &[Op]) -> usize {
    let mut sp: i32 = 0;
    let mut peak: i32 = 0;
    for op in ops {
        let (push, pop) = match op {
            Op::LoadConst(_)
            | Op::LoadLocal(_)
            | Op::LoadGlobal(_)
            | Op::Dup
            | Op::PushIt
            | Op::PushIndex
            | Op::MakeFn(_)
            | Op::Spawn(_)
            | Op::IsAbsent => (1, 0),
            // Empty `[]` allocates and pushes; `MakeArray(n>0)` pops `n` elems then pushes.
            Op::MakeArray(n) => (1, *n as i32),
            Op::StoreLocal(_) | Op::StoreGlobal(_) | Op::Pop | Op::IterBegin => (0, 1),
            Op::JumpIfFalse(_) | Op::JumpIfTrue(_) => (0, 1),
            Op::JumpIfLt(_) | Op::JumpIfGe(_) | Op::JumpIfEq(_) | Op::JumpIfNe(_) => (0, 2),
            Op::AssertEq => (0, 2),
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Rem
            | Op::Eq
            | Op::Ne
            | Op::Lt
            | Op::Le
            | Op::Gt
            | Op::Ge
            | Op::And
            | Op::OrLog
            | Op::BitAnd
            | Op::BitOr
            | Op::BitXor
            | Op::Shl
            | Op::Shr
            | Op::UShr
            | Op::MakeRange
            | Op::IndexGet
            | Op::LoadSliceUnchecked
            | Op::SetField(_) => (1, 2),
            Op::IndexSet | Op::StoreSliceUnchecked => (1, 3),
            Op::Neg
            | Op::Not
            | Op::BitNot
            | Op::AddImm(_)
            | Op::SubImm(_)
            | Op::MulImm(_)
            | Op::ShlImm(_)
            | Op::OrElse
            | Op::Fail
            | Op::Print
            | Op::GetField(_)
            | Op::LoadFieldOff(_)
            | Op::StoreFieldOff(_)
            | Op::IsVariant(_)
            | Op::SumTag
            | Op::IterSetIt
            | Op::Await
            | Op::MakeDuration(_) => (1, 1),
            Op::Call(_, a) | Op::CallMut(_, a, _) | Op::CallHost(_, a) => (1, *a as i32),
            // Callable sits under the args.
            Op::CallValue(a) => (1, *a as i32 + 1),
            Op::MakeClosure(_, n) => (1, *n as i32),
            // Map/struct/sum: `n` (key, value) pairs → pop `2n`, push aggregate.
            Op::MakeMap(n) | Op::MakeStruct(_, n) => (1, (*n as i32) * 2),
            Op::MakeSum { n_fields, .. } => (1, (*n_fields as i32) * 2),
            Op::ChanNew | Op::SharedNew => (1, 0),
            Op::ChanSend => (0, 2),
            Op::ChanRecv | Op::ChanClose | Op::ChanTryRecv | Op::SharedLoad | Op::SharedAdd => {
                (1, 1)
            }
            Op::SelectWhen(n) => (2, (*n as i32) * 2),
            Op::ParEachBegin(_) => (0, 1),
            Op::Panic => (0, 1),
            _ => (0, 0),
        };
        sp -= pop;
        if sp < 0 {
            sp = 0;
        }
        sp += push;
        if sp > peak {
            peak = sp;
        }
    }
    (peak as usize).saturating_add(4)
}

/// Emit typed scalar aliases for locals with a known [`CTag`], syncing from `args` /
/// `locals` so SROA can promote them without `uint8_t*` byte-offset casts.
fn emit_boxed_scalar_views(out: &mut String, f: &FunctionChunk, locals: usize) {
    use crate::bytecode::CTag;
    for i in 0..locals {
        let tag = f.local_tags.get(i).copied().unwrap_or(CTag::Dyn);
        let (cty, acc) = match tag {
            CTag::I64 => ("int64_t", "i"),
            CTag::F64 => ("double", "f"),
            CTag::Bool => ("int", "b"),
            CTag::Str => ("const char *", "s"),
            _ => continue,
        };
        writeln!(
            out,
            "  {cty} sl_{i} = (nargs > {i}) ? args[{i}].as.{acc} : locals[{i}].as.{acc}; /* SROA */"
        )
        .unwrap();
    }
}

fn scalar_tag_accessor(
    tags: Option<&[crate::bytecode::CTag]>,
    slot: usize,
) -> Option<&'static str> {
    use crate::bytecode::CTag;
    match tags?.get(slot).copied()? {
        CTag::I64 => Some("i"),
        CTag::F64 => Some("f"),
        CTag::Bool => Some("b"),
        CTag::Str => Some("s"),
        _ => None,
    }
}

fn span_uses_iter(ops: &[Op]) -> bool {
    ops.iter().any(|op| {
        matches!(
            op,
            Op::IterBegin
                | Op::IterNext(_)
                | Op::IterPop
                | Op::PushIt
                | Op::PushIndex
                | Op::IterSetIt
        )
    })
}

/// PCs that a boxed body actually `goto`s. Straight-line ops must not get a label:
/// GCC `-Wall` reports every unused one, and a real program is tens of thousands of them.
fn boxed_targets(ops: &[Op], start: usize, end: usize) -> HashSet<usize> {
    let mut targets = HashSet::from([start]);
    let last = end.min(ops.len());
    for i in start..last {
        match &ops[i] {
            Op::Jump(dest) => {
                targets.insert(*dest as usize);
            }
            Op::JumpIfFalse(dest)
            | Op::JumpIfTrue(dest)
            | Op::JumpIfLt(dest)
            | Op::JumpIfGe(dest)
            | Op::JumpIfEq(dest)
            | Op::JumpIfNe(dest)
            | Op::IterNext(dest) => {
                targets.insert(*dest as usize);
                targets.insert(i + 1);
            }
            Op::LoopStep { body, .. } => {
                targets.insert(*body as usize);
                targets.insert(i + 1);
            }
            Op::Halt | Op::Nop | Op::ScopeEnter | Op::ScopeExit => {
                targets.insert(i + 1);
            }
            _ => {}
        }
    }
    targets
}

fn emit_op_stream(
    out: &mut String,
    chunk: &Chunk,
    interner: &Interner,
    start: usize,
    end: usize,
    indent: &str,
    writeback_params: bool,
    local_tags: Option<&[crate::bytecode::CTag]>,
) {
    // Boxed (heterogeneous / dynamic) fallback: one `SuperVal` stack machine, labels + gotos.
    // Everything monomorphic is lowered by `unboxed` instead.
    let targets = boxed_targets(&chunk.ops, start, end);
    writeln!(out, "{indent}goto L_{start};").unwrap();
    for i in start..end {
        if targets.contains(&i) {
            writeln!(out, "{indent}L_{i}:;").unwrap();
        }
        if let Some(note) = chunk.opt_notes.iter().find(|n| n.at == i as u32) {
            writeln!(out, "{indent}  /* opt {:?} : {} */", note.kind, note.detail).unwrap();
        }
        emit_op(
            out,
            chunk,
            interner,
            i,
            indent,
            writeback_params,
            local_tags,
        );
    }
    // A branch past the last op lands here. An unreferenced label is a warning, so
    // emit it only when something jumps to it; otherwise fall into the caller's return.
    // Branches past the last op (and module `Halt` after `+main`) must preserve
    // any value left on the operand stack — otherwise `return 1` from `+main`
    // becomes process exit 0.
    if targets.contains(&end) {
        writeln!(
            out,
            "{indent}L_{end}: return sp > 0 ? stack[sp - 1] : SUPER_NULL;"
        )
        .unwrap();
    }
}

fn emit_op(
    out: &mut String,
    chunk: &Chunk,
    interner: &Interner,
    i: usize,
    indent: &str,
    writeback_params: bool,
    local_tags: Option<&[crate::bytecode::CTag]>,
) {
    let ind = format!("{indent}    ");
    match &chunk.ops[i] {
        Op::LoadConst(c) => match chunk.consts.get(*c as usize) {
            Some(Value::Int(_)) => {
                writeln!(out, "{ind}stack[sp++] = super_int(SUPER_I64_{c}); ").unwrap();
            }
            Some(Value::Float(_)) => {
                writeln!(out, "{ind}stack[sp++] = super_float(SUPER_F64_{c}); ").unwrap();
            }
            Some(Value::Bool(_)) => {
                writeln!(out, "{ind}stack[sp++] = super_bool(SUPER_BOOL_{c}); ").unwrap();
            }
            Some(Value::Str(_)) => {
                writeln!(out, "{ind}stack[sp++] = super_str(SUPER_STR_{c}); ").unwrap();
            }
            Some(Value::Array(elems)) if elems.iter().all(|v| matches!(v, Value::Int(_))) => {
                // Dense int LUT (including empty) — materialize a growable i64 array copy.
                writeln!(
                    out,
                    "{ind}{{ SuperArr *a = super_arr_alloc(SUPER_LUT_{c}_LEN + 4); a->kind = 2; \
                     a->len = SUPER_LUT_{c}_LEN; a->cap = SUPER_LUT_{c}_LEN + 4; \
                     a->i64s = (int64_t *)sup_req_alloc(sizeof(int64_t) * a->cap, 8); \
                     for (size_t __k = 0; __k < SUPER_LUT_{c}_LEN; ++__k) a->i64s[__k] = SUPER_LUT_{c}[__k]; \
                     stack[sp++] = super_arr(a); }}"
                )
                .unwrap();
            }
            _ => {
                writeln!(out, "{ind}stack[sp++] = SUPER_NULL; ").unwrap();
            }
        },
        Op::LoadLocal(r) => {
            if let Some(acc) = scalar_tag_accessor(local_tags, *r as usize) {
                let stag = match acc {
                    "i" => "SUPER_TAG_INT",
                    "f" => "SUPER_TAG_FLOAT",
                    "b" => "SUPER_TAG_BOOL",
                    "s" => "SUPER_TAG_STR",
                    _ => "SUPER_TAG_NULL",
                };
                // Prefer the typed SROA view; keep `locals[r]` in sync for escape/call paths.
                writeln!(
                    out,
                    "{ind}{{ locals[{r}].tag = {stag}; locals[{r}].as.{acc} = sl_{r}; stack[sp++] = locals[{r}]; }}"
                )
                .unwrap();
            } else {
                writeln!(out, "{ind}stack[sp++] = locals[{r}]; ").unwrap();
            }
        }
        Op::StoreLocal(r) => {
            if let Some(acc) = scalar_tag_accessor(local_tags, *r as usize) {
                let stag = match acc {
                    "i" => "SUPER_TAG_INT",
                    "f" => "SUPER_TAG_FLOAT",
                    "b" => "SUPER_TAG_BOOL",
                    "s" => "SUPER_TAG_STR",
                    _ => "SUPER_TAG_NULL",
                };
                writeln!(
                    out,
                    "{ind}{{ locals[{r}] = stack[--sp]; locals[{r}].tag = {stag}; sl_{r} = locals[{r}].as.{acc}; }}"
                )
                .unwrap();
            } else {
                writeln!(out, "{ind}locals[{r}] = stack[--sp]; ").unwrap();
            }
        }
        Op::LoadGlobal(s) => {
            let gid = s.index();
            writeln!(out, "{ind}stack[sp++] = super_globals[{gid}]; ").unwrap();
        }
        Op::StoreGlobal(s) => {
            let gid = s.index();
            writeln!(out, "{ind}super_globals[{gid}] = stack[--sp]; ").unwrap();
        }
        Op::Move(dst, src) => {
            writeln!(out, "{ind}locals[{dst}] = locals[{src}]; ").unwrap();
            if let Some(acc) = scalar_tag_accessor(local_tags, *dst as usize) {
                if scalar_tag_accessor(local_tags, *src as usize).is_some() {
                    writeln!(out, "{ind}sl_{dst} = sl_{src}; ").unwrap();
                } else {
                    writeln!(out, "{ind}sl_{dst} = locals[{dst}].as.{acc}; ").unwrap();
                }
            }
        }
        Op::Add => bin_op(out, &ind, i, "super_add"),
        Op::Sub => bin_op(out, &ind, i, "super_sub"),
        Op::Mul => bin_op(out, &ind, i, "super_mul"),
        Op::Div => bin_op(out, &ind, i, "super_div"),
        Op::Rem => bin_op(out, &ind, i, "super_rem"),
        Op::Eq => bin_op(out, &ind, i, "super_eq"),
        Op::Ne => bin_op(out, &ind, i, "super_ne"),
        Op::Lt => bin_op(out, &ind, i, "super_lt"),
        Op::Le => bin_op(out, &ind, i, "super_le"),
        Op::Gt => bin_op(out, &ind, i, "super_gt"),
        Op::Ge => bin_op(out, &ind, i, "super_ge"),
        Op::Neg => {
            writeln!(out, "{ind}stack[sp-1] = super_neg(stack[sp-1]); ").unwrap();
        }
        Op::Not => {
            writeln!(
                out,
                "{ind}stack[sp-1] = super_bool(!super_truthy(stack[sp-1])); "
            )
            .unwrap();
        }
        Op::AddImm(n) => {
            writeln!(
                out,
                "{ind}stack[sp-1] = super_add(stack[sp-1], super_int({n}LL)); "
            )
            .unwrap();
        }
        Op::SubImm(n) => {
            writeln!(
                out,
                "{ind}stack[sp-1] = super_sub(stack[sp-1], super_int({n}LL)); "
            )
            .unwrap();
        }
        Op::MulImm(n) => {
            writeln!(
                out,
                "{ind}stack[sp-1] = super_mul(stack[sp-1], super_int({n}LL)); "
            )
            .unwrap();
        }
        Op::ShlImm(n) => {
            writeln!(
                out,
                "{ind}stack[sp-1] = super_int(stack[sp-1].as.i << {n}); "
            )
            .unwrap();
        }
        Op::And => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_bool(super_truthy(a) && super_truthy(b)); }}")
            .unwrap();
        }
        Op::OrLog => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_bool(super_truthy(a) || super_truthy(b)); }}")
            .unwrap();
        }
        Op::BitAnd => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int(super_as_i(a) & super_as_i(b)); }}")
            .unwrap();
        }
        Op::BitOr => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int(super_as_i(a) | super_as_i(b)); }}")
            .unwrap();
        }
        Op::BitXor => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int(super_as_i(a) ^ super_as_i(b)); }}")
            .unwrap();
        }
        Op::BitNot => {
            writeln!(
                out,
                "{ind}{{ SuperVal a = stack[--sp]; stack[sp++] = super_int(~super_as_i(a)); }}"
            )
            .unwrap();
        }
        Op::Shl => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int(super_as_i(a) << (super_as_i(b) & 63)); }}")
            .unwrap();
        }
        Op::Shr => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int(super_as_i(a) >> (super_as_i(b) & 63)); }}")
            .unwrap();
        }
        Op::UShr => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = super_int((int64_t)((uint64_t)super_as_i(a) >> (super_as_i(b) & 63))); }}")
            .unwrap();
        }
        Op::Jump(t) => {
            writeln!(out, "{ind}goto L_{t};").unwrap();
        }
        Op::JumpIfFalse(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal c = stack[--sp]; if (super_truthy(c)) goto L_{}; else goto L_{t}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::JumpIfTrue(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal c = stack[--sp]; if (super_truthy(c)) goto L_{t}; else goto L_{}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::LoopStep {
            counter,
            limit,
            body,
        } => {
            // Native C for-loop step: locals[counter]++, branch while < limit.
            writeln!(
                out,
                "{ind}locals[{counter}].as.i += 1; if (locals[{counter}].as.i < locals[{limit}].as.i) goto L_{body}; else goto L_{};",
                i + 1
            )
            .unwrap();
        }
        Op::MakeRange => {
            writeln!(
                out,
                "{ind}{{ SuperVal e = stack[--sp]; SuperVal s = stack[--sp]; stack[sp++] = super_range(s.as.i, e.as.i); }}")
            .unwrap();
        }
        Op::IterBegin => {
            writeln!(
                out,
                "{ind}{{ SuperVal coll = stack[--sp]; if (itp < 16) {{ iters[itp].arr = nullptr; if (coll.tag == SUPER_TAG_RANGE) {{ iters[itp].start = coll.as.range.start; iters[itp].end = coll.as.range.end; iters[itp].index = coll.as.range.start; }} else if (coll.tag == SUPER_TAG_ARR && coll.as.arr) {{ iters[itp].arr = coll.as.arr; iters[itp].start = 0; iters[itp].end = (int64_t)coll.as.arr->len; iters[itp].index = 0; }} else {{ iters[itp].start = 0; iters[itp].end = 0; iters[itp].index = 0; }} ++itp; }} }}")
            .unwrap();
        }
        Op::IterPop => {
            writeln!(out, "{ind}if (itp > 0) --itp; ").unwrap();
        }
        Op::IterNext(done) => {
            writeln!(
                out,
                "{ind}{{ if (itp <= 0 || iters[itp-1].index >= iters[itp-1].end) {{ if (itp > 0) --itp; goto L_{done}; }} else {{ goto L_{}; }} }}",
                i + 1
            )
            .unwrap();
        }
        Op::PushIt => {
            writeln!(
                out,
                "{ind}{{ SuperVal v = SUPER_NULL; if (itp > 0) {{ SuperIter *it = &iters[itp-1]; if (it->arr) {{ if (it->index >= 0 && (size_t)it->index < it->arr->len) {{ size_t __ix = (size_t)it->index; if (it->arr->kind == 1) v = super_float(it->arr->f64s[__ix]); else if (it->arr->kind == 2) v = super_int(it->arr->i64s[__ix]); else v = it->arr->items[__ix]; }} }} else {{ v = super_int(it->index); }} it->index += 1; }} stack[sp++] = v; }}")
            .unwrap();
        }
        Op::PushIndex => {
            writeln!(
                out,
                "{ind}{{ int64_t v = 0; if (itp > 0) {{ SuperIter *it = &iters[itp-1]; if (it->arr) v = it->index - 1; else {{ v = (it->index - 1) - it->start; if (v < 0) v = 0; }} }} stack[sp++] = super_int(v); }}")
            .unwrap();
        }
        Op::IterSetIt => {
            writeln!(
                out,
                "{ind}{{ SuperVal val = stack[--sp]; SuperVal outv = SUPER_NULL; if (itp > 0) {{ SuperIter *it = &iters[itp-1]; int64_t idx = it->index - 1; if (it->arr && idx >= 0 && (size_t)idx < it->arr->len) {{ size_t __ix = (size_t)idx; if (it->arr->kind == 1) it->arr->f64s[__ix] = (val.tag == SUPER_TAG_FLOAT) ? val.as.f : (double)val.as.i; else if (it->arr->kind == 2) it->arr->i64s[__ix] = (val.tag == SUPER_TAG_INT) ? val.as.i : (int64_t)((val.tag == SUPER_TAG_FLOAT) ? val.as.f : 0); else it->arr->items[__ix] = val; }} if (it->arr) outv = super_arr(it->arr); }} stack[sp++] = outv; }}")
            .unwrap();
        }
        Op::Call(fi, arity) => {
            let f = &chunk.functions[*fi as usize];
            let fname = function_c_name(chunk, interner, *fi as usize);
            let call = format!(
                "{{ SuperVal args[{arity}]; for (int k = {arity}-1; k >= 0; --k) args[k] = stack[--sp]; stack[sp++] = {fname}(args, {arity}); }}"
            );
            if f.always_inline {
                // Still emit the call; definition is always_inline so GCC can fold it.
                writeln!(out, "{ind}{call}").unwrap();
            } else {
                writeln!(out, "{ind}{call}").unwrap();
            }
        }
        Op::CallMut(fi, arity, wb_id) => {
            let fname = function_c_name(chunk, interner, *fi as usize);
            let wbs = chunk
                .mut_writebacks
                .get(*wb_id as usize)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            write!(
                out,
                "{ind}{{ SuperVal args[{arity}]; for (int k = {arity}-1; k >= 0; --k) args[k] = stack[--sp]; SuperVal ret = {fname}(args, {arity});"
            )
            .unwrap();
            for (slot, param_idx) in wbs {
                write!(out, " locals[{slot}] = args[{param_idx}];").unwrap();
            }
            writeln!(out, " stack[sp++] = ret; }}").unwrap();
        }
        // MakeFn / MakeClosure store the function index as SUPER_TAG_INT so CallValue
        // can switch to the matching boxed `super_fn_*` (closures drop captures for
        // AOT — capture-free lambdas and named fn values are the supported path).
        Op::MakeFn(fi) => {
            writeln!(
                out,
                "{ind}stack[sp++] = (SuperVal){{ .tag = SUPER_TAG_INT, .as.i = {fi} }}; /* MakeFn */"
            )
            .unwrap();
        }
        Op::MakeClosure(fi, n) => {
            writeln!(
                out,
                "{ind}for (int k = 0; k < {n}; ++k) --sp; stack[sp++] = (SuperVal){{ .tag = SUPER_TAG_INT, .as.i = {fi} }}; /* MakeClosure */"
            )
            .unwrap();
        }
        Op::CallValue(arity) => {
            // Only index functions that MakeFn/MakeClosure can produce (typically
            // lambdas kept on the boxed dyn path). Unboxed monomorphized fns are
            // never stored as first-class values.
            let mut callable = std::collections::BTreeSet::new();
            for op in &chunk.ops {
                match op {
                    Op::MakeFn(fi) | Op::MakeClosure(fi, _) => {
                        callable.insert(*fi as usize);
                    }
                    _ => {}
                }
            }
            writeln!(out, "{ind}{{").unwrap();
            writeln!(
                out,
                "{ind}  SuperVal __cv_args[{arity}]; for (int k = {arity}-1; k >= 0; --k) __cv_args[k] = stack[--sp];"
            )
            .unwrap();
            writeln!(out, "{ind}  SuperVal __cv = stack[--sp];").unwrap();
            writeln!(
                out,
                "{ind}  int64_t __fi = (__cv.tag == SUPER_TAG_INT) ? __cv.as.i : (int64_t)-1;"
            )
            .unwrap();
            writeln!(out, "{ind}  SuperVal __ret = SUPER_NULL;").unwrap();
            writeln!(out, "{ind}  switch (__fi) {{").unwrap();
            for fi in callable {
                let fname = function_c_name(chunk, interner, fi);
                writeln!(
                    out,
                    "{ind}    case {fi}: __ret = {fname}(__cv_args, {arity}); break;"
                )
                .unwrap();
            }
            writeln!(out, "{ind}    default: break;").unwrap();
            writeln!(out, "{ind}  }}").unwrap();
            writeln!(out, "{ind}  stack[sp++] = __ret;").unwrap();
            writeln!(out, "{ind}}}").unwrap();
        }
        Op::Return => {
            // Flush typed SROA views back into `locals` / `args` before leaving.
            if let Some(tags) = local_tags {
                use crate::bytecode::CTag;
                for (slot, tag) in tags.iter().enumerate() {
                    let (stag, acc) = match tag {
                        CTag::I64 => ("SUPER_TAG_INT", "i"),
                        CTag::F64 => ("SUPER_TAG_FLOAT", "f"),
                        CTag::Bool => ("SUPER_TAG_BOOL", "b"),
                        CTag::Str => ("SUPER_TAG_STR", "s"),
                        _ => continue,
                    };
                    writeln!(
                        out,
                        "{ind}locals[{slot}].tag = {stag}; locals[{slot}].as.{acc} = sl_{slot};"
                    )
                    .unwrap();
                }
            }
            if writeback_params {
                writeln!(
                    out,
                    "{ind}for (int wi = 0; wi < nargs; ++wi) args[wi] = locals[wi];"
                )
                .unwrap();
            }
            writeln!(out, "{ind}return sp > 0 ? stack[--sp] : SUPER_NULL;").unwrap();
        }
        Op::Pop => {
            writeln!(out, "{ind}--sp; ").unwrap();
        }
        Op::Dup => {
            writeln!(out, "{ind}stack[sp] = stack[sp-1]; ++sp; ").unwrap();
        }
        Op::Print => {
            writeln!(
                out,
                "{ind}super_print(stack[--sp]); stack[sp++] = SUPER_NULL; "
            )
            .unwrap();
        }
        Op::IsAbsent => {
            writeln!(
                out,
                "{ind}{{ SuperVal v = stack[sp-1]; int abs = (v.tag == SUPER_TAG_NULL || v.tag == SUPER_TAG_ERR); stack[sp++] = super_bool(abs); }}")
            .unwrap();
        }
        Op::OrElse => {
            writeln!(
                out,
                "{ind}{{ SuperVal v = stack[--sp]; if (v.tag == SUPER_TAG_NULL || v.tag == SUPER_TAG_ERR) stack[sp++] = SUPER_NULL; else stack[sp++] = v; }}")
            .unwrap();
        }
        Op::AssertEq => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (!super_eq(a,b).as.i) {{ fprintf(stderr, \"assert_eq failed\\n\"); abort(); }} }}")
            .unwrap();
        }
        Op::Halt => {
            // Module script: value of `+main` (if any) stays on the stack.
            writeln!(out, "{ind}return sp > 0 ? stack[sp - 1] : SUPER_NULL;").unwrap();
        }
        Op::Nop => {
            writeln!(out, "{ind}goto L_{};", i + 1).unwrap();
        }
        Op::CallHost(sym, arity) => {
            let raw = interner.resolve(*sym);
            let name = escape_c_string(raw);
            // Inline hot math hosts — avoid strcmp dispatch in numeric kernels.
            let bare = raw.strip_prefix("host.").unwrap_or(raw);
            let inline_math = match (bare, *arity) {
                ("sys_sqrt", 1) => Some("sqrt"),
                ("sys_floor", 1) => Some("floor"),
                ("sys_ceil", 1) => Some("ceil"),
                ("sys_abs", 1) => Some("fabs"),
                ("sys_sin", 1) => Some("sin"),
                ("sys_cos", 1) => Some("cos"),
                ("sys_tan", 1) => Some("tan"),
                ("sys_exp", 1) => Some("exp"),
                ("sys_ln" | "sys_log", 1) => Some("log"),
                _ => None,
            };
            if let Some(cfn) = inline_math {
                writeln!(
                    out,
                    "{ind}{{ SuperVal __a = stack[--sp]; stack[sp++] = (SuperVal){{ .tag = SUPER_TAG_FLOAT, .as.f = {cfn}((__a.tag == SUPER_TAG_FLOAT) ? __a.as.f : (double)__a.as.i) }}; }}"
                )
                .unwrap();
            } else if bare == "sys_to_int" && *arity == 1 {
                writeln!(
                    out,
                    "{ind}{{ SuperVal __a = stack[--sp]; stack[sp++] = (SuperVal){{ .tag = SUPER_TAG_INT, .as.i = (__a.tag == SUPER_TAG_FLOAT) ? (int64_t)__a.as.f : __a.as.i }}; }}"
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "{ind}{{ SuperVal args[{arity}]; for (int k = {arity}-1; k >= 0; --k) args[k] = stack[--sp]; stack[sp++] = super_host(\"{name}\", args, {arity}); }}"
                )
                .unwrap();
            }
        }
        Op::MakeArray(n) => {
            // Homogeneous primitive fills → dense stack arrays (never boxed SuperVal items).
            // n==0 must be a *boxed* growable array: empty loops make `__all_f/__all_i`
            // vacuously true, but `[]` is often later filled with nested arrays / mixed
            // values (sha256 merkle leaves). Dense i64 would coerce those to 0.
            if *n == 0 {
                writeln!(
                    out,
                    "{ind}{{ SuperArr *a = super_arr_alloc(4); a->len = 0; stack[sp++] = super_arr(a); }}"
                )
                .unwrap();
            } else {
                writeln!(
                    out,
                    "{ind}{{ SuperVal tmp[{n}]; for (int k = {n}-1; k >= 0; --k) tmp[k] = stack[--sp]; \
                     int __all_f = 1, __all_i = 1; \
                     for (int k = 0; k < {n}; ++k) {{ \
                       if (tmp[k].tag != SUPER_TAG_FLOAT) __all_f = 0; \
                       if (tmp[k].tag != SUPER_TAG_INT) __all_i = 0; \
                     }} \
                     SuperArr *a; \
                     if (__all_f) {{ \
                       a = super_arr_alloc_f64_stack({n}); \
                       for (int k = 0; k < {n}; ++k) a->f64s[k] = tmp[k].as.f; \
                     }} else if (__all_i) {{ \
                       a = super_arr_alloc_i64_stack({n}); \
                       for (int k = 0; k < {n}; ++k) a->i64s[k] = tmp[k].as.i; \
                     }} else {{ \
                       a = super_arr_alloc({n}); \
                       for (int k = 0; k < {n}; ++k) super_arr_push(a, tmp[k]); \
                     }} \
                     stack[sp++] = super_arr(a); }}"
                )
                .unwrap();
            }
        }
        Op::IndexGet => {
            // Prefer dense raw loads (no bounds shim) so GCC can auto-vectorize SoA loops.
            writeln!(
                out,
                "{ind}{{ SuperVal idx = stack[--sp]; SuperVal base = stack[--sp]; SuperArr *__a = base.as.arr; size_t __ix = (size_t)idx.as.i; if (__builtin_expect(base.tag == SUPER_TAG_ARR && __a && (__a->kind == 1 || __a->kind == 2), 1)) {{ if (__a->kind == 1) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_FLOAT, .as.f = __a->f64s[__ix] }}); else stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_INT, .as.i = __a->i64s[__ix] }}); }} else stack[sp++] = super_index_get(base, idx); }}"
            )
            .unwrap();
        }
        Op::LoadSliceUnchecked => {
            writeln!(
                out,
                "{ind}{{ SuperVal idx = stack[--sp]; SuperVal base = stack[--sp]; if (__builtin_expect(base.tag != SUPER_TAG_ARR, 0)) {{ stack[sp++] = super_index_get(base, idx); }} else {{ SuperArr *__a = base.as.arr; size_t __ix = (size_t)idx.as.i; if (__a->kind == 1) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_FLOAT, .as.f = __a->f64s[__ix] }}); else if (__a->kind == 2) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_INT, .as.i = __a->i64s[__ix] }}); else stack[sp++] = __a->items[__ix]; }} }}"
            )
            .unwrap();
        }
        Op::IndexSet => {
            writeln!(
                out,
                "{ind}{{ SuperVal val = stack[--sp]; SuperVal idx = stack[--sp]; SuperVal base = stack[--sp]; SuperArr *__a = base.as.arr; size_t __ix = (size_t)idx.as.i; if (__builtin_expect(base.tag == SUPER_TAG_ARR && __a && (__a->kind == 1 || __a->kind == 2), 1)) {{ if (__a->kind == 1) __a->f64s[__ix] = (val.tag == SUPER_TAG_FLOAT) ? val.as.f : (double)val.as.i; else __a->i64s[__ix] = (val.tag == SUPER_TAG_INT) ? val.as.i : (int64_t)((val.tag == SUPER_TAG_FLOAT) ? val.as.f : 0); }} else super_index_set(base, idx, val); stack[sp++] = base; }}"
            )
            .unwrap();
        }
        Op::StoreSliceUnchecked => {
            writeln!(
                out,
                "{ind}{{ SuperVal val = stack[--sp]; SuperVal idx = stack[--sp]; SuperVal base = stack[--sp]; if (__builtin_expect(base.tag != SUPER_TAG_ARR, 0)) {{ super_index_set(base, idx, val); }} else {{ SuperArr *__a = base.as.arr; size_t __ix = (size_t)idx.as.i; if (__a->kind == 1) __a->f64s[__ix] = (val.tag == SUPER_TAG_FLOAT) ? val.as.f : (double)val.as.i; else if (__a->kind == 2) __a->i64s[__ix] = (val.tag == SUPER_TAG_INT) ? val.as.i : (int64_t)((val.tag == SUPER_TAG_FLOAT) ? val.as.f : 0); else __a->items[__ix] = val; }} stack[sp++] = base; }}"
            )
            .unwrap();
        }
        Op::JumpIfLt(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (super_lt(a, b).as.b) goto L_{t}; else goto L_{}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::JumpIfGe(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (super_ge(a, b).as.b) goto L_{t}; else goto L_{}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::JumpIfEq(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (super_eq(a, b).as.b) goto L_{t}; else goto L_{}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::JumpIfNe(t) => {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (!super_eq(a, b).as.b) goto L_{t}; else goto L_{}; }}",
                i + 1
            )
            .unwrap();
        }
        Op::MakeStruct(_, _) | Op::MakeMap(_) => {
            // Stack: key0 val0 key1 val1 … (VM pops `val`, then `key`, `n` times).
            let (n, is_map, c_name) = match &chunk.ops[i] {
                Op::MakeMap(n) => (*n, 1u8, "NULL".to_owned()),
                Op::MakeStruct(Some(sym), n) => (
                    *n,
                    0u8,
                    format!("\"{}\"", escape_c_string(interner.resolve(*sym))),
                ),
                Op::MakeStruct(None, n) => (*n, 0u8, "NULL".to_owned()),
                _ => unreachable!(),
            };
            // Escape analysis → birth region (Stay=REQ, Promote/Conservative=DURABLE).
            let alloc = match chunk
                .escape_birth
                .iter()
                .find(|(pc, _)| *pc == i as u32)
                .map(|(_, r)| *r)
            {
                Some(1) => format!("super_obj_alloc_region({n}, {is_map}, {c_name}, (uint8_t)SUP_REG_REQ)"),
                Some(2) => format!("super_obj_alloc_durable({n}, {is_map}, {c_name})"),
                _ => format!("super_obj_alloc({n}, {is_map}, {c_name})"),
            };
            writeln!(
                out,
                "{ind}{{ SuperObj *o = {alloc}; for (int k = 0; k < {n}; ++k) {{ SuperVal v = stack[--sp]; SuperVal key = stack[--sp]; super_obj_put(o, key.tag == SUPER_TAG_STR && key.as.s ? key.as.s : \"\", v); }} stack[sp++] = super_obj_val(o); }}"
            )
            .unwrap();
        }
        Op::MakeSum {
            name,
            tag,
            n_fields,
        } => {
            let c_name = escape_c_string(interner.resolve(*name));
            let n = *n_fields as u32 + 1;
            let alloc = match chunk
                .escape_birth
                .iter()
                .find(|(pc, _)| *pc == i as u32)
                .map(|(_, r)| *r)
            {
                Some(1) => format!(
                    "super_obj_alloc_region({n}, 0, \"{c_name}\", (uint8_t)SUP_REG_REQ)"
                ),
                Some(2) => format!("super_obj_alloc_durable({n}, 0, \"{c_name}\")"),
                _ => format!("super_obj_alloc({n}, 0, \"{c_name}\")"),
            };
            writeln!(
                out,
                "{ind}{{ SuperObj *o = {alloc}; super_obj_put(o, \"__tag\", super_int({tag})); for (int k = 0; k < {n_fields}; ++k) {{ SuperVal v = stack[--sp]; SuperVal key = stack[--sp]; super_obj_put(o, key.tag == SUPER_TAG_STR && key.as.s ? key.as.s : \"\", v); }} stack[sp++] = super_obj_val(o); }}"
            )
            .unwrap();
        }
        Op::IsVariant(tag) => {
            writeln!(
                out,
                "{ind}{{ SuperVal v = stack[--sp]; int ok = 0; if (v.tag == SUPER_TAG_INT) ok = (v.as.i == {tag}); else if (v.tag == SUPER_TAG_OBJ) {{ SuperVal t = super_obj_get(v, \"__tag\"); ok = (t.tag == SUPER_TAG_INT && t.as.i == {tag}); }} stack[sp++] = super_bool(ok); }}"
            )
            .unwrap();
        }
        Op::SumTag => {
            writeln!(
                out,
                "{ind}{{ SuperVal v = stack[--sp]; int64_t tag = 0; if (v.tag == SUPER_TAG_INT) tag = v.as.i; else if (v.tag == SUPER_TAG_OBJ) {{ SuperVal t = super_obj_get(v, \"__tag\"); if (t.tag == SUPER_TAG_INT) tag = t.as.i; }} stack[sp++] = super_int(tag); }}"
            )
            .unwrap();
        }
        Op::GetField(f) => {
            let key = escape_c_string(interner.resolve(*f));
            writeln!(
                out,
                "{ind}{{ SuperVal base = stack[--sp]; stack[sp++] = super_obj_get(base, \"{key}\"); }}"
            )
            .unwrap();
        }
        Op::SetField(f) => {
            // Leaves the updated aggregate on the stack for the compiler's write-back store.
            let key = escape_c_string(interner.resolve(*f));
            writeln!(
                out,
                "{ind}{{ SuperVal v = stack[--sp]; SuperVal base = stack[--sp]; stack[sp++] = super_obj_with(base, \"{key}\", v); }}"
            )
            .unwrap();
        }
        Op::Fail => {
            // VM: pop the message, push `Value::Error`. `or` sees it as absent;
            // `or err` reads it back through `__is_err` / `__err_text`.
            writeln!(
                out,
                "{ind}{{ SuperVal msg = stack[--sp]; const char *s = \"\"; char *owned = NULL; \
                 if (msg.tag == SUPER_TAG_STR || msg.tag == SUPER_TAG_ERR) s = msg.as.s ? msg.as.s : \"\"; \
                 else {{ owned = super_show_alloc(msg, 0); s = owned ? owned : \"\"; }} \
                 stack[sp++] = super_err(s); }}"
            )
            .unwrap();
        }
        Op::Panic => {
            writeln!(
                out,
                "{ind}{{ SuperVal msg = stack[--sp]; const char *s = \"panic\"; char *owned = NULL; \
                 if (msg.tag == SUPER_TAG_STR || msg.tag == SUPER_TAG_ERR) s = msg.as.s ? msg.as.s : \"panic\"; \
                 else {{ owned = super_show_alloc(msg, 0); s = owned ? owned : \"panic\"; }} \
                 fprintf(stderr, \"panic: %s\\n\", s); super_safe_free(owned); abort(); }}"
            )
            .unwrap();
        }
        Op::ScopeEnter | Op::ScopeExit => {
            // Scopes only matter for concurrency, which the sequential C backend never starts.
            writeln!(out, "{ind}goto L_{};", i + 1).unwrap();
        }
        Op::Spawn(fi) => {
            // Sequential AOT: run the thunk now and leave its result on the stack.
            // `Await` is then identity (no Task tag in the C SuperVal ABI).
            let fname = function_c_name(chunk, interner, *fi as usize);
            writeln!(
                out,
                "{ind}{{ SuperVal __spawn_args[1]; stack[sp++] = {fname}(__spawn_args, 0); }}"
            )
            .unwrap();
        }
        Op::Await => {
            // Spawn already produced the value in sequential lowering.
            writeln!(out, "{ind}/* await (sequential AOT) */").unwrap();
        }
        other => {
            // Never skip silently: a dropped op corrupts the operand stack and yields a
            // plausible-looking wrong answer. Fail loudly, but only if the op is reached.
            let name = format!("{other:?}");
            let name = name.split(['(', ' ', '{']).next().unwrap_or("?");
            writeln!(out, "{ind}super_unsupported(\"{name}\");").unwrap();
        }
    }
}

fn bin_op(out: &mut String, ind: &str, i: usize, fname: &str) {
    let _ = i; // fall through to next label
    // Inline the common float×float / int×int paths to avoid a call in hot loops.
    let inline = match fname {
        "super_add" => Some(("+", false)),
        "super_sub" => Some(("-", false)),
        "super_mul" => Some(("*", false)),
        "super_div" => Some(("/", true)),
        _ => None,
    };
    if let Some((op, is_div)) = inline {
        if is_div {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (__builtin_expect(a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_FLOAT, 1)) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_FLOAT, .as.f = a.as.f {op} b.as.f }}); else if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_INT, .as.i = (b.as.i == 0 ? 0 : a.as.i {op} b.as.i) }}); else stack[sp++] = {fname}(a, b); }}"
            )
            .unwrap();
        } else {
            writeln!(
                out,
                "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; if (__builtin_expect(a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_FLOAT, 1)) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_FLOAT, .as.f = a.as.f {op} b.as.f }}); else if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT) stack[sp++] = ((SuperVal){{ .tag = SUPER_TAG_INT, .as.i = a.as.i {op} b.as.i }}); else stack[sp++] = {fname}(a, b); }}"
            )
            .unwrap();
        }
    } else {
        writeln!(
            out,
            "{ind}{{ SuperVal b = stack[--sp]; SuperVal a = stack[--sp]; stack[sp++] = {fname}(a, b); }}"
        )
        .unwrap();
    }
}

/// Args array is `restrict`; `@` params additionally expose `T * restrict` views in the body.
fn typed_param_restrict(_chunk: &Chunk, _interner: &Interner, _fi: usize) -> &'static str {
    "restrict "
}

pub(super) fn function_c_name(chunk: &Chunk, interner: &Interner, fi: usize) -> String {
    let f = &chunk.functions[fi];
    let name = sanitize_ident(interner.resolve(f.name));
    let owner = f
        .owner
        .map(|o| format!("{}_", sanitize_ident(interner.resolve(o))))
        .unwrap_or_default();
    // Deterministic FuncFamily mangling: `mod__fn__T1__T2_{fi}` when param tags known.
    // The trailing `_{fi}` preserves uniqueness across same-tag specializations.
    if !f.param_tags.is_empty()
        && f.param_tags
            .iter()
            .any(|t| !matches!(t, crate::bytecode::CTag::Dyn))
    {
        let tags: Vec<&str> = f.param_tags.iter().map(|t| t.name()).collect();
        return format!("mod__{owner}{name}__{}_{fi}", tags.join("__"));
    }
    format!("super_fn_{owner}{name}_{fi}")
}

/// Super name → C identifier. Keywords in [`crate::c_keywords::C_KEYWORDS`] gain a trailing `_`.
fn sanitize_ident(s: &str) -> String {
    crate::c_keywords::c_ident(s)
}

pub(super) fn escape_c_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn emit_font8() -> String {
    const FONT: &[u8] = include_bytes!("font8x8.bin");
    let mut out = String::with_capacity(FONT.len() * 6 + 128);
    out.push_str("/* Public-domain 8x8 bitmap, U+0000..U+007F. The least significant bit is the leftmost pixel. */\n");
    out.push_str("static const uint8_t SUPER_FONT8[128][8] = {\n");
    for row in FONT.chunks(8) {
        out.push_str("  {");
        for (i, b) in row.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("0x{b:02x}"));
        }
        out.push_str("},\n");
    }
    out.push_str("};\n");
    out
}

const RUNTIME_PRELUDE: &str = r#"
/* Fat string matching Super C-ABI `str` (ptr + len = 16 bytes). */
typedef struct {
  const char *ptr;
  int64_t len;
} SuperStr;

typedef struct {
  uint8_t *restrict base;
  size_t capacity;
  size_t cursor;
} SupStack;

static inline void *sup_stack_alloc(SupStack *restrict st, size_t size, size_t align) {
  if (!st || !st->base) return nullptr;
  align = align < 1 ? 1 : align;
  size_t cur = st->cursor;
  size_t aligned = (cur + align - 1) & ~(align - 1);
  size_t next = aligned + size;
  if (next > st->capacity) return nullptr;
  st->cursor = next;
  return (void *)(st->base + aligned);
}

static inline void sup_stack_reset(SupStack *restrict st) {
  if (st) st->cursor = 0;
}

typedef enum {
  SUPER_TAG_NULL = 0,
  SUPER_TAG_BOOL,
  SUPER_TAG_INT,
  SUPER_TAG_FLOAT,
  SUPER_TAG_STR,
  SUPER_TAG_RANGE,
  SUPER_TAG_ARR,
  SUPER_TAG_OBJ, /* struct / map: ordered (key, value) pairs */
  SUPER_TAG_ERR  /* `fail` payload; absent to `or`, distinct from `none` */
} SuperTag;

typedef struct SuperArr {
  struct SuperVal *items; /* tagged elements (NULL when dense) */
  double *f64s;           /* dense f64 payload when kind==1 */
  int64_t *i64s;          /* dense i64 payload when kind==2 */
  size_t len;
  size_t cap;
  int64_t rc;
  int kind; /* 0 = SuperVal items, 1 = dense f64s, 2 = dense i64s */
} SuperArr;

typedef struct SuperObj SuperObj;

typedef struct SuperVal {
  SuperTag tag;
  union {
    int64_t i;
    double f;
    int b;
    const char *s;
    struct { int64_t start; int64_t end; } range;
    SuperArr *arr;
    SuperObj *obj;
  } as;
} SuperVal;

/* Struct / map payload. Keys and values live in one allocation sized at construction;
   objects are immutable once built (value semantics): `super_obj_with` copies on write,
   mirroring the VM where `SetField` rewrites the aggregate and the compiler stores it back.
   `region`: SUP_REG_DURABLE (calloc) or SUP_REG_REQ (request bump). Frame/stack values
   never use this header. See docs/MEMORY_MODEL.md. */
enum { SUP_REG_DURABLE = 0, SUP_REG_REQ = 1, SUP_REG_CONN = 2 };
struct SuperObj {
  uint32_t n;
  uint32_t is_map;
  uint8_t region;
  const char *name; /* NULL for maps and anonymous records */
  const char **keys;
  SuperVal *vals;
};

typedef struct {
  int64_t start;
  int64_t end;
  int64_t index; /* next value to yield */
  SuperArr *arr; /* when iterating an array */
} SuperIter;

static const SuperVal SUPER_NULL = { .tag = SUPER_TAG_NULL, .as = { .i = 0 } };
static int super_argc = 0;
static char **super_argv = 0;

static inline SuperVal super_err(const char *s) {
  SuperVal r = { .tag = SUPER_TAG_ERR, .as = { .s = s ? s : "" } };
  return r;
}

static inline SuperVal super_int(int64_t v) [[unsequenced]] {
  SuperVal r = { .tag = SUPER_TAG_INT, .as = { .i = v } };
  return r;
}
static inline SuperVal super_float(double v) [[unsequenced]] {
  SuperVal r = { .tag = SUPER_TAG_FLOAT, .as = { .f = v } };
  return r;
}
static inline SuperVal super_bool(int v) [[unsequenced]] {
  SuperVal r = { .tag = SUPER_TAG_BOOL, .as = { .b = v ? 1 : 0 } };
  return r;
}
static inline SuperVal super_str(const char *v) [[unsequenced]] {
  SuperVal r = { .tag = SUPER_TAG_STR, .as = { .s = v } };
  return r;
}
/* Request-scoped bump arena for boxed request lifetimes (HTTP App.serve). */
typedef struct {
  uint8_t *base; size_t cur, cap; int64_t depth; bool ready;
  /* Diagnostics for alloc_probe / soak (reset on enter). */
  uint64_t arena_bytes;   /* bytes bumped this request */
  uint64_t promoted_objs; /* objects deep-copied to durable at promote */
  uint64_t stay_objs;     /* SuperObj born in the arena this request */
} SupReqArena;
static _Thread_local SupReqArena sup_req;
/* Connection-scoped bump arena: Conn + peer/buf born at accept live here and are
   bulk-reclaimed at connection close (MADV_DONTNEED). Nested under request arenas
   so keep-alive revive can keep_str into the conn arena instead of durable malloc.
   See docs/MEMORY_MODEL.md "Conn copy-on-write" / connection churn. */
typedef struct {
  uint8_t *base; size_t cur, cap; int64_t depth; bool ready;
  uint64_t arena_bytes;      /* bytes currently bumped */
  uint64_t reclaimed_bytes;  /* bytes rewound on the last leave */
  uint64_t enter_count;
  uint64_t leave_count;
} SupConnArena;
static _Thread_local SupConnArena sup_conn;
/* Set SUPER_PROMOTE_EVERYTHING=1 to force durable alloc even inside a request
   (differential escape-analysis oracle). */
static inline int sup_promote_everything(void) {
  const char *e = getenv("SUPER_PROMOTE_EVERYTHING");
  return e && e[0] == '1';
}
#ifndef MAP_NORESERVE
#define MAP_NORESERVE 0
#endif
static void sup_req_init(void) {
  if (sup_req.ready) return;
  sup_req.ready = true;
#if defined(__linux__) || defined(__APPLE__) || defined(__FreeBSD__)
  for (size_t cap = (size_t)1 << (sizeof(void *) == 8 ? 34 : 28); cap >= ((size_t)16 << 20); cap >>= 2) {
    void *p = mmap(nullptr, cap, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    if (p == MAP_FAILED) continue;
#if defined(MADV_NOHUGEPAGE)
    (void)madvise(p, (size_t)2 << 20, MADV_NOHUGEPAGE);
#endif
    sup_req.base = (uint8_t *)p; sup_req.cap = cap; return;
  }
#endif
}
static void sup_conn_init(void) {
  if (sup_conn.ready) return;
  sup_conn.ready = true;
#if defined(__linux__) || defined(__APPLE__) || defined(__FreeBSD__)
  /* 64 MiB is plenty for one Conn + peer strings + keep_str copies. */
  size_t cap = (size_t)64 << 20;
  void *p = mmap(nullptr, cap, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
  if (p != MAP_FAILED) {
#if defined(MADV_NOHUGEPAGE)
    (void)madvise(p, (size_t)2 << 20, MADV_NOHUGEPAGE);
#endif
    sup_conn.base = (uint8_t *)p; sup_conn.cap = cap;
  }
#endif
}
#define SUP_CONN_NESTED ((size_t)-1)
static inline int sup_conn_active(void) { return sup_conn.depth > 0 && sup_conn.base != nullptr; }
static inline int sup_conn_contains(const void *p) {
  uintptr_t b = (uintptr_t)sup_conn.base, q = (uintptr_t)p;
  return b && q >= b && q < b + sup_conn.cap;
}
static inline void *sup_conn_alloc(size_t n, size_t align) {
  if (!sup_conn_active()) return nullptr;
  size_t at = (sup_conn.cur + (align - 1)) & ~(align - 1);
  if (at > sup_conn.cap || n > sup_conn.cap - at) return nullptr;
  void *p = sup_conn.base + at;
  memset(p, 0, n ? n : 1);
  size_t used = (n ? n : 1);
  sup_conn.cur = at + used;
  sup_conn.arena_bytes += used;
  return p;
}
static inline size_t sup_conn_enter(void) {
  if (!sup_conn.ready) sup_conn_init();
  if (sup_conn.depth > 0) return SUP_CONN_NESTED;
  if (!sup_conn.base) return SUP_CONN_NESTED; /* no mmap: stay on malloc */
  /* One connection at a time on this thread (Server.serve is sync). */
  if (sup_conn.cur != 0) {
    /* Previous connection forgot leave — reclaim before reuse. */
    size_t old = sup_conn.cur;
    sup_conn.cur = 0;
    sup_conn.reclaimed_bytes = old;
#if defined(MADV_DONTNEED)
    if (old > 0) {
      (void)madvise(sup_conn.base, old, MADV_DONTNEED);
    }
#endif
  }
  sup_conn.depth = 1;
  sup_conn.arena_bytes = 0;
  sup_conn.enter_count += 1;
  return sup_conn.cur;
}
static inline int64_t sup_conn_leave(size_t mark) {
  if (mark == SUP_CONN_NESTED) return 0;
  if (sup_conn.depth <= 0) return 0;
  size_t old = sup_conn.cur;
  size_t from_mark = mark;
  if (from_mark > old) from_mark = 0;
  size_t reclaimed = old > from_mark ? old - from_mark : 0;
  sup_conn.depth = 0;
  sup_conn.cur = from_mark;
  sup_conn.reclaimed_bytes = reclaimed;
  sup_conn.arena_bytes = from_mark;
  sup_conn.leave_count += 1;
#if defined(MADV_DONTNEED)
  if (sup_conn.base && reclaimed > 0) {
    size_t page = (size_t)4096;
    size_t from = (from_mark + page - 1) & ~(page - 1);
    if (from < old) (void)madvise(sup_conn.base + from, old - from, MADV_DONTNEED);
  }
#endif
  {
    const char *tr = getenv("SUPER_CONN_TRACE");
    if (tr && tr[0] == '1') {
      fprintf(stderr,
              "super: conn-arena leave reclaimed_bytes=%zu enter_count=%llu leave_count=%llu\n",
              reclaimed,
              (unsigned long long)sup_conn.enter_count,
              (unsigned long long)sup_conn.leave_count);
    }
  }
  return (int64_t)reclaimed;
}
#define SUP_REQ_NESTED ((size_t)-1)
static inline size_t sup_req_enter(void) {
  if (!sup_req.ready) sup_req_init();
  if (sup_req.depth > 0) return SUP_REQ_NESTED;
  if (!sup_req.base) return SUP_REQ_NESTED; /* no mmap: stay on malloc */
  sup_req.depth = 1;
  /* Nested enter is rejected above; top-level enter clears per-request counters. */
  if (sup_req.cur == 0) {
    /* keep counters across keep-alive only for the current mark window */
  }
  return sup_req.cur;
}
static inline int sup_req_active(void) { return sup_req.depth > 0 && sup_req.base != nullptr; }
static inline int sup_req_contains(const void *p) {
  uintptr_t b = (uintptr_t)sup_req.base, q = (uintptr_t)p;
  return b && q >= b && q < b + sup_req.cap;
}
static inline void *sup_req_alloc(size_t n, size_t align) {
  if (sup_req_active()) {
    size_t at = (sup_req.cur + (align - 1)) & ~(align - 1);
    if (at <= sup_req.cap && n <= sup_req.cap - at) {
      void *p = sup_req.base + at;
      memset(p, 0, n ? n : 1);
      size_t used = (n ? n : 1);
      sup_req.cur = at + used;
      sup_req.arena_bytes += used;
      return p;
    }
  }
  /* Outside a request but inside a connection: birth into the conn arena. */
  if (sup_conn_active()) {
    void *p = sup_conn_alloc(n, align);
    if (p) return p;
  }
  void *p = malloc(n ? n : 1);
  if (!p) abort();
  return p;
}
static inline void *sup_req_calloc(size_t nmemb, size_t sz) {
  size_t n = nmemb * sz;
  if (!n) n = 1;
  if (sup_req_active()) return sup_req_alloc(n, 8);
  if (sup_conn_active()) {
    void *p = sup_conn_alloc(n, 8);
    if (p) return p;
  }
  void *p = calloc(nmemb ? nmemb : 1, sz ? sz : 1);
  if (!p) abort();
  return p;
}
static inline void sup_req_after_leave(void); /* defined after g_sup_stack */
static inline void sup_req_leave(size_t mark) {
  if (mark == SUP_REQ_NESTED) return;
  size_t old = sup_req.cur;
  sup_req.depth = 0;
  if (old != mark) {
    sup_req.cur = mark;
#if defined(MADV_DONTNEED)
    if (sup_req.base && old > mark) {
      size_t page = (size_t)4096;
      size_t from = (mark + page - 1) & ~(page - 1);
      if (from < old) (void)madvise(sup_req.base + from, old - from, MADV_DONTNEED);
    }
#endif
  }
  sup_req_after_leave();
}
/* Copy `s` out of the request arena. Prefer the connection arena so keep-alive
   strings die with the Conn; fall back to durable malloc when no conn is active. */
static inline char *sup_req_keep_str(const char *s) {
  if (!s) return nullptr;
  if (!sup_req_contains(s)) return (char *)s;
  size_t n = strlen(s);
  char *o = nullptr;
  if (sup_conn_active()) o = (char *)sup_conn_alloc(n + 1, 1);
  if (!o) {
    o = (char *)malloc(n + 1);
    if (!o) abort();
  }
  memcpy(o, s, n + 1);
  return o;
}
static inline SuperArr *super_arr_alloc(size_t cap) {
  SuperArr *a = (SuperArr *)sup_req_calloc(1, sizeof(SuperArr));
  if (!a) return nullptr;
  a->cap = cap < 4 ? 4 : cap;
  a->items = (SuperVal *)sup_req_calloc(a->cap, sizeof(SuperVal));
  a->len = 0;
  a->rc = 1;
  a->kind = 0;
  return a;
}
static inline SuperArr *super_arr_alloc_f64(size_t n) {
  SuperArr *a = (SuperArr *)sup_req_calloc(1, sizeof(SuperArr));
  if (!a) return nullptr;
  a->cap = n < 4 ? 4 : n;
  a->f64s = (double *)sup_req_calloc(a->cap, sizeof(double));
  a->len = n;
  a->rc = 1;
  a->kind = 1;
  return a;
}
static inline SuperArr *super_arr_alloc_i64(size_t n) {
  SuperArr *a = (SuperArr *)sup_req_calloc(1, sizeof(SuperArr));
  if (!a) return nullptr;
  a->cap = n < 4 ? 4 : n;
  a->i64s = (int64_t *)sup_req_calloc(a->cap, sizeof(int64_t));
  a->len = n;
  a->rc = 1;
  a->kind = 2;
  return a;
}
/* Scoped data stack: demand-paged anonymous mmap (not a 64 MiB BSS reservation).
   Capacity stays 64 MiB for large kernels; untouched pages never enter RSS. The head is
   opted out of transparent huge pages: under THP=always the first touch of a 2 MiB-aligned
   mapping would fault in a whole 2 MiB page for a few KiB of scratch. Deeper use keeps THP. */
enum { SUP_STACK_CAP = 64u << 20, SUP_STACK_SMALL_PAGES = 2u << 20 };
static uint8_t *g_sup_stack_mem = nullptr;
static SupStack g_sup_stack = { nullptr, 0, 0 };
static inline void sup_req_after_leave(void) {
  /* Frame stack scratch from typed paths must not accumulate across requests.
     Flat RSS relies on arena rewind + MADV_DONTNEED (glibc heap trim removed). */
  if (g_sup_stack.base) sup_stack_reset(&g_sup_stack);
}
static inline void sup_stack_init(void) {
  if (g_sup_stack.base) return;
#if defined(__linux__) || defined(__APPLE__) || defined(__FreeBSD__)
  void *p = mmap(nullptr, SUP_STACK_CAP, PROT_READ | PROT_WRITE,
                 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  if (p == MAP_FAILED) p = nullptr;
#if defined(MADV_NOHUGEPAGE)
  if (p) (void)madvise(p, SUP_STACK_SMALL_PAGES, MADV_NOHUGEPAGE);
#endif
#else
  void *p = malloc(SUP_STACK_CAP);
#endif
  g_sup_stack_mem = (uint8_t *)p;
  g_sup_stack.base = g_sup_stack_mem;
  g_sup_stack.capacity = p ? (size_t)SUP_STACK_CAP : 0;
  g_sup_stack.cursor = 0;
}
static inline SuperArr *super_arr_alloc_i64_stack(size_t n) {
  sup_stack_init();
  SuperArr *a = (SuperArr *)sup_stack_alloc(&g_sup_stack, sizeof(SuperArr), _Alignof(SuperArr));
  int64_t *data = (int64_t *)sup_stack_alloc(&g_sup_stack, (n < 1 ? 1 : n) * sizeof(int64_t), 64);
  if (!a || !data) return super_arr_alloc_i64(n);
  memset(a, 0, sizeof(*a));
  memset(data, 0, n * sizeof(int64_t));
  a->i64s = data;
  a->len = n;
  a->cap = n < 1 ? 1 : n;
  a->rc = 1;
  a->kind = 2;
  return a;
}
static inline SuperArr *super_arr_alloc_f64_stack(size_t n) {
  sup_stack_init();
  SuperArr *a = (SuperArr *)sup_stack_alloc(&g_sup_stack, sizeof(SuperArr), _Alignof(SuperArr));
  /* 64-byte align for AVX/AVX-512 dense kernels. */
  double *data = (double *)sup_stack_alloc(&g_sup_stack, (n < 1 ? 1 : n) * sizeof(double), 64);
  if (!a || !data) return super_arr_alloc_f64(n);
  memset(a, 0, sizeof(*a));
  memset(data, 0, n * sizeof(double));
  a->f64s = data;
  a->len = n;
  a->cap = n < 1 ? 1 : n;
  a->rc = 1;
  a->kind = 1;
  return a;
}
static inline SuperVal super_arr(SuperArr *a) {
  SuperVal r = { .tag = SUPER_TAG_ARR, .as = { .arr = a } };
  return r;
}
static inline SuperObj *super_obj_alloc_region(uint32_t n, uint32_t is_map, const char *name, uint8_t region) {
  /* Escape analysis (docs/MEMORY_MODEL.md): inside a request, non-escaping
     objects Stay in the bump arena. Escaping roots are deep-promoted before
     `sup_req_leave`. SUPER_PROMOTE_EVERYTHING=1 forces durable birth (oracle).
     SUP_REG_CONN births live until `sup_conn_leave` (connection close). */
  if (sup_promote_everything()) region = (uint8_t)SUP_REG_DURABLE;
  if (region == (uint8_t)SUP_REG_REQ && !sup_req_active()) {
    region = sup_conn_active() ? (uint8_t)SUP_REG_CONN : (uint8_t)SUP_REG_DURABLE;
  }
  if (region == (uint8_t)SUP_REG_CONN && !sup_conn_active()) region = (uint8_t)SUP_REG_DURABLE;
  size_t bytes = sizeof(SuperObj) + (size_t)n * (sizeof(const char *) + sizeof(SuperVal));
  if (!bytes) bytes = sizeof(SuperObj);
  SuperObj *o;
  if (region == (uint8_t)SUP_REG_REQ) {
    o = (SuperObj *)sup_req_alloc(bytes, 8);
    if (o) sup_req.stay_objs += 1;
  } else if (region == (uint8_t)SUP_REG_CONN) {
    o = (SuperObj *)sup_conn_alloc(bytes, 8);
    if (!o) {
      /* Conn arena exhausted — fall back to durable calloc (still correct, leaks until process exit). */
      region = (uint8_t)SUP_REG_DURABLE;
      o = (SuperObj *)calloc(1, bytes);
    }
  } else {
    if (getenv("SUPER_TRACE_COW") && getenv("SUPER_TRACE_COW")[0] == '1') {
      fprintf(stderr, "DURABLE_ALLOC n=%u bytes=%zu name=%s\n", n, bytes, name ? name : "(null)");
    }
    o = (SuperObj *)calloc(1, bytes);
  }
  if (!o) { fprintf(stderr, "super: out of memory\n"); abort(); }
  if (region != (uint8_t)SUP_REG_REQ && region != (uint8_t)SUP_REG_CONN) memset(o, 0, bytes);
  o->n = 0;
  o->is_map = is_map;
  o->region = region;
  o->name = name;
  o->keys = (const char **)(o + 1);
  o->vals = (SuperVal *)(o->keys + n);
  return o;
}
static inline SuperObj *super_obj_alloc(uint32_t n, uint32_t is_map, const char *name) {
  uint8_t region;
  if (sup_req_active() && !sup_promote_everything()) region = (uint8_t)SUP_REG_REQ;
  else if (sup_conn_active() && !sup_promote_everything()) region = (uint8_t)SUP_REG_CONN;
  else region = (uint8_t)SUP_REG_DURABLE;
  return super_obj_alloc_region(n, is_map, name, region);
}
static inline SuperObj *super_obj_alloc_durable(uint32_t n, uint32_t is_map, const char *name) {
  return super_obj_alloc_region(n, is_map, name, (uint8_t)SUP_REG_DURABLE);
}
static inline int super_obj_find(const SuperObj *o, const char *key) {
  for (uint32_t i = 0; i < o->n; ++i) {
    const char *k = o->keys[i];
    if (k == key || strcmp(k, key) == 0) return (int)i;
  }
  return -1;
}
/* Builder-side insert/overwrite; the caller guarantees capacity. */
static inline void super_obj_put(SuperObj *o, const char *key, SuperVal v) {
  int i = super_obj_find(o, key);
  if (i >= 0) { o->vals[i] = v; return; }
  o->keys[o->n] = key;
  o->vals[o->n] = v;
  o->n += 1;
}
static inline SuperVal super_obj_val(SuperObj *o) {
  SuperVal r = { .tag = SUPER_TAG_OBJ, .as = { .obj = o } };
  return r;
}
static inline SuperVal super_obj_get(SuperVal base, const char *key) {
  if (base.tag != SUPER_TAG_OBJ || !base.as.obj) return SUPER_NULL;
  int i = super_obj_find(base.as.obj, key);
  return i >= 0 ? base.as.obj->vals[i] : SUPER_NULL;
}
/* Field store. Request-arena objects keep value-semantics COW. Durable objects
   with an existing key are updated in place for immediate / already-durable
   payloads so `App.served` / `App.clock` do not leak a durable App per request.
   Storing an arena obj/arr into a durable object falls through to COW + promote. */
static inline SuperVal super_obj_with(SuperVal base, const char *key, SuperVal v) {
  if (base.tag != SUPER_TAG_OBJ || !base.as.obj) return base;
  SuperObj *src = base.as.obj;
  int hit = super_obj_find(src, key);
  int v_escapes_req = 0;
  if (v.tag == SUPER_TAG_OBJ && v.as.obj
      && (v.as.obj->region == (uint8_t)SUP_REG_REQ || (sup_req_active() && sup_req_contains(v.as.obj))))
    v_escapes_req = 1;
  if (v.tag == SUPER_TAG_ARR && v.as.arr && sup_req_active() && sup_req_contains(v.as.arr))
    v_escapes_req = 1;
  /* In-place durable updates for existing keys with non-escaping payloads.
     Required for flat App.clock/served / Conn RSS. Multi-route 404s were caused
     by sys_arr_new living on the frame stack (fixed separately), not by in-place. */
  if (hit >= 0
      && (src->region == (uint8_t)SUP_REG_DURABLE || src->region == (uint8_t)SUP_REG_CONN)
      && !sup_req_contains(src) && !v_escapes_req) {
    src->vals[hit] = v;
    return base;
  }
  if (getenv("SUPER_TRACE_COW") && getenv("SUPER_TRACE_COW")[0] == '1' && src->n >= 12) {
    fprintf(stderr, "COW n=%u region=%u hit=%d vesc=%d name=%s key=%s tag=%d\n",
            src->n, (unsigned)src->region, hit, v_escapes_req,
            src->name ? src->name : "(null)", key ? key : "", (int)v.tag);
  }
  SuperObj *o = super_obj_alloc(src->n + (hit < 0 ? 1u : 0u), src->is_map, src->name);
  for (uint32_t i = 0; i < src->n; ++i) { o->keys[i] = src->keys[i]; o->vals[i] = src->vals[i]; }
  o->n = src->n;
  super_obj_put(o, key, v);
  return super_obj_val(o);
}
/* Promote memo: identity map old arena ptr -> durable copy (cycle / sharing safe). */
enum { SUP_PROMOTE_MEMO = 256 };
typedef struct { const void *from; void *to; } SupPromotePair;
typedef struct { SupPromotePair pairs[SUP_PROMOTE_MEMO]; int n; } SupPromoteMemo;
static inline void *sup_promote_lookup(SupPromoteMemo *m, const void *from) {
  for (int i = 0; i < m->n; ++i) if (m->pairs[i].from == from) return m->pairs[i].to;
  return nullptr;
}
static inline void sup_promote_remember(SupPromoteMemo *m, const void *from, void *to) {
  if (m->n < SUP_PROMOTE_MEMO) { m->pairs[m->n].from = from; m->pairs[m->n].to = to; m->n += 1; }
}
static inline SuperVal super_promote_val(SuperVal v, SupPromoteMemo *memo);
static inline SuperArr *super_promote_arr(SuperArr *a, SupPromoteMemo *memo) {
  if (!a) return nullptr;
  if (!sup_req_contains(a)) return a;
  SuperArr *hit = (SuperArr *)sup_promote_lookup(memo, a);
  if (hit) return hit;
  SuperArr *n = (SuperArr *)calloc(1, sizeof(SuperArr));
  if (!n) abort();
  *n = *a;
  n->rc = 1;
  sup_promote_remember(memo, a, n);
  if (a->kind == 1 && a->f64s) {
    n->f64s = (double *)calloc(a->cap ? a->cap : 1, sizeof(double));
    if (!n->f64s) abort();
    if (a->len) memcpy(n->f64s, a->f64s, a->len * sizeof(double));
  } else if (a->kind == 2 && a->i64s) {
    n->i64s = (int64_t *)calloc(a->cap ? a->cap : 1, sizeof(int64_t));
    if (!n->i64s) abort();
    if (a->len) memcpy(n->i64s, a->i64s, a->len * sizeof(int64_t));
  } else if (a->items) {
    n->items = (SuperVal *)calloc(a->cap ? a->cap : 1, sizeof(SuperVal));
    if (!n->items) abort();
    for (size_t i = 0; i < a->len; ++i) n->items[i] = super_promote_val(a->items[i], memo);
  }
  return n;
}
static inline SuperObj *super_promote_obj(SuperObj *o, SupPromoteMemo *memo) {
  if (!o) return nullptr;
  /* Durable and connection-scoped objects already survive `sup_req_leave`. */
  if (o->region == (uint8_t)SUP_REG_DURABLE || o->region == (uint8_t)SUP_REG_CONN) return o;
  if (o->region != (uint8_t)SUP_REG_REQ && !sup_req_contains(o)) return o;
  SuperObj *hit = (SuperObj *)sup_promote_lookup(memo, o);
  if (hit) return hit;
  SuperObj *n = sup_conn_active()
      ? super_obj_alloc_region(o->n ? o->n : 1u, o->is_map, o->name, (uint8_t)SUP_REG_CONN)
      : super_obj_alloc_durable(o->n ? o->n : 1u, o->is_map, o->name);
  n->n = 0;
  sup_promote_remember(memo, o, n);
  for (uint32_t i = 0; i < o->n; ++i) {
    const char *k = o->keys[i];
    if (sup_req_contains(k)) k = sup_req_keep_str(k);
    super_obj_put(n, k, super_promote_val(o->vals[i], memo));
  }
  if (sup_req_active()) sup_req.promoted_objs += 1;
  return n;
}
static inline SuperVal super_promote_val(SuperVal v, SupPromoteMemo *memo) {
  if (v.tag == SUPER_TAG_STR && v.as.s && sup_req_contains(v.as.s)) {
    return super_str(sup_req_keep_str(v.as.s));
  }
  if (v.tag == SUPER_TAG_OBJ && v.as.obj) {
    return super_obj_val(super_promote_obj(v.as.obj, memo));
  }
  if (v.tag == SUPER_TAG_ARR && v.as.arr) {
    return super_arr(super_promote_arr(v.as.arr, memo));
  }
  if (v.tag == SUPER_TAG_ERR && v.as.s && sup_req_contains(v.as.s)) {
    return super_err(sup_req_keep_str(v.as.s));
  }
  return v;
}
/* Deep-promote a value that must cross the request boundary into the durable heap. */
static inline SuperVal super_req_promote(SuperVal v) {
  if (!sup_req_active() && !(v.tag == SUPER_TAG_OBJ && v.as.obj && v.as.obj->region == (uint8_t)SUP_REG_REQ)
      && !(v.tag == SUPER_TAG_STR && v.as.s && sup_req_contains(v.as.s))
      && !(v.tag == SUPER_TAG_ARR && v.as.arr && sup_req_contains(v.as.arr))) {
    return v;
  }
  SupPromoteMemo memo = {0};
  return super_promote_val(v, &memo);
}
[[noreturn]] static void super_unsupported(const char *op) {
  fprintf(stderr, "super: bytecode op `%s` is not supported by the C23 AOT backend\n", op);
  abort();
}

static inline char *super_strdup(const char *s);
/* Desktop GUI. Widget documents (`gui\t1`) become native controls. Any other
   script is a canvas in the same window. Linux uses GTK 4 and tries Wayland
   before X11. Windows uses Win32. macOS uses Cocoa. `SUPER_GUI=gtk|wayland|x11|
   win32|cocoa` picks a provider. `SUPER_GUI=0|headless|quiet|ppm` stays
   off-screen. `SUPER_GUI=dump` prints the script. `SUPER_GUI=ppm` writes a pixmap. */
static int g_gui_w = 0, g_gui_h = 0;
static char *g_gui_script = 0;
static char *g_gui_shown = 0;
static uint32_t *g_gui_px = 0;
static int g_gui_quit = 0, g_gui_mx = 0, g_gui_my = 0, g_gui_down = 0;
static char *g_gui_keys[64];
static int g_gui_kn = 0, g_gui_kr = 0;
static int g_gui_live = 0, g_gui_told = 0;

static int super_gui_mode(void) {
  const char *m = getenv("SUPER_GUI");
  if (!m || !m[0]) return 0;
  if (strcmp(m, "1") == 0 || strcmp(m, "native") == 0 || strcmp(m, "window") == 0 ||
      strcmp(m, "gtk") == 0 || strcmp(m, "wayland") == 0 || strcmp(m, "x11") == 0 ||
      strcmp(m, "win32") == 0 || strcmp(m, "cocoa") == 0 || strcmp(m, "macos") == 0)
    return 1;
  if (strcmp(m, "dump") == 0) return 2;
  return 3;
}
static int super_gui_wants_window(void) {
  int mode = super_gui_mode();
  if (mode == 1) return 1;
  if (mode != 0) return 0;
  const char *d = getenv("DISPLAY");
  if (d && d[0]) return 1;
  d = getenv("WAYLAND_DISPLAY");
  return d && d[0];
}
static void super_gui_miss(const char *why) {
  if (g_gui_told || super_gui_mode() == 3) return;
  fprintf(stderr, "gui: %s\n", why);
  g_gui_told = 1;
}

static uint32_t super_gui_pack(int r, int g, int b) {
  if (r < 0) r = 0;
  if (r > 255) r = 255;
  if (g < 0) g = 0;
  if (g > 255) g = 255;
  if (b < 0) b = 0;
  if (b > 255) b = 255;
  return ((uint32_t)r << 16) | ((uint32_t)g << 8) | (uint32_t)b;
}
static void super_gui_put(int x, int y, uint32_t c) {
  if (!g_gui_px || x < 0 || y < 0 || x >= g_gui_w || y >= g_gui_h) return;
  g_gui_px[(size_t)y * (size_t)g_gui_w + (size_t)x] = c;
}
static void super_gui_resize(int w, int h) {
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  if (w > 4096) w = 4096;
  if (h > 4096) h = 4096;
  if (w == g_gui_w && h == g_gui_h && g_gui_px) return;
  size_t n = (size_t)w * (size_t)h;
  uint32_t *px = (uint32_t *)malloc(n * sizeof(uint32_t));
  if (!px) return;
  uint32_t bg = super_gui_pack(18, 18, 24);
  for (size_t i = 0; i < n; i++) px[i] = bg;
  free(g_gui_px);
  g_gui_px = px;
  g_gui_w = w;
  g_gui_h = h;
}
static int super_gui_ints(const char *s, int *out, int n) {
  int got = 0;
  while (*s && got < n) {
    while (*s == ' ' || *s == '\t') s++;
    if (!*s || *s == '|') break;
    char *end = 0;
    long v = strtol(s, &end, 10);
    if (end == s) break;
    out[got++] = (int)v;
    s = end;
  }
  return got;
}
static void super_gui_fill(int x, int y, int w, int h, uint32_t c) {
  for (int yy = y; yy < y + h; yy++)
    for (int xx = x; xx < x + w; xx++)
      super_gui_put(xx, yy, c);
}
static void super_gui_stroke(int x, int y, int w, int h, uint32_t c) {
  if (w <= 0 || h <= 0) return;
  for (int xx = x; xx < x + w; xx++) {
    super_gui_put(xx, y, c);
    super_gui_put(xx, y + h - 1, c);
  }
  for (int yy = y; yy < y + h; yy++) {
    super_gui_put(x, yy, c);
    super_gui_put(x + w - 1, yy, c);
  }
}
static void super_gui_text(int x, int y, uint32_t color, const char *text) {
  if (!text) return;
  const int scale = 2;
  const int advance = 8 * scale;
  for (int i = 0; text[i]; i++) {
    unsigned char ch = (unsigned char)text[i];
    if (ch >= 128) ch = (unsigned char)'?';
    const uint8_t *glyph = SUPER_FONT8[ch];
    int origin = x + i * advance;
    for (int row = 0; row < 8; row++) {
      uint8_t bits = glyph[row];
      for (int col = 0; col < 8; col++) {
        if ((bits & (uint8_t)(1u << col)) == 0) continue;
        int px0 = origin + col * scale;
        int py0 = y + row * scale;
        for (int dy = 0; dy < scale; dy++)
          for (int dx = 0; dx < scale; dx++)
            super_gui_put(px0 + dx, py0 + dy, color);
      }
    }
  }
}
static void super_gui_raster(const char *script) {
  if (!g_gui_px) return;
  uint32_t bg = super_gui_pack(18, 18, 24);
  size_t n = (size_t)g_gui_w * (size_t)g_gui_h;
  for (size_t i = 0; i < n; i++) g_gui_px[i] = bg;
  const char *p = script ? script : "";
  while (*p) {
    const char *nl = strchr(p, '\n');
    size_t len = nl ? (size_t)(nl - p) : strlen(p);
    char line[512];
    if (len >= sizeof(line)) len = sizeof(line) - 1;
    memcpy(line, p, len);
    line[len] = 0;
    char *s = line;
    while (*s == ' ' || *s == '\t') s++;
    int nb[8];
    if (strncmp(s, "fill ", 5) == 0 && super_gui_ints(s + 5, nb, 7) == 7)
      super_gui_fill(nb[0], nb[1], nb[2], nb[3], super_gui_pack(nb[4], nb[5], nb[6]));
    else if (strncmp(s, "rect ", 5) == 0 && super_gui_ints(s + 5, nb, 7) == 7)
      super_gui_stroke(nb[0], nb[1], nb[2], nb[3], super_gui_pack(nb[4], nb[5], nb[6]));
    else if (strncmp(s, "text ", 5) == 0) {
      char *bar = strchr(s, '|');
      if (bar && super_gui_ints(s + 5, nb, 5) == 5)
        super_gui_text(nb[0], nb[1], super_gui_pack(nb[2], nb[3], nb[4]), bar + 1);
    } else if (strncmp(s, "sprite ", 7) == 0) {
      int x = 0, y = 0, w = 0, h = 0, frame = 0;
      char name[64];
      name[0] = 0;
      if (sscanf(s + 7, "%d %d %d %d %63s %d", &x, &y, &w, &h, name, &frame) >= 5) {
        int tint = 80 + ((unsigned char)name[0] * 3) % 160;
        super_gui_fill(x, y, w, h, super_gui_pack(tint, 120, 160));
        super_gui_text(x, y, super_gui_pack(255, 255, 255), name);
      }
    }
    p = nl ? nl + 1 : p + strlen(p);
  }
}
static void super_gui_show(const char *script) {
  if (super_gui_mode() != 2) return;
  if (!script) script = "";
  if (g_gui_shown && strcmp(g_gui_shown, script) == 0) return;
  fputs(script, stdout);
  size_t n = strlen(script);
  if (n == 0 || script[n - 1] != '\n') fputc('\n', stdout);
  fflush(stdout);
  free(g_gui_shown);
  g_gui_shown = super_strdup(script);
}

typedef unsigned long SuperXID;
struct SuperXApi {
  void *(*OpenDisplay)(const char *);
  int (*CloseDisplay)(void *);
  int (*DefaultScreen)(void *);
  SuperXID (*DefaultRootWindow)(void *);
  SuperXID (*BlackPixel)(void *, int);
  void *(*DefaultVisual)(void *, int);
  int (*DefaultDepth)(void *, int);
  SuperXID (*CreateSimpleWindow)(void *, SuperXID, int, int, unsigned, unsigned, unsigned, SuperXID, SuperXID);
  int (*StoreName)(void *, SuperXID, const char *);
  int (*SelectInput)(void *, SuperXID, long);
  int (*MapWindow)(void *, SuperXID);
  int (*DestroyWindow)(void *, SuperXID);
  SuperXID (*InternAtom)(void *, const char *, int);
  int (*SetWMProtocols)(void *, SuperXID, SuperXID *, int);
  SuperXID (*CreateGC)(void *, SuperXID, unsigned long, void *);
  int (*FreeGC)(void *, SuperXID);
  void *(*CreateImage)(void *, void *, unsigned, int, int, char *, unsigned, unsigned, int, int);
  int (*PutImage)(void *, SuperXID, SuperXID, void *, int, int, int, int, unsigned, unsigned);
  int (*PutPixel)(void *, int, int, unsigned long);
  int (*DestroyImage)(void *);
  int (*Flush)(void *);
  int (*Pending)(void *);
  int (*NextEvent)(void *, void *);
  int (*LookupString)(void *, char *, int, unsigned long *, void *);
};
static void *g_xlib = 0;
static struct SuperXApi g_x;
static void *g_xdpy = 0;
static void *g_xvisual = 0;
static SuperXID g_xwin = 0, g_xgc = 0, g_xdel = 0, g_xproto = 0;
static int g_xdepth = 24, g_xrs = 16, g_xgs = 8, g_xbs = 0;

static int super_x_shift(unsigned long mask) {
  int s = 0;
  if (!mask) return 0;
  while ((mask & 1ul) == 0ul) { mask >>= 1; s++; }
  return s;
}
static int super_x_load(void) {
  if (g_xlib && g_x.OpenDisplay) return 1;
  if (!g_xlib) g_xlib = dlopen("libX11.so.6", RTLD_LAZY | RTLD_LOCAL);
  if (!g_xlib) g_xlib = dlopen("libX11.so", RTLD_LAZY | RTLD_LOCAL);
  if (!g_xlib) return 0;
  int missing = 0;
#define SUPER_XSYM(field, name) do { \
    void *sym = dlsym(g_xlib, name); \
    if (!sym) missing = 1; \
    memcpy(&g_x.field, &sym, sizeof g_x.field); \
  } while (0)
  SUPER_XSYM(OpenDisplay, "XOpenDisplay");
  SUPER_XSYM(CloseDisplay, "XCloseDisplay");
  SUPER_XSYM(DefaultScreen, "XDefaultScreen");
  SUPER_XSYM(DefaultRootWindow, "XDefaultRootWindow");
  SUPER_XSYM(BlackPixel, "XBlackPixel");
  SUPER_XSYM(DefaultVisual, "XDefaultVisual");
  SUPER_XSYM(DefaultDepth, "XDefaultDepth");
  SUPER_XSYM(CreateSimpleWindow, "XCreateSimpleWindow");
  SUPER_XSYM(StoreName, "XStoreName");
  SUPER_XSYM(SelectInput, "XSelectInput");
  SUPER_XSYM(MapWindow, "XMapWindow");
  SUPER_XSYM(DestroyWindow, "XDestroyWindow");
  SUPER_XSYM(InternAtom, "XInternAtom");
  SUPER_XSYM(SetWMProtocols, "XSetWMProtocols");
  SUPER_XSYM(CreateGC, "XCreateGC");
  SUPER_XSYM(FreeGC, "XFreeGC");
  SUPER_XSYM(CreateImage, "XCreateImage");
  SUPER_XSYM(PutImage, "XPutImage");
  SUPER_XSYM(PutPixel, "XPutPixel");
  SUPER_XSYM(DestroyImage, "XDestroyImage");
  SUPER_XSYM(Flush, "XFlush");
  SUPER_XSYM(Pending, "XPending");
  SUPER_XSYM(NextEvent, "XNextEvent");
  SUPER_XSYM(LookupString, "XLookupString");
#undef SUPER_XSYM
  return missing ? 0 : 1;
}
static void super_gui_push_key(const char *k) {
  if (!k || g_gui_kn >= 64) return;
  g_gui_keys[g_gui_kn++] = super_strdup(k);
}
static void super_x_pump(void) {
  if (!g_gui_live || !g_xdpy || !g_x.Pending) return;
  while (g_x.Pending(g_xdpy) > 0) {
    _Alignas(8) unsigned char ev[256];
    memset(ev, 0, sizeof ev);
    g_x.NextEvent(g_xdpy, ev);
    int typ = 0;
    memcpy(&typ, ev, sizeof typ);
    if (typ == 2) {
      char buf[32];
      unsigned long sym = 0;
      char compose[32];
      memset(buf, 0, sizeof buf);
      memset(compose, 0, sizeof compose);
      int n = g_x.LookupString(ev, buf, (int)sizeof buf - 1, &sym, compose);
      if (n < 0) n = 0;
      buf[n] = 0;
      if (sym == 0xff1b) super_gui_push_key("esc");
      else if (sym == 0xff0d || sym == 0xff8d) super_gui_push_key("enter");
      else if (sym == 0xff08) super_gui_push_key("backspace");
      else if (sym == 0xff09) super_gui_push_key("tab");
      else if (sym == 0xff51) super_gui_push_key("left");
      else if (sym == 0xff52) super_gui_push_key("up");
      else if (sym == 0xff53) super_gui_push_key("right");
      else if (sym == 0xff54) super_gui_push_key("down");
      else if (n == 1 && (unsigned char)buf[0] >= 32) {
        char one[2] = { buf[0], 0 };
        super_gui_push_key(one);
      }
    } else if (typ == 4 || typ == 5 || typ == 6) {
      int x = 0, y = 0;
      memcpy(&x, ev + 64, sizeof x);
      memcpy(&y, ev + 68, sizeof y);
      g_gui_mx = x;
      g_gui_my = y;
      if (typ == 4 || typ == 5) {
        unsigned int button = 0;
        memcpy(&button, ev + 84, sizeof button);
        if (button == 1 || typ == 5) g_gui_down = typ == 4 ? 1 : 0;
      }
    } else if (typ == 17) {
      g_gui_quit = 1;
    } else if (typ == 33) {
      unsigned long mtype = 0;
      long first = 0;
      memcpy(&mtype, ev + 40, sizeof mtype);
      memcpy(&first, ev + 56, sizeof first);
      if (mtype == g_xproto && first == (long)g_xdel) g_gui_quit = 1;
    }
  }
}
static void super_x_close(void) {
  if (!g_xdpy) {
    g_gui_live = 0;
    return;
  }
  if (g_xgc && g_x.FreeGC) g_x.FreeGC(g_xdpy, g_xgc);
  if (g_xwin && g_x.DestroyWindow) g_x.DestroyWindow(g_xdpy, g_xwin);
  if (g_x.CloseDisplay) g_x.CloseDisplay(g_xdpy);
  g_xdpy = 0;
  g_xvisual = 0;
  g_xwin = 0;
  g_xgc = 0;
  g_xdel = 0;
  g_xproto = 0;
  g_gui_live = 0;
}
static void super_x_blit(void) {
  if (!g_gui_live || !g_xdpy || !g_gui_px || !g_x.CreateImage) return;
  void *img = g_x.CreateImage(g_xdpy, g_xvisual, (unsigned)g_xdepth, 2, 0, 0,
                              (unsigned)g_gui_w, (unsigned)g_gui_h, 32, 0);
  if (!img) return;
  int bpl = *(int *)((char *)img + 44);
  if (bpl < g_gui_w * 4) bpl = g_gui_w * 4;
  char *data = (char *)calloc((size_t)bpl, (size_t)g_gui_h);
  if (!data) {
    g_x.DestroyImage(img);
    return;
  }
  *(char **)((char *)img + 16)  = data;
  for (int y = 0; y < g_gui_h; y++) {
    for (int x = 0; x < g_gui_w; x++) {
      uint32_t c = g_gui_px[(size_t)y * (size_t)g_gui_w + (size_t)x];
      unsigned long p = ((unsigned long)((c >> 16) & 255u) << g_xrs)
                      | ((unsigned long)((c >> 8) & 255u) << g_xgs)
                      | ((unsigned long)(c & 255u) << g_xbs);
      g_x.PutPixel(img, x, y, p);
    }
  }
  g_x.PutImage(g_xdpy, g_xwin, g_xgc, img, 0, 0, 0, 0, (unsigned)g_gui_w, (unsigned)g_gui_h);
  g_x.DestroyImage(img);
  g_x.Flush(g_xdpy);
  super_x_pump();
}
static int super_x_open(int w, int h, const char *title) {
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  super_gui_resize(w, h);
  if (!title || !title[0]) title = "Super";
  if (g_gui_live && g_xdpy && g_xwin) {
    if (g_x.StoreName) g_x.StoreName(g_xdpy, g_xwin, title);
    return 1;
  }
  if (!super_gui_wants_window()) {
    if (super_gui_mode() == 0) super_gui_miss("no display, window not opened");
    return 0;
  }
  if (!super_x_load()) {
    super_gui_miss("libX11 is not available, window not opened");
    return 0;
  }
  g_xdpy = g_x.OpenDisplay(0);
  if (!g_xdpy) {
    super_gui_miss("could not open the display");
    return 0;
  }
  int screen = g_x.DefaultScreen(g_xdpy);
  g_xvisual = g_x.DefaultVisual(g_xdpy, screen);
  g_xdepth = g_x.DefaultDepth(g_xdpy, screen);
  if (g_xvisual) {
    unsigned long rm = *(unsigned long *)((char *)g_xvisual + 24);
    unsigned long gm = *(unsigned long *)((char *)g_xvisual + 32);
    unsigned long bm = *(unsigned long *)((char *)g_xvisual + 40);
    g_xrs = super_x_shift(rm);
    g_xgs = super_x_shift(gm);
    g_xbs = super_x_shift(bm);
  }
  SuperXID root = g_x.DefaultRootWindow(g_xdpy);
  SuperXID black = g_x.BlackPixel(g_xdpy, screen);
  g_xwin = g_x.CreateSimpleWindow(g_xdpy, root, 80, 80, (unsigned)w, (unsigned)h, 0, black, black);
  if (!g_xwin) {
    super_x_close();
    super_gui_miss("could not create a window");
    return 0;
  }
  g_x.StoreName(g_xdpy, g_xwin, title);
  /* KeyPress, ButtonPress/Release, PointerMotion, Exposure, StructureNotify. */
  g_x.SelectInput(g_xdpy, g_xwin, 1L | 4L | 8L | 64L | 32768L | 131072L);
  g_xproto = g_x.InternAtom(g_xdpy, "WM_PROTOCOLS", 0);
  g_xdel = g_x.InternAtom(g_xdpy, "WM_DELETE_WINDOW", 0);
  g_x.SetWMProtocols(g_xdpy, g_xwin, &g_xdel, 1);
  g_xgc = g_x.CreateGC(g_xdpy, g_xwin, 0, 0);
  g_x.MapWindow(g_xdpy, g_xwin);
  g_x.Flush(g_xdpy);
  g_gui_live = 1;
  g_gui_quit = 0;
  return 1;
}
static void super_gui_pull(void);
static void super_gui_present(int w, int h, const char *script) {
  if (!script) script = "";
  super_gui_resize(w, h);
  free(g_gui_script);
  g_gui_script = super_strdup(script);
  super_gui_raster(g_gui_script ? g_gui_script : "");
  super_gui_show(g_gui_script ? g_gui_script : "");
  if (g_gui_live) {
    super_provider_present(g_gui_w, g_gui_h, g_gui_script ? g_gui_script : "", g_gui_px, g_gui_w, g_gui_h);
    super_gui_pull();
  }
  const char *mode = getenv("SUPER_GUI");
  if (mode && strcmp(mode, "ppm") == 0 && g_gui_px) {
    const char *path = getenv("SUPER_GUI_OUT");
    if (!path || !path[0]) path = "super-gui.ppm";
    FILE *fp = fopen(path, "wb");
    if (fp) {
      fprintf(fp, "P6\n%d %d\n255\n", g_gui_w, g_gui_h);
      size_t n = (size_t)g_gui_w * (size_t)g_gui_h;
      for (size_t i = 0; i < n; i++) {
        uint32_t c = g_gui_px[i];
        unsigned char rgb[3] = { (unsigned char)(c >> 16), (unsigned char)(c >> 8), (unsigned char)c };
        fwrite(rgb, 1, 3, fp);
      }
      fclose(fp);
    }
  }
}
static void super_gui_pull(void) {
  char buf[64];
  int x = 0, y = 0, down = 0;
  if (!super_provider_live()) return;
  if (super_provider_pump()) g_gui_quit = 1;
  super_provider_mouse(&x, &y, &down);
  g_gui_mx = x;
  g_gui_my = y;
  g_gui_down = down;
  while (super_provider_take_key(buf, (int)sizeof buf) > 0 && g_gui_kn < 64) {
    g_gui_keys[g_gui_kn++] = super_strdup(buf);
  }
}
static int super_gui_open_provider(int w, int h, const char *title) {
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  super_gui_resize(w, h);
  if (!title || !title[0]) title = "Super";
  if (super_provider_live()) {
    g_gui_live = 1;
    g_gui_quit = 0;
    return super_provider_open(w, h, title);
  }
  if (!super_gui_wants_window()) {
    if (super_gui_mode() == 0) super_gui_miss("no display, window not opened");
    return 0;
  }
  g_gui_live = super_provider_open(w, h, title);
  if (!g_gui_live) super_gui_miss("GTK4 is not available, window not opened");
  return g_gui_live;
}
static SuperVal super_gui_window(int w, int h, const char *title) {
  if (w < 1) w = 1;
  if (h < 1) h = 1;
  int live = super_gui_open_provider(w, h, title);
  SuperObj *o = super_obj_alloc(4, 0, NULL);
  super_obj_put(o, "width", super_int(w));
  super_obj_put(o, "height", super_int(h));
  super_obj_put(o, "title", super_str(title ? title : ""));
  super_obj_put(o, "headless", super_bool(live ? 0 : 1));
  return super_obj_val(o);
}
static SuperVal super_gui_poll(void) {
  super_gui_pull();
  SuperObj *o = super_obj_alloc(4, 0, NULL);
  super_obj_put(o, "quit", super_bool(g_gui_quit));
  SuperArr *keys = super_arr_alloc(0);
  super_obj_put(o, "keys", super_arr(keys));
  super_obj_put(o, "mouse_x", super_float((double)g_gui_mx));
  super_obj_put(o, "mouse_y", super_float((double)g_gui_my));
  return super_obj_val(o);
}

#if defined(_WIN32) || defined(__wasm__) || defined(__wasi__)
static int g_term_raw = 0;
static char g_term_feed[4096];
static int g_term_fn = 0, g_term_fr = 0;
static char *g_term_written = 0;
static size_t g_term_wn = 0, g_term_wc = 0;
static int super_term_raw(int on) { (void)on; g_term_raw = 0; return 0; }
static void super_term_capture(const char *s, size_t n) {
  if (g_term_wn + n + 1 > g_term_wc) {
    size_t nc = g_term_wc ? g_term_wc * 2 : 256;
    while (nc < g_term_wn + n + 1) nc *= 2;
    char *p = (char *)realloc(g_term_written, nc);
    if (!p) return;
    g_term_written = p;
    g_term_wc = nc;
  }
  if (n) memcpy(g_term_written + g_term_wn, s, n);
  g_term_wn += n;
  g_term_written[g_term_wn] = 0;
}
static void super_term_reset(void) {
  g_term_wn = 0;
  if (g_term_written) g_term_written[0] = 0;
}
static SuperVal super_term_read(int64_t max) {
  (void)max;
  return super_str("");
}
#else
static struct termios g_term_saved;
static int g_term_raw = 0, g_term_hook = 0;
static char g_term_feed[4096];
static int g_term_fn = 0, g_term_fr = 0;
static char *g_term_written = 0;
static size_t g_term_wn = 0, g_term_wc = 0;
static void super_term_restore(void) {
  if (g_term_raw) tcsetattr(STDIN_FILENO, TCSANOW, &g_term_saved);
  g_term_raw = 0;
}
static void super_term_on_signal(int sig) {
  super_term_restore();
  signal(sig, SIG_DFL);
  raise(sig);
}
static int super_term_raw(int on) {
  if (!isatty(STDIN_FILENO)) return 0;
  if (!g_term_hook) {
    atexit(super_term_restore);
    signal(SIGINT, super_term_on_signal);
    signal(SIGTERM, super_term_on_signal);
    g_term_hook = 1;
  }
  if (on) {
    if (g_term_raw) return 1;
    if (tcgetattr(STDIN_FILENO, &g_term_saved) != 0) return 0;
    struct termios t = g_term_saved;
    t.c_lflag &= ~(tcflag_t)(ICANON | ECHO | ISIG);
    t.c_cc[VMIN] = 0;
    t.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSANOW, &t) != 0) return 0;
    g_term_raw = 1;
    return 1;
  }
  super_term_restore();
  return 1;
}
static void super_term_capture(const char *s, size_t n) {
  if (g_term_wn + n + 1 > g_term_wc) {
    size_t nc = g_term_wc ? g_term_wc * 2 : 256;
    while (nc < g_term_wn + n + 1) nc *= 2;
    char *p = (char *)realloc(g_term_written, nc);
    if (!p) return;
    g_term_written = p;
    g_term_wc = nc;
  }
  if (n) memcpy(g_term_written + g_term_wn, s, n);
  g_term_wn += n;
  g_term_written[g_term_wn] = 0;
}
static void super_term_reset(void) {
  super_term_restore();
  g_term_fn = 0;
  g_term_fr = 0;
  g_term_wn = 0;
  if (g_term_written) g_term_written[0] = 0;
}
static SuperVal super_term_read(int64_t max) {
  if (max < 1) max = 1;
  if (max > 4096) max = 4096;
  char *out = (char *)malloc((size_t)max + 1);
  if (!out) return super_str("");
  int got = 0;
  while (got < max && g_term_fr < g_term_fn) out[got++] = g_term_feed[g_term_fr++];
  if (g_term_fr >= g_term_fn) g_term_fr = g_term_fn = 0;
  if (got < max && g_term_raw) {
    struct pollfd pfd;
    pfd.fd = STDIN_FILENO;
    pfd.events = POLLIN;
    pfd.revents = 0;
    if (poll(&pfd, 1, 0) > 0) {
      char buf[256];
      ssize_t n = read(STDIN_FILENO, buf, sizeof buf);
      if (n > 0) {
        int i = 0;
        while (got < max && i < n) out[got++] = buf[i++];
        while (i < n && g_term_fn < (int)sizeof g_term_feed) g_term_feed[g_term_fn++] = buf[i++];
      }
    }
  }
  out[got] = 0;
  return super_str(out);
}
#endif
static inline int super_ptr_in_stack(const void *p) [[reproducible]] {
  const uint8_t *b = (const uint8_t *)p;
  if (!g_sup_stack_mem || !g_sup_stack.capacity) return 0;
  return b >= g_sup_stack_mem && b < g_sup_stack_mem + g_sup_stack.capacity;
}
static inline void *sup_grow_buf(void *p, size_t old_bytes, size_t new_bytes) {
  /* Never realloc mmap arena / goose-stack pointers; copy into request bump or heap. */
  if (sup_req_contains(p) || super_ptr_in_stack(p) || sup_req_active()) {
    void *ni = sup_req_alloc(new_bytes ? new_bytes : 1, 8);
    if (p && old_bytes) memcpy(ni, p, old_bytes < new_bytes ? old_bytes : new_bytes);
    return ni;
  }
  void *ni = realloc(p, new_bytes ? new_bytes : 1);
  return ni;
}
static inline void super_safe_free(void *p) {
  if (!p) return;
  if (super_ptr_in_stack(p)) return;
  if (sup_req_contains(p)) {
    /* Arena pointers are never individually freed (MEMORY_MODEL).
       With SUPER_DEBUG_REGIONS=1 the process aborts so leaks of free() calls
       surface in sanitizer / debug runs; otherwise skip (bulk reclaim at leave). */
    if (getenv("SUPER_DEBUG_REGIONS") && getenv("SUPER_DEBUG_REGIONS")[0] == '1') {
      fprintf(stderr, "super: invalid free of request-arena pointer %p\n", p);
      abort();
    }
    return;
  }
  free(p);
}
static inline void super_arr_push(SuperArr *a, SuperVal v) {
  if (!a) return;
  if (a->kind == 1) {
    if (a->len >= a->cap) {
      size_t ncap = a->cap ? a->cap * 2 : 8;
      double *ni = (double *)sup_grow_buf(a->f64s, a->len * sizeof(double), ncap * sizeof(double));
      if (!ni) return;
      a->f64s = ni;
      a->cap = ncap;
    }
    a->f64s[a->len++] = (v.tag == SUPER_TAG_FLOAT) ? v.as.f : (double)v.as.i;
    return;
  }
  if (a->kind == 2) {
    if (a->len >= a->cap) {
      size_t ncap = a->cap ? a->cap * 2 : 8;
      int64_t *ni = (int64_t *)sup_grow_buf(a->i64s, a->len * sizeof(int64_t), ncap * sizeof(int64_t));
      if (!ni) return;
      a->i64s = ni;
      a->cap = ncap;
    }
    a->i64s[a->len++] = (v.tag == SUPER_TAG_INT) ? v.as.i : (int64_t)((v.tag == SUPER_TAG_FLOAT) ? v.as.f : 0);
    return;
  }
  if (a->len >= a->cap) {
    size_t ncap = a->cap ? a->cap * 2 : 8;
    SuperVal *ni = (SuperVal *)sup_grow_buf(a->items, a->len * sizeof(SuperVal), ncap * sizeof(SuperVal));
    if (!ni) return;
    a->items = ni;
    a->cap = ncap;
  }
  a->items[a->len++] = v;
}
static inline int super_truthy(SuperVal v) {
  switch (v.tag) {
    case SUPER_TAG_NULL: return 0;
    case SUPER_TAG_BOOL: return v.as.b;
    case SUPER_TAG_INT: return v.as.i != 0;
    case SUPER_TAG_FLOAT: return v.as.f != 0.0;
    case SUPER_TAG_STR: return v.as.s && v.as.s[0] != '\0';
    case SUPER_TAG_RANGE: return v.as.range.start < v.as.range.end;
    case SUPER_TAG_ARR: return v.as.arr && v.as.arr->len > 0;
    case SUPER_TAG_OBJ: return v.as.obj != nullptr;
    case SUPER_TAG_ERR: return 0;
  }
  return 0;
}
static inline SuperVal super_range(int64_t start, int64_t end) [[unsequenced]] {
  SuperVal r = { .tag = SUPER_TAG_RANGE, .as = { .range = { .start = start, .end = end } } };
  return r;
}
static inline double super_as_f(SuperVal v) [[unsequenced]] {
  return v.tag == SUPER_TAG_FLOAT ? v.as.f : (double)v.as.i;
}
static inline char *super_strdup(const char *s) {
  size_t n = strlen(s ? s : "");
  char *o = (char *)sup_req_alloc(n + 1, 1);
  if (!o) return nullptr;
  memcpy(o, s ? s : "", n + 1);
  return o;
}
typedef struct { char *data; size_t n; size_t cap; } SupBuf;
static void sup_buf_init(SupBuf *b) { b->data = NULL; b->n = 0; b->cap = 0; }
static void sup_buf_reserve(SupBuf *b, size_t extra) {
  if (b->n + extra + 1 <= b->cap) return;
  size_t c = b->cap ? b->cap : 64;
  while (c < b->n + extra + 1) c *= 2;
  char *p = (char *)sup_grow_buf(b->data, b->n + (b->data ? 1 : 0), c);
  if (!p) return;
  b->data = p;
  b->cap = c;
}
static void sup_buf_push(SupBuf *b, const char *s) {
  size_t m = strlen(s ? s : "");
  sup_buf_reserve(b, m);
  if (!b->data) return;
  memcpy(b->data + b->n, s ? s : "", m);
  b->n += m;
  b->data[b->n] = 0;
}
static void sup_buf_char(SupBuf *b, char c) {
  char t[2] = {c, 0};
  sup_buf_push(b, t);
}
static void sup_buf_quote(SupBuf *b, const char *s) {
  sup_buf_push(b, "\"");
  for (const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; ++p) {
    if (*p == '\\' || *p == '"') { sup_buf_char(b, '\\'); sup_buf_char(b, (char)*p); }
    else if (*p == '\n') sup_buf_push(b, "\\n");
    else if (*p == '\r') sup_buf_push(b, "\\r");
    else if (*p == '\t') sup_buf_push(b, "\\t");
    else sup_buf_char(b, (char)*p);
  }
  sup_buf_push(b, "\"");
}
static void sup_buf_float(SupBuf *b, double v) {
  char tmp[64];
  if (isfinite(v) && v == trunc(v) && fabs(v) < 1e15) snprintf(tmp, sizeof(tmp), "%lld", (long long)v);
  else snprintf(tmp, sizeof(tmp), "%g", v);
  sup_buf_push(b, tmp);
}
static SuperVal super_arr_at(const SuperArr *a, size_t i) {
  if (!a || i >= a->len) return SUPER_NULL;
  if (a->kind == 1) return super_float(a->f64s[i]);
  if (a->kind == 2) return super_int(a->i64s[i]);
  return a->items ? a->items[i] : SUPER_NULL;
}
static void super_show_into(SupBuf *b, SuperVal v, int quote, int depth);
static void super_show_fields(SupBuf *b, const char *label, const SuperObj *o, int depth) {
  if (label && label[0]) sup_buf_push(b, label);
  sup_buf_char(b, '{');
  uint32_t order[64];
  uint32_t n = o ? o->n : 0;
  int overflow = n > 64;
  if (overflow) n = 64;
  for (uint32_t i = 0; i < n; ++i) order[i] = i;
  for (uint32_t i = 1; i < n; ++i) {
    uint32_t key = order[i];
    uint32_t j = i;
    while (j > 0 && strcmp(o->keys[order[j - 1]] ? o->keys[order[j - 1]] : "", o->keys[key] ? o->keys[key] : "") > 0) {
      order[j] = order[j - 1];
      --j;
    }
    order[j] = key;
  }
  for (uint32_t i = 0; i < n; ++i) {
    if (i) sup_buf_push(b, ", ");
    sup_buf_push(b, o->keys[order[i]] ? o->keys[order[i]] : "");
    sup_buf_push(b, ": ");
    super_show_into(b, o->vals[order[i]], 1, depth + 1);
  }
  if (overflow) sup_buf_push(b, ", ...");
  sup_buf_char(b, '}');
}
static void super_show_into(SupBuf *b, SuperVal v, int quote, int depth) {
  char tmp[64];
  if (depth > 32) { sup_buf_push(b, "..."); return; }
  switch (v.tag) {
    case SUPER_TAG_NULL: sup_buf_push(b, "none"); break;
    case SUPER_TAG_BOOL: sup_buf_push(b, v.as.b ? "true" : "false"); break;
    case SUPER_TAG_INT: snprintf(tmp, sizeof(tmp), "%lld", (long long)v.as.i); sup_buf_push(b, tmp); break;
    case SUPER_TAG_FLOAT: sup_buf_float(b, v.as.f); break;
    case SUPER_TAG_STR:
      if (quote) sup_buf_quote(b, v.as.s ? v.as.s : "");
      else sup_buf_push(b, v.as.s ? v.as.s : "");
      break;
    case SUPER_TAG_RANGE:
      snprintf(tmp, sizeof(tmp), "%lld..%lld", (long long)v.as.range.start, (long long)v.as.range.end);
      sup_buf_push(b, tmp);
      break;
    case SUPER_TAG_ARR: {
      sup_buf_char(b, '[');
      SuperArr *a = v.as.arr;
      size_t n = a ? a->len : 0;
      for (size_t i = 0; i < n; ++i) {
        if (i) sup_buf_push(b, ", ");
        SuperVal item = super_arr_at(a, i);
        if (a && a->kind == 1) sup_buf_float(b, item.as.f);
        else if (a && a->kind == 2) { snprintf(tmp, sizeof(tmp), "%lld", (long long)item.as.i); sup_buf_push(b, tmp); }
        else super_show_into(b, item, 1, depth + 1);
      }
      sup_buf_char(b, ']');
      break;
    }
    case SUPER_TAG_OBJ: {
      SuperObj *o = v.as.obj;
      const char *label = (o && !o->is_map) ? o->name : NULL;
      super_show_fields(b, label, o, depth);
      break;
    }
    case SUPER_TAG_ERR:
      sup_buf_push(b, "error(");
      sup_buf_quote(b, v.as.s ? v.as.s : "");
      sup_buf_push(b, ")");
      break;
    default: sup_buf_push(b, "none"); break;
  }
}
static char *super_show_alloc(SuperVal v, int quote) {
  SupBuf b; sup_buf_init(&b);
  super_show_into(&b, v, quote, 0);
  if (!b.data) return super_strdup("");
  return b.data;
}
/* RFC JSON encode of any SuperVal (objects/arrays/scalars). Distinct from Display. */
static void super_json_quote(SupBuf *b, const char *s) {
  static const char *hex = "0123456789abcdef";
  sup_buf_char(b, '"');
  for (const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; ++p) {
    unsigned char c = *p;
    if (c == '"' || c == '\\') { sup_buf_char(b, '\\'); sup_buf_char(b, (char)c); }
    else if (c == '\n') sup_buf_push(b, "\\n");
    else if (c == '\r') sup_buf_push(b, "\\r");
    else if (c == '\t') sup_buf_push(b, "\\t");
    else if (c < 0x20) {
      char out[7];
      out[0] = '\\'; out[1] = 'u'; out[2] = '0'; out[3] = '0';
      out[4] = hex[c >> 4]; out[5] = hex[c & 15]; out[6] = 0;
      sup_buf_push(b, out);
    } else {
      sup_buf_char(b, (char)c);
    }
  }
  sup_buf_char(b, '"');
}
static void super_json_into(SupBuf *b, SuperVal v, int depth);
static void super_json_object(SupBuf *b, const SuperObj *o, int depth) {
  sup_buf_char(b, '{');
  uint32_t order[64];
  uint32_t n = o ? o->n : 0;
  int overflow = n > 64;
  if (overflow) n = 64;
  for (uint32_t i = 0; i < n; ++i) order[i] = i;
  for (uint32_t i = 1; i < n; ++i) {
    uint32_t key = order[i];
    uint32_t j = i;
    while (j > 0 && strcmp(o->keys[order[j - 1]] ? o->keys[order[j - 1]] : "", o->keys[key] ? o->keys[key] : "") > 0) {
      order[j] = order[j - 1];
      --j;
    }
    order[j] = key;
  }
  for (uint32_t i = 0; i < n; ++i) {
    if (i) sup_buf_char(b, ',');
    super_json_quote(b, o->keys[order[i]] ? o->keys[order[i]] : "");
    sup_buf_char(b, ':');
    super_json_into(b, o->vals[order[i]], depth + 1);
  }
  if (overflow) {
    if (n) sup_buf_char(b, ',');
    super_json_quote(b, "...");
    sup_buf_push(b, ":null");
  }
  sup_buf_char(b, '}');
}
static void super_json_into(SupBuf *b, SuperVal v, int depth) {
  char tmp[64];
  if (depth > 32) { sup_buf_push(b, "null"); return; }
  switch (v.tag) {
    case SUPER_TAG_NULL: sup_buf_push(b, "null"); break;
    case SUPER_TAG_BOOL: sup_buf_push(b, v.as.b ? "true" : "false"); break;
    case SUPER_TAG_INT: snprintf(tmp, sizeof(tmp), "%lld", (long long)v.as.i); sup_buf_push(b, tmp); break;
    case SUPER_TAG_FLOAT:
      if (!isfinite(v.as.f)) { sup_buf_push(b, "null"); break; }
      if (v.as.f == trunc(v.as.f) && fabs(v.as.f) < 1e15)
        snprintf(tmp, sizeof(tmp), "%lld", (long long)v.as.f);
      else
        snprintf(tmp, sizeof(tmp), "%.17g", v.as.f);
      sup_buf_push(b, tmp);
      break;
    case SUPER_TAG_STR: super_json_quote(b, v.as.s ? v.as.s : ""); break;
    case SUPER_TAG_RANGE:
      sup_buf_char(b, '[');
      snprintf(tmp, sizeof(tmp), "%lld", (long long)v.as.range.start); sup_buf_push(b, tmp);
      sup_buf_char(b, ',');
      snprintf(tmp, sizeof(tmp), "%lld", (long long)v.as.range.end); sup_buf_push(b, tmp);
      sup_buf_char(b, ']');
      break;
    case SUPER_TAG_ARR: {
      SuperArr *a = v.as.arr;
      size_t n = a ? a->len : 0;
      sup_buf_char(b, '[');
      for (size_t i = 0; i < n; ++i) {
        if (i) sup_buf_char(b, ',');
        SuperVal item = super_arr_at(a, i);
        if (a && a->kind == 1) {
          if (!isfinite(item.as.f)) sup_buf_push(b, "null");
          else if (item.as.f == trunc(item.as.f) && fabs(item.as.f) < 1e15) {
            snprintf(tmp, sizeof(tmp), "%lld", (long long)item.as.f); sup_buf_push(b, tmp);
          } else {
            snprintf(tmp, sizeof(tmp), "%.17g", item.as.f); sup_buf_push(b, tmp);
          }
        } else if (a && a->kind == 2) {
          snprintf(tmp, sizeof(tmp), "%lld", (long long)item.as.i); sup_buf_push(b, tmp);
        } else {
          super_json_into(b, item, depth + 1);
        }
      }
      sup_buf_char(b, ']');
      break;
    }
    case SUPER_TAG_OBJ: super_json_object(b, v.as.obj, depth); break;
    case SUPER_TAG_ERR: super_json_quote(b, v.as.s ? v.as.s : ""); break;
    default: sup_buf_push(b, "null"); break;
  }
}
static char *super_json_encode_alloc(SuperVal v) {
  SupBuf b; sup_buf_init(&b);
  super_json_into(&b, v, 0);
  if (!b.data) return super_strdup("null");
  return b.data;
}
static int64_t super_hash_text(const char *s) {
  uint64_t h = 0xcbf29ce484222325ULL;
  for (const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; ++p) {
    h ^= *p;
    h *= 0x100000001b3ULL;
  }
  return (int64_t)h;
}
static SuperVal super_eq_depth(SuperVal a, SuperVal b, int depth);
static int super_obj_eq(const SuperObj *a, const SuperObj *b, int depth) {
  if (!a || !b) return a == b;
  if (a->n != b->n || a->is_map != b->is_map) return 0;
  if ((a->name == NULL) != (b->name == NULL)) return 0;
  if (a->name && b->name && strcmp(a->name, b->name) != 0) return 0;
  for (uint32_t i = 0; i < a->n; ++i) {
    int j = super_obj_find(b, a->keys[i] ? a->keys[i] : "");
    if (j < 0) return 0;
    if (!super_eq_depth(a->vals[i], b->vals[j], depth + 1).as.b) return 0;
  }
  return 1;
}
static int super_arr_eq(const SuperArr *a, const SuperArr *b, int depth) {
  if (!a || !b) return a == b;
  if (a->len != b->len) return 0;
  for (size_t i = 0; i < a->len; ++i) {
    if (!super_eq_depth(super_arr_at(a, i), super_arr_at(b, i), depth + 1).as.b) return 0;
  }
  return 1;
}
static SuperVal super_eq_depth(SuperVal a, SuperVal b, int depth) {
  if (depth > 32) return super_bool(0);
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_FLOAT)
    return super_bool(fabs((double)a.as.i - b.as.f) < 1e-9);
  if (a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_INT)
    return super_bool(fabs(a.as.f - (double)b.as.i) < 1e-9);
  /* Sum discriminant ↔ i64: OBJ with __tag equals the integer tag. */
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_OBJ) {
    SuperVal t = super_obj_get(b, "__tag");
    return super_bool(t.tag == SUPER_TAG_INT && t.as.i == a.as.i);
  }
  if (a.tag == SUPER_TAG_OBJ && b.tag == SUPER_TAG_INT) {
    SuperVal t = super_obj_get(a, "__tag");
    return super_bool(t.tag == SUPER_TAG_INT && t.as.i == b.as.i);
  }
  if (a.tag != b.tag) return super_bool(0);
  switch (a.tag) {
    case SUPER_TAG_NULL: return super_bool(1);
    case SUPER_TAG_INT: return super_bool(a.as.i == b.as.i);
    case SUPER_TAG_FLOAT: return super_bool(fabs(a.as.f - b.as.f) < 1e-9);
    case SUPER_TAG_BOOL: return super_bool(a.as.b == b.as.b);
    case SUPER_TAG_STR: return super_bool(strcmp(a.as.s ? a.as.s : "", b.as.s ? b.as.s : "") == 0);
    case SUPER_TAG_RANGE: return super_bool(a.as.range.start == b.as.range.start && a.as.range.end == b.as.range.end);
    case SUPER_TAG_ARR: return super_bool(super_arr_eq(a.as.arr, b.as.arr, depth));
    case SUPER_TAG_OBJ: return super_bool(super_obj_eq(a.as.obj, b.as.obj, depth));
    case SUPER_TAG_ERR:
      return super_bool(strcmp(a.as.s ? a.as.s : "", b.as.s ? b.as.s : "") == 0);
    default: return super_bool(0);
  }
}
static int super_ord(SuperVal a, SuperVal b, int depth) {
  if (depth > 32) return 2;
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT)
    return (a.as.i > b.as.i) - (a.as.i < b.as.i);
  if (a.tag == SUPER_TAG_BOOL && b.tag == SUPER_TAG_BOOL)
    return (a.as.b > b.as.b) - (a.as.b < b.as.b);
  if (a.tag == SUPER_TAG_STR && b.tag == SUPER_TAG_STR) {
    int c = strcmp(a.as.s ? a.as.s : "", b.as.s ? b.as.s : "");
    return (c > 0) - (c < 0);
  }
  if (a.tag == SUPER_TAG_RANGE && b.tag == SUPER_TAG_RANGE) {
    if (a.as.range.start != b.as.range.start)
      return (a.as.range.start > b.as.range.start) - (a.as.range.start < b.as.range.start);
    return (a.as.range.end > b.as.range.end) - (a.as.range.end < b.as.range.end);
  }
  if (a.tag == SUPER_TAG_ARR && b.tag == SUPER_TAG_ARR && a.as.arr && b.as.arr) {
    size_t n = a.as.arr->len < b.as.arr->len ? a.as.arr->len : b.as.arr->len;
    for (size_t i = 0; i < n; ++i) {
      int c = super_ord(super_arr_at(a.as.arr, i), super_arr_at(b.as.arr, i), depth + 1);
      if (c == 2) return 2;
      if (c) return c;
    }
    if (a.as.arr->len < b.as.arr->len) return -1;
    if (a.as.arr->len > b.as.arr->len) return 1;
    return 0;
  }
  return 2;
}
static inline void super_fmt_val(char *buf, size_t cap, SuperVal v) {
  char *s = super_show_alloc(v, 0);
  snprintf(buf, cap, "%s", s ? s : "");
  super_safe_free(s);
}
static inline int64_t super_as_i(SuperVal v);
static inline SuperVal super_add(SuperVal a, SuperVal b) {
  /* Hot paths: plain floats then ints (benchmark numeric kernels). */
  if (a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_FLOAT)
    return super_float(a.as.f + b.as.f);
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT)
    return super_int(a.as.i + b.as.i);
  /* String concat / interpolation: any operand that is a string stringifies the other. */
  if (a.tag == SUPER_TAG_STR || b.tag == SUPER_TAG_STR) {
    char *as_owned = NULL, *bs_owned = NULL;
    const char *as = (a.tag == SUPER_TAG_STR) ? (a.as.s ? a.as.s : "") : (as_owned = super_show_alloc(a, 0));
    const char *bs = (b.tag == SUPER_TAG_STR) ? (b.as.s ? b.as.s : "") : (bs_owned = super_show_alloc(b, 0));
    if (!as) as = "";
    if (!bs) bs = "";
    size_t na = strlen(as), nb = strlen(bs);
    char *o = (char *)sup_req_alloc(na + nb + 1, 1);
    if (!o) { super_safe_free(as_owned); super_safe_free(bs_owned); return super_str(""); }
    memcpy(o, as, na);
    memcpy(o + na, bs, nb + 1);
    super_safe_free(as_owned);
    super_safe_free(bs_owned);
    return super_str(o);
  }
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_float(super_as_f(a) + super_as_f(b));
  return super_int(super_as_i(a) + super_as_i(b));
}
static inline SuperVal super_sub(SuperVal a, SuperVal b) {
  if (a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_FLOAT)
    return super_float(a.as.f - b.as.f);
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT)
    return super_int(a.as.i - b.as.i);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_float(super_as_f(a) - super_as_f(b));
  return super_int(super_as_i(a) - super_as_i(b));
}
static inline SuperVal super_mul(SuperVal a, SuperVal b) {
  if (a.tag == SUPER_TAG_FLOAT && b.tag == SUPER_TAG_FLOAT)
    return super_float(a.as.f * b.as.f);
  if (a.tag == SUPER_TAG_INT && b.tag == SUPER_TAG_INT)
    return super_int(a.as.i * b.as.i);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_float(super_as_f(a) * super_as_f(b));
  return super_int(super_as_i(a) * super_as_i(b));
}
/* `s[i]` is the i-th Unicode scalar (VM: `chars().nth(i)`), "" when out of range. */
static const char *super_str_char_at(const char *s, int64_t idx) {
  static const char ascii[128][2] = {
#define A4(n) {(char)(n),0},{(char)(n+1),0},{(char)(n+2),0},{(char)(n+3),0}
#define A16(n) A4(n),A4(n+4),A4(n+8),A4(n+12)
#define A64(n) A16(n),A16(n+16),A16(n+32),A16(n+48)
    A64(0), A64(64)
#undef A64
#undef A16
#undef A4
  };
  if (idx < 0) return "";
  const unsigned char *p = (const unsigned char *)s;
  int64_t seen = 0;
  for (; *p; ++p) {
    if ((*p & 0xC0) == 0x80) continue;
    if (seen++ != idx) continue;
    if (*p < 0x80) return ascii[*p];
    size_t n = 1;
    while ((p[n] & 0xC0) == 0x80) ++n;
    char *o = (char *)sup_req_alloc(n + 1, 1);
    if (!o) return "";
    memcpy(o, p, n);
    o[n] = 0;
    return o;
  }
  return "";
}
static inline SuperVal super_index_get(SuperVal base, SuperVal idx) {
  if (base.tag == SUPER_TAG_OBJ) return idx.tag == SUPER_TAG_STR ? super_obj_get(base, idx.as.s ? idx.as.s : "") : SUPER_NULL;
  int64_t iidx = idx.as.i;
  if (__builtin_expect(base.tag == SUPER_TAG_ARR && base.as.arr != nullptr, 1)) {
    SuperArr *a = base.as.arr;
    if (__builtin_expect(iidx >= 0 && (size_t)iidx < a->len, 1)) {
      if (a->kind == 1) return super_float(a->f64s[(size_t)iidx]);
      if (a->kind == 2) return super_int(a->i64s[(size_t)iidx]);
      return a->items[(size_t)iidx];
    }
    return SUPER_NULL;
  }
  if (base.tag == SUPER_TAG_STR) return super_str(super_str_char_at(base.as.s ? base.as.s : "", iidx));
  return SUPER_NULL;
}
static inline void super_index_set(SuperVal base, SuperVal idx, SuperVal val) {
  int64_t iidx = idx.as.i;
  if (__builtin_expect(base.tag == SUPER_TAG_ARR && base.as.arr != nullptr, 1)) {
    SuperArr *a = base.as.arr;
    if (__builtin_expect(iidx >= 0 && (size_t)iidx < a->len, 1)) {
      if (a->kind == 1) a->f64s[(size_t)iidx] = (val.tag == SUPER_TAG_FLOAT) ? val.as.f : (double)val.as.i;
      else if (a->kind == 2) a->i64s[(size_t)iidx] = (val.tag == SUPER_TAG_INT) ? val.as.i : (int64_t)((val.tag == SUPER_TAG_FLOAT) ? val.as.f : 0);
      else a->items[(size_t)iidx] = val;
    }
  }
}
static inline SuperVal super_div(SuperVal a, SuperVal b) {
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_float(super_as_f(a) / super_as_f(b));
  return super_int(b.as.i == 0 ? 0 : a.as.i / b.as.i);
}
static inline SuperVal super_rem(SuperVal a, SuperVal b) {
  return super_int(b.as.i == 0 ? 0 : a.as.i % b.as.i);
}
static inline SuperVal super_neg(SuperVal a) {
  if (a.tag == SUPER_TAG_FLOAT) return super_float(-a.as.f);
  return super_int(-a.as.i);
}
static inline SuperVal super_eq(SuperVal a, SuperVal b) {
  return super_eq_depth(a, b, 0);
}
static inline SuperVal super_ne(SuperVal a, SuperVal b) { return super_bool(!super_eq(a,b).as.b); }
static inline SuperVal super_lt(SuperVal a, SuperVal b) {
  int c = super_ord(a, b, 0);
  if (c != 2) return super_bool(c < 0);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_bool(super_as_f(a) < super_as_f(b));
  return super_bool(a.as.i < b.as.i);
}
static inline SuperVal super_le(SuperVal a, SuperVal b) {
  int c = super_ord(a, b, 0);
  if (c != 2) return super_bool(c <= 0);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_bool(super_as_f(a) <= super_as_f(b));
  return super_bool(a.as.i <= b.as.i);
}
static inline SuperVal super_gt(SuperVal a, SuperVal b) {
  int c = super_ord(a, b, 0);
  if (c != 2) return super_bool(c > 0);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_bool(super_as_f(a) > super_as_f(b));
  return super_bool(a.as.i > b.as.i);
}
static inline SuperVal super_ge(SuperVal a, SuperVal b) {
  int c = super_ord(a, b, 0);
  if (c != 2) return super_bool(c >= 0);
  if (a.tag == SUPER_TAG_FLOAT || b.tag == SUPER_TAG_FLOAT)
    return super_bool(super_as_f(a) >= super_as_f(b));
  return super_bool(a.as.i >= b.as.i);
}
static inline void super_print(SuperVal v) {
  char *s = super_show_alloc(v, 0);
  printf("%s\n", s ? s : "");
  super_safe_free(s);
}
static inline int64_t super_as_i(SuperVal v) {
  if (v.tag == SUPER_TAG_FLOAT) return (int64_t)v.as.f;
  if (v.tag == SUPER_TAG_BOOL) return v.as.b;
  if (v.tag == SUPER_TAG_OBJ) {
    SuperVal t = super_obj_get(v, "__tag");
    if (t.tag == SUPER_TAG_INT) return t.as.i;
    return 0; /* never leak a pointer */
  }
  if (v.tag == SUPER_TAG_ARR) return 0;
  return v.as.i;
}
/* Defined in aot_host::NET_HOST_C (appended after this prelude). */
static SuperVal super_host_net(const char *hn, SuperVal *args, int nargs);
/* String search helpers live in the unboxed prelude. */
static inline int64_t sup_str_find(const char *s, const char *nd, int64_t start);
static inline bool sup_str_match_at(const char *s, const char *nd, int64_t pos);
static inline int64_t sup_str_find_lit_digits(const char *s, const char *lit, int64_t from);
static inline int64_t sup_str_lit_digits_end(const char *s, int64_t start, const char *lit);
static inline int64_t sup_str_len(const char *s);
static inline int64_t sup_byte_len(const char *s);
static inline const char *sup_push_ascii(const char *s, int64_t code);
static inline const char *sup_push_i64(const char *s, int64_t v);
static inline int64_t sup_sb_new(int64_t cap0);
static inline int64_t sup_sb_stack(int64_t cap0);
static inline void sup_sb_litn(int64_t h, const char *s, size_t pn);
static inline void sup_sb_clear(int64_t h);
static inline void sup_sb_cat(int64_t dst, int64_t src);
static inline const char *sup_sb_cstr(int64_t h);
static inline void sup_sb_lit(int64_t h, const char *s);
static inline void sup_sb_ascii(int64_t h, int64_t code);
static inline void sup_sb_i64(int64_t h, int64_t v);
static inline int64_t sup_sb_len(int64_t h);
static inline const char *sup_sb_finish(int64_t h);
static inline int64_t sup_byte_at(const char *s, int64_t i);
static inline const char *sup_byte_slice(const char *s, int64_t lo, int64_t hi);
static inline int64_t sup_byte_find(const char *s, const char *nd, int64_t from);
static inline int64_t sup_byte_find_limit(const char *s, const char *nd, int64_t from, int64_t limit);
static inline bool sup_byte_match_at(const char *s, const char *nd, int64_t pos);
static inline bool sup_byte_eq_range(const char *s, int64_t lo, int64_t hi, const char *t);
static inline bool sup_byte_eq_ranges(const char *a, int64_t alo, int64_t ahi,
                                      const char *b, int64_t blo, int64_t bhi);
/* SHA-256 / HMAC helpers live in the unboxed prelude. */
static inline const char *sup_sha256_hex(const char *s);
static inline int64_t *sup_sha256_i64(const int64_t *msg, int64_t mn, int64_t *ret_len);
static inline const char *sup_sha2_hex_of_i64(const int64_t *bytes, int64_t n);
static inline const char *sup_hmac_sha256_hex(const char *key, const char *msg);
static inline int64_t *sup_hmac_sha256_i64(const int64_t *key, int64_t kn,
                                          const int64_t *msg, int64_t mn, int64_t *ret_len);
static inline int64_t *sup_hmac_sha256_i64_str(const int64_t *key, int64_t kn,
                                              const char *msg, int64_t *ret_len);
static inline int64_t *sup_hmac_sha256_i64_str_range(const int64_t *key, int64_t kn,
                                                    const char *msg, int64_t lo, int64_t hi,
                                                    int64_t *ret_len);
static inline bool sup_sha2_hex_prefix_eq(const int64_t *mac, int64_t nbytes,
                                         const char *text, int64_t lo);
static inline bool sup_hmac_sha256_hex_prefix_eq_range(
    const int64_t *key, int64_t kn, const char *msg, int64_t lo, int64_t hi,
    int64_t nbytes, const char *text, int64_t tlo);
static inline void sup_hmac_sha256_sig8_into_range(
    int64_t sb_h, const int64_t *key, int64_t kn,
    const char *msg, int64_t lo, int64_t hi);
static inline void sup_hmac_sha256_sig8_forged_into_range(
    int64_t sb_h, const int64_t *key, int64_t kn,
    const char *msg, int64_t lo, int64_t hi);
static inline int64_t sup_crc32_sb(int64_t h);
static SuperVal super_host(const char *restrict name, SuperVal *restrict args, int nargs) {
  if (!name) return SUPER_NULL;
  /* strip optional "host." prefix */
  if (strncmp(name, "host.", 5) == 0) name += 5;
  /* Compiler intrinsics for `or err` / `fail`. These are not stdlib hosts. */
  if (strcmp(name, "__is_err") == 0)
    return super_bool(nargs > 0 && args[0].tag == SUPER_TAG_ERR);
  if (strcmp(name, "__err_text") == 0) {
    if (nargs > 0 && (args[0].tag == SUPER_TAG_ERR || args[0].tag == SUPER_TAG_STR))
      return super_str(args[0].as.s ? args[0].as.s : "");
    return super_str("");
  }
  if (strcmp(name, "sys_args") == 0) {
    int n = super_argc > 0 ? super_argc : 0;
    SuperArr *a = super_arr_alloc((size_t)n);
    for (int i = 0; i < n; i++)
      super_arr_push(a, super_str(super_argv && super_argv[i] ? super_argv[i] : ""));
    return super_arr(a);
  }
  if (strcmp(name, "sys_env_get") == 0) {
    const char *k = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *v = getenv(k);
    if (!v) return SUPER_NULL;
    char *owned = super_strdup(v);
    return super_str(owned ? owned : "");
  }
  if (strcmp(name, "sys_env_set") == 0) {
    const char *k = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *v = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    return super_bool(setenv(k, v, 1) == 0);
  }
  if (strcmp(name, "sys_arr_pop") == 0) {
    if (nargs > 0 && args[0].tag == SUPER_TAG_ARR && args[0].as.arr && args[0].as.arr->len > 0)
      args[0].as.arr->len -= 1;
    return nargs > 0 ? args[0] : SUPER_NULL;
  }
  if ((strcmp(name, "display") == 0 || strcmp(name, "to_str") == 0 || strcmp(name, "debug") == 0) && nargs >= 1) {
    char *s = super_show_alloc(args[0], strcmp(name, "debug") == 0);
    return super_str(s ? s : "");
  }
  if (strcmp(name, "sys_json_encode") == 0 && nargs >= 1) {
    char *s = super_json_encode_alloc(args[0]);
    return super_str(s ? s : "null");
  }
  if (strcmp(name, "sys_json_body") == 0 && nargs >= 1) {
    if (args[0].tag == SUPER_TAG_STR) return args[0];
    char *s = super_json_encode_alloc(args[0]);
    return super_str(s ? s : "null");
  }
  if (strcmp(name, "hash") == 0 && nargs >= 1) {
    char *s = super_show_alloc(args[0], 0);
    int64_t h = super_hash_text(s);
    super_safe_free(s);
    return super_int(h);
  }
  if (strcmp(name, "clone") == 0 && nargs >= 1) return args[0];
  if (strcmp(name, "eq") == 0 && nargs >= 2) return super_eq(args[0], args[1]);
  if (strcmp(name, "cmp") == 0 && nargs >= 2) {
    int c = super_ord(args[0], args[1], 0);
    return super_int(c == 2 ? 0 : c);
  }

  if (strcmp(name, "sys_arr_new") == 0) {
    /* Heap/arena — never the frame scratch stack. `arr_new` seeds durable
       structures (Router.prefix_heads, RateLimiter, CookieJar, …). Those must
       survive `sup_stack_reset` at request leave (ROUND7 multi-route 404).
       Unboxed numeric kernels still use super_arr_alloc_*_stack directly. */
    int64_t n = nargs > 0 ? super_as_i(args[0]) : 0;
    if (n < 0) n = 0;
    SuperVal fill = nargs > 1 ? args[1] : SUPER_NULL;
    if (fill.tag == SUPER_TAG_FLOAT) {
      SuperArr *a = super_arr_alloc_f64((size_t)n);
      if (a && fill.as.f != 0.0) {
        for (size_t i = 0; i < a->len; i++) a->f64s[i] = fill.as.f;
      }
      return super_arr(a);
    }
    if (fill.tag == SUPER_TAG_INT || fill.tag == SUPER_TAG_NULL) {
      SuperArr *a = super_arr_alloc_i64((size_t)n);
      if (a && fill.tag == SUPER_TAG_INT && fill.as.i != 0) {
        for (size_t i = 0; i < a->len; i++) a->i64s[i] = fill.as.i;
      }
      return super_arr(a);
    }
    SuperArr *a = super_arr_alloc((size_t)n);
    for (int64_t i = 0; i < n; i++) super_arr_push(a, fill);
    return super_arr(a);
  }
  /* Dense zeroed byte buffer (dense `[n]u8` buffer). Boxed path stores i64 lanes 0..255;
     unboxed AOT lowers `sys_bytes_new` to `uint8_t*` (ArrU8). Heap/arena — same
     durability rule as sys_arr_new (must outlive frame-stack resets). */
  if (strcmp(name, "sys_bytes_new") == 0) {
    int64_t n = nargs > 0 ? super_as_i(args[0]) : 0;
    if (n < 0) n = 0;
    SuperArr *a = super_arr_alloc_i64((size_t)n);
    return super_arr(a);
  }
  if (strcmp(name, "sys_u8_copy_range") == 0) {
    /* dst, dst_off, src, src_off, n — memcpy n byte lanes (i64 0..255 boxed). */
    if (nargs < 5) return SUPER_NULL;
    SuperArr *dst = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *src = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    int64_t dst_off = super_as_i(args[1]), src_off = super_as_i(args[3]), n = super_as_i(args[4]);
    if (!dst || !src || n <= 0 || dst_off < 0 || src_off < 0) return args[0];
    int64_t lim = n;
    if (dst_off + lim > (int64_t)dst->len) lim = (int64_t)dst->len - dst_off;
    if (src_off + lim > (int64_t)src->len) lim = (int64_t)src->len - src_off;
    if (lim <= 0) return args[0];
    for (int64_t i = 0; i < lim; ++i) {
      int64_t v = 0;
      int64_t si = src_off + i;
      if (src->kind == 2 && src->i64s) v = src->i64s[si] & 255;
      else if (src->kind == 0 && src->items) v = super_as_i(src->items[si]) & 255;
      int64_t di = dst_off + i;
      if (dst->kind == 2 && dst->i64s) dst->i64s[di] = v;
      else if (dst->kind == 0 && dst->items) dst->items[di] = super_int(v);
    }
    return args[0];
  }
  if (strcmp(name, "sys_be32_put") == 0) {
    if (nargs < 3) return SUPER_NULL;
    SuperArr *b = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t at = super_as_i(args[1]);
    uint32_t v = (uint32_t)super_as_i(args[2]);
    if (!b || at < 0 || at + 4 > (int64_t)b->len) return args[0];
    int64_t lanes[4] = { (v >> 24) & 255, (v >> 16) & 255, (v >> 8) & 255, v & 255 };
    for (int k = 0; k < 4; ++k) {
      if (b->kind == 2 && b->i64s) b->i64s[at + k] = lanes[k];
      else if (b->kind == 0 && b->items) b->items[at + k] = super_int(lanes[k]);
    }
    return args[0];
  }
  if (strcmp(name, "sys_be32_get") == 0) {
    if (nargs < 2) return super_int(0);
    SuperArr *b = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t at = super_as_i(args[1]);
    if (!b || at < 0 || at + 4 > (int64_t)b->len) return super_int(0);
    int64_t lane = 0;
    int64_t out = 0;
    for (int k = 0; k < 4; ++k) {
      if (b->kind == 2 && b->i64s) lane = b->i64s[at + k] & 255;
      else if (b->kind == 0 && b->items) lane = super_as_i(b->items[at + k]) & 255;
      else lane = 0;
      out = (out << 8) | lane;
    }
    return super_int(out);
  }
  if (strcmp(name, "sys_i64_merge") == 0) {
    /* Dense i64 mergesort merge: args a, tmp, lo, mid, hi */
    if (nargs < 5) return SUPER_NULL;
    SuperArr *a = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *tmp = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t lo = super_as_i(args[2]), mid = super_as_i(args[3]), hi = super_as_i(args[4]);
    if (!a || !tmp || a->kind != 2 || tmp->kind != 2) return args[0];
    int64_t i = lo, j = mid, k = lo;
    int64_t *ap = a->i64s, *tp = tmp->i64s;
    while (i < mid && j < hi) {
      if (ap[i] <= ap[j]) tp[k++] = ap[i++];
      else tp[k++] = ap[j++];
    }
    while (i < mid) tp[k++] = ap[i++];
    while (j < hi) tp[k++] = ap[j++];
    for (int64_t t = lo; t < hi; ++t) ap[t] = tp[t];
    return args[0];
  }
  if (strcmp(name, "sys_arr_truncate") == 0) {
    /* arr, n — shrink length to min(len, max(0,n)); capacity unchanged. */
    if (nargs < 2 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr) return args[0];
    int64_t n = super_as_i(args[1]);
    if (n < 0) n = 0;
    if ((uint64_t)n < args[0].as.arr->len) args[0].as.arr->len = (size_t)n;
    return args[0];
  }
  if (strcmp(name, "sys_i64_bucket_rows") == 0) {
    /* order, gids, fill — order[fill[gid]++] = row for each row (scan-order partitions). */
    if (nargs < 3) return nargs ? args[0] : SUPER_NULL;
    SuperArr *order = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *gids = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *fill = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    if (!order || !gids || !fill || order->kind != 2 || gids->kind != 2 || fill->kind != 2) return args[0];
    int64_t n = (int64_t)gids->len;
    int64_t on = (int64_t)order->len;
    int64_t fn = (int64_t)fill->len;
    for (int64_t p = 0; p < n; ++p) {
      int64_t g = gids->i64s[p];
      if (g < 0 || g >= fn) continue;
      int64_t at = fill->i64s[g];
      if (at >= 0 && at < on) order->i64s[at] = p;
      fill->i64s[g] = at + 1;
    }
    return args[0];
  }
  if (strcmp(name, "sys_group_sum_max_mean") == 0) {
    /* gids, ivals, fvals, ng -> [sums, maxs, means] dense no-null fuse. */
    if (nargs < 4) return SUPER_NULL;
    SuperArr *gids = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *ivals = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *fvals = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    int64_t ng = super_as_i(args[3]);
    if (!gids || !ivals || !fvals || ng < 0) return SUPER_NULL;
    int64_t n = (int64_t)gids->len;
    SuperArr *sums = super_arr_alloc_i64((size_t)ng);
    SuperArr *maxs = super_arr_alloc_i64((size_t)ng);
    SuperArr *means = super_arr_alloc_f64((size_t)ng);
    if (!sums || !maxs || !means) return SUPER_NULL;
    int64_t *seen = (int64_t *)calloc((size_t)ng, sizeof(int64_t));
    int64_t *mcnt = (int64_t *)calloc((size_t)ng, sizeof(int64_t));
    if (!seen || !mcnt) { free(seen); free(mcnt); return SUPER_NULL; }
    for (int64_t p = 0; p < n; ++p) {
      int64_t g = (gids->kind == 2 && gids->i64s) ? gids->i64s[p]
                : (gids->kind == 0 && gids->items) ? super_as_i(gids->items[p]) : -1;
      if (g < 0 || g >= ng) continue;
      int64_t iv = (ivals->kind == 2 && ivals->i64s) ? ivals->i64s[p]
                 : (ivals->kind == 0 && ivals->items) ? super_as_i(ivals->items[p]) : 0;
      double fv = (fvals->kind == 1 && fvals->f64s) ? fvals->f64s[p]
                : (fvals->kind == 0 && fvals->items) ? super_as_f(fvals->items[p]) : 0.0;
      if (!seen[g]) {
        sums->i64s[g] = iv;
        maxs->i64s[g] = iv;
        seen[g] = 1;
      } else {
        sums->i64s[g] += iv;
        if (iv > maxs->i64s[g]) maxs->i64s[g] = iv;
      }
      means->f64s[g] += fv;
      mcnt[g] += 1;
    }
    for (int64_t g = 0; g < ng; ++g) {
      if (mcnt[g] > 0) means->f64s[g] /= (double)mcnt[g];
    }
    free(seen);
    free(mcnt);
    SuperArr *pack = super_arr_alloc(3);
    super_arr_push(pack, super_arr(sums));
    super_arr_push(pack, super_arr(maxs));
    super_arr_push(pack, super_arr(means));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_group_ikey_sum_max_mean") == 0) {
    /* keys, ivals, fvals -> [out_keys, sums, maxs, means, sizes] first-seen order.
       Empty pack when any key < 0 or span > 4n+64. */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *keys = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *ivals = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *fvals = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    if (!keys || !ivals || !fvals) return SUPER_NULL;
    int64_t n = (int64_t)keys->len;
    if (n == 0) {
      SuperArr *pack = super_arr_alloc(5);
      super_arr_push(pack, super_arr(super_arr_alloc_i64(0)));
      super_arr_push(pack, super_arr(super_arr_alloc_i64(0)));
      super_arr_push(pack, super_arr(super_arr_alloc_i64(0)));
      super_arr_push(pack, super_arr(super_arr_alloc_f64(0)));
      super_arr_push(pack, super_arr(super_arr_alloc_i64(0)));
      return super_arr(pack);
    }
    int64_t mx = -1;
    for (int64_t p = 0; p < n; ++p) {
      int64_t k = (keys->kind == 2 && keys->i64s) ? keys->i64s[p]
                : (keys->kind == 0 && keys->items) ? super_as_i(keys->items[p]) : -1;
      if (k < 0) {
        SuperArr *empty = super_arr_alloc(0);
        return super_arr(empty);
      }
      if (k > mx) mx = k;
    }
    int64_t span = mx + 1;
    if (span > n * 4 + 64) {
      SuperArr *empty = super_arr_alloc(0);
      return super_arr(empty);
    }
    int64_t *map = (int64_t *)calloc((size_t)span, sizeof(int64_t));
    if (!map) return SUPER_NULL;
    SuperArr *out_keys = super_arr_alloc_i64(0);
    SuperArr *sums = super_arr_alloc_i64(0);
    SuperArr *maxs = super_arr_alloc_i64(0);
    SuperArr *means = super_arr_alloc_f64(0);
    SuperArr *sizes = super_arr_alloc_i64(0);
    if (!out_keys || !sums || !maxs || !means || !sizes) {
      free(map);
      return SUPER_NULL;
    }
    /* Growable mean counts — capped by distinct keys ≤ span. */
    int64_t *mcnt = (int64_t *)calloc((size_t)span, sizeof(int64_t));
    if (!mcnt) { free(map); return SUPER_NULL; }
    for (int64_t p = 0; p < n; ++p) {
      int64_t k = (keys->kind == 2 && keys->i64s) ? keys->i64s[p]
                : (keys->kind == 0 && keys->items) ? super_as_i(keys->items[p]) : -1;
      int64_t iv = (ivals->kind == 2 && ivals->i64s) ? ivals->i64s[p]
                 : (ivals->kind == 0 && ivals->items) ? super_as_i(ivals->items[p]) : 0;
      double fv = (fvals->kind == 1 && fvals->f64s) ? fvals->f64s[p]
                : (fvals->kind == 0 && fvals->items) ? super_as_f(fvals->items[p]) : 0.0;
      int64_t g = map[k] - 1;
      if (g < 0) {
        g = (int64_t)out_keys->len;
        map[k] = g + 1;
        super_arr_push(out_keys, super_int(k));
        super_arr_push(sums, super_int(iv));
        super_arr_push(maxs, super_int(iv));
        super_arr_push(means, super_float(fv));
        super_arr_push(sizes, super_int(1));
        mcnt[g] = 1;
      } else {
        sums->i64s[g] += iv;
        if (iv > maxs->i64s[g]) maxs->i64s[g] = iv;
        means->f64s[g] += fv;
        sizes->i64s[g] += 1;
        mcnt[g] += 1;
      }
    }
    for (size_t g = 0; g < means->len; ++g) {
      if (mcnt[g] > 0) means->f64s[g] /= (double)mcnt[g];
    }
    free(mcnt);
    free(map);
    SuperArr *pack = super_arr_alloc(5);
    super_arr_push(pack, super_arr(out_keys));
    super_arr_push(pack, super_arr(sums));
    super_arr_push(pack, super_arr(maxs));
    super_arr_push(pack, super_arr(means));
    super_arr_push(pack, super_arr(sizes));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_i64_is_sorted_asc") == 0) {
    /* vals -> 1 if non-decreasing (empty ok), else 0. */
    if (nargs < 1) return SUPER_NULL;
    SuperArr *vals = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    if (!vals) return super_int(1);
    int64_t n = (int64_t)vals->len;
    if (n <= 1) return super_int(1);
    int64_t prev = (vals->kind == 2 && vals->i64s) ? vals->i64s[0]
                 : (vals->kind == 0 && vals->items) ? super_as_i(vals->items[0]) : 0;
    for (int64_t i = 1; i < n; ++i) {
      int64_t v = (vals->kind == 2 && vals->i64s) ? vals->i64s[i]
                : (vals->kind == 0 && vals->items) ? super_as_i(vals->items[i]) : 0;
      if (v < prev) return super_int(0);
      prev = v;
    }
    return super_int(1);
  }
  if (strcmp(name, "sys_f64_roll_parts") == 0) {
    /* src, order, starts, ng, window, minp, mean, nrows -> [vals, nulls] */
    if (nargs < 8) return SUPER_NULL;
    SuperArr *src = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *order = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *starts = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    int64_t ng = super_as_i(args[3]);
    int64_t window = super_as_i(args[4]);
    int64_t minp = super_as_i(args[5]);
    int mean = super_as_i(args[6]) != 0;
    int64_t nrows = super_as_i(args[7]);
    if (!src || !order || !starts || nrows < 0) return SUPER_NULL;
    if (ng < 0) ng = 0;
    if (window < 0) window = 0;
    if (minp < 0) minp = 0;
    SuperArr *vals = super_arr_alloc_f64((size_t)nrows);
    SuperArr *nulls = super_arr_alloc_i64((size_t)nrows);
    if (!vals || !nulls) return SUPER_NULL;
    for (int64_t i = 0; i < nrows; ++i) nulls->i64s[i] = 1;
    int64_t sn = (int64_t)src->len;
    for (int64_t g = 0; g < ng; ++g) {
      int64_t lo = (g < (int64_t)starts->len) ? starts->i64s[g] : 0;
      int64_t hi = (g + 1 < (int64_t)starts->len) ? starts->i64s[g + 1] : 0;
      if (hi < lo) continue;
      int64_t plen = hi - lo;
      double sum = 0.0;
      int64_t have = 0;
      for (int64_t i = 0; i < plen; ++i) {
        int64_t row = -1;
        if (lo + i >= 0 && lo + i < (int64_t)order->len) {
          row = (order->kind == 2 && order->i64s) ? order->i64s[lo + i]
              : (order->kind == 0 && order->items) ? super_as_i(order->items[lo + i]) : -1;
        }
        if (row >= 0 && row < sn) {
          double v = (src->kind == 1 && src->f64s) ? src->f64s[row]
                   : (src->kind == 0 && src->items) ? super_as_f(src->items[row]) : 0.0;
          sum += v;
          have += 1;
        }
        if (window > 0 && i >= window) {
          int64_t leave = -1;
          int64_t li = lo + i - window;
          if (li >= 0 && li < (int64_t)order->len) {
            leave = (order->kind == 2 && order->i64s) ? order->i64s[li]
                  : (order->kind == 0 && order->items) ? super_as_i(order->items[li]) : -1;
          }
          if (leave >= 0 && leave < sn) {
            double v = (src->kind == 1 && src->f64s) ? src->f64s[leave]
                     : (src->kind == 0 && src->items) ? super_as_f(src->items[leave]) : 0.0;
            sum -= v;
            have -= 1;
          }
        }
        if (have >= minp && row >= 0 && row < nrows) {
          vals->f64s[row] = mean ? (sum / (double)have) : sum;
          nulls->i64s[row] = 0;
        }
      }
    }
    SuperArr *pack = super_arr_alloc(2);
    super_arr_push(pack, super_arr(vals));
    super_arr_push(pack, super_arr(nulls));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_f64_roll_by_ikey") == 0) {
    /* keys, vals, window, minp, mean -> [out_vals, out_nulls] scan-order per-key roll.
       Empty pack (len-0 arrays) when keys are negative or span > 4n+64. */
    if (nargs < 5) return SUPER_NULL;
    SuperArr *keys = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *src = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t window = super_as_i(args[2]);
    int64_t minp = super_as_i(args[3]);
    int mean = super_as_i(args[4]) != 0;
    if (!keys || !src) return SUPER_NULL;
    if (window < 0) window = 0;
    if (minp < 0) minp = 0;
    int64_t n = (int64_t)src->len;
    if ((int64_t)keys->len < n) n = (int64_t)keys->len;
    if (n < 0) n = 0;
    int64_t mx = -1;
    int bad = 0;
    for (int64_t i = 0; i < n; ++i) {
      int64_t k = (keys->kind == 2 && keys->i64s) ? keys->i64s[i]
               : (keys->kind == 0 && keys->items) ? super_as_i(keys->items[i]) : -1;
      if (k < 0) { bad = 1; break; }
      if (k > mx) mx = k;
    }
    int64_t span = mx + 1;
    if (bad || span <= 0 || span > n * 4 + 64) {
      SuperArr *ev = super_arr_alloc_f64(0);
      SuperArr *en = super_arr_alloc_i64(0);
      if (!ev || !en) return SUPER_NULL;
      SuperArr *ep = super_arr_alloc(2);
      super_arr_push(ep, super_arr(ev));
      super_arr_push(ep, super_arr(en));
      return super_arr(ep);
    }
    SuperArr *vals = super_arr_alloc_f64((size_t)n);
    SuperArr *nulls = super_arr_alloc_i64((size_t)n);
    if (!vals || !nulls) return SUPER_NULL;
    for (int64_t i = 0; i < n; ++i) nulls->i64s[i] = 1;
    if (window == 0) {
      SuperArr *pack = super_arr_alloc(2);
      super_arr_push(pack, super_arr(vals));
      super_arr_push(pack, super_arr(nulls));
      return super_arr(pack);
    }
    double *sum = (double *)calloc((size_t)span, sizeof(double));
    int64_t *seen = (int64_t *)calloc((size_t)span, sizeof(int64_t));
    double *ring = (double *)calloc((size_t)span * (size_t)window, sizeof(double));
    if (!sum || !seen || !ring) {
      free(sum); free(seen); free(ring);
      return SUPER_NULL;
    }
    for (int64_t i = 0; i < n; ++i) {
      int64_t k = (keys->kind == 2 && keys->i64s) ? keys->i64s[i]
               : (keys->kind == 0 && keys->items) ? super_as_i(keys->items[i]) : -1;
      double v = (src->kind == 1 && src->f64s) ? src->f64s[i]
               : (src->kind == 0 && src->items) ? super_as_f(src->items[i]) : 0.0;
      int64_t pos = seen[k] % window;
      int64_t base = k * window;
      if (seen[k] >= window) sum[k] -= ring[base + pos];
      ring[base + pos] = v;
      sum[k] += v;
      seen[k] += 1;
      int64_t have = seen[k] < window ? seen[k] : window;
      if (have >= minp) {
        vals->f64s[i] = mean ? (sum[k] / (double)have) : sum[k];
        nulls->i64s[i] = 0;
      }
    }
    free(sum); free(seen); free(ring);
    SuperArr *pack = super_arr_alloc(2);
    super_arr_push(pack, super_arr(vals));
    super_arr_push(pack, super_arr(nulls));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_i64_gather3") == 0) {
    /* idx, a, b, c -> [oa, ob, oc] dense gathers, one idx walk. */
    if (nargs < 4) return SUPER_NULL;
    SuperArr *idx = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *a = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *b = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    SuperArr *c = (args[3].tag == SUPER_TAG_ARR) ? args[3].as.arr : nullptr;
    if (!idx || !a || !b || !c) return SUPER_NULL;
    int64_t n = (int64_t)idx->len;
    SuperArr *oa = super_arr_alloc_i64((size_t)n);
    SuperArr *ob = super_arr_alloc_i64((size_t)n);
    SuperArr *oc = super_arr_alloc_i64((size_t)n);
    if (!oa || !ob || !oc) return SUPER_NULL;
    for (int64_t i = 0; i < n; ++i) {
      int64_t ix = (idx->kind == 2 && idx->i64s) ? idx->i64s[i]
                 : (idx->kind == 0 && idx->items) ? super_as_i(idx->items[i]) : -1;
      int64_t va = 0, vb = 0, vc = 0;
      if (ix >= 0) {
        if ((size_t)ix < a->len)
          va = (a->kind == 2 && a->i64s) ? a->i64s[ix]
             : (a->kind == 0 && a->items) ? super_as_i(a->items[ix]) : 0;
        if ((size_t)ix < b->len)
          vb = (b->kind == 2 && b->i64s) ? b->i64s[ix]
             : (b->kind == 0 && b->items) ? super_as_i(b->items[ix]) : 0;
        if ((size_t)ix < c->len)
          vc = (c->kind == 2 && c->i64s) ? c->i64s[ix]
             : (c->kind == 0 && c->items) ? super_as_i(c->items[ix]) : 0;
      }
      oa->i64s[i] = va;
      ob->i64s[i] = vb;
      oc->i64s[i] = vc;
    }
    SuperArr *pack = super_arr_alloc(3);
    super_arr_push(pack, super_arr(oa));
    super_arr_push(pack, super_arr(ob));
    super_arr_push(pack, super_arr(oc));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_join_dense_unique") == 0) {
    /* lkeys, rkeys, max_key -> [1, lidx, ridx] on unique success; [] when not unique. */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *lkeys = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *rkeys = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t max_key = super_as_i(args[2]);
    if (!lkeys || !rkeys || max_key < 0) return SUPER_NULL;
    int64_t span = max_key + 1;
    int64_t rn = (int64_t)rkeys->len;
    int64_t ln = (int64_t)lkeys->len;
    int64_t *head = (int64_t *)malloc((size_t)span * sizeof(int64_t));
    if (!head) return SUPER_NULL;
    for (int64_t i = 0; i < span; ++i) head[i] = -1;
    int multi = 0;
    for (int64_t rrow = 0; rrow < rn; ++rrow) {
      int64_t key = (rkeys->kind == 2 && rkeys->i64s) ? rkeys->i64s[rrow]
                  : (rkeys->kind == 0 && rkeys->items) ? super_as_i(rkeys->items[rrow]) : -1;
      if (key < 0 || key >= span) continue;
      if (head[key] >= 0) { multi = 1; break; }
      head[key] = rrow;
    }
    if (multi) {
      free(head);
      return super_arr(super_arr_alloc(0));
    }
    int64_t n_out = 0;
    for (int64_t lrow = 0; lrow < ln; ++lrow) {
      int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                  : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
      if (key >= 0 && key < span && head[key] >= 0) n_out += 1;
    }
    SuperArr *lidx = super_arr_alloc_i64((size_t)n_out);
    SuperArr *ridx = super_arr_alloc_i64((size_t)n_out);
    if (!lidx || !ridx) { free(head); return SUPER_NULL; }
    int64_t at = 0;
    for (int64_t lrow = 0; lrow < ln; ++lrow) {
      int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                  : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
      if (key >= 0 && key < span && head[key] >= 0) {
        lidx->i64s[at] = lrow;
        ridx->i64s[at] = head[key];
        at += 1;
      }
    }
    free(head);
    SuperArr *pack = super_arr_alloc(3);
    super_arr_push(pack, super_int(1));
    super_arr_push(pack, super_arr(lidx));
    super_arr_push(pack, super_arr(ridx));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_i64_any_nz") == 0) {
    /* arr -> 1 if any element != 0 else 0 (dense null-bitmap probe). */
    if (nargs < 1 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr) return super_int(0);
    SuperArr *a = args[0].as.arr;
    if (a->kind == 2 && a->i64s) {
      for (size_t i = 0; i < a->len; ++i) if (a->i64s[i] != 0) return super_int(1);
    } else if (a->kind == 0 && a->items) {
      for (size_t i = 0; i < a->len; ++i) if (super_as_i(a->items[i]) != 0) return super_int(1);
    }
    return super_int(0);
  }
  if (strcmp(name, "sys_filter_f64_ge_i64_gt") == 0) {
    /* fvals, ivals, fv, iv -> dense i64 idx of rows where f >= fv && i > iv. */
    if (nargs < 4) return SUPER_NULL;
    SuperArr *fvarr = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *ivarr = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    double fv = super_as_f(args[2]);
    int64_t iv = super_as_i(args[3]);
    if (!fvarr || !ivarr) return SUPER_NULL;
    int64_t n = (int64_t)fvarr->len;
    if ((int64_t)ivarr->len < n) n = (int64_t)ivarr->len;
    SuperArr *out = super_arr_alloc_i64((size_t)n);
    if (!out) return SUPER_NULL;
    int64_t at = 0;
    for (int64_t p = 0; p < n; ++p) {
      double x = (fvarr->kind == 1 && fvarr->f64s) ? fvarr->f64s[p]
               : (fvarr->kind == 0 && fvarr->items) ? super_as_f(fvarr->items[p]) : 0.0;
      int64_t y = (ivarr->kind == 2 && ivarr->i64s) ? ivarr->i64s[p]
                : (ivarr->kind == 0 && ivarr->items) ? super_as_i(ivarr->items[p]) : 0;
      if (!(x < fv) && y > iv) out->i64s[at++] = p;
    }
    out->len = (size_t)at;
    return super_arr(out);
  }
  if (strcmp(name, "sys_i64_gids_dense") == 0) {
    /* vals, span -> [gids, firsts, sizes] for non-neg keys in 0..span. */
    if (nargs < 2) return SUPER_NULL;
    SuperArr *vals = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t span = super_as_i(args[1]);
    if (!vals || span < 0) return SUPER_NULL;
    int64_t n = (int64_t)vals->len;
    SuperArr *gids = super_arr_alloc_i64((size_t)n);
    SuperArr *firsts = super_arr_alloc_i64(0);
    SuperArr *sizes = super_arr_alloc_i64(0);
    SuperArr *map = super_arr_alloc_i64((size_t)span);
    if (!gids || !firsts || !sizes || !map) return SUPER_NULL;
    for (int64_t row = 0; row < n; ++row) {
      int64_t slot = (vals->kind == 2 && vals->i64s) ? vals->i64s[row]
                   : (vals->kind == 0 && vals->items) ? super_as_i(vals->items[row]) : -1;
      if (slot < 0 || slot >= span) continue;
      int64_t g = map->i64s[slot] - 1;
      if (g < 0) {
        g = (int64_t)firsts->len;
        map->i64s[slot] = g + 1;
        super_arr_push(firsts, super_int(row));
        super_arr_push(sizes, super_int(0));
      }
      sizes->i64s[g] += 1;
      gids->i64s[row] = g;
    }
    SuperArr *pack = super_arr_alloc(3);
    super_arr_push(pack, super_arr(gids));
    super_arr_push(pack, super_arr(firsts));
    super_arr_push(pack, super_arr(sizes));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_join_dense_inner") == 0) {
    /* lkeys, rkeys, max_key -> [lidx, ridx] dense int inner equi-join. */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *lkeys = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *rkeys = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t max_key = super_as_i(args[2]);
    if (!lkeys || !rkeys || max_key < 0) return SUPER_NULL;
    int64_t span = max_key + 1;
    int64_t rn = (int64_t)rkeys->len;
    int64_t ln = (int64_t)lkeys->len;
    int64_t *head = (int64_t *)malloc((size_t)span * sizeof(int64_t));
    int64_t *nxt = (int64_t *)malloc((size_t)(rn > 0 ? rn : 1) * sizeof(int64_t));
    if (!head || !nxt) { free(head); free(nxt); return SUPER_NULL; }
    for (int64_t i = 0; i < span; ++i) head[i] = -1;
    for (int64_t i = 0; i < rn; ++i) nxt[i] = -1;
    int multi = 0;
    for (int64_t rrow = 0; rrow < rn; ++rrow) {
      int64_t key = (rkeys->kind == 2 && rkeys->i64s) ? rkeys->i64s[rrow]
                  : (rkeys->kind == 0 && rkeys->items) ? super_as_i(rkeys->items[rrow]) : -1;
      if (key < 0 || key >= span) continue;
      if (head[key] < 0) {
        head[key] = rrow;
      } else {
        multi = 1;
        int64_t t = head[key];
        while (nxt[t] >= 0) t = nxt[t];
        nxt[t] = rrow;
      }
    }
    int64_t n_out = 0;
    if (!multi) {
      for (int64_t lrow = 0; lrow < ln; ++lrow) {
        int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                    : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
        if (key >= 0 && key < span && head[key] >= 0) n_out += 1;
      }
      SuperArr *lidx = super_arr_alloc_i64((size_t)n_out);
      SuperArr *ridx = super_arr_alloc_i64((size_t)n_out);
      if (!lidx || !ridx) { free(head); free(nxt); return SUPER_NULL; }
      int64_t at = 0;
      for (int64_t lrow = 0; lrow < ln; ++lrow) {
        int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                    : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
        if (key >= 0 && key < span && head[key] >= 0) {
          lidx->i64s[at] = lrow;
          ridx->i64s[at] = head[key];
          at += 1;
        }
      }
      free(head);
      free(nxt);
      SuperArr *pack = super_arr_alloc(2);
      super_arr_push(pack, super_arr(lidx));
      super_arr_push(pack, super_arr(ridx));
      return super_arr(pack);
    }
    for (int64_t lrow = 0; lrow < ln; ++lrow) {
      int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                  : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
      if (key >= 0 && key < span) {
        int64_t rr = head[key];
        while (rr >= 0) { n_out += 1; rr = nxt[rr]; }
      }
    }
    SuperArr *lidx = super_arr_alloc_i64((size_t)n_out);
    SuperArr *ridx = super_arr_alloc_i64((size_t)n_out);
    if (!lidx || !ridx) { free(head); free(nxt); return SUPER_NULL; }
    int64_t at = 0;
    for (int64_t lrow = 0; lrow < ln; ++lrow) {
      int64_t key = (lkeys->kind == 2 && lkeys->i64s) ? lkeys->i64s[lrow]
                  : (lkeys->kind == 0 && lkeys->items) ? super_as_i(lkeys->items[lrow]) : -1;
      if (key >= 0 && key < span) {
        int64_t rr = head[key];
        while (rr >= 0) {
          lidx->i64s[at] = lrow;
          ridx->i64s[at] = rr;
          at += 1;
          rr = nxt[rr];
        }
      }
    }
    free(head);
    free(nxt);
    SuperArr *pack = super_arr_alloc(2);
    super_arr_push(pack, super_arr(lidx));
    super_arr_push(pack, super_arr(ridx));
    return super_arr(pack);
  }
  if (strcmp(name, "sys_i64_copy_range") == 0) {
    /* dst, dst_off, src, src_off, n — memcpy n i64 lanes into dst. */
    if (nargs < 5) return SUPER_NULL;
    SuperArr *dst = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *src = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    if (!dst || !src) return args[0];
    int64_t dst_off = super_as_i(args[1]);
    int64_t src_off = super_as_i(args[3]);
    int64_t n = super_as_i(args[4]);
    if (n <= 0 || dst_off < 0 || src_off < 0) return args[0];
    if (dst_off + n > (int64_t)dst->len) n = (int64_t)dst->len - dst_off;
    if (src_off + n > (int64_t)src->len) n = (int64_t)src->len - src_off;
    if (n <= 0) return args[0];
    for (int64_t i = 0; i < n; ++i) {
      int64_t v = 0;
      int64_t si = src_off + i;
      if (src->kind == 2 && src->i64s) v = src->i64s[si];
      else if (src->kind == 0 && src->items) v = super_as_i(src->items[si]);
      int64_t di = dst_off + i;
      if (dst->kind == 2 && dst->i64s) dst->i64s[di] = v;
      else if (dst->kind == 0 && dst->items) dst->items[di] = super_int(v);
    }
    return args[0];
  }
  if (strcmp(name, "sys_crc32_bytes_range") == 0) {
    /* data, lo, hi — IEEE CRC-32 of byte lanes data[lo..hi). */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *data = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t lo = super_as_i(args[1]);
    int64_t hi = super_as_i(args[2]);
    if (!data || lo < 0) lo = 0;
    if (hi > (int64_t)data->len) hi = (int64_t)data->len;
    if (hi <= lo) return super_int(0);
    /* Reuse string CRC table via a tiny stack buffer of raw bytes. */
    int64_t n = hi - lo;
    unsigned char *buf = (unsigned char *)malloc((size_t)n);
    if (!buf) return super_int(0);
    for (int64_t i = 0; i < n; ++i) {
      int64_t v = 0;
      int64_t si = lo + i;
      if (data->kind == 2 && data->i64s) v = data->i64s[si];
      else if (data->kind == 0 && data->items) v = super_as_i(data->items[si]);
      buf[i] = (unsigned char)(v & 255);
    }
    /* Inline IEEE CRC (same poly as sup_crc32_str). */
    static uint32_t tbl[256];
    static int ready = 0;
    if (!ready) {
      for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++) c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
        tbl[i] = c;
      }
      ready = 1;
    }
    uint32_t crc = 0xFFFFFFFFu;
    for (int64_t i = 0; i < n; ++i) crc = tbl[(crc ^ buf[i]) & 0xFFu] ^ (crc >> 8);
    free(buf);
    return super_int((int64_t)(crc ^ 0xFFFFFFFFu));
  }
  if (strcmp(name, "sys_i64_gather") == 0 || strcmp(name, "sys_i64_gather_dense") == 0) {
    /* dst, src, idx — dense i64 or boxed; dst[i] = src[idx[i]] (dense still clamps). */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *dst = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *src = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *ix = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    if (!dst || !src || !ix) return args[0];
    int64_t n = (int64_t)dst->len;
    if ((int64_t)ix->len < n) n = (int64_t)ix->len;
    int64_t sn = (int64_t)src->len;
    for (int64_t i = 0; i < n; ++i) {
      int64_t j = (ix->kind == 2 && ix->i64s) ? ix->i64s[i]
                : (ix->kind == 0 && ix->items) ? super_as_i(ix->items[i]) : -1;
      int64_t v = 0;
      if (j >= 0 && j < sn) {
        if (src->kind == 2 && src->i64s) v = src->i64s[j];
        else if (src->kind == 0 && src->items) v = super_as_i(src->items[j]);
      }
      if (dst->kind == 2 && dst->i64s) dst->i64s[i] = v;
      else if (dst->kind == 0 && dst->items) dst->items[i] = super_int(v);
    }
    return args[0];
  }
  if (strcmp(name, "sys_f64_gather") == 0 || strcmp(name, "sys_f64_gather_dense") == 0) {
    /* dst, src, idx — dense f64/i64 or boxed; float Col literals are often kind 0. */
    if (nargs < 3) return SUPER_NULL;
    SuperArr *dst = (args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    SuperArr *src = (args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    SuperArr *ix = (args[2].tag == SUPER_TAG_ARR) ? args[2].as.arr : nullptr;
    if (!dst || !src || !ix) return args[0];
    int64_t n = (int64_t)dst->len;
    if ((int64_t)ix->len < n) n = (int64_t)ix->len;
    int64_t sn = (int64_t)src->len;
    for (int64_t i = 0; i < n; ++i) {
      int64_t j = (ix->kind == 2 && ix->i64s) ? ix->i64s[i]
                : (ix->kind == 0 && ix->items) ? super_as_i(ix->items[i]) : -1;
      double v = 0.0;
      if (j >= 0 && j < sn) {
        if (src->kind == 1 && src->f64s) v = src->f64s[j];
        else if (src->kind == 0 && src->items) v = super_as_f(src->items[j]);
        else if (src->kind == 2 && src->i64s) v = (double)src->i64s[j];
      }
      if (dst->kind == 1 && dst->f64s) dst->f64s[i] = v;
      else if (dst->kind == 0 && dst->items) dst->items[i] = super_float(v);
    }
    return args[0];
  }
  if (strcmp(name, "sys_bytes_to_str") == 0) {
    /* Dense i64 byte ords / boxed ints → UTF-8 string (lossy). */
    if (nargs < 1) return super_str("");
    if (args[0].tag == SUPER_TAG_STR) return args[0];
    if (args[0].tag == SUPER_TAG_ARR) {
      SuperArr *a = args[0].as.arr;
      if (!a) return super_str("");
      size_t n = a->len;
      char *buf = (char *)sup_req_alloc(n + 1, 1);
      if (!buf) return super_str("");
      for (size_t i = 0; i < n; ++i) {
        int64_t v = 0;
        if (a->kind == 2 && a->i64s) v = a->i64s[i];
        else if (a->kind == 0 && a->items) v = super_as_i(a->items[i]);
        buf[i] = (char)(unsigned char)(v & 255);
      }
      buf[n] = 0;
      /* `super_str` borrows; keep `buf` as the owned payload. */
      return super_str(buf);
    }
    return super_str("");
  }
  if (strcmp(name, "__slice") == 0) {
    /* `base[start..end]`: indices are clamped like the VM (`as usize` then `min(len)`),
       strings slice by Unicode scalar, arrays copy and keep their dense kind. */
    if (nargs < 3) return nargs > 0 ? args[0] : SUPER_NULL;
    uint64_t us = (uint64_t)super_as_i(args[1]), ue = (uint64_t)super_as_i(args[2]);
    if (args[0].tag == SUPER_TAG_STR) {
      const char *src = args[0].as.s ? args[0].as.s : "";
      size_t nbytes = strlen(src);
      size_t chars = 0;
      for (size_t i = 0; i < nbytes; ++i) if (((unsigned char)src[i] & 0xC0) != 0x80) ++chars;
      uint64_t lo = us < chars ? us : chars;
      uint64_t hi = ue < chars ? ue : chars;
      if (hi < lo) hi = lo;
      size_t b_lo = nbytes, b_hi = nbytes, seen = 0;
      for (size_t i = 0; i < nbytes; ++i) {
        if (((unsigned char)src[i] & 0xC0) == 0x80) continue;
        if (seen == lo) b_lo = i;
        if (seen == hi) { b_hi = i; break; }
        ++seen;
      }
      if (lo == chars) b_lo = nbytes;
      if (b_hi < b_lo) b_hi = b_lo;
      char *o = (char *)sup_req_alloc(b_hi - b_lo + 1, 1);
      if (!o) return super_str("");
      memcpy(o, src + b_lo, b_hi - b_lo);
      o[b_hi - b_lo] = 0;
      return super_str(o);
    }
    if (args[0].tag == SUPER_TAG_ARR && args[0].as.arr) {
      SuperArr *a = args[0].as.arr;
      uint64_t lo = us < a->len ? us : a->len;
      uint64_t hi = ue < a->len ? ue : a->len;
      if (hi < lo) hi = lo;
      size_t n = (size_t)(hi - lo);
      if (a->kind == 1) {
        SuperArr *r = super_arr_alloc_f64(n);
        if (n) memcpy(r->f64s, a->f64s + lo, n * sizeof(double));
        return super_arr(r);
      }
      if (a->kind == 2) {
        SuperArr *r = super_arr_alloc_i64(n);
        if (n) memcpy(r->i64s, a->i64s + lo, n * sizeof(int64_t));
        return super_arr(r);
      }
      SuperArr *r = super_arr_alloc(n);
      for (size_t i = 0; i < n; ++i) super_arr_push(r, a->items[lo + i]);
      return super_arr(r);
    }
    return args[0];
  }
  if (strcmp(name, "round") == 0) {
    double v = nargs > 0 ? super_as_f(args[0]) : 0.0;
    int64_t places = nargs > 1 ? super_as_i(args[1]) : 0;
    double factor = pow(10.0, (double)places);
    return super_float(round(v * factor) / factor);
  }
  if (strcmp(name, "map") == 0 || strcmp(name, "keep") == 0 || strcmp(name, "group_by") == 0) {
    /* VM host stubs: closures are not reified, the receiver passes through. */
    return nargs > 0 ? args[0] : SUPER_NULL;
  }
  if (strcmp(name, "sum") == 0) {
    /* VM stub parity: only boxed arrays are summed; dense f64/i64 arrays pass through. */
    if (nargs < 1 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr || args[0].as.arr->kind != 0)
      return nargs > 0 ? args[0] : SUPER_NULL;
    const SuperArr *a = args[0].as.arr;
    double t = 0.0;
    for (size_t i = 0; i < a->len; ++i) t += super_as_f(a->items[i]);
    return super_float(t);
  }
  if (strcmp(name, "sys_arr_set") == 0) {
    if (nargs < 3 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr) return args[0];
    SuperArr *a = args[0].as.arr;
    int64_t idx = super_as_i(args[1]);
    if (idx >= 0 && (size_t)idx < a->len) {
      if (a->kind == 1) a->f64s[(size_t)idx] = super_as_f(args[2]);
      else if (a->kind == 2) a->i64s[(size_t)idx] = super_as_i(args[2]);
      else a->items[(size_t)idx] = args[2];
    }
    return args[0];
  }
  if (strcmp(name, "sys_arr_push") == 0) {
    if (nargs < 2 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr) return args[0];
    super_arr_push(args[0].as.arr, args[1]);
    return args[0];
  }
  if (strcmp(name, "sys_arr_len") == 0) {
    if (nargs < 1 || args[0].tag != SUPER_TAG_ARR || !args[0].as.arr) return super_int(0);
    return super_int((int64_t)args[0].as.arr->len);
  }
  if (strcmp(name, "__len") == 0 || strcmp(name, "len") == 0) {
    if (nargs < 1) return super_int(0);
    if (args[0].tag == SUPER_TAG_ARR && args[0].as.arr) return super_int((int64_t)args[0].as.arr->len);
    if (args[0].tag == SUPER_TAG_STR && args[0].as.s)
      return super_int((int64_t)strlen(args[0].as.s));
    return super_int(0);
  }
  if (strcmp(name, "__abs") == 0 || strcmp(name, "abs") == 0) {
    if (nargs < 1) return super_int(0);
    if (args[0].tag == SUPER_TAG_FLOAT) return super_float(fabs(args[0].as.f));
    int64_t v = super_as_i(args[0]);
    return super_int(v < 0 ? -v : v);
  }
  if (strcmp(name, "sys_str_len") == 0) {
    if (nargs < 1 || args[0].tag != SUPER_TAG_STR || !args[0].as.s) return super_int(0);
    /* character count approximating UTF-8 code points */
    const unsigned char *p = (const unsigned char *)args[0].as.s;
    int64_t n = 0;
    while (*p) {
      if ((*p & 0xC0) != 0x80) n++;
      p++;
    }
    return super_int(n);
  }
  if (strcmp(name, "sys_str_find") == 0 || strcmp(name, "sys_str_match_at") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR) ? args[0].as.s : "";
    const char *nd = (nargs > 1 && args[1].tag == SUPER_TAG_STR) ? args[1].as.s : "";
    int64_t at = nargs > 2 ? super_as_i(args[2]) : 0;
    if (name[8] == 'f') return super_int(sup_str_find(s, nd, at));
    return super_bool(sup_str_match_at(s, nd, at));
  }
  if (strcmp(name, "sys_str_find_lit_digits") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR) ? args[0].as.s : "";
    const char *lit = (nargs > 1 && args[1].tag == SUPER_TAG_STR) ? args[1].as.s : "";
    int64_t from = nargs > 2 ? super_as_i(args[2]) : 0;
    return super_int(sup_str_find_lit_digits(s, lit, from));
  }
  if (strcmp(name, "sys_str_lit_digits_end") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR) ? args[0].as.s : "";
    int64_t start = nargs > 1 ? super_as_i(args[1]) : 0;
    const char *lit = (nargs > 2 && args[2].tag == SUPER_TAG_STR) ? args[2].as.s : "";
    return super_int(sup_str_lit_digits_end(s, start, lit));
  }
  if (strcmp(name, "sys_ord") == 0) {
    if (nargs < 1 || args[0].tag != SUPER_TAG_STR || !args[0].as.s || !args[0].as.s[0])
      return super_int(-1);
    return super_int((unsigned char)args[0].as.s[0]);
  }
  if (strcmp(name, "sys_byte_at") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t i = nargs > 1 ? super_as_i(args[1]) : 0;
    return super_int(sup_byte_at(s, i));
  }
  if (strcmp(name, "sys_byte_len") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    return super_int(sup_byte_len(s));
  }
  if (strcmp(name, "sys_str_push_ascii") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t code = nargs > 1 ? super_as_i(args[1]) : 0;
    return super_str(sup_push_ascii(s, code));
  }
  if (strcmp(name, "sys_str_push_i64") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t v = nargs > 1 ? super_as_i(args[1]) : 0;
    return super_str(sup_push_i64(s, v));
  }
  if (strcmp(name, "sys_sb_new") == 0) {
    return super_int(sup_sb_new(nargs > 0 ? super_as_i(args[0]) : 64));
  }
  if (strcmp(name, "sys_sb_stack") == 0) {
    return super_int(sup_sb_stack(nargs > 0 ? super_as_i(args[0]) : 64));
  }
  if (strcmp(name, "sys_sb_lit") == 0) {
    int64_t h = nargs > 0 ? super_as_i(args[0]) : 0;
    const char *s = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    sup_sb_lit(h, s);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_litn") == 0) {
    int64_t h = nargs > 0 ? super_as_i(args[0]) : 0;
    const char *s = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    size_t pn = nargs > 2 ? (size_t)super_as_i(args[2]) : (s ? strlen(s) : 0);
    sup_sb_litn(h, s, pn);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_ascii") == 0) {
    int64_t h = nargs > 0 ? super_as_i(args[0]) : 0;
    int64_t code = nargs > 1 ? super_as_i(args[1]) : 0;
    sup_sb_ascii(h, code);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_i64") == 0) {
    int64_t h = nargs > 0 ? super_as_i(args[0]) : 0;
    int64_t v = nargs > 1 ? super_as_i(args[1]) : 0;
    sup_sb_i64(h, v);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_len") == 0) {
    return super_int(sup_sb_len(nargs > 0 ? super_as_i(args[0]) : 0));
  }
  if (strcmp(name, "sys_sb_clear") == 0) {
    sup_sb_clear(nargs > 0 ? super_as_i(args[0]) : 0);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_cat") == 0) {
    sup_sb_cat(nargs > 0 ? super_as_i(args[0]) : 0, nargs > 1 ? super_as_i(args[1]) : 0);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_sb_cstr") == 0) {
    return super_str(sup_sb_cstr(nargs > 0 ? super_as_i(args[0]) : 0));
  }
  if (strcmp(name, "sys_sb_finish") == 0) {
    return super_str(sup_sb_finish(nargs > 0 ? super_as_i(args[0]) : 0));
  }
  if (strcmp(name, "sys_byte_slice") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t lo = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t hi = nargs > 2 ? super_as_i(args[2]) : 0;
    return super_str(sup_byte_slice(s, lo, hi));
  }
  if (strcmp(name, "sys_byte_find") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *nd = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t from = nargs > 2 ? super_as_i(args[2]) : 0;
    return super_int(sup_byte_find(s, nd, from));
  }
  if (strcmp(name, "sys_byte_find_limit") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *nd = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t from = nargs > 2 ? super_as_i(args[2]) : 0;
    int64_t limit = nargs > 3 ? super_as_i(args[3]) : 0;
    return super_int(sup_byte_find_limit(s, nd, from, limit));
  }
  if (strcmp(name, "sys_byte_match_at") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *nd = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t at = nargs > 2 ? super_as_i(args[2]) : 0;
    return super_bool(sup_byte_match_at(s, nd, at));
  }
  if (strcmp(name, "sys_byte_eq_range") == 0) {
    const char *s = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t lo = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t hi = nargs > 2 ? super_as_i(args[2]) : 0;
    const char *t = (nargs > 3 && args[3].tag == SUPER_TAG_STR && args[3].as.s) ? args[3].as.s : "";
    return super_bool(sup_byte_eq_range(s, lo, hi, t));
  }
  if (strcmp(name, "sys_byte_eq_ranges") == 0) {
    const char *a = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    int64_t alo = nargs > 1 ? super_as_i(args[1]) : 0;
    int64_t ahi = nargs > 2 ? super_as_i(args[2]) : 0;
    const char *b = (nargs > 3 && args[3].tag == SUPER_TAG_STR && args[3].as.s) ? args[3].as.s : "";
    int64_t blo = nargs > 4 ? super_as_i(args[4]) : 0;
    int64_t bhi = nargs > 5 ? super_as_i(args[5]) : 0;
    return super_bool(sup_byte_eq_ranges(a, alo, ahi, b, blo, bhi));
  }
  if (strcmp(name, "sys_chr") == 0) {
    static char buf[8];
    int64_t cp = nargs > 0 ? super_as_i(args[0]) : 0;
    if (cp < 0) cp = 0;
    if (cp < 128) { buf[0] = (char)cp; buf[1] = 0; }
    else { snprintf(buf, sizeof(buf), "%c", (int)(cp & 0x7f)); }
    return super_str(buf);
  }
  if (strcmp(name, "sys_to_int") == 0 || strcmp(name, "to_int") == 0) {
    if (nargs < 1) return super_int(0);
    if (args[0].tag == SUPER_TAG_FLOAT) return super_int((int64_t)args[0].as.f);
    return super_int(super_as_i(args[0]));
  }
  if (strcmp(name, "sys_to_float") == 0 || strcmp(name, "to_float") == 0) {
    if (nargs < 1) return super_float(0.0);
    return super_float(super_as_f(args[0]));
  }
  if (strcmp(name, "sys_floor") == 0) return super_float(floor(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_ceil") == 0) return super_float(ceil(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_round") == 0) return super_float(round(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_sqrt") == 0) return super_float(sqrt(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_abs") == 0) {
    if (nargs && args[0].tag == SUPER_TAG_FLOAT) return super_float(fabs(args[0].as.f));
    int64_t v = nargs ? super_as_i(args[0]) : 0;
    return super_int(v < 0 ? -v : v);
  }
  if (strcmp(name, "sys_sin") == 0) return super_float(sin(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_cos") == 0) return super_float(cos(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_tan") == 0) return super_float(tan(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_asin") == 0) return super_float(asin(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_acos") == 0) return super_float(acos(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_atan") == 0) return super_float(atan(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_atan2") == 0)
    return super_float(atan2(nargs > 0 ? super_as_f(args[0]) : 0.0, nargs > 1 ? super_as_f(args[1]) : 0.0));
  if (strcmp(name, "sys_exp") == 0) return super_float(exp(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_ln") == 0) return super_float(log(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_log2") == 0) return super_float(log2(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_log10") == 0) return super_float(log10(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_pow") == 0)
    return super_float(pow(nargs > 0 ? super_as_f(args[0]) : 0.0, nargs > 1 ? super_as_f(args[1]) : 0.0));
  if (strcmp(name, "sys_fmod") == 0)
    return super_float(fmod(nargs > 0 ? super_as_f(args[0]) : 0.0, nargs > 1 ? super_as_f(args[1]) : 1.0));
  if (strcmp(name, "sys_hypot") == 0)
    return super_float(hypot(nargs > 0 ? super_as_f(args[0]) : 0.0, nargs > 1 ? super_as_f(args[1]) : 0.0));
  if (strcmp(name, "sys_cbrt") == 0) return super_float(cbrt(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_sinh") == 0) return super_float(sinh(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_cosh") == 0) return super_float(cosh(nargs ? super_as_f(args[0]) : 0.0));
  if (strcmp(name, "sys_mono_ns") == 0) {
#if defined(CLOCK_MONOTONIC)
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return super_int((int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec);
#else
    return super_int(0);
#endif
  }
  if (strcmp(name, "sys_unix_ns") == 0) {
#if defined(CLOCK_REALTIME)
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return super_int((int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec);
#else
    return super_int((int64_t)time(NULL) * 1000000000LL);
#endif
  }
  if (strcmp(name, "sys_crc32") == 0) {
    /* IEEE CRC32 over a string payload */
    static uint32_t tbl[256];
    static int ready = 0;
    if (!ready) {
      for (uint32_t i = 0; i < 256; i++) {
        uint32_t c = i;
        for (int k = 0; k < 8; k++) c = (c & 1u) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
        tbl[i] = c;
      }
      ready = 1;
    }
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    uint32_t crc = 0xFFFFFFFFu;
    for (const unsigned char *p = (const unsigned char *)s; *p; p++)
      crc = tbl[(crc ^ *p) & 0xFFu] ^ (crc >> 8);
    return super_int((int64_t)(crc ^ 0xFFFFFFFFu));
  }
  if (strcmp(name, "sys_sha256") == 0) {
    const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    return super_str(sup_sha256_hex(s));
  }
  if (strcmp(name, "sys_sha256_bytes") == 0) {
    SuperArr *ma = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t mn = (ma && ma->kind == 2 && ma->i64s) ? (int64_t)ma->len : 0;
    const int64_t *mp = (ma && ma->kind == 2) ? ma->i64s : nullptr;
    int64_t rn = 0;
    int64_t *dig = sup_sha256_i64(mp, mn, &rn);
    SuperArr *out = super_arr_alloc_i64((size_t)(rn > 0 ? rn : 32));
    if (out && dig && out->i64s) {
      for (int64_t i = 0; i < rn && (size_t)i < out->len; i++) out->i64s[i] = dig[i];
    }
    return super_arr(out);
  }
  if (strcmp(name, "sys_sha2_hex_of") == 0) {
    SuperArr *ma = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t mn = (ma && ma->kind == 2 && ma->i64s) ? (int64_t)ma->len : 0;
    const int64_t *mp = (ma && ma->kind == 2) ? ma->i64s : nullptr;
    return super_str(sup_sha2_hex_of_i64(mp, mn));
  }
  if (strcmp(name, "sys_hmac_sha256") == 0) {
    const char *k = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
    const char *m = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    return super_str(sup_hmac_sha256_hex(k, m));
  }
  if (strcmp(name, "sys_hmac_sha256_bytes") == 0) {
    /* Dense i64 key lanes; msg is i64 lanes or a raw string (UTF-8 / ASCII wire). */
    SuperArr *ka = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t kn = (ka && ka->kind == 2 && ka->i64s) ? (int64_t)ka->len : 0;
    const int64_t *kp = (ka && ka->kind == 2) ? ka->i64s : nullptr;
    int64_t rn = 0;
    int64_t *dig = nullptr;
    if (nargs > 1 && args[1].tag == SUPER_TAG_STR) {
      const char *ms = args[1].as.s ? args[1].as.s : "";
      dig = sup_hmac_sha256_i64_str(kp, kn, ms, &rn);
    } else {
      SuperArr *ma = (nargs > 1 && args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
      int64_t mn = (ma && ma->kind == 2 && ma->i64s) ? (int64_t)ma->len : 0;
      const int64_t *mp = (ma && ma->kind == 2) ? ma->i64s : nullptr;
      dig = sup_hmac_sha256_i64(kp, kn, mp, mn, &rn);
    }
    SuperArr *out = super_arr_alloc_i64((size_t)(rn > 0 ? rn : 32));
    if (out && dig && out->i64s) {
      for (int64_t i = 0; i < rn && (size_t)i < out->len; i++) out->i64s[i] = dig[i];
    }
    return super_arr(out);
  }
  if (strcmp(name, "sys_hmac_sha256_bytes_range") == 0) {
    SuperArr *ka = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t kn = (ka && ka->kind == 2 && ka->i64s) ? (int64_t)ka->len : 0;
    const int64_t *kp = (ka && ka->kind == 2) ? ka->i64s : nullptr;
    const char *ms = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t lo = nargs > 2 ? super_as_i(args[2]) : 0;
    int64_t hi = nargs > 3 ? super_as_i(args[3]) : 0;
    int64_t rn = 0;
    int64_t *dig = sup_hmac_sha256_i64_str_range(kp, kn, ms, lo, hi, &rn);
    SuperArr *out = super_arr_alloc_i64((size_t)(rn > 0 ? rn : 32));
    if (out && dig && out->i64s) {
      for (int64_t i = 0; i < rn && (size_t)i < out->len; i++) out->i64s[i] = dig[i];
    }
    return super_arr(out);
  }
  if (strcmp(name, "sys_sha2_hex_prefix_eq") == 0) {
    SuperArr *ma = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    const int64_t *mp = (ma && ma->kind == 2) ? ma->i64s : nullptr;
    int64_t nbytes = nargs > 1 ? super_as_i(args[1]) : 0;
    const char *tx = (nargs > 2 && args[2].tag == SUPER_TAG_STR && args[2].as.s) ? args[2].as.s : "";
    int64_t lo = nargs > 3 ? super_as_i(args[3]) : 0;
    if (!mp || nbytes < 0 || (ma && (int64_t)ma->len < nbytes)) return super_bool(false);
    return super_bool(sup_sha2_hex_prefix_eq(mp, nbytes, tx, lo));
  }
  if (strcmp(name, "sys_hmac_sha256_hex_prefix_eq_range") == 0) {
    SuperArr *ka = (nargs > 0 && args[0].tag == SUPER_TAG_ARR) ? args[0].as.arr : nullptr;
    int64_t kn = (ka && ka->kind == 2 && ka->i64s) ? (int64_t)ka->len : 0;
    const int64_t *kp = (ka && ka->kind == 2) ? ka->i64s : nullptr;
    const char *ms = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
    int64_t lo = nargs > 2 ? super_as_i(args[2]) : 0;
    int64_t hi = nargs > 3 ? super_as_i(args[3]) : 0;
    int64_t nbytes = nargs > 4 ? super_as_i(args[4]) : 0;
    const char *tx = (nargs > 5 && args[5].tag == SUPER_TAG_STR && args[5].as.s) ? args[5].as.s : "";
    int64_t tlo = nargs > 6 ? super_as_i(args[6]) : 0;
    return super_bool(sup_hmac_sha256_hex_prefix_eq_range(kp, kn, ms, lo, hi, nbytes, tx, tlo));
  }
  if (strcmp(name, "sys_hmac_sha256_sig8_into_range") == 0) {
    int64_t sb = nargs > 0 ? super_as_i(args[0]) : 0;
    SuperArr *ka = (nargs > 1 && args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t kn = (ka && ka->kind == 2 && ka->i64s) ? (int64_t)ka->len : 0;
    const int64_t *kp = (ka && ka->kind == 2) ? ka->i64s : nullptr;
    const char *ms = (nargs > 2 && args[2].tag == SUPER_TAG_STR && args[2].as.s) ? args[2].as.s : "";
    int64_t lo = nargs > 3 ? super_as_i(args[3]) : 0;
    int64_t hi = nargs > 4 ? super_as_i(args[4]) : 0;
    sup_hmac_sha256_sig8_into_range(sb, kp, kn, ms, lo, hi);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_hmac_sha256_sig8_forged_into_range") == 0) {
    int64_t sb = nargs > 0 ? super_as_i(args[0]) : 0;
    SuperArr *ka = (nargs > 1 && args[1].tag == SUPER_TAG_ARR) ? args[1].as.arr : nullptr;
    int64_t kn = (ka && ka->kind == 2 && ka->i64s) ? (int64_t)ka->len : 0;
    const int64_t *kp = (ka && ka->kind == 2) ? ka->i64s : nullptr;
    const char *ms = (nargs > 2 && args[2].tag == SUPER_TAG_STR && args[2].as.s) ? args[2].as.s : "";
    int64_t lo = nargs > 3 ? super_as_i(args[3]) : 0;
    int64_t hi = nargs > 4 ? super_as_i(args[4]) : 0;
    sup_hmac_sha256_sig8_forged_into_range(sb, kp, kn, ms, lo, hi);
    return SUPER_NULL;
  }
  if (strcmp(name, "sys_crc32_sb") == 0) {
    return super_int(sup_crc32_sb(nargs > 0 ? super_as_i(args[0]) : 0));
  }
  /* TCP, signals, stdio color, epoll — see aot_host::NET_HOST_C (NULL = not handled). */
  {
    const char *hn = name;
    if (strncmp(name, "host.", 5) == 0) hn = name + 5;
    SuperVal net = super_host_net(hn, args, nargs);
    if (net.tag != SUPER_TAG_NULL) return net;
  }
  /* Terminal is a real tty. GUI opens an X11 window when a display exists. */
  {
    const char *hn = name;
    if (strncmp(name, "host.", 5) == 0) hn = name + 5;
    if (strcmp(hn, "sys_term_cols") == 0 || strcmp(hn, "sys_term_rows") == 0) {
#if defined(_WIN32) || defined(__wasm__) || defined(__wasi__)
      return super_int(strcmp(hn, "sys_term_cols") == 0 ? 80 : 24);
#else
      struct winsize ws;
      memset(&ws, 0, sizeof ws);
      if (ioctl(STDIN_FILENO, TIOCGWINSZ, &ws) == 0) {
        int n = strcmp(hn, "sys_term_cols") == 0 ? (int)ws.ws_col : (int)ws.ws_row;
        if (n > 0) return super_int(n);
      }
      return super_int(strcmp(hn, "sys_term_cols") == 0 ? 80 : 24);
#endif
    }
#if defined(_WIN32) || defined(__wasm__) || defined(__wasi__)
    if (strcmp(hn, "sys_term_isatty") == 0) return super_bool(0);
#else
    if (strcmp(hn, "sys_term_isatty") == 0) return super_bool(isatty(STDIN_FILENO));
#endif
    if (strcmp(hn, "sys_term_raw") == 0)
      return super_bool(super_term_raw(nargs > 0 && super_as_i(args[0]) != 0));
    if (strcmp(hn, "sys_term_read") == 0)
      return super_term_read(nargs > 0 ? super_as_i(args[0]) : 1);
    if (strcmp(hn, "sys_term_written") == 0)
      return super_str(g_term_written ? g_term_written : "");
    if (strcmp(hn, "sys_tui_diff") == 0) return super_str("");
    if (strcmp(hn, "sys_term_write") == 0) {
      const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
      size_t n = strlen(s);
      if (n) {
        fwrite(s, 1, n, stdout);
        fflush(stdout);
        super_term_capture(s, n);
      }
      return super_int((int64_t)n);
    }
    if (strcmp(hn, "sys_term_feed") == 0) {
      const char *s = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
      for (const char *p = s; *p && g_term_fn < (int)sizeof g_term_feed; p++) g_term_feed[g_term_fn++] = *p;
      return super_bool(1);
    }
    if (strcmp(hn, "sys_term_reset") == 0) {
      super_term_reset();
      return super_bool(1);
    }
    if (strcmp(hn, "sys_write_str") == 0) {
      const char *path = (nargs && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
      const char *body = (nargs > 1 && args[1].tag == SUPER_TAG_STR && args[1].as.s) ? args[1].as.s : "";
      if (!path[0]) return super_err("empty path");
      FILE *fp = fopen(path, "wb");
      if (!fp) return super_err("cannot write");
      fputs(body, fp);
      if (fclose(fp) != 0) return super_err("cannot write");
      return super_bool(1);
    }
    if (strcmp(hn, "sys_sleep_ms") == 0 || strcmp(hn, "sys_sleep_ns") == 0) {
      int64_t n = nargs > 0 ? super_as_i(args[0]) : 0;
      if (n < 0) n = 0;
      struct timespec ts;
      if (strcmp(hn, "sys_sleep_ms") == 0) {
        ts.tv_sec = (time_t)(n / 1000);
        ts.tv_nsec = (long)((n % 1000) * 1000000L);
      } else {
        ts.tv_sec = (time_t)(n / 1000000000LL);
        ts.tv_nsec = (long)(n % 1000000000LL);
      }
      nanosleep(&ts, NULL);
      return super_bool(1);
    }
    if (strcmp(hn, "sys_window_open") == 0) {
      const char *title = (nargs > 2 && args[2].tag == SUPER_TAG_STR) ? args[2].as.s : "";
      return super_gui_window(nargs > 0 ? (int)super_as_i(args[0]) : 1,
                              nargs > 1 ? (int)super_as_i(args[1]) : 1, title);
    }
    if (strcmp(hn, "sys_window_poll") == 0) return super_gui_poll();
    if (strcmp(hn, "sys_gui_open") == 0) {
      const char *title = (nargs > 2 && args[2].tag == SUPER_TAG_STR) ? args[2].as.s : "";
      g_gui_quit = 0;
      return super_bool(super_gui_open_provider(nargs > 0 ? (int)super_as_i(args[0]) : 1,
                                               nargs > 1 ? (int)super_as_i(args[1]) : 1, title));
    }
    if (strcmp(hn, "sys_gui_reset") == 0) {
      super_provider_close();
      super_x_close();
      free(g_gui_script); g_gui_script = 0;
      free(g_gui_shown); g_gui_shown = 0;
      free(g_gui_px); g_gui_px = 0;
      g_gui_w = g_gui_h = g_gui_quit = g_gui_mx = g_gui_my = g_gui_down = 0;
      for (int i = 0; i < g_gui_kn; i++) free(g_gui_keys[i]);
      g_gui_kn = g_gui_kr = 0;
      return super_bool(1);
    }
    if (strcmp(hn, "sys_gui_present") == 0) {
      const char *s = (nargs > 2 && args[2].tag == SUPER_TAG_STR && args[2].as.s) ? args[2].as.s : "";
      super_gui_present(nargs > 0 ? (int)super_as_i(args[0]) : g_gui_w,
                        nargs > 1 ? (int)super_as_i(args[1]) : g_gui_h, s);
      return super_bool(1);
    }
    if (strcmp(hn, "sys_gui_pixel") == 0) {
      int x = nargs > 0 ? (int)super_as_i(args[0]) : 0;
      int y = nargs > 1 ? (int)super_as_i(args[1]) : 0;
      if (!g_gui_px || x < 0 || y < 0 || x >= g_gui_w || y >= g_gui_h) return super_int(0);
      return super_int((int64_t)g_gui_px[(size_t)y * (size_t)g_gui_w + (size_t)x]);
    }
    if (strcmp(hn, "sys_gui_last") == 0) return super_str(g_gui_script ? g_gui_script : "");
    if (strcmp(hn, "sys_gui_quit") == 0) return super_bool(g_gui_quit);
    if (strcmp(hn, "sys_gui_mouse_x") == 0) return super_int(g_gui_mx);
    if (strcmp(hn, "sys_gui_mouse_y") == 0) return super_int(g_gui_my);
    if (strcmp(hn, "sys_gui_mouse_down") == 0) return super_int(g_gui_down);
    if (strcmp(hn, "sys_gui_click") == 0) {
      static char buf[256];
      buf[0] = 0;
      super_provider_take_click(buf, (int)sizeof buf);
      return super_str(buf);
    }
    if (strcmp(hn, "sys_gui_value") == 0) {
      static char buf[1024];
      const char *id = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
      buf[0] = 0;
      super_provider_value(id, buf, (int)sizeof buf);
      return super_str(buf);
    }
    if (strcmp(hn, "sys_gui_backend") == 0) {
      const char *backend = super_provider_backend();
      return super_str(backend ? backend : "headless");
    }
    if (strcmp(hn, "sys_gui_key") == 0) {
      if (g_gui_kr >= g_gui_kn) return super_str("");
      const char *k = g_gui_keys[g_gui_kr] ? g_gui_keys[g_gui_kr] : "";
      g_gui_kr += 1;
      return super_str(k);
    }
    if (strcmp(hn, "sys_gui_feed_key") == 0) {
      if (g_gui_kn < 64) {
        const char *k = (nargs > 0 && args[0].tag == SUPER_TAG_STR && args[0].as.s) ? args[0].as.s : "";
        g_gui_keys[g_gui_kn++] = super_strdup(k);
      }
      return super_bool(1);
    }
    if (strcmp(hn, "sys_gui_feed_mouse") == 0) {
      g_gui_mx = nargs > 0 ? (int)super_as_i(args[0]) : 0;
      g_gui_my = nargs > 1 ? (int)super_as_i(args[1]) : 0;
      g_gui_down = nargs > 2 ? (int)super_as_i(args[2]) : 0;
      return super_bool(1);
    }
    if (strcmp(hn, "sys_gui_feed_quit") == 0) {
      g_gui_quit = nargs > 0 && super_as_i(args[0]) != 0;
      return super_bool(1);
    }
    if (strcmp(hn, "sys_tui_plain") == 0) {
      int64_t w = nargs > 0 ? super_as_i(args[0]) : 0;
      int64_t h = nargs > 1 ? super_as_i(args[1]) : 0;
      if (w < 0) w = 0;
      if (h < 0) h = 0;
      const int64_t *cp = NULL;
      size_t ncp = 0;
      if (nargs > 2 && args[2].tag == SUPER_TAG_ARR && args[2].as.arr && args[2].as.arr->kind == 2 && args[2].as.arr->i64s) {
        cp = args[2].as.arr->i64s;
        ncp = args[2].as.arr->len;
      }
      size_t cap = (size_t)w * (size_t)h * 4u + (size_t)h + 1u;
      char *o = (char *)malloc(cap ? cap : 1);
      if (!o) return super_str("");
      size_t n = 0;
      for (int64_t y = 0; y < h; y++) {
        if (y) o[n++] = '\n';
        for (int64_t x = 0; x < w; x++) {
          size_t i = (size_t)(y * w + x);
          uint32_t c = 32;
          if (cp && i < ncp && cp[i] > 0) c = (uint32_t)cp[i];
          if (c > 0x10FFFFu) c = 32;
          if (c < 0x80u) o[n++] = (char)c;
          else if (c < 0x800u) {
            o[n++] = (char)(0xC0u | (c >> 6));
            o[n++] = (char)(0x80u | (c & 0x3Fu));
          } else if (c < 0x10000u) {
            o[n++] = (char)(0xE0u | (c >> 12));
            o[n++] = (char)(0x80u | ((c >> 6) & 0x3Fu));
            o[n++] = (char)(0x80u | (c & 0x3Fu));
          } else {
            o[n++] = (char)(0xF0u | (c >> 18));
            o[n++] = (char)(0x80u | ((c >> 12) & 0x3Fu));
            o[n++] = (char)(0x80u | ((c >> 6) & 0x3Fu));
            o[n++] = (char)(0x80u | (c & 0x3Fu));
          }
        }
      }
      o[n] = 0;
      return super_str(o);
    }
  }
  fprintf(stderr, "super host call not linked in C build: %s\n", name);
  return SUPER_NULL;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use std::path::PathBuf;
    use super_core::SourceFile;
    use super_frontend::analyze;

    #[test]
    fn c_field_names_escape_keywords() {
        assert_eq!(sanitize_ident("short"), "short_");
        assert_eq!(sanitize_ident("int"), "int_");
        assert_eq!(sanitize_ident("balance"), "balance");
        assert_eq!(sanitize_ident("content-type"), "content_type");
    }

    #[test]
    fn emit_c_wires_escape_analysis_and_omits_malloc_trim() {
        // Synthetic chunk: MakeStruct at pc 0 tagged Stay (region 1).
        let mut interner = super_core::Interner::default();
        let name = interner.intern("Point");
        let mut chunk = Chunk {
            ops: vec![
                Op::MakeStruct(Some(name), 0),
                Op::Halt,
            ],
            consts: vec![],
            functions: vec![],
            tests: vec![],
            opt_notes: vec![],
            mut_writebacks: vec![],
            escape_birth: vec![(0, 1)],
        };
        let c = emit_c(&chunk, &interner, "escape_wire");
        assert!(
            c.contains("escape_analysis:"),
            "emitted C must record escape_analysis banner"
        );
        assert!(
            c.contains("tagged_births=1"),
            "banner must count tagged births"
        );
        assert!(
            !c.contains("malloc_trim(") && !c.contains("include <malloc.h>"),
            "Round 8: malloc_trim call / malloc.h must not appear in emitted C"
        );
        assert!(
            c.contains("super_obj_alloc_region") && c.contains("SUP_REG_REQ"),
            "Stay-tagged MakeStruct must emit request-arena alloc_region; got snippet:\n{}",
            c.lines()
                .filter(|l| l.contains("super_obj") || l.contains("escape_analysis"))
                .take(20)
                .collect::<Vec<_>>()
                .join("\n")
        );
        // Promote/Conservative → durable
        chunk.escape_birth = vec![(0, 2)];
        let c2 = emit_c(&chunk, &interner, "escape_wire");
        assert!(
            c2.contains("super_obj_alloc_durable"),
            "Promote/Conservative must birth durable; snippet:\n{}",
            c2.lines()
                .filter(|l| l.contains("super_obj") || l.contains("escape_analysis"))
                .take(20)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn keyword_fields_are_renamed_in_emitted_c_and_still_run() {
        let src = r#"
+Word = {
 short: i64
 int: i64
 const: i64
 restrict: i64
 volatile: i64
 signed: i64
 unsigned: i64
 extern: i64
 static: i64
 typedef: i64
 thread_local: i64
 typeof_unqual: i64
 _Decimal32: i64
 _Float64: i64
 _Noreturn: i64
 _Imaginary: i64
}
+Flag = {
 int
 short(const: i64)
}
+main() {
 w = Word{short: 1, int: 2, const: 3, restrict: 4, volatile: 5, signed: 6, unsigned: 7, extern: 8, static: 9, typedef: 10, thread_local: 11, typeof_unqual: 12, _Decimal32: 13, _Float64: 14, _Noreturn: 15, _Imaginary: 16}
 print(w.short + w.int + w.const + w._Float64)
}
"#;
        let file = SourceFile::new("keywords.sr", src);
        let mut result = analyze(&file);
        assert!(
            !result.diagnostics.has_errors(),
            "{:?}",
            result.diagnostics.iter().collect::<Vec<_>>()
        );
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, true)
            .with_opt_meta(meta)
            .compile(&result.module);
        let c = emit_c_module(
            &chunk,
            &result.interner,
            "keywords",
            EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
        );
        for word in [
            "short",
            "int",
            "const",
            "restrict",
            "volatile",
            "signed",
            "unsigned",
            "extern",
            "static",
            "typedef",
            "thread_local",
            "typeof_unqual",
            "_Decimal32",
            "_Float64",
            "_Noreturn",
            "_Imaginary",
        ] {
            assert!(
                c.contains(&format!("{word}_")),
                "emitted C did not rename `{word}`"
            );
            assert!(
                !c.lines().any(|line| {
                    let trimmed = line.trim();
                    trimmed.ends_with(&format!(" {word};")) || trimmed == format!("{word};")
                }),
                "emitted C still declares bare keyword `{word}`"
            );
        }
        if !cc_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("super_keywords_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("keywords");
        let opts = CCompileOptions {
            out_bin: Some(&bin),
            fast_math: false,
            max_speed: false,
            lto: LtoMode::Off,
            opt: CodegenOpt::O0,
            target_cpu: None,
            ..CCompileOptions::default()
        };
        write_and_maybe_compile_with(&c, &dir.join("keywords.c"), true, &opts)
            .unwrap_or_else(|err| panic!("{err}"));
        let out = std::process::Command::new(&bin).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "keyword binary failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "20\n");
    }

    #[test]
    fn imported_struct_and_sum_typedefs_precede_their_users() {
        let dir = std::env::temp_dir().join(format!("super_abi_order_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("payload.sr"),
            "+Payload = { n: i64 = 1 }\n+Mode = {\n On\n Off\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.sr"),
            "use payload\n+Holder = {\n value: Payload\n flag: Mode\n}\n+main() { print(4) }\n",
        )
        .unwrap();
        let mut result = super_frontend::analyze_project(&dir.join("main.sr"));
        assert!(
            !result.diagnostics.has_errors(),
            "{:?}",
            result.diagnostics.iter().collect::<Vec<_>>()
        );
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, true)
            .with_opt_meta(meta)
            .compile(&result.module);
        let c = emit_c_module(
            &chunk,
            &result.interner,
            "order",
            EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
        );
        let payload = c
            .find("typedef struct SuperPayload")
            .expect("Payload typedef");
        let mode = c.find("typedef struct SuperMode").expect("Mode typedef");
        let holder = c
            .find("typedef struct SuperHolder")
            .expect("Holder typedef");
        assert!(
            payload < holder,
            "Payload must be complete before Holder\n{c}"
        );
        assert!(mode < holder, "Mode must be complete before Holder\n{c}");
        if !cc_available() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let bin = dir.join("order");
        let opts = CCompileOptions {
            out_bin: Some(&bin),
            fast_math: false,
            max_speed: false,
            lto: LtoMode::Off,
            opt: CodegenOpt::O0,
            target_cpu: None,
            ..CCompileOptions::default()
        };
        write_and_maybe_compile_with(&c, &dir.join("order.c"), true, &opts)
            .unwrap_or_else(|err| panic!("{err}\n{c}"));
        let out = std::process::Command::new(&bin).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "order binary failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "4\n");
    }

    fn compile_chunk(
        src: &str,
        release: bool,
    ) -> (
        crate::bytecode::Chunk,
        super_core::Interner,
        super_frontend::Module,
    ) {
        let file = SourceFile::new("t.sr", src);
        let mut result = analyze(&file);
        assert!(
            !result.diagnostics.has_errors(),
            "{:?}",
            result.diagnostics.iter().collect::<Vec<_>>()
        );
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, release)
            .with_opt_meta(meta)
            .compile(&result.module);
        (chunk, std::mem::take(&mut result.interner), result.module)
    }

    #[test]
    fn law_and_proof_items_emit_zero_runtime_symbols() {
        let src = r#"
#law("add is commutative")
add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}

#proof(add_comm)
prove_add_comm(a: i64, b: i64) {
    #assert(a + b == b + a)
}

+main() { return 0 }
"#;
        let (chunk, interner, module) = compile_chunk(src, true);
        assert!(
            chunk.functions.iter().all(|f| {
                let n = interner.resolve(f.name);
                n != "add_comm" && n != "prove_add_comm"
            }),
            "bytecode must not contain verify-only functions"
        );
        let c = emit_c_module(
            &chunk,
            &interner,
            "t",
            EmitCOptions {
                typed: None,
                module: Some(&module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
        );
        assert!(
            !c.contains("add_comm") && !c.contains("prove_add_comm"),
            "C23 release emit must strip #law/#proof symbols"
        );
    }

    #[test]
    fn c_host_links_sys_unix_ns_and_allows_ephemeral_port() {
        let src = r#"
main() {
    ns = host.sys_unix_ns()
    ensure ns > 0 else {
        print("bad clock")
        return 1
    }
    fd = host.sys_tcp_listen("127.0.0.1:0") or 0 - 1
    ensure fd >= 0 else {
        print("listen failed")
        return 2
    }
    addr = host.sys_fd_local_addr(fd) or "none"
    host.sys_fd_close(fd)
    print(addr)
    0
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "clock_port");
        assert!(
            c.contains("sys_unix_ns"),
            "C host must implement wall clock"
        );
        assert!(
            c.contains("port < 0 || port > 65535") || c.contains("port < 0"),
            "ephemeral port 0 must be accepted"
        );
        if let Some((_, out)) = build_and_run(src) {
            let line = out.lines().next().unwrap_or("");
            assert!(
                line.starts_with("127.0.0.1:") && line != "127.0.0.1:0",
                "expected assigned ephemeral addr, got {line:?}"
            );
        }
    }

    #[test]
    fn aot_main_return_code_becomes_process_exit() {
        // `+main` with `or err` over a boxed host result — its return value is the exit code.
        let src = r#"
+main() {
    x = host.sys_tcp_listen("not-a-port") or err { return 7 }
    host.sys_fd_close(x)
    0
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "exit_code");
        assert!(
            c.contains("return super_u_module()")
                || c.contains("return super_int(super_u_module())")
                || c.contains("return sp > 0 ? stack[sp - 1] : SUPER_NULL"),
            "module wrapper must forward +main's return value"
        );
        if !cc_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("super_exit_code_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let bin = dir.join("t.out");
        let opts = CCompileOptions {
            out_bin: Some(&bin),
            ..CCompileOptions::default()
        };
        write_and_maybe_compile_with(&c, &dir.join("t.c"), true, &opts)
            .unwrap_or_else(|e| panic!("{e}\n--- C ---\n{c}"));
        let status = std::process::Command::new(&bin).status().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(status.code(), Some(7), "expected exit 7, got {status:?}");
    }

    #[test]
    fn aot_bind_errors_include_address_and_errno_text() {
        let src = r#"
+main() {
    a = host.sys_tcp_listen("127.0.0.1:0") or err {
        print(err)
        return 1
    }
    addr = host.sys_fd_local_addr(a) or "x"
    clash = host.sys_tcp_listen(addr) or err {
        print(err)
        host.sys_fd_close(a)
        return 0
    }
    host.sys_fd_close(a)
    host.sys_fd_close(clash)
    0
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "bind_err");
        assert!(
            c.contains("super_sys_err") || c.contains("strerror"),
            "C host must format bind failures with strerror"
        );
        if let Some((_, out)) = build_and_run(src) {
            let line = out.lines().next().unwrap_or("");
            assert!(
                line.contains("bind") && line.contains("127.0.0.1:"),
                "expected descriptive bind error, got {line:?}"
            );
            assert!(
                line.to_ascii_lowercase().contains("address already in use")
                    || line.contains("EADDRINUSE")
                    || line.contains("errno"),
                "expected OS reason in bind error, got {line:?}"
            );
        }
    }

    #[test]
    fn emit_c_contains_main_and_loopstep_lowering() {
        let src = r#"
+add(a: i64, b: i64)  => a + b
main() {
    s = 0
    for 0..10 { s = s + it }
    print(add(s, 1))
}
"#;
        let file = SourceFile::new("t.sr", src);
        let mut result = analyze(&file);
        assert!(!result.diagnostics.has_errors());
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, false)
            .with_opt_meta(meta)
            .compile(&result.module);
        let c = emit_c(&chunk, &result.interner, "t");
        assert!(c.contains("int main(int argc, char **argv)"));
        assert!(c.contains("SuperVal"), "runtime prelude missing");
        assert!(c.contains("#include <stdbool.h>"));
        assert!(
            c.contains("include <stdbit.h>"),
            "emitted C must still request stdbit when the host provides it"
        );
        assert!(c.contains("[[unsequenced]]"));
        assert!(c.contains("[[reproducible]]"));
        assert!(c.contains("sup_stack_alloc"));
        assert!(
            c.contains("super_add") || c.contains("super_fn_") || c.contains("as.i +="),
            "expected arithmetic / LoopStep lowering"
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cc_available() -> bool {
        std::process::Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Emit C for `src`, compile it with `cc -O3 -std=c23` and return `(c_source, stdout)`.
    /// `None` when no C compiler is installed (execution tests are skipped).
    /// GCC 15 deletes the `memcpy` into `sys_bytes_concat` / `sys_bytes_slice`
    /// buffers when `super_host_net` also reads a `volatile` object, so every
    /// served request head reads back empty.
    #[test]
    fn net_host_has_no_volatile_objects() {
        let src = crate::aot_host::NET_HOST_C;
        let code: Vec<&str> = src
            .lines()
            .filter(|l| l.contains("volatile ") && l.trim_end().ends_with(';'))
            .collect();
        assert!(code.is_empty(), "volatile in aot_host NET_HOST_C: {code:?}");
        assert!(src.contains("static _Atomic int g_sig_last"));
    }

    fn build_and_run(src: &str) -> Option<(String, String)> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "t");
        if !cc_available() {
            return None;
        }
        let dir = std::env::temp_dir().join(format!(
            "super_unboxed_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("t.out");
        let opts = CCompileOptions {
            out_bin: Some(&bin),
            ..CCompileOptions::default()
        };
        write_and_maybe_compile_with(&c, &dir.join("t.c"), true, &opts)
            .unwrap_or_else(|e| panic!("{e}\n--- C ---\n{c}"));
        let out = std::process::Command::new(&bin).output().unwrap();
        assert!(
            out.status.success(),
            "binary failed: {:?}\nsource: {}\nstderr: {}",
            out.status,
            dir.join("t.c").display(),
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&dir);
        Some((c, String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    #[test]
    fn aggregates_display_and_compare_the_same_on_vm_and_c() {
        let src = r#"
main() {
    scores = [10, 20, 30]
    ports = {http: 80, https: 443}
    print("scores {scores}")
    print("ports {ports}")
    print(scores == [10, 20, 30])
    print([1, 2] == [1, 3])
}
"#;
        let vm = vm_output(src);
        assert_eq!(
            vm,
            "scores [10, 20, 30]\nports {http: 80, https: 443}\ntrue\nfalse\n"
        );
        let Some((_, c_out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(c_out, vm);
    }

    #[test]
    fn string_slices_stay_unboxed_and_match_the_vm() {
        let src = r#"
main() {
    p = "/ab/:c"
    u = "héllo→w"
    for 0..2 {
        p = p + "x"
        u = u + "!"
    }
    hi, neg = 30, -2
    print("[{p[1..3]}] [{p[0..0]}] [{p[5..hi]}] [{p[4..2]}] [{p[hi..hi]}] [{p[neg..3]}] [{p[1..neg]}]")
    print("[{u[1..4]}] [{u[5..7]}] [{u[6..hi]}]")
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "[ab] [] [cxx] [] [] [] [ab/:cxx]\n[éll] [→w] [w!!]\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(
            c.contains("sup_str_slice("),
            "slice not lowered to the typed helper"
        );
        assert!(!c.contains("boxed fallback: main"), "main boxed:\n{c}");
        assert_eq!(c_out, vm);
    }

    #[test]
    fn iterating_a_returned_struct_array_stays_unboxed() {
        let src = r#"
Hit = {
    index: i64 = 0
    score: f64 = 0.0
}
search(k: i64) {
    hits = []
    for 0..k { hits = host.sys_arr_push(hits, Hit.{index: it * 2, score: 0.5 + it}) }
    hits
}
main() {
    total = 0
    for search(3) { total += it.index }
    for h in search(2) { print("{h.index} {h.score}") }
    print(total)
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "0 0.5\n2 1.5\n6\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(!c.contains("boxed fallback: main"), "main boxed:\n{c}");
        assert_eq!(c_out, vm);
    }

    #[test]
    fn declared_display_and_add_dispatch_on_both_backends() {
        let src = r#"
+Vec2: Add, Display = {
    x: f64 = 0.0
    y: f64 = 0.0
    +display(self) : str => "({self.x}, {self.y})"
    +add(self, other: Vec2) : Vec2 => Vec2.{x: self.x + other.x, y: self.y + other.y}
}
main() {
    a = Vec2.{x: 1.0, y: 2.0}
    b = Vec2.{x: 3.0, y: 4.0}
    c = a + b
    print("{c}")
}
"#;
        let (chunk, _, _) = compile_chunk(src, true);
        assert!(
            chunk
                .opt_notes
                .iter()
                .any(|note| matches!(note.kind, crate::bytecode::OptNoteKind::TraitDispatch)),
            "missing TraitDispatch note: {:?}",
            chunk.opt_notes
        );
        let vm = vm_output(src);
        assert_eq!(vm, "(4, 6)\n");
        let Some((_, c_out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(c_out, vm);
    }

    /// Run `src` through the bytecode VM and return the printed lines.
    fn vm_output(src: &str) -> String {
        let (chunk, interner, _) = compile_chunk(src, true);
        let mut stacks = crate::stacks::StackPool::new(1 << 20);
        let mut vm = crate::vm::Vm::new(chunk, &mut stacks, &interner);
        vm.run().expect("vm run");
        let mut s = vm.output().join("\n");
        s.push('\n');
        s
    }

    #[test]
    fn emit_c_parallel_range_uses_thread_private_openmp_scratch() {
        let src = r#"
main() {
    rows = [0; 128]
    par y in 0..128 { rows[y] = y * y }
    print(rows.sum())
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "parallel_range");
        assert!(
            c.contains("#pragma omp parallel for schedule(guided, 4)"),
            "race-free `par` must become an OpenMP loop (guided chunks):\n{c}"
        );
        assert!(
            !c.contains("firstprivate(locals"),
            "no boxed frame privatization"
        );
    }

    #[test]
    fn parallel_range_executes_correctly() {
        let src = r#"
main() {
    rows = [0; 64]
    par y in 0..64 { rows[y] = y * y }
    print(rows.sum())
}
"#;
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "85344");
        }
    }

    #[test]
    fn racy_par_loop_stays_sequential() {
        // Every iteration writes the same cell: not injective, must not get a pragma.
        let src = r#"
main() {
    acc = [0; 2]
    par y in 0..64 { acc[0] = y }
    print(acc[0])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "racy");
        assert!(!c.contains("#pragma omp parallel for"), "{c}");
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "63");
        }
    }

    #[test]
    fn large_sum_reduction_gets_openmp_reduction_clause() {
        // Constant trip ≥ AUTO_PAR_MIN_TRIP — auto-par must not emit OpenMP for
        // dynamic `0..n` (dead `if(>=32768)` still links libgomp).
        let src = r#"
main() {
    acc = 0
    for 0..1000000 { acc += it }
    print(acc)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "red");
        assert!(c.contains("reduction(+:"), "{c}");
        assert!(
            c.contains("#pragma omp parallel for"),
            "constant large reduction must auto-parallelize:\n{c}"
        );
        // 1_000_000 / 32 grain → team capped at OMP_TEAM_MAX (32).
        assert!(
            c.contains("num_threads(32)"),
            "grain-based team size for large constant trip:\n{c}"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "499999500000");
        }
    }

    #[test]
    fn auto_par_dynamic_trip_stays_serial_no_openmp_link() {
        // Dynamic trip auto-par used to emit a dead `#pragma omp ... if(>=32768)`
        // that forced libgomp + dynamic CRT on sequential benches (lakehouse).
        let src = r#"
sum_to(n: i64) : i64 {
    acc = 0
    for 0..n { acc += it }
    return acc
}
main() { print(sum_to(100)) }
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "dyn_red");
        assert!(
            !c.contains("#pragma omp"),
            "dynamic-trip auto-par must stay serial:\n{c}"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "4950");
        }
    }

    #[test]
    fn oversized_unboxed_body_is_outlined_into_restrict_helpers() {
        // Many statements in a loop body force the outliner past a low threshold.
        let src = r#"
+hot(n: i64) : i64 {
    s = 0
    for 0..n {
        s = s + it
        s = s + it * 2
        s = s + it * 3
        s = s + it * 4
        s = s + it * 5
        s = s + it * 6
        s = s + it * 7
        s = s + it * 8
        s = s + it * 9
        s = s + it * 10
        s = s + it * 11
        s = s + it * 12
        s = s + it * 13
        s = s + it * 14
        s = s + it * 15
        s = s + it * 16
    }
    return s
}
main() { print(hot(10)) }
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c_module(
            &chunk,
            &interner,
            "outline_hot",
            EmitCOptions {
                // Loop body is ~16 assigns; threshold must sit under that count.
                outline_stmt_threshold: 12,
                ..Default::default()
            },
        );
        assert!(
            c.contains("_ol_loop") || c.contains("_ol0"),
            "expected outlined helper symbol, C snippet missing _ol*"
        );
        assert!(
            c.contains("restrict fr") || c.contains("*restrict fr"),
            "expected restrict frame pointer on outlined helper"
        );
        assert!(
            c.contains("typedef struct super_u_hot_"),
            "expected outline frame typedef"
        );
        // Still a distinct modular Super→C function (no whole-program dump into main).
        assert!(c.contains("static int64_t super_u_hot_"));
        assert!(c.contains("super_u_module"));
        // Compile the outlined unit (not the default-threshold emit) for correctness.
        if cc_available() {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "super_outline_{}_{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("t.out");
            let opts = CCompileOptions {
                out_bin: Some(&bin),
                ..CCompileOptions::default()
            };
            write_and_maybe_compile_with(&c, &dir.join("t.c"), true, &opts)
                .unwrap_or_else(|e| panic!("{e}\n--- C ---\n{c}"));
            let out = std::process::Command::new(&bin).output().unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            assert!(out.status.success(), "outlined binary failed");
            // sum_k=1..16 of (it*k) for it in 0..10 = 45*136 = 6120
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "6120");
        }
    }

    #[test]
    fn boxed_stack_depth_counts_empty_array_pushes_for_struct_literals() {
        // Regression: Matches.new / 8-field struct with five `[]` values.
        // `MakeArray(0)` pushes; omitting it under-sized `stack[]` → stack smash.
        let mut ops = Vec::new();
        for _ in 0..5 {
            ops.push(Op::LoadConst(0));
            ops.push(Op::MakeArray(0));
        }
        ops.push(Op::LoadConst(0));
        ops.push(Op::LoadConst(0)); // str field
        ops.push(Op::LoadConst(0));
        ops.push(Op::LoadConst(0)); // bool
        ops.push(Op::LoadConst(0));
        ops.push(Op::LoadConst(0)); // bool
        ops.push(Op::MakeStruct(None, 8));
        let depth = boxed_stack_depth(&ops);
        assert!(
            depth >= 16,
            "expected stack depth ≥ 16 for 8 key/value pairs (got {depth})"
        );
    }

    #[test]
    fn multi_field_struct_with_empty_arrays_does_not_stack_smash() {
        // Mirrors stdlib `Matches.new` shape that smashed in examples/02_task_api.
        let src = r#"
+M = {
    names: _ = []
    values: _ = []
    sources: _ = []
    positional: _ = []
    sub_name: str = ""
    sub_m: _ = []
    help: bool = false
    show_ver: bool = false
    new() : M => M.{
        names: [],
        // Heterogeneous literal: keeps `new()` on the boxed path whose depth is checked.
        values: [1, "x"],
        sources: [],
        positional: [],
        sub_name: "",
        sub_m: [],
        help: false,
        show_ver: false
    }
}
main() {
    m = M.new()
    print(m.help)
    print(m.show_ver)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "matches_new_depth");
        // Peak before MakeStruct is 16; depth = peak + 4 → 20. Pre-fix was 15.
        let new_depth = c
            .lines()
            .skip_while(|l| !l.contains("super_fn_M_new_") || !l.contains("{"))
            .take_while(|l| !l.starts_with("static SuperVal super_fn_") || l.contains("M_new"))
            .find_map(|l| {
                l.trim()
                    .strip_prefix("SuperVal stack[")
                    .and_then(|r| r.strip_suffix("];"))
                    .and_then(|n| n.parse::<usize>().ok())
            });
        let depth = new_depth.expect("M.new should emit a boxed SuperVal stack[]");
        assert!(
            depth >= 16,
            "operand stack under-allocated for Matches-like new(): got stack[{depth}]"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "false\nfalse");
        }
    }

    #[test]
    fn immutable_array_params_also_get_restrict() {
        let src = r#"
+sum(a, n: i64) : i64 {
    s = 0
    for 0..n { s = s + a[it] }
    return s
}
main() {
    xs = [1, 2, 3, 4]
    print(sum(xs, 4))
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "imm_restrict");
        assert!(
            c.contains("int64_t *restrict"),
            "immutable array params must be restrict for SROA:\n{c}"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "10");
        }
    }

    #[test]
    fn typed_functions_lower_to_unboxed_c_without_boxed_frames() {
        let src = r#"
+add(a: i64, b: i64)  => a + b
+fib(n: i64) : i64 {
    if n < 2 { return n }
    return fib(n - 1) + fib(n - 2)
}
+half(x: f64)  => x * 0.5
main() {
    print(fib(15))
    print(add(2, 3))
    print(half(3.0))
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "typed");
        assert!(
            !c.contains("SuperVal locals["),
            "no boxed frame expected:\n{c}"
        );
        assert!(!c.contains("SuperVal stack["), "no boxed stack expected");
        assert!(c.contains("static int64_t super_u_fib_"), "{c}");
        assert!(c.contains("int64_t l0_i"), "unboxed int64_t parameter");
        assert!(
            c.contains("__attribute__((always_inline)) double super_u_half_")
                && c.contains("[[unsequenced]]"),
            "`=>` leaves are always_inline [[unsequenced]]"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, "610\n5\n1.5\n");
        }
    }

    #[test]
    fn growable_empty_array_push_loop_stays_unboxed() {
        // ArrEmpty → ArrI64 across a push loop must refine at loop exit so a
        // following `for xs` / index does not fall back to SuperVal frames.
        let src = r#"
main() {
    xs = []
    for 0..10 { xs = host.sys_arr_push(xs, it * 3) }
    acc = 0
    for xs { acc += it }
    print(acc)
    print(xs[0])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "grow");
        assert!(
            !c.contains("boxed fallback:"),
            "growable push/iter must unbox:\n{}",
            c.lines()
                .filter(|l| l.contains("boxed fallback"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert!(!c.contains("SuperVal locals["), "no boxed frame:\n{c}");
        assert!(!c.contains("SuperVal stack["), "no boxed stack");
        assert!(c.contains("sup_grow("), "expected growable storage");
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, "135\n0\n");
        }
    }

    fn boxed_notes(c: &str) -> String {
        c.lines()
            .filter(|l| l.contains("boxed fallback"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// `World2D`-shaped program: a `[]` struct field that widens to an array of structs.
    const STRUCT_ARRAY_WORLD: &str = r#"
Body = {
    x: f64
    vy: f64 = 0.0
    m: f64 = 1.0
}
World = {
    bodies: _ = []
    gy: f64 = 0.0
}
arr_push(a, v) => host.sys_arr_push(a, v)
arr_set(a, i, v) => host.sys_arr_set(a, i, v)
mk(x: f64, m: f64) : Body => Body.{x: x, vy: 0.0, m: m}
world(gy: f64) : World => World.{bodies: [], gy: gy}
add(@w: World, b: Body) : i64 {
    w.bodies = arr_push(w.bodies, b)
    host.sys_arr_len(w.bodies) - 1
}
pair(a: Body, b: Body) : _ {
    na = a
    nb = b
    na.vy = na.vy + 1.0
    nb.vy = nb.vy - 1.0
    return [na, nb]
}
step(@w: World, dt: f64) : i64 {
    n = host.sys_arr_len(w.bodies)
    moved = 0
    for 0..n {
        b = w.bodies[it]
        if b.m > 0.0 {
            b.vy = b.vy + w.gy * dt
            b.x = b.x + b.vy * dt
            w.bodies = arr_set(w.bodies, it, b)
            moved += 1
        }
    }
    if n >= 2 {
        r = pair(w.bodies[0], w.bodies[1])
        w.bodies = arr_set(w.bodies, 0, r[0])
        w.bodies = arr_set(w.bodies, 1, r[1])
    }
    moved
}
mass(w: World) : f64 {
    s = 0.0
    bs = w.bodies
    for bs { s += it.m }
    s
}
main() {
    w = world(-10.0)
    print(mass(w))
    _ = @w.add(mk(1.0, 2.0))
    _ = @w.add(mk(5.0, 0.0))
    _ = @w.add(mk(9.0, 3.0))
    moved = 0
    for 0..4 { moved += @w.step(0.5) }
    print(moved)
    print(host.sys_arr_len(w.bodies))
    b0 = w.bodies[0]
    b1 = w.bodies[1]
    print(b0.vy)
    print(b1.x)
    print(mass(w))
}
"#;

    #[test]
    fn empty_array_struct_field_widens_to_struct_array_and_unboxes() {
        let (chunk, interner, _) = compile_chunk(STRUCT_ARRAY_WORLD, true);
        let c = emit_c(&chunk, &interner, "arrstruct");
        assert!(
            !c.contains("boxed fallback:"),
            "struct-array world must unbox:\n{}",
            boxed_notes(&c)
        );
        assert!(!c.contains("SuperVal locals["), "no boxed frame expected");
        assert!(
            c.contains("sup_grow(") || c.contains("sup_push_copy("),
            "growable storage"
        );
        let world = c.find("/* World */").expect("World typedef");
        let world = &c[world..];
        let world = &world[..world.find('}').unwrap()];
        assert!(
            world.contains(" *bodies;") && world.contains("int64_t bodies__n;"),
            "{world}"
        );
        if let Some((_, out)) = build_and_run(STRUCT_ARRAY_WORLD) {
            // Bodies 0 and 2 fall (m > 0); body 0/1 get the pair impulse each step.
            assert_eq!(out, "0\n8\n3\n-16\n5\n5\n", "{out}");
        }
    }

    #[test]
    fn if_valued_struct_fields_keep_constant_keys_and_unbox() {
        let src = r#"
M = { status: i64 = 0, rid: i64 = -1, pid: i64 = 0, bad: bool = false }
pick(rid: i64, pid: i64, bad: bool) : M {
    if rid < 0 { return M.{status: if pid > 5 { 405 } else { 404 }} }
    M.{status: 0, rid, pid: if bad || pid < 0 { 0 } else { pid }, bad}
}
main() {
    a = pick(-1, 9, false)
    b = pick(-1, 1, false)
    c = pick(3, 7, false)
    d = pick(3, 7, true)
    print(a.status + b.status)
    print(c.rid * 100 + c.pid)
    print(d.pid)
    print(d.bad)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "if_struct_keys");
        assert!(!c.contains("boxed fallback:"), "{}", boxed_notes(&c));
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, "809\n307\n0\ntrue\n");
        }
    }

    #[test]
    fn struct_array_literal_and_iteration_match_vm() {
        let src = r#"
P = { x: i64, y: i64 }
main() {
    ps = [P.{x: 1, y: 2}, P.{x: 3, y: 4}]
    ps[1] = P.{x: 30, y: 40}
    acc = 0
    for ps { acc += it.x * 10 + it.y }
    q = []
    for 0..3 { q = host.sys_arr_push(q, P.{x: it, y: it * it}) }
    for q { acc += it.y }
    print(acc)
    print(host.sys_arr_len(q))
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "arrstruct_lit");
        assert!(!c.contains("boxed fallback:"), "{}", boxed_notes(&c));
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, "357\n3\n");
        }
    }

    #[test]
    fn growable_nested_digest_arrays_use_sup_ai() {
        // `level = []; level = push(level, [i; 2])` is ArrAI — the sha256 merkle shape.
        let src = r#"
main() {
    level = []
    for 0..4 {
        dig = [0; 2]
        dig[0] = it
        dig[1] = it * 10
        level = host.sys_arr_push(level, dig)
    }
    d = level[2]
    print(d[0])
    print(d[1])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "arrai");
        assert!(!c.contains("boxed fallback:"), "ArrAI push must unbox");
        assert!(c.contains("sup_ai_t"), "nested arrays lower to sup_ai_t");
        assert!(c.contains("sup_grow("), "expected growable ArrAI storage");
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, "2\n20\n");
        }
    }

    #[test]
    fn unit_variant_patterns_on_integer_tags_stay_unboxed() {
        let src = r#"
Kind = {
    Var
    Con
    Arrow
}
classify(kx: i64, ky: i64) : i64 {
    when (kx, ky) {
        (Kind.Var, Kind.Var) => { return 1 }
        (Kind.Var, _) => { return 1 }
        (_, Kind.Var) => { return 2 }
        (Kind.Con, Kind.Con) => { return 3 }
        (Kind.Arrow, Kind.Arrow) => { return 4 }
        else => { return 0 }
    }
}
main() {
    s = 0
    for a in 0..3 {
        for b in 0..3 { s = s * 5 + classify(a, b) }
    }
    print(s)
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "492554\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(
            !c.contains("boxed fallback:"),
            "integer tag dispatch must unbox:\n{c}"
        );
        assert_eq!(c_out, vm);
    }

    #[test]
    fn for_loops_walk_nested_arrays_row_by_row() {
        let src = r#"
count(rows) : i64 {
    n = 0
    for rows { n += 1 }
    n
}
main() {
    ints = []
    floats = []
    for 0..4 {
        ints = host.sys_arr_push(ints, [it, it * 10, 7])
        floats = host.sys_arr_push(floats, [it * 0.5])
    }
    s, w = 0, 0
    for ints { s += it[1] + host.sys_arr_len(it) }
    for r in floats { w += r[0] * 4.0 }
    print("{count(ints)} {count(floats)} {s} {w}")
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "4 4 72 12\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(
            !c.contains("boxed fallback:"),
            "nested array loops must unbox:\n{c}"
        );
        assert_eq!(c_out, vm);
    }

    #[test]
    fn array_slices_copy_a_clamped_range_and_stay_unboxed() {
        let src = r#"
main() {
    xs = [10, 20, 30, 40, 50]
    fs = [0.5, 1.5, 2.5]
    ss = ["hdr", "a", "b", "c"]
    mid = xs[1..4]
    mid[0] = 99
    tail = xs[3..100]
    empty = xs[4..2]
    neg = xs[-1..2]
    t = 0
    for s in ss[1..host.sys_arr_len(ss)] { t += s.len }
    w = 0.0
    for fs[1..3] { w += it }
    print("{xs[1]} {mid[0]} {mid[2]} {host.sys_arr_len(mid)} {host.sys_arr_len(tail)} {tail[1]}")
    print("{host.sys_arr_len(empty)} {host.sys_arr_len(neg)} {t} {w}")
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "20 99 40 3 2 50\n0 0 3 4\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(
            !c.contains("boxed fallback:"),
            "array slices must unbox:\n{c}"
        );
        assert_eq!(c_out, vm);
    }

    #[test]
    fn array_literals_of_arrays_unbox_as_nested_rows() {
        let src = r#"
concat(parts) {
    out = []
    for parts {
        row = it
        for 0..host.sys_arr_len(row) { out = host.sys_arr_push(out, row[it]) }
    }
    out
}
main() {
    a, b = [1, 2, 3], [40, 50]
    ab = concat([a, b, [6]])
    fs = [[0.5, 1.0], [2.0]]
    w = 0.0
    for fs { for it { w += it } }
    print("{host.sys_arr_len(ab)} {ab[0]} {ab[3]} {ab[5]} {w} {fs[1][0]}")
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "6 1 40 6 3.5 2\n");
        let Some((c, c_out)) = build_and_run(src) else {
            return;
        };
        assert!(
            !c.contains("boxed fallback:"),
            "array-of-array literals must unbox:\n{c}"
        );
        assert_eq!(c_out, vm);
    }

    #[test]
    fn growable_f64_row_arrays_unbox_through_calls() {
        let src = r#"
Hit = {
    index: i64 = 0
    score: f64 = 0.0
}
dot(a, b) : f64 {
    s = 0.0
    for 0..host.sys_arr_len(a) { s += a[it] * b[it] }
    s
}
best(query, rows) {
    hits = []
    for 0..host.sys_arr_len(rows) { hits = host.sys_arr_push(hits, Hit.{index: it, score: dot(query, rows[it])}) }
    hits
}
main() {
    rows = []
    for 0..3 {
        r = [it * 1.5, 2.0 - it]
        rows = host.sys_arr_push(rows, r)
    }
    gone = rows[7]
    print("{host.sys_arr_len(rows)} {host.sys_arr_len(gone)} {rows[2][0]}")
    for best([2.0, 1.0], rows) { print("{it.index} {it.score}") }
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "3 0 3\n0 2\n1 4\n2 6\n");
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "arraf");
        assert!(
            !c.contains("boxed fallback:"),
            "f64 row arrays must unbox:\n{c}"
        );
        assert!(c.contains("sup_af_t"), "f64 rows lower to sup_af_t");
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out, vm);
        }
    }

    #[test]
    fn at_array_param_is_restrict_pointer_with_direct_indexing() {
        let src = r#"
fill(@a, n: i64) {
    for 0..n { a[it] = it * 3 }
}
main() {
    xs = [0; 16]
    fill(@xs, 16)
    print(xs[5])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "at");
        assert!(c.contains("int64_t *restrict l0_ai"), "{c}");
        let body = &c[c.find("static void super_u_fill_").unwrap()..];
        let body = &body[..body.find("\n}\n").unwrap()];
        assert!(
            !body.contains("super_host"),
            "no host array shim in loop body"
        );
        assert!(
            !body.contains("super_index_"),
            "no boxed indexing in loop body"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "15");
        }
    }

    #[test]
    fn non_escaping_fixed_array_becomes_c_stack_array() {
        let src = r#"
main() {
    t = [0.0; 32]
    for 0..32 { t[it] = it * 0.5 }
    print(t[31])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "stk");
        assert!(
            c.contains("alignas(64) double a") && c.contains("_buf[32]"),
            "{c}"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "15.5");
        }
    }

    #[test]
    fn returned_array_is_heap_allocated_not_stack() {
        let src = r#"
make(n: i64) {
    a = [0; 8]
    for 0..n { a[it] = it * it }
    return a
}
main() {
    r = make(6)
    print(r[5])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "ret");
        let body = c
            .split("\n}\n")
            .find(|f| {
                f.lines()
                    .any(|l| l.contains("super_u_make_") && l.ends_with(") {"))
            })
            .expect("make is emitted unboxed");
        assert!(
            !body.contains("_buf["),
            "escaping arrays never live in a C stack buffer"
        );
        assert!(
            body.contains("sup_heap_zeroed") || body.contains("SUP_KEEP1(sup_mark, "),
            "escaping arrays live on the heap or are kept above the caller's arena mark"
        );
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "25");
        }
    }

    #[test]
    fn at_scalar_param_is_restrict_out_pointer() {
        let src = r#"
bump(@n: i64, by: i64) { n += by }
main() {
    k = 5
    bump(@k, 3)
    bump(@k, 4)
    print(k)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "sc");
        assert!(c.contains("int64_t *restrict m0"), "{c}");
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(out.trim(), "12");
        }
    }

    /// Value structs lower to typed C structs (`sup_s{id}_t`) on the unboxed path.
    /// Regression: `MakeStruct`/`GetField`/`SetField` used to be silently dropped on the boxed
    /// path, collapsing the roguelike benchmark (VM 37511 vs C23 35), and later forced every
    /// struct-touching function onto the boxed `SuperVal` fallback.
    const STRUCT_PROGRAM: &str = r#"
+Room = {
    x: i64 = 0
    y: i64 = 0
    w: i64 = 0
    h: i64 = 0
}
area(r: Room) : i64 => r.w * r.h
touch(a: Room, b: Room) : bool {
    a.x <= b.x + b.w + 1 && b.x <= a.x + a.w + 1 && a.y <= b.y + b.h + 1 && b.y <= a.y + a.h + 1
}
grow(@grid, r: Room) {
    for yy in r.y..(r.y + r.h) {
        for xx in r.x..(r.x + r.w) { grid[yy * 8 + xx] = 1 }
    }
}
main() {
    grid = [0; 64]
    xs = [1, 5, 0, 0]
    a = Room{x: 1, y: 1, w: 3, h: 2}
    b = Room{x: xs[1], y: 1, w: 2, h: 2}
    print(area(a))
    print(touch(a, b))
    c = a
    c.w = 9
    print(a.w)
    print(c.w)
    c.h += 1
    print(c.h)
    grow(@grid, a)
    s = 0
    for i in 0..64 { s += grid[i] }
    print(s)
    far = Room{x: 40, y: 40, w: 1, h: 1}
    print(touch(a, far))
}
"#;

    /// Every listed function has an unboxed instance and nothing fell back to `SuperVal`.
    fn assert_fully_unboxed(c: &str, fns: &[&str]) {
        let notes: Vec<&str> = c.lines().filter(|l| l.contains("boxed fallback")).collect();
        assert!(
            notes.is_empty(),
            "unexpected boxed fallback:\n{}",
            notes.join("\n")
        );
        assert!(!c.contains("SuperVal locals["), "boxed frame emitted");
        assert!(
            !c.contains("(SuperVal *restrict args, int nargs);"),
            "a function stayed on the boxed path"
        );
        for f in fns {
            assert!(
                c.contains(&format!("super_u_{f}_")),
                "no unboxed `{f}` instance"
            );
        }
    }

    #[test]
    fn struct_program_emits_no_boxed_fallback() {
        let (chunk, interner, _) = compile_chunk(STRUCT_PROGRAM, true);
        let c = emit_c(&chunk, &interner, "rooms");
        assert_fully_unboxed(&c, &["area", "touch", "grow", "main"]);
        assert!(c.contains("typedef struct sup_s0_t { /* Room */"), "{c}");
        // `c.w = 9` / `c.h += 1` update the local in place; `c = a` is a C struct copy.
        assert!(c.contains(".w = 9LL;"), "{c}");
        assert!(c.contains(".h = ("), "{c}");
    }

    const PHYSICS_PROGRAM: &str = r#"
+Vec2 = {
    x: f64 = 0.0
    y: f64 = 0.0
    len2(self) : f64 => self.x * self.x + self.y * self.y
}
+Body = {
    pos: Vec2
    vel: Vec2
    inv_mass: f64 = 1.0
    hits: i64 = 0
    integrate(@self, dt: f64) {
        self.pos = Vec2.{x: self.pos.x + self.vel.x * dt, y: self.pos.y + self.vel.y * dt}
    }
}
+Grid = {
    w: i64 = 0
    cells: _
}
step(@b: Body, dt: f64, g: f64) {
    b.vel = Vec2.{x: b.vel.x, y: b.vel.y - g * dt}
    @b.integrate(dt)
    if b.pos.y < 0.0 {
        b.hits += 1
        b.vel = Vec2.{x: b.vel.x, y: 0.0 - b.vel.y * 0.5}
    }
}
energy(b: Body) : f64 => 0.5 * b.vel.len2() / b.inv_mass
launch(x: f64, y: f64) : Body => Body.{pos: Vec2.{x: x, y: y}, vel: Vec2.{x: 1.0, y: 0.0}, inv_mass: 0.5}
total(g: Grid) : i64 {
    s = 0
    for g.cells { s += it }
    s * g.w
}
main() {
    b = launch(0.0, 10.0)
    for 0..200 { step(@b, 0.05, 9.81) }
    print(b.hits)
    print(b.pos.x > 9.999 && b.pos.x < 10.001)
    e = energy(b)
    print(e > 1.0267 && e < 1.0268)
    c = b
    c.inv_mass = 2.0
    c.pos.x = 99.0
    print(b.inv_mass == 0.5)
    print(c.inv_mass == 2.0)
    print(c.pos.x < 11.0)
    g = Grid{w: 2, cells: [1, 2, 3]}
    print(total(g))
}
"#;

    #[test]
    fn physics_body_structs_stay_unboxed() {
        let (chunk, interner, _) = compile_chunk(PHYSICS_PROGRAM, true);
        let c = emit_c(&chunk, &interner, "physics");
        assert_fully_unboxed(
            &c,
            &[
                "step",
                "Body_integrate",
                "energy",
                "Vec2_len2",
                "launch",
                "total",
                "main",
            ],
        );
        // `@` struct params are restrict out-pointers; nested fields are `.a.b` paths.
        assert!(c.contains("*restrict m0"), "{c}");
        assert!(c.contains(".vel.y"), "{c}");
        // Array fields carry their length member.
        assert!(c.contains("int64_t cells__n;"), "{c}");
    }

    #[test]
    fn physics_body_structs_match_vm_in_c23() {
        let vm = vm_output(PHYSICS_PROGRAM);
        // `c` is a value copy; mutating its nested field does not change `b`.
        assert_eq!(vm, "172\ntrue\ntrue\ntrue\ntrue\nfalse\n12\n");
        let Some((_, out)) = build_and_run(PHYSICS_PROGRAM) else {
            return;
        };
        assert_eq!(out, vm);
    }

    #[test]
    fn struct_programs_match_vm_in_c23() {
        let Some((c, out)) = build_and_run(STRUCT_PROGRAM) else {
            return;
        };
        assert!(
            !c.contains("— skip"),
            "emitter must never skip ops silently"
        );
        // Expected lines were verified against `super run` (the full-host VM).
        assert_eq!(out.trim(), "6\ntrue\n3\n9\n3\n6\nfalse", "C23 output\n{c}");
        let spawned = std::process::Command::new("cc")
            .args([
                "-fsyntax-only",
                "-std=c23",
                "-Wall",
                "-Werror=unused-label",
                "-Werror=unused-variable",
                "-fopenmp",
            ])
            .arg("-x")
            .arg("c")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        if let Ok(mut child) = spawned {
            use std::io::Write;
            {
                let mut stdin = child.stdin.take().unwrap();
                stdin.write_all(c.as_bytes()).unwrap();
            }
            let done = child.wait_with_output().unwrap();
            assert!(
                done.status.success(),
                "boxed C still has unused labels or locals:\n{}",
                String::from_utf8_lossy(&done.stderr)
            );
        }
    }

    #[test]
    fn fail_or_err_and_process_args_match_the_vm() {
        let src = r#"
read(path: str) : str! {
    ensure path != "" else { fail "empty" }
    "ok"
}
main() {
    print(read("cfg") or err { "bad {err}" })
    print(read("") or err { "bad {err}" })
    print(read("") or "fallback")
    print(read("cfg") or "fallback")
}
"#;
        let vm = vm_output(src);
        assert_eq!(vm, "ok\nbad empty\nfallback\nok\n");
        let Some((c, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out, vm, "{c}");
        assert!(
            !c.contains("stack[sp++] = SUPER_NULL; /* fail */"),
            "fail must stay an error value"
        );
        assert!(c.contains("SUPER_TAG_ERR"), "{c}");
        assert!(c.contains("sys_args"), "process argv must be a real host");
    }

    /// Boxed string indexing/slicing must follow the VM: scalar-based, `""` past the end.
    #[test]
    fn boxed_string_index_and_slice_match_vm_in_c23() {
        let src = r#"
+Tag = {
    name: str = ""
}
probe(t: Tag, n: i64) : str {
    s = t.name
    s[1..n] + "|" + s[0] + "|" + s[50] + "|" + s[3..1] + "|"
}
main() {
    print(probe(Tag{name: "hello"}, 4))
    xs = [10, 20, 30, 40]
    ys = xs[1..3]
    print(ys[0] + ys[1])
}
"#;
        let Some((c, out)) = build_and_run(src) else {
            return;
        };
        assert!(!c.contains("— skip"));
        assert_eq!(out.trim(), "ell|h|||\n50", "C23 output\n{c}");
    }

    #[test]
    fn unboxed_output_matches_vm_for_mixed_programs() {
        let src = r#"
+LIMIT = 10
+SCALE = 2.5
collatz(n: i64) : i64 {
    steps = 0
    x = n
    for 0..1000 {
        if x == 1 { break }
        if x % 2 == 0 { x = x / 2 } else { x = 3 * x + 1 }
        steps += 1
    }
    return steps
}
greet(name: str, n: i64) : str => "hi {name} #{n}"
sq(x: f64)  => x * x
main() {
    print(collatz(27))
    print(greet("bob", LIMIT))
    print(sq(SCALE))
    t = 0
    for i in 0..4 {
        for j in 0..3 { t += i * j }
    }
    print(t)
    m = 0
    for 0..50 { if it % 7 == 0 { m += 1 } }
    print(m)
    print(10 / 3)
    print(10.0 / 4.0)
    print(7 % 4)
    print(1 < 2 && 2 < 3)
}
"#;
        if let Some((c, out)) = build_and_run(src) {
            assert!(!c.contains("SuperVal locals["), "{c}");
            assert_eq!(out, vm_output(src));
        }
    }

    #[test]
    fn lazy_stack_values_survive_nested_call_results_and_joins() {
        // Three call results summed (stack temporaries must never be clobbered), an int→float
        // join at an `if` merge, string equality/concat, `continue`, unary and bit ops.
        let src = r#"
id3(x: i64)  => x + 1
pick(flag: bool) {
    v = 1
    if flag { v = 2.5 }
    return v
}
cmpstr(a: str, b: str) : bool => a == b
tri(n: i64) : i64 {
    t = 0
    i = 0
    for 0..n {
        t += id3(i) + id3(i + 1) + id3(i + 2)
        i += 1
    }
    return t
}
neg_loop() : f64 {
    x = 0
    for 0..4 { x = x + 0.5 }
    return x
}
main() {
    print(tri(5))
    print(pick(true))
    print(pick(false))
    print(cmpstr("a", "a"))
    print(cmpstr("a", "b"))
    print(neg_loop())
    s = ""
    for 0..3 { s = s + "x" }
    print(s)
    total = 0
    for i in 0..5 {
        if i == 3 { continue }
        total += i
    }
    print(total)
    print(-(3 - 5))
    print(1 << 10)
    print(0xFF & 15)
}
"#;
        if let Some((c, out)) = build_and_run(src) {
            assert!(!c.contains("SuperVal locals["), "{c}");
            assert_eq!(out, vm_output(src));
        }
    }

    #[test]
    fn heterogeneous_code_keeps_boxed_superval_path() {
        // Nested growable arrays are the "truly dynamic" case: boxed fallback must stay correct.
        let src = r#"
main() {
    rows = []
    for 0..3 {
        row = []
        for 0..2 { row.push(it) }
        rows.push(row)
    }
    print(rows.len())
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "dyn");
        assert!(
            c.contains("boxed fallback"),
            "fallback reason is documented:\n{c}"
        );
        assert!(
            c.contains("SuperVal locals["),
            "dynamic code keeps the boxed frame"
        );
    }

    #[test]
    fn emitter_has_no_benchmark_fingerprint_templates() {
        let src = include_str!("c_emit.rs");
        let unboxed = include_str!("c_emit/unboxed.rs");
        for needle in [
            "fn try_emit_",
            "mandelbrot row par",
            "softening",
            "saxpy",
            "super_sha256_",
            "\"sha2_256\"",
        ] {
            let in_main = src
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .contains(needle);
            assert!(
                !in_main,
                "fingerprint `{needle}` must not return to c_emit.rs"
            );
            assert!(
                !unboxed
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or("")
                    .contains(needle),
                "fingerprint `{needle}` must not appear in unboxed.rs"
            );
        }
    }

    #[test]
    fn emit_c_emits_struct_layout_and_int_lut() {
        let src = r#"
+Pos = { x: f64, y: f64 }
+TABLE = [1, 2, 3]
"#;
        let file = SourceFile::new("t.sr", src);
        let result = analyze(&file);
        assert!(
            !result.diagnostics.has_errors(),
            "{:?}",
            result.diagnostics.iter().collect::<Vec<_>>()
        );
        let chunk = crate::bytecode::Chunk {
            ops: vec![Op::Halt],
            consts: vec![Value::Array(vec![Value::Int(1), Value::Int(2)].into())],
            functions: vec![],
            tests: vec![],
            opt_notes: vec![],
            mut_writebacks: vec![],
            escape_birth: vec![],
        };
        let c = emit_c_module(
            &chunk,
            &result.interner,
            "t",
            EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: false,
                ..Default::default()
            },
        );
        assert!(c.contains("typedef struct SuperPos"));
        assert!(c.contains("double x"));
        assert!(c.contains("SUPER_LUT_0[]"));
    }

    #[test]
    fn emit_c_omit_main_for_shared() {
        let (chunk, interner, module) = compile_chunk("main() { return 0 }", false);
        let c = emit_c_module(
            &chunk,
            &interner,
            "t",
            EmitCOptions {
                typed: None,
                module: Some(&module),
                omit_main: true,
                release: false,
                ..Default::default()
            },
        );
        assert!(!c.contains("int main(void)"));
        assert!(c.contains("super_module_main"));
    }

    #[test]
    fn write_and_maybe_compile_c23_flags() {
        assert_eq!(CCompileOptions::default().std, "c23");
        let src = r#"
main() { print(42) }
"#;
        let file = SourceFile::new("t.sr", src);
        let mut result = analyze(&file);
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, false)
            .with_opt_meta(meta)
            .compile(&result.module);
        let source = emit_c(&chunk, &result.interner, "t");
        let dir = std::env::temp_dir().join(format!("super_c_emit_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let c_path = dir.join("t.c");
        let bin_path = dir.join("t.out");
        let opts = CCompileOptions {
            out_bin: Some(&bin_path),
            opt: CodegenOpt::O3,
            std: "c23",
            ..CCompileOptions::default()
        };
        let out = write_and_maybe_compile_with(&source, &c_path, true, &opts)
            .expect("cc should compile trivial main");
        assert_eq!(out.as_deref(), Some(bin_path.as_path()));
        assert!(bin_path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_shared_name_uses_so_on_linux() {
        let p = PathBuf::from("/tmp/foo.c");
        let sh = default_shared_name(&p);
        assert_eq!(sh.extension().and_then(|e| e.to_str()), Some("so"));
    }

    #[test]
    fn profiles_match_cargo_and_gcc_defaults() {
        let dev = resolve_profile(ProfileOverrides {
            debug: true,
            ..ProfileOverrides::default()
        })
        .unwrap();
        assert_eq!(dev.profile, BuildProfile::Dev);
        assert_eq!(dev.opt, CodegenOpt::O0);
        assert!(!dev.strip_tests);
        assert!(dev.debug_info);
        assert!(dev.summary().contains("-O0"));
        assert!(dev.summary().contains("-g"));

        let release = resolve_profile(ProfileOverrides {
            release: true,
            ..ProfileOverrides::default()
        })
        .unwrap();
        assert_eq!(release.opt, CodegenOpt::O3);
        assert_eq!(release.lto, LtoMode::Fat);
        assert!(release.strip && release.strip_tests && release.fast_math && release.max_speed);
        assert!(
            !release.static_link,
            "release defaults to dynamic CRT on glibc (use --static / musl for static)"
        );
        assert_eq!(release.target_cpu.as_deref(), Some("native"));
        assert!(release.summary().contains("-funroll-loops"));
        assert!(release.summary().contains("-flto"));
        assert!(
            !release.summary().contains("-static"),
            "release summary must not imply -static by default"
        );

        let test_profile = resolve_profile(ProfileOverrides {
            profile: Some("test"),
            ..ProfileOverrides::default()
        })
        .unwrap();
        assert_eq!(test_profile.profile, BuildProfile::Test);
        assert_eq!(test_profile.opt, CodegenOpt::O2);
        assert!(test_profile.debug_info);
        assert!(!test_profile.strip_tests);
        assert!(!test_profile.fast_math);
        assert!(!test_profile.max_speed);
        assert_eq!(test_profile.lto, LtoMode::Off);
        assert_eq!(test_profile.target_cpu.as_deref(), Some("native"));

        let fast = resolve_profile(ProfileOverrides {
            profile: Some("fast"),
            ..ProfileOverrides::default()
        })
        .unwrap();
        assert_eq!(fast.opt, release.opt);
        assert_eq!(fast.lto, release.lto);
        assert_eq!(fast.max_speed, release.max_speed);

        let size = resolve_profile(ProfileOverrides {
            profile: Some("size"),
            opt: Some("z"),
            ..ProfileOverrides::default()
        })
        .unwrap();
        assert_eq!(size.opt, CodegenOpt::Oz);
        assert!(size.gc_sections);
        assert_eq!(size.lto, LtoMode::Fat);

        let err = resolve_profile(ProfileOverrides {
            release: true,
            debug: true,
            ..ProfileOverrides::default()
        });
        assert!(err.is_err());
    }

    #[test]
    fn windows_gnu_cc_args_link_winsock_and_winpthread() {
        let win = crate::target::CompileTarget::parse("x86_64-pc-windows-gnu").unwrap();
        let profile = resolve_profile(ProfileOverrides {
            release: true,
            ..ProfileOverrides::default()
        })
        .unwrap();
        let args = profile
            .compile_options_for(Some("clang"), None, "c23", false, Some(&win))
            .cc_args(Path::new("a.c"), Path::new("a.exe"));
        for need in ["-lws2_32", "-lwinpthread", "-luser32", "-lgdi32", "-lcomctl32"] {
            assert!(
                args.iter().any(|arg| arg == need),
                "missing {need} in {args:?}"
            );
        }
    }

    #[test]
    fn size_profile_cc_args_request_gc_and_lto() {
        let profile = resolve_profile(ProfileOverrides {
            profile: Some("size"),
            ..ProfileOverrides::default()
        })
        .unwrap();
        let opts = profile.compile_options(None, None, "c23", false);
        let args = opts.cc_args(Path::new("a.c"), Path::new("a"));
        assert!(args.iter().any(|arg| arg == "-Os"));
        assert!(
            args.iter().any(|arg| arg == "-flto" || arg == "-flto=auto"),
            "{args:?}"
        );
        let peak = resolve_profile(ProfileOverrides {
            release: true,
            ..ProfileOverrides::default()
        })
        .unwrap();
        let gcc = peak
            .compile_options(None, None, "c23", false)
            .cc_args(Path::new("a.c"), Path::new("a"));
        assert!(
            gcc.iter().any(|arg| arg == "-flto=auto"),
            "gcc fat LTO should be parallel: {gcc:?}"
        );
        assert!(gcc.iter().any(|arg| arg == "-funroll-loops"));
        assert!(gcc.iter().any(|arg| arg == "-fno-plt"));
        assert!(gcc.iter().any(|arg| arg == "-fno-semantic-interposition"));
        assert!(gcc.iter().any(|arg| arg == "-fomit-frame-pointer"));
        assert!(
            !gcc.iter().any(|arg| arg == "-static"),
            "release Linux GNU AOT must not default to -static: {gcc:?}"
        );
        let clang = peak
            .compile_options(Some("clang"), None, "c23", false)
            .cc_args(Path::new("a.c"), Path::new("a"));
        assert!(clang.iter().any(|arg| arg == "-flto=full"));
        let zig = peak
            .compile_options(Some("zig"), None, "c23", false)
            .cc_args(Path::new("a.c"), Path::new("a"));
        assert!(
            zig.iter().any(|arg| arg == "-flto=full"),
            "zig cc fat LTO must use clang spelling: {zig:?}"
        );
        assert!(!zig.iter().any(|arg| arg == "-fno-semantic-interposition"));
        assert!(args.iter().any(|arg| arg == "-ffunction-sections"));
        assert!(args.iter().any(|arg| arg == "-Wl,--gc-sections"));
        assert!(args.iter().any(|arg| arg == "-s"));
        assert!(
            !args.iter().any(|arg| arg == "-static"),
            "size profile must not default to -static: {args:?}"
        );
        assert!(!args.iter().any(|arg| arg.starts_with("-march=")));
        assert!(!args.iter().any(|arg| arg == "-ffast-math"));
        let static_rel = resolve_profile(ProfileOverrides {
            release: true,
            static_link: Some(true),
            ..ProfileOverrides::default()
        })
        .unwrap();
        let mut static_opts = static_rel.compile_options(None, None, "c23", false);
        static_opts.needs_openmp = false;
        let static_args = static_opts.cc_args(Path::new("a.c"), Path::new("a"));
        assert!(
            static_args.iter().any(|arg| arg == "-static"),
            "--static must request -static on sequential Linux AOT: {static_args:?}"
        );
        let dyn_rel = resolve_profile(ProfileOverrides {
            release: true,
            static_link: Some(false),
            ..ProfileOverrides::default()
        })
        .unwrap();
        let dyn_args = dyn_rel
            .compile_options(None, None, "c23", false)
            .cc_args(Path::new("a.c"), Path::new("a"));
        assert!(
            !dyn_args.iter().any(|arg| arg == "-static"),
            "--no-static must suppress -static: {dyn_args:?}"
        );
    }

    #[test]
    fn await_spawn_same_line_emits_contiguous_parent_and_runs() {
        let src = r#"
fetch(id: i64) : i64 => id + 1
main() {
    scope {
        u = await spawn fetch(3)
        print(u)
    }
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let main = chunk
            .functions
            .iter()
            .position(|f| interner.resolve(f.name) == "main")
            .expect("main");
        let spawn = chunk
            .functions
            .iter()
            .position(|f| interner.resolve(f.name).starts_with("__spawn_"))
            .expect("spawn thunk");
        let main_entry = chunk.functions[main].entry as usize;
        let spawn_entry = chunk.functions[spawn].entry as usize;
        assert!(
            spawn_entry > main_entry,
            "spawn thunk must be out-of-line after parent entry"
        );
        // Parent body must not be cut at the thunk entry (old bug: goto L_after with no label).
        let c = emit_c(&chunk, &interner, "await_spawn");
        assert!(
            !c.contains("goto L_") || c.matches("L_").count() > 1,
            "expected labeled boxed body or unboxed lowering:\n{c}"
        );
        // Syntax-check / run through C when a compiler is present.
        let Some((c_src, out)) = build_and_run(src) else {
            return;
        };
        assert!(
            !c_src.contains("super_unsupported(\"Spawn\")")
                && !c_src.contains("super_unsupported(\"Await\")"),
            "Spawn/Await must lower sequentially:\n{c_src}"
        );
        assert_eq!(out.trim(), "4");
        assert_eq!(vm_output(src).trim(), "4");
    }

    #[test]
    fn spawn_then_await_handle_matches_await_spawn() {
        let sep = r#"
fetch(id: i64) : i64 => id * 2
main() {
    scope {
        t = spawn fetch(7)
        u = await t
        print(u)
    }
}
"#;
        let same = r#"
fetch(id: i64) : i64 => id * 2
main() {
    scope {
        u = await spawn fetch(7)
        print(u)
    }
}
"#;
        assert_eq!(vm_output(sep).trim(), "14");
        assert_eq!(vm_output(same).trim(), "14");
        if let (Some((_, a)), Some((_, b))) = (build_and_run(sep), build_and_run(same)) {
            assert_eq!(a.trim(), "14");
            assert_eq!(b.trim(), "14");
        }
    }

    /// Function names the unboxer left on the boxed path.
    fn boxed_fns(c: &str) -> Vec<String> {
        c.lines()
            .filter_map(|l| l.split("boxed fallback: ").nth(1))
            .filter_map(|r| r.split(" — ").next())
            .map(str::to_string)
            .collect()
    }

    /// Emit, compile and run `src`; the AOT output must equal the VM output.
    fn assert_aot_matches_vm(src: &str) -> String {
        let vm = vm_output(src);
        if let Some((_, out)) = build_and_run(src) {
            assert_eq!(
                out.trim_end(),
                vm.trim_end(),
                "AOT output diverged from the VM"
            );
        }
        vm
    }

    const OPTIONALS_AND_ERRORS: &str = r#"
find(xs: _, k: i64) : i64? {
    for xs { if it == k { return index } }
    none
}
parse_port(s: str) : i64! {
    ensure s != "" else { fail "empty" }
    8080
}
deep(s: str) : i64! {
    p = parse_port(s) or err { fail "deep: {err}" }
    p + 1
}
main() {
    xs = [5, 7, 9]
    print(find(xs, 7) or -1)
    print(find(xs, 4) or -1)
    print(parse_port("x") or 3000)
    print(parse_port("") or 3000)
    m = parse_port("") or err {
        print("Failed: {err}")
        0
    }
    print(m)
    d = deep("") or err {
        print(err)
        -2
    }
    print(d)
    print(deep("y") or 0)
    total = 0
    for 0..10 { total += find(xs, it) or 100 }
    print(total)
}
"#;

    #[test]
    fn optional_and_fallible_calls_unbox_is_absent_or_and_fail() {
        let (chunk, interner, _) = compile_chunk(OPTIONALS_AND_ERRORS, true);
        let c = emit_c(&chunk, &interner, "opt_err");
        assert!(
            boxed_fns(&c).is_empty(),
            "everything must unbox:\n{}",
            boxed_notes(&c)
        );
        assert!(
            c.contains("sup_err_msg = sup_none_msg"),
            "`none` returns set the status"
        );
        assert!(
            c.contains("sup_err_msg = nullptr"),
            "callers reset the status after a call"
        );
        let vm = assert_aot_matches_vm(OPTIONALS_AND_ERRORS);
        assert_eq!(
            vm,
            "1\n-1\n8080\n3000\nFailed: empty\n0\ndeep: empty\n-2\n8081\n703\n"
        );
    }

    #[test]
    fn is_absent_on_known_scalar_is_a_constant_probe() {
        // `or` on a value that can never be absent folds away: no status variable, no boxing.
        let src = r#"
main() {
    x = 4
    y = x or 9
    s = "hi" or "no"
    print(y + 1)
    print(s)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "probe");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(!c.contains("sup_err_msg = sup_none_msg"));
        assert_eq!(assert_aot_matches_vm(src), "5\nhi\n");
    }

    const FRAME_ACROSS_BOXED_CALLS: &str = r#"
Col = {
    name: str
    vals: _ = []
    nulls: _ = []
    push(@self, v: i64) {
        self.vals = host.sys_arr_push(self.vals, v)
        self.nulls = host.sys_arr_push(self.nulls, v < 0)
    }
    acc(self) : i64 {
        s = 0
        for self.vals { s += it }
        s
    }
}
Tab = {
    names: _ = []
    cols: _ = []
    flags: _ = []
    n: i64 = 0
    add(@self, c: Col) {
        self.names = host.sys_arr_push(self.names, c.name)
        self.cols = host.sys_arr_push(self.cols, c)
        self.flags = host.sys_arr_push(self.flags, false)
        self.n += 1
    }
    tag(@self, label: str) {
        junk = [1, "x"]
        self.names = host.sys_arr_push(self.names, label)
        self.flags = host.sys_arr_push(self.flags, true)
        self.n += host.sys_arr_len(junk)
    }
    total(self) : i64 {
        s = 0
        cs = self.cols
        for cs { s += it.acc() }
        s
    }
}
boxed_col(name: str) : Col {
    junk = [2, "y"]
    c = Col{name}
    @c.push(host.sys_arr_len(junk))
    @c.push(-1)
    c
}
main() {
    t = Tab{}
    a = Col{name: "a"}
    for 0..4 { @a.push(it * 10) }
    @t.add(a)
    @t.add(boxed_col("b"))
    @t.tag("extra")
    print(t.n)
    print(t.names)
    print(t.flags)
    print(t.total())
    c1 = t.cols[1]
    print(c1.nulls)
    print(c1.vals)
}
"#;

    #[test]
    fn frame_like_struct_with_array_fields_crosses_boxed_calls() {
        let (chunk, interner, _) = compile_chunk(FRAME_ACROSS_BOXED_CALLS, true);
        let c = emit_c(&chunk, &interner, "frame_bridge");
        let mut boxed = boxed_fns(&c);
        boxed.sort();
        // Only the two helpers with a heterogeneous literal stay boxed; `main` bridges to them.
        assert_eq!(boxed, ["Tab_tag", "boxed_col"], "{}", boxed_notes(&c));
        assert!(
            c.contains("sup_box_ab("),
            "bool arrays box for boxed callees"
        );
        assert!(
            c.contains("sup_unbox_astr("),
            "string arrays unbox on the way back"
        );
        assert!(
            c.contains("sup_unbox_as"),
            "struct arrays unbox element-wise"
        );
        let vm = assert_aot_matches_vm(FRAME_ACROSS_BOXED_CALLS);
        assert_eq!(
            vm,
            "4\n[\"a\", \"b\", \"extra\"]\n[false, false, true]\n61\n[false, true]\n[2, -1]\n"
        );
    }

    #[test]
    fn empty_array_field_filled_only_by_boxed_code_is_not_dropped() {
        // The unboxed side never sees a push, so `items` stays `[]` there; the boxed `fill`
        // must not have its elements discarded on the way back.
        let src = r#"
Bag = {
    items: _ = []
    n: i64 = 0
    fill(@self) {
        junk = [1, "x"]
        self.items = host.sys_arr_push(self.items, 42)
        self.n = host.sys_arr_len(junk)
    }
}
main() {
    b = Bag{}
    @b.fill()
    print(b.n)
    print(host.sys_arr_len(b.items))
    print(b.items[0])
}
"#;
        assert_eq!(assert_aot_matches_vm(src), "2\n1\n42\n");
    }

    #[test]
    fn bool_arrays_unbox_as_dense_bool_storage() {
        let src = r#"
Flags = {
    vals: _ = []
    push(@self, b: bool) { self.vals = host.sys_arr_push(self.vals, b) }
    get(self, i: i64) : bool => self.vals[i]
}
count(xs: _) : i64 {
    c = 0
    for xs { if it { c += 1 } }
    c
}
main() {
    flags = [true, false, true]
    flags = host.sys_arr_push(flags, true)
    flags[1] = true
    print(flags)
    print(count(flags))
    f = Flags{}
    for 0..5 { @f.push(it % 2 == 0) }
    print(f.get(0))
    print(f.get(1))
    print(f.vals)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "bools");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(c.contains("bool *"), "dense bool storage");
        assert_eq!(
            assert_aot_matches_vm(src),
            "[true, true, true, true]\n4\ntrue\nfalse\n[true, false, true, false, true]\n"
        );
    }

    #[test]
    fn boxed_result_merges_back_into_typed_value() {
        // `boxed() or fallback` keeps the boxed result as a `SuperVal` and unboxes it at the merge.
        let src = r#"
P = {
    x: i64
    ys: _ = []
}
lookup(k: i64) : P? {
    junk = [k, "z"]
    if k > host.sys_arr_len(junk) { return P{x: k, ys: [k, k + 1]} }
    none
}
main() {
    seed = P{x: 0, ys: [9]}
    a = lookup(3) or seed
    b = lookup(-1) or seed
    print(a.x + b.x)
    print(a.ys)
    print(b.ys)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "dyn_merge");
        assert!(
            !boxed_fns(&c).iter().any(|f| f == "main"),
            "{}",
            boxed_notes(&c)
        );
        assert_eq!(assert_aot_matches_vm(src), "3\n[3, 4]\n[9]\n");
    }

    #[test]
    fn host_str_push_i64_uses_in_place_append() {
        let src = r#"
main() {
    buf = ""
    i = 0
    while i < 3 {
        buf = host.sys_str_push_i64(buf, i)
        buf = host.sys_str_push_ascii(buf, 44)
        i += 1
    }
    print(buf)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "push_append");
        let main_c: String = c
            .lines()
            .skip_while(|l| !l.contains("super_u_main"))
            .take_while(|l| !l.starts_with("SuperVal "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            main_c.contains("sup_sapp_i(&l") || c.contains("sup_sapp_i(&l0"),
            "expected in-place int append in main, got:\n{main_c}"
        );
        assert!(
            c.contains("sup_sapp_ascii(&l"),
            "expected in-place ascii append"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out.trim(), "0,1,2,");
    }

    #[test]
    fn in_place_string_append_keeps_value_semantics() {
        let src = r#"
main() {
    s = "ab"
    alias = s
    s = s + "c"
    one = 1
    s = "{s}{one}"
    print(alias)
    print(s)
    a, b = "x", "y"
    for 0..3 { a = a + "1" }
    a, b = b, a
    b = b + "!"
    print(a)
    print(b)
    acc = ""
    for 0..40 { acc = acc + it }
    snap = acc
    ok, f = true, 1.5
    acc = "{acc}end{ok}{f}"
    print(snap)
    print(acc)
    w = ""
    for 0..3 { w = "{w}[{it}]" }
    print(w)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "sapp");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            c.contains("sup_sapp_s(&l"),
            "string accumulators append in place"
        );
        assert!(
            c.contains("sup_sapp_i(&l"),
            "int operands append without a temporary"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(&lines[..4], ["ab", "abc1", "y", "x111!"]);
        let digits: String = (0..40).map(|i| i.to_string()).collect();
        assert_eq!(lines[4], digits);
        assert_eq!(lines[5], format!("{digits}endtrue1.5"));
        assert_eq!(lines[6], "[0][1][2]");
    }

    #[test]
    fn char_indexed_string_ops_match_vm_in_any_seek_order() {
        let src = r#"
main() {
    s = "héllo wörld/é/x"
    n = host.sys_str_len(s)
    out = ""
    for 0..n { out = out + s[it] + "." }
    print(out)
    print(s[n - 1])
    print(s[2])
    print(host.sys_str_find(s, "wö", 0))
    print(host.sys_str_find(s, "/", 12))
    print(host.sys_str_find(s, "é", 2))
    print(host.sys_str_find(s, "zz", 0))
    print(host.sys_str_match_at(s, "llo", 2))
    print(host.sys_str_match_at(s, "llo", 3))
    print(host.sys_str_match_at(s, "x", n - 1))
    print(s[6..11])
    print(s[13..100])
    print(s[0..1])
    print(s[1..1] == "")
    t = "aé"
    print(t[1])
    print(s[1])
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "utf8_seek");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(c.contains("sup_str_find(") && c.contains("sup_str_match_at("));
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(
            out,
            "h.é.l.l.o. .w.ö.r.l.d./.é./.x.\nx\nl\n6\n13\n12\n-1\ntrue\nfalse\ntrue\nwörld\n/x\nh\ntrue\né\né\n"
        );
    }

    #[test]
    fn single_store_module_constants_fold_into_readers() {
        let src = r#"
+MASK = 255
+SHIFT = 4
mix(x: i64) : i64 => ((x >>> SHIFT) ^ x) & MASK
main() {
    t = 0
    for 0..1000 { t = t + mix(it * 2654435761) }
    print(t)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "gconst");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            !c.lines()
                .any(|l| l.contains("= g_MASK") || l.contains("= g_SHIFT")),
            "constant globals are folded, not reloaded"
        );
        assert_aot_matches_vm(src);
    }

    #[test]
    fn fresh_array_returns_stay_on_the_arena() {
        let src = r#"
squares(n: i64) : _ {
    out = [0; n]
    for 0..n { out[it] = it * it }
    out
}
main() {
    total = 0
    for 0..2000 {
        xs = squares(it % 50 + 1)
        total = total + xs[xs.len - 1]
    }
    print(total)
    keep = squares(5)
    more = squares(3)
    print(keep)
    print(more)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "arena_ret");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            c.contains("SUP_KEEP1(sup_mark, "),
            "the returned array is kept above the mark"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        let want: i64 = (0..2000).map(|i: i64| (i % 50) * (i % 50)).sum();
        assert_eq!(out, format!("{want}\n[0, 1, 4, 9, 16]\n[0, 1, 4]\n"));
    }

    /// `name` is emitted as a region bracket around `name_rb`.
    fn is_region(c: &str, name: &str) -> bool {
        let tag = format!("_u_{name}_");
        c.lines().any(|l| {
            l.contains(&tag) && l.contains("_rb(") && !l.trim_start().starts_with("static")
        })
    }

    #[test]
    fn string_builders_run_in_regions_and_keep_their_result() {
        let src = r#"
label(i: i64) : str {
    s = ""
    for 0..i { s = s + "ab" }
    "<{s}>"
}
inner(i: i64) : str => label(i) + label(i + 1)
total(n: i64) : i64 {
    t = 0
    for 0..n { t += host.sys_str_len(inner(it % 7)) }
    t
}
main() {
    print(total(5000))
    print(label(3))
    print(inner(1))
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "region_str");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(is_region(&c, "label"), "string result is kept: {c}");
        assert!(is_region(&c, "total"), "pointer-free result is scoped");
        assert!(c.contains("sup_region_keep_str(sup_rm, r)"));
        let want: i64 = (0..5000).map(|i: i64| 4 * (i % 7) + 6).sum();
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out, format!("{want}\n<ababab>\n<ab><abab>\n"));
    }

    #[test]
    fn pointers_escaping_through_mut_params_or_failures_stay_on_the_heap() {
        let src = r#"
Box = {
    s: str = ""
    n: i64 = 0
}
fill(@b: Box, i: i64) {
    b.s = "v{i}"
    b.n = i
}
check(s: str) : i64! {
    ensure s != "" else { fail "empty {s}!" }
    host.sys_str_len(s)
}
main() {
    b = Box{}
    for 0..100 { @b.fill(it) }
    print(b.s)
    print(check("abc") or 0)
    r = check("") or err {
        print("err: {err}")
        7
    }
    print(r)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "region_escape");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            !is_region(&c, "fill"),
            "`@` field write of a fresh string escapes"
        );
        assert!(
            !is_region(&c, "check"),
            "a failing frame hands its message out"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out, "v99\n3\nerr: empty !\n7\n");
    }

    #[test]
    fn frames_storing_strings_into_mut_array_params_are_not_regions() {
        let src = r#"
poke(@names: _, i: i64) : i64 {
    names[i % 3] = "k{i}"
    i
}
fill(@names: _, n: i64) : i64 {
    t = 0
    for 0..n { t += @names.poke(it) }
    t
}
main() {
    names = ["", "", ""]
    print(@names.fill(50))
    print(names)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "region_pstore");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            !is_region(&c, "poke"),
            "a fresh string stored into an `@` array escapes"
        );
        assert!(
            !is_region(&c, "fill"),
            "a callee storing into the caller's array escapes"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out, "1225\n[\"k48\", \"k49\", \"k47\"]\n");
    }

    #[test]
    fn struct_field_writes_of_non_allocating_array_results_keep_callers_in_regions() {
        let src = r#"
W = {
    xs: _ = []
    n: i64 = 0
}
poke(xs: _, i: i64, v: i64) : _ {
    xs[i] = v
    xs
}
bump(@w: W, i: i64) {
    w.xs = poke(w.xs, i, w.xs[i] + 1)
}
step(@w: W, n: i64) : i64 {
    tag = ""
    for 0..n {
        @w.bump(it % 4)
        tag = "{it}"
    }
    host.sys_str_len(tag)
}
main() {
    w = W{xs: [0, 0, 0, 0]}
    print(@w.step(10))
    print(w.xs)
}
"#;
        let (chunk, interner, _) = compile_chunk(src, true);
        let c = emit_c(&chunk, &interner, "region_alias");
        assert!(boxed_fns(&c).is_empty(), "{}", boxed_notes(&c));
        assert!(
            is_region(&c, "step"),
            "storing back a param array allocates nothing: {c}"
        );
        let Some((_, out)) = build_and_run(src) else {
            return;
        };
        assert_eq!(out, "1\n[3, 3, 2, 2]\n");
    }
}
