---
name: mastermind-workflow
description: Compact project router for the Mastermind Direct, Verified, and Strict workflows.
metadata:
  version: 2.2.3
  authors: [mastermind]
  tags: [claude-md, workflow, delegation, audit]
---

# <PROJECT_NAME>

<What this project is and is not.>

## Orientation

- **Stack:** <language/framework>
- **Entry point:** `<path>`
- **Core code:** `<path>`
- **Project knowledge:** [CONTEXT.md](CONTEXT.md)

## Commands

```bash
# focused tests
<FOCUSED_TEST_COMMAND>

# repository-required gate
<FULL_GATE_COMMAND>
```

## Mastermind workflow

Use the lightest mode that fits the risk:

| Mode | Use | Flow |
|---|---|---|
| **Direct** | Small, reversible, clear work | inspect → impact if useful → implement → tests → comment-delta gate |
| **Verified** | Normal multi-file or delegated work | compact task contract → deterministic pre/post gates → semantic review |
| **Strict** | Auth, billing, migration, public API, data loss, supply chain | verified flow + critic/security/rollback evidence + independent auditor |

Do not create a spec for Direct work.

Direct work has no controller or post-flight. After finishing, inspect
`git diff -U0 <baseline>` and untracked files for comment additions, edits or
deletions. Spawn `mastermind-comment-auditor` with the delta only if it is non-empty.

### Grounding

Call `mmcg_brief` once with the active `planner`, `executor`, or `auditor` role,
baseline and a 2,000-token budget before non-trivial broad discovery. Skip only
literal-only or already-localized Direct work. Load deferred MCP tools as one
bounded set.
Use the brief's aggregate `disciplines` to route frontend, QA, or migration
evidence. They classify paths, not behavior; inspect the change and preserve any
scope-incomplete boundary before treating a discipline as covered.

Route discovery by question:

- `mmcg_concept_prepare`, source inspection and `mmcg_concept_finish` for operation relevance; `mmcg_concept` for a fast lexical shortlist;
- `mmcg_search` for symbol existence;
- `mmcg_callers` / `mmcg_impact` for blast radius;
- `mmcg_change_impact` and `mmcg_test_impact` for an existing diff;
- `mmcg_history`, then the cited Markdown, for prior decisions or lessons;
- `mmcg_map` for unfamiliar architecture;
- `mmcg_callers` on a component for who renders it — JSX and Vue template usage
  are call edges.

Before concept retrieval, freeze a query-only `request_contract`: `operation`
and exact quoted `constraints` with unique ids and input/state/scope/result
kinds. Follow `operation-roles-v3`; the original query stays authoritative even
if the ledger is incomplete. Every relevant entry needs an own-behavior typed
`witness` and one matched, source-bound check per declared constraint. Bind
helper/entrypoint/contract paths with `supporting_evidence`. A concrete module
or handler branch may implement or expose the operation through delegation.
Being mandatory or earlier alone does not make a domain helper; useful generic
prerequisites are context. Context needs the existing operation with every
explicit constraint. Do not substitute optional selectors for runtime binding,
or rendered instructions for execution. Mismatches are irrelevant; uninspected
stays unknown. Keep `supporting_context`/`query_ambiguities` outside
`semantic_candidates`. Server validation checks consistency, not semantic truth.

Do not use Bash to rediscover symbols, files, callers, imports, or impact already
answered by a fresh, complete graph result. Use `Grep`, `Glob`, or source reads
for exact text, comments, configs, docs, locales, generated code, and graph
warnings. Bash remains the right tool for Git, builds, tests, linters, logs, and runtime probes.

The graph is syntactic and bounded. Preserve stale-index, collision, precision,
and truncation caveats; source reads and tests remain authoritative for runtime behavior.

### Verified and Strict tasks

1. Use `mastermind-task-planning` to create and ground the contract.
2. Run the read-only contract validator: `mastermind verify-spec <spec>`.
3. The user approves Scope and Acceptance Criteria.
4. Run the state-writing pre-flight:

   ```bash
   mastermind run-task .mastermind/tasks/<task>/spec.md --pre-only
   ```

5. Use `mastermind-task-executor`. It writes
   `<task>/executor-report.md` and never writes lifecycle state. For a runnable
   UI change, it records browser observations there before post-flight, or marks
   each unavailable check as `not checked` with the reason.
6. Run controller-owned post-flight:

   ```bash
   mastermind run-task .mastermind/tasks/<task>/spec.md --post-only
   ```

