---
name: mastermind-context-python-fastapi
description: Project-level CONTEXT.md template — Python FastAPI service variant. Pre-seeded with stack conventions (app/api/models/schemas layout, pytest/ruff/mypy commands) and FastAPI-canonical gotchas (sync/async DB drivers, Pydantic v1↔v2, BackgroundTasks scope, dependency-injection caching).
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - claude-md
    - context
    - profile
    - python
    - fastapi
    - api
---

<!--
  Python FastAPI profile — opinionated CONTEXT.md template for async Python APIs.
  Copy everything below the COPY FROM HERE marker to <project-root>/CONTEXT.md.
  Replace <PLACEHOLDERS>. Confirm your Pydantic major version — it changes a lot.
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME> — Context (Python FastAPI)

## Identity

**What it is:** FastAPI-based async HTTP API service.

**What it is not:** <e.g. "not a Django monolith", "not a background-worker pool">

**Primary users:** <web/mobile clients, internal services, third-party API consumers>

---

## Stack conventions

- **Python:** <3.11 / 3.12 — match `pyproject.toml`>
- **Package manager:** <uv / poetry / pip-tools / hatch> — managed via `pyproject.toml`
- **HTTP framework:** FastAPI
- **Validation / serialization:** Pydantic <v1 / v2 — they're not API-compatible>
- **ORM / DB layer:** <SQLAlchemy (sync) / SQLAlchemy 2.0 (async) / SQLModel / Tortoise>
- **Migrations:** <Alembic / aerich>
- **Layout:**
  - `app/main.py` — FastAPI app factory + router mounts
  - `app/api/` — route modules (`app/api/users.py`, `app/api/orders.py`)
  - `app/schemas/` — Pydantic request/response models
  - `app/models/` — DB models (SQLAlchemy / SQLModel classes)
  - `app/db/` — engine + session factory + dependency
  - `app/services/` — business logic (free functions or service classes)
  - `app/core/` — config, security, shared utilities
  - `tests/` — pytest suite (mirrors `app/` layout)
- **Test command:** `pytest`
- **Lint:** `ruff check` (and `ruff format` for formatting)
- **Type-check:** `mypy app` (or `pyright`)
- **Dev server:** `uvicorn app.main:app --reload`
- **Production server:** `uvicorn app.main:app --workers <N>` behind nginx, or `gunicorn -k uvicorn.workers.UvicornWorker`

---

## Active goals

- <Goal 1 — concrete and measurable>

---

## Decision log

### <YYYY-MM-DD> — DB driver: sync vs async

- **Decision:** <chose async SQLAlchemy 2.0 / chose sync + threadpool>
- **Why:** <throughput vs operational complexity>
- **Alternatives rejected:**
  - <other option>: <reason>

---

## Known gotchas

*Pre-seeded with FastAPI-canonical surprises. Prune anything that doesn't apply.*

- **Sync DB code in async endpoints blocks the event loop** — `def get_db()` returning a sync `Session` from inside `async def endpoint()` is a silent perf cliff. Use async session + `AsyncSession`, or wrap sync calls in `run_in_threadpool`.
- **Pydantic v1 and v2 are not compatible** — `Config` class → `model_config = ConfigDict(...)`; `.dict()` → `.model_dump()`; `orm_mode` → `from_attributes`. Mixing v1 and v2 models in one app breaks subtly.
- **`Depends()` results are cached per-request** — calling the same dependency function twice in one request returns the same value. Useful for DB sessions, surprising for "fresh value" helpers.
- **`BackgroundTasks` runs in the response cycle** — they execute AFTER the response is sent but BEFORE the worker is freed. For multi-second work or fire-and-forget, use Celery / arq / RQ, not BackgroundTasks.
- **Path operation order matters** — FastAPI matches the first route that fits. `/users/me` defined AFTER `/users/{user_id}` makes `/users/me` resolve to `user_id="me"`. Order specific before generic.
- **`response_model` strips extra fields silently** — if your endpoint returns a dict with extra keys, they're dropped before the response. Use `response_model_exclude_unset=True` to differentiate "absent" from "null".
- **CORS middleware ordering** — `CORSMiddleware` must be added BEFORE other middleware that might short-circuit (auth, rate limiting) or preflight returns 401/429 with no CORS headers and the browser silently fails.

---

## Domain glossary

- <term> — <local meaning>

---

## External dependencies

- **<service>** — <use> — auth: <env var `X` / OAuth> — version `<X.Y>`

---

## Don't-touch list

- **`.venv/` / `venv/`** — virtual env; recreate, don't edit
- **`__pycache__/` / `*.pyc`** — bytecode cache
- **`alembic/versions/<HASH>_*.py` after merge** — past migrations are append-only; modify only via a new migration
- **`<path>`** — <project-specific area>

---

## How this file gets updated

The planner (`mastermind-task-planning` skill) appends to this file during post-flight semantic review when work surfaces something worth preserving across sessions:

| Discovery type | Section to update |
|---|---|
| Non-trivial design decision the critic agreed with | Decision log |
| Workflow surprised by something — "almost broke X" | Known gotchas |
| New term that took explaining during brainstorming | Domain glossary |
| New external dependency added | External dependencies |
| Code area found to have hidden constraints | Don't-touch list |

The planner does NOT update this file silently. Every change is logged in the spec's Notes section so the audit trail is preserved.

<!-- ─── COPY TO HERE ─── -->
