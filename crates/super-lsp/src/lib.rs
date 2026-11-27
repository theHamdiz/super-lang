//! Super Language Server — JSON-RPC 2.0 over stdio.

#![deny(unsafe_op_in_unsafe_fn)]

mod protocol;
mod server;

pub use protocol::{JsonRpcError, Request, Response};
pub use server::LspServer;

use std::io::{BufRead, Write};

/// Run the language server on stdin/stdout until exit.
pub fn run_stdio() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut server = LspServer::new();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    while let Some(msg) = read_message(&mut reader)? {
        if let Some(out) = server.handle_raw(&msg) {
            write_message(&mut writer, &out)?;
        }
        if server.should_exit() {
            break;
        }
    }
    Ok(())
}

fn read_message(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            content_length = Some(rest.trim().parse::<usize>().unwrap_or(0));
        }
    }
    let len = content_length.unwrap_or(0);
    if len == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn write_message(writer: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{}", body.len(), body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use server::LspServer;

    #[test]
    fn initialize_responds() {
        let mut s = LspServer::new();
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let out = s.handle_raw(req).expect("response");
        assert!(out.contains("capabilities"));
        assert!(out.contains("#"));
    }

    #[test]
    fn diagnostics_resolve_std_use_imports() {
        let mut s = LspServer::new();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/02_task_api/src/main.sr"
        );
        let text = std::fs::read_to_string(path).expect("read example");
        let uri = format!("file://{path}");
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"super","version":1,"text":{text}}}}}}}"#,
            uri = uri,
            text = serde_json::to_string(&text).unwrap()
        );
        let notif = s.handle_raw(&open).expect("publishDiagnostics");
        assert!(
            !notif.contains("is not in scope") && !notif.contains("undefined name"),
            "unexpected undefined/use diagnostics: {notif}"
        );
        assert!(
            notif.contains("publishDiagnostics"),
            "expected diagnostics notification"
        );
    }

    #[test]
    fn lint_findings_are_published_as_diagnostics() {
        let mut s = LspServer::new();
        let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///lint.sr","languageId":"super","version":1,"text":"P = {\n    x: i64\n}\n+make(x: i64) : P => P{x: x}\n"}}}"#;
        let notif = s.handle_raw(open).expect("diagnostics");
        assert!(notif.contains("\"source\":\"super-lint\""), "{notif}");
        assert!(notif.contains("S008 redundant-pun"), "{notif}");
    }

    #[test]
    fn formatting_returns_one_whole_document_edit() {
        let mut s = LspServer::new();
        let init = s
            .handle_raw(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("initialize");
        assert!(
            init.contains("\"documentFormattingProvider\":true"),
            "{init}"
        );
        let open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///f.sr","languageId":"super","version":1,"text":"f() {\n  g()\n}"}}}"#;
        let _ = s.handle_raw(open);
        let req = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/formatting","params":{"textDocument":{"uri":"file:///f.sr"},"options":{"tabSize":4,"insertSpaces":true}}}"#;
        let reply: serde_json::Value =
            serde_json::from_str(&s.handle_raw(req).expect("reply")).unwrap();
        let edits = reply["result"].as_array().expect("edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["newText"], "f() {\n    g()\n}\n");
        assert_eq!(edits[0]["range"]["end"]["line"], 2);
        assert_eq!(edits[0]["range"]["end"]["character"], 1);
        let change = r#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///f.sr","version":2},"contentChanges":[{"text":"f() {\n    g()\n}\n"}]}}"#;
        let _ = s.handle_raw(change);
        let reply: serde_json::Value =
            serde_json::from_str(&s.handle_raw(req).expect("reply")).unwrap();
        assert_eq!(reply["result"], serde_json::json!([]));
    }

    #[test]
    fn hover_and_completion_cover_metaprogramming() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///t.sr","languageId":"super","version":1,
                    "text":"//# unroll(4)\n#N = 4\n#TABLE = #for i in 0..4 { i }\ntest \"t\" { assert_eq(#N, 4) }\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.sr"},"position":{"line":1,"character":1}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(
            h.contains("comptime") || h.contains("#N") || h.contains("```super"),
            "expected concise hover, got: {h}"
        );
        assert!(
            !h.contains("fiber state-machine") && !h.contains("staged metaprogramming"),
            "hover must not dump global boilerplate: {h}"
        );
        let comp = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///t.sr"},"position":{"line":0,"character":0}}}"#;
        let c = s.handle_raw(comp).expect("completion");
        assert!(c.contains("unroll") || c.contains("#goal_type") || c.contains("Code"));
    }

    #[test]
    fn hover_on_hash_get_is_route_attr_not_method() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///route.sr","languageId":"super","version":1,
                    "text":"use std.net.http\n#get(\"/tasks\")\nhandle_list(req: Request) : Response { http.ok(\"[]\") }\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        // Cursor on `g` of `#get` — must describe the comptime route attr, not HashMap.get.
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///route.sr"},"position":{"line":1,"character":2}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(
            h.contains("#get") && (h.contains("comptime") || h.contains("route")),
            "expected comptime route attr hover, got: {h}"
        );
        assert!(
            !h.contains("HashMap") && !h.contains("method `get`") && !h.contains("Client.+get"),
            "must not resolve #get as a runtime method, got: {h}"
        );
    }

    #[test]
    fn hover_shows_documentation_comments() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///docs.sr","languageId":"super","version":1,
                    "text":"//! Account helpers.\n/// Adds tax to an amount.\n+add_tax(amount: f64) = amount\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///docs.sr"},"position":{"line":1,"character":4}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(
            h.contains("Adds tax to an amount."),
            "doc comment hover: {h}"
        );
        let on_name = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///docs.sr"},"position":{"line":2,"character":1}}}"#;
        let name = s.handle_raw(on_name).expect("hover name");
        assert!(
            name.contains("Adds tax to an amount."),
            "declaration hover: {name}"
        );
        assert!(
            !name.contains("pure value return"),
            "documented function must not lead with the generic return note: {name}"
        );
    }

    #[test]
    fn soft_keyword_method_use_is_not_import_hover() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///app_use.sr","languageId":"super","version":1,
                    "text":"+App = {\n  /// Register middleware on the app.\n  use(@_self, mw: i64) : i64 => mw\n}\nmain() {\n  app = App{}\n  @app.use(1)\n}\nuse std.core.str\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        // Character on `use` in `@app.use(1)` (line 6, 0-based line 6 → wait: lines:
        // 0:+App, 1:  ///, 2:  use(, 3:}, 4:main, 5:  app, 6:  @app.use(1)
        let on_method = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///app_use.sr"},"position":{"line":6,"character":7}}}"#;
        let method = s.handle_raw(on_method).expect("method hover");
        assert!(
            method.contains("Register middleware")
                || method.contains("use") && !method.contains("import a stdlib"),
            "`.use` must not show import hover: {method}"
        );
        assert!(
            !method.contains("import a stdlib"),
            "`.use` must not be the `use` directive: {method}"
        );
        // Bare import `use` still documents the directive.
        let on_import = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///app_use.sr"},"position":{"line":8,"character":1}}}"#;
        let import = s.handle_raw(on_import).expect("import hover");
        assert!(
            import.contains("import") || import.contains("use std"),
            "bare `use` should stay the import keyword: {import}"
        );
    }

    #[test]
    fn await_spawn_duplicate_binding_does_not_panic_lsp() {
        let mut s = LspServer::new();
        let text = r#"fetch(id: i64) : i64 => id + 1
main() {
  scope {
    t_user = spawn fetch(1)
    u = await t_user
    u = await spawn fetch(2)
  }
}
"#;
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///await_spawn.sr","languageId":"super","version":1,"text":{}}}}}}}"#,
            serde_json::to_string(text).unwrap()
        );
        let diags = s.handle_raw(&open).expect("diagnostics");
        assert!(
            diags.contains("publishDiagnostics"),
            "expected diagnostics notification: {diags}"
        );
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///await_spawn.sr"},"position":{"line":5,"character":18}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(h.contains("result"), "hover after await spawn: {h}");
        let tokens = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///await_spawn.sr"}}}"#;
        let t = s.handle_raw(tokens).expect("tokens");
        assert!(t.contains("data"), "semantic tokens: {t}");
    }

    #[test]
    fn member_hover_and_completion_use_registered_docs() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///member.sr","languageId":"super","version":1,
                    "text":"+Account = {\n  /// Spendable balance in cents.\n  balance: f64 = 0.0\n  /// Take `percent` percent off the balance.\n  discount(@self, percent: f64) : f64 { self.balance }\n}\n+Ledger = {\n  /// How many entries are stored.\n  balance: i64 = 0\n}\nmain() {\n  u = Account.{balance: 10.0}\n  u.discount(10.0)\n  u.\n}\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let on_method = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///member.sr"},"position":{"line":12,"character":5}}}"#;
        let method = s.handle_raw(on_method).expect("method hover");
        assert!(
            method.contains("Take `percent` percent off the balance."),
            "method hover: {method}"
        );
        assert!(
            !method.contains("pure value return"),
            "method hover: {method}"
        );
        assert!(
            !method.contains("How many entries"),
            "receiver must select Account.discount, got: {method}"
        );
        let on_field = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///member.sr"},"position":{"line":11,"character":16}}}"#;
        let field = s.handle_raw(on_field).expect("field hover");
        assert!(
            field.contains("Spendable balance in cents."),
            "field hover: {field}"
        );
        assert!(
            !field.contains("How many entries"),
            "field hover must use the Account receiver, got: {field}"
        );
        let completion = r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///member.sr"},"position":{"line":13,"character":4}}}"#;
        let items = s.handle_raw(completion).expect("completion");
        assert!(
            items.contains("Spendable balance in cents."),
            "field completion docs: {items}"
        );
        assert!(
            items.contains("Take `percent` percent off the balance."),
            "method completion docs: {items}"
        );
        assert!(
            !items.contains("pure value return"),
            "completion docs: {items}"
        );
        let signature = r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///member.sr"},"position":{"line":12,"character":13}}}"#;
        let help = s.handle_raw(signature).expect("signature");
        assert!(
            help.contains("Take `percent` percent off the balance."),
            "signature docs: {help}"
        );
    }

    #[test]
    fn hover_struct_is_concise_layout() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///u.sr","languageId":"super","version":1,
                    "text":"+User = {\n  id: i64\n  name: str\n}\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///u.sr"},"position":{"line":0,"character":1}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(h.contains("User"), "got: {h}");
        assert!(
            h.contains("align") && (h.contains("B") || h.contains("field")),
            "got: {h}"
        );
        assert!(h.contains("id") && h.contains("name"), "got: {h}");
        assert!(
            !h.contains("stack C-ABI value"),
            "generic blurb leaked: {h}"
        );
        assert!(
            !h.contains("PriorityQueue") && !h.contains("SpatialGrid"),
            "got: {h}"
        );
    }

    #[test]
    fn hover_keywords_and_sigils_are_actionable() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///k.sr","languageId":"super","version":1,
                    "text":"for xs { print(it) }\nwhen n { else => 0 }\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let hover_each = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///k.sr"},"position":{"line":0,"character":1}}}"#;
        let h = s.handle_raw(hover_each).expect("hover");
        assert!(h.contains("for"), "got: {h}");
        assert!(
            !h.contains("ultra-fast embeddable"),
            "branding fallback must not appear: {h}"
        );
        let hover_at = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///k.sr"},"position":{"line":0,"character":0}}}"#;
        // character 0 is 'f' of for — already covered; probe `@` via synthetic doc
        let open2 = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///a.sr","languageId":"super","version":1,
                    "text":"@xs = [1]\n"}
            }
        }"#;
        let _ = s.handle_raw(open2);
        let hover_sigil = r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///a.sr"},"position":{"line":0,"character":0}}}"#;
        let hs = s.handle_raw(hover_sigil).expect("hover @");
        assert!(hs.contains("@") || hs.contains("mutat"), "got: {hs}");
        assert!(!hs.contains("ultra-fast embeddable"), "got: {hs}");
        let _ = hover_at;
    }

    #[test]
    fn hover_fn_shows_real_param_types_not_value_blurb() {
        let mut s = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///f.sr","languageId":"super","version":1,
                    "text":"+User = {\n  id: i64\n  balance: f64\n}\n+discount(@target: User, percent: f64) : f64 {\n  target.balance\n}\n"}
            }
        }"#;
        let _ = s.handle_raw(open);
        let hover = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///f.sr"},"position":{"line":4,"character":2}}}"#;
        let h = s.handle_raw(hover).expect("hover");
        assert!(h.contains("discount"), "got: {h}");
        assert!(h.contains("User") && h.contains("f64"), "got: {h}");
        assert!(!h.contains("stack C-ABI value"), "got: {h}");
        assert!(!h.contains("ultra-fast embeddable"), "got: {h}");
    }

    #[test]
    fn hover_on_local_declaration_shows_inferred_type() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///local.sr","languageId":"super","version":1,
                    "text":"main() : i64 {\n  local = 5\n  return local\n}\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        let hover = r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///local.sr"},"position":{"line":1,"character":3}}}"#;
        let response = server.handle_raw(hover).expect("hover response");
        assert!(response.contains("local: i64"), "got: {response}");
        assert!(response.contains("register"), "got: {response}");
    }

    #[test]
    fn definition_on_local_read_jumps_to_declaration() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///definition.sr","languageId":"super","version":1,
                    "text":"main() : i64 {\n  local = 5\n  return local\n}\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        let definition = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///definition.sr"},"position":{"line":2,"character":9}}}"#;
        let response = server.handle_raw(definition).expect("definition response");
        let value: serde_json::Value = serde_json::from_str(&response).expect("valid JSON");
        assert_eq!(
            value["result"]["range"]["start"]["line"], 1,
            "got: {response}"
        );
        assert_eq!(
            value["result"]["range"]["start"]["character"], 2,
            "got: {response}"
        );
    }

    #[test]
    fn publishes_unused_local_diagnostic() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///unused.sr","languageId":"super","version":1,
                    "text":"main() {\n  unused = 5\n}\n"}
            }
        }"#;
        let response = server.handle_raw(open).expect("diagnostics notification");
        assert!(response.contains("E0303"), "got: {response}");
        assert!(
            response.contains("unused variable 'unused'"),
            "got: {response}"
        );
    }

    #[test]
    fn completion_lists_stdlib_imports_from_manifest() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///import.sr","languageId":"super","version":1,
                    "text":"use std.h\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        let completion = r#"{"jsonrpc":"2.0","id":9,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///import.sr"},"position":{"line":0,"character":9}}}"#;
        let response = server.handle_raw(completion).expect("completion response");
        assert!(
            response.contains("use std.net.http"),
            "expected canonical std.net.http import, got: {response}"
        );
        assert!(
            response.contains("JSON parse") || response.contains("stdlib/net/http.sr"),
            "expected manifest summary in completion docs: {response}"
        );
        assert!(
            !response.contains("use std.io.uring"),
            "filtered completion should omit unrelated io modules: {response}"
        );
    }

    #[test]
    fn completion_inside_use_std_net_is_path_scoped() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///import2.sr","languageId":"super","version":1,
                    "text":"use std.net.\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        // Cursor after the trailing dot of `use std.net.`
        let completion = r#"{"jsonrpc":"2.0","id":19,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///import2.sr"},"position":{"line":0,"character":12}}}"#;
        let response = server.handle_raw(completion).expect("completion response");
        assert!(
            response.contains("\"label\":\"http\"") || response.contains("use std.net.http"),
            "expected net.* path completions, got: {response}"
        );
        assert!(
            !response.contains("\"label\":\"App\"")
                && !response.contains("\"label\":\"Mw\"")
                && !response.contains("\"label\":\"after\"")
                && !response.contains("\"label\":\"ensure\""),
            "import completions must not dump unscoped types/keywords: {response}"
        );
    }

    #[test]
    fn completion_inside_braced_multi_import_lists_siblings() {
        // `use std.data.{` → sibling module segments under `data` (column/frame/…).
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///brace.sr","languageId":"super","version":1,
                    "text":"use std.data.{\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        // Cursor right after `{`
        let completion = r#"{"jsonrpc":"2.0","id":29,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///brace.sr"},"position":{"line":0,"character":14}}}"#;
        let response = server.handle_raw(completion).expect("completion response");
        assert!(
            response.contains("\"label\":\"column\"")
                || response.contains("\"label\":\"frame\"")
                || response.contains("use std.data."),
            "expected data.* sibling completions inside braces, got: {response}"
        );
        assert!(
            !response.contains("\"label\":\"ensure\""),
            "braced import completions must stay path-scoped: {response}"
        );
    }

    #[test]
    fn definition_on_std_use_jumps_to_stdlib_file() {
        let mut server = LspServer::new();
        let manifest_dir = concat!(env!("CARGO_MANIFEST_DIR"));
        let repo = format!("{manifest_dir}/../..");
        let path = format!("{repo}/examples/02_task_api/src/main.sr");
        if !std::path::Path::new(&path).is_file() {
            return;
        }
        let text = std::fs::read_to_string(&path).expect("read example");
        let uri = format!("file://{path}");
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"super","version":1,"text":{text}}}}}}}"#,
            uri = uri,
            text = serde_json::to_string(&text).unwrap()
        );
        let _ = server.handle_raw(&open);
        let line = text
            .lines()
            .position(|l| l.contains("use std.") && l.contains("http"))
            .expect("stdlib http import in example") as u64;
        let definition = format!(
            r#"{{"jsonrpc":"2.0","id":11,"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":6}}}}}}"#
        );
        let response = server.handle_raw(&definition).expect("definition");
        assert!(
            response.contains("net/http.sr"),
            "expected stdlib path in definition, got: {response}"
        );
    }

    #[test]
    fn completion_after_dot_includes_methods_when_typed() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///ufcs.sr","languageId":"super","version":1,
                    "text":"+User = {\n  id: i64\n  balance: f64\n}\n+discount(@target: User, percent: f64) : f64 {\n  target.balance\n}\nmain() {\n  u = User.{id: 1, balance: 20.0}\n  u.\n}\n"}
            }
        }"#;
        let _ = server.handle_raw(open);
        let completion = r#"{"jsonrpc":"2.0","id":10,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///ufcs.sr"},"position":{"line":7,"character":4}}}"#;
        let response = server.handle_raw(completion).expect("completion response");
        assert!(
            response.contains("discount") || response.contains("balance"),
            "expected UFCS field or method on User, got: {response}"
        );
    }

    #[test]
    fn rename_inlay_signature_and_outline_are_available() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///deep.sr","languageId":"super","version":1,
                    "text":"add(a: i64, b: i64) : i64 => a + b\nmain() : i64 {\n  local = add(1, 2)\n  return local\n}\n"}
            }
        }"#;
        let _ = server.handle_raw(open);

        let rename = r#"{"jsonrpc":"2.0","id":3,"method":"textDocument/rename","params":{"textDocument":{"uri":"file:///deep.sr"},"position":{"line":3,"character":9},"newName":"answer"}}"#;
        let renamed = server.handle_raw(rename).expect("rename response");
        assert!(
            renamed.matches("\"newText\":\"answer\"").count() >= 2,
            "got: {renamed}"
        );

        let inlay = r#"{"jsonrpc":"2.0","id":4,"method":"textDocument/inlayHint","params":{"textDocument":{"uri":"file:///deep.sr"},"range":{"start":{"line":0,"character":0},"end":{"line":5,"character":0}}}}"#;
        let hints = server.handle_raw(inlay).expect("inlay response");
        assert!(hints.contains(": i64"), "got: {hints}");

        let signature = r#"{"jsonrpc":"2.0","id":5,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///deep.sr"},"position":{"line":2,"character":18}}}"#;
        let help = server.handle_raw(signature).expect("signature response");
        assert!(help.contains("add(a: i64, b: i64)"), "got: {help}");

        let outline = r#"{"jsonrpc":"2.0","id":6,"method":"textDocument/documentSymbol","params":{"textDocument":{"uri":"file:///deep.sr"}}}"#;
        let symbols = server.handle_raw(outline).expect("outline response");
        assert!(symbols.contains("\"name\":\"add\""), "got: {symbols}");
        assert!(symbols.contains("\"name\":\"main\""), "got: {symbols}");
    }
}

