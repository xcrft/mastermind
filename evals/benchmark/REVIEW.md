# Offline answer review

The review CLI freezes a planned benchmark batch, imports structured assessments
and accounts for every attempt. It makes no model calls and does not execute
Git, an indexer or the researched application. Python 3.10+ and POSIX are required.

Review supports the structured research keys in the
[calibration corpus](README.md#calibration-corpus). Custom keys with only
task/revision fields and decision-task grading are outside this path.

## Export

```sh
python3 -m evals.benchmark_review export /absolute/path/to/batch-id \
  --output /absolute/path/to/new-review-export
```

The output parent must exist. Use a new directory outside the batch. Inputs must
be canonical and contain no symlinks. Relocated archives are supported without
requiring their original runtime executables.

| Output | Purpose |
|---|---|
| `reviewer/packet.json` | Public task, rubric, source descriptions and shuffled opaque attempt IDs |
| `reviewer/source/` | Verified common source snapshot |
| `reviewer/answers/` | Exact retained answers |
| `reviewer/assessment-template.json` | Original blank form |
| `coordinator.json` | Private mapping to conditions, repetitions and execution records |
| `seal.json` | Bound packet, form and coordinator hashes |
| `reviews/` | Immutable imported assessments |

Give the reviewer only `reviewer/` and an editable copy of the form. Keep the
coordinator and raw run artifacts separate. Conditions, model names and timings
are omitted from reviewer metadata, but answer text can still reveal them.
The tool does not contact or send data to a reviewer.

The original reviewer folder has a checked inventory. Extra or missing files,
changed originals or symlinks invalidate the packet. Keep completed forms
outside it. If a batch changes during export, let it stabilize and export again
to a new directory.

## Account for all attempts

| Attempt state | Accounting |
|---|---|
| Completed with an intact declared answer | Completed and reviewable |
| Failed with an intact declared answer | Failed and reviewable |
| Setup failure | Failed |
| Prepared with no result or lock | Not run |
| Lock present without result | Unfinished. Activity versus crash is unknown |
| Missing trial or manifest | Missing artifacts |

Only the answer declared in `result.json` is admitted. Orphan answers or trace
fragments are not recovered as completed outputs. Retained results require their
intact empty `run.lock`. Bound batches also require `execution.lock`, the plan
binding and predecessor chain. Complete chains are `verified`, intact incomplete
prefixes `partial`, and unestablished chains `not_established`. Older unbound
batches remain `unverified_legacy`.

An intact source snapshot from another trial with the same common identity can
supply review evidence for a failed trial. The failed trial remains marked as
such. Retained answers without any intact common source cannot be exported.

## Assess and import

```sh
cp /absolute/path/to/review-export/reviewer/assessment-template.json /absolute/path/to/assessment.json
chmod u+w /absolute/path/to/assessment.json
```

Set a stable lowercase `reviewer` label and complete every retained answer.
Keep generated packet, answer, rubric and export hashes unchanged.

| Field | Required assessment |
|---|---|
| `claims` | Exact excerpts, supported/unsupported/contradicted/unknown status, source anchors, material-error flag and rationale |
| `knowns` | Every required fact, with an answer excerpt or a missing value |
| `unknowns` | Every expected unknown, with appropriate/overclaimed/omitted status and an excerpt or a missing value |

Supported and contradicted claims require source anchors. A material error must
be unsupported or contradicted. The importer checks excerpt membership and
anchor ranges. The reviewer is responsible for semantic judgment and complete
material-claim selection.

```sh
python3 -m evals.benchmark_review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/assessment.json
python3 -m evals.benchmark_review status /absolute/path/to/review-export
```

Each reviewer gets one immutable receipt. Partial, duplicate, stale or invented
assessments are rejected. Concurrent import returns `review_busy`. Retry after
the other import finishes. Additional reviewers are retained separately, up to
64, without averaging away disagreement. Labels do not verify reviewer identity.

`status` verifies artifacts and reports attempts, execution-order integrity,
reviewers, reviewed answers and descriptive assessments overall/by condition.
It preserves material-error flags, required-known coverage, unknown handling
and disagreement. Reviewer-selected claim rows are not aligned votes.
Multiple reviewers do not increase the number of experiments.

## Evidence boundary

Hashes bind local artifacts. They are not signatures or protection against an
owner rewriting the entire archive. Semantic assessments remain declarations.
Original `result.json` files retain their transport and quality status.
Retained answers stay `review_pending` in those original records. Results keep
`comparison_accepted: false` and `quality_uplift: null`. A published calibration
corpus and manual review alone do not establish held-out quality.

Reads/writes reject symlinks. Publication checks final file identity and bytes.
Interrupted unsealed exports are invalid.

| Resource | Limit |
|---|---:|
| Planned attempts | 60 |
| Source files | 128 |
| Common source | 8 MiB |
| Control or assessment JSON | 1 MiB each |
| Exported payload | 64 MiB |
| Input reads including rechecks | 1 GiB |
| Answer | Producer cap, at most 16 MiB |

```sh
python3 -m unittest evals.test_benchmark_review evals.test_benchmark_corpus
```

These tests exercise accounting, relocation, source recovery, stale evidence,
review admission and publication using disposable fixtures without model calls.
