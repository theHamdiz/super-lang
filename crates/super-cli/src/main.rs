//! Super CLI toolchain: run, test, build, install, clean, check, lsp, mcp, bindgen.

mod bindgen;
mod clean;
mod pkg;
mod pod_archive;
mod project;
mod ship;
mod ui;

use clap::{Args, CommandFactory, Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use super_core::SourceFile;
use super_frontend::{
    CfgContext, ProjectAnalyze, SolverKind, VerifyOptions, analyze,
    analyze_project_with_paths_and_cfg, check_benchmark_file, find_benchmarks_root,
    format_laws_lock, parse_laws_lock, path_is_benchmark_source, prove_law_bench_001,
    verify_module,
};
use super_host::default_host_env;
use super_runtime::{
    Compiler, EmitCOptions, IncrementalEmitOptions, JitEngine, ResolvedProfile, StackPool, Vm,
    disassemble, disassemble_native, emit_c_module, emit_incremental_sources, optimize_module,
    run_module_with_host, run_tests_with_host, syntax_check_c, write_and_compile_incremental,
    write_and_maybe_compile_with,
};

#[derive(Parser, Debug)]
#[command(
    name = "super",
    version,
    about = "The compiler and package manager for the Super language",
    long_about = "Build, run, test, install, and clean Super programs.\n\n\
`super install` (`super i`) ships a native release binary onto PATH.\n\
`super uninstall` (`super u`) removes a binary installed that way.\n\
`super list` and `super which` show those binaries.\n\
`super clean` removes target/ artifacts (`-d` debug, `-r` release, `-a` all).\n\
`super check` (`super c`) type-checks and syntax-checks the emitted C.\n\
`super verify` (`super v`) discharges #law invariants (zero runtime overhead).\n\
`super targets` lists every compile destination (linux/windows/macos/wasm/…).\n\
`super mcp` starts the local agent knowledge MCP server.\n\
`super help` and `super help <COMMAND>` print usage.",
    disable_help_subcommand = false,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Native code generation and link settings, shared by `build` and `emit-c`.
#[derive(Args, Debug)]
struct NativeOpts {
    #[command(flatten)]
    features: FeatureOpts,
    /// Release profile: `-O3`, fat LTO, strip, `-march=native`, unroll
    #[arg(short = 'r', long)]
    release: bool,
    /// Debug profile: `-O0 -g`, tests kept
    #[arg(short = 'd', long, visible_alias = "dev")]
    debug: bool,
    /// Named profile: `dev`, `test`, `release`, `size`, `fast`
    #[arg(long, value_name = "NAME")]
    profile: Option<String>,
    /// Optimization level: `0`, `1`, `2`, `3`, `s`, `z`, `g`, `fast`
    #[arg(short = 'O', long = "opt", value_name = "LEVEL")]
    opt: Option<String>,
    /// Link-time optimization: `off`, `thin`, or `fat`. `--lto` alone means thin.
    #[arg(
        long,
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "thin"
    )]
    lto: Option<String>,
    /// Strip symbols from the linked binary (`-s`)
    #[arg(short = 's', long)]
    strip: bool,
    /// Keep symbols even when the profile strips them
    #[arg(long, conflicts_with = "strip")]
    no_strip: bool,
    /// Emit debug info (`-g`)
    #[arg(short = 'g', long = "debug-info")]
    debug_info: bool,
    /// Statically link the C runtime (`-static`). Opt-in — prefer a musl target for true static builds.
    /// Default release/size/fast stays dynamic on glibc so getaddrinfo/dlopen do not warn.
    #[arg(long = "static", conflicts_with_all = ["shared", "no_static"])]
    static_link: bool,
    /// Dynamically link the C runtime (default; useful when a Pod.toml sets `static = true`)
    #[arg(long = "no-static", conflicts_with = "static_link")]
    no_static: bool,
    /// `-march` CPU. Bare `--target-cpu` means `native`. `generic` omits `-march`.
    #[arg(
        long,
        visible_alias = "march",
        value_name = "CPU",
        num_args = 0..=1,
        default_missing_value = "native"
    )]
    target_cpu: Option<String>,
    /// Compile destination triple (`native`, `wasm32-wasi`, `aarch64-unknown-linux-gnu`, …).
    ///
    /// Overrides `[build] target` / `[build] targets`. See `super targets`.
    #[arg(long = "target", value_name = "TRIPLE")]
    target: Option<String>,
    /// Enable `-ffast-math` (already on for `release` and `fast`)
    #[arg(long)]
    fast_math: bool,
    /// Disable `-ffast-math`
    #[arg(long, conflicts_with = "fast_math")]
    no_fast_math: bool,
    /// Extra linker argument, repeated. Bare tokens are passed as `-Wl,<ARG>`.
    #[arg(long = "link-arg", value_name = "ARG")]
    link_args: Vec<String>,
    /// Build a shared library (`.so` / `.dylib` / `.dll`)
    #[arg(long)]
    shared: bool,
    /// C codegen split: `incremental` (default) or `single` (one `.c` file).
    #[arg(long = "codegen-units", value_name = "MODE")]
    codegen_units: Option<String>,
    /// Keep generated `.c` / `.o` under `target/<profile>/` after linking.
    #[arg(long = "keep-c")]
    keep_c: bool,
    /// Discard intermediate `.c` / `.o` after a successful link (default).
    #[arg(long = "no-keep-c", conflicts_with = "keep_c")]
    no_keep_c: bool,
    /// Max C statements per function before outlining (`0` disables; default 1500).
    #[arg(long = "outline-stmt-threshold", value_name = "N")]
    outline_stmt_threshold: Option<usize>,
}

#[derive(Args, Clone, Debug, Default)]
struct FeatureOpts {
    /// Enable comma-separated Pod features. May be repeated.
    #[arg(long, value_name = "FEATURE", value_delimiter = ',')]
    features: Vec<String>,
    /// Enable every feature declared in `[features]`.
    #[arg(long, conflicts_with = "features")]
    all_features: bool,
    /// Do not enable the `[features].default` set.
    #[arg(long)]
    no_default_features: bool,
}

