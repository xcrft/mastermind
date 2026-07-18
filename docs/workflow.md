# How the Mastermind workflow works

Mastermind combines an LLM workflow with deterministic checks against the live repository. Agents can interpret intent and design a change; the codegraph and gates verify factual claims.

## Lifecycle

```mermaid
flowchart LR
  R["Request"] --> I["Refine intent"]
  I --> P["Plan from codegraph"]
  P --> C["Challenge the design"]
  C --> V["Verify the spec"]
  V --> E["Execute the spec"]
  E --> A["Audit the diff"]
  A --> O["Release notes and evidence"]
```

The roles are deliberately separated:

- **Prompt refiner:** turns a rough or multi-intent request into a bounded brief.
- **Planner:** researches the repository and writes the implementation contract.
- **Critic:** challenges the design before implementation.
- **Researcher:** gathers codebase facts without making design decisions.
- **Investigator:** maintains competing hypotheses for unknown-cause bugs.
- **Security auditor:** reviews trust boundaries and high-risk changes.
- **Executor:** implements the accepted spec and records verification evidence.
- **Auditor:** compares the executor's report with the actual repository state.

Roles are workflow responsibilities, not requirements for a particular model vendor or model name.

## Project setup

Initialize the full workflow in a repository:

```bash
mastermind init
mastermind doctor
```

Create a task contract:

```bash
mastermind new-spec "Add account recovery"
mastermind status
mastermind next
```

Task specs live under `.mastermind/tasks/<NNN>-<name>/spec.md`. A spec records goals, non-goals, touched files, expected symbol state, implementation phases, and verification commands.

## Deterministic gates

Before implementation:

```bash
mastermind verify-spec .mastermind/tasks/001-account-recovery/spec.md
```

`verify-spec` checks that required sections are present, referenced files exist, symbol claims match the codegraph, FIND blocks are current, and the planned blast radius is credible.

After implementation:

```bash
mastermind audit-spec .mastermind/tasks/001-account-recovery/spec.md --since main
```

`audit-spec` compares the contract with the real diff. It detects unexpected files, missing planned tests, signature drift, removed symbols, and scope changes. Its verdict is `held`, `drift`, or `broken`.

The gates are deterministic Rust code. Agent interpretation helps with research and review, but it cannot override a failed gate.

## Two-phase task runner

`run-task` provides a deterministic shell around execution:

```bash
mastermind run-task .mastermind/tasks/001-account-recovery/spec.md
```

The first invocation verifies the spec, records the baseline, and prepares the executor handoff. After implementation, the next invocation audits against that baseline. A held contract produces release-note material; drift or breakage keeps state for correction.

## Codegraph support

The workflow uses mmcg to answer repository questions such as:

- Does a claimed symbol exist?
- Which callers are affected?
- Does the change cross a component boundary?
- Which tests are structurally connected?
- Did the implementation alter more files than the spec allowed?

The graph is syntactic and bounded. Dynamic behavior and ambiguous same-name symbols remain limitations, so the workflow preserves precision notes and still runs the repository's full required test gate.

## Installed skills

The default workflow includes skills for:

- task planning and execution;
- codegraph research and project maps;
- change and test impact;
- critical review and structured reports;
- investigation ledgers;
- prompt refinement;
- agent security review;
- cross-client setup;
- verifiable audit attestations.

Run `mastermind list` to see the exact packaged bundle. Install the Claude and
Codex adapters with `mastermind install --client all`, then verify package,
manifest, and filesystem parity with `mastermind doctor --workflow --client
all`. Claude receives the seven spawnable subagent adapters; both clients
receive the portable skills.

## Audit evidence

An audit report can be sealed into a tamper-evident envelope and signed with Ed25519. For CI publication, use the privilege-separated workflows described in [Verifiable audits and GitHub Action](github-action.md).
