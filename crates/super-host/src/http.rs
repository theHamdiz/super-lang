//! Request-scoped HTTP helpers and simulation for tests.

use std::collections::HashMap;
use super_runtime::Value;

use crate::host_net::{HttpWireEncoder, JsonBuffer};

#[derive(Clone, Debug)]
pub struct Response {
    pub status: i64,
    pub body: Value,
}

impl Response {
    pub fn to_value(&self) -> Value {
        let mut fields = HashMap::new();
        fields.insert("status".into(), Value::Int(self.status));
        fields.insert("body".into(), self.body.clone());
        Value::Struct {
            name: None,
            label: None,
            fields,
        }
    }

    /// Zero-allocation JSON wire body into a reusable buffer (destination-driven).
    pub fn encode_json<'a>(&self, buf: &'a mut JsonBuffer) -> &'a str {
        buf.push_http_response(self.status, &self.body);
        buf.as_str()
    }
}

pub fn ok(body: Value) -> Response {
    Response { status: 200, body }
}

pub fn bad_request(body: Value) -> Response {
    Response { status: 400, body }
}

pub fn not_found(body: Value) -> Response {
    Response { status: 404, body }
}

pub fn simulate_post(_path: &str, _body: Value) -> Response {
    Response {
        status: 200,
        body: Value::Null,
    }
}

pub fn simulate_get(_path: &str) -> Response {
    Response {
        status: 200,
        body: Value::Null,
    }
}

/// Reusable encoder for the HTTP/JSON hot path (`encode_ok_order_id`, `encode_response`, …).
pub fn wire_encoder() -> HttpWireEncoder {
    HttpWireEncoder::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_codes() {
        assert_eq!(ok(Value::Null).status, 200);
        assert_eq!(bad_request(Value::Null).status, 400);
        assert_eq!(not_found(Value::Null).status, 404);
    }

    #[test]
    fn encode_json_matches_legacy_shape() {
        let mut buf = JsonBuffer::with_capacity(64);
        let mut m = HashMap::new();
        m.insert("id".into(), Value::Int(42));
        let r = ok(Value::Map(m));
        assert_eq!(
            r.encode_json(&mut buf),
            r#"{"status":200,"body":{"id":42}}"#
        );
        let sim = simulate_post("/orders", Value::Null);
        assert_eq!(
            sim.encode_json(&mut buf),
            crate::host_net::HTTP_JSON_ACK_200
        );
    }
}
