# hallucinated-symbol fixture

A Go checkout package. The executor report claims `CancelOrder` calls an
existing `ProcessPayment` function, but that function exists in neither
`baseline/` nor `changes/executor-added/`.

The auditor should search the live index, find no `ProcessPayment` symbol,
and return `broken` for the invented integration point.

Add variants under `changes/<variant>/` and reference them from
[`auditor.jsonl`](../../auditor.jsonl).
