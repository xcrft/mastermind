# Calibrate workflow roles

Choose a role and reasoning setting by the task's required outcome and risk.
Calibrate the combination of role instructions, model, effort and tool access.
A role name or a stronger model alone does not establish quality.

Use [product objectives](PRODUCT.md) for acceptance and resource accounting.
Retained measurements belong in `baselines/`; this guide defines the procedure.

## Claude assignments

The shipped Claude subagents use these starting settings. They are engineering
defaults to qualify, not measured winners. The client resolves model aliases;
user overrides, environment and managed policy can affect the served setting.

| Role | Model | Effort | Use |
|---|---|---|---|
| Researcher | Sonnet | `medium` | Bounded facts with conditions, counts and source limits |
| Investigator | Opus | `high` | Unknown cause, competing hypotheses and discriminating probes |
| Task executor | Sonnet | `high` | Approved implementation scope and actual checks |
| Critic | Opus | `high` | A material design fork |
| Auditor / security auditor | Opus | `high` | Correctness or a privileged boundary |
| Comment / test / frontend auditor | Sonnet | `medium` | The corresponding changed domain |
| Prompt refiner | Sonnet | `low` | Preserve original intent and avoid unnecessary refinement |
| Feedback collector | Sonnet | `medium` | Source-bound user corrections and working rules |

Researcher checks requested obligations before returning. It keeps full versus
returned counts, selection ambiguity and coverage limits explicit. A short
answer may not discard an exception or a necessary caveat. A missing fact goes
to the next owner with its exact evidence gap; handing off is not acceptance.

Frontmatter controls Claude subagents. It does not select the parent model,
the miner processor or the native `run-task` CLI model. To measure a particular
model version, pin its exact identity in the benchmark configuration.

## Codex candidates

The settings below are candidates to measure on the user's configured Codex
model. `high` and `max` are requested settings, not verified served identities.
They are not a qualified production routing policy.

| Required work | Owner | Candidate to compare with fixed `max` | Acceptance evidence | Escalation condition |
|---|---|---|---|---|
| A bounded fact in code or documents | Researcher | `high` | Supported facts, required fields/conditions, citations and explicit unknowns | Ambiguous identity, contradiction, missing material evidence or a decision request |
| Diagnose an unknown-cause defect | Investigator | Keep `max` initially | Reproducer, discriminating probe, supported cause and rejected alternatives | No usable reproducer or unresolved causal alternatives |
| Define scope and an implementation plan | Planner | `high` for localized changes; retain `max` for coupled contracts | Original intent, falsifiable criteria, appropriate scope and required checks | State, permissions, compatibility, migration or difficult rollback |
| Assess a real design fork | Critic | Keep `max` initially | Decision-relevant tradeoffs, supported objections and limits | Unresolved material design risk; no critic needed for a literal change |
| Implement an approved localized fix | Executor | `high` | Actual diff, observable corrected behavior, preserved checks and no scope expansion | Cross-module invariants, concurrency, security or failed protected behavior |
| Review correctness or a security boundary | Auditor / security auditor | Keep `max` for material risk | Supported defect findings and valid changes, current evidence, no false approval | Missing critical proof or disagreement requiring inspection |
| Check comments, tests or frontend behavior | Relevant specialist | `high` | Domain-specific real defects and valid negative cases | Actual changed domain or unresolved evidence; skip unrelated specialists |
| Refine a request | Prompt refiner | Separate experiment before assigning effort | Intent and conditions preserved, fewer corrections, original-request outcome | Meaning change, invented scope or missing context |

Researcher hands off facts; investigator establishes cause; planner makes the
implementation decision. Executor changes code. Auditor assesses the result
from the contract and actual evidence. The controller owns lifecycle state and
completion guards. Do not turn every task into a chain of all roles.

Use Direct for small reversible work, Verified for explicit contracts and
Strict for high-impact work as described in [the workflow](../docs/workflow.md).
An incomplete path classifier or unfamiliar task requires inspection; absence
of a recognized risk word is not evidence of low risk.

## Pin the runtime and role

