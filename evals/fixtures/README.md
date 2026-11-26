# Concept relevance fixtures

Use these frozen query suites to compare lexical concept search between two `mmcg` builds on identical source bytes. The runner makes no model call. Every suite here has already been looked at, so each one is now regression data, and a new qualification needs freshly written queries and anchors frozen before you see any output.

| Suite | Use |
|---|---|
| `concept-relevance.json`, `concept-regression-20261016.json` | Original regression corpus, and the same intents on the 20261016 snapshot |
| `concept-relevance-20261016*.json`, independent `-v6` to `-v11` | Earlier selection, holdout and independent sets |
| `concept-relevance-20261016-independent-v12.json` with `.source-evidence.json` and `.freeze.json` | The set behind the [source-witness study](../baselines/concept-witness-20261016.md) |
| V12 `.hints.json` and `.request-contracts.json` | Query-only hints and constraints for operation review |

## Prerequisites

You need compiled baseline and candidate `mmcg` binaries, Python 3, Git and a disposable source snapshot whose files match every `source_files_sha256` entry of the suite. If you build the snapshot with `git archive`, run `git init` and `git add --all` inside it before indexing. Keep the SQLite indexes outside the snapshot and outside the project's own index. To rebuild the 20261016 snapshot, archive commit `f418b380bf191877f5903f5c85f483f5e0e86632` into a disposable directory, then apply [concept-source-20261016.patch](../baselines/concept-source-20261016.patch) and [concept-witness-20261016-source-supplement.patch](../baselines/concept-witness-20261016-source-supplement.patch) in that order.

## Compare two builds

Set `CONCEPT_SUITE` to the absolute path of a suite, then run from the repository root:

```sh
"$BASELINE_MMCG" --index "$BASELINE_INDEX" index "$SOURCE_SNAPSHOT"
"$CANDIDATE_MMCG" --index "$CANDIDATE_INDEX" index "$SOURCE_SNAPSHOT"
python3 -m evals.benchmark.concept_relevance --binary "$BASELINE_MMCG" --index "$BASELINE_INDEX" \
  --source "$SOURCE_SNAPSHOT" --suite "$CONCEPT_SUITE" --output /tmp/concept-baseline.json
python3 -m evals.benchmark.concept_relevance --binary "$CANDIDATE_MMCG" --index "$CANDIDATE_INDEX" \
  --source "$SOURCE_SNAPSHOT" --suite "$CONCEPT_SUITE" --output /tmp/concept-candidate.json
```

Each output is a new JSON record with the full replies, binary, index and source digests, and per-query ranks. An existing output path is refused. On an index or source mismatch, rebuild the snapshot and the index with the matching binary, then use a new output path.

| Measurement | How to read it |
|---|---|
| Hit@k | Positive queries with a listed anchor in the top k. Empty and failed queries stay in the denominator |
| Anchor recall | Listed anchors that came back |
| Judged precision | Relevant rows over relevant plus distractor rows. Unjudged rows are reported apart |

These numbers describe the frozen queries only. Inspect candidates returned for unsupported requests separately. Aggregate hit@5 can hide a loss on one query that a gain on another offsets, so check each earlier useful row.

For source-reviewed operation relevance, follow the [operation-review steps](../benchmark/README.md#evaluate-operation-relevance-with-source-review) with the V12 `.hints.json` as `--hints` and `.request-contracts.json` as `--request-contracts`. Keep the fixture and its anchors away from the source reviewers.
