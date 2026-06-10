# Prompts

Reusable system and user prompt templates. All prompts in this repo are organized under `workflow/` — they are designed to be wired into the [Mastermind workflow](../agents/claude-md/mastermind-workflow.md) (planner / executor / reviewer / researcher / refiner). Tool-agnostic where possible.

See [`../docs/prompt-anatomy.md`](../docs/prompt-anatomy.md) for the format.

## Index

### workflow/
| Prompt | Role | Description |
|---|---|---|
| [`api-shape-explorer`](workflow/api-shape-explorer.md) | user | Generates 3 radically different API shapes for a given problem so you can compare tradeoffs. Used by the planner for green-field interface design — the two unpicked options become the spec's rejected alternatives. |

---

## Adding a prompt

1. Read [`../docs/prompt-anatomy.md`](../docs/prompt-anatomy.md).
2. Copy `_template/prompt.md` to `prompts/workflow/<your-slug>.md`.
3. Fill it in. Include at least one example invocation with realistic values.
4. Tag it with the workflow phase it supports (`code-review`, `design`, etc.) as a secondary tag — the first tag must be `workflow` per `docs/conventions.md` §2.3.
5. Add to this index.
6. Open a PR.
