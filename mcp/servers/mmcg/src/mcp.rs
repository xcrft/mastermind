//! MCP server — JSON-RPC over stdio.
//!
//! Implements the subset of the Model Context Protocol that Claude Code
//! needs: `initialize`, `tools/list`, `tools/call`. Hand-rolled (no SDK dep).
//! Protocol reference: https://modelcontextprotocol.io
//!
//! Wire format: newline-delimited JSON-RPC 2.0 messages on stdin/stdout.

use crate::queries;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Run as an MCP stdio server. Blocks until stdin closes.
pub fn serve(mut store: Store) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let input = stdin.lock();

    for line in input.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[mmcg] parse error: {e} (line: {trimmed})");
                continue;
            }
        };

        // Notifications have no id — fire-and-forget, no response.
        let Some(id) = req.id.clone() else {
            continue;
        };

        let response = handle_request(&mut store, &req.method, &req.params, id);

        match serde_json::to_string(&response) {
            Ok(s) => {
                writeln!(out, "{s}")?;
                out.flush()?;
            }
            Err(e) => eprintln!("[mmcg] serialize error: {e}"),
        }
    }
    Ok(())
}

fn handle_request(store: &mut Store, method: &str, params: &Value, id: Value) -> JsonRpcResponse {
    match method {
        "initialize" => ok(id, initialize_result()),
        "initialized" | "notifications/initialized" => ok(id, json!({})),
        "tools/list" => ok(id, tools_list()),
        "tools/call" => match handle_tools_call(store, params) {
            Ok(v) => ok(id, v),
            Err(msg) => err(id, -32603, msg),
        },
        "ping" => ok(id, json!({})),
        other => err(id, -32601, format!("method not found: {other}")),
    }
}

