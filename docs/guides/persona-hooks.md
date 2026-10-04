# Mine working habits from interactions

Mastermind collects selected client interactions locally, retains explicit
statements as candidates without a model, and publishes reviewed habits through MCP.
An explicitly selected processor can also propose semantic candidates.
The profile describes observable working approaches and preferences. It is
advisory context for an agent, not a personality score or permission policy.

The process is:

```text
capture → local candidates → inspect → attest authorship → review habit → select for context
```

## 1. Enable local capture

For project onboarding:

```bash
mastermind init
mastermind status --json
```

The active client, or installed native clients, are selected automatically.
First client setup enables profile delivery and `--mining on --provider native`.
Use `--mining capture` for local learning without model calls. Each native `SessionStart` starts or
observes a bounded worker for that client. A running worker keeps its budget.
A new session may renew an exhausted or failed automatic run; resuming the same
session and explicit stop do not renew it. `mastermind miner start`, `stop` and
`status` use the saved choices. Repeated `init` preserves the spent budget. `miner start` explicitly renews a run. Personal profile delivery
is enabled for the selected project and clients; disable it with `--profile-access off`.

For direct hook control, preview and install hooks:

```bash
mastermind miner hooks setup --client codex --project-root .
mastermind miner hooks setup --client codex --project-root . --write
mastermind miner hooks status --client codex --project-root .
```

Use `--client claude` for Claude Code. Native installation and processor
execution currently require macOS or Linux.

| Client | Project configuration | Activation |
|---|---|---|
| Claude Code | `.claude/settings.local.json` | Reload and inspect `/hooks`. Managed settings may disable hooks |
| Codex | `.codex/hooks.json` | Trust the project and review the definitions in `/hooks` |

Each generated hook has an event-specific `statusMessage` so the review screen
shows what Mastermind captures. After upgrading, repeat setup with `--write`
and review the updated definitions in the client.

| Setup effect | Boundary |
|---|---|
| Capture grant | This client and canonical project root only |
| Existing hooks | Preserved |
| Native hook trust | Review in the client; init never grants trust |
| Profile reads | Enabled for selected clients on first init; explicit opt-outs are retained |
| Activation | Restart the client to capture `SessionStart` |
| Installed receiver | Local capture and bounded explicit-statement candidates when profile delivery is enabled. Prompt refinement and semantic analysis require a selected processor |
| Background mining | Started by `init --mining on` or explicit `miner start` / `hooks worker start`. Direct hook setup and status do not start it |

### Check each boundary

To keep an existing installation in local capture mode without provider calls:

```bash
mastermind init --mining capture --refiner off
mastermind miner hooks worker stop --client codex --project-root .
mastermind miner hooks setup --client codex --project-root . --disable-refiner --write
```

Repeat direct hook commands for each configured client and project. Capture and
deterministic Git mining continue locally. With profile delivery enabled, closed
complete episodes also produce local explicit-statement candidates. Semantic
habit analysis and prompt refinement stay disabled.

`hooks setup`, `hooks status` and `mastermind doctor` expose the same read-only
readiness observations:

`mastermind status` also shows current and historical event counts, complete
and incomplete episodes in the current capture generation, and whether the
saved setup requests semantic analysis. JSON reports expose these as `evidence`
and `pipeline`. Counts are scoped to the exact client and canonical project
root. Coverage gap counts describe affected episodes and can overlap. They
measure capture completeness, not semantic accuracy or accepted preferences.
These reads do not create drafts, call a processor or approve a claim.

In capture mode, terminal records from earlier workers remain visible as
history; they do not require restarting semantic mining. A worker still active
despite that saved choice, unavailable worker state, or a missing worker when
analysis is requested produces a warning. `pipeline.next_actions` explains
native registration and SessionStart recovery without fabricating past events.

