# Persona storage, mining and review

Use this reference for `miner` commands, source admission, review transitions
and profile limits. Start with [client capture](../guides/persona-hooks.md) for
hook setup or [context composition](persona-context.md) for delivery to an agent.

## Storage and authority

| Data | Location | Use |
|---|---|---|
| Hook events, coverage, drafts and exposure records | `~/.mastermind/persona-events.db` | Local capture journal |
| Archived episode payloads | `~/.mastermind/persona-archive/*.json` | Private sources retained outside the working journal |
| Managed worker configuration and runs | `~/.mastermind/persona-workers/<id>/` | Optional separate processor state |
| Personal evidence, claims, reviews and grants | `~/.mastermind/style.db` | User-global profile across repositories |
| Generated profile | `~/.mastermind/style.md` | Local inspection snapshot, schema 5 |
| Unreviewed deep interpretation | `style.deep-candidate-*.md` in the profile directory | Local review input |
| Project context | Repository `CONTEXT.md` and its derived index | Separate project evidence, see `mmcg_project_profile` |

| Profile artifact or event | Contract |
|---|---|
| Manual `style.md` edit | Preserved as fenced unreviewed notes. Cannot accept a preference or observe a habit |
| Agent retrieval | Use `mmcg_profile`. No Markdown fallback for denied, unavailable or empty results |
| `store_revision` / Markdown `store-revision` | Identify canonical SQL inputs |
| Live `profile_revision` | Identifies selected claims and Git aggregate after source checks |
| Markdown `snapshot-revision` | Identifies the published aggregate, not a later MCP selection |
| Claim ID and review revision | Identify the advice supplied to a consumer |
| Publication | Serialized and atomic. Markdown must be a regular file ≤1 MiB, concurrent manual edits rebase with bounded retries |
| SQL store | At most 64 MiB |
| SQL committed but Markdown publication failed | Retry the same operation or refresh the view. Do not create a duplicate claim |

## Git observations

```bash
mmcg miner profile .
mmcg miner profile . --author "Ada Lovelace"
mmcg miner profile . --deep
```

| Input or measurement | Contract |
|---|---|
| `--author` | Literal substring of Git author name/email |
| History | Up to 2,000 recent authored non-merge commits |
| Diff sample | Up to 400 eligible source commits selected in monthly rounds |
| Bulk commit over 2,000 added source lines | Commit-message evidence only |
| Cache reuse | Matching author, detector, grammar, tooling and first-party contract required |
| Tooling conventions | Separate from personal observations |
| Workflow measures | Commit-level delivery patterns |
| Range | Language, area and library exposure |
| Libraries | Parsed imports in complete committed source intersecting added rows. Import coverage disclosed separately; comments, strings, standard modules and recognized first-party modules excluded |
| Interpretation | Neither exposure nor delivery patterns establish proficiency, motives or stable human habits |
| Subdirectories and linked worktrees | Share one repository contribution |
| Independent clones | Retain provenance. Support counts each SHA once and withholds conflicting measurements, historical occurrences may repeat |
| `--deep` | Explicit `claude -p`, bounded I/O, 180 seconds. Output remains a local candidate, excluded from the active profile |
| `--force` | Replaces the whole profile including preserved prose |

The [mining contract](persona-mining-contract.md) defines commit support, Wilson
tiers, duplicate reconciliation and replay checks.

## Profile access

```bash
mmcg miner access grant . --client claude
mmcg miner access list
mmcg miner access revoke . --client claude
```

| Access condition | Result |
|---|---|
| MCP audience | `MMCG_PROFILE_CLIENT=claude` plus matching canonical project root and client grant |
| Granted data | Global profile, including Git observations across mined repositories. No source-level grants |
| Selection | Project, paths/languages, role and workflow filters run before selected claim source reads |
| Private fields | Quotes and local source paths withheld |
| Missing access or absent store | `access_denied`, no store creation |