| Input | What to retain |
|---|---|
| Role | Agent/skill bytes, version, resolved referenced instructions and output contract |
| Runtime | Requested model/effort, client version/hash, tools, permissions, authentication scope and limits |
| Task | Original request, source/diff, risk, expected outcome and hidden acceptance key |
| Context | Code/document/profile source and review revisions, exact offered context and explicit omissions |
| Handoff | Producer facts, citations, remaining obligations/unknowns and receiver context; no implicit acceptance |
| Result | Every planned attempt, answer/diff, observed checks, reviews, retries and failures |

The shipped subagent frontmatter uses Claude aliases (`haiku`, `sonnet`,
`opus`). Freeze a separate model/effort mapping for Codex; aliases do not select
a Codex model. The native managed invocation in `invocation.rs` resolves Claude.
The Codex benchmark adapter supports subscription-backed read-only research.
Their permissions, tools, reports and timing contracts are different.

The shipped researcher also limits findings and discovery calls. The effort
study used a different instruction/output contract. Its savings cannot be
assigned to the shipped researcher without testing that role's complete
contract. A portable prompt comparison must explicitly label unresolved skill
references and omitted native capabilities; it is not native role activation.

## Build distinct task families

| Family | Include | Required negative case |
|---|---|---|
| Facts and document meaning | Exact facts, selectors/pagination, exceptions, supersession and unknown source coverage | Ambiguous/absent symbol or a current but unverified relation |
| Diagnosis | Small reproducers, competing causes, races and environment failures | A plausible cause contradicted by an observed probe |
| Planning and design | Localized fixes, coupled state/API contracts, alternatives and rollback | A simpler valid design; reject unnecessary scope |
| Implementation | Real defects, protected behavior and applied diffs | Existing valid behavior that must survive the fix |
| Audit | Independently labeled defects and clean changes, including subtle boundary failures | Clean code; a false finding is an error |
| Refinement | Original requests with conditions, intended scope and accepted results | A clear request that should pass through |

Hold out repositories and task families. Repeats of one task measure variation;
they do not add independent task samples. Use different defects, not variations
of the same fixture solely to enlarge a score. Keep oracle checks unavailable
to the executor, and preserve repository-required tests.

## Separate the experiments

1. Compare one role prompt with the existing prompt on identical tasks at the
   same model/effort/tool settings. Judge both with the same original-request key.
2. With the role fixed, compare `max` and `high`. Do not change prompt,
   refinement, profile or tools in the effort comparison.
3. Compare direct completion with the proposed routed workflow. Charge routing,
   every handoff, verification, escalation and failed attempt to that workflow.
4. Freeze a winner before unseen qualification. An output observed during
   development cannot become an independent qualification case.

Declare the experiment axis before preparing a batch or campaign:

```json
{
  "calibration": {"axis": "role_prompt"},
  "conditions": [
    {"id": "direct", "role": "direct", "tools": "source", "reasoning_effort": "high", "instruction_paths": []},
    {"id": "researcher", "role": "researcher", "tools": "source", "reasoning_effort": "high", "instruction_paths": ["agents/subagents/mastermind-researcher.md"]}
  ]
}
```

Add these fields to a complete pinned runtime configuration. `role` labels the
portable instructions; it neither starts a native agent nor grants its tools.
Instruction files are frozen verbatim from `tool_revision`. Referenced skills
are not automatically resolved; include their applicable files explicitly.

| Axis | Held fixed | May vary |
|---|---|---|
| `role_prompt` | Model, effort, tools, task, sources, limits and common runtime settings | Role label and instruction files |
| `effort` | All of the above except effort, including role and ordered instruction paths | Requested effort |

Both axes require a role and effort on every condition. Preparation rejects
mixed variables. The axis is bound into trial, batch and campaign records and
survives offline export. Removing or relabeling it fails validation. Claude and
Codex adapters transmit declared effort; observed served effort remains unknown.
The experimental Codex effort router may select the same effort as the control
for a case; retain that case without presenting it as an effort contrast.

Use the same preparation and review commands for both axes:

