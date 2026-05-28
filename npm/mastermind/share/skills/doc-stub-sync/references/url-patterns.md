# Stub URL patterns

This reference covers stub formats the [`doc-stub-sync`](../SKILL.md) skill recognizes, and how to add new ones.

## The default pattern

```
Fetch live documentation: https://docs.example.com/path/to/page
```

Regex: `Fetch live documentation:\s*(https?://\S+)`

This is the format the bundled `scripts/doc_update.py` looks for unless overridden.

### Why this pattern

- **Easy to grep**: single grep across the whole tree finds every stub
- **Self-documenting**: a human reading the file knows where the canonical source is
- **One URL per file**: forces one-doc-per-stub-file, which simplifies sync logic
- **Tool-agnostic prefix**: the words `Fetch live documentation:` are unlikely to appear in normal prose, so false matches are rare

## Where the pattern goes inside the file

The skill doesn't care about position — it greps the whole file. By convention:

- **At the end of the file**, after a `---` separator, as a "source" footer
- **In a frontmatter field** (e.g., `source: https://...`) — for that, switch to a frontmatter-aware pattern (see below)

## Common alternative patterns

If your knowledge base uses a different convention, pass `--stub-pattern` to the script. The pattern must be a Python regex with **exactly one capture group** that captures the URL.

### Source-line convention

```
Source: https://docs.example.com/page
```

```bash
python scripts/doc_update.py --stub-pattern 'Source:\s*(https?://\S+)' ./docs
```

### HTML-comment marker

```html
<!-- mirror: https://docs.example.com/page -->
```

```bash
python scripts/doc_update.py --stub-pattern '<!--\s*mirror:\s*(https?://\S+)\s*-->' ./docs
```

### Frontmatter `source:` field

```yaml
---
title: API Reference
source: https://docs.example.com/api
---
```

```bash
python scripts/doc_update.py --stub-pattern '^source:\s*(https?://\S+)$' ./docs
```

Frontmatter matching is best done with the multiline flag — wrap the pattern in `(?m)` if needed: `'(?m)^source:\s*(https?://\S+)$'`.

## Adding a new pattern to the standard

If you find yourself using a new pattern across multiple projects, send a PR adding:

1. An entry to this file under "Common alternative patterns"
2. An example command line
3. A short note on when this pattern is preferable

Don't add a flag to the script for every pattern — the regex flag is general enough. Only the default in `DEFAULT_STUB_PATTERN` is special, and we don't change the default casually (it would break every existing local KB using this skill).

## Pattern gotchas

- **Greediness**: `https?://.+` will eat trailing punctuation. Use `\S+` to stop at whitespace.
- **Multiline**: stubs that wrap across lines aren't supported — keep the URL on the same line as the marker.
- **Query strings and fragments**: included by default. If you want to strip `?utm_*` and `#fragments` for hashing, do it in the script, not the regex.
- **Multiple stubs per file**: only the *first* match wins. If a file points at multiple URLs, it's not a stub file — it's a hand-edited doc, leave it alone.