impl NativeOpts {
    fn choices<'a>(
        &'a self,
        std: Option<&'a str>,
        cc: Option<&'a str>,
        out: Option<&'a Path>,
        c23: bool,
    ) -> pkg::CliBuildChoices<'a> {
        let strip = if self.no_strip {
            Some(false)
        } else if self.strip {
            Some(true)
        } else {
            None
        };
        let fast_math = if self.no_fast_math {
            Some(false)
        } else if self.fast_math {
            Some(true)
        } else {
            None
        };
        pkg::CliBuildChoices {
            release: self.release,
            debug: self.debug,
            profile: self.profile.as_deref(),
            opt: self.opt.as_deref(),
            lto: self.lto.as_deref(),
            strip,
            debug_info: self.debug_info.then_some(true),
            fast_math,
            static_link: if self.no_static {
                Some(false)
            } else if self.static_link {
                Some(true)
            } else {
                None
            },
            target_cpu: self.target_cpu.as_deref(),
            link_args: &self.link_args,
            shared: self.shared,
            c23,
            std,
            cc,
            out,
            backend: None,
            mode: None,
            target: self.target.as_deref(),
            force_native: false,
            codegen_units: self
                .codegen_units
                .as_deref()
                .and_then(|raw| pkg::CodegenUnits::parse(raw).ok()),
            keep_c: if self.no_keep_c {
                Some(false)
            } else if self.keep_c {
                Some(true)
            } else {
                None
            },
            outline_stmt_threshold: self.outline_stmt_threshold,
            features: &self.features.features,
            all_features: self.features.all_features,
            no_default_features: self.features.no_default_features,
        }
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Create a new Super Pod (`Pod.toml`)
    New {
        /// Pod name and destination directory
        name: String,
        /// Create a reusable library Pod (`kind = "lib"`)
        #[arg(long, group = "kind")]
        lib: bool,
        /// Create an executable Pod (`kind = "bin"`, the default)
        #[arg(long, group = "kind")]
        bin: bool,
        /// Create a comptime macro Pod (`kind = "macro"`)
        #[arg(long = "macro", group = "kind")]
        macro_pod: bool,
    },
    /// Initialize a Super Pod in the current directory
    Init {
        /// Create a reusable library Pod
        #[arg(long, group = "init_kind")]
        lib: bool,
        /// Create an executable Pod (the default)
        #[arg(long, group = "init_kind")]
        bin: bool,
        /// Create a comptime macro Pod
        #[arg(long = "macro", group = "init_kind")]
        macro_pod: bool,
    },
    /// Lint Super sources with named rules (`super lint --rules` lists them)
    Lint {
        /// Files or directories
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Apply the safe mechanical fixes (S007, S008) in place
        #[arg(long)]
        fix: bool,
        /// Print the rule table and exit
        #[arg(long)]
        rules: bool,
        /// TOML file with a `[lint]` table, used instead of each file's Pod.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Format Super source files in place (`-` formats stdin to stdout)
    Fmt {
        /// Files or directories; `-` reads stdin and writes stdout
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Do not write; exit 1 and list the files that are not formatted
        #[arg(long)]
        check: bool,
    },
    /// Generate Markdown API documentation from `///` comments
    Doc {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Add a path or Git dependency to Pod.toml
    Add { package: String },
    /// Remove a dependency from Pod.toml
    Remove { package: String },
    /// Update dependencies and rewrite Pod.lock / Super.lock
    Update,
    /// Fetch dependencies and write Pod.lock / Super.lock
    Fetch,
    /// Copy resolved dependencies into vendor/
    Vendor,
    /// Publish the current Pod into the local filesystem registry (`~/.super/registry`)
    Publish,
    /// Pack / extract / verify `.pod` archives (tar+zstd; D2–D4)
    Pod {
        #[command(subcommand)]
        command: PodCmd,
    },
    /// Generate shell completion scripts
    Completions { shell: clap_complete::Shell },
    /// Compile and run a Super program (`super r`).
    /// With no file, uses Pod.toml and src/main.sr at the package root.
    #[command(visible_alias = "r")]
    Run {
        file: Option<PathBuf>,
        /// Use JIT where profitable
        #[arg(long)]
        jit: bool,
    },
    /// Run all Super test blocks under a path
    Test {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Discharge `#law` invariants (`super v`).
    ///
    /// Runs the three-tier verifier (SMT bitvector, comptime rewrite, bounded
    /// fuzz). `#law` / `#proof` never emit into C23, JIT, or bytecode.
    #[command(visible_alias = "v")]
    Verify {
        /// Source file or package root (default: current Pod entry)
        file: Option<PathBuf>,
        /// Fail if any `#law` body differs from `laws.lock` (CI mode)
        #[arg(long)]
        locked_laws: bool,
        /// Solver: `auto` (default), `smt`, `comptime`, or `fuzz`
        #[arg(long, value_name = "SOLVER")]
        solver: Option<String>,
        /// Per-law timeout in milliseconds
        #[arg(long, value_name = "MS")]
        timeout: Option<u64>,
        /// Emit a machine-readable JSON report on stdout
        #[arg(long)]
        json: bool,
    },
    /// Type-check a source file or the current package, then syntax-check the emitted C (`super c`).
    ///
    /// A green check means the host C compiler accepts the same C23 translation
    /// `super build` would compile. Diagnostics use the same cyan status colors
    /// as the build steps.
    #[command(visible_alias = "c")]
    Check { file: Option<PathBuf> },
    /// Build a native binary (`super b`).
    ///
    /// The default backend is `c23` and the default mode is `aot`, from
    /// `[build]` in Pod.toml when those keys are set. `-r`
    /// selects the release profile. Host binaries land at
    /// `target/<profile>/<package>`; an explicit `--target` (or
    /// `[build] target` / `targets`) writes
    /// `target/<triple>/<profile>/<package>` (`.wasm` for WebAssembly).
    /// `--mode bytecode` writes a portable `.superb` module instead.
    /// With no file, uses Pod.toml and the Pod entry.
    #[command(visible_alias = "b")]
    Build {
        file: Option<PathBuf>,
        #[command(flatten)]
        native: NativeOpts,
        /// Code generator: `c23` (default), `jit`, or `vm`
        #[arg(long, value_name = "BACKEND")]
        backend: Option<String>,
        /// `aot` (default native binary), `jit`, or `bytecode`
        #[arg(long, value_name = "MODE")]
        mode: Option<String>,
        /// Transpile to C and compile with the host C compiler
        #[arg(long)]
        c: bool,
        /// Emit C only (no C compile / link step)
        #[arg(long)]
        emit_c: bool,
        /// Force C compile after emit (redundant with `--c`; useful with `--emit-c --cc`)
        #[arg(long)]
        cc: bool,
        /// C compiler binary (default: `cc`); implies `--c`
        #[arg(long, value_name = "BIN", alias = "compiler")]
        cc_bin: Option<String>,
        /// Output path for generated C (default: `<file>.c`)
        #[arg(long, value_name = "PATH")]
        out_c: Option<PathBuf>,
        /// Output path for compiled binary or shared library
        #[arg(short, long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// C language standard (default `c23`, or `[build].std` in Pod.toml)
        #[arg(long = "std", value_name = "STD")]
        c_std: Option<String>,
        /// Compile through the C23 backend (alias for `--c --std c23`)
        #[arg(long)]
        c23: bool,
    },
    /// Install the current program onto PATH at the maximum release profile (`super i`).
    ///
    /// Builds a native C23 binary and forces the release profile to its peak:
    /// `-O3`, fat LTO, `-march=native`, `-ffast-math`, unroll, and strip. Test
    /// blocks are stripped. The executable name is the package name (`name.exe`
    /// on Windows).
    ///
    /// The destination is `--bin-dir`, else `SUPER_INSTALL_DIR`, else the first
    /// user bin already on `PATH` (`~/.local/bin`, `~/bin`, `~/.cargo/bin`),
    /// else `~/.local/bin`. On Windows those directories sit under the user
    /// profile. When Super creates `~/.local/bin` and it is not on `PATH`, it
    /// adds that directory to the user environment: the shell startup file on
    /// Unix (zsh, bash, fish, nushell, or POSIX sh) and the user `Path` on
    /// Windows. A new terminal can then run the program by name.
    ///
    /// Run it inside a package, from any subdirectory, or pass a path. A
    /// workspace is a `Pod.toml` with `[workspace] members`, or a directory
    /// whose children are packages. Every binary member is installed. A library
    /// (no `src/main.sr`) is skipped. Re-running replaces a binary Super
    /// installed. `--force` replaces an executable Super did not install.
    ///
    /// Receipts live in `installed.toml` under `SUPER_DATA_DIR`,
    /// `$XDG_DATA_HOME/super`, `~/.local/share/super`, or `%LOCALAPPDATA%\super`.
    #[command(visible_alias = "i")]
    Install {
        /// Package or workspace directory (default: the current directory)
        path: Option<PathBuf>,
        /// Directory that receives the executable. Overrides `SUPER_INSTALL_DIR`.
        #[arg(long, value_name = "DIR")]
        bin_dir: Option<PathBuf>,
        /// Replace an existing executable that Super did not install
        #[arg(long)]
        force: bool,
    },
    /// Remove a program installed by `super install` (`super u`).
    ///
    /// With no name, uninstalls the package in the current directory. Only
    /// binaries recorded by `super install` are removed. The `PATH` entry itself
    /// stays, so other installed programs keep working.
    #[command(visible_alias = "u")]
    Uninstall {
        /// Installed package name. Defaults to the package around the current directory.
        name: Option<String>,
    },
    /// List programs installed by `super install`.
    ///
    /// Prints name, version, executable path, and package root. A missing file
    /// is marked `(missing)`.
    #[command(visible_alias = "installed")]
    List,
    /// Print the executable path of an installed program.
    ///
    /// With no name, uses the package around the current directory. Prints only
    /// the path, so the result can be used in a script. Exits with an error
    /// when the program is not installed or the file is gone.
    Which {
        /// Installed package name. Defaults to the current package.
        name: Option<String>,
    },
    /// Remove binary artifacts under `target/`. `-a` is the default.
    ///
    /// `-d` / `--debug` removes debug binaries (`target/dev` and `target/debug`).
    /// `-r` / `--release` removes release binaries (`target/release`).
    /// `-a` / `--all` removes the whole `target/` directory, including dev,
    /// test, release, fast, and size. With no flag, `-a` is used.
    ///
    /// Pass a package, a workspace, or omit the path to use the current
    /// directory. A workspace cleans every member.
    Clean {
        /// Package or workspace directory (default: the current directory)
        path: Option<PathBuf>,
        /// Remove debug binaries (`target/dev`, `target/debug`)
        #[arg(short = 'd', long = "debug", conflicts_with_all = ["release", "all"])]
        debug: bool,
        /// Remove release binaries (`target/release`)
        #[arg(short = 'r', long = "release", conflicts_with_all = ["debug", "all"])]
        release: bool,
        /// Remove every profile under `target/` (the default)
        #[arg(short = 'a', long = "all", conflicts_with_all = ["debug", "release"])]
        all: bool,
    },
    /// List every compile destination Super accepts (`--target`, `[build] target`).
    ///
    /// Prints the host triple plus the catalog of linux, windows, macos,
    /// android, freebsd, and WebAssembly (`wasm32-wasi`, `wasm32-unknown-unknown`, …)
    /// destinations. Cross and wasm builds prefer `zig cc` when installed.
    #[command(visible_alias = "triples")]
    Targets,
    /// Emit optimized C AOT source (no compile unless `--cc`)
    EmitC {
        file: Option<PathBuf>,
        #[command(flatten)]
        native: NativeOpts,
        #[arg(short, long, value_name = "PATH")]
        out: Option<PathBuf>,
        #[arg(long)]
        cc: bool,
        #[arg(long, value_name = "BIN", alias = "compiler")]
        cc_bin: Option<String>,
        /// C language standard (default `c23`, or `[build].std` in Pod.toml)
        #[arg(long = "std", value_name = "STD")]
        c_std: Option<String>,
    },
    /// Start language server (stdio)
    Lsp,
    /// Start the embedded MCP knowledge server for coding agents (stdio)
    ///
    /// Speaks Model Context Protocol over newline-delimited JSON-RPC 2.0.
    /// Tools cover language features, algorithms, examples/, and stdlib coverage.
    Mcp,
    /// Run performance benchmarks (100M ECS + HTTP/JSON)
    Bench,
    /// Generate a C/C++ header from exported Super declarations
    Bindgen {
        file: PathBuf,
        /// Output header path
        #[arg(long)]
        out: PathBuf,
    },
    /// Disassemble optimized bytecode (and optional native JIT dump)
    Disasm {
        file: PathBuf,
        /// Also emit / dump native machine code when the JIT accepts the chunk
        #[arg(long)]
        jit: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PodCmd {
    /// Build a `.pod` (tar+zstd), then extract → compile → test VM/JIT/C23 unless `--skip-gate`
    Pack {
        /// Output archive path (default: `target/<name>-<version>.pod`)
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,
        /// Skip extract/compile/test gate (still writes the archive + Pod.lock hash)
        #[arg(long)]
        skip_gate: bool,
        /// Pod root (default: current package)
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Extract a `.pod` into a directory (rejects unsafe members)
    Extract {
        /// Archive to extract
        file: PathBuf,
        /// Destination directory (created)
        #[arg(long, short = 'd')]
        dir: PathBuf,
    },
    /// Verify zstd magic, `pod.manifest` first entry, and optional `--hash`
    Verify {
        file: PathBuf,
        /// Expected `sha256:…` content hash (or read from Pod.lock when omitted beside a Pod)
        #[arg(long)]
        hash: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::New {
            name,
            lib,
            bin: _,
            macro_pod,
        } => {
            let root = PathBuf::from(&name);
            if root.exists() {
                command_error(format!("destination `{}` already exists", root.display()))
            } else {
                let package_name = root
                    .file_name()
                    .and_then(|part| part.to_str())
                    .unwrap_or(&name);
                let kind = if macro_pod {
                    pkg::PodKind::Macro
                } else if lib {
                    pkg::PodKind::Lib
                } else {
                    pkg::PodKind::Bin
                };
                command_result(project::scaffold_kind(&root, package_name, kind), |()| {
                    format!("created {} pod `{package_name}`", kind.name())
                })
            }
        }
        Commands::Init {
            lib,
            bin: _,
            macro_pod,
        } => {
            let cwd = match std::env::current_dir() {
                Ok(path) => path,
                Err(error) => return command_error(error),
            };
            let name = cwd
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("super_pod");
            let kind = if macro_pod {
                pkg::PodKind::Macro
            } else if lib {
                pkg::PodKind::Lib
            } else {
                pkg::PodKind::Bin
            };
            command_result(project::scaffold_kind(&cwd, name, kind), |()| {
                format!("initialized {} pod `{name}`", kind.name())
            })
        }
        Commands::Fmt { paths, check } => run_fmt(&paths, check),
        Commands::Lint {
            paths,
            fix,
            rules,
            config,
        } => run_lint(&paths, fix, rules, config.as_deref()),
        Commands::Doc { path } => match project::generate_docs(&path) {
            Ok(docs) => {
                print!("{docs}");
                if !docs.ends_with('\n') {
                    println!();
                }
                ui::say_ok("Finished  documentation");
                ExitCode::SUCCESS
            }
            Err(error) => command_error(error),
        },
        Commands::Add { package } => with_manifest(|manifest| {
            let name = pkg::add(manifest, &package)?;
            pkg::resolve_workspace(manifest.parent().unwrap_or(Path::new(".")), false)?;
            Ok(format!("added `{name}`"))
        }),
        Commands::Remove { package } => with_manifest(|manifest| {
            pkg::remove(manifest, &package)?;
            pkg::resolve_workspace(manifest.parent().unwrap_or(Path::new(".")), false)?;
            Ok(format!("removed `{package}`"))
        }),
        Commands::Update => with_workspace(|start| {
            let packages = pkg::resolve_workspace(start, true)?;
            Ok(format!("updated {} package(s)", packages.len()))
        }),
        Commands::Fetch => with_workspace(|start| {
            let packages = pkg::resolve_workspace(start, false)?;
            Ok(format!("fetched {} package(s)", packages.len()))
        }),
        Commands::Publish => with_manifest(pkg::publish),
        Commands::Pod { command } => cmd_pod(command),
        Commands::Vendor => with_manifest(|manifest| {
            let count = pkg::vendor(manifest)?;
            Ok(format!("vendored {count} package(s)"))
        }),
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "super", &mut std::io::stdout());
            ExitCode::SUCCESS
        }
        Commands::Run { file, jit } => {
            let file = match resolve_entry(file) {
                Ok(file) => file,
                Err(error) => return command_error(error),
            };
            let applied = match load_applied(&file, pkg::CliBuildChoices::default()) {
                Ok(applied) => applied,
                Err(error) => return command_error(error),
            };
            cmd_run(
                &file,
                jit,
                applied.profile.strip_tests,
                &applied.target,
                &applied.features,
                matches!(
                    applied.profile.profile,
                    super_runtime::BuildProfile::Dev | super_runtime::BuildProfile::Test
                ),
            )
        }
        Commands::Test { path } => cmd_test(&path),
        Commands::Verify {
            file,
            locked_laws,
            solver,
            timeout,
            json,
        } => match resolve_entry(file) {
            Ok(file) => cmd_verify(&file, locked_laws, solver.as_deref(), timeout, json),
            Err(error) => command_error(error),
        },
        Commands::Check { file } => match resolve_entry(file) {
            Ok(file) => {
                let mut ui = ui::BuildUi::new(ui::CHECK_STEPS);
                cmd_check(&file, &mut ui)
            }
            Err(error) => command_error(error),
        },
        Commands::Build {
            file,
            native,
            backend,
            mode,
            c,
            emit_c,
            cc,
            cc_bin,
            out_c,
            out,
            c_std,
            c23,
        } => {
            let entries = match resolve_build_entries(file) {
                Ok(entries) => entries,
                Err(error) => return command_error(error),
            };
            let mut last = ExitCode::SUCCESS;
            for file in entries {
                let mut choices =
                    native.choices(c_std.as_deref(), cc_bin.as_deref(), out.as_deref(), c23);
                choices.backend = backend.as_deref();
                choices.mode = mode.as_deref();
                choices.force_native = c || c23 || cc || cc_bin.is_some();
                let applied = match load_applied(&file, choices) {
                    Ok(applied) => applied,
                    Err(error) => return command_error(error),
                };
                if maybe_require_verify(&file, applied.profile.strip_tests) != ExitCode::SUCCESS {
                    return ExitCode::FAILURE;
                }
                // `--emit-c` alone writes C and does not link. `--cc` links that C.
                let explicit_c = c || c23 || cc || cc_bin.is_some();
                let emit_only = emit_c && !explicit_c;
                let bytecode = !emit_c && !applied.mode.native();
                let steps: &[&str] = if bytecode {
                    ui::BYTECODE_STEPS
                } else if emit_only {
                    ui::EMIT_STEPS
                } else {
                    ui::NATIVE_STEPS
                };
                let targets = applied.targets.clone();
                for (index, target) in targets.iter().enumerate() {
                    let one = if targets.len() == 1 {
                        applied.clone()
                    } else {
                        match rebind_applied(&file, &applied, target.clone()) {
                            Ok(one) => one,
                            Err(error) => return command_error(error),
                        }
                    };
                    if targets.len() > 1 {
                        ui::say_ok(&format!(
                            "Target    {} ({}/{})",
                            one.target.triple,
                            index + 1,
                            targets.len()
                        ));
                    }
                    let mut progress = ui::BuildUi::new(steps);
                    last = cmd_build(&file, &one, emit_only, out_c.as_deref(), &mut progress);
                    if last != ExitCode::SUCCESS {
                        return last;
                    }
                }
            }
            last
        }
        Commands::Install {
            path,
            bin_dir,
            force,
        } => cmd_install(path, bin_dir, force),
        Commands::Uninstall { name } => cmd_uninstall(name),
        Commands::List => cmd_list(),
        Commands::Which { name } => cmd_which(name),
        Commands::Clean {
            path,
            debug,
            release,
            all,
        } => cmd_clean(path, clean::CleanMode::from_flags(debug, release, all)),
        Commands::Targets => {
            print!("{}", super_runtime::format_catalog());
            ExitCode::SUCCESS
        }
        Commands::EmitC {
            file,
            native,
            out,
            cc,
            cc_bin,
            c_std,
        } => {
            let file = match resolve_entry(file) {
                Ok(file) => file,
                Err(error) => return command_error(error),
            };
            let applied = match load_applied(
                &file,
                native.choices(c_std.as_deref(), cc_bin.as_deref(), out.as_deref(), false),
            ) {
                Ok(applied) => applied,
                Err(error) => return command_error(error),
            };
            cmd_emit_c(
                &file,
                &applied.profile,
                applied.out.as_deref(),
                cc || applied.cc.is_some(),
                applied.cc.as_deref(),
                applied.shared,
                &applied.std,
                applied.outline_stmt_threshold,
                &applied.target,
                &applied.features,
            )
        }
        Commands::Lsp => {
            if let Err(e) = super_lsp::run_stdio() {
                ui::say_err(&format!("error: lsp: {e}"));
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Commands::Mcp => {
            if let Err(e) = super_mcp::run_stdio() {
                ui::say_err(&format!("error: mcp: {e}"));
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Commands::Bench => cmd_bench(),
        Commands::Bindgen { file, out } => match bindgen::generate(&file, &out) {
            Ok(()) => {
                ui::say_ok(&format!("Finished  wrote {}", out.display()));
                ExitCode::SUCCESS
            }
            Err(e) => {
                ui::say_err(&format!("error: bindgen: {e}"));
                ExitCode::FAILURE
            }
        },
        Commands::Disasm { file, jit } => cmd_disasm(&file, jit),
    }
}

fn run_lint(paths: &[PathBuf], fix: bool, rules: bool, config: Option<&Path>) -> ExitCode {
    use super_frontend::lint::Rule;
    if rules {
        for rule in Rule::ALL {
            let fixable = if matches!(rule, Rule::RedundantPlus | Rule::RedundantPun) {
                " (fixable)"
            } else {
                ""
            };
            println!(
                "{}  {:<15} {:<8} {}{fixable}",
                rule.id(),
                rule.name(),
                rule.default_severity().label(),
                rule.summary()
            );
        }
        return ExitCode::SUCCESS;
    }
    let report = match project::lint_paths(paths, fix, config) {
        Ok(report) => report,
        Err(error) => return command_error(error),
    };
    let mut counts = [0usize; 3];
    for (file, finding) in &report.findings {
        counts[finding.severity as usize] += 1;
        println!(
            "{}:{}:{}: {}[{} {}]: {}",
            file.display(),
            finding.line,
            finding.column,
            finding.severity.label(),
            finding.rule.id(),
            finding.rule.name(),
            finding.message
        );
    }
    let [info, warnings, errors] = counts;
    let summary = format!(
        "{} file(s): {errors} error(s), {warnings} warning(s), {info} info, {} fixed",
        report.checked, report.fixed
    );
    if errors + warnings > 0 {
        eprintln!("{summary}");
        ExitCode::from(1)
    } else {
        ui::say_ok(&format!("Finished  {summary}"));
        ExitCode::SUCCESS
    }
}

fn run_fmt(paths: &[PathBuf], check: bool) -> ExitCode {
    use std::io::{Read, Write};
    if paths.len() == 1 && paths[0].as_os_str() == "-" {
        let mut source = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut source) {
            return command_error(error);
        }
        return match super_frontend::format::format_source(&source) {
            Ok(formatted) if check => {
                if formatted == source {
                    ExitCode::SUCCESS
                } else {
                    eprintln!("<stdin> is not formatted");
                    ExitCode::from(1)
                }
            }
            Ok(formatted) => {
                let mut stdout = std::io::stdout().lock();
                match stdout.write_all(formatted.as_bytes()) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => command_error(error),
                }
            }
            Err(error) => command_error(error),
        };
    }
    match project::format_paths(paths, check) {
        Ok(report) if check => {
            for file in &report.changed {
                println!("{}", file.display());
            }
            if report.changed.is_empty() {
                ui::say_ok(&format!(
                    "Finished  {} file(s) already formatted",
                    report.checked
                ));
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "{} of {} file(s) need `super fmt`",
                    report.changed.len(),
                    report.checked
                );
                ExitCode::from(1)
            }
        }
        Ok(report) => {
            ui::say_ok(&format!(
                "Finished  formatted {} of {} file(s)",
                report.changed.len(),
                report.checked
            ));
            ExitCode::SUCCESS
        }
        Err(error) => command_error(error),
    }
}

fn command_result<T, E, F>(result: Result<T, E>, success: F) -> ExitCode
where
    E: std::fmt::Display,
    F: FnOnce(T) -> String,
{
    match result {
        Ok(value) => {
            ui::say_ok(&format!("Finished  {}", success(value)));
            ExitCode::SUCCESS
        }
        Err(error) => command_error(error),
    }
}

fn command_error(error: impl std::fmt::Display) -> ExitCode {
    ui::say_err(&format!("error: {error}"));
    ExitCode::FAILURE
}

fn with_manifest(action: impl FnOnce(&Path) -> Result<String, String>) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => return command_error(error),
    };
    let Some(manifest) = pkg::find_manifest(&cwd) else {
        return command_error(format!("could not find {}", pkg::MANIFEST_POD));
    };
    command_result(action(&manifest), |message| message)
}

fn with_workspace(action: impl FnOnce(&Path) -> Result<String, String>) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => return command_error(error),
    };
    if pkg::find_manifest(&cwd).is_none() {
        return command_error(format!("could not find {}", pkg::MANIFEST_POD));
    }
    command_result(action(&cwd), |message| message)
}

