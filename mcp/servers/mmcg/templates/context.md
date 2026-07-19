---
name: mastermind-context
description: Project-level CONTEXT.md template — accumulated institutional memory for the Mastermind workflow (identity, active goals, decision log, gotchas, glossary, external dependencies, don't-touch list). Lives at the project root alongside CLAUDE.md. Updated by the planner during post-flight semantic review when work surfaces something worth preserving across sessions.
metadata:
  version: 0.2.0
  authors:
    - mastermind
  tags:
    - claude-md
    - context
    - workflow
---

<!--
  Project-level CONTEXT.md template — institutional memory for the Mastermind workflow.

  HOW TO USE
  - Copy the body below (everything after the next ─── marker) to <project-root>/CONTEXT.md
  - Delete sections that don't apply
  - The planner (via `mastermind-task-planning` skill) updates this file during
    post-flight semantic review when new discoveries surface (decisions, gotchas,
    glossary entries)
  - The agent reads CONTEXT.md on session start (via a pointer in CLAUDE.md)

  WHAT BELONGS HERE
  - Things that are TRUE about the project but NOT derivable from the code
  - Decisions, their dates, and *why* — so the audit trail survives author handoff
  - Surprises discovered during work — "we tried X, it broke because Y"
  - Domain terms that mean something specific in this codebase

  WHAT DOES NOT BELONG HERE
  - Style conventions, command lists, architecture — those go in CLAUDE.md
  - Ephemeral state — current in-progress work (use `.mastermind/tasks/`)
  - Anything inferable by reading the code or `git log`
  - Sycophancy ("the team is amazing"), trivia, generic best practices

  GROWTH RULE
  - Prune the Active goals section regularly — finished goals leave, they don't accumulate
  - Decision log and Gotchas grow append-only — past decisions are reference, not clutter
  - If a section gets longer than ~50 entries, archive the oldest half to `CONTEXT-archive-<YYYY>.md`
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME> — Context

## Identity

**What it is:** <one or two sentences — be specific about scope>

**What it is not:** <one or two sentences — useful to bound scope; e.g., "not a real-time system", "not for external users yet">

**Primary users:** <internal team / external customers / open-source contributors>

---

## Active goals

*What this project is trying to achieve right now. Prune as goals complete — this section is not append-only.*

- <Goal 1 — concrete and measurable, e.g., "Ship v2 auth migration by Q3">
- <Goal 2>

---

## Decision log

*Most recent first. Append-only: supersede old decisions explicitly instead of rewriting history.*

<!--
Add an entry only for durable knowledge. Use a dated level-three heading and
fields: Decision, Why, Status, Supersedes, Provenance, Evidence, Alternatives
rejected, Source, optional Critic verdict, and Reusable lesson.
-->

---

## Known gotchas

*Verified surprises discovered during work. Keep empty until one exists.*

---

## Domain glossary

*Terms that mean something specific in this codebase. Keep empty until one exists.*

---

## External dependencies

*Non-derivable constraints for services or vendors. Keep empty until one exists.*

---

## Don't-touch list

*Code or areas with verified hidden constraints. Keep empty until one exists.*

---

## How this file gets updated

The planner (`mastermind-task-planning` skill) appends to this file during post-flight semantic review when work surfaces something worth preserving across sessions:

| Discovery type | Section to update |
|---|---|
| Non-trivial design decision the critic agreed with | Decision log |
| Workflow surprised by something — "almost broke X" | Known gotchas |
| New term that took explaining during brainstorming | Domain glossary |
| New external dependency added | External dependencies |
| Code area found to have hidden constraints | Don't-touch list |

After a completed task, resolve its `history-review.md`: mark Context and Lesson
as `updated` or `not applicable` and record the reason. Never add an entry only
to make the review green.

Approval proves that an authorized person accepted a decision. It does not prove
that a runtime, security, performance, or compatibility claim is technically
true; those claims need their own evidence.

<!-- ─── COPY TO HERE ─── -->
