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
import re
import shlex
import shutil
import sys
from pathlib import Path

if __package__:
    from . import runner
else:
    import runner

VANILLA_SYSTEM = (
    "You are a senior engineer doing a pre-merge review of a code change. The "
    "working directory is a real git repository. Use read-only Git commands and "
    "the provided read/search tools to inspect the actual code. You are "
    "given the author's own summary of what they changed. Verify that summary "
    "against the code and flag anything wrong or risky: a claimed change that "
    "isn't in the diff, a call to a symbol that doesn't exist, a test claimed "
    "but absent, files changed beyond what was described, a removed symbol that "
    "is still referenced. Name the specific symbol or file. If everything checks "
    "out, say the change looks correct."
)

VANILLA_ALLOWED_TOOLS = tuple(
    tool
    for tool in runner.AUDITOR_SAFE_ALLOWED_TOOLS
    if not tool.startswith("Bash(cargo ")
)
VANILLA_STREAM_TOOLS = ("Read", "Glob", "Grep", "Bash", "EndConversation")
_READ_ONLY_GIT_SUBCOMMANDS = frozenset(
    {"diff", "grep", "log", "ls-files", "rev-parse", "show", "status"}
)
_SHELL_CONTROL_RE = re.compile(r"[;&|<>`$()\r\n]")


