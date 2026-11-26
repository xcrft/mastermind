# Security policy

Found a way to cross a documented trust boundary? Tell us privately so we can
verify it without exposing users or working exploit details.

## Supported versions

| Version | Security fixes |
|---|---|
| 4.x | Supported |
| 3.x | Supported |
| 2.x and earlier | Upgrade required |

## Report privately

Do not open a public issue or discussion. Use
[GitHub private vulnerability reporting](https://github.com/xcrft/mastermind/security/advisories/new).

Include:

- Affected Mastermind version and installation method.
- Operating system and relevant client/runtime versions.
- Minimal reproduction or proof of concept.
- Expected and actual security boundary.
- Impact, required attacker access, and whether untrusted repository content is
  involved.
- Logs with credentials, tokens, private source, and personal data removed.

If private reporting is unavailable, contact the maintainer through the GitHub
profile and request a private channel. Do not send exploit details in a public
comment.

## In scope

- Path traversal, unsafe overwrite, or command execution in indexing, setup,
  install, export, or workflow commands.
- SQL injection or database corruption through crafted repository paths or
  imported evidence.
- MCP protocol or tool-permission defects that cross the documented read-only
  and additive-write boundary.
- Lens binding, same-origin, script-injection, CSP, or source-index mutation
  defects.
- Fact, SARIF, coverage, JUnit, OTLP, SCIP, signature, revision, provenance,
  size, or path validation bypasses.
- Workflow artifacts that instruct an agent to expose secrets, bypass explicit
  approval, or perform destructive actions during normal documented use.
- npm, Cargo, Docker Action, GitHub Actions, or release-provenance defects that
  can replace or publish unverified artifacts.
- Setup/uninstall ownership bugs that modify unrelated client configuration or
  user files.

## Usually out of scope

- An upstream dependency advisory without a demonstrated Mastermind impact.
- Generic model jailbreaks that do not bypass a Mastermind-enforced boundary.
- Denial of service that requires the local user to index an intentionally
  hostile repository, unless it bypasses a documented size/work limit or causes
  persistent data loss.
- Scanner output without a reproducible path and impact.
- Social engineering, account compromise, or GitHub/npm/crates.io platform
  issues outside this repository's control.

We still welcome a private report when scope is uncertain.

## Local index and repository boundary

Mastermind treats repository source and its history as untrusted input. It
opens files relative to the selected repository without following symlinks or
Windows reparse points, rejects special files and path escapes, enforces
per-file and total limits, and checks the file identity before and after each
read. The index is local derived data, so protect it like the repository
itself. The documentation-token filter that drops secret-like text reduces
accidental credential storage, but it is not a secret scanner and doesn't
protect against a hostile local machine.

The exact limits are in the reference: request deadlines, refresh caps and
read-only custom indexes in [MCP server](docs/reference/mcp.md#managed-index-refresh),
and the documentation token rules in
[concept search](docs/reference/concept-search.md).

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
