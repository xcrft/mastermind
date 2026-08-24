# Eval scorecard

This is the evidence ledger for complete behavioral regression runs. Read the
environment and trust note with every score: it is not a model leaderboard or
a coverage percentage, case sets differ by suite, and elapsed time depends on
the model, machine, and fixture setup.

Latest complete runs: 2026-07-31, all four suites. The 2026-08-11 rerun did
not reach inference because the local Claude Code OAuth session had expired;
those infrastructure exits are not recorded as model failures.

| suite | model | date | result | first pass | elapsed | evidence |
|---|---|---|---:|---:|---:|---|
| auditor | opus | 2026-07-31 | 9/9 | 8/9 | 2,097.0 s | Real Git fixtures and live mmcg against a release build of this branch; `a-007` needed one sentinel retry |
| critic | opus | 2026-07-31 | 5/5 | 5/5 | 283.0 s | Historical full suite; `c-002` predates the prompt-isolation repair below and is not current verification |
| intake | sonnet | 2026-07-31 | 5/5 | 5/5 | 98.4 s | Full suite |
| workflow | sonnet | 2026-07-31 | 51/56 | 51/56 | 1,005.4 s | Full suite; the nine QA, backend, security, and product cases all passed on first attempt |
| workflow | sonnet | 2026-07-30 | 45/47 | 45/47 | 778.9 s | Superseded snapshot |
| workflow | sonnet | 2026-07-19 | 36/36 | 36/36 | 525.6 s | Superseded snapshot, kept for comparison |

**Every failure was an assertion defect; none was a behaviour.** All nine cases
added since the previous run passed on first attempt, and the five workflow
failures were all pre-existing cases in three now-familiar classes:

- **A proposition quoted in order to reject it.** `w-027` assigned *low
  confidence* to the proposition `"BillingRouter does not exist"` and `w-029`
  wrote `must not be collapsed into a confident "coverage is proven" narrative`
  — both forbade exactly what the case forbids, and both tripped the
  `not_contains` that names it. `w-036` did the same on a third repair round.
  The better an answer is at naming what it rejects, the more reliably it fails
  this kind of assertion, so `not_contains` on a proposition is the wrong tool
  for a skill whose job is rejecting propositions.
- **A stem written differently.** `w-036` wrote `tool-enforced` against a list
  requiring `tooling`; `w-045` wrote `aren't checkable` against `not checkable`.
- **An incidental token.** `w-019` used `parseInt` where the case required
  `Number.parseInt`, while the behaviour under test was comment discipline and
  the `code_comments` policy — which passed.

`w-036` and `w-045` were measured rather than assumed: three runs each showed
1-in-3 and 2-in-3 failure. `w-045`'s root cause was in the artifact, not the
case — `mastermind-design-intake` told the agent to park unverifiable items in
"an explicit section" without naming one, so there was no stable token to
assert. Both intake skills now mandate an output shape with a `Not verifiable` /
`Outcome (not acceptance criteria)` heading, the way every audit skill already
does. All repaired cases passed 3/3 afterwards.

### `c-002` prompt-isolation repair, 2026-08-11

The methodology defect is fixed in the runner. Critic, intake, and workflow cases now
run with safe mode, an empty tool set, and a fresh empty temporary working
directory for every case. They cannot inspect this maintainer checkout or
inherit unrelated files from the system temporary directory. A deterministic
harness regression locks all three conditions.

The 2026-07-31 critic row remains as historical provenance, not proof of the
new isolation path. A complete rerun was attempted on 2026-08-11, but Claude
Code returned `Failed to authenticate: OAuth session expired and could not be
refreshed` before sending any input tokens. Reauthenticate with `claude auth
login`, then rerun the four suites before treating the repaired path as
model-backed evidence.

From the 2026-07-30 run: **the five repaired assertions held.** `w-008`, `w-029`, `w-031`, `w-032`, and
`w-034` all passed, and the eight cases added since (`w-040`–`w-047`) passed on
first attempt. Two *different* pre-existing cases failed, both on a forbidden
phrase that appeared inside a denial or a finding label rather than in a claim:

- `w-018` forbade `--client cursor`. The model wrote *"there's no
  `mastermind install --client cursor` step — we go straight to MCP setup"*,
  which is the exact behaviour the case tests.
