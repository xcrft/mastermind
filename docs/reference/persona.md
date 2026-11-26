# Persona storage, mining and review

Mastermind keeps a private profile of how you work: reviewed preferences and
habits, plus observations from your Git history. This page lists the
`mastermind miner` commands, how claims get reviewed and reach agents, and the
limits. Set up capture with the [persona guide](../guides/persona-hooks.md).
What the evidence can show is in [persona evidence](persona-evidence.md).

## Storage and terms

| Data | Location | Use |
|---|---|---|
| Hook events, coverage, drafts and exposure records | `~/.mastermind/persona-events.db` | Local capture journal |
| Archived episode payloads | `~/.mastermind/persona-archive/*.json` | Private sources kept outside the journal |
| Managed worker configuration and runs | `~/.mastermind/persona-workers/<id>/` | Optional separate processor state |
| Evidence, claims, reviews and grants | `~/.mastermind/style.db` | User-global profile, at most 64 MiB |
| Generated profile | `~/.mastermind/style.md` | Local inspection view, at most 1 MiB |
| Unreviewed deep interpretation | `style.deep-candidate-*.md` beside the profile | Local review input |
| Project context | Repository `CONTEXT.md` | Separate project evidence, see `mmcg_project_profile` |

| Term | Meaning |
|---|---|
| Episode | One captured interaction: a prompt, the response and the tool calls in between. It ends at `Stop`, the end of a response |
| Capture generation | A numbered period of capture for one client and project. Recovery after a failure starts a new one, and older ones keep their gaps |
| Fence | A marker under `~/.mastermind/.persona-capture` that stops capture for one client and project after a failed delivery, until you recover |
| Exposure receipt | A record that profile or refiner text was offered before an event, so later text counts as influenced |
| Review pin | The exact claim revision and evidence set you approved. Any change to them drops the pin |

Agents read the profile only through `mmcg_profile` and context packets. A
manual `style.md` edit is kept as fenced unreviewed notes and cannot accept a
preference or observe a habit. `store_revision` identifies the SQL inputs and
`profile_revision` the selected claims after source checks. If SQL committed but
the Markdown view failed to publish, retry the same command, and do not create a
duplicate claim.

Both databases keep their format version in SQLite `user_version`, and a store
written by a newer version is refused. `style.db` is format 2. The first profile
write converts format 1: replaced claims become `rejected` and renewed habits go
back to `candidate`, each with a review event. A read-only command on an
unconverted store fails until that write.

## Git observations

```bash
mastermind miner profile .
mastermind miner profile . --author "Ada Lovelace"
mastermind miner profile . --deep
```