7. Inspect the post-flight baseline diff and untracked files for a comment
   delta. Spawn `mastermind-comment-auditor` with it only when a comment was
   added, modified, or deleted. Its findings are input to your semantic review,
   not a verdict.
8. Perform semantic review. Strict work additionally requires the read-only
   `mastermind-auditor`.

The controller is the only owner of `state.json`, `audit.md`, lessons, and
release eligibility. `audit.md` retains the complete bounded symbol diff, every
claim check, and the checked executor report used for its verdict. A missing or
malformed executor report fails post-flight.

### Discipline routing

`mmcg_change_impact` reports a `disciplines` block derived from the changed
paths. Read it instead of classifying the request yourself, and load at most one
research skill and one audit skill per detected discipline:

| discipline | before the change | after the change |
|---|---|---|
| `frontend` | [[mastermind-component-research]] | [[mastermind-frontend-audit]]; [[mastermind-browser-verification]] before post-flight when the client is runnable |
| `qa` | [[mastermind-test-impact]] | [[mastermind-test-audit]] |
| `migration` | [[mastermind-runtime-research]] | [[mastermind-architecture-review]] |

`unclassified` means the path cannot establish a discipline; a plain `.ts` file
may contain a queue consumer, migration or auth boundary. For inspected service
or state work, use [[mastermind-runtime-research]] before and
[[mastermind-architecture-review]] after. That decision is yours; the classifier
proposes evidence without locking the set.

A detected `migration` also raises the mode: the table above already lists
migrations as Strict work, so a SQL file or a migrations directory in the diff
is a mode trigger the paths establish rather than a judgement call. It says
nothing about what the migration does — destructive, backfilling, or additive is
a question for the review.

Pre-flight, before a diff exists, route on the paths named in the spec's Scope.
Browser observations stay report evidence: they do not certify visual fidelity
and do not replace declared verification commands.

### Role routing

Keep the selected model; Claude role aliases do not select other clients' models.
Without a per-invocation route, use shipped effort defaults. Route by outcome and
inspected risk, not request length. Each handoff carries supported facts,
citations, obligations and the exact gap. The controller keeps the request open
until those obligations are resolved.

- Researcher: bounded batches of repository facts.
- Investigator: bugs whose cause is not known.
- Critic: real design forks; mandatory for Strict.
- Security research: trace privileged operations, guards and secret readers.
  Missing graph edges do not establish absence of access; present edges do not
  establish enforcement.
- Security auditor: permissions, secrets, tools, untrusted input, delegation,
  supply chain, or audit policy.
- Executor: implementation within approved scope.
- Auditor: independent read-only review for Strict work.
- Comment auditor: non-empty comment delta of a finished change, in every mode;
  skip the spawn when the gate finds no changed comments.
- Runtime research: inspect service consumers, state writers, crossed boundaries
  and invocations the graph cannot see. Zero static callers is a gap, not absence.
- Test auditor: confirm a `direct` candidate's production path, assertion and
  observed relevant run. A `heuristic` filename match is not coverage.
- Frontend auditor: React/Vue unrendered components, props contracts, duplicates
  and raw values. Research the component graph before writing UI.
- Design intake: name components, tokens and falsifiable criteria. Preserve
  source and human visual review in Design Context; park visual fidelity
  explicitly outside acceptance criteria.
- Product intake: separate behaviour, constraints and outcomes. Preserve stable
  source and parked outcome in the derived spec's Product Context. Acceptance
  criteria cover behaviour and measurable constraints, not success metrics;
  ask about cases missing from the PRD before approval.
- Browser verification: record observations and viewport. Accessibility tree
  and console are evidence; screenshots are not. Mark unchecked items unchecked.
- Feedback collector: only on request, copy the person's own words from chosen
  sessions into the private review inbox; never the profile.

Skip roles that add no new evidence.

### Effort routing

Use [[mastermind-agent-handoff]] before each new agent, including ordinary
in-session research, execution and review. Select effort and pass its setting
to the actual spawn. Codex uses the returned `codex_spawn_args` with a fresh
child and a compact context packet; a full-history fork inherits parent effort.
Preserve model, tools, budgets, facts and gaps. Unsupported hosts retain their
setting with an explicit `not_applied` record. See [the contract](https://github.com/xcrft/mastermind/blob/main/docs/guides/effort-routing.md).

### Completion and release

Report only checks actually run and limitations still open. A Held mechanical
contract is not a substitute for semantic judgment.

Commit, push, PR, tag, release, and publication require explicit user
authorization. Never infer release authority from implementation approval.
