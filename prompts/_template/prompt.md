---
name: <your-slug>
description: <One or two sentences. Lead with the verb. State when to use it. See ../../docs/conventions.md §2.2.>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
  role: <system | user | template>
  variables:
    - name: VARIABLE_NAME
      required: true
      description: <What goes here.>
---

# <Prompt Name>

<One-paragraph what-and-when.>

## When to use

- <Concrete trigger 1>
- <Concrete trigger 2>

## Variables

| Name | Required | Description |
|---|---|---|
| `VARIABLE_NAME` | yes | <What goes here.> |

## Prompt

```text
<The actual prompt body. Use {{VARIABLE_NAME}} for placeholders.>
```

## Example invocation

<A filled-in version of the prompt with realistic values, so reviewers see how it looks in use.>

## Notes

<Optional. Model recommendations, gotchas, things that didn't work.>