fn default_entry() -> Option<PathBuf> {
    ["src/main.sr", "src/macros.sr", "src/lib.sr"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn resolve_build_entries(file: Option<PathBuf>) -> Result<Vec<PathBuf>, String> {
    if let Some(file) = file {
        if file.is_dir() {
            return resolve_entry(Some(file)).map(|entry| vec![entry]);
        }
        return Ok(vec![file]);
    }
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    if let Ok(roots) = ship::package_roots(&cwd) {
        let entries: Vec<_> = roots
            .iter()
            .filter_map(|root| pkg::package_entry(&root.manifest))
            .collect();
        if !entries.is_empty() {
            return Ok(entries);
        }
    }
    resolve_entry(None).map(|entry| vec![entry])
}

fn resolve_entry(file: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(file) = file {
        if file.is_dir() {
            let manifest = pkg::find_manifest(&file)
                .ok_or_else(|| format!("{} has no {}", file.display(), pkg::MANIFEST_POD))?;
            return pkg::package_entry(&manifest).ok_or_else(|| {
                format!(
                    "{} has no src/main.sr, src/macros.sr, src/lib.sr, or a single src/*.sr",
                    manifest.display()
                )
            });
        }
        return Ok(file);
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if let Some(manifest) = pkg::find_manifest(&cwd) {
        return pkg::package_entry(&manifest).ok_or_else(|| {
            format!(
                "{} has no src/main.sr, src/macros.sr, or src/lib.sr",
                manifest.display()
            )
        });
    }
    default_entry().ok_or_else(|| {
        format!(
            "no source file supplied and no {} was found",
            pkg::MANIFEST_POD
        )
    })
}

fn ensure_resolved(file: &Path) -> Result<(), String> {
    if pkg::find_manifest(file).is_some() {
        pkg::resolve_workspace(file, false)?;
    }
    Ok(())
}

fn cmd_install(path: Option<PathBuf>, bin_dir: Option<PathBuf>, force: bool) -> ExitCode {
    let start = match command_start(path) {
        Ok(start) => start,
        Err(error) => return command_error(error),
    };
    let roots = match ship::package_roots(&start) {
        Ok(roots) => roots,
        Err(error) => return command_error(error),
    };
    let binaries: Vec<_> = roots
        .iter()
        .filter(|root| root.is_binary())
        .cloned()
        .collect();
    for root in roots.iter().filter(|root| !root.is_binary()) {
        ui::say_warn(&format!(
            "skipped `{}` ({} pod; only bin pods install)",
            root.name,
            root.kind.name()
        ));
    }
    if binaries.is_empty() {
        return command_error("nothing to install: no bin pod with src/main.sr");
    }
    let names: Vec<_> = binaries.iter().map(|root| root.name.as_str()).collect();
    if let Some(dup) = duplicate_name(&names) {
        return command_error(format!(
            "two pods are named `{dup}`; install them separately"
        ));
    }
    let (dest_dir, note) = match ship::prepare_bin_dir(bin_dir.as_deref()) {
        Ok(prepared) => prepared,
        Err(error) => return command_error(error),
    };
    for package in &binaries {
        if let Some(entry) = pkg::package_entry(&package.root.join(pkg::MANIFEST_POD))
            && maybe_require_verify(&entry, true) != ExitCode::SUCCESS
        {
            return ExitCode::FAILURE;
        }
        let out = package
            .root
            .join("target")
            .join("release")
            .join(ship::executable_file_name(&package.name));
        let entry = package.main.as_deref().expect("binary package");
        if build_max_release(entry, &out) != ExitCode::SUCCESS {
            return ExitCode::FAILURE;
        }
        match ship::install_binary(package, &out, &dest_dir, force) {
            Ok(dest) => ui::say_ok(&format!(
                "Finished  installed {} {} → {}",
                package.name,
                package.version,
                dest.display()
            )),
            Err(error) => return command_error(error),
        }
    }
    if let Some(note) = note {
        ui::say_warn(&note);
    }
    ExitCode::SUCCESS
}

fn cmd_uninstall(name: Option<String>) -> ExitCode {
    let name = match name {
        Some(name) => name,
        None => match current_package_name() {
            Ok(name) => name,
            Err(error) => return command_error(error),
        },
    };
    match ship::uninstall_name(&name) {
        Ok(path) => {
            ui::say_ok(&format!(
                "Finished  uninstalled {name} ({})",
                path.display()
            ));
            ExitCode::SUCCESS
        }
        Err(error) => command_error(error),
    }
}

fn cmd_list() -> ExitCode {
    let rows = match ship::list_installed() {
        Ok(rows) => rows,
        Err(error) => return command_error(error),
    };
    if rows.is_empty() {
        ui::say_ok("no programs installed");
        return ExitCode::SUCCESS;
    }
    for row in rows {
        let missing = if row.bin.is_file() {
            String::new()
        } else {
            ui::paint_text(ui::Tone::Err, " (missing)")
        };
        println!(
            "{} {} {} {}{missing}",
            ui::paint_text(ui::Tone::Ok, &row.name),
            row.version,
            row.bin.display(),
            row.root.display()
        );
    }
    ExitCode::SUCCESS
}

fn cmd_which(name: Option<String>) -> ExitCode {
    let name = match name {
        Some(name) => name,
        None => match current_package_name() {
            Ok(name) => name,
            Err(error) => return command_error(error),
        },
    };
    match ship::which(&name) {
        Ok(path) => {
            println!("{}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => command_error(error),
    }
}

fn cmd_clean(path: Option<PathBuf>, mode: clean::CleanMode) -> ExitCode {
    let start = match command_start(path) {
        Ok(start) => start,
        Err(error) => return command_error(error),
    };
    match clean::discover_and_clean(&start, mode) {
        Ok(removed) => {
            ui::say_ok(&clean::report(&removed, mode));
            ExitCode::SUCCESS
        }
        Err(error) => command_error(error),
    }
}

fn command_start(path: Option<PathBuf>) -> Result<PathBuf, String> {
    match path {
        Some(path) if path.exists() => Ok(path),
        Some(path) => Err(format!("{} does not exist", path.display())),
        None => std::env::current_dir().map_err(|error| error.to_string()),
    }
}

fn current_package_name() -> Result<String, String> {
    let start = std::env::current_dir().map_err(|error| error.to_string())?;
    let roots = ship::package_roots(&start)?;
    let binaries: Vec<_> = roots.iter().filter(|root| root.is_binary()).collect();
    match binaries.as_slice() {
        [one] => Ok(one.name.clone()),
        [] => Err("the current package is a library; pass the program name".into()),
        many => Err(format!(
            "this directory installs {} programs; pass a name ({})",
            many.len(),
            many.iter()
                .map(|root| root.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn duplicate_name<'a>(names: &[&'a str]) -> Option<&'a str> {
    let mut seen = Vec::new();
    for name in names {
        if seen.contains(name) {
            return Some(*name);
        }
        seen.push(*name);
    }
    None
}

fn max_release_choices(out: &Path) -> pkg::CliBuildChoices<'_> {
    pkg::CliBuildChoices {
        release: true,
        debug: false,
        profile: Some("release"),
        opt: Some("3"),
        lto: Some("fat"),
        strip: Some(true),
        debug_info: Some(false),
        fast_math: Some(true),
        // Leave unset so release preset stays dynamic on glibc (no NSS ld warnings).
        static_link: None,
        target_cpu: Some("native"),
        link_args: &[],
        shared: false,
        c23: true,
        std: Some("c23"),
        cc: None,
        out: Some(out),
        backend: Some("c23"),
        mode: Some("aot"),
        target: None,
        force_native: true,
        codegen_units: None,
        keep_c: None,
        outline_stmt_threshold: None,
        features: &[],
        all_features: false,
        no_default_features: false,
    }
}

fn build_max_release(entry: &Path, out: &Path) -> ExitCode {
    let applied = match load_applied(entry, max_release_choices(out)) {
        Ok(mut applied) => {
            applied.shared = false;
            applied.c23 = true;
            applied.std = "c23".into();
            applied.out = Some(out.to_path_buf());
            applied
        }
        Err(error) => return command_error(error),
    };
    let mut progress = ui::BuildUi::new(ui::NATIVE_STEPS);
    cmd_build(entry, &applied, false, None, &mut progress)
}

fn load_applied(
    file: &Path,
    choices: pkg::CliBuildChoices<'_>,
) -> Result<pkg::AppliedBuild, String> {
    let Some(manifest_path) = pkg::find_manifest(file) else {
        return pkg::apply_cli(choices);
    };
    pkg::resolve(&manifest_path, false)?;
    let manifest = pkg::parse_manifest(&manifest_path)?;
    let dir = manifest_path.parent().unwrap_or(Path::new("."));
    pkg::apply_manifest(&manifest, dir, choices)
}

fn rebind_applied(
    file: &Path,
    applied: &pkg::AppliedBuild,
    target: super_runtime::CompileTarget,
) -> Result<pkg::AppliedBuild, String> {
    let Some(manifest_path) = pkg::find_manifest(file) else {
        let mut one = applied.clone();
        one.target = target.clone();
        one.targets = vec![target];
        if !one.target.allows_native_cpu() && one.profile.target_cpu.as_deref() == Some("native") {
            one.profile.target_cpu = None;
        }
        return Ok(one);
    };
    let manifest = pkg::parse_manifest(&manifest_path)?;
    let dir = manifest_path.parent().unwrap_or(Path::new("."));
    pkg::apply_for_target(applied.clone(), &manifest, dir, target)
}

fn test_scope(path: &Path) -> PathBuf {
    if path == Path::new(".")
        && let Ok(cwd) = std::env::current_dir()
        && let Some(manifest) = pkg::find_manifest(&cwd)
        && let Some(root) = manifest.parent()
    {
        return root.to_path_buf();
    }
    path.to_path_buf()
}

fn analyze_cli_project(file: &Path) -> ProjectAnalyze {
    analyze_cli_project_mode(file, false, false)
}

fn analyze_cli_project_mode(file: &Path, debug_assertions: bool, test: bool) -> ProjectAnalyze {
    let target = super_runtime::CompileTarget::host();
    let features = pkg::find_manifest(file)
        .and_then(|path| pkg::parse_manifest(&path).ok())
        .and_then(|manifest| manifest.resolve_features(&[], false, false).ok())
        .unwrap_or_default();
    analyze_cli_project_for_mode(file, &target, &features, debug_assertions, test)
}

fn analyze_cli_project_for_mode(
    file: &Path,
    target: &super_runtime::CompileTarget,
    features: &std::collections::BTreeSet<String>,
    debug_assertions: bool,
    test: bool,
) -> ProjectAnalyze {
    let cfg = CfgContext {
        target_os: match target.os {
            super_runtime::Os::Macos => "macos",
            _ => target.os.as_str(),
        }
        .into(),
        target_arch: target.arch.as_str().into(),
        target_env: target.env.as_str().into(),
        target_family: if target.os == super_runtime::Os::Windows {
            "windows"
        } else if target.is_wasm() {
            "wasm"
        } else {
            "unix"
        }
        .into(),
        features: features.clone(),
        debug_assertions,
        test,
    };
    let Some(manifest) = pkg::find_manifest(file) else {
        return analyze_project_with_paths_and_cfg(file, &[], &cfg);
    };
    match pkg::resolve(&manifest, false) {
        Ok(paths) => analyze_project_with_paths_and_cfg(file, &paths, &cfg),
        Err(error) => {
            let mut result = analyze_project_with_paths_and_cfg(file, &[], &cfg);
            result.diagnostics.error(
                super_core::DiagnosticKind::Syntax,
                super_core::Span::default(),
                format!("package resolution failed: {error}"),
            );
            result
        }
    }
}

fn print_diags(file: &Path, diags: &super_core::Diagnostics) {
    let text = fs::read_to_string(file).unwrap_or_default();
    let src = SourceFile::new(file.display().to_string(), text);
    for d in diags.iter() {
        eprint!("{}", d.render_colored(&src));
    }
}

fn cmd_disasm(file: &Path, jit: bool) -> ExitCode {
    let mut result = analyze_cli_project(file);
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        return ExitCode::FAILURE;
    }
    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(&mut result.interner, &result.typed, false, &base)
        .with_opt_meta(meta.clone())
        .compile(&result.module);
    let fused_ops = chunk
        .ops
        .iter()
        .filter(|o| {
            matches!(
                o,
                super_runtime::Op::AddImm(_)
                    | super_runtime::Op::SubImm(_)
                    | super_runtime::Op::MulImm(_)
                    | super_runtime::Op::ShlImm(_)
                    | super_runtime::Op::JumpIfLt(_)
                    | super_runtime::Op::JumpIfGe(_)
                    | super_runtime::Op::JumpIfEq(_)
                    | super_runtime::Op::JumpIfNe(_)
                    | super_runtime::Op::LoadSliceUnchecked
                    | super_runtime::Op::LoopStep { .. }
            )
        })
        .count();
    let nop_slack = chunk
        .ops
        .iter()
        .filter(|o| matches!(o, super_runtime::Op::Nop))
        .count();
    let licm_notes = chunk
        .opt_notes
        .iter()
        .filter(|n| matches!(n.kind, super_runtime::OptNoteKind::LicmHoist))
        .count();
    let bce_notes = chunk
        .opt_notes
        .iter()
        .filter(|n| matches!(n.kind, super_runtime::OptNoteKind::BoundsCheckElim))
        .count();
    let unchecked = chunk
        .ops
        .iter()
        .filter(|o| {
            matches!(
                o,
                super_runtime::Op::LoadSliceUnchecked | super_runtime::Op::StoreSliceUnchecked
            )
        })
        .count();
    println!(
        "; opt stats: folds={} dead_branches={} sroa={} licm={} bce={} unchecked={} | bytecode: ops={} fused_superops={} nops={} notes={}",
        meta.stats.folds,
        meta.stats.dead_branches,
        meta.stats.sroa_locals,
        licm_notes,
        bce_notes,
        unchecked,
        chunk.ops.len(),
        fused_ops,
        nop_slack,
        chunk.opt_notes.len()
    );
    print!(
        "{}",
        disassemble(&chunk, &result.interner, Some(&result.typed))
    );
    if jit {
        let mut engine = JitEngine::new(1 << 16);
        let _ = engine.compile_and_run(chunk, &result.interner);
        if let Some(bytes) = engine.last_native() {
            println!("{}", disassemble_native(bytes));
        } else {
            println!("; --jit: no native kernel emitted (fell back to interpreter)");
        }
    }
    ExitCode::SUCCESS
}

fn cmd_run(
    file: &Path,
    jit: bool,
    strip_tests: bool,
    target: &super_runtime::CompileTarget,
    features: &std::collections::BTreeSet<String>,
    debug_assertions: bool,
) -> ExitCode {
    // `--jit`: prefer cached C23 native (host `cc -O3`). The mmap emitter only
    // covers tiny numeric kernels; without this, benchmarks fall through to the
    // bytecode interpreter and look catastrophically slow vs AOT.
    if jit {
        match run_jit_cached_native(file) {
            Ok(code) => return code,
            Err(e) => {
                ui::say_warn(&format!(
                    "jit: native cache path failed ({e}); falling back to interpreter"
                ));
            }
        }
    }
    let mut result = analyze_cli_project_for_mode(file, target, features, debug_assertions, false);
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        return ExitCode::FAILURE;
    }
    let base = result.entry_dir.clone();
    let outcome = if jit {
        let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::with_base(&mut result.interner, &result.typed, strip_tests, &base)
            .with_opt_meta(meta)
            .compile(&result.module);
        let mut engine = JitEngine::new(1 << 20);
        engine.compile_and_run_with_host(
            chunk,
            &result.interner,
            Some(Arc::new(default_host_env())),
        )
    } else {
        run_module_with_host(
            &result.module,
            &result.typed,
            &mut result.interner,
            strip_tests,
            &base,
            Some(Arc::new(default_host_env())),
        )
    };
    match outcome {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            ui::say_err(&format!("error: runtime: {e:?}"));
            ExitCode::FAILURE
        }
    }
}

/// Compile via C23 AOT into `~/.cache/super/jit/<hash>`, then exec.
///
/// Cold builds use a **cycle-fast** profile (O3, no fat LTO, dynamic CRT) so
/// edit→verify does not wait on release-link; warm hits match AOT run speed.
/// Protocol 2.0.0 treats Super JIT compile as free (like an interpreter) —
/// populate the cache before timing when measuring Cycle.
fn run_jit_cached_native(file: &Path) -> Result<ExitCode, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::process::Command;
    use super_runtime::c_emit::{CCompileOptions, CodegenOpt, LtoMode};

    let src = fs::read(file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let mut hasher = DefaultHasher::new();
    src.hash(&mut hasher);
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    // v4: cycle-fast flags (no fat LTO / no static CRT on the JIT cache path).
    "c23-jit-v4".hash(&mut hasher);
    // Invalidate when the `super` binary itself changes (new peeps / emit).
    if let Ok(exe) = std::env::current_exe()
        && let Ok(meta) = fs::metadata(&exe)
    {
        meta.len().hash(&mut hasher);
        if let Ok(modified) = meta.modified() {
            modified.hash(&mut hasher);
        }
    }
    if let Ok(meta) = fs::metadata(file)
        && let Ok(modified) = meta.modified()
    {
        modified.hash(&mut hasher);
    }
    let key = format!("{:016x}", hasher.finish());
    let cache_root = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("super")
        .join("jit");
    fs::create_dir_all(&cache_root).map_err(|e| format!("cache dir: {e}"))?;
    let bin = cache_root.join(&key);
    let c_path = cache_root.join(format!("{key}.c"));
    if !bin.exists() {
        let mut result = analyze_cli_project(file);
        if result.diagnostics.has_errors() {
            print_diags(file, &result.diagnostics);
            return Ok(ExitCode::FAILURE);
        }
        let base = result.entry_dir.clone();
        let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::with_base(&mut result.interner, &result.typed, true, &base)
            .with_opt_meta(meta)
            .compile(&result.module);
        let stem = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("super_jit");
        let source = emit_c_module(
            &chunk,
            &result.interner,
            stem,
            EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
        );
        // Fast edit→verify: same opt level as release runs, but skip fat LTO /
        // static CRT so cold `cc` finishes in hundreds of ms, not multi-second.
        let compile_opts = CCompileOptions {
            cc: None,
            out_bin: Some(bin.as_path()),
            opt: CodegenOpt::O3,
            shared: false,
            std: "c23",
            lto: LtoMode::Off,
            strip: true,
            debug_info: false,
            static_link: false,
            target_cpu: Some("native"),
            fast_math: true,
            gc_sections: true,
            max_speed: true,
            link_args: &[],
            target: None,
            needs_openmp: false,
        };
        write_and_maybe_compile_with(&source, &c_path, true, &compile_opts)
            .map_err(|e| format!("cc: {e}"))?;
        let _ = fs::remove_file(&c_path);
    }
    let status = Command::new(&bin)
        .status()
        .map_err(|e| format!("exec {}: {e}", bin.display()))?;
    if status.success() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(status.code().unwrap_or(1) as u8))
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_c_pipeline(
    file: &Path,
    profile: &ResolvedProfile,
    out_c: Option<&Path>,
    compile_c: bool,
    cc: Option<&str>,
    out_bin: Option<&Path>,
    shared: bool,
    c_std: &str,
    outline_stmt_threshold: usize,
    target: &super_runtime::CompileTarget,
    features: &std::collections::BTreeSet<String>,
) -> ExitCode {
    let mut result = analyze_cli_project_for_mode(
        file,
        target,
        features,
        matches!(
            profile.profile,
            super_runtime::BuildProfile::Dev | super_runtime::BuildProfile::Test
        ),
        false,
    );
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        return ExitCode::FAILURE;
    }
    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(
        &mut result.interner,
        &result.typed,
        profile.strip_tests,
        &base,
    )
    .with_opt_meta(meta)
    .compile(&result.module);

    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("super_module");
    // Prefer writing `.c` beside `--out` binary so `super build --c` never
    // clobbers a hand-written peer `algo.c` sitting next to `algo.sr`.
    let package_bin = out_bin
        .map(Path::to_path_buf)
        .or_else(|| pkg::default_artifact(file, profile.profile.name(), true, profile.strip_tests));
    let c_path = out_c.map(Path::to_path_buf).unwrap_or_else(|| {
        if let Some(bin) = package_bin.as_ref() {
            bin.with_extension("c")
        } else if let Some(bin) = out_bin {
            bin.with_extension("c")
        } else {
            file.with_extension("c")
        }
    });
    if let Some(parent) = c_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let source = emit_c_module(
        &chunk,
        &result.interner,
        stem,
        EmitCOptions {
            typed: Some(&result.typed),
            module: Some(&result.module),
            omit_main: shared,
            release: profile.strip_tests,
            outline_stmt_threshold,
        },
    );
    let mut compile_opts =
        profile.compile_options(cc, package_bin.as_deref().or(out_bin), c_std, shared);
    compile_opts.target = Some(target);
    match write_and_maybe_compile_with(&source, &c_path, compile_c, &compile_opts) {
        Ok(bin) => {
            if let Some(bin) = bin {
                let kind = if shared { "shared lib" } else { "binary" };
                ui::say_ok(&format!(
                    "Finished  emitted {} ({} AOT, {} bytes, profile {}) → compiled {kind} {} [{} {}]",
                    c_path.display(),
                    c_std.to_ascii_uppercase(),
                    source.len(),
                    profile.profile.name(),
                    bin.display(),
                    cc.unwrap_or("cc"),
                    profile.summary()
                ));
            } else {
                ui::say_ok(&format!(
                    "Finished  emitted {} ({} AOT, {} bytes) — pass --cc to compile",
                    c_path.display(),
                    c_std.to_ascii_uppercase(),
                    source.len()
                ));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            ui::say_err(&format!("error: c emit: {e}"));
            ExitCode::FAILURE
        }
    }
}

fn cmd_check(file: &Path, ui: &mut ui::BuildUi) -> ExitCode {
    ui.begin("dependencies");
    if let Err(error) = ensure_resolved(file) {
        ui.fail(&error.to_string());
        return ExitCode::FAILURE;
    }
    ui.next(short_source(file));
    // LAW-BENCH-001 / E0504: benchmarks/**/*.sr must never import or call `host`.
    if path_is_benchmark_source(file)
        && let Ok(text) = fs::read_to_string(file)
        && let Err(v) = check_benchmark_file(file, &text)
    {
        ui::say_err(&format!("error[{}]: {}", v.code, v.diagnostic_message()));
        ui.fail("check failed (LAW-BENCH-001)");
        return ExitCode::FAILURE;
    }
    let mut result = analyze_cli_project_mode(file, true, false);
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        ui.fail("check failed");
        return ExitCode::FAILURE;
    }
    let warnings = warning_count(&result.diagnostics);
    ui.next(if warnings == 0 {
        "ok".to_string()
    } else {
        format!("{warnings} warning(s)")
    });

    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(&mut result.interner, &result.typed, false, &base)
        .with_opt_meta(meta)
        .compile(&result.module);
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("super_module");
    let source = emit_c_module(
        &chunk,
        &result.interner,
        stem,
        EmitCOptions {
            typed: Some(&result.typed),
            module: Some(&result.module),
            omit_main: false,
            release: false,
            ..Default::default()
        },
    );
    ui.next(format!("C23, {} bytes", source.len()));

    let c_path = check_c_path(file);
    if let Some(parent) = c_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        ui.fail(&format!("mkdir {}: {error}", parent.display()));
        return ExitCode::FAILURE;
    }
    ui.next("cc -fsyntax-only -std=c23 -fopenmp");
    ui.external_follows();
    match syntax_check_c(&source, &c_path, None, "c23") {
        Ok(()) => conclude(
            ui,
            file,
            &result.diagnostics,
            warnings,
            &format!("checked {}", file.display()),
        ),
        Err(error) => {
            ui.fail(&error);
            ExitCode::FAILURE
        }
    }
}

fn check_c_path(file: &Path) -> PathBuf {
    pkg::default_artifact(file, "check", true, false)
        .map(|bin| bin.with_extension("c"))
        .unwrap_or_else(|| {
            let mut name = file.file_stem().unwrap_or_default().to_os_string();
            name.push(".check.c");
            file.with_file_name(name)
        })
}

#[allow(clippy::too_many_arguments)]
fn cmd_emit_c(
    file: &Path,
    profile: &ResolvedProfile,
    out_c: Option<&Path>,
    compile_c: bool,
    cc: Option<&str>,
    shared: bool,
    c_std: &str,
    outline_stmt_threshold: usize,
    target: &super_runtime::CompileTarget,
    features: &std::collections::BTreeSet<String>,
) -> ExitCode {
    emit_c_pipeline(
        file,
        profile,
        out_c,
        compile_c,
        cc,
        None,
        shared,
        c_std,
        outline_stmt_threshold,
        target,
        features,
    )
}

fn cmd_build(
    file: &Path,
    applied: &pkg::AppliedBuild,
    emit_only: bool,
    out_c: Option<&Path>,
    ui: &mut ui::BuildUi,
) -> ExitCode {
    let profile = &applied.profile;
    // A7: unused locals/params/imports are errors under release profiles.
    if profile.strip_tests {
        // SAFETY: single-threaded CLI sets this before analysis; no concurrent env readers.
        unsafe { std::env::set_var("SUPER_DENY_UNUSED", "1") };
    }
    ui.begin("dependencies");
    ui.next(short_source(file));
    let mut result = analyze_cli_project_for_mode(
        file,
        &applied.target,
        &applied.features,
        matches!(
            profile.profile,
            super_runtime::BuildProfile::Dev | super_runtime::BuildProfile::Test
        ),
        false,
    );
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        ui.fail("check failed");
        return ExitCode::FAILURE;
    }
    let warnings = warning_count(&result.diagnostics);
    ui.next(if warnings == 0 {
        "ok".to_string()
    } else {
        format!("{warnings} warning(s)")
    });
    ui.next(profile.summary());

    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(
        &mut result.interner,
        &result.typed,
        profile.strip_tests,
        &base,
    )
    .with_opt_meta(meta)
    .compile(&result.module);

    if !applied.mode.native() && !emit_only {
        return write_bytecode(file, applied, &chunk, &result, warnings, ui);
    }

    let native_out = applied.out.clone().or_else(|| {
        pkg::default_artifact_for(
            file,
            profile.profile.name(),
            true,
            profile.strip_tests,
            &applied.target,
        )
    });
    let c_path = out_c
        .map(Path::to_path_buf)
        .unwrap_or_else(|| match &native_out {
            Some(bin) => bin.with_extension("c"),
            None => file.with_extension("c"),
        });
    if let Some(parent) = c_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        ui.fail(&format!("mkdir {}: {error}", parent.display()));
        return ExitCode::FAILURE;
    }

    let via = match applied.mode {
        pkg::CompileMode::Jit => "C23 AOT (jit image)",
        _ => "C23 AOT",
    };
    ui.next(format!("{via} ({})", applied.target.triple));
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("super_module");
    let emit_opts = EmitCOptions {
        typed: Some(&result.typed),
        module: Some(&result.module),
        omit_main: applied.shared,
        release: profile.strip_tests,
        outline_stmt_threshold: applied.outline_stmt_threshold,
    };

    let codegen_dir = native_out
        .as_ref()
        .and_then(|bin| bin.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| {
            pkg::find_manifest(file)
                .and_then(|m| m.parent().map(|p| p.to_path_buf()))
                .map(|root| applied.target.artifact_dir(&root, profile.profile.name()))
                .unwrap_or_else(|| PathBuf::from("target"))
        });

    if emit_only {
        ui.set_detail(format!("{via}, emit-only"));
        if applied.codegen_units == pkg::CodegenUnits::Single {
            let source = emit_c_module(&chunk, &result.interner, stem, emit_opts);
            if let Err(error) = fs::write(&c_path, source.as_bytes()) {
                ui.fail(&format!("write {}: {error}", c_path.display()));
                return ExitCode::FAILURE;
            }
            return conclude(
                ui,
                file,
                &result.diagnostics,
                warnings,
                &format!(
                    "profile {} target {} [{} {}] → {}",
                    profile.profile.name(),
                    applied.target.triple,
                    applied.mode.name(),
                    applied.backend.name(),
                    c_path.display()
                ),
            );
        }
        let incremental = IncrementalEmitOptions {
            emit: emit_opts,
            module_name: stem,
            out_dir: &codegen_dir,
            keep_c: true,
        };
        if let Err(error) = emit_incremental_sources(&chunk, &result.interner, &incremental) {
            ui.fail(&error);
            return ExitCode::FAILURE;
        }
        return conclude(
            ui,
            file,
            &result.diagnostics,
            warnings,
            &format!(
                "profile {} codegen {} → {}",
                profile.profile.name(),
                applied.codegen_units.name(),
                codegen_dir.display()
            ),
        );
    }

    let cc_label = applied
        .cc
        .as_deref()
        .unwrap_or(if applied.target.is_cross() {
            "zig|clang"
        } else {
            "cc"
        });
    ui.next(format!(
        "{cc_label} {} [{}] codegen={}",
        profile.summary(),
        applied.target.triple,
        applied.codegen_units.name()
    ));
    ui.external_follows();
    let compile_opts = profile.compile_options_for(
        applied.cc.as_deref(),
        native_out.as_deref(),
        &applied.std,
        applied.shared,
        Some(&applied.target),
    );

    let build_result = if applied.codegen_units == pkg::CodegenUnits::Single {
        let source = emit_c_module(&chunk, &result.interner, stem, emit_opts);
        ui.set_detail(format!("{via}, {} bytes", source.len()));
        write_and_maybe_compile_with(&source, &c_path, true, &compile_opts)
    } else {
        ui.set_detail(format!(
            "{via}, incremental TU emission → {}",
            codegen_dir.display()
        ));
        let incremental = IncrementalEmitOptions {
            emit: emit_opts,
            module_name: stem,
            out_dir: &codegen_dir,
            keep_c: applied.keep_c,
        };
        write_and_compile_incremental(&chunk, &result.interner, &incremental, &compile_opts)
            .map(Some)
    };

    match build_result {
        Ok(Some(bin)) => {
            retire_bytecode_stub(&bin);
            conclude(
                ui,
                file,
                &result.diagnostics,
                warnings,
                &format!(
                    "profile {} target {} [{} {}] → {}",
                    profile.profile.name(),
                    applied.target.triple,
                    applied.mode.name(),
                    applied.backend.name(),
                    bin.display()
                ),
            )
        }
        Ok(None) => {
            ui.fail("the C compiler did not produce a binary");
            ExitCode::FAILURE
        }
        Err(error) => {
            ui.fail(&error);
            ExitCode::FAILURE
        }
    }
}

