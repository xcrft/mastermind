#!/usr/bin/env python3
"""
Diagnostic comparison of a vanilla review and the Mastermind auditor.

For each adversarial auditor fixture (a planted defect), run two conditions on
equivalent git fixture trees, preserving committed/uncommitted state:

  vanilla     — plain `claude -p` with shell access to the repo (git/grep/read)
                and a neutral "review this change" prompt. NO mmcg, NO auditor
                system prompt.
  mastermind  — the real auditor path (auditor subagent + live mmcg index).
                Re-run only with --with-mastermind; otherwise compare against
                the suite's own result (`runner.py --suite auditor`).

The vanilla score checks expect.contains and expect.not_contains only. The
Mastermind score includes its full verdict, verification, and telemetry
contract. These are different measures; their difference is not quality uplift.
Neither is an LLM judge or a semantic assessment of the final reasoning.

Golden (no-defect) cases — expect.verdict == "held" — are excluded. Counts
describe these planted-defect cases only, not overall research quality.

Usage:
  python evals/ablation.py                       # vanilla over all defect cases
  python evals/ablation.py --case a-005-...      # one case
  python evals/ablation.py --with-mastermind     # run both conditions
  python evals/ablation.py --model sonnet        # default: opus
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

if __package__:
    from . import runner
else:
    import runner

VANILLA_SYSTEM = (
    "You are a senior engineer doing a pre-merge review of a code change. The "
    "working directory is a real git repository and you have shell access — use "
    "`git diff`, `git log`, `cat`, `grep` to inspect the actual code. You are "
    "given the author's own summary of what they changed. Verify that summary "
    "against the code and flag anything wrong or risky: a claimed change that "
    "isn't in the diff, a call to a symbol that doesn't exist, a test claimed "
    "but absent, files changed beyond what was described, a removed symbol that "
    "is still referenced. Name the specific symbol or file. If everything checks "
    "out, say the change looks correct."
)


def vanilla_message(case: dict, fixture_path, baseline_ref: str, after_ref: str) -> str:
    inp = case["input"]
    if case.get("staged_paths") is not None:
        inspection = (
            "The author's changes are not committed. Compare the baseline with "
            f"the current working tree using `git diff {baseline_ref} --`, then "
            "inspect `git ls-files --others --exclude-standard --` and "
            "`git status --porcelain=v1 --untracked-files=all`.\n\n"
        )
    else:
        inspection = (
            f"Author's commit tag: `{after_ref}` (after).\n"
            f"Inspect it with `git diff {baseline_ref}..{after_ref}`.\n\n"
        )
    return (
        f"Working directory: `{fixture_path}` (a git repo).\n"
        f"Baseline tag: `{baseline_ref}` (before the change).\n"
        f"{inspection}"
        f"Author's summary:\n{inp.get('spec_summary', '')}\n\n"
        f"Author's report:\n```\n{inp.get('executor_report', '')}\n```\n\n"
        "Review it. Is the summary accurate and the change safe to merge?"
    )


def scored_caught(output: str, expect: dict) -> bool:
    o = output.lower()
    contains_ok = all(p.lower() in o for p in expect.get("contains", []))
    not_contains_ok = all(p.lower() not in o for p in expect.get("not_contains", []))
    return contains_ok and not_contains_ok


def run_vanilla(model: str, case: dict) -> bool | None:
    """True if vanilla caught the planted defect, False if missed, None on error."""
    fixture = runner.setup_fixture(
        case["fixture"], case["baseline_ref"], case["after_ref"],
        staged_paths=case.get("staged_paths"),
    )
    try:
        msg = vanilla_message(case, fixture, case["baseline_ref"], case["after_ref"])
        cmd = [
            "claude", "-p",
            "--model", model,
            "--append-system-prompt", VANILLA_SYSTEM,
            "--output-format", "json",
            "--no-session-persistence",
            "--permission-mode", "default",
            "--add-dir", str(fixture),
        ]
        try:
            proc = subprocess.run(
                cmd, input=msg, capture_output=True, text=True,
                env=runner._PROC_ENV, timeout=480,
            )
        except subprocess.TimeoutExpired:
            return None
        if proc.returncode != 0:
            return None
        try:
            output = json.loads(proc.stdout).get("result", "")
        except json.JSONDecodeError:
            output = proc.stdout
        return scored_caught(output, case.get("expect", {}))
    finally:
        runner.teardown_fixture(fixture)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--case", help="run one case by id")
    ap.add_argument("--model", default="opus")
    ap.add_argument("--with-mastermind", action="store_true", help="also re-run the auditor path")
    args = ap.parse_args()

    if not all(map(__import__("shutil").which, ("claude", "git"))):
        print("error: `claude` and `git` must be on PATH.", file=sys.stderr)
        return 2

    cases_file = runner.EVALS_DIR / "auditor.jsonl"
    defect_cases = []
    with cases_file.open() as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("//"):
                continue
            c = json.loads(line)
            if args.case and c["id"] != args.case:
                continue
            verdict = c.get("expect", {}).get("verdict")
            is_golden = verdict == "held" or (isinstance(verdict, list) and verdict == ["held"])
            if is_golden:
                continue  # no planted defect to catch
            defect_cases.append(c)

    if not defect_cases:
        print("no defect cases matched")
        return 0

    print(f"\n=== diagnostic comparison over {len(defect_cases)} defect case(s) · {args.model} ===\n")
    rows = []
    for c in defect_cases:
        cid = c["id"]
        print(f"  [{cid}] vanilla ...", end=" ", flush=True)
        v = run_vanilla(args.model, c)
        v_str = "phrase pass" if v else ("phrase miss" if v is False else "err")
        print(v_str, end="", flush=True)
        m = None
        if args.with_mastermind:
            r = runner.evaluate_case(args.model, "auditor", runner.SUITES["auditor"], c, keep_fixtures=False)
            m = r.passed
            print(f"  · mastermind contract {'pass' if m else 'fail'}", end="")
        print()
        rows.append((cid, v, m))

    v_caught = sum(1 for _, v, _ in rows if v)
    print(f"\n  vanilla phrase checks passed: {v_caught}/{len(rows)}")
    if args.with_mastermind:
        m_caught = sum(1 for _, _, m in rows if m)
        print(f"  mastermind full contract passed: {m_caught}/{len(rows)}")
        print("  Different grading contracts: no quality-uplift estimate.")
    else:
        print("  mastermind was not run; use --with-mastermind for its full contract result.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
