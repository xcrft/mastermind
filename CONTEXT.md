# Mastermind — Context

## Identity

| Topic | Definition |
|---|---|
| Product | Local codegraph and task-evidence workflow for coding agents |
| Runtime | Rust CLI/MCP server and portable client skills |
| Users | Maintainers and teams planning, implementing, and reviewing repository changes |
| Evidence boundary | Mechanical checks, execution records, and semantic judgments remain distinct |

See [how it works](README.md#how-it-works) for components and trust boundaries.

## Active goals

- Align CLI, MCP, installed instructions, documentation, and evaluation contracts.
- Preserve project knowledge with inspectable sources and explicit review.

## Decision log

### 2026-07-19 — Markdown remains authoritative project memory

- **Decision:** Keep CONTEXT, archived context, task evidence, and reviewed lessons authoritative. SQLite is a rebuildable retrieval index.
- **Why:** Knowledge must survive index rebuilds and remain readable without Mastermind.
- **Status:** active
- **Supersedes:** none
- **Provenance:** Repository maintainer requested the context and lessons lifecycle.
- **Evidence:** [indexer.rs](mcp/servers/mmcg/src/indexer.rs), [context_doctor.rs](mcp/servers/mmcg/src/context_doctor.rs), [lessons.rs](mcp/servers/mmcg/src/lessons.rs), and their tests.
- **Alternatives rejected:** Authoritative SQLite memory and automatic promotion of audit findings into active lessons.
- **Source:** Implementation and verification recorded on 2026-07-19.
- **Reusable lesson:** Separate mechanical observations from reviewed guidance.

### 2026-10-14 — The native reviewer answers through a schema-validated structured output

- **Decision:** `review-task run` passes Claude Code `--json-schema` with the closed `NativeAssessment` schema. The reviewer answers through Claude Code's `StructuredOutput` tool, which re-prompts the same model when the answer does not match. Mastermind still parses `result` strictly and requires it to equal `structured_output`; a rejected `StructuredOutput` attempt is not a tool error. New review receipts use policy v2 (Claude Code 2.1.290 or later); v1 stays frozen for historical admission.
- **Why:** Prompt-only output rules failed in practice. A reviewer fenced its JSON and added a field, and the whole paid review was discarded.
- **Status:** active
- **Supersedes:** none
- **Provenance:** A collaborator's report of a lost native review; the captured stream matched the receipt digests.
- **Evidence:** [invocation.rs](mcp/servers/mmcg/src/invocation.rs), [review_invocation.rs](mcp/servers/mmcg/src/review_invocation.rs), [task-runtime.md](docs/reference/task-runtime.md), and their tests.
- **Alternatives rejected:** Stripping a Markdown fence (contradicts the documented assessment-source rule and leaves extra fields), a repair pass by a second model (can change verdicts while the record names the first reviewer), and prompt-only rules (already present and ignored).
- **Source:** Implementation and verification recorded on 2026-10-14.
- **Reusable lesson:** Enforce a model's output shape where the answer is generated, keep the strict parse as a second check, and version any contract that stored receipts depend on.

## Domain glossary

| Term | Meaning |
|---|---|
| Lesson candidate | Audit signal awaiting review before use as guidance |
| History review | Decision about further CONTEXT or lesson updates, bound to the reviewed file revisions |
