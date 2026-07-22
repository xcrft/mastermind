//! MCP server — JSON-RPC over stdio.
//!
//! Implements `initialize`, `tools/list`, `tools/call` of the Model Context
//! Protocol (https://modelcontextprotocol.io). To add a tool: one [`ToolDef`]
//! entry in [`TOOLS`] with a `schema` fn and a `handler` fn.

use crate::queries;
use crate::store::{InterruptSource, Store, WorkBudget};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use std::sync::{mpsc, Arc, Mutex};

const CURRENT_PROTOCOL_VERSION: &str = "2025-11-25";
const LEGACY_PROTOCOL_VERSION: &str = "2024-11-05";
const MCP_FRAME_MAX: usize = 1024 * 1024;
const MCP_RESULT_MAX: usize = 8 * 1024 * 1024;
const SERVER_NOT_INITIALIZED: i32 = -32002;
const SCRATCHPAD_BODY_MAX: usize = 8 * 1024;

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProtocolVersion {
    Legacy,
    Current,
}

impl ProtocolVersion {
    fn negotiate(requested: &str) -> Self {
        if requested == LEGACY_PROTOCOL_VERSION {
            Self::Legacy
        } else {
            Self::Current
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => LEGACY_PROTOCOL_VERSION,
            Self::Current => CURRENT_PROTOCOL_VERSION,
        }
    }

    fn is_current(self) -> bool {
        self == Self::Current
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SessionState {
    #[default]
    Cold,
    Negotiated(ProtocolVersion),
    Ready(ProtocolVersion),
}

impl SessionState {
    fn protocol(self) -> Option<ProtocolVersion> {
        match self {
            Self::Cold => None,
            Self::Negotiated(version) | Self::Ready(version) => Some(version),
        }
    }

    fn is_ready(self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

enum Frame {
    Line(String),
    InvalidUtf8,
    TooLarge(usize),
}

#[derive(Debug)]
enum HandlerError {
    InvalidArguments(String),
    Internal { class: &'static str },
}

impl HandlerError {
    fn internal(class: &'static str, _error: impl std::fmt::Display) -> Self {
        Self::Internal { class }
    }
}

#[derive(Debug)]
enum ToolCallError {
    InvalidParams(String),
    Internal { class: &'static str },
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

/// One MCP tool. `schema` returns the `tools/list` JSON entry; `handler` runs
/// the call and returns the raw payload ([`handle_tools_call`] adds the content
/// envelope).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ToolBehavior {
    ReadOnly,
    AdditiveNonIdempotent,
}

struct ToolDef {
    name: &'static str,
    schema: fn() -> Value,
    handler: fn(&mut Store, &Value) -> Result<Value, HandlerError>,
    behavior: ToolBehavior,
}

const fn read_only_tool(
    name: &'static str,
    schema: fn() -> Value,
    handler: fn(&mut Store, &Value) -> Result<Value, HandlerError>,
) -> ToolDef {
    ToolDef {
        name,
        schema,
        handler,
        behavior: ToolBehavior::ReadOnly,
    }
}

const fn additive_tool(
    name: &'static str,
    schema: fn() -> Value,
    handler: fn(&mut Store, &Value) -> Result<Value, HandlerError>,
) -> ToolDef {
    ToolDef {
        name,
        schema,
        handler,
        behavior: ToolBehavior::AdditiveNonIdempotent,
    }
}

/// Authoritative tool list — order matches `tools/list` output.
static TOOLS: &[ToolDef] = &[
    read_only_tool("mmcg_search", schema_search, handle_search),
    read_only_tool("mmcg_callers", schema_callers, handle_callers),
    read_only_tool("mmcg_callees", schema_callees, handle_callees),
    read_only_tool("mmcg_impact", schema_impact, handle_impact),
    read_only_tool(
        "mmcg_symbols_in_file",
        schema_symbols_in_file,
        handle_symbols_in_file,
    ),
    read_only_tool("mmcg_outline", schema_outline, handle_outline),
    read_only_tool("mmcg_files", schema_files, handle_files),
    read_only_tool("mmcg_imports", schema_imports, handle_imports),
    read_only_tool("mmcg_imported_by", schema_imported_by, handle_imported_by),
    read_only_tool(
        "mmcg_unreferenced",
        schema_unreferenced,
        handle_unreferenced,
    ),
    read_only_tool("mmcg_api_surface", schema_api_surface, handle_api_surface),
    read_only_tool(
        "mmcg_symbols_changed_since",
        schema_symbols_changed_since,
        handle_symbols_changed_since,
    ),
    read_only_tool(
        "mmcg_dependency_cycles",
        schema_dependency_cycles,
        handle_dependency_cycles,
    ),
    read_only_tool("mmcg_tasks", schema_tasks, handle_tasks),
    read_only_tool("mmcg_history", schema_history, handle_history),
    read_only_tool("mmcg_centrality", schema_centrality, handle_centrality),
    read_only_tool("mmcg_map", schema_map, handle_map),
    read_only_tool(
        "mmcg_change_impact",
        schema_change_impact,
        handle_change_impact,
    ),
    read_only_tool("mmcg_test_impact", schema_test_impact, handle_test_impact),
    read_only_tool(
        "mmcg_recent_changes",
        schema_recent_changes,
        handle_recent_changes,
    ),
    read_only_tool("mmcg_status", schema_status, handle_status),
    additive_tool(
        "mmcg_scratchpad_append",
        schema_scratchpad_append,
        handle_scratchpad_append,
    ),
    read_only_tool(
        "mmcg_scratchpad_read",
        schema_scratchpad_read,
        handle_scratchpad_read,
    ),
    read_only_tool(
        "mmcg_change_class",
        schema_change_class,
        handle_change_class,
    ),
];

/// `Stdin` itself is `Send + 'static` (unlike its lock guard) — wrapping it
/// this way lets the `serve_io` reader thread own an input source without
/// holding a non-`Send` lock guard across the thread boundary. Each `read`
/// re-locks momentarily; the underlying buffered reader is process-global, so
/// this is equivalent to holding the lock for the whole call.
struct StdinSource(io::Stdin);

impl Read for StdinSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.lock().read(buf)
    }
}

/// Run as an MCP stdio server. Blocks until stdin closes.
pub fn serve(mut store: Store) -> io::Result<()> {
    let input = io::BufReader::new(StdinSource(io::stdin()));
    let stdout = io::stdout();
    serve_io(&mut store, input, stdout.lock())
}

/// Frames read from `input` on a dedicated background thread and delivered to
/// the main loop over an `mpsc` channel — the reader thread intercepts MCP
/// cancel notifications (`notifications/cancelled`, and legacy
/// `$/cancelRequest`) out of band, so a cancel arriving while the main thread
/// is blocked inside a running query still takes effect promptly. The main
/// loop still consumes frames and answers requests one at a time — the
/// concurrency model is otherwise unchanged; a `ping` sent while a query runs
/// still waits, bounded by the query's work budget rather than fixed here.
fn serve_io<R, W>(store: &mut Store, input: R, mut output: W) -> io::Result<()>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    enum ReaderMsg {
        Frame(Frame),
        Eof,
        Err(io::Error),
    }

    // One mutex owns the in-flight request id: set on request start, cleared
    // on finish, and the reader's cancel-id equality check runs under the
    // same lock — closing the race where `cancel(N)` lands after N finished
    // and would otherwise abort N+1.
    let in_flight: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let cancel_handle = store.cancel_handle();
    let (frame_tx, frame_rx) = mpsc::channel::<ReaderMsg>();

    let reader_in_flight = Arc::clone(&in_flight);
    let reader = std::thread::spawn(move || {
        let mut input = input;
        loop {
            let outcome = read_frame(&mut input);
            let msg = match outcome {
                Ok(Some(Frame::Line(line))) => {
                    if let Some(target_id) = parse_cancel_target(&line) {
                        let guard = reader_in_flight
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if guard.as_ref() == Some(&target_id) {
                            cancel_handle.cancel();
                        }
                        drop(guard);
                        continue;
                    }
                    ReaderMsg::Frame(Frame::Line(line))
                }
                Ok(Some(other)) => ReaderMsg::Frame(other),
                Ok(None) => ReaderMsg::Eof,
                Err(e) => ReaderMsg::Err(e),
            };
            let stop = matches!(msg, ReaderMsg::Eof | ReaderMsg::Err(_));
            if frame_tx.send(msg).is_err() || stop {
                return;
            }
        }
    });

    let mut state = SessionState::Cold;
    while let Ok(msg) = frame_rx.recv() {
        let frame = match msg {
            ReaderMsg::Eof => break,
            ReaderMsg::Err(e) => {
                let _ = reader.join();
                return Err(e);
            }
            ReaderMsg::Frame(frame) => frame,
        };
        let response = match frame {
            Frame::Line(line) if line.trim().is_empty() => continue,
            Frame::Line(line) => {
                let trimmed = line.trim();
                // SQLite's documented no-op-when-idle semantics mean a cancel
                // that races ahead of `in_flight` being set is harmless — the
                // reader will simply see no matching in-flight id and skip.
                let request_id = peek_request_id(trimmed);
                if let Some(id) = &request_id {
                    *in_flight
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id.clone());
                }
                let response = handle_line(&mut state, store, trimmed);
                if request_id.is_some() {
                    *in_flight
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
                response
            }
            Frame::InvalidUtf8 => {
                eprintln!("[mmcg] parse error class=invalid_utf8");
                Some(err(Value::Null, -32700, "Parse error".into()))
            }
            Frame::TooLarge(size) => {
                eprintln!("[mmcg] protocol error class=frame_too_large bytes={size}");
                write_response(
                    &mut output,
                    &err(Value::Null, -32600, "Request frame exceeds 1 MiB".into()),
                )?;
                return Ok(());
            }
        };
        if let Some(response) = response {
            write_response(&mut output, &response)?;
        }
    }
    Ok(())
}

/// Cheap, best-effort extraction of a request's `id` for in-flight tracking —
/// separate from `handle_line`'s own full validation, which still runs and
/// produces the authoritative error response for malformed input.
fn peek_request_id(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value.get("id")?.clone();
    valid_request_id(&id).then_some(id)
}

/// Extract the target request id from an MCP cancel notification
/// (`notifications/cancelled`, `params.requestId`) or the legacy LSP-style
/// `$/cancelRequest` (`params.id`). Any other line — including ordinary
/// requests, which always carry a top-level `id` and are therefore never
/// cancel notifications themselves — returns `None`.
fn parse_cancel_target(line: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("id").is_some() {
        return None;
    }
    let method = value.get("method")?.as_str()?;
    let params = value.get("params")?;
    match method {
        "notifications/cancelled" => params.get("requestId").cloned(),
        "$/cancelRequest" => params.get("id").cloned(),
        _ => None,
    }
}

fn read_frame<R: BufRead>(input: &mut R) -> io::Result<Option<Frame>> {
    let mut bytes = Vec::new();
    loop {
        let (newline, consumed) = {
            let available = input.fill_buf()?;
            if available.is_empty() {
                if bytes.is_empty() {
                    return Ok(None);
                }
                break;
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |index| index + 1);
            let total = bytes.len().saturating_add(consumed);
            if total > MCP_FRAME_MAX {
                return Ok(Some(Frame::TooLarge(total)));
            }
            bytes.extend_from_slice(&available[..consumed]);
            (newline, consumed)
        };
        input.consume(consumed);
        if newline.is_some() {
            break;
        }
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(Some(match String::from_utf8(bytes) {
        Ok(line) => Frame::Line(line),
        Err(_) => Frame::InvalidUtf8,
    }))
}

fn write_response<W: Write>(output: &mut W, response: &JsonRpcResponse) -> io::Result<()> {
    let encoded = serde_json::to_string(response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writeln!(output, "{encoded}")?;
    output.flush()
}

fn handle_line(state: &mut SessionState, store: &mut Store, line: &str) -> Option<JsonRpcResponse> {
    let raw: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("[mmcg] parse error class=invalid_json detail={error}");
            return Some(err(Value::Null, -32700, "Parse error".into()));
        }
    };
    let Some(object) = raw.as_object() else {
        return Some(err(Value::Null, -32600, "Invalid Request".into()));
    };
    let id = match object.get("id") {
        None => None,
        Some(value) if valid_request_id(value) => Some(value.clone()),
        Some(_) => return Some(err(Value::Null, -32600, "Invalid Request".into())),
    };
    let request: JsonRpcRequest = match serde_json::from_value::<JsonRpcRequest>(raw) {
        Ok(request) if request.jsonrpc == "2.0" => request,
        _ => {
            return Some(err(
                id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request".into(),
            ))
        }
    };
    match id {
        Some(id) => Some(handle_request(
            state,
            store,
            &request.method,
            &request.params,
            id,
        )),
        None => {
            handle_notification(state, &request.method);
            None
        }
    }
}

fn valid_request_id(value: &Value) -> bool {
    match value {
        Value::String(_) => true,
        Value::Number(number) => number.as_i64().is_some() || number.as_u64().is_some(),
        _ => false,
    }
}

fn handle_notification(state: &mut SessionState, method: &str) {
    if method == "notifications/initialized" {
        if let SessionState::Negotiated(version) = *state {
            *state = SessionState::Ready(version);
        }
    }
}

fn handle_request(
    state: &mut SessionState,
    store: &mut Store,
    method: &str,
    params: &Value,
    id: Value,
) -> JsonRpcResponse {
    if method == "initialize" {
        return match initialize_result(state, params) {
            Ok(result) => ok(id, result),
            Err(message) => err(id, -32602, message),
        };
    }
    if method == "ping" {
        return ok(id, json!({}));
    }
    if !state.is_ready() {
        return err(id, SERVER_NOT_INITIALIZED, "Server not initialized".into());
    }
    match method {
        "tools/list" => ok(id, tools_list(state.protocol().expect("ready protocol"))),
        "tools/call" => {
            match handle_tools_call(state.protocol().expect("ready protocol"), store, params) {
                Ok(result) => ok(id, result),
                Err(ToolCallError::InvalidParams(message)) => err(id, -32602, message),
                Err(ToolCallError::Internal { class }) => {
                    eprintln!("[mmcg] tool error class={class}");
                    err(id, -32603, "Internal tool error".into())
                }
            }
        }
        _ => err(id, -32601, "Method not found".into()),
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

fn initialize_result(state: &mut SessionState, params: &Value) -> Result<Value, String> {
    if *state != SessionState::Cold {
        return Err("Connection is already initialized".into());
    }
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| "Invalid initialize params".to_string())?;
    let capabilities_valid = params
        .get("capabilities")
        .and_then(Value::as_object)
        .is_some();
    let client_info = params.get("clientInfo").and_then(Value::as_object);
    let client_info_valid = client_info
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .is_some()
        && client_info
            .and_then(|value| value.get("version"))
            .and_then(Value::as_str)
            .is_some();
    if !capabilities_valid || !client_info_valid {
        return Err("Invalid initialize params".into());
    }
    let version = ProtocolVersion::negotiate(requested);
    *state = SessionState::Negotiated(version);
    Ok(json!({
        "protocolVersion": version.as_str(),
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "mmcg", "version": env!("CARGO_PKG_VERSION") }
    }))
}

fn tools_list(version: ProtocolVersion) -> Value {
    let tools = TOOLS
        .iter()
        .map(|tool| {
            let mut schema = (tool.schema)();
            if version.is_current() {
                let annotations = match tool.behavior {
                    ToolBehavior::ReadOnly => json!({ "readOnlyHint": true }),
                    ToolBehavior::AdditiveNonIdempotent => json!({
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false
                    }),
                };
                schema
                    .as_object_mut()
                    .expect("tool schema object")
                    .insert("annotations".into(), annotations);
            }
            schema
        })
        .collect::<Vec<_>>();
    json!({ "tools": tools })
}

fn handle_tools_call(
    version: ProtocolVersion,
    store: &mut Store,
    params: &Value,
) -> Result<Value, ToolCallError> {
    handle_tools_call_inner(version, store, params, None)
}

fn handle_tools_call_inner(
    version: ProtocolVersion,
    store: &mut Store,
    params: &Value,
    impact_engine: Option<&queries::ImpactEngine<'_>>,
) -> Result<Value, ToolCallError> {
    let params = params
        .as_object()
        .ok_or_else(|| ToolCallError::InvalidParams("Invalid tools/call params".into()))?;
    let tool_name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolCallError::InvalidParams("Invalid tools/call params".into()))?;
    let arguments = match params.get("arguments") {
        None => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Err(ToolCallError::InvalidParams(
                "Invalid tools/call params".into(),
            ))
        }
    };
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == tool_name)
        .ok_or_else(|| ToolCallError::InvalidParams("Unknown tool".into()))?;

