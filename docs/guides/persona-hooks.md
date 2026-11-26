# Build a personal profile

At the end of this guide your agent gets your reviewed work preferences on
every prompt, and you know how to review what it learned from you. Setup takes
one command. Reviewing the first drafts takes a few minutes once some work has
been captured. Native hooks need macOS or Linux. For Windows and other modes,
see [getting started](../getting-started.md).

Every rule this guide mentions is defined once in the
[persona reference](../reference/persona.md).

## Set up capture

Run this inside the repository you work on:

```bash
mastermind init
mastermind status --json
```

On first setup, `init` picks the active or installed native clients, turns on
profile delivery and sets `--mining task`. Hooks capture your interactions
locally. The current agent reports work preferences, choices or corrections it
saw in your original prompt, and local code saves them as drafts after a
complete `Stop`. No separate model is launched, but the extra context and tool
use count against the client's normal usage. Drafts need your review before
they become rules. How task mining checks tickets and sources is in
[task mining](../reference/persona.md#mining-in-the-current-task).

If an older setup runs a background miner, switch to task mining:

```bash
mastermind init --mining task
```

Then restart the client so it loads the MCP tool catalog, trust the project,
and review the generated hook definitions in `/hooks`. `init` writes the hooks
but cannot trust them for you.

## Check that capture works

```bash
mastermind miner hooks status --client codex --project-root .
```

Use `--client claude` for Claude Code. Claude Code keeps its hook definitions
in `.claude/settings.local.json` and Codex in `.codex/hooks.json`.

| Observation | What to look at |
|---|---|
| Registration | Current hook definitions, disable flags and client loading |
| Capture | The grant for this client and canonical project root |
| Session | A `SessionStart` in the current capture generation |
| Local analysis | Processed episode revisions and pending retries |
| Task mining | Saved mode, staged and completed reports, empty results and skipped or unreported analysis |
| Profile delivery | Recorded offers, including withheld or omitted context |

`status` only reads. Event counts describe capture, not extraction quality,
and an offer does not show that the model used it. Readiness lists blocked
episodes apart from turns still waiting for `Stop` or tool results. The states
are explained in [native capture](../reference/persona.md#native-capture).

## Inspect drafts

```bash
mastermind miner hooks episodes --project-root . --limit 20
mastermind miner hooks show '<capture-episode-id>'
mastermind miner hooks draft '<draft-id>'
```

`show` lists coverage gaps, kept user and assistant text, tool receipts,
analysis receipts and draft IDs. Tool bodies are not kept, and a missing tool
result still blocks mining. Read the full draft with `draft` before you judge
its meaning or source.

Drafts come from two places:

| Path | Result |
|---|---|
| Local detector | Exact explicit statements, found without a model while profile delivery is on |
| Current task agent | Up to two candidates sent through `mmcg_mining_submit` and sealed after complete capture |

A good draft keeps its condition. "Short reviews only for simple changes" must
stay conditional and not turn into a rule about every review. Only your
original prose counts as evidence. Code, pasted text, quotes, assistant
suggestions and tool output never do.

To run the local detector again over complete kept episodes:

```bash
mastermind miner hooks mine-local --project-root . --limit 16
```

Pass the returned `next_after` as `--after` to continue the same pass. Replay
calls no provider and cannot rebuild missing events. How the detector is
measured is in [persona evidence](../reference/persona-evidence.md#explicit-statement-detector).

## Review authorship and the habit

Read the cited words in your interactive terminal. Attest authorship only for
words you wrote yourself, not for text you pasted or forwarded. Check the
draft's influence class too. A `dependent_observation` or `unknown_influence`
draft stays inspectable but cannot support a habit.

```bash
mastermind miner hooks propose '<draft-id>' --revision '<draft-revision>' \
  --episode project-pr-123 --attest-human
mastermind miner habit show '<habit-id>'
```

The proposal is still a candidate. For `--episode`, use a stable task or PR
name and reuse it across sessions, retries and forks of the same work. The
generated capture ID is not a task name.

A project habit becomes active only with current support from two distinct
sessions and two distinct tasks, no open counterevidence, and your review of
the exact revision:

```bash
mastermind miner habit observe '<habit-id>' --revision '<review-revision>'
```

Don't relabel one task to fake independent support. Global habits need more,
and preferences have their own rules, see
[review transitions](../reference/persona.md#review-preferences-and-habits).

## Use the profile in a task

With profile access on, each admitted prompt gets a planner slice through the
hook's `additionalContext`. Name the files you mean in the prompt: literal
repository paths select advice, and without paths the language-specific advice
is left out. This works in capture mode with the refiner off too.

Native task execution gets the executor slice for its approved scope, and the
independent reviewer gets no profile. Who gets which slice is in
[context packets](../reference/persona-context.md#who-gets-which-slice).

To stop automatic delivery and keep capture and MCP read access:

```bash
mastermind miner hooks setup --client codex --project-root . --disable-profile --write
```

This pauses automatic local extraction too. To revoke profile access for the
selected clients, run `mastermind init --profile-access off`.

## Optional: run separate analysis

Send one inspected episode to its captured native client and model:

```bash
mastermind miner hooks analyze '<capture-episode-id>' \
  --revision '<episode-revision>' --provider native --timeout 60
```

For your own executable, replace `--provider native` with
`--processor /absolute/path/to/persona-processor` and repeat
`--processor-arg=VALUE` for its arguments. Read the kept text first, because it
goes to the provider. Isolation, JSON schema and limits are in the
[processor contract](../reference/persona.md#hook-processor-contract).

To send separate background requests for the selected clients:

```bash
mastermind init --mining on --provider native --max-calls 64 --max-runtime 3600
mastermind miner status --json
mastermind miner stop
```

Repeated `init` keeps the spent counters, and `mastermind miner start` renews a
run. Renewal, checkpoints and recovery are in
[managed workers](../reference/persona.md#managed-workers).

## Optional: refine prompts

```bash
mastermind init --refiner on --provider native
mastermind miner hooks intake '<intake-id>'
```

Each refinement is one extra provider request outside the miner budget, and
the agent still gets your original prompt. Find the intake ID under `intake` in
`hooks show`. To bind an intake to a planned spec:

```bash
mastermind miner hooks bind-task '<intake-id>' --spec .mastermind/tasks/001-example/spec.md
mastermind run-task .mastermind/tasks/001-example/spec.md --pre-only
```

The binding records where the task came from. It does not approve scope or
start execution. Replacement rules and failure states are in
[prompt intake](../reference/persona.md#prompt-refinement-and-intake).

## Recover stopped capture

Check what stopped before you recover:

```bash
mastermind miner hooks status --client codex --project-root .
mastermind miner hooks recover --client codex --project-root .
```

Recovery starts a new capture generation. Restart the client afterwards.
Missing events stay missing, and changed evidence needs review again.

| What you see | What it means | What to do |
|---|---|---|
| `grant.gap` is `capture_delivery_failed` | A journal write failed, and capture for this client and project stopped | Fix the cause, then recover |
| `grant.gap` is `journal_full` | The journal reached its 64 MiB limit with rows that archiving cannot free | Run `mastermind miner hooks archive --project-root .`, list episodes with `mastermind miner hooks episodes --project-root .`, remove the ones you no longer need with `hooks forget`, then recover |
| `capture.markers` is above 0, `grant.gap` is empty, and a next action names `recover` | A delivery stopped before it reached the journal, for example a killed hook or a journal that did not open. `oldest_marker_age_seconds` shows how long ago | When no turn is running, run `mastermind miner hooks recover --client CLIENT --project-root .` |
| `grant.gap` is `capture_recovery_incomplete` | Recovery could not remove every marker, so capture stays stopped | Make `~/.mastermind/.persona-capture` writable for your user, then recover again |
| Hook error or `capture_fence_error` names an unexpected entry | A file such as `.DS_Store` is in `~/.mastermind/.persona-capture`. No marker was created | Move that entry out of the directory |
| Doctor `capture fence markers` names a project that no longer exists | The project folder was deleted or moved, and its grant and markers stayed | Run the `recover` command the hint prints, with `--project-root` set to the old path. It works without the folder |
| Doctor `capture fence markers` reports markers with no known grant | Markers belong to a client and project that no longer have a grant | Recover the projects that have one. Remove the files that start with the printed prefix by hand once no session of that client runs |
| Hook error `capture fence limit reached` | This client and project have 1,024 unfinished deliveries | Recover this client and project |
| Hook error `capture fence directory is full` | Other projects hold 4,096 markers | Recover each client and project that has a `capture_delivery_failed` gap or pending deliveries |
| `database_bytes` stays near `database_byte_limit` | Active episodes, running sessions or rows that a task still needs fill the journal | Run `mastermind miner hooks archive --project-root .` for each captured project, and repeat until `archived` is 0 and `database_bytes` stops falling. Then recover each stopped client and project |
| Hook status reports `capture_journal_unavailable` | The journal is unreadable, over 64 MiB or written by a newer version | Upgrade if the error names a newer format version. If the journal is too large, back it up and run `VACUUM` on it with `sqlite3` |

Ending a session while capture is stopped records nothing, so don't wait for
it. Recovery releases the sessions of the old generation, and the next archive
run removes their receipts. Archive limits are in
[retention](../reference/persona.md#local-extraction-and-retention).

## Remove evidence or stop capture

To stop future capture and mining, run `mastermind init --mining off`. To
remove only the hooks:

```bash
mastermind miner hooks setup --client codex --project-root . --remove --write
```

Removal revokes capture and keeps local evidence. To delete an episode's raw
text, archived copies and the drafts that depend on it:

```bash
mastermind miner hooks forget '<capture-episode-id>' --revision '<episode-revision>'
mastermind miner habit refresh
```

`forget` does not erase transferred profile audit quotes, backups or data
already sent to an external processor.
