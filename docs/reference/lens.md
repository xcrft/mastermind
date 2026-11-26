# Lens and review export

Lens is a local browser view of one change, from a Git baseline to the working
tree. `mastermind ui` serves it on loopback, and `mastermind review export`
writes the same snapshot as a portable package for a pull request. Lens has no
analysis engine of its own: `/api/lens` wraps the schema-v1 map and change-impact
responses with their truncation, precision and work-limit notes.

```bash
mastermind index .
mastermind ui --since main
mastermind ui --since main --path src --depth 2 --top 50 --production-only \
  --sarif semgrep.sarif --coverage lcov.info --junit junit.xml --otel traces.json
mastermind review export --since main --out mastermind-review
```

Run `mastermind index .` first. Lens reports a missing or stale index and never
creates or updates one.

## Open Lens (`mastermind ui`)

| Option | Contract |
|---|---|
| `--since REF` | Required baseline |
| `--path`, `--depth 1..5`, `--top 1..100`, `--production-only` | Bound the initial review |
| Port | `0` by default, so the OS picks a free port. The server binds only to `127.0.0.1` |

| Server rule | Behavior |
|---|---|
| Requests | Same-origin `GET` and `HEAD` only. No route reads arbitrary files or writes |
| Assets | Embedded and offline, under a restrictive content-security policy |
| Index | Opened query-only. A checkpointed index opens as immutable. An active WAL is copied with the database into a private snapshot through the SQLite backup API, at most 2 GiB and 60 seconds or the shorter request deadline. SQLite may create or update the SHM file |
| Refresh | Fails when the repository, index, WAL, baseline or work snapshot changes, or indexed files disappear. The final check runs after evidence, audit and the optional document graph |

| Changed-file field | Meaning |
|---|---|
| `returned` | Items shown. Lens sends at most 200 changed-file records |
| `observed` | The impact engine's count or lower bound, up to its 10,000-file cap |
| `projection_truncated`, `projection_reason: lens_payload_limit` | The 200-record transport cap applied. Impact, SARIF, summaries and manifests still use the full bounded collection |
| `truncation_reason: non_utf8_path` | A changed path could not be shown as UTF-8. It is omitted and counted in a `non_utf8_changed_paths_skipped:N` precision note. When the file cap also applies, `file_limit` stays the reason and the note keeps the separate count |

