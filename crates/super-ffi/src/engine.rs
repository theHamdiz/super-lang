//! Embeddable Super engine with load/reload, host binds, and typed calls.

use crate::abi::{self, SigDesc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use super_core::Interner;
use super_frontend::analyze_project;
use super_frontend::ast::Module;
use super_frontend::sema::{TypeKind, TypedModule};
use super_runtime::{
    Compiler, ExtField, ExtKind, ExtSliceBinding, HostCallback, HostEnv, StackPool, Value, Vm,
    VmError, VmResult, parse_layout_descriptor,
};

/// Host-callable Super function registered from systems peer.
pub type HostFn = HostCallback;

/// Trait for 1-line registration of systems peer closures/functions into Super.
pub trait IntoSuperFn<Args, Ret> {
    fn into_super_fn(self) -> HostFn;
}

impl<F> IntoSuperFn<(), Value> for F
where
    F: Fn() -> Value + Send + Sync + 'static,
{
    fn into_super_fn(self) -> HostFn {
        Arc::new(move |_args: &[Value]| self())
    }
}

impl<F> IntoSuperFn<(Value,), Value> for F
where
    F: Fn(Value) -> Value + Send + Sync + 'static,
{
    fn into_super_fn(self) -> HostFn {
        Arc::new(move |args: &[Value]| {
            let a0 = args.first().cloned().unwrap_or(Value::Null);
            self(a0)
        })
    }
}

impl<F> IntoSuperFn<(Value, Value), Value> for F
where
    F: Fn(Value, Value) -> Value + Send + Sync + 'static,
{
    fn into_super_fn(self) -> HostFn {
        Arc::new(move |args: &[Value]| {
            let a0 = args.first().cloned().unwrap_or(Value::Null);
            let a1 = args.get(1).cloned().unwrap_or(Value::Null);
            self(a0, a1)
        })
    }
}

impl<F> IntoSuperFn<(Value, Value, Value), Value> for F
where
    F: Fn(Value, Value, Value) -> Value + Send + Sync + 'static,
{
    fn into_super_fn(self) -> HostFn {
        Arc::new(move |args: &[Value]| {
            let a0 = args.first().cloned().unwrap_or(Value::Null);
            let a1 = args.get(1).cloned().unwrap_or(Value::Null);
            let a2 = args.get(2).cloned().unwrap_or(Value::Null);
            self(a0, a1, a2)
        })
    }
}

impl<F> IntoSuperFn<(Value, Value, Value, Value), Value> for F
where
    F: Fn(Value, Value, Value, Value) -> Value + Send + Sync + 'static,
{
    fn into_super_fn(self) -> HostFn {
        Arc::new(move |args: &[Value]| {
            let a0 = args.first().cloned().unwrap_or(Value::Null);
            let a1 = args.get(1).cloned().unwrap_or(Value::Null);
            let a2 = args.get(2).cloned().unwrap_or(Value::Null);
            let a3 = args.get(3).cloned().unwrap_or(Value::Null);
            self(a0, a1, a2, a3)
        })
    }
}

/// Zero-copy mutable slice sharing between host and Super.
pub struct BoundSliceMut<'a, T> {
    data: &'a mut [T],
}