fn write_bytecode(
    file: &Path,
    applied: &pkg::AppliedBuild,
    chunk: &super_runtime::Chunk,
    result: &super_frontend::ProjectAnalyze,
    warnings: usize,
    ui: &mut ui::BuildUi,
) -> ExitCode {
    let profile = &applied.profile;
    let const_bytes: usize = chunk
        .consts
        .iter()
        .map(|value| match value {
            super_runtime::Value::Str(text) => text.len(),
            super_runtime::Value::Bytes(bytes) => bytes.len(),
            _ => 8,
        })
        .sum();
    let artifact_bytes = chunk.ops.len() * 8 + const_bytes + chunk.functions.len() * 16;
    ui.next(format!(
        "{} ops, {artifact_bytes} bytecode units",
        chunk.ops.len()
    ));
    let out = applied.out.clone().unwrap_or_else(|| {
        pkg::default_artifact(file, profile.profile.name(), false, profile.strip_tests)
            .unwrap_or_else(|| {
                file.with_extension(if profile.strip_tests {
                    "superb"
                } else {
                    "superb-dev"
                })
            })
    });
    if let Some(parent) = out.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        ui.fail(&format!("mkdir {}: {error}", parent.display()));
        return ExitCode::FAILURE;
    }
    let header = format!(
        "superb1 profile={} opt={} tests={} ops={} consts={} funcs={} bytes≈{}\n",
        profile.profile.name(),
        profile.opt.flag(),
        if profile.strip_tests {
            "stripped"
        } else {
            "kept"
        },
        chunk.ops.len(),
        chunk.consts.len(),
        chunk.functions.len(),
        artifact_bytes
    );
    if let Err(error) = fs::write(&out, header.as_bytes()) {
        ui.fail(&format!("write {}: {error}", out.display()));
        return ExitCode::FAILURE;
    }
    conclude(
        ui,
        file,
        &result.diagnostics,
        warnings,
        &format!(
            "profile {} [bytecode {}] → {}",
            profile.profile.name(),
            applied.backend.name(),
            out.display()
        ),
    )
}

