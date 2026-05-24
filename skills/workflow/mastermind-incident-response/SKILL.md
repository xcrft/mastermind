---
name: mastermind-incident-response
description: Parallel workflow for production incidents — triage, stop the bleeding, investigate root cause via mmcg + git + .mastermind/tasks/ history, write a blameless postmortem, feed lessons back into CONTEXT.md and (if applicable) into the main workflow's spec template or critic dimensions. Use when the user says "incident", "outage", "rollback", "что-то сломалось в проде", or pastes paging alerts / error logs.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - incident-response
    - postmortem
    - operations
  model: opus
---

# Mastermind — Incident Response

A **parallel workflow** for handling production breakage. Different from the main 13-step planning workflow ([`mastermind-task-planning`](../mastermind-task-planning/SKILL.md)) which builds new things — this one **stops bleeding**, finds root cause, and turns lessons into systemic improvements.

## When to Activate

User says or pastes:
- "incident", "outage", "production is down", "что-то сломалось в проде", "rollback"
- Paging alerts (Datadog, PagerDuty, Sentry)
- Error logs with stack traces
- "users are reporting…"
- "deploy broke something"

## What this is NOT

- **Not** the bug-triage flow for development-time bugs — those go through the regular planning workflow
- **Not** a feature-request channel
- **Not** a debugging session for the user's local environment
- **Not** a substitute for paging an actual on-call engineer for sev0/sev1 incidents — the workflow assists, doesn't replace the human

## Different Priorities Than Planning

| Planning workflow | Incident response |
|---|---|
| Optimize for quality | Optimize for **time** |
| 7-dim critic before doing anything | Bias toward **rollback first**, understand later |
| Mandatory specs, alternatives, tests | Hot-fix is OK if rollback impossible |
| "Did we design this right?" | "What's the fastest way to stop the bleeding?" |
| Blameless review post-fact | Blameless reasoning **during** |

You are in **operations mode**. Speed of bleed-stop > completeness of fix > root-cause depth > paperwork. Reverse that order during postmortem.

## Phases

### Phase 1 — Triage (target: first 5 minutes)

Ask the user (or extract from pasted alert):

1. **Symptom** — what users / monitoring see (one sentence, observable)
2. **Scope** — how many users / how much traffic / which surfaces
3. **Severity** — pick a number:
   - **sev0** — total outage, paging fire
   - **sev1** — major degradation, immediate action needed
   - **sev2** — partial degradation, action within hours
   - **sev3** — minor / cosmetic, action within days
4. **Timeline** — when did this start? (correlate with deploys / changes)
5. **What's been tried already**

While asking, parallel-research with `mastermind-researcher` subagent:
```
git log --since='2 hours ago' --oneline    → what changed recently
git log -10 --oneline                       → most recent commits
ls -lt .mastermind/tasks/ | head -10                  → most recent specs
mmcg_status                                → index health
```

Use see [`references/triage-checklist.md`](references/triage-checklist.md) for the full first-response checklist.

### Phase 2 — Stop the bleeding (target: next 10 minutes after triage)

**Order of preference:**
1. **Rollback** to last known good — if you can identify it, do it
2. **Disable the feature** — if feature-flagged, flip the flag off
3. **Hot patch** — only if 1 and 2 not possible; this carries risk
4. **Escalate** — if stuck > 10 min on Phase 2, page additional help / wake on-call

For each option, write to user what you're about to propose. **Do not execute destructive ops** (`git push --force`, deploys) without explicit user confirmation per turn — they're operating the controls, you're advising.

Mitigation tactics by failure type:
- **Recent deploy broke things** → revert the deploy
- **Recent config change broke things** → revert config
- **Data corruption** → freeze writes, restore from backup, investigate cause separately
- **External dependency degraded** → enable degraded-mode fallback if present; otherwise wait + monitor
- **Resource exhaustion (memory, disk, connections)** → kill / restart / scale; investigate cause separately

