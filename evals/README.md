# Evaluation

Run commands from the repository root. Deterministic checks test the harness;
model-backed trials measure answers under an explicit experiment configuration.

Product objectives, the current baseline and audit/fix acceptance rules are in
[Product success measures](PRODUCT.md).
Use [role calibration](ROLE_CALIBRATION.md) to compare role instructions,
reasoning settings and the complete routed workflow separately.

## Choose a package

| Package | Purpose | Instructions |
|---|---|---|
| `behavior` | Shipped role and workflow assertions, fixture-backed reviews | [Run behavioral suites](behavior/README.md) |
| `benchmark` | Matched research trials, blinded review and paired outcome/cost analysis | [Prepare and run](benchmark/README.md), [review](benchmark/REVIEW.md) |
| `control` | Completion guards, finite model and selected production CLI regressions | [Run the control checks](control/README.md) |
| `intake` | Prompt-refiner protocol and retained processor outputs | [Run intake evaluations](intake/README.md) |
| `persona` | Git attribution, mining replay and local extraction labels | [Replay contract](../docs/reference/persona-mining-contract.md) |
| `shared` | Bounded subprocess supervision used across the packages | [Test instructions](../tests/README.md) |

| Location | Contents |
|---|---|
| `evals/<package>/` | Runtime logic and package-owned corpora/fixtures |
| `tests/evals/<package>/` | Deterministic behavioral and integration tests |
| `tests/evals/support/` | Shared disposable fixtures, independent of test classes |
| `tests/validation/` | Repository, publication and document-evidence checks |
| `evals/baselines/` | Retained measurements with their original source bindings |

Reports with `kind: mastermind-public-report-projection` retain the original
report's SHA-256 and list fields whose local paths were redacted. Their `report`
contains the published measurements; the original remains private. A projection
is not a canonical input for review admission or runtime verification.

## Run checks

| Goal | Command |
|---|---|
| All Python harness and repository tests | `just eval-harness` |
| The same tests without just | `python3 -m unittest discover -s tests -t .` |
| One package | `python3 -m unittest discover -s tests/evals/benchmark -t .` |
| Validate research source/key bindings | `python3 -m evals.benchmark.corpus --source-repo .` |
| Require current research source bytes | `python3 -m evals.benchmark.corpus --source-repo . --require-current` |
| Model-only completion checks | `python3 -m evals.control --model-only --output /tmp/new-control-report` |
| Full control/CLI integration | `python3 -m evals.control --output /tmp/new-control-report` |
| Local extraction labels | `mmcg miner hooks evaluate-local --input evals/persona/local.json` |
| Index latency and memory | `just benchmark-index` |

The Python tests use disposable fixtures and make no model calls. Full control
integration needs Rust, Git and Node on POSIX. Run it against stable sources.
Model-backed behavioral runs use [the verified runner](behavior/README.md) and
consume inference usage. Retained outcomes and measurement limits live in the
[scorecard](scorecard.md).

## Measurement contract

| Direction | Metric | Interpretation |
|---|---|---|
| Useful completion | Accepted tasks / all attempted tasks, plus user corrections | Independently judged outcomes, including failed attempts |
| Reasoning quality | Material false conclusions, required evidence coverage, appropriate abstention | Review complete retained answers against a fixed source key |
| Control | Completion without required proof, stale-evidence rejection, budget stops | Per-scenario results with failures, no coverage percentage inferred from counts |
| Context efficiency | Input + cache creation + cache read tokens, p50/p95 | Preserve quality and compare the same tasks, runtime and budgets |
| Execution cost | Total tokens, elapsed time, tool rounds, retries, reported cost | All attempts counted, setup separate, missing values remain unknown |
| Persona quality | Supported habits, contradictions, review/abstention rates, held-out task benefit | Git statistics and extraction volume do not establish human traits |
| Indexing | Cold/warm/changed latency, memory, indexed/failed files | Describe corpus, machine, extraction contract and correctness checks |

For a comparable measured cost `C`, savings are `1 - C_current / C_baseline`.
Do not calculate a savings percentage when the baseline is zero, missing or
incomparable. A reduction is useful only when the required quality and control
criteria still hold. Repeated matched runs are needed to estimate variance.

For useful-work efficiency, also record accepted outcomes `S` across all planned
attempts. Resource per success is `C / S`; yield is `S / C`. Compare yield only
when outcomes are resolved and resources are complete. Zero baseline successes
make a relative yield gain undefined. Failures still contribute their observed
resource use.

## Match the experiment to a feature

Use [persona delivery inspection](persona/README.md) to retain selected rule IDs,
text differences, revisions and omissions separately from model application.

| Feature | Matched comparison | Existing measurement boundary |
|---|---|---|
| Research instructions | `source` vs `portable` on the same source/key | Static skill instructions, not native prompt refinement |
| Code graph retrieval | `portable` vs `portable_mmcg` | Adds graph tools; semantic review still required |
| Combined research flow | `source` vs `portable_mmcg` | Four published calibration tasks, not representative user work |
| Prompt refinement | Raw task vs frozen refined task under the original outcome key | Harness supports explicit arms; generation, native delivery and end-to-end benefit need separate runs |
| Personal profile | Same task with/without a frozen applicable profile, plus a shuffled-profile control | Profile mining and task benefit remain separate; no qualified benefit sample yet |
| Completion guards | All obligations enabled vs each omitted guard | Finite-model safety and sampled CLI conformance, not semantic task quality |
| Incremental index | Cached vs forced-full index of identical source | Latency plus symbol/call/reference equivalence, no inference |
| Local style detector | V1 vs V2 on the same labeled examples | Synthetic candidate/span accuracy, not real habit precision |
| Project/document context | Relevant context vs none, with stale and irrelevant controls | Freshness checks exist; relevance and accepted-task benefit need a controlled corpus |

Use [the campaign commands](benchmark/README.md#run-the-whole-corpus) to run every
research case under one configuration. Model/runtime switches require a new
campaign. The [scorecard](scorecard.md) records current results and gaps.

| Comparison rule | Reason |
|---|---|
| Retain raw cases and recompute aggregates | Prevent selective result reporting |
| Count input + cache creation + cache read tokens | Measure context delivered to the model |
| Keep reported billing cost separate | Token volume is not a billing estimate |
| Match case/source digests, model, CLI and runtime controls | Compare equivalent experiments |
| Reject lower pass rate or regression of a previously passing case | Preserve the required quality gate |
| Require lower context-token p50 and p95 | Apply the runner's declared efficiency gate |
| Use complete current evidence for new baselines | The retained critic baseline alone has a legacy capture exception |

Research reviews retain claim support, required-known coverage, unknown handling
and reviewer disagreement. Causal uplift and automatic semantic scores remain
unmeasured.
