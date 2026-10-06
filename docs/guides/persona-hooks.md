# Build a personal profile

Run inside the repository you want to work on:

```bash
mastermind init
mastermind status --json
```

On first setup, Mastermind selects the active or installed native clients,
enables profile delivery and uses `--mining task`. Hooks capture interactions
locally. The current agent can propose preferences from the original prompt,
and local code saves the drafts after a complete `Stop`. No separate model is
launched. Extra context and tool use still consume the current client's usage.
Drafts require review before they become active rules.

For an existing background-mining installation, switch explicitly:

```bash
mastermind init --mining task
```

Restart the client to load the MCP tool catalog, trust the project and review
the generated definitions in `/hooks`. Init configures hooks but cannot grant
client trust. Native hooks require macOS or Linux. See
[onboarding](../getting-started.md) for other modes and Windows support.

## Check capture

```bash
mastermind miner hooks status --client codex --project-root .
```

Use `--client claude` for Claude Code. Its definitions live in
`.claude/settings.local.json`, while Codex uses `.codex/hooks.json`.

| Observation | What to inspect |
|---|---|
| Registration | Current hook definitions, disable flags and client loading |
| Capture | Grant for this client and canonical project root |
| Session | `SessionStart` in the current capture generation |
| Local analysis | Processed episode revisions and pending retries |
| Task mining | Saved mode and staged/completed submissions |
| Profile delivery | Recorded offers, including withheld or omitted context |

