use super::{SourceFailure, MAX_SOURCE_FACTS};
use serde_json::Value;
use std::time::Instant;

pub(super) struct ParsedOtel {
    pub spans: Vec<OtelSpan>,
    pub facts_total: usize,
    pub partial: bool,
    pub invalid_records: bool,
    pub work_limited: bool,
    pub deadline_reached: bool,
}

pub(super) struct OtelSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub file: Option<String>,
}

pub(super) fn parse(bytes: &[u8], deadline: Option<Instant>) -> Result<ParsedOtel, SourceFailure> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| SourceFailure::InvalidFormat)?;
    let Some(resource_spans) = value.get("resourceSpans").and_then(Value::as_array) else {
        return Err(SourceFailure::InvalidFormat);
    };
    let mut spans = Vec::new();
    let mut facts_total = 0usize;
    let mut partial = false;
    let mut invalid_records = false;
    let mut work_limited = false;
    let mut deadline_reached = false;

    'resources: for resource in resource_spans {
        let scope_spans = resource
            .get("scopeSpans")
            .or_else(|| resource.get("instrumentationLibrarySpans"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        for scope in scope_spans {
            let scope_items = scope
                .get("spans")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for span in scope_items {
                if facts_total.is_multiple_of(1_024)
                    && deadline.is_some_and(|value| Instant::now() >= value)
                {
                    partial = true;
                    deadline_reached = true;
                    break 'resources;
                }
                if facts_total >= MAX_SOURCE_FACTS {
                    partial = true;
                    work_limited = true;
                    break 'resources;
                }
                facts_total += 1;
                let Some(trace_id) = span.get("traceId").and_then(Value::as_str) else {
                    partial = true;
                    invalid_records = true;
                    continue;
                };
                let Some(span_id) = span.get("spanId").and_then(Value::as_str) else {
                    partial = true;
                    invalid_records = true;
                    continue;
                };
                if trace_id.is_empty() || span_id.is_empty() {
                    partial = true;
                    invalid_records = true;
                    continue;
                }
                spans.push(OtelSpan {
                    trace_id: trace_id.to_string(),
                    span_id: span_id.to_string(),
                    parent_span_id: span
                        .get("parentSpanId")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string),
                    name: span
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("unnamed span")
                        .to_string(),
                    file: code_file_path(span),
                });
            }
        }
    }
    Ok(ParsedOtel {
        spans,
        facts_total,
        partial,
        invalid_records,
        work_limited,
        deadline_reached,
    })
}

fn code_file_path(span: &Value) -> Option<String> {
    span.get("attributes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|attribute| {
            let key = attribute.get("key").and_then(Value::as_str)?;
            if !matches!(key, "code.file.path" | "code.filepath") {
                return None;
            }
            attribute
                .pointer("/value/stringValue")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}
