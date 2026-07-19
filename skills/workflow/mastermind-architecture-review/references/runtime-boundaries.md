# Runtime boundaries

Use this reference when execution crosses a process, service, queue, trust,
serialization, plugin, or infrastructure boundary.

## Establish the real path

For every hop, identify:

1. Trigger and production entry point.
2. Transport and serialized request/event shape.
3. Admission checks: authentication, authorization, tenant, feature, or
   capability gates.
4. Domain operation and its caller-visible outcome.
5. State or external side effect.
6. Response, acknowledgement, emitted event, or retry decision.

Evidence should come from the registered route/consumer, adapter, schema,
configuration, deployment wiring, or an observed trace. Imports and filenames
only identify where to look.

## Boundary invariants

- Required identity and tenant context survives serialization.
- Validation occurs before privileged reads or writes.
- Timeout, cancellation, and error semantics are translated deliberately.
- Acknowledgement does not precede an unowned or unrecoverable side effect.
- Queue delivery guarantees match consumer behavior.
- Correlation identifiers cross the same boundary as the operation.
- Ownership is explicit when two components can act on the same state.

## Failure sequences worth proving

- The caller times out after the callee commits, then retries.
- Serialization drops a security- or routing-critical field.
- A worker acknowledges before persistence or external completion.
- The dependency succeeds but the response or follow-up event is lost.
- Two versions of a service disagree on error or enum semantics.
- A framework route exists in source but is not registered in production.

## Verification

Prefer an integration or contract test at the boundary. Assert both sides of
the crossing: serialized shape, admission context, side effect, response or
acknowledgement, and failure behavior. Use a trace or deployment inspection
when source alone cannot prove runtime registration.
