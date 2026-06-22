<!--
  Canonical Mastermind task spec template.

  HOW TO USE
  - The planner ([../SKILL.md](../SKILL.md)) creates the folder .mastermind/tasks/XXX-kebab-feature-name/
    and copies this whole file to .mastermind/tasks/XXX-kebab-feature-name/spec.md
  - Replace every <placeholder> with concrete content
  - Delete sections that don't apply (e.g., drop the Critic Verdict block if no critic was spawned)
  - Do NOT show this file to the executor — show the filled-in spec only

  WHAT THE EXECUTOR SEES
  Everything below this comment block. Keep the language imperative, the FIND blocks
  exact, the VERIFY commands runnable. Specs are contracts.

  YAML FRONTMATTER (RECOMMENDED, ADDITIVE)
  The block between the `---` fences below is the machine-readable contract that
  `mmcg verify-spec` and `mmcg audit-spec` use for high-precision gates:
    - `touches[].file` + `touches[].symbols` — scoped symbol search (no monorepo
      leaf-name collisions like the heuristic path has)
    - `expected_docs` — separate from code touches, audit flags missed doc updates
    - `verify[].cmd` — fed into the VERIFY PATH check + run-task verify gate
    - `breaking_changes.removed_symbols` — STRUCTURED ack list. Replaces the
      old lowercase-substring fallback that misread `Do not remove X` as an ack.
  When frontmatter is ABSENT, the gates fall back to heuristic extraction from
  the Markdown body — fine for trivial / docs-only specs, but the precision
  gain on real code changes is what makes the workflow trustworthy. Migrate.

  CANON COMPLIANCE
  The mandatory sections below (Alternatives Considered, Tests Plan, Documentation
  Plan, Observability Plan, Performance Considerations) exist to enforce engineering
  canons that the critic checks (see ../../../agents/subagents/mastermind-critic.md).
  Removing these sections defeats the canon — the auditor will fail post-flight if
  the spec claimed Tests Plan but no tests were actually added.
-->

---
# Machine-readable spec contract — consumed by `mmcg verify-spec` / `audit-spec` /
# `run-task`. All fields are optional; partial frontmatter is fine. Delete this
# block if you want the heuristic path only (advisory: precision drops).
id: "<NNN>"                         # spec number, string (YAML quirk: bare 042 → 34 octal)
title: <Feature Name>
risk: <low|medium|high>             # informational, surfaced in run-task risk report

touches:                            # files this spec authorizes the executor to modify
  - file: <src/area/file.ext>
    language: <python|typescript|rust|csharp|go|java|php|cpp|...>
    symbols:                        # mix of bare names + detailed objects allowed
      - <symbol_name>               # bare-name form
      - name: <other_symbol>
        signature: "<exact signature>"     # `mmcg_search <name>` to capture
        callers: <N>                       # `mmcg_callers <name>` count at snapshot time

verify:                             # PATH-checked at verify-spec; run by `run-task --exec`
  - <label>                         # informational only (e.g. "typecheck")
  - cmd: "<runnable command>"       # the actual command, e.g. `npm test -- billing`

expected_docs:                      # doc files the spec promises to update — separate
  - <README.md or path/to/doc.md>   # from code touches so audit can flag misses

breaking_changes:                   # ack list for intentional removals
  removed_symbols:
    - <symbol_name>                 # bare-name form OR
    - name: <other_symbol>          # detailed object with file/reason
      file: <path>
      reason: "<one-line explanation>"
---

# Task <NNN>: <Feature Name>

## LLM Agent Directives

You are <doing X> to achieve <Y, the goal in one sentence>.

**Goals:**
1. <Primary goal — what counts as done>
2. <Secondary goal — optional>

