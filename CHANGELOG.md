# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `mastermind init` stack detection is now **runtime + detected framework** instead of a handful of narrow profiles. It reports a rich label — `rust`, `rust (tauri desktop)`, `node (react)`, `node (vue)`, `node (next.js)`, `node (api)`, `python (fastapi)`, `python (django)`, `go`, etc. — and picks the best lean CONTEXT.md template for it (the bespoke TypeScript-API / FastAPI / React-Native / Rust templates where they fit, the generic scaffold otherwise).
- Polyglot monorepos are detected as their own profile rather than `generic`: a bare root whose subpackages carry manifests across ≥2 ecosystems reports `monorepo (go, python, typescript, …)` (enumerated from `git ls-files`) and scaffolds a monorepo CONTEXT.md that says each package owns its stack and tells the agent to read the nearest manifest / per-service CLAUDE.md.

### Changed
- The `rust-cli` profile is now just `rust` — it was mislabelling Rust services and libraries as CLIs. Template renamed `rust-cli.md` → `rust.md` and generalized.

## [0.34.0] - 2026-06-27

### Added
- Author style miner — `mastermind miner profile` learns your code-shape style ("write like me") into `~/.mastermind/style.md`, which the planner reads when drafting `CHANGE TO` blocks. Deterministic (git + line heuristics, no LLM): it derives code-shape idioms (indentation, quotes, line length, comment density, brace style, declarations) and commit conventions (prefix, subject length, body usage). A rule is emitted only with a dominant pattern over enough samples and names the counter-pattern it rejects — no signal, no rule.
- Cross-repo style store — each mine enriches a user-global SQLite store (`~/.mastermind/style.db`) and regenerates `style.md` from the aggregate, so the profile is one fingerprint summed over every repo you mine. Idempotent per repo (re-mining replaces, never doubles), with retention that drops repos gone from disk or unmined for a year. A managed/manual split preserves hand edits across re-mines, and `mastermind doctor` nudges a re-mine once you've accrued enough new commits since the last one. `--deep` optionally adds an LLM-written "design patterns" section via `claude -p` (sends sampled lines + commit messages; off by default).

### Changed
- `mastermind init` is now batteries-included — it auto-detects the repo's stack from its manifests for CONTEXT.md and always installs the workflow CLAUDE.md, dropping the opt-in `--profile` / `--with-claude-md` / `--with-workflow` flags. The product is the whole workflow out of the box.

### Removed
- The standards-library layer — artifact `_template/` scaffolds, the `*-anatomy.md` docs, `docs/conventions.md`, the new-artifact contribution flow, and the unused `filesystem-readonly` MCP example. Mastermind is a product (codegraph + workflow), not a contributable artifact library; `CONTRIBUTING.md` and the PR template are rewritten product-focused.

## [0.33.0] - 2026-06-22

### Changed
- Docs: npm package README rewritten to npm conventions and trimmed ~40%; top-level README onboarding aligned (`install`-led) and inventory updated (added the `coding/` skill domain, the `install` / `update` / `list` commands, and all three eval suites); binary `--help` now points npm users at the `mastermind install` shortcut.
- Task specs now require VERIFY commands to be scoped, cheap, and terminating: the planner skill, executor skill, and canonical spec template forbid whole-repo suites in a per-step VERIFY (use `tsc -p packages/x`, `npm test -- billing`), reserve the full typecheck/test suite for the phase boundary and final block, and ban non-terminating commands (`dev`/`start`/`watch`/`serve`) that hang the executor to its tool timeout. Cuts executor wall-clock on large repos without losing per-step failure localization.

### Tests
- `evals/ablation.py` — ablation harness for the auditor's marginal value: runs each planted-defect fixture through a strong vanilla baseline (`claude -p` + git/grep/read, no mmcg, no auditor prompt) and reports catch-rate, so the codegraph + auditor uplift is evidence, not assertion. `--with-mastermind` runs both conditions head-to-head.