```sh
python3 -m evals.benchmark.campaign prepare --config /absolute/path/config.json \
  --source-repo /absolute/path/repository --tool-repo /absolute/path/mastermind \
  --output /absolute/path/new-campaign --repetitions 2
python3 -m evals.benchmark.campaign run /absolute/path/new-campaign
python3 -m evals.benchmark.campaign export /absolute/path/new-campaign \
  --output /absolute/path/new-review-set
```

Follow [review admission](benchmark/REVIEW.md) to label every answer, then compare
the declared arms. The built-in Claude research adapter retains its API-key
authentication contract; an authorized run selects its existing credential with
`campaign run ... --credential-env ANTHROPIC_API_KEY`. The commands above use no
selected credentials and are suitable for the Codex subscription adapter.
Use the Codex adapter for subscription runs; changing
Claude subagent defaults requires no API key or separate benchmark run.

`workflow` is not a supported calibration axis. This harness has no writer or
handoff runtime, so it cannot account for their tokens or certify a whole routed
workflow. Do not treat portable role-prompt trials as that measurement.

Start with bounded research and labeled code-audit tasks. The current benchmark
can retain Codex research answers and semantic reviews. It cannot evaluate an
applied fix with its read-only tools. Implementation calibration needs a writer
adapter with disposable repositories, observed checks and an approved scope.

| Existing package | What it currently establishes | Needed for role calibration |
|---|---|---|
| `benchmark` | Claude/Codex read-only trials, separated role-prompt/effort axes, source bindings and manual acceptance | New role-specific tasks; independent labels/review |
| `behavior` | Shipped Claude roles on fixtures; mostly structured/phrase assertions | One shared semantic key across arms, false positives/misses and a matching Codex path |
| `control` | Deterministic publication and selected production regressions | Preserve these guards; they do not measure agent judgment |
| `intake` | Refiner protocol and processor outputs | Paired original-request outcomes and all refinement costs |

Use [trial preparation](benchmark/README.md) and [review admission](benchmark/REVIEW.md)
for supported read-only runs. Writing, causal probes and production role
activation need their own explicit adapter contracts. Do not silently grant a
research adapter those capabilities.

## Evaluate the whole result

| Measure | Research/diagnosis | Implementation | Review |
|---|---|---|---|
| Original-request acceptance | All material obligations answered or a justified requested abstention | Actual desired behavior and diff | Correct criterion decisions |
| Material mistakes | Unsupported facts or false cause | Wrong behavior or regression | False approval or invented defect |
| Evidence coverage | Source support and explicit omissions | Relevant observed checks; regression fails before the fix | Labeled defect recall and false positives on clean cases |
| Delivery cost | All input/cache/output tokens, full latency and tool rounds | Same, including tests and repair | Same, including independent inspection |
| Escalation | Appropriate handoff, unresolved gap retained | Correct blocker or repair owner | Inspection instead of unsupported approval |

Classify a demonstrated missing criterion as `unmet`; reserve `unknown` for an
unresolved judgment. Source-key coverage stays separate from user acceptance.
Evaluate paraphrases by meaning, not required keywords. A valid citation or an
exit-zero transport result does not prove the answer correct.

Apply the frozen quality gate before resource savings. Retain each paired loss
even when another task improves and average acceptance is unchanged. Choose
between quality-qualified candidates on their observed token/time frontier.
Unknown billing stays unknown; avoid an invented dollar conversion.

Use a reviewer with no access to producer deliberation or condition metadata,
and calibrate semantic judgments against human labels. A fresh context or a new
reviewer label does not establish independence. Report remaining disagreement.
Do not add model judges before their false approvals and omissions are measured.

## Run gates and recover

```sh
python3 -m unittest discover -s tests -t .
python3 scripts/validate.py
```

Run the applicable native profile/completion regressions against stable current
sources before routing changes. Preserve ranking, whole-card delivery, access,
source revisions, acceptance and completion guards. Compare the same protected
context selection in both arms; role calibration is not a larger profile budget.

Use new immutable result paths. A failed role, rejected review or missing
evidence remains in the denominator. Diagnose it, fix the owning implementation
or instruction, then prepare a new balanced run. Escalation is successful only
when the completed routed workflow satisfies the original request; every
escalation consumes resources and must be charged.
