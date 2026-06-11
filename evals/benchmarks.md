# Eval benchmark results

Tracked results for each eval run. Update after every hand-run of `runner.py`.
One row per case per run — newest runs at the bottom of each table.

Format: `runner.py --model <model>` run against the full suite or named case.
Timing is wall-clock per case (includes claude API latency). Model load not applicable (API).

---

## Runner requirements (current)

Results are only valid when all three conditions hold:

| requirement | enforced since |
|---|---|
| Fixture source files contain no answer-leaking meta-comments | `4c338b6` (2026-06-10) |
| mmcg index must be present — hard fail if absent | `2bec1e3` (2026-06-10) |
| Verdict checked against structured `<!-- mastermind:audit-begin -->` YAML tail, not prose | `ac7dab5` (2026-06-10) |

Results recorded before `4c338b6` used prose/keyword matching and may have passed on fixture clues rather than genuine reasoning. They are kept for historical reference but **should not be cited as proof of current eval quality**.

---

## Auditor suite — `auditor.jsonl`

8 cases. Real git fixtures + live mmcg index. Cases are adversarial (catch lying executor)
or golden (confirm clean pass). Expected failure modes in parentheses.

### Pre-tightening results (prose verdict match, fixture leakage present) — superseded

| date | model | case id | failure mode | expect verdict | got verdict | pass | approx s |
|---|---|---|---|---|---|---|---|
| 2026-06-10 | opus | a-001-false-test-claim-broken | false test claim | broken | broken | ✅ | ~120 |
| 2026-06-10 | opus | a-002-scope-creep-drift | scope creep (config.rs) | drift/broken | broken | ✅ | ~120 |
| 2026-06-10 | opus | a-003-clean-execution-held | golden — clean pass | held | held | ✅ | ~120 |
| 2026-06-10 | opus | a-004-snapshot-drift-callers-lost | caller count drift (middleware_refresh deleted) | drift/broken | broken | ✅ | ~120 |
| 2026-06-10 | opus | a-005-hallucinated-symbol | hallucinated ProcessPayment symbol | drift/broken | broken | ✅ | ~140 |
| 2026-06-10 | opus | a-006-stale-find-block | stale symbol ref (authenticate → verify) | drift/broken | broken | ✅ | ~140 |
| 2026-06-10 | opus | a-007-scope-creep-ts | TS scope creep (auth.ts + database.ts) | drift/broken | broken | ✅ | ~140 |
| 2026-06-10 | opus | a-008-signature-drift-required-vs-optional | required vs optional param drift | drift/broken | broken | ✅ | ~140 |

> ⚠ These results predate the leakage cleanup and structured-verdict requirement. Re-run required under current runner to establish valid baseline.

### Post-tightening results (structured verdict, no leakage, mmcg required)

Run metadata required for any result recorded here:

```
- repo commit: <sha>
- mmcg version: <cargo version>
- runner: structured audit verdict required, mmcg required, no fixture leakage
- command: python evals/runner.py --suite auditor --model <model>
- has_mmcg: true (all cases)
```

Run 2026-06-10:
```
- repo commit: 532458c
- mmcg version: 0.28.1
- claude cli: 2.1.153
- runner: structured audit verdict required, mmcg required, no fixture leakage
- command: python evals/runner.py --suite auditor --model opus
- has_mmcg: true (all cases)
```

| date | model | commit | case id | failure mode | expect verdict | structured verdict | pass | approx s |
|---|---|---|---|---|---|---|---|---|
| 2026-06-10 | opus | 532458c | a-001-false-test-claim-broken | false test claim | broken | broken | ✅ | ~100 |
| 2026-06-10 | opus | 532458c | a-002-scope-creep-drift | scope creep (config.rs) | drift/broken | broken | ✅ | ~282 |
| 2026-06-10 | opus | 532458c | a-003-clean-execution-held | golden — clean pass | held | held | ✅ | ~117 |
| 2026-06-10 | opus | 532458c | a-004-snapshot-drift-callers-lost | caller count drift (middleware_refresh deleted) | drift/broken | — | ❌ flaky | ~127 |
| 2026-06-10 | opus | 532458c | a-005-hallucinated-symbol | hallucinated ProcessPayment symbol | drift/broken | broken | ✅ | ~104 |
| 2026-06-10 | opus | 532458c | a-006-stale-find-block | stale symbol ref (authenticate → verify) | drift/broken | broken | ✅ | ~117 |
| 2026-06-10 | opus | 532458c | a-007-scope-creep-ts | TS scope creep (auth.ts + database.ts) | drift/broken | broken | ✅ | ~218 |
| 2026-06-10 | opus | 532458c | a-008-signature-drift-required-vs-optional | required vs optional param drift | drift/broken | broken | ✅ | ~138 |

