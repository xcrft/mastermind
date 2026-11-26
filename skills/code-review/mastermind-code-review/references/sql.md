# SQL and Migration Best Practices

Detailed rules for application SQL, stored procedures, schema changes,
migrations, and backfills. Apply the actual database engine, migration tool,
data size, availability target, and deployment order.

## Enforceable rules

### sql-parameterise-external-values

**Name:** Parameterise data values

**Rule:** Bind external values as typed parameters. For dynamic identifiers,
select from a strict allowlist and use the engine's identifier-quoting API.

**Why:** String concatenation permits injection and misparses quotes, encodings,
dates, and locale-specific values.

**Detect:** A diff concatenates/interpolates a request, user, tenant, file,
message, or configuration value into executed SQL without a parameter or
whole-value allowlist for an identifier.

### sql-scope-tenant-and-authorisation-predicates

**Name:** Keep tenant and ownership predicates in every access path

**Rule:** Reads, writes, joins, upserts, and deletes of tenant/user-owned data
must constrain the resolved owner at the database boundary.

**Why:** An application-side lookup or later filter can expose or mutate another
tenant's data under concurrency or alternate callers.

**Detect:** A changed query targets tenant/user-owned tables by a record key or
join alone while adjacent queries and schema show a tenant/owner key is required.

### sql-atomic-related-writes

**Name:** Commit related state changes atomically

**Rule:** Changes that must agree must share a transaction and handle rollback;
do not publish or acknowledge an event before the durable commit it represents.

**Why:** Failure between statements leaves partial state or tells another system
about data that never committed.

**Detect:** A diff adds multiple dependent writes or a write plus event/ack with
an error path between them and no owning transaction/outbox/idempotent recovery.

### sql-expand-before-contract

**Name:** Keep migrations compatible with rolling deployments

**Rule:** Add and backfill compatible schema before requiring it; remove or
rename only after old writers/readers are retired.

**Why:** During rollout, old and new application versions access the same
database and can fail against an atomic breaking schema change.

**Detect:** A diff drops/renames a used column/table/type, makes a populated
column immediately required, or changes stored representation without a
compatibility phase and ordered application migration.

### sql-bound-locking-and-backfills

**Name:** Bound migration locks and work

**Rule:** Large-table DDL and backfills need an engine-appropriate online,
batched, resumable plan with bounded transactions and retry behaviour.

**Why:** One migration transaction can lock production tables, fill logs, block
replication, or restart from zero after failure.

**Detect:** A diff adds a full-table update, rewrite, index build, or blocking
constraint to an established large/production table with no batching, online
method, progress key, or deployment window evidence.

### sql-guard-irreversible-operations

**Name:** Protect destructive database targets

**Rule:** Drops, truncation, broad deletes, and data rewrites must validate the
exact environment and target and have the repository's backup/rollback or
explicit irreversibility decision.

**Why:** A valid command against the wrong database or predicate causes
irrecoverable data loss.

**Detect:** A diff adds destructive SQL with a missing/broad predicate,
environment-derived target without validation, or no recovery path in a workflow
that can reach persistent data.

## Advisory rules

### sql-deterministic-bounded-reads

**Name:** Bound and order collection reads

**Rule:** Paginated, top-N, dequeue, and batch queries need a deterministic
unique order and a bounded result size.

**Why:** Rows can repeat, disappear, starve, or exhaust memory as plans and
concurrent writes change.

**Detect:** A diff adds `TOP`/`LIMIT`/pagination/batch selection without a stable
unique ordering, or exposes an unbounded collection on a production path.

### sql-preserve-time-numeric-and-null-semantics

**Name:** Preserve value semantics across storage

**Rule:** Choose explicit precision/scale, timezone, collation, and null/default
semantics that match the application contract.

**Why:** Implicit storage conversions silently round money, shift time, change
comparison, or conflate missing with a real value.

**Detect:** A diff changes a column/expression for currency, time, identifiers,
or optional data to an implicit/lower-fidelity type or default without a data
conversion and consumer update.

### sql-indexes-match-changed-access

**Name:** Support material new access paths

**Rule:** A new high-volume filter/join/order path needs an index strategy that
matches its leading predicates without duplicating an equivalent index.

**Why:** A correct query can become a production scan, while unnecessary indexes
increase write cost and lock duration.

**Detect:** A diff adds a demonstrated hot/large-table access pattern whose
filter/join/order cannot use an existing index visible in the changed schema, or
adds a redundant near-duplicate index.

### sql-handle-concurrent-updates

**Name:** Make read-modify-write concurrency explicit

**Rule:** Use an atomic statement, uniqueness constraint, concurrency token, or
appropriate isolation when concurrent writers can target the same logical row.

**Why:** Separate read and update/insert steps lose updates or create duplicates.

**Detect:** A diff adds check-then-insert or read-calculate-update across
separate statements with no transaction lock, atomic upsert, constraint, or
version predicate.
