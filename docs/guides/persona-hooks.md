# Mine working habits from client interactions

Mastermind can capture selected Claude Code or Codex interactions locally, extract candidate working habits with an explicitly selected processor, and publish reviewed habits through `mmcg_profile`. Capture, analysis, authorship attestation, and publication are separate steps.

The miner extracts observable technical approaches, workflow patterns, communication preferences, tool preferences, and review preferences. It keeps the situation, behavior, exceptions, and source citations. Schema 1 leaves rationale and outcome unknown: a successful tool call does not establish why a person made a choice or whether that choice caused the result.

## 1. Enable local capture for one project

Run from the project whose interactions you want to collect. Native installation and semantic subprocess execution currently require Unix.

```bash
# Preview the exact generated hook definitions without writing configuration.
mmcg miner hooks setup --client codex --project-root .

# Install those hooks and grant local capture for this client/project pair.
mmcg miner hooks setup --client codex --project-root . --write
mmcg miner hooks status --client codex --project-root .
```

Use `--client claude` for Claude Code. Setup preserves unrelated settings and hooks. It writes only the selected project's native configuration:

| Client | Configuration | Activation |
|---|---|---|
| Claude Code | `.claude/settings.local.json` | Reload configuration and inspect `/hooks`. Managed settings can disable hooks. |
| Codex | `.codex/hooks.json` | Trust the project layer, then review the exact definitions in `/hooks`. Setup does not grant native hook trust. |

Restart the client session after setup so capture includes `SessionStart`. Starting in the middle of a session leaves evidence incomplete. Hooks run a short local receiver; installation does not start an LLM, a background analyzer, or a cloud upload.

Capture is bound to the canonical project root and the selected client. Setup does not grant profile reading. Stop capture and remove its generated handlers with:

```bash
mmcg miner hooks setup --client codex --project-root . --remove --write
```

Removal revokes capture before changing the native configuration. Previously collected data remains available for local inspection; revocation makes dependent sources unavailable for profile publication.

## 2. Inspect an episode before analysis

```bash
mmcg miner hooks episodes --project-root . --limit 20
mmcg miner hooks episodes --project-root . --after <next_after>
mmcg miner hooks show <capture-episode-id>
```

Use full IDs and revisions from the JSON receipts. There are two different episode identifiers:

| Identifier | Meaning |
|---|---|
| Capture episode ID | A generated 64-character ID for a captured interaction. Used by `show`, `analyze`, and `forget`. |
| Task identity passed to `propose --episode` | A stable task or PR identifier chosen by the reviewer, such as `edge-ai-pr-123`. Reuse it across every session of that task. |

`show` also returns retained draft IDs and revisions under `drafts`, so you can reopen existing analysis without making another provider request.

An episode includes user-channel text, assistant/tool observations, relevant adjacent-turn context, and coverage state. `Stop` is an observed response boundary; it does not prove task completion. Tool output and assistant text provide context, never independent support for a personal habit.

`UserPromptSubmit` identifies a transport channel. It cannot prove that a person authored the text. Pasted material, automated prompts, delegated work, and source quotations must not become personal evidence.

## 3. Select a semantic processor explicitly

For the built-in Claude adapter:

```bash
mmcg miner hooks analyze <capture-episode-id> \
  --revision <episode-revision> --provider claude --timeout 60
```

This command sends the bounded episode to the selected provider. It requires a compatible `claude` executable and API/provider credentials. The adapter runs Claude in isolated `--bare` mode with tools, MCP discovery, project settings, session persistence, and browser integration disabled. Bare mode does not use subscription OAuth/keychain credentials. Unsupported client flags fail; the adapter does not retry as an ordinary interactive Claude session.

For a processor you control:

```bash
mmcg miner hooks analyze <capture-episode-id> \
  --revision <episode-revision> \
  --processor /absolute/path/to/persona-processor \
  --processor-arg=--model --processor-arg=local-model --timeout 60
```

The executable receives one JSON request on stdin, containing `schema`, `instructions`, `response_example`, and `episode`. It must return exactly one schema-1 JSON object on stdout with matching `episode_id`, `episode_revision`, and a `drafts` array. An empty array is valid when evidence is insufficient. Follow the supplied response example for draft fields and exact event citations; `rationale` and `outcome` must be `null`.

Arguments are passed directly without a shell. The selected executable is trusted local code with the caller's filesystem, environment, and network access; an absolute path is not a sandbox. Use `--processor-arg` for each argument. `--provider` and `--processor` are alternatives.

