<!--
  Mastermind blameless postmortem template.

  HOW TO USE
  - Copy this file to postmortems/<YYYY-MM-DD>-<short-name>.md (or docs/postmortems/, whichever convention your repo uses)
  - Fill in every <placeholder> with concrete content
  - Delete sections that genuinely don't apply (e.g., if a sev3 had no user impact)
  - Keep the file short — a postmortem nobody reads is worse than no postmortem
  - Action items get linked back here from the .mastermind/tasks/ specs they spawn

  BLAMELESS PRINCIPLE
  Write about systems, not people. If a person made a judgment call, frame it as:
  "given the information available, the action was reasonable; the lesson is that
  information X needs to be more accessible / surfaced earlier."

  Anti-pattern: "Engineer X deployed without testing."
  Better: "The deploy pipeline allowed merging with failing tests because the
  test job was marked non-blocking three weeks ago."

  TONE
  - Past tense (this happened, this was tried)
  - Concrete (timestamps, error messages, file paths)
  - Honest about unknowns ("we don't yet know why X" is better than fabricating)
-->

# Postmortem: <short title>

## Summary

**Date:** <YYYY-MM-DD>
**Severity:** <sev0 | sev1 | sev2 | sev3>
**Duration:** <N minutes from start to mitigation, M minutes to full resolution>
**Impact:** <one sentence — what users / systems were affected, magnitude>

<One to three sentences: what happened, what was the user impact, what was the resolution. Anyone reading should know what this is about in 30 seconds.>

---

## Timeline (UTC)

| Time | Event |
|---|---|
| <HH:MM> | First failure observed (per <source — log, monitor, user report>) |
| <HH:MM> | Detection (paged / reported in #ops / noticed by …) |
| <HH:MM> | Incident response engaged |
| <HH:MM> | Triage complete; severity declared as <sev>; <initial hypothesis> |
| <HH:MM> | Mitigation: <what was done> |
| <HH:MM> | Symptoms stopped |
| <HH:MM> | Root cause identified |
| <HH:MM> | Full resolution (fix deployed / patch applied / dependency restored) |
| <HH:MM> | Postmortem started |

---

## What happened

<2-4 paragraphs of narrative. Lead with the proximate cause. Then walk through how the symptom manifested, what was tried, what worked, what didn't.>

<If multiple things went wrong in sequence (cascading failure), name each component and how they interacted.>

---

## Root cause analysis

### Proximate cause
<The specific code change / config / dependency that triggered the symptom. Cite file:line, commit SHA, or external system.>

### Systemic causes (five-whys chain)
1. **Why** did the symptom happen? <because…>
2. **Why** did that happen? <because…>
3. **Why** did that happen? <because…>
4. **Why** did that happen? <because…>
5. **Why** did that happen? <because…>

The deepest "why" we can credibly answer is the **systemic root cause** — that's what the action items should address.

### Failure category

<Pick one from the investigation-playbook.md table: code bug / configuration bug / schema / capacity / external dependency / race or concurrency / data quality / process.>

---

## Detection

**Why did detection take <N> minutes?**
<Why didn't this fire earlier? Was there a monitor for this failure mode? Did the monitor fire but not page? Did it page someone who couldn't act?>

**What detection improvements would have caught this <K> minutes sooner?**
<Specific: "a P99 latency alert on /api/messages would have fired at 14:33 instead of 14:38 when users reported.">

---

## Mitigation

**Why did mitigation take <M> minutes?**
<Was the rollback path clear? Was someone with deploy access available? Was the on-call runbook accurate?>

**What mitigation improvements would have stopped the bleed sooner?**
<Specific: "a feature flag for the new code path would have let us disable in seconds instead of waiting for a rollback deploy.">

---

## What went well

<Yes, name what worked. Reinforces good patterns and supports psychological safety. Be specific.>

- <Thing 1 — e.g., "The Datadog dashboard for /api/messages clearly showed the regression once someone looked at it">
- <Thing 2 — e.g., "On-call rotation was clear; <person> was immediately available">
- <Thing 3>

---

## What didn't go well

<The hard part — be honest, but blameless. Focus on systems, gaps, processes.>

- <Thing 1 — e.g., "The spec for this change didn't include an Observability Plan, so the new code path had no metrics">
- <Thing 2 — e.g., "The deploy pipeline reported success even though the smoke test failed">
- <Thing 3>

---

## Where the mastermind workflow gates failed (if applicable)

*Only relevant if this incident came from a change shipped through the mastermind workflow. If it came from a hot-fix, manual ops, or pre-mastermind code, skip this section.*

- **Critic dimension(s) that should have caught this:** <e.g., "dimension #3 Observability — the design didn't include any metric / log on the failure path, and the critic missed flagging it">
- **Spec template section(s) that were empty or weak:** <e.g., "Observability Plan was 'n/a — no production runtime' but this code DOES run in production">
- **Auditor checks that passed when they shouldn't have:** <e.g., "auditor verified tests ran, but spec didn't include a load test, so capacity issue wasn't tested for">

→ Each of these maps to a workflow-improvement action item (see Action Items below).

---

## Action items

Each action item gets owned, dated, and either (a) becomes a `.mastermind/tasks/` spec or (b) becomes a CONTEXT.md update.

| # | Action | Type | Owner | Due | Spec / CONTEXT entry |
|---|---|---|---|---|---|
| 1 | <Specific change — code, config, process, doc> | <code-fix \| context-md \| workflow-improvement \| process> | <person> | <YYYY-MM-DD> | <`.mastermind/tasks/NNN-…md` or "CONTEXT.md → Known gotchas">|
| 2 | <…> | <…> | <…> | <…> | <…> |

**Avoid action items like:**
- ❌ "Be more careful when deploying" — not actionable
- ❌ "Add monitoring" — too vague
- ❌ "Train the team on X" — training without process change rarely sticks

**Prefer action items like:**
- ✓ "Add P99 latency alert on /api/messages at 200ms threshold via Datadog monitor — `.mastermind/tasks/NNN-add-messages-latency-alert.md`"
- ✓ "Add 'capacity test' as mandatory line in Performance Considerations section of spec-template — `.mastermind/tasks/NNN-spec-template-capacity.md`"
- ✓ "Add CONTEXT.md known-gotcha: 'Redis cluster mode silently drops MULTI on key migrations during rebalance'"

---

## Feed forward to CONTEXT.md

The following entries get appended to project `CONTEXT.md`:

### Known gotchas (append)
- **<one-line summary of the failure pattern>** — <bite scenario>. See `postmortems/<this file>`.

### Don't-touch list (if applicable, append)
- **`<path or symbol>`** — <constraint that emerged from this incident>

### Decision log (if architecture changed, append)
- **<YYYY-MM-DD> — <decision name>** — <one-sentence decision, why, alternatives rejected, source: postmortems/<this file>>

---

## Unknowns

*If root cause is partially or fully unknown, name what's unknown. This is honest — fabricating a cause is worse than admitting uncertainty.*

- <Unknown 1 — e.g., "We don't yet know why the Redis client closed the connection at 14:32 specifically; logs are too sparse to tell">
- <What would we need to know it: a debug log, a packet capture, a repro environment>

---

## Sign-off

- **Author:** <name>
- **Reviewers:** <names who read this before publishing>
- **Distribution:** <team / org / wider>
