# Repo-map prototype

Build a bounded navigation header from frozen indexed definitions and the original
task question. This is an offline Rust experiment; it does not register a new MCP
tool or change production defaults. It does not use PageRank.

The owning implementation is `mcp/servers/mmcg/src/repo_map.rs`. This experimental
stdin wrapper compiles that module directly; native `mmcg repo-map` exports a
bounded indexed candidate set and calls the same builder. Retained pilot selectors
remain frozen copies of their respective versions.

The `focused_shortlist_v3` selector ranks definition names and file basenames
using task-term overlap, file-level inverse frequency, first-sentence emphasis and
co-occurring terms. It excludes declared test functions, module rows and impl
headers. Signatures are used only to identify declared tests. It groups locations
by file, retaining at most three files and three definitions per file. Files below
half the best file score and definitions below one third of their file's best
score are omitted. Spare budget is left unused. These are navigation heuristics,
not guarantees that the owning implementation was found.

## Build and check

Requires Rust/Cargo and the locked dependencies:

```sh
cargo test --locked --manifest-path evals/experiments/repo-map/Cargo.toml
cargo build --locked --manifest-path evals/experiments/repo-map/Cargo.toml
```

Pass one JSON object on stdin to
`evals/experiments/repo-map/target/debug/mastermind-repo-map-prototype`.

| Input | Contract |
|---|---|
| `question` | Original task question, at most 16,384 UTF-8 bytes; no rubric or answer key |
| `source_sha256` | SHA-256 binding supplied and verified by the caller |
| `budget_bytes` | Complete navigation payload budget, 512–16,384 UTF-8 bytes |
| `symbols` | At most 20,000 definitions with `file`, `name`, `kind`, `line`, nullable `signature` |

The caller must validate the source projection, index identity, source allowlist and
freshness before exporting candidates. This process selects metadata; it does not
verify an arbitrary input digest or open source files itself. Input JSON is capped
at 16 MiB. Invalid bounds or source locations exit unsuccessfully.

The JSON output contains the complete `payload`, its UTF-8 byte count, selected and
candidate and query-matching candidate counts, truncation and algorithm identity.
`truncated` means some indexed definitions were omitted, including unrelated rows.
Stable ordering does not depend on input row order. A byte budget
is not an exact model token count; charge actual total model usage in an experiment.

## Pilot contract

The original pilot retains `query_terms_v2`, its inputs and attempts under
`.mastermind/research/repo-map-pilot-20261012/`. Preserve its frozen selector and
results when changing this prototype.

The follow-up retains inputs and attempts under
`.mastermind/research/repo-map-focused-20261012/`. It reuses the verified 20-file
source projection and index without modifying them. It compares four fresh task
pairs: two development questions and two new holdout questions, one attempt per
condition, with condition order alternating between tasks. Both conditions use
the same tools, base instructions, source, requested `gpt-6.1-sol/high` and existing
Codex subscription. The candidate appends the generated shortlist and direct-read
instruction to the first prompt; the baseline receives no map. This tests the
bundle rather than isolating ranking from instruction effects.

The payload budget is 1,600 UTF-8 bytes, including instructions. Both arms have a
600-second deadline and no model turn/output-token cap. At most one retry is
allowed for a capacity/HTTP 5xx failure without completed tool calls or an answer;
retain the failed attempt and mark unreported usage as unknown. Never retry an
unfavorable valid answer or assessment. An unfinished productive run remains a
failure in the denominator.

All source reads remain available. The map is partial navigation metadata;
conclusions require original source evidence. Resource metrics include the map and
any subsequent exploration. Client `turns` are not interpreted as model rounds.
Fresh condition-hidden source reviews use GPT 6.1 Sol. Human reference review is
pending, so the pilot cannot qualify a product benefit. Inspect development and
holdout measurements separately; two holdout questions do not establish general
savings.

The retained follow-up report is `evals/baselines/repo-map-focused-20261012.md`.
This candidate remains experimental: it did not reduce total tokens on the
development questions, and one map answer contradicted the source at its cited
index-opening boundary. New-question gains cannot qualify the candidate while
human reference labels and complete failed-attempt token accounting are missing.

The integrated command's full eight-question development run is retained in
`evals/baselines/repo-map-all-transport-v2-20261012.md`. It preserves each question's
original source binding and includes native-tool readiness checks, fresh paired
answers, separate blind model assessments and an unanswered human review packet.
The earlier failed transport revision remains available for recovery accounting.

Retain every setup and model failure. Freeze a new input revision before changing
ranking or transport; preserve earlier attempts and do not resample a valid answer
because its outcome is unfavorable.
