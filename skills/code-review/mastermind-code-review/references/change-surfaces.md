# Change-surface routing

Choose sections from the changed behaviour and repository context. These are
questions for tracing risk, not standalone findings.

## Public contracts and compatibility

Trace exported symbols, package entry points, HTTP/RPC routes, event names,
tool schemas, serialised field names, enum values, configuration keys,
environment variables, database shapes, ABI layout, artifact names, workflow
parameters, defaults, and error forms. Check old and new readers during rolling
deployments, version negotiation, migration order, deprecation, and rollback.

## Data, transactions, and migrations

Trace transaction boundaries, uniqueness and foreign-key assumptions, tenant
scope, ordering, precision, timezone, nullability, retries, idempotency,
backfills, locking, and irreversible operations. Establish data size and current
deployment order before claiming a migration is safe.

## Authentication, authorisation, and isolation

Follow identity from authentication through tenant/resource resolution and the
final query or action. Check middleware order, anonymous exceptions, cache and
state keys, background jobs, service-to-service credentials, impersonation,
permissions, and failure defaults. Authentication alone does not prove
authorisation.

## Concurrency, lifecycle, and resources

Trace task ownership, cancellation, timeouts, retries, queue bounds,
backpressure, lock scope and ordering, thread affinity, subscriptions, handles,
connections, temporary files, process shutdown, redelivery, and crash recovery.
Look for cleanup on every exit path and for operations repeated after partial
success.

## Services and operability

Check startup configuration, dependency health, readiness versus liveness,
graceful shutdown, structured errors, correlation, retry classification,
bounded metrics labels, sensitive logs, capacity limits, rollout order,
feature flags, rollback, and whether the deployment actually exercises the
changed artifact.

## Dependencies and supply chain

Trace the owning manifest and lockfile, runtime versus tooling use, transitive
changes, supported runtimes and platforms, licences, native assets, package
entry points, feature selection, checksums/signatures, registry credentials,
generated artifacts, and downstream consumers. Do not infer functional proof
from installation or an audit alone.

## CI/CD, infrastructure, and containers

Determine expression phase, event and branch scope, token permissions, untrusted
input flow, reusable-workflow or template consumers, environment and
subscription targeting, plan replacements or deletion, remote state,
concurrency, approval gates, artifact provenance, secret exposure, platform
shell differences, image reproducibility, runtime identity, signals, health,
and rollback behaviour.

## Frontend and browser surfaces

Trace state ownership, stale async work, loading/empty/error/success states,
semantic HTML, accessible names, keyboard and focus behaviour, localisation,
HTML and URL trust boundaries, browser storage, extension messaging and
permissions, CSS isolation, public component contracts, bundle boundaries, and
server/client rendering assumptions.

## Native code, plugins, and FFI

Trace allocation and deallocation ownership, lifetime, aliasing, alignment,
layout, calling convention, panic/exception boundaries, dynamic loading,
signature verification, TOCTOU, IPC framing, platform guards, feature
unification, compiler/runtime pins, cross-compilation, and real-target evidence.

## Prompts, tools, protocols, and AI behaviour

Treat prompt text, model output, tool descriptions, schemas, routing config, and
retrieved context as executable behaviour. Trace instruction hierarchy, prompt
injection, unsafe tool authority, tenant and PII flow, structured output,
timeouts, idempotency, retry/redelivery, model and prompt versioning, fallback,
evaluation coverage, and human confirmation before destructive actions.

## ML, evaluation, and data assets

Trace data origin, consent and licence, split contamination, label semantics,
schema and preprocessing parity, deterministic seeds, model/tokeniser/runtime
pins, metric definition and direction, thresholds, subgroup regressions,
hardware precision, artifact integrity, reproducibility, and whether evaluation
measures the shipped behaviour rather than a proxy.

## Documentation and configuration

Verify commands, paths, branches, variables, defaults, links, anchors, diagrams,
schemas, duplicate sources of truth, compatibility of renamed keys, secret or
PII examples, environment targeting, and whether prose claims a stronger test
or rollout result than the evidence establishes.

## Tests and verification

Ask whether the changed behaviour is directly asserted, whether the test could
fail for the defect, and whether failure, cancellation, retry, authorisation,
tenant, cleanup, and platform paths matter. Separate lint, compile, unit,
integration, startup smoke, functional, live E2E, migration, recovery, and
performance evidence. One layer cannot substitute for another.
