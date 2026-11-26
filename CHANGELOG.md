# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Breaking

- `style.db` is now format 2. The first writable open converts format 1
  replacement records: a replaced preference or habit becomes `rejected`, and a
  habit created by renewal goes back to `candidate` for a new review. Each
  change gets a review event. Mastermind 4.0.x refuses a format 2 store.
- The automatic profile refresh after a task, the task-boundary drain and
  every command that writes the profile convert a format 1 store. A read-only
  command, such as `habit show` or a profile read, does not convert it. While
  the store still holds replacement records, it fails and says that the next
  automatic refresh or any writing command converts them.
- Task records refuse a repeated JSON key or a floating-point number:
  `state.json`, verification, invocation and review invocation receipts,
  preflight approvals, `review-task submit` reports and the guarded execution
  decision log. Records written by 4.0.x still read.
- Digests, receipt IDs, invocation IDs and review pins in those records must be
  lowercase hex.

### Added

- `mastermind doctor` has a `permission rules` check. It reads the Claude Code
  permission lists and modes, Codex `.rules` files, and Codex `config.toml` and
  profile files, and warns about settings that let an agent run code it
  chooses without asking: any command, interpreters and shells, commands that
  run other commands, package managers, task runners and installs, `git` and
  `gh` without a fixed subcommand, network fetches without a fixed host,
  `bypassPermissions`, and Codex running unsandboxed without approval. A rule
  that a deny, ask or stricter rule covers does not warn. It never edits the
  files and never prints rule text.
- One untrusted-content paragraph in `agents/blocks/untrusted-content.md`.
  Ten subagents and 28 skills that read repository, pull request, report,
  transcript or tool content carry it verbatim. It names the person or the
  agent that started the role as the only source of instructions, and keeps
  the approved spec and declared checks part of the request. `just validate`
  fails when a covered file lacks it, has it twice, has a changed copy or
  holds it inside a code fence, an HTML comment or another paragraph.

- Five skills that are safe to point at an untrusted pull request:
  `mastermind-diff-review`, `mastermind-docs-authoring` and
  `mastermind-verify-change` in every profile, and
  `mastermind-dependency-upgrade` and `mastermind-scanner-alert-response` in
  the `security` and `full` profiles. They read repository instructions from
  the base branch, treat everything in the change as data, never run the
  change's scripts with credentials, and never fetch a secret's value. The
  `core` profile now installs 23 skills.

### Changed

- The covered skills and subagents use the shared untrusted-content paragraph
  in place of their own shorter wording, and each has a new patch version.
  The critic opens with its own short data-not-instructions rule instead. The
  FIND precondition notes and the legacy `VERIFY` forms moved from the planner
  to its spec template.

- An index written with a newer extractor or concept contract is refused by
  every open, the same as a newer schema version, and is never rebuilt.
- A symlink or other non-regular source no longer makes the index stale for MCP
  queries, `status` and `doctor`. It is still listed in `unindexable_sources`,
  and `index` and `ci` still exit 1 for it. The `index` error now lists such
  sources apart from retryable failures, which keep the index stale.
- When `index` skips more submodules than it lists, it prints the total.
- `enrich` takes the index writer lock for fact and SCIP imports.
- Project history, workflow status and history review use one rule for task
  folders, so a loose `*.md` file in `.mastermind/tasks` is never a task.
- `mastermind update` checks every selected client before it writes anything
  and lists each edited, linked or unowned workflow entry with a `Next:` step.
  `--move-aside` moves them to `<client root>/.mastermind-preserved/<time>/`
  and installs the bundled copies.
- Updating from 4.0.x: the first `mastermind update` still runs the 4.0.x
  updater, which has no `--move-aside` and stops at the first edited, linked or
  unowned entry. Move those entries out of `~/.claude/skills`, `~/.claude/agents` and
  `~/.codex/skills`, then run `mastermind update` again.

### Fixed

- The frontend audit checks props contracts with `mmcg_change_impact`, which
  includes uncommitted work. `mmcg_symbols_changed_since` only compares
  commits, so it missed a changed component before the commit step.
- The runtime, component and security research skills pass `max_depth` to
  `mmcg_impact`; it has no `depth` argument.
- The comment auditor no longer counts `could_not_verify` as a finding, so an
  unjudged comment cannot turn a clean result into `findings`.
- The comment, test and frontend audits have an `incomplete` verdict. A review
  that stopped early, for example with no baseline, no longer reads as
  `clean`.
- The test and frontend auditors no longer re-index the repository. A query
  refreshes a stale managed index once; if it is still stale they report
  `could_not_verify`.
- The router says the feedback collector writes to the private review inbox
  only when asked; it never edits the profile.
- The auditor's report tail uses a repository-relative spec path, and the
  report contract lists the auditor's full set of fields.
- The planner links its spec template, defect labels, design review packet and
  report schema references.
- `miner profile --deep` help says it saves a separate candidate file for
  review instead of replacing part of the profile.
- A 4.0 task is no longer refused because an `executor-report.md` or
  `audit.md` sits beside its spec. Only a task started before 4.0 gets the
  old-name refusal, and a spec named `audit.md` never counts as its own old
  audit.
- `review-task` with a missing spec or a `_draft` folder no longer leaves a
  controller lock or run-state directory behind.
- Closing the terminal stops a running verification check, and its receipt is
  `interrupted`.
- Terminal control characters from the executor report are escaped in its
  parse errors.
