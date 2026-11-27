//! CLI command reference for agents.

use serde_json::{Value, json};

pub fn commands_json() -> Value {
    json!([
        {"name": "new", "summary": "Scaffold a Pod (bin/lib/macro)"},
        {"name": "init", "summary": "Initialize a Pod in the current directory"},
        {"name": "run", "alias": "r", "summary": "Compile and run a program"},
        {"name": "test", "summary": "Run test blocks under a path"},
        {"name": "verify", "alias": "v", "summary": "Discharge #law invariants (SMT/comptime/fuzz; zero runtime emit)"},
        {"name": "check", "alias": "c", "summary": "Type-check + C23 syntax-check"},
        {"name": "build", "alias": "b", "summary": "Build native or bytecode artifact"},
        {"name": "fmt", "summary": "Format .sr files in place"},
        {"name": "doc", "summary": "Generate Markdown docs from /// comments"},
        {"name": "lsp", "summary": "Language server (stdio JSON-RPC / LSP)"},
        {"name": "mcp", "summary": "Agent knowledge MCP server (stdio JSON-RPC / MCP)"},
        {"name": "install", "alias": "i", "summary": "Install a bin Pod onto PATH"},
        {"name": "uninstall", "alias": "u", "summary": "Remove an installed binary"},
        {"name": "list", "summary": "List installed Super binaries"},
        {"name": "which", "summary": "Print path of an installed binary"},
        {"name": "clean", "summary": "Remove target/ artifacts"},
        {"name": "bench", "summary": "Run ECS + HTTP performance benches"},
        {"name": "bindgen", "summary": "Generate C/C++ headers from Super exports"},
        {"name": "disasm", "summary": "Disassemble bytecode / JIT"},
        {"name": "completions", "summary": "Shell completion scripts"},
        {"name": "add/remove/update/fetch/vendor", "summary": "Dependency management"}
    ])
}
