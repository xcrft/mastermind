# Offline answer review

The review CLI freezes a planned benchmark batch, imports structured assessments
and accounts for every attempt. It makes no model calls and does not execute
Git, an indexer or the researched application. Python 3.10+ and POSIX are required.

Keys with `acceptance_criteria` export schema 3 assessments. Judge every criterion
against its original request excerpt and quote the retained answer when it is met.
Any unmet criterion, material error or overclaimed unknown makes the task
unsatisfied; otherwise unresolved criteria keep it unknown. Source coverage
remains separately visible. Legacy schema 1/2 assessments retain their original
full-key semantics and cannot be pooled with user-acceptance outcomes.

See [product objectives](../PRODUCT.md) for the acceptance and resource gates.

Review supports the structured research keys in the
[calibration corpus](README.md#calibration-corpus). Custom keys with only
task/revision fields and decision-task grading are outside this path.

## Export

```sh
python3 -m evals.benchmark.review export /absolute/path/to/batch-id \
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
| `outcome` | Whether the answer satisfies the original task: `satisfied`, `unsatisfied` or `unknown`, with a rationale |

Supported and contradicted claims require source anchors. A material error must
be unsupported or contradicted. The importer checks excerpt membership and
anchor ranges. The reviewer is responsible for semantic judgment and complete
material-claim selection.

The version 2 form requires an explicit task outcome. `satisfied` must agree
with covered required facts, appropriate unknown handling and supported selected
claims without material errors. This consistency check does not verify the
reviewer's judgment. Version 1 submissions remain readable; their task outcomes
stay unknown rather than being inferred from selected claims.

```sh
python3 -m evals.benchmark.review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/assessment.json
python3 -m evals.benchmark.review status /absolute/path/to/review-export
```

Each reviewer gets one immutable receipt. Partial, duplicate, stale or invented
assessments are rejected. Concurrent import returns `review_busy`. Retry after
the other import finishes. Additional reviewers are retained separately, up to
64, without averaging away disagreement. Labels do not verify reviewer identity.

`status` verifies artifacts and reports attempts, execution-order integrity,
reviewers, reviewed answers and descriptive assessments overall/by condition.
It preserves task outcomes, material-error flags, required-known coverage, unknown handling
and disagreement. Reviewer-selected claim rows are not aligned votes.
Multiple reviewers do not increase the number of experiments.

For a corpus-wide campaign, repeat import in every case directory under the
review set. Use the same reviewer label only for the same reviewer. The campaign
comparison checks each export against its original batch, task and key.

## Compare paired outcomes

```sh
python3 -m evals.benchmark.review compare /absolute/path/to/review-export \
  --baseline raw --candidate refined
```

Use IDs from the planned matrix. For default batches, for example, compare
`source` with `portable`. Every repetition contributes a pair under the same
original task and key. Each reviewer gets a separate result.

| Evidence | Task-success value |
|---|---|
| Completed, verified runtime/source and declared `satisfied` | 1 |
| Declared `unsatisfied`, failed attempt or recorded runtime-contract violation | 0 |
| Unreviewed, unknown, not run, unfinished, missing evidence or unverified runtime | [0, 1] |

`success_delta` is candidate minus baseline across **all planned pairs**. Its
lower/upper values include unresolved outcomes; they are uncertainty bounds,
not confidence intervals. `point` exists only when the bounds coincide. Wins,
ties and losses describe resolved pairs. Repetitions of one task do not establish
performance on unseen tasks or a causal product-quality change.

The coordinator also retains setup/run time, four token counters, turns and cost
per attempt. `resources` reports observed totals and measured/unknown trial counts
per condition, including failures. Missing measurements remain null; a partial
total is not a complete cost estimate. This scope covers trial preparation and
adapter attempts, excluding external instruction generation and native client
delivery. Reviewer files contain no resource metadata.

`efficiency` counts successful outcomes and resources across every planned
attempt, including failures. Each reviewer gets a separate estimate. Campaign
totals combine that reviewer's cases; missing reviews preserve unknown outcome
bounds.

| Metric | Definition and boundary |
|---|---|
| Context tokens | Uncached input + cache read + cache write |
| Total tokens | Context + output |
| `run_seconds` | Adapter invocation time |
| `trial_total_seconds` | Trial setup + invocation; excludes campaign coordination, refinement generation and profile mining |
| Resource per successful outcome | Complete resource total / known successful outcomes; undefined with zero successes |
| Useful outcomes per resource | Known successes / complete positive resource total; zero when known successes are zero |
| Resource savings | `1 - candidate_resource / baseline_resource` |
| Resource per success savings | `1 - candidate_per_success / baseline_per_success` |
| Useful outcome yield gain | `candidate_yield / baseline_yield - 1`; undefined when baseline yield is zero |

Point estimates require resolved outcomes and complete measurements. A partial
token total does not become a complete total. `quality_preserved_on_resolved_pairs`
requires all pairs resolved and no previously successful baseline losing its
success. Zero successes in both arms can meet that flag while providing no
useful-work benefit. These are descriptive results on the declared corpus, not
population confidence intervals or independently established causal uplift.

When resource totals are complete, unresolved outcomes still give deterministic
yield bounds: `S_lower / C` through `S_upper / C`. For a guaranteed positive
baseline yield, relative gain ranges from
`candidate_lower / baseline_upper - 1` through
`candidate_upper / baseline_lower - 1`. A potentially zero baseline, zero
resource total or missing resource measurements leaves the relative range
unknown. These ranges describe possible review outcomes; they are not confidence
intervals and do not establish reviewer independence.

New exports use coordinator version 3. Versions 1 and 2 remain readable with
unknown resource measurements and runtime checks. Old records are not upgraded
to stronger evidence merely by importing them.

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
python3 -m unittest tests.evals.benchmark.test_review tests.evals.benchmark.test_corpus
```

These tests exercise accounting, relocation, source recovery, stale evidence,
review admission and publication using disposable fixtures without model calls.
