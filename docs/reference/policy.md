# Policy as code

`mastermind policy check --since REF` checks a change against architecture rules
that the repository keeps in `mastermind-policy.yml`. It exits nonzero for a
violation or for incomplete required evidence, so it can gate a pull request.
The evaluator runs no Rego, OPA daemon, network call or general-purpose runtime.

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
| File | `mastermind-policy.yml`, or `--config PATH` inside the repository, at most 1 MiB. It is checked again by digest and identity after evidence collection |
| Schema | Closed. Unknown keys, duplicate IDs, invalid globs, more than one action per rule, booleans that do nothing and more than 100 rules are errors |
| Paths | Repository-relative globs with `/` separators |

## What each rule checks

Each rule looks at the change from the baseline to the working tree.

| Rule key | Passes when |
|---|---|
| `deny_imports` | No changed source file has an import edge into the denied paths |
| `max_new_cycles` | The change adds no more new dependency cycles than the limit, compared with the exact baseline Git blobs |
| `when: api_surface_changed` with `require_owner` | Every changed symbol used from another top-level component sits in a file whose CODEOWNERS include the required owner. This is observed use, not a `public` or `export` declaration |
| `max_blast_radius` | Unique impacted symbols with an in-scope changed seed stay within the limit |
| `require_tests` | A graph-linked candidate test or a test file exists inside the configured scope. It does not show that tests ran |
| `deny_ownership_crossings` | CODEOWNER sets match on both sides of each component crossing. Missing ownership makes the rule incomplete |
| `require_workflow: strict` | A canonical strict `spec.md`, matching `state.json`, held `audit.md` and an exact touched-file entry exist for the critical files |

## Strict workflow evidence

The held state carries a SHA-256 snapshot of every declared touch over the
baseline commit, paths, file bytes, missing files and Git executable modes. So
editing a critical file after the audit invalidates the evidence. Run
post-flight again on the final files to get a current snapshot.

| Situation | Effect on the snapshot |
|---|---|
| Staging or committing the same files, also in a separate CI checkout | Still valid |
| Line-ending conversion or filters that change bytes | Needs another audit |
| `core.filemode=false` | The Git index supplies executable modes. Untracked files count as `100644` |
| Symlinks, Git symlink placeholders and submodules | Not supported |
| Unsupported `held_snapshot_version` | Fails |

| Evidence input | Rule |
|---|---|
| Default directory | `.mastermind/tasks` |
| In CI | Restore the task artifacts, or point `--workflow-evidence PATH` at a directory with `<task-id>/{spec.md,state.json,audit.md}`. Action `*.bundle.json` files are a different format |
| Unreadable artifact | Makes evaluation incomplete, even when another task covers the same file |
| CODEOWNERS | GitHub priority order, or `--codeowners PATH` |

Workflow evidence checks that local artifacts are consistent. It does not verify
an audit signature, a GitHub approval or a remote CI run. Policy uses the
syntactic graph only, and SCIP or runtime overlays do not change its edges.

## Limits and output

| Limit | Value |
|---|---|
| Impact depth / top | 3 / 500 by default. `--depth 1..5`, `--top 1..500` |
| Import and cycle edges | 50,000 |
| Baseline cycle comparison | 500 files, 32 MiB |
| Workflow evidence | 1,000 directory entries, 1 MiB per artifact, 32 MiB per read pass |
| Strict snapshot | 1,000 touch files, 16 MiB each, and the task's [snapshot budget](task-runtime.md#project-settings) in total |
| Results | 1,000, one of them reserved for the incomplete-evaluation diagnostic |

Any relevant cap, a stale index, a snapshot change or unreadable required
evidence exits nonzero.

| Format | Content |
|---|---|
| `text` (default) | Baseline and head OIDs, config path and SHA-256, every evaluated rule, locations and incomplete-evidence details |
| `json` | `complete`, diagnostics, both OIDs, config path and SHA-256 |
| `sarif` | Each rule ID as `ruleId` with `partialFingerprints`. Incomplete evaluation emits `mastermind/policy-evaluation-incomplete` at the config location |
