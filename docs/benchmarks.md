# Indexing benchmark

Production indexer and SQLite store, synthetic Rust corpus, no model calls.

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

The command prints JSON with inputs, timings, sampled peak RSS and file counts.
Keep every run, including failures.

## Recorded result

| Parameter | Recorded value |
|---|---|
| Date | 2026-09-27 |
| Implementation revision | `101bed7bdb9912abaf9f6c612f02f252d459038e` |
| Build | Optimized Cargo bench, Rust 1.97.1 |
| Machine | Apple M3 Pro, 12 physical cores, 36 GiB RAM |
| OS | macOS 26.5.2 arm64 |
| Corpus | 1,000 files × 20 functions, 100 changed files |
| Repetitions | 3 sequential runs × 3 phases |
| Parse batch | 64 files |
| Raw observations | [index-20260927.json](../evals/baselines/index-20260927.json) |

| Phase | Indexed / skipped | Run 1 | Run 2 | Run 3 | Median | Min–max | Median peak RSS |
|---|---:|---:|---:|---:|---:|---:|---:|
| Cold | 1,000 / 0 | 1,837 ms | 1,320 ms | 1,319 ms | **1,320 ms** | 1,319–1,837 ms | 28.2 MiB |
| Warm unchanged | 0 / 1,000 | 341 ms | 233 ms | 230 ms | **233 ms** | 230–341 ms | 28.3 MiB |
| Incremental | 100 / 900 | 753 ms | 577 ms | 550 ms | **577 ms** | 550–753 ms | 30.1 MiB |

All **9/9 phases** reported the expected file counts and **0 failed files**.
These are measurements of the recorded implementation, not release thresholds.

## Timing contract

```mermaid
flowchart LR
    F[Generate corpus] --> C[Time cold index]
    C --> W[Time unchanged scan]
    W --> E[Edit 100 files]
    E --> I[Time incremental index]
```

| Included in phase time | Excluded from phase time |
|---|---|
| File discovery and change detection | Compilation |
| Production parsing and bounded batches | Fixture generation and edits |
| SQLite writes through the normal single writer | Temporary store creation |

RSS is sampled every 2 ms. Shorter peaks can be missed.

## Interpretation

| Question | What this run establishes |
|---|---|
| Local indexing cost | Cold, unchanged and changed-file latency and memory on the stated machine |
| Variance | Observed range over 3 runs with uncontrolled background desktop load |
| Regression or improvement | Unmeasured, no matched older-revision run |
| Model quality, tokens or cost | Unmeasured, no inference calls |
| Mixed languages, generated files, remote storage | Outside this corpus |
| SCIP, imported analysis, Lens rendering | Outside the measured phases |

Compare tools only with the same corpus, ignore policy, extraction contract,
storage mode and correctness checks. Other measurement directions are in the
[evaluation scorecard](../evals/scorecard.md).
