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
| Installed receiver | Short local capture only, no model or analysis worker |

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
| Invocation | Explicit provider request using `--bare` |
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
| Incomplete or known profile-influenced evidence | Analysis rejected |
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
| Mastermind injection or recognized MCP/profile-file read | Excludes affected episodes from independent habit mining |
| Repetition of injected advice | Not independent evidence |
| Other delivery paths | Detection is limited to supported paths |

## Maintain or remove evidence

| Evidence change | Required action |
|---|---|
| Later prompt changes an earlier episode revision | Inspect and analyze the new revision |
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
