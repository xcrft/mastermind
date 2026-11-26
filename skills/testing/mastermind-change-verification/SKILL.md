---
name: mastermind-change-verification
description: Choose and run proportionate checks that prove a change works, diagnose a failed CI job from its leaf log, and report exactly what was and was not exercised. Use when asked to verify, validate or test a change, to show it is ready, to explain a red CI run, or to prepare verification evidence for a pull request or release.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - testing
    - verification
    - ci
---

# Mastermind change verification

A passing command proves only what that command exercised. Pick the checks that
cover the requested behaviour and its real risks, run them, and report the gap
between what ran and what is still unproven.

## 1. Fix the target

1. Resolve the baseline, the final revision, the changed paths and the
   environments that matter.
2. Read the repository instructions, README, manifests and CI workflows before
   choosing a command. Use the repository's own commands, not generic ones.
3. Map each requested behaviour or material risk to the narrowest direct check.
   [[mastermind-test-impact]] ranks candidate tests from the changed symbols.
4. If the task is review or diagnosis only, stay read-only. Fix things only when
   the task also asks for the fix.

## 2. Pick evidence by layer

Each layer proves something different. Do not let one stand in for another:

- dependency resolution or install
- format, lint, type check and compile
- affected unit tests
- integration and contract tests
- package or production build
- start-up or process smoke test
- behaviour at a real boundary (HTTP, CLI, queue, browser)
- end-to-end with live integrations
- security, migration, concurrency, performance or recovery checks, when the
  change creates that risk

A build is not a smoke test. A mocked test is not live integration proof. Do not
run every suite when a smaller check covers the contract, and do not use a
narrow unit test to claim cross-service compatibility. For a UI change, record
browser evidence with [[mastermind-browser-verification]].

## 3. Prove a regression test, when protection matters

During review, judge the existing tests and say what they cannot prove
([[mastermind-test-audit]]). When running checks is in scope:

1. Keep the new test, and in a separate checkout or stash revert or break the
   production line it protects.
2. Record the test passing on the final code and failing on the broken variant
   for the expected reason. A set-up, compile or unrelated failure does not
   count.
3. Match the CI layout. Serial, shared-process parallel and process-per-test
   runs exercise different isolation.
4. Restore the code and confirm the working tree matches the final revision.

## 4. Run and diagnose

1. Run focused checks first, then widen in order of risk.
2. Record the exact command, revision, relevant environment, exit status and
   the result that matters.
3. Read the whole failure before retrying. Classify it: product defect, test
   defect, environment limit, flaky, unrelated pre-existing failure, or
   unresolved.
4. If a fix is in scope, make the smallest change and rerun the failed check
   plus any boundary the fix touches.
5. Before reporting, review the final diff and working tree for generated
   noise, unrelated edits, leaked credentials and uncommitted files.

Never retry until green without a reason, and never hide a failing check
because another one passed. Stop after two retries of the same failure with no
new evidence, and report it as unresolved.

### A failed CI run

Start from the failed leaf job and its log at the current head, not the summary
or the required-check badge. Then:

- Separate checkout, dependency restore, compile, test, packaging, publish and
  deploy failures; the same red status hides different owners and fixes.
- Confirm the runner OS and architecture, toolchain version, template or
  reusable-workflow ref, expanded inputs, variables and credentials the job
  actually used. Do not assume Linux, Windows and macOS jobs share a
  filesystem, shell, cache, compiler or signing set-up.
- Trace each artifact to the job and commit that built it. A later green job or
  a local rebuild is not evidence for the failed artifact.
- Reproduce at the nearest stable boundary. A local run can isolate a compile
  or test defect, but runner permissions, hosted images, secrets, signing and
  artifact movement need a hosted rerun after the fix.

## 5. Release evidence

For a release, keep each step separate:

1. source and local checks;
2. hosted build and test at the exact commit;
3. immutable artifact identity and digest;
4. artifact promotion or configuration change;
5. pull, start-up, readiness and contract checks in the target environment;
6. explicit approval, plus rollback readiness;
7. observed traffic and service behaviour.

Later evidence does not prove an earlier step. A promoted artifact is not proof
that instances pulled it; ready is not proof that the contract works; approval
is not traffic. Never overwrite an immutable tag to make environments agree.

## Report

Lead with the verdict: **pass**, **partial**, **fail** or **inconclusive**.
Then list:

- each check, the command, its result, and the behaviour or risk it covers;
- checks skipped as irrelevant, and why;
- checks blocked by missing credentials, services, hardware, data or platform;
- what still needs live, functional, rollout or compatibility evidence.

Never describe an unrun check as passing, and never present an old CI result as
fresh evidence.
