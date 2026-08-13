# Indexing benchmarks

This page reports the repository's synthetic indexing benchmark. It measures
cold indexing, an unchanged incremental scan, and a 10% incremental update.
It does **not** compare Mastermind with another tool: no shared corpus and
equivalent query contract currently exist for a defensible comparison.

## Results

Measurements were taken on 2026-08-13 at commit
`2ee0807e4aab480d95d8b9199e703f9408ab21d5` with the release profile.

Machine: Apple M3 Pro, 12 CPU cores, 36 GB memory, macOS 26.5.2, Rust 1.97.1.
Each generated Rust file contains 20 public functions. Times are medians; the
parenthesized values are the observed minimum and maximum. Peak RSS is the
median sampled process resident set.

| Corpus | Runs | Cold index | Warm unchanged | 10% changed | Peak RSS, cold / warm / changed |
|---|---:|---:|---:|---:|---:|
| 1,000 files / 20,000 functions | 7 | 310 ms (272–416) | 41 ms (39–48) | 81 ms (75–91) | 19.0 / 19.0 / 19.2 MiB |
| 10,000 files / 200,000 functions | 5 | 3.20 s (2.91–3.54) | 353 ms (351–393) | 867 ms (790–1,107) | 79.5 / 79.6 / 80.0 MiB |

All measured runs indexed the expected number of files and reported zero parse
failures. The changed runs reparsed 100 of 1,000 files and 1,000 of 10,000
files respectively.

## What is timed

The benchmark creates a temporary repository and SQLite database, then records:

1. **Cold:** parse and commit every generated source file.
2. **Warm unchanged:** discover the same files and skip all of them by the
   incremental fingerprint.
3. **10% changed:** update the selected files, then discover all files and
   reparse only the changed set.

Fixture generation and Rust compilation occur outside the reported timings.
Parsing is parallel; SQLite writes use the production single-writer path and a
64-file bounded parse batch. Peak RSS is sampled every 2 ms by the benchmark
process.

## Reproduce

From the repository root:

```bash
just benchmark-index
```

The default corpus is 1,000 files, 20 functions per file, and 100 changed
files. To run the larger profile:

```bash
MMCG_BENCH_FILES=10000 \
MMCG_BENCH_SYMBOLS_PER_FILE=20 \
MMCG_BENCH_CHANGED_FILES=1000 \
  just benchmark-index
```

The command prints schema-v1 JSON containing the corpus parameters, elapsed
milliseconds, peak RSS, and indexed/unchanged/failed file counts. Record every
parameter when publishing a result.

## Limits

- The corpus is synthetic Rust. It does not model mixed languages, generated
  files, large translation units, slow network filesystems, Git submodules, or
  pathological parser inputs.
- Results measure indexing only. They do not include SCIP generation, external
  evidence adaptation, Lens rendering, or model calls.
- Warm runs are unchanged scans, not cached cold parses.
- The numbers describe this machine and commit. They are not a CI threshold or
  a portable latency guarantee.
- A competitor comparison belongs here only after both tools run an equivalent
  corpus, ignore policy, extraction contract, storage mode, and correctness
  check.