impl<'a, T> BoundSliceMut<'a, T> {
    pub fn bind_slice_mut(data: &'a mut [T]) -> Self {
        Self { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl<T> AsMut<[T]> for BoundSliceMut<'_, T> {
    fn as_mut(&mut self) -> &mut [T] {
        self.data
    }
}

/// Native C trampoline: `(user_data, args_buf, ret_buf)`.
pub type NativeFn =
    unsafe extern "C" fn(*mut core::ffi::c_void, *mut core::ffi::c_void, *mut core::ffi::c_void);

struct NativeBinding {
    fn_ptr: NativeFn,
    user_data: *mut core::ffi::c_void,
    sig: SigDesc,
}

// SAFETY: host guarantees trampoline + user_data are usable from the embedder thread.
unsafe impl Send for NativeBinding {}
unsafe impl Sync for NativeBinding {}

struct Loaded {
    path: PathBuf,
    module: Module,
    typed: TypedModule,
    interner: Interner,
    chunk: super_runtime::Chunk,
    /// Module stem used as default `module` name in `call`.
    module_name: String,
}

/// Embeddable Super engine.
pub struct Engine {
    host: Arc<Mutex<HostEnv>>,
    natives: Arc<Mutex<HashMap<String, NativeBinding>>>,
    loaded: Option<Loaded>,
    stacks: StackPool,
    last_error: String,
    /// Optional layout overrides: slice name → fields.
    slice_layouts: HashMap<String, (Arc<Vec<ExtField>>, usize)>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            host: Arc::new(Mutex::new(HostEnv::new())),
            natives: Arc::new(Mutex::new(HashMap::new())),
            loaded: None,
            stacks: StackPool::new(1 << 20),
            last_error: String::new(),
            slice_layouts: HashMap::new(),
        }
    }

    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.last_error = msg.into();
    }

    pub fn register<A, R, F>(&self, name: impl Into<String>, f: F)
    where
        F: IntoSuperFn<A, R>,
    {
        let key = name.into();
        let cb = f.into_super_fn();
        let mut host = self.host.lock().unwrap();
        host.register_fn(key.clone(), Arc::clone(&cb));
        if let Some((_, bare)) = key.split_once('.') {
            host.register_fn(bare.to_string(), cb);
        }
    }

    /// Register under `module.name` (and bare `name`).
    pub fn bind_fn_rust<A, R, F>(&self, module: &str, name: &str, f: F)
    where
        F: IntoSuperFn<A, R>,
    {
        if module.is_empty() {
            self.register(name, f);
        } else {
            self.register(format!("{module}.{name}"), f);
        }
    }

    /// Bind a C trampoline with a signature descriptor (`"f32,f32->void"`).
    pub fn bind_native(
        &mut self,
        module: &str,
        name: &str,
        fn_ptr: NativeFn,
        user_data: *mut core::ffi::c_void,
        sig_descriptor: &str,
    ) {
        let key = if module.is_empty() {
            name.to_string()
        } else {
            format!("{module}.{name}")
        };
        let sig = SigDesc::parse(sig_descriptor);
        let natives = Arc::clone(&self.natives);
        let key_for_closure = key.clone();
        let callback: HostCallback = Arc::new(move |args: &[Value]| {
            let natives = natives.lock().unwrap();
            let Some(binding) = natives.get(&key_for_closure) else {
                return Value::Null;
            };
            let packed = abi::pack_args(&binding.sig, args);
            let mut ret_buf = vec![0u8; binding.sig.ret_bytes().max(1)];
            // SAFETY: trampoline and buffers are provided/owned by the host binding.
            unsafe {
                (binding.fn_ptr)(
                    binding.user_data,
                    packed.as_ptr() as *mut core::ffi::c_void,
                    ret_buf.as_mut_ptr() as *mut core::ffi::c_void,
                );
                abi::unpack_ret(&binding.sig, ret_buf.as_ptr())
            }
        });
        {
            let mut host = self.host.lock().unwrap();
            host.register_fn(key.clone(), Arc::clone(&callback));
            host.register_fn(name.to_string(), callback);
        }
        self.natives.lock().unwrap().insert(
            key,
            NativeBinding {
                fn_ptr,
                user_data,
                sig,
            },
        );
    }

    /// Zero-copy share of a host array. `layout` is `"x:f32,y:f32,..."` or empty to
    /// infer from a loaded Super struct whose size matches `elem_size`.
    pub fn bind_slice(
        &mut self,
        name: &str,
        ptr: *mut core::ffi::c_void,
        len: usize,
        elem_size: usize,
        layout: Option<&str>,
    ) {
        let fields = if let Some(desc) = layout.filter(|s| !s.is_empty()) {
            let (f, _sz) = parse_layout_descriptor(desc);
            Arc::new(f)
        } else if let Some((f, _)) = self.slice_layouts.get(name) {
            Arc::clone(f)
        } else {
            Arc::new(self.infer_layout(elem_size).unwrap_or_default())
        };
        self.slice_layouts
            .insert(name.to_string(), (Arc::clone(&fields), elem_size));
        let binding = ExtSliceBinding {
            ptr: ptr as usize,
            len,
            elem_size,
            fields,
        };
        self.host.lock().unwrap().bind_slice(name, binding);
    }

    /// Typed systems peer helper: share a mutable slice of `repr(C)` elements.
    pub fn bind_slice_mut<T>(&mut self, name: &str, data: &mut [T], layout: &str) {
        self.bind_slice(
            name,
            data.as_mut_ptr() as *mut core::ffi::c_void,
            data.len(),
            std::mem::size_of::<T>(),
            Some(layout),
        );
    }

    fn infer_layout(&self, elem_size: usize) -> Option<Vec<ExtField>> {
        let loaded = self.loaded.as_ref()?;
        for tid in loaded.typed.structs.values() {
            if let TypeKind::Struct { fields, size, .. } = loaded.typed.types.get_type(*tid)
                && *size as usize == elem_size
            {
                let mut out = Vec::new();
                for f in fields {
                    let kind = match loaded.typed.types.get_type(f.ty) {
                        TypeKind::Bool => ExtKind::Bool,
                        TypeKind::U8 => ExtKind::U8,
                        TypeKind::U16 => ExtKind::U16,
                        TypeKind::U32 => ExtKind::U32,
                        TypeKind::U64 => ExtKind::U64,
                        TypeKind::I8 => ExtKind::I8,
                        TypeKind::I16 => ExtKind::I16,
                        TypeKind::I32 => ExtKind::I32,
                        TypeKind::I128 => ExtKind::I128,
                        TypeKind::F32 => ExtKind::F32,
                        TypeKind::F64 => ExtKind::F64,
                        TypeKind::I64 => ExtKind::I64,
                        _ => ExtKind::I64,
                    };
                    let sz = match kind {
                        ExtKind::Bool | ExtKind::U8 | ExtKind::I8 => 1,
                        ExtKind::U16 | ExtKind::I16 => 2,
                        ExtKind::F32 | ExtKind::I32 | ExtKind::U32 => 4,
                        ExtKind::U64 | ExtKind::I64 | ExtKind::Ptr | ExtKind::F64 => 8,
                        ExtKind::I128 => 16,
                    };
                    out.push(ExtField {
                        name: loaded.interner.resolve(f.name).to_string(),
                        offset: f.offset,
                        size: sz,
                        kind,
                    });
                }
                return Some(out);
            }
        }
        None
    }

    pub fn load_file(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        let mut result = analyze_project(path);
        if result.diagnostics.has_errors() {
            let msg = result
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            self.set_error(msg.clone());
            return Err(msg);
        }
        let base = result.entry_dir.clone();
        let chunk = Compiler::with_base(&mut result.interner, &result.typed, false, &base)
            .compile_embed(&result.module);
        // Run init to populate globals.
        let init_err = {
            let host = Arc::new(self.host.lock().unwrap().clone());
            let mut vm = Vm::new(chunk.clone(), &mut self.stacks, &result.interner).with_host(host);
            vm.run_init().err().map(|e| format!("{e:?}"))
        };
        if let Some(msg) = init_err {
            self.set_error(msg.clone());
            return Err(msg);
        }
        let module_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("main")
            .to_string();
        self.loaded = Some(Loaded {
            path: path.to_path_buf(),
            module: result.module,
            typed: result.typed,
            interner: result.interner,
            chunk,
            module_name,
        });
        self.last_error.clear();
        Ok(())
    }

    pub fn reload_module(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        // Keep stacks (hot-reload); replace code.
        self.load_file(path)
    }

    pub fn eval(&mut self, path: &str, source: &str) -> VmResult<Value> {
        use super_core::SourceFile;
        use super_frontend::analyze;
        let file = SourceFile::new(path, source);
        let result = analyze(&file);
        if result.diagnostics.has_errors() {
            let msg = result
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(VmError::Runtime(msg));
        }
        let mut interner = result.interner;
        let chunk =
            Compiler::new(&mut interner, &result.typed, false).compile_embed(&result.module);
        let host = Arc::new(self.host.lock().unwrap().clone());
        let mut vm = Vm::new(chunk, &mut self.stacks, &interner).with_host(host);
        vm.run()
    }

    /// Call `module::fn_name` (module may be empty / match loaded stem).
    pub fn call_values(
        &mut self,
        module: &str,
        fn_name: &str,
        args: Vec<Value>,
    ) -> VmResult<Value> {
        let loaded = self
            .loaded
            .as_mut()
            .ok_or_else(|| VmError::Runtime("no module loaded".into()))?;
        // Module label is informational for C hosts; functions live in the loaded chunk.
        if !module.is_empty() && module != loaded.module_name && module != "main" && module != "." {
            // Still allow cross-label calls — the chunk is the source of truth.
        }
        let host_env = self.host.lock().unwrap().clone();
        for name in host_env.slices.keys() {
            loaded.interner.intern(name);
        }
        let host = Arc::new(host_env);
        let mut vm = Vm::new(loaded.chunk.clone(), &mut self.stacks, &loaded.interner)
            .with_host(Arc::clone(&host));
        // Top-level bindings, then overwrite placeholders with zero-copy slices.
        let _ = vm.run_init();
        for (name, binding) in &host.slices {
            if let Some(sym) = loaded.interner.lookup(name) {
                vm.globals_mut().insert(sym, binding.as_value());
            }
        }
        vm.call_named(fn_name, args)
    }

    /// Direct typed invocation using C-ABI packed buffers.
    ///
    /// # Safety
    /// `args_buf` / `ret_buf` must match the function's C layout for the duration of the call.
    pub unsafe fn call_raw(
        &mut self,
        module: &str,
        fn_name: &str,
        args_buf: *mut core::ffi::c_void,
        ret_buf: *mut core::ffi::c_void,
        sig: &SigDesc,
    ) -> Result<(), String> {
        let args = unsafe { abi::read_args(sig, args_buf as *const u8) };
        match self.call_values(module, fn_name, args) {
            Ok(v) => {
                unsafe { abi::write_ret(sig, ret_buf as *mut u8, &v) };
                Ok(())
            }
            Err(e) => {
                let msg = format!("{e:?}");
                self.set_error(msg.clone());
                Err(msg)
            }
        }
    }

    /// Infer a call signature from the loaded typed function when possible.
    pub fn signature_for(&self, fn_name: &str) -> Option<SigDesc> {
        let loaded = self.loaded.as_ref()?;
        let sym = loaded.interner.lookup(fn_name)?;
        let info = loaded.typed.functions.get(&sym)?;
        let TypeKind::Func { params, ret } = loaded.typed.types.get_type(info.ty) else {
            return None;
        };
        let args = params
            .iter()
            .map(|p| type_to_slot(&loaded.typed, *p))
            .collect();
        let ret = type_to_slot(&loaded.typed, *ret);
        Some(SigDesc { args, ret })
    }

    pub fn loaded_path(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|l| l.path.as_path())
    }

    pub fn typed(&self) -> Option<&TypedModule> {
        self.loaded.as_ref().map(|l| &l.typed)
    }

    pub fn interner(&self) -> Option<&Interner> {
        self.loaded.as_ref().map(|l| &l.interner)
    }

    pub fn module_ast(&self) -> Option<&Module> {
        self.loaded.as_ref().map(|l| &l.module)
    }
}

fn type_to_slot(typed: &TypedModule, tid: super_core::TypeId) -> abi::SigSlot {
    match typed.types.get_type(tid) {
        TypeKind::Void | TypeKind::Never => abi::SigSlot::Void,
        TypeKind::Bool => abi::SigSlot::Bool,
        TypeKind::U8
        | TypeKind::I8
        | TypeKind::U16
        | TypeKind::I16
        | TypeKind::U32
        | TypeKind::I32 => abi::SigSlot::I32,
        TypeKind::F32 => abi::SigSlot::F32,
        TypeKind::F64 => abi::SigSlot::F64,
        TypeKind::U64 | TypeKind::I64 | TypeKind::I128 => abi::SigSlot::I64,
        TypeKind::Struct { size, .. } => abi::SigSlot::Struct {
            size: *size as usize,
        },
        _ => abi::SigSlot::I64,
    }
}
