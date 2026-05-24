---
name: mastermind-workflow
description: CLAUDE.md template that pre-wires the Mastermind planner/executor workflow — the main agent acts as a CTO/planner, spawns a task-executor subagent for implementation, then reviews. Use when bootstrapping a project that should use delegation specs in `.mastermind/tasks/` from day one.
metadata:
  version: 0.9.0
  authors:
    - mastermind
  tags:
    - claude-md
    - workflow
    - delegation
    - audit
    - critic
    - context
    - canons
    - incident-response
    - release
---

<!--
This template wires up the Mastermind workflow:
  - the main agent uses `mastermind-task-planning` (planner / CTO mode)
  - it spawns the `mastermind-task-executor` subagent to implement specs
  - planner reviews the executor's report

Prerequisites — install these into the target project (or globally) before using this CLAUDE.md:
  - skill:    skills/workflow/mastermind-task-planning/
  - skill:    skills/workflow/mastermind-task-executor/
  - subagent: agents/subagents/mastermind-task-executor.md
  - subagent: agents/subagents/mastermind-researcher.md
  - subagent: agents/subagents/mastermind-critic.md
  - subagent: agents/subagents/mastermind-auditor.md
  - subagent: agents/subagents/mastermind-release.md  (on-demand — invoked only when user asks to ship)
  - template: agents/claude-md/mastermind-context.md  (copy to project root as CONTEXT.md)
  - MCP server: mcp/servers/mmcg/  (truth layer — run `mmcg index .` then `mmcg watch` to keep current)
  - skill (optional):    skills/prompt-engineering/mastermind-prompt-refiner/
  - subagent (optional): agents/subagents/mastermind-prompt-refiner.md

Copy from the next comment marker down into your project's CLAUDE.md.
Fill in <PLACEHOLDERS>. Delete sections that don't apply.
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME>

<One paragraph: what this project is, what it's not.>

## Quick orientation

- **Language/framework:** <e.g., TypeScript + Bun + React, or Python + FastAPI>
- **Entry point:** `<e.g., src/index.ts>`
- **Where the interesting code lives:** `<e.g., src/features/>`
- **Institutional memory:** see [`CONTEXT.md`](CONTEXT.md) — decision log, known gotchas, glossary, don't-touch list. **Read this before designing anything non-trivial.**

## Commands

```bash
# Run locally
<RUN_COMMAND>

# Tests
<TEST_COMMAND>

# Typecheck / lint
<TYPECHECK_COMMAND>
<LINT_COMMAND>
```

---

## Workflow — Mastermind delegation

This project uses the **Mastermind workflow**: planning is separated from execution. The main agent plans, a subagent executes, the main agent reviews.

### Roles

| Role | Who | Skill | Model tier |
|---|---|---|---|
| **Refiner** (optional) | `mastermind-prompt-refiner` subagent | `mastermind-prompt-refiner` | sonnet |
| **Planner / CTO** | Main agent (this conversation) | `mastermind-task-planning` | opus |
| **Researcher** (on-demand) | `mastermind-researcher` subagent | (built into subagent) | haiku |
| **Critic** (design-time) | `mastermind-critic` subagent | (built into subagent) | opus |
| **Executor** | `mastermind-task-executor` subagent | `mastermind-task-executor` | sonnet |
| **Auditor** (post-flight) | `mastermind-auditor` subagent | (built into subagent) | opus |
| **Reviewer** (semantic) | Planner, after auditor finishes | (same as planner) | opus |
| **Release** (on-demand) | `mastermind-release` subagent | (built into subagent) | sonnet |

### Two independent Opus reviewers — different temporal phases

Both critic and auditor are independent Opus subagents with no prior conversation context. They serve different phases:

| | Critic | Auditor |
|---|---|---|
| **When** | During brainstorming — BEFORE spec is drafted | After execution — AFTER executor returns |
| **What** | Challenges proposed design | Verifies executed work against the spec contract |
| **Output** | 7-dimension verdict table (Correctness / Performance / Observability / Non-breaking / YAGNI / AI slop / Test+doc) + aggregate (ship/caveats/revise/rethink) | Audit verdict (held/drift/broken) + discrepancy list |
| **Stops the flow if** | Verdict is `rethink` — planner must redesign | Verdict is anything other than `held` — planner must address |

