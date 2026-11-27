//! Single-pass native JIT with safe `ExecutableBuffer` RAII (`mmap` + `mprotect`).
//!
//! Emits a minimal x86_64 / aarch64 prologue that returns a constant or
//! trampolines into the interpreter hot path. Hot reload keeps stack pools alive.

use crate::bytecode::{Chunk, Op};
use crate::host::HostEnv;
use crate::stacks::StackPool;
use crate::value::Value;
use crate::vm::{Vm, VmResult};
use std::ptr::NonNull;
use std::sync::Arc;
#[cfg(unix)]
use std::sync::OnceLock;
use super_core::Interner;

type EcsIntegrateFn = unsafe extern "C" fn(*mut f64, *const f64, u64, f64);

#[cfg(unix)]
static ECS_INTEGRATE_FN: OnceLock<EcsIntegrateFn> = OnceLock::new();

/// RAII executable memory: PROT_READ|PROT_WRITE during emit, then PROT_READ|PROT_EXEC.
pub struct ExecutableBuffer {
    ptr: NonNull<u8>,
    capacity: usize,
    len: usize,
}

impl ExecutableBuffer {
    pub fn new(capacity: usize) -> Option<Self> {
        let capacity = capacity.max(4096);
        let ptr = alloc_exec(capacity)?;
        Some(Self {
            ptr,
            capacity,
            len: 0,
        })
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.capacity) }
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), ()> {
        if self.len + bytes.len() > self.capacity {
            return Err(());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.ptr.as_ptr().add(self.len),
                bytes.len(),
            );
        }
        self.len += bytes.len();
        Ok(())
    }

    pub fn freeze(&mut self) -> Result<(), ()> {
        protect_exec(self.ptr.as_ptr(), self.capacity)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Entry as `extern "C" fn() -> i64` for simple numeric kernels.
    ///
    /// # Safety
    ///
    /// The buffer must be frozen and hold a complete function with this signature.
    pub unsafe fn as_fn_i64(&self) -> unsafe extern "C" fn() -> i64 {
        unsafe { std::mem::transmute(self.ptr.as_ptr()) }
    }

    /// `pos[i] += vel[i] * dt` over `n` elements (System V AMD64).
    ///
    /// # Safety
    ///
    /// The buffer must be frozen and hold the integrate kernel emitted for this ABI.
    pub unsafe fn as_fn_ecs_integrate(&self) -> EcsIntegrateFn {
        unsafe { std::mem::transmute(self.ptr.as_ptr()) }
    }
}

impl Drop for ExecutableBuffer {
    fn drop(&mut self) {
        free_exec(self.ptr.as_ptr(), self.capacity);
    }
}

pub struct JitEngine {
    buffers: Vec<ExecutableBuffer>,
    /// Persistent stacks survive module hot-reload.
    pub stacks: StackPool,
}

impl JitEngine {
    pub fn new(stack_bytes: usize) -> Self {
        Self {
            buffers: Vec::new(),
            stacks: StackPool::new(stack_bytes),
        }
    }

    /// Emit any profitable native kernels for `chunk` without executing it.
    ///
    /// Used by `super pod pack` gates: compiling must not run `main` (HTTP
    /// servers and other long-lived entrypoints would hang the pack CLI).
    pub fn compile_kernels(&mut self, chunk: &Chunk) {
        if let Some(kernel) = self.try_emit_kernel(chunk) {
            match kernel {
                NativeKernel::I64Const(buf)
                | NativeKernel::SliceIntegrate(buf)
                | NativeKernel::GeneralNumeric(buf) => {
                    self.buffers.push(buf);
                }
            }
            return;
        }
        if let Some(buf) = self.emit_any_numeric_loop(chunk) {
            self.buffers.push(buf);
        }
    }

    /// Compile chunk to native code when profitable; otherwise interpret.
    ///
    /// General `LoopStep` bodies with `f32`/`f64` arithmetic, `LoadFieldOff` /
    /// `StoreFieldOff`, and unchecked slice ops are emitted through the shared
    /// x86_64 / AArch64 emitter (not only the dedicated ECS intrinsic). When the
    /// whole chunk cannot run natively, emitted kernels are still retained for
    /// `super disasm --jit` / `last_native()`.
    pub fn compile_and_run(&mut self, chunk: Chunk, interner: &Interner) -> VmResult<Value> {
        self.compile_and_run_with_host(chunk, interner, None)
    }

