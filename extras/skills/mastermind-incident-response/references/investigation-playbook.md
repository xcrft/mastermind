# Investigation playbook — find root cause via mmcg + git + .mastermind/tasks/

Reference for the [`mastermind-incident-response`](../SKILL.md) skill, Phase 3. After symptoms have stopped, find what actually broke.

The patterns below are concrete recipes. Use them when the corresponding question comes up — don't run all of them speculatively (wastes context).

---

## Question 1 — "What changed recently?"

Most production incidents trace to a recent change. Start here.

```bash
# What was committed in the window when the incident started?
git log --since='2 hours ago' --until='now' --oneline

# What was committed in the file/dir we suspect?
git log -20 --oneline -- <suspected/path/>

# What did the most recent commits actually change?
git log -5 -p --stat -- <suspected/path/>
```

Then for any candidate commit:
```bash
git show <commit-sha> --stat       # what files
git show <commit-sha>              # full diff
```

**Heuristic for ranking suspect commits:**
- Most recent first
- Bigger diffs first (more surface to have bugs)
- Commits to "interesting" paths (hot paths, recently-incident-prone dirs)
- Commits that touch the symptom's component (grep error message in commit diff)

---

## Question 2 — "What's the blast radius of the change?"

If you have a suspect commit, what does it touch that could explain the symptom?

```bash
# What symbols changed in the suspect commit?
git show <commit-sha> --name-only

# For each changed function/method, what calls it?
mmcg_callers <symbol> --language <lang>

# Transitive — what else depends on it?
mmcg_impact <symbol> --depth 3 --language <lang>
```

If the blast radius doesn't include the symptom's component → the suspect commit probably isn't the cause. Move to the next candidate.

If the blast radius DOES include the symptom's component → strong candidate. Read the code change.

---

## Question 3 — "Were the relevant specs supposed to catch this?"

Spec for any recent change should have had Tests Plan + Observability Plan + Performance Considerations sections (per spec template).

```bash
# Find the spec for this work (look in .mastermind/tasks/ for matching folder or timestamp)
ls -lt .mastermind/tasks/                                  # task folders, newest first
ls -lt .mastermind/tasks/*/spec.md 2>/dev/null             # direct list of spec files by mtime

# Read its Tests Plan
grep -A 20 "Tests Plan" .mastermind/tasks/<NNN>-<name>/spec.md

# Read its Observability Plan
grep -A 10 "Observability Plan" .mastermind/tasks/<NNN>-<name>/spec.md

# Read its Performance Considerations
grep -A 10 "Performance Considerations" .mastermind/tasks/<NNN>-<name>/spec.md
```

Then ask:

- **Did the Tests Plan cover this failure mode?** If no, that's a gap in spec quality. Action item: "harden Tests Plan for similar work."
- **Did Observability fire for this failure?** If no, was it instrumented? If yes, did it page? Detection time is its own failure to root-cause.
- **Did Performance Considerations anticipate this load?** If the issue is scale-related, the spec should have predicted it.

These answers feed the postmortem's "Why detection took N minutes" and "Workflow improvements" sections.

---

## Question 4 — "Has this happened before?"

```bash
# Check known gotchas
grep -B 2 -A 4 -i "<symptom keywords>" CONTEXT.md

# Check don't-touch list
grep -B 2 -A 4 "<file or path>" CONTEXT.md

# Check past postmortems (if you keep them)
ls postmortems/ 2>/dev/null || ls docs/postmortems/ 2>/dev/null
grep -ri "<symptom>" postmortems/ 2>/dev/null
```

If yes → this is a **recurrence**. That's a MUCH bigger finding than a first-time incident:
- The previous fix didn't stick
- The prevention didn't transfer
- The CONTEXT.md entry was either missing or ignored

A recurrence postmortem should focus heavily on Phase 5 (feed forward) and propose a STRUCTURAL fix, not just a code fix.

---

## Question 5 — "What's the failure mode?"

Classify the failure into a category. This guides both immediate response and what kind of structural improvement to propose:

| Category | Examples | Typical fix |
|---|---|---|
| **Code bug** | null-pointer, off-by-one, wrong condition | Code change + test |
| **Configuration bug** | wrong env var, bad timeout, missing flag | Config change + config validation in CI |
| **Schema / migration** | column missing, type mismatch, FK violation | Migration + pre-flight schema check |
| **Capacity / scale** | OOM, connection pool exhausted, CPU pegged | Scaling + capacity model in spec template |
| **External dependency** | upstream API down, vendor SLA breach | Degraded-mode fallback + dep health probe |
| **Race / concurrency** | lost write, deadlock, double-spend | Concurrency model documented + tests |
| **Data quality** | bad input from upstream, encoding issue | Validation at boundary + schema for inputs |
| **Process** | bad deploy, wrong branch, missed code review | Pipeline / process improvement |

The category determines whether the postmortem proposes a CODE fix, a PROCESS fix, or a SYSTEM fix (architectural).

---

## Question 6 — "What's the smallest reproducer?"

Before declaring root cause found, get a reproducer:

- **Unit test** — fastest; if you can write one that fails, you understand the bug
- **Integration test** — if behavior depends on multiple components
- **Manual reproduction** — if neither possible, document the exact steps

A bug without a reproducer is a bug not understood. Don't ship the fix until you can reproduce.

The reproducer becomes Test 1 in the fix spec's Tests Plan.

---

## Five-whys discipline

For the postmortem's "What went wrong" section, apply five-whys:

1. **Why** did the symptom happen? → because <proximate cause>
2. **Why** did that happen? → because <one level deeper>
3. **Why** did THAT happen? → because <one more>
4. **Why**?
5. **Why**?

Stop when:
- The answer is a system property (e.g., "the deploy pipeline is asynchronous, so we ran without rollback safety")
- The answer would require changing fundamental architecture (escalate, don't propose in postmortem)
- You've gone 5 levels — at some point further whys are speculation

Document the chain in the postmortem. The deepest "why" you can credibly answer is the **systemic** root cause; that's what the action items should address.

---

## When to stop investigating

You have enough when:

1. ✓ You can name the proximate cause (the code/config/data thing that broke)
2. ✓ You can name a systemic cause (the why that goes deeper than "engineer X did Y")
3. ✓ You have a reproducer (or explicit decision that one is not feasible)
4. ✓ You know what would have prevented this (a test? a check? a config? a process?)

Then go to Phase 4 — write the postmortem.

If after 1-2 hours you can't answer #1-2 → escalate or accept "we couldn't determine root cause" honestly in the postmortem. **Don't fabricate a cause to look complete.** Unknown root causes are themselves a finding.
