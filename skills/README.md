# Skills

Portable task capabilities included in the npm workflow bundle. Use the skill
that matches the work. Claude Code-specific roles are listed in
[agents](../agents/). `mastermind init` installs them, and
`mastermind update --workflow-only` refreshes them, see
[getting started](../docs/getting-started.md).

`just validate` checks this catalogue and bundle staging against the shipped
`SKILL.md` files.

## Move a task from request to evidence

| Skill | Description |
|---|---|
| [`mastermind-agent-handoff`](workflow/mastermind-agent-handoff/SKILL.md) | Selects effort and applies it to ordinary agent spawns with a bounded context packet. |
| [`mastermind-task-planning`](workflow/mastermind-task-planning/SKILL.md) | Selects Direct, Verified or Strict and writes the task contract. |
| [`mastermind-task-executor`](workflow/mastermind-task-executor/SKILL.md) | Implements the approved contract and writes `executor-report.md`. |
| [`mastermind-codegraph-research`](workflow/mastermind-codegraph-research/SKILL.md) | Uses mmcg to discover symbols and inspect repository structure. |
| [`mastermind-component-research`](workflow/mastermind-component-research/SKILL.md) | Finds existing React/Vue components, their consumers and props contracts. |
| [`mastermind-structured-report-contract`](workflow/mastermind-structured-report-contract/SKILL.md) | Defines the executor report consumed by postflight. |
| [`mastermind-critical-review`](workflow/mastermind-critical-review/SKILL.md) | Reviews assumptions, scope, evidence and failure modes in a design or report. |
| [`mastermind-product-intake`](workflow/mastermind-product-intake/SKILL.md) | Turns a PRD or ticket into testable criteria and unresolved product questions. |
| [`mastermind-runtime-research`](workflow/mastermind-runtime-research/SKILL.md) | Traces service consumers, state writers and runtime evidence gaps. |
| [`mastermind-architecture-review`](workflow/mastermind-architecture-review/SKILL.md) | Reviews runtime paths, state ownership, retries and compatibility. |
| [`mastermind-project-history`](workflow/mastermind-project-history/SKILL.md) | Retrieves prior decisions with sources and evidence limits. |
| [`mastermind-project-map`](workflow/mastermind-project-map/SKILL.md) | Maps components and dependencies with explicit graph limits. |
| [`mastermind-change-impact`](workflow/mastermind-change-impact/SKILL.md) | Traces changed files and symbols to potential downstream impact. |
| [`mastermind-test-impact`](workflow/mastermind-test-impact/SKILL.md) | Finds candidate tests from changed symbols and graph evidence. |
| [`mastermind-cross-client-setup`](workflow/mastermind-cross-client-setup/SKILL.md) | Previews and installs workflows for supported clients. |
| [`mastermind-audit-attestation`](workflow/mastermind-audit-attestation/SKILL.md) | Checks audit integrity, signer provenance and acceptance policy separately. |
| [`mastermind-style-deep`](workflow/mastermind-style-deep/SKILL.md) | Drafts a qualitative coding profile for evidence review. |

## Keep implementation clean

| Skill | Description |
|---|---|
| [`no-ai-slop-comments`](coding/no-ai-slop-comments/SKILL.md) | Reviews new comments while preserving useful rationale. |

## Review what actually changed

| Skill | Description |
|---|---|
| [`mastermind-diff-review`](code-review/mastermind-diff-review/SKILL.md) | Reviews a diff, including untrusted PRs, for proven defects without running it. |
| [`mastermind-comment-audit`](code-review/mastermind-comment-audit/SKILL.md) | Audits changed comments and deleted rationale with quotations. |
| [`mastermind-test-audit`](code-review/mastermind-test-audit/SKILL.md) | Checks whether tests exercise and assert the changed behavior. |
| [`mastermind-frontend-audit`](code-review/mastermind-frontend-audit/SKILL.md) | Reviews changed component usage, props, duplication and design tokens. |

## Turn design intent into a contract

| Skill | Description |
|---|---|
| [`mastermind-design-intake`](design/mastermind-design-intake/SKILL.md) | Turns a design handoff into components, tokens and acceptance criteria. |

## Prove a change works

| Skill | Description |
|---|---|
| [`mastermind-verify-change`](testing/mastermind-verify-change/SKILL.md) | Gates untrusted code, then runs the smallest checks that prove a change. |

## Verify in a browser

| Skill | Description |
|---|---|
| [`mastermind-browser-verification`](testing/mastermind-browser-verification/SKILL.md) | Records browser, accessibility, console, network and viewport observations. |

## Investigate before declaring a cause

| Skill | Description |
|---|---|
| [`mastermind-investigation-ledger`](debugging/mastermind-investigation-ledger/SKILL.md) | Tracks competing bug hypotheses, evidence and focused probes. |

## Map trust boundaries and reachable risk

| Skill | Description |
|---|---|
| [`mastermind-security-research`](security/mastermind-security-research/SKILL.md) | Traces privileged operations, guards, secret readers and unresolved boundaries. |
| [`mastermind-agent-security-review`](security/mastermind-agent-security-review/SKILL.md) | Reviews agent/tool trust boundaries with optional OWASP mapping. |
| [`mastermind-scanner-alert-response`](security/mastermind-scanner-alert-response/SKILL.md) | Takes one scanner alert to a fix from metadata, never a secret's value. |

## Keep dependencies current

| Skill | Description |
|---|---|
| [`mastermind-dependency-upgrade`](ops/mastermind-dependency-upgrade/SKILL.md) | Applies the smallest compatible upgrade with install scripts off. |

## Write documentation that matches the code

| Skill | Description |
|---|---|
| [`mastermind-docs-authoring`](docs/mastermind-docs-authoring/SKILL.md) | Writes and reviews docs, decisions and diagrams against the current code. |

## Refine the request without replacing it

| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Rewrites prompts or handoffs while preserving the original request. |

## Write or change a skill

A host loads only the name and description of every installed skill, so the
description decides whether the body is ever read. The body is read once and
stays in context, so write rules that hold for the whole task.

1. Write the eval case first. Add a case to
   [`workflow.jsonl`](../evals/behavior/cases/workflow.jsonl) that fails
   without the change, and a restraint case where the skill should do nothing
   extra. See [behaviour evals](../evals/behavior/README.md).
2. Start the description with `Use when ...`, then
   `Not for ... (use <sibling>)` for the nearest skill that could be picked by
   mistake, then a short line on what it does. When the skill list is over
   budget, Codex cuts every description to the same length from the end, so
   the trigger and boundary go first. Keep it within 300 characters.
   `just validate` checks this.
   Add prompts for and against the skill to the
   [selection corpus](../evals/selection/README.md).
3. Put the steps in order, with the evidence each step must produce and the
   condition that stops the work or hands it to another skill. A skill that
   reads repository or pull request content carries the
   [untrusted-content paragraph](../agents/blocks/untrusted-content.md).
4. Give the output a fixed shape, with `incomplete` or `could_not_verify` for
   work that stopped early.
5. For a rule the model keeps breaking, name the excuse it gives and the check
   that catches it. For an output with the wrong shape, describe the right
   shape rather than listing what not to do. Enforce anything a script can
   check in the validator instead.
6. Move long reference material into `references/`, one level below
   `SKILL.md`, and bump `metadata.version`.