### Fixed
- Prompt-refiner subagent is now self-contained for passthrough — the intake gate inlines the passthrough decision rule and output format (previously only in the skill, which a subagent can't load at runtime), so an already-tight prompt is returned unchanged (`action: passthrough`) instead of being needlessly rewritten.
- Critic verdict ladder sharpened — a sound approach with fixable gaps now scores `concern`/`revise`, and `rethink` is reserved for a genuinely wrong approach; stops the critic over-escalating under-specified-but-sound designs.

## [0.32.0] - 2026-06-19

### Added
- `mastermind install` / `update` / `list` — one-command Claude Code setup. `install` copies the workflow subagents + skills into `~/.claude/` **and** registers the mmcg MCP server (reusing `setup claude --write-mcp`, so the agents can actually query the codegraph); `update` re-copies the agents; `list` shows what ships. Each prints a completion summary. The per-project codegraph index stays with `mastermind init`.
- `no-ai-slop-comments` skill (new `coding/` domain) — states the one rule LLMs break constantly: a comment must say something the code can't (the *why*); everything else (restating the code, section banners, `// added` edit markers, ownerless TODOs) is slop and gets deleted. Ships in the default install. The `coding/` domain is registered in `docs/conventions.md` §1.3 and the `scripts/validate.py` whitelist.

### Changed
- The task executor (subagent + skill), the canonical spec template, and the auditor now enforce a no-AI-slop-comment rule end to end. The executor applies each `CHANGE TO:` block verbatim and adds no comments of its own; every generated spec carries a global Rule against restating-the-code / edit-marker comments; the auditor gained a post-flight check (§6.7) that flags slop comments added against that Rule while leaving genuine *why* comments alone. Baked self-contained into the subagents (no Skill tool at runtime), with a `[[no-ai-slop-comments]]` pointer for readers.

### Tests
- New auditor eval `a-009-slop-comments-drift` + `slop-comments` fixture — the executor pads an in-scope function with restate-the-code comments and a section banner, then falsely reports it added none; the auditor must read the hunk and flag the contradiction (`drift`/`broken`).

## [0.31.0] - 2026-06-18

### Added
- Shared workflow skills so every agent reads one source instead of duplicating contracts: `mastermind-codegraph-research` (mmcg-first structural lookup), `mastermind-structured-report-contract` (the executor↔planner↔auditor report tail), `mastermind-investigation-ledger` (the unknown-bug hypothesis loop), and `mastermind-critical-review` (a compact design/spec review rubric). The planner and executor skills reference them; subagents (which have no Skill tool at runtime) keep the operational content inline.
- `mastermind-security-auditor` subagent — an independent, Opus-tier security reviewer spawned only on security-sensitive scope (auth, tools, secrets, delegation, supply chain, prompt injection). Read-only Bash, evidence-first, never implements. Optional OWASP mode maps findings to the verified OWASP Top 10 for Agentic Applications (2026), shipped as the `mastermind-agent-security-review` reference pack rather than recited from memory.

### Changed
- Tightened code comments across the mmcg crate — compressed wording, kept every rationale.
- Simplified and actualized the top-level README; trimmed filler from `CONTRIBUTING.md` and several sub-READMEs.

### Fixed
- `.jsx` files were indexed under language `"jsx"`, which isn't in the MCP `language` enum and never matches a `language: "javascript"` filter — so `.jsx` definitions silently vanished from every language-scoped query (the exact monorepo cross-language-collision case the filter exists for, a silent false-negative). `guess_language_for` now folds `.jsx` into `"javascript"`, matching `lang_from_ext` and the schema enum.
- `audit-spec` vacuous-test detection no longer false-flags `cargo test`. Two compounding bugs: (1) `"cargo test"` contains the substring `"go test"`, so the Go detector (checked first) shadowed the cargo branch entirely and flagged every `cargo test` run for having no `_test.go` files in the repo root; (2) the cargo branch only scanned `src/`, so a crate whose tests live entirely in `tests/*.rs` (integration tests, no `#[test]` in `src/`) was flagged vacuous. Now the Go branch is guarded against `cargo`, the scan covers `src/` **and** `tests/`, and recognizes `#[tokio::test]` / `#[async_std::test]` / `#[rstest]` alongside `#[test]`.
- `audit-spec` no longer runs the static vacuous-test file-scan when the executor reported a positive `observed.tests_run` — a confirmed non-zero test count is authoritative and the heuristic must not override it.
- npm install-mode detection now walks up from cwd to find the owning `node_modules`, so a command run from a monorepo subpackage whose dependency is hoisted to the workspace root is classified `project` (was misclassified `global`, making `setup claude` emit the wrong MCP `command` form).
- mmcg MCP server now answers malformed JSON with a JSON-RPC `-32700` error (`id: null`) instead of silently dropping the line, so a strict stdio client doesn't block waiting for a reply. Valid notifications (no `id`) still correctly get no response.
- `verify-spec` PATH resolution (`which_on_path`) now checks the executable bit on Unix (`mode & 0o111`), not just file existence — a non-executable file shadowing a command name no longer suppresses the "command not found" warning.
- `mcp/servers/mmcg/README.md` frontmatter `metadata.version` bumped 0.28.1 → 0.30.0 to match the crate. The runtime `serverInfo.version` was already correct (`CARGO_PKG_VERSION`); only the doc metadata was stale.

### Tests
- New: `jsx_files_indexed_as_javascript`, `malformed_json_gets_parse_error_with_null_id`, `notification_without_id_gets_no_reply`, `file_has_test_attr_recognises_common_spellings`, `cargo_test_not_vacuous_with_only_integration_tests`. 169 lib + 17 golden pass.

## [0.30.0] - 2026-06-14

### Added
- `mcp/integrations/portable-baseline.md` and `mcp/integrations/org-overlay.md` — recipes that split the portable MCP layer (mmcg, carried into every repo, offline, no org account) from the per-project org overlay (SaaS MCP declared in `.mcp.json` and scoped to subagent roles via top-level `mcpServers:`). Makes the workflow portable across projects: same subagents/skills travel, only the per-project `.mcp.json` differs.
- `mcp/integrations/context7.md` — opt-in recipe wiring the hosted context7 MCP (`resolve-library-id` + `query-docs`) into the researcher/executor for live, version-current library docs. Deliberately outside the offline default: it's the one place the portable layer reaches the network.
- `mastermind doctor` check #9 `subagent MCP scoping` — warns when a subagent's `mcpServers:` names a server not registered in the project `.mcp.json` or `~/.claude.json`. Catches "scoped a server to a role but never registered it".
- `mmcg_status` now reports `stale_files` — source files modified since the last index (capped at 100) — so an agent knows when the index is behind the working tree and structural answers may be wrong, instead of trusting a silently stale graph.
- `mmcg_callers` and `mmcg_impact` now carry a `name_collision` field (how many definitions share the queried name), the same over-approximation signal `mmcg_centrality` already exposes — a value > 1 means the result pools call sites across same-named symbols.
- `scripts/validate.py` lints subagent frontmatter: `tools` / `model` / `mcpServers` / `disallowedTools` nested under `metadata` (rather than top-level) is now a hard error, so the runtime-field regression can't ship again. Audit note: skills nest the same fields, but have a different runtime contract — flagged, not yet changed.

### Changed
- `mmcg_centrality` rewritten to pre-aggregate in-degree per name in a single pass (a CTE over the call edges) instead of a per-symbol correlated join. Full-index centrality on a ~34k-file / 1.3M-edge monorepo dropped from ~146s to ~4s. Each hit now also carries a `name_collision` field — how many definitions share the leaf name — so an `in_degree` inflated by same-named call sites (e.g. `get` across hundreds of view classes) is visible rather than misleading.
- `mastermind status` suggests `mastermind watch` (alongside `index .`) when the index is stale, matching the hint `mastermind doctor` already gives.
- README: the workflow's determinism is now stated to live in the deterministic Rust gates (`verify-spec` / `audit-spec`), not in the subagents' MCP calls — the latter is LLM-interpreted context whose payoff scales with repo size. "Sub-millisecond queries" softened to distinguish point queries from whole-graph aggregations (centrality / impact / dependency cycles).

### Fixed
- `mmcg_symbols_changed_since` over MCP no longer errors with `canonicalize root: No such file or directory` when called without an explicit `root`. The default root is now derived from the canonicalized index file (`<root>/.mastermind/mmcg.db` → `<root>`); previously it climbed a *relative* db path to `""`, which failed to canonicalize and broke the default invocation.
- Subagent definitions now declare `tools`, `model`, and (for mmcg-using roles) `mcpServers: [mmcg]` as **top-level** frontmatter keys. They were nested under `metadata:`, which the Claude Code runtime does not read — so on a stock install every subagent silently inherited all tools and the parent's model: no read-only restriction (researcher/critic/auditor could write files), no Haiku/Sonnet/Opus tiering, and MCP access worked only by accident of that inheritance. Fixed across `agents/subagents/`, `extras/subagents/mastermind-release.md`, `agents/_template/subagent.md`, and the convention docs (`docs/conventions.md` §2.4, `docs/agent-anatomy.md`).

### Tests
- New unit tests: `changed_since_root_defaults_to_repo_root_from_db_path`, `definition_count_counts_same_named_non_module_defs`, `subagent_mcp_refs_parses_list_and_handles_absence`, `unregistered_subagent_servers_flags_missing_then_clears`; `centrality_ranks_by_in_degree` asserts the new `name_collision` field. 164 lib + 17 golden pass.

## [0.29.0] - 2026-06-13

### Added
- `mastermind ci` gate: index, verify all specs, audit executor reports, write optional bundle artifacts. Fails with non-zero exit on any parse, audit, or write error.
- `mastermind pr-comment <bundle.json>`: renders audit bundle as GitHub-flavored Markdown for PR comments.
- `mastermind tour`: six-step guided onboarding walkthrough.
- Five deterministic demo scenarios (`hallucinated-symbol`, `scope-creep`, `stale-find-block`, `vacuous-test`, `signature-drift`) — each runs in under one second, no API key required.
- Integration docs for Claude Code, Cursor, Continue, Codex CLI, and generic MCP clients (`docs/integrations/`).
- Audit bundle v2: `baseline`, `head`, `spec_files`, `changed_files`, `verified_claims`, `failed_claims`, `mmcg_queries`, `commands`, `human_summary`; legacy fields `files_diff` and `git_ref` preserved for backward compatibility.
- File-scoped executor claims: `FunctionAdded` gains `file` and `signature`; `Integration` gains `from_file` and `to_file` — narrows symbol lookups to declared files.
- `ObservedOutcome` on `VerifyResult`: CI can attach `exit_code` and `tests_run` so auditor catches "claimed passed but exit code 1" without re-running.
- Three new `Broken`-severity `Finding` variants: `ClaimedSignatureMismatch`, `ObservedExitCodeNonZero`, `ObservedZeroTests`.
- `SymbolHit.precision` field on `mmcg_search` results: carries `confidence`, `resolution`, and `limitations` per source language.
- `mmcg query explain <symbol>`: zero-results diagnostic with actionable hints.
- Six new golden tests: callee edges for Rust/Go/TypeScript, C++ macro limitation, Python dynamic attribute limitation, search precision field.

### Changed
- Auditor eval runner retries once on missing structured sentinel and tracks `first_pass` vs `after_retry` with honest duration accounting.
- README proof badge: `7/8 first, 8/8 retry` (was `8/8` without retry disclosure).
- `Bundle::from_report_full` now accepts `root: Option<&Path>` so `head` SHA is resolved relative to the audited project root, not the caller's working directory.
- Integration claim classification in bundle uses `(from, to)` pair matching; `FunctionAdded` classification uses `(symbol, file)` — prevents one failing claim from polluting another.

### Fixed
- CI bundle write errors now propagate as hard failures (`create_dir_all`, serialization, and `fs::write` all use `?`).
- File-scoped claim path matching normalizes `./` prefix and Windows `\` separators via `norm_path`/`norm_paths_eq`.
- `resolve_head_sha` runs git in `current_dir(root)` — correct SHA when `mastermind ci --root` points to a different directory.

### Tests
- 187 total (160 unit + 10 new_spec + 17 golden) — all pass.
- Two new unit tests: `norm_path_strips_dotslash_and_backslash`, `file_scoped_claim_dotslash_prefix_matches`.

## [0.28.2] - 2026-06-11

### Fixed
- `mastermind new-spec` no longer panics on Unicode descriptions (e.g. Cyrillic). `slugify` previously used byte-index slicing `slug[..40]` which could land inside a multi-byte UTF-8 codepoint. Now filters to ASCII-only chars and truncates with `.chars().take(40)`. Descriptions with no ASCII word characters fall back to the slug `"task"`.
- Generated YAML frontmatter now quotes the title field. Unquoted `title: {description}` caused YAML parse failures on descriptions containing `:`, `#`, or `"`, which silently dropped `frontmatter.mode` and caused `verify-spec` to apply full mandatory sections even on `--mode lite` specs.
- `audit-spec` integration verifier no longer produces false `MissingCallEdge` findings in monorepos with multiple symbols sharing the same name. Previously only the first `search_symbols` hit was checked; now all matching symbols are checked with `iter().any()`.
- `context doctor` module docstring now matches actual thresholds (`fail < 50`, `warn 50–199`, `ok ≥ 200` non-whitespace chars) instead of the incorrect `< 100` claim.

### Changed
- README: added **Daily workflow commands** section (`status`, `next`, `new-spec`, `resume`, `verify-spec`, `audit-spec`, `demo`, `context doctor`).
- README: `mastermind-investigator` now shown as debug-only path in pipeline diagram and `What's inside`, not as a main pipeline step.

### Tests
- 10 new unit tests in `new_spec.rs`: slug ASCII/Unicode/fallback/truncation, `yaml_quote` variants, frontmatter round-trip for all three modes.
- 2 new unit tests in `verify_spec.rs`: `lite_mode_goals_section_passes`, `lite_mode_does_not_require_standard_sections`.

## [0.28.1] - 2026-06-10

### Added
- Auditor eval fixtures for hallucinated symbols, stale symbol refs, TypeScript scope creep, and required-vs-optional signature drift.
- Language coverage table for mmcg.
- Eval fixture clue guard in `scripts/validate.py` — blocks banned meta-commentary from landing in fixture source trees.

### Fixed
- Auditor eval runner now fails hard when mmcg index is unavailable (previously silently skipped, producing results that could not be attributed to codegraph-backed auditing).
- Auditor eval runner now requires a structured `<!-- mastermind:audit-begin -->` verdict tail instead of prose keyword matching, eliminating false-positive passes.
- Suppress TERM/tput warnings in eval runner.
- Removed answer leakage from all auditor eval fixture source files (meta-commentary moved to fixture READMEs).

### Changed
- Align README and npm README to advertise the actual `node >=24` engine requirement (both previously said `18+`).
- Refactor `run-task` CLI dispatch: individual flags collapsed into `RunOpts` struct.

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

[Unreleased]: https://github.com/xcrft/mastermind/compare/npm-v0.28.1...HEAD
[0.28.1]: https://github.com/xcrft/mastermind/compare/npm-v0.28.0...npm-v0.28.1
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
