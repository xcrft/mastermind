# Build a personal profile

At the end of this guide your agent gets your reviewed work preferences on
every prompt, and you know how to review what it learned from you. Setup is one
command. Reviewing the first drafts takes a few minutes once some work has been
captured. Native hooks need macOS or Linux. Every rule mentioned here is defined
once in the [persona reference](../reference/persona.md).

## Set up capture

1. Run this inside the repository you work on:

   ```bash
   mastermind init
   ```

   On first setup, `init` picks the active or installed native clients, turns
   on profile delivery and sets `--mining task`. If an older setup runs a
   background miner, switch with `mastermind init --mining task`.

2. Restart the client so it loads the MCP tools, trust the project and review
   the generated hooks in `/hooks`. `init` writes the hooks but can't trust them
   for you. Claude Code keeps them in `.claude/settings.local.json` and Codex in
   `.codex/hooks.json`.

3. Check capture after a few prompts:

   ```bash
   mastermind miner hooks status --client codex --project-root .
   ```

   Use `--client claude` for Claude Code. The output reports registration, the
   capture grant, a `SessionStart` in the current capture generation, processed
   episodes, task mining reports and recorded profile offers. It only reads.
   Event counts describe capture and say nothing about extraction quality. An
   offer doesn't show that the model used it. The states are in
   [native capture](../reference/persona.md#native-capture).

With task mining, the current agent reports preferences, choices or corrections
it saw in your prompt, and local code saves them as drafts after a complete
`Stop`. No separate model runs, but the extra context and tool calls count
against the client's normal usage. How it checks tickets and sources is in
[task mining](../reference/persona.md#mining-in-the-current-task).

## Review the drafts

1. List episodes and open one:

   ```bash
   mastermind miner hooks episodes --project-root . --limit 20
   mastermind miner hooks show '<capture-episode-id>'
   mastermind miner hooks draft '<draft-id>'
   ```

   `show` lists coverage gaps, the kept user and assistant text, tool and
   analysis receipts and draft IDs. Tool bodies are not kept, and a missing
   tool result still blocks mining. Drafts come from the local detector, which
   finds exact explicit statements without a model, or from the current agent,
   which can send up to two candidates through `mmcg_mining_submit`.

2. Judge the draft. A good draft keeps its condition. "Short reviews only for
   simple changes" must stay conditional and not become a rule about every
   review. Only your own prose counts as evidence. Code, pasted text, quotes,
   assistant suggestions and tool output never do. A `dependent_observation` or
   `unknown_influence` draft stays inspectable but can't support a habit.

3. Propose it, attesting authorship only for words you wrote yourself:

   ```bash
   mastermind miner hooks propose '<draft-id>' --revision '<draft-revision>' \
     --episode project-pr-123 --attest-human
   mastermind miner habit show '<habit-id>'
   ```

   For `--episode`, use a stable task or PR name and reuse it across sessions,
   retries and forks of the same work. The generated capture ID is not a task
   name. The proposal is still a candidate.

4. Activate the habit once it has current support from two distinct sessions
   and two distinct tasks, with no open counterevidence:

   ```bash
   mastermind miner habit observe '<habit-id>' --revision '<review-revision>'
   ```

   Don't relabel one task to fake independent support. Global habits and
   preferences have their own rules, see
   [review transitions](../reference/persona.md#review-preferences-and-habits).

To run the local detector again over complete kept episodes, use
`mastermind miner hooks mine-local --project-root . --limit 16` and pass the
returned `next_after` as `--after` to continue. Replay calls no provider and
can't rebuild missing events. Its measurements are in
[persona evidence](../reference/persona-evidence.md#explicit-statement-detector).

## Use the profile in a task

With profile access on, each admitted prompt gets a planner slice through the
hook's `additionalContext`. Name the files you mean in the prompt, because
literal repository paths select the advice, and without paths the
language-specific advice is left out. Native task execution gets the executor
slice for its approved scope, and the independent reviewer gets no profile, see
[who gets which slice](../reference/persona.md#who-gets-which-slice).

| To | Run |
|---|---|
| Stop automatic delivery and local extraction, keep capture and MCP reads | `mastermind miner hooks setup --client codex --project-root . --disable-profile --write` |
| Revoke profile access for the selected clients | `mastermind init --profile-access off` |
| Stop future capture and mining | `mastermind init --mining off` |
| Remove only the hooks, keep local evidence | `mastermind miner hooks setup --client codex --project-root . --remove --write` |
| Delete an episode's raw text, archived copies and dependent drafts | `mastermind miner hooks forget '<capture-episode-id>' --revision '<episode-revision>'`, then `mastermind miner habit refresh` |

`forget` doesn't erase transferred profile audit quotes, backups or data
already sent to an external processor.

## Optional: separate model analysis

These send your kept text to a provider, so read it first.

| To | Run |
|---|---|
| Analyze one episode with its captured client and model | `mastermind miner hooks analyze '<capture-episode-id>' --revision '<episode-revision>' --provider native --timeout 60` |
| Use your own processor | Replace `--provider native` with `--processor /absolute/path/to/persona-processor` and repeat `--processor-arg=VALUE` |
| Run a background miner with a budget | `mastermind init --mining on --provider native --max-calls 64 --max-runtime 3600`, then `mastermind miner status --json` and `mastermind miner stop` |
| Refine each prompt | `mastermind init --refiner on --provider native`, then `mastermind miner hooks intake '<intake-id>'` with the ID from `intake` in `hooks show` |
| Bind a refined intake to a spec | `mastermind miner hooks bind-task '<intake-id>' --spec .mastermind/tasks/001-example/spec.md` |

A repeated `init` keeps the spent miner counters, and `mastermind miner start`
renews a run. Each refinement is one extra request outside the miner budget,
and the agent still gets your original prompt. A binding records where the task
came from but approves no scope and starts nothing. The rules are in the
[processor contract](../reference/persona.md#hook-processor-contract),
[managed workers](../reference/persona.md#managed-workers) and
[prompt intake](../reference/persona.md#prompt-refinement-and-intake).

## Recover stopped capture

Check what stopped, then recover. Recovery starts a new capture generation.
Restart the client afterwards. Missing events stay missing, and changed
evidence needs review again.

```bash
mastermind miner hooks status --client codex --project-root .
mastermind miner hooks recover --client codex --project-root .
```

| What you see | What it means | What to do |
|---|---|---|
| `grant.gap` is `capture_delivery_failed` | A journal write failed, and capture for this client and project stopped | Fix the cause, then recover |
| `grant.gap` is `journal_full` | The journal reached its 64 MiB limit with rows that archiving can't free | Run `mastermind miner hooks archive --project-root .`, remove episodes you no longer need with `hooks forget`, then recover |
| `capture.markers` above 0, empty `grant.gap`, a next action that names `recover` | A delivery stopped before it reached the journal, for example a killed hook. `oldest_marker_age_seconds` shows how long ago | When no turn is running, recover |
| `grant.gap` is `capture_recovery_incomplete` | Recovery couldn't remove every marker, so capture stays stopped | Make `~/.mastermind/.persona-capture` writable for your user, then recover again |
| Hook error or `capture_fence_error` names an unexpected entry | A file such as `.DS_Store` is in `~/.mastermind/.persona-capture` | Move that entry out of the directory |
| Doctor `capture fence markers` names a project that no longer exists | The project folder was deleted or moved, and its grant and markers stayed | Run the printed `recover` command with `--project-root` set to the old path. It works without the folder |
| Doctor `capture fence markers` reports markers with no known grant | They belong to a client and project with no grant | Recover the projects that have one. Remove the files with the printed prefix by hand once no session of that client runs |
| Hook error `capture fence limit reached` | This client and project have 1,024 unfinished deliveries | Recover this client and project |
| Hook error `capture fence directory is full` | Other projects hold 4,096 markers | Recover each client and project with a `capture_delivery_failed` gap or pending deliveries |
| `database_bytes` stays near `database_byte_limit` | Active episodes, running sessions or rows a task still needs fill the journal | Run `hooks archive` for each captured project until `archived` is 0 and `database_bytes` stops falling, then recover each stopped client and project |
| Hook status reports `capture_journal_unavailable` | The journal is unreadable, over 64 MiB or written by a newer version | Upgrade if the error names a newer format version. If it is too large, back it up and run `VACUUM` on it with `sqlite3` |

Ending a session while capture is stopped records nothing, so don't wait for
it. Recovery releases the sessions of the old generation, and the next archive
run removes their receipts. Archive limits are in
[retention](../reference/persona.md#local-extraction-and-retention).
