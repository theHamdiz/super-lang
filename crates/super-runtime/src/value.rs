//! Runtime values stored on scoped data stacks / VM registers.

use crate::sched::{Channel, SharedCell, SharedInt, TaskHandle};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use super_core::SymId;

/// C-ABI scalar kind for zero-copy external struct fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtKind {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    I128,
    F32,
    F64,
    Ptr,
}

/// Field metadata for an external `#[repr(C)]` struct view.
#[derive(Clone, Debug)]
pub struct ExtField {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub kind: ExtKind,
}

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Embedded binary payload (`embed "file.bin"`).
    Bytes(Vec<u8>),
    /// Half-open integer range `start..end` (never materializes elements).
    Range {
        start: i64,
        end: i64,
    },
    /// Copy-on-write shared storage: clones are O(1); mutation goes through `Arc::make_mut`.
    Array(Arc<Vec<Value>>),
    /// Dense unboxed f64 payload (`[0.0; N]` / `sys_arr_new(n, 0.0)`) — JIT/VM hot path.
    ArrayF64(Arc<Vec<f64>>),
    /// Dense unboxed i64 payload (`[0; N]` / `sys_arr_new(n, 0)`).
    ArrayI64(Arc<Vec<i64>>),
    Map(HashMap<String, Value>),
    Struct {
        name: Option<SymId>,
        /// Resolved type name. Anonymous records leave this empty. Equality and
        /// `Display` use it so the VM and the C backend agree on the witness.
        label: Option<String>,
        fields: HashMap<String, Value>,
    },
    Sum {
        tag: u32,
        name: String,
        fields: HashMap<String, Value>,
    },
    Optional(Option<Box<Value>>),
    Error(String),
    /// Nanoseconds.
    Duration(i64),
    Chan(Arc<Channel>),
    Task(TaskHandle),
    /// Integer hardware atomic (`shared 0`).
    SharedInt(Arc<SharedInt>),
    /// Aggregate behind a futex lock.
    Shared(Arc<SharedCell>),
    /// Zero-copy view of a host `#[repr(C)]` element (pointer as usize).
    ExtRef {
        ptr: usize,
        size: usize,
        fields: Arc<Vec<ExtField>>,
    },
    /// Zero-copy host array/slice shared with Super.
    ExtSlice {
        ptr: usize,
        len: usize,
        elem_size: usize,
        fields: Arc<Vec<ExtField>>,
    },
    /// First-class function (chunk function index).
    Func(u16),
    /// Lambda with by-value captures (`MakeClosure`).
    Closure {
        fn_idx: u16,
        env: Arc<Vec<Value>>,
    },
}

impl Value {
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Null | Value::Optional(None) => false,
            Value::Int(0) => false,
            Value::Optional(Some(v)) => v.as_bool(),
            _ => true,
        }
    }

    pub fn as_int(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => i64::from(*b),
            Value::Sum { tag, .. } => i64::from(*tag),
            Value::Optional(Some(v)) => v.as_int(),
            _ => 0,
        }
    }

    pub fn as_float(&self) -> f64 {
        match self {
            Value::Float(f) => *f,
            Value::Int(n) => *n as f64,
            Value::Optional(Some(v)) => v.as_float(),
            _ => 0.0,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Optional(Some(v)) => v.as_str(),
            Value::Error(e) => e.clone(),
            Value::Array(_)
            | Value::ArrayF64(_)
            | Value::ArrayI64(_)
            | Value::Map(_)
            | Value::Struct { .. }
            | Value::Sum { .. }
            | Value::Range { .. }
            | Value::Duration(_) => crate::traits::display(self),
            _ => String::new(),
        }
    }

    pub fn truthy_eq(a: &Value, b: &Value) -> bool {
        crate::traits::eq(a, b)
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, Value::Null | Value::Optional(None) | Value::Error(_))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&crate::traits::display(self))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        Value::truthy_eq(self, other)
    }
}
