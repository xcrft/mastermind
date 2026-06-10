# Error handling — HTTP responses and how the skill reacts

Reference for the [`doc-stub-sync`](../SKILL.md) skill. The script behavior is in [`../scripts/doc_update.py`](../scripts/doc_update.py).

## How errors are classified

Every stub processed ends up in one of four buckets in the report:

- `updated` — content changed, file rewritten
- `current` — content unchanged, file untouched
- `unreachable` — request failed or returned ≥400, file untouched
- `error` — local issue (permissions, disk, encoding), file untouched

A stub never silently fails. Every problem lands in one bucket and is reported.

## HTTP response handling

| Status | Bucket | Skill action |
|---|---|---|
| 2xx | `current` / `updated` | Parse, hash, compare, maybe rewrite |
| 301, 302, 307, 308 | (followed) | The HTTP client follows redirects automatically; the *final* response is what the skill evaluates. If the final response is ≥400, treat as unreachable. |
| 304 Not Modified | `current` | Treated as no-change. (Note: the bundled script doesn't send `If-Modified-Since` yet — 304 in practice means the server volunteered it.) |
| 401, 403 | `unreachable` | Auth required or forbidden. Don't retry with credentials — surface to user and stop. |
| 404, 410 | `unreachable` | Upstream page is gone. The user has to decide: update the stub's URL, delete the file, or accept the staleness. The skill does NOT auto-delete. |
| 408, 429, 503, 504 | `unreachable` (after one retry) | Transient. Script retries once after a 2-second pause. Still failing → unreachable. |
| 5xx (other) | `unreachable` (after one retry) | Same handling as transient. |
| Network timeout | `unreachable` (after one retry) | Single retry with backoff, then give up for this run. |
| DNS / connection refused | `unreachable` | No retry — the host isn't there. |

## Retry policy

The script retries **once** on a network-level failure (timeout, connection error, DNS). It does **not** retry on HTTP status codes ≥400 — those represent a definitive answer from the server, not a transient transport issue.

Why one retry, not more:
- Most transient failures are resolved by waiting 1-2 seconds
- More retries on a doc-sync job means slowing down the whole batch by minutes
- 429 (rate limit) deserves a different response: respect the limit, don't hammer

If the user is hitting 429 repeatedly, the fix is `--rate-limit` (raise it), not more retries.

## Rate limiting

The script enforces a per-host minimum interval between requests. Default: 1.0 second.

This applies *per host*, not globally: syncing 50 stubs from `docs.claude.com` and 30 from `github.com` runs them in parallel as far as the rate limiter is concerned. Within each host, requests are serialized with the minimum gap.

Raise `--rate-limit` to 2.0 or higher if the upstream is sensitive (small docs site, single-server) or you've seen 429s.

## Local errors

| Situation | Bucket | What to do |
|---|---|---|
| File not writable (permissions) | `error` | Surface to user. They fix the perms and re-run. |
| Disk full | `error` | Surface. Don't keep trying. |
| File contains invalid UTF-8 | (skipped at discovery) | These files aren't even inventoried. If the user expects them to be stubs, they have a separate problem. |
| Atomic rename fails (race, cross-device tmp) | `error` | Surface the underlying OS error verbatim. Often means `tempfile.mkstemp` landed on a different filesystem than the target — usually a misconfigured `TMPDIR`. |

## What the user sees on error

Every error appears in two places:
1. The JSON report (`errors` array, with `path`, `url`, `reason`)
2. The human summary (count + first 3 reasons inline, full list pointer)

Example human summary on errors:

```
Synced 96 stubs in 4 minutes. Updated 8. 80 already current. 6 unreachable (4×404, 2×timeout): see report. 2 errors (permission denied): see report.
```

If the run had *only* errors and no updates, exit code is non-zero — pipelines and cron jobs that depend on the script can detect this.

## What the agent does with the report

The skill's job is to **report**, not to **act on** errors. The user decides:
- A 404 on a doc → maybe the upstream renamed it. Update the stub URL or delete the local file.
- 401/403 → maybe the doc moved behind auth. Out of scope for this skill.
- A permissions error → the user fixes the OS-level perms and re-runs.

The skill does **not** edit stub URLs, delete files, or modify other files in response to errors. Those are destructive operations and they belong to the user (or to a separate, explicitly-invoked tool).