- `audit verify` and workflow status fail at once on a FIFO in place of a file,
  instead of hanging.
- An audit bundle `snapshot_changed` error names its reason, such as
  `snapshot_changed: worktree_not_clean`.
- `run-task` says why it stopped when the task state disappears or cannot be
  read after a review.
- `serve` no longer fails with `database is locked` when it opens the index
  right after an incremental `index`.
- `watch` waits for another index writer with full refreshes that never use up
  its three retries. It checks a removed path again under the writer lock and
  parses under the lock, so it never commits content older than another
  writer's.
- Outside Git, a directory the walk may not read is skipped when the index
  holds no source under it. Before, it failed the run.
- `mmcg_context` and `mastermind context preview` return
  `context_index_unavailable` for an index that exists but does not open, for
  example one a newer mmcg wrote. Before, they built the packet without it.
- `mmcg_unreferenced` hides only names that start with a literal `test_` in
  test files. `_` was a wildcard, so names such as `testing` were hidden too.
- `serve` prints a warning when it cannot remove its index snapshot on exit.
  On Windows a snapshot that SQLite still holds open stays behind.
- The hook journal no longer fills up with rows from ended sessions. Archiving
  drops their event receipts and shrinks intake and task-mining rows to a
  summary that `hooks show` still reads.
- A full hook journal records the `journal_full` gap, `hooks status` lists the
  steps to free space, and `hooks forget` works on a full journal.
- One hook delivery spends at most 2.5 s in total waiting for the journal
  lock, so a busy journal still leaves time to record the gap.
- `hooks recover --project-root PATH` works after the project folder was
  deleted or moved, and `doctor` prints that command for each such project.
- `doctor` shows why hook readiness is unavailable, such as a journal from a
  newer version.
- `.DS_Store`, `__pycache__` and `*.pyc` files in installed workflow folders no
  longer count as edits.
- `init` and `status` print the reason and the `Next:` steps when a workflow
  update fails, and `doctor --workflow` reports a linked or misplaced client
  folder as a layout error with a recovery step.

### Removed

- The `mmcg_scratchpad_append` and `mmcg_scratchpad_read` MCP tools. An
  existing index keeps the old table, and nothing reads it.
- `miner feedback supersede`, `miner habit supersede` and `miner habit renew`.
  To replace a claim, reject it and review a new one.
- The generation column in `habit list`, and the relation and generation lines
  in `habit show` and `feedback show`.
- `docs/README.md`, `docs/architecture.md`, the Claude Code and Codex
  integration pages and `docs/reference/persona-context.md`. Their content now
  lives in the README, getting started, effort routing and the persona
  reference. Eval narratives that only retold a retained JSON record are gone,
  and the scorecard links the JSON directly.

### Development

- `just bootstrap` and `just sync-templates` no longer run `just check` and
  `just validate` by accident: their closing hint quoted the command in
  backticks, which the shell executed.
- Every workflow installs Rust from `rust-toolchain.toml`, and the validator
  checks that the Docker Action builds with the same channel.
- `just release-tags` dispatches the npm and crates.io publish dry runs and
  waits for them and push CI. The publish verify job no longer repeats the
  tests in release mode.
- New `just release-check` and `just test-linux`. `just index`, `just outline`
  and `just recent` are gone.
- Dependencies: notify 9, cargo-deny-action 2.1.4 with `unmaintained = "all"`,
  PyYAML 6.0.4 and a lockfile refresh.

## [4.0.1] - 2026-11-26

### Fixed

- Mining and native runs with Claude Code work again. The `claude` executable
  is now hashed as a stream with a 2 GiB limit. It was read whole into memory
  with a 256 MiB limit, and Claude Code ships as one binary of more than
  350 MiB.
- Task mining with Claude no longer gives up when it reads the session
  transcript while Claude is still writing it. A transcript that changes during
  the read, reads slowly, or ends inside a character now counts as not ready,
  and the finalizer keeps waiting for the response. Before, the episode stopped
  with `finalizer_failed` until a later retry.
- A relative or empty `PATH` entry, such as the `~/.dotnet/tools` entry the
  .NET installer adds, no longer breaks `init`, setup or verification commands.
  Executable lookup skips those entries, since what they find depends on the
  working directory. `mastermind doctor` names them as a warning and explains
  that `~` is not expanded inside `PATH`.
- `mastermind doctor` accepts the `.mastermind/.gitignore` that `init` writes,
  so the project's own `.gitignore` no longer needs a `.mastermind/` line.
- `mastermind doctor` no longer fails a repository that has no source in a
  supported language, such as one with only scripts, YAML or Terraform. The
  empty index is reported as OK with the reason.
- `mastermind init` replaces an unmarked `## Mastermind workflow` section left
  by a 3.x `init` with the managed block, even when the section came from an
  older template or was edited. The previous file is kept as
  `CLAUDE.md.mastermind-backup` (or `AGENTS.md.mastermind-backup`). Before,
  `init` left the file unchanged and only recorded a warning in `--json`.
- `mastermind init` prints the project step's warnings in its normal output.
- `mastermind doctor` hints name commands that run as written:
  `miner hooks status` and `miner hooks recover` get `--client <client>`, and
  missing MCP config or ignore rules point at `mastermind init`.

## [4.0.0] - 2026-11-25

### Security

- `pulldown-cmark` is updated to 0.13.6 for RUSTSEC-2026-0433. Mastermind
  does not enable wikilinks, so the quadratic parse was not reachable.
