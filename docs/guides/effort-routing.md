# Route reasoning effort for a new agent

With this guide you pick the reasoning effort for an agent before it starts and
apply it through the client's own control. The CLI and MCP share one fixed Rust
policy, and a decision needs neither an index nor a model call.

The core workflow does this for you. The
[`mastermind-agent-handoff`](../../skills/workflow/mastermind-agent-handoff/SKILL.md)
skill runs before each new agent in planning, research and delegated execution.
Use this guide to route by hand or to check what the skill did.

## How the policy decides

| Condition | Result |
|---|---|
| Explicit human `user_effort` for this invocation | That level, ahead of automatic routing |
| Researcher with every declaration in the next table | `low`, as a pilot |
| Anything else, including unknown duty, incomplete scope or high or unknown risk | `high` |

| Required for automatic low | Declaration |
|---|---|
| Source-only research | `agent: researcher` or `mastermind-researcher`, `read_only: true` |
| Allowed task | `task_family: callees_trace`, `source_read_reuse` or `task_phase_trace` |
| Bounded source scope and stable inputs | `bounded_scope: true`, `stable_inputs: true` |
| The `mastermind repo-map` shortlist at a 1600-byte budget, with its fixed advice text | `lexical_repo_map: true` |
| Inspected low risk | `risk: low` |

The policy trusts these declarations and checks none of them. The low route was
measured with the exact `repo-map` advice text and budget, so a changed text or
budget makes that condition false. Execution, review, security, removal proofs
and normalization boundaries always get high. Route again when the duty, risk
or source scope changes, not on every lookup.

The low pilot rests on development runs with GPT-6.1 Sol on 8 tasks, where low
and high effort showed no detectable acceptance difference, see the
[statistical reanalysis](../../evals/baselines/statistical-reanalysis-20261015.md).
Other models and task families are not qualified.

## Get a decision

1. Write `route.json` for the receiving agent:

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

2. Ask for the decision:

   ```bash
   mastermind effort-route --request route.json
   ```

   You get `requested_effort`, a reason, and argument arrays for each client:
   `codex_spawn_args`, `codex_args` and `claude_args`. The MCP tool
   `mmcg_effort_route` takes the same object.

Omitted booleans default to false and risk to unknown. `user_effort` takes
`low`, `medium`, `high`, `xhigh` or `max`, only for an explicit human choice.
Unknown fields and invalid values fail. The request is limited to 16 KiB, and
agent and family names to 80 UTF-8 bytes.

## Apply it

A decision changes nothing on its own. A prompt or MCP response can't
reconfigure its caller or a running child, so apply it to the new agent:

| Client | Control | Note |
|---|---|---|
| Codex `spawn_agent` | Merge `codex_spawn_args`: `fork_turns: "none"`, `reasoning_effort` | The child starts fresh, so pass it a self-contained task packet. A full-history fork keeps the parent's effort and counts as `not_applied` |
| Codex CLI | `codex_args`, such as `-c 'model_reasoning_effort="low"'` | Applies to the new session |
| Claude Code CLI | `claude_args`, such as `--effort low` | Agent definitions and environment settings can override session effort |
| Claude Code non-fork `Agent`, 2.1.292 or later | `effort: requested_effort` | `CLAUDE_CODE_EFFORT_LEVEL` takes precedence |
| Older client or another host | Its own per-agent field, if it has one | Otherwise keep the defaults and record `not_applied` |

Keep the model, permissions and budgets as they are, and keep the request, the
decision and the child ID together. Custom Codex agent files can override spawn
settings, so don't set a conflicting fixed effort there.

To launch a native client with the override in one step, pass `--exec`. The
request must come from a file, because stdin goes to the client. Arguments
after `--` go to the client without shell expansion:

```bash
mastermind effort-route --request route.json --exec codex -- exec --sandbox read-only - < task.txt
mastermind effort-route --request route.json --exec claude -- --print --tools Read,Grep,Glob < task.txt
```

The client comes from PATH and inherits your working directory, configuration
and authentication. stdout is the client output. stderr gets a JSON route
record with `override_submitted: true` and `served_effort: null`. Exit codes
0-255 pass through, and signal exits or wider codes return 1. A failed run is
not retried at another effort.

## Check that it applied

The decision always says `applied: false`, `declarations_verified: false` and
`quality_qualified: false`, because it checks none of those. For Codex, the
child's own `turn_context.effort` shows the setting it actually got, and the
parent's record is not enough. Provider behavior and answer quality need a
provider record or a matched answer evaluation.

## When it fails

| Condition | What to do |
|---|---|
| Invalid input or missing client | Fix the request or PATH. No child has started |
| A native effort flag or `CLAUDE_CODE_EFFORT_LEVEL` conflicts | Put the human choice in `user_effort` and remove the conflict for this invocation |
| Another Codex dispatcher, such as TokenX, rejects explicit `reasoning_effort` | Pick one effort authority. The handoff skill stops before spawn until you do. To keep Mastermind, disable that dispatcher's non-managed hooks in `/hooks` and start a new session, see [Codex hook controls](https://developers.openai.com/codex/hooks#review-and-trust-hooks) |
| Full-history fork, older Claude Code or unsupported host | Keep the client's setting and record `not_applied` |
| Authentication or transport failure | Keep the failed attempt, fix the cause, then retry by hand |
| You want to turn off automatic low | Send `user_effort: high`, or drop the routing step |

Client controls are documented in
[Codex subagent configuration](https://developers.openai.com/codex/multi-agent),
[Claude subagent effort](https://code.claude.com/docs/en/sub-agents#choose-an-effort-level)
and [Claude effort precedence](https://code.claude.com/docs/en/model-config#adjust-effort-level).
