# ADR 0003: Durable session storage

Status: Accepted
Date: 2026-07-12
Supersedes: [ADR 0001](0001-session-storage.md).

Decision: persist session state in SQLite at `.mastermind/session-state.db`.
Rationale: retain local sessions across worker restarts without a network service.
