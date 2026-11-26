# Fact-ingestion SDK

Import scanner findings, coverage, test results, traces or custom analysis as a
revision-bound JSON manifest. Mastermind validates and stores the dataset.
Producer code is never loaded into the process.

The [public schema](../schemas/mastermind-facts-v1.schema.json) is
`mastermind-facts/v1`. It supports annotations and relationships.

## Import an existing report

Index the repository, then adapt a report. Set `PRODUCER_VERSION` to the version
that generated it and `REPORT_REVISION` to the full commit OID the producer
analysed, for example `$GITHUB_SHA`. It must equal the indexed Git HEAD.

```bash
mastermind index .
mastermind facts adapt --format sarif \
  --input reports/semgrep.sarif --output reports/semgrep.facts.json \
  --producer semgrep --producer-version "$PRODUCER_VERSION" \
  --report-revision "$REPORT_REVISION" --dataset pr-security
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
| Bind the revision | `--report-revision` is the full lowercase commit OID of the indexed HEAD. When a SARIF run declares `versionControlProvenance` for this repository, it must name the same revision |
| Map report entries | Every fact maps to an indexed repository file. A JUnit testcase needs a `file` attribute and a SARIF result needs a location. OTLP spans without a code path are skipped. Repeated coverage lines are merged |
| Fit text | Control characters become spaces. Titles, messages and labels over the manifest limits are cut with `…` |
| Bind the report | Record its exact size and digest. The input path must be an exact UTF-8 repository-relative path, and ambiguous Unix backslash aliases are rejected |
| Duplicates | Exact duplicate findings collapse by their content-derived IDs |
| Parse or mapping failure | Reject the whole adaptation. `report_entry_without_path` and `report_entry_outside_index` give the counts of entries that could not map, and `artifact_revision_mismatch` names a revision conflict |
| Cut or empty report | Reject an XML report without its closing elements, an LCOV record without `end_of_record`, a JUnit suite that lists fewer cases than its `tests` total or declares failures without a failed case, and a report with no facts. Only an empty SARIF scan produces an empty dataset that clears the previous one |

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
| Key generation | Refuses existing paths and needs exact UTF-8 output paths. Writes the public key first, then the private seed with Unix mode `0600`, so a failure cannot leave a seed without its public key |
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
| Order | Annotations come errors first, then warnings |
| `--top N` cuts a source | That source is `partial`, with a `normalized_fact_limit` diagnostic naming returned and total facts |
| Lens drops findings at its per-file or global cap | That source is also `partial`, and a review export does not attest it |
| Relationship with matching source and target | Can corroborate an existing returned edge. Cannot create or remove codegraph topology |
| Runtime or coverage fact | Retains its own evidence type |
| Successful import | Atomically replaces only `(producer.name, dataset)` |
| Empty `facts` array | Clears that dataset |
| Failed validation | Preserves the previous dataset |
| Changed revision, index or bound source | Reads withhold stale facts. Regenerate and re-import |

## Validation boundary

The complete manifest is validated before the previous dataset is touched.

| Validation | Rejection condition |
|---|---|
| Schema, fact IDs and capabilities | Unknown or duplicate fields, unsupported shape, identity, value or capability |
| Repository and source bindings | Wrong repository or revision, indexed source hash, artifact size or hash, or a stale codegraph file |
| File access | Over-limit input, nonregular file, symlink, path traversal, noncanonical path or path substitution |

| Limit | Value |
|---|---|
| Manifest | 16 MiB |
| Referenced source files | 10,000 files, 512 MiB in total |
| Provenance artifacts | 64, 32 MiB each, 256 MiB in total |
| Facts per manifest | 100,000 |
| Facts returned by `query facts` and `mmcg_facts` | `--top` 1 to 400 |
| Facts shown in Lens | 200 |
| Field sizes in bytes | IDs 256. Producer name, dataset, provenance kind, category and relation 128. Version 128. Path 4 KiB. Title 256. Message 4 KiB. Label 512 |

Producers have no direct SQLite access. Manifests cannot load native code, run
commands, fetch network resources, install grammars or tools, add MCP handlers,
define policy rules or change the codegraph. Only Mastermind writes the
normalized tables.
