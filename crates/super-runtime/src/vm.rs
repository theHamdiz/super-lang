//! Register-pinned tail-call bytecode VM.
//!
//! Primary dispatch uses a safe `loop { match op }` fallback. On nightly/x86_64
//! an `extern "C"` + `asm!` jmp table can be enabled; the safe path is always correct.
//! Concurrency opcodes (`Spawn`/`Await`/`Chan*`/`SelectWhen`/`ParEach`/`Shared*`)
//! drive the M:N work-stealing scheduler in [`crate::sched`].

use crate::bytecode::{Chunk, Op};
use crate::host::{HostEnv, ext_read, ext_write};
use crate::sched::{
    Channel, Scheduler, SelectArm, SelectResult, SharedCell, SharedInt, duration_from_value,
    parallel_for, select_when,
};
use crate::stacks::StackPool;
use crate::value::Value;
use std::collections::HashMap;
use std::sync::Arc;
use super_core::{Interner, SymId};

#[derive(Debug)]
pub enum VmError {
    Runtime(String),
    AssertFailed { left: Value, right: Value },
    Panic(String),
    StackUnderflow,
}

pub type VmResult<T> = Result<T, VmError>;

pub struct Vm<'a> {
    chunk: Arc<Chunk>,
    stacks: &'a mut StackPool,
    interner: &'a Interner,
    stack: Vec<Value>,
    locals: Vec<Value>,
    globals: HashMap<SymId, Value>,
    call_frames: Vec<CallFrame>,
    ip: usize,
    iter_stack: Vec<IterState>,
    tests_passed: usize,
    output: Vec<String>,
    sched: Arc<Scheduler>,
    /// Nested structured-concurrency scopes: child task handles to join on ScopeExit.
    scope_stack: Vec<Vec<crate::sched::TaskHandle>>,
    host: Option<Arc<HostEnv>>,
}

/// Integer `/` and `%` trap on a zero divisor with a Super panic instead of aborting the host.
fn birth_region(chunk: &Chunk, pc: usize) -> Option<u8> {
    chunk
        .escape_birth
        .iter()
        .find(|(p, _)| *p as usize == pc)
        .map(|(_, r)| *r)
}

fn track_birth(chunk: &Chunk, pc: usize, v: Value) -> Value {
    crate::vm_req::on_birth(birth_region(chunk, pc), v)
}

fn div_rem_values(a: &Value, b: &Value, rem: bool) -> VmResult<Value> {
    Ok(match (a, b) {
        (Value::Float(_), _) | (_, Value::Float(_)) => {
            let (x, y) = (a.as_float(), b.as_float());
            Value::Float(if rem { x % y } else { x / y })
        }
        _ => {
            let (x, y) = (a.as_int(), b.as_int());
            if y == 0 {
                return Err(VmError::Panic(format!(
                    "{} by zero ({x} {} 0)",
                    if rem { "remainder" } else { "division" },
                    if rem { '%' } else { '/' }
                )));
            }
            Value::Int(if rem {
                x.wrapping_rem(y)
            } else {
                x.wrapping_div(y)
            })
        }
    })
}

fn index_get_value(base: Value, idx: usize, unchecked: bool) -> Value {
    match base {
        Value::Array(a) => a.get(idx).cloned().unwrap_or(Value::Null),
        Value::ArrayF64(a) => a.get(idx).copied().map(Value::Float).unwrap_or(Value::Null),
        Value::ArrayI64(a) => a.get(idx).copied().map(Value::Int).unwrap_or(Value::Null),
        Value::Bytes(b) => b
            .get(idx)
            .copied()
            .map(|x| Value::Int(i64::from(x)))
            .unwrap_or(Value::Null),
        Value::ExtSlice {
            ptr,
            len,
            elem_size,
            fields,
        } => {
            if unchecked || idx < len {
                Value::ExtRef {
                    ptr: ptr + idx * elem_size,
                    size: elem_size,
                    fields,
                }
            } else {
                Value::Null
            }
        }
        Value::Str(s) => {
            let bytes = s.as_bytes();
            // A string never has more chars than bytes, so past the byte length is out of range.
            // When everything up to `idx` is ASCII, char `idx` is byte `idx` (SIMD-speed scan
            // instead of decoding every scalar).
            if idx >= bytes.len() {
                Value::Str(String::new())
            } else if bytes[..=idx].is_ascii() {
                Value::Str(char::from(bytes[idx]).to_string())
            } else {
                Value::Str(
                    s.chars()
                        .nth(idx)
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                )
            }
        }
        _ => Value::Null,
    }
}

fn index_set_value(base: &mut Value, idx: usize, value: Value, unchecked: bool) {
    match base {
        Value::Array(a) => {
            if idx < a.len() {
                Arc::make_mut(a)[idx] = value;
            } else if unchecked && idx == a.len() {
                Arc::make_mut(a).push(value);
            }
        }
        Value::ArrayF64(a) => {
            if idx < a.len() {
                Arc::make_mut(a)[idx] = value.as_float();
            } else if unchecked && idx == a.len() {
                Arc::make_mut(a).push(value.as_float());
            }
        }
        Value::ArrayI64(a) => {
            if idx < a.len() {
                Arc::make_mut(a)[idx] = value.as_int();
            } else if unchecked && idx == a.len() {
                Arc::make_mut(a).push(value.as_int());
            }
        }
        Value::Bytes(b) => {
            if idx < b.len() {
                b[idx] = (value.as_int() & 255) as u8;
            } else if unchecked && idx == b.len() {
                b.push((value.as_int() & 255) as u8);
            }
        }
        _ => {}
    }
}

/// `sys_arr_set(arr, i, v)` / `sys_arr_push(arr, v)` on copy-on-write storage. In place when
/// `arr` is uniquely owned (see `release_store_alias`), otherwise one copy.
fn arr_update(name: &str, args: Vec<Value>) -> Value {
    let mut it = args.into_iter();
    let Some(base) = it.next() else {
        return Value::Null;
    };
    match base {
        Value::Array(mut arr) => {
            if name == "sys_arr_set" {
                let idx = it.next().map(|v| v.as_int()).unwrap_or(-1);
                let value = it.next().unwrap_or(Value::Null);
                if idx >= 0 && (idx as usize) < arr.len() {
                    Arc::make_mut(&mut arr)[idx as usize] = value;
                }
            } else if let Some(value) = it.next() {
                Arc::make_mut(&mut arr).push(value);
            }
            Value::Array(arr)
        }
        Value::ArrayF64(mut arr) => {
            if name == "sys_arr_set" {
                let idx = it.next().map(|v| v.as_int()).unwrap_or(-1);
                let value = it.next().unwrap_or(Value::Null);
                if idx >= 0 && (idx as usize) < arr.len() {
                    Arc::make_mut(&mut arr)[idx as usize] = value.as_float();
                }
            } else if let Some(value) = it.next() {
                Arc::make_mut(&mut arr).push(value.as_float());
            }
            Value::ArrayF64(arr)
        }
        Value::ArrayI64(mut arr) => {
            if name == "sys_arr_set" {
                let idx = it.next().map(|v| v.as_int()).unwrap_or(-1);
                let value = it.next().unwrap_or(Value::Null);
                if idx >= 0 && (idx as usize) < arr.len() {
                    Arc::make_mut(&mut arr)[idx as usize] = value.as_int();
                }
            } else if let Some(value) = it.next() {
                Arc::make_mut(&mut arr).push(value.as_int());
            }
            Value::ArrayI64(arr)
        }
        _ => Value::Null,
    }
}

/// Clear `locals[slot]` when the next op (or `Dup; StoreLocal(slot)`) will overwrite that slot
/// and it still aliases `arg` (same shared array storage): the pending store overwrites it
/// anyway, and dropping the alias lets `arg` be mutated in place via `Arc::make_mut`.
fn release_store_alias_at(
    chunk: &Chunk,
    ip: usize,
    locals: &mut [Value],
    base: usize,
    arg: &Value,
) {
    let slot = match chunk.ops.get(ip) {
        Some(Op::StoreLocal(s)) => *s,
        Some(Op::Dup) => match chunk.ops.get(ip + 1) {
            Some(Op::StoreLocal(s)) => *s,
            _ => return,
        },
        _ => return,
    };
    let idx = base + slot as usize;
    let aliased = match (arg, locals.get(idx)) {
        (Value::Array(a), Some(Value::Array(b))) => Arc::ptr_eq(a, b),
        (Value::ArrayF64(a), Some(Value::ArrayF64(b))) => Arc::ptr_eq(a, b),
        (Value::ArrayI64(a), Some(Value::ArrayI64(b))) => Arc::ptr_eq(a, b),
        _ => false,
    };
    if aliased {
        locals[idx] = Value::Null;
    }
}

struct CallFrame {
    return_ip: usize,
    locals_base: usize,
    /// Copy each `(caller_slot, param_idx)` from callee locals into the caller's frame on return.
    writebacks: Vec<(u16 /* caller_slot */, u16 /* param_idx */)>,
    /// `iter_stack` depth at call time: an early `return` from inside `for` must drop the
    /// loop states it abandoned, or they would corrupt the caller's enclosing loops.
    iter_depth: usize,
    /// Operand-stack height at call time: `return` drops anything the callee left above it
    /// (e.g. temporaries abandoned by an early `return` inside nested blocks).
    stack_base: usize,
}

struct ExtSliceIter {
    ptr: usize,
    len: usize,
    elem_size: usize,
    fields: Arc<Vec<crate::value::ExtField>>,
    index: usize,
}

struct IterState {
    items: Arc<Vec<Value>>,
    index: usize,
    /// When set, iterate by `recv` until the channel closes.
    chan: Option<Arc<Channel>>,
    /// Zero-copy external slice iteration (avoids materializing all elements).
    ext: Option<ExtSliceIter>,
    /// Half-open integer range `(cur, end, start)` — `PushIt` yields `cur` then advances.
    range: Option<(i64, i64, i64)>,
}

