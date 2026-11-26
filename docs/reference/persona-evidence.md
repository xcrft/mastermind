# Persona evidence: what it proves and how to check it

This page is for maintainers who change mining or extraction, or who want to
know how far a persona claim can be trusted. The checks below test control
flow, algorithms and a synthetic detector corpus. All persona data is local
and writable by its owner, so none of it attests who wrote something, and
nothing here shows that a claim about a person is true or that a profile makes
tasks better. Commands for collection and review are in the
[persona reference](persona.md).

Git mining describes observations attributed to an author filter. It cannot
show a person's identity, authorship of every changed line, skill, intent or
stable habits. Bots, shared accounts, coauthors, generated changes, formatters
and model help stay attribution limits.

## What the task loop tests cover

| Local path | Tested behavior |
|---|---|
| Prompt hook | Capture happens before the profile offer, and incoming flags cannot clear recorded influence |
| Response boundary | Complete closed episodes can produce bounded exact-statement drafts, with no provider and no automatic acceptance |
| Repeated boundary | The same episode revision and detector fingerprint create no second analysis |
| Later context | Reprocessing keeps the source binding current and resets changed authorship attestation |
| Native executor | Refreshes committed observations, keeps the stored author selector, selects the executor's own scope and pins the delivered packet |
| Reviewed task completion | Refreshes committed observations after the completion verdict and records the Git snapshot separately |
| Missing evidence or delivery opt-out | No automatic local candidate is published |
| Independent review | Does not get the personal profile |

Isolated CLI tests run these paths with synthetic hooks, Git history and native
clients. They do not show that a client activates the hooks in a real session,
that a human really wrote the text, that a habit lasts, or that results get
better.

## Git mining contract

| Symbol | Input |
|---|---|
| `H` | Bounded ordered commits for one pinned Git revision and author selector |
| `D` | Detector and grammar contract, tooling scopes and the first-party module classifier from that revision |
| `K` | A valid cache produced for the same inputs |
| `S` | Selected commit set |
| `M` | Measurement data, without publication timestamps and filesystem provenance |

For the ordinary deterministic miner, a cache never changes the result:

```
S(H, D, K) = S(H, D, empty)
M(H, D, K) = M(H, D, empty)
```

| Condition | Guaranteed behavior or limit |
|---|---|
| Preconditions | Intact Git objects and cache, successful bounded reads, no concurrent store change |
| Deep model-assisted candidates | Outside this deterministic contract |
| Selection | Deterministic monthly rounds, at most 400 eligible source commits from 2,000 listed non-merge commits |
| Cache | Skips diff reads only for currently selected SHAs and cannot exclude newly selected commits |
| Reuse | Needs a matching detector, grammar, tooling and first-party fingerprint |
| Import measurement | Parses complete committed blobs and counts import syntax that intersects added rows, once per library per commit. Comments and string contents cannot supply imports |
| Import limits | Standard and first-party modules are excluded. A missing, oversized or unparseable blob makes that commit's import coverage incomplete |
| Import coverage | The MCP and Markdown range section report complete and incomplete import reads for supported languages, apart from diff coverage. An omitted range omits its coverage too |
| New local module | Can change how older imports are classified, which invalidates cached measurements |
| Duplicate SHA | The measured copy wins over a metadata-only copy |
| Equal-strength copies disagree on counters or author date | `evidence.context_conflict` is set and the counters are withheld |
| Checkout area aliases | Provenance is merged, at most one vote per alias per SHA |
| Reordering or repeated copies | Do not change reconciled measurements |
| Added repositories | Can change repository counts, historical occurrences, area aliases and publication hashes |
| Future merge | Needs the original records, not only the reconciled output |
| `diff_sampled` | Distinct non-conflicting SHAs with a measured diff, counted apart from listed commits and added lines |
| Unread diff | Test-marker evidence is unknown |
| No test paths or markers in inspected changes | Does not show that tests are absent, unrun or failing |

## Commit support and Wilson score

A Git observation gets a confidence tier from how many commits support it.
Mastermind counts votes per commit and takes the lower bound of the Wilson
score interval, a standard way to say "at least this share of commits agrees"
that stays careful when there are few commits.

