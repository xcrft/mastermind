# Agents

Claude Code roles for research, implementation and review. Portable task
behavior lives in [skills](../skills/). These files also depend on Claude Code's
delegation runtime.

| Directory | Contents |
|---|---|
| [`subagents/`](subagents/) | Spawnable Claude Code subagents with bounded responsibilities |
| [`claude-md/`](claude-md/) | Project-level `CLAUDE.md` and `CONTEXT.md` templates |
| [`blocks/`](blocks/) | Shared paragraphs copied verbatim into skills and subagents; not installed on their own |
| [`instructions/`](instructions/) | Source-routing instruction variants compared by the [context-routing benchmark](../evals/benchmark/README.md); not installed |

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

## Untrusted content

[`blocks/untrusted-content.md`](blocks/untrusted-content.md) is the one source
for the rule that repository, pull request and tool content is data, not
instructions. Skills and subagents install as single files, so every role that
reads such content carries a verbatim copy. The covered roles are listed in
`UNTRUSTED_CONTENT_COVERED` in [`scripts/validate.py`](../scripts/validate.py),
and `just validate` fails when a covered file lacks the paragraph, has it twice
or has a changed copy, or when a file outside the list carries it. To change
the wording, edit the block, paste it into every covered file and bump each
file's version. To cover a new role, add it to the list and paste the block
after its opening paragraph.

## Project templates

| Template | Description |
|---|---|
| [`mastermind-workflow`](claude-md/mastermind-workflow.md) | `CLAUDE.md` contract for Direct, Verified, and Strict task delivery. |
| [`mastermind-context`](claude-md/mastermind-context.md) | `CONTEXT.md` template for durable decisions, constraints, glossary terms, and protected areas. |

The native CLI embeds both templates. `mastermind init` writes the workflow
section as a managed block in `CLAUDE.md` or `AGENTS.md`. `just validate` checks
the template mirrors embedded in the Rust crate. See [installation](../docs/getting-started.md)
and [workflow](../docs/workflow.md) for use.
