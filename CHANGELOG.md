# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.28.0] - 2026-06-05

### Fixed
- Flaky test temp-dir collision. `doctor.rs::tmp()` and `verify_spec.rs::tmp()` — the only two test helpers that keyed their temp-dir name purely on `process::id()` + a nanosecond timestamp — could resolve to the SAME directory when two of cargo's parallel test threads entered them in the same nanosecond bucket, letting one test's `remove_dir_all` wipe another's working dir mid-run (observed as an intermittent `check_gitignore_warns_when_missing_or_unset` panic, ~1 in 6 runs). Both helpers now mix in a process-global `AtomicU64` counter, guaranteeing a distinct directory per call. Test-harness only — no production code touched.

### Added
- Premature-terminal escalation tiers in the `mastermind-task-planning` SKILL — a forge-`StepEnforcer`-style self-check the planner-LLM runs before drafting a "task done" message to the user. Three tiers: (1, polite) audit chain not started → spawn auditor and wait; (2, direct) `drift`/`broken` verdict → refuse to ship, address discrepancies; (3, aggressive) user asks to skip audit → refuse, explain `_lessons.md` precedent, record an explicit override in the conversation transcript + auto-append a `kind: premature_terminal_temptation` lesson. Pairs with spec-003's typed-report convention — the auditor's structured tail IS the artifact the planner checks for at tier 1. Zero new code, zero new MCP tools, zero CLI changes — pure SKILL-prose convention engineering. New defect kind `premature_terminal_temptation` added to `defect-taxonomy.md`.
- Iteration budget in `mmcg run-task` — every pre-flight on the same spec increments a counter (stored on `RunState.iteration`, additively via `#[serde(default)]` so legacy state files load cleanly). The 4th cycle without landing `Held` is refused with a clear escalation message + auto-appended `kind: iteration_budget_exhausted` lesson in `.mastermind/tasks/_lessons.md`. Two new flags: `--max-iterations N` (default 3, matching the `mastermind-task-planning` SKILL's prose convention and forge's `ErrorTracker.max_retries`) and `--force-iteration` (bypass for explicit one-off overrides; lesson still fires). The counter survives `--reset` so the budget can't be trivially bypassed by repeated state-drops.
- Cross-agent scratchpad MCP tools — `mmcg_scratchpad_append { agent, kind, body }` and `mmcg_scratchpad_read { since?, agent?, kind?, limit? }`. Live in-session channel between Mastermind subagents (planner → executor → auditor), persisted in `.mastermind/mmcg.db` (additive table — no schema-version bump, no re-index needed). Body capped at 8 KiB. Cross-session counterpart remains `.mastermind/tasks/_lessons.md` (auditor-written).
- Structural fingerprints on every indexed file plus a new `mmcg_change_class { file }` MCP tool that returns `structural` / `cosmetic` / `first-seen` for a given path. The fingerprint is a deterministic FNV-1a 64-bit hash of the file's parsed shape (sorted symbol `(kind, name, signature)` tuples + sorted edge `(kind, from, to_name, to_path)` tuples — line numbers and whitespace excluded by design). Stored in a new `files.structural_fingerprint` column added idempotently via `ALTER TABLE` — no `SCHEMA_VERSION` bump, no forced re-index. Existing files report `first-seen` until naturally re-indexed.
- Typed subagent reports — `mastermind-task-executor` and `mastermind-auditor` now emit a fenced-YAML "structured tail" at the end of every report, wrapped in `<!-- mastermind:report-begin -->` / `<!-- mastermind:audit-begin -->` sentinels. Defect taxonomy lives in `skills/workflow/mastermind-task-planning/references/defect-taxonomy.md` (6 executor stop kinds + 5 auditor discrepancy kinds + `unclassified` escape hatch, each with named fix templates). Planner SKILL gains "Defect-aware retry" + "Iteration budget" sections describing the mechanical routing: parse the tail, match `kind:`, apply named fix template, re-spawn — capped at 3 rounds. Bakes the lessons from tasks 001 + 002 (envelope drift, doc-surface gap, zero-filter verify, stale pre-edit snapshot, seed-extractor mismatch, fmt tension) into the workflow as a closed routing table instead of free-form prose. Inspired by [`forge`'s Nudge + ErrorTracker patterns](https://github.com/antoinezambelli/forge). Zero new code in mmcg — pure convention engineering.

## [0.27.1] - 2026-06-03

### Security
- Drop `serde_yml` (and its `libyml` transitive) — both archived as unsound: [RUSTSEC-2025-0068](https://rustsec.org/advisories/RUSTSEC-2025-0068.html) (`serde_yml`) and [RUSTSEC-2025-0067](https://rustsec.org/advisories/RUSTSEC-2025-0067.html) (`libyml`, `yaml_string_extend` UB). Swapped to `serde_norway 0.9.42` — maintained fork of `serde_yaml`, backed by `unsafe-libyaml-norway` (maintained libyaml fork). API-compatible drop-in; no behavior change on spec parsing. Closes the two Dependabot alerts (1 high + 1 medium).

## [0.27.0] - 2026-06-03

### Added
- Strict, contract-driven gates. `mastermind verify-spec --strict` and `run-task --strict` require YAML frontmatter that scopes the change (`touches` with files + symbols) plus at least one `verify[].cmd`, and require an index. `verify-spec --require-index` fails (instead of silently skipping the live symbol checks) when no index is present.
- Deterministic `_lessons.md` writer. `mmcg audit-spec` and `mmcg run-task`'s post-phase now append a `[auto]`-prefixed one-line lesson to `.mastermind/tasks/_lessons.md` on every `Drift` / `Broken` verdict — no LLM in the loop, so the file accumulates real signal even when the planner skips spawning the `mastermind-auditor` subagent. The auditor subagent still writes a richer root-cause line alongside; the `[auto]` entry is the mechanical finding summary (counts of scope creep / caller drift / etc.).

### Changed
- **BREAKING — task specs now live in per-task folders.** New layout is `.mastermind/tasks/<NNN>-<name>/spec.md` (was `.mastermind/tasks/<NNN>-<name>.md`). The folder holds the spec plus any related artifacts (audit notes, screenshots, scratchpad). The only shared asset that may stay flat at the top of `tasks/` is `_lessons.md` (auditor-appended). Bare `.md` files at the top of `tasks/` are no longer indexed by `mmcg_tasks`. `mastermind init` surfaces them with a per-file migration command (`mkdir -p .mastermind/tasks/NNN-name && mv .mastermind/tasks/NNN-name.md .mastermind/tasks/NNN-name/spec.md`). All bundled templates, skills, and subagents updated to the new path shape.
- `mastermind init` no longer drops `_spec-template.md` into `.mastermind/tasks/`. The planner skill reads from its own bundled template (`skills/workflow/mastermind-task-planning/references/spec-template.md`), so the project-level copy was unused — and worse, `.mastermind/*` is gitignored, so any local customization vanished on fresh clone. Existing copies are detected by `init` and surfaced with a `rm` hint.
- Release: npm packages are now published with `npm publish --provenance`, attaching a signed [build provenance attestation](https://docs.npmjs.com/generating-provenance-statements) minted via GitHub Actions OIDC. Each package on npm links back to the exact workflow run and commit that built it.

### Fixed
- `mastermind setup claude` (global/user scope) now registers mmcg via `claude mcp add --scope user` (writes `~/.claude.json`) instead of `~/.claude/.mcp.json`, which Claude Code **ignores** — so global registration silently never loaded the mmcg tools. `--project .` still writes the project `.mcp.json` (already correct). `uninstall --scope global` now uses `claude mcp remove`, and `doctor`'s MCP-config check reads `~/.claude.json` so it can no longer report a false "registered".

## [0.26.0] - 2026-05-28

### Added
- `mastermind init` now also installs prompt-based **slash commands** into `~/.claude/commands/` (alongside the subagents + skills) — the bundled `prompts/` become `/`-commands in Claude Code. `--no-global` skips the whole global install.

## [0.25.0] - 2026-05-28

### Added
- `mastermind init` now installs the workflow subagents + skills into `~/.claude/{agents,skills}/` — the full planner / critic / executor / auditor pipeline (plus workflow skills) from a single npm install, not just the codegraph. The npm package bundles them under `share/`; `init` overwrites Mastermind's own files there to keep them current. `--no-global` skips it; a cargo install ships no bundle and falls back to the plugin marketplace.

## [0.24.0] - 2026-05-28

### Added
- `mastermind init --with-claude-md` now also fills the dropped CLAUDE.md's `<PLACEHOLDER>` sections (project name, run / test / typecheck / lint commands) from the codebase via `claude -p`, in the same hands-off run that drafts CONTEXT.md. `--no-claude` leaves the template unfilled.

## [0.23.1] - 2026-05-28

### Changed
- `mastermind init` runs `claude -p` with `--permission-mode acceptEdits` so it drafts CONTEXT.md hands-off — no mid-run approval prompt for the CONTEXT.md write.

## [0.23.0] - 2026-05-28

### Added
- `mastermind uninstall` — removes a Mastermind setup. `--scope project` (default) deletes `.mastermind/` (index, tasks, run-state) and the project `.mcp.json` mmcg entry; `--scope global` removes the `~/.claude/.mcp.json` entry; `--scope all` does both. Safe dry-run by default; `--force` to apply. Never touches CONTEXT.md / CLAUDE.md.

### Changed
- `mastermind init` now builds the index automatically (`--no-index` to skip) and populates CONTEXT.md from the codebase via `claude -p` (`--no-claude` to skip; falls back to printing the prompt if the Claude CLI is unavailable).
- `mastermind` is now the primary command in all help text and CLI output; `--help` usage, the long description (with an onboarding walkthrough), and every printed command example say `mastermind`. `mmcg` remains a working alias (the cargo-installed binary name).
- Fixed `mastermind init` "Next steps" to reference real commands (`mastermind setup claude --write-mcp`) instead of repo-internal paths that don't exist for npm installs.
- Rewrote the npm README with a step-by-step quick start and a "what gets set up where" guide (per-project index vs. global MCP registration).

## [0.22.1] - 2026-05-28

### Added
- `author` field on all npm packages.
- README badges (npm version, CI, license), a License section, a Node.js version note, and a changelog link.
- Per-package README for each `@xcraftmind/mmcg-*` platform package.

## [0.22.0] - 2026-05-28

### Added
- npm distribution: install via `npx` or `npm` with prebuilt native binaries — no Rust toolchain required.
- Seven prebuilt platform packages (`@xcraftmind/mmcg-*`) covering macOS (arm64, x64), Linux glibc and musl (x64, arm64), and Windows (x64). npm installs only the package matching the host's `os` / `cpu` / `libc`.
- Install-mode-aware `setup claude` that writes the correct MCP `command` form for npx, global, project-local, and cargo installs.

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v0.28.0...HEAD
[0.28.0]: https://github.com/xcrft/mastermind/compare/npm-v0.27.1...npm-v0.28.0
[0.27.1]: https://github.com/xcrft/mastermind/compare/npm-v0.27.0...npm-v0.27.1
[0.27.0]: https://github.com/xcrft/mastermind/compare/npm-v0.26.0...npm-v0.27.0
[0.26.0]: https://github.com/xcrft/mastermind/compare/npm-v0.25.0...npm-v0.26.0
[0.25.0]: https://github.com/xcrft/mastermind/compare/npm-v0.24.0...npm-v0.25.0
[0.24.0]: https://github.com/xcrft/mastermind/compare/npm-v0.23.1...npm-v0.24.0
[0.23.1]: https://github.com/xcrft/mastermind/compare/npm-v0.23.0...npm-v0.23.1
[0.23.0]: https://github.com/xcrft/mastermind/compare/npm-v0.22.1...npm-v0.23.0
[0.22.1]: https://github.com/xcrft/mastermind/compare/npm-v0.22.0...npm-v0.22.1
[0.22.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v0.22.0