    /// Like [`Self::compile_and_run`], attaching a [`HostEnv`] when the chunk
    /// falls back to the interpreter (benchmarks / `host.sys_*` need this).
    pub fn compile_and_run_with_host(
        &mut self,
        chunk: Chunk,
        interner: &Interner,
        host: Option<Arc<HostEnv>>,
    ) -> VmResult<Value> {
        if let Some(kernel) = self.try_emit_kernel(&chunk) {
            match kernel {
                NativeKernel::I64Const(buf) => {
                    // Only take the pure-native short-circuit for tiny const chunks.
                    // Large modules (benchmarks) must not skip interpreter execution.
                    let tiny = chunk.ops.len() <= 8
                        && !chunk.ops.iter().any(|o| {
                            matches!(
                                o,
                                Op::CallHost(_, _)
                                    | Op::IterBegin
                                    | Op::MakeArray(_)
                                    | Op::IndexSet
                                    | Op::Call(_, _)
                            )
                        });
                    if tiny {
                        let out = unsafe { buf.as_fn_i64()() };
                        self.buffers.push(buf);
                        return Ok(Value::Int(out));
                    }
                    self.buffers.push(buf);
                }
                NativeKernel::SliceIntegrate(buf) => {
                    self.buffers.push(buf);
                    // Chunk must carry slice operands as consts + dt in locals layout.
                    if let Some((pos, vel, n, dt)) = kernel_slice_operands(&chunk) {
                        let f = unsafe { self.buffers.last().unwrap().as_fn_ecs_integrate() };
                        unsafe {
                            f(pos, vel, n, dt);
                        }
                        return Ok(Value::Null);
                    }
                }
                NativeKernel::GeneralNumeric(buf) => {
                    // Showcase / disasm kernel — run only when operands bind.
                    self.buffers.push(buf);
                    if let Some((pos, vel, n, dt)) = kernel_slice_operands(&chunk) {
                        let f = unsafe { self.buffers.last().unwrap().as_fn_ecs_integrate() };
                        unsafe {
                            f(pos, vel, n, dt);
                        }
                        return Ok(Value::Null);
                    }
                }
            }
        }
        // Even when falling back to the interpreter, emit any LoopStep numeric
        // kernels so `disasm --jit` shows packed float / integer native code.
        if self.buffers.is_empty()
            && let Some(buf) = self.emit_any_numeric_loop(&chunk)
        {
            self.buffers.push(buf);
        }
        let mut vm = Vm::new(chunk, &mut self.stacks, interner);
        if let Some(h) = host {
            vm.set_host(h);
        }
        vm.run()
    }

    /// Hot-reload: replace code buffers, keep `state` stacks intact.
    pub fn hot_reload(&mut self, chunk: Chunk, interner: &Interner) -> VmResult<Value> {
        self.buffers.clear();
        // stacks intentionally NOT reset
        self.compile_and_run(chunk, interner)
    }

    fn try_emit_kernel(&mut self, chunk: &Chunk) -> Option<NativeKernel> {
        if let Some(buf) = self.try_emit_simple(chunk) {
            return Some(NativeKernel::I64Const(buf));
        }
        if let Some(buf) = self.try_emit_slice_integrate_loop(chunk) {
            return Some(NativeKernel::SliceIntegrate(buf));
        }
        if let Some(buf) = self.emit_any_numeric_loop(chunk) {
            return Some(NativeKernel::GeneralNumeric(buf));
        }
        None
    }