- `w-030` forbade `test proves`. The model titled a finding *"Test proves the
  wrong path"*, describing the very mismatch under review.

Both were repaired by deleting the over-broad fragment and keeping the
affirmative claims already in the same list (`Cursor receives workflow skills`,
`authorization is covered`), then verified by targeted rerun. **No complete run
has been recorded since those two repairs**, so 45/47 stands as the reportable
number.

### The 34/39 intermediate run, 2026-07-30

Kept because its failure shape is the more instructive half of the record. An
earlier run the same day scored 34/39 and failed `w-008`, `w-029`, `w-031`,
`w-032`, and `w-034`. Every one of the five was a *missing required phrase*; not
one was a `not_contains` violation. The suite had lost vocabulary matches, not
behaviors — and the 36/36 snapshot from 2026-07-19 had stopped being
reproducible without a single behavior changing.

Two of the five are morphological near-misses against a model that answered
correctly:

- `w-029` requires the token `approval`. The model wrote *"provenance (`approved
  by product owner`) establishes authorization, not correctness"* and *"Do not
  let `approved by product owner` stand in for technical verification;
  authorization ≠ correctness"* — the exact proposition under test, in the wrong
  part of speech.
- `w-034` accepts `advisory` / `guidance` / `precedence` / `fallback`. The model
  wrote *"no competing tool-enforced or code-shape **precedent**"* while
  correctly applying the manual style preference over a silent repository.

`w-008` said "No spec" against a required "no task spec". `w-032` wrote
"unreviewed audit signal" and "has not been validated" against a list requiring
"unverified audit signal" or "does not establish".

`w-031` was a different defect. It asserted a `Unknown` section against
`mastermind-critical-review`, whose output contract defines `Observed`,
`Inferred`, `Confidence`, and `Would change the verdict` — and no `Unknown` at
all. That field belongs to `mastermind-project-history`. The model emitted the
critical-review envelope correctly and in full; the case had been passing only
because earlier outputs happened to use the word "unknown" in prose. An
assertion copied across two artifacts' contracts can only ever pass by luck.

The four frontend cases (`w-040`–`w-043`) cover component research before a
change and the frontend audit after it. Three of the four failed on first
authoring for the same reason the pre-existing cases did: `w-040` forbade a
phrase the prompt itself contains, `w-042` forbade `duplicate` while the skill
mandates a `Duplicates` heading, and `w-041` demanded a third specific example
of an invisible render path after the proposition had already been established.
All three were assertion defects, not behaviours; repaired, and passing in the
45/47 complete run. The four design and browser cases (`w-044`–`w-047`) needed
no repair and passed on first authoring.

### Assertion repairs, 2026-07-30

All five were repaired on the eval side, each against a stated rule rather than
against the output that failed:

| case | defect | repair |
|---|---|---|
| `w-008` | one spelling of a proposition | accept `no spec` alongside the two longer forms |
| `w-029` | bare noun in `contains` | moved to a `contains_any` lexeme family: `approval` / `approved` / `authorization` |
| `w-031` | asserted another artifact's contract | `Unknown` → `Inferred`, which critical-review actually mandates |
| `w-032` | synonym gap | added `not been validated` to the prove/establish family |
| `w-034` | full-phrase enumeration in both groups | added `precedent`; replaced group 2's four spellings with the stems `silent` / `no neighbor` / `no convention` |

Verified by per-case rerun, then confirmed by the 45/47 complete run above,
where all five passed. `w-034` needed two passes — the first repair exposed
brittleness in its second assertion group, which the same run had satisfied by
chance, so that case was flaky rather than merely mis-worded; it passed 3/3
after the stem repair.

The workflow suite includes four comment-discipline regressions: zero comments
for straightforward code, removal of narrating comments, preservation of one
non-obvious security rationale, and enforcement through the executor workflow
without explicitly invoking the standalone skill.

Those four passed 4/4 while the maintainer reported the write-time rule still
being dropped in real executor runs. The divergence is a property of the case
design rather than a flake: each case is one function, with the skill in the
system prompt and no tools, so it measures whether the rule is understood, not
whether it survives a long multi-file implementation competing with acceptance
criteria, tests, and a report. `mastermind-comment-audit` (`w-037`–`w-039`)
exists because of that gap — the rule is now also checked by a reader after the
fact, which is verification the write-time cases cannot provide.

The three `mastermind-comment-audit` cases (`w-037`–`w-039`) cover added
narration, restraint on load-bearing comments, and deleted rationale. They have
passed 3/3 first-pass in every run since they were added.

The workflow suite also includes four architecture-review regressions: context
loss across a runtime boundary, mutation of derived state instead of the source
of truth, non-durable idempotency under concurrent retries, and a breaking
event change across mixed-version and replay windows.

The lifecycle regressions keep codegraph misses inside an epistemic envelope,
honor superseded decisions and failed approaches, separate approval from proof,
require explicit lesson-candidate review, and prevent style evidence from
crossing author, language, repository, or authority boundaries.

## What a row does—and does not—prove

- Report only complete suite runs in the table. A targeted rerun can diagnose a
  case, but it does not change the suite pass rate.
- Workflow cases load the artifact as the system prompt, run with an empty tool
  set, and present one self-contained scenario. A green row licenses "the
  instruction is stated clearly enough to be followed", not "the behavior holds
  in practice" — nothing in this suite reproduces a long implementation whose
  primary goal competes with the instruction under test.
- A `not_contains` entry must be an affirmative claim, not a fragment. A
  fragment matches inside an explicit denial (`there is no --client cursor`) and
  inside a finding label (`Test proves the wrong path`), failing answers that
  are exactly right. It must also appear in neither the prompt nor the
  artifact's own required output shape.
- `not_contains` on a proposition is unreliable for a skill whose job is to
  reject propositions. A good answer names what it rejects, in quotes, and trips
  the assertion. Require the mandated output shape instead.
- A skill with no mandated output shape cannot be asserted robustly. Every
  spelling of the same idea is a coin flip; name the section the artifact must
  produce and assert on that.
- A count is not a phrase. Requiring `four` fails a model that writes `4`, and
  the number is almost never the proposition under test — assert the structure
  the artifact mandates instead.
- Required-phrase assertions bind a behavior to one vocabulary. A model that
  reasons correctly and words it differently fails, so a drop in the pass rate
  is not by itself evidence of a behavioral regression. Separate the two before
  acting: count `not_contains` violations, which indicate wrong behavior, apart
  from missing-phrase misses, which may only indicate paraphrase. Widening a
  phrase list to admit the output that just failed fits the benchmark to a model
  revision and is not a repair.
- Auditor results are valid only when the mmcg index is present. Missing index
  setup is a hard failure, not a degraded pass.
- Auditor verdicts come from the structured YAML tail rather than prose.
- Fixtures must not contain comments or metadata that reveal the expected
  answer to the model.
- Results before `4c338b6` (2026-06-10) used prose verdict matching and should
  not be compared with current runs.

Run the deterministic gates and all model suites with:

```bash
bash evals/run-verified.sh --model sonnet
```

## Out-of-suite observations

Recorded for provenance. These are single observations, not pass rates: one run
establishes nothing about variance, and none of them belongs in the table above.

**2026-07-30 — `mastermind-comment-audit` on a live diff (sonnet).** Run against
the real 308-line working-tree Rust diff of this repository rather than a
constructed case. Verdict `clean`: 7 comments added, 0 flagged, 7 kept. No
over-flagging on real content, which is the failure mode a dedicated slop hunter
is most exposed to — a reviewer that justifies its existence by finding things
would have deleted project-specific `why` comments here. The reviewer also found
a deleted comment and declined to file `removed_rationale`, reasoning that the
change had replaced that behavior and carried an updated explanation at the same
call site; the distinction between rationale lost and rationale rewritten is not
specified in the contract.

Limits: one run, one diff, one model, no tools. Reported line numbers were
approximate and the contract's file-reading step was never exercised, so this
observation covers judgment on a supplied delta, not delta collection.

## Ablation status

The auditor ablation compares a neutral Claude + Git/read/search baseline with
the real auditor using live mmcg. It has not been run for this snapshot, so this
scorecard makes no causal uplift claim.

```bash
python evals/ablation.py --with-mastermind
```
