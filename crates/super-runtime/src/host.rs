//! Host environment: registered native callbacks and zero-copy external slices.

use crate::value::{ExtField, ExtKind, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Host-callable function: receives Super values, returns a Super value.
pub type HostCallback = Arc<dyn Fn(&[Value]) -> Value + Send + Sync>;

/// Bound external memory shared zero-copy with Super `for` loops.
#[derive(Clone, Debug)]
pub struct ExtSliceBinding {
    pub ptr: usize,
    pub len: usize,
    pub elem_size: usize,
    pub fields: Arc<Vec<ExtField>>,
}

impl ExtSliceBinding {
    pub fn as_value(&self) -> Value {
        Value::ExtSlice {
            ptr: self.ptr,
            len: self.len,
            elem_size: self.elem_size,
            fields: Arc::clone(&self.fields),
        }
    }

    pub fn element_at(&self, index: usize) -> Option<Value> {
        if index >= self.len {
            return None;
        }
        Some(Value::ExtRef {
            ptr: self.ptr + index * self.elem_size,
            size: self.elem_size,
            fields: Arc::clone(&self.fields),
        })
    }
}

/// Shared host registry consulted by the VM for `CallHost` and bound slices.
#[derive(Default, Clone)]
pub struct HostEnv {
    pub fns: HashMap<String, HostCallback>,
    pub slices: HashMap<String, ExtSliceBinding>,
}

impl HostEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_fn(&mut self, key: impl Into<String>, f: HostCallback) {
        self.fns.insert(key.into(), f);
    }

    pub fn bind_slice(&mut self, name: impl Into<String>, binding: ExtSliceBinding) {
        self.slices.insert(name.into(), binding);
    }

    pub fn call(&self, name: &str, args: &[Value]) -> Option<Value> {
        self.fns.get(name).map(|f| f(args))
    }

    pub fn slice(&self, name: &str) -> Option<&ExtSliceBinding> {
        self.slices.get(name)
    }
}

/// Build ExtField list from a flat C-ABI layout descriptor string.
/// Format: `"x:f32,y:f32,vx:f32,vy:f32"` or `"hp:f32,id:i64"`.
pub fn parse_layout_descriptor(desc: &str) -> (Vec<ExtField>, usize) {
    let mut fields = Vec::new();
    let mut offset = 0u32;
    let mut max_align = 1u32;
    for part in desc.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, ty) = match part.split_once(':') {
            Some((n, t)) => (n.trim(), t.trim()),
            None => continue,
        };
        let (kind, size, align) = match ty {
            "bool" | "b" => (ExtKind::Bool, 1u32, 1u32),
            "u8" => (ExtKind::U8, 1, 1),
            "i8" => (ExtKind::I8, 1, 1),
            "u16" => (ExtKind::U16, 2, 2),
            "i16" => (ExtKind::I16, 2, 2),
            "u32" => (ExtKind::U32, 4, 4),
            "i32" | "int" => (ExtKind::I32, 4, 4),
            "u64" | "usize" => (ExtKind::U64, 8, 8),
            "i64" | "i" | "isize" => (ExtKind::I64, 8, 8),
            "i128" => (ExtKind::I128, 16, 16),
            "f32" | "float" => (ExtKind::F32, 4, 4),
            "f64" | "d" | "double" => (ExtKind::F64, 8, 8),
            "ptr" | "p" => (ExtKind::Ptr, 8, 8),
            _ => (ExtKind::I64, 8, 8),
        };
        offset = align_up(offset, align);
        max_align = max_align.max(align);
        fields.push(ExtField {
            name: name.to_string(),
            offset,
            size,
            kind,
        });
        offset += size;
    }
    let size = align_up(offset, max_align.max(1)) as usize;
    (fields, size)
}

fn align_up(offset: u32, align: u32) -> u32 {
    if align == 0 {
        return offset;
    }
    (offset + align - 1) & !(align - 1)
}

/// Read a packed C field from external memory.
pub fn ext_read(ptr: usize, field: &ExtField) -> Value {
    let p = (ptr as *const u8).wrapping_add(field.offset as usize);
    // SAFETY: host guarantees the bound slice lifetime and layout.
    unsafe {
        match field.kind {
            ExtKind::Bool => Value::Bool(std::ptr::read_unaligned(p) != 0),
            ExtKind::U8 => Value::Int(i64::from(std::ptr::read_unaligned(p))),
            ExtKind::I8 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const i8))),
            ExtKind::U16 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const u16))),
            ExtKind::I16 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const i16))),
            ExtKind::U32 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const u32))),
            ExtKind::I32 => Value::Int(i64::from(std::ptr::read_unaligned(p as *const i32))),
            ExtKind::U64 => Value::Int(std::ptr::read_unaligned(p as *const u64) as i64),
            ExtKind::I64 => Value::Int(std::ptr::read_unaligned(p as *const i64)),
            ExtKind::I128 => {
                let n = std::ptr::read_unaligned(p as *const i128);
                Value::Int(n as i64)
            }
            ExtKind::F32 => Value::Float(f64::from(std::ptr::read_unaligned(p as *const f32))),
            ExtKind::F64 => Value::Float(std::ptr::read_unaligned(p as *const f64)),
            ExtKind::Ptr => Value::Int(std::ptr::read_unaligned(p as *const usize) as i64),
        }
    }
}

