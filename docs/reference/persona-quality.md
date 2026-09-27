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

Synthetic improvements do not establish real-history accuracy. This release
makes no such accuracy claim.

See [persona commands](persona.md) for curation and publication, and the
[mining contract](persona-mining-contract.md) for Git measurements.
