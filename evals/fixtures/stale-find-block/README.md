# Stale source snapshot fixture

A Python service whose spec refers to `UserService.authenticate` from an earlier revision.

| Variant | Planted behavior |
|---|---|
| `renamed` | Renames `authenticate` to `verify` while the report retains the old symbol name. |

`baseline/` is the initial tree. Each `changes/<variant>/` is a complete
replacement tree. Omitted baseline files are deleted. The runner creates a
disposable Git repository and supplies a live index where the case requires it.

The case in [`auditor.jsonl`](../../auditor.jsonl) defines the verdict and evidence
assertions. Inspect source and Git changes as well as graph results: an empty
index query alone does not establish that a symbol cannot exist at runtime.

To add a variant, provide its complete tree and a case with this fixture name.
Keep expected verdicts out of the source files shown to the evaluated agent.
See the [eval guide](../../README.md#add-or-change-a-case).
