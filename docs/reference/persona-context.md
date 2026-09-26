# Composing context for an agent

An agent duty (`planner`, `executor`, `auditor`), workflow mode, repository and
task paths select context.

The composition rules are embedded in the installed planner/executor skills and
executor/auditor agents. They do not require this checkout's documentation.
Composition and the combined cap are workflow responsibilities; this release
does not add a single aggregate MCP endpoint.

Keep four components separate in the handoff:

| Component | Request | Retain |
|---|---|---|
| Code | `mmcg_brief` with actual `role`, `since`, `budget_tokens: 2000` | Baseline/head, structural/history checked tokens, graph precision and omissions |
| Project | `mmcg_project_profile` with a relevant query or `top: 2` | Source citations/digests, freshness, candidate/unknown review status |
| Documentation | `mmcg_docs` with the task query and `top: 2` | Paths/line spans, coverage, freshness, retrieval-only caveats |
| Person | `mmcg_profile` with actual `paths`, `role`, `workflow`, `budget_tokens: 1500` | Claim IDs/review revisions, store/view revisions, source verification, omissions |

For example, an executor of a strict task changing `src/service.rs` calls:

```json
[
  {"name":"mmcg_brief","arguments":{"role":"executor","since":"HEAD","budget_tokens":2000}},
  {"name":"mmcg_project_profile","arguments":{"top":2}},
  {"name":"mmcg_docs","arguments":{"query":"transport","top":2}},
  {"name":"mmcg_profile","arguments":{"paths":["src/service.rs"],"role":"executor","workflow":"strict","budget_tokens":1500}}
]
```

Substitute the task's real baseline, scope paths and selected mode. A planner
uses `role: planner`; the independent reviewer uses `role: auditor`. A handoff
keeps its original role and scope: the receiving agent retrieves its own slice.

## Shared context budget

Use a default handoff cap of 8,000 size-estimate units (32,000 UTF-8 JSON bytes)
for the combined object, including its role/mode/scope and provenance metadata.
This byte-based estimate is not a model-specific tokenizer guarantee.
The code/person allocations above consume at most 3,500 units; the remaining
space admits the bounded project and documentation results plus the envelope.

Measure the serialized combined handoff. If it exceeds the cap, narrow the docs
query and lower `top`, or reduce the brief/profile request budget within their
supported limits and retrieve again. Keep provenance and caveats intact.
If an optional component still cannot fit, replace the entire component with
`omitted: budget` and retain its tool, freshness/verification status and reason.
Never cut JSON, a claim's exception or a citation to make it fit. Missing
task-critical evidence remains an explicit gap to resolve, not an empty result.

## Freshness and authority

- Source-current is different from semantically correct or reviewed. Project
  Markdown claims remain candidates even when their source says `active`.
- Code freshness does not refresh Markdown or a personal source. Re-index changed
  documentation, and recollect/rebind/review changed persona evidence separately.
- Personal `store_revision` identifies SQL inputs. `profile_revision` identifies
  a scoped live view; schema-5 `style.md` carries a separate publication revision.
- Missing/denied profile access or no applicable claim yields no personal context.
  Never substitute `style.md`, local notes or a larger unscoped profile.
- The explicit task, repository code/tooling and product contract take precedence
  over personal advice. Git code-shape/range observations do not prove habits.

The executable synthetic scenario is
`tests/persona_transcripts/composition.rs` in the mmcg crate:

```bash
cargo test --test persona_transcripts_cli --locked context_composition -- --nocapture
```

It exercises the real MCP server and isolated profile/home, all three roles,
combined budget, denied access, source changes and independent document freshness.
It performs no mining of the developer's real history.
