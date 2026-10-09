# Comment contract fixture

A Python billing change with an explicit no-comment rule and a supplied implementation block.

| Variant | Planted behavior |
|---|---|
| `with-slop` | Adds `total(items)` plus comments that repeat the code and a banner, contradicting the report and the approved block. |

`baseline/` is the initial tree. Each `changes/<variant>/` is a complete
replacement tree. Omitted baseline files are deleted. The runner creates a
disposable Git repository and supplies a live index where the case requires it.

The case in [`auditor.jsonl`](../../auditor.jsonl) defines the verdict and evidence
assertions. Inspect source and Git changes as well as graph results: an empty
index query alone does not establish that a symbol cannot exist at runtime.

To add a variant, provide its complete tree and a case with this fixture name.
Keep expected verdicts out of the source files shown to the evaluated agent.
See the [eval guide](../../README.md#add-or-change-a-case).