    // Every tool dispatch runs inside the work-budget guard — the single
    // owner of the connection's progress handler. A pre-expired budget (e.g.
    // a zero budget installed by a test) short-circuits before the handler
    // ever runs, so coverage doesn't depend on a query being slow enough to
    // trip the in-flight progress-handler check.
    let budget = store.default_work_budget();
    let already_expired = store.push_work_budget(budget);
    let handled = if already_expired {
        None
    } else {
        Some(match (tool_name, impact_engine) {
            ("mmcg_change_impact", Some(engine)) => {
                handle_change_impact_with_engine(store, &arguments, engine)
            }
            ("mmcg_test_impact", Some(engine)) => {
                handle_test_impact_with_engine(store, &arguments, engine)
            }
            _ => (tool.handler)(store, &arguments),
        })
    };
    let interrupt = store.take_interrupt_source();
    store.pop_work_budget();

    match handled {
        Some(Ok(payload)) => tool_result(version, payload, false),
        Some(Err(handler_error)) => match interrupt {
            Some(InterruptSource::Cancel) => tool_result(version, cancelled_payload(), true),
            Some(InterruptSource::Budget) => tool_result(version, work_limit_payload(budget), true),
            None => match handler_error {
                HandlerError::InvalidArguments(message) => {
                    tool_result(version, json!({ "error": message }), true)
                }
                HandlerError::Internal { class } => Err(ToolCallError::Internal { class }),
            },
        },
        None => tool_result(version, work_limit_payload(budget), true),
    }
}

/// Structured `work_limit_exceeded` tool error — mirrors the `change_impact`
/// work-limit metadata precedent (queries.rs `precision_notes` /
/// `work_limited_collection`).
fn work_limit_payload(budget: WorkBudget) -> Value {
    json!({
        "code": "work_limit_exceeded",
        "budget_ms": budget.deadline.map(|d| d.as_millis() as u64),
        "guidance": "narrow scope (subdirectory path, smaller depth, language filter) or raise MMCG_QUERY_BUDGET_MS"
    })
}