- `rusqlite` is updated to 0.43.0 and the bundled SQLite to 3.54.1
  (`libsqlite3-sys` 0.40.0) for RUSTSEC-2026-0459. Older SQLite could return
  uninitialized memory when a database and its `-wal` file had different page
  sizes. `mmcg` opens the index with its `-wal` from the checkout, so a
  repository that ships a crafted pair could reach it.

### Added

- `mastermind init --worktree-max-total-bytes N` and
  `--snapshot-max-total-bytes N` (1 to 1,073,741,824 bytes) set the two total
  byte budgets that were fixed at 32 MiB. The worktree budget bounds the prior
  local files a task's initial worktree may hold; its first pre-flight records
  it in the manifest, so it does not change for an open task. The snapshot
  budget bounds the files the history, audit and strict workflow snapshots read,
  including the task's prior files, so it should cover the worktree budget plus
  the task's own changes;
  every pre-flight pins it in `state.json` and its approval record, and a new
  value reaches an open task through `run-task <spec> --pre-only`. A budget that
  runs out names its setting, limit, counted bytes, the next file and the flag,
  where the history snapshot used to report a small file as over a "limit" that
  was the bytes left. Defaults stay 32 MiB. A release that predates the setting
  rejects task state written with these fields and a `setup.json` that holds
  the keys.

### Changed

- **Breaking:** `mmcg index` exits 1 when a source file fails to index,
  including a symlink or other non-regular file, after it prints the failed
  paths. Scripts that treated a finished run as success need to handle the new
  exit code.
- **Breaking:** `facts adapt` requires `--report-revision OID`, the full
  commit the report was produced from, such as `$GITHUB_SHA`. Existing adapt
  scripts fail until they pass it. The adapter used to stamp the indexed HEAD
  on a report of unknown revision. The OID must equal the indexed HEAD, and a
  SARIF run that declares `versionControlProvenance` for this repository must
  name the same revision (`artifact_revision_mismatch`).
- **Breaking:** Setup accepts a project-local npm launcher only when it is an
  absolute path ending in `bin/mastermind.js` run by an absolute `node`. Other
  values fail with `npm_launcher_path_invalid`.
- **Breaking:** `state.json` records `format_version`. State without it is
  read as format 1. State with a newer format is refused with its version and
  a request to use the newer binary, where it used to look damaged and the
  advice was to restore it. A release before this one rejects all state this
  release writes, so finish or close open tasks before going back.
- **Breaking:** `mastermind ci` fails and names the files when a source file
  fails to index. Before, it audited the partial graph and could pass.
- **Breaking:** Legacy flat and other non-`spec.md` specs get their own
  executor report and audit, named after the spec, such as
  `050-a.executor-report.md`. Before, two such specs in one folder read and
  overwrote each other's report and audit, and a finished task could fail its
  audit with `task_mismatch`. When a task of such a spec already has state and
  an old `executor-report.md` or `audit.md` sits beside the spec, `run-task`
  stops before it changes the state and prints the `mv` command for each file,
  or `rm` when the new name already exists. A `.gitignore` that lists
  `executor-report.md` or `audit.md` for such specs needs the new names too.
- **Breaking:** Reapproval of a task approved before this change fails with
  `preflight_approval_schema_outdated`. Start a new task. The older parser
  could have dropped preconditions that the edited files can no longer prove.
- **Breaking:** A task started before this change fails preflight and
  postflight with `initial_worktree_schema_outdated`. Its initial worktree
  manifest missed the flagged edits, which would then be charged to the
  executor. Start a new task. Its state stays readable.
- **Breaking:** `audit-spec` and `ci` refuse a change list over 10,000 files
  or with a non-UTF-8 path (`change_inventory_incomplete`). They used to
  report Held for the files they could list.
- **Breaking:** `verify-spec` no longer evaluates index checks against an
  index that no longer matches the files. It reports `not_evaluated`, and
  `--require-index` fails. Standalone `audit-spec` refuses such an index
  instead of holding.
- **Breaking:** `mastermind ci` fails when `--changed-only`,
  `--require-executor-report` or `--bundle-dir` is set and no task contract
  exists. It used to pass when `.mastermind/tasks/` was missing.
- **Breaking:** The hidden compatibility flags are removed. `mastermind init`
  no longer accepts `--no-claude`, since drafting is already off unless
  `--draft-with claude` is set. `mastermind setup` no longer accepts
  `--dry-run` (run it without `--write`), `--write-mcp` (use `--write`) or
  `--project PATH` (use `--scope project --root PATH`).
- `mmcg_dependency_cycles` resolves an import only within its language family.
  JavaScript, TypeScript, TSX and Vue form one family. A name shared by a
  Python and a Rust file no longer closes a cycle between them.
- Index writes take an exclusive writer lock file next to the database, such as
  `.mastermind/mmcg.db-writer.lock`. A refresh that waits too long for it
  returns `index_busy` with `retryable: true` instead of
  `work_limit_exceeded`, and `mmcg index` fails with `the index is busy`.
- An index written by a newer mmcg is refused with an error that names both
  schema versions. Before, the older binary dropped and rebuilt its graph.
- Dependencies are on their latest releases: `rusqlite` 0.43,
  `tree-sitter-c-sharp` 0.24 and compatible updates of the other crates. The
  C# grammar reads C# 14 `extension` blocks, and their members are indexed as
  methods of the enclosing class. Some C# code parses differently, such as
  `(x) && y`, which is now a logical AND instead of a cast. The audit Docker
  Action builds with Rust 1.100.0, and the PR review workflow pins the current
  `dtolnay/rust-toolchain` commit.
