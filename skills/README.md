# Skills

Skills used by the Mastermind workflow. Core skills are installed by `mastermind init`. Non-core skills live in [`extras/`](../extras/).

## Index

### workflow/
| Skill | Description |
|---|---|
| [`mastermind-task-planning`](workflow/mastermind-task-planning/SKILL.md) | CTO/planner mode — brainstorms with user and writes detailed `.mastermind/tasks/<NNN>-<name>/spec.md` files (folder per task) for delegation. Never implements. |
| [`mastermind-task-executor`](workflow/mastermind-task-executor/SKILL.md) | Executes a `.mastermind/tasks/<NNN>-<name>/spec.md` file phase-by-phase, runs VERIFY, marks the checklist, stops on first failure. |
| [`mastermind-codegraph-research`](workflow/mastermind-codegraph-research/SKILL.md) | Shared truth layer — ground structural claims (symbol existence, callers, blast radius, file paths) in mmcg, not memory. Used across plan / research / critique / audit. |
| [`mastermind-structured-report-contract`](workflow/mastermind-structured-report-contract/SKILL.md) | The executor↔planner↔auditor report tail — sentinel-wrapped YAML, defect kinds, complete/partial/failed shapes. |
| [`mastermind-critical-review`](workflow/mastermind-critical-review/SKILL.md) | Stress-test a design, spec, plan, or report for false assumptions, broken contracts, scope creep, missing evidence, and high-risk failure modes. |

### coding/
| Skill | Description |
|---|---|
| [`no-ai-slop-comments`](coding/no-ai-slop-comments/SKILL.md) | Keep only comments that explain a *why* the code can't — delete restating-the-code, section banners, edit markers, and the rest of the slop. Used by the executor when applying a spec. |

### debugging/
| Skill | Description |
|---|---|
| [`mastermind-investigation-ledger`](debugging/mastermind-investigation-ledger/SKILL.md) | Diagnose unknown bugs with a hypothesis ledger and one-probe-at-a-time loop before drafting a spec. |

### security/
| Skill | Description |
|---|---|
| [`mastermind-agent-security-review`](security/mastermind-agent-security-review/SKILL.md) | OWASP reference pack for security audits — the verified OWASP ASI (Agentic) Top 10 mapped to Mastermind surfaces. Read by the `mastermind-security-auditor` subagent in OWASP mode (evidence-based, not from memory). |

### prompt-engineering/
| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Refines a user's rough prompt into a clean version before handing off to another agent. One-pass refine + handoff. |

