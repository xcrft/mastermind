# Optimization research

Use this guide to choose experiments for code audits and fixes. Native batched
symbol lookup and diagnostic returned-range accounting in the Codex harness are
implemented. The remaining applications below are research proposals. Use the
measurements to choose routing and context-selection changes.
Keep objectives and retained results in [PRODUCT.md](PRODUCT.md); do not replace
those results with improvements reported by another paper.

Codex adapter diagnostics also retain reported reasoning output and the union of
client-observed tool intervals. Reasoning tokens are a subset of output, not an
additional cost. Tool event receipt does not isolate server execution; time
outside those intervals includes startup, network, queuing and model work.
Unavailable stages remain unknown.

## Choose a method

Start with deterministic retrieval and context selection. Introduce learned
policies after collecting independent task outcomes. Production references were
checked against the clean 3.2.2 source, separately from local harness changes.

| Order | Method | Mastermind application | Measures to improve | Additional inference |
|---|---|---|---|---|
| Measure first | Task-local returned-range ledger and campaign summary (implemented) | Measure repeats, failures and unknowns across every planned attempt before choosing a reuse policy | Repeated delivery and completed range coverage | None for bookkeeping |
| First | Adaptive retrieval, inspired by [Adaptive-RAG](https://aclanthology.org/2024.naacl-long.389/) | Known location → direct read; named symbol → exact lookup; unresolved cross-file question → concept/graph expansion | Evidence recall, tool rounds, total latency | None for an initial deterministic router |
| First | [Budgeted submodular coverage](https://research.ibm.com/publications/a-note-on-maximizing-a-submodular-set-function-subject-to-a-knapsack-constraint) | Select complementary optional source/document blocks after preserving admitted profile rules and task constraints | Relevant evidence per context unit, actual input tokens | None with fixed relevance tags |
| After acceptance labels | [GEPA](https://arxiv.org/abs/2507.19457v2) | Evolve role/refinement/retrieval instructions from failure feedback; retain complementary candidates | Original-request acceptance, corrections, tokens and latency | Offline candidate generation and evaluation |
| Alternative to GEPA | [MIPRO](https://aclanthology.org/2024.emnlp-main.525/) / [MIPROv2](https://dspy.ai/api/optimizers/MIPROv2/) | Search combinations of instructions and examples across modules | Same outcomes, including optimization overhead | Offline proposal/search calls |
| After routing labels | [Conservative contextual bandits](https://yasinov.github.io/conservative-bandits-nips2017.pdf) | Learn which admitted retrieval route works for each task family, with fallback to the fixed route | Accepted-task yield and routing regret | A local fitted policy; exploration still requires task runs |
| After profile labels | [Conformal risk control](https://arxiv.org/abs/2208.02814) | Calibrate optional candidate advice against labeled errors and abstentions | Erroneous advice emitted per task, delivery coverage | Calibration can be local |
| Evaluation prerequisite | Paired task-level analysis; [confidence sequences](https://arxiv.org/abs/1810.08240) for sequential qualification | Separate actual improvement from variation and repeated inspection | Confidence in acceptance and measured savings | None for statistics |

The adaptive retrieval paper studies question answering with a trained small-LM
classifier. A deterministic code router is our proposed adaptation; its benefit
is unmeasured. Treat a location as usable only after freshness, scope and symbol
identity checks. Exact-name collisions and incomplete graph edges remain explicit.

## Optimize optional context

Let `M` be the protected context produced by the existing admission and profile
selection contract. For optional blocks `i`, define additive cost `c_i`,
nonnegative requirement weights `w_r` and coverage tags `a_ir` in `[0, 1]`:

```text
F(S) = sum_r w_r * min(1, sum_(i in S) a_ir)
maximize F(M union S) - F(M)
subject to sum_(i in S) c_i <= B_remaining
```

This weighted coverage surrogate has diminishing returns: another block covering
the same requirement contributes less. Start by comparing marginal coverage per
cost with the existing layer selection. Deduplicate overlapping ranges first;
keep a statement, exception and source binding together. Freeze tag generation
and weights before evaluation. Gold acceptance labels stay outside model inputs.

[Sviridenko's algorithm](https://research.ibm.com/publications/a-note-on-maximizing-a-submodular-set-function-subject-to-a-knapsack-constraint)
has a `1 - 1/e` approximation guarantee for monotone submodular maximization under
an additive knapsack constraint. Plain gain-per-cost greedy does not automatically
inherit it. The guarantee concerns the defined coverage objective, not answer
accuracy. Evidence that is useful only in combination can violate the surrogate's
assumptions; bundle it or measure that failure explicitly.

Current native budgeting estimates `ceil(UTF-8 JSON bytes / 4)`. It is not a
model-specific token count. Recheck the complete serialized packet after
selection; charge actual runtime token usage in the experiment. Variable omission
metadata and framing cannot silently become additive costs in a proof.

## Reduce unnecessary retrieval

Keep a ledger keyed by source revision, file digest, exact symbol identity,
observed range and result completeness. Include index/extractor identity for
graph results and current access/profile revisions for personal data. Reuse only
after the same authorization and freshness checks as a new read. A changed source,
revoked grant, ambiguous symbol or incomplete result requires a fresh observation.

The benchmark broker already keeps source bytes in memory. Caching its reply alone
does not reduce tokens if the full reply is sent to the model again. Savings
require avoiding repeated requests or selecting only the missing ranges. Preserve
original citations and explicit pagination; a short summary is not equivalent
source evidence. Compare bounded lookup batching with individual calls rather
than dumping a whole large file outline.

For later adaptive stopping, use the
[value-of-computation framework](https://proceedings.mlr.press/r10/hay12a.html):

```text
VOI(next read) = expected reduction in decision loss - cost of the read
```

That expectation needs a calibrated model of outcomes. Initially, stop only
redundant expansion after observed obligations are satisfied; keep unresolved
items explicit. Self-reported LLM confidence cannot establish completeness.
Required verification, regression checks and material unknowns remain protected.
This is a proposed stopping policy, not a new hidden time or token limit.

## Evolve agent DNA

[GEPA](https://arxiv.org/abs/2507.19457v2) uses textual feedback from execution and
evaluation traces to mutate individual prompts and retain candidates that excel
on different instances. Its instance-wise Pareto selection is not the same as
our product frontier over acceptance, tokens and latency; adding those objectives
is a Mastermind adaptation. It offers no guarantee of a globally optimal prompt
or improvement on a new repository.

Use a versioned configuration above the current model:

| Component | Allowed experiment | Protected boundary |
|---|---|---|
| Role instructions | Mutate one instruction module | Original request and required behavior |
| Refinement | Compare unchanged request with request plus frozen native refinement | Request remains visible; generation/delivery costs count |
| Retrieval policy | Change ordering and optional expansion | Scope, source verification, ambiguity and completeness metadata |
| Response instructions | Change structure, examples and relevant style application | No invented user preferences or unsupported certainty |
| Personal profile | Compare applicable reviewed profile, none and shuffled control | Quote/source/review revisions; existing ranked delivery prefix |
| Core control | Run regression and admission gates | No prompt mutation of permissions, evidence contracts or completion guards |

The loop is: freeze a parent → propose one module change → run matched development
tasks → inspect acceptance and resource outcomes → retain a non-dominated candidate
→ freeze it → qualify on unseen tasks. Split by repository/task family. Keep final
qualification keys unavailable to the optimizer. User preferences are reviewed
data; an optimizer's generated instruction cannot become a personal fact.

MIPRO is an alternative when the search mostly chooses combinations of prompts
and demonstrations; its paper and MIPROv2 implementation use task-grounded
proposals and surrogate/Bayesian search. Avoid introducing two optimizers before
one has beaten a simple search baseline.

Candidate trials can use the existing Codex subscription adapter. Reflection
needs an explicit compatible callback; installing DSPy is not a turnkey CLI
integration. Offline model calls still consume subscription usage. Charge
optimization separately and measure how many accepted tasks amortize its cost.

## Calibrate routing and profile advice

Use [role calibration](ROLE_CALIBRATION.md) for task families, role boundaries,
matched experiments and quality gates. Compare role instructions and reasoning
effort separately before evaluating handoffs and escalation.

### Allocate reasoning by difficulty

[Compute-optimal test-time scaling](https://arxiv.org/html/2408.03314v1) compares
revision and verifier-search strategies by model-specific difficulty on MATH.
Its estimated difficulty uses many samples and a learned verifier; the study
does not charge the full difficulty-estimation cost. It motivates a routing
experiment, not a latency or quality guarantee for our code tasks.

The experimental campaign policy `bounded_readonly_v1` is implemented in
[`benchmark/effort.py`](benchmark/effort.py). It uses the public request, explicit
read-only contract and source scope; recognized state, security and removal
markers retain `max`. Other bounded read-only requests use `high`. This is a
conservative English heuristic, not a learned or validated difficulty predictor.
It changes no production default and cannot infer output correctness.

Start with declared task features and a fallback to the existing reasoning
setting. Compare one lower-effort setting with the frozen default before
learning a router. Keep original acceptance, source constraints and all
attempts, charge any routing/review inference, and split unseen qualification
by task family. Keep time/output-token/turn limits disabled when testing effort;
an effort selection is not an output deadline. Explicitly bind setting changes
as experimental conditions rather than hiding them in a runtime environment.

[DiffAdapt](https://arxiv.org/abs/2510.19669v5) trains a probe on hidden states and
routes among prompt/temperature/token-limit configurations. The current Codex
stream exposes neither hidden states nor token probabilities. Direct use needs
a different runtime contract; adopting only its routing idea is a separate
Mastermind experiment. Its reported savings do not transfer to our subscription
workflow or justify restoring token caps.

### Guard routing and optional claims

Conservative linear bandits constrain cumulative expected reward relative to a
baseline under a specified linear reward/noise model. They do not guarantee that
every individual task is safe. Start with offline comparisons and a deterministic
fallback. Freeze features such as task family, known file/symbol locations and
missing evidence; log action probabilities before considering off-policy
evaluation. Our four calibration tasks cannot fit or qualify this policy.

Conformal risk control requires suitable calibration data and a bounded monotone
loss under exchangeability. For a fixed pool of labeled candidate claims, use
`L_j(lambda) = 1` if task `j` delivers any incorrect optional claim, otherwise
`0`. Increasing the threshold must produce nested shrinking delivery sets. The
calibration rule for this bounded loss is:

```text
lambda_hat = inf {lambda: (sum_j L_j(lambda) + 1) / (n + 1) <= alpha}
```

Use right-continuous losses and an abstention endpoint with zero loss. This
controls expected task-level risk, not precision conditional on delivery. Free
LLM answers need not satisfy monotonicity. Track abstention and coverage; empty
delivery cannot establish mining benefit. User/session drift can break
exchangeability. Source validity, authorship and human review remain separate.

## Qualify an improvement

Keep acceptance as a constraint rather than trading it for faster output in a
weighted score. For total observed resource `C` and accepted outcomes `S`, report
`C / S` and `S / C` separately for tokens and time, using every planned attempt.
Zero successes make relative useful-work efficiency undefined; missing outcomes
and resources retain bounds or unknowns. Preserve material-regression gates.

For a request-led retrieval experiment, maintain a frontier of requested facts,
their cited support and unresolved conditions. Choose the next scoped operation
to close a material gap, then check the completed answer against the unchanged
request. This is an instruction heuristic; it does not mechanically prove that
the model found every relevant branch. Do not hide the evaluation key in this
working set or infer quality from a smaller frontier or fewer tool calls.

Use the constrained objective `minimize (tokens / accepted, latency)` subject to
no paired acceptance loss or material error. This states the decision rule;
it is not a proof that the heuristic optimizes either cost on new tasks.

The current request-led policy is rejected in the retained
[calibration](baselines/evidence-frontier-20261008-public.json). Do not promote it or
remove evidence to meet a resource goal. Evaluate a different retrieval policy
or reasoning setting as a separately frozen condition; charge its own overhead
and check complete answers against the original criteria.

| Experiment | Arms | Additional observations |
|---|---|---|
| Retrieval | Existing route vs route + ledger | Unique relevant ranges, duplicates, tool rounds, errors and retrieval overhead |
| Context | Existing packet vs optional coverage selection | Required fitting cards, omissions, evidence recall at equal allowance and actual tokens |
| Refinement | Raw request vs request + frozen native refinement | Original acceptance, refinement tokens/time and delivery receipt |
| DNA instructions | Shipped modules vs one frozen mutation | Development/qualification split, reviewer corrections and search cost |
| Profile | None vs relevant reviewed vs shuffled | Same task/model; profile IDs, scope/source revisions and delivery receipt |

Use independent labels for audit findings and implemented fixes, including
negative and ambiguous cases. A fix needs an observed diff and a meaningful
regression failing on the defect. Add stage spans for preparation, native reads,
tool round trips and final answer before attributing wall-time changes to tokens.
First visible commentary remains separate from first-token latency.

At a fixed sample size, resample paired task units, keeping repeats together;
declare repository clustering when generalizing across repositories. An observed
p95 from eight attempts is descriptive, not a stable population tail estimate.
For repeated inspection of acceptance, select an appropriate bounded paired
confidence sequence and predeclare its assumptions, error level and admission
rule. Repeated ordinary confidence intervals do not provide that protection.
Confidence sequences do not automatically cover uncapped heavy-tailed latency
or token cost. Candidate selection and multiple comparisons need separate control.

## Implementation boundaries

| Owner | Responsibility |
|---|---|
| [`context.rs`](../mcp/servers/mmcg/src/context.rs), [`document_graph.rs`](../mcp/servers/mmcg/src/document_graph.rs) | Optional packet/document selection and explicit omissions |
| [`miner/profile.rs`](../mcp/servers/mmcg/src/miner/profile.rs) | Existing eligibility, ranking, verification and whole-card fitting |
| [`queries.rs`](../mcp/servers/mmcg/src/queries.rs), [`store.rs`](../mcp/servers/mmcg/src/store.rs) | Production symbol/graph/document retrieval |
| [`miner/hooks/refiner.rs`](../mcp/servers/mmcg/src/miner/hooks/refiner.rs) | Native refinement input, generation and delivery contract |
| [`benchmark/tools.py`](benchmark/tools.py) | Frozen-source benchmark broker and retrieval measurements |
| [`benchmark/retrieval.py`](benchmark/retrieval.py), [`retrieval_analysis.py`](benchmark/retrieval_analysis.py) | Bound returned-range accounting with complete and observed-only totals |
| [`benchmark/conditions.py`](benchmark/conditions.py), [`campaign.py`](benchmark/campaign.py) | Frozen candidate identity and matched runs |
| [`benchmark/analysis.py`](benchmark/analysis.py), [`efficiency.py`](benchmark/efficiency.py), [`value.py`](benchmark/value.py) | Paired outcomes, all-attempt costs and qualification reporting |

Keep production optimizers out of the benchmark package; the broker can evaluate
them without becoming their only implementation. Use small package-owned modules
and existing contracts. Add reusable interfaces only when multiple implementations
or packages actually need them.

Before changing delivery, run the selected 3.2.2 regressions described in
[PRODUCT.md](PRODUCT.md#preserve-the-profile-fixes). Preserve eligibility before
the verification cap, deterministic rank, ranked-prefix delivery, configurable
budgets, full cards and source/access rechecks. A learned score cannot bypass them.
For Python harness changes, run `python3 -m unittest discover -s tests -t .` and
`python3 scripts/validate.py`. Retain new experiment records at new paths; keep
earlier failures and rejected candidates unchanged.