`status` reads these observations without starting a process or approving a
claim. Event counts describe capture, not extraction accuracy. A profile offer
does not establish that the model received or used it. The exact states and
failure behavior are in the [capture contract](../reference/persona.md#native-capture).
Readiness reports blocked episodes separately from turns awaiting `Stop` or tool
results, and points to the affected capture when earlier activation was observed.

## Inspect candidates

```bash
mastermind miner hooks episodes --project-root . --limit 20
mastermind miner hooks show '<capture-episode-id>'
mastermind miner hooks draft '<draft-id>'
```

`show` includes coverage gaps, original events, analysis receipts and draft IDs.
Read the full draft with `draft` before reviewing its meaning or source.
A captured episode is an interaction unit. `Stop` ends a response, not
necessarily the user's task.

Two automatic paths can create drafts:

| Path | Result |
|---|---|
| Local detector | Exact explicit statements, processed without a model when profile delivery is enabled |
| Current task agent | Up to two candidates submitted through `mmcg_mining_submit`, sealed after complete capture |

The task agent may skip submission when the prompt contains no concrete work
preference or the tool is unavailable. There is no forced continuation or
fallback model. For example, “short reviews only for simple changes” must retain
the condition, not become a general rule about every review.

Quotes must match eligible original user prose. Code, pasted material,
quotations, assistant suggestions and tool output cannot supply positive
personal evidence. Exact quotation does not establish human authorship or a
correct interpretation. Ticket and source checks are specified in
[task mining](../reference/persona.md#mining-in-the-current-task).

To replay complete retained episodes through the local detector:

```bash
mastermind miner hooks mine-local --project-root . --limit 16
```

Use the returned `next_after` as `--after` to continue that pass. Replay makes
no provider call and cannot reconstruct missing events. For extraction metrics,
run `mastermind miner hooks evaluate-local --input evals/persona-local.json`.
See [quality and evaluation](../reference/persona-quality.md).

## Review authorship and the habit

Inspect the cited words in your interactive terminal. Attest authorship only
if you wrote them, rather than pasted or forwarded them.

Check the draft's influence class too. Dependent or unknown-influence observations
remain inspectable, but cannot supply unexposed habit support.

```bash
mastermind miner hooks propose '<draft-id>' --revision '<draft-revision>' \
  --episode project-pr-123 --attest-human
mastermind miner habit show '<habit-id>'
```

The proposal is still a candidate. Use a stable task or PR identity for
`--episode`, reusing it across sessions, retries and forks of the same work.
The generated capture ID is not that task identity.

An active project habit needs current support from two distinct sessions and
two distinct tasks, no unresolved counterevidence, and review of the exact
revision:

```bash
mastermind miner habit observe '<habit-id>' --revision '<review-revision>'
```

Do not relabel one task to manufacture independent support. Global habits have
additional source requirements. See [review transitions](../reference/persona.md#review-preferences-and-habits)
for preference approval, rejection, withdrawal and replacement.

## Use the profile in a task

With profile access enabled, each admitted prompt receives a planner selection
through hook `additionalContext`. Literal repository paths in that prompt
select relevant advice. Without paths, language-scoped advice is withheld.
This also works in capture mode with the refiner disabled.

Native task execution selects the executor's approved scope and refreshes
committed Git observations before retrieval and after reviewed completion.
Independent semantic review receives no personal profile. See
[context packets](../reference/persona-context.md) for audience, scope and
delivery receipts.

To disable automatic delivery while retaining capture and MCP read access:

```bash
mastermind miner hooks setup --client codex --disable-profile --write
```

This also pauses automatic local extraction. To revoke profile access for the
selected clients, use `mastermind init --profile-access off`. Reviewed advice
is advisory and grants no tool permissions. Storage and access rules are in
[the persona reference](../reference/persona.md#profile-access).

## Optional: separate analysis

Send one inspected episode to its captured native client and model:

```bash
mastermind miner hooks analyze '<capture-episode-id>' \
  --revision '<episode-revision>' --provider native --timeout 60
```

For a custom executable, replace the provider with
`--processor /absolute/path/to/persona-processor` and repeat
`--processor-arg=VALUE` for arguments. Inspect retained text before sending it
to a provider. Native login, model binding, isolation, JSON schema and resource
limits are in the [processor contract](../reference/persona.md#hook-processor-contract).

### Run a managed worker

To opt into separate background requests for selected clients:

```bash
mastermind init --mining on --provider native --max-calls 64 --max-runtime 3600
mastermind miner status --json
mastermind miner stop
```

Repeated init keeps existing counters. `mastermind miner start` explicitly
renews a run. Automatic renewal, checkpoints and failure recovery are described
once in [managed workers](../reference/persona.md#managed-workers).

## Optional: prompt refinement

```bash
mastermind init --refiner on --provider native
mastermind miner hooks intake '<intake-id>'
```

Refinement uses an extra provider request, outside the miner budget. The
original prompt still reaches the agent. Inspect `intake` in `hooks show` for
the proposed text and route. To bind an admitted intake to a planned spec:

```bash
mastermind miner hooks bind-task '<intake-id>' --spec .mastermind/tasks/001-example/spec.md
mastermind run-task .mastermind/tasks/001-example/spec.md --pre-only
```

Binding records provenance without granting execution or approving scope.
Exact revisions, replacement rules and failure states are in
[prompt intake](../reference/persona.md#prompt-refinement-and-intake).

## Maintain or remove evidence

Capture automatically archives inactive payloads when the working journal is
under pressure. Archives retain their evidence revision and original gaps.
They grow until evidence is explicitly forgotten. See
[retention](../reference/persona.md#local-extraction-and-retention) for limits.

For a failed capture generation, inspect the issue before recovery:

```bash
mastermind miner hooks status --client codex --project-root .
mastermind miner hooks recover --client codex --project-root .
```

Recovery starts a new generation. Restart the client afterward. Missing events
remain missing, and changed evidence needs review again.

To stop future capture and mining, use `mastermind init --mining off`. Direct
hook removal is also available:

```bash
mastermind miner hooks setup --client codex --project-root . --remove --write
```

Removal revokes capture and retains local evidence. To remove an episode's raw
text, archived copies and dependent drafts:

```bash
mastermind miner hooks forget '<capture-episode-id>' --revision '<episode-revision>'
mastermind miner habit refresh
```

Transferred profile audit quotes, backups and data already sent to an external
processor are not erased by `forget`.
