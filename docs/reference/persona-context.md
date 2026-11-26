# Agent context packets

An agent gets one bounded packet that keeps your personal advice apart from
project evidence and code. `mmcg_context`, `mastermind context preview` and the
private Lens Profiles view build it with the same read-only assembler. Role,
workflow, repository and task paths decide what goes in. Profile storage,
access and review are in the [persona reference](persona.md).

## Preview a packet

```bash
mastermind context preview --since HEAD --role executor --workflow strict \
  --path src/service.rs --query transport --budget-tokens 8000

mastermind context preview --since HEAD --role executor --profile-client my-client

mastermind ui --since HEAD --role executor --workflow strict \
  --context-path src/service.rs --query transport --profile-client my-client
```

The MCP call with the same selection:

```json
{"name":"mmcg_context","arguments":{"since":"HEAD","role":"executor","workflow":"strict","paths":["src/service.rs"],"query":"transport","budget_tokens":8000}}
```

| Route | Root and audience | Access |
|---|---|---|
| MCP | Server root and `MMCG_PROFILE_CLIENT`. Arguments cannot override them | A matching existing [profile grant](persona.md#profile-access) |
| CLI or Lens | Selected repository and explicit `--profile-client` | A matching existing profile grant |

A preview refreshes no index, mines nothing, calls no model, creates no grant
and creates no store. Denied, unavailable or inapplicable personal context
never falls back to `style.md`.

## Layers and selection

| Layer | Content and selection |
|---|---|
| `person` | Applicable reviewed preferences and habits and Git observations, filtered by `paths`, role and workflow. Granted readers also see bounded review metadata |
| `project` | Root CONTEXT sections and candidate claims selected by `query` |
| `documentation` | Relevant Markdown sections. `not_requested` without a query |
| `code` | Repository diff against `since`. Personal path filters do not narrow it |
| `work` | At most 20 task records with invocation metadata pinned to the same recorded iteration. Completion is history, with `current_checkout: not_verified` |

| Layer state or property | Consumer contract |
|---|---|
| Every layer | Keeps status, verification metadata, data digest and omission reasons |
| Stale code, project text or document text | Withheld |
| Missing, unknown or omitted layer | A gap, never an empty verified result |
| Current source | Shows freshness, not truth. Project claims stay candidates even when Markdown says `active` |
| `consistency: independent_layer_snapshots` | Each layer was read at a different moment. There is no atomic snapshot across stores |
| Repository content | Untrusted evidence that grants no permissions |
| Personal advice | Ranks below explicit tasks, code and tooling contracts and product requirements |
| Explicit paths with no matching language detector | Other languages' conventions and language-scoped preferences are left out. An unknown extension does not turn the path filter off |

## Who gets which slice

| Consumer | Slice and when it is selected |
|---|---|
| Prompt hook | A planner slice on every admitted prompt while delivery is on. The rules are in [native capture](persona.md#native-capture). The planning workflow calls `mmcg_profile` again when its known scope differs |
| Native executor (`run-task --exec`) | The executor slice for the approved scope. The audience is the project's configured Claude profile client, `--profile-client` overrides it, and it is saved as `state.profile_client` for review and completion resumes. After task and client admission, committed Git observations are refreshed locally with the stored author selector. Then the packet is supplied and its receipt and revision are pinned before delivery |
| Manual auditor workflow | Can request an auditor slice for communication and focus |
| Native semantic reviewer | No MCP and no personal profile, see the [reviewer contract](task-runtime.md#native-semantic-reviewer) |

Completion rechecks read access, so a saved audience cannot undo a revocation.
Reviewed task completion refreshes Git observations again and keeps an optional
`state.profile_refresh` receipt with the exact Git snapshot and the published
profile revision. This needs no model and accepts no claim. Uncommitted edits
are left out, a refresh failure is recorded apart from the task verdict, and a
read-only preview never triggers a refresh. Personal context never decides
correctness or grants tools.

## Revisions and budget

| Field or condition | Contract |
|---|---|
| `context_revision` | SHA-256 of the original compact UTF-8 packet JSON with only that top-level member removed. Every other byte and the key order stay |
| Verification | Hash the received wire bytes. Parsing and reserializing floats can change their spelling |
| `layer.revision` | Hashes the serialized layer data before budget omission |
| UI | Keeps the packet it received |
| `budget_tokens` | 1,024-16,000, default 8,000 |
| Estimate | Compact packet bytes divided by 4, metadata included. This is not model tokenization, and MCP framing comes on top |
| Over budget | Whole layers are omitted in this order: documentation, person, project, code, work |
| Omitted layer | Keeps its revision and verification summary |
| Metadata alone over budget | Fails. A claim, exception or citation is never cut |

| Index state | Result |
|---|---|
| No index file | The packet is built without index data |
| Index exists but does not open, for example because a newer mmcg wrote it | `context_index_unavailable` |
| Index kept changing while its snapshot was copied | `context_index_snapshot_unavailable`, after one retry |

## Private Lens view

| Surface | Contract |
|---|---|
| Profiles tab | Selected advice, source freshness, code evidence and task history |
| Review queue | At most 8 unattested hook draft IDs in the selected repository, with source and influence status and counts. No candidate wording or quotes |
| Queue access | An existing exact-root audience grant, also for a profile with no accepted preferences yet. Denial or unavailable evidence returns no private rows |
| Recorded invocation | Delivery revisions and mediation counts for the scanned iteration. The current checkout is not revalidated |
| Model use | Unknown, even when matching context bytes were offered to a process |
| Counts | Describe selection and corpus limits, not character or skill |
| `/api/context` | Separate from `/api/lens`. Loopback, same-origin, read-only, `no-store` |
| Private data lifetime | Loaded on demand and cleared on refresh or when you leave the tab |
| `LensSnapshot` and portable HTML | Leave personal context out. Profiles is disabled in exports |
| Preview | No delivery receipt and no proof of model use |
| Native hook exposure | `mmcg_context` counts as possible profile exposure, so later echoes cannot be independent habit evidence |

## Compose a handoff from individual tools

Installed role instructions can also fetch each part on its own:

| Component | Request | Keep |
|---|---|---|
| Code | `mmcg_brief` with role, baseline and `budget_tokens: 2000` | Revision, structural and history tokens, precision and omissions |
| Project | `mmcg_project_profile` with a query or `top: 2` | Citations, freshness, candidate and review status |
| Documentation | `mmcg_docs` with a query and `top: 2` | Paths, line spans, coverage, freshness and retrieval limits |
| Person | `mmcg_profile` with paths, role and workflow. `budget_tokens` defaults to the [project profile budget](persona.md#profile-access) and an explicit value wins | Claim and review IDs, store and view revisions, source verification and omissions |

Feedback is ranked as in [profile delivery](persona.md#profile-delivery) and
trimmed one rule at a time from the least important end.

| Handoff step | Rule |
|---|---|
| Selection | Use the real baseline and scope. The receiving agent picks its role slice |
| Budget | Measure the final serialized object, metadata included. 8,000 units are 32,000 UTF-8 bytes |
| Over budget | Narrow the queries or budgets and fetch again, or leave out an optional component whole with its reason, freshness and tool identity |
| Missing task-critical evidence | Resolve the gap first |
| Changed documentation | Reindex it |
| Changed persona source | Collect, rebind and review it again |

A current code graph does not refresh documentation or persona evidence.

## Regression check

From the repository root:

```bash
cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --test persona_transcripts_cli \
  --locked context_composition -- --nocapture
```

The synthetic fixture checks roles, budgets, access denial and independent
source freshness through a real local MCP server with an isolated profile and
home directory.
