# Persona extraction quality

## What the detector measures

| `persona-explicit-v2` input or result | Contract |
|---|---|
| Selected input | Complete short human segments stating a preference, self-report or correction |
| Retained evidence | Exact normalized quote, record digest, session, line and segment |
| Conditional language | Requires first-person author wording and rejects detected quoted/reported speech |
| Example | “When reviewing code, I check which layer owns the state.” |
| Future-reply correction | Retained as a correction candidate |
| Task-bound preference | Keeps its limitation |
| Lexical detection | Does not prove attribution or infer durable habits, motives, personality, independent episodes or global scope |

| Collection event | Required action |
|---|---|
| Initial selection | Explicit `miner collect` |
| Later refresh | `miner sync` rereads registered selection |
| Native local hooks | Enabled profile delivery processes complete closed episodes at `Stop` and on later context append, without a model |
| Hook candidate bounds | Exact eligible user prose, prompts up to 128 lines, at most 8 drafts, complete normalized statement up to 200 characters |
| Multiline source | Retain the whole condition/exception and original source span; never truncate to a prefix |
| Hook replay | Explicit `miner hooks mine-local`, no repair of missing events |
| Extractor fingerprint change | Recollect and inspect retained observations |
| Changed evidence binding | Rebind and review |
| Extractor upgrade | Cannot accept a claim |

## Reproducible regression corpus

Fixture: `mcp/servers/mmcg/tests/fixtures/persona_detector.json`.

| Corpus property | Scope |
|---|---|
| Synthetic examples | 58 RU/EN/mixed cases, 32 relevant signals and 26 negative controls |
| Labels | Supplied by the implementation author, not independent human evaluation |
| Coverage | Preferences, decision boundaries, conditional behavior, corrections, negations, task-bound requests, project rules, quotations, reported speech, unrelated details and secret-like strings |
| `expected_kind` | Semantic label |
| `known_miss` | Declared detector limitation: two implicit statements and one segment over 300 characters |
| Population | Synthetic regression set, not user history or a held-out sample |

Run from the repository root:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --lib --locked \
  synthetic_multilingual_candidate_detection_evaluation -- --nocapture
```

| Regression output or gate | Meaning |
|---|---|
| V1/V2 counts | TP, FP, FN, wrong kind, language/category slices and exact missed IDs |
| Supported example | Must preserve kind and complete condition/negation |
| Negative control | Must stay excluded |
| Missed set | Must equal declared limitations. Changing one requires inspecting its labeled example |
| CLI fixtures | Separately check attribution, provenance, source drift and review boundaries |
| Accuracy claim | Candidate detection on this synthetic set only, not human attribution, recurrence, persona usefulness or semantic habit precision |

The [current replay](../../evals/baselines/persona-detector-20261007.json) retains
source hashes and the V1/V2 comparison: 10/32 vs 29/32 detected signals, 0/26
false positives for either detector. Recall changes from 31.25% to 90.625% on
these implementation-authored examples. This is a detector regression result,
not independent accuracy on user histories.

## Qualitative mining and review

| Claim component | Evidence required |
|---|---|
| Engineering approach | Situation, behavior and exception, with exact supporting words or counterexamples |
| Example topics | State ownership, decisions versus effects, contracts, failure handling, review, delivery and testing |
| Observed result | Include only when supported by evidence |
| Topic exposure or formatter setting | Keep separate from habit claims |
| `candidates propose-habit` / `propose-preference` | Retain selected evidence as a candidate |
| Curation | Inspect authorship, task/project limitations and contradictions |
| Habit independence | Distinct sessions and task episodes, plus distinct repository remotes for cross-project claims |
| Publication | Revision-bound local review |
| LLM draft | Candidate only |
| Git-only qualitative draft | Kept in `style.deep-candidate-*.md` until attribution and evidence can be reviewed, never an automatically published rule |

## Evaluation on real histories

| Real-history evaluation requirement | What to record |
|---|---|
| Corpus | Explicitly selected, consented sessions and a separate author-labeled sample |
| Split | By session/task |
| Attribution | Label coauthored, quoted and generated material separately |
| Extraction | Omissions, source coverage, precision/recall by language and signal kind |
| Final claim quality | Support, contradictions, role/workflow applicability and author corrections |

Run the source-span evaluation without a model:

```bash
mastermind miner hooks evaluate-local --input evals/persona/local.json
```

The supplied regression corpus has the 58 detector cases plus two multiline
hook cases. Its labels come from the implementation author. The adapter's
200-character behavior limit can omit detector signals allowed by the
300-character transcript contract. Those omissions stay in the recall denominator.
The [current local-hook replay](../../evals/baselines/persona-local-20261007.json)
records 31 true positives, 0 false positives and 3 misses across 60 cases:
precision 100%, recall 91.18%, held-out cases 0 and task-benefit pairs 0.
The 58-case detector comparison and 60-case hook replay have different input
contracts and denominators.

A selected-history corpus uses this schema:

```json
{
  "schema": 1,
  "provenance": "selected_history",
  "labeler": "reviewer-name",
  "cases": [{
    "id": "request-1", "session": "session-1", "partition": "held_out",
    "text": "I prefer short code reviews\nonly for trivial changes.",
    "expected_quotes": ["I prefer short code reviews\nonly for trivial changes."]
  }],
  "task_pairs": []
}
```

`expected_quotes: []` labels a negative control. The other partition is
`development`; one session cannot appear in both. The output reports exact-span
TP/FP/FN, precision, recall, excluded inputs, failing case IDs and the corpus
SHA-256. It accepts at most 128 cases and 128 task pairs in a 2 MiB file.
Input text and expected quotes are not echoed in the report.

Optional `task_pairs` entries have a SHA-256 `task_revision`, `baseline` and
`with_profile`. Each trial supplies a distinct `session`, `correct` boolean,
`iterations` (1–20), `user_corrections` (0–128), `wall_ms` (1–7200000), and
`profile_digest` (null for baseline, SHA-256 for with-profile). Repeated sessions
and task revisions are rejected. Deltas are profile minus baseline; no pairs
produce null metrics and `unmeasured`, never zero benefit.

The labels and outcomes are reviewer-supplied observations. Provenance and
reviewer independence are not authenticated by the schema. Independent
real-history labels and controlled paired trials are still required before
claiming semantic accuracy or task benefit. No such result is currently qualified.

The local hook adapter reuses this lexical detector and applies the episode's
whole-text prose, source-revision and coverage guards. Its `explicit_statement`
drafts are unreviewed quotations, not automatically accepted habits. Adapter
tests check bounds, excluded material, replay, opt-out and prior-exposure
restrictions. They do not add independent labels or establish task benefit.

See [persona commands](persona.md) for curation and publication, and the
[mining contract](persona-mining-contract.md) for Git measurements.