The planner is invested in their own design (critic catches that) and their own spec (auditor catches that). Two separate gates, two separate sources of bias to interrupt.

### Model tiering — why it matters

The roles intentionally span all three model tiers:

- **Opus** for decisions and tradeoffs (planner, reviewer) — the work where reasoning quality dominates cost
- **Sonnet** for implementation (executor, refiner) — solid execution at moderate cost
- **Haiku** for bulk fact-gathering (researcher) — fast and cheap for "find X", "list Y", "count Z"

If you find the planner running greps and reading dozens of files itself, you're burning Opus tokens on Haiku-grade work — spawn the researcher instead.

### Truth layer — mmcg

Underneath every role sits the `mmcg` codegraph MCP — Python, TypeScript, JavaScript, and Rust code indexed into a SQLite graph with 12 structural query tools. Every role queries mmcg for code-structural questions:

| Role | Uses mmcg for |
|---|---|
| Researcher | First-line lookups: `mmcg_search`, `mmcg_callers`, `mmcg_impact`, `mmcg_files`. Grep/Read are fallback for literal text. |
| Planner | Verifying symbols exist (`mmcg_search`), assessing blast radius (`mmcg_impact`), pre-flight spec validation. |
| Critic | Sanity-checking the design's claims against real code — does the function being proposed actually exist? Is the assumed call hierarchy real? |
| Executor | Pre-edit `mmcg_callers` to verify blast radius matches the spec's scope before each change. |
| Auditor | Post-execution `mmcg_callers` to detect silently-broken consumers; cross-reference reported changes against actual impact. |
| Reviewer | Semantic review on top of auditor's verdict — does NOT re-run mechanical checks. |

**The discipline:** never assume code structure from memory. If you're about to name a function, file, or callsite in a spec or in a fix — confirm it via mmcg first. mmcg is faster than the conversation and never lies.

If `mmcg_status` returns nothing, the index isn't ready — run `mmcg index .` in the project root, then `mmcg watch` for incremental updates.

### Flow

1. **User describes a problem** — "I want feature X".
2. **(Optional) Refine the input.** If the user's request is rough, vague, or bundles multiple intents, spawn the `mastermind-prompt-refiner` subagent. It returns a tight refined prompt or 1-3 clarifying questions. Use the refined version as the planner's input.
3. **Planner brainstorms with user** — clarifies scope, surfaces tradeoffs, picks an approach. **Spawn the `mastermind-researcher` subagent** as needed for facts (callsites, signatures, doc excerpts). Researcher returns structured facts (mmcg-first); planner makes decisions.
4. **Design-time challenge (MANDATORY for sensitive areas).** Before drafting the spec, planner spawns the `mastermind-critic` subagent with a focused brief (problem + design + ≥ 2 alternatives + constraints + **mmcg snapshot**). Critic returns a **7-dimension verdict table** (Correctness, Performance, Observability, Non-breaking, YAGNI, AI slop, Test/doc coverage) + aggregate verdict.
   - **Mandatory** for: auth/authz, billing, schema migrations, public API contracts, anything with rollback complexity
   - **Considered** for: multi-file changes, designs with multiple plausible approaches
   - **Skipped** for: one-line fixes, docs, throwaway exploration
   - Verdict `rethink` (Correctness fails or 2+ dimensions fail) → return to brainstorming. Verdict `revise` (one fail) → fix the failing dimension and re-spawn critic. Verdicts `ship it` (all 7 pass) / `ship with caveats` (some concerns) → proceed to spec; bake every `concern` and `fail`-fix into spec Rules / Do-NOT items. Paste the 7-row dimension table into the spec's Notes.
5. **Planner drafts the spec** in `.mastermind/tasks/XXX-feature-name.md` from `spec-template.md` (next sequential number). Mandatory sections (enforced by the auditor post-flight):
   - **Alternatives Considered** — ≥ 2 entries for non-trivial work
   - **Pre-edit symbol snapshot** — for every function/method the spec edits, planner auto-fills `mmcg_callers` count + `mmcg_search` signature; auditor uses to detect silent breakage. Delete section if no code symbols touched.
   - **Tests Plan** — explicit list of tests to add per phase
   - **Documentation Plan** — checkboxes for API docs / README / CHANGELOG / CONTEXT.md / docs/
   - **Observability Plan** — what's logged/metric'd/probed (or "n/a — no production runtime")
   - **Performance Considerations** — frequency / complexity / risks (or "n/a — not hot path")
   - Caveats from the critic become Rules or Do-NOT entries.
