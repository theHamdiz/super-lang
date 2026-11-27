//! MCP tool definitions and dispatch for Super knowledge.

use crate::catalog::{
    self, Algorithm, CHARACTER_LIMIT, ExampleProject, Feature, algorithms, cheatsheets, cli,
    examples, features, stdlib,
};
use serde_json::{Map, Value, json};

pub const TOOL_NAMES: &[&str] = &[
    "super_overview",
    "super_search",
    "super_list_features",
    "super_get_feature",
    "super_list_algorithms",
    "super_get_algorithm",
    "super_list_examples",
    "super_get_example",
    "super_list_stdlib",
    "super_get_stdlib",
    "super_cheatsheet",
    "super_cli_help",
    "super_verify",
];

fn annotations(read_only: bool) -> Value {
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

pub fn tools_list() -> Value {
    json!([
        {
            "name": "super_overview",
            "title": "Super language overview",
            "description": "Return Super 0.1.0 version, catalog counts, stdlib layers, and how to use the other tools. Call this first when learning Super. Do NOT use for a specific syntax detail — use super_get_feature or super_search instead.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_search",
            "title": "Search Super knowledge",
            "description": "Full-text search across language features, algorithms, examples, and stdlib modules. Use for natural-language questions like 'optional errors', 'websocket', or 'binary search'. Returns ranked hits with kind + id so you can fetch full detail. Supports pagination via offset/limit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "minLength": 1, "maxLength": 200, "description": "Search string" },
                    "kind": {
                        "type": "string",
                        "enum": ["all", "feature", "algorithm", "example", "stdlib"],
                        "default": "all",
                        "description": "Optional filter"
                    },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 },
                    "response_format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown" }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_list_features",
            "title": "List language features",
            "description": "List curated Super language feature entries (bindings, UFCS, tests, ECS, Pods, backends, FFI, MCP, …). Paginated.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "category": { "type": "string", "description": "Optional category filter" },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 30 }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_get_feature",
            "title": "Get language feature",
            "description": "Fetch one language feature by id (e.g. bindings, optionals-errors, ecs-state) with markdown body and Super code samples.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "minLength": 1, "description": "Feature id or title" }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_list_algorithms",
            "title": "List algorithms",
            "description": "List Super algorithm cookbook entries (sort, search, graphs, caches, game broadphase, rate limits, …).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "category": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 30 }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_get_algorithm",
            "title": "Get algorithm with Super code",
            "description": "Return a full Super algorithm implementation and notes by id (binary-search, quicksort, dijkstra, lru-cache, …).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "minLength": 1 }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_list_examples",
            "title": "List example projects",
            "description": "List runnable examples under examples/ (games, APIs, CLI/TUI/GUI, interop).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "description": "Optional kind: game, backend, data, cli, tui, gui, interop, systems" },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 40 }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_get_example",
            "title": "Get example project",
            "description": "Detail for one example id/path with highlights and a starter snippet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "minLength": 1 }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_list_stdlib",
            "title": "List / cover stdlib",
            "description": "Stdlib coverage: 127 modules across layers (+ mod.sr). Filter by layer. Includes vendor Pod pointers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "layer": { "type": "string", "description": "Optional layer: core, fmt, sys, io, net, db, sec, math, game, cli, ai, tui, gui, diag" },
                    "offset": { "type": "integer", "minimum": 0, "default": 0 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 50 },
                    "coverage": { "type": "boolean", "default": false, "description": "If true, return full coverage summary instead of a page" }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_get_stdlib",
            "title": "Get stdlib module",
            "description": "Detail for one module path (e.g. net.http or std.net.http). When SUPER_STDLIB is set, includes export list and optional source excerpt.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "minLength": 1, "description": "Module path like net.http" },
                    "include_source": { "type": "boolean", "default": true },
                    "max_source_chars": { "type": "integer", "minimum": 500, "maximum": 20000, "default": 8000 }
                },
                "required": ["path"],
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_cheatsheet",
            "title": "Syntax cheatsheet",
            "description": "Compact cheatsheet by id: syntax, imports, cli, types, reductions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "default": "syntax" }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_cli_help",
            "title": "CLI command map",
            "description": "Map of `super` subcommands including `super mcp` and `super lsp`.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        },
        {
            "name": "super_verify",
            "title": "Verify #law contracts",
            "description": "Run Super's three-tier formal verifier (SMT bitvector, comptime rewrite, bounded fuzz) on a Pod entry or `.sr` file. Returns structured pass/fail plus E0502 counterexample bindings so agents can self-heal implementations. `#law`/`#proof` never emit into C23/JIT/bytecode.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Pod root or .sr file (default: cwd Pod entry)" },
                    "locked_laws": { "type": "boolean", "default": false, "description": "Fail if #law bodies drift from laws.lock" },
                    "solver": { "type": "string", "enum": ["auto", "smt", "comptime", "fuzz"], "default": "auto" },
                    "json": { "type": "boolean", "default": true }
                },
                "additionalProperties": false
            },
            "outputSchema": { "type": "object" },
            "annotations": annotations(true)
        }
    ])
}

