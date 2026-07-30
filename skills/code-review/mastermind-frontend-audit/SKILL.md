---
name: mastermind-frontend-audit
description: Read-only review of a finished UI change against the codegraph — a component nothing renders, a props contract changed without its callers, a reinvented component, or a raw value where the design system has a token. Use after implementing React or Vue work; triggers "review this UI change", "check the component", or a finished frontend diff.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - code-review
    - frontend
    - components
    - mmcg
---

# Mastermind frontend audit

A UI change can be structurally wrong while looking fine, and structurally fine
while looking wrong. This review covers only the first kind, because only the
first kind can be checked against evidence.

You are read-only. You report findings and change nothing.

## Collect the delta first

```bash
git diff --name-status <baseline>
git diff <baseline> -- <changed component files>
```

Then ground each changed component in the graph. Never assert a structural fact
from reading the diff alone — the diff shows what the change did, the graph
shows what the rest of the codebase expects.

## The four checks

### 1. A component nothing renders

For every component the change added, `mmcg_callers <Name>` gives the components
that render it. Zero callers on a brand-new component means it was never wired
in — the most common way a UI change passes review and ships as nothing.

Do not report this as dead code. Check the paths the graph cannot see first: a
route table, a lazy `import()`, a story or test file, an export from a barrel
consumed outside the indexed tree, or a Vue component auto-imported by build
tooling. If one of those explains it, the finding is `could_not_verify`, not a
defect.

### 2. A props contract changed without its callers

`mmcg_symbols_changed_since <baseline>` reports `signature_changed` for a
component whose parameters moved. A component's signature carries its props, so
this is a public-contract change. Cross it with `mmcg_callers`: every caller not
touched by the diff is a consumer left on the old contract.

A removed or newly-required prop breaks them. An added optional prop does not.
Say which, and name the callers — a count is not evidence.

### 3. A reinvented component

For every component the change added, search the name and its neighbours
(`Button` → `BaseButton`, `AppButton`, `UiButton`) and check
`mmcg_centrality --prefix <component dir>` for what the codebase already leans
on. A new component that duplicates the role of an existing one is a finding
even when the code is correct, because the duplicate is what future changes will
diverge from.

The bar is a named existing component with the same role — not a suspicion that
something similar might exist. If you cannot name it, there is no finding.

### 4. A raw value where the system has a token

Scan the added lines for hex colours, `rgb(...)`, raw pixel spacing, magic
`z-index`, and inline font stacks. Then confirm a token actually exists for that
value — a variable, theme key, or design-system constant. A raw value with no
token to replace it is not a finding; a raw value that shadows an existing token
is.

## Evidence rule

**A finding names the file, the line, and the query result that establishes it.**
"`Card` has no callers" is a claim; "`mmcg_callers Card` → 0, and `Card` appears
in no route table or story file" is a finding. Without the query, downgrade it to
`could_not_verify` or drop it.

## Restraint

Finding nothing is a normal outcome. A change that reuses existing components,
leaves contracts intact, and uses tokens is a clean result, and reporting it as
clean is the correct output.

Do not pad the report to look thorough. A reviewer that always finds four things
is a reviewer nobody reads.

## What this review cannot judge

Visual fidelity, spacing, responsive behaviour, and interaction quality are not
structural facts and are not in scope. They need the running application, and a
`clean` verdict here says nothing about them. Do not imply the change matches a
design.

## Output

````markdown
## Frontend audit: <clean | findings>

### Unrendered components
- `<file>:<line>` — `<Component>` — `mmcg_callers` → 0; checked: <routes, stories, lazy imports, barrels>

### Contract changes
- `<Component>` — `<old signature>` → `<new signature>` — callers left on the old contract: `<file>`, `<file>`

### Duplicates
- `<file>:<line>` — `<New>` duplicates `<Existing>` at `<file>` — <the role they share>

### Raw values
- `<file>:<line>` — `<value>` — existing token: `<token>`

### Kept
- <what the change reused or preserved that was worth reusing>

### Could not verify
- <claim> — <which query was inconclusive and why>

<!-- mastermind:frontend-audit-begin -->
```yaml
baseline: <ref>
verdict: clean | findings
components_added: <N>
components_changed: <N>
flagged: <N>
findings:
  - file: <path>
    symbol: <name>
    kind: unrendered | contract_change | duplicate | raw_value | could_not_verify
```
<!-- mastermind:frontend-audit-end -->
````

`flagged` counts every `findings` entry. `verdict` is `findings` when `flagged`
is above zero and `clean` otherwise. Keep every heading even when empty, and
never omit the sentinel block.

## Boundaries

- Read-only. Never edit source, reports, `audit.md`, `_lessons.md`, or
  `state.json`; never stage, commit, or revert.
- Report findings; do not apply them.
- This is not a contract audit. It produces no `held` / `drift` / `broken`
  verdict and does not replace the controller's post-flight.
