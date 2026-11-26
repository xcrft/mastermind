# Task and agent effort routing

Select effort before starting an agent, then apply the decision through the
host's native control. CLI and MCP share one deterministic Rust policy, and
selection needs neither an index nor a model call.

## Prerequisites

- Install the Mastermind workflow and register MCP, or have `mastermind` on PATH.
- Inspect the receiving agent's duty, source scope and risk.
- Use a host that supports per-invocation effort and an authenticated client for execution.
- Resolve any competing effort dispatcher before spawning an agent.

Routing uses declarations from the controller. It does not inspect the task or
verify those declarations. Keep the selected model, permissions and budgets.

## Choose effort

| Condition | Result |
|---|---|
| Explicit human `user_effort` for this invocation | Requested override, ahead of automatic routing |
| Researcher on an allowed family, with every low condition below | `low`, as a pilot (see below) |
| Other or unknown duty/family, incomplete scope, high or unknown risk | `high` |

| Required for automatic low | Controller declaration |
|---|---|
| Source-only research | `agent: researcher` or `mastermind-researcher`, `read_only: true` |
| Allowed task | `task_family: callees_trace`, `source_read_reuse` or `task_phase_trace` |
| Bounded source scope and stable inputs | `bounded_scope: true`, `stable_inputs: true` |
| Lexical map at a 1600-byte budget, with its fixed advice text unchanged | `lexical_repo_map: true` |
| Inspected low risk | `risk: low` |

The lexical map is the `mastermind repo-map` shortlist given to the researcher.
Its header carries fixed advice to read the cited source directly and to check
the ranges the map does not cover. The low route was measured with exactly that
text, so changing it, or the 1600-byte budget, makes the condition false.

Execution, review, security, removal proofs and normalization boundaries select
high. A phase trace explains an input contract, while changing or approving
workflow state is a different duty. When duty, risk or source scope changes,
route a new handoff with the existing facts and gaps. Do not route per lookup.

## Select a decision

Create `route.json` for an eligible source-reuse request:

```json
{
  "agent": "researcher",
  "task_family": "source_read_reuse",
  "read_only": true,
  "bounded_scope": true,
  "stable_inputs": true,
  "lexical_repo_map": true,
  "risk": "low"
}
```

```bash
mastermind effort-route --request route.json
```

Alternatively, pass that object to `mmcg_effort_route`. Omitted booleans default
to false and risk to unknown. `user_effort` accepts `low`, `medium`, `high`,
`xhigh` or `max`. Use it only for an explicit human choice. Unknown fields,
invalid types and unrecognized values fail validation. CLI input is limited to
16 KiB, and agent and family names to 80 UTF-8 bytes.

## Apply to an ordinary agent

Mastermind planning, research and executor workflows use
[`mastermind-agent-handoff`](../../skills/workflow/mastermind-agent-handoff/SKILL.md)
before each new agent. The controller must:

1. Select the decision for the receiving agent's actual task.
2. Apply the returned setting through the native control below.
3. Carry role instructions, objective, authorized scope, facts, citations, remaining obligations, source/map bindings, permissions and budgets into the child.
4. Retain the request, decision, child ID and runtime evidence together.

| Host | Native control | Constraint |
|---|---|---|
| Codex `spawn_agent` | Merge returned `codex_spawn_args`: `fork_turns: "none"`, `reasoning_effort: requested_effort` | Fresh child with a self-contained packet. Full-history forks inherit the parent's effort |
| Codex CLI | Returned `codex_args`, such as `-c 'model_reasoning_effort="low"'` | Applies to the new native session |
| Claude Code CLI | Returned `claude_args`, such as `--effort low` | Agent definitions and environment settings can override session effort |
| Claude non-fork `Agent`, 2.1.292 or later | `effort: requested_effort` | `CLAUDE_CODE_EFFORT_LEVEL` takes precedence |
| Another host | Supported per-agent field | Record `not_applied` if the control is unavailable |

Keep any existing model choice. Custom Codex agent files can override spawn
settings, so don't set a conflicting fixed effort there. See
[Codex subagent configuration](https://developers.openai.com/codex/multi-agent)
and [Claude subagent effort](https://code.claude.com/docs/en/sub-agents#choose-an-effort-level).
A prompt or MCP response cannot reconfigure its caller or a running child.

## Launch a native client

`--exec` selects and submits the override together. It requires a request file,
leaving stdin for the client's prompt. Arguments after `--` pass directly to the
client, without shell expansion:

```bash
mastermind effort-route --request route.json --exec codex -- exec --sandbox read-only - < task.txt
mastermind effort-route --request route.json --exec claude -- --print --tools Read,Grep,Glob < task.txt
```

The launcher resolves the client from PATH and inherits its working directory,
configuration and authentication. stdout contains client output. stderr includes
a JSON route record after spawn: `override_submitted: true`, `served_effort: null`.
Capture it with the result. Exit codes 0-255 are preserved. Signal exits and wider
codes return 1. A nonzero exit fails the command without retrying at another effort.

## Verify application

| Evidence | Meaning |
|---|---|
| `requested_effort` and reason | Policy decision |
| `applied: false`, `declarations_verified: false`, `quality_qualified: false` | Selection verifies neither the declarations, application nor answer quality |
| Native setting submitted | Requested configuration reached the host |
| Codex child's own `turn_context.effort` | Observed native child setting. The parent's record is not enough |
| Provider record or matched answer evaluation | Separate evidence for provider behavior or semantic quality |

Automatic low is a pilot. It is on only for the narrow researcher case above,
and its only evidence is development runs with GPT-6.1 Sol on 8 tasks, where
low and high effort showed no detectable acceptance difference (see the
[statistical reanalysis](../../evals/baselines/statistical-reanalysis-20261015.md)). To qualify other models or
more task families, use matched inputs, instructions and tools, independent
answer review, and keep failed and unknown outcomes. Routing and transport
checks say nothing about quality or savings.

## Recover or disable

| Condition | Action |
|---|---|
| Invalid input or missing client | Fix the request or PATH. No child has started |
| Conflicting native effort flag or `CLAUDE_CODE_EFFORT_LEVEL` | Put the human choice in `user_effort` and remove the conflict for this invocation |
| Codex full-history fork required, older Claude, or unsupported host | Keep the host's setting and record `not_applied` |
| Dispatcher forbids effort overrides | Stop before spawn and select one authority. See [Codex integration](../integrations/codex.md#route-reasoning-effort) |
| Authentication or transport failure | Retain the failed attempt, fix the cause, then explicitly retry |
| Disable automatic low | Use an explicit `user_effort: high` for new invocations or remove the routing step |

See [Claude effort precedence](https://code.claude.com/docs/en/model-config#adjust-effort-level)
for client controls. Preserve existing results when changing routing. Historical
grades remain bound to their original invocation.
