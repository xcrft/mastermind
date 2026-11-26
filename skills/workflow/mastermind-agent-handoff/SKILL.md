---
name: mastermind-agent-handoff
description: Use for ordinary in-session research, implementation and review handoffs. Not for rewriting the request itself (use mastermind-prompt-refiner). Routes effort and carries bounded context before every new agent in a Mastermind workflow.
metadata:
  version: 0.1.3
  authors: [mastermind]
  tags: [workflow, delegation, effort]
---

# Mastermind agent handoff

Before each new agent in a Mastermind workflow, select and apply effort once.
This step also applies to Direct work when it delegates. It grants no authority
to delegate, edit, approve or publish beyond the current task.

Check the active dispatch rules first. A dispatcher that forbids effort
overrides, such as TokenX assignment routing, cannot share this setting with
Mastermind. Stop the handoff with `not_applied` until the caller selects one
effort authority. Do not launch a child at inherited effort after detecting
this conflict or bypass its guard. Keep the user's selected model.

## Select

Call `mmcg_effort_route` with the receiving agent's actual duty, task family,
source-only/bounded/stable scope, lexical-map condition and inspected risk.
If MCP is unavailable, use `mastermind effort-route` without `--exec` and parse
its JSON. Missing or unknown context selects high; do not invent declarations
to obtain low. Set `user_effort` only for an explicit human choice.

Low requires researcher duty, `callees_trace`, `source_read_reuse` or
`task_phase_trace`, all four scope conditions, inspected low risk, and a lexical
map capped at 1600 bytes with the frozen direct-read/uncovered-range instruction.
Execution, review, security and unknown duties select high. The low pilot has
GPT-6.1 Sol quality evidence only; other model profiles remain uncalibrated.

## Apply to the ordinary spawn

| Host | Per-agent setting |
|---|---|
| Codex `spawn_agent` | Pass returned `codex_spawn_args`: `fork_turns: "none"` and `reasoning_effort: requested_effort`. Preserve any existing model choice; do not add a model override just to route effort. |
| Claude non-fork `Agent`, 2.1.292 or later | Pass `effort: requested_effort`; a conflicting environment override prevents application. |
| Another host | Use its supported per-agent control; record `not_applied` when unavailable. |

Codex full-history forks inherit the parent's effort and cannot accept this
override. Use a fresh child with a self-contained packet: role instructions,
original objective, authorized scope, relevant facts and source citations,
remaining obligations, source/map bindings, permissions and budgets. Include
the router's request, decision and reason as evidence; prose is not the setting.
Do not reconstruct omitted history from guesses.

For example, merge these returned fields into the ordinary Codex spawn call:

```json
{"fork_turns": "none", "reasoning_effort": "low"}
```

Keep the host's selected model, tool permissions and task budget. Custom-agent
configuration can override spawn settings; do not use an agent configuration
with a conflicting fixed effort. If the caller requires a full-history fork or
the host lacks this control, retain its existing setting and record
`not_applied` with the reason. That handoff uses the host's defaults; do not
claim that routing applied or that a prompt changed effort.

## Verify and continue

Retain the decision beside the child ID and native runtime record. A submitted
setting is distinct from observed native effort and provider-internal behavior.
For Codex, check the child's own `turn_context.effort`, rather than the parent's.
For unsupported hosts, keep the limitation explicit.

If duty, risk or source scope changes, end that handoff and route a new one with
the retained facts and gaps. Do not re-route per lookup or blindly restart an
agent after a transport/authentication failure. Answer quality and savings
require separate matched evaluations.
