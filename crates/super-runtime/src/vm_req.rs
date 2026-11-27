//! Bytecode-VM request / connection scoped reclamation (mirrors AOT `sup_req_*`).
//!
//! Stay births are tracked per request and dropped at `leave`; Promote runs a
//! deep clone into durable storage. Uses `stacks::DataStack` for connection
//! string copies (`keep_str`).

use crate::stacks::DataStack;
use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) const SUP_REG_REQ: u8 = 1;
pub(crate) const SUP_REG_DURABLE: u8 = 2;
pub const VM_REQ_NESTED: i64 = -1;

struct ScopeArena {
    bump: DataStack,
    depth: i64,
    pending: Vec<Value>,
    stay_objs: u64,
    promoted_objs: u64,
    arena_bytes: u64,
    enter_count: u64,
    leave_count: u64,
    last_reclaimed: u64,
}

impl ScopeArena {
    fn new(capacity: usize) -> Self {
        Self {
            bump: DataStack::with_capacity(capacity),
            depth: 0,
            pending: Vec::new(),
            stay_objs: 0,
            promoted_objs: 0,
            arena_bytes: 0,
            enter_count: 0,
            leave_count: 0,
            last_reclaimed: 0,
        }
    }

    fn enter(&mut self) -> i64 {
        self.enter_count += 1;
        if self.depth > 0 {
            return VM_REQ_NESTED;
        }
        self.depth = 1;
        self.bump.mark();
        self.pending.clear();
        self.stay_objs = 0;
        self.promoted_objs = 0;
        self.arena_bytes = 0;
        self.bump.used() as i64
    }

    fn leave(&mut self, mark: i64) -> bool {
        if mark == VM_REQ_NESTED {
            return true;
        }
        if self.depth <= 0 {
            return true;
        }
        self.leave_count += 1;
        let nbytes = self
            .pending
            .iter()
            .map(estimate_value_bytes)
            .sum::<u64>()
            .saturating_add(self.bump.used() as u64);
        self.last_reclaimed = nbytes;
        self.pending.clear();
        self.bump.rewind();
        self.depth = 0;
        self.stay_objs = 0;
        self.arena_bytes = 0;
        true
    }

    fn track_stay(&mut self, v: Value) -> Value {
        self.stay_objs += 1;
        self.arena_bytes = self
            .arena_bytes
            .saturating_add(estimate_value_bytes(&v));
        self.pending.push(v.clone());
        v
    }
}

struct VmReqState {
    req: ScopeArena,
    conn: ScopeArena,
}

impl Default for VmReqState {
    fn default() -> Self {
        Self {
            req: ScopeArena::new(64 << 20),
            conn: ScopeArena::new(16 << 20),
        }
    }
}

thread_local! {
    static STATE: RefCell<VmReqState> = RefCell::new(VmReqState::default());
}

fn with_state<R>(f: impl FnOnce(&mut VmReqState) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// True while a top-level request scope is active on this thread.
pub fn req_active() -> bool {
    with_state(|s| s.req.depth > 0)
}

/// Register a bytecode birth for reclaim at request leave.
pub fn on_birth(region: Option<u8>, v: Value) -> Value {
    with_state(|s| {
        if s.req.depth <= 0 {
            return v;
        }
        if matches!(region, Some(SUP_REG_DURABLE)) {
            return v;
        }
        // Stay (`SUP_REG_REQ`) and runtime Auto (`None`) both bulk-reclaim at leave.
        let _ = SUP_REG_REQ;
        s.req.track_stay(v)
    })
}

pub fn req_enter() -> i64 {
    with_state(|s| s.req.enter())
}

pub fn req_leave(mark: i64) -> bool {
    with_state(|s| s.req.leave(mark))
}

pub fn conn_enter() -> i64 {
    with_state(|s| s.conn.enter())
}

pub fn conn_leave(mark: i64) -> i64 {
    with_state(|s| {
        s.conn.leave(mark);
        s.conn.last_reclaimed as i64
    })
}

pub fn req_promote(v: Value) -> Value {
    with_state(|s| {
        s.req.promoted_objs += 1;
        deep_clone(&v)
    })
}

pub fn req_keep_str(v: &Value) -> Value {
    let text = v.as_str();
    with_state(|st| {
        if st.conn.depth > 0 {
            if let Some(off) = st.conn.bump.alloc(text.len().max(1) + 1, 1) {
                let ptr = st.conn.bump.ptr_at(off);
                unsafe {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), ptr, text.len());
                    *ptr.add(text.len()) = 0;
                }
                st.conn.arena_bytes = st
                    .conn
                    .arena_bytes
                    .saturating_add(text.len() as u64 + 1);
            }
        }
        Value::Str(text)
    })
}

pub fn req_stats() -> Value {
    with_state(|s| {
        let mut m = HashMap::new();
        m.insert(
            "arena_bytes".into(),
            Value::Int(s.req.arena_bytes.min(i64::MAX as u64) as i64),
        );
        m.insert(
            "promoted_objs".into(),
            Value::Int(s.req.promoted_objs.min(i64::MAX as u64) as i64),
        );
        m.insert(
            "stay_objs".into(),
            Value::Int(s.req.stay_objs.min(i64::MAX as u64) as i64),
        );
        m.insert(
            "pending".into(),
            Value::Int(s.req.pending.len() as i64),
        );
        Value::Map(m)
    })
}