**Rules (global):**
- DO NOT add features beyond what this spec lists (YAGNI)
- DO NOT refactor unrelated code (KISS)
- DO NOT add code comments not already in the CHANGE TO blocks — keep only comments that explain a *why* the code can't; no restating-the-code, no edit markers (`// added`, `// changed`). See [[no-ai-slop-comments]]
- DO NOT introduce breaking changes to public APIs without explicit Non-breaking section saying so
- SCOPE each `VERIFY:` to the touched package/subtree — `tsc -p packages/x`, `npm test -- billing`, `pytest tests/test_foo.py` — never a whole-repo suite in a per-step VERIFY
- KEEP per-step `VERIFY:` cheap and localizing — it proves *this* edit landed; the full typecheck/test suite runs once at the phase boundary and in the final block, not per step
- `VERIFY:` commands MUST terminate — no `dev` / `start` / `watch` / `serve`; to check a running server, background it with a timeout and `curl` instead of blocking on it
- RUN `<project's typecheck command>` after each phase — must exit 0
- VERIFY no imports break (`mmcg_callers` count stays consistent on touched symbols)
- <Other project-specific globals>

**Critic findings baked into rules** *(if `mastermind-critic` was spawned — paste each `concern`/`fail` here as a hard rule; delete this block if no critic spawn):*
- <Caveat 1 from critic — concrete>
- <Caveat 2>

---

## Alternatives Considered *(MANDATORY for non-trivial work — at least 2 entries)*

The planner must enumerate ≥ 2 plausible approaches and explain why each was rejected. For trivial changes (one-line fix, doc edit, throwaway exploration), write "trivial change — single approach". The critic uses this section to avoid re-suggesting rejected options.

For non-trivial alternatives (multi-module, auth/billing/data-flow, API boundary, migration, anything touching ≥ 3 files), add a Mermaid codeflow diagram per alternative. Nodes must be real files, symbols, modules, or external boundaries — verified via `mmcg_search` or explicitly marked `[NEW]`. Keep each diagram ≤ 8 nodes. Generic boxes (`User → System → Database`) are AI slop and will be flagged by the critic.

### Alternative A — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`, `mmcg_callers <symbol>` → `<N> callers`
- **Tradeoff:** <concrete>
- **Rejected because:** <concrete reason tied to mmcg findings or project constraint>

### Alternative B — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`
- **Tradeoff:** <concrete>
- **Rejected because:** <reason>

### Picked approach — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** <mmcg evidence>
- **Chosen because:** <concrete reason>

---

## Decision Matrix *(required for standard/strict; skip for lite)*

Crystallizes the alternatives comparison into an objective grid. Fill one row per option in Alternatives Considered above.

| Option | Correctness | Complexity | Blast radius | Migration risk | Observability | Reversibility | Verdict |
|---|---|---|---|---|---|---|---|
| A — <name> | pass | low | low | low | good | easy | reject |
| B — <name> | concern | low | high | medium | weak | hard | reject |
| C — <name> | pass | medium | low | none | good | easy | **chosen** |

Column values: `pass / concern / fail` for Correctness; `low / medium / high` for complexity/blast/migration; `good / weak / none` for observability; `easy / medium / hard` for reversibility.

The `Verdict` row must be one of: `chosen`, `reject`, `candidate` (deferred alternative). Exactly one row gets `chosen`.

---

## Risk Register *(required for strict specs; skip for trivial/lite)*

Known risks of the chosen approach. Each risk must have mitigation assigned to a phase. If a risk has no mitigation, say so — don't omit it.

| Risk | Probability | Impact | Evidence | Mitigation | Owner phase |
|---|---|---|---|---|---|
| breaks existing callers | medium | high | `mmcg_callers X → 45` | preserve signature, add compat wrapper | Phase 1 |
| migration leaves stale data | low | high | schema diff shows nullable column | add backfill script, gate on count > 0 | Phase 2 |
| no prod observability | low | medium | assumption — new code path | add log line + metric in Phase 3 | Phase 3 |

A risk with `impact: high` and no mitigation = automatic critic `fail` on dimension #2 (Performance & scale) or #1 (Correctness).

---

## Pre-edit symbol snapshot *(filled by planner via mmcg — auditor uses to detect silent breakage)*

For each function / method this spec edits, planner records the current `mmcg_callers` count and signature so the auditor can compare post-execution. Delete this section if the spec doesn't touch any code symbols (pure doc / config change).

- `<symbol>` — <N> callers (via `mmcg_callers <symbol>`), signature `<sig>` (via `mmcg_search <symbol>`)
- `<another_symbol>` — <N> callers, signature `<sig>`

