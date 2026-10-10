//! Source delivery with explicit, process-local receipts for prior ranges.

use crate::bounded_fs::{self, BoundedReadError, ReadControl};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::Path;

pub(crate) const LINE_LIMIT: u32 = 200;
pub(crate) const DEFAULT_LINES: u32 = 80;
const BYTE_LIMIT: u64 = 1024 * 1024;
const RECEIPT_LIMIT: usize = 128;
const RANGE_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Range {
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Serialize)]
pub(crate) struct SourceLine {
    line: u32,
    text: String,
}

#[derive(Serialize)]
pub(crate) struct Segment {
    start_line: u32,
    end_line: u32,
    lines: Vec<SourceLine>,
}

#[derive(Serialize)]
pub(crate) struct Delivery {
    path: String,
    source_sha256: String,
    total_lines: u32,
    requested_start_line: u32,
    requested_end_line: u32,
    segments: Vec<Segment>,
    reused_ranges: Vec<Range>,
    reuse_status: &'static str,
    range_truncated: bool,
    next_line: Option<u32>,
    receipt: String,
    receipt_ranges: Vec<Range>,
    receipt_coverage_truncated: bool,
}

#[derive(Debug)]
pub(crate) enum Error {
    InvalidSelection,
    Read(BoundedReadError),
    Encoding,
    Randomness,
}

pub(crate) struct Snapshot {
    path: String,
    digest: String,
    lines: Vec<String>,
}

pub(crate) fn load(root: &Path, path: &str, control: ReadControl<'_>) -> Result<Snapshot, Error> {
    let normalized =
        bounded_fs::normalize_repository_relative_path(Path::new(path)).map_err(Error::Read)?;
    if normalized != path || path.split('/').any(|part| part.starts_with('.')) {
        return Err(Error::InvalidSelection);
    }
    let file =
        bounded_fs::read_repository_file(root, Path::new(path), BYTE_LIMIT, BYTE_LIMIT, control)
            .map_err(Error::Read)?;
    let digest = crate::hex::encode(&Sha256::digest(&file.bytes));
    let text = String::from_utf8(file.bytes).map_err(|_| Error::Encoding)?;
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    let last = lines.len().saturating_sub(1);
    for line in &mut lines[..last] {
        if line.ends_with('\r') {
            line.pop();
        }
    }
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    control.check().map_err(Error::Read)?;
    Ok(Snapshot {
        path: path.into(),
        digest,
        lines,
    })
}

struct Receipt {
    id: String,
    task: String,
    path: String,
    digest: String,
    ranges: Vec<Range>,
}

#[derive(Default)]
pub(crate) struct Receipts {
    entries: VecDeque<Receipt>,
}

fn merge(mut ranges: Vec<Range>) -> Vec<Range> {
    ranges.sort_by_key(|range| range.start_line);
    let mut result: Vec<Range> = Vec::new();
    for range in ranges {
        if let Some(last) = result.last_mut() {
            if range.start_line <= last.end_line.saturating_add(1) {
                last.end_line = last.end_line.max(range.end_line);
                continue;
            }
        }
        result.push(range);
    }
    result
}

impl Receipts {
    pub(crate) fn deliver(
        &mut self,
        snapshot: Snapshot,
        task: &str,
        start: u32,
        end: Option<u32>,
        previous: Option<&str>,
    ) -> Result<Delivery, Error> {
        let total = snapshot.lines.len() as u32;
        let requested_end =
            end.unwrap_or_else(|| total.min(start.saturating_add(DEFAULT_LINES - 1)));
        if start == 0
            || (total > 0 && start > total)
            || end.is_some_and(|end| end < start)
            || (total == 0 && (start != 1 || end.is_some()))
        {
            return Err(Error::InvalidSelection);
        }
        let (prior, reuse_status) = match previous {
            None => (Vec::new(), "not_requested"),
            Some(id) => match self.entries.iter().find(|entry| entry.id == id) {
                None => (Vec::new(), "receipt_unavailable"),
                Some(entry) if entry.task != task || entry.path != snapshot.path => {
                    (Vec::new(), "binding_mismatch")
                }
                Some(entry) if entry.digest != snapshot.digest => (Vec::new(), "source_changed"),
                Some(entry) => (entry.ranges.clone(), "applied"),
            },
        };
        let end = total.min(requested_end);
        let mut segments = Vec::new();
        let mut reused = Vec::new();
        let mut position = start;
        let mut count = 0;
        let mut prior_index = 0;
        while position <= end {
            while prior_index < prior.len() && prior[prior_index].end_line < position {
                prior_index += 1;
            }
            if let Some(range) = prior
                .get(prior_index)
                .filter(|range| range.start_line <= position)
            {
                let right = end.min(range.end_line);
                reused.push(Range {
                    start_line: position,
                    end_line: right,
                });
                position = right + 1;
                continue;
            }
            if count == LINE_LIMIT || segments.len() == RANGE_LIMIT {
                break;
            }
            let before_prior = prior
                .get(prior_index)
                .map_or(end, |range| end.min(range.start_line - 1));
            let right = before_prior.min(position + LINE_LIMIT - count - 1);
            let lines = (position..=right)
                .map(|line| SourceLine {
                    line,
                    text: snapshot.lines[line as usize - 1].clone(),
                })
                .collect();
            segments.push(Segment {
                start_line: position,
                end_line: right,
                lines,
            });
            count += right - position + 1;
            position = right + 1;
        }
        let mut ranges = merge(
            prior
                .into_iter()
                .chain(segments.iter().map(|segment| Range {
                    start_line: segment.start_line,
                    end_line: segment.end_line,
                }))
                .collect(),
        );
        // Forgetting old ranges may cost a later repeat, but cannot omit unread text.
        let coverage_truncated = ranges.len() > RANGE_LIMIT;
        ranges.truncate(RANGE_LIMIT);
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(|_| Error::Randomness)?;
        let id = crate::hex::encode(&nonce);
        self.entries.push_back(Receipt {
            id: id.clone(),
            task: task.into(),
            path: snapshot.path.clone(),
            digest: snapshot.digest.clone(),
            ranges: ranges.clone(),
        });
        while self.entries.len() > RECEIPT_LIMIT {
            self.entries.pop_front();
        }
        Ok(Delivery {
            path: snapshot.path,
            source_sha256: snapshot.digest,
            total_lines: total,
            requested_start_line: start,
            requested_end_line: requested_end,
            segments,
            reused_ranges: reused,
            reuse_status,
            range_truncated: position <= end,
            next_line: (position <= end).then_some(position),
            receipt: id,
            receipt_ranges: ranges,
            receipt_coverage_truncated: coverage_truncated,
        })
    }
}
