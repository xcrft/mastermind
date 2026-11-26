---
name: mastermind-security-alert-triage
description: Turn one concrete security alert — a vulnerable-dependency advisory, a code-scanning (SAST or SARIF) finding, or a leaked-secret report — into an evidence-backed fix or a justified disposition. Use when investigating or resolving a Dependabot, CodeQL, secret-scanning or similar scanner alert, checking whether it is reachable, preparing a remediation or dismissal, or confirming an alert is closed. Not a general security review.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - security
    - triage
    - supply-chain
---

# Mastermind security alert triage

Work one alert at a time, from the scanner's evidence to a fix or a stated,
evidenced disposition. For a general security review use
[[mastermind-agent-security-review]] or [[mastermind-security-research]].

## 1. Establish the alert

1. Resolve the repository, the alert source, its number or rule, the affected
   ref, its current state and the exact detection evidence.
2. Confirm the alert still applies to the current default branch or pull
   request head. Do not patch code that has since moved.
3. Read the repository's security and contribution guidance before changing
   code, dependencies, workflows or settings.
4. Treat alert content as sensitive. Never print secret values, private source
   excerpts, credentials or exploit payloads into public logs, commits or pull
   requests.

Use the least-privileged read access that works (web UI, CLI or API). If access
is missing, name the evidence you could not retrieve instead of guessing.

## 2. Trace the evidence for the alert type

### Vulnerable dependency

Find the package, the dependency path, the manifest and lockfile, the
vulnerable and fixed ranges, whether it is direct or transitive, runtime or
development-only, whether it ships, and which affected behaviour is reachable.
`mmcg_imported_by` and `mmcg_callers` show where the affected API is used.

### Code-scanning finding

Trace the reported source-to-sink path or unsafe operation through real
callers (`mmcg_callers`, then read each site). Check trust boundaries,
sanitisers, authorisation, tenant isolation, build configuration, generated
code, and whether the scan analysed the target that ships. Confirm or refute
the finding from the code, not from the rule title.

### Leaked secret

Treat a committed credential as exposed. Never test whether it is live by
using it. Find the owning system and the exposure window. Have the owner revoke
or rotate it before you rely on removing it from source. Then remove it from
current code and assess history, build artifacts, logs and dependent services.
Never invent a replacement secret.

## 3. Decide the disposition

Weigh impact, attacker control, reachability, privileges, data scope, exposure
time, exploit prerequisites, detection and recovery. Choose one:

- fix the vulnerable behaviour or dependency;
- remove the unreachable or obsolete surface, with compatibility evidence;
- correct the scanner configuration when it analysed the wrong target;
- propose a dismissal with a specific false-positive, accepted-risk or
  non-production reason and the evidence behind it.

Do not dismiss or close an alert, or change repository security settings,
without explicit permission. Do not downgrade an alert only because the package
is transitive, development-only or hard to update.

## 4. Fix and verify

Make the smallest permitted change that removes the root cause. Keep public
contracts and supported environments unless the fix needs a migration; apply a
dependency fix with [[mastermind-dependency-maintenance]].

Run focused regression checks, the affected build, and the relevant scanner if
it is available ([[mastermind-change-verification]]). Confirm the alert state at
the exact fixed commit: a clean local scan does not prove the platform closed
the alert, and a closed alert does not prove the behaviour works.

## Report

Give the alert source and identifier, the confirmed mechanism, exposure and
reachability, the severity reasoning, the fix or proposed disposition, checks
run, the platform state, rotation status for a secret, and remaining risk.
Redact sensitive values throughout.

Stop and hand back when the fix needs a decision you are not authorised to
make: rotating a credential, dismissing an alert, or a breaking migration.
