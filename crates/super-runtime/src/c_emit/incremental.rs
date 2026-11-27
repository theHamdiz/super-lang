//! Per-translation-unit C emission: shared `super_rt` + function TUs + incremental `-c` / link.

use super::unboxed;
use super::{
    CCompileOptions, EmitCOptions, RUNTIME_PRELUDE, apply_cc_diagnostic_colors,
    emit_boxed_module_main, emit_c_abi_types, emit_font8, emit_function, function_c_name,
    sanitize_ident,
};
use crate::bytecode::{Chunk, Op};
use crate::value::Value;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use super_core::Interner;

/// Options for incremental C emission under `target/<profile>/`.
#[derive(Clone, Debug)]
pub struct IncrementalEmitOptions<'a> {
    pub emit: EmitCOptions<'a>,
    pub module_name: &'a str,
    pub out_dir: &'a Path,
    pub keep_c: bool,
}

struct TranslationUnit {
    stem: String,
    source: String,
}

fn compile_fingerprint(source: &str, compiler: &str, args: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    compiler.hash(&mut hasher);
    args.hash(&mut hasher);
    hasher.finish()
}

fn tu_includes(guard: &str) -> String {
    format!(
        "#include \"super_rt.h\"\n#include \"super_mod.h\"\n\
#ifndef SUPER_TU_{guard}\n#define SUPER_TU_{guard}\n\n"
    )
}

fn tu_footer(guard: &str) -> String {
    format!("\n#endif /* SUPER_TU_{guard} */\n")
}

fn strip_static_fn(s: &str) -> String {
    // Drop `static` for multi-TU linkage. Keep `always_inline` as a plain
    // attribute (no C99 `inline`) so GCC still emits a linkable definition.
    s.replace(
        "static inline __attribute__((always_inline)) ",
        "__attribute__((always_inline)) ",
    )
    .replace("static inline ", "")
    .replace("static ", "")
}

fn export_prototypes_from_decls(decls: &str, dyn_fns: &[(String, bool)]) -> String {
    let mut out = String::new();
    for line in decls.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("static ") && trimmed.contains('(') && trimmed.ends_with(';') {
            let exported = strip_static_fn(trimmed);
            let _ = writeln!(out, "{exported}");
        }
    }
    for (fname, _always) in dyn_fns {
        let _ = writeln!(out, "SuperVal {fname}(SuperVal *restrict args, int nargs);");
    }
    let _ = writeln!(out, "SuperVal super_module_main(void);");
    out
}

pub(super) fn append_c_standard_includes(out: &mut String) {
    writeln!(out, "#define _DEFAULT_SOURCE").unwrap();
    writeln!(out, "#include <stdint.h>").unwrap();
    writeln!(out, "#include <stdbool.h>").unwrap();
    writeln!(out, "#include <stddef.h>").unwrap();
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
    writeln!(out, "#if defined(_WIN32)").unwrap();
    writeln!(out, "#define WIN32_LEAN_AND_MEAN").unwrap();
    writeln!(out, "#include <windows.h>").unwrap();
    writeln!(out, "#include <io.h>").unwrap();
    writeln!(out, "#include <winsock2.h>").unwrap();
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
    for (name, value) in [("RTLD_LAZY", 1), ("RTLD_LOCAL", 2), ("RTLD_NOW", 4)] {
        writeln!(out, "#ifndef {name}\n#define {name} {value}\n#endif").unwrap();
    }
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
}

