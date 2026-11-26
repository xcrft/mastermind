# Offline answer review

Use this runbook after a benchmark batch or campaign has run. The review CLI freezes the planned batch, imports structured assessments and accounts for every attempt. It makes no model calls and doesn't run Git, an indexer or the researched application. You need Python 3.10+ and a POSIX host. The acceptance and resource gates are in [product success measures](../PRODUCT.md).

Review supports the structured research keys in the [calibration](README.md#calibration-corpus) and [qualification](README.md#qualification-corpus) corpora. Qualification keys always carry acceptance criteria. Custom keys with only task and revision fields, and decision-task grading, are outside this path.

A key with `acceptance_criteria` exports schema 3 assessments. Judge every criterion against its original request excerpt and quote the kept answer when it is met. Any unmet criterion, material error or overclaimed unknown makes the task unsatisfied. Otherwise unresolved criteria keep it unknown. Source coverage stays visible separately. Schema 1 and 2 assessments keep their original full-key meaning and can't be pooled with user-acceptance outcomes.

## Export

```sh
python3 -m evals.benchmark.review export /absolute/path/to/batch-id \
  --output /absolute/path/to/new-review-export
```

The output parent must exist. Use a new directory outside the batch. Inputs must be canonical and contain no symlinks. Relocated archives work without their original runtime executables.

| Output | Purpose |
|---|---|
| `reviewer/packet.json` | Public task, rubric, source descriptions and shuffled opaque attempt IDs |
| `reviewer/source/` | Verified common source snapshot |
| `reviewer/answers/` | Exact kept answers |
| `reviewer/assessment-template.json` | Original blank form |
| `coordinator.json` | Private mapping to conditions, repetitions and execution records |
| `seal.json` | Bound packet, form and coordinator hashes |
| `reviews/` | Immutable imported assessments |

Give the reviewer only `reviewer/` and an editable copy of the form. Keep the coordinator and raw run artifacts separate. Conditions, model names and timings are left out of reviewer metadata, but answer text can still reveal them. The tool never contacts a reviewer or sends them data.

The original reviewer folder has a checked inventory. Extra or missing files, changed originals or symlinks invalidate the packet, so keep completed forms outside it. If a batch changes during export, let it settle and export again to a new directory.

## Account for all attempts

| Attempt state | Accounting |
|---|---|
| Completed with an intact declared answer | Completed and reviewable |
| Failed with an intact declared answer | Failed and reviewable |
| Setup failure | Failed |
| Prepared with no result or lock | Not run |
| Lock present without a result | Unfinished. Whether it is still running or crashed is unknown |
| Missing trial or manifest | Missing artifacts |

Only the answer declared in `result.json` is admitted, and orphan answers or trace fragments are never recovered as completed outputs. Kept results need their intact empty `run.lock`. Bound batches also need `execution.lock`, the plan binding and the predecessor chain. Complete chains are `verified`, intact incomplete prefixes are `partial`, and chains that can't be shown are `not_established`. Older unbound batches stay `unverified_legacy`.

A failed trial can use an intact source snapshot from another trial with the same common identity as review evidence, and it stays marked as failed. Kept answers with no intact common source can't be exported.

## Assess and import

```sh
cp /absolute/path/to/review-export/reviewer/assessment-template.json /absolute/path/to/assessment.json
chmod u+w /absolute/path/to/assessment.json
```

Set a stable lowercase `reviewer` label and complete every kept answer. Leave the generated packet, answer, rubric and export hashes unchanged.

| Field | Required assessment |
|---|---|
| `claims` | Exact answer excerpts, a supported, unsupported, contradicted or unknown status, source anchors, a material-error flag and a rationale |
| `claims[].source_quotes` | Optional literal source excerpts, each with a declared anchor and the pinned file's SHA-256 |
| `knowns` | Every required fact, with an answer excerpt or a missing value |
| `unknowns` | Every expected unknown, with an appropriate, overclaimed or omitted status and an excerpt or a missing value |
| `outcome` | Whether the answer satisfies the original task: `satisfied`, `unsatisfied` or `unknown`, with a rationale |

Supported and contradicted claims need source anchors, and a material error must be unsupported or contradicted. The importer checks that excerpts occur in the answer and that anchors are in range. It checks only the shape of optional source quotes, because the binding audit below checks their bytes and file digests. Neither check judges the reviewer's semantic decisions or whether they picked every material claim.

The form needs an explicit task outcome. `satisfied` must agree with covered required facts, correct unknown handling and supported selected claims without material errors. This consistency check doesn't judge the reviewer's decision. Older submissions without an outcome field stay readable, and their task outcomes stay unknown instead of being inferred from the selected claims.

```sh
python3 -m evals.benchmark.review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/assessment.json
python3 -m evals.benchmark.review status /absolute/path/to/review-export
```

Each reviewer gets one immutable receipt. Partial, duplicate, stale or invented assessments are rejected. A concurrent import returns `review_busy`, so retry after the other import finishes. Up to 64 reviewers are kept separately, and their disagreement is never averaged away. Labels don't prove who the reviewer is.

`status` checks the artifacts and reports attempts, execution-order integrity, reviewers, reviewed answers and descriptive assessments overall and by condition. It keeps task outcomes, material-error flags, required-known coverage, unknown handling and disagreement. Claim rows that reviewers selected are not aligned votes, and more reviewers don't add experiments.

For a corpus-wide campaign, repeat the import in every case directory under the review set. Use the same reviewer label only for the same reviewer. The campaign comparison checks each export against its original batch, task and key.

## Audit literal source bindings

Use this audit to check whether the source excerpts a reviewer declared really occur in the frozen source ranges. It makes no model calls and leaves imported assessments, `support`, acceptance criteria and task outcomes unchanged.

Each claim's `quote` stays an exact excerpt from the kept answer. To check source evidence, add `source_quotes` to the editable assessment before you import it:

```json
"source_quotes": [
  {"anchor": "src/file.py:2-3",
   "source_sha256": "<SHA-256 from reviewer/packet.json source_files for src/file.py>",
   "quote": "<literal excerpt from reviewer/source/src/file.py lines 2-3>"}
]
```

Replace both placeholders with the real values. Each source quote must use a distinct anchor that is already in that claim's `anchors`. A claim can declare at most 16 source quotes of at most 16 KiB of UTF-8 text each. Older assessments without this field stay readable.

You need an intact sealed review export and its imported assessment receipts. An export without receipts can be audited too, and its kept answers stay pending. Use canonical paths without symlinks. The output parent must exist, and the report must be a new file outside the export.

```sh
python3 -m evals.benchmark.review audit-bindings /absolute/path/to/review-export \
  --output /absolute/path/to/new-binding-audit.json
```

| Binding status | Meaning |
|---|---|
| `valid` | The declared digest matches the pinned file, and the exact UTF-8 quote occurs within the anchor's inclusive line range |
| `invalid` | A declared digest, quote or source range fails its check |
| `unverifiable` | An anchor has no explicit source quote, or a claim has no anchors |

Lines are numbered by LF. Quotes are literal substrings, so whitespace, CRLF and Unicode characters are not normalized. A claim is `valid` only when every anchor is valid. Any invalid anchor makes the claim invalid, even when other anchors have no quote. Otherwise missing bindings keep it unverifiable.

| Anchor reason | Meaning |
|---|---|
| `quote_bound` | Digest and literal range checks passed |
| `source_digest_mismatch` | The declared digest differs from the pinned file |
| `source_quote_outside_span` | The quote is absent from the declared line range |
| `source_quote_missing` | This anchor has no explicit source quote |

The JSON report binds the export, packet, coordinator and source revision. It includes every planned attempt state, `answers_without_assessment`, claim and anchor counts by status, and a result for every claim in every imported receipt. `answers_without_assessment` counts kept answers when no assessment was imported, and complete receipts cover all kept answers. Claims that different reviewers selected are separate records, not extra attempts.

Exit 0 means the report was published, even when it contains invalid bindings. Exit 2 means an integrity, schema or publication error.

Changed packet or source files, invalid receipts and unsafe paths fail the command without a report. Restore the original sealed artifacts or create a new export from intact inputs. Fix a declaration mistake in the editable assessment before import. An admitted receipt is immutable, so keep a corrected assessment under a new reviewer label with an explanation and write the next audit to another new path.

A valid binding shows only that the quoted text is literally there. It doesn't show that the source supports the claim, that every material claim was selected, that the source matches the current checkout, that the reviewer is human, or that an arm improves quality or token use.

## Audit citations in producer answers

Use this check for existing Markdown answers. It reads a sealed review export, needs no assessment receipts and makes no model calls. Producers don't have to add snippets or change their answer format.

You need the intact export from `review export`, its pinned source files and an existing output parent. Publish to a new file outside the export:

```sh
python3 -m evals.benchmark.review audit-producer-bindings /absolute/path/to/review-export \
  --output /absolute/path/to/new-producer-binding-audit.json
```

| Input or rule | Contract |
|---|---|
| Citations | `path:line` or `path:first-last` outside fenced code bodies, with an ASCII hyphen (`-`) or an en dash (`–`) between range bounds. Lines are inclusive and numbered by LF. The report keeps the original token and offset, and the checked anchor uses an ASCII hyphen. Em dashes and mixed range delimiters are unsupported and never parsed as a first-line citation |
| Exact paths | Matched against the admitted source inventory first |
| Filename-only paths | Resolved only when exactly one admitted file has that basename. The report keeps the original reference and the resolved path |
| Ambiguous filenames | Stay `unverifiable` with the candidates recorded. No path guessing |
| Unknown or unsafe paths | Reported as invalid references. The checkout is never searched for replacements |
| Fences | Closed backtick or tilde fences with at least three markers and up to three opening spaces. Closing markers must match and be at least as long |
| Quote association | A citation right before its snippet wins. A citation after a snippet is a fallback only when that block is unused and the citation has no following block. Several citations or snippets competing for one association stay unverifiable, and prose in between breaks the association |
| Snippet representations | Two forms are computed: the displayed snippet, with up to the opening fence's Markdown indentation removed per body line, and the raw fenced body with that indentation kept. One final LF or CRLF is removed from each. All other whitespace and Unicode is kept |
| Literal matching | Both forms are checked independently against the same pinned source range. The parser never picks a form by searching the source and never repairs whitespace beyond the fixed framing rule |
| Unsupported code regions | References on lines that start with four or more spaces or a tab are excluded, and such lines also block association across them |
| Bounds | Answer 16 MiB, snippet 16 KiB, at most 4,096 citations and 4,096 fenced blocks per answer |
| Missing snippets | Stay `unverifiable`. An answer without citations is counted explicitly and shows no source coverage |

| Match result | Binding status | Meaning |
|---|---|---|
| Displayed and raw | `valid` | Both forms occur within the pinned line range |
| Displayed only | `valid` | The snippet as Markdown displays it occurs within the range |
| Raw only | `unverifiable` (`producer_quote_raw_only`) | The literal body occurs, but Markdown indentation changes the displayed snippet. Check the source and answer before you treat it as evidence |
| Neither | `invalid` (`source_quote_outside_span`) | Neither form occurs within the range |
| A candidate was not checked | Follows the checked candidate or the input failure | `quote_match_mode` is `null`. A size limit, blank snippet or source failure can't show that a match is absent |

The JSON binds the export, source revision, source inventory and each answer. It records the parser version, original citation spans, path resolution, `quote_match_mode` (`both`, `rendered_only`, `raw_only`, `neither` or `null`) and the `valid`, `invalid` and `unverifiable` counts. `quote` is the displayed snippet. `raw_quote_sha256` and `raw_quote_bytes` identify the raw candidate without repeating its text, and the fence's `raw_body_sha256` includes the original body framing. Source digests come from the pinned inventory, never from producer text. A source snapshot mismatch stays invalid even when the text matches another version of the file. A valid citation shows only a literal snippet within its resolved range, not support for the surrounding statement, complete claim coverage or acceptance.

Exit 0 means the report was published, invalid and unverifiable references included. Exit 2 means input integrity or publication failed. Use a new output path to repeat an audit. If sealed inputs changed, restore the original artifacts or export again from intact trials. Producer audits and parser upgrades never rewrite reviewer receipts, task outcomes or old reports.

The `producer_binding.audit_answer` function also accepts a `source_prefix` from a caller-verified frozen request, for absolute source references. The export CLI has no prefix option. The parser follows only the rules above and is not a full Markdown renderer.

## Assign reference and blind reviewers

Keep the human reference separate from model assessments. For new calibration reviews, use these roles:

| Role | Reviewer label | Responsibility |
|---|---|---|
| Reference | `human` | A real person reviews the frozen sources and answers without seeing condition mappings or model verdicts. These are the reference labels for judge calibration |
| Blind model reviewer | `gpt-6-1-sol-blind` | GPT 6.1 Sol, requested model `gpt-6.1-sol`, reviews the same inputs in a fresh session without the reference labels or earlier judge outputs |
| Historical model reference | The existing label, such as `claude-reference` | Kept model evidence. Never relabel it as human evidence |

You need an intact sealed export. Human-based calibration also needs a real human reference assessment, and the two reviewers can work independently. A reviewer who has already seen aggregate results is not blind, so their assessment can't serve as the blind review. Until a person submits the reference assessment, human calibration stays pending. Seeded-defect agreement can still be reported with its original reference identity.

Give each reviewer only the export's `reviewer/` directory and the instructions for their role. Keep `coordinator.json`, raw trials, condition mappings, imported receipts, the seed `key.json`, earlier results and any discussion of them outside the review session. Use [blind-review-gpt61.md](blind-review-gpt61.md) for the GPT reviewer. Record the requested model, runtime, instruction hash, visible packet hash and any earlier exposure with the kept assessment, because a label alone proves neither the model nor blindness. Answer wording can still reveal a condition.

Each reviewer fills every answer in `assessment-template.json` with their own reviewer label. Import the two assessments separately, after both first assessments are kept, without rewriting the original export:

```sh
python3 -m evals.benchmark.review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/human-assessment.json
python3 -m evals.benchmark.review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/gpt-assessment.json
```

The automated `judge` command below runs a Claude Code transport with an Anthropic-compatible endpoint. It doesn't run Codex, so never pass a Codex CLI as its `cli`. For GPT, keep the fresh session's assessment and import it through the same reviewer contract. An invalid import can be fixed under that reviewer's label before admission. A changed admitted assessment needs a new reviewer label and a kept explanation. Never overwrite the first verdict.

## Assess with a model judge

A model judge fills the same assessment form and goes through the same import checks. Its labels are model declarations, so calibrate it against blind human review before you use it as a primary reviewer.

You need a review export whose key has acceptance criteria, a pinned Claude Code CLI, `ANTHROPIC_API_KEY` and a frozen instruction. The CLI runs in `--bare` mode and never reads OAuth or the keychain. [judge-instruction.md](judge-instruction.md) is a starting instruction, not a calibrated one.

```json
{"cli": {"path": "/absolute/path/to/claude", "sha256": "<sha256>", "version": "<version>", "origin": "npm"},
 "model": "<exact model ID>", "instruction": "/absolute/path/to/judge-instruction.md",
 "reviewer": "judge", "attempts": 3, "timeout_seconds": 1800}
```

```sh
python3 -m evals.benchmark.judge /absolute/path/to/review-export \
  --config /absolute/path/to/judge.json --output /absolute/path/to/new-judge-run
```

An optional `"base_url"` sends the CLI's requests and `ANTHROPIC_API_KEY` to another Anthropic-compatible https endpoint, for example GLM through `https://api.z.ai/api/anthropic` with `"model": "glm-5.3"`. That provider then gets the task, key, sources and answers. Such endpoints can echo the requested model name, so the served model stays unverified.

| Contract | Behavior |
|---|---|
| Visible inputs | One answer per call: the verified packet and sources, that answer and a one-review template, in a fresh temporary directory. No other answers, coordinator, raw trials or earlier attempts |
| Runtime | `--bare`, only the `Read` tool and no allow rule, so `dontAsk` denies reads outside the copy. No MCP servers, no user, project or local settings, and a private `HOME` |
| Observed runtime | The stream's init event must report `Read` only, no MCP servers, the copy as cwd, `dontAsk`, the pinned CLI version and the requested model. Plugins and permission denials are recorded |
| Attempts | The same prompt each time, at most 3 per answer. An answer stops at its first review that passes the import checks for that answer |
| Quote repair | A quote or excerpt that matches the answer only after mapping curly quotes, dashes and non-breaking spaces to ASCII is replaced by the unique exact answer span before validation, and `quotes_repaired` records the count. Ambiguous or paraphrased passages stay rejected |
| Source quotes | The starting instruction asks for a literal source excerpt and the packet's file digest for every cited anchor. Source quotes are never rebuilt or repaired. Older assessments without them stay readable and audit as `unverifiable` |
| Rejected attempt | Unclean exit, timeout, output cap, runtime mismatch, error result, another model, no JSON, not exactly one review, or a rejected review. Kept with its error code |
| Output | `judge.json`, `answer-K-attempt-N.{stdout,stderr,json}`, `assessment.json` when every answer is reviewed, and `judge-run.json` when the assessment loop completes or fails, with `error` when it stopped early. After import, `binding-audit.json` records the literal source checks, and `judge-run.json` binds its path and SHA-256 in `binding_audit`. Exit 0 only after import and audit publication |
| Answer without a valid review | The run stops, no receipt is written and the export's outcomes stay unresolved |

Another reviewer can import during a judge run, because the importer checks the export again under its lock. The CLI's permission layer is not an OS sandbox, and built-in plugins still load under `--bare`. Judge output quotes answers and keys, so keep it with the review export. If the import fails after a valid attempt, import the kept `assessment.json` with `review import`. The importer checks quotes, anchors and outcome consistency, not semantic truth.

After import, the runner audits every receipt that is in the export at that moment. Invalid or missing literal bindings stay diagnostic and don't change reviewer support, acceptance or outcome. If audit publication fails, the command exits 2 and `judge-run.json.error` records the failure, while `receipt` still names the admitted immutable assessment. Keep that receipt and run `review audit-bindings` again to a new output path. Don't rerun the model or replace the receipt. The original run summary stays unchanged, so keep the recovery report next to it.

## Calibrate the judge with seeded defects

This measures whether a judge rejects answers with a known defect and accepts answers that a reference reviewer judged satisfied. Seeded defects give ground truth by construction. They don't replace calibration against blind human review, and a judge may catch artificial edits more easily than natural errors.

You need a review export whose key has acceptance criteria, a reference reviewer's receipt in that export, and mutant answers written from originals that the reference labeled satisfied. Each mutant carries exactly one defect:

| Defect | Target | `passage` | Anchors | Ground truth |
|---|---|---|---|---|
| `criterion_removed` | `criterion_id` | Removed text: in the original, absent from the mutant | none | criterion unmet |
| `fabricated_citation` | `criterion_id` | Text with wrong citations: in the mutant, not the original | correct locations | criterion unmet |
| `listed_false_claim` | `false_claim_index` | The false sentence: in the mutant, not the original | contradicting source | material error |
| `novel_false_claim` | none (`{}`) | The false sentence, not equivalent to a listed false claim | contradicting source | material error |
| `overclaimed_unknown` | `unknown_index` | The unverifiable statement | none | unknown overclaimed |

```json
{"kind": "mastermind-judge-calibration-mutations", "schema_version": 1, "reference": "reference",
 "controls": ["review-<original id>"],
 "mutants": [{"source_review_id": "review-<original id>", "defect": "listed_false_claim",
              "target": {"false_claim_index": 0}, "answer": "mutants/m01.md",
              "passage": "<exact sentence>", "anchors": ["src/file.py:12"], "rationale": "<why it is unsatisfied>"}]}
```

```sh
python3 -m evals.benchmark.judge_calibration build /absolute/path/to/review-export \
  --mutations /absolute/path/to/mutations.json --output /absolute/path/to/new-set
python3 -m evals.benchmark.judge_calibration run /absolute/path/to/new-set \
  --config /absolute/path/to/judge.json --output /absolute/path/to/new-run
python3 -m evals.benchmark.judge_calibration score /absolute/path/to/new-run [...] \
  --output /absolute/path/to/new-report.json
```

`answer` paths are relative to the mutations file. `build` refuses a mutant equal to its original, a passage that breaks its rule, a target outside the key, missing or out-of-range anchors, a control or original that the reference didn't label satisfied, and duplicate answers.

| Contract | Behavior |
|---|---|
| Set | `reviewer/` holds the source export's task, key and sources with new answer IDs in shuffled order, in the export packet format. `key.json` maps answers to defects and `seal.json` binds packet, template and key. Neither is copied to the judge |
| Run | The judge contract above, one answer per call. Every answer is attempted, and an unresolved answer doesn't stop the rest. Output: `judge.json`, attempt files and `calibration-run.json`. No receipt is imported. The judge's reviewer label must differ from the reference's |
| Score | Checks each set and every accepted review again. Refuses a changed set, an interrupted run (`error` set or not every answer attempted), runs with different judge configurations (CLI, model, endpoint, instruction, prompt, tools, attempts, timeout), the judge scored against its own reference, and any answer text scored twice, a rebuilt set included |
| `sensitivity` | Controls judged `satisfied`, over resolved answers and in the worst case (unresolved counted as errors), with one-sided 95% Clopper-Pearson lower bounds that treat answers as independent |
| `specificity` | Mutants judged `unsatisfied`, in the same forms. An `unknown` verdict is a miss. `by_defect` splits it by defect |
| `located` | Mutants where the judge flagged the broken element: the target criterion unmet, the target unknown overclaimed, or a material error on a claim whose every occurrence overlaps the passage. Same forms as `specificity` |
| `youden_j` | `sensitivity + specificity - 1`, in resolved and worst-case forms |
| `gwet_ac1` | Three-category agreement over resolved answers, so `unknown` agrees with neither ground truth. `case_bootstrap_lower_95` is the 5% quantile of a case-level bootstrap and is `null` below `minimum_cases_for_bound` (5) cases. Sets from one export count as one case |

A judge that rejects a mutant for an unrelated reason counts in `specificity` but not in `located`. Keep sets with their export, because the answers quote the key and sources.

## Compare paired outcomes

```sh
python3 -m evals.benchmark.review compare /absolute/path/to/review-export \
  --baseline raw --candidate refined
```

Use IDs from the planned matrix, for example `source` and `portable` for a default batch. Every repetition gives a pair under the same original task and key, and each reviewer gets a separate result.

| Evidence | Task-success value |
|---|---|
| Completed, verified runtime and source, and declared `satisfied` | 1 |
| Declared `unsatisfied`, a failed attempt or a recorded runtime-contract violation | 0 |
| Unreviewed, unknown, not run, unfinished, missing evidence or an unverified runtime | [0, 1] |

`success_delta` is candidate minus baseline across all planned pairs. Its lower and upper values include unresolved outcomes, so they are uncertainty bounds, not confidence intervals. `point` exists only when the bounds are equal. Wins, ties and losses describe resolved pairs. Repetitions of one task say nothing about unseen tasks or a causal product-quality change.

The coordinator also keeps setup and run time, four token counters, turns and cost per attempt. `resources` reports observed totals and measured and unknown trial counts per condition, failures included. Missing measurements stay null, and a partial total is not a complete cost. This scope covers trial preparation and adapter attempts. It leaves out external instruction generation and native client delivery. Reviewer files contain no resource metadata.

`efficiency` counts successful outcomes and resources across every planned attempt, failures included. Each reviewer gets a separate estimate. Campaign totals combine that reviewer's cases, and missing reviews keep the outcome bounds open.

| Metric | Definition and boundary |
|---|---|
| Context tokens | Uncached input + cache read + cache write |
| Total tokens | Context + output |
| `run_seconds` | Adapter invocation time |
| `trial_total_seconds` | Trial setup + invocation. Leaves out campaign coordination, refinement generation and profile mining |
| Resource per successful outcome | Complete resource total / known successful outcomes. Undefined with zero successes |
| Useful outcomes per resource (yield) | Known successes / complete positive resource total. Zero when known successes are zero |
| Resource savings | `1 - candidate_resource / baseline_resource`. Undefined when the baseline is zero or missing. Don't compute it for runs that are not matched |
| Resource per success savings | `1 - candidate_per_success / baseline_per_success` |
| Useful outcome yield gain | `candidate_yield / baseline_yield - 1`. Undefined when the baseline yield is zero |

Point estimates need resolved outcomes and complete measurements, and a partial token total never becomes a complete one. `quality_preserved_on_resolved_pairs` needs every pair resolved and no baseline success turning into a loss. Zero successes in both arms can meet that flag and still give no useful-work benefit. These are descriptive results on the declared corpus, not population confidence intervals or a causal uplift.

When resource totals are complete, unresolved outcomes still give fixed yield bounds from `S_lower / C` to `S_upper / C`. For a baseline yield that is surely positive, the relative gain ranges from `candidate_lower / baseline_upper - 1` to `candidate_upper / baseline_lower - 1`. A baseline that could be zero, a zero resource total or missing resource measurements leave the relative range unknown. These ranges describe possible review outcomes. They are not confidence intervals and say nothing about reviewer independence.

Exports from older coordinator versions stay readable with unknown resource measurements and runtime checks, and importing them doesn't upgrade them to stronger evidence.

## Evidence boundary

Hashes bind local artifacts. They are not signatures and don't protect against an owner who rewrites the whole archive. Semantic assessments stay declarations. Original `result.json` files keep their transport and quality status, and kept answers stay `review_pending` in those records. Results keep `comparison_accepted: false` and `quality_uplift: null`. A published calibration corpus with manual review is not held-out quality.

Reads and writes reject symlinks. Publication checks the final file identity and bytes, and an interrupted unsealed export is invalid.

| Resource | Limit |
|---|---:|
| Planned attempts | 60 |
| Source files | 128 |
| Common source | 8 MiB |
| Control or assessment JSON | 1 MiB each |
| Exported payload | 64 MiB |
| Input reads including rechecks | 1 GiB |
| Answer | The producer cap, at most 16 MiB |

```sh
python3 -m unittest tests.evals.benchmark.test_review tests.evals.benchmark.test_corpus
```

These tests cover accounting, relocation, source recovery, stale evidence, review admission and publication, with disposable fixtures and no model calls.
