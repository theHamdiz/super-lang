//! LSP feature handlers backed by the arena frontend.

use crate::protocol::{Request, Response};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use super_core::{
    BytePos, DiagnosticKind, Diagnostics, Interner, Severity, SourceFile, Span, TypeId,
};
use super_frontend::ast::{Expr, Item, Module};
use super_frontend::meta::{CtValue, MetaCache};
use super_frontend::sema::{StorageClass, SymbolInfo, SymbolKind, TypeKind, TypedModule};
use super_frontend::{
    ImportTarget, SymbolOrigin, TokenKind, analyze, analyze_project_text, module_path_to_file_uri,
    module_source_text, stdlib_import_next_segments, stdlib_use_completions_for,
};

pub struct LspServer {
    docs: HashMap<String, String>,
    exit: bool,
}

impl LspServer {
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
            exit: false,
        }
    }

    pub fn should_exit(&self) -> bool {
        self.exit
    }

    pub fn handle_raw(&mut self, raw: &str) -> Option<String> {
        let req: Request = serde_json::from_str(raw).ok()?;
        self.handle(req)
    }

    pub fn handle(&mut self, req: Request) -> Option<String> {
        match req.method.as_str() {
            "initialize" => Some(
                serde_json::to_string(&Response::result(
                    req.id,
                    json!({
                        "capabilities": {
                            "positionEncoding": "utf-16",
                            "textDocumentSync": { "openClose": true, "change": 2 },
                            "hoverProvider": true,
                            "codeLensProvider": { "resolveProvider": false },
                            "definitionProvider": true,
                            "referencesProvider": true,
                            "renameProvider": true,
                            "inlayHintProvider": true,
                            "signatureHelpProvider": {
                                "triggerCharacters": ["(", ","],
                                "retriggerCharacters": [","]
                            },
                            "documentSymbolProvider": true,
                            "documentFormattingProvider": true,
                            "completionProvider": { "triggerCharacters": [".", "@", "+", "#", "/", "{", ","] },
                            "semanticTokensProvider": {
                                "legend": {
                                    "tokenTypes": [
                                        "namespace", "type", "class", "enum", "interface",
                                        "struct", "typeParameter", "parameter", "variable",
                                        "property", "enumMember", "event", "function",
                                        "method", "macro", "keyword", "modifier", "comment",
                                        "string", "number", "regexp", "operator"
                                    ],
                                    "tokenModifiers": [
                                        "declaration", "definition", "readonly", "static",
                                        "deprecated", "abstract", "async", "modification",
                                        "documentation", "defaultLibrary", "exported"
                                    ]
                                },
                                "full": true
                            }
                        },
                        "serverInfo": { "name": "super-lsp", "version": env!("CARGO_PKG_VERSION") }
                    }),
                ))
                .unwrap(),
            ),
            "initialized" | "textDocument/didSave" => None,
            "shutdown" => Some(serde_json::to_string(&Response::result(req.id, Value::Null)).unwrap()),
            "exit" => {
                self.exit = true;
                None
            }
            "textDocument/didOpen" => {
                let uri = req.params["textDocument"]["uri"].as_str()?.to_owned();
                let text = req.params["textDocument"]["text"].as_str()?.to_owned();
                self.docs.insert(uri.clone(), text.clone());
                Some(self.publish_diagnostics(&uri, &text))
            }
            "textDocument/didChange" => {
                let uri = req.params["textDocument"]["uri"].as_str()?.to_owned();
                let changes = req.params["contentChanges"].as_array()?;
                let base = self.docs.get(&uri).cloned().unwrap_or_default();
                let text = apply_content_changes(&base, changes);
                self.docs.insert(uri.clone(), text.clone());
                Some(self.publish_diagnostics(&uri, &text))
            }
            "textDocument/hover" => Some(self.hover(req)),
            "textDocument/codeLens" => Some(self.code_lens(req)),
            "textDocument/definition" => Some(self.definition(req)),
            "textDocument/references" => Some(self.references(req)),
            "textDocument/rename" => Some(self.rename(req)),
            "textDocument/inlayHint" => Some(self.inlay_hints(req)),
            "textDocument/signatureHelp" => Some(self.signature_help(req)),
            "textDocument/documentSymbol" => Some(self.document_symbols(req)),
            "textDocument/completion" => Some(self.completion(req)),
            "textDocument/semanticTokens/full" => Some(self.semantic_tokens(req)),
            "textDocument/formatting" => Some(self.formatting(req)),
            _ => {
                if req.id.is_some() {
                    Some(
                        serde_json::to_string(&Response::error(
                            req.id,
                            -32601,
                            format!("method not found: {}", req.method),
                        ))
                        .unwrap(),
                    )
                } else {
                    None
                }
            }
        }
    }

    /// `super fmt` for the whole document: one edit, or none when the text is
    /// already formatted or does not lex.
    fn formatting(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"]
            .as_str()
            .unwrap_or_default();
        let text = self.docs.get(uri).map(String::as_str).unwrap_or_default();
        let edits = match super_frontend::format::format_source(text) {
            Ok(formatted) if formatted != text => {
                let last = text.rsplit('\n').next().unwrap_or_default();
                json!([{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": {
                            "line": text.matches('\n').count(),
                            "character": last.encode_utf16().count(),
                        },
                    },
                    "newText": formatted,
                }])
            }
            _ => json!([]),
        };
        serde_json::to_string(&Response::result(req.id, edits)).unwrap()
    }

    fn publish_diagnostics(&self, uri: &str, text: &str) -> String {
        let file = SourceFile::new(uri, text);
        let mut result = self.analyze_doc(uri, text);
        // Inline E0502 / E0503 for `#law` / `#proof` (verify-only; never runtime).
        if !result.diagnostics.has_errors() {
            let opts = super_frontend::VerifyOptions::default();
            let _ = super_frontend::verify_module(
                &result.module,
                &result.interner,
                &mut result.diagnostics,
                &opts,
            );
        }
        let diags: Vec<Value> = result
            .diagnostics
            .iter()
            .map(|d| {
                let range = span_range_json(&file, d.span);
                let severity = match d.severity {
                    Severity::Error => 1,
                    Severity::Warning => 2,
                    Severity::Info => 3,
                    Severity::Hint => 4,
                };
                let source = match d.kind {
                    DiagnosticKind::Syntax => "super-syntax",
                    DiagnosticKind::Type => "super-type",
                    DiagnosticKind::Mutation => "super-mutation",
                    DiagnosticKind::Lifetime => "super-lifetime",
                    _ => "super",
                };
                let mut msg = d.message.clone();
                if let Some(code) = d.code {
                    msg = format!("[{code}] {msg}");
                }
                if let Some(help) = &d.help {
                    msg.push_str("\nhelp: ");
                    msg.push_str(help);
                }
                json!({
                    "range": range,
                    "severity": severity,
                    "source": source,
                    "message": msg,
                })
            })
            .collect();
        let mut diags = diags;
        if !result.diagnostics.has_errors() {
            diags.extend(lint_diagnostics(uri, text));
        }
        Response::notification(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": diags }),
        )
    }

    /// Prefer project analysis so `use std.*` / sibling modules resolve like `super check`.
    /// Panics from the frontend must not kill the LSP process (Zed would stop the extension).
    fn analyze_doc(&self, uri: &str, text: &str) -> AnalyzeDoc {
        let uri_owned = uri.to_owned();
        let text_owned = text.to_owned();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::analyze_doc_inner(&uri_owned, &text_owned)
        })) {
            Ok(doc) => doc,
            Err(_) => {
                // Recover with an empty analysis plus a single diagnostic so the
                // editor stays responsive instead of killing `super-lsp`.
                let mut doc = Self::analyze_doc_inner(uri, "");
                doc.diagnostics = Diagnostics::new();
                doc.diagnostics.error_code_help(
                    DiagnosticKind::Other,
                    Span::new(BytePos(0), BytePos(text.len().max(1) as u32)),
                    "E9999",
                    "internal LSP analysis panic (recovered) — try undoing the last edit",
                    "the language server stayed alive; please report a repro if this repeats",
                );
                doc
            }
        }
    }

    fn analyze_doc_inner(uri: &str, text: &str) -> AnalyzeDoc {
        if let Some(path) = uri_to_path(uri) {
            let pa = analyze_project_text(&path, text);
            return AnalyzeDoc {
                diagnostics: pa.diagnostics,
                typed: pa.typed,
                interner: pa.interner,
                module: pa.module,
                meta_cache: pa.meta_cache,
                std_root: pa.std_root,
                import_targets: pa.import_targets,
                module_docs: pa.module_docs,
                symbol_origins: pa.symbol_origins,
                tokens: pa.tokens,
            };
        }
        let file = SourceFile::new(uri, text);
        let r = analyze(&file);
        let mut module_docs = HashMap::new();
        if !r.interner.resolve(r.module.docs).is_empty() {
            module_docs.insert(PathBuf::from(uri), r.module.docs);
        }
        AnalyzeDoc {
            diagnostics: r.diagnostics,
            typed: r.typed,
            interner: r.interner,
            module: r.module,
            meta_cache: r.meta_cache,
            std_root: None,
            import_targets: Vec::new(),
            module_docs,
            symbol_origins: HashMap::new(),
            tokens: r.tokens,
        }
    }

    fn hover(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let line = req.params["position"]["line"].as_u64().unwrap_or(0) as u32;
        let character = req.params["position"]["character"].as_u64().unwrap_or(0) as u32;
        let pos = file.offset_at_lsp(line, character);
        let result = self.analyze_doc(uri, &text);
        let info = hover_at(&result, &file, pos, &text);
        serde_json::to_string(&Response::result(
            req.id,
            json!({
                "contents": {
                    "kind": "markdown",
                    "value": info
                }
            }),
        ))
        .unwrap()
    }

    fn code_lens(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let result = self.analyze_doc(uri, &text);
        let (laws, _proofs) =
            super_frontend::collect_verify_items(&result.module, &result.interner);
        let mut lenses = Vec::new();
        for law in laws {
            let (line, character) = file.lsp_position(law.span.lo);
            let name = result.interner.resolve(law.name);
            lenses.push(json!({
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character }
                },
                "command": {
                    "title": format!("Verify Law `{name}`"),
                    "command": "super.verifyLaw",
                    "arguments": [uri, name]
                }
            }));
            lenses.push(json!({
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character }
                },
                "command": {
                    "title": "Show Counterexample",
                    "command": "super.showCounterexample",
                    "arguments": [uri, name]
                }
            }));
        }
        serde_json::to_string(&Response::result(req.id, json!(lenses))).unwrap()
    }

    fn definition(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let pos = request_position(&req, &file);
        let result = self.analyze_doc(uri, &text);
        let location = definition_location(&result, uri, &file, &text, pos);
        serde_json::to_string(&Response::result(req.id, location.unwrap_or(Value::Null))).unwrap()
    }

    fn references(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let pos = request_position(&req, &file);
        let result = self.analyze_doc(uri, &text);
        let include_declaration = req.params["context"]["includeDeclaration"]
            .as_bool()
            .unwrap_or(true);
        let locations = resolve_symbol_at(&result, &text, pos)
            .map(|symbol| {
                symbol_occurrences(&result, &text, &symbol, include_declaration)
                    .into_iter()
                    .map(|span| location_json(uri, &file, span))
                    .collect()
            })
            .unwrap_or_default();
        serde_json::to_string(&Response::result(req.id, Value::Array(locations))).unwrap()
    }

    fn rename(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let pos = request_position(&req, &file);
        let new_name = req.params["newName"].as_str().unwrap_or("");
        let result = self.analyze_doc(uri, &text);
        let edits: Vec<Value> = if is_valid_ident(new_name) {
            resolve_symbol_at(&result, &text, pos)
                .map(|symbol| {
                    symbol_occurrences(&result, &text, &symbol, true)
                        .into_iter()
                        .map(|span| {
                            json!({
                                "range": span_range_json(&file, span),
                                "newText": new_name
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        serde_json::to_string(&Response::result(
            req.id,
            json!({ "changes": { (uri): edits } }),
        ))
        .unwrap()
    }

    fn inlay_hints(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let result = self.analyze_doc(uri, &text);
        let hints: Vec<Value> = result
            .typed
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(symbol.kind, SymbolKind::Local | SymbolKind::Const)
                    && !declaration_has_type(&text, &result.interner, symbol)
            })
            .filter_map(|symbol| {
                let name_span = symbol_name_span(&text, &result.interner, symbol)?;
                let (_, end) = file.span_line_col(name_span);
                Some(json!({
                    "position": { "line": end.0 - 1, "character": end.1 - 1 },
                    "label": format!(": {}", format_type(&result.typed, &result.interner, symbol.ty)),
                    "kind": 1,
                    "paddingLeft": false,
                    "paddingRight": true
                }))
            })
            .collect();
        serde_json::to_string(&Response::result(req.id, Value::Array(hints))).unwrap()
    }

    fn signature_help(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let pos = request_position(&req, &file);
        let result = self.analyze_doc(uri, &text);
        let Some((name, active_parameter)) = call_at(&text, pos.as_usize()) else {
            return serde_json::to_string(&Response::result(req.id, Value::Null)).unwrap();
        };
        let bare = name.rsplit('.').next().unwrap_or(name);
        let receiver = name.rfind('.').and_then(|dot| {
            let line_off = text[..pos.as_usize()].rfind(name).unwrap_or(0);
            receiver_type_before_dot(&result.typed, &text, line_off + dot)
        });
        let mut hits: Vec<_> = result
            .typed
            .functions
            .iter()
            .filter(|(sym, _)| {
                let candidate = result.interner.resolve(**sym);
                candidate == name || candidate.rsplit('.').next() == Some(bare)
            })
            .collect();
        if let Some(recv) = receiver {
            let matched: Vec<_> = hits
                .iter()
                .copied()
                .filter(|(_, info)| {
                    info.owner.is_some_and(|owner| {
                        type_accepts_owner(&result.typed, &result.interner, owner, recv)
                    })
                })
                .collect();
            if !matched.is_empty() {
                hits = matched;
            }
        }
        let signatures: Vec<Value> = hits
            .into_iter()
            .take(8)
            .map(|(sym, info)| {
                let label = format_fn_signature(
                    &result.typed,
                    &result.interner,
                    result.interner.resolve(*sym),
                    info,
                );
                let parameters: Vec<Value> = info
                    .params
                    .iter()
                    .map(|(param, ty, mutable)| {
                        json!({
                            "label": format!(
                                "{}{}: {}",
                                if *mutable { "@" } else { "" },
                                result.interner.resolve(*param),
                                format_type(&result.typed, &result.interner, *ty)
                            )
                        })
                    })
                    .collect();
                let mut signature = json!({
                    "label": label,
                    "parameters": parameters
                });
                if let Some(doc) = markdown_doc(result.interner.resolve(info.docs)) {
                    signature["documentation"] = doc;
                }
                signature
            })
            .collect();
        let signature = if signatures.is_empty() {
            Value::Null
        } else {
            json!({
                "signatures": signatures,
                "activeSignature": 0,
                "activeParameter": active_parameter
            })
        };
        serde_json::to_string(&Response::result(req.id, signature)).unwrap()
    }

    fn document_symbols(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let result = self.analyze_doc(uri, &text);
        let symbols: Vec<Value> = result
            .module
            .items
            .iter()
            .filter_map(|item| document_symbol(item, &result.interner, &file, &text))
            .collect();
        serde_json::to_string(&Response::result(req.id, Value::Array(symbols))).unwrap()
    }

    fn completion(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let pos = request_position(&req, &file);
        let result = self.analyze_doc(uri, &text);
        let std_import_suffix = stdlib_import_suffix_at(&text, pos.as_usize());
        let in_std_import = std_import_suffix.is_some();
        let ufcs_only = !in_std_import && dot_receiver_type(&result.typed, &text, pos).is_some();
        // Inside `use std…`, only path-segment / module completions — never dump
        // unscoped types, methods, or keywords (`.` after `std.net` is not UFCS).
        if in_std_import {
            let filter = std_import_suffix
                .as_ref()
                .and_then(|inner| inner.as_deref())
                .unwrap_or("");
            let items = stdlib_import_completion_items(result.std_root.as_deref(), filter);
            return serde_json::to_string(&Response::result(req.id, Value::Array(items))).unwrap();
        }
        let mut items = vec![
            json!({ "label": "it", "kind": 6 }),
            json!({ "label": "index", "kind": 6 }),
            json!({ "label": "err", "kind": 6 }),
            json!({ "label": "or", "kind": 14 }),
            json!({ "label": "ensure", "kind": 14 }),
            json!({ "label": "for", "kind": 14 }),
            json!({ "label": "for parallel", "kind": 14, "detail": "M:N SIMD loop" }),
            json!({ "label": "when", "kind": 14 }),
            json!({ "label": "scope", "kind": 14, "detail": "structured concurrency" }),
            json!({ "label": "spawn", "kind": 14, "detail": "fiber task" }),
            json!({ "label": "await", "kind": 14 }),
            json!({ "label": "chan", "kind": 14, "detail": "chan[T](cap)" }),
            json!({ "label": "shared", "kind": 14, "detail": "atomic / futex cell" }),
            json!({ "label": "after", "kind": 14, "detail": "timer arm in when" }),
            json!({ "label": "send", "kind": 2, "detail": "ch.send(val)" }),
            json!({ "label": "recv", "kind": 2, "detail": "ch.recv()" }),
            json!({ "label": "close", "kind": 2, "detail": "ch.close()" }),
            json!({ "label": "load", "kind": 2, "detail": "shared.load()" }),
            json!({ "label": "test", "kind": 14 }),
            json!({ "label": "trait", "kind": 14, "detail": "+Name = trait { signatures }" }),
            json!({ "label": "+Name: Trait =", "kind": 14, "detail": "declare a trait inline on a type" }),
            json!({ "label": "display", "kind": 2, "detail": "Display.display(self) : str — interpolation witness" }),
            json!({ "label": "debug", "kind": 2, "detail": "Debug.debug(self) : str" }),
            json!({ "label": "to_str", "kind": 2, "detail": "ToString.to_str(self) : str" }),
            json!({ "label": "eq", "kind": 2, "detail": "Eq.eq(self, other) : bool" }),
            json!({ "label": "cmp", "kind": 2, "detail": "Ord.cmp(self, other) : i64" }),
            json!({ "label": "hash", "kind": 2, "detail": "Hash.hash(self) : i64" }),
            json!({ "label": "clone", "kind": 2, "detail": "Clone.clone(self)" }),
            json!({ "label": "{a, b} =", "kind": 14, "detail": "destructuring binding" }),
            json!({ "label": "(.)", "kind": 3, "detail": "call param punning" }),
            json!({ "label": "[lo..hi]", "kind": 14, "detail": "slice shorthand" }),
            json!({ "label": "16.ms", "kind": 14, "detail": "duration literal" }),
            json!({ "label": "#", "kind": 14, "detail": "compile-time value / block" }),
            json!({ "label": "#{ }", "kind": 14, "detail": "comptime block" }),
            json!({ "label": "#if", "kind": 14, "detail": "comptime conditional" }),
            json!({ "label": "#for", "kind": 14, "detail": "comptime unroll loop" }),
            json!({ "label": "'{ }", "kind": 14, "detail": "hygienic Code[T] quote" }),
            json!({ "label": "${ }", "kind": 14, "detail": "splice Code[T] into AST" }),
            json!({ "label": "#goal_type", "kind": 14, "detail": "goal-type reflection" }),
            json!({ "label": "Code", "kind": 25, "detail": "Code[T] quoted AST type" }),
            json!({ "label": "type", "kind": 25, "detail": "type meta-parameter" }),
            json!({ "label": ".fields", "kind": 5, "detail": "comptime type fields" }),
            json!({ "label": ".variants", "kind": 5, "detail": "comptime sum variants" }),
            json!({ "label": ".size", "kind": 5, "detail": "C-ABI byte size" }),
            json!({ "label": ".align", "kind": 5, "detail": "C-ABI alignment" }),
            json!({ "label": "#unroll(N)", "kind": 15, "detail": "unroll next for loop" }),
            json!({ "label": "#simd", "kind": 15, "detail": "SIMD hint on for" }),
            json!({ "label": "#inline", "kind": 15, "detail": "inline next function" }),
            json!({ "label": "#assert_size(T, N)", "kind": 15, "detail": "C-ABI size check" }),
            json!({ "label": "#inspect(T)", "kind": 15, "detail": "print comptime type/AST" }),
            json!({ "label": "#route", "kind": 15, "detail": "generic HTTP route attr: #route(\"GET\", \"/path\")" }),
            json!({ "label": "#get", "kind": 15, "detail": "comptime HTTP GET route attr", "documentation": "Attaches a GET route to the next function or lambda. Not a method." }),
            json!({ "label": "#post", "kind": 15, "detail": "comptime HTTP POST route attr" }),
            json!({ "label": "#put", "kind": 15, "detail": "comptime HTTP PUT route attr" }),
            json!({ "label": "#delete", "kind": 15, "detail": "comptime HTTP DELETE route attr" }),
            json!({ "label": "#todo", "kind": 15, "detail": "panic: not yet implemented" }),
            json!({ "label": "#unimplemented", "kind": 15, "detail": "panic: unfinished path" }),
            json!({ "label": "#unreachable", "kind": 15, "detail": "panic: unreachable code" }),
            json!({ "label": "#assert", "kind": 15, "detail": "panic when condition is false" }),
            json!({ "label": "#dbg", "kind": 15, "detail": "print expr and yield it" }),
            json!({ "label": "panic", "kind": 14, "detail": "unrecoverable abort" }),
            json!({ "label": "|x| =>", "kind": 15, "detail": "lambda expression" }),
            json!({ "label": "#blocking", "kind": 15, "detail": "offload blocking host call" }),
            json!({ "label": "rewrite", "kind": 14, "detail": "algebraic AST rewrite rule" }),
        ];
        if !ufcs_only {
            for (name, info) in &result.typed.functions {
                if info.owner.is_some() {
                    continue;
                }
                let label = result.interner.resolve(*name);
                let detail = format_fn_signature(&result.typed, &result.interner, label, info);
                let mut item = json!({
                    "label": label,
                    "kind": 3,
                    "detail": detail,
                });
                if let Some(doc) = markdown_doc(result.interner.resolve(info.docs)) {
                    item["documentation"] = doc;
                }
                items.push(item);
            }
            for (name, tid) in &result.typed.structs {
                let label = result.interner.resolve(*name);
                let mut item = json!({
                    "label": label,
                    "kind": 22
                });
                if let Some(doc_id) = result.typed.type_docs.get(name)
                    && let Some(doc) = markdown_doc(result.interner.resolve(*doc_id))
                {
                    item["documentation"] = doc;
                }
                items.push(item);
                if let TypeKind::Struct { fields, .. } = result.typed.types.get_type(*tid) {
                    for f in fields {
                        if f.private {
                            continue;
                        }
                        let mut field = json!({
                            "label": format!("+{}", result.interner.resolve(f.name)),
                            "kind": 5,
                            "detail": format!("{label}.{}", result.interner.resolve(f.name)),
                        });
                        if let Some(doc) = markdown_doc(result.interner.resolve(f.docs)) {
                            field["documentation"] = doc;
                        }
                        items.push(field);
                    }
                }
            }
            for item in &result.module.items {
                match item {
                    Item::Function(f) if f.exported => items.push(json!({
                        "label": format!("+{}", result.interner.resolve(f.name)),
                        "kind": 2
                    })),
                    Item::Binding(b) if b.exported => items.push(json!({
                        "label": format!("+{}", result.interner.resolve(b.name)),
                        "kind": 6
                    })),
                    Item::Struct(s) if s.exported => items.push(json!({
                        "label": format!("+{}", result.interner.resolve(s.name)),
                        "kind": 22
                    })),
                    Item::Sum(s) if s.exported => items.push(json!({
                        "label": format!("+{}", result.interner.resolve(s.name)),
                        "kind": 10
                    })),
                    _ => {}
                }
            }
        }
        if let Some(receiver) = dot_receiver_type(&result.typed, &text, pos) {
            push_ufcs_method_completions(&mut items, &result.typed, &result.interner, receiver);
        } else if !ufcs_only {
            for (name, info) in &result.typed.functions {
                if info.owner.is_some() {
                    let label = result.interner.resolve(*name);
                    let display = info
                        .owner
                        .map(|o| format!("{}.{}", result.interner.resolve(o), label))
                        .unwrap_or_else(|| label.to_string());
                    let detail =
                        format_fn_signature(&result.typed, &result.interner, &display, info);
                    let mut item = json!({
                        "label": label,
                        "kind": 2,
                        "detail": detail,
                        "filterText": display,
                    });
                    if let Some(doc) = markdown_doc(result.interner.resolve(info.docs)) {
                        item["documentation"] = doc;
                    }
                    items.push(item);
                }
            }
        }
        let _ = Expr::None; // keep ast import used for future expression completions
        serde_json::to_string(&Response::result(req.id, Value::Array(items))).unwrap()
    }

    fn semantic_tokens(&self, req: Request) -> String {
        let uri = req.params["textDocument"]["uri"].as_str().unwrap_or("");
        let text = self.docs.get(uri).cloned().unwrap_or_default();
        let file = SourceFile::new(uri, &text);
        let result = self.analyze_doc(uri, &text);
        // Tokens are emitted from AST/symbol spans (not a greedy numeric scan).
        // Range exprs (`0..len(xs)`) keep full start/end spans in the parser;
        // we do not synthesize a truncated "range" token that stops at hex-looking
        // letters after `..`.
        let mut tokens: Vec<(Span, u32, u32)> = Vec::new();
        for item in &result.module.items {
            let token = match item {
                Item::Function(function) => {
                    name_span_within(&text, function.span, result.interner.resolve(function.name))
                        .map(|span| (span, 12, 1 | if function.exported { 1 << 10 } else { 0 }))
                }
                Item::Binding(binding) => {
                    name_span_within(&text, binding.span, result.interner.resolve(binding.name))
                        .map(|span| {
                            (
                                span,
                                if binding.comptime { 14 } else { 8 },
                                1 | (if binding.mutable { 1 << 7 } else { 1 << 2 })
                                    | (if binding.comptime { 1 << 3 } else { 0 })
                                    | (if binding.exported { 1 << 10 } else { 0 }),
                            )
                        })
                }
                Item::Struct(def) => {
                    name_span_within(&text, def.span, result.interner.resolve(def.name))
                        .map(|span| (span, 5, 1 | if def.exported { 1 << 10 } else { 0 }))
                }
                Item::Sum(def) => {
                    name_span_within(&text, def.span, result.interner.resolve(def.name))
                        .map(|span| (span, 3, 1 | if def.exported { 1 << 10 } else { 0 }))
                }
                Item::Trait(def) => {
                    name_span_within(&text, def.span, result.interner.resolve(def.name))
                        .map(|span| (span, 5, 1 | if def.exported { 1 << 10 } else { 0 }))
                }
                Item::Rewrite(rewrite) => {
                    name_span_within(&text, rewrite.span, result.interner.resolve(rewrite.name))
                        .map(|span| (span, 14, 1 | 1 << 3))
                }
                _ => None,
            };
            if let Some(token) = token {
                tokens.push(token);
            }
        }
        for symbol in &result.typed.symbols {
            let Some(span) = symbol_name_span(&text, &result.interner, symbol) else {
                continue;
            };
            let token_type = match symbol.kind {
                SymbolKind::Param => 7,
                SymbolKind::Import => 0,
                _ => 8,
            };
            let modifiers = 1
                | (if symbol.mutable { 1 << 7 } else { 1 << 2 })
                | (if matches!(symbol.storage, StorageClass::ConstExpr) {
                    1 << 3
                } else {
                    0
                });
            tokens.push((span, token_type, modifiers));
        }
        for reference in &result.typed.symbol_references {
            let mutable = result.typed.symbols.iter().any(|symbol| {
                symbol.name == reference.name
                    && symbol.def_span == reference.def_span
                    && symbol.mutable
            });
            let parameter = result.typed.symbols.iter().any(|symbol| {
                symbol.name == reference.name
                    && symbol.def_span == reference.def_span
                    && symbol.kind == SymbolKind::Param
            });
            tokens.push((
                reference.span,
                if parameter { 7 } else { 8 },
                if mutable { 1 << 7 } else { 1 << 2 },
            ));
        }
        for tok in &result.tokens {
            if !matches!(tok.kind, TokenKind::DocOuter | TokenKind::DocInner) {
                continue;
            }
            if tok.span.hi.as_usize() > text.len() {
                continue;
            }
            // comment = 16, documentation modifier = bit 8
            tokens.push((tok.span, 16, 1 << 8));
        }
        tokens.sort_by_key(|(span, _, _)| (span.lo, span.hi));
        tokens.dedup_by_key(|(span, _, _)| (span.lo, span.hi));

        // Encoded as LSP deltas: line, startChar, length, tokenType, tokenModifiers.
        let mut data: Vec<u32> = Vec::new();
        let mut last_line = 0u32;
        let mut last_char = 0u32;
        for (span, token_type, mods) in tokens {
            let ((line, col), _) = file.span_line_col(span);
            let line0 = line - 1;
            let col0 = col - 1;
            let d_line = line0 - last_line;
            let d_char = if d_line == 0 {
                col0.saturating_sub(last_char)
            } else {
                col0
            };
            data.extend_from_slice(&[d_line, d_char, span.len().max(1), token_type, mods]);
            last_line = line0;
            last_char = col0;
        }
        serde_json::to_string(&Response::result(req.id, json!({ "data": data }))).unwrap()
    }
}

impl Default for LspServer {
    fn default() -> Self {
        Self::new()
    }
}

struct AnalyzeDoc {
    diagnostics: Diagnostics,
    typed: TypedModule,
    interner: Interner,
    module: Module,
    meta_cache: MetaCache,
    std_root: Option<PathBuf>,
    import_targets: Vec<ImportTarget>,
    module_docs: HashMap<PathBuf, super_core::SymId>,
    symbol_origins: HashMap<super_core::SymId, SymbolOrigin>,
    tokens: Vec<super_frontend::Token>,
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    // Minimal percent-decoding for spaces and common path chars.
    let mut out = String::with_capacity(path.len());
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = from_hex(bytes[i + 1])?;
            let l = from_hex(bytes[i + 2])?;
            out.push((h << 4 | l) as char);
            i += 3;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    let p = PathBuf::from(out);
    if p.exists() || p.extension().and_then(|e| e.to_str()) == Some("sr") {
        Some(p)
    } else {
        None
    }
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[derive(Clone, Copy)]
struct ResolvedSymbol {
    name: super_core::SymId,
    def_span: Span,
}

fn request_position(req: &Request, file: &SourceFile) -> BytePos {
    let line = req.params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = req.params["position"]["character"].as_u64().unwrap_or(0) as u32;
    file.offset_at_lsp(line, character)
}

fn span_contains(span: Span, pos: BytePos) -> bool {
    span.lo.0 <= pos.0 && pos.0 < span.hi.0.max(span.lo.0.saturating_add(1))
}

fn symbol_name_span(text: &str, interner: &Interner, symbol: &SymbolInfo) -> Option<Span> {
    name_span_within(text, symbol.def_span, interner.resolve(symbol.name))
}

fn name_span_within(text: &str, span: Span, name: &str) -> Option<Span> {
    if name.is_empty() || name.starts_with("__") {
        return None;
    }
    let lo = span.lo.as_usize().min(text.len());
    let hi = span.hi.as_usize().min(text.len());
    let slice = &text[lo..hi];
    for (relative, _) in slice.match_indices(name) {
        let start = lo + relative;
        let end = start + name.len();
        let before = start.checked_sub(1).and_then(|i| text.as_bytes().get(i));
        let after = text.as_bytes().get(end);
        if !before.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            && !after.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            return Some(Span::from_offsets(start as u32, end as u32));
        }
    }
    None
}

fn resolve_symbol_at(result: &AnalyzeDoc, text: &str, pos: BytePos) -> Option<ResolvedSymbol> {
    for symbol in &result.typed.symbols {
        let Some(name_span) = symbol_name_span(text, &result.interner, symbol) else {
            continue;
        };
        if span_contains(name_span, pos) {
            return Some(ResolvedSymbol {
                name: symbol.name,
                def_span: symbol.def_span,
            });
        }
    }
    if let Some(reference) = result
        .typed
        .symbol_references
        .iter()
        .filter(|reference| span_contains(reference.span, pos))
        .min_by_key(|reference| reference.span.len())
    {
        return Some(ResolvedSymbol {
            name: reference.name,
            def_span: reference.def_span,
        });
    }
    let word = ident_at(text, pos.as_usize())?;
    let bare = word.trim_start_matches(['+', '-', '#', '@']);
    let name = result
        .interner
        .lookup(bare)
        .or_else(|| result.interner.lookup(word))?;

    if let Some(info) = result.typed.functions.get(&name) {
        return Some(ResolvedSymbol {
            name,
            def_span: info.span,
        });
    }
    if let Some(info) = result.typed.bindings.get(&name) {
        return Some(ResolvedSymbol {
            name,
            def_span: info.span,
        });
    }
    if let Some(origin) = result.symbol_origins.get(&name) {
        return Some(ResolvedSymbol {
            name,
            def_span: origin.span,
        });
    }
    None
}

/// Apply LSP `textDocument/didChange` content changes (full or incremental).
fn apply_content_changes(base: &str, changes: &[Value]) -> String {
    let mut doc = base.to_string();
    for change in changes {
        let Some(replacement) = change.get("text").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(range) = change.get("range") else {
            // Full-document sync: `text` replaces the entire buffer.
            doc = replacement.to_owned();
            continue;
        };
        let file = SourceFile::new("", &doc);
        let start = file.offset_at_lsp(
            range["start"]["line"].as_u64().unwrap_or(0) as u32,
            range["start"]["character"].as_u64().unwrap_or(0) as u32,
        );
        let end = file.offset_at_lsp(
            range["end"]["line"].as_u64().unwrap_or(0) as u32,
            range["end"]["character"].as_u64().unwrap_or(0) as u32,
        );
        let lo = start.as_usize().min(doc.len());
        let hi = end.as_usize().min(doc.len()).max(lo);
        doc.replace_range(lo..hi, replacement);
    }
    doc
}

fn symbol_occurrences(
    result: &AnalyzeDoc,
    text: &str,
    symbol: &ResolvedSymbol,
    include_declaration: bool,
) -> Vec<Span> {
    let mut spans = Vec::new();
    if include_declaration {
        let decl = name_span_within(text, symbol.def_span, result.interner.resolve(symbol.name))
            .unwrap_or(symbol.def_span);
        spans.push(decl);
    }
    spans.extend(
        result
            .typed
            .symbol_references
            .iter()
            .filter(|reference| {
                reference.name == symbol.name && reference.def_span == symbol.def_span
            })
            .map(|reference| reference.span),
    );
    spans.sort_by_key(|span| (span.lo, span.hi));
    spans.dedup();
    spans
        .into_iter()
        .filter_map(|span| {
            if span.len() <= result.interner.resolve(symbol.name).len() as u32 {
                Some(span)
            } else {
                name_span_within(text, span, result.interner.resolve(symbol.name))
            }
        })
        .collect()
}

fn span_range_json(file: &SourceFile, span: Span) -> Value {
    let ((sl, sc), (el, ec)) = file.lsp_span_range(span);
    json!({
        "start": { "line": sl, "character": sc },
        "end": { "line": el, "character": ec }
    })
}

fn location_json(uri: &str, file: &SourceFile, span: Span) -> Value {
    json!({ "uri": uri, "range": span_range_json(file, span) })
}

fn definition_location(
    result: &AnalyzeDoc,
    entry_uri: &str,
    entry_file: &SourceFile,
    text: &str,
    pos: BytePos,
) -> Option<Value> {
    if let Some(import) = result
        .import_targets
        .iter()
        .find(|target| span_contains(target.span, pos))
    {
        return external_location_at(result, &import.path, Span::from_offsets(0, 1));
    }
    if let Some(symbol) = resolve_symbol_at(result, text, pos) {
        if let Some(origin) = result.symbol_origins.get(&symbol.name) {
            // Prefer the origin module for imports; same-file defs keep the entry URI.
            if origin.path.as_os_str().is_empty() {
                return Some(location_json(entry_uri, entry_file, symbol.def_span));
            }
            return external_location_at(result, &origin.path, origin.span);
        }
        // Jump to the declaration name inside the current buffer.
        let target = name_span_within(text, symbol.def_span, result.interner.resolve(symbol.name))
            .unwrap_or(symbol.def_span);
        return Some(location_json(entry_uri, entry_file, target));
    }
    None
}

fn external_location_at(result: &AnalyzeDoc, path: &Path, def_span: Span) -> Option<Value> {
    let uri = module_path_to_file_uri(path, result.std_root.as_deref())?;
    let src = module_source_text(path, result.std_root.as_deref())?;
    let file = SourceFile::new(&uri, &src);
    Some(location_json(&uri, &file, def_span))
}

fn is_valid_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn declaration_has_type(text: &str, interner: &Interner, symbol: &SymbolInfo) -> bool {
    let Some(name_span) = symbol_name_span(text, interner, symbol) else {
        return true;
    };
    let after_name = name_span.hi.as_usize().min(text.len());
    let end = symbol.def_span.hi.as_usize().min(text.len());
    text[after_name..end]
        .split('=')
        .next()
        .is_some_and(|prefix| prefix.contains(':'))
}

fn call_at(text: &str, offset: usize) -> Option<(&str, u32)> {
    let bytes = text.as_bytes();
    let mut depth = 0u32;
    let mut open = None;
    for i in (0..offset.min(bytes.len())).rev() {
        match bytes[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' if depth == 0 => {
                open = Some(i);
                break;
            }
            b'(' | b'[' | b'{' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let open = open?;
    let mut end = open;
    while end > 0 && text.as_bytes()[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let byte = text.as_bytes()[start - 1];
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.') {
            start -= 1;
        } else {
            break;
        }
    }
    let name = &text[start..end];
    if name.is_empty() {
        return None;
    }
    let mut active = 0u32;
    let mut nested = 0u32;
    for byte in &bytes[open + 1..offset.min(bytes.len())] {
        match byte {
            b'(' | b'[' | b'{' => nested += 1,
            b')' | b']' | b'}' => nested = nested.saturating_sub(1),
            b',' if nested == 0 => active += 1,
            _ => {}
        }
    }
    Some((name, active))
}

fn document_symbol(
    item: &Item,
    interner: &Interner,
    file: &SourceFile,
    text: &str,
) -> Option<Value> {
    let (name, kind, span, name_sym) = match item {
        Item::Use(import) => (
            {
                let mut label = import
                    .path
                    .iter()
                    .map(|part| interner.resolve(*part))
                    .collect::<Vec<_>>()
                    .join(".");
                if let Some(alias) = import.alias {
                    label = format!("{label} as {}", interner.resolve(alias));
                }
                label
            },
            2,
            import.span,
            import.alias.or_else(|| import.path.last().copied()),
        ),
        Item::Binding(binding) => (
            interner.resolve(binding.name).to_owned(),
            if binding.comptime { 14 } else { 13 },
            binding.span,
            Some(binding.name),
        ),
        Item::Struct(def) => (
            interner.resolve(def.name).to_owned(),
            23,
            def.span,
            Some(def.name),
        ),
        Item::Sum(def) => (
            interner.resolve(def.name).to_owned(),
            10,
            def.span,
            Some(def.name),
        ),
        Item::Function(function) => (
            interner.resolve(function.name).to_owned(),
            12,
            function.span,
            Some(function.name),
        ),
        Item::State(state) => (
            interner.resolve(state.name).to_owned(),
            13,
            state.span,
            Some(state.name),
        ),
        Item::Test(test) => (
            format!("test {}", interner.resolve(test.name)),
            12,
            test.span,
            None,
        ),
        Item::Rewrite(rewrite) => (
            interner.resolve(rewrite.name).to_owned(),
            12,
            rewrite.span,
            Some(rewrite.name),
        ),
        Item::Trait(def) => (
            interner.resolve(def.name).to_owned(),
            11,
            def.span,
            Some(def.name),
        ),
        Item::Macro(def) => (
            format!("#{}", interner.resolve(def.name)),
            12,
            def.span,
            Some(def.name),
        ),
    };
    if name.starts_with("__") {
        return None;
    }
    let selection = name_sym
        .and_then(|sym| name_span_within(text, span, interner.resolve(sym)))
        .unwrap_or(span);
    let mut symbol = json!({
        "name": name,
        "kind": kind,
        "range": span_range_json(file, span),
        "selectionRange": span_range_json(file, selection)
    });
    if let Some(detail) = item_doc_detail(item, interner) {
        symbol["detail"] = json!(detail);
    }
    Some(symbol)
}

fn item_doc_detail(item: &Item, interner: &Interner) -> Option<String> {
    let docs = match item {
        Item::Function(f) => f.docs,
        Item::Binding(b) => b.docs,
        Item::Struct(s) => s.docs,
        Item::Sum(s) => s.docs,
        Item::State(s) => s.docs,
        Item::Test(t) => t.docs,
        Item::Rewrite(r) => r.docs,
        Item::Trait(t) => t.docs,
        Item::Use(u) => u.docs,
        Item::Macro(m) => m.docs,
    };
    let text = interner.resolve(docs);
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    Some(line.trim().to_string())
}

fn hover_at(result: &AnalyzeDoc, file: &SourceFile, pos: BytePos, text: &str) -> String {
    if let Some(doc) = doc_comment_hover(text, pos.as_usize()) {
        return doc;
    }
    let word = ident_at(text, pos.as_usize());
    let typed = &result.typed;
    let interner = &result.interner;

    // `#get`, `#todo`, `#inline`, … — never resolve as ordinary methods/idents.
    if let Some(macro_card) = hash_macro_hover_at(text, pos.as_usize(), word) {
        return macro_card;
    }

    if let Some(sigil) = sigil_hover(text, pos.as_usize()) {
        return sigil;
    }

    // Soft keywords after `.` are methods/fields (`@app.use`), not import/control forms.
    let after_dot = word
        .and_then(|_| ident_span_at(text, pos.as_usize()))
        .is_some_and(|(lo, _)| preceded_by_dot(text, lo));
    if !after_dot && let Some((title, note)) = word.and_then(keyword_hover) {
        return md_hover(title, note);
    }

    if let Some(builtin) = word.and_then(builtin_type_hover) {
        return builtin;
    }

    if let Some(comptime) = comptime_hover(result, file, pos, text, word) {
        return comptime;
    }

    if let Some(label) = label_hover(text, pos.as_usize(), result, word) {
        return label;
    }

    if let Some(name) = word {
        let bare = name.trim_start_matches(['+', '-', '#', '@']);
        if member_receiver_type(typed, interner, text, pos).is_some()
            && let Some(card) = member_hover(typed, interner, bare, text, pos)
        {
            return card;
        }
    }

    if let Some(symbol) = resolve_symbol_at(result, text, pos)
        && let Some(info) = result
            .typed
            .symbols
            .iter()
            .find(|info| info.name == symbol.name && info.def_span == symbol.def_span)
    {
        return format_symbol_card(result, text, file, info);
    }

    if let Some(name) = word {
        let bare = name.trim_start_matches(['+', '-', '#', '@']);
        let sym = interner.lookup(bare).or_else(|| interner.lookup(name));

        if let Some(sym) = sym {
            if let Some(tid) = typed.structs.get(&sym) {
                return format_struct_card(typed, interner, bare, *tid);
            }
            if let Some(lines) = typed.trait_sigs.get(&sym) {
                return format_trait_card(bare, lines);
            }
            if let Some(tid) = typed.sums.get(&sym) {
                return format_sum_card(typed, interner, bare, *tid);
            }
            if let Some(info) = typed.functions.get(&sym) {
                return format_fn_card(typed, interner, bare, info);
            }
            // UFCS / nested methods are keyed as `Owner.method`.
            if let Some(card) = find_method_card(typed, interner, bare, text, pos) {
                return card;
            }
            if let Some(info) = typed.bindings.get(&sym) {
                return format_binding_card(typed, interner, bare, info);
            }
            if let Some(val) = result.meta_cache.bindings.get(&sym) {
                let body = format!("#{bare} = {}", format_ct_value(val));
                return md_hover(&body, "compile-time constant · stripped from runtime");
            }

            let mut field_hits = Vec::new();
            for (sname, tid) in &typed.structs {
                if let TypeKind::Struct {
                    fields,
                    size,
                    align,
                    ..
                } = typed.types.get_type(*tid)
                    && let Some(f) = fields.iter().find(|f| f.name == sym)
                {
                    field_hits.push((
                        interner.resolve(*sname).to_string(),
                        interner.resolve(f.name).to_string(),
                        f.offset,
                        f.private,
                        format_type(typed, interner, f.ty),
                        *size,
                        *align,
                        interner.resolve(f.docs).to_string(),
                    ));
                }
            }
            if let Some(card) = render_field_hits(bare, &field_hits) {
                return card;
            }
        }

        if let Some(card) = core_trait_hover(bare) {
            return card;
        }

        if let Some(ty) = best_expr_type(typed, pos) {
            return format_typed_name(typed, interner, bare, ty);
        }

        return md_hover(
            bare,
            "unresolved symbol in this scope — check spelling, `use`, or `+` export",
        );
    }

    if let Some(ty) = best_expr_type(typed, pos) {
        return format_typed_name(typed, interner, &format_type(typed, interner, ty), ty);
    }

    contextual_fallback(text, pos.as_usize())
}

fn current_line_prefix(text: &str, offset: usize) -> &str {
    let end = offset.min(text.len());
    let start = text[..end].rfind('\n').map(|i| i + 1).unwrap_or(0);
    &text[start..end]
}

/// `None` = not in a `use std…` import; `Some(None)` / `Some(Some(prefix))` = stdlib path filter.
fn stdlib_import_suffix_at(text: &str, offset: usize) -> Option<Option<String>> {
    let line = current_line_prefix(text, offset);
    let trimmed = line.trim_start();
    if !trimmed.starts_with("use ") {
        return None;
    }
    let rest = trimmed.strip_prefix("use ")?.trim_start();
    if !rest.starts_with("std") {
        return None;
    }
    let after = rest.strip_prefix("std").unwrap_or("");
    if !after.is_empty() && !after.starts_with('.') {
        return None;
    }
    // Stop at selectors / alias so `use std.net.http.{…}` stays an import context
    // only through the path portion.
    let path = after
        .strip_prefix('.')
        .unwrap_or("")
        .split(['{', ' ', '\t'])
        .next()
        .unwrap_or("")
        .trim();
    Some(Some(path.to_string()))
}

/// Completions for `use std…` — next path segment plus matching full modules.
fn stdlib_import_completion_items(std_root: Option<&std::path::Path>, filter: &str) -> Vec<Value> {
    let mut items = Vec::new();
    let parent = filter.trim().trim_end_matches('.');
    // Next segment under the current prefix (`use std.net.` → `http`, not `App`).
    for seg in stdlib_import_next_segments(parent) {
        let insert = if parent.is_empty() {
            format!("std.{seg}")
        } else {
            format!("{parent}.{seg}")
        };
        items.push(json!({
            "label": seg,
            "kind": 9,
            "detail": format!("use std.{insert}"),
            "insertText": seg,
            "filterText": format!("{seg} std.{insert}"),
            "sortText": format!("0{seg}"),
        }));
    }
    for module in stdlib_use_completions_for(std_root, Some(filter)) {
        // Insert only what remains after the already-typed prefix.
        let insert = if parent.is_empty() {
            module.manifest_path.clone()
        } else if let Some(rest) = module.manifest_path.strip_prefix(&format!("{parent}.")) {
            rest.to_string()
        } else if module.manifest_path.starts_with(parent) {
            module.manifest_path[parent.len()..]
                .trim_start_matches('.')
                .to_string()
        } else {
            module
                .manifest_path
                .rsplit('.')
                .next()
                .unwrap_or(&module.manifest_path)
                .to_string()
        };
        items.push(json!({
            "label": module.use_line,
            "kind": 9,
            "detail": format!("stdlib/{} · {}", module.source_rel, module.layer),
            "documentation": {
                "kind": "markdown",
                "value": format!("{}\n\n`{}`", module.summary, module.manifest_path)
            },
            "insertText": insert,
            "filterText": format!("{} {}", module.use_line, module.manifest_path),
            "sortText": format!("1{}", module.manifest_path),
        }));
    }
    items
}

fn dot_receiver_type(typed: &TypedModule, text: &str, pos: BytePos) -> Option<TypeId> {
    let offset = pos.as_usize();
    if offset == 0 || offset > text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut p = offset.saturating_sub(1);
    while p > 0 && bytes[p].is_ascii_whitespace() {
        p -= 1;
    }
    if bytes.get(p) != Some(&b'.') {
        return None;
    }
    receiver_type_before_dot(typed, text, p)
}

/// Type of the expression immediately before `dot`.
fn receiver_type_before_dot(typed: &TypedModule, text: &str, dot: usize) -> Option<TypeId> {
    if dot == 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut q = dot - 1;
    while q > 0 && bytes[q].is_ascii_whitespace() {
        q -= 1;
    }
    if bytes[q].is_ascii_whitespace() {
        return None;
    }
    best_expr_type(typed, BytePos(q as u32))
}

/// Receiver of the member under the cursor: `recv.member`, `recv.` , or `Type.{field`.
fn member_receiver_type(
    typed: &TypedModule,
    interner: &Interner,
    text: &str,
    pos: BytePos,
) -> Option<TypeId> {
    if let Some(ty) = dot_receiver_type(typed, text, pos) {
        return Some(ty);
    }
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let offset = pos.as_usize().min(bytes.len() - 1);
    let mut start = offset;
    if is_ident_byte(bytes[start]) {
        while start > 0 && is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
    }
    if start == 0 {
        return None;
    }
    let mut p = start - 1;
    while p > 0 && bytes[p].is_ascii_whitespace() {
        p -= 1;
    }
    if bytes[p] == b'.' {
        return receiver_type_before_dot(typed, text, p);
    }
    if bytes[p] == b'{' && p > 0 && bytes[p - 1] == b'.' {
        let mut end = p - 1;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let mut name_lo = end - 1;
        while name_lo > 0 && is_ident_byte(bytes[name_lo - 1]) {
            name_lo -= 1;
        }
        let name = &text[name_lo..end];
        let sym = interner.lookup(name)?;
        return typed
            .structs
            .get(&sym)
            .copied()
            .or_else(|| typed.sums.get(&sym).copied());
    }
    None
}

fn type_accepts_owner(
    typed: &TypedModule,
    interner: &Interner,
    owner: super_core::SymId,
    receiver: TypeId,
) -> bool {
    if typed.structs.get(&owner) == Some(&receiver) || typed.sums.get(&owner) == Some(&receiver) {
        return true;
    }
    match typed.types.get_type(receiver) {
        TypeKind::Named(n) if *n == owner => true,
        TypeKind::Struct { name, .. } if *name == owner => true,
        TypeKind::Sum { name, .. } if *name == owner => true,
        _ => interner.resolve(owner) == format_type(typed, interner, receiver),
    }
}

fn push_ufcs_method_completions(
    items: &mut Vec<Value>,
    typed: &TypedModule,
    interner: &Interner,
    receiver: TypeId,
) {
    if let TypeKind::Module { name } = typed.types.get_type(receiver) {
        let module = interner.resolve(*name);
        for type_name in typed.structs.keys() {
            let label = interner.resolve(*type_name);
            items.push(json!({
                "label": label,
                "kind": 22,
                "detail": format!("{module}.{label}"),
            }));
        }
        for type_name in typed.sums.keys() {
            let label = interner.resolve(*type_name);
            items.push(json!({
                "label": label,
                "kind": 10,
                "detail": format!("{module}.{label}"),
            }));
        }
        for (func_name, info) in &typed.functions {
            if info.owner.is_some() {
                continue;
            }
            let label = interner.resolve(*func_name);
            items.push(json!({
                "label": label,
                "kind": 3,
                "detail": format!("{module}.{label}()"),
            }));
        }
        return;
    }
    for (name, info) in &typed.functions {
        let Some(owner) = info.owner else {
            continue;
        };
        if !type_accepts_owner(typed, interner, owner, receiver) {
            continue;
        }
        let method = interner.resolve(*name);
        let owner_name = interner.resolve(owner);
        let detail = format_fn_signature(typed, interner, &format!("{owner_name}.{method}"), info);
        let mut item = json!({
            "label": method,
            "kind": 2,
            "detail": detail,
            "filterText": format!("{owner_name}.{method}")
        });
        if let Some(doc) = markdown_doc(interner.resolve(info.docs)) {
            item["documentation"] = doc;
        }
        items.push(item);
    }
    if let TypeKind::Struct { fields, .. } = typed.types.get_type(receiver) {
        for f in fields {
            if f.private {
                continue;
            }
            let mut item = json!({
                "label": interner.resolve(f.name),
                "kind": 5,
                "detail": format_type(typed, interner, f.ty),
            });
            let field_doc = interner.resolve(f.docs);
            if !field_doc.is_empty() {
                item["documentation"] = json!({
                    "kind": "markdown",
                    "value": field_doc
                });
            }
            items.push(item);
        }
    }
    if let TypeKind::Sum {
        name: sum_name,
        variants,
        ..
    } = typed.types.get_type(receiver)
    {
        let owner_name = interner.resolve(*sum_name);
        for v in variants {
            let vname = interner.resolve(v.name);
            let detail = if v.fields.is_empty() {
                format!("{owner_name}.{vname}")
            } else {
                let payload: Vec<_> = v
                    .fields
                    .iter()
                    .map(|(n, ty)| {
                        format!(
                            "{}: {}",
                            interner.resolve(*n),
                            format_type(typed, interner, *ty)
                        )
                    })
                    .collect();
                format!("{owner_name}.{vname}({})", payload.join(", "))
            };
            let mut item = json!({
                "label": vname,
                "kind": 20, // CompletionItemKind.EnumMember
                "detail": detail,
                "filterText": format!("{owner_name}.{vname}")
            });
            if let Some(doc) = markdown_doc(interner.resolve(v.docs)) {
                item["documentation"] = doc;
            }
            items.push(item);
        }
    }
}

fn markdown_doc(docs: &str) -> Option<Value> {
    let docs = docs.trim();
    if docs.is_empty() {
        return None;
    }
    Some(json!({
        "kind": "markdown",
        "value": docs
    }))
}

fn md_hover(body: &str, note: &str) -> String {
    let mut out = format!("```super\n{body}\n```");
    if !note.is_empty() {
        out.push('\n');
        out.push_str(note);
    }
    out
}

/// Signature in a fence, then the documentation registered on the declaration.
/// A generic note is only the lead text when nothing was documented.
fn card_with_doc(body: &str, note: &str, docs: &str) -> String {
    let docs = docs.trim();
    if docs.is_empty() {
        return md_hover(body, note);
    }
    let mut card = md_hover(body, "");
    append_doc(&mut card, docs);
    if !note.is_empty() && note != "pure value return" {
        card.push_str("\n\n");
        card.push_str(note);
    }
    card
}

fn format_symbol_card(
    result: &AnalyzeDoc,
    text: &str,
    file: &SourceFile,
    symbol: &SymbolInfo,
) -> String {
    let name = result.interner.resolve(symbol.name);
    let prefix = if symbol.mutable { "@" } else { "" };
    let body = format!(
        "{prefix}{name}: {}",
        format_type(&result.typed, &result.interner, symbol.ty)
    );
    let kind = match symbol.kind {
        SymbolKind::Local => "local",
        SymbolKind::Param => "parameter",
        SymbolKind::Binding => "module binding",
        SymbolKind::Import => "import",
        SymbolKind::LoopBinder => "loop binder",
        SymbolKind::Const => "compile-time constant",
    };
    let storage = match symbol.storage {
        StorageClass::Register => "register",
        StorageClass::StackLocal => "stack local",
        StorageClass::StackArray => "stack array",
        StorageClass::ConstExpr => "comptime",
        StorageClass::Param => "parameter ABI",
    };
    let mut notes = vec![
        kind.to_owned(),
        if symbol.mutable {
            "mutable (`@`)".to_owned()
        } else {
            "immutable".to_owned()
        },
        storage.to_owned(),
    ];
    if let Some(value) = result.meta_cache.bindings.get(&symbol.name) {
        notes.push(format!("value `{}`", format_ct_value(value)));
    }
    let module = uri_to_path(&file.path)
        .and_then(|path| {
            path.file_stem()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| file.path.clone());
    notes.push(format!("module `{module}`"));
    let docs = stored_symbol_doc(result, symbol).unwrap_or_else(|| {
        if symbol.def_span.hi.as_usize() <= text.len() {
            doc_comments_before(text, symbol.def_span.lo.as_usize())
        } else {
            String::new()
        }
    });
    card_with_doc(&body, &notes.join(" · "), &docs)
}

fn append_doc(card: &mut String, docs: &str) {
    if docs.is_empty() {
        return;
    }
    card.push_str("\n\n");
    card.push_str(docs);
}

fn stored_symbol_doc(result: &AnalyzeDoc, symbol: &SymbolInfo) -> Option<String> {
    let name = symbol.name;
    if let Some(info) = result.typed.functions.get(&name) {
        let text = result.interner.resolve(info.docs);
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    let short = result.interner.resolve(name);
    for (key, info) in &result.typed.functions {
        let q = result.interner.resolve(*key);
        let Some((_, method)) = q.rsplit_once('.') else {
            continue;
        };
        if method != short || info.span != symbol.def_span {
            continue;
        }
        let text = result.interner.resolve(info.docs);
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    if let Some(docs) = result.typed.binding_docs.get(&name) {
        let text = result.interner.resolve(*docs);
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    if let Some(docs) = result.typed.type_docs.get(&name) {
        let text = result.interner.resolve(*docs);
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    if symbol.kind == SymbolKind::Import {
        let pos = symbol.def_span.lo;
        for target in &result.import_targets {
            if target.span.lo <= pos
                && pos < target.span.hi
                && let Some(docs) = result.module_docs.get(&target.path)
            {
                let text = result.interner.resolve(*docs);
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }
    None
}

/// Hovering a `///`, `//!`, `/**`, or `/*!` comment shows that comment as Markdown.
fn doc_comment_hover(text: &str, offset: usize) -> Option<String> {
    let offset = offset.min(text.len());
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[offset..]
        .find('\n')
        .map(|i| offset + i)
        .unwrap_or(text.len());
    let line = text[line_start..line_end].trim();
    if let Some(rest) = line.strip_prefix("///") {
        if rest.starts_with('/') {
            return None;
        }
        return Some(collect_line_docs(text, line_start, "///"));
    }
    if let Some(rest) = line.strip_prefix("//!") {
        let _ = rest;
        return Some(collect_line_docs(text, line_start, "//!"));
    }
    if text[..offset].rfind("/*").is_some_and(|start| {
        let head = text.get(start..start + 3).unwrap_or("");
        (head == "/**" || head == "/*!")
            && text[start + 2..offset].find("*/").is_none()
            && !text[start..].starts_with("/***")
            && !text[start..].starts_with("/**/")
    }) {
        let start = text[..offset].rfind("/*")?;
        let end = text[start..].find("*/").map(|i| start + i)?;
        let body = text[start + 3..end].trim();
        let cleaned = body
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix('*')
                    .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
                    .unwrap_or(trimmed)
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Some(cleaned.trim().to_string());
    }
    None
}

fn collect_line_docs(text: &str, from_line: usize, marker: &str) -> String {
    let mut start = from_line;
    while start > 0 {
        let prev_end = start - 1;
        let prev_start = text[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let prev = text[prev_start..prev_end].trim();
        if prev.starts_with(marker) && (marker != "///" || !prev.starts_with("////")) {
            start = prev_start;
        } else {
            break;
        }
    }
    let mut lines = Vec::new();
    let mut cursor = start;
    while cursor < text.len() {
        let end = text[cursor..]
            .find('\n')
            .map(|i| cursor + i)
            .unwrap_or(text.len());
        let line = text[cursor..end].trim();
        if let Some(rest) = line.strip_prefix(marker) {
            if marker == "///" && rest.starts_with('/') {
                break;
            }
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            cursor = if end < text.len() { end + 1 } else { end };
        } else {
            break;
        }
    }
    lines.join("\n")
}

fn doc_comments_before(text: &str, offset: usize) -> String {
    let prefix = &text[..offset.min(text.len())];
    let mut docs = Vec::new();
    for line in prefix.lines().rev() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed.strip_prefix("///") {
            docs.push(doc.trim().to_owned());
        } else if trimmed.is_empty() && docs.is_empty() {
            continue;
        } else {
            break;
        }
    }
    docs.reverse();
    docs.join("\n")
}

fn keyword_hover(word: &str) -> Option<(&'static str, &'static str)> {
    Some(match word {
        "when" => (
            "when <expr> { … }",
            "pattern match on a value; arms bind `it`. Use `else =>` as the catch-all.",
        ),
        "each" => (
            "for <iter> { … }  |  for parallel <iter> { … }",
            "iterate with implicit `it` and `index`. Prefix the collection with `@` to mutate in place. `parallel` fans out across M:N workers.",
        ),
        "ensure" => (
            "ensure <cond> else { … }",
            "early-exit guard — runs the `else` block (usually `return` / `fail`) when the condition is false.",
        ),
        "or" => (
            "<T?>/<T!> or <default>  |  or err { … }",
            "unwrap an optional / fallible value. Prefer `or err { … }` when you need the error payload.",
        ),
        "err" => (
            "or err { … }",
            "binds the error string as `err` inside the handler block.",
        ),
        "else" => (
            "else => …  |  else { … }",
            "fallback arm for `when` / `if` / `ensure`.",
        ),
        "if" => (
            "if <cond> { … } else { … }",
            "expression-capable conditional; the last expression in a branch is the value.",
        ),
        "return" => (
            "return <expr>",
            "exit the current function with a value (or bare `return` for void).",
        ),
        "fail" => (
            "fail <msg>",
            "produce a `T!` error value — pair with `: T!` return types and `or`.",
        ),
        "panic" => (
            "panic  |  panic <msg>",
            "unrecoverable abort — prints the message and stops the process. Distinct from `fail` (which yields a `T!` error for `or`). Prefer `#todo` / `#unimplemented` for unfinished code.",
        ),
        "test" => (
            "test \"name\" { … }",
            "first-class unit/integration test. Sees private `-` fields; stripped by `super build --release`.",
        ),
        "trait" => (
            "+Name = trait { +method(self) : T }",
            "method contract. A type declares it inline: `+Toggle: Name = { … }` and writes each method there.",
        ),
        "use" => (
            "use std.<area>.<module>",
            "import a stdlib or sibling module (e.g. `use std.net.http`, `use std.core.str`).",
        ),
        "state" => (
            "state <name> = <expr>",
            "hot-reloadable root binding — survives JIT module reloads (game world, caches).",
        ),
        "scope" => (
            "scope { spawn …; … }",
            "structured concurrency: waits for every spawned fiber before the block exits.",
        ),
        "spawn" => (
            "spawn { … } : task[T]",
            "start a lightweight M:N fiber. `await` the handle to join.",
        ),
        "await" => (
            "await <task[T]>",
            "suspend the current fiber until the task completes; yields `T`.",
        ),
        "chan" => (
            "chan[T](cap)  |  ch.send(v) / ch.recv()",
            "bounded lock-free channel between fibers.",
        ),
        "shared" => (
            "shared <expr>  ·  .load() / .store(v)",
            "atomic / futex cell safe across spawned tasks.",
        ),
        "after" => (
            "when { after <duration> => … }",
            "timer arm — fires once the duration elapses inside a `when` select.",
        ),
        "parallel" => (
            "for parallel <iter> { … }",
            "data-parallel `for` over M:N workers with noalias SIMD when `@` is present.",
        ),
        "in" => (
            "for <x> in <iter>  |  #for i in 0..N",
            "iteration binder for `for` / comptime `#for`.",
        ),
        "get" | "post" | "put" | "delete" => (
            "#get(\"/path\")  ·  #post(\"/path\")  ·  #put  ·  #delete",
            "Soft keyword leftover from the retired route DSL. Use a comptime route attribute on a normal function or lambda: `#get(\"/tasks\") handle(req) { … }`.",
        ),
        "rewrite" => (
            "rewrite <pattern> => <template>",
            "algebraic AST rewrite rule applied during `#` metaprogramming.",
        ),
        "embed" => (
            "embed \"path\"",
            "embed a file as a compile-time string / byte constant.",
        ),
        "true" | "false" => ("bool", "boolean literal"),
        "none" => (
            "none",
            "empty optional (`T?`). Unwrap with `or` / `or err`.",
        ),
        "it" => (
            "it",
            "implicit element binder inside `for` / `when` / pipeline closures.",
        ),
        "index" => ("index", "implicit 0-based index binder inside `for` loops."),
        "type" => (
            "T: type",
            "comptime type meta-parameter — inspect with `.fields` / `.size` / `#goal_type`.",
        ),
        "Code" => (
            "Code[T]",
            "quoted AST value from `'{ … }`; splice back with `${ … }`.",
        ),
        _ => return None,
    })
}

fn builtin_type_hover(word: &str) -> Option<String> {
    let (body, note) = match word {
        "i64" => ("i64", "64-bit signed integer · register / stack scalar"),
        "u8" => ("u8", "8-bit unsigned integer"),
        "u16" => ("u16", "16-bit unsigned integer"),
        "u32" => ("u32", "32-bit unsigned integer"),
        "u64" => ("u64", "64-bit unsigned integer"),
        "i8" => ("i8", "8-bit signed integer"),
        "i16" => ("i16", "16-bit signed integer"),
        "i32" => ("i32", "32-bit signed integer"),
        "i128" => ("i128", "128-bit signed integer"),
        "f32" => ("f32", "32-bit IEEE float · SIMD-friendly"),
        "f64" => (
            "f64",
            "64-bit IEEE float · packed as `mulpd`/`addpd` in JIT loops",
        ),
        "bool" => ("bool", "boolean · 1 byte C-ABI"),
        "str" => ("str", "fat pointer `{ptr, len}` · 16 B C-ABI (`SuperStr`)"),
        "void" => ("void", "no value"),
        "duration" => ("duration", "nanosecond duration · literals: `16.ms`, `1.s`"),
        "_" => ("_", "inferred type — filled from goal type / use site"),
        _ => return None,
    };
    Some(md_hover(body, note))
}

fn sigil_hover(text: &str, offset: usize) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let i = offset.min(bytes.len().saturating_sub(1));
    let b = bytes[i];
    let (body, note) = match b {
        b'@' => (
            "@name  |  @expr.method(…)",
            "in-place mutation / mutable reference. Required to write fields or pass `inout` params.",
        ),
        b'+' if !bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => (
            "+Name / +fn",
            "public export. Top-level items are private unless prefixed with `+`.",
        ),
        b'-' if !bytes.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => (
            "-field",
            "file-private field — visible inside this module and `test` blocks only.",
        ),
        b'#' => {
            // Prefer the specific directive when `#get` / `#todo` / … follows.
            let after = i + 1;
            if after < bytes.len()
                && is_ident_byte(bytes[after])
                && let Some(name) = ident_at(text, after)
            {
                let bare = name.trim_start_matches('#');
                if let Some(card) = hash_macro_hover(bare) {
                    return Some(card);
                }
            }
            (
                "#attr  |  #name = …  |  #{ … }  |  #if / #for",
                "comptime attribute, binding, or block. Route attrs (`#get`/`#post`), diagnostics (`#todo`), and metaprogramming — erased or applied at compile time.",
            )
        }
        b'?' => (
            "T?",
            "zero-cost optional. Must unwrap with `or` / `or err` before use as `T`.",
        ),
        b'!' if !matches!(bytes.get(i + 1), Some(b'=')) => (
            "T!",
            "fallible value / error channel. Unwrap with `or` / `or err`; produce with `fail`.",
        ),
        b'.' if matches!(bytes.get(i + 1), Some(b'{')) => (
            ".{ field, … }",
            "inferred struct literal — fills the goal type (return / param / annotated binding).",
        ),
        _ => return None,
    };
    Some(md_hover(body, note))
}

fn label_hover(
    text: &str,
    offset: usize,
    result: &AnalyzeDoc,
    word: Option<&str>,
) -> Option<String> {
    let name = word?;
    let bare = name.trim_start_matches(['+', '-', '@']);
    let bytes = text.as_bytes();
    let mut i = offset.min(bytes.len().saturating_sub(1));
    // Walk to end of ident, then skip whitespace — a following `:` means label.
    while i < bytes.len() && is_ident_byte(bytes[i]) {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    let typed = &result.typed;
    let interner = &result.interner;
    if let Some(sym) = interner.lookup(bare) {
        if let Some(info) = typed.bindings.get(&sym) {
            return Some(format_binding_card(typed, interner, bare, info));
        }
        let mut hits = Vec::new();
        for (sname, tid) in &typed.structs {
            if let TypeKind::Struct { fields, .. } = typed.types.get_type(*tid)
                && let Some(f) = fields.iter().find(|f| f.name == sym)
            {
                hits.push((*sname, *tid, f));
            }
        }
        let pos = BytePos(offset as u32);
        if let Some(recv) = member_receiver_type(typed, interner, text, pos) {
            hits.retain(|(owner, tid, _)| {
                *tid == recv || type_accepts_owner(typed, interner, *owner, recv)
            });
        }
        if hits.len() == 1 {
            let (sname, _, f) = hits[0];
            let vis = if f.private { "-" } else { "+" };
            let body = format!(
                "{}: {}   // {}.{}{}",
                bare,
                format_type(typed, interner, f.ty),
                interner.resolve(sname),
                vis,
                bare
            );
            return Some(card_with_doc(
                &body,
                &format!("struct field label · C-ABI offset {}", f.offset),
                interner.resolve(f.docs),
            ));
        }
        if hits.len() > 1 {
            let mut body = format!("field `{bare}`:");
            for (sname, _, f) in hits.iter().take(6) {
                body.push_str(&format!(
                    "\n  {}.{}: {}",
                    interner.resolve(*sname),
                    bare,
                    format_type(typed, interner, f.ty)
                ));
                let docs = interner.resolve(f.docs);
                if let Some(line) = docs.lines().find(|line| !line.trim().is_empty()) {
                    body.push_str(&format!("\n    {line}"));
                }
            }
            return Some(md_hover(&body, ""));
        }
    }
    Some(md_hover(
        &format!("{bare}: …"),
        "label — typed binding, struct field, or unquoted map key",
    ))
}

fn contextual_fallback(text: &str, offset: usize) -> String {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return md_hover("(", "expression / grouping");
    }
    let i = offset.min(bytes.len().saturating_sub(1));
    match bytes[i] {
        b'{' => md_hover("{ … }", "block, struct, map, or sum literal"),
        b'}' => md_hover("}", "end of block / literal"),
        b'[' => md_hover("[ … ]", "array literal or index / slice"),
        b']' => md_hover("]", "end of array / index"),
        b'(' => md_hover("( … )", "grouping or call arguments"),
        b')' => md_hover(")", "end of grouping / call"),
        b',' => md_hover(",", "argument / field / element separator"),
        b';' => md_hover(
            "=",
            "binding or assignment (use `@` for mutation of existing values)",
        ),
        b':' => md_hover("name: Type", "type ascription or label separator"),
        b'"' => md_hover("\"…\"", "string literal — interpolate with `{expr}`"),
        b'/' if matches!(bytes.get(i + 1), Some(b'/')) => md_hover(
            "// comment",
            "line comment — `//#` directives are live metaprogramming",
        ),
        c if c.is_ascii_digit() => md_hover("<number>", "numeric literal (`i64` / `f64` inferred)"),
        _ => md_hover(
            "<expr>",
            "move the cursor onto a name, type, keyword, or `@`/`+`/`#` sigil for details",
        ),
    }
}

fn member_hover(
    typed: &TypedModule,
    interner: &Interner,
    bare: &str,
    text: &str,
    pos: BytePos,
) -> Option<String> {
    if let Some(card) = find_method_card(typed, interner, bare, text, pos) {
        return Some(card);
    }
    let mut fields = field_hits(typed, interner, bare);
    if let Some(recv) = member_receiver_type(typed, interner, text, pos) {
        fields.retain(|(sname, ..)| {
            interner
                .lookup(sname)
                .is_some_and(|owner| type_accepts_owner(typed, interner, owner, recv))
        });
    }
    if let Some(card) = render_field_hits(bare, &fields) {
        return Some(card);
    }
    variant_card(typed, interner, bare, text, pos)
}

/// `(struct, field, offset, private, type, size, align, docs)` for hover.
type FieldHit = (String, String, u32, bool, String, u32, u32, String);

fn field_hits(typed: &TypedModule, interner: &Interner, bare: &str) -> Vec<FieldHit> {
    let Some(sym) = interner.lookup(bare) else {
        return Vec::new();
    };
    let mut field_hits = Vec::new();
    for (sname, tid) in &typed.structs {
        if let TypeKind::Struct {
            fields,
            size,
            align,
            ..
        } = typed.types.get_type(*tid)
            && let Some(f) = fields.iter().find(|f| f.name == sym)
        {
            field_hits.push((
                interner.resolve(*sname).to_string(),
                interner.resolve(f.name).to_string(),
                f.offset,
                f.private,
                format_type(typed, interner, f.ty),
                *size,
                *align,
                interner.resolve(f.docs).to_string(),
            ));
        }
    }
    field_hits
}

fn render_field_hits(bare: &str, field_hits: &[FieldHit]) -> Option<String> {
    if field_hits.len() == 1 {
        let (sname, fname, off, private, ty, size, align, docs) = &field_hits[0];
        let vis = if *private { "-" } else { "+" };
        let body = format!("{sname}.{vis}{fname}: {ty}");
        return Some(card_with_doc(
            &body,
            &format!("field @ byte offset {off} · parent `{sname}` is {size} B / align {align}"),
            docs,
        ));
    }
    if field_hits.len() > 1 {
        let mut body = format!("field `{bare}` appears on:");
        for (sname, fname, off, private, ty, _, _, docs) in field_hits.iter().take(6) {
            let vis = if *private { "-" } else { "+" };
            body.push_str(&format!("\n  {sname}.{vis}{fname}: {ty}  //@+{off}"));
            if let Some(line) = docs.lines().find(|line| !line.trim().is_empty()) {
                body.push_str(&format!("\n    {line}"));
            }
        }
        if field_hits.len() > 6 {
            body.push_str("\n  …");
        }
        let note = if field_hits.iter().any(|hit| !hit.7.trim().is_empty()) {
            ""
        } else {
            "ambiguous field name — qualify with the parent type"
        };
        return Some(md_hover(&body, note));
    }
    None
}

fn variant_card(
    typed: &TypedModule,
    interner: &Interner,
    bare: &str,
    text: &str,
    pos: BytePos,
) -> Option<String> {
    let sym = interner.lookup(bare)?;
    let mut hits = Vec::new();
    for (sname, tid) in &typed.sums {
        if let TypeKind::Sum { variants, .. } = typed.types.get_type(*tid)
            && let Some(v) = variants.iter().find(|v| v.name == sym)
        {
            hits.push((*sname, v));
        }
    }
    if hits.is_empty() {
        return None;
    }
    if let Some(recv) = member_receiver_type(typed, interner, text, pos) {
        let matched: Vec<_> = hits
            .into_iter()
            .filter(|(owner, _)| type_accepts_owner(typed, interner, *owner, recv))
            .collect();
        if matched.is_empty() {
            return None;
        }
        hits = matched;
    }
    if hits.len() == 1 {
        let (owner, variant) = hits[0];
        let owner_name = interner.resolve(owner);
        let vname = interner.resolve(variant.name);
        let body = if variant.fields.is_empty() {
            format!("{owner_name}.{vname}")
        } else {
            let payload: Vec<_> = variant
                .fields
                .iter()
                .map(|(n, t)| {
                    format!(
                        "{}: {}",
                        interner.resolve(*n),
                        format_type(typed, interner, *t)
                    )
                })
                .collect();
            format!("{owner_name}.{vname}({})", payload.join(", "))
        };
        return Some(card_with_doc(
            &body,
            &format!("variant of `{owner_name}`"),
            interner.resolve(variant.docs),
        ));
    }
    let mut body = format!("variant `{bare}` appears on:");
    for (owner, variant) in hits.iter().take(6) {
        body.push_str(&format!(
            "\n  {}.{}",
            interner.resolve(*owner),
            interner.resolve(variant.name)
        ));
        let docs = interner.resolve(variant.docs);
        if let Some(line) = docs.lines().find(|line| !line.trim().is_empty()) {
            body.push_str(&format!("\n    {line}"));
        }
    }
    Some(md_hover(&body, ""))
}

fn find_method_card(
    typed: &TypedModule,
    interner: &Interner,
    bare: &str,
    text: &str,
    pos: BytePos,
) -> Option<String> {
    let mut hits = Vec::new();
    for (key, info) in &typed.functions {
        let q = interner.resolve(*key);
        if let Some((owner, method)) = q.rsplit_once('.')
            && method == bare
        {
            hits.push((owner.to_string(), info));
        }
    }
    if hits.is_empty() {
        return None;
    }
    if let Some(recv) = member_receiver_type(typed, interner, text, pos) {
        let matched: Vec<_> = hits
            .into_iter()
            .filter(|(_, info)| {
                info.owner
                    .is_some_and(|owner| type_accepts_owner(typed, interner, owner, recv))
            })
            .collect();
        if matched.is_empty() {
            return None;
        }
        hits = matched;
    }
    let defined_here: Vec<_> = hits
        .iter()
        .filter(|(_, info)| span_contains(info.span, pos) && info.span.hi.as_usize() <= text.len())
        .map(|(owner, info)| (owner.clone(), *info))
        .collect();
    if defined_here.len() == 1 {
        hits = defined_here;
    }
    if hits.len() == 1 {
        let (owner, info) = &hits[0];
        return Some(format_fn_card_named(
            typed,
            interner,
            &format!("{owner}.{bare}"),
            info,
        ));
    }
    let mut body = format!("method `{bare}`");
    for (owner, info) in hits.iter().take(8) {
        let sig = format_fn_signature(typed, interner, bare, info);
        body.push_str(&format!("\n  {owner}.{sig}"));
        let docs = interner.resolve(info.docs);
        if let Some(line) = docs.lines().find(|line| !line.trim().is_empty()) {
            body.push_str(&format!("\n    {line}"));
        }
    }
    if hits.len() > 8 {
        body.push_str("\n  …");
    }
    let note = if hits
        .iter()
        .any(|(_, info)| !interner.resolve(info.docs).trim().is_empty())
    {
        ""
    } else {
        "call as `receiver.method(…)` or `Type.method(…)`"
    };
    Some(md_hover(&body, note))
}

fn format_typed_name(typed: &TypedModule, interner: &Interner, name: &str, ty: TypeId) -> String {
    // Prefer rich cards for named aggregates instead of a generic C-ABI blurb.
    match typed.types.get_type(ty) {
        TypeKind::Struct { name: sn, .. } => {
            let n = interner.resolve(*sn);
            if !n.is_empty() && n != "_" {
                return format_struct_card(typed, interner, n, ty);
            }
        }
        TypeKind::Sum { name: sn, .. } => {
            let n = interner.resolve(*sn);
            if !n.is_empty() {
                return format_sum_card(typed, interner, n, ty);
            }
        }
        TypeKind::Named(sn) => {
            if let Some(tid) = typed.structs.get(sn) {
                return format_struct_card(typed, interner, interner.resolve(*sn), *tid);
            }
            if let Some(tid) = typed.sums.get(sn) {
                return format_sum_card(typed, interner, interner.resolve(*sn), *tid);
            }
        }
        TypeKind::Func { .. } => {
            let body = format!("{name}: {}", format_type(typed, interner, ty));
            return md_hover(&body, "function type");
        }
        _ => {}
    }
    let body = format!("{name}: {}", format_type(typed, interner, ty));
    md_hover(&body, &type_note(typed, interner, ty))
}

/// Hover card for a `#name` comptime attribute or builtin macro.
fn hash_macro_hover(bare: &str) -> Option<String> {
    let (syntax, note) = match bare {
        "get" => (
            "#get(\"/path\")",
            "Comptime HTTP route attribute. Attaches a GET handler to the next function or lambda binding and registers it in the route table at compile time — not a method.",
        ),
        "post" => (
            "#post(\"/path\")",
            "Comptime HTTP route attribute. Attaches a POST handler to the next function or lambda binding.",
        ),
        "put" => (
            "#put(\"/path\")",
            "Comptime HTTP route attribute. Attaches a PUT handler to the next function or lambda binding.",
        ),
        "delete" => (
            "#delete(\"/path\")",
            "Comptime HTTP route attribute. Attaches a DELETE handler to the next function or lambda binding.",
        ),
        "route" => (
            "#route(\"METHOD\", \"/path\")",
            "Comptime HTTP route attribute for an arbitrary method. Prefer `#get`/`#post`/`#put`/`#delete` when the method is fixed.",
        ),
        "inline" => (
            "#inline",
            "Comptime codegen attribute — prefer inlining the next function.",
        ),
        "simd" => (
            "#simd",
            "Comptime codegen attribute — request SIMD-friendly lowering of the next loop.",
        ),
        "unroll" => (
            "#unroll(N)",
            "Comptime codegen attribute — unroll the next `for` loop by a fixed factor `N`.",
        ),
        "blocking" => (
            "#blocking",
            "Comptime attribute — mark the next host call as blocking for the scheduler.",
        ),
        "inspect" => (
            "#inspect(T)",
            "Comptime diagnostic — print the type / AST of `T` during expansion.",
        ),
        "assert_size" => (
            "#assert_size(T, N)",
            "Comptime check — assert that type `T` has C-ABI size `N` bytes.",
        ),
        "law" => (
            "#law(\"description\") name(params) { … }",
            "Human-locked behavioral invariant. Discharged by `super verify` (SMT / comptime / fuzz). Completely stripped from C23, JIT, and bytecode — zero runtime overhead. Prefer `*.law.sr` for AI-safe file separation.",
        ),
        "proof" => (
            "#proof(law_ident) name(params) { … }",
            "AI-authored lemma / witness for a `#law`. May contain equational `#assert`s or induction hints. Stripped from all runtime backends. Incomplete proofs (`#todo`) yield E0503.",
        ),
        "system" => (
            "#system(stage)",
            "Comptime ECS/system stage registration attribute.",
        ),
        "todo" => (
            "#todo  |  #todo(\"message\")",
            "Builtin macro — expands to `panic` with a not-yet-implemented message. Always available; no `use` required.",
        ),
        "unimplemented" => (
            "#unimplemented  |  #unimplemented(\"message\")",
            "Builtin macro — expands to `panic` marking a deliberate unfinished path. Always available.",
        ),
        "unreachable" => (
            "#unreachable  |  #unreachable(\"message\")",
            "Builtin macro — expands to `panic` for a branch that must never run. Always available.",
        ),
        "assert" => (
            "#assert(cond)  |  #assert(cond, \"message\")",
            "Builtin macro — panics when `cond` is false. Always available.",
        ),
        "dbg" => (
            "#dbg(expr)",
            "Builtin macro — prints `expr` with its value and yields `expr`. Always available.",
        ),
        "if" => (
            "#if <cond> { … } else { … }",
            "Comptime conditional — only the taken branch is emitted into the runtime program.",
        ),
        "for" | "each" => (
            "#for i in 0..N { … }",
            "Comptime unroll loop — expands to N copies of the body at compile time.",
        ),
        _ => return None,
    };
    Some(md_hover(
        syntax,
        &format!("comptime macro / attribute · {note}"),
    ))
}

/// True when `word` is a `#…` form or the identifier is immediately preceded by `#`.
fn hash_macro_hover_at(text: &str, offset: usize, word: Option<&str>) -> Option<String> {
    if let Some(w) = word {
        if w.starts_with('#') {
            let bare = w.trim_start_matches('#');
            if let Some(card) = hash_macro_hover(bare) {
                return Some(card);
            }
            return Some(md_hover(
                &format!("#{bare}"),
                "comptime attribute or macro — not a runtime method or binding",
            ));
        }
        // `#` is an ident byte, so `#get` is usually one token; still guard the
        // rare case where the cursor sits on `get` after a lone `#`.
        if let Some(start) = word_start(text, offset)
            && start > 0
            && text.as_bytes()[start - 1] == b'#'
        {
            if let Some(card) = hash_macro_hover(w.trim_start_matches(['+', '-', '@'])) {
                return Some(card);
            }
            return Some(md_hover(
                &format!("#{w}"),
                "comptime attribute or macro — not a runtime method or binding",
            ));
        }
    } else if text.as_bytes().get(offset).copied() == Some(b'#') {
        return sigil_hover(text, offset);
    }
    None
}

fn word_start(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut i = offset.min(bytes.len().saturating_sub(1));
    if !is_ident_byte(bytes[i]) {
        if i > 0 && is_ident_byte(bytes[i - 1]) {
            i -= 1;
        } else {
            return None;
        }
    }
    while i > 0 && is_ident_byte(bytes[i - 1]) {
        i -= 1;
    }
    Some(i)
}

fn comptime_hover(
    result: &AnalyzeDoc,
    file: &SourceFile,
    pos: BytePos,
    text: &str,
    word: Option<&str>,
) -> Option<String> {
    if let Some(w) = word
        && w.starts_with('#')
        && let Some(sym) = result.interner.lookup(w.trim_start_matches('#'))
        && let Some(val) = result.meta_cache.bindings.get(&sym)
    {
        return Some(md_hover(
            &format!("{w} = {}", format_ct_value(val)),
            "compile-time constant",
        ));
    }
    let key = hover_span_key(super_core::Span::from_offsets(
        pos.0,
        pos.0.saturating_add(1),
    ));
    if let Some(v) = result.meta_cache.expansions.get(&key) {
        return Some(md_hover(v, "comptime expansion"));
    }
    if (text.as_bytes().get(pos.as_usize()).copied() == Some(b'$')
        || text
            .get(pos.as_usize().saturating_sub(1)..=pos.as_usize())
            .is_some_and(|s| s.contains("${")))
        && let Some(ty) = best_expr_type(&result.typed, pos)
    {
        return Some(md_hover(
            &format_type(&result.typed, &result.interner, ty),
            "spliced comptime expression",
        ));
    }
    let line = file.line_text(file.line_col(pos).0)?;
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("//#") {
        let token = trimmed
            .trim_start_matches("//#")
            .trim_start_matches('#')
            .split_whitespace()
            .next()?;
        let sym = result.interner.lookup(token)?;
        if let Some(val) = result.meta_cache.bindings.get(&sym) {
            return Some(md_hover(
                &format!("#{token} = {}", format_ct_value(val)),
                "compile-time constant",
            ));
        }
    }
    None
}

fn hover_span_key(span: super_core::Span) -> u64 {
    ((span.lo.0 as u64) << 32) | span.hi.0 as u64
}

fn format_ct_value(v: &CtValue) -> String {
    match v {
        CtValue::Null => "none".into(),
        CtValue::Bool(b) => b.to_string(),
        CtValue::Int(n) => n.to_string(),
        CtValue::Float(f) => f.to_string(),
        CtValue::Str(s) => format!("\"{s}\""),
        CtValue::Array(xs) => {
            let inner: Vec<_> = xs.iter().map(format_ct_value).collect();
            format!("[{}]", inner.join(", "))
        }
        CtValue::Code(_) => "Code[…]".into(),
        CtValue::TypeRef(_) => "type …".into(),
    }
}

fn ident_at(text: &str, offset: usize) -> Option<&str> {
    let (lo, hi) = ident_span_at(text, offset)?;
    let s = &text[lo..hi];
    if s.is_empty() { None } else { Some(s) }
}

fn ident_span_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }
    let bytes = text.as_bytes();
    let mut i = offset.min(bytes.len().saturating_sub(1));
    if !is_ident_byte(bytes[i]) {
        if i > 0 && is_ident_byte(bytes[i - 1]) {
            i -= 1;
        } else {
            return None;
        }
    }
    let mut lo = i;
    while lo > 0 && is_ident_byte(bytes[lo - 1]) {
        lo -= 1;
    }
    let mut hi = i + 1;
    while hi < bytes.len() && is_ident_byte(bytes[hi]) {
        hi += 1;
    }
    Some((lo, hi))
}

/// True when `offset` sits immediately after a `.` accessor (ignoring whitespace).
fn preceded_by_dot(text: &str, offset: usize) -> bool {
    let bytes = text.as_bytes();
    let mut i = offset;
    while i > 0 && bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    i > 0 && bytes[i - 1] == b'.'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'+' || b == b'-' || b == b'#'
}

fn best_expr_type(typed: &TypedModule, pos: BytePos) -> Option<TypeId> {
    let mut best: Option<(u32, TypeId)> = None;
    for (key, tid) in &typed.expr_types {
        let lo = (*key >> 32) as u32;
        let hi = (*key & 0xffff_ffff) as u32;
        if lo <= pos.0 && pos.0 < hi.max(lo + 1) {
            let len = hi.saturating_sub(lo);
            if best.map(|(l, _)| len < l).unwrap_or(true) {
                best = Some((len, *tid));
            }
        }
    }
    best.map(|(_, t)| t)
}

fn core_trait_hover(name: &str) -> Option<String> {
    if let Some(tr) = super_core::traits::core_trait(name) {
        let body = if tr.methods.is_empty() {
            format!("+{name} = trait {{}}")
        } else {
            let lines: Vec<String> = tr
                .methods
                .iter()
                .map(|method| {
                    let receiver = if method.mutable_self {
                        "@self"
                    } else if method.has_self {
                        "self"
                    } else if method.params == 1 {
                        "value"
                    } else {
                        ""
                    };
                    let extra = if method.has_self && method.params > 1 {
                        ", other"
                    } else {
                        ""
                    };
                    let ret = method
                        .ret
                        .map(|ret| format!(" : {ret}"))
                        .unwrap_or_default();
                    format!("{0}({receiver}{extra}){ret}", method.name)
                })
                .collect();
            format_trait_card(name, &lines)
        };
        return Some(if tr.methods.is_empty() {
            card_with_doc(&body, "language trait · marker · ambient", "")
        } else {
            body
        });
    }
    let builtin = super_core::traits::builtin_traits(name);
    if builtin.is_empty() {
        return None;
    }
    Some(md_hover(
        &format!("{name}: {}", builtin.join(" + ")),
        "builtin trait witness · same algorithm on the VM and the C23 backend",
    ))
}

fn format_trait_card(name: &str, lines: &[String]) -> String {
    let mut body = format!("+{name} = trait {{");
    for line in lines.iter().take(12) {
        body.push_str("\n  +");
        body.push_str(line);
    }
    if lines.len() > 12 {
        body.push_str(&format!("\n  // … {} more methods", lines.len() - 12));
    }
    body.push_str("\n}");
    card_with_doc(
        &body,
        "method contract · language traits are ambient · static call when declared",
        "",
    )
}

fn format_struct_card(typed: &TypedModule, interner: &Interner, name: &str, tid: TypeId) -> String {
    let declared = interner.lookup(name).and_then(|sym| typed.impls.get(&sym));
    let trait_list = declared.map(|names| {
        names
            .iter()
            .map(|name| interner.resolve(*name))
            .collect::<Vec<_>>()
            .join(", ")
    });
    match typed.types.get_type(tid) {
        TypeKind::Struct {
            size,
            align,
            fields,
            ..
        } => {
            let header = match &trait_list {
                Some(list) => format!("+{name}: {list} = {{"),
                None => format!("+{name} = {{"),
            };
            let mut body = header;
            let show = fields.len().min(12);
            for f in fields.iter().take(show) {
                let vis = if f.private { "-" } else { "+" };
                body.push_str(&format!(
                    "\n  {vis}{}: {}  //@+{}",
                    interner.resolve(f.name),
                    format_type(typed, interner, f.ty),
                    f.offset
                ));
            }
            if fields.len() > show {
                body.push_str(&format!("\n  // … {} more fields", fields.len() - show));
            }
            body.push_str("\n}");
            let mut footer = format!(
                "{} field{} · {size} B · align {align} · flat C-ABI / stack",
                fields.len(),
                if fields.len() == 1 { "" } else { "s" }
            );
            if let Some(list) = &trait_list {
                footer.push_str(" · ");
                footer.push_str(list);
            }
            card_with_doc(&body, &footer, &named_doc(interner, name, &typed.type_docs))
        }
        _ => card_with_doc(
            &format!("{name}: struct"),
            "named aggregate",
            &named_doc(interner, name, &typed.type_docs),
        ),
    }
}

fn format_sum_card(typed: &TypedModule, interner: &Interner, name: &str, tid: TypeId) -> String {
    match typed.types.get_type(tid) {
        TypeKind::Sum { size, variants, .. } => {
            let mut body = format!("{name} = {{");
            let show = variants.len().min(12);
            for v in variants.iter().take(show) {
                if v.fields.is_empty() {
                    body.push_str(&format!("\n  {}", interner.resolve(v.name)));
                } else {
                    let payload: Vec<_> = v
                        .fields
                        .iter()
                        .map(|(n, t)| {
                            format!(
                                "{}: {}",
                                interner.resolve(*n),
                                format_type(typed, interner, *t)
                            )
                        })
                        .collect();
                    body.push_str(&format!(
                        "\n  {}({})",
                        interner.resolve(v.name),
                        payload.join(", ")
                    ));
                }
            }
            if variants.len() > show {
                body.push_str(&format!("\n  // … {} more variants", variants.len() - show));
            }
            body.push_str("\n}");
            card_with_doc(
                &body,
                &format!(
                    "{} variant{} · {size} B tagged union",
                    variants.len(),
                    if variants.len() == 1 { "" } else { "s" }
                ),
                &named_doc(interner, name, &typed.type_docs),
            )
        }
        _ => card_with_doc(
            &format!("{name}: sum"),
            "tagged union",
            &named_doc(interner, name, &typed.type_docs),
        ),
    }
}

fn named_doc(
    interner: &Interner,
    name: &str,
    docs: &HashMap<super_core::SymId, super_core::SymId>,
) -> String {
    interner
        .lookup(name)
        .and_then(|sym| docs.get(&sym))
        .map(|doc| interner.resolve(*doc).to_string())
        .unwrap_or_default()
}

fn format_fn_signature(
    typed: &TypedModule,
    interner: &Interner,
    name: &str,
    info: &super_frontend::sema::FunctionInfo,
) -> String {
    let export = if info.exported { "+" } else { "" };
    let params: Vec<String> = info
        .params
        .iter()
        .map(|(n, t, mut_ref)| {
            let at = if *mut_ref { "@" } else { "" };
            format!(
                "{at}{}: {}",
                interner.resolve(*n),
                format_type(typed, interner, *t)
            )
        })
        .collect();
    let ret_id = match typed.types.get_type(info.ty) {
        TypeKind::Func { ret, .. } => *ret,
        _ => info.ty,
    };
    let ret = format_type(typed, interner, ret_id);
    format!("{export}{name}({}): {ret}", params.join(", "))
}

fn format_fn_card(
    typed: &TypedModule,
    interner: &Interner,
    name: &str,
    info: &super_frontend::sema::FunctionInfo,
) -> String {
    let display = if let Some(owner) = info.owner {
        format!("{}.{}", interner.resolve(owner), name)
    } else {
        name.to_string()
    };
    format_fn_card_named(typed, interner, &display, info)
}

fn format_fn_card_named(
    typed: &TypedModule,
    interner: &Interner,
    display_name: &str,
    info: &super_frontend::sema::FunctionInfo,
) -> String {
    let body = format_fn_signature(typed, interner, display_name, info);
    let ret_id = match typed.types.get_type(info.ty) {
        TypeKind::Func { ret, .. } => *ret,
        _ => info.ty,
    };
    let mut notes = Vec::new();
    if let Some(owner) = info.owner {
        notes.push(format!("method of `{}`", interner.resolve(owner)));
    }
    for (n, t, m) in &info.params {
        if *m {
            notes.push(format!(
                "mutates `{}: {}` in-place (`@`)",
                interner.resolve(*n),
                format_type(typed, interner, *t)
            ));
        }
    }
    match typed.types.get_type(ret_id) {
        TypeKind::Task(inner) => notes.push(format!(
            "returns fiber handle `task[{}]` — `await` to join",
            format_type(typed, interner, *inner)
        )),
        TypeKind::Optional(inner) => notes.push(format!(
            "returns optional `{}?` — unwrap with `or`",
            format_type(typed, interner, *inner)
        )),
        TypeKind::Error(inner) => notes.push(format!(
            "fallible `{}!` — unwrap with `or` / `or err`",
            format_type(typed, interner, *inner)
        )),
        TypeKind::Struct { name, size, .. } => {
            let n = interner.resolve(*name);
            if !n.is_empty() {
                notes.push(format!("returns `{n}` ({size} B flat C-ABI)"));
            }
        }
        _ => {}
    }
    let docs = interner.resolve(info.docs);
    let note = if notes.is_empty() && docs.trim().is_empty() {
        "pure value return"
    } else {
        ""
    };
    let joined = if note.is_empty() {
        notes.join(" · ")
    } else {
        note.to_string()
    };
    card_with_doc(&body, &joined, docs)
}

fn format_binding_card(
    typed: &TypedModule,
    interner: &Interner,
    name: &str,
    info: &super_frontend::sema::BindingInfo,
) -> String {
    // Prefer struct/sum cards when the binding's type is a named aggregate.
    match typed.types.get_type(info.ty) {
        TypeKind::Struct { name: sn, .. } => {
            let n = interner.resolve(*sn);
            if !n.is_empty() && n != "_" {
                let mut card = format_struct_card(typed, interner, n, info.ty);
                if info.mutable {
                    card.push_str("\n`@` mutable binding");
                } else {
                    card.push_str(&format!("\nbinding `{name}`"));
                }
                append_doc(&mut card, &named_doc(interner, name, &typed.binding_docs));
                return card;
            }
        }
        TypeKind::Sum { name: sn, .. } => {
            let n = interner.resolve(*sn);
            if !n.is_empty() {
                let mut card = format_sum_card(typed, interner, n, info.ty);
                card.push_str(&format!("\nbinding `{name}`"));
                append_doc(&mut card, &named_doc(interner, name, &typed.binding_docs));
                return card;
            }
        }
        TypeKind::Named(sn) => {
            if let Some(tid) = typed.structs.get(sn) {
                return format_binding_card(
                    typed,
                    interner,
                    name,
                    &super_frontend::sema::BindingInfo {
                        ty: *tid,
                        exported: info.exported,
                        mutable: info.mutable,
                        span: info.span,
                    },
                );
            }
        }
        _ => {}
    }
    let prefix = if info.mutable { "@" } else { "" };
    let export = if info.exported { "+" } else { "" };
    let body = format!(
        "{export}{prefix}{name}: {}",
        format_type(typed, interner, info.ty)
    );
    card_with_doc(
        &body,
        &binding_note(typed, interner, info.ty, info.mutable),
        &named_doc(interner, name, &typed.binding_docs),
    )
}

fn binding_note(typed: &TypedModule, interner: &Interner, tid: TypeId, mutable: bool) -> String {
    let base = type_note(typed, interner, tid);
    if mutable {
        if base.is_empty() {
            "mutable binding — writes require `@`".into()
        } else {
            format!("mutable · {base}")
        }
    } else if base.is_empty() {
        "immutable binding (default)".into()
    } else {
        base
    }
}

fn format_type(typed: &TypedModule, interner: &Interner, tid: TypeId) -> String {
    match typed.types.get_type(tid) {
        TypeKind::Void => "void".into(),
        TypeKind::Bool => "bool".into(),
        TypeKind::I64 => "i64".into(),
        TypeKind::I8 => "i8".into(),
        TypeKind::I16 => "i16".into(),
        TypeKind::I32 => "i32".into(),
        TypeKind::I128 => "i128".into(),
        TypeKind::U8 => "u8".into(),
        TypeKind::U16 => "u16".into(),
        TypeKind::U32 => "u32".into(),
        TypeKind::U64 => "u64".into(),
        TypeKind::F32 => "f32".into(),
        TypeKind::F64 => "f64".into(),
        TypeKind::Str => "str".into(),
        TypeKind::Optional(t) => format!("{}?", format_type(typed, interner, *t)),
        TypeKind::Error(t) => format!("{}!", format_type(typed, interner, *t)),
        TypeKind::Array(t) => format!("[{}]", format_type(typed, interner, *t)),
        TypeKind::Map { key, value } => format!(
            "{{{}: {}}}",
            format_type(typed, interner, *key),
            format_type(typed, interner, *value)
        ),
        TypeKind::Struct { name, fields, .. } => {
            let n = interner.resolve(*name);
            if n.is_empty() || n == "_" {
                // Anonymous / inferred record — show a short field sketch.
                let sketch: Vec<_> = fields
                    .iter()
                    .take(4)
                    .map(|f| {
                        format!(
                            "{}: {}",
                            interner.resolve(f.name),
                            format_type(typed, interner, f.ty)
                        )
                    })
                    .collect();
                if sketch.is_empty() {
                    "{…}".into()
                } else if fields.len() > 4 {
                    format!("{{{}, …}}", sketch.join(", "))
                } else {
                    format!("{{{}}}", sketch.join(", "))
                }
            } else {
                n.into()
            }
        }
        TypeKind::Sum { name, .. } => {
            let n = interner.resolve(*name);
            if n.is_empty() { "sum".into() } else { n.into() }
        }
        TypeKind::Func { params, ret } => {
            let ps: Vec<_> = params
                .iter()
                .map(|p| format_type(typed, interner, *p))
                .collect();
            format!(
                "({}) → {}",
                ps.join(", "),
                format_type(typed, interner, *ret)
            )
        }
        TypeKind::Named(n) => {
            if let Some(tid) = typed.structs.get(n).or_else(|| typed.sums.get(n)) {
                format_type(typed, interner, *tid)
            } else {
                interner.resolve(*n).into()
            }
        }
        TypeKind::Module { name } => format!("module {}", interner.resolve(*name)),
        TypeKind::Infer => "_".into(),
        TypeKind::Never => "never".into(),
        TypeKind::Chan(t) => format!("chan[{}]", format_type(typed, interner, *t)),
        TypeKind::Task(t) => format!("task[{}]", format_type(typed, interner, *t)),
        TypeKind::Shared(t) => format!("shared {}", format_type(typed, interner, *t)),
        TypeKind::Duration => "duration".into(),
        TypeKind::TypeMeta => "type".into(),
        TypeKind::Code(t) => format!("Code[{}]", format_type(typed, interner, *t)),
        TypeKind::TraitBound { name } => format!("trait {}", interner.resolve(*name)),
    }
}

fn type_note(typed: &TypedModule, interner: &Interner, tid: TypeId) -> String {
    match typed.types.get_type(tid) {
        TypeKind::Chan(t) => format!(
            "lock-free channel of `{}`",
            format_type(typed, interner, *t)
        ),
        TypeKind::Task(t) => format!(
            "fiber handle of `{}` — await to join",
            format_type(typed, interner, *t)
        ),
        TypeKind::Shared(t) => format!(
            "atomic / futex cell of `{}`",
            format_type(typed, interner, *t)
        ),
        TypeKind::Duration => "nanosecond duration (literals: `16.ms`, `1.s`)".into(),
        TypeKind::Code(t) => format!(
            "quoted AST `Code[{}]` (comptime)",
            format_type(typed, interner, *t)
        ),
        TypeKind::Struct {
            name,
            size,
            align,
            fields,
            ..
        } => {
            let n = interner.resolve(*name);
            if n.is_empty() || n == "_" {
                format!(
                    "anonymous record · {} field{} · {size} B / align {align}",
                    fields.len(),
                    if fields.len() == 1 { "" } else { "s" }
                )
            } else {
                format!("`{n}` · {size} B / align {align} · {} fields", fields.len())
            }
        }
        TypeKind::Sum {
            name,
            size,
            variants,
            ..
        } => {
            format!(
                "`{}` · {size} B tagged union · {} variants",
                interner.resolve(*name),
                variants.len()
            )
        }
        TypeKind::Optional(t) => format!(
            "optional `{}?` — unwrap with `or`",
            format_type(typed, interner, *t)
        ),
        TypeKind::Error(t) => format!(
            "fallible `{}!` — unwrap with `or` / `or err`",
            format_type(typed, interner, *t)
        ),
        TypeKind::Array(t) => format!("contiguous array of `{}`", format_type(typed, interner, *t)),
        TypeKind::Map { key, value } => format!(
            "map `{} → {}`",
            format_type(typed, interner, *key),
            format_type(typed, interner, *value)
        ),
        TypeKind::Module { name } => {
            format!("imported module `{}`", interner.resolve(*name))
        }
        TypeKind::Infer => "type not yet inferred — annotate or use a goal type".into(),
        TypeKind::Str => "fat string `{ptr, len}` (16 B)".into(),
        TypeKind::I8
        | TypeKind::I16
        | TypeKind::I32
        | TypeKind::I64
        | TypeKind::I128
        | TypeKind::U8
        | TypeKind::U16
        | TypeKind::U32
        | TypeKind::U64
        | TypeKind::F32
        | TypeKind::F64
        | TypeKind::Bool => "stack scalar".into(),
        _ => String::new(),
    }
}

/// `super lint` findings (default rules; Pod.toml policy applies on the CLI).
fn lint_diagnostics(uri: &str, text: &str) -> Vec<Value> {
    use super_frontend::lint::{LintContext, Severity as LintSeverity, lint_source};
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let Ok(findings) = lint_source(text, &LintContext::for_path(path)) else {
        return Vec::new();
    };
    findings
        .into_iter()
        .map(|f| {
            let line = f.line.saturating_sub(1);
            let character = f.column.saturating_sub(1);
            json!({
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": line, "character": character + 1 },
                },
                "severity": match f.severity {
                    LintSeverity::Error => 1,
                    LintSeverity::Warning => 2,
                    LintSeverity::Info => 3,
                },
                "source": "super-lint",
                "code": f.rule.id(),
                "message": format!("[{} {}] {}", f.rule.id(), f.rule.name(), f.message),
            })
        })
        .collect()
}
