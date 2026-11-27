//! Zero-boilerplate host FFI: `IntoSuperFn` + `bind_slice_mut` + C ABI + embed API.
//!
//! Build as `rlib`, `staticlib` (`libsuper_ffi.a`), and `cdylib` (`libsuper_ffi.so` / `super.dll`).

#![deny(unsafe_op_in_unsafe_fn)]
// The `extern "C"` entry points take raw pointers from C callers and check them
// for null; they cannot be `unsafe fn` without changing the exported ABI.
#![allow(clippy::not_unsafe_ptr_arg_deref, clippy::result_large_err)]

pub mod abi;
pub mod c_api;
pub mod engine;

pub use abi::{SigDesc, SigSlot};
pub use c_api::SuperEngine;
pub use engine::{BoundSliceMut, Engine, HostFn, IntoSuperFn, NativeFn};
pub use super_frontend::ast::Module;
pub use super_frontend::sema::TypedModule;
pub use super_runtime::{
    ExtField, ExtKind, ExtSliceBinding, HostEnv, Value, VmResult, parse_layout_descriptor,
};

use super_core::{Diagnostics, Interner, SourceFile};
use super_frontend::analyze;

/// Re-export analyze pipeline pieces for embedders.
pub fn compile_source(path: &str, source: &str) -> (Module, TypedModule, Interner, Diagnostics) {
    let file = SourceFile::new(path, source);
    let r = analyze(&file);
    (r.module, r.typed, r.interner, r.diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_closure() {
        let eng = Engine::new();
        eng.register("double", |v: Value| Value::Int(v.as_int() * 2));
    }

    #[test]
    fn bind_slice_mut_shares() {
        let mut data = [1i32, 2, 3];
        let mut bound = BoundSliceMut::bind_slice_mut(&mut data);
        bound.as_mut()[0] = 9;
        assert_eq!(data[0], 9);
    }

    #[test]
    fn engine_load_and_call() {
        let dir = std::env::temp_dir().join(format!("super_ffi_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("add.sr");
        std::fs::write(&path, "+add(a: i64, b: i64) : i64 => a + b\n").unwrap();
        let mut eng = Engine::new();
        eng.load_file(&path).expect("load");
        let v = eng
            .call_values("", "add", vec![Value::Int(2), Value::Int(3)])
            .expect("call");
        assert_eq!(v.as_int(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn host_pipeline_passes_args_without_receiver() {
        use std::sync::{Arc, Mutex};
        let dir = std::env::temp_dir().join(format!("super_host_args_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.sr");
        std::fs::write(&path, "+go() {\n  host.refuel(3, 5.0)\n}\n").unwrap();
        let seen = Arc::new(Mutex::new(None));
        let seen2 = Arc::clone(&seen);
        let mut eng = Engine::new();
        eng.bind_fn_rust("host", "refuel", move |a: Value, b: Value| {
            *seen2.lock().unwrap() = Some((a.as_int(), b.as_float()));
            Value::Null
        });
        eng.load_file(&path).unwrap();
        eng.call_values("", "go", vec![]).unwrap();
        let got = (*seen.lock().unwrap()).expect("callback");
        assert_eq!(got.0, 3);
        assert!((got.1 - 5.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
