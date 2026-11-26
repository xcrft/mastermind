# CI/CD Workflow and Template Best Practices

Detailed rules for CI/CD workflows, reusable workflows, actions, jobs, stages
and shared templates, in GitHub Actions or any other pipeline system. Treat
inputs, defaults, outputs, artifact names, conditions and template paths as
public contracts.

## Enforceable rules

### ci-cd-no-secrets-in-untrusted-code

**Name:** Keep secrets away from untrusted execution

**Rule:** Pull-request code, fork-controlled input, issue text, artifact content,
and mutable external scripts must not execute with privileged secrets or tokens.

**Why:** An attacker can modify the executed code or data to exfiltrate the
credential and compromise repositories or deployment environments.

**Detect:** A diff adds `pull_request_target`, privileged PR/fork execution,
artifact/script download, or checkout of an untrusted ref in a job that exposes
secrets, a write token, a service connection, or deployment credentials.

### ci-cd-minimum-token-and-service-permissions

**Name:** Grant only required pipeline permissions

**Rule:** Set token permissions and cloud credential, service-connection and
environment scope to the minimum needed for that job and stage.

**Why:** A compromised step inherits the workflow's authority and blast radius.

**Detect:** A diff adds write/admin permission, broad service connection, or
production environment access to a job whose changed steps only require read or
a narrower environment/resource scope.

### ci-cd-pin-executable-dependencies

**Name:** Pin external executable workflow dependencies

**Rule:** Pin third-party GitHub actions, templates, scripts, tools, and build
images according to repository provenance policy; immutable digests or commits
are required where policy demands them.

**Why:** A mutable tag, branch, URL, or image can execute different code without
a reviewed repository change.

**Detect:** A diff introduces a third-party action/template/script/image at a
floating branch, latest tag, or unverified download where adjacent policy pins
an immutable revision or integrity value.

### ci-cd-preserve-template-contracts

**Name:** Preserve reusable workflow and template APIs

**Rule:** Input names/types/defaults, output variables, conditions, artifact
names, stage/job names, and template paths must remain compatible with existing
consumers or be migrated together.

**Why:** Shared template changes can break many repositories at compile time or,
worse, silently skip deployment behaviour.

**Detect:** A diff removes/renames or changes the default/meaning of a reusable
input, output, artifact, stage/job, or path without a compatibility alias,
version boundary, or consumer updates.

### ci-cd-use-the-correct-expression-phase

**Name:** Evaluate values in the correct pipeline phase

**Rule:** Use compile-time/template, job condition, macro, and runtime
expressions only where their inputs exist and escaping semantics match.

**Why:** A valid-looking expression can resolve empty, use stale data, or cause
a stage to run/skip incorrectly because it is evaluated in another phase.

**Detect:** A diff references a runtime output in compile-time syntax, a secret
or macro in a template expression, or a step output before its producing
step/job, with no supported phase bridge.

### ci-cd-do-not-swallow-required-failures

**Name:** Required failures must fail the owning gate

**Rule:** Do not use unconditional success, ignored exit codes,
`continue-on-error`, or broad conditions for a check or deployment action whose
failure must prevent later publication or rollout.

**Why:** The pipeline can publish or deploy an artifact that failed its required
validation or partially failed a destructive action.

**Detect:** A diff makes a security, build, test, migration, signing, publish, or
deployment step non-blocking and downstream stages do not explicitly consume
and enforce its result.

### ci-cd-protect-environment-and-branch-routing

**Name:** Deploy only from the intended source to the intended target

**Rule:** Branch/ref filters, environments, subscriptions, regions, variable
groups, service connections, and approval gates must agree on the deployment
target.

**Why:** A typo or fallback can deploy feature or development artifacts into a
production target with valid credentials.

**Detect:** A diff broadens a trigger or changes ref/environment selection so an
unprotected branch can reach a protected credential/target, or maps one
environment to another environment's variables or service connection.

### ci-cd-select-required-review-and-tests

**Name:** Select required review and execution at every supported transition

**Rule:** Events, branch/path filters, job dependencies, and conditions must
select required reviews and tests for every supported change and PR lifecycle.
A draft-skipping audit needs a ready-for-review trigger. Independent review
feedback must not disappear solely because a build dependency failed.

**Why:** A valid workflow can silently omit the affected suite or leave a
promoted PR with no current review result.

**Detect:** Establish the supported change/lifecycle and trace it through
selection. Report a reachable omitted required check, rather than assuming every
repository supports every branch or needs an unconditional whole-workspace run.

### ci-cd-artifacts-have-provenance-and-integrity

**Name:** Preserve artifact identity between build and deploy

**Rule:** Deploy the immutable artifact produced and verified by the owning
build; do not rebuild from a mutable ref or accept an ambiguous external
artifact without provenance and integrity checks.

**Why:** The deployed bytes can differ from those reviewed, tested, signed, or
approved.

**Detect:** A diff makes deployment download a mutable/latest artifact, rebuilds
from branch head after approval, or drops a digest/signature/version association
used by the verified build.

## Advisory rules

### ci-cd-bound-concurrency-and-retries

**Name:** Serialise conflicting runs and bound retries

**Rule:** Use concurrency, environment locks, idempotent operations, and bounded
retry where overlapping runs can mutate the same state or publish identity.

**Why:** Parallel or repeated deployment can race state, tags, migrations,
signing, and rollback.

**Detect:** A diff enables overlapping writes to the same environment, state,
version, artifact, or branch, or adds an unbounded retry around a mutating step.

### ci-cd-quote-for-the-actual-agent-shell

**Name:** Preserve shell and platform semantics

**Rule:** Commands, paths, variables, and line continuations must be valid for
the selected Windows/Linux/macOS agent and exact shell.

**Why:** YAML can parse while PowerShell, Bash, cmd, or task argument parsing
changes quoting, globbing, drive paths, or exit behaviour.

**Detect:** A diff changes agent OS/shell or shares one command across platforms
while using syntax, separators, quoting, or paths valid for only one target.

### ci-cd-clean-temporary-credentials-and-resources

**Name:** Clean up pipeline side effects

**Rule:** Temporary credentials, test infrastructure, mounts, agents, files,
and environment changes need cleanup that runs after failure and cancellation.

**Why:** Failed jobs can leave credentials or costly/conflicting resources for
later runs.

**Detect:** A diff creates a temporary credential/resource or mutates shared
agent state but cleanup is only on the happy path or lacks an always/finally
condition.
