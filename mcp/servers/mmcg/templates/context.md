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

### <YYYY-MM-DD> — <Short decision name>

- **Decision:** <one sentence>
- **Why:** <the reason that survives author handoff>
- **Status:** <`active` or `superseded`>
- **Supersedes:** <earlier decision heading or `none`>
- **Provenance:** <who or what authorized/recorded the decision; origin is not proof>
- **Evidence:** <code, test, runtime observation, audit, or `decision only — not technically verified`>
- **Alternatives rejected:**
  - <option A>: <why rejected>
  - <option B>: <why rejected>
- **Source:** <`.mastermind/tasks/NNN-name/spec.md` or a discussion link>
- **Critic verdict** (if applicable): <`ship it` / `ship with caveats` / etc.>
- **Reusable lesson:** <what future work should preserve or avoid>

### <YYYY-MM-DD> — <Earlier decision>

...

---

## Known gotchas

*Surprises discovered during work — one line each. Keeps the next workflow run from re-discovering. Append-only.*

- **<gotcha 1>** — <concrete bite scenario, e.g., "Redis cluster mode silently drops MULTI on key migrations during rebalance">. Discovered in `.mastermind/tasks/<NNN>-<name>/`.
- **<gotcha 2>** — <bite scenario>.

---

## Domain glossary

*Terms that mean something specific in this codebase. Skip terms that mean the standard thing. Append-only.*

- **<term 1>** — <local meaning>
- **<term 2>** — <local meaning, e.g., "Reservation = booking attempt before payment confirmation; not the same as Booking">

---

## External dependencies

*Services / APIs / vendors this project relies on. Include auth mechanism and (if pinned) version.*

- **<service>** — <what we use it for> — auth: <API key in env `VAR` / OAuth / mTLS> — version `<X.Y or latest>`

---

## Don't-touch list

*Code or areas with hidden constraints. The next planner needs to know NOT to "while I'm in there" these.*

- **`<path>`** — <constraint, e.g., "generated from `proto/`, do not hand-edit">
- **`<path>`** — <constraint, e.g., "stub kept for backwards compat with v0.4 clients; removable after 2026-09-01">

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

The planner does NOT update this file silently. Every change is logged in the spec's Notes section so the audit trail is preserved.

Approval proves that an authorized person accepted a decision. It does not prove
that a runtime, security, performance, or compatibility claim is technically
true; those claims need their own evidence.

<!-- ─── COPY TO HERE ─── -->
