---
name: mastermind-security-research
description: Enumerate what a change can reach and what enforces it — privileged operations and their callers, guards and the sites that apply them, and every module that reads a secret — before designing an auth, permission, or secrets change. Feeds the security review; produces a population and a gap list, never a verdict.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - security
    - workflow
    - authorization
    - mmcg
---

# Mastermind security research

[[mastermind-agent-security-review]] traces untrusted input to sinks and judges
the design. This runs first and answers a narrower question: what can reach the
privileged operation, what claims to enforce it, and which of those claims the
graph can actually establish.

The output is a population and a gap list. It is not a security verdict, and it
must not read like one.

## The graph is wrong in both directions here

This is the rule the whole skill hangs on, and security is where getting it
wrong costs the most.

**A missing edge is not an absence of access.** Middleware applied globally, a
route table, a decorator-based dispatcher, a DI container, or a proxy in front
of the service all invoke code the graph never links. A handler with no static
caller may be reachable from the internet.

**A present edge is not enforcement.** `mmcg_callers authGuard` returning a
handler proves the name appears in that function. It does not prove the guard
runs before the sensitive operation, that its result is checked, that it fails
closed, or that a branch does not skip it. Order, control flow, and error
handling are invisible to a call edge.

So the graph never answers "is this protected". It produces two lists, and the
difference between them is what a human has to read. Say that explicitly in the
output — a research packet that lets a reader infer safety from a query result
has done harm, not work.

## Enumerate what can reach the privileged operation

```text
mmcg_callers <privileged fn>        # direct callers
mmcg_impact <privileged fn> --depth 3   # transitive reach
mmcg_api_surface src/admin/         # what outside code actually reaches
```

Start from the operation that matters — the deletion, the payment, the role
change, the token mint — not from the endpoint. The reachable set is the
population you must account for; every member is either shown to be enforced or
listed as unread.

`mmcg_api_surface` is the empirical attack surface: symbols under a prefix that
code outside it reaches, independent of what is declared public. A module can
export little and still be reached through a re-export.

## Enumerate what claims to enforce

```text
mmcg_search <guard|middleware|policy fn>   # does the enforcement point exist
mmcg_callers <guard>                        # where it is statically applied
```

Then subtract. Reachable-and-privileged minus statically-guarded is the list to
read by hand, and it is the most valuable line in the packet. Do not present it
as a list of vulnerabilities — present it as the set whose enforcement is
unestablished, with the reason each one is unestablished.

## Enumerate who reads secrets

```text
mmcg_imported_by <secrets loader>   # modules importing it
mmcg_callers <secret accessor>      # call sites
```

Unlike enforcement, this one the graph does reasonably well: a secret read
through an imported accessor leaves an edge. Report the module set. The gaps
that remain are environment variables read directly, values injected by
configuration, and anything fetched at runtime from a vault — name those as
unenumerable here rather than implying the list is complete.

## Output

- **Reachable privileged operations** — the operation, its callers, the query used.
- **Enforcement points found** — the guard, the sites that statically apply it.
- **Unestablished** — reachable paths with no shown enforcement, each with why
  it is unestablished rather than a claim about it.
- **Secret readers** — modules and call sites, plus what could not be enumerated.
- **Gaps** — every place the graph could not answer: global middleware, route
  tables, DI, proxies, reflection, cross-service calls.

## Escalation

A change that crosses auth, authorization, secrets, permissions, tool
execution, or the supply chain is strict-mode work and requires the security
review. This research does not substitute for it, does not clear a change, and
does not lower the mode. If the packet's unestablished list is non-empty, that
is input to the review, not a finding of its own.

## What this does not do

It does not judge the design, construct abuse paths, or produce findings — that
is [[mastermind-agent-security-review]]. It establishes no runtime behaviour:
everything here is static structure plus an explicit account of what static
structure cannot see. Tests, traces, and a read of the enforcement code remain
the only proof that a boundary holds.
