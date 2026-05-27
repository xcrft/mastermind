---
name: mastermind-context-rust-cli
description: Project-level CONTEXT.md template — Rust command-line tool variant. Pre-seeded with stack conventions (src/main.rs + src/lib.rs split, clap-style CLI, cargo test/clippy/fmt commands) and Rust-CLI-canonical gotchas (MSRV drift, stdout vs stderr, signal handling, panic-to-exit-code mapping).
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - claude-md
    - context
    - profile
    - rust
    - cli
---

<!--
  Rust CLI profile — opinionated CONTEXT.md template for command-line tools.
  Copy everything below the COPY FROM HERE marker to <project-root>/CONTEXT.md.
  Replace <PLACEHOLDERS>. Confirm whether you're shipping a binary, a library, or both.
-->

<!-- ─── COPY FROM HERE ─── -->

# <PROJECT_NAME> — Context (Rust CLI)

## Identity

**What it is:** Rust-based command-line tool.

**What it is not:** <e.g. "not a daemon", "not a web service", "not a library-first crate">

**Primary users:** <developers, CI scripts, end users via `cargo install`, etc.>

---

## Stack conventions

- **Edition:** 2021 (Rust 1.56+)
- **MSRV:** <pinned in `Cargo.toml` `rust-version` field — e.g. `1.75`>
- **CLI parsing:** <clap (derive) / pico-args / argh>
- **Error handling:** <thiserror + anyhow / hand-rolled Error enum> — `?` propagation throughout
- **Async:** <tokio / async-std / sync-only> — pick one and stick with it
- **Layout:**
  - `Cargo.toml` — manifest (deps, MSRV, profile config, package metadata)
  - `src/main.rs` — binary entry point (`fn main` — thin, delegates to `lib`)
  - `src/lib.rs` — library entry — re-exports the actual logic so it's reusable + testable
  - `src/cli.rs` — argument parsing (clap derive structs) if non-trivial
  - `src/<module>/` — per-feature modules
  - `tests/` — integration tests (each file is a separate test binary)
  - `examples/` — runnable examples via `cargo run --example <name>`
  - `benches/` — criterion benchmarks
- **Test command:** `cargo test --all-targets` (lib + bin + integration + doctests)
- **Lint:** `cargo clippy --all-targets -- -D warnings` (treat warnings as errors)
- **Format:** `cargo fmt --check` (CI), `cargo fmt` (local)
- **Doc:** `cargo doc --no-deps --document-private-items --open`
- **Release build:** `cargo build --release` (use `[profile.release]` LTO + strip for size)
- **Install:** `cargo install --path .` (local) or `cargo install <crate>` (registry)
- **Distribution:** <crates.io / GitHub Releases binary / Homebrew tap>

---

## Active goals

- <Goal 1 — concrete and measurable>

---

## Decision log

### <YYYY-MM-DD> — async runtime choice

- **Decision:** <chose tokio / chose sync>
- **Why:** <ecosystem coverage vs binary size, single-task vs concurrent IO>
- **Alternatives rejected:**
  - <other option>: <reason>

---

## Known gotchas

*Pre-seeded with Rust-CLI-canonical surprises. Prune anything that doesn't apply.*

- **MSRV drift via dependency bumps** — a `cargo update` can pull a transitive dep that requires a newer Rust than your declared `rust-version`. CI must build on the MSRV toolchain, not just `stable`, or you'll ship a crate that won't compile for advertised users.
- **`println!` is stdout, `eprintln!` is stderr — and only stderr should carry log/error/progress.** Mixing them breaks Unix pipelines (`mytool | jq` chokes if errors land in stdout).
- **`panic!` returns exit code 101 by default, not 1.** Set a panic hook + use `std::process::exit(1)` (or `ExitCode`) for user-facing failures so scripts can react predictably.
- **Ctrl-C handling** — without a `signal-hook` / `tokio::signal` handler, an interrupted CLI may leave temp files / locks / partial output. Wire SIGINT explicitly if you create state outside the process.
- **`Cargo.lock` policy** — commit it for binaries (reproducible builds for users), gitignore it for libraries (downstream picks resolution). Don't both.
- **`include_str!` / `include_bytes!` paths are relative to the source file**, not the crate root. Refactoring a module to a subdirectory silently breaks build until you update the path.
- **Test binaries inherit the env from `cargo test`** — `std::env::set_var` in one test leaks into siblings within the same test binary (single thread per default). Use `temp_env::with_var` or run tests in subprocesses for isolation.
- **`unwrap()` in `main` produces ugly stack traces** — return `Result<(), Box<dyn Error>>` from `main`, or use `anyhow::Result<()>`, for clean error messages.

---

## Domain glossary

- <term> — <local meaning>

---

## External dependencies

- **<service / system tool>** — <use> — version constraint `<X.Y>` (in `Cargo.toml`)

---

## Don't-touch list

- **`target/`** — build artifacts; gitignored
- **`Cargo.lock`** — see gotcha above; let `cargo` manage it
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
