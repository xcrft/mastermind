---
name: mastermind-component-research
description: Ground a UI change in the codegraph before writing it — find whether the component already exists, who renders it, and what its props contract is. Use before creating or changing any React or Vue component; triggers "add a component", "change these props", "build this screen", or a Figma frame handed over for implementation.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - frontend
    - components
    - mmcg
---

# Mastermind component research

The default failure of an agent writing UI is not bad markup. It is writing a
component that already exists, or changing a props contract without seeing who
depends on it. Both are structural questions, and the codegraph answers them.

Answer these before writing. Evidence is a query result, not a recollection.

## Does it already exist?

```text
mmcg_search "Button"                    # exact name
mmcg_files --prefix src/components      # what the component directory holds
mmcg_centrality --prefix src/components # the components everything already leans on
```

Search the name you were about to invent, then the words around it. A `Button`
may live as `BaseButton`, `AppButton`, or `UiButton`, and a design system rarely
names things the way a ticket does. `mmcg_centrality` is the fastest read of a
component library you do not know: the highest in-degree symbols are what the
codebase actually builds on.

Report the search as evidence — the names tried and what came back. "I looked
and found nothing" without the queries is not a finding, and reinvention is the
most expensive mistake available here.

## Who renders it?

```text
mmcg_callers "Button"          # components that render it
mmcg_impact "Button" --depth 3 # transitive blast radius
mmcg_imported_by "Button"      # files importing it, including barrels
```

`mmcg_callers` answers "who renders this": JSX usage (`<Button />`) and Vue
template usage (`<base-button />`) are call edges from the containing component.
A component with callers is a shared contract — treat a change to it as a change
to every caller, and say how many there are.

Zero callers means one of three things, and they are not the same: the component
is new, it is dead, or it is reached in a way the graph cannot see. Check the
invisible paths before concluding it is dead — route tables, lazy `import()`,
story and test files, and Vue components auto-imported by build tooling never
produce an edge.

## What is the contract?

```text
mmcg_outline src/components/Button.tsx   # the file's symbol tree
mmcg_search "ButtonProps"                # the props type, if it is named
```

A component's signature carries its parameters, so the props are visible in the
symbol itself: `Button({ variant, onClick }: ButtonProps)`. Read it before
changing it. Adding a required prop is a breaking change for every caller
`mmcg_callers` just listed, and the codegraph will report it as
`signature_changed` after the fact whether or not you planned for it.

## Where does the design source say it already exists?

When the change comes from a design tool rather than from prose, resolve the
mapping before implementing. Figma's Code Connect map states which repository
components a design already corresponds to, and design variables give token
names instead of raw values. Both convert an unverifiable instruction ("match
the mockup") into names that `mmcg_search` can confirm and an audit can check.

Carry the resolved names into the task contract. Acceptance criteria that say
"uses `BaseButton` with the `primary` variant and spacing from `--space-*`" are
verifiable; "looks like the design" is not, and nothing downstream can check it.

## What this does not answer

The graph is syntactic. It reports that a component exists, who renders it, and
what its parameters are. It says nothing about whether the result looks correct,
whether the spacing matches, or whether the interaction feels right. Those need
the running application, and they belong in the report as browser observations
rather than as structural claims.

Preserve the standard caveats: name collisions across modules, stale index,
truncation, and the syntactic limits of call resolution. Component detection
follows the tag convention — a lowercase React component is invisible, and a
Vue component auto-imported without an `import` statement has no edge.
