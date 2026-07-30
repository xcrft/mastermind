---
name: mastermind-product-intake
description: Convert a PRD, ticket, or feature request into a task contract that can fail — behaviour split from outcome, product nouns resolved to symbols that exist, unspecified cases surfaced as questions, and success metrics parked where a merge gate cannot wave them through. Use when product writing is handed over for implementation.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - product
    - planning
    - intake
---

# Mastermind product intake

A PRD is written to decide whether to build something. A task contract is
written to check whether it was built. They are different documents, and pasting
the first into the second produces acceptance criteria that no gate can fail.

This is a planning step. It converts; it does not implement, and it does not
judge whether the work is worth doing.

## Name the source

Record the PRD, ticket, or document with a stable reference — an ID or a URL,
not a title. "Per the spec" is unauditable: a product doc that moved on cannot
be told apart from an implementation that drifted.

## Sort every statement into three piles

This is the whole job, and most of the value is in refusing to blur them.

**Behaviour** — what the system does, observable at merge time. *"An archived
invoice is excluded from the export."* This becomes acceptance criteria.

**Constraint** — a bound the behaviour must respect. *"The export completes
within 30 seconds for 10,000 invoices."* This becomes a criterion **only if**
you also name how it is measured. Unmeasured, it is decoration that reads like
rigour.

**Outcome** — why the work exists. *"Reduce support tickets about missing
invoices."* This is never acceptance criteria. It cannot fail at merge time, it
resolves weeks later in production if anyone looks, and putting it in the
contract means a mechanical gate marks it satisfied while nobody has measured
anything.

A criterion that cannot fail on the day the change lands belongs in the outcome
section, no matter how central it is to the PRD.

## Resolve the nouns

Product writing names things: invoice, export, workspace, role, plan. Search
each one before assuming it is new.

```text
mmcg_search Invoice
mmcg_search exportInvoices
mmcg_api_surface src/billing/
```

A feature that reads as new is usually mostly existing surface with a gap in the
middle. Say which nouns already exist as symbols, which exist under a different
name, and which are genuinely new — that distinction changes the scope, the
risk, and often the mode.

Where the PRD's word and the code's word differ, record both. A contract that
says `export` while the code says `generateStatement` will be audited against
the wrong symbol.

## Surface what the PRD does not say

The most expensive thing in product writing is the case it never mentions.
Before the contract is approved, ask about the ones that change the delivered
behaviour:

- what happens to items already in flight when the feature turns on;
- the empty state, and the single-item state;
- partial failure — half the export succeeded, then what;
- who is allowed to do this, and what an unauthorised attempt sees;
- whether existing data needs backfilling, and what the old rows mean until it
  runs.

Do not answer these silently. An executor handed an under-specified contract
will invent an answer, the audit will hold, and the invented answer ships.

## Write criteria that can fail

- *Verifiable:* "an archived invoice is absent from the export payload";
  "requesting another workspace's export returns 403"; "an export of zero
  invoices returns an empty file rather than an error".
- *Not verifiable, and must not sit in acceptance criteria:* "the export feels
  fast"; "users understand what they downloaded"; "reduces support load".

## Park the outcome explicitly

Give outcomes their own section: the metric, how it is measured, when it is
read, and who reads it. That keeps the product bet visible instead of dissolving
it into criteria a merge gate will approve.

If a metric has no measurement in place, say so. "We will track it" without an
instrument is the same as not tracking it, and the contract should record which
one is true.

## What this does not do

It does not write the PRD, prioritise, or validate the product assumption —
there is no repository fact against which "users want this" can be checked, and
this workflow does not pretend otherwise. It produces a contract whose
behavioural half is checkable and whose unverifiable half is named rather than
hidden. Everything downstream — [[mastermind-task-planning]], the executor, the
audit — operates on the first half only.