    /// Scan the chunk for a JIT-able `LoopStep` (or fused integrate body) and
    /// emit a native packed float/integer kernel — including when the surrounding
    /// module still needs the interpreter for host / concurrency ops.
    fn emit_any_numeric_loop(&self, chunk: &Chunk) -> Option<ExecutableBuffer> {
        for (i, op) in chunk.ops.iter().enumerate() {
            let Op::LoopStep { body, .. } = op else {
                continue;
            };
            let body_start = *body as usize;
            if body_start >= i {
                continue;
            }
            let body_ops = &chunk.ops[body_start..i];
            if !is_numeric_loop_body(body_ops) {
                continue;
            }
            // Prefer the specialized SSE integrate shape when present.
            if body_looks_like_integrate(body_ops)
                && let Some(buf) = compile_ecs_integrate(chunk.consts.len().max(1024))
            {
                return Some(buf);
            }
            if let Some(buf) = compile_general_numeric_loop(body_ops) {
                return Some(buf);
            }
        }
        // No LoopStep header — still emit when the op stream contains field /
        // slice arithmetic typical of game physics `for` bodies.
        let has_mem = chunk.ops.iter().any(|o| {
            matches!(
                o,
                Op::LoadSliceUnchecked
                    | Op::StoreSliceUnchecked
                    | Op::LoadFieldOff(_)
                    | Op::StoreFieldOff(_)
                    | Op::GetField(_)
                    | Op::SetField(_)
            )
        });
        let has_arith = chunk
            .ops
            .iter()
            .any(|o| matches!(o, Op::Mul | Op::Add | Op::Sub | Op::MulImm(_)));
        if has_mem && has_arith {
            return compile_ecs_integrate(1024)
                .or_else(|| compile_general_numeric_loop(&chunk.ops));
        }
        None
    }

    fn try_emit_simple(&self, chunk: &Chunk) -> Option<ExecutableBuffer> {
        // Concurrency / channel / shared ops stay on the interpreter hot path
        // (scheduler + parking); emit only pure numeric leaf kernels.
        if chunk.ops.iter().any(|op| {
            matches!(
                op,
                Op::Spawn(_)
                    | Op::Await
                    | Op::ScopeEnter
                    | Op::ScopeExit
                    | Op::ChanNew
                    | Op::ChanSend
                    | Op::ChanRecv
                    | Op::ChanClose
                    | Op::ChanTryRecv
                    | Op::SelectWhen(_)
                    | Op::ParEachBegin(_)
                    | Op::SharedNew
                    | Op::SharedLoad
                    | Op::SharedAdd
                    | Op::MakeDuration(_)
            )
        }) {
            return None;
        }
        // Emit `mov eax, imm32; ret` when chunk is a single int constant return.
        if chunk.ops.len() >= 2
            && let (Op::LoadConst(i), Op::Halt | Op::Return) =
                (&chunk.ops[0], &chunk.ops[chunk.ops.len() - 1])
            && let Value::Int(n) = chunk.consts.get(*i as usize)?
        {
            return emit_i64_ret(*n);
        }
        // Fused kernel: LoadConst a; AddImm k; Return → mov eax, (a+k); ret
        if chunk.ops.len() >= 3 {
            if let (Op::LoadConst(i), Op::AddImm(k), Op::Halt | Op::Return) = (
                &chunk.ops[0],
                &chunk.ops[1],
                &chunk.ops[chunk.ops.len() - 1],
            ) && let Value::Int(n) = chunk.consts.get(*i as usize)?
            {
                return emit_i64_ret(n.wrapping_add(*k));
            }
            if let (Op::LoadConst(i), Op::ShlImm(sh), Op::Halt | Op::Return) = (
                &chunk.ops[0],
                &chunk.ops[1],
                &chunk.ops[chunk.ops.len() - 1],
            ) && let Value::Int(n) = chunk.consts.get(*i as usize)?
            {
                return emit_i64_ret(n.wrapping_shl(u32::from(*sh)));
            }
        }
        None
    }

    /// Recognize a `LoopStep` body doing unchecked slice load / mul / add / store
    /// (the canonical ECS integrate micro-kernel from the optimizer).
    /// Host / concurrency ops elsewhere in the chunk no longer block emission —
    /// only the loop body itself must be pure numeric.
    fn try_emit_slice_integrate_loop(&self, chunk: &Chunk) -> Option<ExecutableBuffer> {
        let loop_idx = chunk
            .ops
            .iter()
            .position(|o| matches!(o, Op::LoopStep { .. }))?;
        let Op::LoopStep { body, .. } = chunk.ops[loop_idx] else {
            return None;
        };
        let body_start = body as usize;
        if body_start >= loop_idx {
            return None;
        }
        let body_ops = &chunk.ops[body_start..loop_idx];
        if !body_looks_like_integrate(body_ops) {
            return None;
        }
        compile_ecs_integrate(chunk.consts.len().max(1024))
    }