- `MMCG_TASK_FINALIZER_WAIT_MS` sets how long the persona task finalizer waits
  after Claude's Stop hook for the transcript to name the model. The default
  stays 4 seconds. A waiting finalizer renews its 10 second lease, so a longer
  wait is not mistaken for a crashed finalizer. The finalizer tests set the
  wait themselves, so they no longer fail when a loaded machine runs them
  slower than that window.
- `init` keeps the workflow instructions in a managed block. It appends the
  block to an existing `CLAUDE.md` instead of skipping the file, refreshes an
  older block, and writes the same block to `AGENTS.md` for Codex.
  Text outside the block keeps its bytes, and the block follows CRLF files.
  `--workflow off` writes no block.
  `doctor` compares the block in each saved client's file with the current
  template.
- The `core` profile now includes `mastermind-runtime-research`,
  `mastermind-component-research`, `mastermind-frontend-audit` and
  `mastermind-browser-verification`. The workflow router names them, so a
  default install used to point at skills it did not have. `frontend` adds
  `mastermind-design-intake`.
- The planner names a Strict review that works without subagents, such as in
  Codex: `mastermind-critical-review` in a fresh child agent.
- `persona-events.db` and `style.db` record their format version in SQLite
  `user_version`. A store written by a newer mmcg is refused with an error that
  names both versions. The `style.db` feedback re-key runs once per format
  version instead of on every open.
- A hook delivery stops archiving after 250 ms once it has archived one
  episode, so it stays within its 3 s timeout. The journal waits up to 2 s for
  the writer lock instead of 0.5 s. `hooks archive` has no time bound.
- The PR audit example pins `xcrft/mastermind` to a commit instead of
  `uses: ./`, which only worked inside this repository and let a pull request
  change the audit code. The publication comment shows the envelope verdicts
  and marks only `held` with ✅.
- The `.mastermind/.gitignore` that `init` writes carries the lines that commit
  task contracts and executor reports, commented out. The GitHub audit Action
  needs them, and the old `!path/to/file` advice did not work.
- A relative path the runtime treats as repository-relative now always names a
  file from the root, whatever the working directory. Before, a command run in
  a subdirectory read `sub/.mastermind/...`, so `miner hooks bind-task` failed
  there even with absolute arguments and the hooks lost the task binding.
  Command-line paths that name files from the working directory, `--root`
  among them, are made absolute first, so `workflow audit` prints the absolute
  root.
- Entries under `.mastermind/tasks` that start with `_` or `.` are never tasks.
  `ci` no longer audits `_draft/spec.md`, which `status` did not show, and
  `run-task` refuses such a spec instead of creating state that `status` and
  `next` never list.
- `run-task` refuses a spec path that names the file differently from disk,
  such as `SPEC.md` for `spec.md` on macOS or Windows, and prints the path to
  use. Before, the second spelling got its own state, lock, iteration budget
  and artifacts for the same task. Refusing keeps one name per task without
  rewriting the path the user gave: the on-disk name comes from the same
  `canonicalize` that macOS and Windows use to report the stored case.
- `setup` keeps the key order of the client config files it edits, such as
  `~/.claude.json`. It used to rewrite them with the keys sorted.
- A stored semantic review record with a repeated JSON key at any depth fails
  with `semantic_review_record_invalid`.

### Fixed

- `run-task` with a spec path that does not exist fails before it takes the
  controller lock. Before, it left a lock file under `.mastermind/run-state/`.
- `mmcg serve` starts while `mmcg index --force` runs. Opening a current index
  no longer writes its schema, and serve no longer copies a snapshot just to
  read the index root. Before, most starts during a rebuild failed with
  `index changed while creating a read-only snapshot`.
- Two index runs no longer interleave their writes. The second waits for the
  writer lock.
- A source that gains a NUL byte after its first 8 KiB leaves the index.
  `mmcg watch` also drops a source that becomes binary or grows over 5 MiB.
  Before, its old symbols stayed and the index looked fresh.
- A symlinked source no longer keeps the extractor and concept contracts dirty
  and no longer stops `mmcg watch` after three retries.
- A source that fails to read keeps its indexed rows. Before, one transient
  read error purged its symbols.
- Outside Git, a directory the walk cannot read fails the index. Before, its
  sources were purged as if deleted.
- Git submodules are counted as skipped submodules in `mmcg index` and
  `mmcg_status`, instead of as unsupported paths.
- `mmcg watch` ignores renames under skipped directories such as `target` and
  `node_modules`. Before, each one ran a full refresh.
- `mmcg watch` purges a removed path only when it is still gone after the
  debounce, so an editor's atomic save keeps the file's rows.
- A schema migration runs in one transaction and fails the open when a column
  cannot be added. Before, failed `ALTER TABLE` statements were ignored and an
  index without the column passed as current.
- A failed MCP refresh that finished its run reports `unindexable_sources`
  instead of `index_rebuild_required` when it used up the budget.
- `mmcg serve` removes its private index snapshots when the watchdog exits the
  process.
- The context packet retries a read-only index snapshot that a concurrent write
  invalidated, then reports `context_index_snapshot_unavailable`. Before, it
  was built without the index and delivery failed as changed sources.
- A long turn no longer stops capture. Once an episode record reaches 320 KiB,
  text of later events is dropped with an `episode_size_limit` gap, and more
  than 128 open tool calls record `episode_tool_limit`.
