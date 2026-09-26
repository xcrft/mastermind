# Persona extraction quality

## What the detector measures

`persona-explicit-v2` selects complete, short human segments as possible stated
preferences, self-reports or corrections. It retains the exact normalized quote,
record digest, session, line and segment. It does not infer durable habits,
motives, personality, independent episodes or a global scope.

V2 adds first-person engineering language and conditional descriptions such as
“When reviewing code, I check which layer owns the state.” Conditional cues
require first-person author wording and reject detected quoted/reported speech.
These lexical checks do not prove semantic attribution. Short corrections about future replies are
kept as correction candidates. A task-bound preference keeps its limitation.

Initial mining is an explicit `miner collect`; later `miner sync` rereads the
registered selection. V2 changes the extractor fingerprint. Previously retained
observations must be collected again and inspected; changed bindings need
explicit rebinding and review. An extractor upgrade never accepts a claim.

## Reproducible regression corpus

`mcp/servers/mmcg/tests/fixtures/persona_detector.json` contains 58 synthetic
RU/EN/mixed examples: 32 semantically relevant signals and 26 negative controls.
The implementation author supplied the examples and labels. They are not user
history, a held-out sample or an independent human accuracy benchmark.

The corpus covers work preferences, decision boundaries, condition-dependent
behavior, corrections, negations, task-bound requests, project rules, quoted
material, reported speech, unrelated details and secret-like strings.
`expected_kind` is the semantic label; `known_miss` separately records an
intentional detector limitation. Three examples currently remain unsupported:
two implicit statements and one segment longer than 300 characters.

Run from `mcp/servers/mmcg`:

```bash
cargo test --lib --locked synthetic_multilingual_candidate_detection_evaluation -- --nocapture
```

The test prints baseline V1 and current V2 counts (TP, FP, FN, wrong kind),
language/category slices and the exact missed IDs. It requires every supported
example to keep its kind and complete condition/negation, all negative controls
to stay excluded, and the missed set to match the declared limitations.
Changing a limitation requires inspecting its labeled example.

These are candidate-detection counts on this synthetic set. They do not measure
human attribution accuracy, recurrence, usefulness of a final persona or the
precision of semantic habit descriptions. End-to-end CLI fixtures separately
check transcript attribution, provenance, source drift and review boundaries.

## Qualitative mining and review

Engineering profiles can describe state ownership, separation of decisions and
effects, contract boundaries, failure handling, review communication, delivery
and testing habits. Each description should include a situation, behavior,
observed result, exception and exact supporting/counterexamples. Topic exposure
and repository formatter settings are separate observations.

Use `candidates propose-habit` or `propose-preference` to retain the selected
evidence. A curator must inspect actual attribution, task/project limitations
and contradictory examples. Habits need distinct sessions and task episodes;
cross-project claims additionally need distinct repository remotes. The existing
revision-bound author review controls publication. An LLM draft stays a candidate.
Git-only qualitative interpretations remain in `style.deep-candidate-*.md` until
their attribution and evidence can be reviewed; they are not published rules.

## Evaluation on real histories

Before claiming real-world quality, use explicitly selected, consented sessions
and a separate author-labeled sample split by session/task. Keep coauthored,
quoted and generated material labeled separately. Report omissions and source
coverage as well as precision/recall by language and signal kind. Evaluate final
claim support, contradictions, role/workflow applicability and author corrections
separately from extraction. Synthetic improvements do not substitute for this
assessment, and no such real-history accuracy is claimed by this release.
