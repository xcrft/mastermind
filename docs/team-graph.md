# Local team graph

A team graph joins the local indexes of several repositories with the
cross-repository relationships you declare, into one read-only graph for the
CLI or MCP.

## Lock a manifest

1. In each member repository, commit or stash local changes and refresh the
   index with `mastermind index .`.

2. Write a manifest, as defined by the
   [public schema](../schemas/mastermind-team-v1.schema.json). Paths can be
   absolute or relative to the manifest, and repository IDs use only ASCII
   letters, digits, dots, underscores and hyphens:

   ```json
   {
     "api_version": "mastermind-team/v1",
     "repositories": [
       {"id": "checkout", "root": "../checkout", "index": "../checkout/.mastermind/mmcg.db"},
       {"id": "payments", "root": "../payments", "index": "../payments/.mastermind/mmcg.db"}
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

3. Lock it and read the map:

   ```bash
   mastermind team lock team.json --output team.lock.json
   mastermind team map team.lock.json > team-map.json
   ```

   `team lock` pins canonical paths, credential-free repository identities, Git
   revisions and the exact database and active WAL digests. Its JSON summary
   prints the `manifest_sha256` that MCP needs. `team map` checks every pin and
   source freshness again before it answers.

Any change to a member repository or index needs a new lock, because the pin
covers exact bytes. That includes a no-op `mastermind index` and a checkpoint of
a non-empty WAL. An empty WAL beside a running server doesn't count.

Nodes are namespaced by repository, and internal edges keep their codegraph
provenance. Cross-repository edges have `provenance=team-manifest`, because
they come from your declaration and no network call is observed.

## Use through MCP

For `mmcg_team_map`, keep the locked manifest inside the served repository and
set both values in the server environment:

```text
MMCG_TEAM_MANIFEST=team.lock.json
MMCG_TEAM_MANIFEST_SHA256=sha256:<manifest_sha256 from team lock>
```

The tool's manifest path must be repository-relative and equal to the
configured one. Member repositories can live outside the served repository but
must match the lock. After a manifest change, review it and configure its new
digest.

## Limits

| Limit | Value |
|---|---|
| Manifest | 1 MiB |
| Repositories | 16 |
| Explicit relationships | 500 |
| Component endpoints declared per repository | 20. More is a contract error |
| Components returned per repository | 20 |
| Internal import edges | 200 returned per repository, from a probe of at most 20,000 |
| Index file (database or WAL) | 2 GiB each, 4 GiB across all members |
| Deadline | 30 s for each `team lock` or `team map` |

A cut component or edge list sets `partial` and adds a diagnostic. Unsafe paths,
duplicate roots or indexes, stale sources, changed pins and a timeout reject the
query. Hashing opens one parent directory per repository, rejects path
substitution, symlinks and special files, and checks identity before and after
each bounded read. The manifest declares data only, so a query can't fetch
repositories, run plugins or write member indexes.
