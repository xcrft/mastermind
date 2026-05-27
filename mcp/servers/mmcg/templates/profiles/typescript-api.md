---
name: mastermind-context-typescript-api
description: Project-level CONTEXT.md template — TypeScript HTTP/REST/GraphQL API service variant. Pre-seeded with stack conventions (project layout, test/lint/build commands) and TypeScript-API-canonical gotchas (env var typing, JSON Date serialization, swallowed async errors).
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - claude-md
    - context
    - profile
    - typescript
    - nodejs
    - api
---

<!--
  TypeScript API profile — opinionated CONTEXT.md template for Node.js HTTP services.
  Copy everything below the COPY FROM HERE marker to <project-root>/CONTEXT.md.
  Replace <PLACEHOLDERS>. Prune sections that don't apply to your service.
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME> — Context (TypeScript API)

## Identity

**What it is:** TypeScript HTTP/REST/GraphQL API service.

**What it is not:** <e.g. "not a frontend app", "not a shared library">

**Primary users:** <web/mobile clients, internal teams, third-party integrations>

---

## Stack conventions

- **Runtime:** Node.js LTS (v20+)
- **Language:** TypeScript with `"strict": true` in `tsconfig.json`
- **Package manager:** <npm / pnpm / yarn / bun — pick one and document>
- **HTTP framework:** <Express / Fastify / NestJS / Hono — fill in>
- **Validation:** <zod / class-validator / typia> for request bodies and env vars
- **Layout:**
  - `src/routes/` or `src/controllers/` — HTTP-layer handlers
  - `src/services/` — business logic (testable in isolation, no `req`/`res`)
  - `src/db/` or `src/models/` — data access (ORM / query builder)
  - `src/middleware/` — request pipeline (auth, logging, error handling)
  - `src/types/` — shared TypeScript types and DTOs
  - `tests/` or `src/**/*.test.ts` — unit/integration tests
- **Test command:** `npm test` (Vitest or Jest — never both)
- **Lint:** `npm run lint` (ESLint + Prettier)
- **Type-check:** `npm run typecheck` (or `tsc --noEmit`)
- **Build:** `npm run build` (esbuild/tsc/tsup → `dist/`)
- **Dev server:** `npm run dev` (tsx/nodemon with auto-reload)

---

## Active goals

- <Goal 1 — concrete and measurable>

---

## Decision log

### <YYYY-MM-DD> — Initial framework choice

- **Decision:** Chose <framework>
- **Why:** <perf, ecosystem, team familiarity, etc.>
- **Alternatives rejected:**
  - <other framework>: <reason>

---

## Known gotchas

*Pre-seeded with TypeScript-API-canonical surprises. Prune anything that doesn't apply.*

- **`process.env.X` is `string | undefined`** — always validate at startup via `zod`/`envalid`. Reading raw `process.env.PORT` and passing to `app.listen()` crashes at runtime if unset.
- **JSON has no Date type** — Dates serialize to ISO-8601 strings. The wire is `string`; consumers re-parse if they want a `Date`. Don't put `Date` in response DTOs — put `string`.
- **Unawaited async errors silently kill the process** — `app.get('/x', async (req, res) => { await thing() })` without `try`/`catch` crashes if `thing()` rejects. Use a middleware error handler or `express-async-errors`.
- **Number precision** — JSON has one number type. IDs > 2^53 (e.g., Twitter snowflake, big DB serials) lose precision when serialized. Use strings for large IDs.
- **CORS preflight is not the same as the actual request** — middleware ordering matters. Errors in preflight return 200 with no response body and confuse clients.

---

## Domain glossary

- <term> — <local meaning specific to this codebase>

---

## External dependencies

*Services / APIs / vendors this project relies on. Include auth mechanism and pinned version if applicable.*

- **<service>** — <what we use it for> — auth: <env var `X`> — version `<X.Y or latest>`

---

## Don't-touch list

- **`node_modules/`** — generated; never edit
- **`package-lock.json` / `pnpm-lock.yaml` / `yarn.lock`** — let the package manager update; manual edits cause non-reproducible installs
- **`dist/` / `build/`** — build output; should be in `.gitignore`
- **`<path>`** — <project-specific area with hidden constraints>

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
