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
    },
    /// Executor claims symbol X calls existing symbol Y.
    Integration {
        from: String,
        to: String,
        #[serde(default)]
        relation: Option<String>,
    },
}

/// One verify entry from the executor report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub cmd: String,
    #[serde(default)]
    pub claimed: Option<String>,
}

/// The structured tail the executor appended to their report.
///
/// Parsed from a YAML block delimited by
/// `<!-- mastermind:executor-begin -->` / `<!-- mastermind:executor-end -->`
/// inside the executor-report file, OR from a bare YAML file.
///
/// Minimal format:
/// ```yaml
/// claims:
///   - kind: function_added
///     symbol: CancelOrder
///     file: pkg/checkout/checkout.go
///   - kind: integration
///     from: CancelOrder
///     to: ProcessPayment
///     relation: calls
/// verify:
///   - cmd: go test ./pkg/checkout/...
///     claimed: passed
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

/// Parse an executor report from a file. Accepts two formats:
/// - Bare YAML file (no markdown wrapper)
/// - Markdown file containing a sentinel-delimited YAML block
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
