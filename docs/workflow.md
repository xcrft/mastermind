# Workflow

Mastermind separates repository facts from agent judgment. The codegraph
establishes what exists and what a diff can affect; task artifacts record scope,
claims, checks, and review decisions. Use only the depth justified by the risk.

## Choose a mode

| Mode | Use for | Required artifacts |
|---|---|---|
| **Direct** | Small, reversible, clearly scoped work | None |
| **Verified** | Normal multi-file or delegated work | Spec, executor report, audit, task state |
| **Strict** | Auth, billing, migrations, public API, data loss, supply chain, hard rollback | Verified artifacts plus risk, rollback, and independent review evidence |

`lite` and `standard` remain readable for compatibility. New task specs should
use `verified` or `strict`.

## Direct

```bash
mastermind index .
mastermind impact --since main
# implement the change and run focused + repository-required checks
mastermind impact --since main
```

Direct work has no task folder or controller state. The implementation and
verification record live in the normal commit/PR. Use this path when rollback
is easy and the change does not need delegated ownership.

## Verified

### 1. Create and verify the contract

```bash
mastermind new-spec "Add account recovery"
mastermind verify-spec .mastermind/tasks/001-add-account-recovery/spec.md
```

Review the scope, acceptance criteria, and verification commands before
approval. Then record the baseline:

```bash
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --pre-only
```

### 2. Implement against the approved spec

Give `spec.md` to the implementation agent. The executor may change only the
approved product files and must write `executor-report.md`. It must not edit
`state.json`, `audit.md`, or controller-owned history files.

### 3. Audit the real diff

```bash
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --post-only
```

Post-flight compares the approved spec, executor claims, current index, and Git
diff. Uncommitted and untracked files count because this gate normally runs
before commit.

| Verdict | Meaning | Next action |
|---|---|---|
| `held` | Mechanical contract is satisfied | Perform semantic review and delivery gates |
| `drift` | Work differs from the approved contract | Planner reviews and updates or rejects the drift |
| `broken` | Required evidence or behavior is missing | Executor fixes the change before another audit |

Post-flight fails closed when the executor report is absent or malformed.

## Strict

```bash
mastermind new-spec "Rotate signing keys" --mode strict
```

Strict uses the same state machine and adds the evidence that high-risk work
needs: explicit alternatives, threat/failure cases, rollback or migration,
design criticism, and independent review. A security review is required when
the change crosses authentication, authorization, secrets, tool permissions,
agent delegation, or the supply chain.

Strict is not a larger template for ordinary work. If no material failure mode
or difficult rollback exists, Verified is the clearer contract.

## Task artifacts and ownership

Each canonical task lives under `.mastermind/tasks/<NNN>-<slug>/`.

| Artifact | Writer | Contract |
|---|---|---|
| `spec.md` | Planner | Goal, scope, acceptance criteria, verification, mode-specific risk evidence |
| `executor-report.md` | Executor | Changed files, observed checks, claims, defects, and gaps |
| `audit.md` | Controller | Mechanical comparison of spec, report, index, and diff |
| `state.json` | Controller | One task-local lifecycle record |
| `history-review.md` | Controller, then planner | Explicit Context and Lesson disposition after semantic review |

A held audit may also write a release-note candidate under
`.mastermind/releases/`. Markdown remains the durable source of truth;
`state.json` and the SQLite history index are coordination/retrieval layers.

## What the gates prove

Pre-flight checks:

- mandatory sections and mode requirements;
- referenced files and indexed symbols;
- pre-edit caller-count snapshots;
- literal FIND blocks when supplied;
- declared verification commands.

Post-flight checks:

- actual changed files against approved scope;
- required report shape and executed-command claims;
- planned tests and zero-test/vacuous claims;
- symbol removal or signature drift;
- index and snapshot consistency.

They do not prove runtime behavior, product quality, visual correctness,
security, or architectural soundness. Those require tests and human/domain
review.

## Optional review disciplines

Load a discipline because the changed paths or risk require it, not because a
large checklist looks thorough.

| Need | Before implementation | After implementation |
|---|---|---|
| Unknown code structure | `mastermind-codegraph-research` | — |
| Service/state/retry boundary | `mastermind-runtime-research` | `mastermind-architecture-review` |
| UI component reuse and callers | `mastermind-component-research` | `mastermind-frontend-audit` + browser verification |
| Test relevance | `mastermind-test-impact` | `mastermind-test-audit` |
| Security/tool boundary | `mastermind-security-research` | `mastermind-agent-security-review` |
| Changed comments | — | `mastermind-comment-audit` |
| Product prose to task contract | `mastermind-product-intake` | — |

The installed [skill catalog](../skills/README.md) defines each contract. These
reviews are read-only and do not replace the controller audit.

## History and lessons

Mechanical drift may create one lesson candidate per task. A candidate records
the observed failure; it is not reusable guidance until semantic review writes
the actual lesson and changes its status. Repeated failures refresh the same
candidate instead of creating duplicates.

After a held audit, review `history-review.md` and mark Context and Lesson as
`updated` or `not applicable` with a concrete reason. This prevents a successful
diff from silently becoming an invented architectural decision.

## Client model

Planning, implementation, and post-flight are client-neutral. Claude Code and
Codex can install the complete workflow bundle. Cursor, Continue, and generic
MCP clients receive the graph tools but do not have a Mastermind-owned native
workflow-extension format.

`run-task --exec` is a legacy Claude CLI convenience. The portable path is an
explicit handoff followed by `--post-only`.

## Related documentation

- [Getting started](getting-started.md)
- [CLI and MCP reference](reference/mmcg.md)
- [Verifiable GitHub Action](github-action.md)
- [Contributing](../CONTRIBUTING.md)
