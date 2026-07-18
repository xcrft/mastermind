# How the Mastermind workflow works

Mastermind separates structural evidence from agent judgment. The codegraph
answers what exists and what a change can affect; the workflow adds only as
much ceremony as the risk justifies.

## Choose one mode

| Mode | Use it for | Required artifact |
|---|---|---|
| **Direct** | Small, reversible, clearly scoped work | None. Use `map`, `impact`, and tests as needed. |
| **Verified** | Normal multi-file or delegated work | A compact task spec with goals, scope, acceptance criteria, and verification. |
| **Strict** | Auth, billing, migrations, public APIs, data-loss, supply-chain, or difficult rollback | A verified spec plus risk, evidence, rollback, and independent review. |

Direct mode works after `mastermind index .`; it does not require
`mastermind init` or a task folder. `lite` and `standard` task modes remain
readable for compatibility, but new tasks should use `verified` or `strict`.

## Direct workflow

```bash
mastermind index .
mastermind map .
mastermind impact --since main
# implement the change
mastermind impact --since main
# run focused tests and the repository-required gate
```

This is the normal path for a small fix. Mastermind supplies structural facts;
your coding client performs the edit and reports the checks it actually ran.

## Verified workflow

Create the contract:

```bash
mastermind new-spec "Add account recovery"        # verified is the default
mastermind verify-spec .mastermind/tasks/001-add-account-recovery/spec.md
# review Scope and Acceptance Criteria, then approve the task
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --pre-only
```

The task folder owns four durable artifacts:

| Artifact | Owner | Purpose |
|---|---|---|
| `spec.md` | Planner | Goals, scope, acceptance criteria, and verification contract |
| `executor-report.md` | Executor | Files changed, claims, defects, and observed command results |
| `audit.md` | Controller | Mechanical comparison of the report, spec, index, and real diff |
| `state.json` | Controller | One task-local lifecycle record |

`verify-spec` is read-only. `run-task --pre-only` is intentionally later: it
records that the contract was approved and captures the git baseline. After
that transition, hand `spec.md` to the implementation agent in any coding
client. The executor must write `executor-report.md`; it must not update
`state.json`. Then run:

```bash
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --post-only
```

Post-flight fails closed when the report is missing or malformed. A `held`
verdict writes `audit.md`, marks the task complete, and drafts release notes.
`drift` or `broken` keeps the task blocked for planner review. A completed task
is idempotent unless `--post-only` explicitly requests another audit.

`run-task --exec` is a legacy convenience that shells out to `claude -p`.
Normal handoff plus `--post-only` is client-neutral and works with Claude Code,
Codex, Cursor, Continue, or another client.

## Strict workflow

```bash
mastermind new-spec "Rotate signing keys" --mode strict
```

Strict mode uses the same controller and artifacts, but adds only material
risk controls: alternatives, evidence ledger, rollback/migration, a design
critic, and a read-only independent auditor. Security review is required when
the change crosses auth, secrets, permissions, agent/tool boundaries, or the
supply chain.

## What the checks prove

Pre-flight checks required sections, referenced paths, indexed symbols,
snapshot drift, literal FIND blocks when present, and verification commands.
Post-flight checks changed-file scope, removed or changed symbols, snapshot
drift, planned tests, and executor claims.

The graph is syntactic and bounded. Dynamic dispatch, reflection, re-exports,
overloads, and cross-language calls can reduce precision. A held mechanical
contract therefore still needs a short semantic check: did the implementation
solve the original request, and do the tests demonstrate the acceptance
criteria?

## Client capabilities

`mastermind install --client all` installs portable skills for Claude Code and
Codex, plus spawnable subagent definitions for Claude Code. Codex receives the
same workflow guidance as skills, but not Claude's native subagent runtime.
Cursor and Continue use Mastermind through MCP; they do not receive a claimed
native workflow bundle.

Verify installed ownership and content hashes with:

```bash
mastermind doctor --workflow --client all
```

For signed, policy-bound CI evidence, see [Verifiable audits and GitHub
Action](github-action.md).
