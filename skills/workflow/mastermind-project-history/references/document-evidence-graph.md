# Document evidence graph

Use this helper when a research packet needs to preserve explicit links between
documents and code across edits. It records a bounded set of declared relations
and checks whether their source files have changed. It does not infer links,
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
  --output .mastermind/research/session-evidence-v1.json

python3 "$SKILL_DIR/scripts/document_graph.py" check \
  --root "$REPO_DIR" \
  --graph .mastermind/research/session-evidence-v1.json
```

The declared repository must be a Git worktree root with a HEAD commit. Snapshot
writes a **new** file under that root's `.mastermind/research/`; it will not
overwrite an existing file, a source endpoint, or the input manifest. Check is
read-only. Neither command changes the Git index or project history.

The snapshot has `schema_version: 1`, kind
`mastermind_document_evidence_graph`, a canonical local `root`,
`revision: {head, dirty}`, a sorted `files` inventory, `edges`, and `sha256`.
Each file records `path`, `sha256`, `bytes`, and `lines`. Each edge records its
deterministic `id`, original endpoints, relation, and `verification: unverified`.
The snapshot contains no full source contents. Its hash checks internal
consistency; it is not a signature or a semantic verdict.

Check returns kind `mastermind_document_evidence_check`, `status`, `root`,
`snapshot_sha256`, live `revision`, `snapshot_revision`, `revision_changed`,
`changed_files` with path and reasons, and the edges with their `freshness`.

| Result | Meaning and action |
|---|---|
| `current` | The named endpoint files match the snapshot. Reassess the meaning of a relation before using it as proof. |
| `needs_review` | At least one endpoint changed or disappeared. Every incident edge needs source review; unaffected edges remain current. |
| `error` | The snapshot or operation cannot be trusted. Resolve the schema, confinement, read, or race error before reusing evidence. |

Exit codes are `0` for a saved snapshot or current check, `1` for a check needing
review, and `2` for an invalid or unsafe operation. Errors are JSON on stdout:
`{"schema_version":1,"status":"error","error":{"code":"..."}}`, with a
path when relevant.

## Scope and limits

- Limits: 1 MiB manifest, 4 MiB saved graph, 256 relations, 128 unique endpoint
  files, 1 MiB per file, and 16 MiB total source bytes. Only explicitly named
  files are read as source evidence. Git reads its repository metadata and
  ignore rules for the revision and dirty-state summary.
- Paths cannot traverse symlinks, escape the root, or enter `.git`. Hidden
  endpoint paths are restricted to supported project evidence and workflow
  directories; secret dotfiles are rejected.
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
- **Unlisted and newly added documents are outside this graph.** Search history
  again for superseding records and contradictions before claiming a policy is
  current or evidence is complete. Snapshot hashes do not validate the upstream
  sources of a generated report unless those sources are also declared.

For each reviewed relation, keep the source passage, explicit decision status,
contradictions, and any actual test result in the research packet. Capture a new
snapshot after source review; do not edit the old snapshot to clear freshness
or verification fields.