#[cfg(test)]
mod hover_goto_and_sync {
    use super::*;

    fn open_task_api(server: &mut LspServer) -> (String, String) {
        let manifest_dir = concat!(env!("CARGO_MANIFEST_DIR"));
        let path = format!("{manifest_dir}/../../examples/02_task_api/src/main.sr");
        let text = std::fs::read_to_string(&path).expect("read");
        let uri = format!("file://{path}");
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"super","version":1,"text":{text}}}}}}}"#,
            uri = uri,
            text = serde_json::to_string(&text).unwrap()
        );
        let _ = server.handle_raw(&open);
        (uri, text)
    }

    fn pos_of(text: &str, needle: &str, occurrence: usize) -> (u64, u64) {
        let mut idx = 0;
        let mut found = 0;
        loop {
            let i = text[idx..].find(needle).expect("needle") + idx;
            if found == occurrence {
                let line = text[..i].bytes().filter(|&b| b == b'\n').count() as u64;
                let col = (i - text[..i].rfind('\n').map(|n| n + 1).unwrap_or(0)) as u64;
                return (line, col + (needle.len() as u64 / 2));
            }
            found += 1;
            idx = i + 1;
        }
    }

    #[test]
    fn hover_and_definition_resolve_same_file_and_stdlib_symbols() {
        let mut server = LspServer::new();
        let (uri, text) = open_task_api(&mut server);

        // Same-file: call site of `build_app` → definition.
        let (line, character) = pos_of(&text, "build_app", 1);
        let hover = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        let response = server.handle_raw(&hover).expect("hover");
        assert!(!response.contains("unresolved symbol"), "got: {response}");
        assert!(response.contains("build_app"), "got: {response}");

        let def = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        let response = server.handle_raw(&def).expect("def");
        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert!(!value["result"].is_null(), "definition null: {response}");
        assert_eq!(
            value["result"]["range"]["start"]["line"],
            text.lines()
                .position(|l| l.contains("+build_app"))
                .expect("+build_app") as u64
        );

        // Stdlib: `App` from `use std.net.http` → http.sr (`Mw.*` variants resolve as tags).
        let (line, character) = pos_of(&text, "App.new", 0);
        let hover = format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        let response = server.handle_raw(&hover).expect("app hover");
        assert!(!response.contains("unresolved symbol"), "got: {response}");
        assert!(response.contains("App"), "got: {response}");

        let def = format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"textDocument/definition","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        let response = server.handle_raw(&def).expect("app def");
        assert!(
            response.contains("http.sr"),
            "expected stdlib definition, got: {response}"
        );
    }

    #[test]
    fn incremental_did_change_preserves_hover_and_definition() {
        let mut server = LspServer::new();
        let open = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///sync.sr","languageId":"super","version":1,
                    "text":"+H_CREATE = 1\nmain() : i64 {\n  return H_CREATE\n}\n"}
            }
        }"#;
        let _ = server.handle_raw(open);

        // Editors advertise/send TextDocumentSyncKind.Incremental (change=2): a single
        // inserted space must NOT replace the whole buffer with that fragment.
        let change = r#"{
            "jsonrpc":"2.0","method":"textDocument/didChange","params":{
                "textDocument":{"uri":"file:///sync.sr","version":2},
                "contentChanges":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},
                    "rangeLength":0,
                    "text":" "
                }]
            }
        }"#;
        let _ = server.handle_raw(change);

        let hover = r#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///sync.sr"},"position":{"line":2,"character":11}}}"#;
        let response = server
            .handle_raw(hover)
            .expect("hover after incremental edit");
        assert!(
            !response.contains("unresolved symbol"),
            "incremental sync corrupted the buffer; hover: {response}"
        );
        assert!(
            response.contains("H_CREATE"),
            "expected binding hover after edit, got: {response}"
        );

        let def = r#"{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":"file:///sync.sr"},"position":{"line":2,"character":11}}}"#;
        let response = server
            .handle_raw(def)
            .expect("definition after incremental edit");
        let value: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert!(
            !value["result"].is_null(),
            "ctrl-click fails after incremental edit: {response}"
        );
        assert_eq!(
            value["result"]["range"]["start"]["line"], 0,
            "should jump to +H_CREATE declaration: {response}"
        );
    }
}

