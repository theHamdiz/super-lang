//! Builtin witnesses for the language traits.
//!
//! The VM, the fiber interpreter, and the C23 backend all implement this
//! contract. Display, equality, ordering, and hashing of arrays, maps, and
//! structs go through here so a hole such as `"{scores}"` has one meaning on
//! every backend. A user type that declares `Display` (or `Eq`, `Add`, …)
//! replaces the witness with a direct call; this module is the fallback.

use crate::value::Value;
use std::collections::HashMap;

const DEPTH_LIMIT: u32 = 32;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

pub fn display(value: &Value) -> String {
    let mut out = String::new();
    write_show(&mut out, value, false, 0);
    out
}

pub fn debug(value: &Value) -> String {
    let mut out = String::new();
    write_show(&mut out, value, true, 0);
    out
}

/// FNV-1a of the display text. Equal values display the same, so they hash
/// the same; the C backend hashes the same bytes.
pub fn hash_value(value: &Value) -> i64 {
    fnv(display(value).as_bytes()) as i64
}

pub fn eq(left: &Value, right: &Value) -> bool {
    eq_depth(left, right, 0)
}

/// `Some(-1 | 0 | 1)` when the two values are ordered by a language trait.
/// `None` leaves mixed numeric comparisons on the existing arithmetic path.
pub fn ord(left: &Value, right: &Value) -> Option<i8> {
    ord_depth(left, right, 0)
}

pub fn add(left: &Value, right: &Value) -> Value {
    match (left, right) {
        (Value::Str(_), _) | (_, Value::Str(_)) | (Value::Bytes(_), _) | (_, Value::Bytes(_)) => {
            Value::Str(format!("{}{}", display(left), display(right)))
        }
        (Value::Array(xs), Value::Array(ys)) => {
            let mut out = Vec::with_capacity(xs.len() + ys.len());
            out.extend(xs.iter().cloned());
            out.extend(ys.iter().cloned());
            Value::Array(out.into())
        }
        (Value::Float(_), _) | (_, Value::Float(_)) => {
            Value::Float(left.as_float() + right.as_float())
        }
        _ => Value::Int(left.as_int().wrapping_add(right.as_int())),
    }
}

/// Host entry for `value.display()`, `value.eq(other)`, and the other builtin
/// witnesses. `None` means the name is not a language-trait intrinsic.
pub fn host_call(name: &str, args: &[Value]) -> Option<Value> {
    let bare = name.rsplit_once('.').map(|(_, bare)| bare).unwrap_or(name);
    Some(match (bare, args) {
        ("display" | "to_str", [value, ..]) => Value::Str(display(value)),
        ("debug", [value, ..]) => Value::Str(debug(value)),
        ("hash", [value, ..]) => Value::Int(hash_value(value)),
        ("clone", [value, ..]) => value.clone(),
        ("eq", [left, right, ..]) => Value::Bool(eq(left, right)),
        ("cmp", [left, right, ..]) => Value::Int(i64::from(ord(left, right).unwrap_or(0))),
        _ => return None,
    })
}

fn eq_depth(left: &Value, right: &Value, depth: u32) -> bool {
    if depth > DEPTH_LIMIT {
        return false;
    }
    match (left, right) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < 1e-9,
        (Value::Int(x), Value::Float(y)) | (Value::Float(y), Value::Int(x)) => {
            (*x as f64 - *y).abs() < 1e-9
        }
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (Value::Optional(None), Value::Optional(None)) => true,
        (Value::Optional(Some(x)), Value::Optional(Some(y))) => eq_depth(x, y, depth + 1),
        (Value::Error(x), Value::Error(y)) => x == y,
        (Value::Duration(x), Value::Duration(y)) => x == y,
        (Value::Bytes(x), Value::Bytes(y)) => x == y,
        (Value::Range { start: a, end: b }, Value::Range { start: c, end: d }) => a == c && b == d,
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| eq_depth(x, y, depth + 1))
        }
        (Value::ArrayI64(a), Value::ArrayI64(b)) => a.as_slice() == b.as_slice(),
        (Value::ArrayF64(a), Value::ArrayF64(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-9)
        }
        (Value::ArrayI64(a), Value::Array(b)) | (Value::Array(b), Value::ArrayI64(a)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(x, y)| eq_depth(&Value::Int(*x), y, depth + 1))
        }
        (Value::Map(a), Value::Map(b)) => map_eq(a, b, depth),
        (
            Value::Struct {
                label: la,
                fields: fa,
                ..
            },
            Value::Struct {
                label: lb,
                fields: fb,
                ..
            },
        ) => la == lb && map_eq(fa, fb, depth),
        (
            Value::Sum {
                tag: ta,
                name: na,
                fields: fa,
            },
            Value::Sum {
                tag: tb,
                name: nb,
                fields: fb,
            },
        ) => {
            if ta != tb || na != nb {
                return false;
            }
            // Tag-only pattern: empty fields on either side match by discriminant.
            if fa.is_empty() || fb.is_empty() {
                return true;
            }
            map_eq(fa, fb, depth)
        }
        (Value::Int(n), Value::Sum { tag, .. }) | (Value::Sum { tag, .. }, Value::Int(n)) => {
            *n == i64::from(*tag)
        }
        (Value::SharedInt(a), Value::SharedInt(b)) => a.load() == b.load(),
        _ => false,
    }
}

