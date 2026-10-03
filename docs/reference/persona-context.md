# Agent context packets

`mmcg_context`, `mastermind context preview` and the private Lens Profiles view
share a read-only assembler. Role, workflow, repository and task paths select
context while keeping personal advice separate from project evidence.

## Preview

```bash
mastermind context preview --since HEAD --role executor --workflow strict \
  --path src/service.rs --query transport --budget-tokens 8000

mastermind context preview --since HEAD --role executor --profile-client my-client

mastermind ui --since HEAD --role executor --workflow strict \
  --context-path src/service.rs --query transport --profile-client my-client
```

The MCP equivalent is:

```json
{"name":"mmcg_context","arguments":{"since":"HEAD","role":"executor","workflow":"strict","paths":["src/service.rs"],"query":"transport","budget_tokens":8000}}
```

| Route | Root and audience | Access |
|---|---|---|
| MCP | Server root and `MMCG_PROFILE_CLIENT`. Arguments cannot override them | Matching existing [profile grant](persona.md#profile-access) |
| CLI or Lens | Selected repository and explicit `--profile-client` | Matching existing profile grant |

| Preview boundary | Behavior |
|---|---|
| Writes or execution | No index refresh, mining, model call, grant creation or absent-store creation |
| Denied, unavailable or inapplicable personal context | No fallback to `style.md` |

## Layers and selection

| Layer | Content and selection |
|---|---|
| `person` | Applicable reviewed preferences/habits and Git observations, filtered by `paths`, role and workflow. Granted readers can also see bounded review metadata |
| `project` | Root CONTEXT sections and candidate claims selected by `query` |
| `documentation` | Relevant Markdown sections, `not_requested` without a query |
| `code` | Repository diff against `since`. Personal path filters do not narrow this diff |
| `work` | At most 20 task records with invocation metadata pinned to the same recorded iteration. Completion is historical, `current_checkout: not_verified` |

| Layer state or property | Consumer contract |
|---|---|
| Every layer | Retains status, verification metadata, data digest and omission reasons |
| Stale code, project text or document text | Withheld |
| Missing, unknown or omitted layer | A gap, not an empty verified result |
| Current source | Establishes freshness, not semantic truth. Project claims remain candidates even if Markdown says `active` |
| `consistency: independent_layer_snapshots` | Readers ran at different instants. No atomic cross-store snapshot |
| Repository content | Untrusted evidence, no permission grants |
| Personal advice | Subordinate to explicit tasks, code/tooling contracts and product requirements |
| Explicit paths with no applicable language detector | Excludes other languages' conventions and language-scoped preferences. Unknown extensions do not disable the path filter |

The native prompt hook offers a planner slice on every admitted prompt when
profile delivery is enabled. It selects literal task paths without a provider;
missing paths withhold language-scoped advice. A current explicitly bound
continuation can select the spec's paths and workflow. The planning workflow
retrieves `mmcg_profile` again when its known scope differs from that selection.
The native task controller automatically uses the project's configured Claude
profile audience for `run-task --exec`, with `--profile-client` as an override.
After task and native-client admission, it refreshes committed Git observations
locally using the stored author selector, then supplies the executor's context packet and
pins its receipt/revision before delivery. The manual auditor workflow can
request an auditor slice for communication and focus. The native independent
semantic reviewer runs without MCP or a personal profile. Personal context
does not decide correctness or authorize tools. `style.md` remains a local
inspection snapshot; it is never a replacement for denied MCP access.

Reviewed task completion also refreshes Git observations and retains an optional
`state.profile_refresh` receipt with the exact Git snapshot and published profile
revision. No model or personal-claim acceptance is involved. Uncommitted edits
are excluded. Refresh failures are recorded separately from the task verdict;
read-only context previews do not trigger refresh.

## Revisions and budget

| Field or condition | Contract |
|---|---|
| `context_revision` | SHA-256 of original compact UTF-8 packet JSON with only that top-level member removed. Preserve every other byte and key order |
| Verification | Hash received wire bytes. Parsing and reserializing floats can change spelling |
| `layer.revision` | Hashes serialized layer data before budget omission |
| UI | Retains the received packet |
| `budget_tokens` | 1,024–16,000, default 8,000 |
| Estimate | Compact packet bytes ÷ 4, including metadata. No model tokenization guarantee, MCP framing is additional |
| Over budget | Omit whole layers in order: documentation → person → project → code → work |
| Omitted layer | Retain revision and verification summaries |
| Metadata alone exceeds budget | Fail. Never cut a claim, exception or citation |

## Private Lens view

| Surface | Contract |
|---|---|
| Profiles tab | Selected advice, source freshness, code evidence and task history |
| Review queue | At most 8 unattested hook draft IDs in the selected repository, source/influence status and counts. No candidate wording or source quotes |
| Queue access | Existing exact-root audience grant, including a profile with no accepted preferences yet. Denial or unavailable evidence returns no private rows |
| Recorded invocation | Delivery revisions and mediation counts for the scanned iteration. Does not revalidate the current checkout |
| Model use | Unknown, even when matching context bytes were offered to a process |
| Counts | Describe selection and corpus limits, not character or competence |
| `/api/context` | Separate from `/api/lens`, loopback, same-origin, read-only, `no-store` |
| Private data lifetime | Lazy load. Clear on refresh or leaving the tab |
| `LensSnapshot` and portable HTML | Exclude personal context. Profiles is disabled in exports |
| Preview | No delivery receipt or proof of model use |
| Native hook exposure | `mmcg_context` counts as possible profile exposure. Following echoes cannot supply independent habit evidence |

## Compose a handoff from individual tools

Installed role instructions can also retrieve components separately:

| Component | Request | Preserve |
|---|---|---|
| Code | `mmcg_brief` with role, baseline and `budget_tokens: 2000` | Revision, structural/history tokens, precision and omissions |
| Project | `mmcg_project_profile` with query or `top: 2` | Citations, freshness, candidate/review status |
| Documentation | `mmcg_docs` with query and `top: 2` | Paths, line spans, coverage, freshness and retrieval limits |
| Person | `mmcg_profile` with paths, role, workflow and `budget_tokens: 1500` | Claim/review IDs, store/view revisions, source verification and omissions |

| Handoff step | Rule |
|---|---|
| Selection | Use the actual baseline and scope. Receiving agent selects its role slice |
| Budget | Measure the final serialized object, including metadata. 8,000 units correspond to 32,000 UTF-8 bytes |
| Over budget | Narrow queries/budgets and fetch again, or omit an optional component whole with its reason, freshness and tool identity |
| Missing task-critical evidence | Resolve the gap |
| Changed documentation | Reindex it |
| Changed persona source | Recollect, rebind and review it |

A current code graph does not refresh documentation or persona evidence.

## Regression check

From the repository root:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --test persona_transcripts_cli \
  --locked context_composition -- --nocapture
```

The synthetic fixture checks roles, budgets, access denial and independent source
freshness through a real local MCP server with an isolated profile/home.
