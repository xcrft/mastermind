# Offline answer review

Use this runbook after a benchmark batch or campaign has run. You export the answers as blinded packets, have reviewers fill an assessment form, import each assessment and compare the arms. The review CLI makes no model calls and doesn't run Git, an indexer or the researched application, except the optional `judge` command, which calls a Claude model.

You need Python 3.10+, a POSIX host and a task key with structured research fields, as in the [calibration](README.md#calibration-corpus) and [qualification](README.md#qualification-corpus) corpora. Run every command from the repository root.

## Review a batch

1. Export the batch to a new directory outside it. The output parent must exist:

   ```sh
   python3 -m evals.benchmark.review export /absolute/path/to/batch-id \
     --output /absolute/path/to/new-review-export
   ```

2. Copy the blank form outside the export and make it writable:

   ```sh
   cp /absolute/path/to/review-export/reviewer/assessment-template.json /absolute/path/to/assessment.json
   chmod u+w /absolute/path/to/assessment.json
   ```

3. Give the reviewer only `reviewer/` and the form copy. The reviewer sets a stable lowercase `reviewer` label and fills every answer, as [the next section](#fill-the-assessment) describes.

4. Import the assessment and check the export:

   ```sh
   python3 -m evals.benchmark.review import /absolute/path/to/review-export \
     --assessment /absolute/path/to/assessment.json
   python3 -m evals.benchmark.review status /absolute/path/to/review-export
   ```

   `status` reports attempts, execution-order integrity, reviewers and outcomes overall and by condition.

5. Compare the arms, as described in [Compare paired outcomes](#compare-paired-outcomes). For a campaign, repeat steps 2 to 4 in every case directory of the review set, with the same reviewer label only for the same reviewer.

| Export file | Who sees it |
|---|---|
| `reviewer/packet.json` | Reviewer. Task, rubric, source descriptions and shuffled opaque attempt IDs |
| `reviewer/source/`, `reviewer/answers/`, `reviewer/assessment-template.json` | Reviewer. Verified source snapshot, exact kept answers and the blank form |
| `coordinator.json` | Only you. Maps attempts to conditions, repetitions and run records |
| `seal.json` | Binds packet, form and coordinator hashes |
| `reviews/` | Immutable imported assessments |

Conditions, model names and timings stay out of reviewer files, but answer wording can still give a condition away. Extra, missing or changed files inside `reviewer/` invalidate the packet, so keep completed forms outside it.

## Fill the assessment

A key with `acceptance_criteria` exports schema 3 forms. Judge every criterion against its original request excerpt, and quote the answer when it is met. Any unmet criterion, material error or overclaimed unknown makes the task `unsatisfied`, and unresolved criteria otherwise keep it `unknown`. Schema 1 and 2 assessments keep their old full-key meaning and can't be pooled with these.

| Field | What to fill |
|---|---|
| `claims` | Exact answer excerpts, a `supported`, `unsupported`, `contradicted` or `unknown` status, source anchors, a material-error flag and a rationale |
| `claims[].source_quotes` | Optional literal source excerpts, each with an anchor from the claim and the file's SHA-256 from `packet.json` |
| `knowns` | Every required fact, with an answer excerpt or a missing value |
| `unknowns` | Every expected unknown, with an `appropriate`, `overclaimed` or `omitted` status |
| `outcome` | `satisfied`, `unsatisfied` or `unknown`, with a rationale |

Leave the generated packet, answer, rubric and export hashes unchanged. Supported and contradicted claims need anchors, and `satisfied` must agree with the covered facts and claims. The importer checks that excerpts occur in the answer and that anchors are in range. It doesn't judge the reviewer's decisions.

Each source quote uses a distinct anchor from its claim, and a claim holds at most 16 quotes of at most 16 KiB each:

```json
"source_quotes": [{"anchor": "src/file.py:2-3", "source_sha256": "<SHA-256 from reviewer/packet.json source_files>", "quote": "<literal excerpt from reviewer/source/src/file.py lines 2-3>"}]
```

## Fix a failed import

| Symptom | What to do |
|---|---|
| Partial, duplicate, stale or invented assessment rejected | Fix the form and import again under the same label |
| `review_busy` | Another import holds the lock. Retry after it finishes |
| Batch changed during export | Let it settle and export again to a new directory |
| A wrong assessment was already admitted | Receipts are immutable. Import the corrected one under a new reviewer label and keep the reason |
| Packet or source files changed | Restore the sealed artifacts or export again from intact trials |

Up to 64 reviewers are kept separately, and their disagreement is never averaged. A label doesn't prove who the reviewer is.

Every planned attempt is accounted for. Completed and failed attempts with an intact declared answer are reviewable with their original status, a setup failure is failed, a prepared trial without a lock is not run, a lock without a result is unfinished, and a missing trial or manifest is missing artifacts. Only the answer declared in `result.json` is admitted.

## Assign reference and blind reviewers

| Role | Label | Who |
|---|---|---|
| Reference | `human` | A real person who sees no condition mappings or model verdicts |
| Blind model reviewer | `gpt-6-1-sol-blind` | GPT 6.1 Sol in a fresh session, following [blind-review-gpt61.md](blind-review-gpt61.md) |
| Historical model reference | The existing label, such as `claude-reference` | Kept model evidence, never relabeled as human |

Keep the human reference separate from model assessments, and keep `coordinator.json`, raw trials, receipts, the seed `key.json` and earlier results out of every review session. A reviewer who has seen aggregate results is not blind. Record the requested model, runtime, instruction hash, packet hash and any earlier exposure with the assessment. Import each reviewer's first assessment separately, and never overwrite it to improve agreement.

## Assess with a model judge

The `judge` command fills the same form with a pinned Claude Code CLI, one answer per call, and imports the result. It needs `ANTHROPIC_API_KEY`, a key with acceptance criteria and a frozen instruction. [judge-instruction.md](judge-instruction.md) is a starting instruction, not a calibrated one. It doesn't run Codex, so import GPT reviews by hand as above.

```json
{"cli": {"path": "/absolute/path/to/claude", "sha256": "<sha256>", "version": "<version>", "origin": "npm"},
 "model": "<exact model ID>", "instruction": "/absolute/path/to/judge-instruction.md", "reviewer": "judge", "attempts": 3, "timeout_seconds": 1800}
```

```sh
python3 -m evals.benchmark.judge /absolute/path/to/review-export \
  --config /absolute/path/to/judge.json --output /absolute/path/to/new-judge-run
```

An optional `"base_url"` sends the requests and the API key to another Anthropic-compatible https endpoint, for example `https://api.z.ai/api/anthropic` with `"model": "glm-5.3"`. That provider then gets the task, key, sources and answers, and the served model stays unverified.

Each call runs in `--bare` mode with only the `Read` tool, no MCP servers and a private `HOME`, in a fresh directory with one answer. The stream's init event must report exactly that runtime. An answer gets at most `attempts` tries and stops at the first review that passes import checks. A curly quote or dash that matches the answer only after ASCII mapping is repaired and counted in `quotes_repaired`. Source quotes are never repaired.

The output directory keeps every attempt as `answer-K-attempt-N.{stdout,stderr,json}`, rejected ones with their error code. It adds `assessment.json` when every answer has a valid review, `binding-audit.json` after import, and `judge-run.json` with `error` when the run stopped early. The command exits 0 only after import and audit publication. An answer with no valid review stops the run without a receipt. If the import fails after valid attempts, import the kept `assessment.json` with `review import`. If only the audit fails, keep the receipt and run `review audit-bindings` to a new path. Don't rerun the model.

## Calibrate the judge with seeded defects

This checks whether the judge rejects answers with one known defect and accepts answers the reference judged satisfied. It doesn't replace calibration against blind human review. You need an export with a reference receipt and mutants written from satisfied originals:

| Defect | Target | `passage` | Ground truth |
|---|---|---|---|
| `criterion_removed` | `criterion_id` | Text in the original, absent from the mutant | Criterion unmet |
| `fabricated_citation` | `criterion_id` | Text with wrong citations in the mutant | Criterion unmet |
| `listed_false_claim` | `false_claim_index` | The false sentence in the mutant | Material error |
| `novel_false_claim` | `{}` | A false sentence not in the key | Material error |
| `overclaimed_unknown` | `unknown_index` | The unverifiable statement | Unknown overclaimed |

```json
{"kind": "mastermind-judge-calibration-mutations", "schema_version": 1, "reference": "reference", "controls": ["review-<original id>"],
 "mutants": [{"source_review_id": "review-<original id>", "defect": "listed_false_claim",
              "target": {"false_claim_index": 0}, "answer": "mutants/m01.md",
              "passage": "<exact sentence>", "anchors": ["src/file.py:12"], "rationale": "<why it is unsatisfied>"}]}
```

```sh
python3 -m evals.benchmark.judge_calibration build /absolute/path/to/review-export \
  --mutations /absolute/path/to/mutations.json --output /absolute/path/to/new-set
python3 -m evals.benchmark.judge_calibration run /absolute/path/to/new-set \
  --config /absolute/path/to/judge.json --output /absolute/path/to/new-run
python3 -m evals.benchmark.judge_calibration score /absolute/path/to/new-run \
  --output /absolute/path/to/new-report.json
```

`answer` paths are relative to the mutations file. `build` refuses a mutant equal to its original, a passage that breaks its rule, bad anchors and controls the reference didn't mark satisfied. `score` refuses a changed set, an interrupted run and runs with different judge configurations. It reports `sensitivity`, `specificity`, `located`, `youden_j` and `gwet_ac1`, each over resolved answers and in a worst case where unresolved answers count as errors.

## Audit source bindings

Two audits check literal citations against the pinned source. Both leave assessments and outcomes unchanged, need a sealed export and write a new report outside it:

```sh
python3 -m evals.benchmark.review audit-bindings /absolute/path/to/review-export \
  --output /absolute/path/to/new-binding-audit.json
python3 -m evals.benchmark.review audit-producer-bindings /absolute/path/to/review-export \
  --output /absolute/path/to/new-producer-binding-audit.json
```

`audit-bindings` checks the reviewers' `source_quotes`. `audit-producer-bindings` reads `path:line` and `path:first-last` citations in the producers' Markdown answers and the fenced snippet right after each one. It needs no receipts.

| Status | Meaning |
|---|---|
| `valid` | The digest matches the pinned file, and the exact quote occurs within the inclusive line range |
| `invalid` | The digest, quote or range check fails (`source_digest_mismatch`, `source_quote_outside_span`) |
| `unverifiable` | No quote for the anchor, an ambiguous filename, or a producer snippet that matches only with its raw Markdown indentation (`producer_quote_raw_only`) |

Quotes are literal substrings, numbered by LF, with no whitespace or Unicode normalization. Exit 0 means the report was published, invalid bindings included. Exit 2 means an integrity, schema or publication error, and no report is written. A valid binding shows only that the text is there, not that it supports the claim.

## Compare paired outcomes

```sh
python3 -m evals.benchmark.review compare /absolute/path/to/review-export \
  --baseline raw --candidate refined
```

Use condition IDs from the planned matrix, for example `source` and `portable`. Each repetition gives a pair under the same task and key, and each reviewer gets a separate result.

| Evidence | Task-success value |
|---|---|
| Completed, verified runtime and source, declared `satisfied` | 1 |
| `unsatisfied`, a failed attempt or a runtime-contract violation | 0 |
| Unreviewed, unknown, not run, unfinished or missing evidence | [0, 1] |

`success_delta` is candidate minus baseline over all planned pairs. Its lower and upper values come from unresolved outcomes, so they are bounds, not confidence intervals, and `point` exists only when they are equal.

| Metric | Definition |
|---|---|
| Context tokens | Uncached input + cache read + cache write |
| Total tokens | Context + output |
| `run_seconds` | Adapter invocation time |
| `trial_total_seconds` | Trial setup + invocation |
| Resource per success | Complete resource total / known successes. Undefined with zero successes |
| Yield | Known successes / complete positive resource total |
| Resource savings | `1 - candidate_resource / baseline_resource`. Only for matched runs |
| Yield gain | `candidate_yield / baseline_yield - 1`. Undefined when the baseline yield is zero |

Every planned attempt counts, failures included, and a missing measurement stays null. A partial token total never becomes a complete one. `quality_preserved_on_resolved_pairs` needs every pair resolved and no baseline success turning into a loss. With unresolved outcomes and complete resources, yield ranges from `S_lower / C` to `S_upper / C`.

## Limits and tests

An export holds at most 60 planned attempts, 128 source files, 8 MiB of common source and 64 MiB in total. Control and assessment JSON files are limited to 1 MiB each, and an answer to the producer cap, at most 16 MiB. Hashes bind local artifacts but are not signatures, so an owner can still rewrite the whole archive. Reads and writes reject symlinks.

```sh
python3 -m unittest tests.evals.benchmark.test_review tests.evals.benchmark.test_corpus
```
