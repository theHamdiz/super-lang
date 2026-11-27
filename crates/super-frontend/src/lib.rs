//! Zero-allocation frontend: lex → parse → single-pass sema.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod ast;
pub mod bench_purity;
pub mod cfg;
pub mod format;
pub mod intrinsics;
pub mod lexer;
pub mod lint;
pub mod macros;
pub mod meta;
pub mod modules;
pub mod parser;
pub mod sema;
pub mod template;
pub mod token;
pub mod verify;

use super_core::{Diagnostics, Interner, SourceFile};

pub use ast::{Ast, Module};
pub use bench_purity::{
    BenchHostBanProof, HostBanViolation, LAW_BENCH_001, LAW_BENCH_001_NAME, LAW_BENCH_001_TITLE,
    check_benchmark_file, check_source_bans_host, find_benchmarks_root, path_is_benchmark_source,
    prove_law_bench_001,
};
pub use cfg::CfgContext;
pub use lexer::Lexer;
pub use meta::{CtValue, MetaCache, MetaEngine};
pub use modules::{
    ImportTarget, ProjectAnalyze, StdlibModuleMeta, SymbolOrigin, analyze_project,
    analyze_project_text, analyze_project_text_with_paths, analyze_project_text_with_paths_and_cfg,
    analyze_project_with_paths, analyze_project_with_paths_and_cfg, find_std_root,
    find_std_root_for_entry, find_std_root_from_env, find_std_root_near, module_path_to_file_uri,
    module_source_text, std_embedded_source, stdlib_import_next_segments, stdlib_module_catalog,
    stdlib_module_catalog_for, stdlib_use_completions, stdlib_use_completions_for,
    stdlib_use_line_for_manifest,
};
pub use parser::Parser;
pub use sema::{SemanticAnalyzer, TypedModule};
pub use token::{Token, TokenKind};
pub use verify::{
    Counterexample, DEFAULT_VERIFY_TIMEOUT_MS, DischargeTier, LawResult, SolverKind, VerifyOptions,
    VerifyReport, collect_verify_items, discover_law_proof_files, format_laws_lock,
    is_verify_sidecar, parse_laws_lock, resolve_sidecar_primary, verify_module,
};

/// Lex, parse, expand macros and `#` metaprogramming, and type-check a source file.
/// Prefer [`analyze_project`] for real files so `use` / `std.*` resolve.
pub fn analyze(source: &SourceFile) -> AnalyzeResult {
    let mut interner = Interner::with_capacity(256);
    let mut diagnostics = Diagnostics::new();
    let tokens = Lexer::new(&source.text, &mut interner, &mut diagnostics).tokenize();
    let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diagnostics);
    let mut module = parser.parse_module();
    cfg::strip_cfg_items(
        &mut module,
        &interner,
        &mut diagnostics,
        &CfgContext::host(),
    );
    macros::expand_module(&mut module, &mut interner, &mut diagnostics);
    let mut meta = MetaEngine::new(&mut interner, &mut diagnostics);
    meta.expand_module(&mut module);
    let meta_cache = meta.cache;
    template::check_module_templates(&module, &interner, &mut diagnostics);
    let sema = SemanticAnalyzer::new(&mut interner, &mut diagnostics);
    let typed = sema.analyze(&module);
    AnalyzeResult {
        module,
        typed,
        interner,
        diagnostics,
        tokens,
        meta_cache,
    }
}

pub struct AnalyzeResult {
    pub module: Module,
    pub typed: TypedModule,
    pub interner: Interner,
    pub diagnostics: Diagnostics,
    pub tokens: Vec<Token>,
    pub meta_cache: MetaCache,
}
