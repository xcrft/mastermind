# Mastermind product roadmap

Mastermind should grow as a local, deterministic engineering layer for coding
agents: first compute evidence from the repository, then let agents explain and
act on that evidence. New skills must not simulate capabilities that the binary
cannot verify.

This roadmap starts from the current product, not a rewrite:

- `mmcg` already indexes nine languages and exposes 20 MCP tools over stdio.
- `verify-spec`, `audit-spec`, `run-task`, and `mastermind ci` already provide
  mechanical workflow gates.
- `audit-spec --bundle` already emits a portable JSON bundle, and `pr-comment`
  already renders it for GitHub.
- Installation is automated for Claude Code; Cursor, Continue, Codex, and
  generic MCP clients currently rely on manual integration guides.

## Delivery order

| Priority | Milestone | User outcome | Depends on |
|---|---|---|---|
| P0 | MCP foundation | Current clients receive a truthful, versioned MCP contract | current stdio server |
| P1 | `mastermind map` | One command explains an unfamiliar repository | healthy codegraph |
| P1 | Cross-client setup | One installer configures every supported MCP client | MCP foundation |
| P1 | Change and test impact | A diff answers what can break and which tests matter | codegraph + git diff |
| P1 | Verifiable audit + GitHub Action | PRs carry independently checkable workflow evidence | existing audit bundle + `mastermind ci` |
| P2 | Product skills | Agents consume each deterministic capability consistently | the corresponding engine feature |

The P1 items can be developed independently after P0. `mastermind map` should
ship first because it is the clearest new user-facing reason to install the
product.

## P0 — MCP foundation

Upgrade the existing Rust stdio server without replacing it with an SDK or
adding a network transport.

Deliverables:

- Negotiate the stable MCP `2025-11-25` revision while preserving tested
  `2024-11-05` compatibility.
- Enforce a closed connection lifecycle: `Cold -> Negotiated -> Ready`.
- Distinguish JSON parse errors, invalid JSON-RPC envelopes, invalid MCP params,
  tool execution errors, and internal failures.
- Return structured tool content to current clients while retaining the legacy
  text payload. Version-gate modern fields so a legacy client keeps its current
  wire contract.
- Advertise 19 read-only tools and the one additive, non-idempotent local write
  (`mmcg_scratchpad_append`) truthfully.
- Bound request and response frames, redact protocol diagnostics, and keep
  notification handling side-effect free except for the explicit
  `notifications/initialized` transition.
- Update the built-in `doctor` handshake and add transcript-level regression
  tests. The official conformance runner is HTTP/URL-oriented today, so stdio
  conformance remains an in-repo transcript gate until a supported adapter is
  available.

The executable implementation contract lives in
`.mastermind/tasks/001-mcp-foundation/spec.md`.

## P1 — `mastermind map`

`mastermind map` should turn the codegraph into a compact architecture briefing,
not dump every symbol.

Proposed surface:

```text
mastermind map [path] [--format text|json|mermaid] [--depth N]
```

The deterministic result should contain:

- language and package/module inventory;
- likely entry points and structurally central symbols;
- empirical module boundaries from external callers and imports;
- dependency cycles and boundary crossings;
- recently changed structural areas when git history is available;
- precision notes wherever name collisions or syntactic resolution weaken a
  conclusion.

The stable JSON form is the product contract. Text and Mermaid are projections
of that JSON. The matching MCP tool should return the same data, not implement a
second map algorithm.

Acceptance criteria:

- a new contributor can identify the main components and entry points without
  reading the whole tree;
- every claim links back to files/symbols already present in the index;
- large repositories are summarized with explicit limits rather than producing
  unbounded output;
- snapshot fixtures cover monorepos, cycles, isolated packages, and same-name
  symbol collisions.

## P1 — real cross-client setup

Replace manual copy/paste guides with client adapters behind one command:

```text
mastermind setup <claude|codex|cursor|continue|generic>
mastermind setup <client> --write
mastermind setup <client> --remove
mastermind doctor --client <client>
```