| Component | What the state establishes |
|---|---|
| Native registration | Expected definitions are current, missing/stale, unavailable or unsupported. Local disable flags are separate |
| Capture | This client and project have a grant, a revocation or an incomplete delivery |
| Session | `SessionStart` was observed in the current capture generation. Client trust/loading remains unverified |
| Local candidates | Completed episode revisions for the local detector. An empty result also counts as processed |
| Profile delivery | Recorded offers in the current capture generation. Receipt does not prove client receipt or use |
| Refiner | A processor is configured. Provider execution and interpretation quality are not tested |
| Managed miner | Worker ownership, state and budget. Foreground workers are not observed by this check |

An absent refiner or miner is optional. Unknown or missing evidence is never a
successful provider test. Status starts no process and changes no configuration.

### Load the profile for each task

With profile access enabled, every admitted `UserPromptSubmit` selects a local
planner profile and offers it through `additionalContext`. This also works in
capture mode with the refiner disabled. It makes no provider calls.

```bash
mastermind init --client all --mining capture --refiner off --profile-access on
```

Direct hook setup reuses the configured reader, or this native client's existing
project read grant. Ordinary setup preserves the selection. It grants no new
profile access. The hook selects literal repository paths from the original
prompt. With no paths, language-scoped advice is withheld. An explicitly bound
refiner continuation can use its current spec's declared paths and workflow;
an ordinary request does not inherit the previous task's scope.

`run-task --exec` automatically selects the configured Claude audience and
refreshes committed Git observations before retrieving an executor slice from
the approved spec. It preserves the stored author selector and reuses eligible
diff measurements. After reviewed completion, it refreshes those observations
again and records the pinned Git revision in `state.json` under `profile_refresh`.
Uncommitted edits are not mined. `--profile-client` overrides the audience and
is retained as `state.profile_client` across review and completion resumes.
An older native task recovers the audience from its validated invocation receipt.
Completion rechecks read access; a saved audience cannot undo a revocation.
Independent semantic review still receives no personal profile.

To stop automatic delivery while keeping local capture and MCP read access:

```bash
mastermind miner hooks setup --client codex --disable-profile --write
```

This opt-out survives setup. An explicit `--profile-client codex` re-enables it;
`init --profile-access on` also reconciles delivery for its selected clients.
`hooks status` reports this component under `readiness.profile`. Exposure
receipts retain the selected paths, role and workflow. They establish an offer,
not model use. Fresh prompts retain their original prior-exposure classification.
Session exposure summaries retain the most recent 32 entries and disclose an
omission count. Earlier episode receipts and prior-exposure flags remain intact;
repeated profile delivery does not itself create a capture gap.

### Collect explicit statements without a model

With an existing profile read grant and delivery enabled, `Stop` processes the
closed episode locally. Appended later context triggers reprocessing of the
previous closed episode; changed bindings require authorship review again.
`Stop` is a response boundary, not verified task completion.

| Local candidate | Boundary |
|---|---|
| Detector | `persona-explicit-v2`, shared with transcript collection |
| Text | Exact eligible user-prose quotation, no inferred role, motive or result |
| Bounds | Prompts up to 128 lines, at most 8 drafts per episode, complete normalized statement of at most 200 characters |
| Continuation | Conditions and exceptions on later lines stay in the same source span. Oversized spans are omitted whole |
| Exclusions | Incomplete capture, generated input, code, quotations, pasted wrappers and credential-like text |
| Repeat | Checkpoint by episode revision and processor fingerprint, including empty results. Durable queue retries at native events and task boundaries |
| Prior profile/refiner exposure | Candidate remains inspectable; dependent support cannot be promoted as independent evidence |
| Publication | No automatic acceptance; authorship attestation and habit review remain required |
| Opt-out | `--disable-profile` stops automatic local extraction as well as delivery; capture continues |
| Quality | Synthetic regression checks, no measured real-history accuracy or task benefit |

Inspect candidates with `hooks show` and `hooks draft`. For an explicit local
replay of retained complete episodes:

```bash
mastermind miner hooks mine-local --project-root . --limit 16
mastermind miner hooks mine-local --project-root . --limit 16 --after '<next_after>'
```