Analysis refuses incomplete or profile-influenced episodes. It verifies exact source spans, event origin, bounded fields, and matching revisions before retaining drafts. These checks establish provenance and structure; meaning and authorship still require review. A citation's existence alone does not prove the generated interpretation.

### Process a batch of captured episodes

Invoke the worker separately from the native hooks. It runs in the foreground and does not install a scheduler. A normal invocation processes one bounded batch; `--follow` keeps processing while this command is running.

```bash
mmcg miner hooks mine --project-root . --provider claude --limit 4

# Continue this pass only after a successful command with a next_after value.
mmcg miner hooks mine --project-root . --provider claude --limit 4 \
  --after <next_after>

# The same worker can use an explicitly selected local processor.
mmcg miner hooks mine --project-root . \
  --processor /absolute/path/to/persona-processor \
  --processor-arg=--model --processor-arg=local-model --limit 4

# Start once to analyze newly completed or revised episodes automatically.
mmcg miner hooks mine --project-root . --provider claude --follow
```

Each batch examines at most 32 episodes in ID order and attempts at most `--limit` eligible analyses, default 4 and maximum 16. The JSON result contains `results` with episode/revision/draft receipts, `skipped` with reasons, and `next_after` when the pass has more records. Review returned draft IDs with `hooks draft` before proposing them. Start each new manual pass without `--after` so earlier IDs whose revisions changed are considered again.

`--follow` drains successive batches and watches the local journal for new or changed episodes, including changes to earlier IDs. It checks for journal changes about every two seconds and makes a recovery pass every 30 seconds for expired worker leases. Unchanged completed revisions do not generate new provider requests. Only startup, new analysis results, errors, and shutdown are printed. `--after` cannot be combined with `--follow`. Ctrl-C or SIGTERM stops the worker, terminates its active processor group, and releases the unfinished lease for retry. Stopping local processing cannot retract a request already accepted by a remote provider. No worker is automatically restarted after exit.

A completion checkpoint binds the episode revision to the selected processor path/provider, argument digest, and extraction protocol. Empty draft results are also complete and are skipped on the next matching batch. Short database leases normally prevent simultaneous workers from requesting the same analysis; expired leases allow interrupted work to be retried. A previous attempt may already have reached the provider.

The worker stops at its first processor failure and exits nonzero. An episode superseded during analysis is skipped and can be reconsidered on the next pass. The failed episode remains retryable: rerun without advancing past it, or use `hooks analyze` for its exact current revision. Single-episode `analyze` explicitly re-evaluates an episode even when its batch checkpoint is complete. Use it after changing a processor or model behind an otherwise unchanged processor configuration.

## 4. Attest authorship and propose a candidate

```bash
mmcg miner hooks draft <draft-id>

# Run in the author's interactive terminal after inspecting the citations.
mmcg miner hooks propose <draft-id> --revision <draft-revision> \
  --episode edge-ai-pr-123 --attest-human
```

`--attest-human` confirms that the cited words are the person's own statements, not pasted, delegated, or generated text. The terminal requirement is a workflow check, not identity authentication. An agent must not make this assertion on the person's behalf merely because the hook used the user channel.

The stable task identity is also a reviewer assertion about independence. Repeated turns, sessions, retries, and forks of one task must not count as different tasks. Once a draft has an attested task identity, it cannot be relabeled to manufacture independent support.

Propose creates a project-scoped habit candidate in the existing review store. It does not activate the habit. A draft with unresolved counterevidence cannot be proposed. Schema-1 proposals retain an explicitly unknown outcome.

To add a separately inspected draft to an existing habit generation, use `--habit <habit-id>`. Its entire definition, scope, role, and workflow must match; similarity does not merge habits.

```bash
mmcg miner hooks propose <another-draft-id> --revision <draft-revision> \
  --episode edge-ai-pr-456 --attest-human --habit <habit-id>
mmcg miner habit show <habit-id>
mmcg miner habit observe <habit-id> --revision <review-revision>
```

