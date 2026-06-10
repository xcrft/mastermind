# stale-find-block fixture

A minimal Python `UserService` used to verify that the auditor catches specs
whose pre-edit symbol snapshot is stale — the referenced symbol was renamed
or deleted before the executor ran.

## Failure mode: stale find block

The spec was authored when `UserService.authenticate` existed. Between spec
authoring and execution, a separate PR renamed `authenticate` → `verify` and
landed on main. The executor ran against the updated codebase but its report
copied the stale symbol name from the spec.

The auditor runs `mmcg_search authenticate` against the live index (the
after-tree) → zero results. The symbol in the pre-edit snapshot no longer
exists. This is a "stale find block" — the snapshot recorded in the spec
was accurate at authoring time but is outdated at execution time.

## Layout

```
baseline/
  src/auth.py   — UserService with authenticate(), get_user(), invalidate()

changes/
  renamed/
    src/auth.py — authenticate() renamed to verify(); rest unchanged
```

## What the auditor must catch

1. Run `mmcg_search authenticate` → empty (renamed to `verify`).
2. Spec pre-edit snapshot references a symbol that no longer exists.
3. Executor report does not acknowledge the rename.
4. Verdict: `drift` or `broken`.

## Adding a new variant

Create `changes/<variant>/src/` with the modified file tree and add a JSONL
entry in `evals/auditor.jsonl` with `"fixture": "stale-find-block"`.
