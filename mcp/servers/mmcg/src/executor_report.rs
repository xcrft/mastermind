use serde::{Deserialize, Serialize};
use std::path::Path;

/// A single claim an executor made in their structured report tail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Claim {
    /// Executor claims they added a new function/symbol.
    FunctionAdded {
        symbol: String,
        #[serde(default)]
        file: Option<String>,
        /// Optional exact signature the executor claims was written. When
        /// present, the auditor verifies the stored signature matches.
        #[serde(default)]
        signature: Option<String>,
    },
    /// Executor claims symbol X calls existing symbol Y.
    Integration {
        from: String,
        /// File containing `from` — scopes the callee-edge check.
        #[serde(default)]
        from_file: Option<String>,
        to: String,
        /// File containing `to` — narrows the "does Y exist" lookup.
        #[serde(default)]
        to_file: Option<String>,
        #[serde(default)]
        relation: Option<String>,
    },
}

/// Observed runtime outcome the executor (or CI) can attach to a verify entry.
/// When present, the auditor catches "claimed passed, but exit code was 1"
/// contradictions without re-running the command.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservedOutcome {
    /// Process exit code. 0 = success by convention.
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Test cases the runner actually executed. 0 with `exit_code` 0 is the
    /// vacuous-pass signature.
    #[serde(default)]
    pub tests_run: Option<u32>,
}

/// One verify entry from the executor report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub cmd: String,
    #[serde(default)]
    pub claimed: Option<String>,
    /// Observed outcome from the executor or CI. Optional — when absent the
    /// auditor falls back to static heuristics (no test files, no #[test] attrs,
    /// etc.).
    #[serde(default)]
    pub observed: Option<ObservedOutcome>,
}

/// The structured tail the executor appended to their report.
///
/// Parsed from a YAML block delimited by `<!-- mastermind:executor-begin -->` /
/// `<!-- mastermind:executor-end -->` inside the report file, OR from a bare
/// YAML file.
///
/// Minimal format:
/// ```yaml
/// claims:
///   - kind: function_added
///     symbol: CancelOrder
///     file: pkg/checkout/checkout.go
///     signature: "func CancelOrder(ctx context.Context, id string) error"
///   - kind: integration
///     from: CancelOrder
///     from_file: pkg/checkout/checkout.go
///     to: ProcessPayment
///     to_file: pkg/payment/payment.go
///     relation: calls
/// verify:
///   - cmd: go test ./pkg/checkout/...
///     claimed: passed
///     observed:
///       exit_code: 0
///       tests_run: 12
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorReport {
    #[serde(default)]
    pub claims: Vec<Claim>,
    #[serde(default)]
    pub verify: Vec<VerifyResult>,
}

impl ExecutorReport {
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty() && self.verify.is_empty()
    }
}

/// Parse an executor report from a file. Two formats:
/// - Bare YAML file (no markdown wrapper)
/// - Markdown file with a sentinel-delimited YAML block
pub fn parse_file(path: &Path) -> Result<ExecutorReport, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_str(&text)
}

pub fn parse_str(text: &str) -> Result<ExecutorReport, String> {
    let yaml = extract_yaml(text);
    serde_norway::from_str::<ExecutorReport>(yaml)
        .map_err(|e| format!("parse executor report YAML: {e}"))
}

fn extract_yaml(text: &str) -> &str {
    if let Some(start) = find_sentinel_start(text) {
        if let Some(end) = find_sentinel_end(text, start) {
            return &text[start..end];
        }
    }
    text.trim()
}

fn find_sentinel_start(text: &str) -> Option<usize> {
    let marker = "mastermind:executor-begin";
    let pos = text.find(marker)?;
    let after_marker = &text[pos..];
    let fence_pos = after_marker.find("```")?;
    let after_fence = &after_marker[fence_pos + 3..];
    let newline = after_fence.find('\n')?;
    Some(pos + fence_pos + 3 + newline + 1)
}

fn find_sentinel_end(text: &str, start: usize) -> Option<usize> {
    let tail = &text[start..];
    let end_fence = tail.find("```")?;
    Some(start + end_fence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_yaml() {
        let yaml = "claims:\n  - kind: function_added\n    symbol: Foo\nverify:\n  - cmd: go test\n    claimed: passed\n";
        let r = parse_str(yaml).unwrap();
        assert_eq!(r.claims.len(), 1);
        assert_eq!(r.verify.len(), 1);
    }

    #[test]
    fn parses_sentinel_block() {
        let md = "Some prose.\n\n<!-- mastermind:executor-begin -->\n```yaml\nclaims:\n  - kind: integration\n    from: A\n    to: B\n    relation: calls\n```\n<!-- mastermind:executor-end -->\n";
        let r = parse_str(md).unwrap();
        assert_eq!(r.claims.len(), 1);
    }

    #[test]
    fn empty_on_no_yaml() {
        let r = parse_str("").unwrap();
        assert!(r.is_empty());
    }
}
