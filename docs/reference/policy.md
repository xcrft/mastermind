# Policy as code

`mastermind policy check --since REF` checks a change against architecture rules
that the repository owns in `mastermind-policy.yml`. It exits nonzero for a
violation or for incomplete required evidence, so it can gate a pull request.
The evaluator is deterministic and runs no Rego, OPA daemon, network call or
general-purpose runtime.

```bash
mastermind index .
mastermind policy check --since main
mastermind policy check --since main --format sarif > mastermind-policy.sarif
```

[`docs/examples/mastermind-policy.yml`](../examples/mastermind-policy.yml) shows
all seven rule forms.

## Write the config

| Rule | Contract |
|---|---|
| File | `mastermind-policy.yml` by default, or `--config PATH` inside the repository. A symlink that resolves outside is rejected |
| Read | A bounded regular file through a repository capability, checked again by path, digest and file identity after evidence collection |
| Schema | Closed. Unknown keys, duplicate IDs, invalid globs, more than one action per rule, booleans that do nothing and more than 100 rules are errors |
| Paths | Repository-relative glob patterns with `/` separators |

## What each rule checks

Evaluation looks at the baseline-to-working-tree change:

| Rule key | Passes when |
|---|---|
| `deny_imports` | No changed source file has a current import edge into the denied paths |
| `max_new_cycles` | The change adds no more new dependency cycles than the limit. Current cycle membership is compared with the exact baseline Git blobs, and the baseline worktree is never checked out |
| `when: api_surface_changed` with `require_owner` | Every changed symbol that reaches another inferred top-level component sits in a file whose CODEOWNERS include the required owner. This is observed cross-component use, not a `public` or `export` declaration |
| `max_blast_radius` | Unique returned impacted symbols whose changed seed is in scope stay within the limit |
| `require_tests` | A graph-linked candidate test or a test file exists inside the configured scope. A test elsewhere under the same top-level directory does not count. The rule does not show that tests ran or passed |
| `deny_ownership_crossings` | CODEOWNER sets match on both sides of each observed component crossing. Missing ownership makes the rule incomplete |
| `require_workflow: strict` | A canonical strict `spec.md`, matching baseline `state.json`, held `audit.md` and an exact touched-file entry exist for the critical files |

## Strict workflow evidence

The held state carries a SHA-256 snapshot of every declared touch, so editing a
critical file after the audit invalidates the evidence. The digest binds the
baseline commit, normalized touch paths, file bytes, missing files and Git
executable modes.

| Situation | Effect on the snapshot |
|---|---|
| Staging or committing the same files, also in a separate CI checkout | Still valid |
| Line-ending conversion or clean and smudge filters that change bytes | Needs another audit |
| `core.filemode=false` | The Git index supplies executable modes |
| Untracked files | Count as `100644`, as in Git |
| Symlinks, Git symlink placeholders and submodules | Not supported as regular-file evidence |
| Unsupported `held_snapshot_version` | Fails |

Task state with an older snapshot version is handled in
[task runtime](task-runtime.md). Run post-flight again on the final files to
get a current snapshot.

| Evidence input | Rule |
|---|---|
| Default directory | `.mastermind/tasks` |
| In CI | Restore the canonical task artifacts, or point `--workflow-evidence PATH` at the downloaded directory, also outside the checkout. Layout: `<directory>/<task-id>/{spec.md,state.json,audit.md}`. Action `*.bundle.json` files are a different format |
| Reads | One directory capability, no symlink components below it, the whole read set checked again after repository and index validation |
| Unreadable artifact | Makes evaluation incomplete, even when another task covers the same file |
| CODEOWNERS | GitHub priority order, or `--codeowners PATH` |

Workflow evidence checks that local artifacts are consistent. It does not
verify an audit signature, a GitHub approval or a remote CI run. Policy topology
is the default syntactic graph. SCIP and runtime overlays do not add or remove
policy edges.

## Limits and output

| Limit | Value |
|---|---|
| Impact depth / top | 3 / 500 by default. `--depth 1..5`, `--top 1..500` |
| Import and cycle edges | 50,000 |
| Baseline cycle comparison | 500 files, 32 MiB |
| Workflow evidence | 1,000 directory entries, 1 MiB per artifact, 32 MiB per read pass, unrelated specs included |
| Strict snapshot | 1,000 touch files, 16 MiB each, and the task's pinned snapshot budget in total (32 MiB unless `init --snapshot-max-total-bytes` sets another) |
| Results | 1,000, with one reserved for the incomplete-evaluation diagnostic |

Any relevant cap, a stale index, a snapshot change or unreadable required
evidence exits nonzero.

| Format | Content |
|---|---|
| `text` (default) | Baseline and head OIDs, config path and SHA-256, every evaluated rule, locations, result properties, incomplete-evidence details and precision notes. Repository text is terminal-escaped |
| `json` | `complete`, diagnostics, both OIDs, config path and SHA-256 |
| `sarif` | Each rule ID as `ruleId`, with `partialFingerprints` so alerts keep identity when lines move. Incomplete evaluation emits `mastermind/policy-evaluation-incomplete` at the config location |