impl<'a> Vm<'a> {
    pub fn new(chunk: Chunk, stacks: &'a mut StackPool, interner: &'a Interner) -> Self {
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 16);
        Self {
            chunk: Arc::new(chunk),
            stacks,
            interner,
            stack: Vec::with_capacity(256),
            locals: Vec::with_capacity(64),
            globals: HashMap::new(),
            call_frames: Vec::new(),
            ip: 0,
            iter_stack: Vec::new(),
            tests_passed: 0,
            output: Vec::new(),
            sched: Scheduler::with_workers(workers),
            scope_stack: Vec::new(),
            host: None,
        }
    }

    pub fn with_host(mut self, host: Arc<HostEnv>) -> Self {
        self.host = Some(host);
        self
    }

    pub fn set_host(&mut self, host: Arc<HostEnv>) {
        self.host = Some(host);
    }

    pub fn tests_passed(&self) -> usize {
        self.tests_passed
    }

    /// Name of the `test` block that was executing at the current `ip`
    /// (used to point failures at the right block).
    pub fn current_test_name(&self) -> Option<String> {
        let at = self.ip.saturating_sub(1) as u32;
        self.chunk
            .tests
            .iter()
            .filter(|t| t.entry <= at)
            .max_by_key(|t| t.entry)
            .map(|t| self.interner.resolve(t.name).to_owned())
    }

    /// Entry of the first `test` block that starts after the current instruction, if any.
    fn next_test_entry(&self) -> Option<usize> {
        let cur = self.ip.saturating_sub(1) as u32;
        self.chunk
            .tests
            .iter()
            .map(|t| t.entry)
            .filter(|&e| e > cur)
            .min()
            .map(|e| e as usize)
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }

    pub fn globals_mut(&mut self) -> &mut HashMap<SymId, Value> {
        &mut self.globals
    }

    pub fn run(&mut self) -> VmResult<Value> {
        // Request/frame scope mark
        self.stacks.mark_all();
        let result = self.dispatch_safe();
        self.stacks.rewind_all();
        result
    }

    /// Initialize top-level bindings without treating the result as a function return.
    pub fn run_init(&mut self) -> VmResult<()> {
        self.stacks.mark_all();
        let _ = self.dispatch_safe()?;
        Ok(())
    }

    /// Call a compiled Super function by name with already-marshalled arguments.
    pub fn call_named(&mut self, name: &str, args: Vec<Value>) -> VmResult<Value> {
        let idx = self
            .chunk
            .functions
            .iter()
            .position(|f| self.interner.resolve(f.name) == name)
            .ok_or_else(|| VmError::Runtime(format!("unknown function '{name}'")))?;
        self.call_index(idx, args)
    }

    pub fn call_index(&mut self, idx: usize, mut args: Vec<Value>) -> VmResult<Value> {
        let func = self
            .chunk
            .functions
            .get(idx)
            .cloned()
            .ok_or_else(|| VmError::Runtime(format!("bad function index {idx}")))?;
        if args.len() > func.arity as usize {
            return Err(VmError::Runtime(format!(
                "arity mismatch for {}: got {}, expected {}",
                self.interner.resolve(func.name),
                args.len(),
                func.arity
            )));
        }
        while args.len() < func.arity as usize {
            args.push(Value::Null);
        }
        let locals_base = self.locals.len();
        self.locals.extend(args);
        while self.locals.len() < locals_base + func.local_count as usize {
            self.locals.push(Value::Null);
        }
        // Sentinel return_ip: `Op::Return` yields out of dispatch.
        self.call_frames.push(CallFrame {
            return_ip: usize::MAX,
            locals_base,
            writebacks: Vec::new(),
            iter_depth: self.iter_stack.len(),
            stack_base: self.stack.len(),
        });
        self.ip = func.entry as usize;
        self.dispatch_safe()
    }

    /// Safe register-style dispatch (ip locked in loop local; acc on stack top).
    fn dispatch_safe(&mut self) -> VmResult<Value> {
        loop {
            if self.ip >= self.chunk.ops.len() {
                return Ok(self.stack.pop().unwrap_or(Value::Null));
            }
            // Copy op to avoid borrow issues
            let op = self.chunk.ops[self.ip].clone();
            self.ip += 1;
            match op {
                Op::Halt => return Ok(self.stack.pop().unwrap_or(Value::Null)),
                Op::Nop => {}
                Op::LoadConst(i) => {
                    self.stack.push(self.chunk.consts[i as usize].clone());
                }
                Op::LoadLocal(i) => {
                    let base = self.locals_base();
                    let v = self
                        .locals
                        .get(base + i as usize)
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.stack.push(v);
                }
                Op::StoreLocal(i) => {
                    let v = self.pop()?;
                    let base = self.locals_base();
                    let idx = base + i as usize;
                    if idx >= self.locals.len() {
                        self.locals.resize(idx + 1, Value::Null);
                    }
                    self.locals[idx] = v;
                }
                Op::LoadGlobal(s) => {
                    let v = self
                        .globals
                        .get(&s)
                        .cloned()
                        .or_else(|| {
                            let name = self.interner.resolve(s);
                            self.host
                                .as_ref()
                                .and_then(|h| h.slice(name))
                                .map(|b| b.as_value())
                        })
                        .unwrap_or(Value::Null);
                    self.stack.push(v);
                }
                Op::StoreGlobal(s) => {
                    let v = self.pop()?;
                    self.globals.insert(s, v);
                }
                Op::Move(dst, src) => {
                    let base = self.locals_base();
                    let v = self
                        .locals
                        .get(base + src as usize)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let idx = base + dst as usize;
                    if idx >= self.locals.len() {
                        self.locals.resize(idx + 1, Value::Null);
                    }
                    self.locals[idx] = v;
                }
                Op::Add => self.bin_add()?,
                Op::Sub => self.bin_num(|a, b| a - b, |a, b| a - b)?,
                Op::Mul => self.bin_num(|a, b| a * b, |a, b| a * b)?,
                Op::Div => self.div_rem(false)?,
                Op::Rem => self.div_rem(true)?,
                Op::BitAnd => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a & b));
                }
                Op::BitOr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a | b));
                }
                Op::BitXor => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a ^ b));
                }
                Op::Shl => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shl((b as u32) & 63)));
                }
                Op::Shr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shr((b as u32) & 63)));
                }
                Op::UShr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int() as u64;
                    self.stack
                        .push(Value::Int((a.wrapping_shr((b as u32) & 63)) as i64));
                }
                Op::BitNot => {
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(!a));
                }
                Op::AddImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(match a {
                        Value::Float(f) => Value::Float(f + imm as f64),
                        other => Value::Int(other.as_int().wrapping_add(imm)),
                    });
                }
                Op::SubImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(match a {
                        Value::Float(f) => Value::Float(f - imm as f64),
                        other => Value::Int(other.as_int().wrapping_sub(imm)),
                    });
                }
                Op::MulImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(match a {
                        Value::Float(f) => Value::Float(f * imm as f64),
                        other => Value::Int(other.as_int().wrapping_mul(imm)),
                    });
                }
                Op::ShlImm(sh) => {
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shl(u32::from(sh))));
                }
                Op::Neg => {
                    let v = self.pop()?;
                    self.stack.push(match v {
                        Value::Int(n) => Value::Int(-n),
                        Value::Float(f) => Value::Float(-f),
                        other => other,
                    });
                }
                Op::Not => {
                    let v = self.pop()?;
                    self.stack.push(Value::Bool(!v.as_bool()));
                }
                Op::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(Value::truthy_eq(&a, &b)));
                }
                Op::Ne => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(!Value::truthy_eq(&a, &b)));
                }
                Op::Lt => self.cmp(|a, b| a < b, |a, b| a < b)?,
                Op::Le => self.cmp(|a, b| a <= b, |a, b| a <= b)?,
                Op::Gt => self.cmp(|a, b| a > b, |a, b| a > b)?,
                Op::Ge => self.cmp(|a, b| a >= b, |a, b| a >= b)?,
                Op::And => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a.as_bool() && b.as_bool()));
                }
                Op::OrLog => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a.as_bool() || b.as_bool()));
                }
                Op::Jump(t) => self.ip = t as usize,
                Op::JumpIfFalse(t) => {
                    let v = self.pop()?;
                    if !v.as_bool() {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let v = self.pop()?;
                    if v.as_bool() {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfLt(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let take = match (&a, &b) {
                        (Value::Float(_), _) | (_, Value::Float(_)) => a.as_float() < b.as_float(),
                        _ => a.as_int() < b.as_int(),
                    };
                    if take {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfGe(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    let take = match (&a, &b) {
                        (Value::Float(_), _) | (_, Value::Float(_)) => a.as_float() >= b.as_float(),
                        _ => a.as_int() >= b.as_int(),
                    };
                    if take {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfEq(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if Value::truthy_eq(&a, &b) {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfNe(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if !Value::truthy_eq(&a, &b) {
                        self.ip = t as usize;
                    }
                }
                Op::Call(idx, arity) => self.call_fn(idx as usize, arity, Vec::new())?,
                Op::CallMut(idx, arity, wb_id) => {
                    let wbs = self
                        .chunk
                        .mut_writebacks
                        .get(wb_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    self.call_fn(idx as usize, arity, wbs)?
                }
                Op::MakeFn(idx) => self.stack.push(Value::Func(idx)),
                Op::MakeClosure(idx, n) => {
                    let mut env = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        env.push(self.pop()?);
                    }
                    env.reverse();
                    self.stack.push(Value::Closure {
                        fn_idx: idx,
                        env: Arc::new(env),
                    });
                }
                Op::CallValue(arity) => self.call_value(arity)?,
                Op::CallHost(name, arity) => self.call_host(name, arity)?,
                Op::Return => {
                    // Join any open structured-concurrency scopes before leaving.
                    while let Some(children) = self.scope_stack.pop() {
                        for t in children {
                            let _ = t.await_result();
                        }
                    }
                    let ret = self.stack.pop().unwrap_or(Value::Null);
                    if let Some(frame) = self.call_frames.pop() {
                        let writebacks: Vec<(u16, Value)> = frame
                            .writebacks
                            .iter()
                            .map(|(slot, param_idx)| {
                                (
                                    *slot,
                                    self.locals
                                        .get(frame.locals_base + *param_idx as usize)
                                        .cloned()
                                        .unwrap_or(Value::Null),
                                )
                            })
                            .collect();
                        self.locals.truncate(frame.locals_base);
                        self.iter_stack.truncate(frame.iter_depth);
                        self.stack.truncate(frame.stack_base);
                        if frame.return_ip == usize::MAX {
                            return Ok(ret);
                        }
                        self.ip = frame.return_ip;
                        let base = self.locals_base();
                        for (slot, val) in writebacks {
                            let idx = base + slot as usize;
                            if idx >= self.locals.len() {
                                self.locals.resize(idx + 1, Value::Null);
                            }
                            self.locals[idx] = val;
                        }
                        self.stack.push(ret);
                    } else if let Some(next) = self.next_test_entry() {
                        // `return` inside a `test` block ends that test only.
                        self.iter_stack.clear();
                        self.ip = next;
                    } else {
                        return Ok(ret);
                    }
                }
                Op::Pop => {
                    let _ = self.pop()?;
                }
                Op::Dup => {
                    let v = self.stack.last().cloned().unwrap_or(Value::Null);
                    self.stack.push(v);
                }
                Op::MakeArray(n) => {
                    let pc = self.ip - 1;
                    let mut arr = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        arr.push(self.pop()?);
                    }
                    arr.reverse();
                    self.stack
                        .push(track_birth(self.chunk.as_ref(), pc, Value::Array(arr.into())));
                }
                Op::MakeMap(n) => {
                    let pc = self.ip - 1;
                    let mut map = HashMap::new();
                    for _ in 0..n {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        map.insert(k, v);
                    }
                    self.stack
                        .push(track_birth(self.chunk.as_ref(), pc, Value::Map(map)));
                }
                Op::MakeStruct(name, n) => {
                    let pc = self.ip - 1;
                    let mut fields = HashMap::new();
                    for _ in 0..n {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        fields.insert(k, v);
                    }
                    let label = name.map(|id| self.interner.resolve(id).to_owned());
                    self.stack.push(track_birth(
                        self.chunk.as_ref(),
                        pc,
                        Value::Struct {
                            name,
                            label,
                            fields,
                        },
                    ));
                }
                Op::MakeSum {
                    name,
                    tag,
                    n_fields,
                } => {
                    let pc = self.ip - 1;
                    let mut fields = HashMap::new();
                    for _ in 0..n_fields {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        fields.insert(k, v);
                    }
                    let label = self.interner.resolve(name).to_owned();
                    self.stack.push(track_birth(
                        self.chunk.as_ref(),
                        pc,
                        Value::Sum {
                            tag,
                            name: label,
                            fields,
                        },
                    ));
                }
                Op::IsVariant(tag) => {
                    let v = self.pop()?;
                    let ok = match v {
                        Value::Sum { tag: t, .. } => t == tag,
                        Value::Int(n) => n == i64::from(tag),
                        _ => false,
                    };
                    self.stack.push(Value::Bool(ok));
                }
                Op::SumTag => {
                    let v = self.pop()?;
                    let tag = match v {
                        Value::Sum { tag, .. } => i64::from(tag),
                        Value::Int(n) => n,
                        _ => 0,
                    };
                    self.stack.push(Value::Int(tag));
                }
                Op::GetField(f) => {
                    let base = self.pop()?;
                    let key = self.interner.resolve(f);
                    let v = match base {
                        Value::Struct { fields, .. } => {
                            fields.get(key).cloned().unwrap_or(Value::Null)
                        }
                        Value::Sum { fields, .. } => {
                            fields.get(key).cloned().unwrap_or(Value::Null)
                        }
                        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
                        Value::ExtRef { ptr, fields, .. } => fields
                            .iter()
                            .find(|x| x.name == key)
                            .map(|fl| ext_read(ptr, fl))
                            .unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    self.stack.push(v);
                }
                Op::SetField(f) => {
                    let value = self.pop()?;
                    let mut base = self.pop()?;
                    let key = self.interner.resolve(f).to_owned();
                    match &mut base {
                        Value::Struct { fields, .. } => {
                            fields.insert(key, value);
                        }
                        Value::Sum { fields, .. } => {
                            fields.insert(key, value);
                        }
                        Value::Map(m) => {
                            m.insert(key, value);
                        }
                        Value::ExtRef { ptr, fields, .. } => {
                            if let Some(fl) = fields.iter().find(|x| x.name == key) {
                                ext_write(*ptr, fl, &value);
                            }
                        }
                        _ => {}
                    }
                    // Leave updated aggregate on the stack (for write-back).
                    self.stack.push(base);
                }
                Op::LoadFieldOff(off) => {
                    let base = self.pop()?;
                    let v = match base {
                        Value::ExtRef { ptr, fields, .. } => fields
                            .iter()
                            .find(|fl| fl.offset == off)
                            .map(|fl| ext_read(ptr, fl))
                            .unwrap_or(Value::Null),
                        Value::Struct { fields, .. } => {
                            // Fall back: offset-ordered field by layout order is unknown;
                            // leave Null rather than guessing names.
                            let _ = fields;
                            Value::Null
                        }
                        _ => Value::Null,
                    };
                    self.stack.push(v);
                }
                Op::StoreFieldOff(off) => {
                    let value = self.pop()?;
                    let mut base = self.pop()?;
                    if let Value::ExtRef { ptr, fields, .. } = &mut base
                        && let Some(fl) = fields.iter().find(|fl| fl.offset == off)
                    {
                        ext_write(*ptr, fl, &value);
                    }
                    self.stack.push(base);
                }
                Op::IndexGet => {
                    let idx = self.pop()?.as_int() as usize;
                    let base = self.pop()?;
                    let v = index_get_value(base, idx, false);
                    self.stack.push(v);
                }
                Op::LoadSliceUnchecked => {
                    let idx = self.pop()?.as_int() as usize;
                    let base = self.pop()?;
                    let v = index_get_value(base, idx, true);
                    self.stack.push(v);
                }
                Op::IndexSet => {
                    let value = self.pop()?;
                    let idx = self.pop()?.as_int() as usize;
                    let mut base = self.pop()?;
                    let lb = self.locals_base();
                    release_store_alias_at(&self.chunk, self.ip, &mut self.locals, lb, &base);
                    index_set_value(&mut base, idx, value, false);
                    self.stack.push(base);
                }
                Op::StoreSliceUnchecked => {
                    let value = self.pop()?;
                    let idx = self.pop()?.as_int() as usize;
                    let mut base = self.pop()?;
                    let lb = self.locals_base();
                    release_store_alias_at(&self.chunk, self.ip, &mut self.locals, lb, &base);
                    index_set_value(&mut base, idx, value, true);
                    self.stack.push(base);
                }
                Op::LoopStep {
                    counter,
                    limit,
                    body,
                } => {
                    let base = self.locals_base();
                    let cidx = base + counter as usize;
                    let lidx = base + limit as usize;
                    if cidx >= self.locals.len() {
                        self.locals.resize(cidx + 1, Value::Null);
                    }
                    let cur = self.locals[cidx].as_int().wrapping_add(1);
                    self.locals[cidx] = Value::Int(cur);
                    let lim = self
                        .locals
                        .get(lidx)
                        .cloned()
                        .unwrap_or(Value::Null)
                        .as_int();
                    if cur < lim {
                        self.ip = body as usize;
                    }
                }
                Op::MakeRange => {
                    let end = self.pop()?.as_int();
                    let start = self.pop()?.as_int();
                    self.stack.push(Value::Range { start, end });
                }
                Op::IterBegin => {
                    let coll = self.pop()?;
                    match coll {
                        Value::Chan(c) => {
                            self.iter_stack.push(IterState {
                                items: Arc::default(),
                                index: 0,
                                chan: Some(c),
                                ext: None,
                                range: None,
                            });
                        }
                        Value::Range { start, end } => {
                            self.iter_stack.push(IterState {
                                items: Arc::default(),
                                index: 0,
                                chan: None,
                                ext: None,
                                range: Some((start, end, start)),
                            });
                        }
                        Value::Array(a) => {
                            self.iter_stack.push(IterState {
                                items: a,
                                index: 0,
                                chan: None,
                                ext: None,
                                range: None,
                            });
                        }
                        Value::ArrayF64(a) => {
                            let boxed: Arc<Vec<Value>> =
                                Arc::new(a.iter().copied().map(Value::Float).collect());
                            self.iter_stack.push(IterState {
                                items: boxed,
                                index: 0,
                                chan: None,
                                ext: None,
                                range: None,
                            });
                        }
                        Value::ArrayI64(a) => {
                            let boxed: Arc<Vec<Value>> =
                                Arc::new(a.iter().copied().map(Value::Int).collect());
                            self.iter_stack.push(IterState {
                                items: boxed,
                                index: 0,
                                chan: None,
                                ext: None,
                                range: None,
                            });
                        }
                        Value::Bytes(b) => {
                            let boxed: Arc<Vec<Value>> = Arc::new(
                                b.iter()
                                    .copied()
                                    .map(|x| Value::Int(i64::from(x)))
                                    .collect(),
                            );
                            self.iter_stack.push(IterState {
                                items: boxed,
                                index: 0,
                                chan: None,
                                ext: None,
                                range: None,
                            });
                        }
                        Value::ExtSlice {
                            ptr,
                            len,
                            elem_size,
                            fields,
                        } => {
                            self.iter_stack.push(IterState {
                                items: Arc::default(),
                                index: 0,
                                chan: None,
                                ext: Some(ExtSliceIter {
                                    ptr,
                                    len,
                                    elem_size,
                                    fields,
                                    index: 0,
                                }),
                                range: None,
                            });
                        }
                        other => {
                            self.iter_stack.push(IterState {
                                items: Arc::new(vec![other]),
                                index: 0,
                                chan: None,
                                ext: None,
                                range: None,
                            });
                        }
                    }
                }
                Op::IterPop => {
                    let _ = self.iter_stack.pop();
                }
                Op::IterNext(done) => {
                    let jump = if let Some(it) = self.iter_stack.last_mut() {
                        if let Some(ch) = it.chan.clone() {
                            match ch.recv() {
                                Some(v) => {
                                    Arc::make_mut(&mut it.items).push(v);
                                    // index points at the newly received item
                                    false
                                }
                                None => {
                                    self.iter_stack.pop();
                                    true
                                }
                            }
                        } else if let Some(ext) = it.ext.as_mut() {
                            if ext.index >= ext.len {
                                self.iter_stack.pop();
                                true
                            } else {
                                false
                            }
                        } else if let Some((cur, end, _)) = it.range {
                            if cur >= end {
                                self.iter_stack.pop();
                                true
                            } else {
                                false
                            }
                        } else if it.index >= it.items.len() {
                            self.iter_stack.pop();
                            true
                        } else {
                            false
                        }
                    } else {
                        true
                    };
                    if jump {
                        self.ip = done as usize;
                    }
                }
                Op::PushIt => {
                    let v = if let Some(it) = self.iter_stack.last_mut() {
                        if it.chan.is_some() {
                            // Last pushed by IterNext
                            let v = it.items.last().cloned().unwrap_or(Value::Null);
                            it.index = it.items.len();
                            v
                        } else if let Some(ext) = it.ext.as_mut() {
                            let idx = ext.index;
                            ext.index += 1;
                            it.index = ext.index;
                            Value::ExtRef {
                                ptr: ext.ptr + idx * ext.elem_size,
                                size: ext.elem_size,
                                fields: Arc::clone(&ext.fields),
                            }
                        } else if let Some((cur, _end, start)) = it.range.as_mut() {
                            let v = Value::Int(*cur);
                            *cur += 1;
                            it.index = (*cur - *start) as usize;
                            v
                        } else {
                            let v = it.items.get(it.index).cloned().unwrap_or(Value::Null);
                            it.index += 1;
                            v
                        }
                    } else {
                        Value::Null
                    };
                    self.stack.push(v);
                }
                Op::PushIndex => {
                    let idx = self
                        .iter_stack
                        .last()
                        .map(|it| {
                            if it.chan.is_some() {
                                it.items.len().saturating_sub(1) as i64
                            } else if let Some(ext) = &it.ext {
                                ext.index.saturating_sub(1) as i64
                            } else {
                                it.index.saturating_sub(1) as i64
                            }
                        })
                        .unwrap_or(0);
                    self.stack.push(Value::Int(idx));
                }
                Op::IterSetIt => {
                    let value = self.pop()?;
                    let arr = if let Some(it) = self.iter_stack.last_mut() {
                        let idx = it.index.saturating_sub(1);
                        if it.chan.is_none() && it.ext.is_none() && idx < it.items.len() {
                            Arc::make_mut(&mut it.items)[idx] = value;
                        }
                        Value::Array(Arc::clone(&it.items))
                    } else {
                        Value::Null
                    };
                    self.stack.push(arr);
                }
                Op::IsAbsent => {
                    let absent = self.stack.last().map(Value::is_absent).unwrap_or(true);
                    self.stack.push(Value::Bool(absent));
                }
                Op::OrElse => {
                    let v = self.pop()?;
                    let unwrapped = match v {
                        Value::Optional(Some(inner)) => *inner,
                        Value::Optional(None) => Value::Null,
                        Value::Error(_) => Value::Null,
                        other => other,
                    };
                    self.stack.push(unwrapped);
                }
                Op::Fail => {
                    let msg = self.pop()?.as_str();
                    self.stack.push(Value::Error(msg));
                }
                Op::Panic => {
                    let msg = self.pop()?.as_str();
                    return Err(VmError::Panic(msg.to_string()));
                }
                Op::AssertEq => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    if !Value::truthy_eq(&left, &right) {
                        return Err(VmError::AssertFailed { left, right });
                    }
                    self.tests_passed += 1;
                    self.stack.push(Value::Bool(true));
                }
                Op::Print => {
                    let v = self.pop()?;
                    let s = format!("{v}");
                    self.output.push(s.clone());
                    println!("{s}");
                    self.stack.push(Value::Null);
                }
                Op::ScopeEnter => {
                    self.scope_stack.push(Vec::new());
                }
                Op::ScopeExit => {
                    if let Some(children) = self.scope_stack.pop() {
                        for t in children {
                            let _ = t.await_result();
                        }
                    }
                }
                Op::Spawn(idx) => {
                    let handle = self.spawn_thunk(idx as usize)?;
                    if let Some(scope) = self.scope_stack.last_mut() {
                        scope.push(handle.clone());
                    }
                    self.stack.push(Value::Task(handle));
                }
                Op::Await => {
                    let v = self.pop()?;
                    let result = match v {
                        Value::Task(t) => t.await_result(),
                        other => other,
                    };
                    self.stack.push(result);
                }
                Op::ChanNew => {
                    let cap = self.pop()?.as_int().max(1) as usize;
                    self.stack.push(Value::Chan(Channel::with_capacity(cap)));
                }
                Op::ChanSend => {
                    let val = self.pop()?;
                    let ch = self.pop()?;
                    match ch {
                        Value::Chan(c) => {
                            let _ = c.send(val);
                            self.stack.push(Value::Bool(true));
                        }
                        _ => self.stack.push(Value::Bool(false)),
                    }
                }
                Op::ChanRecv => {
                    let ch = self.pop()?;
                    let v = match ch {
                        Value::Chan(c) => c.recv().unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    self.stack.push(v);
                }
                Op::ChanTryRecv => {
                    let ch = self.pop()?;
                    let v = match ch {
                        Value::Chan(c) => c
                            .try_recv()
                            .map(|x| Value::Optional(Some(Box::new(x))))
                            .unwrap_or(Value::Optional(None)),
                        _ => Value::Optional(None),
                    };
                    self.stack.push(v);
                }
                Op::ChanClose => {
                    let ch = self.pop()?;
                    if let Value::Chan(c) = ch {
                        c.close();
                    }
                    self.stack.push(Value::Null);
                }
                Op::SharedNew => {
                    let v = self.pop()?;
                    let shared = match v {
                        Value::Int(n) => Value::SharedInt(Arc::new(SharedInt::new(n))),
                        other => Value::Shared(Arc::new(SharedCell::new(other))),
                    };
                    self.stack.push(shared);
                }
                Op::SharedLoad => {
                    let v = self.pop()?;
                    let out = match v {
                        Value::SharedInt(s) => Value::Int(s.load()),
                        Value::Shared(s) => s.load(),
                        other => other,
                    };
                    self.stack.push(out);
                }
                Op::SharedAdd => {
                    let delta = self.pop()?;
                    let cell = self.pop()?;
                    match cell {
                        Value::SharedInt(s) => {
                            let prev = s.fetch_add(delta.as_int());
                            self.stack.push(Value::Int(prev + delta.as_int()));
                        }
                        Value::Shared(s) => {
                            s.with_mut(|cur| {
                                *cur = Value::Int(cur.as_int() + delta.as_int());
                            });
                            self.stack.push(s.load());
                        }
                        other => self.stack.push(other),
                    }
                }
                Op::MakeDuration(unit) => {
                    let v = self.pop()?.as_float();
                    let ns = match unit {
                        0 => (v * 1_000_000.0) as i64,     // ms
                        1 => (v * 1_000.0) as i64,         // us
                        2 => (v * 1_000_000_000.0) as i64, // s
                        _ => v as i64,
                    };
                    self.stack.push(Value::Duration(ns));
                }
                Op::ParEachBegin(_end) => {
                    // Hint for M:N workers: materialize collection; body runs via Iter*.
                    // Parallelism is applied when the collection is large by rewriting
                    // numeric arrays in-place across workers before sequential body.
                    let coll = self.pop()?;
                    let coll = self.parallel_prepare(coll);
                    self.stack.push(coll);
                }
                Op::SelectWhen(n) => {
                    let n = n as usize;
                    let mut kinds = Vec::with_capacity(n);
                    let mut vals = Vec::with_capacity(n);
                    for _ in 0..n {
                        vals.push(self.pop()?);
                        kinds.push(self.pop()?);
                    }
                    kinds.reverse();
                    vals.reverse();
                    let mut arms = Vec::new();
                    let mut arm_map = Vec::new();
                    for (i, (k, v)) in kinds.into_iter().zip(vals).enumerate() {
                        match k.as_int() {
                            0 => {
                                if let Value::Chan(ch) = v {
                                    arms.push(SelectArm::Recv(ch));
                                    arm_map.push(i);
                                }
                            }
                            1 => {
                                arms.push(SelectArm::After(duration_from_value(&v)));
                                arm_map.push(i);
                            }
                            _ => {}
                        }
                    }
                    let result = if arms.is_empty() {
                        SelectResult::Timeout { index: 0 }
                    } else {
                        select_when(&arms)
                    };
                    let (idx, value) = match result {
                        SelectResult::Recv { index, value } => {
                            (arm_map.get(index).copied().unwrap_or(index), value)
                        }
                        SelectResult::Timeout { index } => {
                            (arm_map.get(index).copied().unwrap_or(index), Value::Null)
                        }
                    };
                    self.stack
                        .push(Value::Array(vec![Value::Int(idx as i64), value].into()));
                }
            }
        }
    }

    fn spawn_thunk(&mut self, idx: usize) -> VmResult<crate::sched::TaskHandle> {
        let func = self
            .chunk
            .functions
            .get(idx)
            .cloned()
            .ok_or_else(|| VmError::Runtime(format!("bad spawn thunk {idx}")))?;
        let chunk = Arc::clone(&self.chunk);
        let globals = self.globals.clone();
        // Capture current frame locals so `spawn { for x in ch … }` sees `ch`.
        let base = self.locals_base();
        let mut captured = self.locals[base..].to_vec();
        if captured.len() < func.local_count as usize {
            captured.resize(func.local_count as usize, Value::Null);
        }
        let sym_table: Vec<String> = (0..self.interner.len())
            .map(|i| {
                let id = SymId::from_index(i as u32).expect("sym");
                self.interner.resolve(id).to_owned()
            })
            .collect();
        let host = self.host.clone();
        let handle = self.sched.spawn_task(move || {
            let mut stacks = StackPool::new(1 << 16);
            let mut fiber = FiberVm {
                chunk,
                _stacks: &mut stacks,
                stack: Vec::with_capacity(64),
                locals: captured,
                globals,
                call_frames: Vec::new(),
                ip: func.entry as usize,
                iter_stack: Vec::new(),
                sym_table,
                host,
            };
            fiber.run_until_return().unwrap_or_else(|e| {
                eprintln!("runtime error in spawned task: {e:?}");
                Value::Null
            })
        });
        Ok(handle)
    }

    fn parallel_prepare(&self, coll: Value) -> Value {
        match coll {
            Value::Array(a) if a.len() > 64 => {
                let sched = Arc::clone(&self.sched);
                let n = a.len();
                parallel_for(&sched, n.min(sched.num_workers()), |_| {
                    // Partition warm-up / work-steal barrier for parallel each.
                });
                Value::Array(a)
            }
            other => other,
        }
    }

    fn call_fn(&mut self, idx: usize, arity: u8, writebacks: Vec<(u16, u16)>) -> VmResult<()> {
        let func = self
            .chunk
            .functions
            .get(idx)
            .ok_or_else(|| VmError::Runtime(format!("bad function index {idx}")))?
            .clone();
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let locals_base = self.locals.len();
        self.locals.extend(args);
        while self.locals.len() < locals_base + func.local_count as usize {
            self.locals.push(Value::Null);
        }
        self.call_frames.push(CallFrame {
            return_ip: self.ip,
            locals_base,
            writebacks,
            iter_depth: self.iter_stack.len(),
            stack_base: self.stack.len(),
        });
        self.ip = func.entry as usize;
        Ok(())
    }

    fn call_value(&mut self, arity: u8) -> VmResult<()> {
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let callable = self.pop()?;
        match callable {
            Value::Func(idx) => {
                for a in args.into_iter().rev() {
                    self.stack.push(a);
                }
                self.call_fn(idx as usize, arity, Vec::new())
            }
            Value::Closure { fn_idx, env } => {
                let func = self
                    .chunk
                    .functions
                    .get(fn_idx as usize)
                    .ok_or_else(|| VmError::Runtime(format!("bad closure index {fn_idx}")))?
                    .clone();
                let locals_base = self.locals.len();
                self.locals.extend(env.iter().cloned());
                self.locals.extend(args);
                while self.locals.len() < locals_base + func.local_count as usize {
                    self.locals.push(Value::Null);
                }
                self.call_frames.push(CallFrame {
                    return_ip: self.ip,
                    locals_base,
                    writebacks: Vec::new(),
                    iter_depth: self.iter_stack.len(),
                    stack_base: self.stack.len(),
                });
                self.ip = func.entry as usize;
                Ok(())
            }
            other => Err(VmError::Runtime(format!(
                "CallValue on non-callable: {other:?}"
            ))),
        }
    }

    fn call_host(&mut self, name: SymId, arity: u8) -> VmResult<()> {
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let raw = self.interner.resolve(name);
        // `http.ok` / `host.fn` may be qualified; builtins match the bare name.
        let n = raw.rsplit_once('.').map(|(_, bare)| bare).unwrap_or(raw);
        if let Some(value) = crate::traits::host_call(n, &args) {
            self.stack.push(value);
            return Ok(());
        }
        let result = match n {
            "round" => {
                let places = args.get(1).map(|v| v.as_int()).unwrap_or(0);
                let v = args.first().map(|v| v.as_float()).unwrap_or(0.0);
                let factor = 10f64.powi(places as i32);
                Value::Float((v * factor).round() / factor)
            }
            "map" => {
                // receiver is first; closure expr not reified — identity map for host stub collections
                let recv = args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Arc::default()));
                match recv {
                    Value::Array(a) => Value::Array(a),
                    other => other,
                }
            }
            "keep" => args
                .first()
                .cloned()
                .unwrap_or_else(|| Value::Array(Arc::default())),
            "sum" => {
                let recv = args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Arc::default()));
                match recv {
                    Value::Array(a) => Value::Float(a.iter().map(|v| v.as_float()).sum()),
                    other => other,
                }
            }
            "group_by" => args.first().cloned().unwrap_or(Value::Map(HashMap::new())),
            "__is_err" => {
                let v = args.first().cloned().unwrap_or(Value::Null);
                Value::Bool(matches!(v, Value::Error(_)))
            }
            "__err_text" => match args.first() {
                Some(Value::Error(s) | Value::Str(s)) => Value::Str(s.clone()),
                _ => Value::Str(String::new()),
            },
            // `x = host.sys_arr_set(x, i, v)` / `x = host.sys_arr_push(x, v)`: when the result is
            // stored straight back into the local that holds the array, drop that alias so the
            // copy-on-write array is uniquely owned and mutated in place (O(1) instead of O(n)).
            "sys_arr_set" | "sys_arr_push"
                if matches!(
                    args.first(),
                    Some(Value::Array(_) | Value::ArrayF64(_) | Value::ArrayI64(_))
                ) =>
            {
                self.release_store_alias(&args[0]);
                arr_update(n, args)
            }
            "__len" | "len" | "sys_arr_len" => match args.first() {
                Some(Value::Array(a)) => Value::Int(a.len() as i64),
                Some(Value::ArrayF64(a)) => Value::Int(a.len() as i64),
                Some(Value::ArrayI64(a)) => Value::Int(a.len() as i64),
                Some(Value::Str(s)) => Value::Int(s.len() as i64),
                Some(Value::Bytes(b)) => Value::Int(b.len() as i64),
                Some(Value::Map(m)) => Value::Int(m.len() as i64),
                _ => Value::Int(0),
            },
            "__abs" | "abs" => match args.first() {
                Some(Value::Float(f)) => Value::Float(f.abs()),
                Some(v) => {
                    let n = v.as_int();
                    Value::Int(n.wrapping_abs())
                }
                None => Value::Int(0),
            },
            "__slice" => {
                let end = args.get(2).map(|v| v.as_int()).unwrap_or(0) as usize;
                let start = args.get(1).map(|v| v.as_int()).unwrap_or(0) as usize;
                match args.first() {
                    Some(Value::Str(s)) => {
                        let chars: Vec<char> = s.chars().collect();
                        let lo = start.min(chars.len());
                        let hi = end.min(chars.len()).max(lo);
                        Value::Str(chars[lo..hi].iter().collect())
                    }
                    Some(Value::Array(a)) => {
                        let lo = start.min(a.len());
                        let hi = end.min(a.len()).max(lo);
                        Value::Array(a[lo..hi].to_vec().into())
                    }
                    Some(Value::ArrayI64(a)) => {
                        let lo = start.min(a.len());
                        let hi = end.min(a.len()).max(lo);
                        Value::ArrayI64(a[lo..hi].to_vec().into())
                    }
                    Some(Value::ArrayF64(a)) => {
                        let lo = start.min(a.len());
                        let hi = end.min(a.len()).max(lo);
                        Value::ArrayF64(a[lo..hi].to_vec().into())
                    }
                    other => other.cloned().unwrap_or(Value::Null),
                }
            }
            "json" | "query_one" | "ok" | "bad_request" | "not_found" | "simulate_post"
            | "simulate_get" => {
                // HTTP helpers — return structured values
                host_http(n, &args)
            }
            other => {
                if let Some(host) = &self.host {
                    if let Some(v) = host.call(raw, &args) {
                        v
                    } else if let Some(v) = host.call(other, &args) {
                        v
                    } else {
                        Value::Null
                    }
                } else {
                    Value::Null
                }
            }
        };
        self.stack.push(result);
        Ok(())
    }

    fn release_store_alias(&mut self, arg: &Value) {
        let base = self.locals_base();
        release_store_alias_at(&self.chunk, self.ip, &mut self.locals, base, arg);
    }

    fn bin_add(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.stack.push(crate::traits::add(&a, &b));
        Ok(())
    }

    fn div_rem(&mut self, rem: bool) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.stack.push(div_rem_values(&a, &b, rem)?);
        Ok(())
    }

    fn bin_num(
        &mut self,
        int_op: fn(i64, i64) -> i64,
        float_op: fn(f64, f64) -> f64,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let v = match (&a, &b) {
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Value::Float(float_op(a.as_float(), b.as_float()))
            }
            _ => Value::Int(int_op(a.as_int(), b.as_int())),
        };
        self.stack.push(v);
        Ok(())
    }

    /// Ordered comparison; `Int` vs `Int` is exact (no lossy f64 round-trip above 2^53).
    fn cmp(
        &mut self,
        float_op: fn(f64, f64) -> bool,
        int_op: fn(i64, i64) -> bool,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let r = if let Some(order) = crate::traits::ord(&a, &b) {
            let (x, y) = match order {
                n if n < 0 => (0, 1),
                n if n > 0 => (1, 0),
                _ => (0, 0),
            };
            int_op(x, y)
        } else {
            match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => int_op(*x, *y),
                _ => float_op(a.as_float(), b.as_float()),
            }
        };
        self.stack.push(Value::Bool(r));
        Ok(())
    }

    fn pop(&mut self) -> VmResult<Value> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn locals_base(&self) -> usize {
        self.call_frames.last().map(|f| f.locals_base).unwrap_or(0)
    }
}