The existing observation gate requires reviewed, current support from two distinct sessions and two task episodes, with no unresolved counterexamples or limitations. `observe` requires an interactive terminal and the exact current review revision. This hook path does not automatically promote one task into a global habit. See the [miner reference](../reference/mmcg.md#cli-usage) for rejection, source dismissal, replacement, and stronger cross-project requirements.

## 5. Deliver reviewed context to an LLM

The storage layers have different jobs:

| Layer | Purpose |
|---|---|
| `~/.mastermind/persona-events.db` | Local hook observations, coverage state, exposure receipts, and semantic drafts. |
| `~/.mastermind/style.db` | Global profile claims, source bindings, authorship receipts, and review history. |
| `~/.mastermind/style.md` | Static inspection snapshot generated from the profile. Editing it does not accept a claim. |
| `mmcg_profile` over MCP | Live, source-checked selection for the current project, changed paths/languages, role, workflow, and token budget. |

Grant profile access separately and configure the MCP server with the same client ID:

```bash
mmcg miner access grant . --client codex
# Set MMCG_PROFILE_CLIENT=codex in this project's mmcg MCP server environment.
```

The read grant covers the global profile, including aggregate observations across repositories. Selection narrows relevance; it is not source-level access control. `mmcg_profile` withholds unavailable or unreviewed claims. Clients must not fall back to `style.md` when live access is denied or evidence is unavailable.

Optional native context delivery uses an existing read grant:

```bash
mmcg miner hooks setup --client codex --project-root . \
  --profile-client codex --write
```

On an eligible prompt, the receiver can offer a reviewed profile packet as advisory context and record its revision and claim receipts. It does not grant permission to act or prove project facts.

Known profile exposure excludes that session's affected episodes from independent habit mining. This includes the miner's own injection and recognized native tool calls to `mmcg_profile` or reads of `style.md`. Detection is conservative and incomplete outside covered tool paths. A model repeating an injected preference cannot become new independent evidence for it.

## Revisions, recovery, and deletion

A later prompt can add correction context to an earlier closed episode. Its revision changes, invalidating earlier drafts and dependent source verification. Re-run `show`, analyze the current revision, inspect the new draft, and attest again using the same task identity. An unchanged hypothesis retains its draft ID, allowing its existing source binding to be repaired. Changed evidence requires a fresh habit review; it does not silently reuse the old observation pin.

The receiver creates a durable per-delivery capture marker before opening the journal and records a pending capture before reading stdin. Journal contention, a crash, missing tool result, unknown schema, oversized input, redacted content, or incomplete lifecycle can withhold dependent evidence. An unsuccessful delivery retains its marker instead of making the missing event appear successful. Inspect `status` and `show` rather than treating missing records as a clean episode.

```bash
mmcg miner hooks status --client codex --project-root .
mmcg miner hooks recover --client codex --project-root .
```

Use `recover` after addressing a capture failure and ensuring old receiver processes have stopped. It starts a new capture generation, invalidates earlier receipts, and clears the selected client/project's interrupted capture markers and pending fence. Restart the client session afterward. Recovery does not reconstruct or certify the missing event.

To remove captured raw text:

```bash
mmcg miner hooks show <capture-episode-id>
mmcg miner hooks forget <capture-episode-id> --revision <episode-revision>
mmcg miner habit refresh
```

`forget` removes the selected episode, its raw event text copied into adjacent context, and the session's derived drafts. It marks the session incomplete and makes dependent profile sources unavailable. It does **not** erase existing quotes and audit records already transferred into `style.db`, external backups, or data previously sent to a processor. The refresh regenerates the static profile snapshot; live verified reads already withhold unavailable sources.

## Bounds and coverage

| Boundary | Limit |
|---|---|
| Native hook command | 3 seconds |
| Native JSON input / retained text per event | 256 KiB / 16 KiB |
| Journal / retained episodes | 64 MiB / 2,000 |
| Events / stored bytes per episode | 128 / 512 KiB |
| Semantic request / stdout / stderr | 512 KiB / 64 KiB / 16 KiB |
| CLI processor timeout | 60 seconds by default; 1–120 seconds |
| Batch worker | At most 32 inspected episodes; 4 processed by default, maximum 16 |
| Drafts / supports / contradictions | 8 drafts; up to 8 supports and 8 contradictions each |

Secret detection is heuristic. Detected secret-like content is withheld and marks coverage incomplete; undetected secrets may still reach local storage or an explicitly selected processor. Inspect episodes before sending them outside the machine.

The native adapters have coverage gaps. Codex omits some hosted/specialized tool paths and new `PreToolUse` checks for `write_stdin`; its main-thread interruption/end hooks do not cover every subagent lifecycle. Claude's `Stop` does not fire on user interruption. Hooks can be disabled, unsupported, or untrusted. An episode without recorded gaps is only evidence about the events the adapter received.

These hooks collect evidence. They are not a complete action-permission system, a proof of human identity, or a guarantee against hallucinations. Local tests exercise isolated client configuration and real subprocess fixtures; a live authenticated Claude provider run is not part of those default tests. For native client behavior, consult the [Codex hooks documentation](https://learn.chatgpt.com/docs/hooks) and [Claude Code hooks reference](https://code.claude.com/docs/en/hooks).
