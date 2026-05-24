# Agents

Configurations that shape **how an agent behaves** in a project or session — distinct from skills (capabilities) and prompts (instruction blocks).

| Sub-folder | What it is |
|---|---|
| [`subagents/`](subagents/) | Specialized agents the main agent can spawn (Claude Code subagent format) |
| [`claude-md/`](claude-md/) | `CLAUDE.md` templates for common project shapes |
| [`hooks/`](hooks/) | Hook configurations and scripts for `settings.json` (empty — contribute one!) |
| [`settings/`](settings/) | Composable `settings.json` snippets (empty — contribute one!) |

See [`../docs/agent-anatomy.md`](../docs/agent-anatomy.md) for the format of each.

## Index

### subagents/
| Subagent | Description |
|---|---|
| [`mastermind-critic`](subagents/mastermind-critic.md) | Independent Opus-tier design-time challenger. Spawned by the planner during brainstorming (mandatory for auth/billing/migrations/public APIs) to stress-test the proposed approach BEFORE it becomes a spec. Returns 3 concrete weaknesses + verdict. Paired with `mastermind-auditor` (same tier, post-execution phase). |
| [`mastermind-task-executor`](subagents/mastermind-task-executor.md) | Executes a `.mastermind/tasks/XXX-*.md` spec produced by the Mastermind planner. Pair with the [`mastermind-workflow`](claude-md/mastermind-workflow.md) CLAUDE.md template. |
| [`mastermind-prompt-refiner`](subagents/mastermind-prompt-refiner.md) | Read-only subagent that refines a rough user prompt and returns a clean version ready to pass to the next agent. Optional preprocessor in the Mastermind workflow. |
| [`mastermind-researcher`](subagents/mastermind-researcher.md) | Haiku-tier read-only fact-gatherer. Spawned by the planner for bulk grep/read/glob work — returns structured citations, never decides. Completes the Opus/Sonnet/Haiku tier hierarchy. |
| [`mastermind-auditor`](subagents/mastermind-auditor.md) | Opus-tier independent post-flight auditor. Spawned by the planner after the executor returns — mechanically verifies report claims against `git diff`, re-runs cheap VERIFY commands, checks mmcg consistency. Adversarial to the report. Closes confirmation bias in the post-flight gate. |

### claude-md/
| Template | Description |
|---|---|
| [`mastermind-workflow`](claude-md/mastermind-workflow.md) | `CLAUDE.md` that pre-wires the planner+executor delegation workflow — drop-in setup for projects using `.mastermind/tasks/` specs. |
| [`mastermind-context`](claude-md/mastermind-context.md) | `CONTEXT.md` template — project-level institutional memory (decision log, gotchas, glossary, don't-touch). Lives at project root alongside CLAUDE.md; updated by the planner during post-flight semantic review. |

---

## Adding a new agent config

1. Read [`../docs/agent-anatomy.md`](../docs/agent-anatomy.md) — pick the right sub-category first.
2. Copy the matching template from `_template/`.
3. Fill it in. Test it in a real project.
4. Add to this index.
5. Open a PR.
