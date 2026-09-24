# stale-find-block fixture

The spec's pre-edit snapshot names `UserService.authenticate`. The
`renamed` variant has `verify` instead; the executor report does not mention
the rename.

The auditor should search the current index, find no `authenticate` symbol,
and return `drift` or `broken` for the stale snapshot.

Add variants under `changes/<variant>/` and reference them from
[`auditor.jsonl`](../../auditor.jsonl).
