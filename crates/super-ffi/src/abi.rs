//! C-ABI value marshalling between host buffers and Super [`Value`]s.

use super_runtime::{ExtField, ExtKind, Value};

/// One slot in a `super_bind_fn` / `super_call` signature descriptor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SigSlot {
    #[default]
    Void,
    Bool,
    I32,
    I64,
    F32,
    F64,
    /// Opaque pointer / `ExtRef` payload (usize).
    Ptr,
    /// Struct blob of fixed byte size (copied in/out).
    Struct {
        size: usize,
    },
}

/// Parsed `"i64,f32->f64"` / `"Entity:16,f32->void"` signature.
#[derive(Clone, Debug, Default)]
pub struct SigDesc {
    pub args: Vec<SigSlot>,
    pub ret: SigSlot,
}

impl SigDesc {
    pub fn parse(desc: &str) -> Self {
        let desc = desc.trim();
        if desc.is_empty() {
            return Self {
                args: Vec::new(),
                ret: SigSlot::Void,
            };
        }
        let (args_part, ret_part) = if let Some((a, r)) = desc.split_once("->") {
            (a, r)
        } else if let Some((a, r)) = desc.split_once('>') {
            (a, r)
        } else {
            (desc, "void")
        };
        let args = args_part
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_slot)
            .collect();
        Self {
            args,
            ret: parse_slot(ret_part.trim()),
        }
    }

    pub fn args_bytes(&self) -> usize {
        self.args.iter().map(slot_size_aligned).sum()
    }

    pub fn ret_bytes(&self) -> usize {
        slot_size_aligned(&self.ret)
    }
}

fn parse_slot(s: &str) -> SigSlot {
    if s.is_empty() || s == "void" || s == "()" {
        return SigSlot::Void;
    }
    if let Some((name, size)) = s.split_once(':') {
        let _ = name;
        if let Ok(n) = size.trim().parse::<usize>() {
            return SigSlot::Struct { size: n };
        }
    }
    match s {
        "bool" | "b" => SigSlot::Bool,
        "u8" | "i8" | "u16" | "i16" | "u32" | "i32" | "int" => SigSlot::I32,
        "u64" => SigSlot::I64,
        "i64" | "i" | "isize" | "usize" => SigSlot::I64,
        "f32" | "float" => SigSlot::F32,
        "f64" | "d" | "double" => SigSlot::F64,
        "ptr" | "p" | "void*" => SigSlot::Ptr,
        other => {
            // Bare type name without size — treat as i64 placeholder.
            let _ = other;
            SigSlot::I64
        }
    }
}

fn slot_align(slot: &SigSlot) -> usize {
    match slot {
        SigSlot::Void => 1,
        SigSlot::Bool => 1,
        SigSlot::I32 | SigSlot::F32 => 4,
        SigSlot::I64 | SigSlot::F64 | SigSlot::Ptr => 8,
        SigSlot::Struct { .. } => 8,
    }
}

fn slot_size(slot: &SigSlot) -> usize {
    match slot {
        SigSlot::Void => 0,
        SigSlot::Bool => 1,
        SigSlot::I32 | SigSlot::F32 => 4,
        SigSlot::I64 | SigSlot::F64 | SigSlot::Ptr => 8,
        SigSlot::Struct { size } => *size,
    }
}

fn slot_size_aligned(slot: &SigSlot) -> usize {
    let size = slot_size(slot);
    let align = slot_align(slot);
    if size == 0 {
        return 0;
    }
    (size + align - 1) & !(align - 1)
}

fn align_ptr(ptr: usize, align: usize) -> usize {
    if align == 0 {
        return ptr;
    }
    (ptr + align - 1) & !(align - 1)
}

/// Read packed C args from `args_buf` according to `sig`.
///
/// # Safety
/// `args_buf` must point at a valid buffer of at least `sig.args_bytes()` bytes
/// (or be null when there are no args).
pub unsafe fn read_args(sig: &SigDesc, args_buf: *const u8) -> Vec<Value> {
    if sig.args.is_empty() {
        return Vec::new();
    }
    assert!(!args_buf.is_null());
    let mut out = Vec::with_capacity(sig.args.len());
    let mut offset = 0usize;
    for slot in &sig.args {
        offset = align_ptr(offset, slot_align(slot));
        // SAFETY: caller guarantees args_buf spans the packed signature.
        let v = unsafe {
            let p = args_buf.add(offset);
            match slot {
                SigSlot::Void => Value::Null,
                SigSlot::Bool => Value::Bool(std::ptr::read_unaligned(p) != 0),
                SigSlot::I32 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const i32))),
                SigSlot::I64 => Value::Int(std::ptr::read_unaligned(p as *const i64)),
                SigSlot::F32 => Value::Float(f64::from(std::ptr::read_unaligned(p as *const f32))),
                SigSlot::F64 => Value::Float(std::ptr::read_unaligned(p as *const f64)),
                SigSlot::Ptr => {
                    let addr = std::ptr::read_unaligned(p as *const usize);
                    Value::ExtRef {
                        ptr: addr,
                        size: 0,
                        fields: std::sync::Arc::new(Vec::new()),
                    }
                }
                SigSlot::Struct { size } => Value::ExtRef {
                    ptr: p as usize,
                    size: *size,
                    fields: std::sync::Arc::new(Vec::new()),
                },
            }
        };
        out.push(v);
        offset += slot_size_aligned(slot);
    }
    out
}

