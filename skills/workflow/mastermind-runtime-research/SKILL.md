---
name: mastermind-runtime-research
description: Gather who already depends on a service, handler, or state owner before changing it, and name the runtime gaps the codegraph cannot span. Use before designing a change to an API, queue consumer, background job, migration, or shared state; it feeds the architecture review rather than replacing it.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - backend
    - runtime
    - mmcg
---

# Mastermind runtime research

[[mastermind-architecture-review]] reconstructs the runtime path and judges the
design. This runs first and does something narrower: find who already depends on
what you are about to change, and state where the graph stops being able to tell
you. Feed both into the review.

## Zero callers is not zero callers

Start here, because every other answer inherits it. The graph is syntactic, so
whole classes of real invocation produce no edge at all:

- **A queue, topic, or bus.** A producer and its consumer are two separate
  static islands. There is no edge between them and there never will be.
- **Framework registration.** A route table, a DI container, a decorator-based
  dispatcher, or a config-declared handler calls code the graph never links.
- **Reflection, dynamic dispatch, and interface indirection.** The call lands on
  a name the source never spells.
- **Cross-process and cross-language.** A worker invoked by a scheduler, or a
  service reached over HTTP, is outside the index entirely.

So `mmcg_callers` returning nothing on a handler means *no static caller was
found*, not *nothing calls this*. Report it that way, and go find the
registration, the topic name, or the schedule by reading. A change that looks
unreferenced is the single most expensive thing to get wrong here.

## Who depends on this today

```text
mmcg_api_surface src/orders/      # symbols under a prefix used from OUTSIDE it
mmcg_callers <symbol>             # static callers, with the caveat above
mmcg_impact <symbol> --depth 3    # transitive blast radius
mmcg_imported_by <symbol>         # importing files, including barrels
```

`mmcg_api_surface` is the one most often skipped and the one that matters most:
it reports what the rest of the codebase *actually* reaches into, independent of
what is declared public. A module can export twenty symbols and have three real
consumers, or export none and still be reached through a re-export. Change the
three, not the twenty.

## Who owns the state

Before changing a write path, find every writer — source-of-truth problems start
with more than one. `mmcg_callers` on the mutating function and
`mmcg_imported_by` on the store or model give the static set; the gaps above give
you the list of places to read for the rest.

Say plainly whether the change adds a second writer to state that already has
one, and whether the new write is authoritative or derived. That single sentence
is what the architecture review's source-of-truth check needs as input.

## What the change crosses

```text
mmcg_map <dir>                    # components, entry points, boundaries
mmcg_change_impact --since <ref>  # api_crossings, affected_components
mmcg_dependency_cycles            # does this add a cycle
```

A crossing is where guarantees change: in-process calls become at-least-once
delivery, ordering stops being free, and a partial failure becomes visible. List
the crossings the change touches so the review can reason about each one instead
of rediscovering them.

## Output

A short evidence packet, not a verdict:

- consumers found, by query, with the query named;
- state writers found, and whether the change adds one;
- boundaries crossed;
- **runtime gaps** — every place the graph could not answer, and what you read
  or still need to read instead.

The gap list is the most valuable line in the packet. An architecture review
built on "the graph showed no other callers" without that list is confident
about a question nobody asked.

## What this does not do

It does not judge the design, reconstruct the ordered runtime path, or produce a
verdict — that is [[mastermind-architecture-review]]. It does not establish
runtime behaviour: everything here is static evidence plus a list of what static
evidence cannot see. Tests, traces, and logs remain the only proof of what
actually runs.