Auditor will re-run `mmcg_callers` / `mmcg_search` post-execution and surface any delta (gained / lost callers, signature change). A delta isn't automatically a fail, but it MUST be acknowledged in the verdict.

---

## Evidence Ledger *(required for strict specs; skip for trivial/lite)*

Every non-trivial claim in this spec must be backed by one of: mmcg evidence, file evidence, a runnable command, user-provided input, or an explicit assumption. If a claim has no backing, it's a guess — name it as an assumption so the critic and auditor can flag it.

| Claim | Evidence type | Evidence | Confidence |
|---|---|---|---|
| `<symbol>` has N callers | mmcg | `mmcg_callers <symbol> → N` | high |
| `<file>` contains `<pattern>` | file | `grep '<pattern>' <file>` | high |
| no prod runtime | assumption | internal build script only; confirmed with user | medium |
| `<claim>` | user-provided | user stated in session on <date> | medium |

Rules:
- No `"this should be safe"` without an evidence row
- No `"existing callers are fine"` without a `mmcg_callers` count
- Assumptions are allowed, but must be explicit — they become critic `concern` targets

---

## Phase 1: <First logical step — name it by outcome, not process>

### 1.1 <Specific action — one verb, one location>

**File:** `<src/path/to/file.ext>`

**Pre-edit check via mmcg** *(executor runs `mmcg_callers <symbol>` before editing):*
- Expected callers: ≤ <N> in scope (planner verified during pre-flight)
- If actual > expected: executor stops and reports

FIND:
```<language>
<exact existing code — copy-paste from the file, whitespace-sensitive>
```

CHANGE TO:
```<language>
<exact new code>
```

<!-- VERIFY must be cheap, scoped, and terminating: the narrowest command that proves THIS edit. Not the full suite (that runs at the phase/final block), not a dev server. -->
VERIFY: `<scoped command that proves this change landed — e.g. tsc -p packages/x, npm test -- billing>`

### 1.2 <Next specific action>

**File:** `<another/path.ext>`

FIND / CHANGE TO / VERIFY — same pattern.

---

## Phase 2: <Next logical step>

<Same pattern as Phase 1.>

---

## Phase N: Final verification

RUN all of these. Each must pass:

```bash
<typecheck command>
<lint command>
<test command — including new tests from Tests Plan below>
<smoke command if applicable>
```

---

## Tests Plan *(MANDATORY — auditor verifies these were added)*

What tests cover the new behavior? Where do they live? For each:

- **<test name>** in `<test file path>` — covers <case>. Asserts <expected behavior>.
- **<test name>** — covers edge case <X>.

If a phase intentionally adds no new tests (e.g., pure refactor with existing coverage), say so explicitly here and justify: "Phase N: no new tests — refactor preserves behavior, existing `tests/foo_test.rs::test_bar` already covers."

The auditor will compare this list against `git diff --name-only` post-execution. Tests claimed → must appear in diff.

---

## Documentation Plan *(MANDATORY — auditor verifies these were updated)*

What docs need to change because of this work? Pick from:

- [ ] **API docs / docstrings** — `<file:line>` for `<symbol>` — explain the new param / return / behavior
- [ ] **User-facing README** — section `<section>` — note new feature / breaking change
- [ ] **CHANGELOG** — new entry under `[Unreleased]`
- [ ] **`CONTEXT.md`** updates — decision log entry / gotcha / glossary term (planner adds during Step 12)
- [ ] **`docs/`** — new or updated page at `<path>`
- [ ] **No external doc changes needed** — explain why (internal refactor, no behavior change)

The auditor checks each box claimed actually has a corresponding diff change.

---

## Observability Plan *(MANDATORY for code that runs in production)*

How will operators / on-call know if this code is working / broken?

- **What we'll see in success:** <log line / metric / trace span>
- **What we'll see on failure:** <error log / failure metric / span error attribute>
- **Health probes affected:** <none / `/healthz` updated / etc.>
- **Existing observability reused:** <yes — emits via existing `tracing::instrument` / metrics framework / etc.>

If this is internal code with no production runtime (dev tools, build scripts, tests): write "n/a — no production runtime" and skip.

