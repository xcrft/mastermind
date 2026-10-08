# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v3.2.1...HEAD
[3.2.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.1
[3.2.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.0
[3.1.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.1.0
[3.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.0.1
