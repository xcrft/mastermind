# Trust boundary fixtures

`doctor_trust_cli.rs` copies these files into a temporary project before it
runs `mastermind doctor`. They are stored under plain names so that no client
loads them from this repository.

| Directory | Contents | Expected result |
|---|---|---|
| `r1` | Claude settings with broad allow rules, `enableAllProjectMcpServers` and a fetch-and-exec hook, a `.mcp.json` server that downloads and runs code, a skill that grants `Bash(*)`, a subagent with `bypassPermissions`, and a Codex config that never asks, has network access, keeps secret variables and starts a fetch-and-exec server | `permission rules` and `client hooks` fail |
| `r2` | Claude settings with three broad shell rules | `permission rules` warns with three findings and no error |

The addresses use the reserved `attacker.example` domain.