/// Structured `cancelled` tool error — distinct from `work_limit_exceeded` so
/// a client cancel is never conflated with a budget expiry.
fn cancelled_payload() -> Value {
    json!({
        "code": "cancelled",
        "guidance": "the request was cancelled by the client before it completed"
    })
}

#[cfg(test)]
fn handle_tools_call_with_impact_engine(
    version: ProtocolVersion,
    store: &mut Store,
    params: &Value,
    impact_engine: &queries::ImpactEngine<'_>,
) -> Result<Value, ToolCallError> {
    handle_tools_call_inner(version, store, params, Some(impact_engine))
}

fn tool_result(
    version: ProtocolVersion,
    payload: Value,
    is_error: bool,
) -> Result<Value, ToolCallError> {
    let text = serde_json::to_string(&payload).map_err(|_| ToolCallError::Internal {
        class: "serialize_tool_payload",
    })?;
    if text.len() > MCP_RESULT_MAX {
        return small_tool_error(version, "Tool result exceeds 8 MiB; narrow the query");
    }
    let structured = if version.is_current() {
        Some(match payload {
            Value::Object(object) => Value::Object(object),
            Value::Array(entries) => json!({ "entries": entries }),
            value => json!({ "value": value }),
        })
    } else {
        None
    };
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    });
    if let Some(structured) = structured {
        result
            .as_object_mut()
            .expect("tool result object")
            .insert("structuredContent".into(), structured);
    }
    Ok(result)
}

fn small_tool_error(
    version: ProtocolVersion,
    message: &'static str,
) -> Result<Value, ToolCallError> {
    let payload = json!({ "error": message });
    let text = serde_json::to_string(&payload).expect("static error serializes");
    let mut result = json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true
    });
    if version.is_current() {
        result
            .as_object_mut()
            .expect("tool result object")
            .insert("structuredContent".into(), payload);
    }
    Ok(result)
}

fn str_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, HandlerError> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| HandlerError::InvalidArguments(format!("Invalid argument: {name}")))
}

fn opt_str_arg<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name).and_then(|v| v.as_str())
}

fn opt_bool_arg(args: &Value, name: &str) -> Option<bool> {
    args.get(name).and_then(|v| v.as_bool())
}

fn schema_search() -> Value {
    json!({
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
    })
}

fn schema_callers() -> Value {
    json!({
        "name": "mmcg_callers",
        "description": "List symbols that reference the given name. Matches both leaf names (`obj.foo()` → 'foo') AND type prefixes (`SessionStore::new()` → 'SessionStore'). Use before editing to assess blast radius. Pass `language` to filter against monorepo collisions. Pass `edge_kind` (default 'calls') to switch between call/import/inherit edges. Result carries `name_collision` — how many definitions share this name; a value > 1 means the caller set pools across same-named symbols (edges resolve by name), so trust it less.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Name or type to look up" },
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] },
                "edge_kind": { "type": "string", "enum": ["calls", "imports", "inherits"], "default": "calls", "description": "Which kind of incoming edge to consider" }
            },
            "required": ["name"]
        }
    })
}

fn schema_callees() -> Value {
    json!({
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
    })
}

fn schema_impact() -> Value {
    json!({
        "name": "mmcg_impact",
        "description": "Transitive callers of the symbol up to max_depth. Use for blast-radius analysis on widely-called functions. Matches by name OR type prefix (like mmcg_callers). Result carries `name_collision`: a value > 1 means the blast radius is pooled across same-named definitions and over-approximates the real reach — verify before acting on the number. Bounded at 5,001 rows: above that, `truncated: true` is returned alongside the (partial) `impact` list — narrow `max_depth` or add a `language` filter to see the rest.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 10, "default": 2 },
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] }
            },
            "required": ["name"]
        }
    })
}

fn schema_symbols_in_file() -> Value {
    json!({
        "name": "mmcg_symbols_in_file",
        "description": "List every symbol (function, class, method, struct, etc.) defined in a file, in source order. Faster than Read for getting a structural overview.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
            },
            "required": ["file"]
        }
    })
}

fn schema_outline() -> Value {
    json!({
        "name": "mmcg_outline",
        "description": "Return the symbol tree of a file (classes / impls own their methods; modules own top-level functions). One call replaces a search + multiple symbols_in_file lookups. Useful for refactor planning or jumping to a symbol by structure.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
            },
            "required": ["file"]
        }
    })
}

fn schema_files() -> Value {
    json!({
        "name": "mmcg_files",
        "description": "List indexed files. Optionally filter by path prefix and/or language.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prefix": { "type": "string", "description": "Optional path prefix" },
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" }
            }
        }
    })
}

fn schema_imports() -> Value {
    json!({
        "name": "mmcg_imports",
        "description": "List names imported by a file. Useful for understanding a file's dependencies.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Relative file path (as it appears in the index)" }
            },
            "required": ["file"]
        }
    })
}

fn schema_imported_by() -> Value {
    json!({
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
    })
}

fn schema_unreferenced() -> Value {
    json!({
        "name": "mmcg_unreferenced",
        "description": "Symbols that no edge references (no caller, no importer). Dead-code candidates. Optional `kind` / `language` filters. WARNING: false-positives for entry points (main, framework-registered handlers), dynamic dispatch / reflection, and cross-language calls (e.g. TS subprocess into Python). Review hits manually before deleting.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "kind": { "type": "string", "description": "Filter by symbol kind (function / class / method / struct / etc.)" },
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] }
            }
        }
    })
}

fn schema_api_surface() -> Value {
    json!({
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
    })
}

fn schema_symbols_changed_since() -> Value {
    json!({
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
    })
}

fn schema_dependency_cycles() -> Value {
    json!({
        "name": "mmcg_dependency_cycles",
        "description": "Detect circular imports — strongly-connected components in the file-level import graph. Returns each cycle as a list of files. Pre-merge guard: 'does this PR introduce a new cycle?'. Architectural hygiene: 'what cycles already exist?'. Edges are resolved by leaf-name match (over-approximating — two unrelated symbols sharing a name produce a cross-edge; verify before acting). Set `min_size` higher to hide trivial A↔B and surface only larger structural issues. Work-capped: above a large import-graph size, `truncated: true` is returned with an empty `cycles` list — the true cycle set is incomplete and possibly inaccurate, not merely 'more available'; narrow with `language` and retry.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"], "description": "Optional language filter" },
                "min_size": { "type": "integer", "minimum": 2, "maximum": 100, "default": 2, "description": "Smallest SCC to report. 2 = any cycle. 3 hides trivial A↔B pairs." }
            }
        }
    })
}

fn schema_tasks() -> Value {
    json!({
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
    })
}

fn schema_history() -> Value {
    json!({
        "name": "mmcg_history",
        "description": "Search durable project history across active and archived CONTEXT files, canonical task specs, executor reports, audits, release notes, and lessons. Candidate lessons are unresolved audit signals, not active guidance. Returns observed FTS matches plus skipped/truncated signals; ranking and co-occurrence do not establish causality or correctness. The returned Markdown paths remain the source of truth, and callers should re-index after Markdown changes.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "FTS5 MATCH query (e.g. 'rate limit', 'auth OR session', '\"token bucket\"')" },
                "kind": { "type": "string", "enum": ["context", "lesson", "task_spec", "executor_report", "audit", "release_notes"], "description": "Optional exact artifact-kind filter" },
                "top": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10, "description": "How many observed matches to return" }
            },
            "required": ["query"]
        }
    })
}

fn schema_centrality() -> Value {
    json!({
        "name": "mmcg_centrality",
        "description": "Rank symbols by in-degree (distinct callers, matched by name OR type prefix). Pre-flight 'where is the gravity' tool — top hits are the codebase's structural load-bearing points. Filter by path `prefix` (e.g. 'src/auth/') and/or `kind` (function, class, method…). Higher `top` reveals the long tail; default 20 covers most planning needs. Each hit carries `name_collision` — how many definitions share the leaf name; a high value means the in-degree is inflated by same-named call sites (edges resolve syntactically), not concentrated on this one symbol.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "prefix": { "type": "string", "description": "Optional path prefix to limit ranking scope (e.g. 'src/auth/'). LIKE-matched." },
                "language": { "type": "string", "enum": ["python", "typescript", "tsx", "javascript", "rust", "csharp", "go", "java", "php", "cpp"] },
                "kind": { "type": "string", "description": "Optional kind filter (function, class, method, struct, etc.)" },
                "top": { "type": "integer", "minimum": 1, "maximum": 200, "default": 20, "description": "How many results to return" }
            }
        }
    })
}

fn schema_map() -> Value {
    json!({
        "name": "mmcg_map",
        "description": "Build a bounded deterministic architecture briefing for an indexed repository scope. Entry points are heuristic; graph precision and truncation are explicit in the result.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": ".", "description": "Repository-relative file or directory scope" },
                "depth": { "type": "integer", "minimum": 1, "maximum": 6, "default": 2 },
                "top": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
                "production_only": { "type": "boolean", "default": false, "description": "Exclude tests, fixtures, examples, generated code, and vendored dependencies" }
            }
        }
    })
}