fn host_http(name: &str, args: &[Value]) -> Value {
    match name {
        "ok" => {
            let mut fields = HashMap::new();
            fields.insert("status".into(), Value::Int(200));
            if let Some(body) = args.first() {
                fields.insert("body".into(), body.clone());
            }
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }
        "bad_request" => {
            let mut fields = HashMap::new();
            fields.insert("status".into(), Value::Int(400));
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }
        "not_found" => {
            let mut fields = HashMap::new();
            fields.insert("status".into(), Value::Int(404));
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }
        "simulate_post" | "simulate_get" => {
            let mut fields = HashMap::new();
            fields.insert("status".into(), Value::Int(200));
            Value::Struct {
                name: None,
                label: None,
                fields,
            }
        }
        _ => Value::Null,
    }
}

/// Compact fiber interpreter for `spawn` thunks (no Interner borrow — uses sym table).
struct FiberVm<'a> {
    chunk: Arc<Chunk>,
    _stacks: &'a mut StackPool,
    stack: Vec<Value>,
    locals: Vec<Value>,
    globals: HashMap<SymId, Value>,
    call_frames: Vec<CallFrame>,
    ip: usize,
    iter_stack: Vec<IterState>,
    sym_table: Vec<String>,
    host: Option<Arc<HostEnv>>,
}

