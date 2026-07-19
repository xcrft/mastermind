# Backward compatibility

Use this reference for APIs, events, schemas, persisted data, configuration,
CLI output, SDKs, migrations, and independently deployed producers/consumers.

## Identify the compatibility window

1. Active and lagging readers, writers, producers, and consumers.
2. Stored payloads or records that will be replayed after deployment.
3. Oldest supported version and rollback target.
4. Rollout order and the period when mixed versions coexist.
5. Defaults, unknown fields, missing fields, and unknown enum behavior.

Repository-local callers are not a complete consumer inventory for public,
event-driven, persisted, or cross-service contracts.

## Common breaking changes

- Renaming, removing, or changing the type or meaning of a field.
- Making an optional field required without a compatible default.
- Reusing an enum value or rejecting values introduced by a newer writer.
- Changing error, pagination, ordering, timeout, or retry semantics.
- Rewriting stored data before old code can read or roll it back.
- Deploying a new producer before old consumers tolerate its payload.
- Treating an additive field as safe when signatures or strict decoders reject it.

## Safer evolution patterns

- Expand readers before writers; contract only after the compatibility window.
- Dual-read or dual-write only with an owner, convergence rule, and removal gate.
- Version envelopes when semantics cannot evolve additively.
- Preserve old stored/event forms until replay and rollback windows close.
- Add consumer-driven contract and fixture replay tests for every supported form.

## Verification

Exercise the rollout matrix: old reader/new writer, new reader/old writer,
mixed deployments, stored replay, rollback, and malformed or unknown values.
State which combinations are supported rather than saying "backward compatible"
without a window.
