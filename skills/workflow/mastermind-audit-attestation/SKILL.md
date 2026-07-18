---
name: mastermind-audit-attestation
description: Generate, verify, sign, or explain Mastermind audit envelopes and GitHub attestations. Use when checking audit-bundle tampering, validating exact repository/baseline/head policy, working with detached Ed25519 signatures, reviewing CI provenance, or distinguishing content integrity from signer authenticity and policy acceptance.
metadata:
  version: 0.1.0
  authors: [mastermind]
  tags: [workflow, security, audit]
---

# Mastermind Audit Attestation

Always keep three questions separate: is the content internally intact, who or
what authenticated it, and does the verifier's policy accept those claims?

## Generate

Create a sealed bundle from an actual spec audit, using an explicit baseline
and repository-relative output path:

```bash
mastermind audit-spec SPEC --since REF \
  --executor-report REPORT --bundle audit-bundle.json
```

Generation is not verification. Record the resolved repository, full baseline
and HEAD OIDs, clean-worktree expectation, and bundle path so verification can
use independently trusted values.

## Verify

Run with independently trusted expectations:

```bash
mastermind audit verify BUNDLE \
  --expected-repository OWNER/REPO \
  --expected-baseline FULL_OID \
  --expected-head FULL_OID \
  --root . --json
```

When a detached signature is required, add both `--signature` and
`--public-key` plus `--require-signature`. Treat a present but invalid signature
as failure; never ignore it.

Explain `content_integrity`, `provenance_authenticity`, and
`policy_acceptance` independently. An unsigned hash is tamper-evident only
relative to trusted expected inputs; it does not prove authorship. A valid key
signature proves control of that key, not identity outside key policy. A GitHub
attestation proves only the issuer/workflow/repository/ref accepted by the
verification policy.

## Sign and CI

Sign only when the user explicitly requests it and the private key has safe
permissions:

```bash
mastermind audit sign BUNDLE --private-key KEY --signature OUT.json
```

Never display or copy a private key into the repository. For GitHub Actions,
require full commit pins, exact head/repository checks, no secrets or OIDC in
the PR analysis job, and no PR checkout/execution in privileged
`workflow_run` jobs. Treat downloaded artifacts as hostile until digest,
workflow identity, and envelope policy are verified.

Never call an artifact tamper-proof. Report key rotation, revocation, verifier
time, or GitHub-plan limitations when they affect acceptance.