This command invokes no provider and cannot repair missing evidence. Its limit
is 1–16 episodes per page. Start a fresh pass to reconsider revised episodes.
Status exposes this path under `pipeline.local_analysis` and reports task
benefit as `unmeasured`. Git refresh success and hook delivery are separate
observations; neither establishes correctness of a personal claim.

Unavailable profile storage leaves local work queued. The next admitted native
event, executor context or reviewed completion retries up to four entries under
the current grants. Analysis failures use bounded backoff. While idle, work stays
durable until the next trigger; no provider worker or always-running process is
started. `pipeline.local_analysis.retry_queue` exposes pending and retried work.
Disabling delivery or revoking profile access pauses automatic processing.
Every later change to a closed episode, including `SessionEnd` and late tool
results, queues its new source revision. A successful `Stop` analysis cannot
stand in for changed final evidence.

### Retain sources without filling the working journal

At the active episode limit or database pressure, capture archives inactive
payloads in private `~/.mastermind/persona-archive/*.json` files. Each session's
current episode stays in the working set. Retired incomplete episodes keep their
original gaps and missing-Stop state; archiving does not repair them. Identity,
deduplication and review bindings remain in the journal. Source reads verify the
archive digest and retain the same evidence revision; changed or missing files
withhold dependent claims. Archive storage grows until an explicit `hooks forget`.
Forgetting removes archived copies as well as the journal source.

```bash
mastermind miner hooks archive --project-root . --limit 32
```

`pipeline.retention` separates active and archived counts. The 2000-episode
limit applies to active payloads. The database still has a 64 MiB bound for
metadata and other records; archives are not proof of complete capture.

### Measure extraction and task outcomes

```bash
mastermind miner hooks evaluate-local --input evals/persona-local.json
```

The offline evaluator uses the production local extractor, binds the corpus
digest and compares exact source spans with supplied labels. Sessions cannot
cross development and held-out partitions. Optional `task_pairs` compare one
task revision with and without a profile, reporting correctness, iterations,
user corrections and elapsed time. The evaluator runs no model or task, reads
no journal, and publishes no rule. See [quality](../reference/persona-quality.md)
for the schema and the remaining measurement boundaries.

### Refine every admitted prompt

Select a processor once for this client and project:

```bash
mastermind miner hooks setup --client codex --project-root . \
  --refiner-provider native --refiner-timeout 8 --write
```

Use `--refiner-processor /absolute/path/to/processor` for a local or custom
processor. Repeat `--refiner-arg=VALUE` for its arguments. The native adapters
use the captured client and model with their existing login, as described below. Setup without
refiner options preserves the selection. `--disable-refiner --write` disables
refinement while retaining capture. Restart and trust the updated hook definitions.

```text
native prompt → local capture → durable intake → selected processor
                                            → validated advisory → native agent
```

| Intake contract | Behavior |
|---|---|
| Original | Stored from the captured text, with a SHA-256 digest of its UTF-8 bytes |
| `passthrough` | Result must equal the original byte for byte |
| `refined` | Separate proposed text, bounded to 16 KiB |
| `ask` | Up to 3 questions. No planner or executor handoff |
| Workflow activation | Model interprets intent in any language. An exact eligible user-prose citation is required |
| Activation result | Advisory handoff to the existing `mastermind-task-planning` skill |
| Continuation | Receives only this session's explicitly bound task. Changed specs, incomplete handoffs and completed tasks are withheld |
| Permission | Intake grants no execution, tools or approval |
| Native delivery | `additionalContext`. The original prompt still reaches the agent |
| Incomplete, redacted or generated input | No processor invocation |
| Failure or invalid result | `degraded`, original retained, no workflow handoff |
| Concurrent revocation, reconfiguration, new prompt or end | Result withheld when publication admission changes |

| Bound | Value |
|---|---|
| Processor invocations | At most 1 attempt per captured prompt, including failures |
| Processor timeout | Default 8 s, configurable 1–20 s |
| Native prompt hook timeout | Processor timeout + 3 s |
| Other installed hooks | 3 s |
| Original / processor response | 16 KiB / 64 KiB |
| Combined native context | 8 KiB. Oversized refinement uses a receipt reference. Profile delivery may be omitted |
| Client model/API retries and token usage | Not measured by this adapter |

