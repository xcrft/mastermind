# Security policy

Found a way to cross a documented trust boundary? Tell us privately so we can
verify it without exposing users or working exploit details.

## Supported versions

| Version | Security fixes |
|---|---|
| 2.x | Supported |
| 1.x and earlier | Upgrade required |

## Report privately

Do not open a public issue or discussion. Use
[GitHub private vulnerability reporting](https://github.com/xcrft/mastermind/security/advisories/new).

Include:

- affected Mastermind version and installation method.
- operating system and relevant client/runtime versions.
- minimal reproduction or proof of concept.
- expected and actual security boundary.
- impact, required attacker access, and whether untrusted repository content is
  involved.
- logs with credentials, tokens, private source, and personal data removed.

If private reporting is unavailable, contact the maintainer through the GitHub
profile and request a private channel. Do not send exploit details in a public
comment.

## In scope

- path traversal, unsafe overwrite, or command execution in indexing, setup,
  install, export, or workflow commands.
- SQL injection or database corruption through crafted repository paths or
  imported evidence.
- MCP protocol or tool-permission defects that cross the documented read-only
  and additive-write boundary.
- Lens binding, same-origin, script-injection, CSP, or source-index mutation
  defects.
- fact, SARIF, coverage, JUnit, OTLP, SCIP, signature, revision, provenance,
  size, or path validation bypasses.
- workflow artifacts that instruct an agent to expose secrets, bypass explicit
  approval, or perform destructive actions during normal documented use.
- npm, Cargo, Docker Action, GitHub Actions, or release-provenance defects that
  can replace or publish unverified artifacts.
- setup/uninstall ownership bugs that modify unrelated client configuration or
  user files.

## Usually out of scope

- an upstream dependency advisory without a demonstrated Mastermind impact.
- generic model jailbreaks that do not bypass a Mastermind-enforced boundary.
- denial of service that requires the local user to index an intentionally
  hostile repository, unless it bypasses a documented size/work limit or causes
  persistent data loss.
- scanner output without a reproducible path and impact.
- social engineering, account compromise, or GitHub/npm/crates.io platform
  issues outside this repository's control.

We still welcome a private report when scope is uncertain.

## Local index and repository boundary

Repository source and durable history are untrusted local inputs. Mastermind
opens admitted files relative to the selected repository capability without
following symlinks or Windows reparse points, rejects special files and path
escapes, enforces per-file and aggregate limits, and compares descriptor
identity before and after reads. History is derived retrieval evidence.
Markdown remains authoritative, and its structural freshness is tracked
separately from the source graph.

| Boundary | Enforcement |
|---|---|
| One MCP request | Absolute deadline and cancellation across SQLite, Git, file reads, refresh, single retry, snapshots and serialization |
| Cancellation | `cancelled` |
| Deadline expiry | `work_limit_exceeded` |
| Automatic refresh | At most 20,000 source candidates and 512 MiB declared source bytes |
| Writable refresh | Only canonical `ROOT/.mastermind/mmcg.db` opened by `serve` |
| Custom `--index` | Read-only snapshot, no creation, migration, truncation or source-side SQLite sidecars |
| Incompatible schema | `schema_incompatible` |

Concept documentation is derived from untrusted repository text. Rust outer
docs, Python owned docstrings, and JavaScript/TypeScript JSDoc are inspected
while their declaration AST is available, but raw text is never written to the
index or returned by a tool. Only bounded normalized search tokens are stored.

| Documentation stage | Limit or rejection |
|---|---|
| Raw inspection | 2 KiB per candidate |
| Normalized tokens | 512 UTF-8 bytes per symbol, 64 KiB per file |
| Whole-candidate omission | Private-key header, AWS access-key shape, bearer value or non-placeholder api-key/secret/token/password assignment |

These checks reduce accidental credential persistence. They are not a general
secret scanner or a confidentiality boundary for a hostile local machine. The
SQLite index remains local derived data and should receive the same filesystem
protection as the repository. Count-only omission metadata and query precision
notes never include documentation excerpts.

## Response and disclosure

This is a small-maintainer open-source project. Targets are best effort:

| Stage | Target |
|---|---|
| Acknowledgement | 7 days |
| Initial triage | 14 days |
| Remediation | Based on severity and release safety |

We use coordinated disclosure. Please allow up to 90 days by default and avoid
publishing details while a fix or registry release is in progress. Material
issues may receive a GitHub Security Advisory and CVE. Reporter credit is
included when requested.
