# Skills

Skills used by the Mastermind workflow. Every skill in this tree is included in
the installable workflow bundle.

## Index

### workflow/
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

### coding/
| Skill | Description |
|---|---|
| [`no-ai-slop-comments`](coding/no-ai-slop-comments/SKILL.md) | Keeps useful comments and removes narration introduced by the current change without widening scope. |

### code-review/
| Skill | Description |
|---|---|
| [`mastermind-comment-audit`](code-review/mastermind-comment-audit/SKILL.md) | Reviews the comment delta of a finished change with quoted evidence, names what it kept, and reports deleted rationale. |
| [`mastermind-test-audit`](code-review/mastermind-test-audit/SKILL.md) | Checks whether a change's tests prove its behaviour: uncovered symbols, a test on the wrong path, an assertion moved with the code, and non-asserting tests. |
| [`mastermind-frontend-audit`](code-review/mastermind-frontend-audit/SKILL.md) | Checks a finished UI change against the codegraph: unrendered components, props contracts changed without their callers, duplicates, and raw values shadowing tokens. |

### design/
| Skill | Description |
|---|---|
| [`mastermind-design-intake`](design/mastermind-design-intake/SKILL.md) | Converts a design handoff into named components, token names, and criteria that can fail, parking visual fidelity explicitly. |

### testing/
| Skill | Description |
|---|---|
| [`mastermind-browser-verification`](testing/mastermind-browser-verification/SKILL.md) | Records browser checks as evidence — accessibility tree over screenshot, console and network errors, viewports as a checklist, unchecked marked unchecked. |

### debugging/
| Skill | Description |
|---|---|
| [`mastermind-investigation-ledger`](debugging/mastermind-investigation-ledger/SKILL.md) | Diagnoses unknown bugs with competing hypotheses, evidence for/against, and bounded decision-changing probes. |

### security/
| Skill | Description |
|---|---|
| [`mastermind-security-research`](security/mastermind-security-research/SKILL.md) | Enumerates reachable privileged operations, the sites that statically apply a guard, and secret readers — reporting the difference as unestablished rather than as a verdict. |
| [`mastermind-agent-security-review`](security/mastermind-agent-security-review/SKILL.md) | Portable security review protocol for agent/tool trust boundaries, with optional evidence-based OWASP mapping. |

### prompt-engineering/
| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Rewrites explicit prompts or cold-agent handoffs while preserving the original request verbatim. |