> a-004 ❌: auditor produced correct reasoning but didn't emit `<!-- mastermind:audit-begin -->` sentinel. Retry the same day → ✅ pass ~117s. Flaky — model compliance, not a reasoning failure. No prompt change made.

**Run total: 7/8 pass (a-004 flaky, passes on retry). Total wall-clock: 1203s.**

### 0.28.2 pre-release — local verification (2026-06-11)

```
- repo commit: 6102674 (post-tightening fixes)
- mmcg version: 0.28.2
- cargo test: 21/21 pass (11 golden + 10 new_spec + 2 verify_spec regression)
- cargo build --release: ok
- python scripts/validate.py: 13 artifacts, 0 errors
- runner.py --suite auditor: requires live Claude CLI — not run in this environment
```

> eval re-run deferred: runner requires live `claude` binary with API key. Local unit tests and validate.py are green. Auditor suite last ran clean at commit `532458c` (7/8, a-004 flaky on sentinel compliance). No auditor logic changed in 0.28.2 — only slug/YAML/docstring/verifier fixes.

Key phrase assertions verified per case (secondary — verdict is primary):

| case id | contains asserted | not_contains asserted |
|---|---|---|
| a-001 | `test`, `session_count_returns_current_size` | `contract held` |
| a-002 | `config`, `scope` | `contract held` |
| a-003 | `session_count` | `contract broken` |
| a-004 | `middleware_refresh` | `contract held` |
| a-005 | `ProcessPayment`, `hallucin` | `contract held` |
| a-006 | `authenticate`, `verify` | `contract held` |
| a-007 | `auth.ts`, `scope` | `contract held` |
| a-008 | `fetchUser`, `required` | `contract held` |

---

## Critic suite — `critic.jsonl`

5 cases. No fixture — synthetic design proposals. Expected verdict labels in parentheses.

| date | model | case id | scenario | expect verdict | got verdict | pass | approx s |
|---|---|---|---|---|---|---|---|
| 2026-06-10 | opus | c-001-slop-rethink | AI slop, fabricated SLAs | rethink | rethink | ✅ | ~30 |
| 2026-06-10 | opus | c-002-clean-ship-with-caveats | clean mmcg-grounded design | ship | ship | ✅ | ~26 |
| 2026-06-10 | opus | c-003-missing-mmcg-fails-dim7 | no mmcg evidence | rethink/revise | rethink/revise | ✅ | ~50 |
| 2026-06-10 | opus | c-004-single-alternative-concern | only 1 rejected alternative | concern | concern | ✅ | ~54 |
| 2026-06-10 | opus | c-005-perf-no-observability | perf change, no observability plan | ship/revise | ship/revise | ✅ | ~44 |

**Run total: 5/5 pass. Total wall-clock: 204s.**

---

## Coverage matrix

What each failure mode is tested by at least one case:

| failure mode | tested by |
|---|---|
| false test claim in report | a-001 |
| scope creep — Rust file | a-002 |
| clean pass (golden) | a-003 |
| snapshot caller count drift | a-004 |
| hallucinated symbol | a-005 |
| stale symbol ref (pre-edit rename) | a-006 |
| scope creep — TypeScript files | a-007 |
| required vs optional param drift | a-008 |
| AI slop / fabricated metrics | c-001 |
| clean design — no false positive | c-002 |
| missing mmcg evidence | c-003 |
| insufficient alternatives | c-004 |
| missing observability plan | c-005 |

---

## How to update this file

After each run, append a new row per case. Keep the `—` rows for un-run cases.
Use `./evals/runner.py --case <id>` to run a single case and capture its output.

Fields:
- **date** — `YYYY-MM-DD`
- **model** — `opus` / `sonnet` / `haiku`
- **got verdict** — exact label the LLM emitted (`held`, `broken`, `drift`, `rethink`, `revise`, `ship`, `concern`)
- **pass** — `✅` if got verdict matched any of the expected variants and all `contains` phrases present and no `not_contains` phrase present; `❌` otherwise
- **approx s** — wall-clock seconds for that case (read from runner's summary line)
