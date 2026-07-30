# Agents

Configurations that shape **how an agent behaves** in a project or session — distinct from skills (capabilities) and prompts (instruction blocks).

| Sub-folder | What it is |
|---|---|
| [`subagents/`](subagents/) | Specialized agents the main agent can spawn (Claude Code subagent format) |
| [`claude-md/`](claude-md/) | `CLAUDE.md` templates for common project shapes |

## Index

### subagents/
| Subagent | Description |
|---|---|
| [`mastermind-prompt-refiner`](subagents/mastermind-prompt-refiner.md) | Normalizes explicit prompt rewrites or cold-agent handoffs while preserving the original request. |
| [`mastermind-critic`](subagents/mastermind-critic.md) | Pre-spec design challenger. Stress-tests a proposed approach, returns 3 weaknesses + verdict. |
| [`mastermind-researcher`](subagents/mastermind-researcher.md) | Haiku-tier fact-gatherer. Runs grep/read/glob and returns structured citations, never decides. |
| [`mastermind-task-executor`](subagents/mastermind-task-executor.md) | Executes an approved task by acceptance criteria, uses bounded in-scope repair, and writes the canonical report. |
| [`mastermind-auditor`](subagents/mastermind-auditor.md) | Post-flight auditor. Verifies executor report claims against `git diff` and mmcg. |
| [`mastermind-comment-auditor`](subagents/mastermind-comment-auditor.md) | Post-implementation comment reviewer. Flags added narration with quoted evidence and reports deleted rationale. |
| [`mastermind-frontend-auditor`](subagents/mastermind-frontend-auditor.md) | Post-implementation UI reviewer. Uses the codegraph for unrendered components, props-contract breaks, duplicates, and raw values. |
| [`mastermind-test-auditor`](subagents/mastermind-test-auditor.md) | Post-implementation test reviewer. Uses `mmcg_test_impact` classifications to separate real coverage from a filename match. |
| [`mastermind-security-auditor`](subagents/mastermind-security-auditor.md) | Independent security reviewer. Spawned only on security-sensitive scope (auth, tools, secrets, delegation, supply chain, prompt injection); optional OWASP ASI mode. |

### claude-md/
| Template | Description |
|---|---|
| [`mastermind-workflow`](claude-md/mastermind-workflow.md) | `CLAUDE.md` that pre-wires the planner+executor delegation workflow — drop-in setup for projects using `.mastermind/tasks/` specs. |
| [`mastermind-context`](claude-md/mastermind-context.md) | `CONTEXT.md` template — project-level institutional memory (decision log, gotchas, glossary, don't-touch). Lives at project root alongside CLAUDE.md; updated by the planner during post-flight semantic review. |