fn ok(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Value, code: i32, message: String) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError { code, message }),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "mmcg",
            "version": env!("CARGO_PKG_VERSION"),
        }
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "mmcg_search",
                "description": "Find symbols (functions, classes, methods, structs, traits, etc.) by exact name. Returns location, kind, signature, and any decorators/attributes. Pass `language` to filter by `python`/`typescript`/`tsx`/`javascript`/`rust`/`csharp` — defends against name collisions in monorepos. C# `partial class` declarations across files are collapsed into a single hit with a `locations` array by default; pass `collapse_partials: false` to see every declaration.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Symbol name (exact match)" },
                        "kind": { "type": "string", "description": "Optional kind filter (function, class, method, struct, enum, trait, interface, record, property, etc.)" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" },
                        "collapse_partials": { "type": "boolean", "default": true, "description": "When true (default), C# `partial class Foo` declarations across N files return one hit with a `locations` array of all N declarations. Set false to see each declaration as a separate row." }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "mmcg_callers",
                "description": "List symbols that reference the given name. Matches both leaf names (`obj.foo()` → 'foo') AND type prefixes (`SessionStore::new()` → 'SessionStore'). Use before editing to assess blast radius. Pass `language` to filter against monorepo collisions. Pass `edge_kind` (default 'calls') to switch between call/import/inherit edges.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Name or type to look up" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] },
                        "edge_kind": { "type": "string", "enum": ["calls", "imports", "inherits"], "default": "calls", "description": "Which kind of incoming edge to consider" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "mmcg_callees",
                "description": "List names that the given symbol references. Pass `edge_kind` (default 'calls') to switch between call/import/inherit edges.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Symbol whose outgoing edges you want to inspect" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] },
                        "edge_kind": { "type": "string", "enum": ["calls", "imports", "inherits"], "default": "calls" }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "mmcg_impact",
                "description": "Transitive callers of the symbol up to max_depth. Use for blast-radius analysis on widely-called functions. Matches by name OR type prefix (like mmcg_callers).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "max_depth": { "type": "integer", "minimum": 1, "maximum": 10, "default": 2 },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] }
                    },
                    "required": ["name"]
                }
            },
            {
                "name": "mmcg_symbols_in_file",
                "description": "List every symbol (function, class, method, struct, etc.) defined in a file, in source order. Faster than Read for getting a structural overview.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "mmcg_outline",
                "description": "Return the symbol tree of a file (classes / impls own their methods; modules own top-level functions). One call replaces a search + multiple symbols_in_file lookups. Useful for refactor planning or jumping to a symbol by structure.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "mmcg_files",
                "description": "List indexed files. Optionally filter by path prefix and/or language.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prefix": { "type": "string", "description": "Optional path prefix" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" }
                    }
                }
            },
            {
                "name": "mmcg_imports",
                "description": "List names imported by a file. Useful for understanding a file's dependencies.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
                    },
                    "required": ["file"]
                }
            },
            {
                "name": "mmcg_imported_by",
                "description": "List files whose top-level import declarations reference the given name or fully-qualified path. Use for 'who depends on this?' before renaming. Pass `match` = 'name' (default, leaf binding name) or 'path' (fully-qualified import path like 'foo.bar.baz' for Python or 'foo::bar' for Rust). Pass `language` to scope against monorepo name collisions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Name or path to look up" },
                        "match": { "type": "string", "enum": ["name", "path"], "default": "name", "description": "How to match the query — by leaf binding name or fully-qualified path" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "mmcg_unreferenced",
                "description": "Symbols that no edge references (no caller, no importer). Dead-code candidates. Optional `kind` / `language` filters. WARNING: false-positives for entry points (main, framework-registered handlers), dynamic dispatch / reflection, and cross-language calls (e.g. TS subprocess into Python). Review hits manually before deleting.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string", "description": "Filter by symbol kind (function / class / method / struct / etc.)" },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] }
                    }
                }
            },
            {
                "name": "mmcg_api_surface",
                "description": "Symbols defined under `prefix` that have at least one caller from OUTSIDE `prefix`. Empirical 'who-uses-this-module' map — does not require declared visibility. Useful for boundary planning before refactor / extract / rename.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prefix": { "type": "string", "description": "Path prefix (e.g. 'src/runtime/'). LIKE-matched." },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] }
                    },
                    "required": ["prefix"]
                }
            },
            {
                "name": "mmcg_tasks",
                "description": "Full-text search past task specs in `.mastermind/tasks/`. Use to recall prior designs and surface 'we already tried this' before drafting a new spec. FTS5 MATCH syntax — bare words AND-joined ('rate limit'), phrases double-quoted ('\\\"rate limit\\\"'), OR/NOT supported. Returns paths, titles, and snippet excerpts with «match» highlights ranked by BM25.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "FTS5 MATCH query (e.g. 'rate limit', 'auth OR session', '\\\"token bucket\\\"')" },
                        "top": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10, "description": "How many results to return" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "mmcg_centrality",
                "description": "Rank symbols by in-degree (distinct callers, matched by name OR type prefix). Pre-flight 'where is the gravity' tool — top hits are the codebase's structural attractors (core utilities, central domain types, framework hooks). Use on unfamiliar code or a `prefix` like 'src/auth/' to learn what to read first. Excludes synthetic `<module>` rows and symbols with zero in-degree.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prefix": { "type": "string", "description": "Optional path prefix to limit ranking scope (e.g. 'src/auth/'). LIKE-matched." },
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] },
                        "kind": { "type": "string", "description": "Optional kind filter (function, class, method, struct, etc.)" },
                        "top": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20, "description": "How many results to return" }
                    }
                }
            },
            {
                "name": "mmcg_recent_changes",
                "description": "Files re-indexed within a recent time window (per the watcher's `indexed_at` mtime). Useful when investigating a recent incident or asking 'what changed in the last hour?'. Pass `since` as a short duration string: 30s / 10m / 2h / 1d.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since": { "type": "string", "description": "Time window — e.g. '2h', '30m', '1d'" }
                    },
                    "required": ["since"]
                }
            },
            {
                "name": "mmcg_status",
                "description": "Show index health — file count, symbol count, db path.",
                "inputSchema": { "type": "object", "properties": {} }
            }
        ]
    })
}

