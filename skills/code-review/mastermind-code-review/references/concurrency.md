# Concurrency Best Practices

These cross-cutting rules supplement language rules for lock scope, task
ownership, atomic SQL writes, and durable messaging. A detector is a lead;
identify a reachable competing schedule and an observable contract failure.

## Trace the transition

Review only risks the change affects.

1. Name the shared resource and its scope: request, session, device, process,
   database, tenant or replica group. A single event-loop thread can still
   interleave tasks at each `await`.
2. Find every reader, writer, admission check and cleanup path. Locate the
   authoritative update and the primitive that makes it indivisible.
3. Trace two competing operations through check, reserve, execute, commit and
   release. Check that the protection covers every participant. A local mutex
   does not coordinate replicas; separate atomics do not protect compound state.
4. Trace lock order, and re-entrant callbacks or waits while a lock is held.
   Check whether awaited work needs a resource its caller still holds.
5. Follow timeout and cancellation to real completion. Dropping a waiter does
   not stop blocking work, a remote request or a foreign call. Check that
   obsolete work cannot commit or release resources owned by a newer generation.
6. Follow shutdown and reload through stopped admission, drain or cancel,
   observed completion and release. Bound fan-out, queues and retries.

For session handlers, keep history and cancellation state at session scope.
Trace a second message arriving before the first completes, two sessions,
cancel followed by another turn, and disconnect during a tool call. Replacing a
current-task pointer does not transfer ownership of the earlier worker.

For each candidate, build the interleaving and look for the invariant that would
refute it. Report a defect only with a reachable path and a concrete impact.
Stress repetitions, a passing serial run or the presence of a lock do not prove
the absence of a race.

## Enforceable rules

### concurrency-atomic-authoritative-transition

**Name:** Protect the authoritative state transition

**Rule:** Admission, reservation, deduplication, and state updates shared by
competing operations must enforce their invariant at the authoritative boundary
with one strategy covering every participant and the real sharing scope.

**Why:** Check-then-act and replica-local protection permit duplicate execution,
lost updates, and inconsistent decisions.

**Detect:** A change separates a check from its dependent update or introduces
a competing reader/writer without protection spanning both. Prove the competing
path and impact; independent calls or a mutable field alone are not a finding.

### concurrency-reject-obsolete-completion

**Name:** Prevent obsolete work from committing

**Rule:** When work can outlive cancellation, timeout, replacement, or reload,
its completion must not update or release state owned by a newer operation.
Use the established ownership, identity, or generation contract.

**Why:** A late result can overwrite current state, repeat a side effect, or
release resources still in use.

**Detect:** A changed timeout/retry/reload path allows an older operation to
complete against reused state without an authoritative ownership check.
Prove that the work can continue and reach the mutation.

### concurrency-release-after-quiescence

**Name:** Release resources after their users stop

**Rule:** Shutdown, unload, and test teardown must observe completion of users
before freeing their resources. A deadline may end waiting but cannot make a
live callback, worker, or native library reference safe to release.

**Why:** Early release causes use-after-free, crashes, and cross-test corruption.

**Detect:** A changed lifecycle releases captured state or unloads code after a
timeout/cancel signal while a reachable user can still execute.

## Advisory rules

### concurrency-prove-progress-and-resource-ownership

**Name:** Prove progress and safe concurrent resource use

**Rule:** Concurrent fan-out needs independent operations, permitted resource
sharing, bounded acquisition, and a progress path through waits and reentrancy.

**Why:** Shared sessions, recursive waits, and unbounded work can deadlock,
corrupt state, or exhaust resources.

**Detect:** A change introduces fan-out or waits using a shared resource whose
contract forbids concurrent use, or a dependency cycle requiring a held resource.
Do not require parallelisation or serialisation without this evidence.
