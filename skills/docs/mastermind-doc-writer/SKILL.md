---
name: mastermind-doc-writer
description: Write or update technical documentation that matches the current code and the repository's own conventions — READMEs, guides, references, runbooks, docs indexes, decision records and Mermaid diagrams. Use when asked to document something, fix stale or wrong docs, record a design decision, or add a diagram, and when reviewing changed documentation.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - docs
    - writing
    - mmcg
---

# Mastermind doc writer

Documentation is a contract with the reader. Every command, flag, path and
claim must be true of the code as it is now. Follow the repository's layout and
voice; do not impose a new one.

## 1. Find what already owns the topic

1. Read the repository instructions, the root README, the nearest docs index
   and the documents next to the one you will change.
2. Search before writing: `mmcg_docs` with a query for the topic (it covers
   `CONTEXT.md`, the root `README.md` and `docs/**/*.md`) and
   `mmcg_history` with `kind: architecture_decision` for earlier decisions. A
   match is a place to read, not a fact; open the source path it returns.
3. If a page already owns the topic, update that page. Do not start a second
   page that also claims to be the source of truth.

Identify the reader, the document type (tutorial, task guide, explanation or
reference), the naming style, heading depth and link style. Reuse an existing
template when there is one.

Do not add prefixed file names, a new docs tree, an index in every folder or a
table of contents rule unless the repository already does that.

## 2. Check every fact against the source

- Run each command you document, or at least `<command> --help`, from the
  directory the page says to use. Copy flags and output from what you saw, not
  from memory.
- Read the code, config or schema behind each described behaviour. Name the
  file when the reader will need it.
- Describe the system as it is. Mark planned behaviour as planned, and leave
  out ticket numbers and change history; that belongs in the changelog.
- Use placeholders that cannot be mistaken for real values. Never include real
  secrets, personal data, internal host names or production identifiers.

## 3. Write for the task

Lead with what the reader gets. Include only the sections they need:
prerequisites, steps or contract, examples, expected result, failure handling
and rollback, related pages.

- Keep the repository's package manager, shell, platform and version
  conventions. Name the platform when a command only works on one.
- Mark destructive steps clearly, with what they delete and how to undo them.
- Say what a successful check proves and what still needs a live environment.
- Link to the owning config or contract instead of copying a large table that
  will drift.
- Generated reference stays generated: change its source or generator.
- Use short, plain sentences. Cut filler, hype and repeated summaries.

## 4. Record decisions where the project keeps them

A decision record is for a durable constraint or trade-off future maintainers
need; exploration without a decision is a design note.

- If the project keeps decisions in `CONTEXT.md`, add it to the decision log
  there, with the date, reason, evidence, status (`active` or `superseded`)
  and a `supersedes` link when it replaces an earlier one.
- If it has an ADR folder, follow its numbering, status words and template. Use
  `docs/adr/` only when nothing exists yet.
- Record context, the decision in testable words, rejected alternatives and
  consequences (compatibility, security, data, deployment, rollback).
- Never rewrite an accepted decision. Mark it superseded and link the old and
  new records both ways.

## 5. Diagrams

Add a Mermaid diagram only when it shows a relationship or sequence more
clearly than prose. Pick the type that fits: flowchart for control, sequence
for interaction, state for lifecycle, class or ER for structure. Keep labels
short, quote labels with punctuation, and make the diagram and the prose next
to it describe the same thing. Render it with the repository's existing
tooling; do not install a global tool for one diagram. If nothing renders it
locally, say the diagram was not checked.

## 6. Finish

1. Update the nearest index only if the repository has one.
2. Remove or redirect superseded pages so two pages do not both claim
   authority.
3. Check every changed relative link and anchor resolves, and every code fence
   is closed and tagged.
4. Run the repository's docs checks (link checker, linter, validator) if it has
   them, and report the command and result.

Stop and ask when the code and the existing docs disagree and you cannot tell
which one is intended. Do not pick one silently.

## Reviewing documentation

Apply [documentation checks](references/documentation-checks.md) to changed
docs. Report only problems that break a contract, lead the reader into a wrong
action, hide a risk, or break navigation or automation. Wording preferences are
not findings.