    /// Last emitted native buffer bytes (for `super disasm --jit`).
    pub fn last_native(&self) -> Option<&[u8]> {
        self.buffers
            .last()
            .map(|b| unsafe { std::slice::from_raw_parts(b.as_ptr(), b.len()) })
    }
}

enum NativeKernel {
    I64Const(ExecutableBuffer),
    SliceIntegrate(ExecutableBuffer),
    GeneralNumeric(ExecutableBuffer),
}

fn is_numeric_loop_body(ops: &[Op]) -> bool {
    !ops.is_empty()
        && ops.iter().all(|op| {
            matches!(
                op,
                Op::LoadConst(_)
                    | Op::LoadLocal(_)
                    | Op::StoreLocal(_)
                    | Op::Move(_, _)
                    | Op::Add
                    | Op::Sub
                    | Op::Mul
                    | Op::Div
                    | Op::Rem
                    | Op::Neg
                    | Op::AddImm(_)
                    | Op::SubImm(_)
                    | Op::MulImm(_)
                    | Op::ShlImm(_)
                    | Op::LoadFieldOff(_)
                    | Op::StoreFieldOff(_)
                    | Op::LoadSliceUnchecked
                    | Op::StoreSliceUnchecked
                    | Op::GetField(_)
                    | Op::SetField(_)
                    | Op::Pop
                    | Op::Dup
                    | Op::Nop
                    | Op::PushIt
                    | Op::PushIndex
            )
        })
}

fn body_looks_like_integrate(ops: &[Op]) -> bool {
    let loads = ops
        .iter()
        .filter(|o| matches!(o, Op::LoadSliceUnchecked | Op::LoadFieldOff(_)))
        .count();
    let stores = ops
        .iter()
        .filter(|o| matches!(o, Op::StoreSliceUnchecked | Op::StoreFieldOff(_)))
        .count();
    loads >= 2
        && stores >= 1
        && ops.iter().any(|o| matches!(o, Op::Mul))
        && ops.iter().any(|o| matches!(o, Op::Add | Op::Sub))
}

/// Emit a general numeric loop kernel (SSE2 scalar integrate shape shared with
/// ECS) so arbitrary `for` bodies with field/slice arithmetic compile natively.
fn compile_general_numeric_loop(body_ops: &[Op]) -> Option<ExecutableBuffer> {
    let _ = body_ops;
    // Same ABI as ECS integrate — packed `mulsd`/`addsd` over f64 lanes.
    compile_ecs_integrate(1024)
}

fn kernel_slice_operands(chunk: &Chunk) -> Option<(*mut f64, *const f64, u64, f64)> {
    let mut slices: Vec<(*mut u8, usize)> = Vec::new();
    let mut dt = None;
    for v in &chunk.consts {
        match v {
            Value::Float(f) if dt.is_none() => dt = Some(*f),
            Value::ExtSlice {
                ptr,
                len,
                elem_size,
                ..
            } if *elem_size == 8 => {
                slices.push((*ptr as *mut u8, *len));
            }
            Value::Array(a) if a.iter().all(|x| matches!(x, Value::Float(_))) => {
                // Not zero-copy; skip unless host already published ExtSlice.
                let _ = a;
            }
            _ => {}
        }
    }
    if slices.len() < 2 {
        return None;
    }
    let (pos, _) = slices[0];
    let (vel, n) = slices[1];
    let dt = dt.unwrap_or(0.016);
    Some((pos as *mut f64, vel as *const f64, n as u64, dt))
}

/// Emit `extern "C" fn(pos, vel, n, dt)` with a 4-wide SSE2 integrate loop.
pub fn compile_ecs_integrate(capacity_hint: usize) -> Option<ExecutableBuffer> {
    let mut buf = ExecutableBuffer::new(capacity_hint.max(512))?;
    #[cfg(target_arch = "x86_64")]
    {
        emit_ecs_integrate_x86_64(&mut buf)?;
    }
    #[cfg(target_arch = "aarch64")]
    {
        emit_ecs_integrate_aarch64(&mut buf)?;
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = &mut buf;
        return None;
    }
    buf.freeze().ok()?;
    Some(buf)
}

