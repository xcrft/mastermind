# Example: hallucinated symbol

**"Tests passed" is not enough.**

This example shows why: the executor's report looked clean — one file changed,
scope honored, `go test ./...` green. The auditor proved it was broken anyway.

## What the executor claimed

> "[x] Wired CancelOrder to call the existing ProcessPayment() for refund flow"

## What the auditor found

`ProcessPayment` does not exist. It was never defined in the codebase.
`CancelOrder` does not call anything. The go test passed because there are
zero tests — a vacuous green.

## Why this matters

A planner reading the executor's report would believe a refund integration
existed. It does not. No test would catch the missing behavior at runtime.
Without the audit gate, a false implementation narrative ships.

## The Mastermind signal chain

1. `git diff --name-only` — one file changed (`pkg/checkout/checkout.go`) ✓  
2. `mmcg_search ProcessPayment` — **zero results** → symbol hallucinated ✗  
3. Grep `CancelOrder` body — single guard clause, **no call to anything** ✗  
4. No `*_test.go` files — `go test` green but **zero tests executed** ✗

Verdict: **contract broken**. The headline claim ("wired to existing symbol")
is false on two levels: the symbol doesn't exist, and the wiring wasn't added.

## Files

- `spec.md` — what the planner wrote before handing off to the executor
- `executor-report.md` — what the executor claimed it did
- `audit-output.md` — the auditor's mechanical verification
- Source fixtures live in `evals/fixtures/hallucinated-symbol/`
- Eval case: `evals/auditor.jsonl` → `a-005-hallucinated-symbol`