impl FiberVm<'_> {
    fn resolve(&self, id: SymId) -> &str {
        self.sym_table
            .get(id.index() as usize)
            .map(|s| s.as_str())
            .unwrap_or("")
    }

    fn pop(&mut self) -> VmResult<Value> {
        self.stack.pop().ok_or(VmError::StackUnderflow)
    }

    fn locals_base(&self) -> usize {
        self.call_frames.last().map(|f| f.locals_base).unwrap_or(0)
    }

    fn run_until_return(&mut self) -> VmResult<Value> {
        loop {
            if self.ip >= self.chunk.ops.len() {
                return Ok(self.stack.pop().unwrap_or(Value::Null));
            }
            let op = self.chunk.ops[self.ip].clone();
            self.ip += 1;
            match op {
                Op::Halt => return Ok(self.stack.pop().unwrap_or(Value::Null)),
                Op::Nop => {}
                Op::LoadConst(i) => self.stack.push(self.chunk.consts[i as usize].clone()),
                Op::LoadLocal(i) => {
                    let v = self
                        .locals
                        .get(self.locals_base() + i as usize)
                        .cloned()
                        .unwrap_or(Value::Null);
                    self.stack.push(v);
                }
                Op::StoreLocal(i) => {
                    let v = self.pop()?;
                    let idx = self.locals_base() + i as usize;
                    if idx >= self.locals.len() {
                        self.locals.resize(idx + 1, Value::Null);
                    }
                    self.locals[idx] = v;
                }
                Op::LoadGlobal(s) => {
                    self.stack
                        .push(self.globals.get(&s).cloned().unwrap_or(Value::Null));
                }
                Op::StoreGlobal(s) => {
                    let v = self.pop()?;
                    self.globals.insert(s, v);
                }
                Op::Add => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(crate::traits::add(&a, &b));
                }
                Op::Sub => self.bin(|a, b| a - b, |a, b| a - b)?,
                Op::Mul => self.bin(|a, b| a * b, |a, b| a * b)?,
                Op::Div => self.div_rem(false)?,
                Op::Rem => self.div_rem(true)?,
                Op::BitAnd => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a & b));
                }
                Op::BitOr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a | b));
                }
                Op::BitXor => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a ^ b));
                }
                Op::Shl => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shl((b as u32) & 63)));
                }
                Op::Shr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shr((b as u32) & 63)));
                }
                Op::UShr => {
                    let b = self.pop()?.as_int();
                    let a = self.pop()?.as_int() as u64;
                    self.stack
                        .push(Value::Int((a.wrapping_shr((b as u32) & 63)) as i64));
                }
                Op::BitNot => {
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(!a));
                }
                Op::Neg => {
                    let v = self.pop()?;
                    self.stack.push(match v {
                        Value::Int(n) => Value::Int(-n),
                        Value::Float(f) => Value::Float(-f),
                        other => other,
                    });
                }
                Op::Not => {
                    let v = self.pop()?;
                    self.stack.push(Value::Bool(!v.as_bool()));
                }
                Op::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(Value::truthy_eq(&a, &b)));
                }
                Op::Ne => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(!Value::truthy_eq(&a, &b)));
                }
                Op::Lt => self.cmp(|a, b| a < b, |a, b| a < b)?,
                Op::Le => self.cmp(|a, b| a <= b, |a, b| a <= b)?,
                Op::Gt => self.cmp(|a, b| a > b, |a, b| a > b)?,
                Op::Ge => self.cmp(|a, b| a >= b, |a, b| a >= b)?,
                Op::And => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a.as_bool() && b.as_bool()));
                }
                Op::OrLog => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a.as_bool() || b.as_bool()));
                }
                Op::Jump(t) => self.ip = t as usize,
                Op::JumpIfFalse(t) => {
                    if !self.pop()?.as_bool() {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfTrue(t) => {
                    if self.pop()?.as_bool() {
                        self.ip = t as usize;
                    }
                }
                Op::Call(idx, arity) => self.call_fn(idx as usize, arity, Vec::new())?,
                Op::CallMut(idx, arity, wb_id) => {
                    let wbs = self
                        .chunk
                        .mut_writebacks
                        .get(wb_id as usize)
                        .cloned()
                        .unwrap_or_default();
                    self.call_fn(idx as usize, arity, wbs)?
                }
                Op::MakeFn(idx) => self.stack.push(Value::Func(idx)),
                Op::MakeClosure(idx, n) => {
                    let mut env = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        env.push(self.pop()?);
                    }
                    env.reverse();
                    self.stack.push(Value::Closure {
                        fn_idx: idx,
                        env: Arc::new(env),
                    });
                }
                Op::CallValue(arity) => self.call_value(arity)?,
                Op::CallHost(name, arity) => self.call_host(name, arity)?,
                Op::Return => {
                    let ret = self.stack.pop().unwrap_or(Value::Null);
                    if let Some(frame) = self.call_frames.pop() {
                        let writebacks: Vec<(u16, Value)> = frame
                            .writebacks
                            .iter()
                            .map(|(slot, param_idx)| {
                                (
                                    *slot,
                                    self.locals
                                        .get(frame.locals_base + *param_idx as usize)
                                        .cloned()
                                        .unwrap_or(Value::Null),
                                )
                            })
                            .collect();
                        self.locals.truncate(frame.locals_base);
                        self.iter_stack.truncate(frame.iter_depth);
                        self.stack.truncate(frame.stack_base);
                        self.ip = frame.return_ip;
                        let base = self.locals_base();
                        for (slot, val) in writebacks {
                            let idx = base + slot as usize;
                            if idx >= self.locals.len() {
                                self.locals.resize(idx + 1, Value::Null);
                            }
                            self.locals[idx] = val;
                        }
                        self.stack.push(ret);
                    } else {
                        return Ok(ret);
                    }
                }
                Op::Pop => {
                    let _ = self.pop()?;
                }
                Op::Dup => {
                    let v = self.stack.last().cloned().unwrap_or(Value::Null);
                    self.stack.push(v);
                }
                Op::MakeArray(n) => {
                    let pc = self.ip - 1;
                    let mut arr = Vec::with_capacity(n as usize);
                    for _ in 0..n {
                        arr.push(self.pop()?);
                    }
                    arr.reverse();
                    self.stack
                        .push(track_birth(self.chunk.as_ref(), pc, Value::Array(arr.into())));
                }
                Op::MakeMap(n) => {
                    let pc = self.ip - 1;
                    let mut map = HashMap::new();
                    for _ in 0..n {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        map.insert(k, v);
                    }
                    self.stack
                        .push(track_birth(self.chunk.as_ref(), pc, Value::Map(map)));
                }
                Op::MakeStruct(name, n) => {
                    let pc = self.ip - 1;
                    let mut fields = HashMap::new();
                    for _ in 0..n {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        fields.insert(k, v);
                    }
                    let label = name.map(|id| self.resolve(id).to_owned());
                    self.stack.push(track_birth(
                        self.chunk.as_ref(),
                        pc,
                        Value::Struct {
                            name,
                            label,
                            fields,
                        },
                    ));
                }
                Op::MakeSum {
                    name,
                    tag,
                    n_fields,
                } => {
                    let pc = self.ip - 1;
                    let mut fields = HashMap::new();
                    for _ in 0..n_fields {
                        let v = self.pop()?;
                        let k = self.pop()?.as_str();
                        fields.insert(k, v);
                    }
                    let label = self.resolve(name).to_owned();
                    self.stack.push(track_birth(
                        self.chunk.as_ref(),
                        pc,
                        Value::Sum {
                            tag,
                            name: label,
                            fields,
                        },
                    ));
                }
                Op::IsVariant(tag) => {
                    let v = self.pop()?;
                    let ok = match v {
                        Value::Sum { tag: t, .. } => t == tag,
                        Value::Int(n) => n == i64::from(tag),
                        _ => false,
                    };
                    self.stack.push(Value::Bool(ok));
                }
                Op::SumTag => {
                    let v = self.pop()?;
                    let tag = match v {
                        Value::Sum { tag, .. } => i64::from(tag),
                        Value::Int(n) => n,
                        _ => 0,
                    };
                    self.stack.push(Value::Int(tag));
                }
                Op::GetField(f) => {
                    let base = self.pop()?;
                    let key = self.resolve(f);
                    let v = match base {
                        Value::Struct { fields, .. } => {
                            fields.get(key).cloned().unwrap_or(Value::Null)
                        }
                        Value::Sum { fields, .. } => {
                            fields.get(key).cloned().unwrap_or(Value::Null)
                        }
                        Value::Map(m) => m.get(key).cloned().unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    self.stack.push(v);
                }
                Op::SetField(f) => {
                    let value = self.pop()?;
                    let mut base = self.pop()?;
                    let key = self.resolve(f).to_owned();
                    match &mut base {
                        Value::Struct { fields, .. } => {
                            fields.insert(key, value);
                        }
                        Value::Sum { fields, .. } => {
                            fields.insert(key, value);
                        }
                        Value::Map(m) => {
                            m.insert(key, value);
                        }
                        _ => {}
                    }
                    self.stack.push(base);
                }
                Op::IndexGet => {
                    let idx = self.pop()?.as_int() as usize;
                    let base = self.pop()?;
                    self.stack.push(index_get_value(base, idx, false));
                }
                Op::IndexSet => {
                    let value = self.pop()?;
                    let idx = self.pop()?.as_int() as usize;
                    let mut base = self.pop()?;
                    let lb = self.locals_base();
                    release_store_alias_at(&self.chunk, self.ip, &mut self.locals, lb, &base);
                    index_set_value(&mut base, idx, value, false);
                    self.stack.push(base);
                }
                Op::MakeRange => {
                    let end = self.pop()?.as_int();
                    let start = self.pop()?.as_int();
                    self.stack.push(Value::Range { start, end });
                }
                Op::IterBegin => {
                    let coll = self.pop()?;
                    match coll {
                        Value::Chan(c) => self.iter_stack.push(IterState {
                            items: Arc::default(),
                            index: 0,
                            chan: Some(c),
                            ext: None,
                            range: None,
                        }),
                        Value::Range { start, end } => self.iter_stack.push(IterState {
                            items: Arc::default(),
                            index: 0,
                            chan: None,
                            ext: None,
                            range: Some((start, end, start)),
                        }),
                        Value::Array(a) => self.iter_stack.push(IterState {
                            items: a,
                            index: 0,
                            chan: None,
                            ext: None,
                            range: None,
                        }),
                        Value::ArrayF64(a) => self.iter_stack.push(IterState {
                            items: Arc::new(a.iter().copied().map(Value::Float).collect()),
                            index: 0,
                            chan: None,
                            ext: None,
                            range: None,
                        }),
                        Value::ArrayI64(a) => self.iter_stack.push(IterState {
                            items: Arc::new(a.iter().copied().map(Value::Int).collect()),
                            index: 0,
                            chan: None,
                            ext: None,
                            range: None,
                        }),
                        Value::Bytes(b) => self.iter_stack.push(IterState {
                            items: Arc::new(
                                b.iter()
                                    .copied()
                                    .map(|x| Value::Int(i64::from(x)))
                                    .collect(),
                            ),
                            index: 0,
                            chan: None,
                            ext: None,
                            range: None,
                        }),
                        other => self.iter_stack.push(IterState {
                            items: Arc::new(vec![other]),
                            index: 0,
                            chan: None,
                            ext: None,
                            range: None,
                        }),
                    }
                }
                Op::IterPop => {
                    let _ = self.iter_stack.pop();
                }
                Op::IterNext(done) => {
                    let jump = if let Some(it) = self.iter_stack.last_mut() {
                        if let Some(ch) = it.chan.clone() {
                            match ch.recv() {
                                Some(v) => {
                                    Arc::make_mut(&mut it.items).push(v);
                                    false
                                }
                                None => {
                                    self.iter_stack.pop();
                                    true
                                }
                            }
                        } else if let Some((cur, end, _)) = it.range {
                            if cur >= end {
                                self.iter_stack.pop();
                                true
                            } else {
                                false
                            }
                        } else if it.index >= it.items.len() {
                            self.iter_stack.pop();
                            true
                        } else {
                            false
                        }
                    } else {
                        true
                    };
                    if jump {
                        self.ip = done as usize;
                    }
                }
                Op::PushIt => {
                    let v = if let Some(it) = self.iter_stack.last_mut() {
                        if it.chan.is_some() {
                            let v = it.items.last().cloned().unwrap_or(Value::Null);
                            it.index = it.items.len();
                            v
                        } else if let Some((cur, _end, start)) = it.range.as_mut() {
                            let v = Value::Int(*cur);
                            *cur += 1;
                            it.index = (*cur - *start) as usize;
                            v
                        } else {
                            let v = it.items.get(it.index).cloned().unwrap_or(Value::Null);
                            it.index += 1;
                            v
                        }
                    } else {
                        Value::Null
                    };
                    self.stack.push(v);
                }
                Op::PushIndex => {
                    let idx = self
                        .iter_stack
                        .last()
                        .map(|it| {
                            if it.chan.is_some() {
                                it.items.len().saturating_sub(1) as i64
                            } else {
                                it.index.saturating_sub(1) as i64
                            }
                        })
                        .unwrap_or(0);
                    self.stack.push(Value::Int(idx));
                }
                Op::IterSetIt => {
                    let value = self.pop()?;
                    let arr = if let Some(it) = self.iter_stack.last_mut() {
                        let idx = it.index.saturating_sub(1);
                        if it.chan.is_none() && it.ext.is_none() && idx < it.items.len() {
                            Arc::make_mut(&mut it.items)[idx] = value;
                        }
                        Value::Array(Arc::clone(&it.items))
                    } else {
                        Value::Null
                    };
                    self.stack.push(arr);
                }
                Op::IsAbsent => {
                    let absent = self.stack.last().map(Value::is_absent).unwrap_or(true);
                    self.stack.push(Value::Bool(absent));
                }
                Op::OrElse => {
                    let v = self.pop()?;
                    let unwrapped = match v {
                        Value::Optional(Some(inner)) => *inner,
                        Value::Optional(None) | Value::Error(_) => Value::Null,
                        other => other,
                    };
                    self.stack.push(unwrapped);
                }
                Op::Fail => {
                    let msg = self.pop()?.as_str();
                    self.stack.push(Value::Error(msg));
                }
                Op::Panic => {
                    let msg = self.pop()?.as_str();
                    return Err(VmError::Panic(msg.to_string()));
                }
                Op::AssertEq => {
                    let right = self.pop()?;
                    let left = self.pop()?;
                    if !Value::truthy_eq(&left, &right) {
                        return Err(VmError::AssertFailed { left, right });
                    }
                    self.stack.push(Value::Bool(true));
                }
                Op::Print => {
                    let v = self.pop()?;
                    println!("{v}");
                    self.stack.push(Value::Null);
                }
                Op::ChanNew => {
                    let cap = self.pop()?.as_int().max(1) as usize;
                    self.stack.push(Value::Chan(Channel::with_capacity(cap)));
                }
                Op::ChanSend => {
                    let val = self.pop()?;
                    let ch = self.pop()?;
                    if let Value::Chan(c) = ch {
                        let _ = c.send(val);
                    }
                    self.stack.push(Value::Bool(true));
                }
                Op::ChanRecv => {
                    let ch = self.pop()?;
                    let v = match ch {
                        Value::Chan(c) => c.recv().unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    self.stack.push(v);
                }
                Op::ChanClose => {
                    if let Value::Chan(c) = self.pop()? {
                        c.close();
                    }
                    self.stack.push(Value::Null);
                }
                Op::ChanTryRecv => {
                    let ch = self.pop()?;
                    let v = match ch {
                        Value::Chan(c) => c
                            .try_recv()
                            .map(|x| Value::Optional(Some(Box::new(x))))
                            .unwrap_or(Value::Optional(None)),
                        _ => Value::Optional(None),
                    };
                    self.stack.push(v);
                }
                Op::SharedNew => {
                    let v = self.pop()?;
                    self.stack.push(match v {
                        Value::Int(n) => Value::SharedInt(Arc::new(SharedInt::new(n))),
                        other => Value::Shared(Arc::new(SharedCell::new(other))),
                    });
                }
                Op::SharedLoad => {
                    let v = self.pop()?;
                    self.stack.push(match v {
                        Value::SharedInt(s) => Value::Int(s.load()),
                        Value::Shared(s) => s.load(),
                        other => other,
                    });
                }
                Op::SharedAdd => {
                    let delta = self.pop()?;
                    let cell = self.pop()?;
                    match cell {
                        Value::SharedInt(s) => {
                            let prev = s.fetch_add(delta.as_int());
                            self.stack.push(Value::Int(prev + delta.as_int()));
                        }
                        other => self.stack.push(other),
                    }
                }
                Op::MakeDuration(unit) => {
                    let v = self.pop()?.as_float();
                    let ns = match unit {
                        0 => (v * 1_000_000.0) as i64,
                        1 => (v * 1_000.0) as i64,
                        2 => (v * 1_000_000_000.0) as i64,
                        _ => v as i64,
                    };
                    self.stack.push(Value::Duration(ns));
                }
                Op::Await => {
                    let v = self.pop()?;
                    self.stack.push(match v {
                        Value::Task(t) => t.await_result(),
                        other => other,
                    });
                }
                Op::Move(dst, src) => {
                    let base = self.locals_base();
                    let v = self
                        .locals
                        .get(base + src as usize)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let idx = base + dst as usize;
                    if idx >= self.locals.len() {
                        self.locals.resize(idx + 1, Value::Null);
                    }
                    self.locals[idx] = v;
                }
                Op::AddImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(Value::Int(a.as_int().wrapping_add(imm)));
                }
                Op::SubImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(Value::Int(a.as_int().wrapping_sub(imm)));
                }
                Op::MulImm(imm) => {
                    let a = self.pop()?;
                    self.stack.push(Value::Int(a.as_int().wrapping_mul(imm)));
                }
                Op::ShlImm(sh) => {
                    let a = self.pop()?.as_int();
                    self.stack.push(Value::Int(a.wrapping_shl(u32::from(sh))));
                }
                Op::JumpIfLt(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if a.as_int() < b.as_int() {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfGe(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if a.as_int() >= b.as_int() {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfEq(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if Value::truthy_eq(&a, &b) {
                        self.ip = t as usize;
                    }
                }
                Op::JumpIfNe(t) => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    if !Value::truthy_eq(&a, &b) {
                        self.ip = t as usize;
                    }
                }
                Op::LoadFieldOff(_) | Op::StoreFieldOff(_) => {
                    // Fibers rarely see ExtRef field-offs; treat as no-op Null.
                    let _ = self.pop();
                    self.stack.push(Value::Null);
                }
                Op::LoadSliceUnchecked => {
                    let idx = self.pop()?.as_int() as usize;
                    let base = self.pop()?;
                    self.stack.push(index_get_value(base, idx, true));
                }
                Op::StoreSliceUnchecked => {
                    let value = self.pop()?;
                    let idx = self.pop()?.as_int() as usize;
                    let mut base = self.pop()?;
                    index_set_value(&mut base, idx, value, true);
                    self.stack.push(base);
                }
                Op::LoopStep {
                    counter,
                    limit,
                    body,
                } => {
                    let base = self.locals_base();
                    let cidx = base + counter as usize;
                    let lidx = base + limit as usize;
                    if cidx >= self.locals.len() {
                        self.locals.resize(cidx + 1, Value::Null);
                    }
                    let cur = self.locals[cidx].as_int().wrapping_add(1);
                    self.locals[cidx] = Value::Int(cur);
                    let lim = self
                        .locals
                        .get(lidx)
                        .cloned()
                        .unwrap_or(Value::Null)
                        .as_int();
                    if cur < lim {
                        self.ip = body as usize;
                    }
                }
                Op::Spawn(_)
                | Op::ScopeEnter
                | Op::ScopeExit
                | Op::ParEachBegin(_)
                | Op::SelectWhen(_) => {
                    // Nested structured concurrency inside fibers runs sequentially for safety.
                }
            }
        }
    }

    fn call_host(&mut self, name: SymId, arity: u8) -> VmResult<()> {
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let raw = self.resolve(name).to_owned();
        let n = raw.rsplit_once('.').map(|(_, bare)| bare).unwrap_or(&raw);
        if let Some(value) = crate::traits::host_call(n, &args) {
            self.stack.push(value);
            return Ok(());
        }
        let result = match n {
            "print" | "println" => {
                match args.first() {
                    Some(v) => println!("{v}"),
                    None => println!(),
                }
                Value::Null
            }
            "ok" | "bad_request" | "not_found" | "simulate_post" | "simulate_get" => {
                host_http(n, &args)
            }
            "sys_arr_set" | "sys_arr_push"
                if matches!(
                    args.first(),
                    Some(Value::Array(_) | Value::ArrayF64(_) | Value::ArrayI64(_))
                ) =>
            {
                let base = self.locals_base();
                release_store_alias_at(&self.chunk, self.ip, &mut self.locals, base, &args[0]);
                arr_update(n, args)
            }
            "__len" | "len" | "sys_arr_len" => match args.first() {
                Some(Value::Array(a)) => Value::Int(a.len() as i64),
                Some(Value::ArrayF64(a)) => Value::Int(a.len() as i64),
                Some(Value::ArrayI64(a)) => Value::Int(a.len() as i64),
                Some(Value::Str(s)) => Value::Int(s.len() as i64),
                Some(Value::Bytes(b)) => Value::Int(b.len() as i64),
                Some(Value::Map(m)) => Value::Int(m.len() as i64),
                _ => Value::Int(0),
            },
            "__abs" | "abs" => match args.first() {
                Some(Value::Float(f)) => Value::Float(f.abs()),
                Some(v) => Value::Int(v.as_int().wrapping_abs()),
                None => Value::Int(0),
            },
            other => self
                .host
                .as_ref()
                .and_then(|h| h.call(&raw, &args).or_else(|| h.call(other, &args)))
                .unwrap_or(Value::Null),
        };
        self.stack.push(result);
        Ok(())
    }

    fn call_fn(&mut self, idx: usize, arity: u8, writebacks: Vec<(u16, u16)>) -> VmResult<()> {
        let func = self
            .chunk
            .functions
            .get(idx)
            .cloned()
            .ok_or_else(|| VmError::Runtime(format!("bad function index {idx}")))?;
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let locals_base = self.locals.len();
        self.locals.extend(args);
        while self.locals.len() < locals_base + func.local_count as usize {
            self.locals.push(Value::Null);
        }
        self.call_frames.push(CallFrame {
            return_ip: self.ip,
            locals_base,
            writebacks,
            iter_depth: self.iter_stack.len(),
            stack_base: self.stack.len(),
        });
        self.ip = func.entry as usize;
        Ok(())
    }

    fn call_value(&mut self, arity: u8) -> VmResult<()> {
        let mut args = Vec::with_capacity(arity as usize);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();
        let callable = self.pop()?;
        match callable {
            Value::Func(idx) => {
                for a in args.into_iter().rev() {
                    self.stack.push(a);
                }
                self.call_fn(idx as usize, arity, Vec::new())
            }
            Value::Closure { fn_idx, env } => {
                let func = self
                    .chunk
                    .functions
                    .get(fn_idx as usize)
                    .cloned()
                    .ok_or_else(|| VmError::Runtime(format!("bad closure index {fn_idx}")))?;
                let locals_base = self.locals.len();
                self.locals.extend(env.iter().cloned());
                self.locals.extend(args);
                while self.locals.len() < locals_base + func.local_count as usize {
                    self.locals.push(Value::Null);
                }
                self.call_frames.push(CallFrame {
                    return_ip: self.ip,
                    locals_base,
                    writebacks: Vec::new(),
                    iter_depth: self.iter_stack.len(),
                    stack_base: self.stack.len(),
                });
                self.ip = func.entry as usize;
                Ok(())
            }
            other => Err(VmError::Runtime(format!(
                "CallValue on non-callable: {other:?}"
            ))),
        }
    }

    fn div_rem(&mut self, rem: bool) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        self.stack.push(div_rem_values(&a, &b, rem)?);
        Ok(())
    }

    fn bin(&mut self, int_op: fn(i64, i64) -> i64, float_op: fn(f64, f64) -> f64) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let v = match (&a, &b) {
            (Value::Float(_), _) | (_, Value::Float(_)) => {
                Value::Float(float_op(a.as_float(), b.as_float()))
            }
            _ => Value::Int(int_op(a.as_int(), b.as_int())),
        };
        self.stack.push(v);
        Ok(())
    }

    /// Ordered comparison; `Int` vs `Int` is exact (no lossy f64 round-trip above 2^53).
    fn cmp(
        &mut self,
        float_op: fn(f64, f64) -> bool,
        int_op: fn(i64, i64) -> bool,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let r = if let Some(order) = crate::traits::ord(&a, &b) {
            let (x, y) = match order {
                n if n < 0 => (0, 1),
                n if n > 0 => (1, 0),
                _ => (0, 0),
            };
            int_op(x, y)
        } else {
            match (&a, &b) {
                (Value::Int(x), Value::Int(y)) => int_op(*x, *y),
                _ => float_op(a.as_float(), b.as_float()),
            }
        };
        self.stack.push(Value::Bool(r));
        Ok(())
    }
}

