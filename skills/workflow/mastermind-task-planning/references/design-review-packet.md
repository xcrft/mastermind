# Design Review Packet

Canonical input format for spawning `mastermind-critic`. Copy this template, fill every section, and pass it as the critic's input. Do not send raw brainstorming — cold context is the point.

## When to use

- Before drafting a standard or strict spec.
- Whenever you are unsure which of several approaches to pick.
- Always for mandatory critic categories (auth, billing, migration, public-API, blast-radius ≥ 20).

---

## Template

```markdown
# Design Review: <short title>

## Problem

<1-2 sentences — what is broken, missing, or required. Concrete, not abstract.>

## Proposed design

<The approach you intend to implement. 1-3 paragraphs. Concrete enough to critique — name files, symbols, modules. Vague prose like "improve the architecture" gives the critic nothing to work with.>

## Alternatives considered

List ≥ 2 rejected alternatives for non-trivial changes. For each:

### Alternative A — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Rejected because:** <concrete reason tied to mmcg findings or project constraint>

### Alternative B — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Rejected because:** <reason>

### Picked approach — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Chosen because:** <concrete reason>

## Decision Matrix

Crystallizes the alternatives comparison. Fill one row per option above.

| Option | Correctness | Complexity | Blast radius | Migration risk | Observability | Reversibility | Verdict |
|---|---|---|---|---|---|---|---|
| A — <name> | pass | low | low | none | good | easy | reject |
| B — <name> | concern | medium | high | medium | weak | hard | reject |
| C — <name> | pass | medium | low | none | good | easy | chosen |

Column values: `pass / concern / fail` for Correctness; `low / medium / high` for complexity/blast/migration; `good / weak / none` for observability; `easy / medium / hard` for reversibility. Exactly one row gets `chosen`.

## Constraints

<Hard limits — programming language, runtime version, deadline, backward-compatibility requirements, ops constraints. The critic uses these to distinguish intentional tradeoffs from oversights.>

## mmcg snapshot

<Paste the mmcg evidence that grounds the design. Without this, the critic cannot verify claims and will flag dimension #7 as `fail`. Include at minimum:>

- `mmcg_search <primary_symbol>` → `<file:line>` (<brief description>)
- `mmcg_callers <primary_symbol>` → `<N> callers` (<impact summary>)
- `mmcg_impact <primary_symbol> --depth 3` → `<M> transitive` (if relevant)
- `mmcg_search <secondary_symbol>` → `<file:line>` (if touching ≥ 2 symbols)

For pure doc / config changes with no code symbols: write "no code symbols — mmcg not applicable".

## Risk surface

<Known unknowns and failure modes for the proposed design. The critic will probe these. Being explicit here prevents the critic from marking concerns you already know about as `fail`.>

- <risk 1>: <what could go wrong>
- <risk 2>: <what could go wrong>
```

---

## Completeness rules

- **mmcg snapshot is mandatory** for any change touching code symbols.
- **Decision Matrix is mandatory** for standard and strict specs.
- **Codeflow diagrams** are required for non-trivial alternatives (multi-module, auth/billing/data-flow, API boundary, migration, ≥ 3 files). All nodes must be real files, symbols, modules, or external boundaries — verified via `mmcg_search` or explicitly marked `[NEW]`. Generic boxes (`User → System → Database`) are AI slop and will cause critic `fail` on dimension #6.
- **≥ 2 alternatives** for non-trivial changes. For trivial changes (one-line fix, doc edit, mechanical rename), write "trivial change — single approach".
- **Constraints section must name the actual hard limits** — not vague preferences. If there are none, write "none beyond standard project conventions".

## What NOT to include

- The full brainstorming conversation — that imports your bias.
- Speculative alternatives you haven't thought through — shallow rejection reasons give the critic nothing to work with.
- Implementation details that belong in the spec's Scope section — this packet is for design validation, not execution instructions.

---

## For 3-lens critic panels (strict mode)

When spawning three critics in parallel, prepend a lens directive to the same packet:

**Security lens:** "Review primarily for authz/authn holes, secret exposure, injection, and privilege escalation. Other dimensions are secondary."

**Performance lens:** "Review primarily for latency, throughput, memory pressure, and lock contention. Other dimensions are secondary."

**Simplicity lens:** "Review primarily for YAGNI violations, unnecessary abstraction, complexity creep, and AI slop. Other dimensions are secondary."

Same packet body, same mmcg snapshot, same alternatives — only the lens directive differs. Spawn all three in one message so they run concurrently.
