//! MCP JSON-RPC method handlers.

use crate::catalog::{LANGUAGE_VERSION, MCP_PROTOCOL_VERSION};
use crate::protocol::{Request, Response};
use crate::tools;
use serde_json::{Value, json};

pub struct McpServer {
    initialized: bool,
    exit: bool,
}

impl McpServer {
    pub fn new() -> Self {
        Self {
            initialized: false,
            exit: false,
        }
    }

    pub fn should_exit(&self) -> bool {
        self.exit
    }

    pub fn handle_raw(&mut self, raw: &str) -> Option<String> {
        let req: Request = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(_) => {
                return Some(
                    serde_json::to_string(&Response::error(None, -32700, "Parse error")).unwrap(),
                );
            }
        };
        self.handle(req)
    }

    pub fn handle(&mut self, req: Request) -> Option<String> {
        match req.method.as_str() {
            "initialize" => {
                self.initialized = true;
                Some(
                    serde_json::to_string(&Response::result(
                        req.id,
                        json!({
                            "protocolVersion": MCP_PROTOCOL_VERSION,
                            "capabilities": {
                                "tools": { "listChanged": false },
                                "resources": { "subscribe": false, "listChanged": false },
                                "prompts": { "listChanged": false }
                            },
                            "serverInfo": {
                                "name": "super-mcp",
                                "version": LANGUAGE_VERSION
                            },
                            "instructions": "Super language knowledge MCP. Prefer super_search, then get_* tools for detail. Use super_list_stdlib / super_get_stdlib for import paths and live source when SUPER_STDLIB is available."
                        }),
                    ))
                    .unwrap(),
                )
            }
            "notifications/initialized" | "initialized" => None,
            "ping" => Some(serde_json::to_string(&Response::result(req.id, json!({}))).unwrap()),
            "shutdown" => {
                // MCP often uses exit notification; keep JSON-RPC shutdown for symmetry with LSP.
                Some(serde_json::to_string(&Response::result(req.id, Value::Null)).unwrap())
            }
            "exit" => {
                self.exit = true;
                None
            }
            "tools/list" => Some(
                serde_json::to_string(&Response::result(
                    req.id,
                    json!({ "tools": tools::tools_list() }),
                ))
                .unwrap(),
            ),
            "tools/call" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = req
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let result = tools::call_tool(name, &args);
                Some(serde_json::to_string(&Response::result(req.id, result)).unwrap())
            }
            "resources/list" => Some(
                serde_json::to_string(&Response::result(
                    req.id,
                    json!({ "resources": tools::resources_list() }),
                ))
                .unwrap(),
            ),
            "resources/read" => {
                let uri = req.params.get("uri").and_then(|v| v.as_str()).unwrap_or("");
                match tools::resource_read(uri) {
                    Ok(result) => {
                        Some(serde_json::to_string(&Response::result(req.id, result)).unwrap())
                    }
                    Err(msg) => {
                        Some(serde_json::to_string(&Response::error(req.id, -32602, msg)).unwrap())
                    }
                }
            }
            "resources/templates/list" => Some(
                serde_json::to_string(&Response::result(
                    req.id,
                    json!({
                        "resourceTemplates": [
                            {
                                "uriTemplate": "super://feature/{id}",
                                "name": "Language feature",
                                "mimeType": "text/markdown"
                            },
                            {
                                "uriTemplate": "super://algorithm/{id}",
                                "name": "Algorithm",
                                "mimeType": "text/markdown"
                            },
                            {
                                "uriTemplate": "super://example/{id}",
                                "name": "Example project",
                                "mimeType": "text/markdown"
                            },
                            {
                                "uriTemplate": "super://stdlib/{path}",
                                "name": "Stdlib module",
                                "mimeType": "application/json"
                            }
                        ]
                    }),
                ))
                .unwrap(),
            ),
            "prompts/list" => Some(
                serde_json::to_string(&Response::result(
                    req.id,
                    json!({ "prompts": tools::prompts_list() }),
                ))
                .unwrap(),
            ),
            "prompts/get" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let empty = serde_json::Map::new();
                let args = req
                    .params
                    .get("arguments")
                    .and_then(|v| v.as_object())
                    .unwrap_or(&empty);
                match tools::prompts_get(name, args) {
                    Ok(result) => {
                        Some(serde_json::to_string(&Response::result(req.id, result)).unwrap())
                    }
                    Err(msg) => {
                        Some(serde_json::to_string(&Response::error(req.id, -32602, msg)).unwrap())
                    }
                }
            }
            _ => {
                if req.id.is_some() {
                    Some(
                        serde_json::to_string(&Response::error(
                            req.id,
                            -32601,
                            format!("Method not found: {}", req.method),
                        ))
                        .unwrap(),
                    )
                } else {
                    None
                }
            }
        }
    }
}

impl Default for McpServer {
    fn default() -> Self {
        Self::new()
    }
}
