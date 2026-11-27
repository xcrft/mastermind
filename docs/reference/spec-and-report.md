# Specs, reports and CI

A task spec says which files and symbols may change, what must be checked and
which criteria decide completion. These are the rules the gates apply to specs,
executor reports and CI. The [workflow guide](../workflow.md) shows the order to
follow, and [task runtime](task-runtime.md) covers the runner and the executor.

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
| `verify-spec` | Checks the pre-edit contract. `--strict` forces strict checks. `--require-index` rejects a missing, empty or stale index. Without it, skipped checks are `not_evaluated` |
| `verification run` | Runs one declared `verify[].run` and records a bound receipt. Every result except a pass exits nonzero. See [observed verification](task-runtime.md#observed-verification) |
| `acceptance status` | Read-only view of each criterion and its receipts. See [acceptance mapping](task-runtime.md#acceptance-mapping) |
| `audit-spec SPEC --since REF` | Compares the contract with the working tree. See [audit-spec](#audit-spec) |
| `run-task` | Records preflight, reads the executor report, audits, then completes only through the review and final evidence gates |

### audit-spec

| Input or condition | Behavior |
|---|---|
| Working tree | Uncommitted and untracked files count, including edits hidden by `assume-unchanged` or `skip-worktree` |
| Broken verdict | Exits nonzero |
| More than 10,000 changed files or a non-UTF-8 path | `change_inventory_incomplete` |
| Index that does not match the files | Refused. Run `mastermind index .` first |
| `--bundle` | Seals a commit-range diff instead and needs a baseline other than HEAD. See [GitHub audit Action](../github-action.md) |
| Relative spec and `--executor-report` paths | Resolve from the working directory |

### Task controller options

| Option | Behavior |
|---|---|
| `--pre-only` | Preflight only. Retries keep the original baseline and increase the iteration |
| `--post-only` | Audit an existing task again. Runs no executor and no verification command |
| `--reset` | Repeat preflight, keeping baseline and iteration count |
| `--strict` | Enforce strict contract checks |
| `--allow-no-index` | Allow contracts without indexed source or symbol evidence. Errors from a populated index still count |
| `--max-iterations N` | Total preflight limit, default 3. `0` turns it off |
| `--force-iteration` | Skip the iteration limit once and record a lesson candidate |
| `--exec` | Start a new preflight iteration and one bound Claude run. See [native executor](task-runtime.md#native-executor) |
| `--guarded-exec` | With `--exec`, limit native tools to the declared scope. See [guarded execution](task-runtime.md#guarded-execution) |
| `--exec-timeout N`, `--exec-max-turns N` | Executor limits for one run |
| `--profile-client ID` | With `--exec`, override the profile audience for one run. It still needs a grant |
| `--auto-repair` | With `--exec`, retry fresh failed checks. See [automatic repair](task-runtime.md#automatic-repair) |
| `--auto-review` | Review a held task once. Without `--exec`, review an existing held task only |
| `--auto-follow-up` | With `--exec --auto-review`, allow one repair of unmet criteria. See [one semantic follow-up](task-runtime.md#one-semantic-follow-up) |
| `--review-timeout N`, `--review-max-turns N` | Reviewer limits for one run |

`--auto-repair` and `--auto-review` need structured acceptance and refuse
`--pre-only` and `--post-only`. Native runs record provenance in
`invocation.json`, and scope is still audited afterwards. Neither native mode is
an OS sandbox.

### Task state and report files

| Rule | Behavior |
|---|---|
| State binding | `state.json` binds the repository identity and the exact relative spec path. State from another repository or spec is rejected |
| Relative spec path | Resolves from `--root` |
| Spelling | A path that differs from the name on disk, such as `SPEC.md`, is refused with the path to use |
| Folder names | A folder under `.mastermind/tasks` that starts with `_` or `.` is refused |
| Spec outside a task folder | State and releases go under `.mastermind/run-state/.noncanonical/` and `.mastermind/releases/.noncanonical/` |
| Executor report and audit | Beside the spec. `spec.md` uses `executor-report.md` and `audit.md`, and `docs/specs/a.md` uses `a.executor-report.md` and `a.audit.md`. `run-task --pre-only` prints the exact report path |

## Semantic task review

```bash
mastermind review-task prepare .mastermind/tasks/042-feature/spec.md --json
mastermind review-task submit .mastermind/tasks/042-feature/spec.md \
  --report .mastermind/tasks/042-feature/review-input.json --json
mastermind review-task status .mastermind/tasks/042-feature/spec.md --json
mastermind review-task follow-up .mastermind/tasks/042-feature/spec.md --json
mastermind review-task run .mastermind/tasks/042-feature/spec.md --timeout 600 --max-turns 30 --json
```

`prepare` returns the evidence and an unknown `.draft`. Fill in and submit that
draft. A submission needs a judgment and a reason for every criterion,
verification quality, scope control and proportionality. Negative or unknown
judgments are stored but block completion, and changed evidence rejects the
submission. The command does not check who the reviewer is or whether a judgment
is true. [Review and complete](../workflow.md#review-and-complete) walks through
it, and [native semantic reviewer](task-runtime.md#native-semantic-reviewer)
covers `review-task run`.

| Field | Allowed values |
|---|---|
| Every `status` | `satisfied`, `unsatisfied` or `unknown` |
| `reviewer.kind`, `reviewer.name` | `human` or `llm`, and a name. The draft leaves them empty |
| `history.context`, `history.lessons` | A decision (`no_change`, `update_required` or `unknown`), a reason and evidence citing `knowledge:context` or `knowledge:lessons` |

A standalone accepted review can exit 0 with unresolved history. Completion also
needs both `no_change` decisions and current final evidence.

`follow-up` returns the bound review, a prioritized role, action and focus, and
the arguments to continue. It is JSON of at most 1 MiB, writes nothing and runs
nothing. Exit 0 means a packet was produced, not an approval.

`submit` names the failing part of a rejected report and never quotes report
text:

| Error | Meaning |
|---|---|
| `semantic_review_report_invalid: <kind> at line L column C` | Not readable as a report. Kind is `syntax error`, `invalid field or value` or `unexpected end of input`. For a missing field the position is the end of the object that lacks it |
| `semantic_review_schema_version_unsupported`, `semantic_review_target_revision_mismatch`, `semantic_review_reviewer_name_invalid`, `semantic_review_reviewer_kind_required` | Not for this target or reviewer |
| `criteria[<index>]: semantic_review_unknown_criterion` | A criterion ID the spec does not have |
| `criterion:<id>: semantic_review_duplicate_criterion`, `semantic_review_criterion_missing`, `semantic_review_required_check_not_cited` | A criterion is repeated, missing or does not cite a required check |
| `<location>: <rule>` for `criterion:<id>`, `verification_quality`, `scope_control` or `proportionality` | One of `semantic_review_reason_invalid`, `semantic_review_evidence_limit`, `semantic_review_evidence_duplicate`, `semantic_review_evidence_required`, `semantic_review_evidence_unknown` |
| `history.context` or `history.lessons` with a rule | One of `history_disposition_reason_invalid`, `history_disposition_evidence_limit`, `history_disposition_evidence_duplicate`, `history_disposition_evidence_unknown`, `history_disposition_source_not_cited` |

## Spec frontmatter

`mastermind new-spec` writes these fields with placeholders. Every field is
optional. An unknown field at any level, a null or a wrong type fails
verification and audit with `invalid_frontmatter`, and contract fields must be
YAML strings.

| Field | Form | Meaning |
|---|---|---|
| `id`, `title` | String or integer, string | Task number and title |
| `mode` | `verified` or `strict` (`lite` and `standard` still accepted) | Contract mode, see [choose a mode](../workflow.md#choose-a-mode) |
| `risk` | `low`, `medium` or `high` | Shown in the risk report |
| `touches` | List of `file`, `scope`, `reason`, `language`, `symbols` | Files the executor may change, see [touch scope](#touch-scope) |
| `touches[].symbols` | Names, or `name`, `signature`, `callers`, `file`, `language`, `reason` | Pre-edit snapshot of each changed symbol, see [snapshots](#spec-snapshots-and-removals) |
| `creates` | Paths | Files new to the task, see [created files](#created-files) |
| `expected_docs` | Paths | Documents the change must update. A new one is also in `creates` |
| `verify` | Labels, `cmd`, or `cmd` with `run` | A label is informational, `cmd` is an obligation, and `run` is executed by `verification run` |
| `acceptance` | List of `id`, `statement`, `checks` | Each criterion requires every listed `verify[].run` ID |
| `breaking_changes.removed_symbols` | Names or detailed symbols | APIs the change removes. An unlisted removal breaks the audit |

## Touch scope

| `touches[].scope` | Requirement | Use it for |
|---|---|---|
| `symbols` (default) | Nonempty `symbols` in strict preflight, no `reason` | Changes to named declarations |
| `file` | A nonblank `reason`, no symbols | Documentation, configuration, notebooks or new tests in an existing file |
| `delete` | A nonblank `reason`, no symbols | Removals and moves, see [deleted files](#deleted-files-and-moves) |

Scope is never inferred from a file extension, and new files belong in
`creates`.

## Declared file scope

Preflight, audit, postflight and `ci` share one check for declared files.
Frontmatter `touches`, `creates` and `expected_docs` decide when nonempty. Paths
in the spec body count only when no structured file scope exists.

| Rule | Behavior |
|---|---|
| Target | A contained regular file with a valid relative path. Directories, symlinks, special files, absolute, parent and empty paths fail. `./` and separator variants are normalized |
| `touches` and `creates` | Cannot name the same target, and one declared file cannot be the parent of another |
| Limits | 1,024 declarations and 16,384 work units per check, 4,096 path bytes and 64 components per path |

| Failure | Result |
|---|---|
| File missing at preflight | `missing_file` |
| Other preflight failure | `declared_file_unavailable` |
| Any postflight failure | `declared_file_unavailable` and Broken |
| Unchanged existing expected file | Advisory `missing_expected_file` |

## Created files

Declare additions in a top-level list such as `creates: [src/new.py, docs/new.md]`.

| Phase | Requirement |
|---|---|
| Preflight | A regular draft file, or proven absence. A read error, dangling link or missing parent never counts as absence |
| Postflight | Each created file is regular, shows in the added or untracked diff and is absent from the baseline tree |

`creates` does not waive snapshots, removal acknowledgements or FIND blocks.

## Deleted files and moves

Declare a removal with `scope: delete`, a reason and no `symbols`. For a move,
also list the destination in `creates`:

```yaml
touches:
  - file: src/old.test.ts
    scope: delete
    reason: Move the callback-only tests to their new path
creates: [src/new.test.ts]
```

Postflight requires the source to be absent and the diff to prove its deletion
from a regular file in the Git baseline or the initial preimage. A symlink,
submodule or missing baseline entry cannot prove it. A deleted path cannot also
be an ordinary touch, a creation or an `expected_docs` entry.

Delete scope does not acknowledge removed APIs. Every other removed declaration
needs a `breaking_changes.removed_symbols` entry, except:

| Removal | Why no entry is needed |
|---|---|
| A function declared inside a function body in Python, Rust or C# | No caller can reach it |
| Every declaration of a deleted file whose exact bytes appear at a new indexed source path in the same diff | The file moved. A move with any content change, or to a path the index skips, counts as removal |

The first preflight needs the source to exist. After a move, change the source
touch to `scope: delete`, run `run-task <spec> --pre-only`, then the checks,
postflight and `review-task prepare` again, and keep the task state.

## Spec snapshots and removals

A snapshot names a declaration with its lexical qualification, such as `B.run`
or `B::run`. Verification and audit resolve it before they compare the
recorded signature and caller count.

| Case | Result |
|---|---|
| Unqualified name that matches several declarations | `snapshot_unresolved` |
| Malformed name, failed query or index change during resolution | Unresolved. Verification fails and audit returns Broken |
| Uniquely resolved signature change | Drift in the post-execution audit |
| Snapshot declaration gone from the diff | `SnapshotSymbolGone` and Broken, unless `removed_symbols` names the same baseline declaration |
| Removed class | Does not acknowledge its methods |

Removal acknowledgements take qualified names and an optional file, language
and signature:

```yaml
breaking_changes:
  removed_symbols:
    - name: B.run
      file: service.py
      language: python
      signature: "def run(self)"
```

A signature is checked only after the identity is unique. An unresolved
acknowledgement is `removal_acknowledgement_unresolved` and makes the audit
Broken. Caller counts are name and type graph candidates in the declared
language.

| Baseline read | Rule |
|---|---|
| Scope | All tracked files with a supported extractor, or only the declared files |
| Limits | 10,000 source files, 4 MiB of tree metadata, 5 MiB per blob, 64 MiB of blobs, 200,000 declarations |
| Cannot prove uniqueness | Syntax errors, unsupported path encoding, incomplete diffs, missing baseline objects |
| Extractor | `mmcg-extractors-v11`. Output from another contract needs a reindex |

| Language or case | Rule |
|---|---|
| Matching | Inside matched parent scopes. Identical signatures pair first, one remaining pair is a signature change, and several unmatched declarations are removals and additions |
| C# namespaces | Match whether stored as one qualified name or nested |
| Rust impl blocks | Select by file and exact header, for example `{name: B, file: service.rs, signature: "impl Marker for B"}`. Each removed child needs its own entry |
| Ambiguous by nature | Several trait impls, overloads, constructors and partial declarations |
| Rust signatures | Include attached outer `#[...]` attributes, so attribute edits are `signature_changed` |
| Python signatures | Include decorators in source order, so decorator edits are `signature_changed` |

## Literal FIND preconditions

`verify-spec` and `run-task --pre-only` check literal `FIND:` blocks against the
current files before execution.

| Result | Cause |
|---|---|
| `find_block_mismatch` | A complete read lacks the literal |
| `find_block_unavailable` with a reason | Missing marker, invalid path, unreadable or changed file, invalid UTF-8, deadline or budget |

| FIND rule | Behavior |
|---|---|
| Targets | Repository-relative regular files. Absolute, parent, symlink and special-file targets are rejected |
| Limits per check | 1,024 blocks, 16,384 work units, 32 MiB of reads, 5 MiB per file |
| Final check | Every read file is compared again by identity and hash, and a change turns the result into `find_block_unavailable` |

Postflight and `ci` do not need the FIND text to survive the edit. A passing
FIND says nothing about whether the edit happened.

## Lines a gate refuses to read

Only one `##` heading may define each contract section, so `## Scope` and
`## Scope *(MANDATORY)*` together are `duplicate_section`. Headings and
directives inside fenced examples are example text. `verify-spec` reports an
unparsed line as `unparsed_line`, and `audit-spec` as a `broken` finding.

| Input | Result |
|---|---|
| `---` after a BOM or a blank line, or with trailing spaces | `invalid_frontmatter: frontmatter_not_at_start` |
| A count (`N callers`) or a signature outside a list item, or in an item without a backticked name | `snapshot_line_unparsed` |
| A Pre-edit Snapshot heading that is not a top-level `## ` section | `snapshot_heading_unparsed` |
| Several symbols or several counts in one item (`` `a`, `b` — 2 callers ``) | `snapshot_line_unparsed` |
| A count that is not one whole number before `callers` (`callers: 9`, `~40`, `40+`, `1,234`, `3–5`, `two`) | `snapshot_caller_count_unparsed` |
| `signature` followed by an unreadable code span, or by words before the span (`` signature is `sig` ``) | `snapshot_signature_unparsed` |
| An upper-case `FIND` with text after its colon outside a checklist, or with no fence | `find_block_unparsed` |
| An upper-case `VERIFY:` with nothing after it, `VERIFY` without a colon, or several or unclosed code spans | `verify_command_unparsed` |

A snapshot item reads `` - `name` — N callers; signature `sig` ``, the form
`new-spec` writes.

| Snapshot rule | Behavior |
|---|---|
| Count | The whole number nearest before `caller` or `callers` in the same clause. `no callers` reads 0 |
| Clause end | `,`, `;`, `:`, a parenthesis or a dash, but not between digits |
| Not extra symbols | File locations such as `` `src/a.rs:12` ``, the span after `signature`, and spans after the count |
| Name | The first span that is not a file location, so `` - `foo.vue` and `Widget` — 2 callers `` names `Widget`. An item whose only span is a file is `snapshot_line_unparsed` |
| No count or signature | A planner note |
| Snapshot heading | A top-level `##` heading whose words start with `pre edit` and name a snapshot, after an optional number |

| Marker rule | Behavior |
|---|---|
| Where | `FIND`, `VERIFY` and `File` are read in list items, after a checkbox and with a qualifier such as `FIND (exact):` |
| `VERIFY` | On its own line in upper case it declares the text after the colon. In a list item or another case it declares only a backticked command |
| `Find:` | Counts only when a fence follows |
| Indentation | A line four or more columns past the list item's content is an example and gates skip it |

## CI

`mastermind ci` indexes the repository, verifies the selected specs, parses
their executor reports, audits the real diff and can write sealed bundles. A
source that fails to index fails the run, because a partial graph can miss
removed symbols. For pull requests:

```bash
mastermind ci --since origin/main \
  --changed-only --require-executor-report \
  --bundle-dir .mastermind/audit-output
```

| Flag or rule | Behavior |
|---|---|
| No `--changed-only` | Walks all task specs |
| `--changed-only`, `--require-executor-report` or `--bundle-dir` | A checkout without a task contract fails |
| `--bundle-dir` | Always needs a canonical `executor-report.md` |
| Changed-task discovery | A change anywhere under a task folder maps to its spec. Linked directories are not followed |
| Bundle inputs | Path swaps, symlinks and special files are rejected before their digests are recorded |

## Executor report evidence

`run-task --post-only`, `audit-spec` and `ci` read the executor report with these
rules. The format is in the
[structured report contract](../../skills/workflow/mastermind-structured-report-contract/SKILL.md).

| Rule | Behavior |
|---|---|
| Canonical schema-v1 report | Task path, status, phases, modified files, defects and verification excerpts are read, at most 1 MiB |
| `partial` or `failed` status | `executor_report_rejected` and a `broken` audit |
| `spec` | Must name the audited task inside the repository |
| Reported files | Compared both ways with the diff, without `.mastermind/` files. A missing or extra entry makes the audit `broken` |
| Older `mastermind:executor-begin` format | Rejected by `run-task` postflight and by `ci` with `--require-executor-report` or `--bundle-dir` |
| Parsing | Nulls, wrong scalar types and ambiguous sentinel blocks are rejected |

Postflight needs a passing reported result for every nonempty `verify[].cmd`
and every `VERIFY:`, `**VERIFY**:` or `**VERIFY:**` command line. `verify-spec
--strict` needs at least one such command.

### Reported verification

| Report row | Result |
|---|---|
| No row for a required command | `verification_requirement_unmet` with `missing_result` |
| Canonical pass without `observed.exit_code: 0` | Rejected by the parser |
| Older-format pass without an exit receipt | `unobserved_exit` |
| No pass claim, nonzero exit, or zero tests for a recognized test run | `not_passed` |
| Failing and passing rows for one command | `conflicting_results` |
| Claimed pass with a nonzero `observed.exit_code` | `observed_exit_code_non_zero` |
| `observed.tests_run: 0` for a recognized test run | `observed_zero_tests` |

Command matching trims only outer whitespace, so separate rows cannot satisfy a
compound command. A reported zero exit is a self-consistency receipt, not proof
that the command ran. A `verify[].run` declaration also needs a current runner
receipt, see [observed verification](task-runtime.md#observed-verification), and
`acceptance` adds `acceptance_criterion_unmet`.

| Recognized test runs | Notes |
|---|---|
| Direct `cargo test`, `go test`, `pytest`, `python -m pytest`, `python3 -m pytest`, `jest`, `vitest run`, `vitest --run`, also `.exe` names | Plain paths, filters and some options |
| Compile, collection, help, version and watch modes | Not a test run |
| Wrappers, shell expansion, npm or yarn scripts, unsupported flags | Unknown. A zero count alone does not break the audit |

`vacuous_test_claim` is an advisory warning for a recognized test command with
no positive count when a complete scan finds no conventional tests. The scan
covers Cargo `src/` and `tests/`, one Go directory with optional `/...`, and one
pytest directory or file. It stops at 16,384 work units, 8 MiB of reads, 1 MiB
per file and depth 12, and an incomplete scan suppresses the warning.

### Executor claim evidence

`audit-spec --executor-report ... --json` includes ordered `claim_checks`: each
claim, its index, a `verified`, `failed` or `unresolved` status, and evidence or
a finding.

| Claim | Rule |
|---|---|
| `function_added` | Selects one exact declaration, compares the signature, and matches a declaration the diff introduced in that file |
| `integration` with `relation: calls` (default) | Both endpoints resolve uniquely and one compatible indexed call candidate exists across files. The result is a `compatible_call_candidate`, not a runtime call |
| Indistinguishable parents or edited overload sets | `executor_claim_unresolved` |
| Existing declaration claimed as added | `claimed_symbol_not_added` |

Stale files, incomplete diffs, index changes and exceeded budgets stay
unresolved and make the audit `broken`. A bundle binds the whole checked report,
so the same claims with different completion data give a broken bundle.
