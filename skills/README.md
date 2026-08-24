# Skills

Portable, client-neutral capabilities for taking a task from intent to
evidence. Pick the smallest skill that owns the job; combine them only when the
task genuinely crosses research, implementation, review, or security
boundaries.

Every `SKILL.md` in this tree is staged into the npm workflow bundle;
`scripts/validate.py` fails if this index or the staged copy drifts.

Skills define task behavior. Claude Code-specific spawnable roles live under
[`agents/`](../agents/).

## Move a task from request to evidence

| Skill | Description |
|---|---|
| [`mastermind-task-planning`](workflow/mastermind-task-planning/SKILL.md) | Chooses Direct, Verified, or Strict and creates the lightest evidence-grounded delegation contract that fits the risk. |
| [`mastermind-task-executor`](workflow/mastermind-task-executor/SKILL.md) | Executes an approved contract by outcomes and acceptance criteria, uses bounded repair, and writes `executor-report.md`. |
| [`mastermind-codegraph-research`](workflow/mastermind-codegraph-research/SKILL.md) | Uses mmcg for structural discovery while preserving syntactic, collision, precision, stale-index, and runtime-proof limits. |
| [`mastermind-component-research`](workflow/mastermind-component-research/SKILL.md) | Answers whether a React or Vue component already exists, who renders it, and what its props contract is, before the change is written. |
| [`mastermind-structured-report-contract`](workflow/mastermind-structured-report-contract/SKILL.md) | Defines the file-backed executor report consumed by post-flight and the advisory Strict auditor tail. |
| [`mastermind-critical-review`](workflow/mastermind-critical-review/SKILL.md) | Stress-test a design, spec, plan, or report for false assumptions, broken contracts, scope creep, missing evidence, and high-risk failure modes. |
| [`mastermind-product-intake`](workflow/mastermind-product-intake/SKILL.md) | Converts a PRD or ticket into criteria that can fail, resolves product nouns to symbols, surfaces unspecified cases, and parks outcome metrics. |
| [`mastermind-runtime-research`](workflow/mastermind-runtime-research/SKILL.md) | Gathers consumers, state writers, and boundary crossings before a service change, and names the runtime gaps the graph cannot span. |
| [`mastermind-architecture-review`](workflow/mastermind-architecture-review/SKILL.md) | Reviews runtime paths, state ownership, retry behavior, and compatibility against concrete system invariants. |
| [`mastermind-project-history`](workflow/mastermind-project-history/SKILL.md) | Explains prior decisions from durable evidence while separating observation, inference, provenance, and technical proof. |
| [`mastermind-project-map`](workflow/mastermind-project-map/SKILL.md) | Builds a bounded architecture map from the live codegraph, including collision and truncation evidence. |
| [`mastermind-change-impact`](workflow/mastermind-change-impact/SKILL.md) | Reports changed files, symbols, structural impact, and risk from a live diff. |
| [`mastermind-test-impact`](workflow/mastermind-test-impact/SKILL.md) | Selects tests from changed symbols and graph evidence without claiming runtime certainty. |
| [`mastermind-cross-client-setup`](workflow/mastermind-cross-client-setup/SKILL.md) | Installs or previews portable setup across supported clients with explicit write and force boundaries. |
| [`mastermind-audit-attestation`](workflow/mastermind-audit-attestation/SKILL.md) | Separates content integrity, signer provenance, and policy acceptance for audit evidence. |
| [`mastermind-style-deep`](workflow/mastermind-style-deep/SKILL.md) | Adds a grounded qualitative coding portrait consumed as advisory planner/executor input. |

## Keep implementation clean

| Skill | Description |
|---|---|
| [`no-ai-slop-comments`](coding/no-ai-slop-comments/SKILL.md) | Keeps useful comments and removes narration introduced by the current change without widening scope. |

## Review what actually changed

| Skill | Description |
|---|---|
| [`mastermind-comment-audit`](code-review/mastermind-comment-audit/SKILL.md) | Reviews the comment delta of a finished change with quoted evidence, names what it kept, and reports deleted rationale. |
| [`mastermind-test-audit`](code-review/mastermind-test-audit/SKILL.md) | Checks whether a change's tests prove its behaviour: uncovered symbols, a test on the wrong path, an assertion moved with the code, and non-asserting tests. |
| [`mastermind-frontend-audit`](code-review/mastermind-frontend-audit/SKILL.md) | Checks a finished UI change against the codegraph: unrendered components, props contracts changed without their callers, duplicates, and raw values shadowing tokens. |

## Turn design intent into a contract

| Skill | Description |
|---|---|
| [`mastermind-design-intake`](design/mastermind-design-intake/SKILL.md) | Converts a design handoff into named components, token names, and criteria that can fail, parking visual fidelity explicitly. |

## Record browser evidence honestly

| Skill | Description |
|---|---|
| [`mastermind-browser-verification`](testing/mastermind-browser-verification/SKILL.md) | Records browser checks as evidence — accessibility tree over screenshot, console and network errors, viewports as a checklist, unchecked marked unchecked. |

## Investigate before declaring a cause

| Skill | Description |
|---|---|
| [`mastermind-investigation-ledger`](debugging/mastermind-investigation-ledger/SKILL.md) | Diagnoses unknown bugs with competing hypotheses, evidence for/against, and bounded decision-changing probes. |

## Map trust boundaries and reachable risk

| Skill | Description |
|---|---|
| [`mastermind-security-research`](security/mastermind-security-research/SKILL.md) | Enumerates reachable privileged operations, the sites that statically apply a guard, and secret readers — reporting the difference as unestablished rather than as a verdict. |
| [`mastermind-agent-security-review`](security/mastermind-agent-security-review/SKILL.md) | Portable security review protocol for agent/tool trust boundaries, with optional evidence-based OWASP mapping. |

## Refine the request without replacing it

| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Rewrites explicit prompts or cold-agent handoffs while preserving the original request verbatim. |
