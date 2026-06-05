# Defect taxonomy

Each defect a subagent surfaces during workflow execution maps to a `kind:` key.
Planner uses the key to mechanically route to a fix template — no LLM judgment
needed for known cases. When a NEW defect surfaces that doesn't match any known
kind, the subagent marks it `kind: unclassified` and the planner promotes it
into a named entry here as part of the follow-up.

The full structured-report schema (what `kind:` slots into) lives alongside this
file as [`structured-report-schema.md`](structured-report-schema.md).

## Executor stop kinds

### `envelope_drift`

- **What**: Test asserts on the raw return value of `handle_tools_call`, but the
  dispatcher wraps every successful payload in
  `{ "content": [{ "type": "text", "text": "<serialized JSON>" }] }`. Field
  comparisons against the wrapper always fail.
- **Surfaced as**: `assertion left == right` panic where `left` is a JSON object
  and `right` is a sub-field that lives inside `content[0].text`.
- **Fix template**: Reuse the `unwrap_content` helper that lives in
  `mcp/servers/mmcg/src/mcp.rs::tests` from task 001. Wrap every `handle_tools_call`
  return with `unwrap_content(&v)` before asserting on fields. Do NOT redefine
  the helper.
- **First observed**: Task 001 Phase 2.3.

### `doc_surface_gap`

- **What**: Spec's Phase 3 (docs) covers fewer files than
  `scripts/validate.py::validate_mmcg_tool_drift` enforces. Validator finds tool
  names in `mcp.rs` but missing from one or more of: mmcg README, repo README,
  `.claude-plugin/marketplace.json`, `plugins/mmcg/.claude-plugin/plugin.json`.
- **Surfaced as**: `python3 scripts/validate.py` exits non-zero with
  `tool 'mmcg_X' missing — declared in mcp/servers/mmcg/src/mcp.rs but absent
  from this file` (one error per missing surface).
- **Fix template**: Add the three missing surfaces to `expected_docs[]` in the
  spec frontmatter, then add three Phase 3.x sub-steps with FIND/CHANGE TO blocks.
  Pattern: `marketplace.json` and `plugin.json` each carry ONE prose `description`
  string with the comma-separated tool list and `N tools` count; the repo
  `README.md` carries TWO occurrences (table cell + standalone-crate paragraph).
  Insert the new tool name before the trailing `status` entry in each list and
  bump the count by 1.
- **First observed**: Task 001 Phase 4.

### `zero_filter_verify`

- **What**: VERIFY command uses `cargo test --lib <module>::` (trailing `::`)
  which cargo treats as a literal path that no test matches. Command exits 0
  with zero tests run — false-positive "pass".
- **Surfaced as**: `cargo test ... <module>::` output reads `0 passed; 0 failed;
  N filtered out` even though the module HAS tests.
- **Fix template**: Drop the trailing `::`. Use the bare module name as the
  substring filter: `cargo test --lib <module>`. Cargo matches any test whose
  path contains the substring.
- **First observed**: Task 001 Phase 1.3.

### `stale_pre_edit_snapshot`

- **What**: Spec's Pre-edit symbol snapshot or a Phase's FIND block claims a
  function has visibility / signature X, but the on-disk function already has
  visibility / signature Y. The FIND text doesn't appear in the file.
- **Surfaced as**: Executor returns `find_block_mismatch: <file> doesn't contain
  the FIND text` for a phase that's nominally just a visibility change or
  signature tweak.
- **Fix template**: Either (a) drop the phase entirely if the change is already
  in place (the more common case — re-check whether the goal is satisfied by
  the current state), or (b) update the FIND/CHANGE TO blocks to match the
  actual current state. Re-capture the snapshot via
  `./mcp/servers/mmcg/target/debug/mmcg query symbols-in-file <path>` before
  rewriting.
- **First observed**: Task 002 Phase 1.5.

### `seed_extractor_mismatch`

- **What**: Integration test hand-crafts an intermediate type (e.g. `PendingFile`
  with placeholder `kind: "fn"`, hand-written `signature: "fn foo()"`) to seed
  storage. The consumer-under-test re-derives the same type from real input via
  a parser (e.g. tree-sitter via `extractor_for_path` + `parse_one`), which
  produces a structurally-equivalent but byte-different shape
  (`kind: "function"`, fully-qualified signature). Hash/compare assertions fail
  even on semantically-identical input.
- **Surfaced as**: A round-trip test that should be a no-op returns a
  "structural change" / "different" verdict; classifier or comparator is
  correct, the seeding path is the bug.
- **Fix template**: Seed via the same pipeline the consumer uses. For mmcg
  fingerprint / structural tests, call `crate::indexer::extractor_for_path`
  followed by `crate::indexer::parse_one` on a real on-disk fixture, then pass
  the resulting `PendingFile` to `commit_file`. Never construct intermediate
  parser-output types by hand.
- **First observed**: Task 002 Phase 2.4.

### `fmt_tension`

- **What**: Spec's verbatim Rust code blocks are line-wrapped for documentation
  readability (e.g. multi-line `Vec::with_capacity(…)` calls, broken-out
  `std::fs::write(…)` arg lists). Rustfmt collapses these. `cargo fmt --check`
  fails even though `cargo test` passes — the diffs are cosmetic only.
- **Surfaced as**: `cargo fmt --check` exits non-zero with format-only diffs in
  files the executor just wrote from spec FIND/CHANGE TO blocks; no semantic
  divergence.
- **Fix template**: Default to (b) — add an explicit Rule to the spec
  authorizing one `cargo fmt` normalization pass on touched files, with a note
  that fmt may only collapse/expand whitespace and must not change logic. Use
  (a) — re-author the spec blocks in rustfmt style preemptively — only for
  surgical edits to a single function. Future planners SHOULD include the fmt
  authorization Rule from the start on any spec that emits >50 LOC of Rust.
