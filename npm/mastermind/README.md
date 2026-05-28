# @xcraftmind/mastermind

Mastermind workflow CLI + mmcg codegraph for AI coding agents — verify-spec / audit-spec gates, MCP server, multi-language tree-sitter indexer (Python, TypeScript, JavaScript, Rust, C#, Go, Java, PHP, C/C++).

Prebuilt native binaries via optional platform packages — **no Rust toolchain required**.

## Install

### One-shot (no install)

```sh
npx -y @xcraftmind/mastermind doctor
npx -y @xcraftmind/mastermind init --profile typescript-api
npx -y @xcraftmind/mastermind run-task .mastermind/tasks/042-feature.md
```

`npx` is great for these one-shot commands. **Avoid it for the MCP server**, though: an MCP config that runs `npx ... serve` pays an npm-cache/network resolution cost on every Claude Code launch and is less deterministic than a real install. For `setup claude`, prefer **global** or **project-local** (below). If you do register via npx, `setup claude` pins the version (`npx -y @xcraftmind/mastermind@<ver> serve`) so at least the version is stable — but it's an escape hatch, not the recommended path.

### Global (recommended for most users)

```sh
npm install -g @xcraftmind/mastermind
mastermind setup claude --write-mcp       # register mmcg as an MCP server
mastermind doctor                          # verify the environment
```

`setup claude` writes `~/.claude/.mcp.json` with `command: "mastermind"` so Claude Code launches the wrapper from PATH.

### Project-local

```sh
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --project . --write-mcp
```

`setup claude --project .` writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"` so the project gets a reproducible, version-pinned MCP server.

### Build from source (contributors / unsupported platforms)

```sh
cargo install mmcg
```

Requires Rust 1.75+. The cargo-installed binary is `mmcg`, not `mastermind` — same code, same subcommands, only the wrapper name differs.

## Supported platforms

Prebuilt binaries ship for:

| Platform | Architecture |
|---|---|
| macOS (Apple Silicon) | aarch64 |
| macOS (Intel) | x86_64 |
| Linux glibc | x86_64, aarch64 |
| Linux musl (Alpine) | x86_64, aarch64 |
| Windows | x86_64 |

Other targets fall back to `cargo install mmcg`.

## What's in the box

- `mastermind` — public CLI command (alias for `mmcg` with install-mode hints)
- `mmcg` — compatibility command (same binary, same subcommands as cargo-installed `mmcg`)

Both commands resolve to the same native binary. Use whichever your team has documented.

### Top-level subcommands

```sh
mastermind init --profile typescript-api    # scaffold .mastermind/ with stack-specific CONTEXT.md
mastermind index .                          # build/refresh the codegraph index
mastermind watch                            # long-running watcher (re-indexes on file changes)
mastermind doctor                           # environment health check
mastermind serve                            # MCP stdio server
mastermind setup claude --write-mcp         # register with Claude Code's MCP layer
mastermind verify-spec <path>               # pre-execution mechanical gate on a task spec
mastermind audit-spec <path> --since main   # post-execution audit vs git baseline
mastermind run-task <path>                  # two-phase orchestrator: verify → executor → audit
mastermind query callers <symbol>           # one-shot CLI query (use MCP for agents)
```

See `mastermind <subcommand> --help` for full options.

## Architecture

The npm package uses the prebuilt-platform-package pattern (same as `esbuild`, `swc`, `lefthook`, `turbo`). The root `@xcraftmind/mastermind` package contains only JavaScript wrappers and lists all seven platform packages as `optionalDependencies`. npm installs only the package matching the host's `os` + `cpu` (+ `libc` for Linux); the others are skipped.

```
@xcraftmind/mastermind                    # JS wrappers + optionalDependencies
├── @xcraftmind/mmcg-darwin-arm64         # one of these installs, the rest skip
├── @xcraftmind/mmcg-darwin-x64
├── @xcraftmind/mmcg-linux-x64-gnu
├── @xcraftmind/mmcg-linux-arm64-gnu
├── @xcraftmind/mmcg-linux-x64-musl
├── @xcraftmind/mmcg-linux-arm64-musl
└── @xcraftmind/mmcg-win32-x64-msvc
```

No `postinstall` script. No network calls beyond the npm registry itself.

## Links

- Source: [github.com/xcrft/mastermind](https://github.com/xcrft/mastermind)
- mmcg Rust crate: [crates.io/crates/mmcg](https://crates.io/crates/mmcg)
- MCP protocol: [modelcontextprotocol.io](https://modelcontextprotocol.io)
