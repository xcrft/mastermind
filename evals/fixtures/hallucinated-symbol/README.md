# hallucinated-symbol fixture

A minimal Go checkout package used to verify that the auditor catches
executor reports that reference symbols that do not exist in the codebase.

## Failure mode: hallucinated symbol

The executor's report claims it "wired `CancelOrder` to call the existing
`ProcessPayment()` for refund flows". `ProcessPayment` was never defined
in either the baseline or the after-tree. Running `mmcg_search ProcessPayment`
against the live index returns zero results.

## Layout

```
baseline/
  pkg/checkout/checkout.go   — SubmitOrder + ValidateCart (no ProcessPayment)

changes/
  executor-added/
    pkg/checkout/checkout.go — adds CancelOrder per spec; ProcessPayment still absent
```

## What the auditor must catch

1. Run `mmcg_search ProcessPayment` → empty result.
2. Executor report references a symbol that does not exist in the index.
3. Verdict: `broken` — the claimed integration point was hallucinated.

## Adding a new variant

Follow the same pattern as `evals/fixtures/fake-session`. Add a JSONL entry
in `evals/auditor.jsonl` with `"fixture": "hallucinated-symbol"`.
