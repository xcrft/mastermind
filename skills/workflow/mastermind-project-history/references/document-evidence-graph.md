# Document evidence graph

Use this helper when a research packet needs to preserve explicit links between
documents and code across edits. It records a bounded set of declared relations
and checks whether their source files have changed. Optional directory tracking
also detects new or changed uncited Markdown documents. It does not infer links,
approve decisions, run tests, or import document nodes into the codegraph.

## Declare the relations

Read the relevant source passages first. Write a JSON manifest with these exact
fields and one-based source lines:

```json
{
  "schema_version": 1,
  "relations": [
    {
      "from": {"path": "docs/adr/002-session-expiry.md", "line": 8},
      "relation": "constrains",
      "to": {"path": "src/sessions.py", "line": 23}
    },
    {
      "from": {"path": "docs/adr/002-session-expiry.md", "line": 12},
      "relation": "supersedes",
      "to": {"path": "docs/adr/001-session-storage.md", "line": 4}
    }
  ]
}
```

Paths must be canonical repository-relative POSIX paths. The `from` endpoint
must be Markdown. The `to` endpoint can be documentation, code, a test, or a
text evidence artifact. The source lines must exist. Unknown fields, duplicate
relations, malformed types, and unsafe paths are errors.

Supported relation names are:

| Relation | What the caller is declaring |
|---|---|
| `documents` | The document describes the target. |
| `constrains` | The document states a constraint relevant to the target. |
| `supports` | The document offers evidence for the target claim. |
| `contradicts` | The document conflicts with the target claim or behavior. |
| `supersedes` | The source document explicitly claims to replace the target. |
| `verified_by` | The source document names the target as verification evidence. |
| `mentions` | The document mentions the target, without a stronger claim. |

All relations have `verification: unverified`, including `verified_by`.
Read status and acceptance evidence before treating a superseding proposal as
an active decision. A matching filename or phrase alone supports only a mention.

## Snapshot and check

Requires Python 3.11+, Git, and POSIX no-follow directory-descriptor support
(macOS or Linux). Unsupported platforms fail closed. No additional Python
packages, mmcg binary, model, or network service are needed.

Use the installed skill directory, or
`skills/workflow/mastermind-project-history` in a repository checkout:

```bash
SKILL_DIR=/path/to/mastermind-project-history
REPO_DIR=/path/to/repository

python3 "$SKILL_DIR/scripts/document_graph.py" snapshot \
  --root "$REPO_DIR" \
  --relations .mastermind/research/session-relations.json \
  --corpus-dir docs/adr \
  --corpus-dir .mastermind/decisions \
  --output .mastermind/research/session-evidence-v2.json

python3 "$SKILL_DIR/scripts/document_graph.py" check \
  --root "$REPO_DIR" \
  --graph .mastermind/research/session-evidence-v2.json
```

When the native `mastermind` binary is available, the same saved packet can be
checked beside a history query:

```bash
mastermind history "session expiry" \
  --document-graph .mastermind/research/session-evidence-v2.json

mastermind query history "session expiry" \
  --document-graph .mastermind/research/session-evidence-v2.json
```

The MCP equivalent is `mmcg_history` with the optional `document_graph` string
argument. Without that argument, all three entry points retain the existing
history-only response. With it, the normal history fields stay at the top level
and a separate `document_graph` object contains the live check.

Choose existing directories relevant to the question. Each `--corpus-dir`
recursively tracks non-hidden `.md` and `.markdown` files (case insensitive),
including uncited documents. Hidden descendants and regular non-Markdown files
are excluded. Repeated or overlapping roots, including physical directory
aliases, are rejected. Omit these options to retain endpoint-only tracking.

The declared repository must be a Git worktree root with a HEAD commit. Snapshot
writes a **new** file under that root's `.mastermind/research/`; it will not
overwrite an existing file, a source endpoint, or the input manifest. Check is
read-only. Neither command changes the Git index or project history.
With corpus tracking, Markdown output filenames are rejected; use JSON.
A JSON output within a watched directory does not add itself to the corpus.

Without corpus tracking the snapshot retains `schema_version: 1`, kind
`mastermind_document_evidence_graph`, a canonical local `root`,
`revision: {head, dirty}`, a sorted `files` inventory, `edges`, and `sha256`.
Each file records `path`, `sha256`, `bytes`, and `lines`. Each edge records its
deterministic `id`, original endpoints, relation, and `verification: unverified`.
The snapshot contains no full source contents. Its hash checks internal
consistency; it is not a signature or a semantic verdict.

With corpus tracking, the snapshot has `schema_version: 2` and additionally
`corpus: {directories, files}`. Directories and file records are sorted; each
corpus file uses the same `path`, `sha256`, `bytes`, and `lines` fields.
Endpoint and corpus inventories share a capture, so a cited document in a
watched directory has the same record in both inventories. Only file records
are persisted; a directory timestamp change does not invalidate the corpus.

Check accepts both snapshot versions and returns `schema_version: 2`, kind
`mastermind_document_evidence_check`, `status`, `root`,
`snapshot_sha256`, live `revision`, `snapshot_revision`, `revision_changed`,
`changed_files` with endpoint paths and reasons, and the edges with their
`freshness`. The separate `corpus` result contains `status`, `directories`, and
`changed_files` with `added`, `missing`, or `content_changed` reasons. A rename
appears as a missing old path and an added new path.