def vanilla_message(case: dict, fixture_path, baseline_ref: str, after_ref: str) -> str:
    inp = case["input"]
    baseline_tag = runner.fixture_tag_ref(baseline_ref, "baseline_ref")
    after_tag = runner.fixture_tag_ref(after_ref, "after_ref")
    if case.get("staged_paths") is not None:
        inspection = (
            "The author's changes are not committed. Compare the baseline with "
            f"the current working tree using `git diff {baseline_tag} --`, then "
            "inspect `git ls-files --others --exclude-standard --` and "
            "`git status --porcelain=v1 --untracked-files=all`.\n\n"
        )
    else:
        inspection = (
            f"Author's commit tag: `{after_tag}` (after).\n"
            f"Inspect it with `git diff {baseline_tag}..{after_tag} --`.\n\n"
        )
    return (
        f"Working directory: `{fixture_path}` (a git repo).\n"
        f"Baseline tag: `{baseline_tag}` (before the change).\n"
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


def _successful_git_inspection(executions: list[runner.ToolExecution]) -> bool:
    for execution in executions:
        if (
            execution.name != "Bash"
            or not execution.result_seen
            or not execution.succeeded
        ):
            continue
        command = execution.arguments.get("command")
        if not isinstance(command, str) or _SHELL_CONTROL_RE.search(command):
            continue
        try:
            arguments = shlex.split(command)
        except ValueError:
            continue
        if (
            len(arguments) >= 2
            and arguments[0] == "git"
            and arguments[1] in _READ_ONLY_GIT_SUBCOMMANDS
        ):
            return True
    return False


def run_vanilla(
    model: str,
    case: dict,
    *,
    claude_binary: str | Path = "claude",
    claude_version: str | None = None,
    git_binary: str | Path = "git",
    fixtures_dir: Path | None = None,
) -> bool | None:
    """True if vanilla caught the planted defect, False if missed, None on error."""
    try:
        fixture = runner.setup_fixture(
            case["fixture"], case["baseline_ref"], case["after_ref"],
            staged_paths=case.get("staged_paths"),
            fixtures_dir=fixtures_dir,
            git_binary=git_binary,
            mmcg_binary=None,
        )
    except (OSError, RuntimeError, ValueError):
        return None
    try:
        msg = vanilla_message(case, fixture, case["baseline_ref"], case["after_ref"])
        limits = runner.case_runtime_limits("auditor", case.get("expect", {}))
        cmd = [
            str(claude_binary), "-p",
            "--model", model,
            "--effort", runner.evaluation_effort(
                "auditor", runner.SUITES["auditor"]["subagent"]
            ),
            "--append-system-prompt", VANILLA_SYSTEM,
            "--output-format", "stream-json",
            "--verbose",
            "--no-session-persistence",
            "--permission-mode", "dontAsk",
            "--max-turns", str(limits["max_turns"]),
            "--tools", "Read,Glob,Grep,Bash",
            "--allowedTools", ",".join(VANILLA_ALLOWED_TOOLS),
            "--setting-sources", "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-chrome",
            "--add-dir", str(fixture),
        ]
        git_path = Path(git_binary)
        pinned_executables = (git_path,) if git_path.is_absolute() else ()
        environment = runner.evaluation_environment(
            limits["max_output_tokens"],
            pinned_executables=pinned_executables,
        )
        process = runner.run_bounded(
            cmd,
            cwd=fixture,
            env=environment,
            stdin=msg.encode("utf-8"),
            timeout=runner.CLAUDE_CASE_TIMEOUT_SECONDS,
            stdout_limit=runner.CLAUDE_STDOUT_LIMIT_BYTES,
            stderr_limit=runner.CLAUDE_STDERR_LIMIT_BYTES,
            start_new_session=True,
        )
        if process.stop_reason is not None or process.returncode != 0:
            return None
        try:
            stdout = process.stdout.decode("utf-8")
        except UnicodeDecodeError:
            return None
        try:
            runtime_contract = (
                None
                if claude_version is None
                else runner.StreamRuntimeContract(
                    cwd=str(fixture),
                    tools=VANILLA_STREAM_TOOLS,
                    mcp_servers=(),
                    claude_code_version=runner.claude_stream_version(
                        claude_version
                    ),
                )
            )
            payload, _, executions = runner.parse_claude_output(
                stdout,
                streamed=True,
                runtime_contract=runtime_contract,
            )
        except (json.JSONDecodeError, TypeError, ValueError):
            return None
        telemetry = runner.telemetry_from_payload(payload)
        if telemetry["complete"] is not True or payload.get("permission_denials"):
            return None
        if not _successful_git_inspection(executions):
            return None
        output = payload["result"]
        return scored_caught(output, case.get("expect", {}))
    finally:
        runner.teardown_fixture(fixture)


def _mastermind_outcome(result: runner.Result) -> bool | None:
    if not result.telemetry_complete or any(
        reason.startswith("permission denied for tools") for reason in result.reasons
    ):
        return None
    return result.passed


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--case", help="run one case by id")
    ap.add_argument("--model", default="opus")
    ap.add_argument("--with-mastermind", action="store_true", help="also re-run the auditor path")
    args = ap.parse_args()

    claude_location = shutil.which("claude")
    git_location = shutil.which("git")
    if claude_location is None or git_location is None:
        print("error: `claude` and `git` must be on PATH.", file=sys.stderr)
        return 2
    mmcg_location = shutil.which(str(runner.MMCG_BIN))
    try:
        claude_binary = Path(claude_location).resolve(strict=True)
        git_binary = Path(git_location).resolve(strict=True)
        mmcg_binary = (
            None
            if mmcg_location is None
            else Path(mmcg_location).resolve(strict=True)
        )
        claude_definition = runner._stable_regular_file_definition(claude_binary)
        claude_version = runner.claude_cli_version(claude_binary)
        fixture_runtime = runner.fixture_runtime_definition(
            git_binary, mmcg_binary if args.with_mastermind else None
        )
    except (OSError, RuntimeError, ValueError) as error:
        print(f"error: cannot freeze ablation runtime: {error}", file=sys.stderr)
        return 2
    if claude_version is None:
        print("error: cannot read `claude` CLI version.", file=sys.stderr)
        return 2

    cases_file = runner.EVALS_DIR / "auditor.jsonl"
    try:
        cases = runner.load_case_records(cases_file, suite_name="auditor")
    except (OSError, ValueError) as error:
        print(f"error: cannot load auditor cases: {error}", file=sys.stderr)
        return 2
    selected = [case for case in cases if not args.case or case["id"] == args.case]
    if not selected:
        print("error: no auditor cases matched", file=sys.stderr)
        return 2
    defect_cases = []
    for case in selected:
        verdict = case["expect"]["verdict"]
        is_golden = verdict == "held" or (
            isinstance(verdict, list) and verdict == ["held"]
        )
        if not is_golden:
            defect_cases.append(case)

    if not defect_cases:
        print("no defect cases matched")
        return 0
    if (
        args.with_mastermind
        and mmcg_binary is None
        and any(not case.get("allow_no_mmcg") for case in defect_cases)
    ):
        print(
            "error: `mmcg` must be available for the selected Mastermind cases.",
            file=sys.stderr,
        )
        return 2

    print(f"\n=== diagnostic comparison over {len(defect_cases)} defect case(s) · {args.model} ===\n")
    rows = []
    for c in defect_cases:
        cid = c["id"]
        print(f"  [{cid}] vanilla ...", end=" ", flush=True)
        v = run_vanilla(
            args.model,
            c,
            claude_binary=claude_binary,
            claude_version=claude_version,
            git_binary=git_binary,
        )
        v_str = "phrase pass" if v else ("phrase miss" if v is False else "err")
        print(v_str, end="", flush=True)
        m = None
        if args.with_mastermind:
            result = runner.evaluate_case(
                args.model,
                "auditor",
                runner.SUITES["auditor"],
                c,
                keep_fixtures=False,
                claude_binary=claude_binary,
                claude_version=claude_version,
                git_binary=git_binary,
                mmcg_binary=mmcg_binary,
            )
            m = _mastermind_outcome(result)
            m_str = "pass" if m else ("fail" if m is False else "err")
            print(f"  · mastermind contract {m_str}", end="")
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
    errors = sum(
        v is None or (args.with_mastermind and m is None) for _, v, m in rows
    )
    if errors:
        print(f"  infrastructure errors: {errors}", file=sys.stderr)

    try:
        runtime_stable = (
            runner._stable_regular_file_definition(claude_binary)
            == claude_definition
            and runner.claude_cli_version(claude_binary) == claude_version
            and runner.fixture_runtime_definition(
                git_binary, mmcg_binary if args.with_mastermind else None
            )
            == fixture_runtime
        )
    except (OSError, RuntimeError, ValueError):
        runtime_stable = False
    if not runtime_stable:
        print("error: ablation runtime changed during evaluation", file=sys.stderr)
        return 1
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