| Vote | Definition |
|---|---|
| `E` | Commits with a measurable opportunity |
| `A` | Supporting commits, `0 <= A <= E` |
| Unit | One vote per eligible commit |
| Tied patterns | Eligible, but support neither strict majority |
| Neither signal | Unknown and excluded |
| Preferred side and support | Use the same votes |
| More lines within a commit | Cannot reverse the decision while that commit's majority holds |

The score is the lower Wilson bound with `z = 1.96` and `p = A/E`:

```
L = (p + z²/(2E) - z sqrt(p(1-p)/E + z²/(4E²))) / (1 + z²/E)
```

For `E > 0` and `0 < q < 1`, inverting the score inequality gives:

```
L >= q  iff  A >= E*q + z*sqrt(E*q*(1-q))
```

| Tier or check | Value |
|---|---|
| Minimum support | At least 8 eligible commits |
| `medium` / `high` | `L >= 0.6` / `L >= 0.8` |
| Example at `E=400` | 260 / 336 supporting commits |
| Inverted inequality | Independent algebraic oracle for the square-root formula |
| Regression enumeration | Every integer `A` in `[0,E]` and `E` in `[1,2000]`, at both thresholds |
| Other assertions | Monotonicity and `0 <= L <= p` within floating-point tolerance |
| Proof boundary | Finite executable check plus algebra, not a machine-checked universal proof |

