# Prompt anatomy

A **prompt** is a reusable instruction block — system prompt, user prompt template, or a parameterizable snippet — that you paste into a chat, drop into an agent's config, or wire into a workflow. Prompts in this repo are tool-agnostic where possible.

Read [`conventions.md`](conventions.md) first.

---

## Entry point

A prompt is a single markdown file: `prompts/<domain>/<slug>.md`.

If the prompt needs companions (variants, examples, references), promote it to a folder:

```
prompts/workflow/api-shape-explorer.md              # single-file
prompts/workflow/deep-design-review/                # folder
├── prompt.md                                       # required entry
├── README.md                                       # explainer
└── examples/
```

Folder entry file is `prompt.md`, not `SKILL.md`.

---

## Frontmatter

```yaml
---
name: api-shape-explorer
description: Generates three radically different API/interface shapes for the same problem so you can compare tradeoffs before committing. Use when you're about to design a new module, service boundary, or public API.
metadata:
  version: 0.1.0
  authors:
    - alice
  tags:
    - workflow
    - design
  role: user                 # system | user | template
  variables:
    - name: PROBLEM
      required: true
      description: What the API needs to do. Concrete use cases, not abstract goals.
    - name: CONSTRAINTS
      required: false
      description: Hard constraints — language, framework, latency, compatibility.
---
```

Type-specific fields:

- `role` — `system`, `user`, or `template`. A `template` is a parameterizable block to embed inside another prompt.
- `variables` — list of placeholders the body uses. Each has `name`, `required`, `description`.

---

## Body structure

```markdown
# <Prompt name>

<One-paragraph what-and-when.>

## When to use

- Concrete trigger 1
- Concrete trigger 2

## Variables

| Name | Required | Description |
|---|---|---|
| `DIFF` | yes | The unified diff to review. |
| `CONTEXT` | no | Optional design doc or surrounding code. |

## Prompt

```text
<The actual prompt body. Use {{VARIABLE}} syntax for placeholders.>
```

## Example invocation

<A filled-in version of the prompt with realistic values, so reviewers can see what it actually looks like in use.>

## Notes

(Optional.) Model recommendations, gotchas, things that didn't work.
```

---

## Placeholder syntax

Use `{{VARIABLE_NAME}}` — double-brace, SCREAMING_SNAKE_CASE for the variable name. Match the names listed in `metadata.variables`.

```text
You are reviewing the following diff:

{{DIFF}}

{{#if CONTEXT}}
Additional context:
{{CONTEXT}}
{{/if}}
```

Conditionals (`{{#if VAR}}…{{/if}}`) are allowed and follow Handlebars conventions. Keep logic minimal — if a prompt needs heavy templating, it's probably a skill or a script instead.

---

## Reviewing a prompt PR

1. **Description specificity** — same rule as skills.
2. **Variables documented** — every `{{VAR}}` in the body appears in the `variables` table, and vice versa.
3. **Example invocation present** — at least one realistic filled-in version.
4. **No tool-specific lock-in** unless the prompt explicitly targets one (note in description).
