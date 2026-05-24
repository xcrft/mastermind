# Conventions — the standard

This document is the single source of truth for how artifacts in this repo are named, structured, and described. **All four artifact types follow the same set of rules**, with type-specific details in the matching `*-anatomy.md`.

Read this first. The anatomy docs assume you already know what's here.

---

## 1. Naming

### 1.1 Folder and file names
- **kebab-case**, lowercase, ASCII only. `code-review`, not `CodeReview` or `code_review`.
- Be specific: `pr-review-staff-style` beats `review`.
- No version numbers in names. Use the `metadata.version` frontmatter field instead.

### 1.2 Slugs in frontmatter
The `name:` field in frontmatter must match the directory or file slug exactly. If the file lives at `skills/code-review/pr-review/SKILL.md`, then `name: pr-review`.

### 1.3 Domains (the second-level folder)
Top-level is **type** (`skills/`, `prompts/`, `agents/`, `mcp/`). Second level is **domain**. Reuse existing domains:

- `code-review/`
- `testing/`
- `design/` (system, API, UI)
- `debugging/`
- `docs/` (writing documentation, READMEs, etc.)
- `refactoring/`
- `ops/` (deploys, incidents, SRE)
- `security/`
- `workflow/` (delegation, planning, orchestration patterns)
- `prompt-engineering/` (refining, evaluating, designing prompts)

Add a new domain only if nothing fits — and justify it in the PR.

---

## 2. Frontmatter

Every artifact that supports YAML frontmatter (skills, prompts, agent configs as markdown) uses this shape:

```yaml
---
name: <slug-matching-the-file-or-folder>
description: <one or two sentences, see §2.2>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
    - <optional-tags>
---
```

### 2.1 Required fields
- `name` — slug, matches file/folder.
- `description` — see below.

### 2.2 Writing the description

The description is **how the artifact gets discovered and triggered**. It's the most important line in the file. Rules:

- **Lead with the verb.** "Review a pull request for…" not "A skill that reviews…".
- **State when to use it.** "Use when reviewing TypeScript code for…" — list the triggers explicitly.
- **List concrete signals.** "Triggers on: 'review my PR', 'code review', 'check this diff'."
- **One or two sentences max.** If you need more, the artifact does too much.
- **Don't say what it doesn't do** unless it's a common confusion. ("Do NOT use for performance review — use [[other-skill]] instead.")

Good:
> Review TypeScript code for type-safety bugs, missing error handling, and strict-mode violations. Use when reviewing TS/Node services or when the user says "review this TS code".

Bad:
> A helpful skill for code review.

### 2.3 Optional metadata fields
- `version` — SemVer. Start at `0.1.0`.
- `authors` — list of GitHub handles.
- `tags` — domain + any helpful tags. First tag should be the domain folder.
- `requires` — list of MCP servers, env vars, or tools the artifact needs.
- `model` — recommended model (`opus`, `sonnet`, `haiku`) if the artifact is sensitive to this.

---

## 3. File layout

### 3.1 Single-file vs. folder

| If the artifact is… | Use |
|---|---|
| One markdown file, no extras | A single file: `skills/code-review/quick-review.md` |
| Has scripts, assets, sub-prompts, references | A folder: `skills/code-review/pr-review/` with `SKILL.md` inside |

Don't make a folder for a single file. Don't put a single file where a folder is needed.

### 3.2 Folder contents

When an artifact is a folder, the layout is:

```
<slug>/
├── SKILL.md          # or prompt.md / agent.md / server.json — the entry point
├── README.md         # optional: human-readable explainer (frontmatter goes in SKILL.md)
├── references/       # optional: long docs, examples, citations
├── scripts/          # optional: helper scripts
└── assets/           # optional: images, templates, fixtures
```

The entry-point file name is **fixed** per type (see anatomy docs). Don't rename it.

---

## 4. Writing style inside artifacts

These rules apply to the body of skills, prompts, and agent configs:

- **Imperative voice.** "Read the file. Find the function." Not "You should read…".
- **Numbered steps for procedures.** Bullets for lists of options.
- **Concrete examples beat abstract description.** Show a 3-line example before a paragraph of explanation.
- **No filler.** Cut "In order to", "Please note that", "It is important to". Get to the verb.
- **One idea per section.** If a section needs an `## H2`, it's probably its own artifact.

---

## 5. Cross-references

Link to other artifacts in this repo with their relative path:

```markdown
See [`skills/testing/flaky-finder`](../../skills/testing/flaky-finder/SKILL.md).
```

In skill bodies, you can also use `[[slug]]` shorthand — it's a hint to the reader (and to authoring tools) that the target is another artifact, even if no link exists yet.

---

## 6. Versioning and changes

- Start every artifact at `metadata.version: 0.1.0`.
- **Patch (`0.1.0 → 0.1.1`)** — wording fixes, typos, no behavior change.
- **Minor (`0.1.0 → 0.2.0`)** — added capability, still backwards-compatible.
- **Major (`0.1.0 → 1.0.0`)** — breaking change (renamed, removed field, behavior shift). Discuss in an issue first.

Bumping the version is part of the PR that changes the artifact, not a follow-up.

---

## 7. What NOT to do

- Don't add a new top-level folder. The four (`skills/`, `prompts/`, `agents/`, `mcp/`) are the standard.
- Don't add an artifact you haven't actually used. We'd rather have 20 working artifacts than 200 plausible ones.
- Don't write a description that could apply to ten other artifacts. Specificity is what makes them findable.
- Don't ship `TODO` or `WIP` — open a draft PR if it's not done.
- Don't reproduce another artifact's content. Link to it.
