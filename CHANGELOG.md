# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v3.2.0...HEAD
[3.2.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.2.0
[3.1.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.1.0
[3.0.1]: https://github.com/xcrft/mastermind/releases/tag/npm-v3.0.1