- `hooks recover` tolerates markers that other deliveries clear during its
  listing. When it cannot remove every marker, it keeps the old generation and
  stops capture with a `capture_recovery_incomplete` gap.
- `hooks status` and doctor report the capture marker count and the oldest
  marker age. A marker with no journal gap gets its own next action that names
  `mastermind miner hooks recover`. Doctor's `capture fence markers` check
  counts markers in every project and the prefixes with no known grant.
- An unexpected entry such as `.DS_Store` in `~/.mastermind/.persona-capture`
  is named before a marker is created. Markers no longer pile up, recovery
  keeps the generation, and status no longer reports the journal as
  unavailable.
- Another session's delivery in flight no longer degrades prompt intake or
  skips a task-mining offer. The hook waits up to 1.5 s for it, and fails
  closed at once when the other delivery's marker is older than any hook can
  run.
- A `pending` intake whose processor timeout plus 3 s has passed reads as
  `degraded` with reason `processor_interrupted`.
- Doctor accepts the `node bin/mastermind.js serve` registration that setup
  writes for a project-local npm install. Setup and doctor share one rule.
- When native setup has removed the old registration and the new add fails,
  setup adds the old entry back and reports `native_add_failed_restored` or
  `native_add_failed_not_restored`.
- The npm wrapper forwards SIGTERM, SIGINT and SIGHUP to `mmcg` and exits with
  its status, so stopping an MCP server no longer leaves `mmcg` running.
- Persona CLI tests clear the inherited environment, so an exported
  `MMCG_INPUT_ORIGIN` or `CLAUDE_CONFIG_DIR` no longer breaks them.
- Prompt intake no longer degrades because another project or client finished
  a capture at the same moment. The capture-pending check treats a marker
  cleared during its read as absent and rereads a listing that changed, instead
  of reporting the project as pending.
- Post-flight no longer asks to acknowledge removals that are not API
  removals. A function declared inside a function or method body in Python,
  Rust or C# is local, and a file moved with its bytes unchanged to indexed
  source keeps its declarations, so a pure `git mv` of a component with local
  helpers passes without `removed_symbols`. Each destination covers one deleted
  file. A move with content changes, or to a path the index skips such as
  `api.py.orig`, still counts the old path's declarations as removed. Nested
  declarations that callers can reach still need an acknowledgement: methods of
  an object literal or class expression a JS or TS function returns, nested PHP
  functions and members of a type declared in a function.
- The observed verification snapshot now reads files up to its 512 MiB pass
  budget, so a tracked file over 128 MiB no longer stops pre-flight. When the
  budget runs out, the error names the file, its size and the bytes left. A
  read failure also names the file. An uncommitted file over 5 MiB still stops
  pre-flight, and the error now says how to clear it.
- An index that lacks a table the query tools read now counts as outdated, so
  `mmcg_docs` and `mmcg_project_profile` answer `schema_incompatible` for an
  old read-only index instead of an internal error.
- `doctor` looks for the Claude MCP registration where `setup` writes it, under
  `CLAUDE_CONFIG_DIR` or an existing `.config.json`. Before, it reported
  `mmcg_registration_missing` as an error for such installs.
- An indexed file that becomes ignored now counts as stale, so MCP refresh drops
  its symbols. Before, `mmcg_status` reported fresh and search kept serving it.
- The 100,000-file auto refresh cap counts only files in a supported language.
  Before, a repository with many data or text files hit `refresh_limit_exceeded`
  on every MCP call, and `mastermind index .` did not clear it.
- `run-task`, `review-task`, `verification run`, `acceptance status`,
  `verify-spec` and `audit-spec` accept an absolute spec path that reaches the
  repository through a link, such as `$PWD` in a linked directory or `/var` on
  macOS, or through `..`, as `../spec.md` does in a subdirectory. Before,
  `audit-spec --bundle` refused such a spec and a failed audit dropped its
  lesson candidate without an error. `audit-spec --executor-report` resolves
  its path the same way as the spec.
- `audit-spec --json` reports a lesson candidate it could not write on stderr.
  It used to drop the error.
- `init` counts a legacy flat spec once. Its own `050-a.audit.md` and
  `050-a.executor-report.md` were counted as more specs, and the migration
  example could name one of them.
- Error messages that name a file from Git, such as a snapshot or initial
  worktree file over its limit, escape terminal control characters in the name.
- `status` names the real report or audit file of a failed post-flight, such
  as `a.executor-report.md`, instead of `executor-report.md`.
- The executor skill, its agent and the report contract tell the executor to
  write the report at the path `run-task --pre-only` prints. They named
  `<task>/executor-report.md`, which post-flight does not read for a spec with
  another name.
- `miner hooks show` no longer marks a fresh analysis as not current when the
  task finalizer binds the model and records the analysis while `show` reads.
  The episode revision and the analysis receipts are now read together.
- `facts adapt` no longer rejects a whole report as "not all mapped" because
  of entries that carry no file fact, such as an OTLP span without a code path
  or a coverage line repeated across methods. Entries without a path or outside
  the index are named with counts (`report_entry_without_path`,
  `report_entry_outside_index`).
- `facts adapt` turns control characters in titles, messages and labels into
  spaces and cuts text over the manifest limits with `…`. A multi-line SARIF
  message or a JUnit failure trace used to fail the import.