fn tool_result(data: Value, markdown: String) -> Value {
    let (text, truncated) = catalog::truncate_text(markdown);
    let mut structured = data;
    if let Some(obj) = structured.as_object_mut()
        && truncated
    {
        obj.insert("truncated".into(), json!(true));
    }
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: impl Into<String>) -> Value {
    let message = message.into();
    json!({
        "content": [{ "type": "text", "text": format!("Error: {message}") }],
        "isError": true
    })
}

fn int_param(params: &Value, key: &str, default: i64) -> i64 {
    params.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn bool_param(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(|v| v.as_str())
}

fn feature_json(f: &Feature) -> Value {
    json!({
        "id": f.id,
        "title": f.title,
        "category": f.category,
        "summary": f.summary,
        "tags": f.tags,
        "body_md": f.body_md
    })
}

fn algorithm_json(a: &Algorithm) -> Value {
    json!({
        "id": a.id,
        "title": a.title,
        "category": a.category,
        "summary": a.summary,
        "tags": a.tags,
        "code": a.code
    })
}

fn example_json(e: &ExampleProject) -> Value {
    json!({
        "id": e.id,
        "title": e.title,
        "path": e.path,
        "kind": e.kind,
        "summary": e.summary,
        "highlights": e.highlights,
        "snippet": e.snippet
    })
}

pub fn call_tool(name: &str, params: &Value) -> Value {
    match name {
        "super_overview" => {
            let data = catalog::overview_json();
            let md = format!(
                "# Super {}\n\nMCP: `super mcp` (stdio). Catalog: {} features, {} algorithms, {} examples, {} stdlib modules.\n\n{}",
                catalog::LANGUAGE_VERSION,
                features::ALL.len(),
                algorithms::ALL.len(),
                examples::ALL.len(),
                stdlib::MODULES.len(),
                data["how_to_use"]
                    .as_array()
                    .map(|a| a
                        .iter()
                        .filter_map(|x| x.as_str())
                        .map(|s| format!("- {s}"))
                        .collect::<Vec<_>>()
                        .join("\n"))
                    .unwrap_or_default()
            );
            tool_result(data, md)
        }
        "super_search" => {
            let Some(query) = str_param(params, "query").filter(|q| !q.trim().is_empty()) else {
                return tool_error("query is required (1–200 chars)");
            };
            if query.len() > 200 {
                return tool_error("query must be ≤ 200 characters");
            }
            let kind = str_param(params, "kind").unwrap_or("all");
            let offset = int_param(params, "offset", 0).max(0) as usize;
            let limit = int_param(params, "limit", 20).clamp(1, 50) as usize;
            let format = str_param(params, "response_format").unwrap_or("markdown");

            let mut hits: Vec<Value> = Vec::new();
            if kind == "all" || kind == "feature" {
                for f in features::search(query) {
                    hits.push(json!({"kind": "feature", "id": f.id, "title": f.title, "summary": f.summary}));
                }
            }
            if kind == "all" || kind == "algorithm" {
                for a in algorithms::search(query) {
                    hits.push(json!({"kind": "algorithm", "id": a.id, "title": a.title, "summary": a.summary}));
                }
            }
            if kind == "all" || kind == "example" {
                for e in examples::search(query) {
                    hits.push(json!({"kind": "example", "id": e.id, "title": e.title, "summary": e.summary, "path": e.path}));
                }
            }
            if kind == "all" || kind == "stdlib" {
                for m in stdlib::search(query) {
                    hits.push(json!({"kind": "stdlib", "id": m.path, "title": format!("std.{}", m.path), "summary": m.summary, "layer": m.layer}));
                }
            }
            let total = hits.len();
            let (page, has_more) = catalog::paginate(&hits, offset, limit);
            let page = page.to_vec();
            let data = json!({
                "query": query,
                "kind": kind,
                "total": total,
                "offset": offset,
                "limit": limit,
                "has_more": has_more,
                "next_offset": if has_more { offset + page.len() } else { offset },
                "hits": page
            });
            let md = if format == "json" {
                serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string())
            } else {
                let mut lines = vec![format!("# Search: {query} ({total} hits)\n")];
                for h in data["hits"].as_array().into_iter().flatten() {
                    lines.push(format!(
                        "- **{}** `{}` — {}",
                        h["kind"].as_str().unwrap_or("?"),
                        h["id"].as_str().unwrap_or("?"),
                        h["summary"].as_str().unwrap_or("")
                    ));
                }
                if has_more {
                    lines.push(format!(
                        "\nMore results: pass offset={} limit={limit}",
                        offset + limit
                    ));
                }
                lines.join("\n")
            };
            tool_result(data, md)
        }
        "super_list_features" => {
            let category = str_param(params, "category");
            let offset = int_param(params, "offset", 0).max(0) as usize;
            let limit = int_param(params, "limit", 30).clamp(1, 50) as usize;
            let filtered: Vec<_> = features::ALL
                .iter()
                .filter(|f| category.is_none_or(|c| f.category == c))
                .collect();
            let (page, has_more) = catalog::paginate(&filtered, offset, limit);
            let items: Vec<_> = page
                .iter()
                .map(|f| {
                    json!({"id": f.id, "title": f.title, "category": f.category, "summary": f.summary})
                })
                .collect();
            let data = json!({
                "total": filtered.len(),
                "offset": offset,
                "limit": limit,
                "has_more": has_more,
                "next_offset": if has_more { offset + items.len() } else { offset },
                "features": items
            });
            let md = items
                .iter()
                .map(|f| {
                    format!(
                        "- `{}` — {}",
                        f["id"].as_str().unwrap_or(""),
                        f["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            tool_result(data, format!("# Features\n\n{md}"))
        }
        "super_get_feature" => {
            let Some(id) = str_param(params, "id") else {
                return tool_error("id is required");
            };
            let Some(f) = features::find(id) else {
                return tool_error(format!(
                    "Unknown feature `{id}`. Call super_list_features or super_search."
                ));
            };
            let data = feature_json(f);
            tool_result(data, f.body_md.to_string())
        }
        "super_list_algorithms" => {
            let category = str_param(params, "category");
            let offset = int_param(params, "offset", 0).max(0) as usize;
            let limit = int_param(params, "limit", 30).clamp(1, 50) as usize;
            let filtered: Vec<_> = algorithms::ALL
                .iter()
                .filter(|a| category.is_none_or(|c| a.category == c))
                .collect();
            let (page, has_more) = catalog::paginate(&filtered, offset, limit);
            let items: Vec<_> = page
                .iter()
                .map(|a| {
                    json!({"id": a.id, "title": a.title, "category": a.category, "summary": a.summary})
                })
                .collect();
            let data = json!({
                "total": filtered.len(),
                "offset": offset,
                "has_more": has_more,
                "next_offset": if has_more { offset + items.len() } else { offset },
                "algorithms": items
            });
            let md = items
                .iter()
                .map(|a| {
                    format!(
                        "- `{}` — {}",
                        a["id"].as_str().unwrap_or(""),
                        a["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            tool_result(data, format!("# Algorithms\n\n{md}"))
        }
        "super_get_algorithm" => {
            let Some(id) = str_param(params, "id") else {
                return tool_error("id is required");
            };
            let Some(a) = algorithms::find(id) else {
                return tool_error(format!("Unknown algorithm `{id}`."));
            };
            let data = algorithm_json(a);
            let md = format!(
                "# {}\n\n{}\n\n```super\n{}\n```\n",
                a.title, a.summary, a.code
            );
            tool_result(data, md)
        }
        "super_list_examples" => {
            let kind = str_param(params, "kind");
            let offset = int_param(params, "offset", 0).max(0) as usize;
            let limit = int_param(params, "limit", 40).clamp(1, 50) as usize;
            let filtered: Vec<_> = examples::ALL
                .iter()
                .filter(|e| kind.is_none_or(|k| e.kind == k))
                .collect();
            let (page, has_more) = catalog::paginate(&filtered, offset, limit);
            let items: Vec<_> = page
                .iter()
                .map(|e| {
                    json!({"id": e.id, "title": e.title, "path": e.path, "kind": e.kind, "summary": e.summary})
                })
                .collect();
            let data = json!({
                "total": filtered.len(),
                "offset": offset,
                "has_more": has_more,
                "next_offset": if has_more { offset + items.len() } else { offset },
                "examples": items
            });
            let md = items
                .iter()
                .map(|e| {
                    format!(
                        "- `{}` ({}) — {}",
                        e["id"].as_str().unwrap_or(""),
                        e["path"].as_str().unwrap_or(""),
                        e["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            tool_result(data, format!("# Examples\n\n{md}"))
        }
        "super_get_example" => {
            let Some(id) = str_param(params, "id") else {
                return tool_error("id is required");
            };
            let Some(e) = examples::find(id) else {
                return tool_error(format!("Unknown example `{id}`."));
            };
            let data = example_json(e);
            let md = format!(
                "# {}\n\nPath: `{}`\n\n{}\n\nHighlights: {}\n\n```super\n{}\n```\n",
                e.title,
                e.path,
                e.summary,
                e.highlights.join(", "),
                e.snippet
            );
            tool_result(data, md)
        }
        "super_list_stdlib" => {
            if bool_param(params, "coverage", false) {
                let data = stdlib::coverage_summary();
                return tool_result(
                    data.clone(),
                    format!(
                        "# Stdlib coverage\n\n{} modules / 101 files. Layers: {}.\n",
                        stdlib::MODULES.len(),
                        stdlib::LAYERS.join(", ")
                    ),
                );
            }
            let layer = str_param(params, "layer");
            let offset = int_param(params, "offset", 0).max(0) as usize;
            let limit = int_param(params, "limit", 50).clamp(1, 100) as usize;
            let filtered: Vec<_> = if let Some(layer) = layer {
                stdlib::by_layer(layer)
            } else {
                stdlib::MODULES.iter().collect()
            };
            let (page, has_more) = catalog::paginate(&filtered, offset, limit);
            let items: Vec<_> = page
                .iter()
                .map(|m| json!({"path": m.path, "layer": m.layer, "summary": m.summary, "import": format!("use std.{}", m.path)}))
                .collect();
            let data = json!({
                "total": filtered.len(),
                "offset": offset,
                "has_more": has_more,
                "next_offset": if has_more { offset + items.len() } else { offset },
                "modules": items
            });
            let md = items
                .iter()
                .map(|m| {
                    format!(
                        "- `std.{}` — {}",
                        m["path"].as_str().unwrap_or(""),
                        m["summary"].as_str().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            tool_result(data, format!("# Stdlib modules\n\n{md}"))
        }
        "super_get_stdlib" => {
            let Some(path) = str_param(params, "path") else {
                return tool_error("path is required (e.g. net.http)");
            };
            let include_source = bool_param(params, "include_source", true);
            let max_chars = int_param(params, "max_source_chars", 8000).clamp(500, 20_000) as usize;
            let Some(data) =
                stdlib::module_detail(path, include_source, max_chars.min(CHARACTER_LIMIT))
            else {
                return tool_error(format!("Unknown stdlib module `{path}`."));
            };
            let md = format!(
                "# {}\n\nLayer: **{}**\n\n{}\n\nImport: `{}`\n",
                data["path"].as_str().unwrap_or(path),
                data["layer"].as_str().unwrap_or(""),
                data["summary"].as_str().unwrap_or(""),
                data["import"].as_str().unwrap_or("")
            );
            let md = if let Some(src) = data.get("source").and_then(|s| s.as_str()) {
                format!("{md}\n## Source excerpt\n\n```super\n{src}\n```\n")
            } else {
                md
            };
            tool_result(data, md)
        }
        "super_cheatsheet" => {
            let id = str_param(params, "id").unwrap_or("syntax");
            let Some(c) = cheatsheets::find(id) else {
                return tool_error(format!(
                    "Unknown cheatsheet `{id}`. Try: syntax, imports, cli, types."
                ));
            };
            let data = json!({"id": c.id, "title": c.title, "body_md": c.body_md});
            tool_result(data, c.body_md.to_string())
        }
        "super_verify" => {
            let path = str_param(params, "path").unwrap_or(".");
            let locked = bool_param(params, "locked_laws", false);
            let solver = str_param(params, "solver").unwrap_or("auto");
            let want_json = bool_param(params, "json", true);
            match run_super_verify(path, locked, solver) {
                Ok(report) => {
                    let data = serde_json::from_str::<Value>(&report)
                        .unwrap_or_else(|_| json!({"raw": report}));
                    let md = if want_json {
                        format!("```json\n{report}\n```")
                    } else {
                        format!(
                            "Verification finished. See structuredContent for details.\n\n```json\n{report}\n```"
                        )
                    };
                    tool_result(data, md)
                }
                Err(e) => tool_error(e),
            }
        }
        "super_cli_help" => {
            let data = json!({"commands": cli::commands_json()});
            let md = "# Super CLI\n\nUse `super mcp` for this server; `super lsp` for editor IntelliSense.\n".to_string();
            tool_result(data, md)
        }
        _ => tool_error(format!(
            "Unknown tool `{name}`. Available: {}",
            TOOL_NAMES.join(", ")
        )),
    }
}

fn list_cwd_laws() -> Value {
    use std::path::PathBuf;
    use super_frontend::{analyze_project, collect_verify_items};

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let candidates = [
        cwd.join("src/main.sr"),
        cwd.join("src/lib.sr"),
        cwd.join("main.sr"),
        cwd.join("lib.sr"),
    ];
    let Some(entry) = candidates.into_iter().find(|p| p.is_file()) else {
        return json!({"laws": [], "note": "no Pod entry in cwd"});
    };
    let result = analyze_project(&entry);
    let (laws, proofs) = collect_verify_items(&result.module, &result.interner);
    let laws_json: Vec<_> = laws
        .iter()
        .map(|l| {
            json!({
                "name": result.interner.resolve(l.name),
                "description": l.law_description()
                    .map(|d| result.interner.resolve(d).to_string())
                    .unwrap_or_default(),
            })
        })
        .collect();
    let proofs_json: Vec<_> = proofs
        .iter()
        .map(|p| {
            json!({
                "name": result.interner.resolve(p.name),
                "law": p.proof_target()
                    .map(|d| result.interner.resolve(d).to_string())
                    .unwrap_or_default(),
            })
        })
        .collect();
    json!({
        "entry": entry.display().to_string(),
        "laws": laws_json,
        "proofs": proofs_json
    })
}

fn run_super_verify(path: &str, locked_laws: bool, solver: &str) -> Result<String, String> {
    use std::path::PathBuf;
    use super_frontend::{
        SolverKind, VerifyOptions, analyze_project, parse_laws_lock, verify_module,
    };

    let entry = PathBuf::from(path);
    let entry = if entry.is_dir() {
        // Prefer Pod entry files.
        let candidates = [
            entry.join("src/main.sr"),
            entry.join("src/lib.sr"),
            entry.join("main.sr"),
            entry.join("lib.sr"),
        ];
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .ok_or_else(|| format!("no Super entry under `{path}`"))?
    } else {
        entry
    };
    if !entry.is_file() {
        return Err(format!("path not found: {path}"));
    }

    let solver = SolverKind::parse(solver)?;
    let pod_root = entry.parent().and_then(|p| {
        let mut cur = p.to_path_buf();
        loop {
            if cur.join("Pod.toml").is_file() {
                return Some(cur);
            }
            if !cur.pop() {
                return None;
            }
        }
    });
    let mut law_lock = std::collections::BTreeMap::new();
    if let Some(root) = &pod_root
        && let Ok(text) = std::fs::read_to_string(root.join("laws.lock"))
    {
        law_lock = parse_laws_lock(&text);
    }

    let mut result = analyze_project(&entry);
    if result.diagnostics.has_errors() {
        return Err(format!(
            "analyze failed with {} error(s)",
            result.diagnostics.iter().count()
        ));
    }
    let bench_root = super_frontend::find_benchmarks_root(Some(&entry)).or_else(|| {
        pod_root
            .as_deref()
            .and_then(|r| super_frontend::find_benchmarks_root(Some(r)))
    });
    let opts = VerifyOptions {
        locked_laws,
        solver,
        timeout_ms: super_frontend::DEFAULT_VERIFY_TIMEOUT_MS,
        pod_root,
        law_lock,
        bench_root,
    };
    let report = verify_module(
        &result.module,
        &result.interner,
        &mut result.diagnostics,
        &opts,
    );
    Ok(report.to_json())
}

pub fn resources_list() -> Value {
    let mut resources = Vec::new();
    resources.push(json!({
        "uri": "super://overview",
        "name": "Super overview",
        "mimeType": "application/json",
        "description": "Language version and catalog counts"
    }));
    resources.push(json!({
        "uri": "super://laws",
        "name": "Active Pod #law contracts",
        "mimeType": "application/json",
        "description": "Lists #law items discovered under the current working directory Pod"
    }));
    for f in features::ALL {
        resources.push(json!({
            "uri": format!("super://feature/{}", f.id),
            "name": f.title,
            "mimeType": "text/markdown",
            "description": f.summary
        }));
    }
    for a in algorithms::ALL {
        resources.push(json!({
            "uri": format!("super://algorithm/{}", a.id),
            "name": a.title,
            "mimeType": "text/markdown",
            "description": a.summary
        }));
    }
    for e in examples::ALL {
        resources.push(json!({
            "uri": format!("super://example/{}", e.id),
            "name": e.title,
            "mimeType": "text/markdown",
            "description": e.summary
        }));
    }
    for m in stdlib::MODULES {
        resources.push(json!({
            "uri": format!("super://stdlib/{}", m.path),
            "name": format!("std.{}", m.path),
            "mimeType": "text/markdown",
            "description": m.summary
        }));
    }
    json!(resources)
}

pub fn resource_read(uri: &str) -> Result<Value, String> {
    if uri == "super://overview" {
        let data = catalog::overview_json();
        return Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&data).unwrap_or_default()
            }]
        }));
    }
    if uri == "super://laws" {
        let data = list_cwd_laws();
        return Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&data).unwrap_or_default()
            }]
        }));
    }
    if let Some(id) = uri.strip_prefix("super://feature/") {
        let f = features::find(id).ok_or_else(|| format!("Unknown feature resource `{uri}`"))?;
        return Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": f.body_md }]
        }));
    }
    if let Some(id) = uri.strip_prefix("super://algorithm/") {
        let a =
            algorithms::find(id).ok_or_else(|| format!("Unknown algorithm resource `{uri}`"))?;
        let text = format!(
            "# {}\n\n{}\n\n```super\n{}\n```\n",
            a.title, a.summary, a.code
        );
        return Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": text }]
        }));
    }
    if let Some(id) = uri.strip_prefix("super://example/") {
        let e = examples::find(id).ok_or_else(|| format!("Unknown example resource `{uri}`"))?;
        let text = format!(
            "# {}\n\n`{}`\n\n{}\n\n```super\n{}\n```\n",
            e.title, e.path, e.summary, e.snippet
        );
        return Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": text }]
        }));
    }
    if let Some(path) = uri.strip_prefix("super://stdlib/") {
        let data = stdlib::module_detail(path, true, 12_000)
            .ok_or_else(|| format!("Unknown stdlib resource `{uri}`"))?;
        let text = serde_json::to_string_pretty(&data).unwrap_or_default();
        return Ok(json!({
            "contents": [{ "uri": uri, "mimeType": "application/json", "text": text }]
        }));
    }
    Err(format!("Unknown resource `{uri}`"))
}