6. **Pre-flight validation (MANDATORY).** Planner runs through the spec self-check before showing to the user:
   - Every `**File:**` exists in the working tree
   - Every named symbol verified via `mmcg_search`
   - Every `FIND:` block matches current file contents (whitespace-sensitive)
   - `mmcg_impact` on each symbol-to-be-changed agrees with the spec's stated scope
   - `VERIFY:` commands look executable for this project
   - **All canon-mandated sections are non-empty:** Alternatives Considered (≥ 2 OR "trivial change" justification), Tests Plan, Documentation Plan, Observability Plan, Performance Considerations
   - **Failure → revise the spec, do not show.** Pre-flight is cheap; failed executor runs are expensive.
7. **User approves the validated spec.**
8. **Planner spawns the `mastermind-task-executor` subagent**, passing the task path.
9. **Executor implements** phase-by-phase. Before each function edit it runs `mmcg_callers` and bails if blast radius exceeds the spec's scope. Runs VERIFY commands after each step. Returns a report.
10. **Post-flight audit — mechanical (MANDATORY).** Planner spawns the `mastermind-auditor` subagent, passing the spec path + the executor's report. The auditor independently verifies:
    - Claimed "Files modified" vs `git diff --name-only` — no false claims, no scope creep
    - Each `[x] Phase N` vs visible CHANGE TO content in the diff
    - Cheap `VERIFY:` commands re-run (typecheck/lint/fmt-check)
    - `mmcg_callers` consistency for each changed symbol
    - "What I did NOT do" — flags any critical-deferred items
    - **Spec canon-sections actually addressed**: every test in Tests Plan grep'd in diff, every doc in Documentation Plan touched in diff, every observability hook from Observability Plan present in code
    - **Pre-edit snapshot drift** (when section present): re-run `mmcg_callers` + `mmcg_search` on each snapshot entry; report gained/lost callers + signature changes. Deltas don't auto-fail but MUST appear in verdict.
    - Returns verdict: `contract held` / `partial drift` / `contract broken`
    - **Anything other than `contract held` → planner must address before user is told "done"**
11. **Post-flight review — semantic (MANDATORY).** Planner reads the auditor's verdict and adds the semantic layer:
    - Was the approach right in retrospect?
    - Are deferred items consistent with quality bar?
    - Any discoveries that should become **[CONTEXT.md](CONTEXT.md) updates** (see table below) or follow-up specs?
12. **Update CONTEXT.md (when applicable).** If anything from this task is worth preserving across sessions, append to the right section of `CONTEXT.md`:
    | Discovery type | CONTEXT.md section |
    |---|---|
    | Non-trivial design decision the critic agreed with | Decision log |
    | Workflow surprised by something — "almost broke X" | Known gotchas |
    | New term that took explaining during brainstorming | Domain glossary |
    | New external dependency added | External dependencies |
    | Code area found to have hidden constraints | Don't-touch list |
13. **Planner reports to user** with the auditor's verdict table + semantic notes inline + a line on whether CONTEXT.md was updated. End-of-report line: "If you want this committed / PR'd, say the word — I'll spawn `mastermind-release`."
14. **(On-demand) Release packaging.** Only if the user explicitly asks to ship (triggers: "ship it", "commit", "PR", "merge", "отправляй", "коммить", "мерж"). Planner spawns the `mastermind-release` subagent.
    - **Preconditions** (planner verifies before spawning): auditor verdict = `contract held`; `git status` non-empty; `git diff --name-only` matches spec scope (modulo formatter / lockfile noise).
    - **Subagent returns** a draft commit message + draft PR description + explicit stage list + execution checklist. It does **not** run any git/gh write commands.
    - **Planner shows the draft to the user.** User approves, edits, or rejects.
    - **Planner executes the approved commands** (under user supervision): `git add <files>`, `git commit`, `git push`, `gh pr create`. Never `git add -A`; never `--force`; never `--no-verify`; never `--amend` on a published commit.
    - If the user asks to skip the subagent and commit directly (one-line fix, trivial change), planner does so without spawning — but still mirrors recent commit style.

