# Contributing

The point of this repo is to be a **predictable** library — every artifact should look like every other artifact of its kind. That predictability is the value; please don't break it.

## Before you start

1. **Read [`docs/conventions.md`](docs/conventions.md).** This is the standard. Naming, frontmatter, file layout — all there.
2. **Read the matching anatomy doc** for what you're adding:
   - Skill → [`docs/skill-anatomy.md`](docs/skill-anatomy.md)
   - Prompt → [`docs/prompt-anatomy.md`](docs/prompt-anatomy.md)
   - Agent config → [`docs/agent-anatomy.md`](docs/agent-anatomy.md)
   - MCP server → [`docs/mcp-anatomy.md`](docs/mcp-anatomy.md)
3. **Search the existing tree.** If a similar artifact already exists, prefer improving it over creating a parallel one.

## Adding a new artifact

1. Pick the right top-level folder (`skills/`, `prompts/`, `agents/`, `mcp/`).
2. Pick the right domain folder inside (`code-review/`, `testing/`, `design/`, …). Create a new domain only if none of the existing ones fit and you can justify it in the PR.
3. **Copy the `_template/` from that category** — do not invent your own structure.
4. Fill it in. Run it. Make sure it actually works before opening a PR.
5. Add an entry to the category's `README.md` index.
6. Open a PR using the [PR template](.github/PULL_REQUEST_TEMPLATE.md).

## What we'll ask in review

- **Does it work?** Have you actually used this artifact yourself?
- **Is the description precise?** A skill's `description` field is what makes it trigger correctly. Vague descriptions are the #1 reason skills don't get picked up.
- **Is the scope right?** A skill that does five unrelated things should be five skills.
- **Does it match the standard?** Frontmatter, naming, file layout per `docs/conventions.md`. CI runs [`scripts/validate.py`](scripts/validate.py) on every PR — if it fails, fix what it flags.
- **Does it duplicate something?** Check the category index first.

## Running the validator locally

Before opening a PR, run the same check CI will run:

```bash
python3 -m venv .venv
.venv/bin/pip install -r scripts/requirements.txt
.venv/bin/python scripts/validate.py
```

Exit code is 0 if clean. See [`scripts/README.md`](scripts/README.md) for what it checks and how to extend it.

## Changing an existing artifact

Backwards-compatible improvements: open a PR.

Breaking changes (renames, removed fields, changed behavior): open an issue first to discuss.

## Reporting bugs

Use the [bug issue template](.github/ISSUE_TEMPLATE/bug.md). Include the artifact path, what you expected, what happened, and your environment.

## Code of conduct

See [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
