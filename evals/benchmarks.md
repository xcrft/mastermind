# Eval benchmark results

Tracked results for each eval run. Update after every run of `evals/run-verified.sh` or `runner.py`.
One row per case per run — newest runs at the bottom of each table.

---

## Current baseline

| suite | run date | commit | model | result | first_pass |
|---|---|---|---|---|---|
| auditor (8 cases) | 2026-06-13 | `75630fb` | opus | **8/8** | 7/8 |
| critic (5 cases) | 2026-06-10 | `532458c` | opus | **5/5** | 5/5 |

Auditor a-004 is a persistent sentinel-compliance flake (reasoning correct, sentinel block not emitted before response ends). Runner retries once on missing sentinel; passes on retry. See [a-004 history](#a-004-history) below.

---

## Runner requirements (current)

Results are only valid when all three conditions hold:

| requirement | enforced since |
|---|---|
| Fixture source files contain no answer-leaking meta-comments | `4c338b6` (2026-06-10) |
| mmcg index must be present — hard fail if absent | `2bec1e3` (2026-06-10) |
| Verdict checked against structured `<!-- mastermind:audit-begin -->` YAML tail, not prose | `ac7dab5` (2026-06-10) |

Results recorded before `4c338b6` used prose/keyword matching and may have passed on fixture clues rather than genuine reasoning. **Do not cite pre-tightening results as proof of current eval quality.**

---

## Auditor suite — `auditor.jsonl`

8 cases. Real git fixtures + live mmcg index. Cases are adversarial (catch lying executor) or golden (confirm clean pass).

### Run: 2026-06-10 · commit `532458c` · mmcg 0.28.1 · post-tightening

```
claude cli: 2.1.153
runner: structured verdict required, mmcg required, no fixture leakage
command: python evals/runner.py --suite auditor --model opus
```

| case id | failure mode | expect | got | pass | s |
|---|---|---|---|---|---|
| a-001-false-test-claim-broken | false test claim | broken | broken | ✅ | ~100 |
| a-002-scope-creep-drift | scope creep (config.rs) | drift/broken | broken | ✅ | ~282 |
| a-003-clean-execution-held | golden — clean pass | held | held | ✅ | ~117 |
| a-004-snapshot-drift-callers-lost | caller count drift | drift/broken | — | ❌ flaky | ~127 |
| a-005-hallucinated-symbol | hallucinated ProcessPayment | drift/broken | broken | ✅ | ~104 |
| a-006-stale-find-block | stale symbol ref | drift/broken | broken | ✅ | ~117 |
| a-007-scope-creep-ts | TS scope creep | drift/broken | broken | ✅ | ~218 |
| a-008-signature-drift-required-vs-optional | required vs optional param | drift/broken | broken | ✅ | ~138 |

**7/8 pass · a-004 flaky (retry → ✅) · 1203s total**

### Run: 2026-06-13 · commit `75630fb` · mmcg 0.28.2 · 0.29 proof

```
claude cli: 2.1.153
cargo test: 179/179 pass · cargo build --release: ok · validate.py: 13 artifacts, 0 errors
runner: structured verdict required, mmcg required, no fixture leakage, retry-on-sentinel enabled
command: bash evals/run-verified.sh
```

| case id | failure mode | expect | got | pass | first_pass | s |
|---|---|---|---|---|---|---|
| a-001-false-test-claim-broken | false test claim | broken | broken | ✅ | ✅ | ~102 |
| a-002-scope-creep-drift | scope creep (config.rs) | drift/broken | broken | ✅ | ✅ | ~152 |
| a-003-clean-execution-held | golden — clean pass | held | held | ✅ | ✅ | ~133 |
| a-004-snapshot-drift-callers-lost | caller count drift | drift/broken | broken | ✅ | ❌ retry | ~133 |
| a-005-hallucinated-symbol | hallucinated ProcessPayment | drift/broken | broken | ✅ | ✅ | ~113 |
| a-006-stale-find-block | stale symbol ref | drift/broken | broken | ✅ | ✅ | ~122 |
| a-007-scope-creep-ts | TS scope creep | drift/broken | broken | ✅ | ✅ | ~303 |
| a-008-signature-drift-required-vs-optional | required vs optional param | drift/broken | broken | ✅ | ✅ | ~92 |

**8/8 pass · first_pass 7/8 · after_retry 8/8 · 1150s total**

### a-004 history

a-004 has flaked on sentinel compliance in every post-tightening run. The auditor's reasoning is correct each time — it identifies the `middleware_refresh` deletion and the caller-count drift — but the response ends before emitting `<!-- mastermind:audit-begin -->`. Root cause: output compliance, not reasoning. Fixes applied:

| date | change | effect |
|---|---|---|
| 2026-06-10 | structured verdict requirement added | exposed the flake class |
| 2026-06-13 | final self-check section added to auditor prompt | no first-pass improvement observed yet |
| 2026-06-13 | retry-on-sentinel added to runner | 8/8 after retry, honest first_pass tracking |

If a future run shows a-004 passing first_pass consistently, update this table and close the flake.

### Key phrase assertions (secondary — verdict is primary)

<details>
<summary>expand</summary>

| case id | contains | not_contains |
|---|---|---|
| a-001 | `test`, `session_count_returns_current_size` | `contract held` |
| a-002 | `config`, `scope` | `contract held` |
| a-003 | `session_count` | `contract broken` |
| a-004 | `middleware_refresh` | `contract held` |
| a-005 | `ProcessPayment`, `hallucin` | `contract held` |
| a-006 | `authenticate`, `verify` | `contract held` |
| a-007 | `auth.ts`, `scope` | `contract held` |
| a-008 | `fetchUser`, `required` | `contract held` |

</details>

### Pre-tightening results — superseded

<details>
<summary>expand (do not cite)</summary>

Run 2026-06-10 with prose verdict matching and fixture leakage present. All 8 passed, but the runner accepted keyword matches and fixtures contained meta-comments that leaked the expected answer. These results predate `4c338b6` and are not valid proof of reasoning quality.

| case id | got verdict | pass |
|---|---|---|
| a-001 | broken | ✅ |
| a-002 | broken | ✅ |
| a-003 | held | ✅ |
| a-004 | broken | ✅ |
| a-005 | broken | ✅ |
| a-006 | broken | ✅ |
| a-007 | broken | ✅ |
| a-008 | broken | ✅ |

</details>

---

## Critic suite — `critic.jsonl`

5 cases. No fixture — synthetic design proposals. Critic does not use the audit sentinel; verdict matched via keyword.

### Run: 2026-06-10 · commit `532458c` · opus

| case id | scenario | expect | got | pass | s |
|---|---|---|---|---|---|
| c-001-slop-rethink | AI slop, fabricated SLAs | rethink | rethink | ✅ | ~30 |
| c-002-clean-ship-with-caveats | clean mmcg-grounded design | ship | ship | ✅ | ~26 |
| c-003-missing-mmcg-fails-dim7 | no mmcg evidence | rethink/revise | rethink/revise | ✅ | ~50 |
| c-004-single-alternative-concern | only 1 rejected alternative | concern | concern | ✅ | ~54 |
| c-005-perf-no-observability | perf change, no observability plan | ship/revise | ship/revise | ✅ | ~44 |

**5/5 pass · 204s total**

---

## Coverage matrix

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

Run `bash evals/run-verified.sh` (full gate) or `python evals/runner.py --case <id>` for a single case.

After each run, add a new `### Run:` block under the relevant suite with the metadata header, then append one row per case to a fresh table. Update the **Current baseline** table at the top.

Fields:

- **case id** — from the `.jsonl` file
- **expect** — expected verdict(s) from the case definition
- **got** — exact label emitted (`held`, `broken`, `drift`, `rethink`, `revise`, `ship`, `concern`)
- **pass** — `✅` if verdict matched and all `contains`/`not_contains` assertions held; `❌` otherwise
- **first_pass** — `✅` if passed without retry; `❌ retry` if the runner needed a sentinel retry to pass; omit column for non-auditor suites
- **s** — wall-clock seconds for the case