### Phase 3 — Investigate (after symptoms stop)

With pressure off, find **root cause** — not just the symptom. Five-whys discipline.

**Investigation playbook** — see [`references/investigation-playbook.md`](references/investigation-playbook.md) for the full set of mmcg + git + log patterns. Quick summary:

- **What changed recently?** `git log --since='<time of incident start - 1h>' -- <suspected paths>`
- **What's the blast radius of the change?** `mmcg query impact <symbol> --depth 3`
- **Were the relevant specs in `.mastermind/tasks/` going to catch this?** Read their Tests Plan + Observability Plan sections
- **Did observability fire?** If yes, why didn't it page sooner? If no, why wasn't it instrumented?
- **Is this a recurrence?** Grep `CONTEXT.md` for the symptom — known gotcha?

If a fix is needed, **don't write it inline in this incident flow** — open a `.mastermind/tasks/<NNN>-<short-name>.md` spec via the main workflow. The fix goes through the normal critic/auditor gates. Incident response identifies the need; planner designs the response.

### Phase 4 — Postmortem (within 24h of resolution)

Use [`references/postmortem-template.md`](references/postmortem-template.md). Sections:

- **Summary** (1-2 sentences — what happened, impact, resolution)
- **Timeline** (UTC, minute-resolution where relevant)
- **What went wrong** (root cause, contributing factors)
- **What went well** (yes, name what worked — psychological safety + reinforces good patterns)
- **Why detection took N minutes** (separate from why-it-happened — detection is its own failure mode)
- **Why mitigation took N minutes** (rollback fast? unclear who could act? missing runbook?)
- **Action items** (specific, owned, dated — each becomes a `.mastermind/tasks/` spec or a CONTEXT.md update)

**Blameless framing** — write about systems, not people:
- ❌ "Engineer X deployed without testing"
- ✓ "The deploy pipeline allowed merging with failing tests because the test job was marked non-blocking three weeks ago"

If a person made a judgment call that turned out wrong, frame it as: "given the information available at the time, the action was reasonable; the lesson is that information X needs to be more accessible / surfaced earlier."

### Phase 5 — Feed forward

Two destinations:

**A. Project `CONTEXT.md`** (immediate):
- **Known gotchas** entry for the failure pattern — concrete + scenario + reference to postmortem path
- **Don't-touch list** entry if a code area has subtle constraints now known
- **Decision log** entry if the postmortem changed an architectural decision

**B. Action items as new `.mastermind/tasks/` specs** (within days):
- Each action item becomes a spec
- Specs go through normal workflow (planner → critic → executor → auditor)
- Link back to postmortem in spec's Notes section

**C. Workflow improvements** (if applicable):
- Did the spec for the offending change include an Observability Plan? If no, that's evidence the planner skill should make it more mandatory.
- Did the critic's 7 dimensions miss this category of issue? Propose an 8th dimension or sharpening an existing one.
- Did the auditor pass when it shouldn't have? Add a new check.

Workflow improvements go into the mastermind repo itself as a meta-improvement spec. **The workflow learns from its own failures.**

## Roles & subagents

Most of incident response is run by the planner (in this mode), with these spawns:

- **`mastermind-researcher`** — for git/mmcg fact-gathering during Phase 1 and Phase 3
- **`mastermind-critic`** — for the postmortem's "what went wrong" section if there's a design question (e.g., "was this design fundamentally flawed?"). Optional.
- **`mastermind-auditor`** — NOT used in incident response (it's a post-flight checker, doesn't apply here)
- **`mastermind-task-planning`** (in main mode) — for any follow-up specs that come out of the postmortem

## References

- [`references/triage-checklist.md`](references/triage-checklist.md) — first 5 minutes
- [`references/investigation-playbook.md`](references/investigation-playbook.md) — mmcg + git + .mastermind/tasks/ patterns for finding root cause
- [`references/postmortem-template.md`](references/postmortem-template.md) — blameless postmortem fill-in
