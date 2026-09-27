# Mine working habits from interactions

Mastermind can collect selected client interactions locally, extract candidate
habits with an explicit processor, and publish reviewed habits through MCP.
The profile describes observable working approaches and preferences. It is
advisory context for an agent, not a personality score or permission policy.

The process is:

```text
capture → inspect → analyze → attest authorship → review habit → select for context
```

## 1. Enable local capture

From the repository you want to collect, preview and install hooks:

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

| Setup effect | Boundary |
|---|---|
| Capture grant | This client and canonical project root only |
| Existing hooks | Preserved |
| Native hook trust and profile reads | Require separate authorization |
| Activation | Restart the client to capture `SessionStart` |
| Installed receiver | Local capture. Prompt refinement requires a selected processor. Habit analysis uses a separate worker |
| Background mining | Optional `worker start`, never started by hook installation or status checks |

### Check each boundary

`hooks setup`, `hooks status` and `mastermind doctor` expose the same read-only
readiness observations:

| Component | What the state establishes |
|---|---|
| Native registration | Expected definitions are current, missing/stale, unavailable or unsupported. Local disable flags are separate |
| Capture | This client and project have a grant, a revocation or an incomplete delivery |
| Session | `SessionStart` was observed in the current capture generation. Client trust/loading remains unverified |
| Refiner | A processor is configured. Provider execution and interpretation quality are not tested |
| Managed miner | Worker ownership, state and budget. Foreground workers are not observed by this check |

An absent refiner or miner is optional. Unknown or missing evidence is never a
successful provider test. Status starts no process and changes no configuration.

### Refine every admitted prompt

Select a processor once for this client and project:

```bash
mastermind miner hooks setup --client codex --project-root . \
  --refiner-provider claude --refiner-timeout 8 --write
```

Use `--refiner-processor /absolute/path/to/processor` for a local or custom
processor. Repeat `--refiner-arg=VALUE` for its arguments. The built-in Claude
adapter uses the isolated API/provider contract described below. Setup without
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

| Event ordering | Capture behavior |
|---|---|
| `SessionEnd` | Closes admission. A fresh `SessionStart` is required for another prompt |
| Late `Stop` with a turn ID | Closes only its matching turn and cannot replace newer response context |
| Unknown or reused turn ID | Records a coverage gap instead of assigning the event to the active turn |
| Empty current response | Clears earlier assistant context |
| Repeated `SessionEnd` without an event ID | Closes admission and records ambiguity. A replay cannot be distinguished from another end |

## 3. Analyze with a selected processor

To send one inspected episode to the built-in Claude processor:

```bash
mastermind miner hooks analyze '<capture-episode-id>' \
  --revision '<episode-revision>' --provider claude --timeout 60
```

| Built-in Claude processor | Contract |
|---|---|
| Invocation | Explicit provider request using `--bare`, limited to one agentic turn with [`--max-turns`](https://code.claude.com/docs/en/cli-reference) |
| Disabled | Tools, MCP discovery, project settings, persistence and browser integration |
| Credentials | API/provider credentials required. Subscription OAuth/keychain credentials are not used |
| Unsupported client | Fail without an interactive-session fallback |

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
mastermind miner hooks mine --project-root . --provider claude --limit 4
mastermind miner hooks mine --project-root . --provider claude --follow
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
  --provider claude --max-calls 64 --max-runtime 3600
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
| Transitive scripts/model version | Not fingerprinted. Explicit restart/re-analysis remains available |
| Failure | Stops. Retry requires an explicit start |
| Stop/revocation | Cancels the owned process group and withholds unfinished drafts. Cannot retract input already sent |
| Crash/SIGKILL | Lost ownership reports `interrupted`. Restart respects completed checkpoints and outstanding lease expiry. Cleanup of an already running external processor is not guaranteed |
| Boot/session restart | No automatic startup or budget renewal |
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
