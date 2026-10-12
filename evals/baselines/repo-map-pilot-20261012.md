# Repo-map pilot

The prototype reduced uncached input on the completed answers, while increasing
reported context and tool use. Keep it experimental: two tasks and model-only
reviews do not establish a product benefit or justify a production default.

## Intervention and inputs

The [Rust prototype](../experiments/repo-map/README.md) ranks frozen indexed
definitions by overlap with the original task question. It normalizes identifier
words and places declared tests after other definitions. PageRank was not tested.
The header contains source locations and abbreviated signatures, with a complete
4,000-UTF-8-byte payload budget. This is not an exact model token budget.

Both conditions use the same 20 source files at
`f9c336c306d3c2a81dcbd8ba6e50bd6ce8bafa90`, native runtime, tools, common instructions,
requested `gpt-6.1-sol/high` and existing Codex subscription. Baseline receives no
map; the candidate receives the map in its supplied first prompt. Both retain
source reading. The maps contain 17 and 22 selected definitions out of 2,225
candidates. Their payloads are 3,987 and 3,993 bytes, respectively.

Two new questions cover source-read receipts and concept matching. Each repeats
twice in reversed condition order; two case workers run concurrently. These are
development tasks, not a held-out qualification corpus. The map uses no rubric,
answer, expected claim or reviewer verdict.

The first CPU-only map favored test names. Its inputs were retained and ranking
was revised before any model invocation. Ranking and producer prompts were then
frozen. The standalone pilot uses the existing frozen benchmark adapter; the
benchmark harness and production MCP catalog were not changed.

## Completed-answer measurements

These totals describe four completed answer attempts in each condition. They
include the map in model input and subsequent exploration. They exclude a failed
capacity attempt whose token usage is unreported; they are not full resource cost
per accepted task slot.

| Metric | Baseline | Map | Change |
|---|---:|---:|---:|
| Uncached input tokens | 308,056 | 256,753 | −16.7% |
| Cached input tokens | 1,524,608 | 1,826,688 | +19.8% |
| Output tokens, including reasoning | 16,037 | 16,676 | +4.0% |
| Total reported tokens | 1,848,701 | 2,100,117 | +13.6% |
| MCP calls | 142 | 174 | +22.5% |
| Sum of completed-attempt seconds | 662.0 | 1,269.3 | +91.7% |
| Blind model-declared satisfied outcomes | 3/4 | 4/4 | One additional satisfied declaration |
| Completed human reference judgments | 0/4 | 0/4 | Pending |

The map reduced outline calls (4→2), but increased source searches (34→53) and
source reads (91→108). It supplied navigation hints without reducing total
exploration in this sample. Three of four completed pairs reduced uncached input;
the second receipt pair increased it. Do not generalize the aggregate reduction
to every task.

Elapsed time includes startup, model, transport and provider queuing. Capacity
errors occurred during the experiment. Including the failed producer invocation
and setup failures gives 1,755.3 baseline seconds versus 1,269.7 map seconds—the
opposite aggregate ordering. Neither timing comparison isolates the map's effect.
Map generation and index preparation times were not measured. Model round counts
remain unknown; the CLI `turns` counter is not interpreted as retrieval rounds.
Subscription billing and served model/effort remain unobserved.

## Blind review and the quality boundary

Each answer was sealed before its review and assessed in a fresh source-only
session requesting GPT 6.1 Sol. Review prompts expose one answer, the original
request, acceptance criteria and frozen source. Condition labels, map payloads,
resource totals and previous verdicts are withheld. The reviewer is the same model
family requested for answer production; it is not an independent human reference.

All eight final assessments have validated exact answer excerpts and source quote
anchors. Those checks establish bindings, not semantic correctness. Valid verdicts
were never resampled. Seven already-sealed answers were reviewed while the final
producer slot was still pending; the eighth was appended in a retained second
review phase. No review changed producer prompts or map selection.

The model rejected one baseline answer for omitting the separately enforced limit
of 128 newly delivered segments. The answer described the 200-new-line limit,
128 retained ranges and 128 cached receipts, but omitted the segment limit.
The frozen `source_read.rs:174` guard checks both new-line count and segment count.
Human adjudication is needed to confirm how that omission affects acceptance.

The unanswered, condition-hidden human packet is retained locally at
`.mastermind/research/repo-map-pilot-20261012/human-review/`. It contains all eight
answers, questions, source files and an empty assessment template. Its `human`
role is a requested reference role; no human scores have been created.

## Failure denominator and recovery

Eight task slots produced eight final answers through nine model sessions. One
producer attempt ended with explicit `503`/model-capacity failure after partial
source exploration. Its result and raw stream are retained. One additional fresh
attempt used the same canonical request and produced the final answer. Its failed
predecessor has unknown token usage, so complete-attempt token efficiency remains
unknown.

Eight earlier setup attempts failed before model invocation: the pilot wrapper
omitted Python isolation flags and created extra bytecode files inside the frozen
adapter bundle. The wrapper was fixed to use `-I -S -B`, with all eight request
identities unchanged. Original failures and both controller versions are retained.
Producer transport denominator: 17 attempts, including those eight setup failures.

Blind review produced eight valid assessments through nine fresh sessions. One
capacity-failed review was retried once with identical request content. The failed
review remains in the retained denominator. Provider capacity did not trigger a
model or effort change.

## Reproduction, checks and decision

[Bound per-attempt measurements](repo-map-pilot-20261012.json) retain source
inventories, runtime identities, all eight final answer identities, failures and
unknowns. Local raw records, input/answer seals, frozen implementations, reviews
and the human packet are under `.mastermind/research/repo-map-pilot-20261012/`.

With those retained artifacts, regenerate the descriptive report from the
repository root:

```sh
python3 .mastermind/research/repo-map-pilot-20261012/summarize.py
```

Do not rerun a valid producer answer or reviewer verdict to improve its outcome.
Future interventions need a new frozen input revision and retained failures.

Checks passed: two Rust behavior/boundary tests, formatting, Clippy with warnings
denied, source/index/request/answer integrity checks and the project validator
(40 artifacts, zero errors or warnings). Python benchmark harness code was not
changed. These checks do not establish semantic acceptance.

**Decision:** retain the prototype and measurements; keep production defaults
unchanged. The uncached-input reduction and model-declared acceptance difference
are useful hypotheses. The higher context and retrieval cost, missing failure
usage and pending human reference prevent qualification. Any next selection or
budget change should be frozen as a new intervention, then tested on new tasks.