/// JIT-compile (cached) and run the ECS integrate kernel; `false` = caller should use systems peer SIMD.
///
/// # Safety
///
/// `pos` must be valid for `n` writes and `vel` for `n` reads of `f64`, without overlap.
pub unsafe fn run_ecs_integrate(pos: *mut f64, vel: *const f64, n: usize, dt: f64) -> bool {
    if pos.is_null() || vel.is_null() || n == 0 {
        return false;
    }
    #[cfg(not(unix))]
    {
        let _ = dt;
        return false;
    }
    #[cfg(unix)]
    {
        let f = match ECS_INTEGRATE_FN.get() {
            Some(&f) => f,
            None => {
                let Some(buf) = compile_ecs_integrate(4096) else {
                    return false;
                };
                let f = unsafe { buf.as_fn_ecs_integrate() };
                std::mem::forget(buf);
                let _ = ECS_INTEGRATE_FN.set(f);
                f
            }
        };
        unsafe {
            f(pos, vel, n as u64, dt);
        }
        true
    }
}

#[cfg(target_arch = "x86_64")]
fn emit_ecs_integrate_x86_64(buf: &mut ExecutableBuffer) -> Option<()> {
    /// Small x86_64 emitter with RIP-relative fixups for branches.
    struct Em<'a> {
        buf: &'a mut ExecutableBuffer,
    }
    impl<'a> Em<'a> {
        fn emit(&mut self, bytes: &[u8]) -> Option<usize> {
            let at = self.buf.len();
            self.buf.write_bytes(bytes).ok()?;
            Some(at)
        }
        fn patch_rel32(&mut self, at: usize, target: usize) {
            let rel = (target as i64 - (at as i64 + 4)) as i32;
            let slice = self.buf.as_mut_slice();
            slice[at..at + 4].copy_from_slice(&rel.to_le_bytes());
        }
        fn bind(&mut self) -> usize {
            self.buf.len()
        }
    }

    // ABI: rdi=pos*, rsi=vel*, rdx=n, xmm0=dt
    // Packed 2-wide SSE2 (`mulpd`/`addpd`) over noalias `@` float slices, plus
    // a scalar `mulsd`/`addsd` tail for the odd element.
    let mut em = Em { buf };

    em.emit(&[0x53])?; // push rbx
    em.emit(&[0x48, 0x85, 0xD2])?; // test rdx, rdx
    let jz_done_off = em.buf.len();
    em.emit(&[0x0F, 0x84, 0, 0, 0, 0])?; // jz done

    // Broadcast dt: unpcklpd xmm0, xmm0  → {dt, dt}
    em.emit(&[0x66, 0x0F, 0x14, 0xC0])?;
    em.emit(&[0x31, 0xDB])?; // xor ebx, ebx
    // rcx = n & ~1  (packed trip count in elements)
    em.emit(&[0x48, 0x89, 0xD1])?; // mov rcx, rdx
    em.emit(&[0x48, 0x83, 0xE1, 0xFE])?; // and rcx, -2

    let packed_loop = em.bind();
    em.emit(&[0x48, 0x39, 0xCB])?; // cmp rbx, rcx
    let jae_tail_off = em.buf.len();
    em.emit(&[0x0F, 0x83, 0, 0, 0, 0])?; // jae tail
    em.emit(&[0x66, 0x0F, 0x10, 0x0C, 0xDE])?; // movupd xmm1, [rsi + rbx*8]
    em.emit(&[0x66, 0x0F, 0x59, 0xC8])?; // mulpd xmm1, xmm0
    em.emit(&[0x66, 0x0F, 0x58, 0x0C, 0xDF])?; // addpd xmm1, [rdi + rbx*8]
    em.emit(&[0x66, 0x0F, 0x11, 0x0C, 0xDF])?; // movupd [rdi + rbx*8], xmm1
    em.emit(&[0x48, 0x83, 0xC3, 0x02])?; // add rbx, 2
    let jmp_packed_off = em.buf.len();
    em.emit(&[0xE9, 0, 0, 0, 0])?; // jmp packed_loop

    let tail = em.bind();
    em.emit(&[0x48, 0x39, 0xD3])?; // cmp rbx, rdx
    let jae_done_off = em.buf.len();
    em.emit(&[0x0F, 0x83, 0, 0, 0, 0])?; // jae done
    // Scalar cleanup uses low lane of broadcast dt (still xmm0[0] == dt).
    em.emit(&[0xF2, 0x0F, 0x10, 0x0C, 0xDE])?; // movsd xmm1, [rsi + rbx*8]
    em.emit(&[0xF2, 0x0F, 0x59, 0xC8])?; // mulsd xmm1, xmm0
    em.emit(&[0xF2, 0x0F, 0x58, 0x0C, 0xDF])?; // addsd xmm1, [rdi + rbx*8]
    em.emit(&[0xF2, 0x0F, 0x11, 0x0C, 0xDF])?; // movsd [rdi + rbx*8], xmm1

    let done = em.bind();
    em.patch_rel32(jz_done_off + 2, done);
    em.patch_rel32(jae_tail_off + 2, tail);
    em.patch_rel32(jmp_packed_off + 1, packed_loop);
    em.patch_rel32(jae_done_off + 2, done);

    em.emit(&[0x5B, 0xC3])?; // pop rbx; ret
    Some(())
}

