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
| `team lock` | Pins canonical paths, repository identities, Git revisions and database/WAL digests |
| `team map` | Rechecks every pin and source freshness |
| Changed repository or index | Requires a new lock |
| Nodes | Namespaced by repository |
| Internal edges | Retain static codegraph provenance |
| Cross-repository edges | Retain `provenance=team-manifest`. A declaration is not an observed network call |

## Use through MCP

For `mmcg_team_map`, keep the locked manifest inside the served repository.
Configure both values in the server environment:

```text
MMCG_TEAM_MANIFEST=team.lock.json
MMCG_TEAM_MANIFEST_SHA256=sha256:<digest printed by team lock>
```

| Input | Rule |
|---|---|
| Tool manifest path | Repository-relative and equal to the configured manifest |
| Member repository paths | May be outside the served repository, but must match the lock |
| Changed manifest | Review it and configure its new digest |

## Boundaries

| Boundary | Behavior |
|---|---|
| Version 1 input | At most 16 repositories and 500 explicit relationships |
| Bounded output | Inspect `partial` and diagnostics for omitted components or edges |
| Unsafe paths, duplicate roots/indexes, stale sources or changed pins | Query rejected |
| Manifest authority | Declares data only. Queries cannot fetch repositories, run plugins or write member indexes |

See the [public schema](../schemas/mastermind-team-v1.schema.json) and
[reference](reference/mmcg.md#local-team-graph) for exact limits and endpoint rules.
