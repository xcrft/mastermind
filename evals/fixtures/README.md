# Concept relevance diagnostics

Compare lexical symbol retrieval on identical source bytes. Queries and sparse
source anchors must be frozen before inspecting candidate output. Every observed
suite becomes a regression set, so choose fresh queries for further qualification.

| Suite | Use after the retained runs |
|---|---|
| `concept-relevance.json` | Original 20261015 regression corpus |
| `concept-regression-20261016.json` | Same intents rebound to the enriched source snapshot |
| `concept-relevance-20261016.json` | First 20261016 selection set, with targeted checks separate |
| `concept-relevance-20261016-holdout2.json` | Rejected query-filtering variant's observed qualification set |
| `concept-relevance-20261016-holdout3.json` | Final ranking variant's observed qualification set |
| Independent V6-V11 fixtures | Retained observed sets. Use them as regression data |
| Independent V12 fixture, source evidence and freeze | Query/anchor freeze for the [source-witness study](../baselines/concept-witness-20261016.md) |
| V12 `.hints.json` and `.request-contracts.json` | Query-only recall and constraint inputs fixed before retrieval |

Prerequisites: compiled baseline and candidate `mmcg` binaries, Python 3, Git,
and a disposable source snapshot matching every `source_files_sha256` entry.
The snapshot must have a Git source inventory: when using `git archive`, run
`git init` and `git add --all` inside that disposable directory before indexing.
Keep the SQLite indexes outside that snapshot and the working project index.

```sh
"$BASELINE_MMCG" --index "$BASELINE_INDEX" index "$SOURCE_SNAPSHOT"
"$CANDIDATE_MMCG" --index "$CANDIDATE_INDEX" index "$SOURCE_SNAPSHOT"
python3 -m evals.benchmark.concept_relevance \
  --binary "$BASELINE_MMCG" --index "$BASELINE_INDEX" \
  --source "$SOURCE_SNAPSHOT" --suite "$CONCEPT_SUITE" \
  --output /tmp/concept-baseline.json
python3 -m evals.benchmark.concept_relevance \
  --binary "$CANDIDATE_MMCG" --index "$CANDIDATE_INDEX" \
  --source "$SOURCE_SNAPSHOT" --suite "$CONCEPT_SUITE" \
  --output /tmp/concept-candidate.json
```

Each output is a new immutable JSON record with full replies, binary/index/source
digests, per-query ranks and separate development/independent summaries. Existing
output paths are refused. On an index or source mismatch, reconstruct the pinned
snapshot and rebuild with the matching binary, then use a new output path.
Set `CONCEPT_SUITE` to the selected fixture's absolute path. The retained
20261016 source is reconstructed from Git commit
`f418b380bf191877f5903f5c85f483f5e0e86632` plus
[`concept-source-20261016.patch`](../baselines/concept-source-20261016.patch), then
[`concept-witness-20261016-source-supplement.patch`](../baselines/concept-witness-20261016-source-supplement.patch).
Apply both patches in that order inside a disposable Git archive before initializing its source
inventory. Its checked source-tree digest and implementation bindings are in
the [source-witness report](../baselines/concept-witness-20261016.md).

| Measurement | Interpretation |
|---|---|
| Hit@k | Fraction of positive queries with a listed relevant anchor. Empty queries stay in the denominator |
| Anchor recall | Fraction of listed anchors returned. Alternatives need not all be returned for a query to be useful |
| Judged precision | Relevant rows divided by relevant plus explicit distractor rows. Always report unjudged rows |
| Complete source review | Judge the union of both result pages with variant and rank hidden. Keep unknowns and disclose model review separately from frozen anchors |
| Negative results | Returned lexical candidates for unsupported domain requests. Inspect these separately |
| Failed queries | Unknown measurements kept in planned denominators. Point hit@k is withheld |

These measurements describe behavior on the frozen queries. They do not measure
general semantic quality, token savings or the frequency of real user queries.

For source-reviewed behavior, follow the [operation-review commands](../benchmark/README.md#evaluate-operation-relevance-with-source-review).
Pass the matching `.hints.json` with `--hints` and `.request-contracts.json`
with `--request-contracts`. Keep the fixture and its anchors away from source
reviewers. A second reviewer assesses every emitted declaration without seeing
the first labels, roles or ranks. Rerunning these now-observed queries provides
regression evidence, and further qualification needs a newly authored frozen set.
