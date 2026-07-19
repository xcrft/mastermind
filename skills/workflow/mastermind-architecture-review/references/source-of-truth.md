# Source of truth

Use this reference when a design adds or changes authoritative state, caches,
indexes, replicas, materialized views, projections, or dual writes.

## Ownership questions

1. Which component accepts the authoritative write?
2. Which durable record decides conflicts and recovery?
3. Which copies are derived, and how are they rebuilt?
4. Can reads observe a derived copy after its authority has changed?
5. Who reconciles missed, duplicated, delayed, or out-of-order updates?
6. What happens when authoritative and derived state disagree?

Do not infer authority from the fastest or nearest read path. Name the owner,
write contract, and recovery source explicitly.

## Risk patterns

- A command writes only a cache or search index.
- Two services can author the same business field.
- A dual write has no transaction, outbox, or reconciliation path.
- Cache invalidation is best-effort while authorization depends on it.
- A migration temporarily makes both old and new stores writable.
- A derived state rebuild loses ordering or version information.
- Conflict resolution is implicit last-write-wins across unsynchronized clocks.

## Required design evidence

- Authoritative write and read-after-write behavior.
- Derivation or replication mechanism and its delivery guarantees.
- Version, sequence, or conflict rule where updates can race.
- Rebuild/backfill path and the source it trusts.
- Failure recovery for partial dual writes.
- Observability for divergence and reconciliation lag.

## Verification

Test divergence deliberately: delay projection updates, drop one dual write,
reorder events, rebuild derived state, and read during recovery. The assertion
should name which value wins and why.
