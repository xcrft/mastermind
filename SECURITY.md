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

- affected Mastermind version and installation method;
- operating system and relevant client/runtime versions;
- minimal reproduction or proof of concept;
- expected and actual security boundary;
- impact, required attacker access, and whether untrusted repository content is
  involved;
- logs with credentials, tokens, private source, and personal data removed.

If private reporting is unavailable, contact the maintainer through the GitHub
profile and request a private channel. Do not send exploit details in a public
comment.

## In scope

- path traversal, unsafe overwrite, or command execution in indexing, setup,
  install, export, or workflow commands;
- SQL injection or database corruption through crafted repository paths or
  imported evidence;
- MCP protocol or tool-permission defects that cross the documented read-only
  and additive-write boundary;
- Lens binding, same-origin, script-injection, CSP, or source-index mutation
  defects;
- fact, SARIF, coverage, JUnit, OTLP, SCIP, signature, revision, provenance,
  size, or path validation bypasses;
- workflow artifacts that instruct an agent to expose secrets, bypass explicit
  approval, or perform destructive actions during normal documented use;
- npm, Cargo, Docker Action, GitHub Actions, or release-provenance defects that
  can replace or publish unverified artifacts;
- setup/uninstall ownership bugs that modify unrelated client configuration or
  user files.

## Usually out of scope

- an upstream dependency advisory without a demonstrated Mastermind impact;
- generic model jailbreaks that do not bypass a Mastermind-enforced boundary;
- denial of service that requires the local user to index an intentionally
  hostile repository, unless it bypasses a documented size/work limit or causes
  persistent data loss;
- scanner output without a reproducible path and impact;
- social engineering, account compromise, or GitHub/npm/crates.io platform
  issues outside this repository's control.

We still welcome a private report when scope is uncertain.

## Local index and repository boundary

Repository source and durable history are untrusted local inputs. Mastermind
opens admitted files relative to the selected repository capability without
following symlinks or Windows reparse points, rejects special files and path
escapes, enforces per-file and aggregate limits, and compares descriptor
identity before and after reads. History is derived retrieval evidence;
Markdown remains authoritative, and its structural freshness is tracked
separately from the source graph.

One absolute MCP request deadline and cancellation state spans SQLite, Git,
filesystem inventory and reads, automatic refresh, the single retry, private
snapshots, and serialization. A cancel reports `cancelled`; deadline expiry
reports `work_limit_exceeded`. Automatic refresh is limited to 20,000 source
candidates and 512 MiB of declared source bytes.

Only the canonical `ROOT/.mastermind/mmcg.db` opened by `serve` is eligible for
automatic writable refresh. A custom `--index` is served through a read-only
snapshot and is never created, migrated, truncated, or given source-side
SQLite sidecars; an incompatible schema fails as `schema_incompatible`.

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