- `facts adapt` refuses a cut report: JUnit or Cobertura XML without its
  closing elements, an LCOV record without `end_of_record`, and a report with
  no facts. Only an empty SARIF scan clears a dataset. Before, a truncated CI
  upload could replace a dataset with part of it or with nothing.
- A JUnit suite that lists fewer cases than its `tests` total, or declares
  failures without a failed case, is partial in Lens and refused by the
  adapter (`junit_totals_mismatch`).
- `query facts` and `mmcg_facts` return error annotations before warnings and
  notes. When `--top` cuts a source, that source is `partial` with a
  `normalized_fact_limit` diagnostic, instead of `loaded`.
- Lens marks a fact source partial when its findings are dropped at the
  per-file or global cap, and review export no longer attests such a source as
  producer-attested.
- Persona capture hooks no longer fail when parallel tool calls deliver
  events at once. Another hook's commit changes SQLite's rollback journal while
  a hook opens the journal, and the open refused it with `file identity changed
  during read`. Claude Code showed this as a hook that gave no usable answer,
  and each failure left a pending capture marker until `miner hooks recover`.
- `verify-spec` and `audit-spec` refuse spec lines they cannot read instead of
  dropping them. A frontmatter block after a BOM or blank line, or with a
  trailing space, fails as `frontmatter_not_at_start`. A snapshot line that
  states a caller count or signature the parser cannot read, such as `~40
  callers`, or states claims for several symbols, fails as `unparsed_line`, and
  so does an upper-case FIND with text after its colon or inline code, plain
  words above its fence, an empty fence or no fence, an empty upper-case
  `VERIFY:`, a `VERIFY` line with several commands, and a snapshot heading that
  is not a top-level `##` section. Snapshot prose without a count or signature
  stays a note. Every top-level `##` heading that starts with `pre edit` and
  names a snapshot is read. FIND, VERIFY and File markers are read inside list
  items, and a `**File**:` marker no longer leaves FIND checked against the
  previous file. A `- [ ] VERIFY: commands are runnable` checklist entry is no
  longer a declared command.
- A heading indented past a list item's content no longer splits spec sections
  or resets the FIND target.
- The audit change list now includes edits that `assume-unchanged` or
  `skip-worktree` hide from `git diff`. Git diffs those files again on a copy
  of the index with both flags cleared, so its own attribute, mode, symlink
  and removal rules apply. In a sparse checkout, an absent `skip-worktree`
  file is not read as a deletion. Standalone `audit-spec`, `ci` and the
  task history binding use the same list.
- Git reads for audits and inventories no longer run repository hooks. Git
  runs `post-index-change` on any index write, including the refresh `diff`
  may do.
- CLI `query`, `map`, `repo-map` and `impact` warn on stderr when the index is
  not current. MCP tools use the same freshness rule.
- `mmcg index`, automatic refresh and the freshness check share one source
  inventory: Git inside a worktree, a walk outside it or when `git` is not on
  `PATH`. Before, `.ignore` files,
  parent `.gitignore` files and nested repositories made the manual index
  differ, so it never became current. `.ignore` files are no longer read.
- A directory outside Git can now be checked for freshness and refreshed by MCP
  tools. It used to report `index_stale` on every structural query.
- A source symlink or other non-regular source file is now named. `mmcg index`
  prints it as `failed source:`, `index_stale` lists it in
  `unindexable_sources`, and `run-task` names it when its refresh fails. Before,
  the index stayed stale with only a failure count.
- MCP tools return `index_rebuild_required` instead of `work_limit_exceeded`
  when a forced full rebuild does not fit the request budget.
- `serve` with an external `--index` returns `index_replaced` after the file
  is rebuilt, instead of a `snapshot_changed` that retrying never cleared.
- `verify-spec`, `audit-spec` and `run-task` read `## Pre-edit Snapshot`, the
  heading the spec templates write. The section was ignored before, so symbol,
  caller and signature checks did not run on it. Existing specs with a filled
  snapshot now get these checks, and an unfilled `<symbol>` line is an error.
- Git commands from Mastermind no longer run the `core.fsmonitor` program from
  a repository's config.
- `team map` keeps a lock valid after a member starts `mmcg serve`. Indexing
  leaves the path classification current, and an empty WAL no longer changes
  the index digest. Locks made while an empty WAL existed need a new `team lock`.
- Persona capture no longer fills its journal with tool receipts. Archiving now
  drops them for ended sessions, and `pipeline.retention` reports `database_bytes`.
- A failed journal write records one `capture_delivery_failed` gap and stops
  capture for that client and project until `miner hooks recover`. It used to
  leave a marker for every later event.
- The 1,024 capture marker limit now applies per client and project, so one
  failing project no longer stops capture everywhere.
- `mastermind uninstall --force` stops each client's worker, removes its hooks
  and capture grant and revokes its profile access before it removes
  `.mastermind/`. Prompts were still captured after uninstall before.
- `mastermind install` refuses to replace an edited Mastermind skill or agent
  and one it does not own. It used to replace them and delete the backup.

## [3.4.2] - 2026-10-26

### Added

- `mastermind init --exec-max-turns N`, `--exec-timeout N` and
  `--review-timeout N` store the native executor turns, executor timeout and
  reviewer timeout as project defaults in `.mastermind/setup.json`, like
  `--review-max-turns`. `run-task --exec`, `review-task run` and
  `run-task --auto-review` use them when the command line passes no flag; an
  explicit flag wins, and without a setting the limits stay 40 turns, 1800
  seconds and 600 seconds.

### Fixed

