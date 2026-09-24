# Offline answer review

`benchmark_review.py` exports a complete planned batch for manual assessment.
It does not call a model, Git, an indexer, or the researched code. It accepts
the research-key schema used by the [calibration corpus](README.md#calibration-corpus).

## 1. Export

```bash
python3 -m evals.benchmark_review export /absolute/path/to/batch-id \
  --output /absolute/path/to/new-review-export
```

The output directory must be new and outside the batch. Give reviewers only
`reviewer/` and a writable copy of `assessment-template.json`. Keep
`coordinator.json` private: it maps opaque answer IDs to conditions and trials.

The packet includes every planned attempt. Completed answers and partial answers
from failed runs are retained when available. Missing, not-run, and unfinished
attempts remain in the accounting; they are not silently dropped. Answer text
can reveal its condition, so blinding is imperfect.

## 2. Assess every retained answer

```bash
cp /absolute/path/to/review-export/reviewer/assessment-template.json \
  /absolute/path/to/assessment.json
chmod u+w /absolute/path/to/assessment.json
```

Set `reviewer` to a stable lowercase label. Complete every entry in `reviews`:

| Field | Record |
|---|---|
| `claims` | Exact answer excerpt, source anchors, support judgment, material-error flag, rationale |
| `knowns` | Coverage of each required-known item |
| `unknowns` | Handling of each expected-unknown item |

Use exact excerpts from the retained answer. Supported or contradicted claims
need source anchors. The importer checks excerpts and anchor locations; it
cannot decide whether the review judgment is true. Partial forms are rejected.

## 3. Import and inspect

```bash
python3 -m evals.benchmark_review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/assessment.json
python3 -m evals.benchmark_review status /absolute/path/to/review-export
```

An import creates one immutable receipt per reviewer. Other reviewers can
submit separate assessments. `status` verifies the packet and receipts, then
reports attempt counts, execution-order evidence, coverage declarations, and
reviewer disagreement. It does not average reviews or rank conditions.

The original `result.json` files retain their transport status and
`review_pending` state. An assessment does not make the experiment comparable.

## Limits

The export checks source, answer, and result identities. It needs an intact
source snapshot when answers are retained. A retained result without its
one-shot `run.lock` is rejected; bound batches also need `execution.lock`. Legacy
batches cannot claim verified execution order.

The workflow caps batches at 60 attempts, 128 source files, 8 MiB of common
source, and 64 MiB of exported payload. Larger or changed evidence fails
explicitly. Hashes detect mismatch, not a host owner rewriting all artifacts.
Separate directories are not a host sandbox. Reviews are declarations, not an
automatic semantic score; `comparison_accepted` remains `false` and
`quality_uplift` remains `null`.