For new production code paths with NO observability plan, the critic will flag dimension #3 as `fail`.

---

## Performance Considerations *(MANDATORY for hot-path or scale-sensitive code)*

If the code runs in a request path, in a tight loop, or on data that scales unboundedly:

- **Expected call frequency:** <one-time / per-request / per-second / per-item-in-stream>
- **Time complexity:** <O(1) / O(n in active sessions) / etc.>
- **Memory:** <allocates per call / reuses buffer / etc.>
- **Existing perf baseline:** <e.g., "mmcg_impact shows this function on the auth hot path">
- **Risks at scale:** <none / lock contention if > 100 req/sec / etc.>

If this is dev-time / cold-path code, write "n/a — not hot path" and skip. The critic uses this section for dimension #2.

---

## Checklist

The executor ticks `[ ]` → `[x]` as it completes each item. The auditor verifies each tick during post-flight.

### Phase 1
- [ ] 1.1 — <action> done; `mmcg_callers` matched expectation pre-edit
- [ ] 1.2 — <action> done
- [ ] `<typecheck>` passes for Phase 1

### Phase 2
- [ ] 2.1 — <action> done
- [ ] `<test>` passes for Phase 2

### Phase N (final)
- [ ] All commands in Final verification passed
- [ ] No files changed outside the **File:** paths listed above
- [ ] Every test in Tests Plan appears in `git diff`
- [ ] Every doc in Documentation Plan appears in `git diff`

---

## Do NOT Do

Explicit anti-patterns specific to this task. Distinct from the global Rules above.

- Do NOT <X — a thing the executor might be tempted to do but must not>
- Do NOT <Y>
- <Specific anti-patterns surfaced by the critic — paste here if not absorbed into Rules>

---

## Notes (planner-only — executor ignores)

### Pre-flight validation
*(Planner ticks each before showing this spec to the user.)*

- [ ] All `**File:**` paths exist in the working tree
- [ ] All named symbols verified via `mmcg_search`
- [ ] All `FIND:` blocks match current file contents (whitespace-sensitive)
- [ ] `mmcg_impact` on each symbol-to-be-changed agrees with this spec's stated scope
- [ ] `VERIFY:` commands look executable, scoped to the touched subtree, and terminating (no `dev`/`start`/`watch`)
- [ ] **Alternatives Considered has ≥ 2 entries** (or "trivial change" justification)
- [ ] **Codeflow diagrams** present for every non-trivial alternative, each node mmcg-verified or marked `[NEW]` (or section explicitly skipped as trivial)
- [ ] **Decision Matrix** filled for standard/strict specs, or explicitly skipped (write "lite — no decision matrix")
- [ ] **Risk Register** filled for strict specs, or explicitly skipped (write "lite/standard — no risk register")
- [ ] **Evidence Ledger** — every non-trivial claim has a row, assumptions are explicit (or section skipped for lite)
- [ ] **Pre-edit symbol snapshot** filled via mmcg for every edited function/method (or section deleted if no code symbols touched)
- [ ] **Tests Plan is concrete** (per-test what's covered)
- [ ] **Documentation Plan** lists every doc touched
- [ ] **Observability Plan** addresses production runtime OR explicitly marked n/a
- [ ] **Performance Considerations** addresses hot/scale OR explicitly marked n/a

### Design-time critic verdict
*(If `mastermind-critic` was spawned — paste the 7-dimension table here.)*

- **Spawn:** <YYYY-MM-DD HH:MM> — brief: <what was sent>
- **Dimension scores:** <copy the 7-row table from the critic output>
- **Aggregate verdict:** `<ship it | ship with caveats | revise | rethink>`
- **Planner's disagreements (if any):** <if planner overrode any critic finding, document why here>

### CONTEXT.md updates from this task
*(Filled in by planner during Step 12 — what gets appended to project's CONTEXT.md.)*

- Decision log: <YYYY-MM-DD> — <decision name>
- Known gotchas: <one-line summary>
- (etc.)

### Context links
- Spec author: <github-handle or "planner">
- Related issues / docs: <links>
- mmcg index version at spec time: `<output of mmcg_status>`
