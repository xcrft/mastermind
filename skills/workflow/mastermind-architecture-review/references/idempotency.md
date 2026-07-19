# Idempotency

Use this reference for retryable commands, webhooks, jobs, payments, queue
consumers, imports, and at-least-once delivery.

## Define the logical operation

Idempotency means repeating the same logical operation produces no additional
externally visible effect. Establish:

1. Operation identity and who creates it.
2. Key scope: tenant, actor, endpoint, resource, and operation type.
3. Durable owner of the key and result.
4. Atomic relationship between claiming the key and applying the side effect.
5. Behavior while the first attempt is in progress.
6. Response or outcome replay for completed attempts.
7. Retention and what makes reuse safe after expiry.

An HTTP verb, an in-memory set, or a key field by itself is not proof.

## Failure sequences worth testing

- Two replicas receive the same operation concurrently.
- The side effect commits and the process dies before recording completion.
- The key is recorded before the side effect, then the side effect fails.
- A downstream dependency succeeds but the caller sees a timeout.
- The same key is reused across tenants or different payloads.
- Deduplication expires before the upstream retry window ends.
- A redelivered message arrives while the original is still running.

## Safe evidence

- A uniqueness constraint or compare-and-set at the durable owner.
- A transaction that binds claim, state transition, and durable outcome.
- A documented recovery state for in-progress operations.
- Payload hash or immutable operation binding that rejects key misuse.
- Downstream idempotency or a compensating/reconciliation mechanism when the
  external side effect cannot share the local transaction.

## Verification

Run concurrent duplicates and crash-point tests. Assert effect count, durable
state, returned result, and recovery after restart; a sequential happy-path
retry is insufficient.
