//! Panic-safe C ABI matching `include/super.h`.

use crate::abi::SigDesc;
use crate::engine::{Engine, NativeFn};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

/// Opaque engine handle for C hosts.
pub struct SuperEngine {
    inner: Engine,
    error_cstring: Option<CString>,
}

impl SuperEngine {
    fn set_err(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.inner.set_error(msg.clone());
        self.error_cstring = CString::new(msg).ok();
    }
}

fn cstr<'a>(p: *const c_char) -> Result<&'a str, String> {
    if p.is_null() {
        return Err("null string".into());
    }
    // SAFETY: caller guarantees a valid C string for the duration of the call.
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map_err(|e| e.to_string())
}

fn ok_int() -> c_int {
    0
}
fn err_int() -> c_int {
    -1
}

/// Create a new Super engine.
#[unsafe(no_mangle)]
pub extern "C" fn super_engine_new() -> *mut SuperEngine {
    match catch_unwind(|| SuperEngine {
        inner: Engine::new(),
        error_cstring: None,
    }) {
        Ok(engine) => Box::into_raw(Box::new(engine)),
        Err(_) => ptr::null_mut(),
    }
}

/// Free an engine previously returned by [`super_engine_new`].
#[unsafe(no_mangle)]
pub extern "C" fn super_engine_free(engine: *mut SuperEngine) {
    if engine.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: engine came from Box::into_raw in super_engine_new.
        unsafe {
            drop(Box::from_raw(engine));
        }
    }));
}

/// Compile and load a `.sr` file into the engine.
#[unsafe(no_mangle)]
pub extern "C" fn super_load_file(engine: *mut SuperEngine, path: *const c_char) -> c_int {
    if engine.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: opaque pointer from super_engine_new.
        let eng = unsafe { &mut *engine };
        let path = match cstr(path) {
            Ok(p) => p,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        match eng.inner.load_file(path) {
            Ok(()) => ok_int(),
            Err(e) => {
                eng.set_err(e);
                err_int()
            }
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            // SAFETY: engine non-null checked above.
            unsafe { (*engine).set_err("panic in super_load_file") };
            err_int()
        }
    }
}

/// Hot-reload a module, keeping host bindings and data stacks.
#[unsafe(no_mangle)]
pub extern "C" fn super_reload_module(engine: *mut SuperEngine, path: *const c_char) -> c_int {
    if engine.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let path = match cstr(path) {
            Ok(p) => p,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        match eng.inner.reload_module(path) {
            Ok(()) => ok_int(),
            Err(e) => {
                eng.set_err(e);
                err_int()
            }
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_reload_module") };
            err_int()
        }
    }
}

/// Last error message for `engine`, or empty string. Valid until the next error.
#[unsafe(no_mangle)]
pub extern "C" fn super_last_error(engine: *mut SuperEngine) -> *const c_char {
    if engine.is_null() {
        return ptr::null();
    }
    let eng = unsafe { &mut *engine };
    if eng.error_cstring.is_none() {
        eng.error_cstring = CString::new(eng.inner.last_error()).ok();
    }
    eng.error_cstring
        .as_ref()
        .map(|c| c.as_ptr())
        .unwrap_or(ptr::null())
}

/// Register a native host function.
///
/// `sig_descriptor` examples: `"f32,f32->void"`, `"i64->i64"`, `"Entity:16,f32->f32"`.
#[unsafe(no_mangle)]
pub extern "C" fn super_bind_fn(
    engine: *mut SuperEngine,
    module: *const c_char,
    name: *const c_char,
    fn_ptr: Option<NativeFn>,
    user_data: *mut c_void,
    sig_descriptor: *const c_char,
) -> c_int {
    if engine.is_null() {
        return err_int();
    }
    let Some(fn_ptr) = fn_ptr else {
        return err_int();
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let module = cstr(module).unwrap_or("");
        let name = match cstr(name) {
            Ok(n) => n,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        let sig = cstr(sig_descriptor).unwrap_or("");
        eng.inner.bind_native(module, name, fn_ptr, user_data, sig);
        ok_int()
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_bind_fn") };
            err_int()
        }
    }
}

/// Zero-copy share of a host array with Super (`for @name` loops).
#[unsafe(no_mangle)]
pub extern "C" fn super_bind_slice(
    engine: *mut SuperEngine,
    name: *const c_char,
    ptr: *mut c_void,
    len: usize,
    elem_size: usize,
) -> c_int {
    if engine.is_null() || name.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let name = match cstr(name) {
            Ok(n) => n,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        eng.inner.bind_slice(name, ptr, len, elem_size, None);
        ok_int()
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_bind_slice") };
            err_int()
        }
    }
}

/// Optional layout-aware bind: `layout` is `"x:f32,y:f32,vx:f32,vy:f32"`.
#[unsafe(no_mangle)]
pub extern "C" fn super_bind_slice_ex(
    engine: *mut SuperEngine,
    name: *const c_char,
    ptr: *mut c_void,
    len: usize,
    elem_size: usize,
    layout: *const c_char,
) -> c_int {
    if engine.is_null() || name.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let name = match cstr(name) {
            Ok(n) => n,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        let layout = if layout.is_null() {
            None
        } else {
            cstr(layout).ok()
        };
        eng.inner.bind_slice(name, ptr, len, elem_size, layout);
        ok_int()
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_bind_slice_ex") };
            err_int()
        }
    }
}

/// Call an exported Super function with C-ABI packed args/return.
#[unsafe(no_mangle)]
pub extern "C" fn super_call(
    engine: *mut SuperEngine,
    module: *const c_char,
    fn_name: *const c_char,
    args_buf: *mut c_void,
    ret_buf: *mut c_void,
) -> c_int {
    if engine.is_null() || fn_name.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let module = cstr(module).unwrap_or("");
        let fn_name = match cstr(fn_name) {
            Ok(n) => n,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        let sig = eng
            .inner
            .signature_for(fn_name)
            .unwrap_or_else(|| SigDesc::parse("->void"));
        match unsafe { eng.inner.call_raw(module, fn_name, args_buf, ret_buf, &sig) } {
            Ok(()) => ok_int(),
            Err(e) => {
                eng.set_err(e);
                err_int()
            }
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_call") };
            err_int()
        }
    }
}

/// Call with an explicit signature descriptor (when inference is unavailable).
#[unsafe(no_mangle)]
pub extern "C" fn super_call_sig(
    engine: *mut SuperEngine,
    module: *const c_char,
    fn_name: *const c_char,
    args_buf: *mut c_void,
    ret_buf: *mut c_void,
    sig_descriptor: *const c_char,
) -> c_int {
    if engine.is_null() || fn_name.is_null() {
        return err_int();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        let eng = unsafe { &mut *engine };
        let module = cstr(module).unwrap_or("");
        let fn_name = match cstr(fn_name) {
            Ok(n) => n,
            Err(e) => {
                eng.set_err(e);
                return err_int();
            }
        };
        let sig = SigDesc::parse(cstr(sig_descriptor).unwrap_or(""));
        match unsafe { eng.inner.call_raw(module, fn_name, args_buf, ret_buf, &sig) } {
            Ok(()) => ok_int(),
            Err(e) => {
                eng.set_err(e);
                err_int()
            }
        }
    }));
    match result {
        Ok(code) => code,
        Err(_) => {
            unsafe { (*engine).set_err("panic in super_call_sig") };
            err_int()
        }
    }
}
