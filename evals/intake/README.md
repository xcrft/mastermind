# Refiner evaluation

Run the 40-case multilingual corpus through the production refiner parser.
Select an installed processor explicitly; the harness does not choose a provider
or credentials. Ordinary Python tests use disposable processors without model calls.

```sh
python3 -m evals.intake --binary /absolute/path/to/mmcg \
  --processor /absolute/path/to/protocol-processor \
  --output /private/new-report-directory
```

| Setting | Contract |
|---|---|
| Processor | Must implement the native refiner JSON protocol |
| Arguments | Repeat `--processor-arg=VALUE`; do not pass credentials as arguments |
| Corpus | `cases.jsonl`, synthetic intent and active-task scenarios |
| Repetitions | 1–5, all attempts retained |
| Native timeout | 1–20 seconds; processor budget is 0.25 seconds shorter for recorder finalization |
| Output | New private directory, bounded request/response streams, status and digests |
| Publication | `admission: false`; no chat capture or task publication |
| Labels | Hidden from the processor, supplied by the implementation author |
| Unknowns | Independent label review and real-world semantic quality remain unestablished |

Protocol admission and label agreement describe this corpus. They do not show
that a refined prompt improves a task. For that comparison, freeze the original
task and outcome key, generate the refined input separately, record its generation
cost and run raw/refined arms with the same model and execution budget. Review
complete answers against the original key. Native prompt delivery and total
workflow time must also be measured before an end-to-end claim.

Use the [research condition matrix](../benchmark/README.md#conditions-and-corpus)
for static instruction comparisons and the [scorecard](../scorecard.md) for
current evidence. The research skill comparison is a different treatment from
native prompt refinement.
