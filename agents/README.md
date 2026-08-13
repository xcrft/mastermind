# Agents

Agent definitions are Claude Code-specific orchestration roles. Portable
capabilities live under [`skills/`](../skills/); do not copy an agent file into
another client and assume equivalent delegation behavior.

| Directory | Contents |
|---|---|
| [`subagents/`](subagents/) | Spawnable Claude Code subagents with bounded responsibilities |
| [`claude-md/`](claude-md/) | Project-level `CLAUDE.md` and `CONTEXT.md` templates |

## Index

## Subagents
| Subagent | Description |
|---|---|
| [`mastermind-prompt-refiner`](subagents/mastermind-prompt-refiner.md) | Normalizes explicit prompt rewrites or cold-agent handoffs while preserving the original request. |
| [`mastermind-critic`](subagents/mastermind-critic.md) | Pre-spec design challenger. Stress-tests a proposed approach, returns 3 weaknesses + verdict. |
| [`mastermind-investigator`](subagents/mastermind-investigator.md) | Debugging investigator that tracks competing hypotheses and evidence before confirming a cause. |
| [`mastermind-researcher`](subagents/mastermind-researcher.md) | Read-only fact gathering with file and line citations; does not make design decisions. |
| [`mastermind-task-executor`](subagents/mastermind-task-executor.md) | Executes an approved task by acceptance criteria, uses bounded in-scope repair, and writes the canonical report. |
| [`mastermind-auditor`](subagents/mastermind-auditor.md) | Post-flight auditor. Verifies executor report claims against `git diff` and mmcg. |
| [`mastermind-comment-auditor`](subagents/mastermind-comment-auditor.md) | Post-implementation comment reviewer. Flags added narration with quoted evidence and reports deleted rationale. |
| [`mastermind-frontend-auditor`](subagents/mastermind-frontend-auditor.md) | Post-implementation UI reviewer. Uses the codegraph for unrendered components, props-contract breaks, duplicates, and raw values. |
| [`mastermind-test-auditor`](subagents/mastermind-test-auditor.md) | Post-implementation test reviewer. Uses `mmcg_test_impact` classifications to separate real coverage from a filename match. |
| [`mastermind-security-auditor`](subagents/mastermind-security-auditor.md) | Independent security reviewer. Spawned only on security-sensitive scope (auth, tools, secrets, delegation, supply chain, prompt injection); optional OWASP ASI mode. |

## Project templates
| Template | Description |
|---|---|
| [`mastermind-workflow`](claude-md/mastermind-workflow.md) | `CLAUDE.md` contract for Direct, Verified, and Strict task delivery. |
| [`mastermind-context`](claude-md/mastermind-context.md) | `CONTEXT.md` template for durable decisions, constraints, glossary terms, and protected areas. |

The npm package stages these files into its installable workflow bundle.
`scripts/validate.py` checks the canonical template mirrors embedded in the Rust
crate.