Inspect `intake` in `hooks show`, then read the full receipt:

```bash
mastermind miner hooks intake '<intake-id>'
mastermind miner hooks status --client codex --project-root .
```

A `pending` receipt after a crash means the outcome is unknown and is not
automatically retried. Clients without stable event IDs cannot distinguish
identical intentional repeats from retries, so ambiguous replay is quarantined.
Native clients can ignore or truncate context and may continue with the original
after a hook timeout. An offered advisory does not prove the workflow ran.

### Bind intake to a task

After planning a verified or strict spec, bind its exact source before preflight:

```bash
mastermind miner hooks bind-task '<intake-id>' --spec .mastermind/tasks/001-example/spec.md
mastermind run-task .mastermind/tasks/001-example/spec.md --pre-only
```

```text
session → admitted intake → explicit spec binding → preflight → invocation
                                                             → checks → review → completion
```

| Handoff | Rule |
|---|---|
| Source | Current offered activation or continuation, exact original and proposed response digests |
| Replacement | Requires `--expected-binding <revision>` from the task's `state.intake.json` |
| Repeat | Same intake and target returns the same receipt. It never creates or executes another task |
| Continuation | Same session, exact spec and current binding. No repository-wide latest-task selection |
| Crash | A `prepared` marker blocks execution. Retry the same intake or CAS-replace it with a current admitted intake |
| Revocation, gaps or forgetting | Block new use. Previously completed task history remains historical |
| Local marker | Identifiers and digests only. Raw input remains in the global journal |
| Execution evidence | Preflight, invocation, verification and review bind the same intake revision |
| Authority | Binding records provenance. It does not approve scope, grant tools or verify preserved meaning |

The executor receives original and proposed text as source data inside its hashed
prompt. Receipt hashes establish which bytes were offered, not how the model used them.

Each event records prior profile/refiner exposure before a new advisory is
offered. Structural validation binds the result to its source and does not prove
preservation of meaning.

| Evidence class | Candidate extraction | Habit promotion |
|---|---|---|
| `no_recorded_prior_exposure` | Allowed for original eligible user prose | Requires authorship attestation and review |
| `dependent_observation` | Allowed, with exposure flags | Cannot count as unexposed habit support |
| `unknown_influence` | Allowed for inspection | Requires new eligible evidence |
| Generated/refined text | Context only | Never human habit evidence |

A current offer cannot reclassify the already captured original. Later messages
retain prior exposure, including a refiner `passthrough` that adds routing advice.
The host combines every support and contradiction when classifying a draft.
Resume and capture recovery cannot establish that earlier exposure disappeared.
No class establishes statistical independence, human authorship or semantic truth.

## 2. Inspect captured episodes

```bash
mastermind miner hooks episodes --project-root . --limit 20
mastermind miner hooks show '<capture-episode-id>'
```

| Inspect before analysis | Meaning |
|---|---|
| Full ID and current revision | Use the values returned by `episodes` and `show` |
| `show` | Coverage, source events and retained draft IDs |
| Capture episode / `Stop` | Interaction unit / response boundary, not task completion |
| User-channel text | Requires authorship review. Pasted documents, quotations, delegated work and automated prompts are not personal evidence |
| Assistant and tool output | Context only, not independent habit support |
| Mastermind controller prompts | Excluded from capture |

Capture version 3 keeps unavailable retained text on its attributed episode,
including late tool results. The following prompt also supplies context to the
preceding episode, so redacting that prompt marks both uses incomplete. Other
complete episodes in the same session remain usable. Ambiguous event identities,
unattributed events, forks and delivery failures still fence the session or
capture grant. Earlier capture versions and their stored gaps remain historical
evidence; upgrading never clears those gaps or declares their sources complete.

