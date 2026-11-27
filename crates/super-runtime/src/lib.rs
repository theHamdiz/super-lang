//! Runtime: scoped stacks, work-stealing scheduler, bytecode VM, and JIT.

#![deny(unsafe_op_in_unsafe_fn)]
// Style lints the emitters and optimizer opt out of: index loops walk several
// parallel op arrays, emitter entry points take many independent knobs, and
// error values stay unboxed because the error path is cold.
#![allow(
    clippy::needless_range_loop,
    clippy::result_large_err,
    clippy::result_unit_err,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::wrong_self_convention
)]

pub mod aot_host;
pub mod bytecode;
pub mod c_codegen;
pub mod c_emit;
pub mod c_keywords;
pub mod compiler;
pub mod escape_region;
pub mod host;
pub mod jit;
pub mod opt;
pub mod sched;
pub mod stacks;
pub mod target;
pub mod traits;
pub mod value;
pub mod vm;
pub mod vm_req;

pub use bytecode::{Chunk, Op, OptNoteKind};
pub use escape_region::{
    AllocSiteFacts, EscapeClass, EscapeEdge, EscapeSummary, SiteClass, classify_module,
    classify_module_summary, classify_promote_everything, classify_site,
};
pub use c_emit::{
    BuildProfile, CCompileOptions, CodegenOpt, DEFAULT_OUTLINE_STMT_THRESHOLD, EmitCOptions,
    IncrementalEmitOptions, LtoMode, ProfileOverrides, ResolvedProfile, emit_c, emit_c_module,
    emit_incremental_sources, resolve_profile, syntax_check_c, write_and_compile_incremental,
    write_and_maybe_compile, write_and_maybe_compile_with,
};
pub use c_keywords::{C_KEYWORDS, c_ident, is_c_keyword};
pub use compiler::Compiler;
pub use host::{ExtSliceBinding, HostCallback, HostEnv, parse_layout_descriptor};
pub use jit::{ExecutableBuffer, JitEngine, compile_ecs_integrate, run_ecs_integrate};
pub use opt::{
    OptMeta, OptStats, disassemble, disassemble_native, escape_birth_region, optimize_module,
    peephole,
};
pub use sched::{Channel, Scheduler, SharedCell, SharedInt, TaskHandle};
pub use stacks::{DataStack, StackPool, TaskStack, WorkerStackBank};
pub use target::{
    Arch, CcInvocation, CcKind, CompileTarget, Env, KNOWN_TARGETS, Os, TargetCatalogEntry, Vendor,
    format_catalog, select_compiler,
};
pub use value::{ExtField, ExtKind, Value};
pub use vm::{Vm, VmError, VmResult};
pub use vm_req::{
    conn_enter as vm_conn_enter, conn_leave as vm_conn_leave, conn_stats as vm_conn_stats,
    deep_clone as vm_deep_clone, on_birth as vm_on_birth, req_active as vm_req_active,
    req_enter as vm_req_enter, req_keep_str as vm_req_keep_str, req_leave as vm_req_leave,
    req_promote as vm_req_promote, req_stats as vm_req_stats,
};

use super_core::Interner;
use super_frontend::ast::Module;
use super_frontend::sema::TypedModule;

/// Compile a typed module to bytecode and execute it.
pub fn run_module(
    module: &Module,
    typed: &TypedModule,
    interner: &mut Interner,
    strip_tests: bool,
) -> VmResult<Value> {
    run_module_in(module, typed, interner, strip_tests, ".")
}

/// Like [`run_module`], but resolve `embed` paths relative to `base_dir`.
pub fn run_module_in(
    module: &Module,
    typed: &TypedModule,
    interner: &mut Interner,
    strip_tests: bool,
    base_dir: impl Into<std::path::PathBuf>,
) -> VmResult<Value> {
    run_module_with_host(module, typed, interner, strip_tests, base_dir, None)
}

/// Like [`run_module_in`], optionally attaching a [`HostEnv`] for Tier-1 syscalls.
pub fn run_module_with_host(
    module: &Module,
    typed: &TypedModule,
    interner: &mut Interner,
    strip_tests: bool,
    base_dir: impl Into<std::path::PathBuf>,
    host: Option<std::sync::Arc<HostEnv>>,
) -> VmResult<Value> {
    let mut module = module.clone();
    let meta = optimize_module(&mut module, typed, interner);
    let compiler = Compiler::with_base(interner, typed, strip_tests, base_dir).with_opt_meta(meta);
    let chunk = compiler.compile(&module);
    let _ = &module; // optimized AST is what we compile
    let mut stacks = StackPool::new(1 << 20);
    let mut vm = Vm::new(chunk, &mut stacks, interner);
    if let Some(h) = host {
        vm.set_host(h);
    }
    vm.run()
}

/// Compile and run only `test` blocks; returns number of tests passed.
pub fn run_tests(module: &Module, typed: &TypedModule, interner: &mut Interner) -> VmResult<usize> {
    run_tests_in(module, typed, interner, ".")
}

pub fn run_tests_in(
    module: &Module,
    typed: &TypedModule,
    interner: &mut Interner,
    base_dir: impl Into<std::path::PathBuf>,
) -> VmResult<usize> {
    run_tests_with_host(module, typed, interner, base_dir, None)
}

/// Like [`run_tests_in`], optionally attaching a [`HostEnv`] for Tier-1 syscalls.
pub fn run_tests_with_host(
    module: &Module,
    typed: &TypedModule,
    interner: &mut Interner,
    base_dir: impl Into<std::path::PathBuf>,
    host: Option<std::sync::Arc<HostEnv>>,
) -> VmResult<usize> {
    let mut module = module.clone();
    let meta = optimize_module(&mut module, typed, interner);
    let compiler = Compiler::with_base(interner, typed, false, base_dir).with_opt_meta(meta);
    let chunk = compiler.compile_tests(&module);
    let mut stacks = StackPool::new(1 << 20);
    let mut vm = Vm::new(chunk, &mut stacks, interner);
    if let Some(h) = host {
        vm.set_host(h);
    }
    if let Err(e) = vm.run() {
        if let Some(name) = vm.current_test_name() {
            eprintln!(
                "  in test \"{name}\" (after {} passing assertion(s))",
                vm.tests_passed()
            );
        }
        return Err(e);
    }
    Ok(vm.tests_passed())
}
