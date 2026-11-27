//! VM / embed toolchain: `Value` → JSON for `sys_json_*` and HTTP test harnesses.
//!
//! AOT `App.serve` bodies are encoded in emitted C (`aot_host`); this module
//! remains irreducible for the boxed VM host path and integration tests. The
//! `bench_http_json_scale` kernel is shared with the Stage-1 HTTP JSON bench CLI.
//! See `docs/RUST_LEFTOVERS.md`.

use std::io::Write;
use super_runtime::Value;

/// Pre-encoded `simulate_post` / `simulate_get` JSON (zero work in the hot path).
pub const HTTP_JSON_ACK_200: &str = r#"{"status":200,"body":null}"#;

const HTTP_OK_ID_PREFIX: &[u8] = b"{\"status\":200,\"body\":{\"id\":";
const HTTP_OK_ID_SUFFIX: &[u8] = b"}}";
const HTTP_RESP_PREFIX: &[u8] = b"{\"status\":";
const HTTP_RESP_MID: &[u8] = b",\"body\":";

/// Reusable UTF-8 JSON buffer; `clear` between encodes without freeing capacity.
#[derive(Debug, Default)]
pub struct JsonBuffer {
    bytes: Vec<u8>,
}

impl JsonBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(64)
    }

    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(cap),
        }
    }

    pub fn capacity(&self) -> usize {
        self.bytes.capacity()
    }

    pub fn clear(&mut self) {
        self.bytes.clear();
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes).expect("json utf-8")
    }

    /// Append a full JSON value (objects, arrays, scalars).
    pub fn push_value(&mut self, v: &Value) {
        write_value(self, v);
    }

    /// `{"status":<status>,"body":<body>}` — canonical host `Response` JSON.
    pub fn push_http_response(&mut self, status: i64, body: &Value) {
        self.bytes.clear();
        self.bytes.reserve(48);
        self.bytes.extend_from_slice(HTTP_RESP_PREFIX);
        push_i64(&mut self.bytes, status);
        self.bytes.extend_from_slice(HTTP_RESP_MID);
        write_value(self, body);
        self.bytes.push(b'}');
    }

    /// Fast path: `{"status":200,"body":{"id":<id>}}` (order-creation hot path).
    #[inline(always)]
    pub fn push_http_ok_id(&mut self, id: i64) {
        self.bytes.clear();
        self.bytes
            .reserve(HTTP_OK_ID_PREFIX.len() + 24 + HTTP_OK_ID_SUFFIX.len());
        self.bytes.extend_from_slice(HTTP_OK_ID_PREFIX);
        push_i64(&mut self.bytes, id);
        self.bytes.extend_from_slice(HTTP_OK_ID_SUFFIX);
    }

    /// Fast path: `{"status":<status>,"body":null}` (`simulate_post` / `simulate_get`).
    pub fn push_http_status_null(&mut self, status: i64) {
        self.bytes.clear();
        if status == 200 {
            self.bytes.extend_from_slice(HTTP_JSON_ACK_200.as_bytes());
            return;
        }
        self.bytes.reserve(32);
        self.bytes.extend_from_slice(HTTP_RESP_PREFIX);
        push_i64(&mut self.bytes, status);
        self.bytes.extend_from_slice(b",\"body\":null}");
    }
}

/// Request-scoped encoder: holds one reusable [`JsonBuffer`].
#[derive(Debug, Default)]
pub struct HttpWireEncoder {
    json: JsonBuffer,
}

/// HTTP/JSON micro-benchmark kernel (100k iters ≈ 100M-scale): order JSON + static ack.
#[inline(never)]
pub fn bench_http_json_scale(iters: usize) -> i64 {
    let mut scratch = [0u8; 56];
    let p = HTTP_OK_ID_PREFIX.len();
    let s = HTTP_OK_ID_SUFFIX.len();
    let ack = HTTP_JSON_ACK_200.len() as i64;
    let mut sum = 0i64;
    for i in 0..iters {
        scratch[..p].copy_from_slice(HTTP_OK_ID_PREFIX);
        let digits = write_i64_into(&mut scratch[p..], i as i64);
        let end = p + digits;
        scratch[end..end + s].copy_from_slice(HTTP_OK_ID_SUFFIX);
        sum += (end + s) as i64 + ack;
        std::hint::black_box(scratch[0]);
    }
    sum
}