See [MCP arguments and result fields](mmcg.md#mcp-tools).

## Native capture

First client setup through `init` enables capture, profile access and task mining
on macOS/Linux. Direct `hooks setup` previews changes unless `--write` is given:

```bash
mmcg miner hooks setup --client codex --project-root .
mmcg miner hooks setup --client codex --project-root . --write
mmcg miner hooks status --client codex --project-root .
```

| Setup operation | Contract |
|---|---|
| Capture grant | Exact native client and canonical project root |
| Existing hooks | Preserved in `.codex/hooks.json` or `.claude/settings.local.json` |
| Direct setup | Reuses the configured profile reader or the native client's existing read grant. Creates no new profile read grant |
| Ordinary setup | Preserves delivery and refiner choices. Starts no managed worker |
| `--disable-profile` | Disables delivery and automatic local extraction, retaining capture and MCP read access |
| Reenable delivery | Explicit `--profile-client CLIENT` or `init --profile-access on` |
| `--remove --write` | Revokes capture before editing configuration. Retains local evidence |
| Client trust/loading | Must be completed in the native client. Restart for a fresh `SessionStart` |
| Windows | Native hooks and managed workers unsupported |

Status and doctor report registration, capture, session, local analysis, task
mining, profile, refiner and managed-worker observations separately. Registration
can be current, missing/stale, unavailable or unsupported. Local disable flags
are separate. `SessionStart` establishes an observation in the current generation,
not client trust. Offers and processed empty analyses count as observations,
not model use or accuracy. Reads start no process, approve no claim and create
no drafts. `pipeline.next_actions` describes recovery for missing evidence.

| Capture event | Admission and coverage |
|---|---|
| `SessionEnd` | Closes prompt admission until a fresh `SessionStart`. An empty closing receipt after `Stop` preserves the episode's evidence revision |
| Late `Stop` with turn ID | Closes only the matching turn, without replacing newer response context |
| Unknown/reused turn ID or overlapping prompts without both IDs | Coverage gap rather than inferred attribution |
| Empty current response | Clears earlier assistant context |
| Repeated `SessionEnd` without event ID | Records replay ambiguity |
| Lost retained text | Marks every episode using that event incomplete, including later context uses |
| Tool body | Not retained. Native identity, digest and pairing remain recorded; a missing result still blocks evidence |
| Ambiguous identity, unattributed loss, fork or delivery failure | Fences the session or capture grant |
| Earlier capture versions | Remain historical with their original gaps. Upgrading cannot repair them |
| `MMCG_INPUT_ORIGIN=controller` or miner recursion guard | Skips generated input before journal writes |
| `MMCG_INPUT_ORIGIN=automation` | Retains client/model observations, but labels input `automation_or_agent`, ineligible for personal quotes |

An admitted prompt can receive a planner profile through `additionalContext`
after capture and its exposure receipt commit. Selection uses literal repository
paths from the original prompt. Without paths, language-scoped advice is
withheld. A refiner continuation may use its explicitly bound spec's paths and
workflow. An ordinary request cannot inherit the preceding task's scope.
Missing SessionStart, incomplete capture, changed grants or unavailable storage
withhold delivery without discarding the captured event.

| Recorded influence | Candidate extraction and promotion |
|---|---|
| `no_recorded_prior_exposure` | Eligible original prose still requires authorship attestation and review |
| `dependent_observation` | Inspectable, cannot count as unexposed habit support |
| `unknown_influence` | Inspectable, requires new eligible evidence for promotion |
| Generated/refined text | Context only, never positive human habit evidence |

Influence is recorded before the current advisory. Profile/refiner offers and
recognized MCP/profile-file reads affect later events. Every support and
contradiction contributes to a draft's classification. Resume and recovery do
not clear prior exposure. Detection covers supported delivery paths only and
does not establish statistical independence. Session and episode summaries retain
up to 32 exposures and an omission count. Committed context offers take priority
over possible tool reads. Citation influence remains intact when summaries rotate.

## Mining in the current task

`--mining task` uses the current native agent instead of launching a processor.
`UserPromptSubmit` offers a ticket and a short optional instruction. The agent
may call [`mmcg_mining_submit`](mmcg.md#mcp-tools) before its final answer, or skip
it when no concrete preference exists. There is no forced continuation or
provider fallback. Extra context, reasoning and tool calls use the client's
normal allowance.

| Task submission | Contract |
|---|---|
| Ticket | Bound to original prompt event, capture generation, client and repository |
| Candidate | `when`, `behavior`, `exception`, `evidence_kind`, exact original `quote` |
| Bounds | 0–2 candidates, submission at most 8 KiB. Text and citation limits match the processor schema below |
| Source selection | Host-derived event. Caller cannot select another root, client, session or author |
| Client and model metadata | From native capture, never caller-supplied. Proposer identity remains unverified |
| Admission | Rejects stale/foreign tickets, changed sources, ineligible quotations and incomplete capture |
| Retry | Identical active submission is idempotent. Conflicting submission rejected |
| Before `Stop` | Pending submission, no analysis or persisted semantic draft |
| Complete `Stop` | Rechecks source, grants and mode, then seals unreviewed drafts |
| Late paired result after `Stop` | Retries local sealing when capture becomes complete. Later source changes cannot automatically rebind a completed draft |
| Delayed Claude model | Local finalizer can wait up to 4 seconds for transcript binding, outside the native hook deadline |
| Finalizer | Local admission, sealing and authored Git refresh. No model call |
| Inspection | `hooks show` exposes `task_mining`, analyses and draft IDs. Read full content with `hooks draft` |
| Mode change | Explicit `init --mining task` stops the managed miner. Repeated init preserves saved choices |

An offered instruction does not prove agent adherence, interpretation quality
or usefulness. Sealing neither attests authorship nor activates a rule. A new
client session is required to load an updated MCP catalog.

## Local extraction and retention

With profile delivery enabled and an existing read grant, complete closed
episodes also run the `persona-explicit-v2` detector. It shares the detector
with transcript collection and invokes no model. `Stop`, later context and late
tool results queue changed episode revisions. Empty closing receipts preserve
the completed revision, including when reading older retained episodes.

| Local extraction | Contract |
|---|---|
| Text | Exact eligible original user prose, no inferred role, motive or result |
| Bounds | 128 prompt lines, 8 drafts per episode, complete normalized statement at most 200 characters |
| Multiline condition/exception | Retained in the same source span. Oversized spans omitted whole |
| Checkpoint | Episode revision and processor fingerprint, including empty results |
| Replay | `hooks mine-local`, 1–16 episodes/page. Use `next_after` for the same pass, start without it for revised sources |
| Retry | Native events, executor context and reviewed completion process up to four durable queue entries under current grants, with bounded failure backoff |
| Unavailable store | Work remains queued until the next trigger. No idle background service is started |
| Delivery disabled or profile access revoked | Automatic local processing paused |
| Changed bindings | Require authorship review again |
| Quality | [Synthetic regression and source-span evaluation](persona-quality.md), real-history accuracy and task benefit unmeasured |

Capture archives inactive payloads under active-episode or database pressure.
Current episodes remain in the working set. Archives preserve gaps, deduplication
identity and review bindings. Source reads check the archive digest and keep
the original evidence revision. Missing or changed archives withhold dependent
claims. `pipeline.retention` separates active and archived counts.

```bash
mmcg miner hooks archive --project-root . --limit 32
```

The 2,000-episode bound applies to active payloads. Metadata and other journal
records still share the 64 MiB database cap. Archive storage grows until an
explicit `hooks forget`, which removes archived copies and adjacent copied
context. Transferred profile audit quotes, backups and external processor data
are outside that erasure. Recovery starts a new generation without reconstructing
lost events or clearing historical gaps.

## Hook processor contract

| Entry point | Execution contract |
|---|---|
| [Native capture](../guides/persona-hooks.md) | Collects events and, with profile delivery enabled, bounded explicit-statement candidates. No provider or automatic acceptance |
| `hooks mine-local` | Explicit bounded replay of complete episodes using the same local detector |
| `hooks analyze` or foreground `hooks mine` | Explicitly selects a processor |
| Custom processor | Direct argv, no inserted shell, caller permissions |

The stdin JSON request contains `schema`, `instructions`, `response_example`
and `episode`. Stdout must contain one strict JSON object:

```json
{
  "schema": 1,
  "episode_id": "exact-request-episode-id",
  "episode_revision": "exact-request-revision",
  "drafts": []
}
```

Empty drafts are valid. A draft follows the supplied response example:

| Field | Contract |
|---|---|
| `when`, `behavior` | 8–200 characters each |
| `exception` | 1–200 characters, preserve limits or state that none was observed |
| `rationale`, `outcome` | `null` in schema 1 |
| `role` | `null`, `planner`, `executor` or `auditor` |
| `workflow` | `null` or a lowercase ASCII letters/digits/underscore/hyphen identifier, at most 64 characters |
| `evidence_kind` | `technical_approach`, `workflow_pattern`, `communication_preference`, `tool_preference` or `review_preference` |
| `supports`, `contradictions` | Exact `{event_id, quote}` citations, quotes 8–300 characters |

| Evidence or processor condition | Contract |
|---|---|
| Positive support | Exact user-prose span with `kind=UserPromptSubmit` and `origin=user_channel_unverified` |
| Contradiction | May also cite `next_turn_context` |
| Ineligible span | Code, quotation, pasted document, instruction wrapper, assistant/tool text or recognized secret |
| Exact source match | Does not establish human authorship or semantic accuracy |
| Coverage gap | Reject analysis |
| Known profile/refiner influence | Retain exposure classification. Cannot become independent habit support |
| Inference boundary | No identity, psychology, sensitive traits, permissions or global habit from one task |
| `--provider native` | Uses the captured native client and model; may send episode text externally |
| Built-in isolation | Claude safe mode with tools disabled; Codex ephemeral read-only inference with context discovery disabled and tool executions rejected |
| Credentials | Native CLI login, including subscriptions. No credentials are copied |
| Unsupported flag | Fail without falling back to a normal client session |

`native` resolves to the captured client. Explicit `claude` or `codex` must
match it. The captured model is passed to the CLI. Codex supplies model events,
and Claude also records `PostModelSwitch`. When Claude omits the model, a bounded
transcript tail must match the session, project, latest response and prompt ID
when available. Pending transcript flush is retried without a model call.
Missing metadata skips separate semantic mining, retaining local collection.
Closed episodes keep their original model.

Claude uses `--safe-mode --tools "" --strict-mcp-config` with an empty MCP list
and no persistence. Codex disables user config, project instructions, hooks,
plugins, apps, memory and shell tools in an ephemeral read-only invocation.
Its account directory is retained with a private HOME. Results containing tool
execution are rejected. Catalog warnings are diagnostics. Receipts retain
client/model source, executable digest and adapter version, including empty
analyses. Transcript binding also records the response digest and any available
prompt identity and digest.

| Capture or analysis resource | Bound |
|---|---|
| Native hook command | 3 seconds |
| Native JSON / retained user and assistant text per event | 4 MiB / 16 KiB. Lost prose marks each episode that uses it incomplete; an unattributed loss or envelope over 4 MiB fences the session/capture until recovery |
| Tool capture | Metadata only, first 32 receipts in the episode context. `ToolTrace` counts and hashes every observed tool event; individual native identities and digests remain in the journal |
| Journal / active episode payloads | 64 MiB / 2,000, with [archive retention](#local-extraction-and-retention) |
| Retained context events / stored bytes per episode | 128 / 512 KiB. Tool events beyond the first 32 update the trace without consuming context slots |
| Processor request / stdout / stderr | 512 KiB / 64 KiB / 16 KiB |
| Processor timeout | 1–120 seconds, default 60 |
| Drafts / supports / contradictions | 8 drafts, at most 8 supports and 8 contradictions each |
| Worker batch | At most 32 inspected episodes and 1–16 attempts, default 4 |

| Worker or capture event | Result |
|---|---|
| Successful analysis, including no drafts | Checkpoint episode revision for the selected processor |
| `next_after` | Continues one successful pass. Start without it for a new pass |
| `--follow` | Foreground only, journal polling about every 2 seconds, expired-lease checks every 30 seconds. Incompatible with `--after` |
| Failure | Stop worker |
| Interruption | Release unfinished lease for retry. Cannot retract a received provider request |
| Contention, crash, unsupported input, lost user/assistant prose or missing lifecycle/tool event | Mark incomplete delivery and withhold evidence |
| Recovery | New generation, no reconstruction of lost events |

Native client contracts: [Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Codex exec](https://learn.chatgpt.com/docs/cli/reference),
[Claude CLI](https://code.claude.com/docs/en/cli-reference).

## Managed workers

Separate analysis is optional. Enable it through `init --mining on --provider
native` or start a worker directly:

```bash
mmcg miner hooks worker start --client codex --project-root . \
  --provider native --max-calls 64 --max-runtime 3600
mmcg miner hooks worker status --client codex --project-root .
mmcg miner hooks worker stop --client codex --project-root .
```

The shorter `miner start`, `stop` and `status` commands use saved project/client
choices. Add `--client claude` or `codex` to select one configured client.

| Managed worker | Contract |
|---|---|
| Ownership | One owner per client and canonical root |
| Start | `started: true` means the bound child published run state. Inspect `run.reason`, it may already have failed |
| Running owner | Reused without resetting counters. Stop before changing worker settings |
| Restart | Without processor/budget flags, reuses saved settings for a new bounded run |
| Call budget | Reserved before invocation. Default 64, range 1–10,000. Client retries and token/billing usage unmeasured |
| Runtime budget | Includes idle time. Default 3,600 s, range 1–86,400 s |
| Timeout / batch | Default 60 s / 4, maximum 120 s / 16 |
| Repeated init | Retains run ID and spent counters, including stopped, failed, interrupted and exhausted runs |
| Checkpoint | Episode revision and processor fingerprint, including executable digest, native model and adapter |
| Fingerprint exclusions | Transitive script dependencies and server-side model weights |
| Native automatic start | Saved `mining: on` and `provider: native` start/observe the worker at `SessionStart` |
| Automatic renewal | A new session or newly completed episode may renew a terminal automatic run. Live runs retain budget. Resume, replay and explicit stop do not renew it |
| Native update | A new session or completed episode replaces an automatic worker whose executable or adapter changed under unchanged settings/grant |
| Changed settings, generation or custom processor | Requires explicit restart |
| Automatic transient failure | Up to three consecutive attempts within the current budget |
| Other worker failure | Stops, requires explicit start |
| Stop/revocation | Cancels the owned process group, withholds unfinished drafts. Cannot retract sent provider input |
| Crash/SIGKILL | Lost ownership reports `interrupted`. Completed checkpoints and outstanding lease expiry retained, external processor cleanup not guaranteed |
| Status | Read-only ownership, state, heartbeat and budget. Foreground workers not observed |
| Capture-mode status | Earlier terminal workers are history. An active unwanted worker or unavailable state produces a warning |

No separate queue or login service is installed. Completed revisions use the
capture journal checkpoints. Exact worker bounds are implemented in
[background.rs](../../mcp/servers/mmcg/src/miner/hooks/background.rs).

## Prompt refinement and intake

Refinement is separately enabled through `init --refiner on --provider native`
or direct hook configuration:

```bash
mmcg miner hooks setup --client codex --project-root . \
  --refiner-provider native --refiner-timeout 8 --write
```

For a custom processor, use `--refiner-processor PATH` and repeated
`--refiner-arg=VALUE`. Setup without these options preserves the selection.
`--disable-refiner --write` disables refinement without disabling capture.
Restart and trust the updated definitions.

| Intake result | Contract |
|---|---|
| Original | Captured text and SHA-256 of its UTF-8 bytes, still delivered to the agent |
| `passthrough` | Byte-identical original |
| `refined` | Separate proposed text, at most 16 KiB |
| `ask` | Up to 3 questions, no planner/executor handoff |
| Workflow activation | Model interprets intent, must cite exact eligible original prose. Advisory handoff to `mastermind-task-planning` |
| Continuation | Only this session's explicitly bound current spec. Changed specs, incomplete handoffs and completed tasks withheld |
| Native delivery | `additionalContext`, no execution, tool permission or approval |
| Incomplete/redacted/generated input | No processor invocation |
| Failure/invalid output | `degraded`, original retained, no workflow handoff |
| Admission changes | Concurrent revocation, reconfiguration, new prompt or end withholds the result |
| Crash with `pending` receipt | Unknown outcome, not automatically retried |
| Unstable native event identity | Ambiguous identical repeats quarantined |

| Refiner bound | Value |
|---|---|
| Attempts | At most one per captured prompt, including failure, outside managed miner budget |
| Processor timeout | 1–20 s, default 8 s |
| Native prompt hook timeout | Processor timeout + 3 s, other hooks 3 s |
| Original / response | 16 KiB / 64 KiB |
| Combined native context | 8 KiB. Oversized refinement uses a receipt reference, profile may be omitted |
| Client retries and token usage | Unmeasured |

Inspect `intake` in `hooks show`, then read `hooks intake INTAKE_ID`. Bind a
planned spec with `hooks bind-task INTAKE_ID --spec PATH` before preflight:

| Task binding | Rule |
|---|---|
| Source | Current offered activation/continuation, exact original and proposed digests |
| Replacement | `--expected-binding REVISION` from `state.intake.json` |
| Identical repeat | Same receipt, never creates or executes another task |
| Continuation | Same session, exact spec and current binding, no latest-task inference |
| Crash | `prepared` marker blocks execution. Retry the same intake or CAS-replace with a current admitted intake |
| Revocation, gaps, forgetting | Block new use, retain historical completed tasks |
| Local marker | IDs and digests only, raw input stays in the global journal |
| Execution evidence | Preflight, invocation, verification and review bind the same intake revision |

Binding establishes provenance, not scope approval or preserved meaning. The
executor receives original and proposed text as source data in its hashed
prompt. Clients may ignore/truncate context or continue after a hook timeout.
Neither a receipt nor an offered advisory proves that the workflow ran.

## Transcript admission

Supported source layouts:

| Client | Source and attribution requirements |
|---|---|
| Claude Code | A selected JSONL file under `~/.claude/projects/<project>/`, explicit human origin, a valid `sessionId`, and absolute `cwd` matching the supplied project root |
| Codex | An explicit path under `CODEX_HOME` (default `~/.codex`), in `sessions/YYYY/MM/DD/rollout-*.jsonl` or `archived_sessions/rollout-*.jsonl`, one initial `session_meta` with `source=cli\|vscode`, `thread_source=user`, matching `turn_context`, and canonical project `cwd` |

| Transcript condition | Admission or binding |
|---|---|
| Codex user text | Aligned `response_item` user message with `user.text` content |
| Unknown attribution schema, fork, conflicting context or project change | Unsupported |
| Compaction, attachment, pasted/service content, tool output or subagent message | Cannot supply personal quotes |
| Quote | Bound to exact source line, segment and digest. Codex also binds session and turn context |
| Copy or archive | Retains one source identity |
| Relocated known source | Recollect or recite to rebind the locator |
| Changed evidence | Requires review again |
| Separate sessions | Do not themselves establish separate tasks or human authorship |

## Collect and sync an inbox

```bash
mmcg miner collect --project-root . --transcript /path/to/session.jsonl --dry-run
mmcg miner collect --project-root . --transcript /path/to/session.jsonl
mmcg miner sources list --project-root . --limit 50
mmcg miner sources exclude SOURCE_ID --project-root .
mmcg miner sources include SOURCE_ID --project-root .
mmcg miner sync --project-root . --dry-run
mmcg miner sync --project-root . --limit 16
```

| Collection operation | Contract |
|---|---|
| Repeated `--transcript` | Explicitly selects several files |
| `collect` and `sync` | Use the [explicit detector](persona-quality.md), no model call. Write private observations, not approved claims |
| Changed / unchanged snapshot | Rescan / do not rewrite rows |
| Dismissed or removed observation | Retains review history |
| `sync` | Rereads registered selections only. No session discovery or relocation search |
| Excluded source | Remains visible with `sync_enabled=false`. Explicit collection does not reenable it |
| Source exclusion | Does not withdraw reviewed evidence or publish the profile |
| `next_cursor` and `--after` | Continue the same pass. Start a new pass without a cursor, pages are not frozen |
| Source listing | Stored metadata, `freshness=not_checked` |
| Sync validation | Checks identity and attribution before deduplication |
| Unavailable, redirected, invalid or over-budget source | Abort page without advancing checkpoints |
| Atomic SQL commit | Does not make sequential transcript reads simultaneous |
| Dry run | Does not pin the next write's inputs |

## Inspect and propose

```bash
mmcg miner candidates list --status pending --limit 50
mmcg miner candidates show CANDIDATE_ID
mmcg miner candidates search "review" --project-root . --status all --limit 20
mmcg miner candidates dismiss CANDIDATE_ID --revision REVISION

mmcg miner candidates propose-preference CANDIDATE_ID --revision REVISION \
  --statement "Keep review replies concise" --category communication

mmcg miner candidates propose-habit CANDIDATE_ID --revision REVISION \
  --episode TASK_ID --when "Reviewing state changes" \
  --behavior "Check state ownership and rollback" --outcome "Cited observed result" \
  --role auditor --workflow strict
```

| Inspection or proposal | Contract |
|---|---|
| ID and revision | Use full values from `show` |
| Search | Literal Unicode case-insensitive matching within retained inbox quotes only. No match is not proof of absence |
| Search reads | Filters precede source reads. Results retain citation identity, freshness and bounded proposal links |
| Proposal transaction | Verify saved citation and project under the writer lock, then commit definition, evidence, receipt and event together |
| Proposal state | Candidate only, never accepted/observed |
| Curator input | Scope, episode, condition, behavior, outcome and exceptions require source review. No inferred motives or episode independence |
| Exact retry | Same claim/evidence IDs |
| Changed definition or episode | New request |
| Relocated citation rebound | Invalidates prior acceptance/observation |
| New transfer | Cannot silently add support to a reviewed claim, revive a terminal claim or restore dismissed evidence |

| Claim input | Allowed values and matching |
|---|---|
| Preference default scope | `project:<stable-id>` |
| Explicit preference scope | `global`, `language:<name>`, `repo:<name>`, `path:<prefix>`, `project:<id>`, `role:planner\|executor\|auditor`, `workflow:<name>` |
| Preference category | `code`, `process`, `communication`, `tooling`, `review` |
| Habit default scope | Verified project. `--global` requests a cross-project claim |
| `--habit ID` | Existing candidate generation with matching definition, scope, role and workflow |
| No `--habit` | Generation 1 |
| Existing transfer receipt | Bound to its original claim, cannot redirect evidence to another generation |

## Review preferences and habits

| Operation | Preference | Habit |
|---|---|---|
| Inspect | `miner feedback show KEY` | `miner habit show ID` |
| Approve exact revision | `miner feedback accept KEY --revision REVISION` | `miner habit observe ID --revision REVISION` |
| Reject | `miner feedback reject KEY` | `miner habit reject ID` |
| Withdraw one citation | `miner feedback dismiss-source KEY CANDIDATE_ID --revision REVISION` | `miner habit dismiss ID EVIDENCE_ID` |
| Recheck and publish | `miner feedback refresh` | `miner habit refresh` |

Prefix these commands with `mmcg`.

| Approval condition | Preference | Habit |
|---|---|---|
| Verified support | One source may suffice | Two distinct sessions and task episodes |
| Global independence | Same source rules | Also two distinct configured Git origin URLs, not just checkout paths |

Unresolved `contradicts` or `limits` citations block habit observation.

Direct habit curation is available when no inbox observation was selected:

```bash
mmcg miner habit propose --project-root . --transcript /path/to/session.jsonl \
  --quote "Exact human words" --episode TASK_ID --when "Situation" \
  --behavior "Action" --outcome "Cited observed result"
mmcg miner habit cite HABIT_ID --project-root . --transcript /path/to/other.jsonl \
  --quote "Exact human words" --episode OTHER_TASK_ID --relation supports
```

Reuse one episode ID for every session of the same task. `cite` also accepts
`--relation contradicts` and `--relation limits`.

| Review event | Result |
|---|---|
| Approval | Binds definition, scope, role/workflow and complete retained evidence set |
| Change, rebind or dismissal | Revokes review pin |
| Exact duplicate evidence | Keeps review pin |
| Publication and retrieval | Recheck sources, withhold missing, changed, unverifiable or over-budget claims |
| Interactive-terminal gate | Local workflow check, not operator identity proof |

### Replace or reconsider a claim

```bash
mmcg miner feedback supersede OLD_KEY --with NEW_KEY \
  --old-revision OLD_REVISION --new-revision NEW_REVISION
mmcg miner habit supersede OLD_ID --with NEW_ID \
  --old-revision OLD_REVISION --new-revision NEW_REVISION
mmcg miner habit renew RETIRED_ID --revision REVISION
```

| Replacement condition | Contract |
|---|---|
| Revisions | Both reviewed revisions required |
| Preference match | Scope and category |
| Habit match | Scope, role and workflow |
| Successor | Must meet current source and approval requirements |
| Predecessor | May have unavailable sources or counterevidence. Retirement does not turn that evidence into support |
| Commit | Relations and review events are atomic |
| Graph | One successor per predecessor, multiple predecessors may converge |
| Invalid relation | Self-link, cycle or competing replacement rejected |
| Superseded/rejected claim | Terminal |
| Successor loses sources | Withhold successor without restoring predecessor |
| Exact committed retry | Republish current state, no new event or revival of a changed successor |

| `renew` | Contract |
|---|---|
| Input | Rejected or superseded habit at the exact revision |
| New generation | One candidate child, copies description and scope only |
| Evidence and approval | Empty, no review pin. Cite the child and review normally |
| Parent and successor | Unchanged |
| Exact retry | Return existing child, cannot restore its approval |
| Interpretation | Renewal does not prove that a behavior has returned |

### Legacy feedback

| Legacy operation or state | Compatibility contract |
|---|---|
| `feedback scan`, `feedback add`, `feedback import-memory` | Local inspection/import retained |
| Row without exact source locator | `legacy-unverifiable`, cannot be accepted or published |
| Supported category | Collect original human source and propose a strictly bound preference |
| `memory` category | Requires manual curation into a supported category, no automatic migration |
| Legacy evidence added to an accepted preference | Invalidates review pin |

## Work and storage limits

| Operation or collection | Bound |
|---|---|
| Source list page | 1–100 rows |
| Sync page / collection | 16 files, 32 MiB/file, 64 MiB total, 1 million JSONL lines |
| Collection candidates | 8,192 eligible human segments/file, 512 observations from changed sources |
| Stored inbox | 2,000 sources, 10,000 candidates, 20,000 revisions |
| Candidate search | 10,000 metadata rows inspected/page, up to 8 proposal links/result |
| Verified profile read | Shared 16-file/64 MiB/1-million-line budget, 64 matching pinned habits and 64 accepted preferences |
| Active citations | 32 per claim |
| Parsed cited records | 64 KiB/record, 8 MiB total work |
| Codex provenance | 4,096 contexts, 64 KiB per metadata/context/message record |
| Legacy scan/add | 256 MiB transcript input |
| Provenance receipts, supersession relations, renewal lineage | 20,000 rows per collection within the store cap |
| Shown review events or adjacent relations/lineage | 20 with explicit truncation |
| Profile database | 64 MiB |

All transcript input must be valid UTF-8. Human records are capped at 64 KiB and
timestamps at 64 printable ASCII bytes. Unsupported or unchecked claims remain
local review data. Incomplete verification is reported rather than interpreted
as an empty, verified profile. Read-only access to older stores does not migrate
them or invent missing review pins.
