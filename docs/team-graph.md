# Local team graph

A team graph combines several existing local indexes into one bounded,
read-only architecture view. Each repository stays independently indexed.
Mastermind neither copies repositories into a central database nor infers
cross-repository calls; the manifest declares those relationships.

The public v1 schema is
[`schemas/mastermind-team-v1.schema.json`](../schemas/mastermind-team-v1.schema.json),
with API identifier `mastermind-team/v1`.

## Prerequisites

Each member repository must have:

- a clean, current Git revision;
- a fresh `.mastermind/mmcg.db` created by `mastermind index .`;
- a stable local root and index path available to the querying process.

## Define, lock, and query

Create a draft manifest. Paths may be absolute or relative to the manifest.

```json
{
  "api_version": "mastermind-team/v1",
  "repositories": [
    {
      "id": "checkout",
      "root": "../checkout",
      "index": "../checkout/.mastermind/mmcg.db"
    },
    {
      "id": "payments",
      "root": "../payments",
      "index": "../payments/.mastermind/mmcg.db"
    }
  ],
  "relationships": [
    {
      "id": "checkout-to-payments",
      "relation": "calls_service",
      "from": {"repository": "checkout", "component": "src/api"},
      "to": {"repository": "payments", "component": "src/api"},
      "label": "Checkout invokes the payments API"
    }
  ]
}
```

Resolve and pin every repository before querying it:

```bash
mastermind team lock team.json --output team.lock.json
mastermind team map team.lock.json > team-map.json
```

`team lock` writes canonical root and index paths plus the credential-free
repository identity, exact Git revision, and a domain-separated digest of the
SQLite database and active WAL bytes. Its JSON result also prints the exact
`manifest_sha256` value to use as `MMCG_TEAM_MANIFEST_SHA256`. `team map`
reopens every index through
Mastermind's private read-only snapshot path and rechecks all pins. Revision,
identity, source freshness, DB/WAL drift, duplicate canonical roots, or
duplicate canonical indexes fail closed.

The result namespaces every node (`repo:checkout` and
`repo:checkout/component:src/api`). Internal component edges remain
Tree-sitter-derived with `confidence=medium`; cross-repository edges exist only
when declared in the manifest and carry `confidence=declared` plus
`provenance=team-manifest`.

## MCP

The fixed read-only `mmcg_team_map` tool accepts a canonical
repository-relative `manifest` path. For MCP, the locked manifest must live
inside the repository served by that MCP process and the server operator must
authorize that exact file through `MMCG_TEAM_MANIFEST` (an absolute path or a
path relative to the served repository) and pin its exact bytes through
`MMCG_TEAM_MANIFEST_SHA256=sha256:<digest>`. Without both values the tool fails
closed; a changed manifest also fails until the operator reviews and repins it.
The referenced repositories and indexes may be elsewhere on the local
filesystem, but must still match the locked identities, revisions, and snapshot
digests. This prevents a repository-controlled manifest from silently widening
an agent's filesystem read scope.

## Bounds and trust model

Version 1 allows at most 16 repositories and 500 explicit relationships. It
returns at most 20 components and 200 internal edges per repository, probes at
most 20,000 import edges, caps each DB/WAL file at 2 GiB and all index bytes at
4 GiB, and enforces a 30-second operation deadline. Any bounded projection is
marked `partial` with diagnostics; it is never presented as a complete graph.
Component endpoints use the same canonical depth-2 component model as the
default repository map. Declared endpoints are retained inside the 20-component
budget even when they are not among the largest components.
Repository IDs are unambiguous ASCII slugs containing only letters, digits,
dots, underscores, and hyphens; the `team:internal:` edge-ID namespace is
reserved for Mastermind-derived edges.

The manifest is inert data: no commands, credentials, network fetches, native
plugins, custom MCP handlers, policy code, or SQLite writes are permitted. A
declared edge is evidence supplied by the manifest owner, not compiler- or
runtime-resolved proof. Keep the lock file under normal code-review ownership
and regenerate it whenever a member repository or its index changes.

Version 1 does not provide remote repository discovery, distributed locking,
access control, or a hosted graph.
