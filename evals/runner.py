#!/usr/bin/env python3
"""
Local eval runner for mastermind subagents.

Uses the `claude` CLI in non-interactive mode (`-p`), so authentication runs
through your existing Claude Code login — no ANTHROPIC_API_KEY needed, costs
count against your Claude subscription (flat monthly, not per-token).

Usage:
  python evals/runner.py                  # all suites
  python evals/runner.py --suite critic   # one suite
  python evals/runner.py --case c-001     # one case
  python evals/runner.py --model opus     # model alias or full name (default: opus)

Prerequisites:
  - `claude` CLI on PATH (Claude Code installed, logged in)
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
EVALS_DIR = Path(__file__).resolve().parent

SUITES = {
    "critic": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-critic.md",
        "cases": EVALS_DIR / "critic.jsonl",
        "renderer": "render_critic_input",
    },
    "auditor": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-auditor.md",
        "cases": EVALS_DIR / "auditor.jsonl",
        "renderer": "render_auditor_input",
    },
}


@dataclass
class Result:
    case_id: str
    suite: str
    passed: bool
    reasons: list[str]
    duration_ms: int


def strip_frontmatter(text: str) -> str:
    if text.startswith("---\n"):
        end = text.find("\n---\n", 4)
        if end != -1:
            return text[end + 5 :].lstrip()
    return text


def render_critic_input(inp: dict) -> str:
    alternatives = inp.get("alternatives", "")
    if isinstance(alternatives, list):
        alternatives = "\n".join(f"- {a}" for a in alternatives)
    return (
        f"**Problem:** {inp.get('problem', '')}\n\n"
        f"**Proposed design:** {inp.get('design', '')}\n\n"
        f"**Alternatives considered:**\n{alternatives}\n\n"
        f"**Constraints:** {inp.get('constraints', '')}\n\n"
        f"**mmcg snapshot:** {inp.get('mmcg_snapshot', '')}"
    )


def render_auditor_input(inp: dict) -> str:
    return (
        f"**Spec summary:**\n{inp.get('spec_summary', '')}\n\n"
        f"**Executor report:**\n```\n{inp.get('executor_report', '')}\n```\n\n"
        f"**git diff (synthetic):**\n```\n{inp.get('git_diff', '')}\n```"
    )


RENDERERS = {
    "render_critic_input": render_critic_input,
    "render_auditor_input": render_auditor_input,
}


def evaluate_case(model: str, suite_name: str, suite_cfg: dict, case: dict) -> Result:
    case_id = case["id"]
    system_prompt = strip_frontmatter(suite_cfg["subagent"].read_text())
    user_message = RENDERERS[suite_cfg["renderer"]](case["input"])

    cmd = [
        "claude",
        "-p",
        "--model", model,
        "--append-system-prompt", system_prompt,
        "--output-format", "json",
        "--no-session-persistence",
        "--permission-mode", "default",
        user_message,
    ]

    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired:
        return Result(case_id, suite_name, False, ["timeout after 180s"], 0)

    if proc.returncode != 0:
        err = (proc.stderr or proc.stdout or "").strip()[:300]
        return Result(case_id, suite_name, False, [f"claude exit {proc.returncode}: {err}"], 0)

    # claude -p --output-format json returns: {"type":"result","result":"...","duration_ms":..., ...}
    try:
        payload = json.loads(proc.stdout)
        output = payload.get("result", "")
        duration_ms = int(payload.get("duration_ms", 0))
    except json.JSONDecodeError:
        output = proc.stdout
        duration_ms = 0

    expect = case.get("expect", {})
    reasons: list[str] = []
    passed = True

    expected_verdict = expect.get("verdict")
    if expected_verdict:
        # accept string OR list of acceptable verdicts (any-of)
        candidates = [expected_verdict] if isinstance(expected_verdict, str) else list(expected_verdict)
        if not any(re.search(rf"\b{re.escape(v)}\b", output, re.IGNORECASE) for v in candidates):
            passed = False
            reasons.append(f"none of expected verdicts {candidates} found")

    for phrase in expect.get("contains", []):
        if phrase.lower() not in output.lower():
            passed = False
            reasons.append(f"missing phrase: {phrase!r}")

    for phrase in expect.get("not_contains", []):
        if phrase.lower() in output.lower():
            passed = False
            reasons.append(f"forbidden phrase present: {phrase!r}")

    return Result(case_id, suite_name, passed, reasons, duration_ms)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--suite", choices=list(SUITES.keys()), help="run one suite only")
    parser.add_argument("--case", help="run one case by id")
    parser.add_argument("--model", default="opus", help="model alias (opus/sonnet/haiku) or full name; default opus")
    args = parser.parse_args()

    if not shutil.which("claude"):
        print("error: `claude` CLI not on PATH. Install Claude Code: https://claude.com/claude-code", file=sys.stderr)
        return 2

    suites_to_run = [args.suite] if args.suite else list(SUITES.keys())
    results: list[Result] = []

    for suite_name in suites_to_run:
        suite_cfg = SUITES[suite_name]
        if not suite_cfg["cases"].exists():
            print(f"  skip suite {suite_name} — no cases file at {suite_cfg['cases']}")
            continue
        print(f"\n=== {suite_name} ===")
        with suite_cfg["cases"].open() as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("//"):
                    continue
                case = json.loads(line)
                if args.case and case["id"] != args.case:
                    continue
                print(f"  [{case['id']}] running ...", end=" ", flush=True)
                r = evaluate_case(args.model, suite_name, suite_cfg, case)
                results.append(r)
                status = "✓ pass" if r.passed else "✗ FAIL"
                print(f"{status}  ({r.duration_ms}ms)")
                for reason in r.reasons:
                    print(f"      → {reason}")

    if not results:
        print("\nno cases matched filter")
        return 0

    n_pass = sum(r.passed for r in results)
    n_fail = len(results) - n_pass
    total_ms = sum(r.duration_ms for r in results)
    print(f"\n=== summary ===")
    print(f"  passed: {n_pass}/{len(results)}")
    print(f"  total time: {total_ms / 1000:.1f}s")
    return 0 if n_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