fn impact_input_schema(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": {
                "since": { "type": "string", "description": "Git ref used as the baseline" },
                "root": { "type": "string", "description": "Repository root or a subdirectory within the indexed repository" },
                "depth": { "type": "integer", "minimum": 1, "maximum": 5, "default": 3 },
                "top": { "type": "integer", "minimum": 1, "maximum": 500, "default": 100 }
            },
            "required": ["since"]
        }
    })
}

fn schema_change_impact() -> Value {
    impact_input_schema(
        "mmcg_change_impact",
        "Deterministic schema-v1 analysis of working-tree changes, callers, component crossings, and candidate tests.",
    )
}

fn schema_test_impact() -> Value {
    impact_input_schema(
        "mmcg_test_impact",
        "Exact candidate-test projection of mmcg_change_impact. Focused candidates never replace the full repository gate.",
    )
}

fn schema_recent_changes() -> Value {
    json!({
        "name": "mmcg_recent_changes",
        "description": "Files re-indexed within a recent time window (per the watcher's `indexed_at` mtime). Useful when investigating a recent incident or asking 'what changed in the last hour?'. Pass `since` as a short duration string: 30s / 10m / 2h / 1d.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "since": { "type": "string", "description": "Time window — e.g. '2h', '30m', '1d'" }
            },
            "required": ["since"]
        }
    })
}

fn schema_status() -> Value {
    json!({
        "name": "mmcg_status",
        "description": "Show index health — file count, symbol count, db path, extractor-contract compatibility, and `stale_files`: the number of added, deleted, or newer indexable source paths relative to their stored snapshot (capped at 100). If `extractor_contract_current` is false or `stale_files` is non-zero, re-index before trusting structural answers.",
        "inputSchema": { "type": "object", "properties": {} }
    })
}

fn schema_scratchpad_append() -> Value {
    json!({
        "name": "mmcg_scratchpad_append",
        "description": "Append a one-line intent / note / handoff to the cross-agent scratchpad. Live in-session channel between Mastermind subagents (planner → executor → auditor). Use to hand off context the next agent needs without polluting the chat or the spec. Persists in `.mastermind/mmcg.db` (additive table, gitignored). Reviewed durable knowledge belongs in CONTEXT.md or `_lessons.md`, not the scratchpad. Body capped at 8 KiB — scratchpad is for one-liners, not blob dumps.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "agent": { "type": "string", "description": "Agent identifier — conventionally `planner` / `executor` / `auditor` / `critic`, but freeform." },
                "kind": { "type": "string", "description": "Entry kind — conventionally `intent` / `note` / `handoff` / `risk`, but freeform." },
                "body": { "type": "string", "description": "The one-line content. ≤ 8 KiB." }
            },
            "required": ["agent", "kind", "body"]
        }
    })
}

fn schema_scratchpad_read() -> Value {
    json!({
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
    })
}

fn schema_change_class() -> Value {
    json!({
        "name": "mmcg_change_class",
        "description": "Classify a file's last change as `structural` (signatures, edges, or imports changed), `cosmetic` (only line numbers / whitespace / comments differ), or `first-seen` (file never indexed). Pre-edit signal for planner and auditor: a diff of 20 files where 17 are cosmetic-only has a much smaller blast radius than its raw line count suggests. Backed by a deterministic FNV-1a 64-bit hash of the file's parsed structural shape — same source on any machine yields the same fingerprint.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "Path relative to the project root (e.g. `src/auth/login.ts`)." }
            },
            "required": ["file"]
        }
    })
}

