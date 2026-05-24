# Agent anatomy

The `agents/` tree holds configurations that shape **how an agent behaves** in a project or session — distinct from skills (capabilities) and prompts (instruction blocks). Four sub-categories:

| Sub-folder | What it is |
|---|---|
| `subagents/` | Definitions of specialized agents the main agent can spawn (Claude Code subagent format). |
| `claude-md/` | `CLAUDE.md` templates — project-level instructions an agent reads on session start. |
| `hooks/` | Hook configurations and scripts for `settings.json`. |
| `settings/` | Snippets for `~/.claude/settings.json` or project `.claude/settings.json`. |

Read [`conventions.md`](conventions.md) first.

---

## 1. Subagents (`agents/subagents/`)

A subagent is a markdown file with frontmatter that defines a specialized agent (model, tools, system prompt).

### Layout
```
agents/subagents/<slug>.md
```

### Frontmatter
```yaml
---
name: critic
description: Independent reviewer that critiques a proposed change without seeing prior conversation. Use to get a second opinion before merging.
metadata:
  version: 0.1.0
  tags:
    - code-review
  model: opus
  tools:
    - Read
    - Grep
    - Bash
---
```

### Body
The body is the subagent's system prompt. Same writing style rules as skills (imperative, concrete, examples).

---

## 2. CLAUDE.md templates (`agents/claude-md/`)

A `CLAUDE.md` is a project-level instruction file that an agent reads when it opens the project. Templates here are starting points for common project shapes.

### Layout
```
agents/claude-md/<slug>.md
```

Where `<slug>` describes the project type: `python-backend`, `react-frontend`, `rust-cli`, `monorepo-pnpm`.

### Frontmatter
```yaml
---
name: python-backend
description: CLAUDE.md template for a Python backend service (FastAPI/Django) — establishes test commands, lint rules, deploy flow, and common pitfalls. Use as a starting point for new Python services.
metadata:
  version: 0.1.0
  tags:
    - python
    - backend
    - claude-md
---
```

### Body
The body is what gets copied into the target project's `CLAUDE.md`. Use placeholders like `<PROJECT_NAME>`, `<TEST_COMMAND>` for things the adopter must fill in.

---

## 3. Hooks (`agents/hooks/`)

A hook is a shell command that runs in response to an agent event (tool call, session start, etc.). Hooks live in `settings.json` under the `hooks` key.

### Layout
```
agents/hooks/<slug>/
├── hook.md           # entry: describes the hook, when it fires, what it does
├── settings.json     # the snippet to merge into ~/.claude/settings.json
└── scripts/          # optional: scripts the hook invokes
```

### Frontmatter (hook.md)
```yaml
---
name: pre-commit-format
description: Runs gofmt/prettier/black before each commit. Triggers as a PreToolUse hook on the Bash tool when the command starts with "git commit".
metadata:
  version: 0.1.0
  tags:
    - hook
    - formatting
  event: PreToolUse
---
```

### Body
Explain what the hook does, what event it fires on, how to install it.

---

## 4. Settings snippets (`agents/settings/`)

Composable chunks of `settings.json` — permission lists, MCP server registrations, env vars.

### Layout
```
agents/settings/<slug>/
├── README.md         # explains what the snippet does
└── settings.json     # the snippet itself
```

No frontmatter on `settings.json` (it's JSON). The README has the standard frontmatter at the top.

---

## Reviewing an agent-config PR

1. **Sub-category match.** Is it really a subagent vs. a CLAUDE.md vs. a hook? Wrong category = wrong file.
2. **Tool list (subagents).** Is the tools list minimal? A subagent with `tools: *` is a smell.
3. **Hooks are sandboxed.** Hook scripts run with user permissions — read them carefully for side effects.
4. **No secrets.** No API keys, no hardcoded paths to private resources.