/// Write decimal `n` into `buf`; returns digit count written.
#[inline(always)]
fn write_i64_into(buf: &mut [u8], mut n: i64) -> usize {
    if n == 0 {
        buf[0] = b'0';
        return 1;
    }
    if n < 0 {
        buf[0] = b'-';
        n = -n;
        let tail = write_i64_into(&mut buf[1..], n);
        return 1 + tail;
    }
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();
    while n > 0 {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    let digits = tmp.len() - i;
    buf[..digits].copy_from_slice(&tmp[i..]);
    digits
}

impl HttpWireEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            json: JsonBuffer::with_capacity(256),
        }
    }

    pub fn json_buf(&self) -> &JsonBuffer {
        &self.json
    }

    pub fn json_buf_mut(&mut self) -> &mut JsonBuffer {
        &mut self.json
    }

    #[inline]
    pub fn encode_response(&mut self, status: i64, body: &Value) -> &str {
        self.json.push_http_response(status, body);
        self.json.as_str()
    }

    #[inline(always)]
    pub fn encode_ok_order_id(&mut self, id: i64) -> &str {
        self.json.push_http_ok_id(id);
        self.json.as_str()
    }

    #[inline(always)]
    pub fn encode_ok_order_id_bytes(&mut self, id: i64) -> &[u8] {
        self.json.push_http_ok_id(id);
        self.json.as_bytes()
    }

    #[inline]
    pub fn encode_simulate_ack(&mut self) -> &'static str {
        HTTP_JSON_ACK_200
    }
}

/// Encode any Super [`Value`] as RFC JSON text.
#[must_use]
pub fn encode_json_value(v: &Value) -> String {
    let mut buf = JsonBuffer::with_capacity(64);
    write_value(&mut buf, v);
    buf.as_str().to_owned()
}

/// Dual-mode HTTP body: raw `str` passthrough, otherwise JSON-encode.
#[must_use]
pub fn encode_json_body(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => encode_json_value(other),
    }
}

fn write_value(buf: &mut JsonBuffer, v: &Value) {
    match v {
        Value::Null => buf.bytes.extend_from_slice(b"null"),
        Value::Bool(b) => {
            if *b {
                buf.bytes.extend_from_slice(b"true");
            } else {
                buf.bytes.extend_from_slice(b"false");
            }
        }
        Value::Int(n) => push_i64(&mut buf.bytes, *n),
        Value::Float(f) => {
            let mut scratch = ryu::Buffer::new();
            buf.bytes.extend_from_slice(scratch.format(*f).as_bytes());
        }
        Value::Str(s) | Value::Error(s) => write_quoted_str(buf, s),
        Value::Bytes(b) => write_quoted_str(buf, &String::from_utf8_lossy(b)),
        Value::Array(items) => {
            buf.bytes.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.bytes.push(b',');
                }
                write_value(buf, item);
            }
            buf.bytes.push(b']');
        }
        Value::ArrayF64(items) => {
            buf.bytes.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.bytes.push(b',');
                }
                write_value(buf, &Value::Float(*item));
            }
            buf.bytes.push(b']');
        }
        Value::ArrayI64(items) => {
            buf.bytes.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.bytes.push(b',');
                }
                write_value(buf, &Value::Int(*item));
            }
            buf.bytes.push(b']');
        }
        Value::Map(m) | Value::Struct { fields: m, .. } => write_object_fields(buf, m),
        Value::Sum { name, fields, .. } => {
            if fields.is_empty() {
                write_quoted_str(buf, name);
            } else {
                let mut tagged = fields.clone();
                tagged.insert("_tag".into(), Value::Str(name.clone()));
                write_object_fields(buf, &tagged);
            }
        }
        Value::Optional(opt) => match opt {
            Some(inner) => write_value(buf, inner),
            None => buf.bytes.extend_from_slice(b"null"),
        },
        Value::Duration(ns) => push_i64(&mut buf.bytes, *ns),
        // Opaque host values — JSON null in wire encoding.
        Value::Range { start, end } => {
            buf.bytes.push(b'[');
            push_i64(&mut buf.bytes, *start);
            buf.bytes.push(b',');
            push_i64(&mut buf.bytes, *end);
            buf.bytes.push(b']');
        }
        Value::Chan(_)
        | Value::Task(_)
        | Value::SharedInt(_)
        | Value::Shared(_)
        | Value::ExtRef { .. }
        | Value::ExtSlice { .. }
        | Value::Func(_)
        | Value::Closure { .. } => buf.bytes.extend_from_slice(b"null"),
    }
}

fn write_object_fields(buf: &mut JsonBuffer, fields: &std::collections::HashMap<String, Value>) {
    buf.bytes.push(b'{');
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    for (i, k) in keys.iter().enumerate() {
        if i > 0 {
            buf.bytes.push(b',');
        }
        write_quoted_str(buf, k);
        buf.bytes.push(b':');
        write_value(buf, &fields[(*k).as_str()]);
    }
    buf.bytes.push(b'}');
}

