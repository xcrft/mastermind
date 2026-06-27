---
name: mastermind-context-monorepo
description: Project-level CONTEXT.md template — polyglot monorepo variant. For repos with no single root stack and many per-package manifests across several languages. Pre-seeded with the convention that each package owns its stack and the agent reads the nearest manifest / per-service CLAUDE.md rather than assuming one toolchain.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - claude-md
    - context
    - profile
    - monorepo
    - polyglot
---

<!--
  Polyglot monorepo profile — CONTEXT.md template for repos with many stacks.
  Copy everything below the COPY FROM HERE marker to <project-root>/CONTEXT.md.
  Replace <PLACEHOLDERS>. List the actual top-level packages and their stacks.
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME> — Context (Polyglot monorepo)

## Identity

**What it is:** a polyglot monorepo — multiple independent stacks live in subpackages; the repo root has no single toolchain.

**What it is not:** <e.g. "not a single deployable service", "not one language">

**Primary users:** <teams owning each package; CI; release tooling>

---

## Stack conventions

**There is no one stack — each package owns its own.** Don't assume a root toolchain.

- **Detected languages:** <e.g. Python (services), TypeScript (web), Go (infra) — fill in from the actual tree>
- **Where stacks live:** manifests are per-package, not at the root — e.g.
  - `<services/*/pyproject.toml>` — Python services
  - `<apps/*/package.json>` — TS/JS apps
  - `<cmd/*/go.mod>` — Go binaries
- **Read the nearest context first:** before touching a package, read the closest `CLAUDE.md` and the package's own manifest — they define the test/lint/build commands for *that* package. The root commands (if any) only orchestrate.
- **Run commands scoped to the package**, never the whole repo: `pytest services/billing`, `npm test -w apps/web`, `go test ./cmd/sync/...`. A whole-repo build/test is a CI concern, not a per-change one.
- **Cross-package changes:** a change that crosses a package boundary touches two stacks — verify each side with its own toolchain.

---

## Active goals

- <Goal 1 — concrete and measurable; name the package(s) it touches>

---

## Decision log

### <YYYY-MM-DD> — Monorepo boundaries

- **Decision:** <how packages are split — by service, by language, by domain>
- **Why:** <shared CI, atomic cross-cutting changes, single review surface, etc.>
- **Alternatives rejected:**
  - <polyrepo / separate repos>: <reason>

---

## Known gotchas

*Pre-seeded with monorepo-canonical surprises. Prune anything that doesn't apply.*

- **No single "install" / "test" / "build"** — each package has its own. A command that works in one package fails in another. Always scope to the package.
- **The nearest manifest wins** — a tool reading the root finds nothing (or the wrong thing). Resolve config (tsconfig, pyproject, lockfile) from the package dir up, not from the root down.
- **Cross-language boundaries are contracts, not calls** — packages in different languages talk over HTTP/gRPC/queues/generated types, not direct imports. Changing one side without the other breaks the contract silently.
- **Shared lockfiles / workspaces** — if a workspace tool (pnpm/yarn workspaces, uv, cargo workspace) hoists deps, a version bump in one package can move another's resolved version. Check the workspace root lockfile.
- **Generated code mirrors across packages** — protobufs / OpenAPI / schema types are generated into multiple packages; edit the source, regenerate, don't hand-edit the copies.

---

## Domain glossary

- <term> — <local meaning specific to this codebase>

---

## External dependencies

*Services / APIs / vendors this monorepo relies on. Note which package owns each.*

- **<service>** — owned by `<package>` — <what we use it for> — auth: <env var `X`>

---

## Don't-touch list

- **`<package>/<generated dir>`** — generated from <source>; regenerate, don't edit
- **Workspace lockfiles** (`pnpm-lock.yaml`, `uv.lock`, `Cargo.lock`) — let the workspace tool update them
- **`<vendored / mirrored path>`** — synced by tooling (e.g. Copybara); edit upstream, not here
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
