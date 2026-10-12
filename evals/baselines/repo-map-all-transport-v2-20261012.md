# Integrated repo-map: all task questions

**Result:** total producer tokens -6.9%, uncached input -12.0%, calls +0.3%. Blind model reviews satisfied 6/8 map answers and 5/8 baseline answers; baseline has 1 unresolved assessment. Human reference is pending. The native command is available on demand; this is not a qualified universal improvement.

Eight task questions, sixteen fresh producer slots, alternating arm order. Four registered benchmark tasks retain their original revision; four added repo-map questions retain theirs. Both arms use the same frozen native binary, source projection and tools per question. The candidate adds the native-generated shortlist and direct-read instruction to the first prompt.

| Task | Producer baseline/map | Total tokens change | Calls change | Blind baseline/map |
|---|---|---:|---:|---|
| task-phase-continuity-01 | completed/completed | +7.1% | +35.3% | satisfied/satisfied |
| document-evidence-boundaries-01 | completed/completed | -23.2% | -25.0% | unsatisfied/satisfied |
| callees-definition-boundaries-01 | completed/completed | -8.1% | -6.1% | satisfied/satisfied |
| reference-removal-evidence-01 | completed/completed | +2.8% | +15.6% | unresolved/unsatisfied |
| source-receipt-navigation-01 | completed/completed | -0.2% | -12.9% | satisfied/satisfied |
| concept-match-navigation-01 | completed/completed | +30.9% | +36.4% | unsatisfied/satisfied |
| source-root-bindings-01 | completed/completed | -9.4% | +2.1% | satisfied/satisfied |
| documentation-concepts-01 | completed/completed | -33.9% | -28.6% | satisfied/unsatisfied |

| Scope | Total tokens change | Calls change |
|---|---:|---:|
| all, all attempts | -6.9% | +0.3% |
| public, all attempts | -1.5% | +10.8% |
| repo_map, all attempts | -9.9% | -6.1% |
| 8/8 matched completed pairs | -6.9% | +0.3% |

Producer attempts: 16, failed attempts: 0. Blind assessments: 15/16 source-anchor-valid, from 16 review attempts. Unique model sessions: 32. Human reference labels are pending.

The initial invalid transport revision retained sixteen extra producer sessions with no research tools. Their token usage is reported separately as recovery overhead in JSON, and the combined producer workflow accounting includes them. Interrupted review usage can remain unknown. They are not sixteen additional independent tasks or algorithm evidence.

Unknown usage stays unknown; a failed slot is not a zero-token answer or a semantic rejection. Matched completed subsets do not measure all-attempt spend. Calls include unique started calls; completed calls, pending calls and delivery ledgers are retained in JSON. Requested model identity is observed only as a request, and elapsed time includes service queuing.

## Run and recover

Requires the retained source/index/native/CLI bindings and existing Codex subscription. Inspect input-seal.json before running. Do not edit or delete immutable attempt records.

```sh
python3 -B .mastermind/research/repo-map-all-transport-v2-20261012/pilot.py --run
python3 -B .mastermind/research/repo-map-all-transport-v2-20261012/review.py
python3 -B .mastermind/research/repo-map-all-transport-v2-20261012/summarize.py
```

Human packet: `.mastermind/research/repo-map-all-transport-v2-20261012/human-review/`. Use the source_directory bound to each question; conditions and model outcomes are omitted. After interruption, preserve partial traces and create a new input revision for recovery rather than deleting or resampling productive/unfavorable attempts.

All questions are development calibration on one repository. One attempt per condition does not establish broad savings. The native source snapshot includes uncommitted working changes and is pinned separately from its base revision. This is an opt-in implementation and measurement, not human-qualified production benefit.
