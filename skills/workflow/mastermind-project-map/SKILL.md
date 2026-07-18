---
name: mastermind-project-map
description: Build an evidence-backed architecture briefing with `mastermind map` or `mmcg_map`. Use when exploring an unfamiliar repository, identifying components and entry points, choosing a reading order, inspecting dependency cycles or hotspots, or explaining codegraph precision and truncation limits.
metadata:
  version: 0.2.0
  authors: [mastermind]
  tags: [workflow, mmcg, architecture]
---

# Mastermind Project Map

Use the deterministic map as the source of claims. Do not reconstruct a map
from filenames or model intuition when the backend is available.

## Workflow

1. Confirm the repository has a fresh index with `mastermind status`. If it is
   absent or stale and the request permits local indexing, run
   `mastermind index .` before mapping.
2. Choose the narrowest useful repository-relative scope. Use `.` only for a
   repository-wide briefing.
3. Run JSON first:

   ```bash
   mastermind map PATH --format json --depth 2 --top 20
   ```

   Add `--production-only` for architecture work where tests, fixtures,
   examples, generated output, vendored code, and dependency trees would
   otherwise dominate the briefing. With MCP, pass `production_only: true`.

   Use `mmcg_map` instead when MCP is the active interface. Treat both as the
   same schema-v1 engine.
4. Check `scope`, every collection's `truncated`/`truncation_reason`, `limits`,
   and `precision_notes` before interpreting results.
5. Produce a reading order: entry points first, then central symbols, component
   boundaries, and cycles. Cite repository-relative files and symbol lines.

Treat directory-derived components as navigation groups, not proven semantic
subsystems. Prefer the explicit production filter over manually ignoring
fixture/example/generated results. Keep those paths when the user is asking
about tests, examples, or packaging. De-prioritize high-collision symbol names.
A `top_probe` proves
only that more rows exist; do not imply the backend can return an uncapped full
list. Narrow the path or change `top` within its supported range instead.

Use text for a compact terminal briefing and Mermaid only when the user asks
for a diagram. Do not infer runtime entry points from a `heuristic` label.

## Reporting contract

Separate:

- indexed facts: languages, files, symbols, callers, cycles;
- heuristics: filename entry points;
- approximations: syntactic edges, name collisions, work-limit skips.

If the response is truncated, describe what is missing. If the command/tool is
unavailable, report that the deterministic backend is missing; do not simulate
its output.

Never emit absolute paths, claim dynamic dispatch coverage, or present a
Mermaid projection as additional analysis.