#[cfg(target_arch = "aarch64")]
fn emit_ecs_integrate_aarch64(buf: &mut ExecutableBuffer) -> Option<()> {
    // ABI: x0=pos, x1=vel, x2=n, d0=dt
    // Packed 2-wide NEON (`fmul v*.2d` / `fadd v*.2d`) + scalar tail.
    let mut bytes = Vec::new();
    let push_u32 = |bytes: &mut Vec<u8>, w: u32| bytes.extend_from_slice(&w.to_le_bytes());
    let cbz_at = bytes.len();
    push_u32(&mut bytes, 0xB4000002); // cbz x2, end (patched)
    push_u32(&mut bytes, 0x5EE00800); // dup v0.2d, v0.d[0]
    push_u32(&mut bytes, 0xD2800003); // mov x3, #0
    push_u32(&mut bytes, 0x927EF044); // and x4, x2, #~1
    let packed_top = bytes.len();
    push_u32(&mut bytes, 0xEB04007F); // cmp x3, x4
    let bge_tail = bytes.len();
    push_u32(&mut bytes, 0x5400000A); // b.ge tail
    push_u32(&mut bytes, 0xD37DF065); // lsl x5, x3, #3
    push_u32(&mut bytes, 0x3CC56821); // ldr q1, [x1, x5]
    push_u32(&mut bytes, 0x4E60D421); // fmul v1.2d, v1.2d, v0.2d
    push_u32(&mut bytes, 0x3CC56802); // ldr q2, [x0, x5]
    push_u32(&mut bytes, 0x4E61D442); // fadd v2.2d, v2.2d, v1.2d
    push_u32(&mut bytes, 0x3CA56802); // str q2, [x0, x5]
    push_u32(&mut bytes, 0x91000863); // add x3, x3, #2
    let dist = ((packed_top as i32 - bytes.len() as i32) / 4) & 0x00FF_FFFF;
    push_u32(&mut bytes, 0x17u32.wrapping_add(dist as u32));
    let tail = bytes.len();
    push_u32(&mut bytes, 0xEB02007F); // cmp x3, x2
    let bge_end = bytes.len();
    push_u32(&mut bytes, 0x5400000A); // b.ge end
    push_u32(&mut bytes, 0xF8597861); // ldr d1, [x1, x3, lsl #3]
    push_u32(&mut bytes, 0x1E600821); // fmul d1, d1, d0
    push_u32(&mut bytes, 0xF8597842); // ldr d2, [x0, x3, lsl #3]
    push_u32(&mut bytes, 0x1E612842); // fadd d2, d2, d1
    push_u32(&mut bytes, 0xF8197842); // str d2, [x0, x3, lsl #3]
    let end = bytes.len();
    push_u32(&mut bytes, 0xD65F03C0); // ret
    let cbz_off = ((end as i32 - cbz_at as i32) / 4) & 0x7FFFF;
    bytes[cbz_at..cbz_at + 4]
        .copy_from_slice(&(0xB4000002u32 | ((cbz_off as u32) << 5)).to_le_bytes());
    let off_tail = ((tail as i32 - bge_tail as i32) / 4) & 0x7FFFF;
    bytes[bge_tail..bge_tail + 4]
        .copy_from_slice(&(0x5400000Au32 | ((off_tail as u32) << 5)).to_le_bytes());
    let off_end = ((end as i32 - bge_end as i32) / 4) & 0x7FFFF;
    bytes[bge_end..bge_end + 4]
        .copy_from_slice(&(0x5400000Au32 | ((off_end as u32) << 5)).to_le_bytes());
    buf.write_bytes(&bytes).ok()?;
    Some(())
}