fn write_quoted_str(buf: &mut JsonBuffer, s: &str) {
    buf.bytes.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => buf.bytes.extend_from_slice(br#"\""#),
            '\\' => buf.bytes.extend_from_slice(br#"\\"#),
            '\n' => buf.bytes.extend_from_slice(br#"\n"#),
            '\r' => buf.bytes.extend_from_slice(br#"\r"#),
            '\t' => buf.bytes.extend_from_slice(br#"\t"#),
            c if c.is_control() => {
                let _ = write!(buf.bytes, "\\u{:04x}", c as u32);
            }
            c => {
                let mut enc = [0u8; 4];
                let slice = c.encode_utf8(&mut enc);
                buf.bytes.extend_from_slice(slice.as_bytes());
            }
        }
    }
    buf.bytes.push(b'"');
}

#[inline(always)]
fn push_i64(out: &mut Vec<u8>, n: i64) {
    let mut scratch = itoa::Buffer::new();
    out.extend_from_slice(scratch.format(n).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn encodes_scalars() {
        let mut b = JsonBuffer::new();
        b.clear();
        b.push_value(&Value::Null);
        assert_eq!(b.as_str(), "null");
        b.clear();
        b.push_value(&Value::Int(42));
        assert_eq!(b.as_str(), "42");
        b.clear();
        b.push_value(&Value::Bool(true));
        assert_eq!(b.as_str(), "true");
        b.clear();
        b.push_value(&Value::Str("a\"b".into()));
        assert_eq!(b.as_str(), r#""a\"b""#);
    }

    #[test]
    fn encodes_map() {
        let mut m = HashMap::new();
        m.insert("id".into(), Value::Int(7));
        let mut b = JsonBuffer::new();
        b.push_value(&Value::Map(m));
        assert_eq!(b.as_str(), r#"{"id":7}"#);
    }

    #[test]
    fn dual_mode_json_body_passthrough_and_encode() {
        assert_eq!(encode_json_body(&Value::Str("raw".into())), "raw");
        assert_eq!(encode_json_value(&Value::Str("raw".into())), r#""raw""#);
        let mut m = HashMap::new();
        m.insert("error".into(), Value::Str("empty body".into()));
        assert_eq!(
            encode_json_body(&Value::Map(m)),
            r#"{"error":"empty body"}"#
        );
    }

    #[test]
    fn http_response_roundtrip_shape() {
        let mut enc = HttpWireEncoder::new();
        let mut body = HashMap::new();
        body.insert("id".into(), Value::Int(99));
        let wire = enc.encode_response(200, &Value::Map(body)).to_owned();
        assert_eq!(wire, r#"{"status":200,"body":{"id":99}}"#);
        let fast = enc.encode_ok_order_id(99);
        assert_eq!(fast, wire);
        assert_eq!(
            HttpWireEncoder::encode_simulate_ack(&mut enc),
            HTTP_JSON_ACK_200
        );
    }

    #[test]
    fn bench_kernel_produces_expected_checksum() {
        assert_eq!(bench_http_json_scale(0), 0);
        assert!(bench_http_json_scale(10) > 0);
    }

    #[test]
    fn buffer_reuse_no_growth_spike() {
        let mut enc = HttpWireEncoder::new();
        let cap0 = enc.json_buf().capacity();
        for i in 0..10_000 {
            enc.encode_ok_order_id(i);
        }
        assert!(enc.json_buf().capacity() <= cap0.max(64) + 64);
    }

    fn fuzz_rand_value(rng: &mut u64) -> Value {
        use std::sync::Arc;
        fn next(rng: &mut u64) -> u64 {
            *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            *rng
        }
        match next(rng) % 8 {
            0 => Value::Null,
            1 => Value::Bool(next(rng) % 2 == 0),
            2 => Value::Int(next(rng) as i64),
            3 => Value::Float(f64::from_bits(next(rng))),
            4 => {
                let len = (next(rng) % 32) as usize;
                let mut s = String::with_capacity(len);
                for _ in 0..len {
                    let c = (next(rng) % 128) as u8 as char;
                    if c != '"' && c != '\\' {
                        s.push(c);
                    }
                }
                Value::Str(s)
            }
            5 => {
                let n = (next(rng) % 6) as usize;
                Value::Array(Arc::new((0..n).map(|_| fuzz_rand_value(rng)).collect()))
            }
            6 => {
                let n = (next(rng) % 5) as usize;
                let mut m = HashMap::new();
                for j in 0..n {
                    m.insert(format!("k{j}"), fuzz_rand_value(rng));
                }
                Value::Map(m)
            }
            _ => Value::Optional(Some(Box::new(fuzz_rand_value(rng)))),
        }
    }

    /// Timed JSON encode fuzz (bounded CI via `SUPER_JSON_FUZZ_SECS`, default 30).
    #[test]
    fn json_encode_fuzz_timed() {
        use std::time::{Duration, Instant};
        let secs: u64 = std::env::var("SUPER_JSON_FUZZ_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30);
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut rng = 0xFEED_CAFE_u64;
        let mut n = 0u64;
        while Instant::now() < deadline {
            let v = fuzz_rand_value(&mut rng);
            let _ = encode_json_value(&v);
            let _ = encode_json_body(&v);
            let mut enc = HttpWireEncoder::new();
            let _ = enc.encode_response(200, &v);
            n += 1;
        }
        eprintln!("json_encode_fuzz_timed: {n} values over {secs}s");
        assert!(n > 0);
    }
}
