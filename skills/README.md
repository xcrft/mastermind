# Skills

Portable task capabilities included in the npm workflow bundle. Use the skill
that matches the work. Claude Code-specific roles are listed in
[agents](../agents/). Install with the [client setup guides](../docs/integrations/generic-mcp.md).

`just validate` checks this catalogue and bundle staging against the shipped
`SKILL.md` files.

## Move a task from request to evidence

| Skill | Description |
|---|---|
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
| [`mastermind-comment-audit`](code-review/mastermind-comment-audit/SKILL.md) | Audits changed comments and deleted rationale with quotations. |
| [`mastermind-test-audit`](code-review/mastermind-test-audit/SKILL.md) | Checks whether tests exercise and assert the changed behavior. |
| [`mastermind-frontend-audit`](code-review/mastermind-frontend-audit/SKILL.md) | Reviews changed component usage, props, duplication and design tokens. |

## Turn design intent into a contract

| Skill | Description |
|---|---|
| [`mastermind-design-intake`](design/mastermind-design-intake/SKILL.md) | Turns a design handoff into components, tokens and acceptance criteria. |

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

## Refine the request without replacing it

| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Rewrites prompts or handoffs while preserving the original request. |