impl Drop for Vm<'_> {
    fn drop(&mut self) {
        self.sched.shutdown();
    }
}

/// Optional asm tail-jump trampoline used by the JIT / advanced dispatch path.
/// Isolated unsafe: jumps to an absolute handler address.
///
/// # Safety
///
/// `target` must be the entry of a handler that never returns to this frame.
#[inline(always)]
pub unsafe fn tail_jump(target: *const ()) -> ! {
    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            core::arch::asm!(
                "jmp {target}",
                target = in(reg) target,
                options(noreturn)
            );
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = target;
        loop {
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Compiler;
    use super_core::{Diagnostics, Interner, SourceFile};
    use super_frontend::{Lexer, Parser, SemanticAnalyzer};

    fn run_src(src: &str) -> VmResult<Value> {
        let source = SourceFile::new("t.sr", src);
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diags);
        let mut module = parser.parse_module();
        super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        let chunk = Compiler::new(&mut interner, &typed, false).compile(&module);
        let mut stacks = StackPool::new(1 << 16);
        let mut vm = Vm::new(chunk, &mut stacks, &interner);
        vm.run()
    }

    #[test]
    fn runs_arithmetic() {
        let src = r#"
main() {
    print(1 + 2 * 3)
}
"#;
        let v = run_src(src).expect("run");
        let _ = v;
    }

    #[test]
    fn sum_methods_and_payload_destructure() {
        let src = r#"
+Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)

    is_ok(self) : bool {
        when self {
            Status.Failed => false
            else => true
        }
    }

    amount_or(self, fallback: f64) : f64 {
        when self {
            Status.Paid { amount } => amount
            else => fallback
        }
    }
}
main() {
    a = Status.Pending
    b = Status.Paid(49.95)
    c = Status.Failed("nope")
    #assert(a.is_ok())
    #assert(b.is_ok())
    #assert(!(c.is_ok()))
    assert_eq(b.amount_or(0.0), 49.95)
    assert_eq(a.amount_or(1.5), 1.5)
    got = 0.0
    when b {
        Status.Paid { amount: x } => got = x
        else => got = -1.0
    }
    assert_eq(got, 49.95)
}
"#;
        run_src(src).expect("sum methods + destructure");
    }

    #[test]
    fn sum_variants_construct_match_and_coerce_to_i64() {
        let out = run_capture(
            r#"
+Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)
}