| Corpus status | Meaning |
|---|---|
| `not_tracked` | The snapshot has no corpus inventory. This is explicit for legacy v1 snapshots. |
| `current` | The Markdown files within the selected directories match. An empty inventory can be current only after a successful scan. |
| `changed` | A selected document was added, removed, renamed, or edited. Renew the research even if the cited endpoints still match. |

| Result | Meaning and action |
|---|---|
| `current` | The named endpoints match and any tracked corpus matches. Check `corpus.status` to distinguish tracked from endpoint-only evidence. Reassess a relation's meaning before using it as proof. |
| `needs_review` | An endpoint changed or disappeared, or the tracked corpus changed. Endpoint drift marks incident edges for source review; corpus changes can leave every edge current while the research needs review. |
| `error` | The snapshot or operation cannot be trusted. Resolve the schema, confinement, read, or race error before reusing evidence. |

Exit codes are `0` for a saved snapshot or current check, `1` for a check needing
review, and `2` for an invalid or unsafe operation. Errors are JSON on stdout:
`{"schema_version":1,"status":"error","error":{"code":"..."}}`, with a
path when relevant.

The native history projection validates the same strict v1/v2 packet and live
file/corpus bytes, but has its own response contract: `schema_version: 1`, kind
`mastermind_native_document_evidence_check`, `status`, `root`, `packet`,
`snapshot_revision`, `changed_files`, `corpus`, `edges`, and `limits`. `packet`
keeps the raw artifact digest and byte length separate from the snapshot's
internal digest and schema version. The native reader accepts only a
repository-relative or root-contained absolute packet path under
`.mastermind/research`, follows no links, writes nothing to SQLite, and does not
run Git. It therefore does not return a current revision or `revision_changed`;
use the portable `check` command or an explicit Git inspection when that fact is
needed.

History `freshness` describes the derived Markdown FTS inventory.
`document_graph.status` describes the packet's named endpoint and optional
corpus bytes. Either can be stale while the other is current. Re-indexing can
refresh history, but it cannot clear document graph drift; capture a new packet
only after reviewing the changed sources. A native `current` edge still has
`verification: unverified`. The combined native operation rechecks the history
inventory and SQLite data version after graph capture; concurrent history drift
returns `snapshot_changed` instead of pairing an old `fresh` result with the
new graph check.

## Scope and limits

- Limits: 1 MiB manifest, 4 MiB saved graph, 256 relations, 128 unique endpoint
  files, 1 MiB per file, and 16 MiB of unique endpoint plus corpus content per
  read pass. Content is read twice. Git reads repository metadata and ignore
  rules for the revision and dirty-state summary.
- Corpus limits: 8 non-overlapping roots, 256 Markdown files, 16 directory
  levels below each root, and 32 repository-relative path components including
  the filename. A shared budget permits 8,192 entry/root visits and checks a
  10-second capture deadline across four inventories and both content passes.
  Hidden and non-Markdown entries encountered during enumeration consume visits
  too. This is a cooperative capture limit, not a timeout for the entire CLI or
  its Git reads. Exceeding a limit fails the operation; no partial corpus is saved.
- Paths cannot traverse symlinks, escape the root, or enter `.git`. Hidden
  endpoint paths are restricted to supported project evidence and workflow
  directories; secret dotfiles are rejected. Corpus roots follow the same policy,
  including supported roots such as `.mastermind/decisions`. Visible symlinks
  and special files encountered during a corpus scan are errors, even with a
  non-Markdown suffix. A missing or unreadable watched root is an error, not an
  empty inventory. Read errors, interruption, and observed directory replacement
  likewise cannot produce a successful current check.
- File bytes are hashed, so edits with unchanged size and mtime are detected.
  Snapshot capture rejects observed file races and HEAD changes. A check is
  evidence about its capture, not a lock preventing later edits.
- `revision_changed` reports a different HEAD separately from endpoint drift.
  Dirty worktree state is conservative and informational: metadata-only touches
  can mark it dirty. Neither an unchanged HEAD nor a clean worktree establishes
  correctness. Git metadata reads disable optional index writes and configured
  filesystem monitors, external diffs, and text conversions.
- The snapshot is bound to its local canonical repository root. Recreate it
  when moving to another checkout.
- **Corpus coverage is limited to the selected non-hidden Markdown documents.**
  Without tracking, unlisted and newly added documents remain outside the graph.
  With tracking, `corpus.status: changed` requires renewed research, even with
  unchanged HEAD and dirty state. No matching inventory establishes semantic
  supersession or activates a proposed decision. Search history for superseding
  records and contradictions before claiming a policy is current or evidence is
  complete. Snapshot hashes do not validate the upstream sources of a generated
  report unless those sources are also declared.

For each reviewed relation, keep the source passage, explicit decision status,
contradictions, and any actual test result in the research packet. Capture a new
snapshot after source review; do not edit the old snapshot to clear freshness
or verification fields.
