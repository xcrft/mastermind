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
                "name": "mmcg_symbols_changed_since",
                "description": "Symbol-level diff between a git ref and the current index. Returns {added, removed, signature_changed} symbol sets across the files in `git diff --name-only <ref>..HEAD`. Re-parses old blobs from `git show <ref>:<path>` with the same extractor used at index time. Different from `mmcg_recent_changes` (which uses watcher mtime) — this is git-ref-based and answers 'what symbols did THIS PR/branch touch?'. Use cases: PR-review pre-flight, auditor verifying executor's claimed-files vs reality, 'what new public API appeared in v2.3?'.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "git_ref": { "type": "string", "description": "Git ref to diff against (tag, branch, commit, HEAD~3, main, etc.). Must resolve via `git rev-parse`." },
                        "root": { "type": "string", "description": "Project root — symbol paths are relative to this. Defaults to the index's working directory." }
                    },
                    "required": ["git_ref"]
                }
            },
            {
                "name": "mmcg_dependency_cycles",
                "description": "Detect circular imports — strongly-connected components in the file-level import graph. Returns each cycle as a list of files. Pre-merge guard: 'does this PR introduce a new cycle?'. Architectural hygiene: 'what cycles already exist?'. Edges are resolved by leaf-name match (over-approximating — two unrelated symbols sharing a name produce a cross-edge; verify before acting). Set `min_size` higher to hide trivial A↔B and surface only larger structural issues.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" },
                        "min_size": { "type": "integer", "minimum": 2, "maximum": 100, "default": 2, "description": "Smallest SCC to report. 2 = any cycle. 3 hides trivial A↔B pairs." }
                    }
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
            },
            {
                "name": "mmcg_scratchpad_append",
                "description": "Append a one-line intent / note / handoff to the cross-agent scratchpad. Live in-session channel between Mastermind subagents (planner → executor → auditor). Use to hand off context the next agent needs without polluting the chat or the spec. Persists in `.mastermind/mmcg.db` (additive table, gitignored). The cross-session counterpart is `_lessons.md` (auditor-written). Body capped at 8 KiB — scratchpad is for one-liners, not blob dumps.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "agent": { "type": "string", "description": "Agent identifier — conventionally `planner` / `executor` / `auditor` / `critic`, but freeform." },
                        "kind": { "type": "string", "description": "Entry kind — conventionally `intent` / `note` / `handoff` / `risk`, but freeform." },
                        "body": { "type": "string", "description": "The one-line content. ≤ 8 KiB." }
                    },
                    "required": ["agent", "kind", "body"]
                }
            },
            {
                "name": "mmcg_scratchpad_read",
                "description": "Read recent scratchpad entries, newest first. All filters optional — call with no args to get the last 20 entries. Use `since` (unix seconds) to grab everything since you last checked.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "since": { "type": "integer", "description": "Unix timestamp (seconds). Only entries with `ts >= since` are returned." },
                        "agent": { "type": "string", "description": "Filter by agent identifier." },
                        "kind": { "type": "string", "description": "Filter by entry kind." },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20, "description": "Max entries returned." }
                    }
                }
            },
            {
                "name": "mmcg_change_class",
                "description": "Classify a file's last change as `structural` (signatures, edges, or imports changed), `cosmetic` (only line numbers / whitespace / comments differ), or `first-seen` (file never indexed). Pre-edit signal for planner and auditor: a diff of 20 files where 17 are cosmetic-only has a much smaller blast radius than its raw line count suggests. Backed by a deterministic FNV-1a 64-bit hash of the file's parsed structural shape — same source on any machine yields the same fingerprint.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "file": { "type": "string", "description": "Path relative to the project root (e.g. `src/auth/login.ts`)." }
                    },
                    "required": ["file"]
                }
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
        "mmcg_symbols_changed_since" => {
            let git_ref = str_arg(&args, "git_ref")?;
            let root_arg = opt_str_arg(&args, "root");
            // Default to the directory containing the index database — that's
            // where `mmcg index` was run, so paths are relative to it.
            let root = match root_arg {
                Some(s) => std::path::PathBuf::from(s),
                None => store
                    .db_path()
                    .parent()
                    .and_then(|d| d.parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from(".")),
            };
            let root = root
                .canonicalize()
                .map_err(|e| format!("canonicalize root: {e}"))?;
            let diff =
                queries::symbols_changed_since(store, &root, git_ref).map_err(|e| e.to_string())?;
            serde_json::to_value(diff).map_err(|e| e.to_string())?
        }
        "mmcg_dependency_cycles" => {
            let language = opt_str_arg(&args, "language");
            let min_size = args
                .get("min_size")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(2)
                .clamp(2, 100);
            let r =
                queries::dependency_cycles(store, language, min_size).map_err(|e| e.to_string())?;
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
        "mmcg_scratchpad_append" => {
            let agent = str_arg(&args, "agent")?;
            let kind = str_arg(&args, "kind")?;
            let body = str_arg(&args, "body")?;
            const BODY_MAX: usize = 8 * 1024;
            if body.len() > BODY_MAX {
                return Err(format!(
                    "scratchpad body too large: {} bytes (max {})",
                    body.len(),
                    BODY_MAX
                ));
            }
            let (id, ts) = store
                .scratchpad_append(agent, kind, body)
                .map_err(|e| e.to_string())?;
            json!({ "id": id, "ts": ts })
        }
        "mmcg_scratchpad_read" => {
            let since = args.get("since").and_then(|v| v.as_i64());
            let agent = opt_str_arg(&args, "agent");
            let kind = opt_str_arg(&args, "kind");
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .min(200) as u32;
            let r = store
                .scratchpad_read(since, agent, kind, limit)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(r).map_err(|e| e.to_string())?
        }
        "mmcg_change_class" => {
            let file = str_arg(&args, "file")?;
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let r = queries::classify_change(store, &root, file)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: handle_tools_call wraps every successful payload in the MCP
    /// content envelope (`{ "content": [{ "type": "text", "text": <serialized> }] }`).
    /// This unwraps it back to the raw payload for assertion convenience.
    fn unwrap_content(v: &serde_json::Value) -> serde_json::Value {
        let text = v["content"][0]["text"].as_str().expect("content[0].text");
        serde_json::from_str(text).expect("content[0].text was not valid JSON")
    }

    #[test]
    fn scratchpad_round_trip_via_tools_call() {
        let path = std::env::temp_dir().join("mmcg_mcp_scratchpad.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();

        let append_env = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_scratchpad_append",
                "arguments": {
                    "agent": "planner",
                    "kind": "intent",
                    "body": "spec 001 ready for executor"
                }
            }),
        )
        .unwrap();
        let append = unwrap_content(&append_env);
        assert!(append.get("id").and_then(|v| v.as_i64()).is_some());
        assert!(append.get("ts").and_then(|v| v.as_i64()).is_some());

        let read_env = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_scratchpad_read",
                "arguments": { "limit": 10 }
            }),
        )
        .unwrap();
        let read = unwrap_content(&read_env);
        let arr = read.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["agent"], "planner");
        assert_eq!(arr[0]["kind"], "intent");
        assert_eq!(arr[0]["body"], "spec 001 ready for executor");

        // Body-size guard — error path returns an Err(String), NOT a wrapped payload.
        let too_big = "x".repeat(8 * 1024 + 1);
        let err = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_scratchpad_append",
                "arguments": { "agent": "a", "kind": "n", "body": too_big }
            }),
        )
        .unwrap_err();
        assert!(err.contains("too large"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn change_class_round_trip_via_tools_call() {
        // Layout: tmp/.mastermind/mmcg.db + tmp/src/foo.rs
        let tmp = std::env::temp_dir().join("mmcg_change_class_rt");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        let db_path = tmp.join("mmcg.db");
        let mut store = crate::store::Store::open(&db_path).unwrap();

        let foo_path = tmp.join("src/foo.rs");
        let rel = "src/foo.rs";
        std::fs::write(&foo_path, "// header\nfn foo() {}\nfn bar() { foo(); }\n").unwrap();

        // Switch into tmp so `crate::indexer::parse_one` (which the dispatch
        // arm invokes via std::env::current_dir) resolves paths relative to it.
        let _ = std::env::set_current_dir(&tmp);

        // Stage 1: file never indexed → first-seen.
        let first_env = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_change_class",
                "arguments": { "file": rel }
            }),
        )
        .unwrap();
        let first = unwrap_content(&first_env);
        assert_eq!(first["class"], "first-seen");
        assert!(first["current_fingerprint"]
            .as_str()
            .unwrap()
            .chars()
            .all(|c| c.is_ascii_hexdigit()));

        // Stage 2: index the file using the same parser path the classifier
        // will use later. This guarantees the stored fingerprint and the
        // current fingerprint use identical extractor output (kind strings,
        // signatures), so a cosmetic edit really does hash to the same value.
        let extractor =
            crate::indexer::extractor_for_path(&foo_path).expect("rust extractor available");
        let pending =
            crate::indexer::parse_one(&foo_path, &tmp, extractor.as_ref()).expect("parse foo.rs");
        let stored_fp = crate::fingerprint::compute_structural_fingerprint(&pending);
        store.commit_file(pending).unwrap();
        assert_eq!(
            store.file_fingerprint(rel).unwrap().as_deref(),
            Some(stored_fp.as_str())
        );

        // Stage 3: cosmetic edit — only comments + line positions change.
        // Same (kind, name, signature) tuples, same call edge → same hash.
        std::fs::write(
            &foo_path,
            "// header v2 (longer)\n// extra comment\nfn foo() {}\nfn bar() { foo(); }\n",
        )
        .unwrap();
        let cosmetic_env = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_change_class",
                "arguments": { "file": rel }
            }),
        )
        .unwrap();
        let cosmetic = unwrap_content(&cosmetic_env);
        assert_eq!(cosmetic["class"], "cosmetic");
        assert_eq!(
            cosmetic["stored_fingerprint"].as_str(),
            Some(stored_fp.as_str())
        );

        // Stage 4: structural edit — add a new function. New symbol tuple →
        // hash diverges → classifier returns "structural".
        std::fs::write(
            &foo_path,
            "fn foo() {}\nfn bar() { foo(); }\nfn baz() { bar(); }\n",
        )
        .unwrap();
        let structural_env = handle_tools_call(
            &mut store,
            &serde_json::json!({
                "name": "mmcg_change_class",
                "arguments": { "file": rel }
            }),
        )
        .unwrap();
        let structural = unwrap_content(&structural_env);
        assert_eq!(structural["class"], "structural");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