pub fn prompts_list() -> Value {
    json!([
        {
            "name": "learn-super",
            "title": "Learn Super",
            "description": "Guided walkthrough of Super 0.1.0 for a coding agent",
            "arguments": [
                { "name": "goal", "description": "What you want to build", "required": false }
            ]
        },
        {
            "name": "write-super",
            "title": "Write Super code",
            "description": "Prompt to generate idiomatic Super given a task",
            "arguments": [
                { "name": "task", "description": "Task description", "required": true }
            ]
        },
        {
            "name": "explore-stdlib",
            "title": "Explore stdlib",
            "description": "Prompt to explore a stdlib layer or module",
            "arguments": [
                { "name": "layer_or_path", "description": "e.g. net or net.http", "required": true }
            ]
        }
    ])
}

pub fn prompts_get(name: &str, args: &Map<String, Value>) -> Result<Value, String> {
    match name {
        "learn-super" => {
            let goal = args
                .get("goal")
                .and_then(|v| v.as_str())
                .unwrap_or("general language mastery");
            Ok(json!({
                "description": "Learn Super",
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": format!(
                            "You are learning Super {ver}. Call super_overview, then super_cheatsheet id=syntax, then super_search for topics related to: {goal}. Prefer super_get_feature and super_get_example for details. Write idiomatic .sr with test blocks. When changing behavior under `#law` contracts, put proofs in `*.proof.sr`, never edit `*.law.sr`, and run `super_verify` (or `super verify`) before finishing.",
                            ver = catalog::LANGUAGE_VERSION,
                            goal = goal
                        )
                    }
                }]
            }))
        }
        "write-super" => {
            let task = args
                .get("task")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "argument `task` is required".to_string())?;
            Ok(json!({
                "description": "Write Super",
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": format!(
                            "Write idiomatic Super 0.1.0 for: {task}\nUse UFCS, inferred literals, optionals/errors with `or`, and include a `test` block. Consult super_search / super_get_algorithm / super_get_stdlib as needed.\nLaw-guarded workflow: human-locked invariants live in `*.law.sr` (`#law`); AI proofs/lemmas go in `*.proof.sr` (`#proof`). Never modify `#law` bodies in CI (`--locked-laws`). After implementing, call `super_verify` and heal from E0502 counterexamples."
                        )
                    }
                }]
            }))
        }
        "explore-stdlib" => {
            let target = args
                .get("layer_or_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "argument `layer_or_path` is required".to_string())?;
            Ok(json!({
                "description": "Explore stdlib",
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": format!(
                            "Explore Super stdlib target `{target}`. Use super_list_stdlib (coverage or layer filter) and super_get_stdlib with include_source=true. Summarize public exports and show a minimal usage snippet."
                        )
                    }
                }]
            }))
        }
        _ => Err(format!("Unknown prompt `{name}`")),
    }
}
