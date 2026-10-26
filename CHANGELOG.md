# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v3.4.2...HEAD
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