- **First observed**: Task 002 Phase 2.4.

## Auditor discrepancy kinds

### `scope_creep`

- **What**: `git diff --name-only HEAD` shows files NOT in the spec's `touches[]`
  + `expected_docs[]` union.
- **Surfaced as**: Auditor's diff-vs-spec check enumerates files outside scope.
- **Fix template**: Either revert the out-of-scope edits or extend the spec's
  scope (with rationale) and re-spawn the audit. Zero tolerance unless the
  planner explicitly excepted (e.g. authorized `cargo fmt` normalize affects
  format-only).
- **First observed**: (none — all 001/002 audits clean. Listed for completeness.)

### `phase_not_in_diff`

- **What**: Executor marked Phase X as `[x]` complete but the phase's CHANGE TO
  content isn't present in the file.
- **Surfaced as**: Auditor greps for canonical anchor strings from the CHANGE TO
  block and finds nothing.
- **Fix template**: Investigate whether the executor lied or a later phase
  reverted the change. Re-run that phase's VERIFY command in isolation.

### `verify_failed_on_rerun`

- **What**: Auditor re-ran a VERIFY command that the executor reported as passing,
  and it now fails.
- **Surfaced as**: Discrepancy entry with the verbatim re-run output.
- **Fix template**: Snapshot the environment diff (env vars, working directory,
  locked dependencies). Almost always a flake or env-specific behavior; if not,
  the executor's claim is suspect.

### `snapshot_caller_drift`

- **What**: Pre-edit snapshot in spec said symbol X had N callers; post-execution
  `mmcg query callers X` returns M ≠ N.
- **Surfaced as**: Auditor's drift check enumerates the delta.
- **Fix template**: Either the executor changed something out of scope (check
  the diff for new call sites involving X), or the snapshot was wrong to start
  with. If the latter, drop the snapshot's per-symbol claim and re-run the audit.

### `snapshot_signature_drift`

- **What**: Symbol X's signature changed but the spec didn't authorize it (e.g.
  spec said "public signature stays unchanged" but the diff shows a parameter
  added).
- **Surfaced as**: Auditor compares pre-edit `mmcg query search X` signature
  against post-edit.
- **Fix template**: Almost always a real contract violation. Stop, revert the
  signature change, re-issue the phase preserving the original signature.

### `validator_link_policy_gap`

- **What**: Spec's CHANGE TO content adds a relative markdown link from an
  installable artifact (e.g. `agents/subagents/foo.md`) to a target that
  escapes its installable package (e.g. `../../skills/workflow/bar/refs/x.md`).
  `scripts/validate.py` warns: `installable file escapes package — link goes
  N levels up (max 0 for this file class). Reference the artifact by name
  instead`. Subagents and CLAUDE.md templates are flat-installed to
  `~/.claude/agents/` and can't follow `../`-style paths there.
- **Surfaced as**: `python3 scripts/validate.py` exits 0 (errors clean) but
  emits one warning per offending link. The spec's Phase 5 / Phase N VERIFY
  treats `≥ 1 warning` as a failure depending on the spec's strictness rule.
- **Fix template**: Replace each cross-package relative markdown link with a
  bare-name reference using the convention from
  `feedback_artifact_references.md` in user memory: subagent → `name`, skill →
  `/name`, doc reference → "X.md in <skill>'s references" or similar prose.
  The LLM agent has the referenced artifact loaded; no path lookup needed.
  Same-package relative links (within one skill tree, e.g.
  `../mastermind-task-planning/references/…` from `mastermind-task-executor/SKILL.md`)
  stay inside the installable package and pass the validator — only links
  CROSSING the `agents/`↔`skills/` boundary or going > 0 levels up from a
  subagent fall foul.
- **First observed**: Task 003 Phase 5.1 (executor stopped, planner promoted
  the kind into the taxonomy in the same flight).

### `verify_grep_window_too_small`

- **What**: Spec's VERIFY command uses `grep -A N "anchor" file | grep -c "phrase"`
  to confirm a phrase landed inside an "anchor + first few lines" window, but
  `N` is sized for the spec author's mental layout (e.g. "header, blank, heading,
  one bullet" = 4 lines) while the on-disk file has more pre-existing content
  between the anchor and the new phrase (e.g. multiple prior bullets in the
  same group). The phrase is correctly added to the file but lives outside the
  `-A N` window.
- **Surfaced as**: `grep -c` prints `0` even though the file contains the
  phrase exactly as specified. `grep <phrase> <file>` confirms presence.
- **Fix template**: Drop the windowed grep and use the bare
  `grep -c "<unique phrase>" <file>` form. Pick a phrase that's unique to the
  new content so the count remains 1 even on whole-file scan. Only keep `-A N`
  when the anchor → phrase distance is short AND constant across spec authors
  (e.g. immediately-following H2 with first paragraph).
- **First observed**: Task 003 Phase 6.1 (planner sized `-A 4` for 2 bullets;
  by execution time there were already 2 prior bullets pushing the new one to
  line 6).

### `unclassified`

- **What**: A defect that doesn't match any kind above.
- **Surfaced as**: Subagent emits `kind: unclassified` with a verbatim `details:`
  description.
- **Fix template**: Read the verbatim details, design the fix manually. After
  the task lands, promote this defect into a named entry in this taxonomy via a
  follow-up spec (or a direct doc PR — taxonomy edits don't need their own
  spec). The `[auto]` `_lessons.md` entry from `mmcg audit-spec` is a good
  starting point for the writeup.

## Status (no defect)

When NO defect applies → `kind: clean` and the workflow proceeds normally.
Empty `defects: []` / `discrepancies: []` arrays in the structured tail also
indicate the clean case.
