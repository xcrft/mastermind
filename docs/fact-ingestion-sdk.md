# Fact-ingestion SDK

Import scanner findings, coverage, test results, traces or custom analysis as a
revision-bound JSON manifest. Mastermind validates and stores the dataset.
Producer code is never loaded into the process.

The [public schema](../schemas/mastermind-facts-v1.schema.json) is
`mastermind-facts/v1`. It supports annotations and relationships.

## Import an existing report

Index the repository, then adapt a report. Set `PRODUCER_VERSION` to the version
that generated it.

```bash
mastermind index .
mastermind facts adapt --format sarif \
  --input reports/semgrep.sarif --output reports/semgrep.facts.json \
  --producer semgrep --producer-version "$PRODUCER_VERSION" --dataset pr-security
mastermind enrich --facts reports/semgrep.facts.json
mastermind query facts --path src --top 100
```

| Adapter | Input |
|---|---|
| `sarif` | SARIF findings |
| `coverage` | LCOV or Cobertura |
| `junit` | JUnit test report |
| `otel` | OpenTelemetry OTLP JSON |

| Adaptation step | Required result |
|---|---|
| Map report entries | Every fact maps to an indexed repository file |
| Bind the report | Record its exact size and digest |
| Parse or mapping failure | Reject the whole adaptation |

## Write a custom producer

First obtain the repository contract:

```bash
mastermind query facts --top 1 > mastermind-facts-contract.json
```

| Manifest input | Source |
|---|---|
| API version | Exact `contract.api_version` |
| Repository identity and revision | Exact `contract.repository.identity` and `contract.repository.revision` |
| Capabilities | Supported values returned in the contract |
| Source files and provenance artifacts | Hash every referenced file and record its size |

Replace all example identities, sizes, hashes and paths with measured values:

```json
{
  "api_version": "mastermind-facts/v1",
  "capabilities": ["annotations"],
  "repository": {
    "identity": "git-remote:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "revision": "0123456789abcdef0123456789abcdef01234567"
  },
  "producer": {"name": "com.example.arch-lint", "version": "1.0.0"},
  "dataset": "default",
  "provenance": {"kind": "static-analysis", "artifacts": ["analysis"]},
  "files": [
    {
      "path": "src/payment.rs",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "bytes": 1240
    }
  ],
  "artifacts": [
    {
      "id": "analysis",
      "path": "reports/arch-lint.json",
      "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      "bytes": 4312
    }
  ],
  "facts": [
    {
      "kind": "annotation",
      "id": "payment-boundary",
      "path": "src/payment.rs",
      "line": 42,
      "severity": "warning",
      "category": "architecture.boundary",
      "title": "Payment boundary crossed",
      "message": "The function crosses the declared payment boundary."
    }
  ]
}
```

```bash
mastermind enrich --facts mastermind-facts.json
mastermind query facts --top 100
```

For relationships, declare the capability and provide exact source and target
endpoints as defined by the schema.

## Sign producer evidence

Use signing when import policy should require a particular producer key:

```bash
mastermind facts keygen --private-key producer.seed --public-key producer.pub
mastermind facts sign mastermind-facts.json \
  --private-key producer.seed --signature mastermind-facts.sig.json
mastermind facts verify mastermind-facts.json \
  --signature mastermind-facts.sig.json --public-key producer.pub \
  --trusted-key-id "sha256:<public-key-digest>" --json
mastermind enrich --facts mastermind-facts.json \
  --signature mastermind-facts.sig.json --public-key producer.pub \
  --trusted-key-id "sha256:<public-key-digest>" --require-signature
```

| Signing input or event | Contract |
|---|---|
| Key ID | Use the ID printed by key generation |
| Key generation | Refuses existing paths. Creates the private seed with Unix mode `0600` |
| Incomplete policy | Import fails |
| `--revoked-key-id` | Overrides trust |
| Verified import | Stores the key and signature proof |
| Changed trust or revocation policy | Re-import the dataset to apply the decision |
| Valid signature | Establishes control of the allowed key, not human identity, signing time or finding accuracy |

See the [signature schema](../schemas/mastermind-fact-signature-v1.schema.json)
for the signed statement.

## Read and replace datasets

| Operation or condition | Result |
|---|---|
| `query facts`, `mmcg_facts`, Lens or review export | Same data with source and producer provenance. Unsigned imports stay labeled unsigned |
| Relationship with matching source and target | Can corroborate an existing returned edge. Cannot create or remove codegraph topology |
| Runtime or coverage fact | Retains its own evidence type |
| Successful import | Atomically replaces only `(producer.name, dataset)` |
| Empty `facts` array | Clears that dataset |
| Failed validation | Preserves the previous dataset |
| Changed revision, index or bound source | Reads withhold stale facts. Regenerate and re-import |

## Validation boundary

| Validation | Rejection condition |
|---|---|
| Schema, fact IDs and capabilities | Unsupported shape, identity or value |
| Repository and source bindings | Wrong repository/revision, indexed source hash or artifact hash |
| File access | Over-limit input, nonregular file, symlink, noncanonical path or path substitution |

Producers have no direct SQLite access. Manifests cannot run commands, fetch
network resources, install tools, or add MCP handlers. For exact input and
response limits, see the
[fact-ingestion reference](reference/mmcg.md#declarative-fact-ingestion-sdk).
