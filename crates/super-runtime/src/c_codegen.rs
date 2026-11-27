//! Re-export surface for Super → C AOT codegen (`emit_c`, compile driver).

pub use crate::c_emit::{
    CCompileOptions, EmitCOptions, IncrementalEmitOptions, emit_c, emit_c_module,
    emit_incremental_sources, write_and_compile_incremental, write_and_maybe_compile,
    write_and_maybe_compile_with,
};
