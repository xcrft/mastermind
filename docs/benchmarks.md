# Indexing benchmark

Measure the production indexer and SQLite store without model calls. Compare
cached processing with a forced full reparse of the same source, then check the
resulting symbols, calls and references for equivalence.

## Reproduce

From the repository root:

```sh
just benchmark-index

# Larger corpus
MMCG_BENCH_FILES=10000 \
MMCG_BENCH_SYMBOLS_PER_FILE=20 \
MMCG_BENCH_CHANGED_FILES=1000 \
  just benchmark-index
```

Retain stdout, stderr and exit status for every run, including failures. Compile
once before repeated measurements; compilation is outside the timed phases.

| Input | Default |
|---|---:|
| `MMCG_BENCH_FILES` | 1,000 Rust files |
| `MMCG_BENCH_SYMBOLS_PER_FILE` | 20 functions per file |
| `MMCG_BENCH_CHANGED_FILES` | 100 files, one new function each |
| Parse batch | 64 files |
| Fixture | Each function after the first calls the previous function in its file |

## Phase and correctness contract

| Phase | Processing | Expected indexed / unchanged |
|---|---|---:|
| `cold` | New store, all source | 1,000 / 0 |
| `warm` | Unchanged source, cache enabled | 0 / 1,000 |
| `warm_full` | Same unchanged source, force full parsing | 1,000 / 0 |
| `incremental` | 100 changed files, cache enabled | 100 / 900 |
| `incremental_full` | Same changed source, force full parsing | 1,000 / 0 |

Schema 2 emits all five phases and `warm_equivalent` / `incremental_equivalent`.
Outside the timers, the benchmark checks function counts and every fixture call
chain against its expected target and line. It hashes sorted symbol attributes,
parent identities, calls and references after both cached and full processing.
Volatile SQLite row IDs are excluded. A mismatch fails the process.

| Included in phase time | Excluded |
|---|---|
| Discovery and change detection | Compilation |
| Production parsing and bounded batches | Fixture generation and edits |
| SQLite writes through the normal writer | Temporary store creation |
| Indexing all files when forced | Correctness queries and hashing |

RSS is sampled every 2 ms; shorter peaks can be missed. Reported RSS includes
memory retained from earlier phases, so it is not isolated per-phase allocation.

## Current retained measurement

[index-20261007.json](../evals/baselines/index-20261007.json) retains three
sequential runs, benchmark/binary digests, platform and a dirty-worktree marker
at starting revision `86b46d4`. All 15 phases reported zero failed files and
all six equivalence checks passed.

| Phase | Run 1 | Run 2 | Run 3 | Median | Median peak RSS |
|---|---:|---:|---:|---:|---:|
| Cold | 1,949 ms | 1,753 ms | 1,722 ms | 1,753 ms | 33.31 MiB |
| Warm cached | 264 ms | 262 ms | 260 ms | 262 ms | 33.38 MiB |
| Warm full | 3,891 ms | 3,747 ms | 4,129 ms | 3,891 ms | 35.92 MiB |
| Incremental cached | 623 ms | 616 ms | 673 ms | 623 ms | 36.00 MiB |
| Incremental full | 3,638 ms | 3,738 ms | 4,481 ms | 3,738 ms | 36.25 MiB |

Compute each paired saving as `1 - cached_ms / full_ms`, then take the median
over the three runs:

| Same-source comparison | Median latency saving | Observed range |
|---|---:|---:|
| Warm cached vs forced full | 93.22% | 93.01–93.70% |
| Incremental cached vs forced full | 83.52% | 82.88–84.98% |

These figures measure avoided reprocessing on this fixture. Cached processing
runs before forced processing in a fixed order, with uncontrolled desktop load.
There is no randomized timing experiment or population confidence interval.
Neither comparison measures LLM tokens, task success, mixed-language projects,
SCIP imports or Lens rendering.

Schema 1 used constant-only functions and three phases. Its
[retained report](../evals/baselines/index-20260927.json) describes that older
corpus; do not interpret the schema 2 timings as a version regression against it.
Other feature measurements and gaps are in the [scorecard](../evals/scorecard.md).