fn handle_tools_call(store: &mut Store, params: &Value) -> Result<Value, String> {
    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing 'name' in tools/call params".to_string())?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let payload = match tool_name {
        "mmcg_search" => {
            let name = str_arg(&args, "name")?;
            let kind = opt_str_arg(&args, "kind");
            let language = opt_str_arg(&args, "language");
            // Default true — collapsing partial-class duplicates is the safer default.
            // Pass `collapse_partials: false` to see every declaration as a separate row.
            let collapse = opt_bool_arg(&args, "collapse_partials").unwrap_or(true);
            let r = queries::search(store, name, kind, language, collapse)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_callers" => {
            let name = str_arg(&args, "name")?;
            let language = opt_str_arg(&args, "language");
            let edge_kind = opt_str_arg(&args, "edge_kind");
            let r =
                queries::callers(store, name, language, edge_kind).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_callees" => {
            let name = str_arg(&args, "name")?;
            let language = opt_str_arg(&args, "language");
            let edge_kind = opt_str_arg(&args, "edge_kind");
            let r =
                queries::callees(store, name, language, edge_kind).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_impact" => {
            let name = str_arg(&args, "name")?;
            let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
            let language = opt_str_arg(&args, "language");
            let r = queries::impact(store, name, max_depth, language).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_symbols_in_file" => {
            let file = str_arg(&args, "file")?;
            let r = queries::symbols_in_file(store, file).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_outline" => {
            let file = str_arg(&args, "file")?;
            let r = queries::outline(store, file).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_files" => {
            let prefix = opt_str_arg(&args, "prefix");
            let language = opt_str_arg(&args, "language");
            let r = queries::files(store, prefix, language).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_imports" => {
            let file = str_arg(&args, "file")?;
            let r = queries::imports(store, file).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_imported_by" => {
            // Accept both old shape ({"name": ...}) and new shape ({"query": ..., "match": ...})
            let query = str_arg(&args, "query").or_else(|_| str_arg(&args, "name"))?;
            let match_kind = opt_str_arg(&args, "match").unwrap_or("name");
            let language = opt_str_arg(&args, "language");
            let r = queries::imported_by(store, query, match_kind, language)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_unreferenced" => {
            let kind = opt_str_arg(&args, "kind");
            let language = opt_str_arg(&args, "language");
            let r = queries::unreferenced(store, kind, language).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_api_surface" => {
            let prefix = str_arg(&args, "prefix")?;
            let language = opt_str_arg(&args, "language");
            let r = queries::api_surface(store, prefix, language).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_tasks" => {
            let query = str_arg(&args, "query")?;
            let top = args
                .get("top")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(10)
                .clamp(1, 50);
            let r = queries::tasks(store, query, top).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_centrality" => {
            let prefix = opt_str_arg(&args, "prefix");
            let language = opt_str_arg(&args, "language");
            let kind = opt_str_arg(&args, "kind");
            let top = args
                .get("top")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(20)
                .clamp(1, 200);
            let r = queries::centrality(store, prefix, language, kind, top)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_recent_changes" => {
            let since = str_arg(&args, "since")?;
            let r = queries::recent_changes(store, since)?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_status" => {
            let r = queries::status(store).map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        other => return Err(format!("unknown tool: {other}")),
    };

    Ok(json!({
        "content": [
            { "type": "text", "text": serde_json::to_string_pretty(&payload).unwrap_or_default() }
        ]
    }))
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("missing or non-string argument '{name}'"))
}

fn opt_str_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(|v| v.as_str())
}

fn opt_bool_arg(args: &Value, name: &str) -> Option<bool> {
    args.get(name).and_then(|v| v.as_bool())
}
