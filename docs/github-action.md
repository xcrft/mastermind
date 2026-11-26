# GitHub audit Action

The Docker Action audits the task contracts a pull request changes and seals
the result as schema-v3 evidence for an exact repository revision. The example
workflows keep pull-request analysis apart from privileged publication, so pull
request code never runs with secrets or write permissions.

## Set up the workflows

1. Copy [PR analysis](examples/mastermind-audit-pr.yml) and
   [verification and publication](examples/mastermind-audit-publish.yml) into
   `.github/workflows/`. Keep their exact revision checks, pinned dependencies,
   artifact validation and split permissions.
2. Replace the `xcrft/mastermind` commit SHA in the PR example with the
   Mastermind commit you reviewed. A pinned Action runs its own `action.yml` and
   Dockerfile, so a pull request can't change the audit code.
3. Commit the task evidence as described below.

| Stage | Trigger | Permissions | Runs PR code |
|---|---|---|---|
| Analyze | `pull_request` | Contents read, no secrets or OIDC | Yes |
| Verify | `workflow_run` | Metadata and artifact reads | No |
| Attest | Verified artifact | OIDC and attestations write | No |
| Publish | Verified artifact | PR comment write | No |

Publication does no checkout. Before it attests or comments, it checks the
source run, attempt, repository and workflow identity, the PR base and head, and
the server-owned artifact identity. Missing metadata or artifact proof stops
publication. Attestation depends on the repository's GitHub plan and policy, so
check that it works before you make publication a required release gate.

## Commit task evidence

`mastermind init` keeps `.mastermind/` local, but the Action reads task
contracts and executor reports from Git. Uncomment these lines in
`.mastermind/.gitignore` and commit the two files of each task:

```gitignore
*
!.gitignore
!tasks/
!tasks/*/
!tasks/*/spec.md
!tasks/*/executor-report.md
```

A single `!path/to/file` line doesn't work here, because `*` already ignores the
parent directories. The Action audits each task folder that changed between the
baseline and HEAD, using its `spec.md` and a valid `executor-report.md`, and
leaves unchanged tasks out. Missing evidence gives no publishable result.

## Action inputs

| Input | Value |
|---|---|
| `root` | Repository-relative root under `GITHUB_WORKSPACE` |
| `since`, `expected-baseline` | The same full baseline commit OID |
| `expected-head` | Full head commit OID |
| `expected-repository` | Exact `owner/repo` |
| `bundle-dir` | A new repository-relative output directory |
| `require-clean-worktree` | `true` for publication |

The outputs `bundle-dir` and `result-json` name the verified bundle and the
aggregate result. Inputs are data and never run as shell commands. The
[Action definition](../action.yml) is the full contract.

## Check an envelope locally

From a clean checkout, with your own task path and repository, and with the
bundle inside the ignored `.mastermind/` so it doesn't dirty the checkout:

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

# Optional signature
mastermind audit sign .mastermind/audit.bundle.json \
  --private-key /private/path/audit-ed25519.seed \
  --signature .mastermind/audit.bundle.sig.json
mastermind audit verify .mastermind/audit.bundle.json \
  --signature .mastermind/audit.bundle.sig.json \
  --public-key /private/path/audit-ed25519.pub \
  --require-signature --trusted-key-id "sha256:<public-key-digest>"
```

The private key is one line of base64 with a 32-byte Ed25519 seed and Unix mode
`0600`. The signature is a detached, domain-separated schema-v1 statement over
the envelope schema, hash algorithm, canonicalization, key ID and manifest
digest. When you pass repository and signature policies, both must pass. Keep
trusted and revoked key IDs in separately reviewed policy. Signing time is not
recorded, so Ed25519 alone can't tell a signature made before a key compromise
from one made after.

## What a passing result shows

`audit-spec --bundle` seals the mechanical report as canonical JSON with a
SHA-256 manifest digest. `audit verify` passes only with a complete repository,
baseline, head, root and clean-worktree policy, a required signature from a
trusted key ID that is not revoked, or both.

| Evidence | Shows | Doesn't show |
|---|---|---|
| Matching canonical digest | The envelope is internally consistent | Authenticity or finding accuracy |
| Signature under a trusted key | The key approved those bytes | Independent source inspection, a person's identity or signing time |
| Repository policy checks | The repository and revisions match | Semantic correctness |
| GitHub artifact attestation | The publication workflow verified the archive and statement | That PR analysis ran in a trusted environment, or finding accuracy |
| `--integrity-only` | Digest consistency only, labeled untrusted, with authenticity and policy `not_evaluated` | Anything fit for publication |

Keep external Actions pinned to full commits and OCI images to digests, and
review upstream changes before you update the pins and the validator's
allowlist.
