# Local team graph

A team graph combines existing local indexes with declared cross-repository
relationships. Queries are read-only.

## Prepare the repositories

| Required input | Preparation |
|---|---|
| Clean Git revision for each member | Resolve uncommitted changes before locking |
| Fresh `.mastermind/mmcg.db` for each member | Run `mastermind index .` in that repository |
| Reviewed relationship manifest | Declare the cross-repository edges to display |

## Create and pin a manifest

Paths can be absolute or relative to the manifest:

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

```bash
mastermind team lock team.json --output team.lock.json
mastermind team map team.lock.json > team-map.json
```

| Operation or result | Contract |
|---|---|
| `team lock` | Pins canonical paths, credential-free repository identities, Git revisions and the exact database and active-WAL digests. Its JSON summary prints the `manifest_sha256` that MCP authorization needs |
| Repository IDs | ASCII letters, digits, dots, underscores and hyphens only, so namespaced node IDs cannot collide |
| `team map` | Rechecks every pin and source freshness |
| Changed repository or index | Requires a new lock. This includes a no-op `mastermind index` and a checkpoint of a non-empty WAL, because the pin covers exact bytes. An empty WAL beside a running server does not count |
| Nodes | Namespaced by repository |
| Internal edges | Retain static codegraph provenance |
| Cross-repository edges | Retain `provenance=team-manifest`. A declaration is not an observed network call |

## Use through MCP

For `mmcg_team_map`, keep the locked manifest inside the served repository.
Configure both values in the server environment:

```text
MMCG_TEAM_MANIFEST=team.lock.json
MMCG_TEAM_MANIFEST_SHA256=sha256:<manifest_sha256 from team lock>
```

| Input | Rule |
|---|---|
| Tool manifest path | Repository-relative and equal to the configured manifest |
| Member repository paths | May be outside the served repository, but must match the lock |
| Changed manifest | Review it and configure its new digest |

## Boundaries

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

| Boundary | Behavior |
|---|---|
| Bounded output | A cut component or edge list sets `partial` and adds a diagnostic |
| Unsafe paths, duplicate roots or indexes, stale sources, changed pins or a timeout | Query rejected |
| Database and WAL hashing | One parent-directory capability per repository. Path substitution, symlinks and special files are rejected, and identity is checked before and after each bounded read |
| Manifest authority | Declares data only. Queries cannot fetch repositories, run plugins or write member indexes |

The [public schema](../schemas/mastermind-team-v1.schema.json) defines every
manifest field and endpoint rule.
