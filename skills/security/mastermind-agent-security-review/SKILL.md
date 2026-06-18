---
name: mastermind-agent-security-review
description: OWASP reference pack for Mastermind security audits — the verified OWASP ASI (Agentic) Top 10 mapped to Mastermind surfaces, plus slots for LLM/Web/API. Read by the security auditor in OWASP mode so mappings are evidence-based, not from memory.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - security
    - review
    - agentic-security
    - owasp
    - mcp
---

# Mastermind Agent Security Review

The OWASP reference pack the `mastermind-security-auditor` subagent reads in OWASP mode. The review *protocol* lives in that subagent (it runs self-contained, without the Skill tool); this skill exists to ship the **evidence-grounded OWASP lists** so compliance mappings aren't reconstructed from memory — OWASP IDs and wording drift, and the agentic/LLM lists are new.

## When to use

OWASP mode only — when the user asks for OWASP / ASI / compliance / agentic security audit, or a caller explicitly requests OWASP mapping. For ordinary security review the auditor finds concrete risks without OWASP mapping.

## Reference pack

- [`references/owasp-asi-compliance.md`](references/owasp-asi-compliance.md) — OWASP Top 10 for Agentic Applications (2026), ASI01–ASI10, mapped to Mastermind surfaces.

LLM / Web / API OWASP references can be added here as needed. Until a local reference exists for a framework, the auditor reports `insufficient evidence for exact OWASP mapping` rather than guessing IDs from memory.

## Related skills

- [[mastermind-codegraph-research]] — structural facts the review verifies against
- [[mastermind-critical-review]] — general (non-security) design / spec critique