fn conclude(
    ui: &mut ui::BuildUi,
    file: &Path,
    diags: &super_core::Diagnostics,
    warnings: usize,
    detail: &str,
) -> ExitCode {
    ui.seal();
    if warnings > 0 {
        print_diags(file, diags);
    }
    ui.status(warnings, detail);
    ExitCode::SUCCESS
}

fn warning_count(diags: &super_core::Diagnostics) -> usize {
    diags
        .iter()
        .filter(|diag| diag.severity == super_core::Severity::Warning)
        .count()
}

fn short_source(file: &Path) -> String {
    match (
        file.parent().and_then(|parent| parent.file_name()),
        file.file_name(),
    ) {
        (Some(dir), Some(name)) => {
            format!("{}/{}", dir.to_string_lossy(), name.to_string_lossy())
        }
        (_, Some(name)) => name.to_string_lossy().into_owned(),
        _ => file.display().to_string(),
    }
}

fn retire_bytecode_stub(bin: &Path) {
    let Some(dir) = bin.parent() else {
        return;
    };
    let Some(name) = bin.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let _ = fs::remove_file(dir.join(format!("{name}.superb")));
    let _ = fs::remove_file(dir.join(format!("{name}.superb-dev")));
}

fn cmd_verify(
    file: &Path,
    locked_laws: bool,
    solver: Option<&str>,
    timeout: Option<u64>,
    json: bool,
) -> ExitCode {
    // LAW-BENCH-001 / E0504 structural discharge before behavioral #law tiers.
    if path_is_benchmark_source(file)
        && let Ok(text) = fs::read_to_string(file)
        && let Err(v) = check_benchmark_file(file, &text)
    {
        ui::say_err(&format!("error[{}]: {}", v.code, v.diagnostic_message()));
        return ExitCode::FAILURE;
    }
    let mut result = analyze_cli_project(file);
    if result.diagnostics.has_errors() {
        print_diags(file, &result.diagnostics);
        return ExitCode::FAILURE;
    }

    let pod_root = pkg::find_manifest(file).and_then(|m| m.parent().map(|p| p.to_path_buf()));

    let mut law_lock = std::collections::BTreeMap::new();
    let lock_path = pod_root.as_ref().map(|r| r.join("laws.lock"));
    if let Some(path) = &lock_path
        && let Ok(text) = fs::read_to_string(path)
    {
        law_lock = parse_laws_lock(&text);
    }

    let mut solver_kind = SolverKind::Auto;
    if let Some(raw) = solver {
        match SolverKind::parse(raw) {
            Ok(s) => solver_kind = s,
            Err(e) => {
                ui::say_err(&format!("error: {e}"));
                return ExitCode::FAILURE;
            }
        }
    } else if let Some(manifest_path) = pkg::find_manifest(file)
        && let Ok(manifest) = pkg::parse_manifest(&manifest_path)
        && let Some(raw) = manifest.verify.solver.as_deref()
    {
        match SolverKind::parse(raw) {
            Ok(s) => solver_kind = s,
            Err(e) => {
                ui::say_err(&format!("error: Pod.toml [verify] solver: {e}"));
                return ExitCode::FAILURE;
            }
        }
    }

    // LAW-BENCH-001: structural discharge over benchmarks/ when present in the
    // repo (or when the entry itself lives under benchmarks/).
    let bench_root = find_benchmarks_root(Some(file))
        .or_else(|| {
            pod_root
                .as_deref()
                .and_then(|r| find_benchmarks_root(Some(r)))
        })
        .or_else(|| find_benchmarks_root(None));

    let opts = VerifyOptions {
        locked_laws,
        solver: solver_kind,
        timeout_ms: timeout.unwrap_or(super_frontend::DEFAULT_VERIFY_TIMEOUT_MS),
        pod_root: pod_root.clone(),
        law_lock,
        bench_root: bench_root.clone(),
    };

    let mut report = verify_module(
        &result.module,
        &result.interner,
        &mut result.diagnostics,
        &opts,
    );

    // If the module had zero `#law` items but we still have a benchmarks tree,
    // ensure LAW-BENCH-001 ran (verify_module already does when bench_root is set;
    // this branch covers a dedicated `super verify --bench-purity` style invoke
    // where analyze might be of a non-law file).
    if bench_root.is_some()
        && !report
            .results
            .iter()
            .any(|r| r.name == super_frontend::LAW_BENCH_001_NAME)
        && let Some(root) = &bench_root
    {
        let (_proof, result_law) = prove_law_bench_001(root);
        report.laws_seen += 1;
        report.proofs_seen += 1;
        if !result_law.passed {
            ui::say_err(&format!(
                "FAIL {} — {}",
                result_law.name, result_law.message
            ));
        }
        report.results.push(result_law);
    }

    if !locked_laws
        && let Some(path) = &lock_path
        && !report.new_lock.is_empty()
    {
        let _ = fs::write(path, format_laws_lock(&report.new_lock));
    }

    if json {
        println!("{}", report.to_json());
    } else {
        for r in &report.results {
            if r.passed {
                let tier = r.tier.map(|t| t.name()).unwrap_or("—");
                ui::say_ok(&format!("ok  {} ({}) — {}", r.name, tier, r.description));
            } else {
                ui::say_err(&format!("FAIL {} — {}", r.name, r.message));
            }
        }
        if report.laws_seen == 0 {
            ui::say_ok("super verify: no #law items found");
        } else {
            let summary = format!(
                "super verify: {} passed, {} failed ({} laws, {} proofs, solver {})",
                report.laws_seen - report.failed_count(),
                report.failed_count(),
                report.laws_seen,
                report.proofs_seen,
                solver_kind.name()
            );
            if report.passed() {
                ui::say_ok(&summary);
            } else {
                ui::say_err(&summary);
            }
        }
        if result.diagnostics.has_errors() {
            print_diags(file, &result.diagnostics);
        }
    }

    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// When `[verify] require-on-release = true`, run verification before a release build.
fn maybe_require_verify(file: &Path, release: bool) -> ExitCode {
    if !release {
        return ExitCode::SUCCESS;
    }
    let Some(manifest_path) = pkg::find_manifest(file) else {
        return ExitCode::SUCCESS;
    };
    let Ok(manifest) = pkg::parse_manifest(&manifest_path) else {
        return ExitCode::SUCCESS;
    };
    if !manifest.verify.require_on_release {
        return ExitCode::SUCCESS;
    }
    ui::say_ok("verify: [verify] require-on-release — running `super verify --locked-laws`");
    cmd_verify(file, true, manifest.verify.solver.as_deref(), None, false)
}

fn cmd_pod(command: PodCmd) -> ExitCode {
    match command {
        PodCmd::Pack {
            out,
            skip_gate,
            path,
        } => cmd_pod_pack(&path, out.as_deref(), skip_gate),
        PodCmd::Extract { file, dir } => match pod_archive::extract_archive(&file, &dir) {
            Ok(info) => {
                ui::say_ok(&format!(
                    "extracted {} {} (format {}) → {}",
                    info.name,
                    info.version,
                    info.format,
                    dir.display()
                ));
                ExitCode::SUCCESS
            }
            Err(e) => command_error(e),
        },
        PodCmd::Verify { file, hash } => cmd_pod_verify(&file, hash.as_deref()),
    }
}

fn cmd_pod_pack(path: &Path, out: Option<&Path>, skip_gate: bool) -> ExitCode {
    let Some(manifest_path) = pkg::find_manifest(path) else {
        return command_error(format!(
            "could not find {} from {}",
            pkg::MANIFEST_POD,
            path.display()
        ));
    };
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    let manifest = match pkg::parse_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => return command_error(e),
    };
    let id = match manifest.identity() {
        Ok(id) => id,
        Err(e) => return command_error(e),
    };
    let default_out = root
        .join("target")
        .join(format!("{}-{}.pod", id.name, id.version));
    let out_path = out.map(Path::to_path_buf).unwrap_or(default_out);

    let packed = match pod_archive::pack_directory(root, &out_path) {
        Ok(p) => p,
        Err(e) => return command_error(e),
    };
    if let Err(e) = pod_archive::write_lock_content_hash(&manifest_path, &packed.content_hash) {
        return command_error(e);
    }
    ui::say_ok(&format!(
        "packed {} ({} bytes, {})",
        packed.archive.display(),
        packed.bytes,
        packed.content_hash
    ));

    if skip_gate {
        return ExitCode::SUCCESS;
    }

    // D4 gate: extract → compile clean → test VM + JIT + C23.
    let tmp = match tempfile_dir_for_pack() {
        Ok(t) => t,
        Err(e) => return command_error(e),
    };
    let extract_root = tmp.join("extract");
    if let Err(e) = pod_archive::extract_archive(&packed.archive, &extract_root) {
        return command_error(format!("gate extract failed: {e}"));
    }
    if let Err(e) = pod_archive::verify_hash(&packed.archive, &packed.content_hash) {
        return command_error(format!("gate hash failed: {e}"));
    }
    let entry = match resolve_pod_entry(&extract_root) {
        Ok(e) => e,
        Err(e) => return command_error(e),
    };
    ui::say_ok(&format!("gate: testing {}", entry.display()));

    // VM backend (interpreter tests)
    match run_tests_on_entry(&entry) {
        Ok(n) => ui::say_ok(&format!("gate VM: {n} assertion(s) passed")),
        Err(e) => return command_error(format!("gate VM failed: {e}")),
    }

    // C23 backend: emit + syntax-check
    match gate_c23(&entry) {
        Ok(()) => ui::say_ok("gate C23: emit + syntax-check ok"),
        Err(e) => return command_error(format!("gate C23 failed: {e}")),
    }

    // JIT backend: attempt native/JIT image build (best-effort compile path)
    match gate_jit(&entry) {
        Ok(()) => ui::say_ok("gate JIT: compile path ok"),
        Err(e) => return command_error(format!("gate JIT failed: {e}")),
    }

    ui::say_ok("pod pack gate: VM + JIT + C23 green");
    ExitCode::SUCCESS
}