pub fn conn_stats() -> Value {
    with_state(|s| {
        let mut m = HashMap::new();
        m.insert(
            "arena_bytes".into(),
            Value::Int(s.conn.bump.used() as i64),
        );
        m.insert(
            "reclaimed_bytes".into(),
            Value::Int(s.conn.last_reclaimed.min(i64::MAX as u64) as i64),
        );
        m.insert(
            "enter_count".into(),
            Value::Int(s.conn.enter_count.min(i64::MAX as u64) as i64),
        );
        m.insert(
            "leave_count".into(),
            Value::Int(s.conn.leave_count.min(i64::MAX as u64) as i64),
        );
        Value::Map(m)
    })
}

/// Approximate heap footprint for diagnostics (not exact C-ABI object sizes).
pub fn estimate_value_bytes(v: &Value) -> u64 {
    match v {
        Value::Str(s) => s.len() as u64 + 24,
        Value::Bytes(b) => b.len() as u64 + 24,
        Value::Array(a) => 32 + a.len() as u64 * 16,
        Value::ArrayF64(a) => 32 + a.len() as u64 * 8,
        Value::ArrayI64(a) => 32 + a.len() as u64 * 8,
        Value::Map(m) => {
            32 + m.iter().map(|(k, v)| k.len() as u64 + estimate_value_bytes(v)).sum::<u64>()
        }
        Value::Struct { fields, .. } | Value::Sum { fields, .. } => {
            32 + fields
                .values()
                .map(estimate_value_bytes)
                .sum::<u64>()
        }
        Value::Optional(Some(inner)) => estimate_value_bytes(inner),
        Value::Closure { env, .. } => 24 + env.iter().map(estimate_value_bytes).sum::<u64>(),
        _ => 16,
    }
}

pub fn deep_clone(v: &Value) -> Value {
    match v {
        Value::Str(s) => Value::Str(s.clone()),
        Value::Bytes(b) => Value::Bytes(b.clone()),
        Value::Array(a) => Value::Array(Arc::new(a.iter().map(deep_clone).collect())),
        Value::ArrayF64(a) => Value::ArrayF64(Arc::new(a.to_vec())),
        Value::ArrayI64(a) => Value::ArrayI64(Arc::new(a.to_vec())),
        Value::Map(m) => {
            let out: HashMap<String, Value> = m.iter().map(|(k, v)| (k.clone(), deep_clone(v))).collect();
            Value::Map(out)
        }
        Value::Struct { name, label, fields } => Value::Struct {
            name: *name,
            label: label.clone(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.clone(), deep_clone(v)))
                .collect(),
        },
        Value::Sum { tag, name, fields } => Value::Sum {
            tag: *tag,
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.clone(), deep_clone(v)))
                .collect(),
        },
        Value::Optional(Some(inner)) => Value::Optional(Some(Box::new(deep_clone(inner)))),
        Value::Closure { fn_idx, env } => Value::Closure {
            fn_idx: *fn_idx,
            env: Arc::new(env.iter().map(deep_clone).collect()),
        },
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn req_enter_leave_drops_pending_stays() {
        req_leave(0);
        let mark = req_enter();
        assert_ne!(mark, VM_REQ_NESTED);
        let _ = on_birth(Some(SUP_REG_REQ), Value::Str("temp".into()));
        let _ = on_birth(None, Value::Map(HashMap::from([("k".into(), Value::Int(1))])));
        let stats = req_stats();
        let Value::Map(m) = stats else {
            panic!("stats map");
        };
        assert_eq!(m.get("stay_objs").map(Value::as_int), Some(2));
        assert_eq!(m.get("pending").map(Value::as_int), Some(2));
        assert!(req_leave(mark));
        let after = req_stats();
        let Value::Map(m2) = after else {
            panic!("stats");
        };
        assert_eq!(m2.get("pending").map(Value::as_int), Some(0));
        assert_eq!(m2.get("pending").map(Value::as_int), Some(0));
    }

    #[test]
    fn durable_birth_not_tracked() {
        let mark = req_enter();
        let _ = on_birth(Some(SUP_REG_DURABLE), Value::Str("durable".into()));
        let stats = req_stats();
        let Value::Map(m) = stats else {
            panic!("stats");
        };
        assert_eq!(m.get("stay_objs").map(Value::as_int), Some(0));
        req_leave(mark);
    }

    #[test]
    fn promote_deep_clones() {
        let v = Value::Struct {
            name: None,
            label: Some("T".into()),
            fields: HashMap::from([("n".into(), Value::Int(42))]),
        };
        let p = req_promote(v.clone());
        assert!(Value::truthy_eq(&v, &p));
        assert!(matches!(&p, Value::Struct { fields, .. } if fields.get("n").map(Value::as_int) == Some(42)));
    }

    #[test]
    fn conn_enter_leave_rewinds_bump() {
        let mark = conn_enter();
        let _ = req_keep_str(&Value::Str("peer".into()));
        let before = conn_stats();
        let reclaimed = conn_leave(mark);
        assert!(reclaimed >= 0);
        let Value::Map(m) = before else {
            panic!("conn stats");
        };
        assert!(m.get("arena_bytes").map(Value::as_int).unwrap_or(0) > 0);
    }
}