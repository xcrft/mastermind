---
name: mastermind-dependency-maintenance
description: Keep dependency changes narrow, reproducible and compatible — audit update-bot coverage, assess and apply a version upgrade, or remove an unused package with evidence. Use for dependency upgrades, lockfile changes, package manifests, Dependabot or Renovate configuration and grouping, version compatibility questions, and dependency clean-up.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - ops
    - dependencies
    - supply-chain
---

# Mastermind dependency maintenance

Change as few packages as the request needs, keep the lockfile in step with the
manifest, and prove the result still builds, loads and behaves.

## 1. Inventory before editing

1. Read the repository instructions and find every relevant manifest, lockfile,
   workspace definition, package-manager config and generated binding.
2. Decide which request this is: update-bot configuration, a version update, or
   removing a package that looks unused. A concrete security alert goes to
   [[mastermind-security-alert-triage]] first; come back here to apply the
   update.
3. Classify each affected package as direct or transitive, and as runtime,
   build, development, tooling or deployment-only.
4. Note the supported runtimes, compiler versions, targets, feature flags,
   private registries, published packages and downstream consumers.

Do not remove compatibility dependencies, credential plumbing, environment
variables, package-manager settings or optional platform paths without direct
evidence that nothing supported still uses them.

## 2. Audit update-bot coverage

Compare the bot configuration (for example `.github/dependabot.yml` or a
Renovate config) with the repository as it is now:

- cover each maintained ecosystem and manifest folder, including CI actions
  when workflows exist;
- target the real integration branch and respect monorepo boundaries;
- group only updates that can be checked and rolled back together;
- keep major or risky runtime updates separate so they get reviewed;
- reference private-registry credentials through secrets, never inline;
- keep version-update config apart from repository settings for the
  dependency graph, alerts and security updates.

Check the bot's current official documentation before using a rare or new
option. A valid YAML file does not prove every workspace is covered.

## 3. Assess an update

1. Pin down the exact old and new versions and why the update is needed.
2. Read the release notes and migration guide from the primary source.
3. Check runtime, compiler, peer-dependency, feature, ABI, schema, licence and
   deployment compatibility.
4. Trace changed public APIs to direct consumers with `mmcg_imported_by`
   (`match: path` for a fully qualified import) and `mmcg_callers`, then read
   the call sites. Dynamic loading, install scripts and generated types are
   invisible to the graph; check them by hand.
5. Separate required migration work from optional clean-up. Ask before turning
   a dependency-only request into a wider removal.

## 4. Apply the smallest update

Use the repository's package manager and documented commands. Update the
manifest and lockfile together; do not hand-edit generated lockfile sections.
Do not refresh unrelated packages or reformat whole manifests. Keep exact pins
and feature selections that the repository documents as contracts.

The dependency reference in [[mastermind-code-review]] lists the defects a
reviewer will look for.

## 5. Verify and hand off

Choose checks with [[mastermind-change-verification]]. For dependency work that
usually means: resolution and duplicate-version inspection, the repository's
audit command, compile, type check, lint and affected tests, and a packaging or
start-up smoke test where runtime loading can change. A clean install or a
clean audit alone is not functional proof.

Report the changed versions and why, compatibility findings, the manifest and
lockfile scope, the commands run with results, unavailable live integrations,
and any remaining consumer or rollout risk.
