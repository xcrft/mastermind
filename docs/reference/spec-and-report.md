# Specs, reports and CI

A task spec is the contract a task runs against: which files and symbols may
change, what must be checked and which criteria decide completion. This page
holds the exact rules the gates apply to specs, executor reports and CI. The
[workflow guide](../workflow.md) shows the sequence a person follows, and
[task runtime](task-runtime.md) owns the runner, native executor, reviewer and
repair contracts.

## Task gates

```bash
mastermind new-spec "Add account recovery"
mastermind new-spec "Rotate signing keys" --mode strict
mastermind verify-spec .mastermind/tasks/042-feature/spec.md --require-index
mastermind run-task .mastermind/tasks/042-feature/spec.md --pre-only
mastermind verification run .mastermind/tasks/042-feature/spec.md --id unit --json
mastermind acceptance status .mastermind/tasks/042-feature/spec.md --json
mastermind run-task .mastermind/tasks/042-feature/spec.md --post-only
```

| Command | Contract |
|---|---|
| `verify-spec` | Checks the pre-edit contract. `--strict` forces strict checks. `--require-index` rejects a missing, empty or stale index. Without it, skipped checks are `not_evaluated` and the reason goes to stderr |
| `verification run` | Runs one declared `verify[].run` after preflight and records a task- and file-bound observation on macOS and Linux. Every result except a pass exits nonzero. See [observed verification](task-runtime.md#observed-verification) |
| `acceptance status` | Read-only view of each criterion, its declared checks and their current receipts. Exit 0 means `requirements_satisfied`. A spec without the field is `not_declared`. See [acceptance mapping](task-runtime.md#acceptance-mapping) |
| `audit-spec SPEC --since REF` | Compares the contract with the working tree, see [audit-spec](#audit-spec) |
| `run-task` | Records preflight, reads the executor report, audits, then completes only through the review and final evidence gates that apply |

### audit-spec

| Input or condition | Behavior |
|---|---|
| Working tree | Uncommitted and untracked files count, including edits hidden by `assume-unchanged` or `skip-worktree` |
| Broken verdict | Exits nonzero |
| More than 10,000 changed files or a non-UTF-8 path | Refused with `change_inventory_incomplete` |
| Index that does not match the files | Refused. Run `mastermind index .` first |
| `--bundle` | Seals a commit-range diff instead and needs a baseline different from HEAD. See [GitHub audit Action](../github-action.md) |
| Spec and `--executor-report` paths | A relative path resolves from the working directory. An absolute path also resolves through a link or `..` that reaches the root |
| Lesson candidate that cannot be written | Reported on stderr, also with `--json` |

### Task controller options

| Option | Behavior |
|---|---|
| `--pre-only` | Preflight only. Retries keep the original baseline and increase the iteration |
| `--post-only` | Audit an existing task again. Runs no executor and no verification command |
| `--reset` | Repeat preflight, keeping baseline and iteration count |
| `--strict` | Enforce strict contract checks |
| `--allow-no-index` | Allow contracts without indexed source or symbol evidence. Errors from a populated index still count, and a revised code contract brings the requirement back |
| `--max-iterations N` | Total preflight limit, default 3. `0` turns the limit off |
| `--force-iteration` | Skip the iteration limit once and record a lesson candidate |
| `--exec` | Start a new preflight iteration and one bound Claude run. See [native executor](task-runtime.md#native-executor) |
| `--guarded-exec` | With `--exec`, limit supported native tools to the declared scope and exact verification commands. See [guarded execution](task-runtime.md#guarded-execution) |
| `--exec-timeout N`, `--exec-max-turns N` | Executor time and turn limits, see [native executor](task-runtime.md#native-executor) |
| `--profile-client ID` | With `--exec`, the profile audience is the project's configured client for Claude. This flag overrides it for one run and still needs an existing grant |
| `--auto-repair` | With `--exec`, retry fresh failed checks inside the approved scope. See [automatic repair](task-runtime.md#automatic-repair) |
| `--auto-review` | Review a held task once. Without `--exec`, review an existing held task without running preflight, the executor or checks |
| `--auto-follow-up` | With `--exec --auto-review`, allow one repair of concrete unmet criteria and a fresh review. See [one semantic follow-up](task-runtime.md#one-semantic-follow-up) |
| `--review-timeout N`, `--review-max-turns N` | Reviewer limits, see [native semantic reviewer](task-runtime.md#native-semantic-reviewer) |

`--auto-repair` and `--auto-review` need structured acceptance and refuse
`--pre-only` and `--post-only`. `--auto-repair` also refuses `--force-iteration`.
Native runs record provenance in `invocation.json`, and scope is still audited
afterwards. Neither native mode is an OS file system or network sandbox.

### Task state and report files

| Rule | Behavior |
|---|---|
| State binding | `state.json` binds the repository identity and the exact UTF-8 relative spec path. State from another repository or spec is rejected. A missing audit snapshot binding needs another audit |
| Relative spec path | Resolves from `--root`, not the working directory |
| Spelling | A path that differs from the name on disk, such as `SPEC.md` for `spec.md` on a case-insensitive file system, is refused with the path to use |
| Folder names | A folder under `.mastermind/tasks` that starts with `_` or `.` is refused, because `status`, `ci` and the index never treat it as a task |
| Canonical task folder | Keeps its own state and review files |
| Spec outside a task folder | State and releases use path-SHA-256 keys under `.mastermind/run-state/.noncanonical/` and `.mastermind/releases/.noncanonical/` |
| Executor report and audit | Beside the spec. `spec.md` uses `executor-report.md` and `audit.md`. Another spec adds its stem, so `docs/specs/a.md` uses `a.executor-report.md` and `a.audit.md`. `run-task --pre-only` prints the exact report path |

The `state.json` format version and older task state are described in
[task runtime](task-runtime.md).

## Semantic task review

```bash
mastermind review-task prepare .mastermind/tasks/042-feature/spec.md --json
mastermind review-task submit .mastermind/tasks/042-feature/spec.md \
  --report .mastermind/tasks/042-feature/review-input.json --json
mastermind review-task status .mastermind/tasks/042-feature/spec.md --json
mastermind review-task follow-up .mastermind/tasks/042-feature/spec.md --json
mastermind review-task run .mastermind/tasks/042-feature/spec.md --timeout 600 --max-turns 30 --json
mastermind run-task .mastermind/tasks/042-feature/spec.md --auto-review
```

`prepare` returns the evidence and an unknown `.draft`. Fill in and save that
draft, not the whole request. A submission needs a judgment and a reason for
every criterion, verification quality, scope control and proportionality.
Negative or unknown judgments are stored but block completion, and changed
evidence or a new review revision rejects the submission. The command does not
check reviewer identity, independence or whether a judgment is true.
[Review and complete](../workflow.md#review-and-complete) walks through it, and
[native semantic reviewer](task-runtime.md#native-semantic-reviewer) covers
`review-task run`.

| Field | Allowed values |
|---|---|
| Every `status` | `satisfied`, `unsatisfied` or `unknown` |
| `reviewer.kind` | `human` or `llm`. The draft leaves it `null` and `reviewer.name` empty for the reviewer to fill in |
| `history.context`, `history.lessons` | Each has a decision (`no_change`, `update_required` or `unknown`), a reason and evidence. `no_change` means the current canonical file needs no update. Resolved decisions cite `knowledge:context` and `knowledge:lessons`, with pinned file revisions |

A standalone accepted review can exit 0 with unresolved history. Completion also
needs both `no_change` decisions and current final evidence, and Markdown cannot
override a typed decision.

`follow-up` returns the bound review and target, a prioritized role, action and
focus, and the arguments to continue. It is always JSON, at most 1 MiB, and it
writes nothing, runs no check and calls no model. Exit 0 means a packet was
produced, not an approval. Stale, missing or revoked evidence gives no actionable
packet, and review reasons stay untrusted data that cannot widen scope or
permissions.

`submit` names the failing part of a rejected report and never quotes report
text:

| Error | Meaning |
|---|---|
| `semantic_review_report_invalid: <kind> at line L column C` | Not readable as a report. Kind is `syntax error`, `invalid field or value` or `unexpected end of input`. The position points at an unknown field or a wrong value. For a missing field it points at the end of the object that lacks it, so compare that object with the draft |
| `semantic_review_schema_version_unsupported`, `semantic_review_target_revision_mismatch`, `semantic_review_reviewer_name_invalid`, `semantic_review_reviewer_kind_required` | The parsed report is not for this target or reviewer |
| `criteria[<index>]: semantic_review_unknown_criterion` | A criterion ID the spec does not have |
| `criterion:<id>: semantic_review_duplicate_criterion`, `semantic_review_criterion_missing`, `semantic_review_required_check_not_cited` | A criterion is repeated, missing or does not cite a required check |
| `<location>: <rule>` for `criterion:<id>`, `verification_quality`, `scope_control` or `proportionality` | The first broken rule among `semantic_review_reason_invalid`, `semantic_review_evidence_limit`, `semantic_review_evidence_duplicate`, `semantic_review_evidence_required` and `semantic_review_evidence_unknown` |
| `history.context` or `history.lessons` with a rule | One of `history_disposition_reason_invalid`, `history_disposition_evidence_limit`, `history_disposition_evidence_duplicate`, `history_disposition_evidence_unknown`, `history_disposition_source_not_cited` |

## Spec frontmatter

`mastermind new-spec` writes a spec with these fields filled with placeholders.
Every field is optional. An unknown field or a wrong type fails verification and
audit with `invalid_frontmatter`.

| Field | Form | Meaning | Details |
|---|---|---|---|
| `id`, `title` | String or integer, string | Task number and title. An integer ID is read as a string | |
| `mode` | `verified` or `strict` | Workflow contract mode. `lite` and `standard` are also accepted | [Choose a mode](../workflow.md#choose-a-mode) |
| `risk` | `low`, `medium` or `high` | Shown in the risk report | |
| `touches` | List of `file`, `scope`, `reason`, `language`, `symbols` | Files the executor may change. `scope` is `symbols` (default), `file` or `delete` | [Touch scope](#touch-scope), [deleted files](#deleted-files-and-moves) |
| `touches[].symbols` | Names, or `name`, `signature`, `callers`, `file`, `language`, `reason` | Pre-edit snapshot of each changed symbol | [Spec snapshots](#spec-snapshots-and-removals) |
| `creates` | Paths | Files that are new relative to the task baseline | [Created files](#created-files) |
| `expected_docs` | Paths | Documents the change must update. A new document is also listed in `creates` | [Declared file scope](#declared-file-scope) |
| `verify` | Labels, `cmd`, or `cmd` with `run` | A label is informational. `cmd` is a command obligation. `run` (`id`, `argv`, `cwd`, `timeout_secs`, optional `inputs`) is run by `verification run` | [Observed verification](task-runtime.md#observed-verification) |
| `acceptance` | List of `id`, `statement`, `checks` | Each criterion requires every listed `verify[].run` ID. An explicit empty or null list fails | [Acceptance mapping](task-runtime.md#acceptance-mapping) |
| `breaking_changes.removed_symbols` | Names or detailed symbols | APIs the change removes. An unlisted removal breaks the audit | [Spec snapshots](#spec-snapshots-and-removals) |

| Parsing rule | Behavior |
|---|---|
| Unknown fields | Rejected at every level, so a misspelled key never disappears silently |
| Strings | Contract fields must be YAML strings. Unquoted numbers and booleans are not turned into paths, names, commands or labels |
| Nulls | Rejected everywhere. Leave an optional field out instead |
| Invalid or unterminated block | A hard `invalid_frontmatter` in preflight and audit. The body is kept for diagnostics and never replaces the rejected contract |

## Declaration identity

Symbol diff matches declarations inside their matched parent scopes, so
same-name methods in different classes and trait impls stay apart. Matching
ignores line shifts and uses signatures to keep identity when declarations move
around in a file.

| Case in one scope, name and kind | Result |
|---|---|
| Identical signatures | Matched first. Repeats pair in source order |
| One remaining pair | A signature change |
| Several unmatched declarations | Explicit removals and additions, because the pairing is ambiguous |
| Declaration moved to another parent | Removal and addition |

Changed-test evidence uses the current declaration line and never matches a
removed test to a surviving declaration with the same name. Graph propagation
keeps its name- and type-based precision limits.

## Touch scope

| `touches[].scope` | Requirement | Use it for |
|---|---|---|
| `symbols` (default) | Nonempty `symbols` in strict preflight. A `reason` is invalid | Changes to named declarations |
| `file` | A nonblank `reason` and no symbols | File-level work such as documentation, configuration, notebooks or new tests in an existing file |
| `delete` | A nonblank `reason` and no symbols | Removals and moves, see [deleted files](#deleted-files-and-moves) |

Scope is never inferred from a file extension, and new files belong in
`creates`. File scope keeps file admission, exact postflight path checks,
verification, index requirements and removal checks. An unsupported value or
conflicting declarations invalidate the frontmatter in every mode.

## Spec snapshots and removals

A snapshot names a declaration with its lexical qualification, such as `B.run`
or `B::run`. Verification and audit resolve that declaration before they compare
its recorded signature.

| Case | Result |
|---|---|
| Unqualified name that matches several declarations | `snapshot_unresolved`, even when one candidate still has the old signature |
| File and language from a matching frontmatter touch | Also apply to snapshot bullets |
| Missing qualifier | Never falls back to a leaf-name match |
| Malformed name, broken parent chain, failed query or index update during resolution | Unresolved. Verification fails and audit returns Broken |
| Uniquely resolved signature change | Drift in the post-execution audit |
| Snapshot declaration gone from the diff | `SnapshotSymbolGone` and Broken, unless `breaking_changes.removed_symbols` uniquely names the same baseline declaration and the diff proves that exact declaration was removed |
| Removed class | Does not acknowledge its methods |

Audit checks recorded signatures and caller counts in Markdown snapshots and in
frontmatter touches. Bare touches stay pre-edit existence and file-scope
declarations. Caller counts and risk totals are name and type graph candidates
in the declared language, not edges bound to one declaration.

Removal acknowledgements accept qualified names and an optional file and
language:

```yaml
breaking_changes:
  removed_symbols:
    - name: B.run
      file: service.py
      language: python
      signature: "def run(self)"
```

A signature is checked only after the identity is unique, so matching one
overload's signature cannot select it from several. Bare names work when unique
in the baseline. An unresolved acknowledgement is
`removal_acknowledgement_unresolved` and makes the audit Broken.

| Baseline read | Rule |
|---|---|
| Without a file scope | Searches all tracked regular files with a supported extractor, unchanged files included |
| With a file scope | Reads only the needed baseline files |
| Cannot prove uniqueness | Syntax errors, unsupported path encoding, incomplete diffs and missing baseline objects. Use precise file scopes or repair the baseline |
| Limits | One shared deadline, 10,000 source files, 4 MiB of tree metadata, 5 MiB per blob, 64 MiB of blobs, 200,000 declarations |
| Git behavior | Replacement refs are ignored and missing objects are not fetched |
| Vue | The embedded script tree is checked too. Several script blocks, external scripts and unsupported script languages stay incomplete. Quoted `lang="ts"` selects the TypeScript parser |
| Extractor | Contract `mmcg-extractors-v10`. Output from another contract needs a reindex |

`mastermind ci` uses the same removal identity for snapshots and touches after
execution, and accepts a deleted touched file when all its removed declarations
are acknowledged. Other missing files, expected docs, mandatory sections and
scoped snapshot checks still apply. `verify-spec` runs before execution, so it
needs those targets to exist. Leading `./` and path separators are treated the
same in current symbols, baseline declarations and deleted-file checks, and an
invalid relative scope cannot grant a deletion exception.

## Declared file scope

Preflight, audit, controller postflight and `ci` share one admission check for
declared files. Frontmatter `touches`, `creates` and `expected_docs` are
authoritative when nonempty. Paths mentioned in the spec body are used only when
no structured file scope exists, and never after invalid frontmatter.

| Declaration rule | Behavior |
|---|---|
| Target | A contained regular file with a valid relative path. Directories, symlinks and reparse points, special files, absolute, parent and empty paths fail |
| Aliases | Leading `./` and separator variants are normalized the same way in scope checks, bundles and controller snapshots, whatever the working directory |
| `touches` and `creates` | Cannot name the same target |
| Ancestors | A declared file cannot be the parent of another declared file |
| Read | No-follow opens through one root capability, identity and metadata checked again. No content is read, so binary assets work |
| Limits | 1,024 declarations and 16,384 work units per check, 4,096 path bytes and 64 components per path. A declaration costs one unit, each open or recheck one plus its component count |

The checks share a deadline and an interrupt state. They show presence and type
at check time, not file contents or an atomic snapshot.

| Failure | Result |
|---|---|
| File missing at preflight | `missing_file` |
| Other preflight admission failure | Hard `declared_file_unavailable` |
| Any postflight admission failure | `declared_file_unavailable` and Broken, also for a deleted required document |
| Acknowledged deleted code touch | May be absent after baseline proof, checked again. An `expected_docs` alias of it must still exist |
| Unchanged existing expected file | Advisory `missing_expected_file` |

Rejected declarations stay in diagnostic findings, never in path fields of the
bundle. Empty, oversized or aggregate targets use `file: null` with a reason. A
failed controller re-audit clears earlier approval snapshots and needs planner
review. Repair and re-audit keep the first baseline. External files are not
allowed. Use `run-task --post-only` to request a fresh audit.

## Created files

Declare additions in a top-level list such as `creates: [src/new.py, docs/new.md]`.

| Phase | Requirement |
|---|---|
| Preflight | A regular draft file, or proven absence after no-follow inspection of every existing parent. A read error, dangling link or missing parent never counts as absence. Preflight creates nothing, so a revised spec can retry with its first baseline |
| Postflight | Each created file is regular, appears in the added or untracked diff, and is absent from the baseline tree. An old file removed from the Git index and left untracked is rejected |
| Baseline queries | Names only, literal paths in bounded batches, sharing the admission budget, deadline and cancel. A failed query is not proof of absence |

A new required document goes in both `creates` and `expected_docs`. Snapshots,
removal acknowledgements and FIND blocks keep their own requirements, and
`creates` does not waive them. Created paths enter audit, report and bundle
scope, history receipts and strict controller and policy snapshots. The spec hash
binds which files are new and which existed.

## Deleted files and moves

Declare a removal with `scope: delete`, a nonblank reason and no `symbols`. For a
move, also list the destination in `creates`:

```yaml
touches:
  - file: src/old.test.ts
    scope: delete
    reason: Move the callback-only tests to their new path
creates: [src/new.test.ts]
```

This works for files without indexed declarations, such as callback-only tests,
re-export barrels and assets. Postflight requires the source to be absent and the
diff to prove its deletion. The original must be a regular file in the Git
baseline or in a captured initial preimage. A missing baseline entry, a symlink,
a submodule or a failed baseline read cannot prove a deletion. Staging a rename
is optional.

Delete scope does not acknowledge removed APIs. Every other removed non-module
declaration still needs a uniquely resolved `breaking_changes.removed_symbols`
entry. Two removals need none:

| Removal | Why it is not an API removal |
|---|---|
| A function declared inside a function or method body in Python, Rust or C# | It stays in that body's scope, so no caller can reach it |
| Every declaration of a deleted file whose exact bytes appear at a new or changed indexed source path in the same diff | The file moved and its declarations still exist. Each destination covers one deleted file. A move with any content change, or to a path the index skips such as `api.py.orig` or `build/api.py`, counts the old declarations as removed |

Other nested declarations can be API, so their removal needs an
acknowledgement: methods of an object literal or class expression that a JS or
TS function returns, a nested PHP function (global once the outer one runs) and
members of a type declared inside a function. A deleted path cannot also be an
ordinary touch, a creation or an `expected_docs` entry.

The first preflight and `verify-spec` need the source to exist. After a move,
change the source touch to `scope: delete` and run
`mastermind run-task <spec> --pre-only`. A task with captured initial inputs can
approve a proven deletion again with its original baseline. Then run the
declared checks again, postflight and `review-task prepare`. Do not delete task
state or reset the baseline to hide finished work.

## Literal FIND preconditions

`verify-spec` and `run-task --pre-only` check literal `FIND:` blocks against the
current working files before execution.

| Result | Cause |
|---|---|
| `find_block_mismatch` | A complete read that lacks the literal |
| `find_block_unavailable` with a reason | Missing target marker, invalid path, missing or unreadable file, incomplete read, invalid UTF-8, changed file, expired deadline or exhausted budget |

A failed preflight cannot approve the task, so fix the input and retry.

| FIND rule | Behavior |
|---|---|
| Targets | Repository-relative regular files. `./` and separator aliases work. Absolute, parent, symlink and special-file targets are rejected |
| Reads | One root capability, no-follow opens, complete UTF-8 contents |
| Limits per check | 1,024 blocks, 16,384 work units, 32 MiB of reads, 5 MiB per file, 4,096 path bytes, 64 path components. A block costs one unit, a read one plus its component count. Failed reads still spend their byte allowance |
| Final check | Every inspected file is read again and compared by identity and hash, then the root is checked. A changed receipt turns even an earlier mismatch into `find_block_unavailable`. This is not an atomic snapshot of all files |

Postflight and `ci` do not need the old FIND text to survive a replacement or an
acknowledged deletion, and they do not use the Git baseline in its place. The
parser does not model `CHANGE TO` payloads, so a passing FIND says nothing about
whether the edit happened. Review the diff and the acceptance criteria.

## Qualified declarations and signatures

| Language or case | Rule |
|---|---|
| Qualification | Follows extracted lexical parents. C# namespaces match whether stored as one qualified name or nested. File modules are not package names, and package scope is not inferred from a path |
| Rust impl blocks | Give method scope but are not named types. Snapshots of impl blocks are not supported by name |
| Rust impl removal | Select it by file and its exact baseline header, for example `{name: B, file: service.rs, signature: "impl Marker for B"}`. Identical headers stay ambiguous. The type and each removed child need their own acknowledgement. This selector does not apply to method overloads |
| Ambiguous by nature | Multiple trait implementations, overloads, constructors and partial declarations |
| Rust signatures | Include attached outer `#[...]` attributes with arguments, so attribute edits show as `signature_changed`. Comments between attributes and the item are ignored. Inner `#![...]` attributes do not attach. Macros are not expanded |
| Python signatures | Include attached decorators in source order, multiline arguments too, separated by newlines. Decorator edits show as `signature_changed` for functions, methods and classes, and a method decorator edit can also mark its class `body_changed` |
| Concept normalization | Treats Python `//` as division and keeps Python quote escaping |

When changed files need extraction, diff and impact reject an index from an
older extractor contract with `index_stale`. Reindex and refresh affected spec
snapshots, because bare `def ...` or `fn ...` snapshots no longer describe the
full declaration. Keep multiline signatures in YAML. Adding a test attribute marks
a changed test candidate. It does not show the test ran.

## CI

`mastermind ci` indexes the repository, verifies the selected specs, parses
their executor reports, audits the real diff and can write sealed bundles. A
source file that fails to index fails the run and is named, because an audit over
a partial graph can miss changed or removed symbols. For pull requests, limit the
gate to changed task folders and require evidence:

```bash
mastermind ci --since origin/main \
  --changed-only --require-executor-report \
  --bundle-dir .mastermind/audit-output
```

| Flag or rule | Behavior |
|---|---|
| No `--changed-only` | Walks all task specs |
| `--changed-only`, `--require-executor-report` or `--bundle-dir` | A checkout without a task contract fails. Task files must be in the checkout |
| `--bundle-dir` | Always needs a canonical `executor-report.md`, even without `--require-executor-report` |
| Changed-task discovery | Resolves baseline and HEAD to commit IDs, uses a literal task-directory pathspec and the bounded Git deadline, and ignores ambient repository-routing and external-diff settings. A change anywhere under a task folder maps to its spec. Linked directories are not followed |
| Bundle inputs | The spec and report are read through one repository capability. Path swaps, symlinks and special files are rejected before their digests are recorded |

## Executor report evidence

`run-task --post-only`, `audit-spec` and `ci` read the executor report with
these rules. The report format is in the
[structured report contract](../../skills/workflow/mastermind-structured-report-contract/SKILL.md).

### Report admission

| Rule | Behavior |
|---|---|
| Canonical schema-v1 report | Task path, completion status, phases, modified files, defects and verification excerpts are read |
| `partial` or `failed` status | `executor_report_rejected` and a `broken` audit, even without claims |
| `spec` | Must name the audited task inside the repository, as an absolute or repository-relative path |
| Reported files | Normalized and compared both ways with the diff, without `.mastermind/` controller files. A missing or extra entry makes the audit `broken`. The list does not replace the spec scope |
| Report in the older `mastermind:executor-begin` format | Rejected by `run-task` postflight and by `ci` with `--require-executor-report` or `--bundle-dir`. Plain `audit-spec` and `ci` still read it |
| Parsing | Explicit nulls, wrong scalar types and ambiguous sentinel blocks are rejected. Sentinel comments sit on their own unindented lines, and marker text inside YAML strings stays data |
| Reads | Bounded no-follow snapshots. An explicitly selected report that changes during the read is rejected |
| Empty phase and verification lists | Valid |

Canonical postflight also needs a passing reported result for every nonempty
`verify[].cmd` and every `VERIFY:`, `**VERIFY**:` or `**VERIFY:**` command line in
the spec. Labels and ordinary shell fences declare nothing. `verify-spec
--strict` needs at least one such command, and labels, blank `cmd` values or
shell comments do not count.

### Lines a gate refuses to read

Only one normalized `##` heading may define each contract section, so `## Scope`
and `## Scope *(MANDATORY)*` together are a `duplicate_section` error. Headings
and directives inside fenced examples are example text. A gate never reads an
unparsed line as nothing to check: `verify-spec` reports `unparsed_line` as an
error and `audit-spec` as a `broken` finding.

| Input | Result |
| --- | --- |
| `---` after a BOM or a blank line, or with trailing spaces | `invalid_frontmatter: frontmatter_not_at_start` |
| A count (`N callers`) or a signature (`` signature `sig` ``) outside a list item, or in an item without a backticked name before `signature` | `snapshot_line_unparsed` |
| A Pre-edit Snapshot heading that is not a top-level `## ` section (`###`, `##` and a tab, a setext heading, or inside a list item) | `snapshot_heading_unparsed` |
| Claims for several symbols or several counts in one item (`` `a`, `b` — 2 callers ``, `` `a` — 0 callers, `b` — 3 callers ``) | `snapshot_line_unparsed` |
| A count that is not one whole number before `callers` in the same clause (`callers: 9`, `callers (4)`, `~40`, `40+`, `1,234`, `3–5`, `3 to 5`, `two`) | `snapshot_caller_count_unparsed` |
| `signature` followed by an unreadable code span, or by a dash, arrow, parenthetical or up to three words before the span (`` signature is `sig` ``) | `snapshot_signature_unparsed` |
| An upper-case `FIND` with text after its colon outside a checklist, inline code, plain words above a fence, an empty fence or no fence | `find_block_unparsed` |
| An upper-case `VERIFY:` with nothing after it, a `VERIFY` with a command but no colon, or a `VERIFY` line with several or unclosed code spans | `verify_command_unparsed` |

A refused line gives no claim. A snapshot item reads
`` - `name` — N callers; signature `sig` ``, the form `new-spec` writes.

| Snapshot rule | Behavior |
|---|---|
| Count | The whole number nearest before `caller` or `callers` in the same clause. `39 pooled indexed callers` reads 39, `no callers` reads 0. A number only after the word, as in `callers: 4`, is refused |
| Clause end | `,`, `;`, `:`, a parenthesis or a dash, but not between digits. `callers;` and `callers—` are the word, `caller-side` is not |
| Spans that are not extra symbols | File locations such as `` `src/a.rs:12` ``, the span after `signature`, and spans after the count that name callers or parameters without their own count |
| Several symbols | A later clause that opens with its own symbol and states a claim, as in `` `a` — 3 callers; `b` — signature `fn b()` `` |
| No count or signature | A planner note |
| Snapshot heading | Every top-level `##` heading whose words start with `pre edit` and name a snapshot, after an optional section number. `## 4. Pre edit snapshot:` counts |

| Marker rule | Behavior |
|---|---|
| Where | `FIND`, `VERIFY` and `File` are read inside list items, after a task checkbox and with a qualifier such as `FIND (exact):` |
| `VERIFY` | Needs a colon. On its own line in upper case it declares the text after it. In a list item, after a checkbox or in another case it declares only a backticked command, so `- [ ] VERIFY: commands are runnable` stays prose |
| `Find:` | Counts only when a fence follows, with or without the colon |
| Qualifier | A `)` inside a code span does not close it, as in ``FIND (inside `run()`):`` |
| Indentation | Columns from the list item's content column, a tab to the next multiple of four. A line four or more columns further in is an example and gates skip it. A fence line four columns past its opener is content |
| Fences in list items | Their content loses the fence indentation |

### Reported verification

| Report row | Result |
|---|---|
| No row for a required command | `verification_requirement_unmet` with `missing_result` |
| Canonical pass without `observed.exit_code: 0` | Rejected by the parser |
| Older-format pass without an exit receipt | `unobserved_exit` |
| Row that does not claim a pass, reports a nonzero exit, or reports zero tests for a recognized test run | `not_passed` |
| Failing rows mixed with passing rows | `conflicting_results` |
| Repeated passing rows | Allowed, with different excerpts or positive test counts |

Command matching trims only outer whitespace, so arguments, case, wrappers and
inner whitespace count, and separate rows cannot satisfy a compound command.
These findings make the audit and bundle `broken` and return the task to
planner review. `tests_run` is optional, and a reported zero exit code is a
self-consistency receipt, not proof that the command ran. When a spec has no
command obligations, empty results pass.

A `verify[].run` declaration adds a local execution requirement on top: the
latest matching runner record must be a success, and a reported pass cannot
replace a missing, pending, failed, cancelled, stale or changed record. See
[observed verification](task-runtime.md#observed-verification). The `acceptance`
field adds per-criterion checks and the `acceptance_criterion_unmet` finding.
See [acceptance mapping](task-runtime.md#acceptance-mapping).

### Test execution claims

| Claim | Result |
|---|---|
| Claimed pass with a nonzero `observed.exit_code` | `observed_exit_code_non_zero`, for any command |
| `observed.tests_run: 0` for a recognized test run | `observed_zero_tests`, with a zero or missing exit code |
| Positive count with a nonzero exit | Still `observed_exit_code_non_zero` |
| Missing test count | Not treated as zero |

| Recognized test runs | Notes |
|---|---|
| Direct `cargo test`, `go test`, `pytest`, `python -m pytest`, `python3 -m pytest`, `jest`, `vitest run`, `vitest --run`, also `.exe` names | Plain paths, filters and some runner options. Use `vitest run` for bare positional filters, because `--run` can still select another subcommand |
| Compile, collection, help, version and watch modes | Not a finite test run |
| Unsupported flags, forwarded harness arguments, shell quoting and expansion, wrappers, npm or yarn scripts | Unknown. A zero count alone does not break the audit |

Recognition does not read implicit configuration, authenticate a run or change
command matching.

`vacuous_test_claim` is an advisory warning for a recognized test command that
reports no positive count when the file system shows no conventional tests.
Finding no test files does not prove nothing ran, and finding files does not
override an explicit zero. Rust attributes are parsed, so `#[test]` in a comment
or string does not count, and malformed Rust makes the scan unknown.

| Scan rule | Behavior |
|---|---|
| Warning | Only after a complete scan with rechecked entries, path kinds and Rust source receipts |
| Unknown | External paths, parent traversal, symlinks, special files, read errors, invalid UTF-8 or incomplete rechecks. Unknown suppresses the warning and is not evidence that tests exist |
| Budget | One capability and budget for the whole report: 16,384 work units, 8 MiB of reads, 1 MiB per file, depth 12, 64 path components for starting scopes, plus the audit deadline |
| Cargo | `src/` and `tests/` under one crate, or that directory when both are missing. Package, workspace and doctest selectors are unknown |
| Go | One local directory and a trailing `/...` |
| Pytest | One directory or Python file, `test_*.py` and `*_test.py`. Several targets and node selectors are unknown |
| Paths | Git metadata, indexed declarations and task bindings resolve from the repository root, even from a nested working directory |

Hard contradictions in reported outcomes are checked on their own, also after
the scanner runs out of budget. The scan does not read package configuration or
plugins.

The JSON audit result includes the complete checked `executor_report`. A bundle
binds all report metadata and verification evidence as well as claims, so a
report with the same claims but different completion or task data gives a broken
bundle. When bundle construction gets no report, it reuses the checked snapshot,
and sealing checks the input file against that snapshot again.

### Executor claim evidence

`audit-spec --executor-report ... --json` includes ordered `claim_checks`: the
complete claim, its zero-based index, a `verified`, `failed` or `unresolved`
status, and either evidence or a finding. A missing field means claims were not
evaluated, and `[]` means an empty report was evaluated.

| Claim | Rule |
|---|---|
| `function_added` | Must select one exact lexical declaration, then compare the signature, and match a declaration the baseline diff introduced in that file. A body edit, signature edit or matching overload is not enough. Evidence records file, line and baseline OID. Moves across files are not detected |
| `integration` with `relation: calls` (the default) | Both endpoints resolve uniquely and one compatible indexed call candidate exists across files in the source language. `to_file` cannot hide another compatible target. The result is a `compatible_call_candidate` with `name_based_candidates` precision, not a compiler binding or runtime call. Callbacks, other relations, unresolved prefixes and ambiguous candidates fail |
| Indistinguishable parents or edited overload sets | `executor_claim_unresolved` |
| Existing declaration claimed as added | `claimed_symbol_not_added` |

Changed source files and claim endpoint files must match the index hashes and
parse without recovery. Query failures, stale files, incomplete diffs, index
changes and exceeded budgets stay unresolved and make the audit `broken`, in
`ci` and controller postflight too. Schema-v3 bundles keep their layout and
allow the two finding kinds above. Claim labels carry their position, so
repeated names stay separate, and a bundle needs outcomes for the exact claim
sequence. An unchecked or substituted report gives a broken bundle with no
verified claims.
