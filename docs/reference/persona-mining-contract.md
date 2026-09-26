# Persona mining: evidence and reproducibility

Git mining describes observations attributed to an author filter. It cannot
establish a person's identity, authorship of every changed line, proficiency,
intent, or stable habits. Bots, shared accounts, coauthors, generated changes,
formatters and model assistance remain attribution limits.

## Algorithmic contract

Let `H` be the bounded, ordered commit listing for one pinned Git revision and
author selector. Let `D` contain the detector version, tooling scopes and the
first-party module classifier derived from that revision. Let `K` be a valid
cache produced for the same inputs. The ordinary deterministic miner satisfies:

```
S(H, D, K) = S(H, D, empty)
M(H, D, K) = M(H, D, empty)
```

Here `S` is the selected commit set and `M` is the resulting measurement data,
excluding publication timestamps and filesystem provenance. Deep model-assisted
candidate generation is outside this contract. The assumptions include intact
Git objects and cache records, successful bounded reads, and no concurrent store
mutation.

Selection traverses the current corpus in deterministic monthly rounds, up to
400 eligible source commits from at most 2,000 listed non-merge commits. Cached
entries only avoid diff reads for currently selected SHAs. Thus a full cache
cannot prevent newly selected commits from being measured. Reuse requires a
matching detector/tooling/first-party fingerprint. A newly introduced local
module can change historical import classification, so that change invalidates
cached measurements.

The global store groups raw copies by SHA. A measured copy takes precedence over
a metadata-only copy. Equal-strength copies must agree on measurement counters
and author date; otherwise the observation is marked `evidence.context_conflict`
and its counters are withheld. Checkout area aliases are provenance and are
unioned with at most one vote per alias per SHA. This construction is invariant
to input order and repeated copies. It does not promise invariance of repository
counts, historical commit occurrences, new area aliases or publication hashes
when repositories are added. Reconciled outputs are not a substitute for the
original records in a future merge.

The published `diff_sampled` counts distinct, non-conflicting SHAs with an
actually measured diff. Listed commits and added lines are separate counts.
An unread diff gives unknown test-marker evidence. Absence of test paths and
added test markers in inspected changes cannot establish whether tests exist,
were run or passed.

## Commit support and Wilson score

For each predicate, let `E` be the commits with a measurable opportunity and `A`
the commits supporting it. Each eligible commit contributes one vote, with
`0 <= A <= E`. A tie between two observed patterns is eligible but supports
neither strict majority. A commit without either signal is unknown and excluded.
The preferred side and its support use the same commit votes. Multiplying lines
within a commit while preserving its majority cannot reverse that decision.

The score is the lower Wilson bound with `z = 1.96`, `p = A/E`:

```
L = (p + z²/(2E) - z sqrt(p(1-p)/E + z²/(4E²))) / (1 + z²/E)
```

For `E > 0`, `0 < q < 1`, inversion of the score inequality gives:

```
L >= q  iff  A >= E*q + z*sqrt(E*q*(1-q))
```

This is an independent algebraic oracle for the implementation's square-root
formula. The Rust test checks all integer `A` in `[0,E]`, `E` in `[1,2000]`, at
both thresholds: 4,006,000 comparisons, plus monotonicity and `0 <= L <= p` up
to floating-point tolerance. This is finite executable verification coupled
with an algebraic argument, not a machine-checked universal proof.

Rules require at least 8 eligible commits. The descriptive tiers are `medium`
at `L >= 0.6` and `high` at `L >= 0.8`. For example, at 400 eligible commits the
thresholds are 260 and 336 supporting commits. These tiers are **not calibrated
probabilities of a personal trait**. Commits are dependent, selection is
deterministic, and many predicates are examined. The usual binomial interval
interpretation needs assumptions this corpus does not establish. See the
[NIST description of Wilson intervals](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm).

MCP `mmcg_profile` uses `schema_version: 2`. Each convention exposes
`counterpattern` instead of the former `not` field. A counterpattern is an
observed alternative, not a prohibition. The `confidence` field retains its
descriptive tier string for clients; the response's evidence notes explain its
limits. Observations do not automatically accept preferences or create observed
human habits.

## Local history replay

`evals/persona_replay.py` runs the actual binary under isolated profile homes,
without a model or network request. It freezes all local commit refs and HEAD,
includes unmerged histories, pins a private copy of the binary, and records
SHA-256 digests. The anchor tree supplies current tooling/classification context
for the historical corpus. Author email selection is checked against raw Git
metadata; it does not merge aliases into people. Coauthor trailers are reported
as attribution limitations and are not assigned the primary author's diff.

```bash
python3 evals/persona_replay.py \
  --repo /path/to/local/repo --anchor origin/main \
  --binary mcp/servers/mmcg/target/debug/mmcg \
  --output .mastermind/research/persona-replay/baseline

python3 evals/persona_replay.py \
  --repo /path/to/local/repo \
  --binary mcp/servers/mmcg/target/debug/mmcg \
  --manifest .mastermind/research/persona-replay/baseline/manifest.json \
  --output .mastermind/research/persona-replay/candidate
```

Each identity is checked for selection/identity purity, repeated-run equality,
cold versus historical-then-current equality, measured diff accounting, and
absence of automatically published personal claims. Temporal coverage records
seed measurements, overlap and whether the classifier permits reuse. No overlap
is explicitly `not_exercised`; successful cold-versus-cold equality is not
evidence of cache reuse. Monthly `authored_nonmerge`, `listed`, and `measured`
counts are descriptive coverage, not probabilities of random inclusion.

The source repository is read-only. The replay borrows its immutable object
files via a local alternate; missing objects abort. Confirmed non-commit refs
are explicitly reported as skipped. The result directory includes private
author metadata, SQL stores and profile views and must stay outside source
control. The output is tied to local refs at the snapshot; it does not certify
that those refs match the current remote server.

Regression gates include real Git histories for the 400-commit sample boundary,
module-classifier changes, renamed clones, single-tip and unmerged corpora, and
missing or drifted frozen objects. They test algorithmic behavior. Trait
precision/recall still requires separately reviewed labels and held-out tasks.