Profile delivery is optional after local capture commits. A missing
SessionStart, an incomplete prompt, a changed grant or an unavailable personal
store withholds the profile and emits diagnostic metadata. The native hook
still returns its normal protocol, preserving the recorded event. Profile text
is offered only after its exposure receipt is durable.

| Event ordering | Capture behavior |
|---|---|
| `SessionEnd` | Closes admission. A fresh `SessionStart` is required for another prompt |
| Late `Stop` with a turn ID | Closes only its matching turn and cannot replace newer response context |
| Unknown or reused turn ID | Records a coverage gap instead of assigning the event to the active turn |
| Empty current response | Clears earlier assistant context |
| Repeated `SessionEnd` without an event ID | Closes admission and records ambiguity. A replay cannot be distinguished from another end |

## 3. Analyze with a selected processor

To send one inspected episode to its native client and model:

```bash
mastermind miner hooks analyze '<capture-episode-id>' \
  --revision '<episode-revision>' --provider native --timeout 60
```

| Native processor | Contract |
|---|---|
| Selection | `native` resolves to the captured client; explicit `claude` or `codex` must match it |
| Model | Passed explicitly to the CLI from native events. When Claude omits it, Stop reads a bounded transcript tail and requires the same session, project, prompt ID when available, and exact latest response. If Claude has not flushed its response yet, the bounded worker retries without making a model call. Missing metadata skips semantic mining; local collection continues |
| Model changes | Codex model metadata is read from native events; Claude also records `PostModelSwitch`. Closed episodes keep their original model |
| Claude | `--safe-mode --tools "" --strict-mcp-config` with an empty MCP list and no persistence. Subscription auth remains available |
| Codex | Ephemeral read-only invocation with user config, project instructions, hooks, plugins, apps, memory and shell tools disabled. Its account directory is retained, with a private HOME |
| Tool output | Native Codex results containing a tool execution are rejected; CLI catalog warnings are diagnostics |
| Provenance | Processor executable digest, adapter version, client, model and model source are in the checkpoint and `show` receipts, including empty results. Transcript recovery also retains the response record digest and, when available, the prompt ID and prompt record digest |
| Credentials | Read only by the native CLI. Mastermind never copies credentials or falls back to another provider |
| Unsupported client | Fails on unsupported flags or missing login. No ordinary-session fallback |

Automated native callers can set `MMCG_INPUT_ORIGIN=automation` to retain
client/model delivery observations while labeling their prompts
`automation_or_agent`. These prompts cannot supply personal evidence. Generated
workflow controllers keep using `MMCG_INPUT_ORIGIN=controller`, which skips
capture entirely.