fn append_const_pool(out: &mut String, chunk: &Chunk) {
    writeln!(out, "/* --- constant pool --- */").unwrap();
    for (i, c) in chunk.consts.iter().enumerate() {
        match c {
            Value::Str(s) => {
                let escaped = super::escape_c_string(s);
                writeln!(
                    out,
                    "static const char *const SUPER_STR_{i} = \"{escaped}\";"
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
}

fn append_module_globals(out: &mut String, chunk: &Chunk) {
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
}

fn runtime_impl_slice() -> &'static str {
    const TYPE_SPLIT: &str = "static void super_gui_miss";
    RUNTIME_PRELUDE
        .split_once(TYPE_SPLIT)
        .map(|(_, tail)| tail)
        .unwrap_or("")
}

fn runtime_types_slice() -> &'static str {
    const TYPE_SPLIT: &str = "static void super_gui_miss";
    RUNTIME_PRELUDE
        .split_once(TYPE_SPLIT)
        .map(|(head, _)| head)
        .unwrap_or(RUNTIME_PRELUDE)
}

fn append_runtime_body(out: &mut String, guard: &str) {
    out.push_str(&emit_font8());
    out.push_str("#define SUPER_GUI_SPLIT 1\n");
    out.push_str("#define SUPER_PROVIDER_API static\n");
    out.push_str("#define SUPER_GUI_PASTED 1\n");
    out.push_str(include_str!("../gui_provider.h"));
    out.push('\n');
    out.push_str(include_str!("../gui_doc.h"));
    out.push('\n');
    out.push_str(include_str!("../gtk_provider.c"));
    out.push('\n');
    out.push_str(include_str!("../win32_provider.c"));
    out.push('\n');
    out.push_str(include_str!("../cocoa_provider.c"));
    out.push('\n');
    out.push_str(include_str!("../gui_dispatch.c"));
    out.push('\n');
    out.push_str("static void super_gui_miss");
    out.push_str(runtime_impl_slice());
    writeln!(out).unwrap();
    out.push_str(crate::aot_host::NET_HOST_C);
    writeln!(out, "/* SUPER_RT_{guard} */").unwrap();
}

pub fn emit_incremental_sources(
    chunk: &Chunk,
    interner: &Interner,
    opts: &IncrementalEmitOptions<'_>,
) -> Result<(), String> {
    std::fs::create_dir_all(opts.out_dir)
        .map_err(|e| format!("mkdir {}: {e}", opts.out_dir.display()))?;
    let (rt_header, runtime_c, mod_header, units) =
        plan_translation_units(chunk, interner, &opts.emit, opts.module_name);
    let guard = sanitize_ident(opts.module_name).to_uppercase();
    std::fs::write(opts.out_dir.join("super_rt.h"), rt_header)
        .map_err(|e| format!("write super_rt.h: {e}"))?;
    std::fs::write(opts.out_dir.join("super_rt.c"), runtime_c)
        .map_err(|e| format!("write super_rt.c: {e}"))?;
    let mut mod_file = format!(
        "#include \"super_rt.h\"\n#ifndef SUPER_MOD_HDR_{guard}\n#define SUPER_MOD_HDR_{guard}\n"
    );
    mod_file.push_str(&mod_header);
    writeln!(mod_file, "#endif /* SUPER_MOD_HDR_{guard} */").unwrap();
    std::fs::write(opts.out_dir.join("super_mod.h"), mod_file)
        .map_err(|e| format!("write super_mod.h: {e}"))?;
    for unit in &units {
        let path = opts.out_dir.join(format!("{}.c", unit.stem));
        std::fs::write(&path, &unit.source)
            .map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn plan_translation_units(
    chunk: &Chunk,
    interner: &Interner,
    opts: &EmitCOptions<'_>,
    module_name: &str,
) -> (String, String, String, Vec<TranslationUnit>) {
    let guard = sanitize_ident(module_name).to_uppercase();
    let planned = unboxed::plan(chunk, interner, opts.outline_stmt_threshold);

    let mut rt_header = String::new();
    writeln!(
        rt_header,
        "/* Super shared runtime header — generated; do not edit. */"
    )
    .unwrap();
    writeln!(rt_header, "#ifndef SUPER_RT_HDR_{guard}").unwrap();
    writeln!(rt_header, "#define SUPER_RT_HDR_{guard}").unwrap();
    append_c_standard_includes(&mut rt_header);
    writeln!(rt_header).unwrap();
    rt_header.push_str(runtime_types_slice());
    rt_header.push_str(&unboxed::unboxed_prelude());
    writeln!(rt_header).unwrap();
    writeln!(rt_header, "#endif /* SUPER_RT_HDR_{guard} */").unwrap();

    let mut runtime_c = String::new();
    writeln!(
        runtime_c,
        "/* Super shared runtime — generated; do not edit. */"
    )
    .unwrap();
    writeln!(runtime_c, "#include \"super_rt.h\"").unwrap();
    writeln!(runtime_c, "#ifndef SUPER_RT_{guard}").unwrap();
    writeln!(runtime_c, "#define SUPER_RT_{guard}").unwrap();
    append_runtime_body(&mut runtime_c, &guard);

    if let (Some(typed), Some(module)) = (opts.typed, opts.module) {
        emit_c_abi_types(&mut runtime_c, typed, interner, module);
        writeln!(runtime_c).unwrap();
    }
    append_const_pool(&mut runtime_c, chunk);
    append_module_globals(&mut runtime_c, chunk);
    writeln!(runtime_c).unwrap();
    for note in &planned.notes {
        writeln!(runtime_c, "{note}").unwrap();
    }
    writeln!(runtime_c, "/* typed globals + forward-only decl storage */").unwrap();
    runtime_c.push_str(&planned.types);
    runtime_c.push_str(&planned.decls);
    writeln!(runtime_c).unwrap();

    let dyn_proto: Vec<(String, bool)> = planned
        .dyn_fns
        .iter()
        .map(|&fi| {
            (
                function_c_name(chunk, interner, fi),
                chunk.functions[fi].always_inline,
            )
        })
        .collect();
    let mod_header = format!(
        "{}{}",
        planned.types,
        export_prototypes_from_decls(&planned.decls, &dyn_proto)
    );

    // Module init may call dyn fns that live in other TUs.
    writeln!(
        runtime_c,
        "/* --- dyn fn prototypes (bodies in per-fn TUs) --- */"
    )
    .unwrap();
    for (fname, _) in &dyn_proto {
        writeln!(
            runtime_c,
            "SuperVal {fname}(SuperVal *restrict args, int nargs);"
        )
        .unwrap();
    }
    writeln!(runtime_c).unwrap();

    writeln!(
        runtime_c,
        "/* --- unboxed instance bodies (shared runtime TU) --- */"
    )
    .unwrap();
    // Emit `#inline` / always_inline bodies before callers (smallest first so
    // leaves are visible when larger always_inline helpers are compiled).
    let mut inst_order: Vec<&(String, String)> = planned.inst_units.iter().collect();
    inst_order.sort_by(|(_, a), (_, b)| {
        let ai = a.contains("__attribute__((always_inline))");
        let bi = b.contains("__attribute__((always_inline))");
        match (ai, bi) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.len().cmp(&b.len()),
        }
    });
    for (_, body) in inst_order {
        runtime_c.push_str(&strip_static_fn(body));
        writeln!(runtime_c).unwrap();
    }
    match &planned.module_wrapper {
        Some(wrapper) => runtime_c.push_str(&strip_static_fn(wrapper)),
        None => {
            let mut boxed = String::new();
            emit_boxed_module_main(&mut boxed, chunk, interner);
            runtime_c.push_str(&strip_static_fn(&boxed));
        }
    }
    writeln!(runtime_c).unwrap();

    let mut units = Vec::new();

    for &fi in &planned.dyn_fns {
        let stem = format!("super_fn_{fi}");
        let mut body = String::new();
        emit_function(&mut body, chunk, interner, fi, &chunk.functions[fi], true);
        let mut src = tu_includes(&stem.to_uppercase());
        src.push_str(&body);
        src.push_str(&tu_footer(&stem.to_uppercase()));
        units.push(TranslationUnit { stem, source: src });
    }

    let mut main_body = String::new();
    if !opts.omit_main {
        writeln!(main_body, "int main(int argc, char **argv) {{").unwrap();
        writeln!(main_body, "  super_argc = argc;").unwrap();
        writeln!(main_body, "  super_argv = argv;").unwrap();
        writeln!(main_body, "  SuperVal r = super_module_main();").unwrap();
        writeln!(
            main_body,
            "  if (r.tag == SUPER_TAG_INT) return (int)r.as.i;"
        )
        .unwrap();
        writeln!(main_body, "  return 0;").unwrap();
        writeln!(main_body, "}}").unwrap();
    }
    let mut main_src = tu_includes("MAIN");
    main_src.push_str(&main_body);
    main_src.push_str(&tu_footer("MAIN"));
    units.push(TranslationUnit {
        stem: "super_main".into(),
        source: main_src,
    });

    writeln!(runtime_c, "#endif /* SUPER_RT_{guard} */").unwrap();

    (rt_header, runtime_c, mod_header, units)
}

fn fingerprint_path(out_dir: &Path) -> PathBuf {
    out_dir.join(".super_codegen_hashes")
}

fn load_fingerprints(path: &Path) -> HashMap<String, u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (k, v) = line.split_once('=')?;
            Some((k.to_string(), v.parse().ok()?))
        })
        .collect()
}

