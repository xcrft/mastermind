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
# review the comment delta with `mastermind-comment-audit`
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

The task folder owns five workflow artifacts:

| Artifact | Owner | Purpose |
|---|---|---|
| `spec.md` | Planner | Goals, scope, acceptance criteria, and verification contract |
| `executor-report.md` | Executor | Files changed, claims, defects, and observed command results |
| `audit.md` | Controller | Mechanical comparison of the report, spec, index, and real diff |
| `state.json` | Controller | One task-local lifecycle record |
| `history-review.md` | Controller, then planner | Explicit Context/Lesson disposition after semantic review |

`verify-spec` is read-only. `run-task --pre-only` is intentionally later: it
records that the contract was approved and captures the git baseline. After
that transition, hand `spec.md` to the implementation agent in any coding
client. The executor must write `executor-report.md`; it must not update
`state.json`. Then run:

```bash
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --post-only
```

Post-flight fails closed when the report is missing or malformed. A `held`
verdict writes `audit.md`, marks the task complete, writes release notes under
`.mastermind/releases/`, and creates a pending `history-review.md` without
inventing a decision or lesson.
`drift` or `broken` keeps the task blocked for planner review. A completed task
is idempotent unless `--post-only` explicitly requests another audit.

Mechanical `drift` and `broken` findings create a lesson `candidate` with
provenance and evidence — **one per task**, keyed on the task alone. A later
failure on the same task refreshes that entry to the newest observation and
raises its `Occurrences` count rather than filing a sibling, so an
iterate-until-green loop leaves one reviewable record instead of a pile of
near-identical ones. It becomes reusable guidance only after semantic review
supplies the actual lesson and changes its status to `active`, `resolved`, or
`superseded`; once reviewed, later mechanical noise never rewrites it. A successful task may still produce a
durable lesson; a failed audit does not automatically prove one.

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

## Review the comment delta

Comment discipline is asked of the implementation agent, but an agent optimizing
for acceptance criteria drops it first, and the mechanical contract cannot see
it: post-flight proves that a file changed, not that the change stopped
narrating itself. So the rule is verified by a reader.

`mastermind-comment-audit` (skill, portable) and `mastermind-comment-auditor`
(Claude subagent) review only the comments a change added, modified, or deleted.
They are read-only: findings carry the comment verbatim, the code line that
already says it, and what would be lost — and the report names what it kept, so
a reviewer that flags nothing is reporting a clean result rather than failing.
Deleted rationale is reported too; a rename that takes a non-obvious `why` with
it is a regression no write-time rule catches.

Two entry points, because Direct work has no controller:

- **Verified and strict:** after `run-task --post-only`, as input to semantic
  review. Post-flight prints the reminder with the recorded baseline on `held`
  and `drift`, and withholds it on `broken` — the executor still has to iterate,
  so that comment delta is not the one a reviewer would judge.
- **Direct:** invoke it against the branch point once the change is finished.
  This is the only review Direct work gets.

A comment audit produces no `held` / `drift` / `broken` verdict and never
substitutes for the contract audit.

## Review a UI change

The codegraph indexes React components (including arrow and `memo`/`forwardRef`
forms) and Vue single-file components, and treats `<Button />` and
`<base-button />` as call edges. That makes two frontend questions structural
rather than a matter of opinion: does this component already exist and who
renders it, and did a props contract change without its callers.

`mastermind-component-research` answers them before the change is written —
reinvention is the most expensive frontend mistake, and it is invisible in a
diff. `mastermind-frontend-audit` (skill) and `mastermind-frontend-auditor`
(Claude subagent) check the finished change: a component nothing renders, a
required prop added while callers stay on the old contract, a duplicate of an
existing component, and a raw value shadowing a design token.

Neither judges whether the result looks right. Visual fidelity, spacing, and
interaction quality need the running application, and a `clean` frontend audit
says nothing about them — record those separately as browser observations, marked
as checked or not checked, never as structural claims.

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