fn handle_search(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let name = str_arg(args, "name")?;
    let kind = opt_str_arg(args, "kind");
    let language = opt_str_arg(args, "language");
    let collapse = opt_bool_arg(args, "collapse_partials").unwrap_or(true);
    let r = queries::search(store, name, kind, language, collapse)
        .map_err(|error| HandlerError::internal("search_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_callers(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let name = str_arg(args, "name")?;
    let language = opt_str_arg(args, "language");
    let edge_kind = opt_str_arg(args, "edge_kind");
    let r = queries::callers(store, name, language, edge_kind)
        .map_err(|error| HandlerError::internal("callers_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_callees(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let name = str_arg(args, "name")?;
    let language = opt_str_arg(args, "language");
    let edge_kind = opt_str_arg(args, "edge_kind");
    let r = queries::callees(store, name, language, edge_kind)
        .map_err(|error| HandlerError::internal("callees_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_impact(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let name = str_arg(args, "name")?;
    let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
    let language = opt_str_arg(args, "language");
    let r = queries::impact(store, name, max_depth, language)
        .map_err(|error| HandlerError::internal("impact_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_symbols_in_file(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let file = str_arg(args, "file")?;
    let r = queries::symbols_in_file(store, file)
        .map_err(|error| HandlerError::internal("symbols_in_file_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_outline(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let file = str_arg(args, "file")?;
    let r = queries::outline(store, file)
        .map_err(|error| HandlerError::internal("outline_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_files(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let prefix = opt_str_arg(args, "prefix");
    let language = opt_str_arg(args, "language");
    let r = queries::files(store, prefix, language)
        .map_err(|error| HandlerError::internal("files_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_imports(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let file = str_arg(args, "file")?;
    let r = queries::imports(store, file)
        .map_err(|error| HandlerError::internal("imports_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_imported_by(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let query = str_arg(args, "query").or_else(|_| str_arg(args, "name"))?;
    let match_kind = opt_str_arg(args, "match").unwrap_or("name");
    let language = opt_str_arg(args, "language");
    let r = queries::imported_by(store, query, match_kind, language)
        .map_err(|error| HandlerError::internal("imported_by_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_unreferenced(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let kind = opt_str_arg(args, "kind");
    let language = opt_str_arg(args, "language");
    let r = queries::unreferenced(store, kind, language)
        .map_err(|error| HandlerError::internal("unreferenced_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_api_surface(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let prefix = str_arg(args, "prefix")?;
    let language = opt_str_arg(args, "language");
    let r = queries::api_surface(store, prefix, language)
        .map_err(|error| HandlerError::internal("api_surface_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn changed_since_root(
    root_arg: Option<&str>,
    db_path: &std::path::Path,
) -> Result<std::path::PathBuf, HandlerError> {
    let root = match root_arg {
        Some(s) => std::path::PathBuf::from(s),
        None => {
            let db = db_path
                .canonicalize()
                .map_err(|error| HandlerError::internal("changed_since_root", error))?;
            db.parent()
                .and_then(|d| d.parent())
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        }
    };
    root.canonicalize()
        .map_err(|error| HandlerError::internal("changed_since_root", error))
}

fn handle_symbols_changed_since(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let git_ref = str_arg(args, "git_ref")?;
    let root = changed_since_root(opt_str_arg(args, "root"), store.db_path())?;
    let diff = queries::symbols_changed_since(store, &root, git_ref)
        .map_err(|error| HandlerError::internal("git_diff", error))?;
    serde_json::to_value(diff).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_dependency_cycles(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let language = opt_str_arg(args, "language");
    let min_size = args
        .get("min_size")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(2)
        .clamp(2, 100);
    let r = queries::dependency_cycles(store, language, min_size)
        .map_err(|error| HandlerError::internal("dependency_cycles_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_tasks(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let query = str_arg(args, "query")?;
    let top = args
        .get("top")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(10)
        .clamp(1, 50);
    let r = queries::tasks(store, query, top)
        .map_err(|error| HandlerError::internal("tasks_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_history(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let query = str_arg(args, "query")?;
    let kind = opt_str_arg(args, "kind");
    let top = args
        .get("top")
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(10)
        .clamp(1, 50);
    let response = queries::history(store, query, kind, top)
        .map_err(|error| HandlerError::internal("history_query", error))?;
    serde_json::to_value(response)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_centrality(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let prefix = opt_str_arg(args, "prefix");
    let language = opt_str_arg(args, "language");
    let kind = opt_str_arg(args, "kind");
    let top = args
        .get("top")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(20)
        .clamp(1, 200);
    let r = queries::centrality(store, prefix, language, kind, top)
        .map_err(|error| HandlerError::internal("centrality_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_map(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let path = match args.get("path") {
        None => ".",
        Some(value) => value
            .as_str()
            .ok_or_else(|| HandlerError::InvalidArguments("Invalid argument: path".into()))?,
    };
    queries::normalize_map_path(path)
        .map_err(|_| HandlerError::InvalidArguments("Invalid argument: path".into()))?;
    let depth = match args.get("depth") {
        None => 2,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=6).contains(value))
            .ok_or_else(|| HandlerError::InvalidArguments("Invalid argument: depth".into()))?,
    };
    let top = match args.get("top") {
        None => 20,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=100).contains(value))
            .ok_or_else(|| HandlerError::InvalidArguments("Invalid argument: top".into()))?,
    };
    let production_only = match args.get("production_only") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| {
            HandlerError::InvalidArguments("Invalid argument: production_only".into())
        })?,
    };
    let result =
        queries::project_map_with_options(store, path, depth as u8, top as u32, production_only)
            .map_err(|error| HandlerError::internal("project_map_query", error))?;
    serde_json::to_value(result)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn impact_arguments(
    store: &Store,
    args: &Value,
) -> Result<(String, std::path::PathBuf, u32, usize), HandlerError> {
    let since = str_arg(args, "since")?.to_string();
    let root = match args.get("root") {
        None => changed_since_root(None, store.db_path())?,
        Some(Value::String(value)) => std::path::PathBuf::from(value)
            .canonicalize()
            .map_err(|_| HandlerError::InvalidArguments("root_mismatch".to_string()))?,
        Some(_) => {
            return Err(HandlerError::InvalidArguments(
                "Invalid argument: root".to_string(),
            ))
        }
    };
    let depth = match args.get("depth") {
        None => 3,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=5).contains(value))
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| HandlerError::InvalidArguments("Invalid argument: depth".into()))?,
    };
    let top = match args.get("top") {
        None => 100,
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=500).contains(value))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| HandlerError::InvalidArguments("Invalid argument: top".into()))?,
    };
    Ok((since, root, depth, top))
}

fn run_change_impact(
    store: &Store,
    args: &Value,
) -> Result<queries::ChangeImpactResponse, HandlerError> {
    run_change_impact_with_engine(store, args, &queries::change_impact)
}

fn run_change_impact_with_engine(
    store: &Store,
    args: &Value,
    impact_engine: &queries::ImpactEngine<'_>,
) -> Result<queries::ChangeImpactResponse, HandlerError> {
    let (since, root, depth, top) = impact_arguments(store, args)?;
    impact_engine(store, &root, &since, depth, top)
        .map_err(|error| HandlerError::InvalidArguments(error.code().to_string()))
}

fn handle_change_impact(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let response = run_change_impact(store, args)?;
    serde_json::to_value(response)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_change_impact_with_engine(
    store: &mut Store,
    args: &Value,
    impact_engine: &queries::ImpactEngine<'_>,
) -> Result<Value, HandlerError> {
    let response = run_change_impact_with_engine(store, args, impact_engine)?;
    serde_json::to_value(response)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn test_impact_projection(
    response: &queries::ChangeImpactResponse,
) -> Result<Value, serde_json::Error> {
    let value = serde_json::to_value(response)?;
    Ok(json!({
        "schema_version": value["schema_version"],
        "baseline": value["baseline"],
        "scope": value["scope"],
        "changes": value["changes"],
        "tests": value["tests"],
        "limits": value["limits"],
        "precision_notes": value["precision_notes"],
    }))
}

fn handle_test_impact(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let response = run_change_impact(store, args)?;
    test_impact_projection(&response)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_test_impact_with_engine(
    store: &mut Store,
    args: &Value,
    impact_engine: &queries::ImpactEngine<'_>,
) -> Result<Value, HandlerError> {
    let response = run_change_impact_with_engine(store, args, impact_engine)?;
    test_impact_projection(&response)
        .map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_recent_changes(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let since = str_arg(args, "since")?;
    queries::parse_duration(since)
        .map_err(|_| HandlerError::InvalidArguments("Invalid argument: since".into()))?;
    let r = queries::recent_changes(store, since)
        .map_err(|error| HandlerError::internal("recent_changes_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_status(store: &mut Store, _args: &Value) -> Result<Value, HandlerError> {
    let r =
        queries::status(store).map_err(|error| HandlerError::internal("status_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_scratchpad_append(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let agent = str_arg(args, "agent")?;
    let kind = str_arg(args, "kind")?;
    let body = str_arg(args, "body")?;
    if body.len() > SCRATCHPAD_BODY_MAX {
        return Err(HandlerError::InvalidArguments(
            "Scratchpad body exceeds 8 KiB".into(),
        ));
    }
    let (id, ts) = store
        .scratchpad_append(agent, kind, body)
        .map_err(|error| HandlerError::internal("scratchpad_write", error))?;
    Ok(json!({ "id": id, "ts": ts }))
}

fn handle_scratchpad_read(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let since = args.get("since").and_then(|v| v.as_i64());
    let agent = opt_str_arg(args, "agent");
    let kind = opt_str_arg(args, "kind");
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(200) as u32;
    let r = store
        .scratchpad_read(since, agent, kind, limit)
        .map_err(|error| HandlerError::internal("scratchpad_read", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

fn handle_change_class(store: &mut Store, args: &Value) -> Result<Value, HandlerError> {
    let file = str_arg(args, "file")?;
    let root = std::env::current_dir()
        .map_err(|error| HandlerError::internal("change_class_root", error))?;
    let r = queries::classify_change(store, &root, file)
        .map_err(|error| HandlerError::internal("change_class_query", error))?;
    serde_json::to_value(r).map_err(|error| HandlerError::internal("serialize_response", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn unwrap_content(v: &serde_json::Value) -> serde_json::Value {
        let text = v["content"][0]["text"].as_str().expect("content[0].text");
        serde_json::from_str(text).expect("content[0].text was not valid JSON")
    }

    #[derive(Default)]
    struct CountingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl std::io::Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }

    struct CountingBufRead {
        bytes: Vec<u8>,
        position: usize,
        fill_buf_calls: usize,
        consume_calls: usize,
        read_calls: usize,
    }

    impl std::io::Read for CountingBufRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.read_calls += 1;
            let remaining = &self.bytes[self.position..];
            let count = remaining.len().min(buf.len());
            buf[..count].copy_from_slice(&remaining[..count]);
            self.position += count;
            Ok(count)
        }
    }

    impl std::io::BufRead for CountingBufRead {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            self.fill_buf_calls += 1;
            Ok(&self.bytes[self.position..])
        }

        fn consume(&mut self, amount: usize) {
            self.consume_calls += 1;
            self.position = self.position.saturating_add(amount).min(self.bytes.len());
        }
    }

    #[test]
    fn malformed_json_gets_parse_error_with_null_id() {
        let path = std::env::temp_dir().join("mmcg_mcp_parse_err.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut state = SessionState::Cold;

        let resp = handle_line(&mut state, &mut store, "{ not valid json")
            .expect("parse error must reply");
        assert_eq!(resp.id, Value::Null);
        assert!(resp.result.is_none());
        let e = resp.error.expect("error payload present");
        assert_eq!(e.code, -32700);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn notification_without_id_gets_no_reply() {
        let path = std::env::temp_dir().join("mmcg_mcp_notif.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut state = SessionState::Cold;

        // Valid JSON-RPC notification (no `id`) — server stays silent.
        let none = handle_line(
            &mut state,
            &mut store,
            r#"{"jsonrpc":"2.0","method":"initialized"}"#,
        );
        assert!(none.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn protocol_version_is_gated_per_connection() {
        for (requested, expected, version) in [
            (
                LEGACY_PROTOCOL_VERSION,
                LEGACY_PROTOCOL_VERSION,
                ProtocolVersion::Legacy,
            ),
            (
                CURRENT_PROTOCOL_VERSION,
                CURRENT_PROTOCOL_VERSION,
                ProtocolVersion::Current,
            ),
            (
                "2099-01-01",
                CURRENT_PROTOCOL_VERSION,
                ProtocolVersion::Current,
            ),
        ] {
            let mut state = SessionState::Cold;
            let params = json!({
                "protocolVersion": requested,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            });
            let result = initialize_result(&mut state, &params).unwrap();
            assert_eq!(result["protocolVersion"], expected);
            assert_eq!(state, SessionState::Negotiated(version));
            assert!(initialize_result(&mut state, &params).is_err());
            assert_eq!(state, SessionState::Negotiated(version));
            handle_notification(&mut state, "notifications/initialized");
            assert_eq!(state, SessionState::Ready(version));
        }
    }

    #[test]
    fn lifecycle_rejects_side_effecting_notifications() {
        let path = std::env::temp_dir().join("mmcg_mcp_lifecycle.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut state = SessionState::Cold;

        for line in [
            r#"{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"mmcg_scratchpad_append","arguments":{"agent":"executor","kind":"test","body":"must not write"}}}"#,
            r#"{"jsonrpc":"2.0","method":"ping","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#,
        ] {
            assert!(handle_line(&mut state, &mut store, line).is_none());
        }
        assert_eq!(state, SessionState::Cold);

        let initialized = handle_line(
            &mut state,
            &mut store,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
        )
        .unwrap();
        assert!(initialized.error.is_none());
        assert_eq!(state, SessionState::Negotiated(ProtocolVersion::Current));
        handle_notification(&mut state, "notifications/initialized");
        handle_notification(&mut state, "notifications/initialized");
        assert_eq!(state, SessionState::Ready(ProtocolVersion::Current));

        let none = handle_line(
            &mut state,
            &mut store,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"mmcg_scratchpad_append","arguments":{"agent":"executor","kind":"test","body":"must not write"}}}"#,
        );
        assert!(none.is_none());
        assert!(store
            .scratchpad_read(None, None, None, 10)
            .unwrap()
            .is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn invalid_envelopes_have_stable_codes() {
        let path = std::env::temp_dir().join("mmcg_mcp_invalid_envelopes.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut state = SessionState::Cold;

        let malformed = handle_line(&mut state, &mut store, "{raw repository data").unwrap();
        assert_eq!(malformed.error.as_ref().unwrap().code, -32700);
        assert_eq!(malformed.error.as_ref().unwrap().message, "Parse error");
        for line in [
            "[]",
            r#"{"jsonrpc":"1.0","id":7,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":8}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":{},"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":[],"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1.5,"method":"ping"}"#,
        ] {
            let response = handle_line(&mut state, &mut store, line).unwrap();
            assert_eq!(response.error.as_ref().unwrap().code, -32600);
            assert_eq!(response.error.as_ref().unwrap().message, "Invalid Request");
        }
        let bad_initialize = handle_line(
            &mut state,
            &mut store,
            r#"{"jsonrpc":"2.0","id":9,"method":"initialize","params":{}}"#,
        )
        .unwrap();
        assert_eq!(bad_initialize.error.as_ref().unwrap().code, -32602);
        assert_eq!(state, SessionState::Cold);

        initialize_result(
            &mut state,
            &json!({
                "protocolVersion": CURRENT_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "1" }
            }),
        )
        .unwrap();
        handle_notification(&mut state, "notifications/initialized");
        for params in [
            json!({ "name": "mmcg_search", "arguments": [] }),
            json!({ "name": "private-repository-tool", "arguments": {} }),
        ] {
            let response = handle_request(&mut state, &mut store, "tools/call", &params, json!(10));
            let error = response.error.unwrap();
            assert_eq!(error.code, -32602);
            assert!(!error.message.contains("private-repository-tool"));
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tool_errors_are_typed_and_sanitized() {
        let path = std::env::temp_dir().join("mmcg_mcp_tool_errors.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut state = SessionState::Ready(ProtocolVersion::Current);

        let invalid = handle_request(
            &mut state,
            &mut store,
            "tools/call",
            &json!({ "name": "mmcg_search", "arguments": {} }),
            json!(1),
        );
        let result = invalid.result.unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(unwrap_content(&result)["error"], "Invalid argument: name");

        let internal = handle_request(
            &mut state,
            &mut store,
            "tools/call",
            &json!({
                "name": "mmcg_symbols_changed_since",
                "arguments": { "git_ref": "HEAD", "root": "/path/that/does/not/exist" }
            }),
            json!(2),
        );
        let error = internal.error.unwrap();
        assert_eq!(error.code, -32603);
        assert_eq!(error.message, "Internal tool error");
        assert!(!error.message.contains("/path/that/does/not/exist"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn internal_errors_discard_raw_detail() {
        const SENTINEL: &str = "AUDIT_RAW_ARGUMENT_SENTINEL.secret";
        let handler_error =
            HandlerError::internal("audit_internal", format!("downstream detail {SENTINEL}"));
        let handler_debug = format!("{handler_error:?}");
        assert!(!handler_debug.contains(SENTINEL));
        assert!(!handler_debug.contains("downstream detail"));

        let path = std::env::temp_dir().join("mmcg_mcp_raw_detail.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let params = json!({
            "name": "mmcg_symbols_changed_since",
            "arguments": { "git_ref": "HEAD", "root": SENTINEL }
        });
        let typed_error =
            handle_tools_call(ProtocolVersion::Current, &mut store, &params).unwrap_err();
        let typed_debug = format!("{typed_error:?}");
        assert!(!typed_debug.contains(SENTINEL));
        assert!(!typed_debug.contains("No such file"));

        let mut state = SessionState::Ready(ProtocolVersion::Current);
        let public_error = handle_request(&mut state, &mut store, "tools/call", &params, json!(1));
        let public_diagnostic = serde_json::to_string(&public_error).unwrap();
        assert!(!public_diagnostic.contains(SENTINEL));
        assert!(!public_diagnostic.contains("No such file"));
        assert_eq!(public_error.error.unwrap().message, "Internal tool error");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tool_results_are_versioned_and_bounded() {
        let object = json!({ "value": 1 });
        let legacy = tool_result(ProtocolVersion::Legacy, object.clone(), false).unwrap();
        assert_eq!(unwrap_content(&legacy), object);
        assert!(legacy.get("structuredContent").is_none());
        assert_eq!(legacy["isError"], false);

        let current = tool_result(ProtocolVersion::Current, object.clone(), false).unwrap();
        assert_eq!(unwrap_content(&current), object);
        assert_eq!(current["structuredContent"], object);
        assert_eq!(current["content"][0]["text"], r#"{"value":1}"#);

        let array = json!([{ "body": "handoff" }]);
        let scratchpad = tool_result(ProtocolVersion::Current, array.clone(), false).unwrap();
        assert_eq!(unwrap_content(&scratchpad), array);
        assert_eq!(scratchpad["structuredContent"], json!({ "entries": array }));

        let bounded = tool_result(
            ProtocolVersion::Current,
            json!({ "blob": "x".repeat(MCP_RESULT_MAX) }),
            false,
        )
        .unwrap();
        assert_eq!(bounded["isError"], true);
        assert!(bounded["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("narrow the query"));
    }

    #[test]
    fn tool_annotations_match_behavior_table() {
        let legacy = tools_list(ProtocolVersion::Legacy);
        assert_eq!(legacy["tools"].as_array().unwrap().len(), 24);
        assert!(legacy["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool.get("annotations").is_none()));

        let current = tools_list(ProtocolVersion::Current);
        let tools = current["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 24);
        let mut readers = 0;
        for tool in tools {
            let annotations = &tool["annotations"];
            assert!(annotations.get("openWorldHint").is_none());
            if tool["name"] == "mmcg_scratchpad_append" {
                assert_eq!(
                    annotations,
                    &json!({
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false
                    })
                );
            } else {
                assert_eq!(annotations, &json!({ "readOnlyHint": true }));
                readers += 1;
            }
        }
        assert_eq!(readers, 23);
    }

    #[test]
    fn history_tool_returns_observed_retrieval_with_epistemic_contract() {
        let path = std::env::temp_dir().join("mmcg_mcp_history.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        store
            .replace_project_history(&[crate::store::ProjectHistoryEntry {
                path: "CONTEXT.md".into(),
                kind: "context".into(),
                title: "Webhook decision".into(),
                body: "Use a durable idempotency key for webhook retries.".into(),
            }])
            .unwrap();

        let envelope = handle_tools_call(
            ProtocolVersion::Current,
            &mut store,
            &json!({
                "name": "mmcg_history",
                "arguments": { "query": "idempotency", "kind": "context" }
            }),
        )
        .unwrap();
        let result = unwrap_content(&envelope);
        assert_eq!(result["count"], 1);
        assert_eq!(result["observed"][0]["path"], "CONTEXT.md");
        assert!(result["inference"].as_str().unwrap().contains("none"));
        assert!(result["source_of_truth"]
            .as_str()
            .unwrap()
            .contains("Markdown"));
        assert_eq!(result["skipped_artifacts"], 0);
        assert_eq!(result["truncated"], false);
        assert!(result["freshness"]
            .as_str()
            .unwrap()
            .contains("not checked"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn serve_io_bounds_frames_and_flushes_once() {
        let path = std::env::temp_dir().join("mmcg_mcp_serve_io.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let input = "\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1\"}}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\",\"params\":{}}\n".to_string();
        let mut output = CountingWriter::default();
        serve_io(&mut store, Cursor::new(input.into_bytes()), &mut output).unwrap();
        assert_eq!(output.flushes, 2);
        assert_eq!(output.bytes.split(|byte| *byte == b'\n').count() - 1, 2);

        let mut invalid_output = CountingWriter::default();
        serve_io(
            &mut store,
            Cursor::new(vec![0xff, b'\n']),
            &mut invalid_output,
        )
        .unwrap();
        assert_eq!(invalid_output.flushes, 1);

        let mut oversized = vec![b'x'; MCP_FRAME_MAX + 1];
        oversized.push(b'\n');
        oversized.extend_from_slice(br#"{"jsonrpc":"2.0","id":3,"method":"ping"}\n"#);
        let mut oversized_output = CountingWriter::default();
        serve_io(&mut store, Cursor::new(oversized), &mut oversized_output).unwrap();
        assert_eq!(oversized_output.flushes, 1);
        assert_eq!(
            oversized_output.bytes.split(|byte| *byte == b'\n').count() - 1,
            1
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oversized_buffer_is_rejected_before_copy_or_consume() {
        // `read_frame` itself (unaffected by `serve_io`'s reader-thread
        // ownership transfer) must reject an oversized frame from its very
        // first `fill_buf` peek, without ever calling `consume` or `read`.
        let mut bytes = vec![b'x'; MCP_FRAME_MAX + 1];
        bytes.push(b'\n');
        bytes.extend_from_slice(br#"{"jsonrpc":"2.0","id":3,"method":"ping"}\n"#);
        let mut input = CountingBufRead {
            bytes: bytes.clone(),
            position: 0,
            fill_buf_calls: 0,
            consume_calls: 0,
            read_calls: 0,
        };
        let frame = read_frame(&mut input).unwrap().unwrap();
        assert!(matches!(frame, Frame::TooLarge(_)));
        assert_eq!(input.fill_buf_calls, 1);
        assert_eq!(input.consume_calls, 0);
        assert_eq!(input.read_calls, 0);

        // End to end through `serve_io` (owned, `Send + 'static` source, as
        // the reader thread requires): exactly one flushed error response.
        let path = std::env::temp_dir().join("mmcg_mcp_counting_reader.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let mut output = CountingWriter::default();
        serve_io(&mut store, Cursor::new(bytes), &mut output).unwrap();
        assert_eq!(output.flushes, 1);
        assert_eq!(output.bytes.split(|byte| *byte == b'\n').count() - 1, 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn map_tool_matches_the_shared_engine_payload() {
        let path = std::env::temp_dir().join("mmcg_mcp_map.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        store.upsert_file("src/main.rs", 1, 1).unwrap();
        store.upsert_file("src/lib.rs", 1, 1).unwrap();
        let expected =
            serde_json::to_value(queries::project_map(&store, ".", 2, 20).unwrap()).unwrap();
        let actual = handle_map(&mut store, &json!({})).unwrap();
        assert_eq!(actual, expected);
        let production_expected = serde_json::to_value(
            queries::project_map_with_options(&store, ".", 2, 20, true).unwrap(),
        )
        .unwrap();
        let production_actual =
            handle_map(&mut store, &json!({ "production_only": true })).unwrap();
        assert_eq!(production_actual, production_expected);
        assert!(matches!(
            handle_map(&mut store, &json!({ "production_only": "yes" })),
            Err(HandlerError::InvalidArguments(message))
                if message == "Invalid argument: production_only"
        ));
        let listed = tools_list(ProtocolVersion::Current);
        let map = listed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "mmcg_map")
            .unwrap();
        assert_eq!(map["annotations"], json!({ "readOnlyHint": true }));
        std::fs::remove_file(&path).ok();
    }

    fn impact_fixture(name: &str) -> (std::path::PathBuf, crate::store::Store) {
        let root =
            std::env::temp_dir().join(format!("mmcg-mcp-impact-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q", "--initial-branch=main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("src/app.py"), "def value():\n    return 1\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "baseline"]);
        std::fs::write(root.join("src/app.py"), "def value():\n    return 2\n").unwrap();
        let db =
            std::env::temp_dir().join(format!("mmcg-mcp-impact-{}-{name}.db", std::process::id()));
        let _ = std::fs::remove_file(&db);
        let mut store = crate::store::Store::open(db).unwrap();
        crate::indexer::Indexer::new(&root)
            .index_all(&mut store, true)
            .unwrap();
        (root, store)
    }

    #[test]
    fn change_impact_cli_and_mcp_share_the_same_payload() {
        let (root, mut store) = impact_fixture("shared");
        let expected =
            serde_json::to_value(queries::change_impact(&store, &root, "HEAD", 3, 100).unwrap())
                .unwrap();
        let actual = handle_change_impact(
            &mut store,
            &json!({ "since": "HEAD", "root": root.to_string_lossy(), "depth": 3, "top": 100 }),
        )
        .unwrap();
        assert_eq!(actual, expected);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn test_impact_is_an_exact_projection_of_change_impact() {
        let (root, mut store) = impact_fixture("projection");
        let full = handle_change_impact(
            &mut store,
            &json!({ "since": "HEAD", "root": root.to_string_lossy(), "depth": 3, "top": 100 }),
        )
        .unwrap();
        let projected = handle_test_impact(
            &mut store,
            &json!({ "since": "HEAD", "root": root.to_string_lossy(), "depth": 3, "top": 100 }),
        )
        .unwrap();
        assert_eq!(projected["schema_version"], full["schema_version"]);
        for field in [
            "baseline",
            "scope",
            "changes",
            "tests",
            "limits",
            "precision_notes",
        ] {
            assert_eq!(projected[field], full[field]);
        }
        let keys = projected
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            keys,
            [
                "baseline",
                "changes",
                "limits",
                "precision_notes",
                "schema_version",
                "scope",
                "tests"
            ]
            .into_iter()
            .map(String::from)
            .collect()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn impact_tools_validate_depth_top_and_root() {
        let path = std::env::temp_dir().join("mmcg-mcp-impact-validation.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        for (argument, value) in [
            ("depth", json!(0)),
            ("depth", json!(6)),
            ("depth", json!("3")),
            ("top", json!(0)),
            ("top", json!(501)),
            ("top", json!(false)),
            ("root", json!(false)),
        ] {
            for tool in ["mmcg_change_impact", "mmcg_test_impact"] {
                let mut args = json!({ "since": "HEAD" });
                args[argument] = value.clone();
                let result = handle_tools_call(
                    ProtocolVersion::Current,
                    &mut store,
                    &json!({ "name": tool, "arguments": args }),
                )
                .unwrap();
                assert_eq!(result["isError"], true);
            }
        }
        let order: Vec<_> = TOOLS.iter().map(|tool| tool.name).collect();
        let map = order.iter().position(|name| *name == "mmcg_map").unwrap();
        assert_eq!(order[map + 1], "mmcg_change_impact");
        assert_eq!(order[map + 2], "mmcg_test_impact");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn impact_failure_codes_are_sanitized_in_cli_current_and_legacy_mcp() {
        let path = std::env::temp_dir().join("mmcg-mcp-impact-errors.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let root = std::env::temp_dir().canonicalize().unwrap();
        for (error, code) in [
            (queries::ChangeImpactError::InvalidRef, "invalid_ref"),
            (queries::ChangeImpactError::RootMismatch, "root_mismatch"),
            (queries::ChangeImpactError::IndexStale, "index_stale"),
            (
                queries::ChangeImpactError::SnapshotChanged,
                "snapshot_changed",
            ),
            (queries::ChangeImpactError::GitTimeout, "git_timeout"),
            (
                queries::ChangeImpactError::GitOutputLimit,
                "git_output_limit",
            ),
        ] {
            assert_eq!(error.to_string(), code);
            for version in [ProtocolVersion::Current, ProtocolVersion::Legacy] {
                for tool in ["mmcg_change_impact", "mmcg_test_impact"] {
                    let injected_detail = "injected-engine-detail-must-not-leak";
                    let engine =
                        |_: &Store, _: &std::path::Path, git_ref: &str, _: u32, _: usize| {
                            assert_eq!(git_ref, injected_detail);
                            Err(error)
                        };
                    let result = handle_tools_call_with_impact_engine(
                        version,
                        &mut store,
                        &json!({
                            "name": tool,
                            "arguments": {
                                "since": injected_detail,
                                "root": root.to_string_lossy(),
                                "depth": 3,
                                "top": 100
                            }
                        }),
                        &engine,
                    )
                    .unwrap();
                    assert_eq!(result["isError"], true);
                    assert_eq!(unwrap_content(&result), json!({ "error": code }));
                    let transcript = serde_json::to_string(&result).unwrap();
                    assert!(!transcript.contains(injected_detail));
                    if version == ProtocolVersion::Current {
                        assert_eq!(result["structuredContent"], json!({ "error": code }));
                    } else {
                        assert!(result.get("structuredContent").is_none());
                    }
                }
            }
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn map_tool_rejects_wrong_typed_optional_arguments() {
        let path = std::env::temp_dir().join("mmcg_mcp_map_types.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        let cases = [
            ("path", json!(null)),
            ("path", json!(true)),
            ("path", json!(7)),
            ("path", json!([])),
            ("path", json!({})),
            ("depth", json!(null)),
            ("depth", json!(true)),
            ("depth", json!("2")),
            ("depth", json!(2.5)),
            ("depth", json!(-1)),
            ("depth", json!(0)),
            ("depth", json!(7)),
            ("depth", json!(u64::MAX)),
            ("top", json!(null)),
            ("top", json!(false)),
            ("top", json!("20")),
            ("top", json!(20.5)),
            ("top", json!(-1)),
            ("top", json!(0)),
            ("top", json!(101)),
            ("top", json!(u64::MAX)),
        ];

        for version in [ProtocolVersion::Current, ProtocolVersion::Legacy] {
            for (name, value) in &cases {
                let mut arguments = json!({});
                arguments[*name] = value.clone();
                let result = handle_tools_call(
                    version,
                    &mut store,
                    &json!({
                        "name": "mmcg_map",
                        "arguments": arguments
                    }),
                )
                .unwrap();
                assert_eq!(result["isError"], true);
                assert_eq!(
                    unwrap_content(&result)["error"],
                    format!("Invalid argument: {name}")
                );
                if version.is_current() {
                    assert_eq!(result["structuredContent"], unwrap_content(&result));
                } else {
                    assert!(result.get("structuredContent").is_none());
                }
            }
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn changed_since_root_defaults_to_repo_root_from_db_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mastermind = tmp.path().join(".mastermind");
        std::fs::create_dir_all(&mastermind).unwrap();
        let db = mastermind.join("mmcg.db");
        std::fs::write(&db, b"").unwrap();

        let root = changed_since_root(None, &db).unwrap();
        assert_eq!(root, tmp.path().canonicalize().unwrap());

        let explicit = changed_since_root(Some(tmp.path().to_str().unwrap()), &db).unwrap();
        assert_eq!(explicit, tmp.path().canonicalize().unwrap());
    }

    #[test]
    fn scratchpad_round_trip_via_tools_call() {
        let path = std::env::temp_dir().join("mmcg_mcp_scratchpad.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();

        let append_env = handle_tools_call(
            ProtocolVersion::Current,
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
            ProtocolVersion::Current,
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

        let too_big = "x".repeat(8 * 1024 + 1);
        let too_big_env = handle_tools_call(
            ProtocolVersion::Current,
            &mut store,
            &serde_json::json!({
                "name": "mmcg_scratchpad_append",
                "arguments": { "agent": "a", "kind": "n", "body": too_big }
            }),
        )
        .unwrap();
        assert_eq!(too_big_env["isError"], true);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn change_class_round_trip_via_tools_call() {
        let tmp = std::env::temp_dir().join("mmcg_change_class_rt");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        let db_path = tmp.join("mmcg.db");
        let mut store = crate::store::Store::open(&db_path).unwrap();

        let foo_path = tmp.join("src/foo.rs");
        let rel = "src/foo.rs";
        std::fs::write(&foo_path, "// header\nfn foo() {}\nfn bar() { foo(); }\n").unwrap();

        let _ = std::env::set_current_dir(&tmp);

        let first_env = handle_tools_call(
            ProtocolVersion::Current,
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

        std::fs::write(
            &foo_path,
            "// header v2 (longer)\n// extra comment\nfn foo() {}\nfn bar() { foo(); }\n",
        )
        .unwrap();
        let cosmetic_env = handle_tools_call(
            ProtocolVersion::Current,
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

        std::fs::write(
            &foo_path,
            "fn foo() {}\nfn bar() { foo(); }\nfn baz() { bar(); }\n",
        )
        .unwrap();
        let structural_env = handle_tools_call(
            ProtocolVersion::Current,
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

    #[test]
    fn tools_list_covers_every_handler() {
        let listed: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(listed.len(), 24, "expected 24 tools, got {}", listed.len());
        for name in &listed {
            assert!(
                TOOLS.iter().any(|t| &t.name == name),
                "tool '{name}' listed but has no entry in TOOLS"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "duplicate tool name: {}", t.name);
        }
    }

    #[test]
    fn work_budget_zero_covers_every_graph_tool() {
        let path = std::env::temp_dir().join("mmcg_mcp_zero_budget_coverage.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        // A budget that is already exhausted at install time — short-circuits
        // before the handler runs at all, so this doesn't depend on any
        // particular tool's query being slow enough to trip mid-flight.
        store.set_default_work_budget(WorkBudget {
            deadline: Some(std::time::Duration::ZERO),
            op_ticks: Some(0),
        });
        for tool in TOOLS {
            let params = json!({ "name": tool.name, "arguments": {} });
            let result = handle_tools_call(ProtocolVersion::Current, &mut store, &params)
                .unwrap_or_else(|e| panic!("tool {} must not hang or panic: {e:?}", tool.name));
            assert_eq!(
                result["isError"], true,
                "tool {} did not report an error under a zero budget",
                tool.name
            );
            let content = unwrap_content(&result);
            assert_eq!(
                content.get("code").and_then(Value::as_str),
                Some("work_limit_exceeded"),
                "tool {} did not report work_limit_exceeded",
                tool.name
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    /// A `BufRead` that serves a fixed byte buffer and, once past `delay_at`,
    /// sleeps once for `delay` before yielding the rest — used to guarantee a
    /// later line (e.g. a cancel notification) is only observed by the
    /// reader thread after the main thread has had ample time to start
    /// processing an earlier request.
    struct DelayedCursor {
        bytes: Vec<u8>,
        pos: usize,
        delay_at: usize,
        delay: std::time::Duration,
        slept: bool,
    }

    impl std::io::Read for DelayedCursor {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let chunk = self.fill_buf()?;
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            self.consume(n);
            Ok(n)
        }
    }

    impl std::io::BufRead for DelayedCursor {
        fn fill_buf(&mut self) -> io::Result<&[u8]> {
            if self.pos >= self.delay_at && !self.slept {
                self.slept = true;
                std::thread::sleep(self.delay);
            }
            Ok(&self.bytes[self.pos..])
        }

        fn consume(&mut self, amount: usize) {
            self.pos += amount;
        }
    }

    /// A dense name-collision graph — every one of a handful of names has
    /// many same-named definitions, all cross-connected — so a `max_depth =
    /// 10` transitive-callers walk (`mmcg_impact`) does enough real SQL work
    /// to still be running tens of milliseconds in, without depending on
    /// exact timing to complete or hit its own internal 2s/250k-tick cap.
    fn seed_dense_collision_fixture(store: &crate::store::Store) {
        for index in 0..80 {
            let name = format!("node{}", index % 8);
            let id = store
                .insert_symbol(
                    &name,
                    "function",
                    &format!("src/{index}.rs"),
                    1,
                    2,
                    None,
                    None,
                )
                .unwrap();
            for target in 0..8 {
                store
                    .insert_edge(id, None, &format!("node{target}"), "calls", 1)
                    .unwrap();
            }
        }
    }

    const INITIALIZE_LINE: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    const INITIALIZED_LINE: &str =
        r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#;

    #[test]
    fn serve_io_cancel_interrupts_inflight_request() {
        let path = std::env::temp_dir().join("mmcg_mcp_cancel_inflight.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        // Unlimited outer budget: only the client cancel — not a budget —
        // may interrupt this call.
        store.set_default_work_budget(WorkBudget::UNLIMITED);
        seed_dense_collision_fixture(&store);

        let slow_call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mmcg_impact","arguments":{"name":"node0","max_depth":10}}}"#;
        let cancel =
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        let ping = r#"{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}"#;

        let mut bytes =
            format!("{INITIALIZE_LINE}\n{INITIALIZED_LINE}\n{slow_call}\n").into_bytes();
        let delay_at = bytes.len();
        bytes.extend_from_slice(format!("{cancel}\n{ping}\n").as_bytes());

        let input = DelayedCursor {
            bytes,
            pos: 0,
            delay_at,
            delay: std::time::Duration::from_millis(150),
            slept: false,
        };
        let mut output = CountingWriter::default();
        let started = std::time::Instant::now();
        serve_io(&mut store, input, &mut output).unwrap();
        let elapsed = started.elapsed();

        let text = String::from_utf8(output.bytes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "initialize + tools/call + ping responses");

        let call_response: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(call_response["id"], 1);
        assert_eq!(call_response["result"]["isError"], true);
        let content = unwrap_content(&call_response["result"]);
        assert_eq!(
            content.get("code").and_then(Value::as_str),
            Some("cancelled")
        );

        let ping_response: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(ping_response["id"], 2);
        assert!(ping_response.get("error").is_none());

        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "cancel should interrupt well before the walk's own internal 2s cap: {elapsed:?}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn serve_io_late_cancel_does_not_abort_next_request() {
        let path = std::env::temp_dir().join("mmcg_mcp_late_cancel.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        store
            .insert_symbol("solo", "function", "src/solo.rs", 1, 2, None, None)
            .unwrap();

        // `ping` (id 1) finishes near-instantly; a cancel notification for id
        // 1 arrives only after a delay, by which point request 1 has long
        // finished — it must not abort request 2.
        let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#;
        let stale_cancel =
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        let next_call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mmcg_impact","arguments":{"name":"solo"}}}"#;

        let mut bytes = format!("{INITIALIZE_LINE}\n{INITIALIZED_LINE}\n{ping}\n").into_bytes();
        let delay_at = bytes.len();
        bytes.extend_from_slice(format!("{stale_cancel}\n{next_call}\n").as_bytes());

        let input = DelayedCursor {
            bytes,
            pos: 0,
            delay_at,
            delay: std::time::Duration::from_millis(80),
            slept: false,
        };
        let mut output = CountingWriter::default();
        serve_io(&mut store, input, &mut output).unwrap();

        let text = String::from_utf8(output.bytes).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);

        let ping_response: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(ping_response["id"], 1);
        assert!(ping_response.get("error").is_none());

        let call_response: Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(call_response["id"], 2);
        assert_eq!(call_response["result"]["isError"], false);
        let content = unwrap_content(&call_response["result"]);
        assert_ne!(
            content.get("code").and_then(Value::as_str),
            Some("cancelled")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cancel_maps_to_cancelled_not_work_limit() {
        // Budget-expiry path: a pre-expired budget yields work_limit_exceeded.
        let path = std::env::temp_dir().join("mmcg_mcp_cancel_vs_work_limit.db");
        let _ = std::fs::remove_file(&path);
        let mut store = crate::store::Store::open(&path).unwrap();
        store.set_default_work_budget(WorkBudget {
            deadline: Some(std::time::Duration::ZERO),
            op_ticks: Some(0),
        });
        let budget_result = handle_tools_call(
            ProtocolVersion::Current,
            &mut store,
            &json!({ "name": "mmcg_search", "arguments": { "name": "anything" } }),
        )
        .unwrap();
        let budget_content = unwrap_content(&budget_result);
        assert_eq!(
            budget_content.get("code").and_then(Value::as_str),
            Some("work_limit_exceeded")
        );

        // Cancel path, on a fresh store with an unlimited budget: the same
        // interrupt machinery reports `cancelled`, never `work_limit_exceeded`.
        let cancel_path = std::env::temp_dir().join("mmcg_mcp_cancel_vs_work_limit_cancel.db");
        let _ = std::fs::remove_file(&cancel_path);
        let mut cancel_store = crate::store::Store::open(&cancel_path).unwrap();
        cancel_store.set_default_work_budget(WorkBudget::UNLIMITED);
        seed_dense_collision_fixture(&cancel_store);

        let slow_call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"mmcg_impact","arguments":{"name":"node0","max_depth":10}}}"#;
        let cancel =
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#;
        let mut bytes =
            format!("{INITIALIZE_LINE}\n{INITIALIZED_LINE}\n{slow_call}\n").into_bytes();
        let delay_at = bytes.len();
        bytes.extend_from_slice(format!("{cancel}\n").as_bytes());
        let input = DelayedCursor {
            bytes,
            pos: 0,
            delay_at,
            delay: std::time::Duration::from_millis(150),
            slept: false,
        };
        let mut output = CountingWriter::default();
        serve_io(&mut cancel_store, input, &mut output).unwrap();
        let text = String::from_utf8(output.bytes).unwrap();
        let call_response: Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
        let cancel_content = unwrap_content(&call_response["result"]);
        assert_eq!(
            cancel_content.get("code").and_then(Value::as_str),
            Some("cancelled")
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&cancel_path);
    }
}
