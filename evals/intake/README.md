# Refiner evaluation

This runs the 40 synthetic multilingual cases in `cases.jsonl` through the production refiner parser and records how the processor's output compares with the author's labels. You pick the processor, because the harness never chooses a provider or credentials. The ordinary Python tests use disposable processors and make no model calls. From the repository root:

```sh
python3 -m evals.intake --binary /absolute/path/to/mmcg \
  --processor /absolute/path/to/protocol-processor \
  --output /private/new-report-directory
```

| Option | Contract |
|---|---|
| `--processor` | Must speak the native refiner JSON protocol. It never sees the expected labels |
| `--processor-arg=VALUE` | Repeat for each argument. Never pass credentials this way |
| `--processor-source` | Extra code or config files whose bytes the report pins |
| `--case`, `--cases`, `--repetitions` | One case ID, another corpus file, and 1 to 5 repetitions with every attempt kept |
| `--timeout` | 1 to 20 seconds. The processor gets 0.25 seconds less so the recorder can finish |
| `--output` | A new private directory with bounded request and response streams, status and digests |

The report keeps `admission: false` and publishes no task or chat capture. Label agreement describes this corpus with labels the implementation author wrote, so it says nothing about whether a refined prompt improves a task. For that, run raw and refined arms against the original task key in the [research benchmark](../benchmark/README.md#choose-the-conditions).