fn tempfile_dir_for_pack() -> Result<PathBuf, String> {
    let base = std::env::temp_dir().join(format!(
        "super-pod-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    Ok(base)
}

fn resolve_pod_entry(root: &Path) -> Result<PathBuf, String> {
    let manifest_path = root.join(pkg::MANIFEST_POD);
    pkg::package_entry(&manifest_path)
        .ok_or_else(|| format!("could not resolve Pod entry under {}", root.display()))
}

fn run_tests_on_entry(entry: &Path) -> Result<usize, String> {
    let mut result = analyze_cli_project(entry);
    if result.diagnostics.has_errors() {
        return Err(format!(
            "analyze errors: {}",
            result
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let base = result.entry_dir.clone();
    run_tests_with_host(
        &result.module,
        &result.typed,
        &mut result.interner,
        &base,
        Some(Arc::new(default_host_env())),
    )
    .map_err(|e| format!("{e:?}"))
}

fn gate_c23(entry: &Path) -> Result<(), String> {
    let mut result = analyze_cli_project(entry);
    if result.diagnostics.has_errors() {
        return Err("analyze failed".into());
    }
    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(&mut result.interner, &result.typed, false, &base)
        .with_opt_meta(meta)
        .compile(&result.module);
    let stem = entry
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pod_module");
    let source = emit_c_module(
        &chunk,
        &result.interner,
        stem,
        EmitCOptions {
            typed: Some(&result.typed),
            module: Some(&result.module),
            omit_main: false,
            release: true,
            ..Default::default()
        },
    );
    let out_c = std::env::temp_dir().join(format!("super-pod-gate-{stem}.c"));
    syntax_check_c(&source, &out_c, None, "c23").map_err(|e| format!("cc -fsyntax-only: {e}"))?;
    Ok(())
}

fn gate_jit(entry: &Path) -> Result<(), String> {
    let mut result = analyze_cli_project(entry);
    if result.diagnostics.has_errors() {
        return Err("analyze failed".into());
    }
    let base = result.entry_dir.clone();
    let meta = optimize_module(&mut result.module, &result.typed, &mut result.interner);
    let chunk = Compiler::with_base(&mut result.interner, &result.typed, false, &base)
        .with_opt_meta(meta)
        .compile(&result.module);
    // Compile kernels only — never execute `main`. Pack must exit after the
    // .pod is gated; apps with `listen`/`serve` would hang otherwise.
    let mut jit = JitEngine::new(1 << 20);
    jit.compile_kernels(&chunk);
    let _ = jit.last_native();
    Ok(())
}

fn cmd_pod_verify(file: &Path, hash: Option<&str>) -> ExitCode {
    let bytes = match fs::read(file) {
        Ok(b) => b,
        Err(e) => return command_error(e.to_string()),
    };
    if bytes.len() < 4 || bytes[..4] != pod_archive::ZSTD_MAGIC {
        return command_error(format!(
            "{} is not a zstd .pod (missing magic)",
            file.display()
        ));
    }
    let tmp = match tempfile_dir_for_pack() {
        Ok(t) => t,
        Err(e) => return command_error(e),
    };
    let dest = tmp.join("v");
    match pod_archive::extract_archive(file, &dest) {
        Ok(info) => {
            ui::say_ok(&format!(
                "verify structure: {} {} format={} ok",
                info.name, info.version, info.format
            ));
        }
        Err(e) => return command_error(e),
    }
    let expected = if let Some(h) = hash {
        Some(h.to_owned())
    } else if let Some(manifest) = pkg::find_manifest(Path::new(".")) {
        match pkg::parse_manifest(&manifest) {
            Ok(m) => match m.identity() {
                Ok(id) => pod_archive::lock_content_hash(&manifest, &id.name)
                    .ok()
                    .flatten(),
                Err(_) => None,
            },
            Err(_) => None,
        }
    } else {
        None
    };
    if let Some(h) = expected {
        if let Err(e) = pod_archive::verify_hash(file, &h) {
            return command_error(e);
        }
        ui::say_ok(&format!("verify hash: {h}"));
    } else {
        let got = pod_archive::content_hash(&bytes);
        ui::say_ok(&format!("verify hash (computed): {got}"));
    }
    ExitCode::SUCCESS
}

fn cmd_test(path: &Path) -> ExitCode {
    let path = test_scope(path);
    if let Err(error) = ensure_test_profile(&path) {
        return command_error(error);
    }
    let files = collect_sr_files(&path);
    if files.is_empty() {
        ui::say_err(&format!("error: no .sr files under {}", path.display()));
        return ExitCode::FAILURE;
    }
    let mut failed = 0;
    let mut passed = 0;
    for file in files {
        // Skip stdlib sources when testing a broader tree (e.g. `super test
        // examples/` or `super test .`), but always run them when the user
        // points at `stdlib/` or a specific `.sr` file.
        let path_is_file = path.is_file();
        let in_std = file
            .components()
            .any(|c| c.as_os_str().to_str() == Some("stdlib"));
        let testing_std = path_is_file
            || path.ends_with("stdlib")
            || path
                .components()
                .any(|c| c.as_os_str().to_str() == Some("stdlib"));
        if in_std && !testing_std {
            continue;
        }
        let mut result = analyze_cli_project_mode(&file, false, true);
        if result.diagnostics.has_errors() {
            print_diags(&file, &result.diagnostics);
            failed += 1;
            continue;
        }
        let base = result.entry_dir.clone();
        match run_tests_with_host(
            &result.module,
            &result.typed,
            &mut result.interner,
            &base,
            Some(Arc::new(default_host_env())),
        ) {
            Ok(n) => {
                ui::say_ok(&format!(
                    "Finished  {}: {n} assertion(s) passed",
                    file.display()
                ));
                passed += n;
            }
            Err(e) => {
                ui::say_err(&format!("error: {}: {e:?}", file.display()));
                failed += 1;
            }
        }
    }
    let summary = format!("super test: {passed} passed, {failed} file(s) failed");
    if failed > 0 {
        ui::say_err(&format!("Failed  {summary}"));
        ExitCode::FAILURE
    } else {
        ui::say_ok(&format!("Finished  {summary}"));
        ExitCode::SUCCESS
    }
}

fn ensure_test_profile(path: &Path) -> Result<(), String> {
    if pkg::find_manifest(path).is_none() {
        return Ok(());
    };
    pkg::resolve_workspace(path, false)?;
    let Some(manifest_path) = pkg::find_manifest(path) else {
        return Ok(());
    };
    let manifest = pkg::parse_manifest(&manifest_path)?;
    let choices = pkg::CliBuildChoices {
        profile: Some("test"),
        ..Default::default()
    };
    let dir = manifest_path.parent().unwrap_or(Path::new("."));
    pkg::apply_manifest(&manifest, dir, choices).map(|_| ())
}

/// Canonical Super source extension is `.sr`; `.sup` is accepted as a legacy alias.
fn is_sr_source(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("sr") | Some("sup")
    )
}

/// `*.law.sr` / `*.proof.sr` are verify-only companions — not standalone test units.
fn is_verify_sidecar(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    name.ends_with(".law.sr") || name.ends_with(".proof.sr")
}

fn collect_sr_files(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        if is_sr_source(path) && !is_verify_sidecar(path) {
            out.push(path.to_path_buf());
        }
        return out;
    }
    fn collect_into(dir: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    // Don't walk into systems peer target dirs
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if matches!(name, "target" | ".git" | "node_modules") {
                        continue;
                    }
                    collect_into(&p, out);
                } else if is_sr_source(&p) && !is_verify_sidecar(&p) {
                    out.push(p);
                }
            }
        }
    }
    collect_into(path, &mut out);
    out.sort();
    out
}