#[cfg(test)]
mod syntax_coverage {
    use super::*;
    use server::LspServer;

    const FIXTURE: &str = include_str!("../fixtures/syntax_coverage.sr");

    fn open_fixture(server: &mut LspServer) -> String {
        let uri = "file:///syntax_coverage.sr".to_string();
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{uri}","languageId":"super","version":1,"text":{text}}}}}}}"#,
            uri = uri,
            text = serde_json::to_string(FIXTURE).unwrap()
        );
        let diags = server.handle_raw(&open).expect("publishDiagnostics");
        assert!(
            !diags.contains("E0001"),
            "fixture must parse; got parse diagnostics: {diags}"
        );
        assert!(
            diags.contains("publishDiagnostics"),
            "expected diagnostics notification, got: {diags}"
        );
        uri
    }

    fn pos_of(needle: &str, occurrence: usize) -> (u64, u64) {
        let mut idx = 0usize;
        let mut found = 0usize;
        loop {
            let i = FIXTURE[idx..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing needle {needle:?}"))
                + idx;
            if found == occurrence {
                let line = FIXTURE[..i].bytes().filter(|&b| b == b'\n').count() as u64;
                let col = (i - FIXTURE[..i].rfind('\n').map(|n| n + 1).unwrap_or(0)) as u64;
                return (line, col);
            }
            found += 1;
            idx = i + needle.len().max(1);
        }
    }

    fn hover(server: &mut LspServer, uri: &str, id: u64, line: u64, character: u64) -> String {
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"textDocument/hover","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        server.handle_raw(&req).expect("hover")
    }

    #[test]
    fn fixture_opens_without_parse_errors() {
        let mut server = LspServer::new();
        let _ = open_fixture(&mut server);
    }

    #[test]
    fn multi_binding_is_accepted_not_rejected() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);
        let (line, character) = pos_of("+tasks, +next_id, owner", 0);
        // Cursor on `tasks` — binding hover, not a parse fault.
        let h = hover(&mut server, &uri, 1, line, character + 1);
        assert!(
            !h.contains("E0001") && !h.contains("expected declaration"),
            "multi-binding rejected: {h}"
        );
        assert!(
            h.contains("tasks") || h.contains("[]") || h.contains("array") || h.contains("i64"),
            "expected binding hover for tasks, got: {h}"
        );
    }

    #[test]
    fn comptime_bindings_and_attrs_hover() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);

        let (line, character) = pos_of("#N = 4", 0);
        let h = hover(&mut server, &uri, 2, line, character + 1);
        assert!(
            h.contains("comptime") || h.contains("#N"),
            "comptime binding hover: {h}"
        );

        let (line, character) = pos_of("#inline", 0);
        let h = hover(&mut server, &uri, 3, line, character + 1);
        assert!(
            h.contains("#inline") || h.contains("comptime") || h.contains("inline"),
            "meta attr hover: {h}"
        );

        let (line, character) = pos_of("#get(\"/tasks\")", 0);
        let h = hover(&mut server, &uri, 4, line, character + 1);
        assert!(
            h.contains("#get") && (h.contains("comptime") || h.contains("route")),
            "route attr hover: {h}"
        );
        assert!(
            !h.contains("HashMap") && !h.contains("method `get`"),
            "#get must not resolve as a method: {h}"
        );
    }

    #[test]
    fn builtin_macros_and_panic_hover() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);

        for (needle, expect) in [
            ("#assert(flag)", "#assert"),
            ("#dbg(1)", "#dbg"),
            ("#todo", "#todo"),
            ("#unimplemented", "#unimplemented"),
            ("#unreachable", "#unreachable"),
            ("panic \"never\"", "panic"),
        ] {
            let (line, character) = pos_of(needle, 0);
            let h = hover(&mut server, &uri, 10, line, character + 1);
            assert!(
                h.contains(expect) || h.contains("comptime") || h.contains("panic"),
                "hover for {needle}: {h}"
            );
        }
    }

    #[test]
    fn documentation_sugar_and_control_flow_hover() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);

        let (line, character) = pos_of("/// Multi-binding with per-name export", 0);
        let h = hover(&mut server, &uri, 20, line, character + 4);
        assert!(
            h.contains("Multi-binding") || h.contains("per-name"),
            "doc comment hover: {h}"
        );

        let (line, character) = pos_of("ensure path", 0);
        let h = hover(&mut server, &uri, 21, line, character);
        assert!(h.contains("ensure"), "ensure hover: {h}");

        let (line, character) = pos_of("when self", 0);
        let h = hover(&mut server, &uri, 22, line, character);
        assert!(h.contains("when"), "when hover: {h}");

        let (line, character) = pos_of("for @scores", 0);
        let h = hover(&mut server, &uri, 23, line, character);
        assert!(h.contains("for"), "for hover: {h}");

        let (line, character) = pos_of("par j in", 0);
        let h = hover(&mut server, &uri, 24, line, character);
        assert!(
            h.contains("par") || h.contains("parallel"),
            "par hover: {h}"
        );

        // Structured concurrency lives in a tiny companion buffer so the main
        // fixture stays C-AOT clean while still covering `scope`/`spawn`/`await`.
        let open_conc = r#"{
            "jsonrpc":"2.0","method":"textDocument/didOpen","params":{
                "textDocument":{"uri":"file:///concurrency.sr","languageId":"super","version":1,
                    "text":"+main() : i64 {\n  scope {\n    t = spawn { 1 + 1 }\n    await t\n  }\n}\n"}
            }
        }"#;
        let diags = server.handle_raw(open_conc).expect("concurrency diags");
        assert!(
            !diags.contains("E0001"),
            "concurrency snippet must parse: {diags}"
        );
        // `+main(){\n  scope {\n    t = spawn {…}\n    await t\n  }\n}`
        let h = hover(&mut server, "file:///concurrency.sr", 25, 1, 2);
        assert!(h.contains("scope"), "scope hover: {h}");
        let h = hover(&mut server, "file:///concurrency.sr", 26, 2, 8);
        assert!(
            h.contains("spawn") || h.contains("task") || h.contains("Task"),
            "spawn hover: {h}"
        );
        let h = hover(&mut server, "file:///concurrency.sr", 27, 3, 4);
        assert!(h.contains("await"), "await hover: {h}");
        let _ = uri;
    }

    #[test]
    fn types_traits_sums_and_state_are_outlined() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);
        let outline = format!(
            r#"{{"jsonrpc":"2.0","id":30,"method":"textDocument/documentSymbol","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#
        );
        let symbols = server.handle_raw(&outline).expect("outline");
        for name in [
            "tasks",
            "next_id",
            "owner",
            "width",
            "Pos",
            "Status",
            "Widget",
            "world",
            "hot",
            "handle_list",
            "main",
        ] {
            assert!(
                symbols.contains(&format!("\"name\":\"{name}\"")),
                "outline missing {name}: {symbols}"
            );
        }
    }

    #[test]
    fn completion_covers_keywords_macros_and_ufcs() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);

        let (line, _) = pos_of("+main() {", 0);
        // Start of main body — keyword / macro completions should surface.
        let comp = format!(
            r#"{{"jsonrpc":"2.0","id":40,"method":"textDocument/completion","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":0}}}}}}"#
        );
        let c = server.handle_raw(&comp).expect("completion");
        assert!(
            c.contains("ensure")
                || c.contains("when")
                || c.contains("panic")
                || c.contains("#todo")
                || c.contains("for"),
            "expected language completions, got: {c}"
        );

        let (line, character) = pos_of("s.amount_or", 0);
        let comp = format!(
            r#"{{"jsonrpc":"2.0","id":41,"method":"textDocument/completion","params":{{"textDocument":{{"uri":"{uri}"}},"position":{{"line":{line},"character":{character}}}}}}}"#
        );
        let c = server.handle_raw(&comp).expect("ufcs completion");
        assert!(
            c.contains("amount_or") || c.contains("Paid") || c.contains("Status"),
            "expected sum method / UFCS completion, got: {c}"
        );
    }

    #[test]
    fn semantic_tokens_cover_exports_and_sigils() {
        let mut server = LspServer::new();
        let uri = open_fixture(&mut server);
        let req = format!(
            r#"{{"jsonrpc":"2.0","id":50,"method":"textDocument/semanticTokens/full","params":{{"textDocument":{{"uri":"{uri}"}}}}}}"#
        );
        let tokens = server.handle_raw(&req).expect("semantic tokens");
        assert!(
            tokens.contains("\"data\"") && tokens.contains("["),
            "expected semantic token data, got: {tokens}"
        );
        // Non-empty token stream for a large fixture.
        assert!(tokens.len() > 200, "semantic tokens look empty: {tokens}");
    }

    #[test]
    fn range_call_rhs_does_not_break_semantic_tokens() {
        // LSP semantic tokens are symbol-based (not a greedy number span), but
        // opening `0..len(xs)` must still analyze cleanly — same fixture as
        // the site highlighter / lexer regressions.
        let mut server = LspServer::new();
        let text = r#"
len(xs: [i64]) : i64 => 0
foo(x: i64) : i64 => x
main() {
  xs = [1, 2, 3]
  for 0..len(xs) { }
  for 0..foo(bar.baz) { }
}
"#;
        let open = format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"file:///range_rhs.sr","languageId":"super","version":1,"text":{}}}}}}}"#,
            serde_json::to_string(text).unwrap()
        );
        let diags = server.handle_raw(&open).expect("diagnostics");
        assert!(
            diags.contains("publishDiagnostics"),
            "expected diagnostics notification: {diags}"
        );
        let tokens = r#"{"jsonrpc":"2.0","id":51,"method":"textDocument/semanticTokens/full","params":{"textDocument":{"uri":"file:///range_rhs.sr"}}}"#;
        let t = server.handle_raw(tokens).expect("semantic tokens");
        assert!(t.contains("\"data\""), "semantic tokens: {t}");
        // Encoded data array must be non-empty (at least main / len / foo).
        assert!(
            t.contains('[') && t.len() > 40,
            "expected non-empty semantic token data for range RHS fixture: {t}"
        );
    }
}
