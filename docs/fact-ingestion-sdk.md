# Fact ingestion

You can import scanner findings, coverage, test results, traces or your own
analysis as a JSON manifest bound to one Git revision. Mastermind validates and
stores the dataset, and Lens, review export, `query facts` and the `mmcg_facts`
MCP tool show it next to the codegraph. Producer code never runs inside
Mastermind.

The manifest format is `mastermind-facts/v1`, defined by the
[public schema](../schemas/mastermind-facts-v1.schema.json). It supports
annotations and relationships.

## Import an existing report

Set `PRODUCER_VERSION` to the version of the tool that wrote the report and
`REPORT_REVISION` to the full commit OID it analysed, for example
`$GITHUB_SHA`. That commit must be the indexed Git HEAD.

```bash
mastermind index .
mastermind facts adapt --format sarif \
  --input reports/semgrep.sarif --output reports/semgrep.facts.json \
  --producer semgrep --producer-version "$PRODUCER_VERSION" \
  --report-revision "$REPORT_REVISION" --dataset pr-security
mastermind enrich --facts reports/semgrep.facts.json
mastermind query facts --path src --top 100
```

| `--format` | Input |
|---|---|
| `sarif` | SARIF findings |
| `coverage` | LCOV or Cobertura |
| `junit` | JUnit test report |
| `otel` | OpenTelemetry OTLP JSON |

| Adapter rule | Result |
|---|---|
| Revision | `--report-revision` is the full lowercase OID of the indexed HEAD. A SARIF run that declares `versionControlProvenance` for this repository must name the same revision |
| Paths | Every fact maps to an indexed file. A JUnit testcase needs a `file` attribute and a SARIF result needs a location. OTLP spans without a code path are skipped. Repeated coverage lines are merged |
| Text | Control characters become spaces. Titles, messages and labels over the limits are cut with `…` |
| Report binding | The report's exact size and digest are recorded. The input path must be an exact UTF-8 repository-relative path, and ambiguous Unix backslash aliases are rejected |
| Duplicates | Exact duplicate findings collapse by their content-derived IDs |
| Parse or mapping failure | The whole adaptation fails. `report_entry_without_path` and `report_entry_outside_index` count the entries that could not map, and `artifact_revision_mismatch` names a revision conflict |
| Cut or empty report | Rejected: an XML report without its closing elements, an LCOV record without `end_of_record`, a JUnit suite with fewer cases than its `tests` total or with failures but no failed case, and a report with no facts. Only an empty SARIF scan gives an empty dataset, which clears the previous one |

## Write your own producer

1. Get the repository contract:

   ```bash
   mastermind query facts --top 1 > mastermind-facts-contract.json
   ```

   Copy `contract.api_version`, `contract.repository.identity` and
   `contract.repository.revision` exactly, and use only the capabilities the
   contract lists.

2. Write the manifest. Hash every source file and provenance artifact you
   reference and record its size. Replace all identities, sizes, hashes and
   paths in this example with measured values:

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
       {"path": "src/payment.rs", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bytes": 1240}
     ],
     "artifacts": [
       {"id": "analysis", "path": "reports/arch-lint.json", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "bytes": 4312}
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

   For relationships, declare the capability and give the exact source and
   target endpoints the schema defines.

3. Import and read it:

   ```bash
   mastermind enrich --facts mastermind-facts.json
   mastermind query facts --top 100
   ```

## Sign producer evidence

Sign a manifest when the import must come from a particular producer key:

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

Use the key ID that `keygen` prints. `keygen` refuses existing paths, writes
the public key first and then the private seed with Unix mode `0600`, so a
failure can't leave a seed without its public key. An import with an
incomplete signature policy fails, and `--revoked-key-id` overrides trust. A
verified import stores the key and the signature proof. After a trust or
revocation change, import the dataset again to apply it. A valid signature
shows control of the allowed key, not a person's identity, the signing time or
that the findings are right. The signed statement is in the
[signature schema](../schemas/mastermind-fact-signature-v1.schema.json).

## Read and replace datasets

| Operation or condition | Result |
|---|---|
| `query facts`, `mmcg_facts`, Lens or review export | The same data with source and producer provenance. Unsigned imports stay labeled unsigned |
| Order | Annotations come errors first, then warnings |
| `--top N` cuts a source | That source is `partial`, with a `normalized_fact_limit` diagnostic naming returned and total facts |
| Lens drops findings at its per-file or global cap | That source is `partial` too, and a review export doesn't attest it |
| Relationship with a matching source and target | Can back an existing returned edge. Can't create or remove codegraph edges |
| Runtime or coverage fact | Keeps its own evidence type |
| Successful import | Atomically replaces only `(producer.name, dataset)` |
| Empty `facts` array | Clears that dataset |
| Failed validation | Keeps the previous dataset |
| Changed revision, index or bound source | Reads withhold the stale facts. Regenerate and import again |

## Validation and limits

The whole manifest is validated before the previous dataset is touched. It is
rejected for unknown or duplicate fields, an unsupported shape, identity, value
or capability, a wrong repository or revision, a source hash that doesn't match
the index, a wrong artifact size or hash, a stale codegraph file, or an unsafe
file: over the limit, not a regular file, a symlink, path traversal, a
noncanonical path or path substitution.

| Limit | Value |
|---|---|
| Manifest | 16 MiB |
| Referenced source files | 10,000 files, 512 MiB in total |
| Provenance artifacts | 64, 32 MiB each, 256 MiB in total |
| Facts per manifest | 100,000 |
| Facts returned by `query facts` and `mmcg_facts` | `--top` 1 to 400 |
| Facts shown in Lens | 200 |
| Field sizes in bytes | IDs 256. Producer name, dataset, provenance kind, category, relation and version 128. Path 4 KiB. Title 256. Message 4 KiB. Label 512 |

Producers have no direct SQLite access. A manifest can't load native code, run
commands, fetch network resources, install grammars or tools, add MCP handlers,
define policy rules or change the codegraph. Only Mastermind writes the
normalized tables.