The tiers are not calibrated probabilities of a personal trait. Commits depend
on each other, selection is deterministic and many patterns are examined, so
the usual binomial reading of the interval does not hold. See
[NIST's Wilson interval description](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm).

| MCP `mmcg_profile` field | Contract |
|---|---|
| `schema_version` | `2` |
| `counterpattern` | An observed alternative, not a prohibition |
| `confidence` | Descriptive tier string with evidence notes on its limits |
| Git observations | Cannot accept preferences or create observed habits on their own |

## Replay local history

`evals/persona/replay.py` runs the real binary against frozen local history
under isolated profile homes. It makes no model or network request.

From the repository root, record a baseline and then replay a candidate binary
against the same refs:

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
| Frozen history | All local commit refs and HEAD, unmerged histories included |
| Binary | A private pinned copy with SHA-256 digests |
| Historical classifier | The anchor tree supplies tooling and classification context |
| Author selector | Email is checked against raw Git metadata. Aliases are not merged into people |
| Coauthor trailers | Reported as attribution limits and not assigned the primary author's diff |
| Per-identity checks | Selection and identity purity, equal repeated runs, cold run equal to historical-then-current run, measured diff accounting and no automatic personal claims |
| Temporal coverage | Seed measurements, overlap and whether the classifier could be reused |
| No overlap | `not_exercised`. Comparing two cold runs does not test cache reuse |
| Monthly `authored_nonmerge`, `listed`, `measured` | Descriptive coverage, not inclusion probabilities |
| Source repository | Read-only. Objects are borrowed through a local alternate |
| Missing object | Abort |
| Confirmed non-commit ref | Reported as skipped |
| Output directory | Private author metadata, SQL stores and profile views. Keep it out of source control |
| Revision scope | Local refs at snapshot time. Current remote refs are not checked |

The regression gates cover real Git histories for the 400-commit sample
boundary, module-classifier changes, renamed clones, single-tip and unmerged
corpora, and missing or drifted frozen objects. Trait precision and recall
would still need reviewed labels and held-out tasks.

## Explicit-statement detector

`persona-explicit-v2` is the lexical detector behind `miner collect`,
`miner sync` and local hook extraction. Its bounds in hooks are in
[local extraction](persona.md#local-extraction-and-retention).

| Input or result | Contract |
|---|---|
| Selected input | Complete short human segments that state a preference, a self-report or a correction |
| Kept evidence | Exact normalized quote, record digest, session, line and segment |
| Conditional language | Needs first-person author wording, and detected quoted or reported speech is rejected |
| Example | "When reviewing code, I check which layer owns the state." |
| Correction about future replies | Kept as a correction candidate |
| Task-bound preference | Keeps its limitation |
| Lexical detection | Says nothing about attribution, lasting habits, motives, personality, independent episodes or global scope |
| Detector fingerprint change | Collect again and inspect the kept observations. An upgrade cannot accept a claim |

### Regression corpus

The fixture `mcp/servers/mmcg/tests/fixtures/persona_detector.json` has 58
synthetic Russian, English and mixed cases: 32 relevant signals and 26
negative controls. The implementation author wrote the labels, so this is a
regression set, not an independent accuracy measurement on user history.

| Corpus property | Scope |
|---|---|
| Coverage | Preferences, decision boundaries, conditional behavior, corrections, negations, task-bound requests, project rules, quotations, reported speech, unrelated details and secret-like strings |
| `expected_kind` | Semantic label |
| `known_miss` | Declared detector limits: two implicit statements and one segment over 300 characters |

Run it from the repository root:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --lib --locked \
  synthetic_multilingual_candidate_detection_evaluation -- --nocapture
```

| Output or gate | Meaning |
|---|---|
| V1 and V2 counts | TP, FP, FN, wrong kind, language and category slices and exact missed IDs |
| Supported example | Must keep its kind and the complete condition or negation |
| Negative control | Must stay excluded |
| Missed set | Must equal the declared limits. To change one, inspect its labeled example |
| CLI fixtures | Check attribution, provenance, source drift and review boundaries on their own |

The retained V1 and V2 comparison with source hashes is in
[persona-detector-20261007.json](../../evals/baselines/persona-detector-20261007.json).

### Evaluate source spans

`miner hooks evaluate-local` runs the hook adapter on a labeled corpus without
a model. The adapter uses the same detector, plus the episode's whole-text,
source-revision and coverage checks. Its `explicit_statement` drafts are
unreviewed quotes. From the repository root:

```bash
mastermind miner hooks evaluate-local --input evals/persona/local.json
```

`evals/persona/local.json` holds the 58 detector cases plus two multiline hook
cases, labeled by the implementation author. The adapter's 200-character
behavior limit can drop signals that the 300-character transcript contract
allows, and those drops stay in the recall denominator. So the detector
comparison and this replay have different inputs and denominators. The
retained run is
[persona-local-20261007.json](../../evals/baselines/persona-local-20261007.json).

A real selected-history corpus needs:

| Requirement | What to record |
|---|---|
| Corpus | Explicitly selected, consented sessions and a separate author-labeled sample |
| Split | By session or task |
| Attribution | Coauthored, quoted and generated material labeled separately |
| Extraction | Omissions, source coverage, precision and recall by language and signal kind |
| Final claim quality | Support, contradictions, role and workflow fit, author corrections |

It uses this schema:

```json
{
  "schema": 1,
  "provenance": "selected_history",
  "labeler": "reviewer-name",
  "cases": [{
    "id": "request-1", "session": "session-1", "partition": "held_out",
    "text": "I prefer short code reviews\nonly for trivial changes.",
    "expected_quotes": ["I prefer short code reviews\nonly for trivial changes."]
  }],
  "task_pairs": []
}
```

| Field or limit | Contract |
|---|---|
| `provenance` | `synthetic` or `selected_history` |
| `partition` | `development` or `held_out`. One session cannot be in both |
| `expected_quotes: []` | A negative control |
| File | At most 2 MiB, 128 cases and 128 task pairs |
| `task_pairs` entry | A SHA-256 `task_revision`, a `baseline` trial and a `with_profile` trial |
| Trial | A distinct `session`, `correct` boolean, `iterations` (1-20), `user_corrections` (0-128), `wall_ms` (1-7,200,000) and `profile_digest` (`null` for baseline, SHA-256 with the profile). Repeated sessions and task revisions are rejected |
| Output | Exact-span TP, FP and FN, precision, recall, excluded inputs, failing case IDs and the corpus SHA-256. Input text and expected quotes are not echoed |
| Deltas | Profile minus baseline. With no pairs, the metrics are `null` and `unmeasured`, never zero benefit |

Labels and outcomes are what the reviewer supplied, and the schema does not
authenticate provenance or reviewer independence. Semantic accuracy or task
benefit would need independent real-history labels and controlled paired
trials, and no such result exists yet.
