# Lens and review export

Lens is a local browser view of one change, from a Git baseline to the working
tree. `mastermind ui` serves it on loopback, and `mastermind review export`
writes the same snapshot as a portable package for a pull request. Lens has no
analysis engine of its own. `/api/lens` wraps the schema-v1 map and
change-impact results with their truncation and precision notes.

```bash
mastermind ui --since main
mastermind ui --since main --path src --depth 2 --top 50 --production-only \
  --sarif semgrep.sarif --coverage lcov.info --junit junit.xml --otel traces.json
mastermind review export --since main --out mastermind-review
```

Run `mastermind index .` first. Lens reports a missing or stale index and never
creates or updates one.

## Open Lens (`mastermind ui`)

| Option or rule | Contract |
|---|---|
| `--since REF` | Required baseline |
| `--path`, `--depth 1..5`, `--top 1..100`, `--production-only` | Bound the initial review |
| Port | `0` by default, so the OS picks one. The server binds only to `127.0.0.1` |
| Requests | Same-origin `GET` and `HEAD` only. No route reads arbitrary files or writes |
| Assets | Embedded and offline, under a restrictive content-security policy |
| Index | Opened query-only. An active WAL is copied with the database into a private snapshot, at most 2 GiB and 60 seconds |
| Refresh | Fails when the repository, index, baseline or worktree changes during the read |

| Changed-file field | Meaning |
|---|---|
| `returned` | Items shown. Lens sends at most 200 changed-file records |
| `observed` | The impact engine's count or lower bound, up to its 10,000-file cap |
| `projection_truncated` with `projection_reason: lens_payload_limit` | The 200-record cap applied. Impact, SARIF and manifests still use the full collection |
| `truncation_reason: non_utf8_path` | A changed path could not be shown and is counted in a `non_utf8_changed_paths_skipped:N` note |

