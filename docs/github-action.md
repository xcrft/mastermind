# Mastermind verifiable audit Action

The Action produces schema-v3 audit envelopes and verifies them against exact repository snapshot inputs. It is designed for a two-workflow boundary: untrusted pull-request analysis has no secrets or OIDC, while a later `workflow_run` publication workflow obtains narrowly separated permissions only after independent verification.

## What each proof means

- Content integrity means the manifest's Mastermind Canonical JSON v1 bytes match its SHA-256 digest. Anyone who can replace both the manifest and digest can create another internally consistent envelope.
- Provenance authenticity means a detached Ed25519 signature verified with the configured public key and its recomputed key ID is trusted and not revoked. It proves control of that key only; it provides neither signer identity beyond policy, signing time, nor pre/post-compromise distinction.
- Policy acceptance means every configured independent trust anchor passed. A repository snapshot anchor requires exact `owner/repo`, full baseline and head OIDs, trusted-root recomputation, and `worktree_clean:true`. A signature anchor requires the signature, public key, and a trusted non-revoked key-ID allowlist. Partial or empty policy fails closed with `incomplete_trust_anchor` or `no_trust_anchor`.
- GitHub artifact attestation over the deterministic verified archive/statement means publication-workflow verification provenance. It does not prove that PR analysis ran in a trusted environment or that findings are true.

`--integrity-only` is an explicitly labelled diagnostic. It sets authenticity and policy to `not_evaluated`; the Docker Action, privileged `pr-comment`, and publication workflow never use it.

## Envelope and signature contract

The envelope digest covers the canonical manifest, not pretty-printed storage. Canonical JSON uses UTF-8, sorted object keys, array order, minimal JSON string escaping, explicit i64/u64 integers only, and no trailing newline. Strict readers reject duplicate/unknown fields, floats, unknown schemas, algorithms, or canonicalization identifiers.

The manifest binds repository identity, full baseline and HEAD, clean-worktree state, tool/config/index metadata, spec and executor-report paths and digests, normalized name/status entries, binary diff digest, verdict, file scope, claims, discrepancies, snapshot drift, logical mmcg queries, recorded verify commands, and summary.

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

Key files contain exactly one base64 line. The private file encodes a 32-byte Ed25519 seed and must have mode 0600 on Unix; the public file encodes 32 bytes. Keep trusted and revoked key-ID allowlists in independently reviewed policy. Rotation is the operator's responsibility: add the new trusted ID, deploy verifiers, rotate signing, then revoke the old ID while retaining the historical policy needed to verify old evidence.

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

The snapshot and signature modes can be supplied together; every supplied policy must pass. A present signature is never ignored.

## Action inputs and output

The repository-root `action.yml` defines a Docker Action with these inputs:

- `root`: repository-relative root contained below `GITHUB_WORKSPACE`;
- `since` and `expected-baseline`: the same full lowercase baseline OID;
- `bundle-dir`: a new, non-symlink repository-relative output directory;
- `expected-repository`: exact GitHub `owner/repo`;
- `expected-head`: full lowercase head OID;
- `require-clean-worktree`: must remain `true` for publication.

It outputs the verified bundle directory and aggregate result JSON path. Inputs are passed as data and never evaluated, sourced, or rendered into shell syntax.

## Copyable workflows and trusted verifier

Start from `docs/examples/mastermind-audit-pr.yml` and `docs/examples/mastermind-audit-publish.yml`. The unprivileged PR workflow uses the Action from its already checked-out PR tree; this executes untrusted code but receives no secrets, OIDC, or write permission. The privileged workflow contains its strict schema-v3 verifier inline, so the verifier implementation and identity are bound to the independently allowlisted trusted workflow blob. Executable examples contain no unresolved Action or verifier placeholder. External Actions must remain pinned to their audited 40-character commits.

The PR workflow triggers only on `pull_request`, checks out the exact head with credentials disabled, and has only `contents: read`. It uploads exactly one fixed-name artifact. Uploaded PR numbers, SHAs, workflow strings, and digests remain hostile claims.

The publication workflow has no checkout. Its read-only verify job keys API lookups to the source run ID and attempt, checks repository ID/name, event, conclusion, workflow path/blob, independent PR/base/head association, and exactly one server-owned artifact ID/digest/size. It caps extraction at 64 MiB total, 16 MiB per regular file, 256 files, and 240-byte relative paths; links, devices, traversal, nested archives, and extra names are rejected. It then runs only the trusted verify-only implementation and creates a deterministic statement/archive.

The attestation job alone has `id-token: write` and `attestations: write`. The publication job alone has `pull-requests: write`, independently rechecks the PR head and artifact identity, and updates at most one constant-marker comment owned by `github-actions[bot]`. Neither job executes commands stored in an envelope.

Treat all `workflow_run` downloads as hostile until this chain completes. If GitHub cannot independently return the executed workflow blob, PR association, server artifact digest, or exact run attempt, fail closed.

## Updating pins

For every pinned Action or OCI base:

1. Read the upstream release notes and security advisories.
2. Resolve the release tag from the authoritative upstream repository or registry.
3. Verify the full commit or multi-architecture manifest digest independently.
4. Review the diff from the old pin and update the allowlist in `scripts/validate.py` in the same change.
5. Run the full Rust tests, repository validator, YAML parser check, and Docker build.

Never shorten a commit or pin a mutable tag such as `v7`, `main`, or `master`.

## GitHub plan limitations

Artifact attestation availability and verification behavior depend on repository visibility and the organization's GitHub plan and policy. In particular, private/internal repository support and API access can differ from public repositories. Confirm current GitHub documentation and organization settings before treating attestations as a required release gate. The local schema-v3 verifier performs no network lookup; GitHub issuer/workflow/repository/ref verification remains the responsibility of the trusted publication workflow and GitHub tooling.