| Input or measurement | Contract |
|---|---|
| `--author` | Literal substring of the Git author name or email |
| History | Up to 2,000 recent authored non-merge commits |
| Diff sample | Up to 400 source commits, selected in monthly rounds |
| Bulk commit over 2,000 added source lines | Commit-message evidence only |
| Libraries | Parsed imports in committed source that intersect added lines. Standard and first-party modules are left out |
| Tooling conventions | Kept apart from personal observations |
| Linked worktrees and subdirectories | One repository contribution. Independent clones keep their own provenance and each SHA counts once |
| Confidence tiers | `medium` and `high`, defined in [commit support](persona-evidence.md#commit-support-and-wilson-score) |
| `--deep` | Runs `claude -p` with bounded input and output and a 180 second limit. The output is a candidate file outside the active profile. On Windows only the direct child is stopped at the timeout |
| `--force` | Replaces the whole profile, including kept prose |

## Profile access

```bash
mastermind miner access grant . --client claude
mastermind miner access list
mastermind miner access revoke . --client claude
```

| Access condition | Result |
|---|---|
| MCP audience | `MMCG_PROFILE_CLIENT` plus a matching project root and client grant |
| Granted data | The whole global profile, including Git observations from every mined repository. There are no per-source grants |
| Selection | Project, path, language, role and workflow filters run before the selected claims' sources are read |
| Private fields | Quotes and local source paths are withheld |
| Missing access or absent store | `access_denied`. No store is created, and there is no `style.md` fallback |
| Budget | `init --profile-budget <256-8000>`, default 4,000, used when the caller passes no `budget_tokens` |

### Profile delivery

| Result field or choice | Contract |
|---|---|
| Feedback order | Global, path, language, repo/project, role, workflow. Within a tier the newest `last_at` comes first |
| Admission | Reviewed feedback and habits alternate, then Git diagnostics. Whole entries only, so an oversized entry is skipped and never loses its conditions |
| `feedback_total` | Eligible feedback before the 64-rule source check and the budget |
| `omitted`, `omitted_counts` | Lists with no delivered entry, and how many entries each list withheld |
| `doctor` budget advice | A byte-based estimate of the budget that fits all verified feedback and habits, not measured model tokens |

## Context packets

An agent can get one bounded packet that keeps personal advice apart from
project evidence and code. `mmcg_context`, `mastermind context preview` and the
Lens Profiles view build it the same way. A preview refreshes no index, mines
nothing, calls no model and creates no grant or store.

```bash
mastermind context preview --since HEAD --role executor --workflow strict \
  --path src/service.rs --query transport --profile-client my-client
mastermind ui --since HEAD --role executor --context-path src/service.rs --profile-client my-client
```

| Route | Root and audience | Access |
|---|---|---|
| MCP | Server root and `MMCG_PROFILE_CLIENT`. Arguments cannot override them | An existing [profile grant](#profile-access) |
| CLI or Lens | Selected repository and explicit `--profile-client` | An existing profile grant |

| Layer | Content |
|---|---|
| `person` | Applicable reviewed preferences and habits and Git observations, filtered by `paths`, role and workflow |
| `project` | Root `CONTEXT.md` sections and candidate claims selected by `query` |
| `documentation` | Relevant Markdown sections, or `not_requested` without a query |
| `code` | Repository diff against `since`. Personal path filters do not narrow it |
| `work` | At most 20 task records, with `current_checkout: not_verified` |

Every layer keeps its status, digest and omission reasons. A stale, missing or
omitted layer is a gap, never an empty verified result, and
`consistency: independent_layer_snapshots` says layers were read at different
moments. Repository content is untrusted and grants nothing. Personal advice
ranks below the task, code, tooling contracts and product requirements.

| Field or condition | Contract |
|---|---|
| `context_revision` | SHA-256 of the compact packet JSON with only that member removed. Hash the received bytes, because reserializing floats can change them |
| `layer.revision` | Hash of the layer data before budget cuts |
| `budget_tokens` | 1,024-16,000, default 8,000. The estimate is packet bytes divided by 4 |
| Over budget | Whole layers are dropped in this order: documentation, person, project, code, work. Metadata alone over budget fails |
| Index exists but does not open | `context_index_unavailable` |
| Index kept changing while copied | `context_index_snapshot_unavailable`, after one retry |

### Who gets which slice

| Consumer | Slice |
|---|---|
| Prompt hook | A planner slice on every admitted prompt while delivery is on, see [native capture](#native-capture) |
| Native executor (`run-task --exec`) | The executor slice for the approved scope, for the configured Claude profile client or `--profile-client`, saved as `state.profile_client`. Git observations are refreshed first, and the packet revision is pinned before delivery |
| Manual auditor workflow | Can request an auditor slice |
| Native semantic reviewer | Nothing, see the [reviewer contract](task-runtime.md#native-semantic-reviewer) |

Completion rechecks read access, so a saved audience cannot undo a revocation.
Reviewed completion refreshes Git observations again and records
`state.profile_refresh` with the Git snapshot and published profile revision.
A refresh failure is recorded apart from the task verdict. Personal context
never decides correctness or grants tools.

### Private Lens view

| Surface | Contract |
|---|---|
| Profiles tab | Selected advice, source freshness, code evidence and task history |
| Review queue | At most 8 unattested hook draft IDs for the repository, with status and counts but no wording or quotes |
| Access | An existing exact-root grant. Denial returns no private rows |
| `/api/context` | Separate from `/api/lens`. Loopback, same-origin, read-only, `no-store`, cleared when you leave the tab |
| Exports | `LensSnapshot` and portable HTML leave personal context out |
| Exposure | `mmcg_context` counts as possible profile exposure, so later echoes cannot be independent habit evidence |

## Native capture

The first client setup through `init` turns on capture, profile access and task
mining on macOS and Linux. Windows has no native hooks or managed workers.
Direct `hooks setup` previews changes until you add `--write`:

```bash
mastermind miner hooks setup --client codex --project-root . --write
mastermind miner hooks status --client codex --project-root .
```

| Setup operation | Contract |
|---|---|
| Capture grant | Exact native client and canonical project root |
| Existing hooks | Kept in `.codex/hooks.json` or `.claude/settings.local.json` |
| Direct setup | Reuses the existing read grant and keeps delivery and refiner choices. It creates no grant and starts no worker |
| `--disable-profile` | Stops delivery and local extraction. Capture and MCP read access stay. `--profile-client CLIENT` or `init --profile-access on` turns delivery back on |
| `--remove --write` | Revokes capture before it edits configuration. Local evidence stays |
| Client trust | Done in the native client. Restart it for a fresh `SessionStart` |

`hooks status` and `doctor` report each pipeline stage separately and start
nothing. A `SessionStart` in the current generation shows the hooks ran.
`pipeline.next_actions` says how to recover missing evidence.

| Capture event | Admission and coverage |
|---|---|
| `SessionEnd` | Closes prompt admission until a fresh `SessionStart` |
| Late `Stop` with turn ID | Closes only the matching turn |
| Unknown or reused turn ID, or overlapping prompts without IDs | A coverage gap, with no guessed attribution |
| Tool body | Not kept. Identity, digest and pairing are recorded |
| Ambiguous identity, fork or delivery failure | Fences the session or capture grant |
| Episode over 320 KiB | Later event text is dropped with an `episode_size_limit` gap |
| More than 128 open tool calls in a turn | Later calls get an `episode_tool_limit` gap |
| `MMCG_INPUT_ORIGIN=controller` | Skipped before any journal write |
| `MMCG_INPUT_ORIGIN=automation` | Kept, labelled `automation_or_agent`, and cannot supply personal quotes |

After an admitted prompt and its exposure receipt commit, the hook can add a
planner slice through `additionalContext`. It selects advice by literal
repository paths in the prompt, and without paths it leaves out language-scoped
advice. An ordinary request never inherits the previous task's scope. Missing
`SessionStart`, incomplete capture, changed grants or unavailable storage
withhold delivery and keep the event.

| Recorded influence | Effect on promotion |
|---|---|
| `no_recorded_prior_exposure` | Eligible prose still needs authorship attestation and review |
| `dependent_observation` | Inspectable, never unexposed habit support |
| `unknown_influence` | Inspectable, needs new eligible evidence |
| Generated or refined text | Context only |

## Mining in the current task

With `--mining task`, the current native agent does the mining and no separate
processor runs. `UserPromptSubmit` offers a ticket and asks the agent to report
through [`mmcg_mining_submit`](mcp.md#mcp-tools) before it finishes. The extra
work uses the client's normal allowance.

| Task submission | Contract |
|---|---|
| Ticket | Bound to the prompt event, capture generation, client and repository |
| Candidate | `when`, `behavior`, `exception`, `evidence_kind` and the exact `quote` from the user's prompt |
| Bounds | 0-2 candidates, at most 8 KiB. Text limits match the [processor schema](#hook-processor-contract) |
| Source, client and model | Set by the host and native capture, never by the caller |
| Admission | Rejects stale or foreign tickets, changed sources, ineligible quotes and incomplete capture |
| Retry | An identical submission is idempotent, a conflicting one is rejected |
| Missing report | One `Stop` continuation while capture is complete. A second miss is recorded as skipped analysis |
| Empty report (`candidates=[]`) | A completed analysis with result `no_signal` and no draft |
| Complete `Stop` | Rechecks source, grants and mode, then seals unreviewed drafts |
| Delayed Claude model | The finalizer waits up to `MMCG_TASK_FINALIZER_WAIT_MS` for the transcript, then the binding stays pending |
| Recovery | Pending bindings and failed refreshes are retried at later native events, executor context, reviewed completion or `hooks mine-local`, up to four episodes per pass |
| Inspection | `hooks show` exposes `task_mining`, finalization status and draft IDs. Read drafts with `hooks draft` |
| `legacy_completed` | A ticket from an older version with no saved candidate count, not `no_signal` |

A sealed draft is unreviewed and activates nothing. Load an updated MCP catalog
with a new client session. Continuation follows the
[Codex](https://learn.chatgpt.com/docs/hooks#stop) and
[Claude](https://code.claude.com/docs/en/hooks#stop-decision-control) Stop protocols.

## Local extraction and retention

With delivery on and a read grant, closed episodes also go through the
`persona-explicit-v2` detector, the one transcript collection uses. It calls no
model.

| Local extraction | Contract |
|---|---|
| Text | Exact eligible user prose, with no inferred role, motive or result |
| Bounds | 128 prompt lines, 8 drafts per episode, a complete statement of at most 200 characters |
| Checkpoint | Episode revision and detector fingerprint, empty results included |
| Replay | `hooks mine-local`, 1-16 episodes per page. Pass `next_after` as `--after` to continue a pass |
| Delivery off or access revoked | Local processing pauses |

The journal archives inactive episodes once it uses 48 MiB, or at 2,000 active
episodes. Archives keep gaps, review bindings and the evidence revision, and a
missing or changed archive withholds the claims that depend on it.
`pipeline.retention` shows active and archived counts and `database_bytes`
against `database_byte_limit`. Each archive run also frees rows that ended
sessions no longer need: up to 4,096 event receipts and the intake and mining
rows of 64 archived episodes. Running sessions keep their receipts.

```bash
mastermind miner hooks archive --project-root . --limit 32
```

A hook delivery archives up to 32 episodes and stops 250 ms after the first, to
stay inside its 3 second timeout. `hooks archive` has no time limit. The archive
grows until `hooks forget`, which removes archived copies. Data already sent to
an external processor and backups are outside that erasure.

## Hook processor contract

Native capture and `hooks mine-local` call no processor. `hooks analyze` and a
foreground `hooks mine` run the processor you select. A custom processor runs as
direct argv with no shell and your permissions. The processor reads a JSON request on stdin with `schema`, `instructions`,
`response_example` and `episode`. Stdout must hold one strict JSON object:

```json
{
  "schema": 1,
  "episode_id": "exact-request-episode-id",
  "episode_revision": "exact-request-revision",
  "drafts": []
}
```

| Draft field | Contract |
|---|---|
| `when`, `behavior` | 8-200 characters each |
| `exception` | 1-200 characters. Keep the limits or say none was observed |
| `rationale`, `outcome` | `null` in schema 1 |
| `role` | `null`, `planner`, `executor` or `auditor` |
| `workflow` | `null` or lowercase ASCII letters, digits, `_` and `-`, at most 64 characters |
| `evidence_kind` | `technical_approach`, `workflow_pattern`, `communication_preference`, `tool_preference` or `review_preference` |
| `supports`, `contradictions` | Exact `{event_id, quote}` citations, quotes of 8-300 characters |

Positive support must cite user prose with `kind=UserPromptSubmit` and
`origin=user_channel_unverified`, and a contradiction can also cite
`next_turn_context`. Code, quotations, pasted documents, assistant or tool text
and recognized secrets are ineligible. A coverage gap rejects the analysis. An
exact match shows the words are in the source, not that a human wrote them.

| Client | How `--provider native` runs |
|---|---|
| Claude Code | `--safe-mode --tools "" --strict-mcp-config` with an empty MCP list and no persistence. The model comes from `PostModelSwitch` or a matching transcript tail |
| Codex | Ephemeral read-only run with user config, project instructions, hooks, plugins, memory and shell tools off. A result with a tool execution is rejected |

Explicit `claude` or `codex` must match the captured client. Native CLI login is
reused and no credentials are copied. Episode text may leave the machine. Missing
model metadata skips semantic mining and keeps local collection.

| Capture or analysis resource | Limit |
|---|---|
| Native hook command | 3 seconds. Journal lock waits end 2.5 s after the hook starts |
| Native JSON / retained text per event | 4 MiB / 16 KiB. Lost prose marks the episode incomplete |
| Tool capture | Metadata only, the first 32 receipts in the episode context |
| Journal / active episodes | 64 MiB / 2,000 |
| Context events / stored bytes per episode | 128 / 512 KiB |
| Processor request / stdout / stderr | 512 KiB / 64 KiB / 16 KiB |
| Processor timeout | 1-120 seconds, default 60 |
| Drafts / citations | 8 drafts, 8 supports and 8 contradictions each |
| Worker batch | 32 inspected episodes, 1-16 attempts, default 4 |

| Worker or capture event | Result |
|---|---|
| Successful analysis, also with no drafts | Checkpoints the episode revision for that processor |
| `--follow` | Foreground only. Polls about every 2 seconds. Cannot be combined with `--after` |
| Interruption | Releases the lease for retry. A request already sent to a provider stays sent |
| Journal write failure | Records `capture_delivery_failed`, or `journal_full` at the size limit, and fences that client and project until recovery |
| 1,024 fence markers for one client and project | Refuses that grant only |
| Recovery | New generation. Lost events are not rebuilt |

## Managed workers

Separate analysis is optional. Turn it on with `init --mining on --provider
native`, or start a worker directly:

```bash
mastermind miner hooks worker start --client codex --project-root . \
  --provider native --max-calls 64 --max-runtime 3600
mastermind miner hooks worker status --client codex --project-root .
mastermind miner hooks worker stop --client codex --project-root .
```

`mastermind miner start`, `stop` and `status` use the saved project and client
choices. Add `--client claude` or `--client codex` to pick one.

| Managed worker | Contract |
|---|---|
| Ownership | One owner per client and project root |
| Start | `started: true` means the child published its run state. Check `run.reason`, because the run may already have failed |
| Running owner | Reused without resetting counters. Stop it before you change settings |
| Call budget | Default 64, 1-10,000, reserved before each call. Client retries and token use are not measured |
| Runtime budget | Default 3,600 s, 1-86,400 s, idle time included |
| Automatic start | Saved `mining: on` and `provider: native` start the worker at `SessionStart`. A new session or episode can renew a finished automatic run |
| Changed settings, generation or custom processor | Needs an explicit restart |
| Automatic transient failure | Up to three attempts in a row. Other failures stop the worker |
| Stop or revocation | Cancels the process group and withholds unfinished drafts |
| Crash or SIGKILL | Shows as `interrupted`. Completed checkpoints stay |
| Profile refresh failure | `run.reason: profile_refresh_failed` while the worker continues |

## Prompt refinement and intake

Refinement is a separate opt-in through `init --refiner on --provider native`
or `hooks setup ... --refiner-provider native --refiner-timeout 8 --write`. A
custom processor takes `--refiner-processor PATH` with repeated
`--refiner-arg=VALUE`. `--disable-refiner --write` turns it off and keeps
capture.

| Intake result | Contract |
|---|---|
| Original | Captured text and its SHA-256. The agent still gets it |
| `passthrough` | Byte-identical original |
| `refined` | Separate proposed text, at most 16 KiB |
| `ask` | Up to 3 questions, no planner or executor handoff |
| Workflow activation | Must cite exact eligible prose. The handoff to `mastermind-task-planning` is advice only |
| Native delivery | `additionalContext`. No execution, permission or approval |
| Failure or invalid output | `degraded`. The original stays |
| Crash with a `pending` receipt | Reads as `degraded` with `processor_interrupted` once the timeout plus 3 s has passed. No automatic retry |

| Refiner limit | Value |
|---|---|
| Attempts | One per captured prompt, failures included |
| Processor timeout | 1-20 s, default 8 s. The prompt hook gets the timeout plus 3 s |
| Original / response | 16 KiB / 64 KiB |
| Combined native context | 8 KiB. An oversized refinement goes as a receipt reference |
| Another session's delivery in flight | Waits up to 1.5 s |

Inspect `intake` in `hooks show`, read it with `hooks intake INTAKE_ID`, and bind
a planned spec with `hooks bind-task INTAKE_ID --spec PATH` before preflight.
Replace a binding with `--expected-binding REVISION` from `state.intake.json`,
which works only if the binding still has that revision. An identical repeat
returns the same receipt. A `prepared` marker after a crash blocks execution
until you retry the same intake. The binding records where the task came from.
It does not approve scope or show that the meaning survived refinement.

## Transcript admission

| Client | Source and attribution requirements |
|---|---|
| Claude Code | A JSONL file under `~/.claude/projects/<project>/` with explicit human origin, a valid `sessionId`, an absolute `cwd` on every record, and a cited human turn whose `cwd` is inside the project, its Claude folder or the temp directory |
| Codex | A path under `CODEX_HOME` (default `~/.codex`) in `sessions/YYYY/MM/DD/rollout-*.jsonl` or `archived_sessions/rollout-*.jsonl`, with one `session_meta` with `source=cli\|vscode`, `thread_source=user`, a matching `turn_context` and the project `cwd` |

| Transcript condition | Result |
|---|---|
| Unknown attribution schema, fork, conflicting context or a Codex project change | Unsupported |
| Compaction, attachment, pasted or service content, tool output, subagent message | Cannot supply personal quotes |
| Quote | Bound to the exact line, segment and digest |
| Moved source | Collect or cite it again |
| Changed evidence | Needs review again |

## Collect and sync an inbox

```bash
mastermind miner collect --project-root . --transcript /path/to/session.jsonl --dry-run
mastermind miner collect --project-root . --transcript /path/to/session.jsonl
mastermind miner sources list --project-root . --limit 50
mastermind miner sources exclude SOURCE_ID --project-root .
mastermind miner sync --project-root . --limit 16
```

`collect` and `sync` use the
[explicit-statement detector](persona-evidence.md#explicit-statement-detector)
with no model call. They write private observations, not approved claims.

| Collection operation | Contract |
|---|---|
| Repeated `--transcript` | Selects several files |
| Changed / unchanged source | Rescanned / left as is |
| `sync` | Rereads registered sources only, no discovery |
| Excluded source | Stays visible with `sync_enabled=false`. It does not withdraw reviewed evidence |
| `next_cursor` and `--after` | Continue the same pass |
| Unavailable, redirected or over-budget source | Aborts the page without moving checkpoints |

## Inspect and propose

```bash
mastermind miner candidates list --status pending --limit 50
mastermind miner candidates show CANDIDATE_ID
mastermind miner candidates search "review" --project-root . --status all --limit 20
mastermind miner candidates dismiss CANDIDATE_ID --revision REVISION

mastermind miner candidates propose-preference CANDIDATE_ID --revision REVISION \
  --statement "Keep review replies concise" --category communication

mastermind miner candidates propose-habit CANDIDATE_ID --revision REVISION \
  --episode TASK_ID --when "Reviewing state changes" \
  --behavior "Check state ownership and rollback" --outcome "Cited observed result" \
  --role auditor --workflow strict
```

Use the full ID and revision from `show`. Search is a literal, case-insensitive
match over kept quotes. A proposal stays a candidate, an exact retry returns the
same IDs, and it cannot change a reviewed, terminal or dismissed record.

| Claim input | Allowed values |
|---|---|
| Preference scope | Default `project:<stable-id>`. Also `global`, `language:<name>`, `repo:<name>`, `path:<prefix>`, `role:planner\|executor\|auditor`, `workflow:<name>` |
| Preference category | `code`, `process`, `communication`, `tooling`, `review` |
| Habit scope | The verified project. `--global` asks for a cross-project claim |
| `--habit ID` | An existing candidate habit with the same definition, scope, role and workflow. Without it, that habit is found or created |

## Review preferences and habits

| Operation | Preference | Habit |
|---|---|---|
| Inspect | `mastermind miner feedback show KEY` | `mastermind miner habit show ID` |
| Approve exact revision | `mastermind miner feedback accept KEY --revision REVISION` | `mastermind miner habit observe ID --revision REVISION` |
| Reject | `mastermind miner feedback reject KEY` | `mastermind miner habit reject ID` |
| Withdraw one citation | `mastermind miner feedback dismiss-source KEY CANDIDATE_ID --revision REVISION` | `mastermind miner habit dismiss ID EVIDENCE_ID` |
| Recheck and publish | `mastermind miner feedback refresh` | `mastermind miner habit refresh` |

| Approval condition | Preference | Habit |
|---|---|---|
| Verified support | One source can be enough | Two distinct sessions and two distinct task episodes |
| Global claim | Same rules | Also two distinct configured Git origin URLs |

Unresolved `contradicts` or `limits` citations block a habit observation.

To curate a habit without an inbox observation:

```bash
mastermind miner habit propose --project-root . --transcript /path/to/session.jsonl \
  --quote "Exact human words" --episode TASK_ID --when "Situation" \
  --behavior "Action" --outcome "Cited observed result"
mastermind miner habit cite HABIT_ID --project-root . --transcript /path/to/other.jsonl \
  --quote "Exact human words" --episode OTHER_TASK_ID --relation supports
```

Reuse one episode ID for every session of the same task. `cite` also takes
`--relation contradicts` and `--relation limits`. Approval pins the definition,
scope, role, workflow and the whole evidence set, and any change or dismissal
drops the pin. Publication and retrieval recheck sources and withhold changed or
unverifiable claims.

To replace a claim, reject it and propose a new one. A rejected claim stays
rejected.

`feedback scan`, `feedback add` and `feedback import-memory` still work for local
inspection, with transcript input up to 256 MiB. A row without an exact source
locator is `legacy-unverifiable` and cannot be accepted. A `memory` row needs
manual curation into a supported category.

## Work and storage limits

| Operation or collection | Limit |
|---|---|
| Source list page | 1-100 rows |
| Sync page or collection | 16 files, 32 MiB per file, 64 MiB total, 1 million JSONL lines |
| Collection candidates | 8,192 human segments per file, 512 observations from changed sources |
| Stored inbox | 2,000 sources, 10,000 candidates, 20,000 revisions |
| Candidate search | 10,000 rows inspected per page, 8 proposal links per result |
| Verified profile read | The 16-file, 64 MiB, 1-million-line budget, 64 pinned habits and 64 accepted preferences |
| Active citations | 32 per claim |
| Parsed cited records | 64 KiB per record, 8 MiB total |
| Codex provenance | 4,096 contexts, 64 KiB per record |
| Shown review events | 20, with explicit truncation |

Transcript input must be valid UTF-8. Incomplete verification is reported as
incomplete, never as an empty verified profile.
