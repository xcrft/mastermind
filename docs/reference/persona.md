# Persona storage, mining and review

Use this reference for `miner` commands, source admission, review transitions
and profile limits. Start with [client capture](../guides/persona-hooks.md) for
hook setup or [context composition](persona-context.md) for delivery to an agent.

## Storage and authority

| Data | Location | Use |
|---|---|---|
| Hook events, coverage, drafts and exposure records | `~/.mastermind/persona-events.db` | Local capture journal |
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

## Hook processor contract

| Entry point | Execution contract |
|---|---|
| [Native capture](../guides/persona-hooks.md) | Collects only |
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
| Coverage gap or known profile influence | Reject analysis |
| Inference boundary | No identity, psychology, sensitive traits, permissions or global habit from one task |
| `--provider claude` | Explicit provider request, may send episode text externally |
| Built-in isolation | Bare mode, no tools, MCP discovery, project settings or persistent sessions |
| Credentials | API/provider credentials, not subscription OAuth/keychain access |
| Unsupported flag | Fail without falling back to a normal client session |

| Capture or analysis resource | Bound |
|---|---|
| Native hook command | 3 seconds |
| Native JSON / retained text per event | 4 MiB / 16 KiB. Unavailable retained content marks each episode that uses the event incomplete; an unattributed loss or envelope over 4 MiB fences the session/capture until recovery |
| Journal / retained episodes | 64 MiB / 2,000 |
| Events / stored bytes per episode | 128 / 512 KiB |
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
| Contention, crash, unsupported input, redaction or missing lifecycle/tool event | Mark incomplete delivery and withhold evidence |
| Recovery | New generation, no reconstruction of lost events |
| `MMCG_INPUT_ORIGIN=controller` or miner recursion guard | Skip generated input before journal writes |
| Other profile exposure | Recognition limited to supported paths |

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