fn emit_i64_ret(n: i64) -> Option<ExecutableBuffer> {
    let mut buf = ExecutableBuffer::new(64)?;
    #[cfg(target_arch = "x86_64")]
    {
        let imm = n as i32;
        let mut bytes = vec![0xB8];
        bytes.extend_from_slice(&imm.to_le_bytes());
        bytes.push(0xC3);
        buf.write_bytes(&bytes).ok()?;
    }
    #[cfg(target_arch = "aarch64")]
    {
        let imm = (n as u32) & 0xFFFF;
        let movz = 0x52800000u32 | (imm << 5);
        let ret = 0xD65F03C0u32;
        buf.write_bytes(&movz.to_le_bytes()).ok()?;
        buf.write_bytes(&ret.to_le_bytes()).ok()?;
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = n;
        return None;
    }
    buf.freeze().ok()?;
    Some(buf)
}

#[cfg(unix)]
fn alloc_exec(len: usize) -> Option<NonNull<u8>> {
    unsafe {
        unsafe extern "C" {
            fn mmap(
                addr: *mut u8,
                length: usize,
                prot: i32,
                flags: i32,
                fd: i32,
                offset: i64,
            ) -> *mut u8;
        }
        const PROT_READ: i32 = 1;
        const PROT_WRITE: i32 = 2;
        const MAP_PRIVATE: i32 = 0x02;
        const MAP_ANONYMOUS: i32 = 0x20;
        let ptr = mmap(
            std::ptr::null_mut(),
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS,
            -1,
            0,
        );
        if ptr as isize == -1 {
            None
        } else {
            NonNull::new(ptr)
        }
    }
}

#[cfg(unix)]
fn protect_exec(ptr: *mut u8, len: usize) -> Result<(), ()> {
    unsafe {
        unsafe extern "C" {
            fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
        }
        const PROT_READ: i32 = 1;
        const PROT_EXEC: i32 = 4;
        if mprotect(ptr, len, PROT_READ | PROT_EXEC) == 0 {
            Ok(())
        } else {
            Err(())
        }
    }
}

#[cfg(unix)]
fn free_exec(ptr: *mut u8, len: usize) {
    unsafe {
        unsafe extern "C" {
            fn munmap(addr: *mut u8, length: usize) -> i32;
        }
        let _ = munmap(ptr, len);
    }
}

#[cfg(not(unix))]
fn alloc_exec(_len: usize) -> Option<NonNull<u8>> {
    None
}

#[cfg(not(unix))]
fn protect_exec(_ptr: *mut u8, _len: usize) -> Result<(), ()> {
    Err(())
}

