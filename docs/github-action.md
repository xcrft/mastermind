# Mastermind verifiable audit Action

Turn a Mastermind audit into evidence GitHub can verify and publish without giving
pull-request code an OIDC token or write permission.

The Docker Action creates schema-v3 audit envelopes for an exact repository
snapshot. The secure deployment splits untrusted analysis from privileged
verification, attestation, and publication.

| Stage | Trigger | Authority | Executes PR code |
|---|---|---|---|
| Analyze | `pull_request` | `contents: read`; no secrets or OIDC | Yes |
| Verify | `workflow_run` | Read-only GitHub metadata and artifact access | No |
| Attest | verified result | OIDC and attestations write only | No |
| Publish | verified result | Pull-request comment write only | No |

Do not assemble that privilege split from memory. Start from the pinned,
repository-validated examples:

[`mastermind-audit-pr.yml`](examples/mastermind-audit-pr.yml) and
[`mastermind-audit-publish.yml`](examples/mastermind-audit-publish.yml).

## Read the proof correctly

- **Content integrity:** the Mastermind Canonical JSON v1 bytes match their
  SHA-256 digest. Replacing both the manifest and digest can still create a
  different internally consistent envelope.
- **Provenance authenticity:** a detached Ed25519 signature validates under a
  trusted, non-revoked key ID. This proves control of that key, not signer
  identity, signing time, or whether signing preceded key compromise.
- **Policy acceptance:** every configured trust anchor passes. A repository
  anchor requires exact `owner/repo`, full baseline and head OIDs, trusted-root
  recomputation, and `worktree_clean:true`. A signature anchor requires the
  signature, public key, and a trusted non-revoked key-ID allowlist. Partial or
  empty policy fails with `incomplete_trust_anchor` or `no_trust_anchor`.
- **GitHub artifact attestation:** the publication workflow verified the
  deterministic archive and statement. This does not prove that PR analysis
  ran in a trusted environment or that its findings are correct.

`--integrity-only` is a diagnostic. It sets authenticity and policy to
`not_evaluated`; the Docker Action, privileged `pr-comment`, and publication
workflow do not use it.

## Envelope and signature contract

The envelope digest covers the canonical manifest, not pretty-printed storage.
Canonical JSON uses UTF-8, sorted object keys, array order, minimal JSON string
escaping, explicit i64/u64 integers only, and no trailing newline. Strict
readers reject duplicate or unknown fields, floats, and unknown schemas,
algorithms, or canonicalization identifiers.

The manifest binds repository identity, full baseline and HEAD, clean-worktree
state, tool/config/index metadata, spec and executor-report paths and digests,
normalized name/status entries, binary diff digest, verdict, file scope,
claims, discrepancies, snapshot drift, logical mmcg queries, recorded verify
commands, and summary.

Detached signatures sign a domain-separated canonical statement containing:

```text
domain = mastermind/audit-envelope-signature/v1
signature_schema = 1
signature_algorithm = ed25519
key_id = sha256:<public-key digest>
envelope_schema = 3
hash_algorithm = sha256
canonicalization = mastermind-cjson-v1
manifest_digest = sha256:<manifest digest>
```

Key files contain one base64 line. The private file encodes a 32-byte Ed25519
seed and must have mode `0600` on Unix; the public file encodes 32 bytes. Keep
trusted and revoked key-ID allowlists in independently reviewed policy. For
rotation, add the new trusted ID, deploy verifiers, rotate signing, then revoke
the old ID while retaining the policy needed to verify historical evidence.

## Local commands

```bash
mastermind audit-spec .mastermind/tasks/005-example/spec.md \
  --since 1111111111111111111111111111111111111111 \
  --root . --bundle audit.bundle.json

mastermind audit sign audit.bundle.json \
  --private-key audit-ed25519.seed \
  --signature audit.bundle.sig.json

mastermind audit verify audit.bundle.json \
  --root . \
  --expected-repository owner/repo \
  --expected-baseline 1111111111111111111111111111111111111111 \
  --expected-head 2222222222222222222222222222222222222222

mastermind audit verify audit.bundle.json \
  --signature audit.bundle.sig.json \
  --public-key audit-ed25519.pub \
  --require-signature \
  --trusted-key-id sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
```