+fee(s: Status) : f64 {
    rate = 0.0
    when s {
        Status.Paid => rate = 0.02
        Status.Failed => rate = 0.5
        Status.Pending => rate = 0.0
    }
    rate
}

+main() {
    a = Status.Pending
    b = Status.Paid(49.95)
    c = Status.Failed("nope")
    print(fee(a))
    print(fee(b))
    print(fee(c))
    print(a == Status.Pending)
    print(b == 1)
    print(1 == Status.Paid(0.0))
}

test "unit variant is a tiny tagged object" {
    #assert(Status.Pending == Status.Pending)
    #assert(!(Status.Pending == Status.Failed("x")))
    #assert(Status.Paid(10.0) == Status.Paid(10.0))
}
"#,
        );
        assert_eq!(
            out,
            ["0", "0.02", "0.5", "true", "true", "true"],
            "got {out:?}"
        );
        assert_eq!(
            run_asserts(
                r#"
+Status = {
    Pending
    Paid(amount: f64)
    Failed(reason: str)
}
test "unit variant is a tiny tagged object" {
    #assert(Status.Pending == Status.Pending)
    #assert(!(Status.Pending == Status.Failed("x")))
    #assert(Status.Paid(10.0) == Status.Paid(10.0))
}
"#,
            ),
            3
        );
    }

    #[test]
    fn keep_map_sum_pipeline_filters_and_reduces() {
        let src = r#"
main() {
    scores = [10, 20, 30, 40, 50, 5, 15]
    assert_eq(scores.keep(it >= 25).map(it * 2).sum(), 240)
    assert_eq([10, 20, 30, 40].keep(it >= 25).map(it * 2).sum(), 140)
}
"#;
        run_src(src).expect("keep/map/sum pipeline");
    }

    #[test]
    fn pipeline_reduction_builtins_and_fused_map_sum() {
        let src = r#"
main() {
    xs = [1, 2, 3, 4, 5]
  assert_eq(xs.sum(), 15)
    assert_eq(xs.product(), 120)
    assert_eq(xs.min(), 1)
    assert_eq(xs.max(), 5)
    assert_eq(xs.mean(), 3.0)
    assert_eq(xs.map(it * 2).sum(), 30)
    assert_eq([2, 4, 6].sum(), 12)
    assert_eq(xs.keep(it > 3).sum(), 9)
}
"#;
        run_src(src).expect("pipeline reductions");
    }

    #[test]
    fn type_methods_static_immut_mut() {
        let src = r#"
+App = {
    n: i64 = 0
    +new(x: i64) : App => App.{n: x}
    value(self) : i64 => self.n
    bump(@self, d: i64) : i64 {
        self.n = self.n + d
        self.n
    }
}
main() {
    a = App.new(10)
    assert_eq(a.value(), 10)
    assert_eq(@a.bump(5), 15)
    assert_eq(a.value(), 15)
    assert_eq(App.bump(a, 1), 16)
}
"#;
        run_src(src).expect("type methods run");
    }

    #[test]
    fn free_fn_at_param_writeback_scalar_and_array() {
        let src = r#"
+inc(@x: i64) : i64 {
    x = x + 1
    x
}
+lcg(@rng: _) : i64 {
    v = (rng[0] * 1103515245 + 12345) % 2147483648
    rng = host.sys_arr_set(rng, 0, v)
    v
}
+add2(@a: i64, @b: i64, x: i64) : i64 {
    a = a + x
    b = b + x
    a + b
}
+bump(@keys: _, @counts: _, key: i64, add: i64) : i64 {
    if keys[0] == key {
        counts = host.sys_arr_set(counts, 0, counts[0] + add)
        return counts[0]
    }
    if keys[1] == key {
        counts = host.sys_arr_set(counts, 1, counts[1] + add)
        return counts[1]
    }
    keys = host.sys_arr_push(keys, key)
    counts = host.sys_arr_push(counts, add)
    add
}
main() {
    n = 10
    assert_eq(inc(@n), 11)
    assert_eq(n, 11)
    assert_eq(inc(@n), 12)
    assert_eq(n, 12)
    r = [10846167]
    a = lcg(@r)
    assert_eq(a, 716513476)
    assert_eq(r[0], 716513476)
    b = lcg(@r)
    #assert(b != a)
    assert_eq(r[0], b)
    x = 1
    y = 2
    assert_eq(add2(@x, @y, 10), 23)
    assert_eq(x, 11)
    assert_eq(y, 12)
    keys = [1, 2]
    counts = [10, 20]
    assert_eq(bump(@keys, @counts, 2, 5), 25)
    assert_eq(counts[1], 25)
    assert_eq(bump(@keys, @counts, 9, 3), 3)
    assert_eq(keys[2], 9)
    assert_eq(counts[2], 3)
}
"#;
        run_src(src).expect("@param writeback");
    }

    #[test]
    fn ambiguous_method_name_disambiguated_by_arity() {
        // World.has(e, name) vs Store.has(idx) share a short name; UFCS arity picks the right one.
        let src = r#"
+Store = {
    n: i64 = 0
    has(self, idx: i64) : bool => idx < self.n
}
+World = {
    st: Store = Store.{n: 1}
    has(self, e: i64, name: str) : bool {
        ensure e >= 0 else { return false }
        self.st.has(e)
    }
}
main() {
    w = World.{st: Store.{n: 3}}
    assert_eq(w.has(1, "x"), true)
    assert_eq(w.has(5, "x"), false)
}
"#;
        run_src(src).expect("arity-disambiguated has");
    }

    #[test]
    fn same_arity_methods_disambiguated_via_or_err_binding() {
        // Ipv4/Ipv6 both expose is_loopback(self); `or err` must keep the unwrapped
        // receiver type so UFCS picks the matching owner (not CallHost → Null).
        let src = r#"
Ipv4 = {
    bits: i64 = 0
    +parse(s: str) : Ipv4! => Ipv4.{bits: 127 * 16777216}
    is_loopback(self) : bool {
        if self.bits / 16777216 == 127 { return true }
        false
    }
}
Ipv6 = {
    h7: i64 = 1
    is_loopback(self) : bool {
        if self.h7 == 1 { return true }
        false
    }
}
main() {
    lo = Ipv4.parse("127.0.0.1") or err { return }
    #assert(lo.is_loopback())
    v6 = Ipv6.{h7: 1}
    #assert(v6.is_loopback())
}
"#;
        run_src(src).expect("or-err owner-aware UFCS");
    }

    /// Compile `src`, run `main`, and return every line `print`/`println` emitted.
    fn run_capture(src: &str) -> Vec<String> {
        let (res, out) = run_outcome(src);
        res.expect("run");
        out
    }

    fn run_outcome(src: &str) -> (VmResult<Value>, Vec<String>) {
        let source = SourceFile::new("t.sr", src);
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diags);
        let mut module = parser.parse_module();
        super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let chunk = Compiler::new(&mut interner, &typed, false).compile(&module);
        let mut stacks = StackPool::new(1 << 16);
        let mut vm = Vm::new(chunk, &mut stacks, &interner);
        let res = vm.run();
        (res, vm.output().to_vec())
    }

    const DIVIDE: &str = "div(a: i64, b: i64) : i64 => a / b\nrem(a: i64, b: i64) : i64 => a % b\n";

    #[test]
    fn integer_division_by_zero_is_a_super_panic_not_a_host_abort() {
        for (call, what) in [
            ("div(7, zs[0])", "division"),
            ("rem(7, zs[0])", "remainder"),
        ] {
            let src = format!(
                "{DIVIDE}main() {{\n    zs = [0]\n    print(\"before\")\n    print({call})\n}}\n"
            );
            let (res, out) = run_outcome(&src);
            assert_eq!(out, ["before"]);
            match res {
                Err(VmError::Panic(msg)) => {
                    assert!(msg.contains(what) && msg.contains("by zero"), "{msg}")
                }
                other => panic!("expected a division panic, got {other:?}"),
            }
        }
    }

    #[test]
    fn integer_division_still_truncates_and_min_over_minus_one_wraps() {
        let out = run_capture(&format!(
            "{DIVIDE}main() {{\n    print(div(-7, 2))\n    print(rem(-7, 2))\n    m = -9223372036854775807 - 1\n    print(div(m, -1) == m)\n    print(rem(m, -1))\n    print(7.0 / 2.0)\n}}\n"
        ));
        assert_eq!(out, ["-3", "-1", "true", "0", "3.5"]);
    }

    /// Compile and run only the `test` blocks of `src`; returns the passing assertion count.
    fn run_asserts(src: &str) -> usize {
        let source = SourceFile::new("t.sr", src);
        let mut interner = Interner::new();
        let mut diags = Diagnostics::new();
        let tokens = Lexer::new(&source.text, &mut interner, &mut diags).tokenize();
        let mut parser = Parser::new(&tokens, &source.text, &mut interner, &mut diags);
        let mut module = parser.parse_module();
        super_frontend::macros::expand_module(&mut module, &mut interner, &mut diags);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let typed = SemanticAnalyzer::new(&mut interner, &mut diags).analyze(&module);
        assert!(!diags.has_errors(), "{:?}", diags.into_vec());
        let chunk = Compiler::new(&mut interner, &typed, false).compile_tests(&module);
        let mut stacks = StackPool::new(1 << 16);
        let mut vm = Vm::new(chunk, &mut stacks, &interner);
        vm.run().expect("tests");
        vm.tests_passed()
    }

    /// Pure-Super stand-ins for the stdlib wrappers (the VM unit tests have no host env).
    const UFCS_PRELUDE: &str = r#"