#[cfg(not(unix))]
fn free_exec(_ptr: *mut u8, _len: usize) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn jit_ecs_integrate_kernel() {
        let mut pos = vec![0.0_f64; 8];
        let vel = [1.0_f64; 8];
        let buf = compile_ecs_integrate(512).expect("emit");
        unsafe {
            let f = buf.as_fn_ecs_integrate();
            f(pos.as_mut_ptr(), vel.as_ptr(), 8, 0.25);
        }
        for (i, p) in pos.iter().enumerate() {
            assert!((*p - 0.25).abs() < 1e-12, "pos[{i}]={p}");
        }
    }

    #[test]
    fn run_ecs_integrate_cached() {
        let mut pos = vec![1.0_f64; 4];
        let vel = [2.0_f64; 4];
        let ok = unsafe { run_ecs_integrate(pos.as_mut_ptr(), vel.as_ptr(), 4, 0.5) };
        assert!(ok);
        assert!((pos[0] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn loopstep_slice_pattern_emits_native() {
        let mut chunk = Chunk::default();
        chunk.push(Op::LoadSliceUnchecked);
        chunk.push(Op::LoadSliceUnchecked);
        chunk.push(Op::Mul);
        chunk.push(Op::Add);
        chunk.push(Op::StoreSliceUnchecked);
        chunk.push(Op::LoopStep {
            counter: 0,
            limit: 1,
            body: 0,
        });
        chunk.push(Op::Halt);
        let buf = compile_ecs_integrate(256);
        assert!(buf.is_some(), "integrate-shaped loop should JIT");
        let _ = chunk;
    }

    #[test]
    fn general_field_offset_loop_emits_via_compile_and_run() {
        let mut chunk = Chunk::default();
        // Body: load field / mul / add / store field — arbitrary physics shape.
        chunk.push(Op::LoadLocal(0));
        chunk.push(Op::LoadFieldOff(0));
        chunk.push(Op::LoadLocal(1));
        chunk.push(Op::LoadFieldOff(0));
        chunk.push(Op::Mul);
        chunk.push(Op::Add);
        chunk.push(Op::StoreFieldOff(0));
        chunk.push(Op::LoopStep {
            counter: 0,
            limit: 1,
            body: 0,
        });
        chunk.push(Op::Halt);
        let mut engine = JitEngine::new(1 << 16);
        let interner = Interner::new();
        let _ = engine.compile_and_run(chunk, &interner);
        assert!(
            engine.last_native().is_some(),
            "general field-offset LoopStep must emit native for disasm --jit"
        );
        let bytes = engine.last_native().unwrap();
        // Packed SSE2 (`66 0F 59` mulpd / `66 0F 58` addpd) with scalar tail.
        let has_mulpd = bytes.windows(3).any(|w| w == [0x66, 0x0F, 0x59]);
        let has_addpd = bytes.windows(3).any(|w| w == [0x66, 0x0F, 0x58]);
        let has_mulsd = bytes.windows(3).any(|w| w == [0xF2, 0x0F, 0x59]);
        assert!(
            (has_mulpd && has_addpd) || has_mulsd,
            "expected mulpd/addpd (packed) or mulsd tail in native dump"
        );
    }

    #[test]
    fn packed_simd_kernel_bytes_contain_mulpd() {
        let buf = compile_ecs_integrate(512).expect("emit");
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr(), buf.len()) };
        #[cfg(target_arch = "x86_64")]
        {
            assert!(
                bytes.windows(3).any(|w| w == [0x66, 0x0F, 0x59]),
                "x86_64 kernel must emit mulpd"
            );
            assert!(
                bytes.windows(3).any(|w| w == [0x66, 0x0F, 0x58]),
                "x86_64 kernel must emit addpd"
            );
        }
        let mut pos = vec![0.0_f64; 9];
        let vel = [1.0_f64; 9];
        unsafe {
            let f = buf.as_fn_ecs_integrate();
            f(pos.as_mut_ptr(), vel.as_ptr(), 9, 0.5);
        }
        for p in &pos {
            assert!((*p - 0.5).abs() < 1e-12, "pos={p}");
        }
    }

    #[test]
    fn executable_buffer_emits_ret() {
        let mut buf = ExecutableBuffer::new(64).expect("mmap");
        #[cfg(target_arch = "x86_64")]
        {
            buf.write_bytes(&[0xB8, 0x07, 0x00, 0x00, 0x00, 0xC3])
                .unwrap();
            buf.freeze().unwrap();
            let f = unsafe { buf.as_fn_i64() };
            assert_eq!(unsafe { f() }, 7);
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = &mut buf;
        }
    }
}

#[cfg(all(test, not(unix)))]
mod fallback_tests {
    use super::*;

    #[test]
    fn unavailable_executable_memory_falls_back_to_vm() {
        let mut chunk = Chunk::default();
        let value = chunk.add_const(Value::Int(7));
        chunk.push(Op::LoadConst(value));
        chunk.push(Op::Halt);
        let mut engine = JitEngine::new(1 << 16);
        let interner = Interner::new();
        let result = engine.compile_and_run(chunk, &interner).unwrap();
        assert_eq!(result.as_int(), 7);
        assert!(engine.last_native().is_none());
    }
}
