# Unsupported symbol fixture

A Go checkout package whose executor report claims an integration with `ProcessPayment`. The definition is absent from both source trees.

| Variant | Planted behavior |
|---|---|
| `executor-added` | Adds `CancelOrder` while the claimed `ProcessPayment` integration remains absent. |

`baseline/` is the initial tree. Each `changes/<variant>/` is a complete
replacement tree. Omitted baseline files are deleted. The runner creates a
disposable Git repository and supplies a live index where the case requires it.

The case in [`auditor.jsonl`](../../auditor.jsonl) defines the verdict and evidence
assertions. Inspect source and Git changes as well as graph results: an empty
index query alone does not establish that a symbol cannot exist at runtime.

To add a variant, provide its complete tree and a case with this fixture name.
Keep expected verdicts out of the source files shown to the evaluated agent.
See the [eval guide](../../README.md#add-or-change-a-case).
