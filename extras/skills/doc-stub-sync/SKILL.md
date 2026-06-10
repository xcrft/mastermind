---
name: doc-stub-sync
description: 'Sync local documentation stubs with their current online versions — finds files matching a stub pattern (default `Fetch live documentation: <URL>`), compares content hashes, refetches changed pages, reports diffs. Use when the user says "update docs", "sync docs with online sources", "refresh local docs", or has a folder of stub files pointing at upstream URLs.'
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - docs
    - automation
    - sync
  model: sonnet
  requires:
    - Bash
    - Python 3.10+ (for the bundled script), OR any HTTP-capable runtime if doing it manually
---

# Documentation Stub Sync

Keeps a tree of local documentation files in sync with their online sources. Works on the **stub-link pattern**: each local file contains a marker line like `Fetch live documentation: https://...` pointing at the canonical upstream URL. The skill finds those stubs, checks whether the upstream changed, and refetches only what's stale.

This is for the specific workflow where you maintain a local mirror of upstream docs (vendor reference, framework docs, internal wiki snapshots) as part of your personal or team knowledge base. **Not** a general "fix all my docs" tool.

## When to use

- User says "update docs", "sync docs with online sources", "refresh local docs"
- User points at a folder and says "the stubs in here are stale"
- A local docs tree has files matching the stub pattern and the user wants them current
- Do NOT use to *write* new documentation. Use a writing-focused skill for that.
- Do NOT use on files that don't have the stub marker — the skill will skip them, but you shouldn't even point it there.

## Prerequisites

- A local directory containing markdown files with the stub pattern
- Bash + Python 3.10+ if using the bundled script (`scripts/doc_update.py`)
- Network access for the URLs in the stubs
- Optional: configurable stub pattern (default `Fetch live documentation: <URL>`)

## Steps

### 1. Inventory

Find every file matching the stub pattern under the target directory:

```bash
grep -rl "Fetch live documentation:" <target-dir> --include="*.md"
```

Then for each matched file, extract the URL. Report: total count, list of unique URLs, files-per-URL if any URL repeats. Show this to the user **before** any network requests — confirm scope.

### 2. Confirm with the user

Show the inventory and ask before proceeding. For >10 files, also note expected time (rule of thumb: ~1.5s/file with rate limiting). For >100 files, suggest breaking into batches by subdirectory.

### 3. Detect what changed

For each stub file:
- Fetch the upstream URL (with timeout, single retry on transient failure)
- Extract the main content region (`<main>`, `<article>`, or the largest content block)
- Compute a hash (SHA-256 of normalized text — strip whitespace at line ends, collapse runs of blank lines)
- Compute the same hash on the local file's content body (excluding the stub line)
- If hashes match: **skip** (already current)
- If hashes differ: **mark for update**
- If URL returns ≥400 or times out: **mark unreachable**

Rate limit: minimum 1 second between requests to the same host. Use the bundled script — `scripts/doc_update.py` — for the actual implementation. The script handles rate limiting, retry, and parallelism within rate limits.

### 4. Update changed files

For each "mark for update" file:
- Build the new body: extracted content + the stub line preserved at the bottom
- Write atomically (tmp file + rename) to avoid partial writes
- Log: old hash → new hash, byte delta

Never delete files. Never modify files that don't have the stub marker. Never touch files outside the target directory.

### 5. Report

Output a two-part report:

**Machine-readable JSON** (for downstream tooling):
```json
{
  "summary": {
    "scanned": 96,
    "updated": 12,
    "skipped_current": 82,
    "unreachable": 2,
    "errors": 0,
    "duration_seconds": 187
  },
  "updated": [
    {"path": "anthropic/configuration/settings.md", "url": "...", "old_hash": "a1b2...", "new_hash": "c3d4...", "delta_bytes": 412}
  ],
  "unreachable": [
    {"path": "...", "url": "...", "reason": "404 Not Found"}
  ],
  "errors": []
}
```

**Human summary** (one paragraph + the deltas):
```
Synced 96 stubs in 3 minutes. Updated 12 files (largest delta: configuration/settings.md +412 B). 82 already current. 2 unreachable (404): see report. No write errors.
```

## Outputs

- The JSON + human summary shown above
- Optionally write the JSON to `<target-dir>/.doc-stub-sync-report.json` if the user asks for it
- Only file modifications: the stub files themselves, atomically rewritten

## ALWAYS / NEVER

**ALWAYS**
- Rate-limit (≥1s between requests to the same host)
- Show inventory and ask for confirmation before any network requests
- Preserve the stub marker line in the rewritten file
- Atomic writes (tmp + rename)
- Continue on individual file failures; never abort the whole batch on one error
- Report unreachable URLs in the final summary

**NEVER**
- Modify files outside the target directory
- Modify files that don't contain the stub marker
- Delete files
- Overwrite files where the hash is unchanged (it's a no-op, but skip the write — preserves mtime for downstream tools that watch it)
- Fire requests at the same host faster than the rate limit, even on retries
- Trust the upstream content blindly — strip scripts, styles, and tracking elements before hashing/storing

## Examples

### Full sync

**User:** "Sync everything under `~/Documents/kb/anthropic/`."

**Agent:**
```
Found 96 stub files under ~/Documents/kb/anthropic/. They reference 96 unique URLs across 3 hosts (docs.claude.com, www.anthropic.com, github.com/anthropics).

Estimated time: ~3 minutes with rate limiting.

Proceed? [yes / no / dry-run]
```

After confirmation, runs `scripts/doc_update.py` and emits the JSON + summary report.

### Targeted

**User:** "Just the configuration subdirectory."

**Agent:**
```
Found 5 stub files under ~/Documents/kb/anthropic/configuration/:
  - settings.md → docs.claude.com/.../settings
  - model-config.md → docs.claude.com/.../model-config
  - terminal-config.md → docs.claude.com/.../terminal
  - memory.md → docs.claude.com/.../memory
  - statusline.md → docs.claude.com/.../statusline

Estimated time: ~10 seconds. Proceed?
```

### Dry-run

**User:** "What would change if I ran it now?"

**Agent:** Runs the hash comparison only, skipping the update step. Reports which files *would* change and which would stay.

## References

- [`references/url-patterns.md`](references/url-patterns.md) — recognized stub formats and how to add a new one
- [`references/error-handling.md`](references/error-handling.md) — HTTP error codes and how the skill responds to each
- [`scripts/doc_update.py`](scripts/doc_update.py) — bundled implementation (Python 3.10+)
- [`scripts/requirements.txt`](scripts/requirements.txt) — Python deps for the script

## Customizing the stub pattern

Default: `Fetch live documentation: <URL>` (case-sensitive, URL is the rest of the line).

To use a different pattern, pass `--stub-pattern` to the script with a regex containing one capture group for the URL. Example for a different convention:

```bash
python scripts/doc_update.py --stub-pattern 'Source: (https?://\S+)' <target-dir>
```

The skill body assumes the default pattern unless the user tells you otherwise. If you see they're using a different convention, ask first before assuming.
