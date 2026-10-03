# Persona mining: evidence and reproducibility

Git mining describes observations attributed to an author filter. It cannot
establish a person's identity, authorship of every changed line, proficiency,
intent, or stable habits. Bots, shared accounts, coauthors, generated changes,
formatters and model assistance remain attribution limits.

For collection and review commands, see [Persona](persona.md).

## Task loop verification

| Local path | Established behavior |
|---|---|
| Prompt hook | Capture precedes profile offer; retained event influence cannot be cleared by incoming flags |
| Response boundary | Complete closed episodes can produce bounded exact-statement drafts, without a provider or automatic acceptance |
| Repeated boundary | Same episode revision and detector fingerprint do not create another analysis |
| Later context | Reprocessing keeps the source binding current and resets changed authorship attestation |
| Native executor | Refresh committed observations, preserve the stored author selector, select the executor's own scope, pin the delivered packet |
| Reviewed task completion | Refresh committed observations after the completion verdict, record the Git snapshot separately |
| Missing evidence or delivery opt-out | No automatic local candidate publication |
| Independent review | Does not receive the personal profile |

Isolated CLI tests exercise these links with synthetic hooks, Git history and
native clients. They establish those tested control-flow properties. They do
not establish client activation in a real session, honest human authorship,
durable habits or a causal improvement in task results. None of these links
turns the Wilson score below into a probability that a personal claim is true.

## Algorithmic contract

| Symbol | Input |
|---|---|
| `H` | Bounded ordered commits for one pinned Git revision and author selector |
| `D` | Detector and grammar contract, tooling scopes and first-party module classifier derived from that revision |
| `K` | Valid cache produced for the same inputs |
| `S` | Selected commit set |
| `M` | Measurement data, excluding publication timestamps and filesystem provenance |

For the ordinary deterministic miner:

```
S(H, D, K) = S(H, D, empty)
M(H, D, K) = M(H, D, empty)
```

| Condition | Guaranteed behavior or limit |
|---|---|
| Preconditions | Intact Git objects/cache, successful bounded reads, no concurrent store mutation |
| Deep model-assisted candidates | Outside this deterministic contract |
| Selection | Deterministic monthly rounds, at most 400 eligible source commits from 2,000 listed non-merge commits |
| Cache | Skips diff reads only for currently selected SHAs, cannot exclude newly selected commits |
| Reuse | Requires matching detector/grammar/tooling/first-party fingerprint; v7 replaces legacy line-based imports and incorrect hunk-row counters |
| Import measurement | Parse complete committed blobs, count import syntax intersecting added rows, once per library per commit. Comments and string contents cannot supply imports |
| Import limits | Standard and first-party modules excluded; missing, oversized or unparseable blobs make the sampled commit's import coverage incomplete |
| Import coverage | The MCP and Markdown range section disclose complete / incomplete import reads for supported import languages separately from diff coverage. An omitted range also omits its coverage |
| New local module | Can change historical import classification, invalidating cached measurements |
| Duplicate SHA | Measured copy wins over metadata-only copy |
| Equal-strength copies disagree on counters or author date | Mark `evidence.context_conflict` and withhold counters |
| Checkout area aliases | Union provenance, at most one vote per alias per SHA |
| Reordering or repeated copies | Does not change reconciled measurements |
| Added repositories | May change repository counts, historical occurrences, area aliases and publication hashes |
| Future merge | Needs original records, not only reconciled output |
| `diff_sampled` | Distinct non-conflicting SHAs with a measured diff, separate from listed commits and added lines |
| Unread diff | Unknown test-marker evidence |
| No test paths or markers in inspected changes | Does not prove tests are absent, unrun or failing |

## Commit support and Wilson score

| Vote | Definition |
|---|---|
| `E` | Commits with a measurable opportunity |
| `A` | Supporting commits, `0 <= A <= E` |
| Unit | One vote per eligible commit |
| Tied patterns | Eligible, supports neither strict majority |
| Neither signal | Unknown, excluded |
| Preferred side and support | Use the same votes |
| More lines within a commit | Cannot reverse the decision if that commit's majority is preserved |

The score is the lower Wilson bound with `z = 1.96`, `p = A/E`:

```
L = (p + z²/(2E) - z sqrt(p(1-p)/E + z²/(4E²))) / (1 + z²/E)
```

For `E > 0`, `0 < q < 1`, inversion of the score inequality gives:

```
L >= q  iff  A >= E*q + z*sqrt(E*q*(1-q))
```

| Check or score | Interpretation |
|---|---|
| Inverted inequality | Independent algebraic oracle for the square-root formula |
| Regression enumeration | Every integer `A` in `[0,E]`, `E` in `[1,2000]`, at both thresholds |
| Additional assertions | Monotonicity and `0 <= L <= p` within floating-point tolerance |
| Proof boundary | Finite executable verification plus algebra, not a machine-checked universal proof |
| Minimum support | At least 8 eligible commits |
| `medium` / `high` | `L >= 0.6` / `L >= 0.8` |
| Example at `E=400` | 260 / 336 supporting commits |
| Trait interpretation | **Not calibrated probabilities of a personal trait**. Commits are dependent, selection is deterministic and many predicates are examined |

The usual binomial interval interpretation needs assumptions this corpus does
not establish. See [NIST's Wilson interval description](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm).

| MCP `mmcg_profile` field | Contract |
|---|---|
| `schema_version` | `2` |
| `counterpattern` | Observed alternative, not a prohibition. Replaces the former `not` field |
| `confidence` | Descriptive tier string with evidence notes describing its limits |
| Git observations | Cannot automatically accept preferences or create observed human habits |

## Local history replay

| Replay input or operation | Contract |
|---|---|
| `evals/persona_replay.py` | Actual binary under isolated profile homes, no model or network request |
| Frozen history | All local commit refs and HEAD, including unmerged histories |
| Binary | Private pinned copy with SHA-256 digests |
| Historical classifier | Anchor tree supplies tooling/classification context |
| Author selector | Email checked against raw Git metadata, aliases are not merged into people |
| Coauthor trailers | Reported as attribution limits, not assigned the primary author's diff |

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

| Replay result or boundary | Interpretation |
|---|---|
| Per-identity checks | Selection/identity purity, repeated-run equality, cold versus historical-then-current equality, measured diff accounting and no automatic personal claims |
| Temporal coverage | Seed measurements, overlap and classifier reuse eligibility |
| No overlap | `not_exercised`. Cold-versus-cold equality does not exercise cache reuse |
| Monthly `authored_nonmerge`, `listed`, `measured` | Descriptive coverage, not random-inclusion probabilities |
| Source repository | Read-only, immutable objects borrowed through a local alternate |
| Missing object | Abort |
| Confirmed non-commit ref | Explicitly reported as skipped |
| Output directory | Private author metadata, SQL stores and profile views. Keep outside source control |
| Revision scope | Local refs at the snapshot, no certification of current remote refs |

Regression gates include real Git histories for the 400-commit sample boundary,
module-classifier changes, renamed clones, single-tip and unmerged corpora, and
missing or drifted frozen objects. They test algorithmic behavior. Trait
precision/recall still requires separately reviewed labels and held-out tasks.
