# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v0.26.0...HEAD
[0.26.0]: https://github.com/xcrft/mastermind/compare/npm-v0.25.0...npm-v0.26.0
[0.25.0]: https://github.com/xcrft/mastermind/compare/npm-v0.24.0...npm-v0.25.0
[0.24.0]: https://github.com/xcrft/mastermind/compare/npm-v0.23.1...npm-v0.24.0
[0.23.1]: https://github.com/xcrft/mastermind/compare/npm-v0.23.0...npm-v0.23.1
[0.23.0]: https://github.com/xcrft/mastermind/compare/npm-v0.22.1...npm-v0.23.0
[0.22.1]: https://github.com/xcrft/mastermind/compare/npm-v0.22.0...npm-v0.22.1
[0.22.0]: https://github.com/xcrft/mastermind/releases/tag/npm-v0.22.0
