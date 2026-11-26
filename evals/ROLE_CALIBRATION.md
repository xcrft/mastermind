# Calibrate workflow roles

Use this guide to pick a role prompt and reasoning effort by measuring them on tasks with a known outcome. A role name or a stronger model alone says nothing about quality, so we calibrate the whole combination of role instructions, model, effort and tool access. Acceptance and resource rules are in [product success measures](PRODUCT.md), and retained results go to `baselines/`.

## Start from the shipped settings

Each Claude subagent sets its starting `model` and `effort` in the frontmatter of its file in [agents/subagents](../agents/subagents). These are defaults to qualify, not measured winners. They use the Claude aliases `sonnet` and `opus`, which the client resolves, and user overrides, environment and managed policy can still change the served setting. Frontmatter doesn't pick the parent model, the miner processor or the model of the native `run-task` CLI. To measure one model version, pin its exact ID in the benchmark configuration.

The aliases don't select a Codex model either. For Codex, freeze a separate model and effort mapping and compare `high` against a fixed `max` for each role. `high` and `max` are requested settings, and the served effort stays unknown.

## Run the experiments in order

1. Compare one role prompt with the existing prompt on identical tasks at the same model, effort and tools. Judge both arms with the same original-request key. Use the `role_prompt` axis.
2. With the role fixed, compare `max` and `high`. Don't change the prompt, refinement, profile or tools in this step. Use the `effort` axis.
3. Compare direct completion with the routed workflow. Charge routing, every handoff, verification, escalation and failed attempt to the workflow. The benchmark has no writer or handoff runtime, so this step needs its own adapter and `workflow` is not a calibration axis.
4. Freeze the winner before unseen qualification. An output you saw during development can't become an independent qualification case.

The axis declaration, the preparation commands and the campaign steps are in [Prepare an experiment](benchmark/README.md#prepare-an-experiment). Label every answer with [review admission](benchmark/REVIEW.md) before you compare arms.

## Build distinct task families

| Family | Include | Required negative case |
|---|---|---|
| Facts and document meaning | Exact facts, selectors and pagination, exceptions, supersession, unknown source coverage | An ambiguous or absent symbol, or a current but unverified relation |
| Diagnosis | Small reproducers, competing causes, races, environment failures | A plausible cause that an observed probe contradicts |
| Planning and design | Localized fixes, coupled state and API contracts, alternatives, rollback | A simpler valid design, where extra scope is wrong |
| Implementation | Real defects, protected behavior, applied diffs | Existing valid behavior that must survive the fix |
| Audit | Independently labeled defects and clean changes, subtle boundary failures included | Clean code, where any finding is an error |
| Refinement | Original requests with conditions, intended scope and accepted results | A clear request that should pass through unchanged |

Hold out whole repositories and task families. Repeats of one task measure variation, not new samples. Use different defects instead of variations of one fixture, keep oracle checks away from the executor, and keep the repository's required tests.

Today the benchmark can keep Codex and Claude research answers and their reviews. It can't evaluate an applied fix, because its tools are read-only. Implementation calibration needs a writer adapter with disposable repositories, observed checks and an approved scope.

## Judge the whole result

| Measure | Research or diagnosis | Implementation | Review |
|---|---|---|---|
| Original-request acceptance | Every material obligation answered, or a justified abstention | The desired behavior and diff | Correct criterion decisions |
| Material mistakes | Unsupported facts or a false cause | Wrong behavior or a regression | A false approval or an invented defect |
| Evidence coverage | Source support and explicit omissions | Relevant observed checks, with a regression that fails before the fix | Labeled defect recall and false positives on clean cases |
| Delivery cost | All input, cache and output tokens, full latency, tool rounds | Same, plus tests and repair | Same, plus independent inspection |
| Escalation | The right handoff, with the open gap kept | The right blocker or repair owner | Inspection instead of an unsupported approval |

Mark a demonstrated missing criterion `unmet` and keep `unknown` for a judgment nobody could resolve. Apply the frozen quality gate before you look at resource savings, and keep each paired loss even when another task improves. Among candidates that pass the gate, choose on their observed token and time costs. Use a reviewer who can't see producer reasoning or condition metadata, and calibrate model judgments against human labels before a model judge becomes a primary reviewer.

A failed role, a rejected review or missing evidence stays in the denominator. Fix the owning code or instruction and prepare a new balanced run with new result paths.