fn save_fingerprints(path: &Path, map: &HashMap<String, u64>) -> Result<(), String> {
    let mut lines: Vec<_> = map.iter().map(|(k, v)| format!("{k}={v}")).collect();
    lines.sort();
    std::fs::write(path, lines.join("\n")).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Emit translation units under `out_dir`, compile incrementally, and link.
pub fn write_and_compile_incremental(
    chunk: &Chunk,
    interner: &Interner,
    opts: &IncrementalEmitOptions<'_>,
    compile: &CCompileOptions<'_>,
) -> Result<PathBuf, String> {
    // Per-fn TUs cannot yet see the shared const pool / `static inline` helpers in
    // `super_rt.c`. When any function stays dynamic (not fully unboxed), fall back to
    // a single self-contained `.c` — same output as `--codegen-units single`.
    let planned = unboxed::plan(chunk, interner, opts.emit.outline_stmt_threshold);
    if !planned.dyn_fns.is_empty() {
        std::fs::create_dir_all(opts.out_dir)
            .map_err(|e| format!("mkdir {}: {e}", opts.out_dir.display()))?;
        let source = super::emit_c_module(chunk, interner, opts.module_name, opts.emit.clone());
        let c_path = opts
            .out_dir
            .join(format!("{}.c", sanitize_ident(opts.module_name)));
        return super::write_and_maybe_compile_with(&source, &c_path, true, compile)
            .and_then(|opt| opt.ok_or_else(|| "single-file fallback produced no binary".into()));
    }

    emit_incremental_sources(chunk, interner, opts)?;

    let (_, runtime_c, _, units) =
        plan_translation_units(chunk, interner, &opts.emit, opts.module_name);

    let mut compile = compile.clone();
    compile.needs_openmp = super::c_source_needs_openmp(&runtime_c)
        || units
            .iter()
            .any(|unit| super::c_source_needs_openmp(&unit.source));

    let fp_path = fingerprint_path(opts.out_dir);
    let mut fingerprints = load_fingerprints(&fp_path);
    let target = compile.resolved_target();
    let invocation = crate::target::select_compiler(&target, compile.cc)?;
    let compile = &compile;
    let bin = compile
        .out_bin
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| opts.out_dir.join("super.out"));

    let mut objects = Vec::new();
    let mut artifact_paths: Vec<PathBuf> = Vec::new();

    let mut compile_unit = |stem: &str, source: &str| -> Result<PathBuf, String> {
        let c_path = opts.out_dir.join(format!("{stem}.c"));
        let o_path = opts.out_dir.join(format!("{stem}.o"));
        std::fs::write(&c_path, source).map_err(|e| format!("write {}: {e}", c_path.display()))?;
        artifact_paths.push(c_path.clone());
        let args = compile.cc_object_args(&c_path, &o_path);
        let hash = compile_fingerprint(source, &invocation.display(), &args);
        let prev = fingerprints.get(stem).copied();
        if prev == Some(hash) && o_path.is_file() {
            objects.push(o_path.clone());
            return Ok(o_path);
        }
        let mut cmd = std::process::Command::new(&invocation.program);
        invocation.apply(&mut cmd);
        cmd.args(&args);
        apply_cc_diagnostic_colors(&mut cmd);
        let status = cmd
            .status()
            .map_err(|e| format!("spawn {}: {e}", invocation.display()))?;
        if !status.success() {
            return Err(format!("compile {stem}: {} ({})", status, c_path.display()));
        }
        fingerprints.insert(stem.to_string(), hash);
        objects.push(o_path.clone());
        artifact_paths.push(o_path.clone());
        Ok(o_path)
    };

    compile_unit("super_rt", &runtime_c)?;
    for unit in &units {
        compile_unit(&unit.stem, &unit.source)?;
    }

    let link_args = compile.cc_link_args(&objects, &bin);
    let mut cmd = std::process::Command::new(&invocation.program);
    invocation.apply(&mut cmd);
    cmd.args(&link_args);
    apply_cc_diagnostic_colors(&mut cmd);
    let status = cmd
        .status()
        .map_err(|e| format!("spawn linker {}: {e}", invocation.display()))?;
    if !status.success() {
        return Err(format!("link {}: {status}", bin.display()));
    }

    save_fingerprints(&fp_path, &fingerprints)?;

    if !opts.keep_c {
        for path in artifact_paths {
            if path.extension().is_some_and(|e| e == "c" || e == "o") {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use crate::{CodegenOpt, LtoMode};
    use super_core::SourceFile;
    use super_frontend::analyze;

    fn cc_available() -> bool {
        std::process::Command::new("cc")
            .arg("--version")
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn object_fingerprint_includes_compiler_and_flags() {
        let args = vec!["-c".to_string(), "-O2".to_string()];
        let baseline = compile_fingerprint("int x;", "cc", &args);
        assert_ne!(
            baseline,
            compile_fingerprint("int x;", "x86_64-w64-mingw32-gcc", &args)
        );
        assert_ne!(
            baseline,
            compile_fingerprint("int x;", "cc", &["-c".into(), "-O3".into()])
        );
    }

    #[test]
    fn incremental_emits_rt_and_function_units() {
        let src = "main() { print(5) }";
        let file = SourceFile::new("inc.sr", src);
        let mut result = analyze(&file);
        assert!(!result.diagnostics.has_errors());
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, true)
            .with_opt_meta(meta)
            .compile(&result.module);
        let dir = std::env::temp_dir().join(format!("super_inc_emit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let emit_opts = IncrementalEmitOptions {
            emit: EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
            module_name: "inc",
            out_dir: &dir,
            keep_c: true,
        };
        let (_hdr, rt_c, _mod, units) =
            plan_translation_units(&chunk, &result.interner, &emit_opts.emit, "inc");
        assert!(rt_c.contains("SUPER_RT_INC"));
        assert!(rt_c.contains("#include \"super_rt.h\""));
        assert!(units.iter().any(|u| u.stem == "super_main"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("super_rt.h"), "#pragma once").unwrap();
        std::fs::write(dir.join("super_rt.c"), &rt_c).unwrap();
        for u in &units {
            std::fs::write(dir.join(format!("{}.c", u.stem)), &u.source).unwrap();
        }
        assert!(dir.join("super_rt.c").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn incremental_single_and_incremental_emit_single_file_parity() {
        let src = "main() { print(5) }";
        let file = SourceFile::new("parity.sr", src);
        let mut result = analyze(&file);
        assert!(!result.diagnostics.has_errors());
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, true)
            .with_opt_meta(meta)
            .compile(&result.module);
        let single = super::super::emit_c_module(
            &chunk,
            &result.interner,
            "parity",
            EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
        );
        assert!(single.contains("int main"));
        let dir = std::env::temp_dir().join(format!("super_inc_parity_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let emit = IncrementalEmitOptions {
            emit: EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
            module_name: "parity",
            out_dir: &dir,
            keep_c: true,
        };
        emit_incremental_sources(&chunk, &result.interner, &emit).unwrap();
        assert!(dir.join("super_rt.c").is_file());
        assert!(dir.join("super_main.c").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn incremental_links_small_program() {
        if !cc_available() {
            return;
        }
        let src = "main() { print(9) }";
        let file = SourceFile::new("link.sr", src);
        let mut result = analyze(&file);
        assert!(!result.diagnostics.has_errors());
        let meta = crate::optimize_module(&mut result.module, &result.typed, &mut result.interner);
        let chunk = Compiler::new(&mut result.interner, &result.typed, true)
            .with_opt_meta(meta)
            .compile(&result.module);
        let dir = std::env::temp_dir().join(format!("super_inc_link_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("link.out");
        let emit = IncrementalEmitOptions {
            emit: EmitCOptions {
                typed: Some(&result.typed),
                module: Some(&result.module),
                omit_main: false,
                release: true,
                ..Default::default()
            },
            module_name: "link",
            out_dir: &dir,
            keep_c: true,
        };
        let opts = CCompileOptions {
            out_bin: Some(&bin),
            opt: CodegenOpt::O0,
            fast_math: false,
            max_speed: false,
            lto: LtoMode::Off,
            target_cpu: None,
            ..CCompileOptions::default()
        };
        write_and_compile_incremental(&chunk, &result.interner, &emit, &opts)
            .unwrap_or_else(|e| panic!("{e}"));
        let out = std::process::Command::new(&bin).output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "9\n");
    }

    #[test]
    fn fingerprint_skips_recompile_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("super_inc_fp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fp_path = fingerprint_path(&dir);
        let mut map = HashMap::new();
        map.insert("super_rt".into(), 42);
        save_fingerprints(&fp_path, &map).unwrap();
        let loaded = load_fingerprints(&fp_path);
        assert_eq!(loaded.get("super_rt"), Some(&42));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