The Profiles tab reads personal context from a separate route, described in
[private Lens view](persona.md#private-lens-view).

## Selected-scope audit

The audit view covers the selected `--path` and `--production-only` scope, not
the whole repository.

| Collection | Cap and coverage |
|---|---|
| Dead-code candidates | 100 symbols with exact `total` and per-symbol edge precision. A candidate is never proof that code is dead |
| Largest files | 20 by line span |
| Change hotspots | 20. `total` is unknown when the 2,000-row centrality window fills |
| Bus factor | Every map component over a 2,000-commit window. A judgment needs 5 touches, and fewer sets `partial` with `components_below_minimum_history` |
| Churn and ownership | Git's NUL-delimited paths, identities by mailmap name and email |

Each collection reports `total`, `returned`, `truncated` and
`truncation_reason`. Counts show the exact total when known, a lower bound for a
partial list, and `?` for an empty list with an unknown total. A failed SQLite or
Git source gives a null total with its reason, so it never shows as `Healthy` or
`Clear`. The overall verdict is `Incomplete` when SCIP evidence is partial,
temporal analysis is unavailable or partial, or a document graph needs review.

## Optional AI audit narrative

Lens can show a model-written interpretation from a sidecar file. It never
calls a model itself.

1. Read `audit.narrative_binding` from the current `/api/lens` response.
2. Write `.mastermind/audit-narrative.json` with that object unchanged in its
   `binding` field. Validate it against
   [`mastermind-audit-narrative-v1`](../../schemas/mastermind-audit-narrative-v1.schema.json).
3. Reload Lens.

| Rule | Behavior |
|---|---|
| Path | `MMCG_AUDIT_NARRATIVE` overrides the default. The path must resolve inside the repository |
| Read | No-follow, at most 256 KiB |
| Binding | Repository identity, baseline and HEAD, the worktree snapshot and the exact returned map. A stale or foreign binding is rejected and Lens shows facts only |
| Repository identity unavailable | No binding, and the snapshot stops with `repository_identity_unavailable` |
| Components and red-team routes | Only exact component paths from the bound map |

`audit.narrative_state` is `absent`, `available`, `partial`, `rejected` or
`unavailable`, with a stable `reason`. Lens labels the prose as AI interpretation
and the routes as claims to verify.

## Evidence overlays

Lens can correlate the returned change, impact and test trace with read-only
evidence. Evidence matches only files already in the trace. Lens writes none of
it to source files or SQLite.

| Input | Flag | Rule |
|---|---|---|
| SARIF 2.1 findings | `--sarif PATH`, repeatable | Errors and warnings rank ahead of notes |
| Coverage | `--coverage PATH`, repeatable | LCOV or Cobertura, detected automatically |
| JUnit XML | `--junit PATH`, repeatable | Only testcase `file` attributes. Mismatched totals give `junit_totals_mismatch` |
| OTLP JSON traces | `--otel PATH`, repeatable | Only `code.file.path` or `code.filepath` span attributes. Runtime pairs decorate an existing static edge and never create one |
| Project history | On by default, `--no-project-knowledge` turns it off | Exact path mentions in indexed project history. Changed documents give `project_history_stale` |
| Document graph | `--document-graph PATH` | One packet under `.mastermind/research`. See [document graph](#document-graph) |
| CODEOWNERS | `--codeowners PATH` overrides discovery | `.github/CODEOWNERS`, root `CODEOWNERS`, then `docs/CODEOWNERS`, last match wins. GitHub accounts and teams are not checked |
| Git churn and contributors | `--git-commits 0..1000`, default 200, `0` turns it off | History is pinned to HEAD and does not follow renames |
| SCIP overlay, declarative facts | None | Used when present and fresh. Import with `mastermind enrich` |

| Overlay limit | Value |
|---|---:|
| One source file / artifact inputs | 32 MiB / 64 combined |
| Findings / coverage | 5,000 total and 100 per file / 500,000 unique lines |
| JUnit / OTLP | 100,000 cases and 1,000 returned failures / 100,000 spans and 1,000 file pairs |
| CODEOWNERS | 3 MiB, 50,000 rules, 50 owners per rule |
| Project-history matches / contributor names per file / diagnostics | 500 / 5 / 100 |

A source that is missing, changes during the read, goes over a limit or has
invalid syntax is reported as partial or error, never as clean. A source label
must be an exact UTF-8 slash path of at most 180 characters. Report paths from
another build root may match by a unique path suffix.

### Document graph

The [project-history skill](../../skills/workflow/mastermind-project-history/SKILL.md)
ships the packet producer and checker. Lens reads only a packet inside
`.mastermind/research`, checks its endpoints again, and shows a separate relation
review queue.

| State | Meaning |
|---|---|
| `needs_review` | Partial evidence |
| `current` | The named bytes match. Unrelated changes are not covered |
| `corpus: not_tracked` | An endpoint-only packet. New documents stay outside its freshness scope |
| Every relation | `unverified` |

## PR evidence package (`mastermind review export`)

`mastermind review export --since REF --out DIR` captures the same snapshot
without an HTTP server. `DIR` must not exist yet. The exporter builds the
package in a private sibling directory and renames it only after every file is
synced, so a failed run publishes nothing. Export takes the same inputs as Lens,
plus `--evidence-attestation`.

| File | Content |
|---|---|
| `index.html` | One self-contained Lens document under a hash-only CSP, with no fetch, CDN or telemetry |
| `mastermind.sarif` | Map and change-impact SARIF as two runs |
| `summary.md` | A short reviewer summary linking the HTML and SARIF |
| `manifest.json` | Repository, scope, evidence, payload digest and partial-state bindings |
| `mastermind-review.yml` | The pinned [GitHub Actions example](../examples/mastermind-review-pr.yml) |

| Export check | Behavior |
|---|---|
| External report files | Read before and after analysis. The manifest records their SHA-256 next to the head OID. This shows which bytes a reviewer saw, not who produced them |
| Final boundary | Before the rename, HEAD, the worktree, the history inventory and the index are checked again. Any race removes the staging directory |
| CODEOWNERS | A file that appears, disappears or moves during the run aborts it |
| Document graph | The manifest binds the packet and its status. `needs_review` sets `analysis.partial`. `--out` under a tracked corpus directory is rejected |

| Facts state | Label |
|---|---|
| Unsigned | `producer-attested` |
| Imported through an allowlisted Ed25519 key | `producer-signed`, with key ID and signature digests |
| Mixed | `partially-producer-signed` |
| Stale, invalid proof or truncated | Partial |

For a producer claim about other reports, pass `--evidence-attestation PATH`:

```json
{"schema_version": 1, "head_oid": "0123456789abcdef0123456789abcdef01234567",
 "artifacts": [{"kind": "sarif", "path": "reports/semgrep.sarif",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}
```

Every path is repository-relative, every digest must match a requested input,
and `head_oid` must match the snapshot. Unknown or duplicate fields fail. The
package records the file as unsigned CI evidence, so authenticate its producer
separately. The schemas are
[`mastermind-review-manifest-v1`](../../schemas/mastermind-review-manifest-v1.schema.json)
and [`mastermind-evidence-attestation-v1`](../../schemas/mastermind-evidence-attestation-v1.schema.json).

## SARIF export

| Command | Rule ID | Results |
|---|---|---|
| `mastermind map --format sarif` | `mastermind/dependency-cycle` | Returned dependency cycles |
| `mastermind impact --format sarif` | `mastermind/component-boundary-change` | Returned cross-component impacts. The changed symbol is the primary location and the impacted symbol is related |
| `mastermind policy check --format sarif` | The configured rule IDs | See [policy as code](policy.md) |

Output is SARIF 2.1.0 with repository-relative, percent-encoded URIs and run
properties for scope, baseline, head and partial state. A truncated query is
never reported as complete. Upload it with `github/codeql-action/upload-sarif`
so GitHub fills in fingerprints. See
[GitHub's SARIF support](https://docs.github.com/en/code-security/reference/code-scanning/sarif-files/sarif-support)
for ingestion limits.