/// Write a Super return value into `ret_buf`.
///
/// # Safety
/// `ret_buf` must be valid for `sig.ret_bytes()` writes when ret is non-void.
pub unsafe fn write_ret(sig: &SigDesc, ret_buf: *mut u8, value: &Value) {
    if matches!(sig.ret, SigSlot::Void) || ret_buf.is_null() {
        return;
    }
    // SAFETY: caller guarantees ret_buf is writable for the return slot.
    unsafe {
        match &sig.ret {
            SigSlot::Void => {}
            SigSlot::Bool => {
                std::ptr::write_unaligned(ret_buf, u8::from(value.as_bool()));
            }
            SigSlot::I32 => {
                std::ptr::write_unaligned(ret_buf as *mut i32, value.as_int() as i32);
            }
            SigSlot::I64 => {
                std::ptr::write_unaligned(ret_buf as *mut i64, value.as_int());
            }
            SigSlot::F32 => {
                std::ptr::write_unaligned(ret_buf as *mut f32, value.as_float() as f32);
            }
            SigSlot::F64 => {
                std::ptr::write_unaligned(ret_buf as *mut f64, value.as_float());
            }
            SigSlot::Ptr => {
                let addr = match value {
                    Value::ExtRef { ptr, .. } => *ptr,
                    _ => value.as_int() as usize,
                };
                std::ptr::write_unaligned(ret_buf as *mut usize, addr);
            }
            SigSlot::Struct { size } => {
                if let Value::ExtRef { ptr, .. } = value
                    && *ptr != 0
                {
                    std::ptr::copy_nonoverlapping(*ptr as *const u8, ret_buf, *size);
                }
            }
        }
    }
}

/// Pack Super values into a temporary C args buffer for calling a native host fn.
pub fn pack_args(sig: &SigDesc, args: &[Value]) -> Vec<u8> {
    let mut buf = vec![0u8; sig.args_bytes().max(1)];
    let mut offset = 0usize;
    for (i, slot) in sig.args.iter().enumerate() {
        offset = align_ptr(offset, slot_align(slot));
        let value = args.get(i).cloned().unwrap_or(Value::Null);
        let p = buf.as_mut_ptr().wrapping_add(offset);
        // SAFETY: buffer is owned and sized to fit the signature.
        unsafe {
            match slot {
                SigSlot::Void => {}
                SigSlot::Bool => std::ptr::write_unaligned(p, u8::from(value.as_bool())),
                SigSlot::I32 => {
                    std::ptr::write_unaligned(p as *mut i32, value.as_int() as i32);
                }
                SigSlot::I64 => std::ptr::write_unaligned(p as *mut i64, value.as_int()),
                SigSlot::F32 => {
                    std::ptr::write_unaligned(p as *mut f32, value.as_float() as f32);
                }
                SigSlot::F64 => std::ptr::write_unaligned(p as *mut f64, value.as_float()),
                SigSlot::Ptr => {
                    let addr = match &value {
                        Value::ExtRef { ptr, .. } => *ptr,
                        _ => value.as_int() as usize,
                    };
                    std::ptr::write_unaligned(p as *mut usize, addr);
                }
                SigSlot::Struct { size } => {
                    if let Value::ExtRef { ptr, .. } = &value
                        && *ptr != 0
                    {
                        std::ptr::copy_nonoverlapping(
                            *ptr as *const u8,
                            p,
                            (*size).min(slot_size_aligned(slot)),
                        );
                    }
                }
            }
        }
        offset += slot_size_aligned(slot);
    }
    buf
}

/// Unpack a native return buffer into a Super value.
///
/// # Safety
/// `ret_buf` must be valid for the return slot size.
pub unsafe fn unpack_ret(sig: &SigDesc, ret_buf: *const u8) -> Value {
    if matches!(sig.ret, SigSlot::Void) || ret_buf.is_null() {
        return Value::Null;
    }
    // SAFETY: caller guarantees ret_buf is readable for the return slot.
    unsafe {
        match &sig.ret {
            SigSlot::Void => Value::Null,
            SigSlot::Bool => Value::Bool(std::ptr::read_unaligned(ret_buf) != 0),
            SigSlot::I32 => Value::Int(i64::from(std::ptr::read_unaligned(ret_buf as *const i32))),
            SigSlot::I64 => Value::Int(std::ptr::read_unaligned(ret_buf as *const i64)),
            SigSlot::F32 => {
                Value::Float(f64::from(std::ptr::read_unaligned(ret_buf as *const f32)))
            }
            SigSlot::F64 => Value::Float(std::ptr::read_unaligned(ret_buf as *const f64)),
            SigSlot::Ptr => {
                let addr = std::ptr::read_unaligned(ret_buf as *const usize);
                Value::ExtRef {
                    ptr: addr,
                    size: 0,
                    fields: std::sync::Arc::new(Vec::new()),
                }
            }
            SigSlot::Struct { size } => Value::ExtRef {
                ptr: ret_buf as usize,
                size: *size,
                fields: std::sync::Arc::new(Vec::new()),
            },
        }
    }
}

/// Infer ExtField layout from a Super type name using a precomputed field list.
pub fn fields_from_pairs(pairs: &[(&str, ExtKind, u32, u32)]) -> Vec<ExtField> {
    pairs
        .iter()
        .map(|(name, kind, offset, size)| ExtField {
            name: (*name).to_string(),
            offset: *offset,
            size: *size,
            kind: *kind,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signature() {
        let s = SigDesc::parse("i64,f32->f64");
        assert_eq!(s.args.len(), 2);
        assert!(matches!(s.ret, SigSlot::F64));
    }

    #[test]
    fn roundtrip_args() {
        let sig = SigDesc::parse("i64,f64->i64");
        let vals = vec![Value::Int(7), Value::Float(1.5)];
        let buf = pack_args(&sig, &vals);
        let back = unsafe { read_args(&sig, buf.as_ptr()) };
        assert_eq!(back[0].as_int(), 7);
        assert!((back[1].as_float() - 1.5).abs() < 1e-9);
    }
}
