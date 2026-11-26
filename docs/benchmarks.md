# Indexing benchmark

`just benchmark-index` measures the production indexer and SQLite store with no
model calls. It compares cached processing with a forced full reparse of the
same source, then checks that both produce the same symbols, calls and
references.

## Run it

From the repository root:

```sh
just benchmark-index

# Larger corpus
MMCG_BENCH_FILES=10000 \
MMCG_BENCH_SYMBOLS_PER_FILE=20 \
MMCG_BENCH_CHANGED_FILES=1000 \
  just benchmark-index
```

The report is JSON on stdout. Keep stdout, stderr and the exit status of every
run, failures included. Compile once before repeated runs, because compilation
is not part of the timed phases.

| Input | Default |
|---|---:|
| `MMCG_BENCH_FILES` | 1,000 Rust files |
| `MMCG_BENCH_SYMBOLS_PER_FILE` | 20 functions per file, each calling the previous one in its file |
| `MMCG_BENCH_CHANGED_FILES` | A tenth of the files, each with one new function |
| Parse batch | 64 files, fixed by the indexer |

## What each phase does

| Phase | Processing | Expected indexed / unchanged, with defaults |
|---|---|---:|
| `cold` | New store, all source | 1,000 / 0 |
| `warm` | Unchanged source, cache on | 0 / 1,000 |
| `warm_full` | Same unchanged source, forced full parse | 1,000 / 0 |
| `incremental` | Changed files, cache on | 100 / 900 |
| `incremental_full` | Same changed source, forced full parse | 1,000 / 0 |

Phase time covers discovery, change detection, parsing and SQLite writes
through the normal writer. It leaves out compilation, fixture generation and
edits, store creation and the correctness checks. RSS is sampled every 2 ms,
so shorter peaks can be missed, and it includes memory kept from earlier
phases.

Outside the timers, the benchmark checks function counts and every fixture call
chain against its expected target and line. It hashes sorted symbol attributes,
parent identities, calls and references after cached and full processing,
without the volatile SQLite row IDs, and reports `warm_equivalent` and
`incremental_equivalent`. A mismatch fails the run.

## Read the results

The retained run is
[index-20261007.json](../evals/baselines/index-20261007.json): three sequential
runs at `86b46d4`, with binary digests and the platform. The medians are in the
[scorecard](../evals/scorecard.md). They measure avoided reprocessing on this
one fixture, with cached runs always before forced runs and an uncontrolled
desktop load. They say nothing about tokens, task success, mixed-language
projects, SCIP imports or Lens rendering.

Report schema 2 has all five phases. The older schema 1
[report](../evals/baselines/index-20260927.json) used constant-only functions
and three phases, so its timings are not comparable with schema 2.
