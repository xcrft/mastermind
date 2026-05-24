# Skill template

Copy this folder when adding a new skill that needs more than a single file. If your skill is just one markdown file with no scripts/assets/references, use a single file instead: `skills/<domain>/<slug>.md`.

```bash
cp -r skills/_template skills/<domain>/<your-slug>
# then edit SKILL.md, delete optional folders you don't need
```

Optional folders (delete what you don't use):

- `references/` — long-form docs, checklists, citations
- `scripts/` — helper scripts the skill invokes
- `assets/` — templates, fixtures, images