fn map_eq(left: &HashMap<String, Value>, right: &HashMap<String, Value>, depth: u32) -> bool {
    left.len() == right.len()
        && left.iter().all(|(key, value)| {
            right
                .get(key)
                .is_some_and(|other| eq_depth(value, other, depth + 1))
        })
}

fn ord_depth(left: &Value, right: &Value, depth: u32) -> Option<i8> {
    if depth > DEPTH_LIMIT {
        return None;
    }
    match (left, right) {
        (Value::Int(x), Value::Int(y)) => Some(cmp_ord(x, y)),
        (Value::Bool(x), Value::Bool(y)) => Some(cmp_ord(&u8::from(*x), &u8::from(*y))),
        (Value::Str(x), Value::Str(y)) => Some(cmp_ord(x, y)),
        (Value::Duration(x), Value::Duration(y)) => Some(cmp_ord(x, y)),
        (Value::Range { start: a, end: b }, Value::Range { start: c, end: d }) => {
            Some(cmp_ord(&(a, b), &(c, d)))
        }
        (Value::Array(a), Value::Array(b)) => {
            let n = a.len().min(b.len());
            for i in 0..n {
                let order = ord_depth(&a[i], &b[i], depth + 1)?;
                if order != 0 {
                    return Some(order);
                }
            }
            Some(cmp_ord(&a.len(), &b.len()))
        }
        (Value::ArrayI64(a), Value::ArrayI64(b)) => {
            let n = a.len().min(b.len());
            for i in 0..n {
                let order = cmp_ord(&a[i], &b[i]);
                if order != 0 {
                    return Some(order);
                }
            }
            Some(cmp_ord(&a.len(), &b.len()))
        }
        _ => None,
    }
}

fn cmp_ord<T: Ord>(left: &T, right: &T) -> i8 {
    match left.cmp(right) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn write_show(out: &mut String, value: &Value, quote: bool, depth: u32) {
    if depth > DEPTH_LIMIT {
        out.push_str("...");
        return;
    }
    match value {
        Value::Null | Value::Optional(None) => out.push_str("none"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Int(value) => out.push_str(&value.to_string()),
        Value::Float(value) => push_float(out, *value),
        Value::Str(value) if quote => push_quoted(out, value),
        Value::Str(value) => out.push_str(value),
        Value::Bytes(value) => {
            out.push_str("bytes[");
            out.push_str(&value.len().to_string());
            out.push(']');
        }
        Value::Range { start, end } => {
            out.push_str(&start.to_string());
            out.push_str("..");
            out.push_str(&end.to_string());
        }
        Value::Array(values) => write_list(out, values.iter(), depth),
        Value::ArrayI64(values) => {
            out.push('[');
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&value.to_string());
            }
            out.push(']');
        }
        Value::ArrayF64(values) => {
            out.push('[');
            for (i, value) in values.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                push_float(out, *value);
            }
            out.push(']');
        }
        Value::Map(fields) => write_fields(out, None, fields, depth),
        Value::Struct { label, fields, .. } => write_fields(out, label.as_deref(), fields, depth),
        Value::Sum { name, fields, .. } => {
            if fields.is_empty() {
                out.push_str(name);
            } else {
                write_fields(out, Some(name.as_str()), fields, depth);
            }
        }
        Value::Optional(Some(value)) => write_show(out, value, quote, depth + 1),
        Value::Error(message) => {
            out.push_str("error(");
            push_quoted(out, message);
            out.push(')');
        }
        Value::Duration(ns) => {
            out.push_str(&ns.to_string());
            out.push_str("ns");
        }
        Value::Chan(channel) => {
            out.push_str("chan(len=");
            out.push_str(&channel.len().to_string());
            out.push(')');
        }
        Value::Task(task) => {
            out.push_str("task(");
            out.push_str(&task.id.to_string());
            out.push(')');
        }
        Value::SharedInt(cell) => {
            out.push_str("shared(");
            out.push_str(&cell.load().to_string());
            out.push(')');
        }
        Value::Shared(cell) => {
            out.push_str("shared(");
            write_show(out, &cell.load(), true, depth + 1);
            out.push(')');
        }
        Value::ExtRef { ptr, size, .. } => {
            out.push_str("extref(");
            out.push_str(&format!("{ptr:#x},{size}"));
            out.push(')');
        }
        Value::ExtSlice { len, elem_size, .. } => {
            out.push_str("extslice[");
            out.push_str(&len.to_string());
            out.push('x');
            out.push_str(&elem_size.to_string());
            out.push(']');
        }
        Value::Func(idx) => {
            out.push_str("fn#");
            out.push_str(&idx.to_string());
        }
        Value::Closure { fn_idx, env } => {
            out.push_str("closure#");
            out.push_str(&fn_idx.to_string());
            out.push('[');
            out.push_str(&env.len().to_string());
            out.push(']');
        }
    }
}