Each adapter owns its actual config format and scope rules. Setup must preserve
unrelated servers and settings, default to a dry-run diff, write atomically, and
make rollback possible. Client formats and paths must be verified against the
current client release when implementation starts; documentation alone is not
evidence of current behavior.

Acceptance criteria:

- project and user scope are explicit;
- running setup twice is idempotent;
- customized entries are never overwritten without an explicit force flag;
- Windows, macOS, and Linux path/command forms have fixtures;
- `doctor` validates the config that the selected client really reads and runs
  a complete MCP handshake.

This milestone installs the MCP server. Porting Claude-specific subagent runtime
semantics to every client is a separate decision, not an implied promise.

## P1 — change impact and test impact

The current `mmcg_impact` answers a symbol-level question. The product feature
should answer a change-level question:

```text
mastermind impact --since <git-ref> [--format text|json]
```

The result should aggregate:

- added, removed, and signature-changed symbols;
- direct and transitive callers with collision confidence;
- imports and API-surface crossings;
- affected packages/modules;
- candidate test files and test symbols, each labeled `direct`, `transitive`, or
  `heuristic` with the evidence that produced the label.

Test impact must remain a prioritization signal, never a claim that unlisted
tests are safe to skip. The command should recommend a focused set and preserve
the repository's full-suite gate at the phase/final boundary.

The CLI and MCP surfaces should share one response model, tentatively exposed as
`mmcg_change_impact` and `mmcg_test_impact`.

## P1 — cryptographically verifiable audit and GitHub Action

Build on the existing bundle instead of creating a parallel audit engine.

First make the artifact tamper-evident:

- version the bundle schema;
- use a precisely specified canonical serialization;
- bind the complete security-relevant manifest, including repository identity,
  full baseline and HEAD object IDs, tool version/config, spec and executor
  report digests, diff metadata, verdict, claims, findings, and commands;
- add `mastermind audit verify <bundle>`;
- support a detached signature or OIDC-backed attestation when an external trust
  root is configured.

Unsigned hashes prove consistency only relative to independently trusted inputs.
They do not prove authorship or protect a bundle that an attacker can rewrite
along with its hashes. Signed provenance proves only what the verifier policy
accepts: issuer, repository, workflow identity, ref, key/certificate lifetime,
rotation, revocation, and verification time must all be explicit. Never call the
artifact tamper-proof.

Then ship a GitHub Action that:

- runs `mastermind ci` on `pull_request` with read-only permissions and without
  secrets or OIDC;
- uploads the bundle and machine-readable result;
- uses a separate privileged publication job that never checks out or executes
  pull-request code, validates provenance and the exact head SHA, then updates
  the PR check/comment;
- pins third-party Actions by full commit SHA;
- grants only `contents: read` plus the minimum check/comment permission, and
  grants `id-token: write` only to the attestation job.

`pull_request_target` must never execute or index untrusted pull-request code.

## P2 — skills to add

Skills ship after their deterministic backend so they standardize use rather
than invent answers.

### `mastermind-project-map`

Runs `mastermind map`, selects the right depth/format, explains weak-confidence
edges, and turns the deterministic result into a reading order for an unfamiliar
repository.

### `mastermind-change-impact`

Builds a pre-edit or PR impact brief from `mastermind impact`: affected public
surface, consumers, migration risk, and the exact evidence behind each claim.

### `mastermind-test-impact`

Produces a focused test plan from the deterministic test-impact response, keeps
confidence labels, and never treats the focused set as a replacement for the
project's required full gate.

### `mastermind-cross-client-setup`

Chooses the requested client/scope, runs setup in dry-run mode first, explains
the config diff, and verifies the result through `doctor`.

### `mastermind-audit-attestation`

Generates, verifies, and explains audit bundles and CI attestations. It must
separate content integrity, provenance authenticity, and policy acceptance in
every report.

## Explicit non-goals for this sequence

- No hosted codegraph or mandatory cloud account.
- No replacement of the existing Rust server with an MCP SDK solely for API
  fashion.
- No HTTP/OAuth surface until a remote deployment use case exists.
- No claim that syntactic edges are semantic program analysis.
- No separate safe-onboarding project in this roadmap; it was not selected for
  this delivery sequence.
