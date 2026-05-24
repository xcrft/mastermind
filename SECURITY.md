# Security policy

## Reporting a vulnerability

**Do not open a public issue for security problems.** Use GitHub's private vulnerability reporting:

1. Go to <https://github.com/xcrft/mastermind/security/advisories/new>
2. Describe the issue with enough detail to reproduce — the more concrete, the faster the fix
3. Include affected versions if you know them (e.g. `mmcg 0.6.0`, plugin `mastermind-workflow 0.6.0`)

If GitHub's private reporting is unavailable to you, email the main contributor (see [`CONTRIBUTORS.md`](CONTRIBUTORS.md) for current handle → email is on the GitHub profile).

## What we consider in scope

- **`mmcg` (Rust crate):** parser bugs that crash on malformed input, SQL injection via crafted file paths, path traversal during `mmcg index` or `mmcg init`, MCP protocol handling bugs
- **Workflow artifacts:** skills/subagents that instruct an LLM to take dangerous actions (delete files, exfil data, bypass user approval) when invoked normally
- **Plugin manifests / build scripts:** anything that runs untrusted code at install or build time

## What we do NOT consider in scope

- LLM jailbreaks against the subagents — the workflow assumes the user trusts the model
- Vulnerabilities in upstream dependencies (`tree-sitter-*`, `rusqlite`, `notify`, etc.) — report to those projects; we'll bump versions
- Self-induced issues (e.g. running `mmcg index` on a path you don't trust, then complaining about what the indexer wrote there)
- Findings from automated scanners with no proof of exploitability

## Response expectations

This is a small-team OSS project. Response timing is best-effort:

- Acknowledgement: within 1 week
- Triage / first reply: within 2 weeks
- Fix shipped: depends on severity (sev0 days, sev1 weeks, lower handled in next normal release)

If a fix is non-trivial we may publish a GitHub Security Advisory with a CVE. Reporters who want credit get attribution in the advisory.

## Disclosure

We follow coordinated disclosure: please give us a reasonable window (default 90 days) before public disclosure. We'll work with you on the timeline if a quick fix isn't possible.
