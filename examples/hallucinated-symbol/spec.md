# Task: add CancelOrder to checkout package

## Goal

Add `CancelOrder(id string) error` to `pkg/checkout/checkout.go`.
The function should validate the id is non-empty and return an error otherwise.

## Pre-edit symbol snapshot

| symbol | callers | signature |
|---|---|---|
| `SubmitOrder` | 0 (no external callers) | `func SubmitOrder(o Order) error` |

## Scope

Single file change: `pkg/checkout/checkout.go` only.

## Tests Plan

- Add `TestCancelOrder_emptyID` asserting `CancelOrder("")` returns a non-nil error.
- Add `TestCancelOrder_validID` asserting `CancelOrder("ord-123")` returns nil.

## What I am NOT doing

- No refund integration in this task — that is a separate spec.
- No changes to `SubmitOrder`, `ValidateCart`, or any other function.
