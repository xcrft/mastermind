# Agents

Claude Code roles for research, implementation and review. Portable task
behavior lives in [skills](../skills/). These files also depend on Claude Code's
delegation runtime.

| Directory | Contents |
|---|---|
| [`subagents/`](subagents/) | Spawnable Claude Code subagents with bounded responsibilities |
| [`claude-md/`](claude-md/) | Project-level `CLAUDE.md` and `CONTEXT.md` templates |

## Choose a role

| Subagent | Description |
|---|---|
| [`mastermind-prompt-refiner`](subagents/mastermind-prompt-refiner.md) | Rewrites prompts or prepares handoffs while retaining the original request. |
| [`mastermind-critic`](subagents/mastermind-critic.md) | Challenges a proposed design before the spec is written. |
| [`mastermind-investigator`](subagents/mastermind-investigator.md) | Investigates competing causes and records supporting or conflicting evidence. |
| [`mastermind-researcher`](subagents/mastermind-researcher.md) | Collects source facts and citations without changing files. |
| [`mastermind-task-executor`](subagents/mastermind-task-executor.md) | Implements an approved task and writes its executor report. |
| [`mastermind-auditor`](subagents/mastermind-auditor.md) | Checks executor claims against the diff and codegraph. |
| [`mastermind-comment-auditor`](subagents/mastermind-comment-auditor.md) | Reviews added comments and removed rationale. |
| [`mastermind-frontend-auditor`](subagents/mastermind-frontend-auditor.md) | Reviews component usage, props contracts, duplication and raw values. |
| [`mastermind-test-auditor`](subagents/mastermind-test-auditor.md) | Reviews test relevance, assertions and uncovered behavior. |
| [`mastermind-feedback-collector`](subagents/mastermind-feedback-collector.md) | Collects stated preferences as quoted candidates for profile review. |
| [`mastermind-security-auditor`](subagents/mastermind-security-auditor.md) | Reviews security-sensitive boundaries with optional OWASP ASI mapping. |

## Project templates

| Template | Description |
|---|---|
| [`mastermind-workflow`](claude-md/mastermind-workflow.md) | `CLAUDE.md` contract for Direct, Verified, and Strict task delivery. |
| [`mastermind-context`](claude-md/mastermind-context.md) | `CONTEXT.md` template for durable decisions, constraints, glossary terms, and protected areas. |

The npm workflow bundle includes these files. `just validate` checks the
template mirrors embedded in the Rust crate. See [installation](../docs/getting-started.md)
and [workflow](../docs/workflow.md) for use.
