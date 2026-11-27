//! Embedded Super MCP server — language features, examples, algorithms, stdlib.
//!
//! Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (`super mcp`).

#![deny(unsafe_op_in_unsafe_fn)]

mod catalog;
mod protocol;
mod server;
mod tools;

pub use catalog::{LANGUAGE_VERSION, MCP_PROTOCOL_VERSION};
pub use protocol::{JsonRpcError, Request, Response};
pub use server::McpServer;

use std::io::{BufRead, Read, Write};

/// Run the MCP server on stdin/stdout until EOF or exit.
pub fn run_stdio() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = McpServer::new();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Some clients still speak LSP-style Content-Length framing; accept both.
        if trimmed.to_ascii_lowercase().starts_with("content-length:") {
            let len = trimmed
                .split(':')
                .nth(1)
                .and_then(|s| s.trim().parse::<usize>().ok())
                .unwrap_or(0);
            // Consume header block until blank line.
            loop {
                line.clear();
                let n = reader.read_line(&mut line)?;
                if n == 0 || line.trim().is_empty() {
                    break;
                }
            }
            if len == 0 {
                continue;
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            let msg = String::from_utf8_lossy(&buf);
            if let Some(out) = server.handle_raw(&msg) {
                write_framed(&mut writer, &out)?;
            }
        } else if let Some(out) = server.handle_raw(trimmed) {
            write_ndjson(&mut writer, &out)?;
        }
        if server.should_exit() {
            break;
        }
    }
    Ok(())
}

fn write_ndjson(writer: &mut impl Write, body: &str) -> std::io::Result<()> {
    // MCP stdio: one JSON object per line, no embedded newlines.
    let compact = body.replace(['\n', '\r'], " ");
    writeln!(writer, "{compact}")?;
    writer.flush()
}

fn write_framed(writer: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::{algorithms, examples, features, stdlib};
    use serde_json::json;

    fn call(
        server: &mut McpServer,
        method: &str,
        id: i64,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let raw = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })
        .to_string();
        let out = server.handle_raw(&raw).expect("response");
        serde_json::from_str(&out).expect("json")
    }

    #[test]
    fn initialize_advertises_tools_resources_prompts() {
        let mut s = McpServer::new();
        let resp = call(&mut s, "initialize", 1, json!({}));
        assert_eq!(resp["result"]["serverInfo"]["name"], "super-mcp");
        assert_eq!(resp["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
        assert!(resp["result"]["capabilities"]["resources"].is_object());
        assert!(resp["result"]["capabilities"]["prompts"].is_object());
    }

    #[test]
    fn tools_list_has_expected_names() {
        let mut s = McpServer::new();
        let resp = call(&mut s, "tools/list", 2, json!({}));
        let tools = resp["result"]["tools"].as_array().expect("tools");
        assert!(tools.len() >= 12);
        let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"super_overview"));
        assert!(names.contains(&"super_search"));
        assert!(names.contains(&"super_get_stdlib"));
        assert!(names.contains(&"super_get_algorithm"));
    }

    #[test]
    fn overview_and_search_work() {
        let mut s = McpServer::new();
        let ov = call(
            &mut s,
            "tools/call",
            3,
            json!({"name": "super_overview", "arguments": {}}),
        );
        assert_eq!(ov["result"]["isError"], false);
        assert!(
            ov["result"]["structuredContent"]["counts"]["stdlib_modules"]
                .as_u64()
                .unwrap_or(0)
                >= 90
        );

        let search = call(
            &mut s,
            "tools/call",
            4,
            json!({"name": "super_search", "arguments": {"query": "optional error"}}),
        );
        assert_eq!(search["result"]["isError"], false);
        assert!(
            search["result"]["structuredContent"]["total"]
                .as_u64()
                .unwrap_or(0)
                >= 1
        );
    }

    #[test]
    fn feature_algorithm_example_stdlib_roundtrip() {
        let mut s = McpServer::new();
        let f = call(
            &mut s,
            "tools/call",
            5,
            json!({"name": "super_get_feature", "arguments": {"id": "bindings"}}),
        );
        assert_eq!(f["result"]["isError"], false);
        assert!(
            f["result"]["structuredContent"]["body_md"]
                .as_str()
                .unwrap_or("")
                .contains("TaxRate")
        );

        let a = call(
            &mut s,
            "tools/call",
            6,
            json!({"name": "super_get_algorithm", "arguments": {"id": "binary-search"}}),
        );
        assert_eq!(a["result"]["isError"], false);
        assert!(
            a["result"]["structuredContent"]["code"]
                .as_str()
                .unwrap_or("")
                .contains("bin_search")
        );

        let e = call(
            &mut s,
            "tools/call",
            7,
            json!({"name": "super_get_example", "arguments": {"id": "task-api"}}),
        );
        assert_eq!(e["result"]["isError"], false);

        let st = call(
            &mut s,
            "tools/call",
            8,
            json!({"name": "super_get_stdlib", "arguments": {"path": "net.http", "include_source": false}}),
        );
        assert_eq!(st["result"]["isError"], false);
        assert_eq!(st["result"]["structuredContent"]["path"], "std.net.http");
    }

    #[test]
    fn resources_and_prompts() {
        let mut s = McpServer::new();
        let list = call(&mut s, "resources/list", 9, json!({}));
        let resources = list["result"]["resources"].as_array().unwrap();
        assert!(resources.len() > 50);

        let read = call(
            &mut s,
            "resources/read",
            10,
            json!({"uri": "super://feature/bindings"}),
        );
        assert!(
            read["result"]["contents"][0]["text"]
                .as_str()
                .unwrap_or("")
                .contains("Bindings")
        );

        let prompts = call(&mut s, "prompts/list", 11, json!({}));
        assert!(
            prompts["result"]["prompts"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0)
                >= 3
        );

        let got = call(
            &mut s,
            "prompts/get",
            12,
            json!({"name": "write-super", "arguments": {"task": "HTTP echo server"}}),
        );
        assert!(
            got["result"]["messages"][0]["content"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("HTTP echo")
        );
    }

    #[test]
    fn catalog_sizes_are_substantial() {
        assert!(features::ALL.len() >= 15);
        assert!(algorithms::ALL.len() >= 10);
        assert!(examples::ALL.len() >= 30);
        assert!(stdlib::MODULES.len() >= 126);
    }

    #[test]
    fn unknown_tool_is_tool_error_not_protocol_error() {
        let mut s = McpServer::new();
        let resp = call(
            &mut s,
            "tools/call",
            13,
            json!({"name": "nope", "arguments": {}}),
        );
        assert_eq!(resp["result"]["isError"], true);
        assert!(resp["error"].is_null());
    }

    #[test]
    fn stdlib_coverage_tool() {
        let mut s = McpServer::new();
        let resp = call(
            &mut s,
            "tools/call",
            14,
            json!({"name": "super_list_stdlib", "arguments": {"coverage": true}}),
        );
        assert_eq!(resp["result"]["isError"], false);
        assert_eq!(
            resp["result"]["structuredContent"]["module_count"],
            stdlib::MODULES.len()
        );
    }
}