Native CLI contracts: [Codex hooks](https://learn.chatgpt.com/docs/hooks),
[Codex exec](https://learn.chatgpt.com/docs/cli/reference), and
[Claude CLI](https://code.claude.com/docs/en/cli-reference).

For a processor you control:

```bash
mastermind miner hooks analyze '<capture-episode-id>' \
  --revision '<episode-revision>' \
  --processor /absolute/path/to/persona-processor \
  --processor-arg=--model --processor-arg=local-model
```

| Custom processor or validation | Contract |
|---|---|
| Invocation | Direct argv with caller permissions. Absolute path is not a sandbox |
| Input / output | JSON episode and response example / one matching JSON object |
| Empty draft list | Valid result |
| Source checks | Exact revision and citations required |
| Incomplete evidence | Analysis rejected |
| Prior profile/refiner exposure | Inspectable candidates, promotion restricted by event provenance |
| Schema 1 rationale and outcome | Unknown |
| Validated binding | Does not establish interpretation accuracy |

### Process new episodes

```bash
mastermind miner hooks mine --project-root . --provider native --limit 4
mastermind miner hooks mine --project-root . --provider native --follow
```

| Worker condition | Behavior |
|---|---|
| Batch | Scan at most 32 episodes, attempt `--limit` analyses. Default 4, maximum 16 |
| Successful revision, including empty drafts | Checkpoint for that processor |
| Continue the same pass | Use the returned `--after` cursor |
| Reconsider revised episodes | Start a new pass without a cursor |
| `--follow` | Foreground worker until stopped. No service or scheduler, incompatible with `--after` |
| Failure | Stop, leave the attempt retryable |
| Interruption | Cannot retract a request already received by the provider |
| Single-episode `analyze` | Explicitly re-evaluates a revision despite its batch checkpoint |

### Run a managed worker

```bash
mastermind miner hooks worker start --client codex --project-root . \
  --provider native --max-calls 64 --max-runtime 3600
mastermind miner hooks worker status --client codex --project-root .
mastermind miner hooks worker stop --client codex --project-root .
```

| Managed worker | Contract |
|---|---|
| Ownership | One worker per client and canonical project root |
| Start result | `started: true` confirms the bound child published its run state. Check `status` and `run.reason`, a fast failure may already be terminal |
| Repeated start | Returns the running owner. Does not reset its budget |
| Saved selection | Restart without processor/budget options preserves settings and starts a new bounded run |
| Call budget | Reserved durably before each processor invocation. Default 64, range 1–10,000. Provider-internal retries and token costs are not measured |
| Runtime budget | Default 3,600 s, range 1–86,400 s |
| Processor timeout / batch | 60 s / 4 by default, maximum 120 s / 16 |
| Checkpoint | Episode revision and processor fingerprint, including direct executable digest |
| Model selection | Native model identifier and adapter version are fingerprinted per episode. Transitive script dependencies and server-side model weight changes are not fingerprinted |
| Failure | Automatic native runs retry up to three consecutive attempts within the existing budget. Other workers stop and require an explicit start |
| Stop/revocation | Cancels the owned process group and withholds unfinished drafts. Cannot retract input already sent |
| Crash/SIGKILL | Lost ownership reports `interrupted`. Restart respects completed checkpoints and outstanding lease expiry. Cleanup of an already running external processor is not guaranteed |
| Native session | With saved `mining: on` and `provider: native`, `SessionStart` starts mining. A live run keeps its budget; only a new session may renew a terminal automatic run. Resume/replay and explicit stop do not renew it |
| Native update | A new session replaces an automatic worker whose native executable or adapter changed, under the same saved settings and capture grant. No repeated init is required. Custom processors and changed budgets still require explicit restart |
| Output | Local unreviewed drafts. No automatic habit acceptance |

Settings and run state live in `~/.mastermind/persona-workers/<id>/`. Completed
revisions use the existing journal checkpoints. No separate queue or login service
is installed. Bounds come from [background.rs](../../mcp/servers/mmcg/src/miner/hooks/background.rs).

## 4. Review authorship and the habit

```bash
mastermind miner hooks draft '<draft-id>'
mastermind miner hooks propose '<draft-id>' --revision '<draft-revision>' \
  --episode project-pr-123 --attest-human
```

| Review input | Required decision |
|---|---|
| `--attest-human` | Inspect the cited words in the person's interactive terminal and attest that they authored them, rather than pasted or forwarded them |
| Terminal check | Local workflow check, not identity authentication |
| `--episode` | Stable task or PR identity chosen by the reviewer. Reuse across sessions, retries and forks of that task |
| Generated capture ID | Distinct from the reviewer-assigned task identity |
| Proposal | Creates a candidate only |
| `--habit <id>` | Adds evidence to an existing matching habit. Definition, scope, role and workflow must match |

```bash
mastermind miner habit show '<habit-id>'
mastermind miner habit observe '<habit-id>' --revision '<review-revision>'
```

| Observation gate | Requirement |
|---|---|
| Independence | Current reviewed support from two distinct sessions and two task identities |
| Counterevidence | No unresolved counterexamples or limitations |
| Approval | Interactive terminal and exact revision |
| Scope | Hook proposals are project-scoped. Cross-project habits have [additional requirements](../reference/persona.md#review-preferences-and-habits) |

Do not relabel one task to manufacture independent support.

## 5. Offer reviewed context to an agent

| Storage or interface | Purpose |
|---|---|
| `~/.mastermind/persona-events.db` | Raw hook observations, coverage, exposures, and drafts |
| `~/.mastermind/persona-workers/<id>/` | Managed worker settings, current run, heartbeat and locks |
| `~/.mastermind/style.db` | Global profile claims, citations, and review history |
| `~/.mastermind/style.md` | Generated static inspection snapshot |
| `mmcg_profile` | Live source-checked selection for the project, role, workflow, and paths |

Grant profile access separately:

```bash
mastermind miner access grant . --client codex
```

| Delivery setting | Contract |
|---|---|
| MCP audience | Set `MMCG_PROFILE_CLIENT=codex` in this project's server environment |
| Read grant | Covers the global profile, including other repositories |
| Selection | Narrows relevance, not source-level access control |
| Unavailable or unreviewed claim | Withheld |
| Live access denied | No fallback to `style.md` |

Optional delivery through native hooks uses that existing grant:

```bash
mastermind miner hooks setup --client codex --project-root . \
  --profile-client codex --write
```

| Exposure | Mining effect |
|---|---|
| Mastermind profile/refiner offer or recognized MCP/profile-file read | Marks later captured events as exposed. Original eligible prose stays inspectable |
| Draft citing any exposed or unknown-influence event | Cannot count as unexposed habit support, including exposed counterevidence |
| Generated text or repetition of injected advice | Does not establish an independent human preference |
| Other delivery paths | Detection is limited to supported paths |

The private Lens Profiles view shows selected claims, a metadata-only review
queue and recorded invocation delivery. `offered_to_process` means bytes were
offered to the native process. `model_use` remains `unknown`. A context preview
records no new delivery and does not approve any candidate.

## Maintain or remove evidence

| Evidence change | Required action |
|---|---|
| Later prompt changes an earlier episode revision | Inspect and analyze the new revision |
| User-prose eligibility rules change | Earlier analysis and source proofs become stale. Inspect and analyze again |
| Same task in a new session | Retain the same task identity |
| New or changed evidence | Review again. Old approval cannot be silently reused |

For capture failures, inspect status, stop old receivers, then recover:

```bash
mastermind miner hooks status --client codex --project-root .
mastermind miner hooks recover --client codex --project-root .
```

| Recovery effect | Boundary |
|---|---|
| New generation | Invalidates old capture receipts. Restart the client |
| Missing events | Cannot be reconstructed |
| Crash, journal contention or incomplete lifecycle | Evidence withheld |
| Older capture semantics | Raw data stays available. Start a new session or recover to collect current evidence, then review new candidates |

To stop future capture:

```bash
mastermind miner hooks setup --client codex --project-root . --remove --write
```

Removal revokes capture before editing configuration. Existing local data stays.

To remove an episode's raw text and dependent drafts:

```bash
mastermind miner hooks forget '<capture-episode-id>' --revision '<episode-revision>'
mastermind miner habit refresh
```

| `forget` effect | Limit |
|---|---|
| Raw text and dependent drafts | Removed, including raw text copied into adjacent context |
| Dependent sources | Become unavailable |
| Transferred profile audit quotes, backups or external processor data | Not erased |

## Coverage and limits

| Boundary | Interpretation |
|---|---|
| Disabled, unsupported or untrusted hooks | Capture may be incomplete |
| Native client coverage | Not every tool or interruption path is exposed |
| Complete captured episode | Describes received events, not all activity |
| Overlapping prompts without both turn IDs | The session is incomplete because a later `Stop` cannot identify the completed prompt |
| Profile delivery | Rechecks the capture generation and selected profile reader before recording the offer. Revocation cannot retract context already offered |
| Secret screening | Heuristic. Inspect text before an external processor request |
| Journal and processor limits | See [exact bounds](../reference/persona.md#hook-processor-contract) |
| Enforcement | Hooks collect evidence. They do not enforce every action or guarantee truthful model behavior |

Native behavior is documented by [Codex](https://learn.chatgpt.com/docs/hooks)
and [Claude Code](https://code.claude.com/docs/en/hooks).
