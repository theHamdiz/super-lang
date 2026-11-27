//! Curated Super 0.1.0 knowledge catalog for coding agents.

pub mod algorithms;
pub mod cheatsheets;
pub mod cli;
pub mod examples;
pub mod features;
pub mod stdlib;

use serde_json::{Value, json};

pub const LANGUAGE_VERSION: &str = "0.1.0";
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
pub const CHARACTER_LIMIT: usize = 25_000;

#[derive(Clone, Copy)]
pub struct Feature {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub summary: &'static str,
    pub body_md: &'static str,
    pub tags: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub struct Algorithm {
    pub id: &'static str,
    pub title: &'static str,
    pub category: &'static str,
    pub summary: &'static str,
    pub code: &'static str,
    pub tags: &'static [&'static str],
}

#[derive(Clone, Copy)]
pub struct ExampleProject {
    pub id: &'static str,
    pub title: &'static str,
    pub path: &'static str,
    pub kind: &'static str,
    pub summary: &'static str,
    pub highlights: &'static [&'static str],
    pub snippet: &'static str,
}

#[derive(Clone, Copy)]
pub struct StdlibModule {
    pub path: &'static str,
    pub layer: &'static str,
    pub summary: &'static str,
}

pub fn overview_json() -> Value {
    json!({
        "language": "Super",
        "version": LANGUAGE_VERSION,
        "extension": ".sr",
        "mcp": {
            "name": "super-mcp",
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "command": "super mcp",
            "transport": "stdio (newline-delimited JSON-RPC 2.0)"
        },
        "counts": {
            "features": features::ALL.len(),
            "algorithms": algorithms::ALL.len(),
            "examples": examples::ALL.len(),
            "stdlib_modules": stdlib::MODULES.len(),
            "stdlib_files": 147,
            "cheatsheets": cheatsheets::ALL.len()
        },
        "stage1": {
            "protocol": "2.0.0",
            "composite_winner": "super_aot",
            "composite_score": 90.06,
            "category_winners": {
                "speed": "super_aot",
                "memory": "super_aot",
                "loc": "super_aot",
                "cycle": "super_jit",
                "reliability": "c"
            },
            "super_code_loc": 2198,
            "site": "docs/",
            "law": "LAW-BENCH-001",
            "stdlib": "stdlib/**/*.sr only; host builtins in crates/super-host"
        },
        "layers": stdlib::LAYERS,
        "how_to_use": [
            "Call super_search with a natural-language query first.",
            "Use super_get_feature / super_get_algorithm / super_get_example for full detail.",
            "Use super_list_stdlib + super_get_stdlib for import paths and live source when SUPER_STDLIB is set.",
            "Use super_cheatsheet for compact syntax reminders.",
            "Stage-1 standings: composite Super AOT 90.06 #1; see feature id stage1-benchmarks and docs/."
        ]
    })
}

pub fn truncate_text(mut text: String) -> (String, bool) {
    if text.len() <= CHARACTER_LIMIT {
        return (text, false);
    }
    text.truncate(CHARACTER_LIMIT);
    text.push_str("\n\n… truncated. Narrow the query or raise offset/limit.");
    (text, true)
}

pub fn matches_query(hay: &str, query: &str) -> bool {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return true;
    }
    let h = hay.to_ascii_lowercase();
    q.split_whitespace().all(|part| h.contains(part))
}

pub fn paginate<T>(items: &[T], offset: usize, limit: usize) -> (&[T], bool) {
    let start = offset.min(items.len());
    let end = (start + limit).min(items.len());
    (&items[start..end], end < items.len())
}
