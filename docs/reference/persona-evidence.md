# Persona evidence: what it proves and how to check it

This page is for maintainers who change mining or extraction. All persona data
is local and writable by its owner, so none of it attests who wrote something.
The checks below test control flow, the confidence formula and a synthetic
detector corpus, and none shows that a claim about a person is true or that a
profile makes tasks better. Git observations are attributed to an author filter,
so bots, shared accounts, coauthors, formatters and model help stay attribution
limits. Collection and review commands are in the [persona reference](persona.md).

## Git measurement fields

A cache never changes the selected commits or the measurements, as long as Git
objects and the cache are intact and no other process changes the store.

| Field or condition | Meaning |
|---|---|
| `diff_sampled` | Distinct SHAs with a measured diff, counted apart from listed commits |
| `evidence.context_conflict` | Equal-strength copies of one SHA disagree on counters or author date, so the counters are withheld |
| Duplicate SHA | The measured copy wins over a metadata-only copy |
| Import coverage | Complete and incomplete import reads, reported apart from diff coverage. A missing or unparseable blob makes that commit's coverage incomplete |
| New local module | Can reclassify older imports and invalidates cached measurements |
| Unread diff | Test-marker evidence is unknown |
| No test paths in the inspected changes | Does not show that tests are absent or failing |

## Commit support and Wilson score

A Git observation gets a confidence tier from the commits that support it. Each
eligible commit votes once. `E` is the number of commits with a measurable
opportunity and `A` the supporting ones. Tied commits are eligible but support
neither side, and commits with neither signal are excluded.

The score is the lower bound of the Wilson interval with `z = 1.96` and
`p = A/E`:

```
L = (p + z²/(2E) - z sqrt(p(1-p)/E + z²/(4E²))) / (1 + z²/E)
```

| Tier or check | Value |
|---|---|
| Minimum support | 8 eligible commits |
| `medium` / `high` | `L >= 0.6` / `L >= 0.8` |
| Example at `E=400` | 260 / 336 supporting commits |
| Regression check | Every integer `A` in `[0,E]` for `E` in `[1,2000]` at both thresholds, against the inverted inequality `A >= E*q + z*sqrt(E*q*(1-q))` |

The tiers are not calibrated probabilities. Commits depend on each other,
selection is deterministic and many patterns are examined, so the usual binomial
reading does not hold. See
[NIST's Wilson interval description](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm).

In `mmcg_profile` (`schema_version: 2`), `confidence` is a descriptive tier with
notes on its limits, and `counterpattern` is an observed alternative, not a
prohibition. Git observations cannot accept preferences or create habits.

## Replay local history

`evals/persona/replay.py` runs the real binary against frozen local history
under isolated profile homes, with no model or network request. From the
repository root, record a baseline and replay a candidate against the same refs:

```bash
python3 -m evals.persona \
  --repo /path/to/local/repo --anchor origin/main \
  --binary mcp/servers/mmcg/target/debug/mmcg \
  --output .mastermind/research/persona-replay/baseline

python3 -m evals.persona \
  --repo /path/to/local/repo \
  --binary mcp/servers/mmcg/target/debug/mmcg \
  --manifest .mastermind/research/persona-replay/baseline/manifest.json \
  --output .mastermind/research/persona-replay/candidate
```

| Replay input or result | Meaning |
|---|---|
| Frozen history | All local commit refs and HEAD at snapshot time. Remote refs are not checked |
| Binary | A private pinned copy with SHA-256 digests |
| Author selector | Email checked against raw Git metadata. Aliases are not merged |
| Per-identity checks | Selection purity, equal repeated runs, cold run equal to cached run, diff accounting and no automatic personal claims |
| No overlap between runs | `not_exercised`, because two cold runs do not test cache reuse |
| Monthly `authored_nonmerge`, `listed`, `measured` | Descriptive coverage |
| Missing object | Abort |
| Source repository | Read-only, objects borrowed through a local alternate |
| Output directory | Private author metadata and stores. Keep it out of source control |

## Explicit-statement detector

`persona-explicit-v2` is the lexical detector behind `miner collect`,
`miner sync` and [local hook extraction](persona.md#local-extraction-and-retention).
It selects complete short human segments that state a preference, a
self-report or a correction, such as "When reviewing code, I check which layer
owns the state." It keeps the exact quote, record digest, session, line and
segment. Conditional language needs first-person wording, and quoted or reported
speech is rejected. Detection says nothing about attribution, lasting habits,
motives or global scope. After a detector fingerprint change, collect again and
inspect the kept observations.

### Regression corpus

`mcp/servers/mmcg/tests/fixtures/persona_detector.json` has 58 synthetic
Russian, English and mixed cases: 32 relevant signals and 26 negative controls.
The implementation author wrote the labels, so this is a regression set, not an
accuracy measurement. `known_miss` declares the detector limits: two implicit
statements and one segment over 300 characters. Run it from the repository root:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --lib --locked \
  synthetic_multilingual_candidate_detection_evaluation -- --nocapture
```

The test prints TP, FP, FN, wrong-kind counts and missed IDs for V1 and V2. A
supported example must keep its kind and its full condition or negation, a
negative control must stay excluded, and the missed set must equal the declared
limits. The retained comparison is
[persona-detector-20261007.json](../../evals/baselines/persona-detector-20261007.json).

The context packet fixture checks roles, budgets, access denial and separate
source freshness through a local MCP server:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --test persona_transcripts_cli \
  --locked context_composition -- --nocapture
```

### Evaluate source spans

`miner hooks evaluate-local` runs the hook adapter on a labeled corpus without a
model. It uses the same detector plus the episode's whole-text, revision and
coverage checks. From the repository root:

```bash
mastermind miner hooks evaluate-local --input evals/persona/local.json
```

`evals/persona/local.json` holds the 58 detector cases plus two multiline hook
cases. The adapter's 200-character behavior limit can drop signals that the
300-character transcript limit allows, and those drops stay in the recall
denominator. The retained run is
[persona-local-20261007.json](../../evals/baselines/persona-local-20261007.json).

The input schema:

```json
{"schema": 1, "provenance": "selected_history", "labeler": "reviewer-name", "task_pairs": [],
 "cases": [{"id": "request-1", "session": "session-1", "partition": "held_out",
            "text": "I prefer short code reviews\nonly for trivial changes.",
            "expected_quotes": ["I prefer short code reviews\nonly for trivial changes."]}]}
```

| Field or limit | Contract |
|---|---|
| `provenance` | `synthetic` or `selected_history` |
| `partition` | `development` or `held_out`. One session cannot be in both |
| `expected_quotes: []` | A negative control |
| File | At most 2 MiB, 128 cases and 128 task pairs |
| `task_pairs` entry | A SHA-256 `task_revision`, a `baseline` trial and a `with_profile` trial |
| Trial | A distinct `session`, `correct`, `iterations` (1-20), `user_corrections` (0-128), `wall_ms` (1-7,200,000) and `profile_digest` (`null` for baseline) |
| Output | Exact-span TP, FP and FN, precision, recall, failing case IDs and the corpus SHA-256. Input text is not echoed |
| Deltas | Profile minus baseline. With no pairs, metrics are `null` and `unmeasured` |

The schema does not authenticate provenance or reviewer independence. A real
measurement of semantic accuracy or task benefit would need consented
real-history sessions split by session, independent labels and controlled paired
trials. No such result exists yet.
