# Security policy

If you found a way to cross a documented trust boundary, tell us privately so
we can check it without exposing users or working exploit details.

## Supported versions

| Version | Security fixes |
|---|---|
| 4.x | Supported |
| 3.x | Supported |
| 2.x and earlier | Upgrade required |

## Report privately

Use [GitHub private vulnerability reporting](https://github.com/xcrft/mastermind/security/advisories/new),
not a public issue or discussion. If it is unavailable, ask the maintainer for a
private channel through the GitHub profile. Include:

- the Mastermind version and installation method
- the operating system and the relevant client and runtime versions
- a minimal reproduction or proof of concept
- the expected and the actual security boundary
- the impact, the attacker access it needs and whether untrusted repository
  content is involved
- logs with credentials, tokens, private source and personal data removed

## What is in scope

- Path traversal, unsafe overwrite or command execution in indexing, setup,
  install, export or workflow commands
- SQL injection or database corruption through crafted repository paths or
  imported evidence
- MCP protocol or tool-permission defects that cross the read-only and
  additive-write boundary in the [MCP reference](docs/reference/mcp.md)
- Lens binding, same-origin, script injection, CSP or source-index mutation
  defects
- Bypasses of fact, SARIF, coverage, JUnit, OTLP, SCIP, signature, revision,
  provenance, size or path validation
- Workflow artifacts that tell an agent to expose secrets, skip explicit
  approval or run destructive actions during normal documented use
- npm, Cargo, Docker Action, GitHub Actions or release provenance defects that
  can replace or publish unverified artifacts
- Setup or uninstall bugs that change unrelated client configuration or user
  files

Usually out of scope, though a private report is still welcome when you are
unsure:

- an upstream dependency advisory without a shown Mastermind impact
- a generic model jailbreak that bypasses no Mastermind-enforced boundary
- denial of service that needs the local user to index a hostile repository,
  unless it bypasses a documented size or work limit or loses data
- scanner output without a reproducible path and impact
- social engineering, account compromise, or GitHub, npm or crates.io platform
  issues

## How Mastermind treats repository content

Mastermind treats repository source and its history as untrusted input. It
opens files relative to the selected repository without following symlinks or
Windows reparse points, rejects special files and path escapes, enforces
per-file and total limits, and checks the file identity before and after each
read. The index is local derived data, so protect it like the repository. The
filter that drops secret-like documentation tokens reduces accidental
credential storage, but it is not a secret scanner and doesn't protect against
a hostile local machine. The exact limits are in
[managed index refresh](docs/reference/mcp.md#managed-index-refresh) and
[concept search](docs/reference/concept-search.md).

## Response and disclosure

This is a small-maintainer project, so these targets are best effort:

| Stage | Target |
|---|---|
| Acknowledgement | 7 days |
| Initial triage | 14 days |
| Remediation | Based on severity and release safety |

We use coordinated disclosure. Please allow up to 90 days and don't publish
details while a fix or registry release is in progress. Material issues may get
a GitHub Security Advisory and a CVE, with reporter credit when you ask for it.
