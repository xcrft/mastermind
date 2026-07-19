# Mastermind — Context

## Identity

**What it is:** An open-source, local-first codegraph and verifiable workflow
for AI coding agents. It provides a Rust CLI/MCP server plus portable workflow
skills for supported coding clients.

**What it is not:** A hosted agent runtime, a model provider, or proof that an
agent's semantic judgment is correct.

**Primary users:** Maintainers and teams using coding agents to understand,
plan, implement, and audit repository changes.

---

## Active goals

- Keep CLI, MCP, workflow skills, documentation, and evaluation contracts aligned.
- Preserve project knowledge without turning generated observations into unsupported facts.

---

## Decision log

### 2026-07-19 — Markdown remains authoritative project memory

- **Decision:** Keep `CONTEXT.md`, archived context, task evidence, and reviewed lessons as the source of truth; SQLite remains a rebuildable retrieval index.
- **Why:** Human-readable evidence must survive index rebuilds and remain inspectable without Mastermind.
- **Status:** active
- **Supersedes:** none
- **Provenance:** repository maintainer requested the context and lessons lifecycle hardening.
- **Evidence:** `mcp/servers/mmcg/src/indexer.rs`, `mcp/servers/mmcg/src/context_doctor.rs`, `mcp/servers/mmcg/src/lessons.rs`, and their tests.
- **Alternatives rejected:**
  - SQLite as authoritative memory: rejected because a local generated database is not portable or reviewable project history.
  - Automatic audit findings as active lessons: rejected because a mechanical verdict does not establish a reusable root cause.
- **Source:** implementation and verification performed on 2026-07-19.
- **Reusable lesson:** Separate observed audit signals from reviewed project guidance, and make the transition explicit.

---

## Known gotchas

*Verified project-specific surprises only. Keep empty until one exists.*

---

## Domain glossary

- **Lesson candidate** — a deduplicated mechanical audit signal awaiting semantic review; it is not active guidance.
- **History review** — the per-task disposition that records whether CONTEXT or lessons were updated or intentionally left unchanged.

---

## External dependencies

*Non-derivable constraints for services or vendors. Keep empty until one exists.*

---

## Don't-touch list

*Code or areas with verified hidden constraints. Keep empty until one exists.*

---

## How this file gets updated

Update this file only after semantic review produces durable project knowledge.
Every completed task may explicitly conclude that no CONTEXT update is needed.
