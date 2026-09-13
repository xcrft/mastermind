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
import tempfile
from dataclasses import dataclass
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


@dataclass(frozen=True)
class ConditionOutcome:
    passed: bool
    resolved_models: tuple[str, ...]

    def __bool__(self) -> bool:
        return self.passed


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
) -> ConditionOutcome | None:
    """Return the phrase result and resolved model identity, or None on error."""
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
        return ConditionOutcome(
            passed=scored_caught(output, case.get("expect", {})),
            resolved_models=tuple(telemetry["resolved_models"]),
        )
    finally:
        runner.teardown_fixture(fixture)


def _mastermind_outcome(result: runner.Result) -> ConditionOutcome | None:
    if not result.telemetry_complete or any(
        reason.startswith("permission denied for tools") for reason in result.reasons
    ):
        return None
    return ConditionOutcome(
        passed=result.passed,
        resolved_models=tuple(result.resolved_models),
    )


def merge_model_identity(
    expected: tuple[str, ...] | None,
    outcome: ConditionOutcome,
    label: str,
) -> tuple[tuple[str, ...], str | None]:
    if expected is None:
        return outcome.resolved_models, None
    if outcome.resolved_models == expected:
        return expected, None
    return (
        expected,
        f"{label} resolved model ids {list(outcome.resolved_models)!r} "
        f"!= {list(expected)!r}",
    )


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
        ablation_definition = runner._stable_regular_file_definition(
            Path(__file__).resolve()
        )
        harness_definition = runner.evaluation_harness_definition()
        repository_revision = runner.git_revision(git_binary)
    except (OSError, RuntimeError, ValueError) as error:
        print(f"error: cannot freeze ablation runtime: {error}", file=sys.stderr)
        return 2
    if claude_version is None:
        print("error: cannot read `claude` CLI version.", file=sys.stderr)
        return 2
    if repository_revision is None:
        print("error: cannot identify repository HEAD.", file=sys.stderr)
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

    cargo_binary: Path | None = None
    rustc_binary: Path | None = None
    frozen_verification_runtime: dict[str, object] | None = None
    uses_verification = args.with_mastermind and any(
        runner.reported_cargo_verification_commands(case)
        for case in defect_cases
    )
    if uses_verification:
        cargo_location = shutil.which("cargo")
        rustc_location = shutil.which("rustc")
        if cargo_location is None or rustc_location is None:
            print(
                "error: `cargo` and `rustc` must be on PATH for "
                "auditor verification.",
                file=sys.stderr,
            )
            return 2
        try:
            cargo_binary, rustc_binary = runner.resolve_verification_binaries(
                Path(cargo_location).absolute(),
                Path(rustc_location).absolute(),
            )
            frozen_verification_runtime = runner.verification_runtime_definition(
                cargo_binary, rustc_binary
            )
            runner.evaluation_environment(
                1,
                source={},
                pinned_executables=(git_binary, cargo_binary, rustc_binary),
            )
        except (OSError, RuntimeError, ValueError) as error:
            print(
                f"error: cannot freeze auditor verification runtime: {error}",
                file=sys.stderr,
            )
            return 2

    case_ids = [case["id"] for case in defect_cases]
    fixture_snapshot = tempfile.TemporaryDirectory(
        prefix="mastermind-ablation-fixtures-"
    )
    target_snapshot: tempfile.TemporaryDirectory[str] | None = None
    fixture_snapshot_root = Path(fixture_snapshot.name)
    suite_cfg_for_run = runner.SUITES["auditor"]
    frozen_target_digest: str | None = None
    try:
        live_case_digest = runner.case_definition_digest("auditor", case_ids)
        loaded_case_digest = runner.case_definition_digest_from_records(
            "auditor", defect_cases
        )
        runner.snapshot_fixture_definitions(
            defect_cases, fixture_snapshot_root
        )
        frozen_case_digest = runner.case_definition_digest_from_records(
            "auditor",
            defect_cases,
            fixtures_dir=fixture_snapshot_root,
        )
        live_case_digest_after_snapshot = runner.case_definition_digest(
            "auditor", case_ids
        )
        if not (
            live_case_digest
            == loaded_case_digest
            == frozen_case_digest
            == live_case_digest_after_snapshot
        ):
            raise ValueError("auditor cases changed while being frozen")

        if args.with_mastermind:
            target_snapshot = tempfile.TemporaryDirectory(
                prefix="mastermind-ablation-target-"
            )
            live_target_digest = runner.evaluation_target_digest(
                "auditor", runner.SUITES["auditor"], defect_cases
            )
            suite_cfg_for_run, _ = runner.snapshot_evaluation_targets(
                "auditor",
                runner.SUITES["auditor"],
                defect_cases,
                Path(target_snapshot.name),
            )
            frozen_target_digest = runner.evaluation_target_digest(
                "auditor", suite_cfg_for_run, defect_cases
            )
            live_target_digest_after_snapshot = runner.evaluation_target_digest(
                "auditor", runner.SUITES["auditor"], defect_cases
            )
            if not (
                live_target_digest
                == frozen_target_digest
                == live_target_digest_after_snapshot
            ):
                raise ValueError("auditor target changed while being frozen")
    except (KeyError, OSError, RuntimeError, ValueError) as error:
        if target_snapshot is not None:
            target_snapshot.cleanup()
        fixture_snapshot.cleanup()
        print(f"error: cannot freeze ablation inputs: {error}", file=sys.stderr)
        return 2

    print(
        f"\n=== diagnostic comparison over {len(defect_cases)} "
        f"defect case(s) · {args.model} ===\n"
    )
    rows: list[
        tuple[str, ConditionOutcome | None, ConditionOutcome | None]
    ] = []
    expected_models: tuple[str, ...] | None = None
    model_issues: list[str] = []
    runtime_stable = False
    definition_stable = False
    target_stable = not args.with_mastermind
    source_stable = False
    repository_stable = False
    verification_runtime_stable = frozen_verification_runtime is None
    try:
        for case in defect_cases:
            case_id = case["id"]
            print(f"  [{case_id}] vanilla ...", end=" ", flush=True)
            vanilla = run_vanilla(
                args.model,
                case,
                claude_binary=claude_binary,
                claude_version=claude_version,
                git_binary=git_binary,
                fixtures_dir=fixture_snapshot_root,
            )
            vanilla_label = (
                "err"
                if vanilla is None
                else "phrase pass" if vanilla.passed else "phrase miss"
            )
            print(vanilla_label, end="", flush=True)
            if vanilla is not None:
                expected_models, issue = merge_model_identity(
                    expected_models, vanilla, f"{case_id} vanilla"
                )
                if issue is not None:
                    model_issues.append(issue)

            mastermind = None
            if args.with_mastermind:
                result = runner.evaluate_case(
                    args.model,
                    "auditor",
                    suite_cfg_for_run,
                    case,
                    keep_fixtures=False,
                    fixtures_dir=fixture_snapshot_root,
                    claude_binary=claude_binary,
                    claude_version=claude_version,
                    git_binary=git_binary,
                    mmcg_binary=mmcg_binary,
                    cargo_binary=cargo_binary,
                    rustc_binary=rustc_binary,
                )
                mastermind = _mastermind_outcome(result)
                mastermind_label = (
                    "err"
                    if mastermind is None
                    else "pass" if mastermind.passed else "fail"
                )
                print(
                    f"  · mastermind contract {mastermind_label}", end=""
                )
                if mastermind is not None:
                    expected_models, issue = merge_model_identity(
                        expected_models,
                        mastermind,
                        f"{case_id} mastermind",
                    )
                    if issue is not None:
                        model_issues.append(issue)
            print()
            rows.append((case_id, vanilla, mastermind))

        vanilla_caught = sum(
            1
            for _, outcome, _ in rows
            if outcome is not None and outcome.passed
        )
        print(
            f"\n  vanilla phrase checks passed: {vanilla_caught}/{len(rows)}"
        )
        if args.with_mastermind:
            mastermind_caught = sum(
                1
                for _, _, outcome in rows
                if outcome is not None and outcome.passed
            )
            print(
                "  mastermind full contract passed: "
                f"{mastermind_caught}/{len(rows)}"
            )
            print("  Different grading contracts: no quality-uplift estimate.")
        else:
            print(
                "  mastermind was not run; use --with-mastermind for its "
                "full contract result."
            )
        errors = sum(
            vanilla is None
            or (args.with_mastermind and mastermind is None)
            for _, vanilla, mastermind in rows
        )
        if errors:
            print(f"  infrastructure errors: {errors}", file=sys.stderr)
        if expected_models is not None:
            print(f"  resolved model ids: {list(expected_models)!r}")
        for issue in model_issues:
            print(f"  model identity error: {issue}", file=sys.stderr)

        try:
            runtime_stable = (
                runner._stable_regular_file_definition(claude_binary)
                == claude_definition
                and runner.claude_cli_version(claude_binary) == claude_version
                and runner.fixture_runtime_definition(
                    git_binary,
                    mmcg_binary if args.with_mastermind else None,
                )
                == fixture_runtime
            )
        except (OSError, RuntimeError, ValueError):
            runtime_stable = False
        if frozen_verification_runtime is not None:
            try:
                if cargo_binary is None or rustc_binary is None:
                    raise ValueError("pinned Cargo/Rust runtime unavailable")
                runner.evaluation_environment(
                    1,
                    source={},
                    pinned_executables=(
                        git_binary,
                        cargo_binary,
                        rustc_binary,
                    ),
                )
                verification_runtime_stable = (
                    runner.verification_runtime_definition(
                        cargo_binary, rustc_binary
                    )
                    == frozen_verification_runtime
                )
            except (OSError, RuntimeError, ValueError):
                verification_runtime_stable = False
        try:
            definition_stable = (
                runner.case_definition_digest("auditor", case_ids)
                == runner.case_definition_digest_from_records(
                    "auditor",
                    defect_cases,
                    fixtures_dir=fixture_snapshot_root,
                )
                == frozen_case_digest
            )
        except (KeyError, OSError, ValueError):
            definition_stable = False
        if args.with_mastermind:
            try:
                target_stable = (
                    runner.evaluation_target_digest(
                        "auditor", runner.SUITES["auditor"], defect_cases
                    )
                    == runner.evaluation_target_digest(
                        "auditor", suite_cfg_for_run, defect_cases
                    )
                    == frozen_target_digest
                )
            except (KeyError, OSError, ValueError):
                target_stable = False
        try:
            source_stable = (
                runner._stable_regular_file_definition(Path(__file__).resolve())
                == ablation_definition
                and runner.evaluation_harness_definition()
                == harness_definition
            )
        except (OSError, ValueError):
            source_stable = False
        repository_stable = (
            runner.git_revision(git_binary) == repository_revision
        )
    finally:
        if target_snapshot is not None:
            target_snapshot.cleanup()
        fixture_snapshot.cleanup()

    if not runtime_stable:
        print("error: ablation runtime changed during evaluation", file=sys.stderr)
    if not verification_runtime_stable:
        print(
            "error: auditor verification runtime changed during evaluation",
            file=sys.stderr,
        )
    if not definition_stable:
        print("error: ablation case inputs changed during evaluation", file=sys.stderr)
    if not target_stable:
        print("error: auditor target changed during evaluation", file=sys.stderr)
    if not source_stable:
        print("error: ablation harness changed during evaluation", file=sys.stderr)
    if not repository_stable:
        print("error: repository HEAD changed during evaluation", file=sys.stderr)
    return 1 if (
        errors
        or model_issues
        or not runtime_stable
        or not verification_runtime_stable
        or not definition_stable
        or not target_stable
        or not source_stable
        or not repository_stable
    ) else 0


if __name__ == "__main__":
    sys.exit(main())
