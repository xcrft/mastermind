# Design review packet

Use this only when verified work has a real design fork or when strict mode
requires an independent critic. Send evidence, not the planning transcript.

```markdown
# Design review: <title>

## Problem
<Required behavior and why it matters.>

## Proposed design
<Concrete files, symbols, boundaries, and compatibility behavior.>

## Evidence
- `mmcg_search <symbol>` → <location or no match>
- `mmcg_callers <symbol>` → <count and precision notes>
- `mmcg_impact <symbol>` → <material transitive impact>
- Repository constraint: <test/runtime/API/operations fact>

## Plausible alternatives
- <alternative> — rejected because <evidence-based reason>
- <alternative> — rejected because <evidence-based reason>

## Risks and rollback
- <failure mode> — <mitigation or detection>
- Rollback: <safe reversal or migration boundary>

## Questions for the critic
- <decision or assumption that needs pressure-testing>
```

Rules:

- Omit alternatives when there was no real choice; never invent filler options.
- Preserve stale-index, collision, truncation, and syntactic-graph caveats.
- Add a diagram only when a multi-component sequence or trust boundary is hard
  to explain in prose.
- One critic is the default. Use independent security/performance/simplicity
  lenses only when those dimensions can reach different conclusions.
