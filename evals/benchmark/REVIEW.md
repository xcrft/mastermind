# Offline answer review

`evals/benchmark_review.py` freezes a complete planned batch into a reviewer
packet, validates submitted evidence assessments and reports attempt counts.
It uses Python 3.10+ on POSIX. It never invokes Git, a model, an indexer or the
researched code, and does not require the old runtime binaries to remain
installed. Generic manifest versions 1 and 2 are supported, including relocated
batch directories.

This workflow accepts the structured, source-reviewed **research** key schema
used by the calibration corpus. Decision-task grading and older custom keys
with only task/revision fields are not supported by this reviewer. Their
original preparation/run paths remain available.

## Freeze the review packet

```bash
python3 -m evals.benchmark_review export /absolute/path/to/batch-id \
  --output /absolute/path/to/new-review-export
```

The output parent must exist. The output must be new and outside the batch.
Input directories must be canonical, without symlinks. An export contains:

- `reviewer/packet.json`: public task, the shared rubric, source descriptors
  and a shuffled list of opaque review IDs, one per planned attempt.
- `reviewer/source/`: verified source bytes for the common pinned task.
- `reviewer/answers/`: exact retained answer bytes under opaque names.
- `reviewer/assessment-template.json`: the original, unfilled assessment form.
- `coordinator.json`: private mapping to conditions/repetitions/trial IDs,
  runtime metadata, artifact hashes and attempt states.
- `seal.json`: hashes of the packet, original form and coordinator mapping.
- `reviews/`: immutable imported assessments, created on the first import.

Only give the reviewer the `reviewer/` folder and an editable copy of the form.
Keep the coordinator mapping and run artifacts separate. The tool does not send
anything to another person or service. The reviewer folder has an exact checked
inventory: extra files, missing files, symlinks and changes to the original form
invalidate the export. Do not put completed assessments into that folder.

Conditions, models, tool traces and elapsed times are omitted from generated
reviewer metadata. The answer itself can disclose the tools or condition; it is
preserved verbatim, so perfect blinding is not claimed. Identical answer bytes
from different repetitions retain different review IDs.

## Account for every attempt

The collector checks the producer's three-condition counterbalanced matrix,
unique trial directories and all planned repetitions. It rejects a batch that
omits trial directories still present on disk. A missing manifest or missing
trial directory remains an anonymous slot with no reviewable answer.

| State in the coordinator | Treatment |
|---|---|
| Completed run | Retain the hash-checked answer and count it as completed |
| Failed run with a retained answer | Retain the answer and count the run as failed |
| Setup failure | Count as failed, even when setup failed before source/runtime identity existed |
| Prepared, no result and no lock | Count as not run |
| Prepared, lock but no result | Count as unfinished; the files do not distinguish an active process from a crashed one |
| Missing trial or manifest | Count as missing artifacts |

The collector reads only the answer declared in `result.json`. It does not
recover an answer from a trace or an orphan `answer.md`. It checks the result's
raw manifest hash, canonical task/key/common/condition/request identities and
the exact answer size/hash. It never rechecks executable availability or opens
the native SQLite index.

Source files are checked against their recorded bytes, modes and hashes. If a
failed run changed its source, an intact snapshot from another trial with the
same common identity can supply the original review evidence. The failed
attempt's source remains marked unavailable. If retained answers exist but no
intact common source remains, export fails. A batch of early setup failures can
still produce an accounting packet with no source or answers. At least one
intact manifest/key pair is required to establish the task.

Inputs are rechecked before the seal is published. A run finishing during
collection causes an error; export again to a new directory after the batch
stabilizes. An interrupted export without its seal is not a valid packet.

## Complete and import an assessment

Copy the form outside the export and make the copy writable:

```bash
cp /absolute/path/to/review-export/reviewer/assessment-template.json /absolute/path/to/assessment.json
chmod u+w /absolute/path/to/assessment.json
```

Set `reviewer` to a stable lowercase label such as `alice`, and complete every
entry in `reviews`. Keep the generated export, packet, answer and rubric hashes.
Only retained answers appear in the form; failed or missing attempts stay in
the coordinator's accounting. Partial drafts are not admitted: submit after
every retained answer has been assessed.

