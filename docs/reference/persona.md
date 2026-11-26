# Persona storage, mining and review

Mastermind keeps a private profile of how you work. It holds reviewed
preferences and habits, plus observations from your Git history. This page is the reference
for the `mastermind miner` commands, what each source may contribute, how a
claim gets reviewed and the limits on all of it. To set up capture, start with
the [persona guide](../guides/persona-hooks.md). For delivery of the profile to
an agent, see [context packets](persona-context.md). What the evidence proves
and how to reproduce the checks is in [persona evidence](persona-evidence.md).

## Storage and authority

| Data | Location | Use |
|---|---|---|
| Hook events, coverage, drafts and exposure records | `~/.mastermind/persona-events.db` | Local capture journal |
| Archived episode payloads | `~/.mastermind/persona-archive/*.json` | Private sources kept outside the working journal |
| Managed worker configuration and runs | `~/.mastermind/persona-workers/<id>/` | Optional separate processor state |
| Personal evidence, claims, reviews and grants | `~/.mastermind/style.db` | User-global profile across repositories |
| Generated profile | `~/.mastermind/style.md` | Local inspection snapshot, schema 5 |
| Unreviewed deep interpretation | `style.deep-candidate-*.md` in the profile directory | Local review input |
| Project context | Repository `CONTEXT.md` and its derived index | Separate project evidence, see `mmcg_project_profile` |

These words come up on the whole page:

| Term | Meaning |
|---|---|
| Episode | One captured interaction: a prompt, the response and the tool calls in between. It ends at `Stop`, which is the end of a response, not of your task |
| Capture generation | A numbered period of capture for one client and project. Recovery after a failure starts a new generation, and older generations keep their gaps |
| Fence | A marker file under `~/.mastermind/.persona-capture` that stops capture for one client and project after a delivery fails, until you recover |
| Exposure receipt | A record that profile or refiner text was offered to the agent before an event, so later text can be classified as influenced |
| Review pin | The exact claim revision and evidence set you approved. Any change to them drops the pin |

| Profile artifact or event | Contract |
|---|---|
| Manual `style.md` edit | Kept as fenced unreviewed notes. It cannot accept a preference or observe a habit |
| Agent retrieval | `mmcg_profile` only. There is no Markdown fallback for denied, unavailable or empty results |
| `store_revision` / Markdown `store-revision` | Identify the canonical SQL inputs |
| Live `profile_revision` | Identifies the selected claims and Git aggregate after source checks |
| Markdown `snapshot-revision` | Identifies the published aggregate, not a later MCP selection |
| Claim ID and review revision | Identify the advice supplied to a consumer |
| Publication | Serialized and atomic. Markdown must be a regular file of at most 1 MiB, and concurrent manual edits rebase with bounded retries |
| SQL store | At most 64 MiB |
| SQL committed but Markdown publication failed | Retry the same operation or refresh the view. Do not create a duplicate claim |

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
| Diff sample | Up to 400 eligible source commits, selected in monthly rounds |
| Bulk commit over 2,000 added source lines | Commit-message evidence only |
| Cache reuse | Needs matching author, detector, grammar, tooling and first-party contract |
| Tooling conventions | Kept separate from personal observations |
| Workflow measures | Commit-level delivery patterns |
| Range | Language, area and library exposure |
| Libraries | Parsed imports in complete committed source that intersect added rows. Comments, strings, standard modules and recognized first-party modules are excluded, and import coverage is reported separately |
| Interpretation | Exposure and delivery patterns say nothing about proficiency, motives or stable habits |
| Subdirectories and linked worktrees | Share one repository contribution |
| Independent clones | Keep their provenance. Support counts each SHA once and withholds conflicting measurements, historical occurrences may repeat |
| `--deep` | Explicit `claude -p` with bounded I/O and a 180 second limit. The output is a local candidate in `style.deep-candidate-*.md`, outside the active profile |
| `--force` | Replaces the whole profile, including preserved prose |

On Unix, `--deep` ends its private process group on every exit path. Windows
ends only the direct child, so descendants can outlive the timeout.

