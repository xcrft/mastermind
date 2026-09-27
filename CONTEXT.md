# Mastermind — Context

## Identity

| Topic | Definition |
|---|---|
| Product | Local codegraph and task-evidence workflow for coding agents |
| Runtime | Rust CLI/MCP server and portable client skills |
| Users | Maintainers and teams planning, implementing, and reviewing repository changes |
| Evidence boundary | Mechanical checks, execution records, and semantic judgments remain distinct |

See [Architecture](docs/architecture.md) for components and trust boundaries.

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

## Domain glossary

| Term | Meaning |
|---|---|
| Lesson candidate | Audit signal awaiting review before use as guidance |
| History review | Decision about further CONTEXT or lesson updates, bound to the reviewed file revisions |

## Updates

Record durable project knowledge here. A completed task may need no update.
Keep tutorials and component details in the linked documentation.