The Profiles tab reads personal context from a separate local route. Its
contract is in [private Lens view](persona-context.md#private-lens-view).

## Selected-scope audit

The audit view projects the selected map scope, not a whole-repository scan.
Component structure, dead-code candidates, centrality, largest files by line
span, Git churn and authorship apply `--path` and `--production-only` before
their caps, and the browser does not filter capped results again.

| Collection | Cap and coverage |
|---|---|
| Dead-code candidates | 100 symbols. `total` stays exact. Keeps static-graph `precision_notes` and per-symbol edge precision. A candidate is never proof that code is dead |
| Largest files | 20. `total` becomes unknown when a probe finds more |
| Change hotspots | Ranking capped at 20. `total` is unknown when the 2,000-row centrality window fills, because churn can change the order |
| Bus factor | Every returned map component over a 2,000-commit window, zero-history components included. A judgment needs five touches. A readable history with any component below that sets `partial` with `components_below_minimum_history` |
| Churn and ownership | Read with Git's NUL-delimited paths, so unusual paths are kept. Identities use mailmap name and email pairs |

Each collection reports `total`, `returned`, `truncated` and
`truncation_reason` in the method ledger. Overview counts show exact totals when
known, a lower bound for a partial non-empty collection, and `?` for an empty
collection with an unknown total. An unavailable SQLite or Git source returns a
null total with its reason, so a failed query never shows as `Healthy` or
`Clear`. The `Widest returned reach` callout ranks only what the returned trace
holds.

Static findings in the security card stay change-scoped, because overlays
correlate only to the returned trace. Severity is kept: errors and warnings rank
ahead of notes. The overall verdict is `Incomplete` when SCIP evidence is
partial, temporal analysis is unavailable or partial, or a selected document
graph needs review.

## Optional AI audit narrative

Lens can show a model-written interpretation from a sidecar file without
calling a model.

1. Read `audit.narrative_binding` from the current `/api/lens` response.
2. Write `.mastermind/audit-narrative.json` with that object unchanged in its
   required `binding` field. Validate it against
   [`mastermind-audit-narrative-v1`](../../schemas/mastermind-audit-narrative-v1.schema.json).
3. Reload Lens.

| Rule | Behavior |
|---|---|
| Path | `MMCG_AUDIT_NARRATIVE` overrides the default. A relative path is repository-relative, an absolute one must resolve inside the repository |
| Read | No-follow, at most 256 KiB, under the request deadline |
| Binding | Covers repository identity, baseline and HEAD, the working-tree snapshot and the exact returned map. A stale or foreign binding is rejected and Lens shows facts only |
| Repository identity unavailable | No binding is emitted and the snapshot stops with `repository_identity_unavailable`. A Git timeout stays `analysis_timeout` |
| Components and red-team routes | Only exact component paths from the bound map |
| Default sidecar | Runtime state, so writing it does not invalidate the snapshot it describes |

`audit.narrative_state` is `absent`, `available`, `partial`, `rejected` or
`unavailable`, with a stable `reason`. Lens labels narrative prose as AI
interpretation and routes as claims to verify. Neither is topology, runtime
evidence or a confirmed vulnerability. Review exports keep non-ready states in
their partial-state list.

## Evidence overlays

Lens can correlate the returned change, impact and candidate-test trace with
read-only evidence:

| Input | Flag | Rule |
|---|---|---|
| SARIF 2.1 findings | `--sarif PATH`, repeatable | |
| Coverage | `--coverage PATH`, repeatable | LCOV or Cobertura, detected automatically. Duplicate lines merge by maximum hit |
| JUnit XML | `--junit PATH`, repeatable | Only explicit testcase `file` attributes. A suite with fewer cases than its `tests` total, or failures without a failed case, is partial with `junit_totals_mismatch` |
| OTLP JSON traces | `--otel PATH`, repeatable | Only `code.file.path` or `code.filepath` span attributes |
| Project history | On by default, `--no-project-knowledge` turns it off | Exact path mentions in indexed specs, reports, audits, lessons, context, release notes and ADRs under `docs/adr`, `docs/adrs`, `docs/decisions`, `adr`, `adrs` or `.mastermind/decisions`. Changed documents give `project_history_stale`, incomplete admission `project_history_incomplete` |
| Document graph | `--document-graph PATH` | One packet under `.mastermind/research`. See [document graph](#document-graph) |
| CODEOWNERS | `--codeowners PATH` overrides discovery | `.github/CODEOWNERS`, root `CODEOWNERS`, then `docs/CODEOWNERS` |
| Git churn and contributors | `--git-commits 0..1000`, default 200, `0` turns it off | Text line totals, binary-change counts and line-count completeness are separate |
| SCIP overlay | None | Used when present and fresh. Import with `mastermind enrich --scip` |
| Declarative facts | None | Used when present. Import with `mastermind enrich --facts`. Stale bindings are omitted with a diagnostic |

Evidence matches only files already in the trace. When the changed-file list is
already truncated, symbol, impact and candidate-test paths go first, then at most
200 file-only paths, and the result reports `relevant_file_limit`.

| Overlay limit | Value |
|---|---:|
| One source file | 32 MiB |
| Findings | 5,000 total, 100 per file |
| Coverage | 500,000 unique lines |
| JUnit | 100,000 cases, 1,000 returned failure details |
| OTLP | 100,000 matched spans, 1,000 file pairs |
| Project-history matches | 500 |
| Artifact inputs | 64 combined |
| CODEOWNERS | Under GitHub's 3 MiB, 50,000 rules, 50 owners per rule. Discovery has a 16,384-entry directory budget |
| Contributor names | 5 recent names per file |
| Git output | 8 MiB |
| Diagnostics | 100 |

A source that is missing, changes during the read, goes over a limit, has invalid
syntax or misses the deadline is reported as partial or error, never as clean.
Exact totals show when known, lower bounds for partial lists, and `?` for an
empty list with an unknown total.

| Matching rule | Behavior |
|---|---|
| Explicit evidence paths | Resolved once, then read through a no-follow parent. Only bounded regular files. Swaps and special files fail |
| Source labels | Exact UTF-8, canonical slash paths, at most 180 characters. A bad label makes Lens evidence partial and makes review export fail |
| Artifact paths | Repository-relative paths match exactly. Reports from another build root may match by a unique path suffix. Windows drive and UNC paths stay portable. A relative backslash path on Unix is omitted and makes the source partial |
| Git paths | Must already be canonical slash paths. Binary changes have no line counts and mark the file's line-count completeness false. Invalid records make the source partial. History is pinned to the snapshot HEAD and does not follow renames |
| CODEOWNERS | Working-tree file with last-match-wins and no-owner rules. Unreadable or changing locations make evidence partial and temporal ownership totals unknown. Lens does not check GitHub accounts, teams or permissions, and GitHub assigns reviewers from the base branch file |

Exact SCIP endpoint pairs replace Tree-sitter as static provenance without
changing the graph. Runtime file pairs may decorate an existing static edge in
either direction but never create a node or edge. Overlay switches change
emphasis and inspector detail only. Lens writes none of this evidence to source
files or SQLite. Lens alone cannot show that a report was produced from the
current revision. A review package binds the report bytes to HEAD, and an
optional attestation records the producer's claim.

### Document graph

The [project-history skill](../../skills/workflow/mastermind-project-history/SKILL.md)
ships the producer and a standalone checker for the packet. Lens reads only a
packet inside `.mastermind/research`, checks its named endpoints and optional
Markdown corpus again, and shows a separate relation review queue.

| State | Meaning |
|---|---|
| `needs_review` | Partial evidence |
| `current` | The named bytes match. Unrelated changes are not covered |
| `corpus: not_tracked` | An endpoint-only packet. New or unlisted documents stay outside its freshness scope, and Lens says so in the summary, card, notices, ledger and announcement |
| Every relation | `unverified`. Labels and unchanged hashes do not show that a relation is right |

A packet Git revision that differs from the review head is shown apart from
content freshness. Lens shows the selected corpus roots beside document
freshness.

## PR evidence package (`mastermind review export`)

`mastermind review export --since REF --out DIR` captures the same Lens snapshot
without an HTTP server. `DIR` must not exist yet and needs an exact UTF-8 path.
The exporter builds the package in a private sibling directory and renames it
only after every file is synced, so a failed run publishes nothing.

| File | Content |
|---|---|
| `index.html` | One self-contained Lens document with snapshot, CSS and JS under a hash-only CSP. No fetch, CDN, telemetry or write path. It omits the document graph's local root and keeps its repository-relative label, path and bindings |
| `mastermind.sarif` | Map and change-impact SARIF as two separately identified runs |
| `summary.md` | A short reviewer summary that links the HTML and SARIF |
| `manifest.json` | Strict schema-v1 repository, scope, evidence, payload digest and partial-state bindings. With a document graph, also the tracked corpus roots and every endpoint and corpus change with its reason |
| `mastermind-review.yml` | The pinned [GitHub Actions example](../examples/mastermind-review-pr.yml) for artifact and SARIF upload |

Export takes the same `--path`, `--depth`, `--top`, `--production-only`,
`--sarif`, `--coverage`, `--junit`, `--otel`, `--codeowners`, `--git-commits`,
`--no-project-knowledge` and `--document-graph` inputs as Lens, plus
`--evidence-attestation`.

| Export check | Behavior |
|---|---|
| External report files | Read before and after analysis and checked again after the package is synced. The manifest records their SHA-256 next to the head OID. This proves which bytes a reviewer saw, not that Semgrep, CodeQL, a test runner or a collector produced them from that revision |
| Final boundary | Before the rename, the exporter checks Git HEAD, the worktree projection, the history inventory, the root and the database and WAL identity again, then rereads every payload byte. Any race removes the staging directory |
| Staging inside the repository | The worktree check leaves out only that new private directory |
| CODEOWNERS | The discovered path is fixed before analysis and checked again before publication. A file that appears, disappears or moves aborts the package |
| Document graph | The manifest binds the packet digest, its snapshot digest, the live observation digest, snapshot Git metadata, whether that head matches the review head, change counts, corpus status and relation count. `needs_review` sets `analysis.partial`. `--out` under a tracked corpus directory is rejected, because the new `summary.md` would make the package stale |
| Dynamic text in `summary.md` | Repository names escape Markdown punctuation. Refs, scopes and paths use code spans longer than any backtick run inside them |

Datasets imported with `mastermind enrich --facts` need no extra attestation,
because import already checked identity, digests and the declared revision.

| Facts state | Label |
|---|---|
| Unsigned | `producer-attested` |
| Imported through an allowlisted Ed25519 key | `producer-signed`, with key ID, public key, signature, signature digest and signed-manifest digest |
| Mixed | `partially-producer-signed` |
| Stale, invalid proof or truncated | Partial, never promoted to a revision-bound claim |

For a producer claim about other reports, pass `--evidence-attestation PATH`:

```json
{
  "schema_version": 1,
  "head_oid": "0123456789abcdef0123456789abcdef01234567",
  "artifacts": [
    {
      "kind": "sarif",
      "path": "reports/semgrep.sarif",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  ]
}
```

Every path is canonical and repository-relative, every digest must match a
requested input, and `head_oid` must match the snapshot. Unknown or duplicate
fields and any drift fail. The package records this file as unsigned CI
evidence, so authenticate its producer separately, for example with a signed
workflow or artifact attestation. The schemas are
[`mastermind-review-manifest-v1`](../../schemas/mastermind-review-manifest-v1.schema.json)
and [`mastermind-evidence-attestation-v1`](../../schemas/mastermind-evidence-attestation-v1.schema.json).

The example workflow builds the checked-out binary when it runs in the
Mastermind repository, so a new command in a pull request is tested before
release. In other repositories it installs the exact npm version in the
template and checks `review export` before indexing. Analysis runs with a
read-only token, and a separate job with `security-events: write` uploads the
SARIF.

## SARIF export

| Command | Rule ID | Results |
|---|---|---|
| `mastermind map --format sarif` | `mastermind/dependency-cycle` | Returned dependency cycles |
| `mastermind impact --format sarif` | `mastermind/component-boundary-change` | Returned cross-component impacts. The primary location is the changed symbol and the impacted symbol is a related location |
| `mastermind policy check --format sarif` | The configured rule IDs | See [policy as code](policy.md) |

Both map and impact output are SARIF 2.1.0 with repository-relative,
percent-encoded URIs, the Mastermind version and run properties for scope,
baseline and head identity, and partial state. A truncated query is never
reported as complete. Impact SARIF takes its partial state and reasons from
changed files, changed symbols, affected components, impacted symbols, API
crossings and candidate tests, and keeps returned and total counts for all six,
with null totals where a work limit blocked an exact count.

Upload with `github/codeql-action/upload-sarif`, so GitHub fills in missing
fingerprints. Rule IDs and relative paths stay stable across runs. See
[GitHub's SARIF support](https://docs.github.com/en/code-security/reference/code-scanning/sarif-files/sarif-support)
for ingestion limits.