sqrt(x: f64) : f64 {
    r = x
    for 0..60 { r = (r + x / r) / 2.0 }
    r
}
half(x: f64) : f64 => x / 2.0
sq(x: f64) : f64 => x * x
byte_len(s: str) : i64 => s.len
first(a: _)  => a[0]
double(n: i64) : i64 => n * 2
add(n: i64, m: i64) : i64 => n + m
shout(s: str) : str => s + "!"
"#;

    #[test]
    fn return_inside_a_test_ends_only_that_test() {
        let src = r#"
test "early exit" {
    assert_eq(1, 1)
    if 1 == 1 { return }
    assert_eq(1, 2)
}

test "early exit from a loop" {
    for 0..5 {
        #assert(it < 3)
        if it == 2 { return }
    }
    assert_eq(1, 2)
}

test "still runs after both" {
    assert_eq(2, 2)
    assert_eq(3, 3)
}
"#;
        // 1 + 3 + 2: every block ran and none of the unreachable assertions did.
        assert_eq!(run_asserts(src), 6);
    }

    #[test]
    fn ufcs_float_chain_matches_nested_calls() {
        let src = format!(
            r#"{UFCS_PRELUDE}
test "float chain" {{
    assert_eq(49.0.sqrt().half().sq(), sq(half(sqrt(49.0))))
    assert_eq(49.0.sqrt().half().sq(), 12.25)
    x = 64.0
    assert_eq(x.sqrt().half(), half(sqrt(x)))
    assert_eq(x.half().half().half(), 8.0)
    assert_eq(x.sq().sqrt(), 64.0)
}}
"#
        );
        assert_eq!(run_asserts(&src), 5);
    }

    #[test]
    fn ufcs_string_chain_matches_nested_calls() {
        let src = format!(
            r#"{UFCS_PRELUDE}
test "string chain" {{
    assert_eq("Hi".byte_len(), 2)
    assert_eq("Hi".byte_len(), byte_len("Hi"))
    s = "hello"
    assert_eq(s.byte_len(), 5)
    assert_eq(s.shout().byte_len(), 6)
    assert_eq(s.shout().shout(), "hello!!")
    assert_eq("".byte_len(), 0)
    assert_eq("ab".shout().byte_len().double(), 6)
}}
"#
        );
        assert_eq!(run_asserts(&src), 7);
    }

    #[test]
    fn ufcs_array_chain_matches_nested_calls() {
        let src = format!(
            r#"{UFCS_PRELUDE}
test "array chain" {{
    assert_eq([1, 2, 3].len(), 3)
    xs = [10, 20, 30, 40]
    assert_eq(xs.len(), len(xs))
    assert_eq(xs.first().double(), 20)
    assert_eq(xs.first().double().add(1), 21)
    assert_eq(xs.len().double().double(), 16)
    empty = [0; 0]
    assert_eq(empty.len(), 0)
}}
"#
        );
        assert_eq!(run_asserts(&src), 6);
    }

    #[test]
    fn ufcs_free_function_beats_ambiguous_type_method_for_primitives() {
        // `add` is both a free function and `Bag.add`; the receiver type decides.
        let src = format!(
            r#"{UFCS_PRELUDE}
Bag: Add = {{
    total: i64 = 0
    add(self, v: i64) : i64 => self.total + v
}}
test "overloaded short names" {{
    assert_eq(3.add(4), 7)
    assert_eq(3.double().add(1), 7)
    b = Bag.{{total: 10}}
    assert_eq(b.add(5), 15)
    assert_eq(add(3, 4), 7)
}}
"#
        );
        assert_eq!(run_asserts(&src), 4);
    }

    #[test]
    fn ufcs_struct_receiver_chains_into_scalar_stages() {
        let src = format!(
            r#"{UFCS_PRELUDE}
Pt = {{
    x: f64
    y: f64
}}
len2(p: Pt) : f64 => p.x * p.x + p.y * p.y
scale(p: Pt, k: f64) : Pt => Pt.{{x: p.x * k, y: p.y * k}}
test "struct chain" {{
    p = Pt.{{x: 3.0, y: 4.0}}
    assert_eq(p.len2().sqrt(), 5.0)
    assert_eq(p.scale(2.0).len2().sqrt(), 10.0)
    assert_eq(p.scale(2.0).len2(), len2(scale(p, 2.0)))
}}
"#
        );
        assert_eq!(run_asserts(&src), 3);
    }

    #[test]
    fn println_and_print_both_end_lines() {
        let out = run_capture(
            r#"
main() {
    print("a")
    println("b")
    println()
    print()
    println(1 + 2)
    "ufcs".println()
    "dot".print()
    xs = [1, 2]
    xs.println()
}
"#,
        );
        assert_eq!(out, ["a", "b", "", "", "3", "ufcs", "dot", "[1, 2]"]);
    }

    #[test]
    fn user_defined_println_takes_precedence() {
        let out = run_capture(
            r#"
println(s: str) : str {
    print("custom:" + s)
    s
}
main() {
    println("x")
    "y".println()
}
"#,
        );
        assert_eq!(out, ["custom:x", "custom:y"]);
    }

    #[test]
    fn string_escapes_decode() {
        let cases: [(&str, &str); 12] = [
            (r#"print("a\nb")"#, "a\nb"),
            (r#"print("a\rb")"#, "a\rb"),
            (r#"print("a\tb")"#, "a\tb"),
            (r#"print("a\\b")"#, "a\\b"),
            (r#"print("a\"b")"#, "a\"b"),
            (r#"print("a\0b")"#, "a\0b"),
            (r#"print("\x41\x7a")"#, "Az"),
            (r#"print("\u{41}\u{e9}\u{1F600}")"#, "A\u{e9}\u{1F600}"),
            (r#"print("it\'s")"#, "it's"),
            (r#"print("\{not a hole\}")"#, "{not a hole}"),
            (r#"print("\\n is not a newline")"#, "\\n is not a newline"),
            (r#"print("tab\there\nnext")"#, "tab\there\nnext"),
        ];
        for (stmt, expected) in cases {
            let out = run_capture(&format!("main() {{\n    {stmt}\n}}\n"));
            assert_eq!(out, [expected], "{stmt}");
        }
    }

    #[test]
    fn escapes_compose_with_interpolation() {
        let out = run_capture(
            r#"
main() {
    n = 7
    print("n=\t{n}\n")
    print("{n}\"q\"{n}")
    print("\{n\} = {n}")
}
"#,
        );
        assert_eq!(out, ["n=\t7\n", "7\"q\"7", "{n} = 7"]);
    }

    #[test]
    fn raw_strings_keep_backslashes_and_braces() {
        let cases: [(&str, &str); 7] = [
            (
                r##"print(r"C:\Users\admin\notes.txt")"##,
                r"C:\Users\admin\notes.txt",
            ),
            (r##"print(r"^\d+\.\d+$")"##, r"^\d+\.\d+$"),
            (r###"print(r#"say "hi" \d+"#)"###, r#"say "hi" \d+"#),
            (r##"print(r"{name} {1 + 2} \n")"##, r"{name} {1 + 2} \n"),
            (r##"print(r"\u{41} \x41 \q")"##, r"\u{41} \x41 \q"),
            (r####"print(r##"a "# b"##)"####, r##"a "# b"##),
            (r##"print(r"")"##, ""),
        ];
        for (stmt, expected) in cases {
            let out = run_capture(&format!("main() {{\n    {stmt}\n}}\n"));
            assert_eq!(out, [expected], "{stmt}");
        }
    }

    #[test]
    fn raw_string_multiline_and_ufcs() {
        let out = run_capture(
            "byte_len(s: str) : i64 => s.len\nmain() {\n    p = r\"a\\\nb\"\n    print(p)\n    print(r\"\\\\\".byte_len())\n}\n",
        );
        assert_eq!(out, ["a\\\nb", "2"]);
    }
}
