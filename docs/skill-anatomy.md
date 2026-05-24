# Skill anatomy

A **skill** is a markdown file (or folder containing one) that an AI agent loads as a capability. In Claude Code, skills live in `~/.claude/skills/` or inside a plugin and are triggered by their `description` matching the user's intent.

This doc covers the shape of skills in this repo. Read [`conventions.md`](conventions.md) first.

---

## Entry point

The entry file is **always** `SKILL.md`. Single-file skills go straight into the domain folder; folder-style skills get their own subfolder.

```
skills/code-review/quick-review.md           # single-file, body lives directly inside
skills/code-review/pr-review/SKILL.md        # folder-style, can ship scripts/references
```

For single-file skills, the file name is `<slug>.md`. For folder-style, the file is `SKILL.md` and the folder is named `<slug>/`.

---

## Frontmatter

```yaml
---
name: pr-review
description: Review a pull request for correctness, security, and design issues. Use when the user asks to review a PR, audit a diff, or check changes before merge. Triggers on "review my PR", "code review", "check this diff".
metadata:
  version: 0.1.0
  authors:
    - alice
  tags:
    - code-review
  model: opus
  requires:
    - gh CLI
---
```

The `description` is what makes the skill trigger. See [`conventions.md` §2.2](conventions.md#22-writing-the-description) — this is the single most important field.

---

## Body structure

A working skill has these sections, in this order:

```markdown
# <Skill name as a heading>

<One-paragraph what-and-when. Not a marketing pitch — a usage cue.>

## When to use

- Concrete trigger 1
- Concrete trigger 2
- "Do NOT use for X" if there's a common confusion

## Prerequisites

(Optional — only if the skill needs setup, env vars, or installed tools.)

## Steps

1. First step (imperative).
2. Second step.
3. ...

## Outputs

What the user should expect to see after the skill runs. A report? A diff? A list of issues?

## Examples

(Optional but strongly encouraged.) A short before/after, an example invocation, or a sample output.
```

Sections are guidance, not law — if your skill genuinely doesn't need "Prerequisites", drop the section, don't fill it with "None."

---

## Folder layout (for folder-style skills)

```
skills/code-review/pr-review/
├── SKILL.md             # required, the entry point
├── README.md            # optional, human-readable explainer (no frontmatter)
├── references/          # optional, long-form docs, checklists, citations
│   └── checklist.md
├── scripts/             # optional, helper scripts the skill invokes
│   └── fetch-diff.sh
└── assets/              # optional, templates, fixtures, images
```

If you have no extras, use a single file — don't make an empty folder.

---

## Installing a skill from this repo

```bash
# Single-file:
cp skills/code-review/quick-review.md ~/.claude/skills/quick-review.md

# Folder-style:
cp -r skills/code-review/pr-review ~/.claude/skills/
```

(Plugins or symlinks work too. Pick whatever your team standardizes on.)

---

## Reviewing a skill PR

When reviewing, check in this order:

1. **Description specificity.** Could this description apply to a different skill? If yes, it's too vague.
2. **Actually used.** Has the author run it? Are there example outputs?
3. **Scope.** Is this one skill or three?
4. **Layout.** Single-file vs. folder choice match the rule above?
5. **Style.** Imperative, concrete, examples present.

If 1 or 2 fail, request changes before reading the rest.