For each answer, provide:

- `claims`: exact answer excerpts with `support` set to `supported`,
  `unsupported`, `contradicted` or `unknown`, source `anchors`, a Boolean
  `material_error`, and a rationale. Supported or contradicted claims require
  at least one source anchor. A material error must be unsupported or
  contradicted. Review the whole answer; the validator cannot establish that
  the reviewer selected every material claim.
- `knowns`: one entry for every zero-based `required_knowns` index, with
  `coverage` set to `covered`, `partial` or `missing`, an exact `answer_excerpt`
  or null when missing, and a rationale.
- `unknowns`: one entry for every zero-based `expected_unknowns` index, with
  `handling` set to `appropriate`, `overclaimed` or `omitted`, an exact
  `answer_excerpt` or null when omitted, and a rationale.

These records cover claim support, critical evidence coverage, material false
conclusions and appropriate uncertainty separately. One claim entry could be:

```json
{
  "quote": "The helper leaves all declared relations unverified.",
  "support": "supported",
  "anchors": ["skills/workflow/mastermind-project-history/scripts/document_graph.py:433-450"],
  "material_error": false,
  "rationale": "Every returned relation carries an unverified status."
}
```

Use that excerpt only if it actually occurs in the retained answer. The importer
checks excerpt membership and source anchor ranges, not whether the cited code
supports the judgment. It cannot establish the semantic truth of a review.

```bash
python3 -m evals.benchmark_review import /absolute/path/to/review-export \
  --assessment /absolute/path/to/assessment.json

python3 -m evals.benchmark_review status /absolute/path/to/review-export
```

Each reviewer gets one immutable receipt. Duplicate submissions, stale hashes,
missing answers/rubric entries and invented excerpts are rejected. Another
reviewer can submit a different assessment; both are retained without averaging
or erasing disagreement. Reviewer labels are declarations, not verified human
identities. At most 64 reviewers are admitted. An overlapping import returns
`review_busy`; retry after the other import finishes. The OS releases the
admission lock if the importing process exits.

`status` verifies the exported evidence and imported receipts. It reports planned,
completed, failed, not-run, unfinished, missing-artifact and retained-answer
counts, plus reviewers and distinct reviewed attempts. Multiple reviewers do
not multiply the number of experiments. Original `result.json` files retain
their transport status and `review_pending` state; assessments are separate.

## Limits and interpretation

Reads and writes use directory descriptors and do not follow symlinks. Publication
is exclusive and files are linked into place only after their bytes are written.
Known unlinked staging files from an interrupted assessment publication are not
counted as submitted reviews. No cleanup of other files is performed.

The limits are 60 planned attempts, 128 source files and 8 MiB of common source,
the producer's answer cap up to 16 MiB per answer, 1 MiB per control/assessment
JSON, 64 MiB of exported payload, and 1 GiB of input reads including rechecks.
Directory enumeration stops at 512 entries; paths are bounded to 4096 UTF-8
bytes and 128 components. Limits fail explicitly without truncating evidence.

Hashes detect mismatched artifacts; they are not signatures, runtime provenance
or proof against a host owner coherently rewriting all files. Separate folders
are not an OS sandbox. Published calibration keys and manual reviews do not
establish held-out model quality. Every review status/receipt keeps
`comparison_accepted: false` and `quality_uplift: null`. There is no automatic
semantic grade, accepted quality uplift or live-model validation in this slice.

## Deterministic verification

```bash
python3 -m unittest evals.test_benchmark_review evals.test_benchmark_corpus
```

Tests use disposable Git fixtures to produce real trial artifacts, fixture
adapters/indexers and the actual review CLI with an empty PATH. They exercise
relocated archives with removed runtime executables, complete failure accounting,
partial answers, intact-source recovery, v1/v2 requests, stale/tampered evidence,
symlink boundaries, reviewer-folder contamination, duplicate/admission limits,
exclusive imports and unfinished publications. No model or native build is part
of these tests.