fn write_list<'a, I>(out: &mut String, values: I, depth: u32)
where
    I: Iterator<Item = &'a Value>,
{
    out.push('[');
    for (i, value) in values.enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_show(out, value, true, depth + 1);
    }
    out.push(']');
}

fn write_fields(
    out: &mut String,
    label: Option<&str>,
    fields: &HashMap<String, Value>,
    depth: u32,
) {
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        out.push_str(label);
    }
    out.push('{');
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(key);
        out.push_str(": ");
        write_show(out, &fields[*key], true, depth + 1);
    }
    out.push('}');
}

fn push_float(out: &mut String, value: f64) {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        out.push_str(&(value as i64).to_string());
    } else {
        out.push_str(&format!("{value}"));
    }
}

fn push_quoted(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn array(values: Vec<Value>) -> Value {
        Value::Array(Arc::new(values))
    }

    #[test]
    fn display_renders_arrays_maps_and_structs() {
        let scores = array(vec![Value::Int(10), Value::Int(20), Value::Int(30)]);
        assert_eq!(display(&scores), "[10, 20, 30]");
        let mut ports = HashMap::new();
        ports.insert("https".into(), Value::Int(443));
        ports.insert("http".into(), Value::Int(80));
        assert_eq!(display(&Value::Map(ports)), "{http: 80, https: 443}");
        let mut fields = HashMap::new();
        fields.insert("name".into(), Value::Str("Amina".into()));
        fields.insert("id".into(), Value::Int(7));
        let user = Value::Struct {
            name: None,
            label: Some("User".into()),
            fields,
        };
        assert_eq!(display(&user), r#"User{id: 7, name: "Amina"}"#);
        assert_eq!(debug(&Value::Str("hi".into())), r#""hi""#);
        assert_eq!(display(&Value::Str("hi".into())), "hi");
    }

    #[test]
    fn eq_is_reflexive_and_hash_follows_display() {
        let scores = array(vec![Value::Int(1), Value::Int(2)]);
        let again = array(vec![Value::Int(1), Value::Int(2)]);
        let other = array(vec![Value::Int(1), Value::Int(3)]);
        assert!(eq(&scores, &scores));
        assert!(eq(&scores, &again));
        assert!(!eq(&scores, &other));
        assert_eq!(hash_value(&scores), hash_value(&again));
        assert_ne!(hash_value(&scores), hash_value(&other));
        assert_eq!(
            ord(&Value::Str("a".into()), &Value::Str("b".into())),
            Some(-1)
        );
        assert_eq!(ord(&Value::Bool(false), &Value::Bool(true)), Some(-1));
    }

    #[test]
    fn string_plus_array_uses_display() {
        let scores = array(vec![Value::Int(1), Value::Int(2)]);
        let text = add(&Value::Str("scores ".into()), &scores);
        assert_eq!(text, Value::Str("scores [1, 2]".into()));
    }
}