/// Write a Super value into a packed C field.
pub fn ext_write(ptr: usize, field: &ExtField, value: &Value) {
    let p = (ptr as *mut u8).wrapping_add(field.offset as usize);
    // SAFETY: host guarantees the bound slice lifetime and layout.
    unsafe {
        match field.kind {
            ExtKind::Bool => std::ptr::write_unaligned(p, u8::from(value.as_bool())),
            ExtKind::U8 => std::ptr::write_unaligned(p, value.as_int() as u8),
            ExtKind::I8 => std::ptr::write_unaligned(p as *mut i8, value.as_int() as i8),
            ExtKind::U16 => std::ptr::write_unaligned(p as *mut u16, value.as_int() as u16),
            ExtKind::I16 => std::ptr::write_unaligned(p as *mut i16, value.as_int() as i16),
            ExtKind::U32 => std::ptr::write_unaligned(p as *mut u32, value.as_int() as u32),
            ExtKind::I32 => std::ptr::write_unaligned(p as *mut i32, value.as_int() as i32),
            ExtKind::U64 => std::ptr::write_unaligned(p as *mut u64, value.as_int() as u64),
            ExtKind::I64 => std::ptr::write_unaligned(p as *mut i64, value.as_int()),
            ExtKind::I128 => {
                let n = value.as_int() as i128;
                std::ptr::write_unaligned(p as *mut i128, n);
            }
            ExtKind::F32 => std::ptr::write_unaligned(p as *mut f32, value.as_float() as f32),
            ExtKind::F64 => std::ptr::write_unaligned(p as *mut f64, value.as_float()),
            ExtKind::Ptr => {
                std::ptr::write_unaligned(p as *mut usize, value.as_int() as usize);
            }
        }
    }
}

#[cfg(test)]
mod scalar_abi_tests {
    use super::*;
    use crate::value::ExtKind;

    #[test]
    fn fixed_width_ext_layout_round_trips() {
        let (fields, size) =
            parse_layout_descriptor("a:u8,b:i8,c:u16,d:i16,e:u32,f:i32,g:u64,h:i64,i:i128");
        assert_eq!(fields.len(), 9);
        assert!(size >= 1 + 1 + 2 + 2 + 4 + 4 + 8 + 8 + 16);
        // Pack into aligned buffer and round-trip each field.
        let mut buf = vec![0u8; size + 16];
        let ptr = buf.as_mut_ptr() as usize;
        let samples: &[(usize, i64)] = &[
            (0, 255),
            (1, -5),
            (2, 65535),
            (3, -300),
            (4, 4_000_000_000),
            (5, -2_000_000_000),
            (6, -1), // as u64 bits via i64
            (7, i64::MIN / 2),
            (8, 42),
        ];
        for &(idx, v) in samples {
            ext_write(ptr, &fields[idx], &Value::Int(v));
        }
        assert_eq!(ext_read(ptr, &fields[0]).as_int(), 255);
        assert_eq!(ext_read(ptr, &fields[1]).as_int(), -5);
        assert_eq!(ext_read(ptr, &fields[2]).as_int(), 65535);
        assert_eq!(ext_read(ptr, &fields[3]).as_int(), -300);
        assert_eq!(ext_read(ptr, &fields[4]).as_int(), 4_000_000_000);
        assert_eq!(ext_read(ptr, &fields[5]).as_int(), -2_000_000_000);
        assert_eq!(ext_read(ptr, &fields[7]).as_int(), i64::MIN / 2);
        assert_eq!(ext_read(ptr, &fields[8]).as_int(), 42);
        // Sizes / aligns match C ABI expectations.
        assert_eq!(fields[0].kind, ExtKind::U8);
        assert_eq!(fields[0].size, 1);
        assert_eq!(fields[2].kind, ExtKind::U16);
        assert_eq!(fields[2].size, 2);
        assert_eq!(fields[4].kind, ExtKind::U32);
        assert_eq!(fields[4].size, 4);
        assert_eq!(fields[6].kind, ExtKind::U64);
        assert_eq!(fields[6].size, 8);
        assert_eq!(fields[8].kind, ExtKind::I128);
        assert_eq!(fields[8].size, 16);
    }
}
