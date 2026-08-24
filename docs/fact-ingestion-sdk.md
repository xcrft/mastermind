# Declarative fact-ingestion SDK

Bring SARIF, coverage, tests, traces, or your own analysis into the same review
without loading producer code into Mastermind.

The extension boundary is data ingestion, not plugin execution. A producer
emits a revision-bound JSON manifest; Mastermind validates the complete input
before atomically replacing that producer's normalized dataset in private
SQLite tables. Lens, CLI, and the fixed read-only `mmcg_facts` tool read the
result.

The public v1 schema is
[`schemas/mastermind-facts-v1.schema.json`](../schemas/mastermind-facts-v1.schema.json).
Its API identifier is `mastermind-facts/v1`, with two capabilities:
`annotations` and `relationships`.

## The contract in one screen

| Property | v1 behavior |
|---|---|
| Producer output | Inert JSON matching the public schema |
| Repository binding | Exact repository identity and 40-character Git revision |
| File binding | Canonical path, byte size, and SHA-256 digest |
| Provenance | Bounded local artifacts, optionally signed with Ed25519 |
| Database writes | Performed only by Mastermind after full validation |
| Read surfaces | `query facts`, `mmcg_facts`, Lens, and review export |
| Executable extension points | None |

## Built-in adapters

Built-in adapters convert common reports into the same manifest. An adapter
reads one bounded local artifact, maps every fact to the current index, records
the exact digest and size, and emits nothing if parsing is partial or any fact
cannot be mapped to an indexed repository file.

```bash
mastermind facts adapt --format sarif \
  --input reports/semgrep.sarif --output reports/semgrep.facts.json \
  --producer semgrep --producer-version 1.82.0 --dataset pr-security

mastermind facts adapt --format coverage \
  --input coverage/lcov.info --output coverage.facts.json \
  --producer vitest --producer-version 3.2.0 --dataset unit-coverage

mastermind facts adapt --format junit \
  --input test-results/junit.xml --output junit.facts.json \
  --producer pytest --producer-version 8.4.0 --dataset unit-tests

mastermind facts adapt --format otel \
  --input traces/otlp.json --output runtime.facts.json \
  --producer otel-collector --producer-version 0.130.0 --dataset review-traces
```

`coverage` auto-detects LCOV and Cobertura XML. OTLP runtime parent-child
relationships use `confidence=observed`; they still only decorate matching
static endpoints in Lens and never create codegraph topology.

## Custom producer flow

Index the repository, then ask Mastermind for the exact contract that the
manifest must bind:

```bash
mastermind index .
mastermind query facts --top 1 > mastermind-facts-contract.json
```

The response contains `contract.api_version`, `contract.repository.identity`,
`contract.repository.revision`, and `contract.supported_capabilities`. Copy
those exact values, hash every referenced source and provenance artifact, and
emit only the declared fact kinds.

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

## Signed producer provenance

For a producer-controlled trust boundary, sign the strict manifest with a
local Ed25519 seed and verify it against an explicit trusted-key allowlist. The
private-key file is a single base64-encoded 32-byte seed and must be owned by
the current user with mode `0600` on Unix. The public-key file contains the
base64-encoded 32-byte public key.

```bash
mastermind facts keygen \
  --private-key producer.seed \
  --public-key producer.pub

mastermind facts sign mastermind-facts.json \
  --private-key producer.seed \
  --signature mastermind-facts.sig.json

mastermind facts verify mastermind-facts.json \
  --signature mastermind-facts.sig.json \
  --public-key producer.pub \
  --trusted-key-id sha256:<public-key-digest> \
  --json

mastermind enrich --facts mastermind-facts.json \
  --signature mastermind-facts.sig.json \
  --public-key producer.pub \
  --trusted-key-id sha256:<public-key-digest> \
  --require-signature
```

`facts keygen` uses the operating system CSPRNG, writes the seed with private
permissions on Unix, prints the derived `sha256:<public-key-digest>`, and
refuses to replace either key file.

The detached format is defined by
[`mastermind-fact-signature-v1.schema.json`](../schemas/mastermind-fact-signature-v1.schema.json).
It signs a domain-separated canonical statement over the validated manifest,
including its repository identity, revision, source files, provenance
artifacts, and facts. Revocation wins over trust; pass `--revoked-key-id`
during verify/import to reject a compromised key. A partial signature policy
(for example a signature without a public key or trusted key ID) fails closed.

The import stores the verified key ID and reproducible public-key/signature
proof with the normalized facts. Lens and `mmcg_facts` expose the result as
`signature_status=verified`; unsigned imports remain explicitly `unsigned`.
Trust and revocation are evaluated at import time. Rotate or revoke a producer
by updating the allowlist and re-importing that dataset under the new policy.

Ed25519 proves control of the allowlisted key. It does not by itself prove a
human or organization identity, signing time, transparency-log inclusion, or
whether a signature predates key compromise. Those claims require an external
identity and timestamp/transparency policy.

The MCP equivalent is the built-in, read-only `mmcg_facts` tool with `path`
and `top` arguments. Its bounded response includes verified provenance artifact
paths, sizes, and SHA-256 digests. Lens loads current facts and shows the
binding manifest digest on the producer card. Annotations appear as
source-labelled findings. Relationships can corroborate a returned codegraph
edge only when both file and line endpoints match; facts never create or remove
graph topology.

`mastermind review export` carries the same normalized facts into its autonomous
HTML. Unsigned datasets remain `producer-attested`; verified datasets and their
provenance artifacts are `producer-signed`, with the key ID, public key,
signature, detached-signature digest, and signed manifest digest recorded in
the package manifest. Mixed packages are explicitly
`partially-producer-signed`.

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
