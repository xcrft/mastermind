# ADR 0001: Session storage

Status: Superseded by [ADR 0003](0003-durable-session-storage.md).
Date: 2026-06-01

Decision: keep session state in a process-local memory cache.
Rationale: one worker process owns the initial deployment.