- `review-task submit` names the field and rule that reject a report, for
  example `criterion:<id>: semantic_review_evidence_unknown` or
  `invalid field or value at line L column C`, without quoting report text.
  `review-task prepare` lists the allowed statuses, reviewer kinds and history
  decisions. Accepted and rejected reports are unchanged.
- Refresh repository-owned Windows indexes through default CLI and MCP queries,
  including concept queries. External custom indexes remain read-only.
- Subscribe before the initial watcher scan, retain failed refresh work and retry
  without another file edit. Queue overflow and exhausted retries are reported.
- Reject unsupported native execution and review on Windows before changing task
  approval, source pins or iteration.
- Report optional Git profile refresh failures in background worker status and
  onboarding output while preserving otherwise successful project setup.

## [3.4.1] - 2026-10-22

### Fixed

- Declare file removals with `scope: delete`, including tests and re-export
  barrels without indexed declarations. Repeated preflight accepts proven
  deletions against the original task inputs. API removals and required docs
  remain checked before Held and semantic review.
- Retry task-mining finalization after missing transcripts, interrupted workers
  or failed Git refreshes without overlapping finalizers.
- Share query freshness checks across CLI and MCP, retain the original deadline
  and reject changed review source bindings.
- Clean up owned deep-capture process groups on Unix and report failed Claude
  registration removal instead of claiming successful uninstall.

## [3.4.0] - 2026-10-21

### Added

- Source-bound concept review through CLI and MCP, with operation witnesses,
  explicit request constraints and separate lexical candidates.
- Explicit `scope: file` with a reason for existing-file changes that do not
  use symbol snapshots. File admission and postflight checks remain required.

### Fixed

- Evaluate dependency-cycle risk for declared task files without materializing
  unrelated graph components. Preserve external cycle paths and reject incomplete
  searches instead of reporting zero risk.
- Keep CLI command construction within the Windows stack budget and report
  guard-runner preparation failures with specific reasons.
- Bind task verification kits and declared ignored inputs to observed check receipts.
- Preserve initial local work across task retries and audit only subsequent changes.
- Reapprove revised specs after implementation while retaining validated preconditions
  and checking new requirements.
- Support contained tracked symlinks on Unix and up to 100,000 observed files;
  reject unavailable snapshot inputs before executor handoff.
- Exclude plural test filenames from production maps and refresh old classification
  flags without replacing graph rows.

## [3.3.1] - 2026-10-15

### Added

- Opt-in graph ranking for repo-maps with `--ranking graph`. Lexical ranking
  remains the default; graph ranking is an experiment without a quality claim.
- Offline source-quote binding audits for judge and producer assessments against
  pinned snapshots, including raw and rendered Markdown framing diagnostics.

### Fixed

- Native review (`review-task run`, `run-task --auto-review`) is no longer lost
  when the model fences its JSON or adds a field: Claude Code validates the
  answer against the reviewer schema and the same model corrects it in the run.

### Changed

- The native reviewer needs Claude Code 2.1.290 or later in the 2.1 series and
  records review
  policy v2 (Read, Grep, Glob and StructuredOutput). Completed reviews recorded
  under policy v1 stay valid.
- The reviewer has 30 turns by default, configurable per project with
  `mastermind init --review-max-turns N`. A run that stops at its turn or
  schema-retry limit records `invocation_native_max_turns` or
  `invocation_native_structured_output_retries`.

## [3.3.0] - 2026-10-13

### Added

- Bounded source-range reads with source hashes and task-bound receipts for
  extending previously delivered ranges.
- Question-specific lexical repo-maps with bounded payloads and source/index
  bindings. Separate map-only and instruction-only modes support controlled
  comparisons. Claims still require source reads.
- Shared task and agent effort routing through CLI and MCP, with native client
  launch arguments and ordinary Codex spawn fields. The core agent-handoff
  skill applies routing before delegated research, execution and review.
- Seeded judge calibration, blind review exports and pre-registered
  qualification decisions with simulation-calibrated critical values.

### Changed

- Research and evaluation workflows retain source coverage, lookup usage,
  failed attempts and unknown measurements. Human reference labels remain
  distinct from model reviews.
- Missing effort context and duties outside the bounded research pilot select
  high. Explicit human effort choices take precedence; unsupported hosts retain
  their settings with an application limitation recorded.
- Update tree-sitter, TOML and pinned CI action dependencies.

### Fixed

- Keep repo-map CLI parsing within the Windows stack budget.
- Align native worker test waits with the worker's runtime budget.

## [3.2.4] - 2026-10-09

### Added

- Batch exact-name lookup in `mmcg_search`: query up to eight distinct names
  from one index snapshot, retaining each query's counts and truncation metadata.

### Changed

- Deliver reviewed feedback and habits before Git diagnostics. Admit whole
  claims without cutting their conditions or exceptions, report partial-list
  omissions and include reviewed habits in `doctor`'s profile-budget advice.
- Use Sonnet for bounded workflow research and Opus for cause investigation.
  Executor and auditor instructions preserve relevant preference conditions
  and check their application against the actual change.
- Organize deterministic eval harnesses by responsibility and use module
  commands. Benchmark accounting retains failed trials, unknown measurements,
  source coverage and stage-specific usage separately.

### Fixed

- Accept plain hyphenated terms in documentation, history, task and project
  context searches while preserving explicit FTS operators.
- Git pipe completion shares the operation's deadline and cancellation checks,
  avoiding false timeouts when pipe workers are scheduled after Git exits.
