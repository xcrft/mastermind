# Triage checklist — first 5 minutes

Reference for the [`mastermind-incident-response`](../SKILL.md) skill, Phase 1. Run through these questions / commands in parallel with talking to the user. Goal: enough information to choose a mitigation in Phase 2.

---

## What to ask the user (priority order)

1. **Symptom — what's observable?**
   - "What error are you / users seeing?"
   - Paste actual error message, not a paraphrase
   - Single sentence, ideally
   - Example: `❌ "the app is slow"` → `✓ "/api/messages returning 500 with 'connection refused' since 14:32"`

2. **Scope**
   - "How many users / what % of traffic / which surfaces?"
   - Distinguish: 1 user vs 1 region vs everyone
   - If unclear → ask: do we know yet?

3. **Severity classification** (pick one; if unclear, default to one level higher):
   - **sev0** — total outage / safety / data loss; immediate
   - **sev1** — major surface broken; act within minutes
   - **sev2** — partial degradation; act within hours
   - **sev3** — cosmetic / single user; act within days
   - Severity drives whether to ask "should we page?" before doing anything else

4. **Timeline — when did this start?**
   - "What's the first timestamp you have?"
   - Convert to UTC immediately, write it down
   - This is what you'll correlate against deploys / git log

5. **What's been tried?**
   - Avoid duplicate effort
   - If user already rolled back / restarted / flipped a flag — note it
   - **Critical:** if a previous action made things WORSE, the next action shouldn't be the same kind

---

## What to gather in parallel (mmcg / git / files)

Run these via `mastermind-researcher` subagent or directly — should take < 1 minute:

```bash
# What committed recently (might have shipped the issue)
git log --since='2 hours ago' --oneline

# What's deployed (if there's a deploy marker file or git tag)
git tag --sort=-creatordate | head -5
git log -10 --oneline

# What specs were finished recently
ls -lt .mastermind/tasks/ | head -10

# Are there in-progress specs that might be related?
git status -s

# Is the index reachable (am I working from stale info?)
mmcg_status

# Quick scan for the symptom — has this been seen before?
grep -i "<error string>" CONTEXT.md 2>/dev/null
```

---

## Severity-driven branching

After triage, the severity determines what comes next:

| Severity | What you do next |
|---|---|
| **sev0** | Ask user: "should we page on-call before continuing?" Then go to Phase 2 with the most conservative mitigation. |
| **sev1** | Go directly to Phase 2. Watch the clock — if no improvement in 10 min, escalate. |
| **sev2** | Phase 2 with more deliberation — rollback is still preferred if available. |
| **sev3** | Skip Phase 2's "stop bleeding" urgency. Go to Phase 3 investigation. The postmortem (Phase 4) may even be lightweight (a CONTEXT.md gotcha entry, not a full doc). |

---

## What to write down (timeline starts now)

For the postmortem later, you'll need a timeline. Start it during triage:

```
14:32 UTC — first failure observed (per <source>)
14:36 UTC — user reported in #ops
14:38 UTC — incident response engaged
14:39 UTC — triage complete; sev1 declared; investigating recent deploys
...
```

Even rough timestamps are useful. The postmortem can refine them from logs later, but having any timeline beats reconstructing from memory.

---

## Anti-patterns during triage

- **Don't speculate about root cause yet.** Triage answers WHAT and WHEN, not WHY. WHY comes in Phase 3.
- **Don't write a fix yet.** Even if you're sure you know what's wrong, Phase 2 prefers rollback over hot patches.
- **Don't change scope unilaterally.** "While I'm here let me also fix X" is exactly the wrong instinct under time pressure.
- **Don't blame the deploy author** — the system allowed the bad deploy; that's the systemic issue, not the human.

---

## When to call this phase done

You're done with triage when you can answer all five:
1. ✓ What's the symptom? (concrete)
2. ✓ Who/what is affected? (scope)
3. ✓ Severity?
4. ✓ When did it start?
5. ✓ What's been tried?

…and you have either:
- A candidate mitigation in mind, OR
- An explicit "I don't know yet, going to Phase 3 first" decision

Move to Phase 2.
