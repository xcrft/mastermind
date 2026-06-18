# OWASP ASI compliance reference

Source: **OWASP Top 10 for Agentic Applications (2026)**, OWASP GenAI Security Project (genai.owasp.org). The ASI IDs and titles are OWASP's; the **Mastermind surface** column — where each risk shows up in Mastermind's own architecture — is ours.

Use in OWASP mode only. Map each finding to the ASI ID below; do not reconstruct IDs or wording from memory — read them here.

| ID | Risk | What it is | Mastermind surface |
|---|---|---|---|
| ASI01 | Agent Goal Hijack | hidden prompts redirect the agent into unauthorized actions (e.g. silent exfiltration) | injected instructions in specs, external docs, MCP tool outputs, executor reports, user snippets |
| ASI02 | Tool Misuse | agent weaponizes legitimate tools for destructive output | Bash, file writes, MCP tools, GitHub connector, destructive actions |
| ASI03 | Identity & Privilege Abuse | agent operates beyond authorized scope; leaked credentials | per-role tool permissions, subagent scope, secrets/tokens in specs or env |
| ASI04 | Agentic Supply Chain | runtime components / ecosystem poisoned | copied skills, MCP servers, npm packages, GitHub Actions, native binaries |
| ASI05 | Unexpected Code Execution | natural-language paths unlock remote code execution | executor edits + VERIFY commands + Bash; agent-generated code executed unsandboxed |
| ASI06 | Memory & Context Poisoning | corrupted persistent memory reshapes later behavior | CONTEXT.md, `.mastermind/tasks/` history, scratchpad, `_lessons.md` |
| ASI07 | Insecure Inter-Agent Communication | spoofed / unverified messages between agents | planner ↔ executor ↔ auditor structured report tails; trusting a report without verifying it |
| ASI08 | Cascading Failures | false signals propagate through automated pipelines | run-task loops, retry / escalation logic, planner→executor→auditor automation |
| ASI09 | Human-Agent Trust Exploitation | persuasive output leads the operator to approve harmful actions | executor / auditor / critic verdicts that talk the user into "done" or approval |
| ASI10 | Rogue Agents | misalignment, concealment, autonomous self-direction | behavioral monitoring, kill/stop, runaway loops, an agent hiding what it did |

For each risk in scope: find the concrete Mastermind surface, look for a **deterministic** control (not just a prompt instruction), and record a finding with severity + the missing control. One row per real exposure — don't pad to all ten.