Snapshot and signature modes can be supplied together; every supplied policy
must pass. A present signature is never ignored.

## Action inputs and output

The repository-root `action.yml` defines a Docker Action with these inputs:

- `root`: repository-relative root contained below `GITHUB_WORKSPACE`;
- `since` and `expected-baseline`: the same full lowercase baseline OID;
- `bundle-dir`: a new, non-symlink repository-relative output directory;
- `expected-repository`: exact GitHub `owner/repo`;
- `expected-head`: full lowercase head OID;
- `require-clean-worktree`: must remain `true` for publication.

The Action outputs the verified bundle directory and aggregate result JSON
path. Inputs are passed as data; they are not evaluated, sourced, or rendered
into shell syntax.

The Action audits only canonical task folders changed between `since` and
`HEAD`. Every selected task must include `spec.md` and a valid
`executor-report.md`; missing evidence fails the run. Historical task folders
that were not changed in the pull request are not re-audited against the new
baseline. This keeps a PR scoped to the contract it introduces or updates and
prevents an empty report from producing publishable evidence.

## Copyable workflows and trusted verifier

The unprivileged PR workflow uses the Action from its checked-out PR tree. That
executes untrusted code but receives no secrets, OIDC, or write permission. The
privileged workflow contains its strict schema-v3 verifier inline, so the
verifier implementation and identity are bound to the independently allowlisted
trusted workflow blob. The examples contain no unresolved Action or verifier
placeholder. External Actions remain pinned to audited 40-character commits.

The PR workflow triggers only on `pull_request`, checks out the exact head with
credentials disabled, and has only `contents: read`. It uploads one
attempt-specific artifact. Uploaded PR numbers, SHAs, workflow strings, and
digests remain hostile claims.

The publication workflow has no checkout. Its read-only verify job keys API
lookups to the source run ID and attempt, then checks repository ID/name, event,
conclusion, workflow path/blob, independent PR/base/head association, and one
server-owned artifact ID/digest/size. Extraction is capped at 64 MiB total,
16 MiB per regular file, 256 files, and 240-byte relative paths. Links, devices,
traversal, nested archives, and extra names are rejected. Only the trusted
verify implementation runs before the deterministic statement/archive is
created.

Only the attestation job has `id-token: write` and `attestations: write`. Only
the publication job has `pull-requests: write`; it rechecks PR head and artifact
identity, then updates at most one constant-marker comment owned by
`github-actions[bot]`. Neither job executes commands stored in an envelope.

Treat every `workflow_run` download as hostile until this chain completes. If
GitHub cannot independently return the workflow blob, PR association, server
artifact digest, or exact run attempt, fail closed.

## Updating pins

For every pinned Action or OCI base:

1. Read the upstream release notes and security advisories.
2. Resolve the release tag from the authoritative upstream repository or registry.
3. Verify the full commit or multi-architecture manifest digest independently.
4. Review the diff from the old pin and update the allowlist in `scripts/validate.py` in the same change.
5. Run the full Rust tests, repository validator, YAML parser check, and Docker build.

Never shorten a commit or pin a mutable tag such as `v7`, `main`, or `master`.

## GitHub plan limitations

Artifact attestation availability and verification behavior depend on
repository visibility and the organization's GitHub plan and policy.
Private/internal support and API access can differ from public repositories.
Confirm current GitHub documentation and organization settings before making
attestations a required release gate. The local schema-v3 verifier performs no
network lookup; the trusted publication workflow and GitHub tooling remain
responsible for issuer, workflow, repository, and ref verification.