### When to spawn the researcher

Spawn it when:
- Planner needs facts about the codebase to decide (callsites, signatures, file counts, configs)
- Planner needs to read documentation or specs that the user pointed at
- The lookup would otherwise spend Opus tokens on Haiku-grade work
- The planner is about to make assumptions about what's in the codebase — verify first

Do NOT spawn it when:
- The question is a design decision ("should we do X or Y?") — that's planner's job, not researcher's
- The information is in the conversation already
- It's one quick file read the planner can do inline

### When to use the refiner

Use it when:
- The user dropped a rough idea, not a structured request
- You're about to spend planning effort and want to confirm the brief first
- The same prompt would be passed downstream multiple times (worth tightening once)

Skip it when:
- The user's request is already tight (clear verb, deliverable, constraints)
- The work is exploratory and tightening the brief prematurely would constrain ideation

### When to spawn the release subagent

Spawn it when:
- User has explicitly asked to commit / PR / ship (triggers above)
- The work being shipped was specified, executed, and audited (`contract held`)
- There are non-trivial changes to package — multiple files, or a single file with caveats / observability / docs to surface in the PR body

Skip it when:
- One-line fix where you can mirror recent commit style inline
- Hot-fix during an active incident — the incident workflow has its own urgency
- The user is making a non-mastermind commit (e.g. README typo)

The subagent is **read-only**. It produces a draft; the planner runs `git add` / `git commit` / `git push` / `gh pr create` only after the user signs off on the draft. This separation is intentional — the subagent has no permission to publish.

### Rules

- **Planner never implements.** If the planner is editing code directly, the workflow has broken.
- **Executor never deviates.** If the spec is wrong, executor stops and reports — does not improvise.
- **Every spec is a contract.** `FIND:` blocks must match exactly; `VERIFY:` commands must pass.
- **One spec per delegation.** Don't bundle unrelated changes into a single task file.
- **Release is opt-in and read-only-to-draft.** Planner never auto-commits / auto-PRs. Release subagent never runs git/gh writes — it drafts; planner executes after user approves.
- **Destructive git ops require explicit user intent in the current turn.** No `--force`, no `--no-verify`, no `git reset --hard`, no `--amend` of a published commit, no branch deletion without the user saying so.

### Spec naming

`.mastermind/tasks/XXX-kebab-case-name.md` where `XXX` is the next sequential number (`001`, `002`, …).

### When to use this workflow vs. just doing it

Use the workflow for:
- Multi-file changes
- Changes that touch sensitive areas (auth, billing, migrations, public APIs)
- Anything where the cost of getting it wrong > the cost of writing a spec

Skip the workflow (just do it) for:
- One-line fixes
- Pure documentation edits
- Throwaway exploration / spikes

If unsure, ask the user: "spec or direct?"

### Parallel workflow — incident response

This planning workflow is for **building things**. For **production breakage**, there's a separate parallel workflow: `mastermind-incident-response`.

Triggers: user says "incident", "outage", "rollback", "что-то сломалось в проде", or pastes paging alerts / error logs.

Different priorities:
- Time > completeness — stop the bleed first, understand later
- Rollback > hot-fix
- Blameless analysis (systems, not people)
- Output: blameless postmortem + action items that become regular planning specs

The two workflows connect via action items: postmortems generate `.mastermind/tasks/<NNN>-*.md` specs that flow through this planning workflow normally. Lessons feed into CONTEXT.md (Known gotchas / Don't-touch list / Decision log).

If the incident traces to a workflow gate that should have caught it (critic missed an Observability concern, spec template lacked a section, auditor passed when it shouldn't have), the postmortem proposes workflow improvements as their own specs.

---

## Conventions

- <Convention 1>
- <Convention 2>

## Common pitfalls

- <Pitfall 1>
- <Pitfall 2>

## When in doubt

- <Where to look — docs path, architecture overview>

<!-- ─── COPY TO HERE ─── -->
