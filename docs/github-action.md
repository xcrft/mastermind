# GitHub audit Action

The Docker Action audits changed task contracts and produces schema-v3 evidence
for an exact repository revision. The accompanying workflows separate
pull-request analysis from privileged publication.

## Set up the workflows

Start from these maintained examples:

- [PR analysis](examples/mastermind-audit-pr.yml)
- [Verification and publication](examples/mastermind-audit-publish.yml)

Review them for the destination repository before copying them into
`.github/workflows/`. Retain the exact revision checks, pinned dependencies,
artifact validation, and separation of permissions.

| Stage | Trigger | Permissions | Executes PR code |
|---|---|---|---|
| Analyze | `pull_request` | Contents read, no secrets or OIDC | Yes |
| Verify | `workflow_run` | Metadata and artifact reads | No |
| Attest | Verified artifact | OIDC and attestations write | No |
| Publish | Verified artifact | PR comment write | No |

Publication performs no checkout. Before attestation or a comment, it verifies:

| Binding | Required match |
|---|---|
| Workflow execution | Source run, attempt, repository and workflow identity |
| Change | PR base and head |
| Artifact | Server-owned artifact identity |

Downloaded PR artifacts remain untrusted until all checks pass.

## Provide task evidence

| Task selection | Result |
|---|---|
| Canonical task folder changed between baseline and HEAD | Audit its `spec.md` and valid `executor-report.md` |
| Unchanged historical task | Excluded from this audit |
| Missing evidence or no results | No publishable evidence |

The [Action definition](../action.yml) accepts:

| Input | Value |
|---|---|
| `root` | Repository-relative root under `GITHUB_WORKSPACE` |
| `since`, `expected-baseline` | The same full baseline commit OID |
| `expected-head` | Full head commit OID |
| `expected-repository` | Exact `owner/repo` |
| `bundle-dir` | A new repository-relative output directory |
| `require-clean-worktree` | `true` for publication |

Outputs identify the verified bundle directory and aggregate result JSON.
Inputs are data and are not evaluated as shell commands.

## Inspect an envelope locally

Use a clean checkout and actual full commit IDs:

```bash
BASELINE_OID=$(git merge-base HEAD origin/main)
HEAD_OID=$(git rev-parse HEAD)

mastermind audit-spec .mastermind/tasks/005-example/spec.md \
  --since "$BASELINE_OID" --root . \
  --executor-report .mastermind/tasks/005-example/executor-report.md \
  --bundle .mastermind/audit.bundle.json

mastermind audit verify .mastermind/audit.bundle.json --root . \
  --expected-repository owner/repo \
  --expected-baseline "$BASELINE_OID" --expected-head "$HEAD_OID"
```

The baseline must identify the reviewed change range. Replace the example task
and repository identity. Keep generated bundles in the ignored `.mastermind/`
directory so they do not dirty the checkout. For optional signing:

```bash
mastermind audit sign .mastermind/audit.bundle.json \
  --private-key /private/path/audit-ed25519.seed \
  --signature .mastermind/audit.bundle.sig.json

mastermind audit verify .mastermind/audit.bundle.json \
  --signature .mastermind/audit.bundle.sig.json \
  --public-key /private/path/audit-ed25519.pub \
  --require-signature --trusted-key-id "sha256:<public-key-digest>"
```

Protect private seed files. Keep trusted and revoked key IDs in independently
reviewed policy. When snapshot and signature policies are supplied, both must
pass.

## Interpret the result

| Evidence | Establishes | Does not establish |
|---|---|---|
| Matching canonical digest | Envelope content is internally consistent | Authenticity or finding accuracy |
| Signature under a trusted key | The signing key approved those bytes | Independent source inspection or human identity |
| Repository policy checks | Required repository and revisions match | Semantic correctness |
| GitHub artifact attestation | Archive and statement passed through the attestation workflow | Finding accuracy |
| `--integrity-only` | Digest consistency only | Authenticity or policy admission. Unsuitable for publication |

See the [audit-envelope reference](reference/mmcg.md#schema-v3-audit-envelopes)
for canonicalization, signatures, trust anchors, and validation limits.

## Maintain the integration

Keep external Actions pinned to full commits and OCI images to immutable
digests. Review upstream changes before updating pins and the repository
validator's allowlist. Run the required validation described in
[Contributing](../CONTRIBUTING.md).

Attestation availability depends on the repository's GitHub plan and policy.
Confirm it before making publication a required release gate. Missing required
metadata or artifact proof must stop publication.