- Benchmark supervision keeps the child leader's PID reserved until process
  group cleanup and handles macOS zombie-only groups, avoiding signals to a
  recycled group and lost trial results.
- npm publication waits longer for accepted native archives to become public.
  Recovery dispatches from the immutable release tag while using the current
  publication helper from `main`.

## [3.2.3] - 2026-10-08

### Fixed

- Claude transcript collection no longer rejects a whole session because an
  assistant, attachment or tool-result record carries a different `cwd`
  (another directory, a worktree, a deleted folder). Only a cited human
  turn's `cwd` is checked, and it now counts inside the project tree, the
  project's Claude folder or the system temp directory, including after that
  directory is deleted.
- Resolve directory aliases for missing Claude memory and scratchpad paths,
  including macOS `/var` and `/private/var`. Skip recorded paths containing `..`
  even when the directory exists.
- `mastermind doctor` no longer reports a false "subagent MCP scoping"
  diagnostic for the `mmcg` registration `mastermind setup` itself writes,
  whose `env` is exactly `{"MMCG_PROFILE_CLIENT": <client id>}`.

## [3.2.2] - 2026-10-08

### Added

- `mastermind init --profile-budget <256-8000>` to set the project's default
  `mmcg_profile` delivery budget, saved in `.mastermind/setup.json` as
  `profile_budget_tokens` (default 4000) and omitted while it equals the
  default. Older binaries reject the key once it is written.
- `mastermind doctor` "profile budget" check: warns when the configured
  budget cannot deliver every eligible, source-verified rule of the
  whole-profile selection, with how many rules fit in ranked order, the
  tokens the full ranked set needs, the shortfall and an
  `init --profile-budget` hint.

### Changed

- Rank eligible feedback (active, not superseded) by scope — global, path,
  language, repo/project, role, workflow, newest stored `last_at` first
  within a tier — instead of alphabetical key order. The old fixed 12-item
  display cap is gone; the existing 64-rule source-verification limit now
  applies to eligible rules after ranking, instead of to whatever alphabetical
  key order selected first.
- Raise the default profile delivery budget and derive it from the project
  setting everywhere: `mmcg_profile`, the `UserPromptSubmit` hook (bounded by
  its 8 KiB context budget) and the `mmcg_context` / `run-task --exec` person
  layer (sized to the context budget left after the other layers).
- No shipped agent or skill instruction passes a fixed `budget_tokens` to
  `mmcg_profile` anymore; the project's configured budget applies.

### Fixed

- Deliver accepted preference rules again at the default budget: over
  budget, feedback is now trimmed one rule at a time from the least
  important end instead of the whole list being dropped, and `feedback_total`
  reports the eligible count before any cap. The old fixed 12-item cap on
  feedback (applied in alphabetical key order) no longer silently hides
  accepted rules beyond the 12th key.

## [3.2.1] - 2026-10-06

### Changed

- Require a result for each task-mining ticket, including empty results. Ask the
  current task agent for one continuation when a submission is missing.
- Distinguish missing reports, explicit no-signal results and legacy completions
  in hook diagnostics, and reserve prompt context for mining.

### Fixed

- Keep mining continuations in the original episode and exclude their generated
  prompts from personal evidence.
- Preserve completed source bindings across empty session-end receipts and late
  tool results.
- Bound large native tool payloads and retained traces without dropping valid capture.
- Report doctor failures and failed native runtime probes correctly.
- Fix concurrent capture-marker deletion on Windows, stale worker status during
  completion and unstable native-script tests on Linux.

## [3.2.0] - 2026-10-05

### Changed

- Use the current task agent for preference proposals on first native client
  setup. Capture runs locally and drafts require review. Separate background
  analysis remains available through `--mining on`.
- Configure profile delivery and task mining automatically on first `init`.
  Repeated setup preserves saved choices, including explicit opt-outs.
- Use the episode's native client and captured model for separate mining and
  prompt refinement. Delayed Claude transcripts can supply the actual response model.
- Update dependencies and consolidate the mining setup guide and reference.

### Fixed

- Preserve multiline conditions in local preference candidates, retry local
  analysis after a store outage, and retain the chosen profile client on completion.
- Archive closed episodes when the capture journal fills, and refresh candidates
  after session closure without manual replay.
- Keep mining active across tasks and native client updates while preserving
  explicit stops, retry budgets and completed checkpoints.
- Fix Windows CLI stack use and profile paths.
- Stop foreground mining cleanly when Ctrl-C interrupts processor hashing.

## [3.1.0] - 2026-09-28

### Fixed

- Align automatic refresh and status freshness checks for large repositories:
  100,000 candidates, 512 MiB of source, and a 30-second status scan.
- Handle binary files and unsupported file paths consistently across indexing,
  freshness checks, and Lens.
- Avoid a panic on Unicode whitespace in secret-like Markdown assignments.

## [3.0.1] - 2026-09-28

### Changed

- Give each Mastermind native hook an event-specific label in the client review
  screen. Re-running hook setup updates previous labels without duplicate hooks.

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v4.0.1...HEAD
[4.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v4.0.1
[4.0.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v4.0.0
[3.4.2]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.2
[3.4.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.1
[3.4.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.4.0
[3.3.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.3.1
[3.3.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.3.0
[3.2.4]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.4
[3.2.3]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.3
[3.2.2]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.2
[3.2.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.1
[3.2.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.0
[3.1.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.1.0
[3.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.0.1
