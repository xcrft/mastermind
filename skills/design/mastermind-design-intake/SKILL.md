---
name: mastermind-design-intake
description: Turn a design handoff into a task contract that can actually be checked — named source, resolved components, token names instead of raw values, and an explicit list of what stays unverifiable. Use when a Figma frame, mockup, or design spec is handed over for implementation.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - design
    - frontend
    - planning
    - figma
---

# Mastermind design intake

"Make it look like the design" cannot be verified by anything. Not by a test,
not by the codegraph, not by a reviewer six weeks later. A design handoff has to
be converted into names before it becomes acceptance criteria, or the contract
is decorative.

This is a planning step. It produces the contract; it does not implement it.

## Name the source

Record the exact design source in the spec: file key and node id, or the frame
URL. A spec that says "per the mockup" is unauditable — nobody can re-derive
what was asked, and a design that moved on cannot be distinguished from an
implementation that drifted.

If no stable reference exists, say so in the contract rather than pretending
there is one.

## Resolve components before writing criteria

A design tool that carries a code mapping states which repository components a
design already corresponds to. Read it, then confirm each mapped name against
the repository itself with `mmcg_search` — a mapping can name a component that
was since renamed or deleted.

**A design element missing from the mapping does not mean the component is
missing from the codebase.** Coverage is usually partial, and treating an
unmapped element as "new" is exactly how a fourth `Button` gets written. Fall
back to [[mastermind-component-research]] for anything unmapped, and record what
you searched.

The output of this step is a list of component names the implementation must
use, each confirmed to exist.

## Resolve tokens before writing criteria

Pull the design variables — colours, spacing, typography, radii — and carry
**token names** into the contract, never the resolved values. `--color-danger`
is checkable against the diff; `#e5484d` is a number that will drift the moment
the palette changes, and a reviewer cannot tell an intentional value from a
hardcoded one.

Where the design uses a value with no token behind it, that is a finding for the
design system, not something to hardcode silently. Say which it is.

## Write criteria that can fail

Each acceptance criterion should name a component, a token, a state, or an
observable behaviour:

- *Verifiable:* "renders `BaseButton` with `variant="primary"`"; "spacing comes
  from `--space-*`"; "empty state renders when `items` is empty"; "the disabled
  control is not focusable".
- *Not verifiable, and must not sit in acceptance criteria:* "matches the
  design"; "looks polished"; "spacing feels right".

## Park what stays unverifiable

Visual fidelity is real work and a real risk — it just is not a checkable
criterion. Put it in an explicit section of the contract: what a human needs to
look at, at which viewports, and against which frame. That keeps it visible
instead of smuggling it into criteria that a mechanical gate will wave through.

The same applies to motion, easing, and anything whose correctness is a
judgement call.

## What this step does not do

It does not implement, and it does not promise the result will match. A contract
built this way makes the *structural* half checkable — the right components, the
right tokens, the right states. The visual half still needs eyes, and
[[mastermind-browser-verification]] is where those observations get recorded as
observations rather than as claims.