fn cmd_bench() -> ExitCode {
    use std::time::Instant;
    use super_host::ecs::World;

    const N: u64 = 100_000_000;
    const ENTITIES: usize = 10_000;
    const DT: f64 = 0.016;

    let mut world = World::new();
    for _ in 0..ENTITIES {
        let e = world.spawn();
        world.insert(e, "Pos", super_runtime::Value::Float(0.0));
        world.insert(e, "Vel", super_runtime::Value::Float(1.0));
    }
    let (pos, vel) = world
        .soa_pair_mut("Pos", "Vel")
        .expect("Pos/Vel SoA columns");
    let lane_len = pos.len();
    let iters = (N as usize / lane_len).max(1);

    let t0 = Instant::now();
    let mut path = "rust-simd";
    let mut jit_ok = false;
    for _ in 0..iters {
        jit_ok = unsafe {
            super_runtime::run_ecs_integrate(pos.as_mut_ptr(), vel.as_ptr(), lane_len, DT)
        } || jit_ok;
        if !jit_ok {
            super_host::ecs::integrate_soa(pos, vel, DT);
        }
    }
    if jit_ok {
        path = "jit-native";
    }
    let ecs_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let acc: f64 = pos.iter().take(8).sum::<f64>() + vel.iter().take(8).sum::<f64>();
    ui::say_ok(&format!(
        "Finished  ECS 100M-iter bench: {ecs_ms:.2} ms ({path}, acc={acc})"
    ));

    let t1 = Instant::now();
    let status_sum = std::hint::black_box(super_host::host_net::bench_http_json_scale(
        (N / 1000) as usize,
    ));
    let http_ms = t1.elapsed().as_secs_f64() * 1000.0;
    ui::say_ok(&format!(
        "Finished  HTTP/JSON 100M-scale bench: {http_ms:.2} ms (status_sum={status_sum})"
    ));

    let src = SourceFile::new("bench.sr", "main() { for 0..1000 { } }");
    let mut r = analyze(&src);
    let chunk = Compiler::new(&mut r.interner, &r.typed, true).compile(&r.module);
    let mut stacks = StackPool::new(1 << 20);
    let mut vm = Vm::new(chunk, &mut stacks, &r.interner);
    let _ = vm.run();

    // M:N channel + for parallel throughput
    let conc = SourceFile::new(
        "conc_bench.sr",
        r#"
+bench() : i64 {
    hits = shared 0
    ch = chan[i64](1024)
    scope {
        w = spawn {
            n = 0
            for x in ch {
                @hits += 1
                n += 1
            }
            return n
        }
        for parallel 0..10000 {
            ch.send(it)
        }
        ch.close()
        got = await w
        return got + hits.load()
    }
}
test "conc" { #assert(bench() > 0) }
"#,
    );
    let t2 = Instant::now();
    let mut cr = analyze(&conc);
    match run_tests_with_host(
        &cr.module,
        &cr.typed,
        &mut cr.interner,
        ".",
        Some(Arc::new(default_host_env())),
    ) {
        Ok(n) => {
            let ms = t2.elapsed().as_secs_f64() * 1000.0;
            ui::say_ok(&format!(
                "Finished  M:N channel + for parallel bench: {ms:.2} ms ({n} asserts)"
            ));
        }
        Err(e) => {
            ui::say_err(&format!("error: concurrency bench failed: {e:?}"));
            return ExitCode::FAILURE;
        }
    }

    // --- # metaprogramming before/after: loop unroll + comptime LUT ---
    let runtime_loop = SourceFile::new(
        "meta_runtime.sr",
        r#"
+sum_runtime() : i64 {
    s = 0
    for 0..64 {
        s += it
    }
    return s
}
test "rt" { assert_eq(sum_runtime(), 2016) }
"#,
    );
    let unrolled = SourceFile::new(
        "meta_unroll.sr",
        r#"
#TABLE = #for i in 0..64 { i }
+sum_unrolled() : i64 {
    s = 0
    //# unroll(64)
    for 0..64 {
        s += it
    }
    return s
}
+table_len() : i64 {
    n = 0
    for #TABLE {
        n += 1
    }
    return n
}
test "unroll" { assert_eq(sum_unrolled(), 2016) }
test "lut" { assert_eq(table_len(), 64) }
"#,
    );
    let t3 = Instant::now();
    let mut rr = analyze(&runtime_loop);
    let _ = run_tests_with_host(
        &rr.module,
        &rr.typed,
        &mut rr.interner,
        ".",
        Some(Arc::new(default_host_env())),
    );
    let runtime_ms = t3.elapsed().as_secs_f64() * 1000.0;

    let t4 = Instant::now();
    let mut ur = analyze(&unrolled);
    match run_tests_with_host(
        &ur.module,
        &ur.typed,
        &mut ur.interner,
        ".",
        Some(Arc::new(default_host_env())),
    ) {
        Ok(n) => {
            let unroll_ms = t4.elapsed().as_secs_f64() * 1000.0;
            ui::say_ok(&format!(
                "Finished  metaprogramming bench: runtime-for={runtime_ms:.3} ms, unroll+LUT={unroll_ms:.3} ms ({n} asserts; expansions={})",
                ur.meta_cache.expansions.len() + ur.meta_cache.bindings.len()
            ));
            if unroll_ms <= runtime_ms * 3.0 + 50.0 {
                ui::say_ok(
                    "Finished  metaprogramming: unroll/LUT path within expected envelope vs runtime for",
                );
            }
        }
        Err(e) => {
            ui::say_err(&format!("error: metaprogramming bench failed: {e:?}"));
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod cli_tests {
    use super::{Cli, Commands, NativeOpts, pkg};
    use clap::Parser;
    use super_runtime::BuildProfile;

    fn native(argv: &[&str]) -> NativeOpts {
        let cli = Cli::try_parse_from(argv).unwrap_or_else(|err| panic!("{err}"));
        match cli.command {
            Commands::Build { native, .. } => native,
            other => panic!("expected build, got {other:?}"),
        }
    }

    fn resolved(argv: &[&str]) -> super_runtime::ResolvedProfile {
        let opts = native(argv);
        pkg::apply_cli(opts.choices(None, None, None, false))
            .unwrap_or_else(|err| panic!("{err}"))
            .profile
    }

    #[test]
    fn short_run_and_build_aliases() {
        let run = Cli::try_parse_from(["super", "r", "main.sr"]).expect("super r");
        assert!(matches!(run.command, Commands::Run { jit: false, .. }));
        let built = native(&["super", "b", "main.sr"]);
        assert!(!built.release && !built.debug);
        assert_eq!(
            resolved(&["super", "b", "main.sr"]).profile,
            BuildProfile::Dev
        );
        let bare = Cli::try_parse_from(["super", "b"]).expect("super b");
        assert!(matches!(bare.command, Commands::Build { file: None, .. }));
        let bare_run = Cli::try_parse_from(["super", "r"]).expect("super r");
        assert!(matches!(bare_run.command, Commands::Run { file: None, .. }));
    }

    #[test]
    fn lsp_and_mcp_parse() {
        assert!(matches!(
            Cli::try_parse_from(["super", "lsp"]).unwrap().command,
            Commands::Lsp
        ));
        assert!(matches!(
            Cli::try_parse_from(["super", "mcp"]).unwrap().command,
            Commands::Mcp
        ));
    }

    #[test]
    fn install_clean_and_which_parse() {
        let install = Cli::try_parse_from(["super", "i", "--force", "--bin-dir", "bin"]).unwrap();
        assert!(matches!(
            install.command,
            Commands::Install {
                force: true,
                bin_dir: Some(_),
                ..
            }
        ));
        let uninstall = Cli::try_parse_from(["super", "u", "hello"]).unwrap();
        assert!(matches!(
            uninstall.command,
            Commands::Uninstall { name: Some(_) }
        ));
        assert!(Cli::try_parse_from(["super", "list"]).is_ok());
        assert!(Cli::try_parse_from(["super", "installed"]).is_ok());
        assert!(Cli::try_parse_from(["super", "which"]).is_ok());
        let debug = Cli::try_parse_from(["super", "clean", "-d"]).unwrap();
        assert!(matches!(
            debug.command,
            Commands::Clean {
                debug: true,
                release: false,
                all: false,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["super", "clean", "-r"]).is_ok());
        assert!(Cli::try_parse_from(["super", "clean", "-a"]).is_ok());
        assert!(Cli::try_parse_from(["super", "clean"]).is_ok());
        assert!(Cli::try_parse_from(["super", "clean", "-d", "-r"]).is_err());
        assert!(Cli::try_parse_from(["super", "clean", "-d", "-a"]).is_err());
    }

    #[test]
    fn release_and_debug_short_flags() {
        let release = native(&["super", "b", "-r", "--c23", "main.sr"]);
        assert!(release.release);
        assert_eq!(
            resolved(&["super", "b", "-r", "--c23", "main.sr"]).profile,
            BuildProfile::Release
        );
        let debug = resolved(&["super", "b", "-d", "main.sr"]);
        assert_eq!(debug.profile, BuildProfile::Dev);
        assert!(debug.summary().contains("-O0"));
        let mixed = native(&["super", "b", "-r", "-d", "main.sr"]);
        assert!(pkg::apply_cli(mixed.choices(None, None, None, false)).is_err());
    }
}
