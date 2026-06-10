# Skills

Skills used by the Mastermind workflow. Core skills are installed by `mastermind init`. Non-core skills live in [`extras/`](../extras/).

See [`../docs/skill-anatomy.md`](../docs/skill-anatomy.md) for the format. Adding a new skill? Copy [`_template/`](_template/) and follow [`../CONTRIBUTING.md`](../CONTRIBUTING.md).

## Index

### workflow/
| Skill | Description |
|---|---|
| [`mastermind-task-planning`](workflow/mastermind-task-planning/SKILL.md) | CTO/planner mode — brainstorms with user and writes detailed `.mastermind/tasks/<NNN>-<name>/spec.md` files (folder per task) for delegation. Never implements. |
| [`mastermind-task-executor`](workflow/mastermind-task-executor/SKILL.md) | Executes a `.mastermind/tasks/<NNN>-<name>/spec.md` file phase-by-phase, runs VERIFY, marks the checklist, stops on first failure. |

### prompt-engineering/
| Skill | Description |
|---|---|
| [`mastermind-prompt-refiner`](prompt-engineering/mastermind-prompt-refiner/SKILL.md) | Refines a user's rough prompt into a clean version before handing off to another agent. One-pass refine + handoff. |

---

## Adding a skill

1. Read [`../docs/skill-anatomy.md`](../docs/skill-anatomy.md).
2. Pick a domain folder (or propose a new one in your PR).
3. Copy `_template/`:
   ```bash
   cp -r skills/_template skills/<domain>/<your-slug>
   ```
4. Fill in `SKILL.md`. Drop the folder layout if your skill is a single file.
5. Add an entry to this index.
6. Open a PR.
