# Declarative fact-ingestion SDK

Mastermind extensions are data producers, not in-process plugins. A producer
writes a strict, revision-bound JSON manifest; Mastermind validates the entire
manifest and then atomically normalizes it into private SQLite tables. Lens and
the fixed read-only `mmcg_facts` MCP tool consume those normalized facts.

The public v1 schema is
[`schemas/mastermind-facts-v1.schema.json`](../schemas/mastermind-facts-v1.schema.json).
Its API identifier is `mastermind-facts/v1`, with two capabilities:
`annotations` and `relationships`.

## Producer flow

Index the repository, then ask Mastermind for the exact contract that the
manifest must bind:

```bash
mastermind index .
mastermind query facts --top 1 > mastermind-facts-contract.json
```

The response contains `contract.api_version`, `contract.repository.identity`,
`contract.repository.revision`, and `contract.supported_capabilities`. A
producer copies the API version and exact repository values into its manifest,
hashes every referenced source and provenance artifact, and emits facts only.

```json
{
  "api_version": "mastermind-facts/v1",
  "capabilities": ["annotations", "relationships"],
  "repository": {
    "identity": "git-remote:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "revision": "0123456789abcdef0123456789abcdef01234567"
  },
  "producer": {
    "name": "com.example.arch-lint",
    "version": "1.4.0"
  },
  "dataset": "default",
  "provenance": {
    "kind": "static-analysis",
    "artifacts": ["analyzer-output"]
  },
  "files": [
    {
      "path": "src/payment.rs",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "bytes": 1240
    },
    {
      "path": "src/checkout.rs",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "bytes": 980
    }
  ],
  "artifacts": [
    {
      "id": "analyzer-output",
      "path": "reports/arch-lint.json",
      "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      "bytes": 4312
    }
  ],
  "facts": [
    {
      "kind": "annotation",
      "id": "payment-owner-boundary",
      "path": "src/payment.rs",
      "line": 42,
      "severity": "warning",
      "category": "architecture.boundary",
      "title": "Payment ownership boundary crossed",
      "message": "The changed function crosses the declared payment boundary."
    },
    {
      "kind": "relationship",
      "id": "checkout-to-payment",
      "relation": "calls",
      "from": {"path": "src/checkout.rs", "line": 18},
      "to": {"path": "src/payment.rs", "line": 42},
      "confidence": "high",
      "label": "Producer-resolved checkout to payment call"
    }
  ]
}
```

Import and inspect the normalized dataset:

```bash
mastermind enrich --facts mastermind-facts.json
mastermind query facts --path src --top 400
```

The MCP equivalent is the built-in, read-only `mmcg_facts` tool with `path`
and `top` arguments. Its bounded response includes the verified provenance
artifact paths, sizes, and SHA-256 digests. Lens loads current facts
automatically and shows the binding manifest digest on the producer card. Annotations appear
as source-labelled findings; relationships may corroborate an already returned
codegraph edge only when both file and line endpoints match. Facts never create
or remove graph topology.

`mastermind review export` carries the same normalized facts into its autonomous
HTML and records the loaded fact-manifest and provenance-artifact digests as
an unsigned `producer-attested` claim for the exported Git head. A surrounding
workflow must supply any stronger identity or signature trust anchor.

## Validation and replacement

Before any database write, Mastermind verifies all of the following:

- the exact API version, declared capability allowlist, required fields, and
  absence of duplicate or unknown JSON fields;
- the indexed repository identity and current 40-character Git HEAD;
- canonical repository-relative paths with no traversal, absolute roots,
  backslashes, control bytes, or symlinks;
- regular-file sizes and lowercase SHA-256 digests for every referenced source
  and provenance artifact;
- that each source digest also matches the current codegraph index;
- unique fact IDs, one-based locations, bounded text, supported severities and
  confidence values, and references only to declared files and artifacts.

The repository identity is a credential-free digest of the canonical origin
host/path when a supported Git remote exists, or a digest of the canonical
local worktree path otherwise. The manifest is capped at 16 MiB, referenced
sources at 10,000 files and 512 MiB total, provenance at 64 artifacts, 32 MiB
each and 256 MiB total, and facts at 100,000. Query responses expose their own
smaller limits and explicit partial/truncation states.

A successful import atomically replaces only the dataset identified by
`producer.name` plus `dataset`. An empty `facts` array therefore clears that
dataset while preserving its validated provenance record. If validation fails,
the previous dataset is untouched. If HEAD, repository identity, the codegraph,
or any bound source later changes, reads omit that source and report it as
stale; they never silently mix revisions.

## Security boundary

The v1 contract deliberately has no executable extension points:

- no native or in-process plugin loading;
- no producer-defined MCP handlers;
- no executable custom policy rules;
- no direct SQLite access or schema migrations;
- no native Tree-sitter grammar packs;
- no network fetches or commands from manifest fields.

Only Mastermind writes its normalized fact tables. The language registry, MCP
tool table, policy DSL, and Tree-sitter graph remain compiled into Mastermind.
Future community query packs, framework recognizers, and evidence importers can
target this ingestion boundary without receiving process or database authority.
