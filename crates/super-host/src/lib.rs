//! Runtime **host builtins** for Super — Tier-1 `sys_*` / crypto / IO kernels,
//! plus a few systems peer helpers used by the CLI harness.
//!
//! The Super **standard library** is only `stdlib/**/*.sr`. Those modules call
//! into this crate through the language `host` surface (`host.sys_*`). This
//! crate must never be named or documented as “the Super stdlib.”

#![deny(unsafe_op_in_unsafe_fn)]
// Kernels walk several parallel buffers by index.
#![allow(clippy::needless_range_loop)]

use std::collections::HashMap;
use super_runtime::Value;

pub mod ecs;
pub mod host_net;
pub mod host_sys;
pub mod http;
pub mod pq_crypto;

pub use host_net::{HttpWireEncoder, JsonBuffer, encode_json_body, encode_json_value};

pub use host_sys::{default_host_env, register_host_sys};

/// Built-in collection helpers used by UFCS pipelines.
pub fn map_identity(recv: Value) -> Value {
    recv
}

pub fn keep_identity(recv: Value) -> Value {
    recv
}

pub fn sum_array(recv: Value) -> Value {
    match recv {
        Value::Array(a) => Value::Float(a.iter().map(|v| v.as_float()).sum()),
        other => other,
    }
}

pub fn group_by_stub(recv: Value) -> Value {
    match recv {
        Value::Array(a) => {
            let mut m = HashMap::new();
            m.insert("all".into(), Value::Array(a));
            Value::Map(m)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_works() {
        let v = sum_array(Value::Array(
            vec![Value::Int(1), Value::Int(2), Value::Int(3)].into(),
        ));
        assert_eq!(v.as_float(), 6.0);
    }
}