Commit support and the `medium` and `high` tiers are defined in
[persona evidence](persona-evidence.md#commit-support-and-wilson-score).

## Profile access

```bash
mastermind miner access grant . --client claude
mastermind miner access list
mastermind miner access revoke . --client claude
```

| Access condition | Result |
|---|---|
| MCP audience | `MMCG_PROFILE_CLIENT=claude` plus a matching canonical project root and client grant |
| Granted data | The global profile, including Git observations across mined repositories. There are no source-level grants |
| Selection | Project, path, language, role and workflow filters run before the selected claims' sources are read |
| Private fields | Quotes and local source paths are withheld |
| Missing access or absent store | `access_denied`. No store is created |
| Delivery budget | `init --profile-budget <256-8000>` (default 4000) writes `profile_budget_tokens` to `.mastermind/setup.json`. `mmcg_profile`, the prompt hook and the context person layer use it when the caller passes no `budget_tokens`. The key is written only when it differs from the default |

MCP arguments and result fields are in the [MCP tool reference](mcp.md#mcp-tools).

### Profile delivery

| Result field or choice | Contract |
|---|---|
| Feedback order | Global, path, language, repo/project, role, workflow. Inside each tier the newest `last_at` comes first, then the stable key |
| Claim admission | Reviewed feedback and habits alternate, then Git diagnostics follow. Whole entries are admitted, and an oversized entry is skipped instead of losing its conditions or exceptions |
| `feedback_total` | Eligible feedback before the 64-rule source-verification cap and before the response budget |
| `omitted` | Lists with no delivered entries, plus omitted metadata fields. A partly delivered list is not in it |
| `omitted_counts` | Entries withheld from each selected list. At the minimum budget this detail can be omitted too |
| `doctor` budget advice | Counts real feedback delivery and recommends a byte-based budget that fits all verified feedback and habits, without Git diagnostics. It is an estimate, not measured model tokens |

## Native capture

The first client setup through `init` turns on capture, profile access and task
mining on macOS and Linux. Windows has no native hooks or managed workers.
Direct `hooks setup` only previews changes until you add `--write`:

```bash
mastermind miner hooks setup --client codex --project-root .
mastermind miner hooks setup --client codex --project-root . --write
mastermind miner hooks status --client codex --project-root .
```

| Setup operation | Contract |
|---|---|
| Capture grant | Exact native client and canonical project root |
| Existing hooks | Kept in `.codex/hooks.json` or `.claude/settings.local.json` |
| Direct setup | Reuses the configured profile reader or the client's existing read grant. It creates no new read grant |
| Ordinary setup | Keeps delivery and refiner choices and starts no managed worker |
| `--disable-profile` | Stops delivery and automatic local extraction. Capture and MCP read access stay |
| Turn delivery back on | `--profile-client CLIENT` or `init --profile-access on` |
| `--remove --write` | Revokes capture before it edits configuration. Local evidence stays |
| Client trust and loading | Done in the native client. Restart it for a fresh `SessionStart` |

`hooks status` and `doctor` report each part on its own: registration,
capture, session, local analysis, task mining, profile, refiner and managed
worker. Registration is current, missing or stale, unavailable or unsupported.
A `SessionStart` in the current generation shows the hooks ran, not that the
client trusts them, and an offer or an empty analysis counts as an observation,
not as model use. Status starts no process, approves no claim and creates no
drafts. `pipeline.next_actions` says how to recover missing evidence.

| Capture event | Admission and coverage |
|---|---|
| `SessionEnd` | Closes prompt admission until a fresh `SessionStart`. An empty closing receipt after `Stop` keeps the episode's evidence revision |
| Late `Stop` with turn ID | Closes only the matching turn and keeps newer response context |
| Unknown or reused turn ID, or overlapping prompts without both IDs | Recorded as a coverage gap, with no guessed attribution |
| Empty current response | Clears earlier assistant context |
| Repeated `SessionEnd` without event ID | Recorded as replay ambiguity |
| Lost retained text | Marks every episode that uses that event incomplete, including later context uses |
| Tool body | Not kept. Native identity, digest and pairing are recorded, and a missing result still blocks evidence |
| Ambiguous identity, unattributed loss, fork or delivery failure | Fences the session or capture grant |
| Episode record over 320 KiB | Text of later events is dropped with an `episode_size_limit` gap. Capture stays open |
| More than 128 open tool calls in a turn | Later calls are not paired and get an `episode_tool_limit` gap |
| Episode captured by an older version | Keeps its original gaps. Upgrading cannot repair it |
| `MMCG_INPUT_ORIGIN=controller` or the miner recursion guard | Generated input is skipped before any journal write |
| `MMCG_INPUT_ORIGIN=automation` | Client and model observations stay, but input is labeled `automation_or_agent` and cannot supply personal quotes |

After an admitted prompt and its exposure receipt commit, the hook can add a
planner slice of the profile through `additionalContext`. It selects advice by
literal repository paths in the original prompt, and without paths it withholds
language-scoped advice. A refiner continuation can use the paths and workflow
of its explicitly bound spec, but an ordinary request never inherits the
previous task's scope. A missing `SessionStart`, incomplete capture, changed
grants or unavailable storage withhold delivery and keep the captured event.

| Recorded influence | Candidate extraction and promotion |
|---|---|
| `no_recorded_prior_exposure` | Eligible original prose still needs authorship attestation and review |
| `dependent_observation` | Inspectable, but cannot count as unexposed habit support |
| `unknown_influence` | Inspectable, and needs new eligible evidence before promotion |
| Generated or refined text | Context only, never positive habit evidence |

Influence is recorded before the current advisory. Profile and refiner offers
and recognized MCP or profile-file reads affect later events, and resume or
recovery does not clear them. Every support and contradiction counts toward a
draft's class. Detection covers the supported delivery paths only. Session and
episode summaries keep up to 32 exposures and an omission count, and committed
context offers rank above possible tool reads.

## Mining in the current task

With `--mining task`, the current native agent does the mining and no
processor is launched. `UserPromptSubmit` offers a ticket and asks the agent to
report through [`mmcg_mining_submit`](mcp.md#mcp-tools) before it finishes.
The extra context, reasoning, tool calls and continuation use the client's
normal allowance, and there is no provider fallback.

| Task submission | Contract |
|---|---|
| Ticket | Bound to the original prompt event, capture generation, client and repository |
| Candidate | `when`, `behavior`, `exception`, `evidence_kind` and the exact original `quote` |
| Bounds | 0-2 candidates, submission at most 8 KiB. Text and citation limits match the [processor schema](#hook-processor-contract) |
| Source selection | Derived by the host. The caller cannot pick another root, client, session or author |
| Client and model metadata | From native capture, never from the caller. The proposer's identity stays unverified |
| Admission | Rejects stale or foreign tickets, changed sources, ineligible quotes and incomplete capture |
| Retry | An identical active submission is idempotent. A conflicting one is rejected |
| Missing report | One `Stop` continuation while capture is complete. `stop_hook_active` and the saved checkpoint prevent a loop |
| Continued capture | The first stop is a `MiningCheckpoint`, not a closed response. Codex's generated prompt is a `MiningContinuation` in the original episode with origin `automation_or_agent`, so it cannot support a personal claim |
| Before `Stop` | Pending submission. No analysis and no saved draft |
| Complete `Stop` | Rechecks source, grants and mode, then seals unreviewed drafts |
| Empty report (`candidates=[]`) | A completed analysis receipt with client and model binding and result `no_signal`, without a draft |
| Second missing report | Recorded as skipped analysis, never as an empty successful result |
| Unavailable tool or incomplete capture | Finishes normally and keeps a skip reason when a ticket was offered. Missing events cannot be rebuilt |
| Late paired result after `Stop` | Sealing is retried when capture becomes complete. Later source changes do not rebind a completed draft |
| Delayed Claude model | The local finalizer waits up to 4 seconds (`MMCG_TASK_FINALIZER_WAIT_MS`, at most 120,000) for the transcript record, outside the hook deadline. A transcript that is missing, still being written, slow to read or cut inside a character is not ready yet, so the binding stays pending. A mismatched or malformed record never supplies a model |
| Finalizer | Submitted work is saved before sealing. A journal owner token and one private file lock serialize the detached binding and the optional authored Git refresh. Completion is recorded after the refresh succeeds or when there is no profile access. No model call |
| Recovery | Pending bindings, interrupted workers and failed refreshes are retried by ordinary native events, native executor context, reviewed completion or `hooks mine-local`, at most four pending episodes per pass. A failed refresh waits for the next boundary and starts no worker or retry chain |
| Inspection | `hooks show` exposes `task_mining`, its result or skip reason, finalization status, attempts and reason, analyses and draft IDs. `hooks status` counts reports for active episodes in this generation, including empty results, skipped analysis and unreported old tickets. Read full content with `hooks draft` |
| `legacy_completed` | A completed ticket from an older version with no saved candidate count. It is not a `no_signal` result |
| Mode change | `init --mining task` stops the managed miner. Repeated `init` keeps saved choices |

A sealed draft is unreviewed. Sealing attests nothing and activates no rule, and
an offered instruction does not show the agent followed it. A new client
session is needed to load an updated MCP catalog. Continuation follows the
[Codex Stop protocol](https://learn.chatgpt.com/docs/hooks#stop) and the
[Claude Stop protocol](https://code.claude.com/docs/en/hooks#stop-decision-control),
and it cannot make a client run untrusted hooks or supply a missing MCP tool.

## Local extraction and retention

With profile delivery on and an existing read grant, complete closed episodes
also go through the `persona-explicit-v2` detector. It is the same detector
transcript collection uses, and it calls no model. `Stop`, later context and
late tool results queue the changed episode revision.

| Local extraction | Contract |
|---|---|
| Text | Exact eligible original user prose, with no inferred role, motive or result |
| Bounds | 128 prompt lines, 8 drafts per episode, a complete normalized statement of at most 200 characters |
| Multiline condition or exception | Kept in the same source span. An oversized span is omitted whole |
| Checkpoint | Episode revision and processor fingerprint, including empty results |
| Replay | `hooks mine-local`, 1-16 episodes per page. Pass `next_after` as `--after` to continue the same pass, and start without it after sources change |
| Retry | Native events, executor context and reviewed completion process up to four queued entries under current grants, with bounded backoff on failure |
| Unavailable store | Work stays queued until the next trigger. No background service starts |
| Delivery disabled or access revoked | Automatic local processing pauses |
| Changed bindings | Need authorship review again |
| Quality | See the [detector evaluation](persona-evidence.md#explicit-statement-detector). Real-history accuracy and task benefit are not measured |

Capture moves inactive payloads to the archive when active episodes or the
database size push on the limit. Current episodes stay in the working set.
Archives keep gaps, review bindings and the original evidence revision, and a
source read checks the archive digest. A missing or changed archive withholds
the claims that depend on it. `pipeline.retention` shows active and archived
counts and `database_bytes` against `database_byte_limit`.

At 48 MiB of used pages the journal archives episodes. Every archive run, from
a hook delivery or from `hooks archive`, also frees the rows that ended
sessions and older generations no longer need:

| Rows of an ended session | What stays |
|---|---|
| Event receipts other than `SessionStart` and `SessionEnd` | Nothing. Up to 4,096 rows per run across all projects. The archived episode keeps its tool trace |
| Intake and task-mining rows of archived episodes | A summary in the episode metadata, which `hooks show` reads. Up to 64 episodes per run |
| Intake that a task binding cites, submitted or unfinished task mining, pending intake | Kept until the binding, finalizer or hook is done with it |

Running sessions keep their receipts because they pair tool results and
deduplicate replays. So the `events` counts in the capture summary cover
retained receipts, not every delivered event. A replayed event of an ended
session is ignored, but if that session resumed, the replay can join its
current turn and mark that episode incomplete.

```bash
mastermind miner hooks archive --project-root . --limit 32
```

A hook delivery archives up to 32 episodes and stops 250 ms after the first
one, so the hook stays inside its 3 second timeout. `hooks archive` has no time
limit.

`persona-events.db` and `style.db` store their format version in SQLite
`user_version`. A store written by a newer version is refused with an error
that names both versions. `style.db` data migrations run once per version.

`style.db` is format 2, so Mastermind 4.0.x refuses a store that a newer
version has opened for writing. Format 1 could replace one claim with another
and renew a rejected habit. The first writable open converts those records in
one transaction:

| Format 1 record | After conversion |
|---|---|
| Replaced preference or habit | `rejected`. Its acceptance or observation is removed |
| Habit created by renewal | Back to `candidate` without its observation, so review it again |
| Replacement and renewal tables | Dropped |

Each status change gets a review event named `replacement_removed:<old>->rejected`
or `renewal_removed:<old>->candidate`. A read-only open, such as `feedback show`,
`habit show` or a profile read, does not convert. When the store still holds
those records, it fails and says that the next automatic profile refresh or any
command that writes the profile converts them. The refresh after each task and
`miner access revoke` are such writes, so no manual step is needed.

The 2,000-episode limit applies to active payloads. Metadata and other journal
records share the 64 MiB database cap. The archive grows until you run
`hooks forget`, which removes archived copies and the copied context next to
them. Transferred profile audit quotes, backups and data already sent to an
external processor are outside that erasure. Recovery starts a new generation
and does not rebuild lost events or clear old gaps.

## Hook processor contract

| Entry point | Execution contract |
|---|---|
| [Native capture](#native-capture) | Collects events and, with profile delivery on, bounded explicit-statement candidates. No provider and no automatic acceptance |
| `hooks mine-local` | Explicit bounded replay of complete episodes through the same local detector |
| `hooks analyze` or foreground `hooks mine` | Explicitly selects a processor |
| Custom processor | Direct argv, no shell in between, runs with your permissions |

The processor reads a JSON request on stdin with `schema`, `instructions`,
`response_example` and `episode`. Stdout must hold one strict JSON object:

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
| `when`, `behavior` | 8-200 characters each |
| `exception` | 1-200 characters. Keep the limits or say that none was observed |
| `rationale`, `outcome` | `null` in schema 1 |
| `role` | `null`, `planner`, `executor` or `auditor` |
| `workflow` | `null` or an identifier of lowercase ASCII letters, digits, underscore and hyphen, at most 64 characters |
| `evidence_kind` | `technical_approach`, `workflow_pattern`, `communication_preference`, `tool_preference` or `review_preference` |
| `supports`, `contradictions` | Exact `{event_id, quote}` citations, quotes of 8-300 characters |

| Evidence or processor condition | Contract |
|---|---|
| Positive support | An exact user-prose span with `kind=UserPromptSubmit` and `origin=user_channel_unverified` |
| Contradiction | Can also cite `next_turn_context` |
| Ineligible span | Code, quotation, pasted document, instruction wrapper, assistant or tool text, or a recognized secret |
| Exact source match | Shows the words are in the source, not that a human wrote them or that the reading is right |
| Coverage gap | The analysis is rejected |
| Known profile or refiner influence | The exposure class stays, so it cannot become independent habit support |
| Inference boundary | No identity, psychology, sensitive traits, permissions or global habit from one task |
| `--provider native` | Resolves to the captured client and passes the captured model. Explicit `claude` or `codex` must match it. Episode text may leave the machine |
| Credentials | Native CLI login, subscriptions included. No credentials are copied |
| Unsupported flag | Fails, with no fallback to a normal client session |

| Client | How it runs and where the model comes from |
|---|---|
| Claude Code | `--safe-mode --tools "" --strict-mcp-config` with an empty MCP list and no persistence. The model comes from `PostModelSwitch` or, when Claude omits it, from a bounded transcript tail that must match the session, project, latest response and prompt ID. A pending transcript flush is retried without a model call |
| Codex | Ephemeral read-only run with user config, project instructions, hooks, plugins, apps, memory and shell tools turned off. The account directory is reused under a private HOME. The model comes from model events. A result that contains a tool execution is rejected |

Missing model metadata skips separate semantic mining and keeps local
collection. Closed episodes keep their original model. Catalog warnings are
only diagnostics. Receipts keep the client and model source, executable digest
and adapter version, empty analyses included. A transcript binding also records
the response digest and any prompt identity and digest it found.

| Capture or analysis resource | Limit |
|---|---|
| Native hook command | 3 seconds. All waits for the journal writer lock end 2.5 s after the hook starts, so a busy journal still leaves time to record the gap |
| Native JSON / retained user and assistant text per event | 4 MiB / 16 KiB. Lost prose marks each episode that uses it incomplete. An unattributed loss or an envelope over 4 MiB fences the session or capture until recovery |
| Tool capture | Metadata only, the first 32 receipts in the episode context. `ToolTrace` counts and hashes every tool event, and each native identity and digest stays in the journal |
| Journal / active episode payloads | 64 MiB / 2,000, with [archive retention](#local-extraction-and-retention) |
| Retained context events / stored bytes per episode | 128 / 512 KiB. Tool events after the first 32 update the trace without using context slots |
| Processor request / stdout / stderr | 512 KiB / 64 KiB / 16 KiB |
| Processor timeout | 1-120 seconds, default 60 |
| Drafts / supports / contradictions | 8 drafts, at most 8 supports and 8 contradictions each |
| Worker batch | At most 32 inspected episodes and 1-16 attempts, default 4 |

| Worker or capture event | Result |
|---|---|
| Successful analysis, including no drafts | Checkpoints the episode revision for the selected processor |
| `next_after` | Continues one successful pass. Start without it for a new pass |
| `--follow` | Foreground only. Polls the journal about every 2 seconds and checks expired leases every 30 seconds. Cannot be combined with `--after` |
| Failure | Stops the worker |
| Interruption | Releases the unfinished lease for retry. A request already sent to a provider cannot be taken back |
| Contention, crash, unsupported input, lost prose or missing lifecycle or tool event | Marks delivery incomplete and withholds evidence |
| Journal write failure | Records `capture_delivery_failed`, or `journal_full` when the journal reached its size limit. Keeps one fence marker and stops capture for that client and project until recovery |
| 1,024 markers for one client and project | Refuses that grant only. Other projects keep capturing |
| Recovery | New generation. Lost events are not rebuilt |

Native client contracts: [Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Codex exec](https://learn.chatgpt.com/docs/cli/reference) and the
[Claude CLI](https://code.claude.com/docs/en/cli-reference).

## Managed workers

Separate analysis is optional. Turn it on with `init --mining on --provider
native`, or start a worker directly:

```bash
mastermind miner hooks worker start --client codex --project-root . \
  --provider native --max-calls 64 --max-runtime 3600
mastermind miner hooks worker status --client codex --project-root .
mastermind miner hooks worker stop --client codex --project-root .
```

The shorter `mastermind miner start`, `stop` and `status` use the saved project
and client choices. Add `--client claude` or `--client codex` to pick one
configured client.

| Managed worker | Contract |
|---|---|
| Ownership | One owner per client and canonical root |
| Start | `started: true` means the bound child published its run state. Check `run.reason`, the run may already have failed |
| Running owner | Reused without resetting counters. Stop it before you change worker settings |
| Restart | Without processor or budget flags, reuses saved settings for a new bounded run |
| Call budget | Reserved before each call. Default 64, range 1-10,000. Client retries and token or billing use are not measured |
| Runtime budget | Includes idle time. Default 3,600 s, range 1-86,400 s |
| Timeout / batch | Default 60 s / 4, maximum 120 s / 16 |
| Repeated `init` | Keeps the run ID and spent counters, including stopped, failed, interrupted and exhausted runs |
| Checkpoint | Episode revision and processor fingerprint: executable digest, native model and adapter |
| Not in the fingerprint | Script dependencies and server-side model weights |
| Native automatic start | Saved `mining: on` and `provider: native` start or observe the worker at `SessionStart` |
| Automatic renewal | A new session or a newly completed episode can renew a finished automatic run. Live runs keep their budget. Resume, replay and explicit stop do not renew it |
| Native update | A new session or completed episode replaces an automatic worker whose executable or adapter changed under the same settings and grant |
| Changed settings, generation or custom processor | Needs an explicit restart |
| Automatic transient failure | Up to three attempts in a row within the current budget |
| Other worker failure | Stops and needs an explicit start |
| Stop or revocation | Cancels the owned process group and withholds unfinished drafts. Input already sent to a provider stays sent |
| Crash or SIGKILL | Lost ownership shows as `interrupted`. Completed checkpoints and lease expiry stay, external processor cleanup is not guaranteed |
| Optional profile refresh failure | Recorded as `run.reason: profile_refresh_failed` while the worker continues. A later successful refresh clears it, and a terminal reason such as a stop request wins |
| Status | Read-only ownership, state, heartbeat and budget. Foreground workers are not shown |
| Status in capture mode | Earlier finished workers are history. An active unwanted worker or unreadable state gives a warning |

No separate queue or login service is installed. Completed revisions use the
capture journal checkpoints.

## Prompt refinement and intake

Refinement is a separate opt-in through `init --refiner on --provider native`
or direct hook setup:

```bash
mastermind miner hooks setup --client codex --project-root . \
  --refiner-provider native --refiner-timeout 8 --write
```

For a custom processor, use `--refiner-processor PATH` and repeat
`--refiner-arg=VALUE`. Setup without these options keeps the current choice.
`--disable-refiner --write` turns refinement off and keeps capture. Restart the
client and trust the updated definitions.

| Intake result | Contract |
|---|---|
| Original | Captured text and the SHA-256 of its UTF-8 bytes. The agent still gets it |
| `passthrough` | Byte-identical original |
| `refined` | Separate proposed text, at most 16 KiB |
| `ask` | Up to 3 questions, no planner or executor handoff |
| Workflow activation | The model reads intent and must cite exact eligible original prose. The handoff to `mastermind-task-planning` is advice only |
| Continuation | Only this session's explicitly bound current spec. Changed specs, incomplete handoffs and completed tasks are withheld |
| Native delivery | `additionalContext`. No execution, tool permission or approval |
| Incomplete, redacted or generated input | The processor is not called |
| Failure or invalid output | `degraded`. The original stays and there is no workflow handoff |
| Admission changes | A concurrent revocation, reconfiguration, new prompt or session end withholds the result |
| Another session's delivery in flight | Waits up to 1.5 s for it. A marker older than any hook can run fails closed at once |
| Crash with a `pending` receipt | Unknown outcome, no automatic retry. Reads as `degraded` with reason `processor_interrupted` once the processor timeout plus 3 s has passed |
| Unstable native event identity | Ambiguous identical repeats are quarantined |

| Refiner limit | Value |
|---|---|
| Attempts | At most one per captured prompt, failures included, outside the managed miner budget |
| Processor timeout | 1-20 s, default 8 s |
| Native prompt hook timeout | Processor timeout plus 3 s. Other hooks get 3 s |
| Original / response | 16 KiB / 64 KiB |
| Combined native context | 8 KiB. An oversized refinement goes as a receipt reference, and the profile may be left out |
| Client retries and token use | Not measured |

Inspect `intake` in `hooks show`, then read it with `hooks intake INTAKE_ID`.
Bind a planned spec with `hooks bind-task INTAKE_ID --spec PATH` before
preflight:

| Task binding | Rule |
|---|---|
| Source | The current offered activation or continuation, with exact original and proposed digests |
| Replacement | `--expected-binding REVISION` from `state.intake.json`. The binding is replaced only if it still has that revision (a compare-and-swap) |
| Identical repeat | Returns the same receipt and never creates or runs another task |
| Continuation | Same session, exact spec and current binding. The latest task is never guessed |
| Crash | A `prepared` marker blocks execution. Retry the same intake, or replace the binding with a current admitted intake through `--expected-binding` |
| Revocation, gaps, forgetting | Block new use. Historical completed tasks stay |
| Local marker | IDs and digests only. Raw input stays in the global journal |
| Execution evidence | Preflight, invocation, verification and review bind the same intake revision |

A binding records where the task came from. It does not approve scope or show
that the meaning survived refinement. The executor gets the original and the
proposed text as source data in its hashed prompt. Clients can ignore or cut
context, or continue after a hook timeout, so neither a receipt nor an offered
advisory shows that the workflow ran.

## Transcript admission

| Client | Source and attribution requirements |
|---|---|
| Claude Code | A selected JSONL file under `~/.claude/projects/<project>/` with explicit human origin, a valid `sessionId`, an absolute `cwd` on every record, and a cited human turn whose `cwd` is inside the project root, the project's Claude folder or the system temp directory |
| Codex | An explicit path under `CODEX_HOME` (default `~/.codex`), in `sessions/YYYY/MM/DD/rollout-*.jsonl` or `archived_sessions/rollout-*.jsonl`, with one initial `session_meta` with `source=cli\|vscode`, `thread_source=user`, a matching `turn_context` and the canonical project `cwd` |

| Transcript condition | Admission or binding |
|---|---|
| Codex user text | Aligned `response_item` user message with `user.text` content |
| Claude human `cwd` | Directory aliases resolve through the nearest existing parent when the recorded directory is gone. Paths with `..` are skipped |
| Claude agent or tool record | Can run anywhere. Its `cwd` never rejects the transcript |
| Unknown attribution schema, fork, conflicting context or a Codex project change (`turn_context` cwd) | Unsupported |
| Compaction, attachment, pasted or service content, tool output or subagent message | Cannot supply personal quotes |
| Quote | Bound to the exact source line, segment and digest. Codex also binds session and turn context |
| Copy or archive | Keeps one source identity |
| Moved known source | Collect or cite it again to rebind the locator |
| Changed evidence | Needs review again |
| Separate sessions | Do not by themselves show separate tasks or human authorship |

## Collect and sync an inbox

```bash
mastermind miner collect --project-root . --transcript /path/to/session.jsonl --dry-run
mastermind miner collect --project-root . --transcript /path/to/session.jsonl
mastermind miner sources list --project-root . --limit 50
mastermind miner sources exclude SOURCE_ID --project-root .
mastermind miner sources include SOURCE_ID --project-root .
mastermind miner sync --project-root . --dry-run
mastermind miner sync --project-root . --limit 16
```

| Collection operation | Contract |
|---|---|
| Repeated `--transcript` | Selects several files explicitly |
| `collect` and `sync` | Use the [explicit-statement detector](persona-evidence.md#explicit-statement-detector) with no model call. They write private observations, not approved claims |
| Changed / unchanged snapshot | Rescanned / rows left as they are |
| Dismissed or removed observation | Keeps its review history |
| `sync` | Rereads registered selections only. No session discovery or search for moved files |
| Excluded source | Stays visible with `sync_enabled=false`. Explicit collection does not turn it back on |
| Source exclusion | Does not withdraw reviewed evidence or publish the profile |
| `next_cursor` and `--after` | Continue the same pass. Start a new pass without a cursor, pages are not frozen |
| Source listing | Stored metadata, `freshness=not_checked` |
| Sync validation | Checks identity and attribution before deduplication |
| Unavailable, redirected, invalid or over-budget source | Aborts the page without moving checkpoints |
| Atomic SQL commit | Does not make sequential transcript reads simultaneous |
| Dry run | Does not pin the inputs of the next write |

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

| Inspection or proposal | Contract |
|---|---|
| ID and revision | Use the full values from `show` |
| Search | Literal, Unicode case-insensitive match over kept inbox quotes only. No match does not mean the signal is absent |
| Search reads | Filters run before source reads. Results keep citation identity, freshness and bounded proposal links |
| Proposal transaction | Checks the saved citation and project under the writer lock, then commits definition, evidence, receipt and event together |
| Proposal state | Candidate only, never accepted or observed |
| Curator input | Scope, episode, condition, behavior, outcome and exceptions need source review. No guessed motives or episode independence |
| Exact retry | Same claim and evidence IDs |
| Changed definition or episode | A new request |
| Moved citation rebound | Drops any earlier acceptance or observation |
| New transfer | Cannot quietly add support to a reviewed claim, revive a terminal claim or restore dismissed evidence |

| Claim input | Allowed values and matching |
|---|---|
| Preference default scope | `project:<stable-id>` |
| Explicit preference scope | `global`, `language:<name>`, `repo:<name>`, `path:<prefix>`, `project:<id>`, `role:planner\|executor\|auditor`, `workflow:<name>` |
| Preference category | `code`, `process`, `communication`, `tooling`, `review` |
| Habit default scope | The verified project. `--global` asks for a cross-project claim |
| `--habit ID` | Existing candidate habit with matching definition, scope, role and workflow |
| No `--habit` | The habit with this exact definition, scope, role and workflow. It is created when none exists |
| Existing transfer receipt | Stays bound to its original claim and cannot move evidence to another habit |

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
| Global independence | Same source rules | Also two distinct configured Git origin URLs, not just two checkout paths |
| Open counterevidence | | Unresolved `contradicts` or `limits` citations block observation |

When you did not pick an inbox observation, curate a habit directly:

```bash
mastermind miner habit propose --project-root . --transcript /path/to/session.jsonl \
  --quote "Exact human words" --episode TASK_ID --when "Situation" \
  --behavior "Action" --outcome "Cited observed result"
mastermind miner habit cite HABIT_ID --project-root . --transcript /path/to/other.jsonl \
  --quote "Exact human words" --episode OTHER_TASK_ID --relation supports
```

Reuse one episode ID for every session of the same task. `cite` also takes
`--relation contradicts` and `--relation limits`.

| Review event | Result |
|---|---|
| Approval | Pins definition, scope, role, workflow and the complete kept evidence set |
| Change, rebind or dismissal | Drops the review pin |
| Exact duplicate evidence | Keeps the review pin |
| Publication and retrieval | Recheck sources and withhold missing, changed, unverifiable or over-budget claims |
| Interactive-terminal gate | A local workflow check, not proof of who is at the keyboard |

Older feedback commands still work for local inspection and import:

| Older operation or state | Behavior |
|---|---|
| `feedback scan`, `feedback add`, `feedback import-memory` | Local inspection and import. Transcript input up to 256 MiB |
| Row without an exact source locator | `legacy-unverifiable`. It cannot be accepted or published |
| Row in a supported category | Collect the original human source and propose a strictly bound preference |
| `memory` category | Needs manual curation into a supported category. There is no automatic migration |
| Older evidence added to an accepted preference | Drops the review pin |

### Replace a claim

There is no replacement command. To replace a preference or habit, reject the
old one with `feedback reject` or `habit reject`, then propose, cite and review
the new one as usual. A rejected claim stays rejected, and a later proposal
cannot add evidence to it.

## Work and storage limits

| Operation or collection | Limit |
|---|---|
| Source list page | 1-100 rows |
| Sync page or collection | 16 files, 32 MiB per file, 64 MiB total, 1 million JSONL lines |
| Collection candidates | 8,192 eligible human segments per file, 512 observations from changed sources |
| Stored inbox | 2,000 sources, 10,000 candidates, 20,000 revisions |
| Candidate search | 10,000 metadata rows inspected per page, up to 8 proposal links per result |
| Verified profile read | The shared 16-file, 64 MiB, 1-million-line budget, 64 matching pinned habits and 64 accepted preferences |
| Active citations | 32 per claim |
| Parsed cited records | 64 KiB per record, 8 MiB total work |
| Codex provenance | 4,096 contexts, 64 KiB per metadata, context or message record |
| Provenance receipts | 20,000 rows per collection within the store cap |
| Shown review events | 20, with explicit truncation |
| Profile database | 64 MiB |

All transcript input must be valid UTF-8. Human records are capped at 64 KiB
and timestamps at 64 printable ASCII bytes. Unsupported or unchecked claims
stay local review data. Incomplete verification is reported as incomplete,
never as an empty verified profile. Read-only access to an older store does not
migrate it or invent missing review pins.
