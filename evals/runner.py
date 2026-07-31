#!/usr/bin/env python3
"""
Local eval runner for mastermind subagents.

Uses the `claude` CLI in non-interactive mode (`-p`), so authentication runs
through your existing Claude Code login — no ANTHROPIC_API_KEY needed, costs
count against your Claude subscription (flat monthly, not per-token).

Auditor cases use **real git fixtures** — each case names a fixture under
`evals/fixtures/<name>/` plus two refs (`baseline_ref` + `after_ref`). The
runner builds a real tmp git repo with those two commits/tags and hands the
path to the auditor via `--add-dir`. The auditor runs `git diff`, `git log`,
etc. itself against actual hunks. No synthetic paraphrased diff strings.

Usage:
  python evals/runner.py                  # all suites
  python evals/runner.py --suite critic   # one suite
  python evals/runner.py --case c-001     # one case
  python evals/runner.py --model opus     # model alias or full name (default: opus)
  python evals/runner.py --keep-fixtures  # don't tmp-cleanup (debugging)

Prerequisites:
  - `claude` CLI on PATH (Claude Code installed, logged in)
  - `git` on PATH
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

try:
    import yaml as _yaml
    _YAML_AVAILABLE = True
except ImportError:
    _YAML_AVAILABLE = False

REPO_ROOT = Path(__file__).resolve().parent.parent
EVALS_DIR = Path(__file__).resolve().parent
FIXTURES_DIR = EVALS_DIR / "fixtures"

# Prefer the in-tree build (matches the current SCHEMA_VERSION) over whatever
# version is installed in ~/.cargo/bin — avoids "schema mismatch — rebuilding"
# noise during eval and ensures the indexer + server speak the same SQL.
_LOCAL_MMCG = REPO_ROOT / "mcp" / "servers" / "mmcg" / "target" / "release" / "mmcg"
MMCG_BIN = str(_LOCAL_MMCG) if _LOCAL_MMCG.is_file() else "mmcg"

SUITES = {
    "critic": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-critic.md",
        "cases": EVALS_DIR / "critic.jsonl",
        "renderer": "render_critic_input",
        "uses_fixture": False,
    },
    "auditor": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-auditor.md",
        "cases": EVALS_DIR / "auditor.jsonl",
        "renderer": "render_auditor_input",
        "uses_fixture": True,
    },
    "intake": {
        "subagent": REPO_ROOT / "agents/subagents/mastermind-prompt-refiner.md",
        "cases": EVALS_DIR / "intake.jsonl",
        "renderer": "render_intake_input",
        "uses_fixture": False,
    },
    "workflow": {
        "subagent": None,
        "cases": EVALS_DIR / "workflow.jsonl",
        "renderer": "render_workflow_input",
        "uses_fixture": False,
    },
}

WORKFLOW_ARTIFACTS = frozenset(
    {
        "skills/code-review/mastermind-comment-audit/SKILL.md",
        "skills/code-review/mastermind-frontend-audit/SKILL.md",
        "skills/code-review/mastermind-test-audit/SKILL.md",
        "skills/coding/no-ai-slop-comments/SKILL.md",
        "skills/design/mastermind-design-intake/SKILL.md",
        "skills/testing/mastermind-browser-verification/SKILL.md",
        "skills/debugging/mastermind-investigation-ledger/SKILL.md",
        "skills/prompt-engineering/mastermind-prompt-refiner/SKILL.md",
        "skills/security/mastermind-agent-security-review/SKILL.md",
        "skills/security/mastermind-security-research/SKILL.md",
        "skills/workflow/mastermind-audit-attestation/SKILL.md",
        "skills/workflow/mastermind-architecture-review/SKILL.md",
        "skills/workflow/mastermind-change-impact/SKILL.md",
        "skills/workflow/mastermind-codegraph-research/SKILL.md",
        "skills/workflow/mastermind-component-research/SKILL.md",
        "skills/workflow/mastermind-critical-review/SKILL.md",
        "skills/workflow/mastermind-cross-client-setup/SKILL.md",
        "skills/workflow/mastermind-project-history/SKILL.md",
        "skills/workflow/mastermind-product-intake/SKILL.md",
        "skills/workflow/mastermind-project-map/SKILL.md",
        "skills/workflow/mastermind-runtime-research/SKILL.md",
        "skills/workflow/mastermind-structured-report-contract/SKILL.md",
        "skills/workflow/mastermind-style-deep/SKILL.md",
        "skills/workflow/mastermind-task-executor/SKILL.md",
        "skills/workflow/mastermind-task-planning/SKILL.md",
        "skills/workflow/mastermind-test-impact/SKILL.md",
    }
)

# Deterministic identity for fixture commits — avoids machine-specific git
# config noise in eval reproducibility.
GIT_ENV = {
    "GIT_AUTHOR_NAME": "mmcg-eval",
    "GIT_AUTHOR_EMAIL": "eval@mastermind.local",
    "GIT_COMMITTER_NAME": "mmcg-eval",
    "GIT_COMMITTER_EMAIL": "eval@mastermind.local",
    # Avoid GPG signing inside fixtures (some dev machines force it globally).
    "GIT_CONFIG_COUNT": "1",
    "GIT_CONFIG_KEY_0": "commit.gpgsign",
    "GIT_CONFIG_VALUE_0": "false",
}

# Base env for all subprocesses spawned by the runner.
# TERM=dumb prevents tput from emitting warnings in non-interactive shells
# (CI, piped output, shells where $TERM is unset). Prefer the caller's TERM
# when available so color hints still work in interactive mode.
_PROC_ENV: dict[str, str] = {**os.environ, "TERM": os.environ.get("TERM") or "dumb"}


@dataclass
class Result:
    case_id: str
    suite: str
    passed: bool
    reasons: list[str] = field(default_factory=list)
    duration_ms: int = 0
    fixture_path: Path | None = None
    retry_used: bool = False
    retry_attempted: bool = False
    output_excerpt: str = ""


def strip_frontmatter(text: str) -> str:
    if text.startswith("---\n"):
        end = text.find("\n---\n", 4)
        if end != -1:
            return text[end + 5 :].lstrip()
    return text


# ----- fixture lifecycle ----------------------------------------------------


def _run_git(args: list[str], cwd: Path) -> None:
    """Run git with our scrubbed environment. Raises on non-zero exit."""
    proc = subprocess.run(
        ["git", *args],
        cwd=cwd,
        env={**_PROC_ENV, **GIT_ENV},
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"git {' '.join(args)} failed in {cwd}:\n"
            f"  stdout: {proc.stdout.strip()}\n  stderr: {proc.stderr.strip()}"
        )


def setup_fixture(fixture_name: str, baseline_ref: str, after_ref: str) -> Path:
    """Build a real tmp git repo with baseline → after as two tagged commits.

    Layout expected at `evals/fixtures/<name>/`:
        baseline/             — files at the baseline tag
        changes/<after_ref>/  — files at the after tag (full tree, replaces baseline)

    Returns the tmp repo path. Caller is responsible for cleanup via
    `teardown_fixture`.
    """
    src = FIXTURES_DIR / fixture_name
    if not src.is_dir():
        raise FileNotFoundError(f"fixture not found: {src}")
    baseline_src = src / "baseline"
    after_src = src / "changes" / after_ref
    if not baseline_src.is_dir():
        raise FileNotFoundError(f"fixture baseline missing: {baseline_src}")
    if not after_src.is_dir():
        raise FileNotFoundError(f"fixture variant missing: {after_src}")

    tmp = Path(tempfile.mkdtemp(prefix=f"mmcg-eval-{fixture_name}-"))

    # Phase 1: baseline tree.
    _copy_tree_into(baseline_src, tmp)
    _run_git(["init", "-q", "--initial-branch=main"], tmp)
    _run_git(["add", "-A"], tmp)
    _run_git(["commit", "-q", "-m", "baseline"], tmp)
    _run_git(["tag", baseline_ref], tmp)

    # Phase 2: replace working tree with `after` variant content.
    # We wipe everything except `.git/` then re-overlay so deletions are
    # reflected too (e.g. a variant that removes a file present in baseline).
    for entry in tmp.iterdir():
        if entry.name == ".git":
            continue
        if entry.is_dir():
            shutil.rmtree(entry)
        else:
            entry.unlink()
    _copy_tree_into(after_src, tmp)
    _run_git(["add", "-A"], tmp)
    _run_git(["commit", "-q", "-m", f"executor change ({after_ref})", "--allow-empty"], tmp)
    _run_git(["tag", after_ref], tmp)

    # Phase 3: build an mmcg index of the after-tree so the auditor can run
    # real `mmcg_callers` / `mmcg_search` against the working state and
    # compare against the spec's pre-edit snapshot. Failure here is non-fatal
    # — the auditor can still operate on `git diff` alone.
    try:
        _build_mmcg_index(tmp)
    except (RuntimeError, FileNotFoundError) as e:
        sys.stderr.write(f"  [fixture] mmcg index skipped: {e}\n")

    return tmp


def _build_mmcg_index(repo: Path) -> None:
    """Run `mmcg index .` in `repo`, leaving `.mastermind/mmcg.db` behind.

    Uses the in-tree binary when available so the SQL schema matches the MCP
    server's expectations. Quiet on success — index summary is suppressed.
    """
    db_path = repo / ".mastermind" / "mmcg.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        [MMCG_BIN, "--index", str(db_path), "index", str(repo)],
        cwd=repo,
        env=_PROC_ENV,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"mmcg index failed: {proc.stderr.strip() or proc.stdout.strip()}"
        )


def _copy_tree_into(src: Path, dst: Path) -> None:
    """Recursive copy src/* into dst/, creating dst if needed."""
    dst.mkdir(parents=True, exist_ok=True)
    for entry in src.iterdir():
        target = dst / entry.name
        if entry.is_dir():
            shutil.copytree(entry, target, dirs_exist_ok=True)
        else:
            shutil.copy2(entry, target)


def teardown_fixture(path: Path) -> None:
    shutil.rmtree(path, ignore_errors=True)


# ----- renderers ------------------------------------------------------------


def render_intake_input(inp: dict) -> str:
    target = inp.get("target_consumer", "planner")
    ctx = inp.get("project_context", "")
    ctx_block = f"\n\n**Project context:** {ctx}" if ctx else ""
    return (
        f"**Target consumer:** {target}{ctx_block}\n\n"
        f"**Raw prompt to refine:**\n\n{inp.get('raw_prompt', '')}"
    )


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


def render_workflow_input(inp: dict) -> str:
    return str(inp.get("prompt", ""))


def workflow_prompt_path(case: dict) -> Path:
    artifact = case.get("artifact")
    if artifact not in WORKFLOW_ARTIFACTS:
        raise ValueError(f"workflow artifact is not allowlisted: {artifact!r}")
    path = (REPO_ROOT / artifact).resolve()
    try:
        path.relative_to(REPO_ROOT.resolve())
    except ValueError as error:
        raise ValueError("workflow artifact must stay inside the repository") from error
    if not path.is_file():
        raise FileNotFoundError(f"workflow artifact not found: {path}")
    return path


def render_auditor_input(
    inp: dict,
    *,
    fixture_path: Path,
    baseline_ref: str,
    after_ref: str,
    has_mmcg: bool,
) -> str:
    """Build the auditor's user message.

    Crucially: NO synthetic git_diff. The auditor is told the working
    directory and the two tag names and is expected to run `git diff`,
    `git log`, `git show --stat` etc. itself via Bash. When `has_mmcg` is
    true, the auditor also has live `mmcg_callers` / `mmcg_search` MCP
    tools pointed at an index of the after-tree state.
    """
    mmcg_note = (
        "\n\n**mmcg available:** the working dir has a fresh `.mastermind/mmcg.db` "
        "indexed at the after-commit state. You can call `mmcg_callers`, "
        "`mmcg_search`, `mmcg_outline` etc. via the MCP tools — use them to "
        "verify the spec's pre-edit symbol snapshot against the current state.\n"
    ) if has_mmcg else "\n"
    return (
        f"Audit the executor's work against the spec.\n\n"
        f"**Working directory:** `{fixture_path}` — a real git repo with two commits.\n"
        f"**Baseline tag:** `{baseline_ref}` (state before the executor ran)\n"
        f"**Executor commit tag:** `{after_ref}` (state after the executor ran)\n\n"
        f"Use real `git diff {baseline_ref}..{after_ref}`, `git log`, "
        f"`git show --stat` against this repo. Do NOT trust the executor's "
        f"narrative — verify each claim against the diff."
        f"{mmcg_note}\n"
        f"**Spec summary:**\n{inp.get('spec_summary', '')}\n\n"
        f"**Executor report (what they claim they did):**\n```\n{inp.get('executor_report', '')}\n```"
    )


# ----- verdict extraction ---------------------------------------------------

_AUDIT_BLOCK_RE = re.compile(
    r"<!--\s*mastermind:audit-begin\s*-->.*?```ya?ml(.*?)```.*?<!--\s*mastermind:audit-end\s*-->",
    re.S,
)

_INTAKE_BLOCK_RE = re.compile(
    r"<!--\s*mastermind:intake-begin\s*-->.*?```ya?ml(.*?)```.*?<!--\s*mastermind:intake-end\s*-->",
    re.S,
)


def extract_audit_verdict(output: str) -> str | None:
    """Return the `verdict` field from a structured audit tail, or None.

    Requires valid YAML inside the sentinel block — no regex fallback.
    A malformed block is treated the same as an absent block (None),
    so the caller will fail the case rather than guess.
    """
    m = _AUDIT_BLOCK_RE.search(output)
    if not m:
        return None
    if not _YAML_AVAILABLE:
        return None
    try:
        data = _yaml.safe_load(m.group(1))
        if isinstance(data, dict) and data.get("verdict"):
            return str(data["verdict"]).lower().strip()
    except Exception:
        pass
    return None


def extract_intake_action(output: str) -> str | None:
    """Return the `action` field from a structured intake metadata block, or None."""
    m = _INTAKE_BLOCK_RE.search(output)
    if not m:
        return None
    if not _YAML_AVAILABLE:
        return None
    try:
        data = _yaml.safe_load(m.group(1))
        if isinstance(data, dict) and data.get("action"):
            return str(data["action"]).lower().strip()
    except Exception:
        pass
    return None


def contains_any_phrase(output: str, phrases: list[str]) -> bool:
    """Accept one of several semantically equivalent case-insensitive phrases."""
    lowered = output.lower()
    return bool(phrases) and any(phrase.lower() in lowered for phrase in phrases)


_FENCED_CODE_RE = re.compile(r"```[^\n]*\n(.*?)```", re.DOTALL)


def _comment_start(line: str, prefixes: list[str]) -> int | None:
    """Find a comment marker outside simple quoted strings."""
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"', "`"}:
            quote = char
            continue
        if any(line.startswith(prefix, index) for prefix in prefixes):
            return index
    return None


def extract_code_comments(output: str, prefixes: list[str]) -> tuple[list[str], bool]:
    """Return comment fragments from fenced code and whether code was fenced."""
    blocks = _FENCED_CODE_RE.findall(output)
    if not blocks:
        return [], False
    comments: list[str] = []
    for block in blocks:
        for line in block.splitlines():
            position = _comment_start(line, prefixes)
            if position is not None:
                comments.append(line[position:].strip())
    return comments, True


def code_comment_policy_reasons(output: str, policy: dict) -> list[str]:
    """Deterministically score comments in model-generated fenced code."""
    prefixes = policy.get("prefixes", ["//", "/*"])
    comments, fenced = extract_code_comments(output, prefixes)
    reasons: list[str] = []
    if policy.get("require_fenced_code", True) and not fenced:
        reasons.append("comment policy requires a fenced code block")
        return reasons
    minimum = policy.get("min", 0)
    maximum = policy.get("max")
    if len(comments) < minimum:
        reasons.append(f"found {len(comments)} code comment(s), expected at least {minimum}")
    if maximum is not None and len(comments) > maximum:
        reasons.append(
            f"found {len(comments)} code comment(s), expected at most {maximum}: {comments!r}"
        )
    comment_text = "\n".join(comments)
    for alternatives in policy.get("contains_any", []):
        if not contains_any_phrase(comment_text, alternatives):
            reasons.append(f"comments lack required alternatives: {alternatives!r}")
    for phrase in policy.get("not_contains", []):
        if phrase.lower() in comment_text.lower():
            reasons.append(f"forbidden comment phrase present: {phrase!r}")
    return reasons


# ----- per-case evaluator ---------------------------------------------------


def evaluate_case(
    model: str,
    suite_name: str,
    suite_cfg: dict,
    case: dict,
    *,
    keep_fixtures: bool,
) -> Result:
    case_id = case["id"]
    prompt_path = (
        workflow_prompt_path(case)
        if suite_name == "workflow"
        else suite_cfg["subagent"]
    )
    system_prompt = strip_frontmatter(prompt_path.read_text())

    fixture_path: Path | None = None
    extra_cmd: list[str] = []
    try:
        if suite_cfg["uses_fixture"]:
            fixture_name = case["fixture"]
            baseline_ref = case["baseline_ref"]
            after_ref = case["after_ref"]
            fixture_path = setup_fixture(fixture_name, baseline_ref, after_ref)

            db_path = fixture_path / ".mastermind" / "mmcg.db"
            has_mmcg = db_path.is_file()
            if suite_name == "auditor" and not has_mmcg and not case.get("allow_no_mmcg"):
                return Result(
                    case_id, suite_name, False,
                    ["mmcg index unavailable — build the mmcg binary first (`cargo build --release`)"],
                    0, fixture_path,
                )
            user_message = render_auditor_input(
                case["input"],
                fixture_path=fixture_path,
                baseline_ref=baseline_ref,
                after_ref=after_ref,
                has_mmcg=has_mmcg,
            )
            extra_cmd = ["--add-dir", str(fixture_path)]
            if has_mmcg:
                # Spawn an mmcg MCP stdio server pointed at the fixture's index.
                # This matches the production wiring — auditor calls mmcg_callers
                # via MCP, not via CLI subprocess.
                mcp_cfg = json.dumps({
                    "mcpServers": {
                        "mmcg": {
                            "command": MMCG_BIN,
                            "args": ["--index", str(db_path), "serve"],
                        }
                    }
                })
                extra_cmd += ["--mcp-config", mcp_cfg]
        elif suite_name == "intake":
            user_message = render_intake_input(case["input"])
        elif suite_name == "workflow":
            user_message = render_workflow_input(case["input"])
        else:
            user_message = render_critic_input(case["input"])

        # Pass the user message via stdin — passing it as a positional arg
        # collides with `--add-dir <directories...>` (variadic), which would
        # swallow the message as another directory.
        prompt_flag = "--system-prompt" if suite_name == "workflow" else "--append-system-prompt"
        workflow_safety = (
            ["--safe-mode", "--tools", ""] if suite_name == "workflow" else []
        )
        cmd = [
            "claude",
            "-p",
            "--model", model,
            prompt_flag, system_prompt,
            "--output-format", "json",
            "--no-session-persistence",
            "--permission-mode", "dontAsk",
            *workflow_safety,
            *extra_cmd,
        ]

        # Auditor with real-git fixtures runs many Bash + Read tool calls per
        # case; observed range is ~120–280s on Sonnet, can blow past 300s. Set
        # generously at 480s to absorb tail-latency variance.
        try:
            proc = subprocess.run(
                cmd, input=user_message, capture_output=True, text=True,
                env=_PROC_ENV,
                cwd=tempfile.gettempdir() if suite_name == "workflow" else None,
                timeout=480,
            )
        except subprocess.TimeoutExpired:
            return Result(
                case_id, suite_name, False, ["timeout after 480s"], 0, fixture_path
            )

        if proc.returncode != 0:
            err = (proc.stderr or proc.stdout or "").strip()[:300]
            return Result(
                case_id, suite_name, False,
                [f"claude exit {proc.returncode}: {err}"], 0, fixture_path,
            )

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

        expected_action = expect.get("action")
        if expected_action and suite_name == "intake":
            structured = extract_intake_action(output)
            if structured is not None:
                if structured != expected_action.lower():
                    passed = False
                    reasons.append(
                        f"intake action {structured!r} != expected {expected_action!r}"
                    )
            else:
                passed = False
                reasons.append(
                    "no structured intake metadata block found "
                    "(<!-- mastermind:intake-begin --> ... <!-- mastermind:intake-end -->)"
                )

        expected_verdict = expect.get("verdict")
        if expected_verdict:
            candidates = (
                [expected_verdict]
                if isinstance(expected_verdict, str)
                else list(expected_verdict)
            )
            if suite_name == "auditor":
                structured = extract_audit_verdict(output)
                if structured is not None:
                    if not any(v.lower() == structured for v in candidates):
                        passed = False
                        reasons.append(
                            f"structured verdict {structured!r} not in expected {candidates}"
                        )
                else:
                    passed = False
                    reasons.append(
                        "no structured audit verdict block found "
                        "(<!-- mastermind:audit-begin --> ... <!-- mastermind:audit-end -->)"
                    )
            elif not any(
                re.search(rf"\b{re.escape(v)}\b", output, re.IGNORECASE)
                for v in candidates
            ):
                passed = False
                reasons.append(f"none of expected verdicts {candidates} found")

        for phrase in expect.get("contains", []):
            if phrase.lower() not in output.lower():
                passed = False
                reasons.append(f"missing phrase: {phrase!r}")

        for alternatives in expect.get("contains_any", []):
            if not isinstance(alternatives, list) or not all(
                isinstance(phrase, str) for phrase in alternatives
            ):
                passed = False
                reasons.append(f"invalid contains_any group: {alternatives!r}")
            elif not contains_any_phrase(output, alternatives):
                passed = False
                reasons.append(f"none of alternative phrases found: {alternatives!r}")

        for phrase in expect.get("not_contains", []):
            if phrase.lower() in output.lower():
                passed = False
                reasons.append(f"forbidden phrase present: {phrase!r}")

        comment_policy = expect.get("code_comments")
        if comment_policy is not None:
            comment_reasons = code_comment_policy_reasons(output, comment_policy)
            if comment_reasons:
                passed = False
                reasons.extend(comment_reasons)

        return Result(
            case_id,
            suite_name,
            passed,
            reasons,
            duration_ms,
            fixture_path,
            output_excerpt=output[:4000],
        )
    finally:
        if fixture_path is not None and not keep_fixtures:
            teardown_fixture(fixture_path)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--suite", choices=list(SUITES.keys()), help="run one suite only")
    parser.add_argument("--case", help="run one case by id")
    parser.add_argument(
        "--model", default="opus",
        help="model alias (opus/sonnet/haiku) or full name; default opus",
    )
    parser.add_argument(
        "--keep-fixtures", action="store_true",
        help="don't delete fixture tmp dirs after each case (debugging)",
    )
    parser.add_argument(
        "--verbose-failures", action="store_true",
        help="print up to 4000 characters of model output for failed cases",
    )
    args = parser.parse_args()

    if not shutil.which("claude"):
        print(
            "error: `claude` CLI not on PATH. Install Claude Code: https://claude.com/claude-code",
            file=sys.stderr,
        )
        return 2
    if not shutil.which("git"):
        print("error: `git` not on PATH (required for fixture suites).", file=sys.stderr)
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
                r = evaluate_case(
                    args.model, suite_name, suite_cfg, case,
                    keep_fixtures=args.keep_fixtures,
                )
                _SENTINEL_MISSING = "no structured audit verdict block found"
                if (
                    not r.passed
                    and suite_name == "auditor"
                    and len(r.reasons) == 1
                    and _SENTINEL_MISSING in r.reasons[0]
                ):
                    print(f"retry (sentinel missing) ...", end=" ", flush=True)
                    first_duration_ms = r.duration_ms
                    r2 = evaluate_case(
                        args.model, suite_name, suite_cfg, case,
                        keep_fixtures=args.keep_fixtures,
                    )
                    r2.retry_attempted = True
                    r2.duration_ms += first_duration_ms
                    if r2.passed:
                        r2.retry_used = True
                    r = r2
                results.append(r)
                status = "✓ pass" if r.passed else "✗ FAIL"
                retry_tag = " [retry]" if r.retry_used else ""
                print(f"{status}{retry_tag}  ({r.duration_ms}ms)")
                if args.keep_fixtures and r.fixture_path is not None:
                    print(f"      fixture: {r.fixture_path}")
                for reason in r.reasons:
                    print(f"      → {reason}")
                if args.verbose_failures and not r.passed and r.output_excerpt:
                    print("      --- model output ---")
                    for line in r.output_excerpt.splitlines():
                        print(f"      {line}")

    if not results:
        print("\nno cases matched filter")
        return 0

    n_pass = sum(r.passed for r in results)
    n_fail = len(results) - n_pass
    n_first_pass = sum(r.passed and not r.retry_used for r in results)
    n_retry_attempted = sum(r.retry_attempted for r in results)
    n_retry_pass = sum(r.passed and r.retry_used for r in results)
    total_ms = sum(r.duration_ms for r in results)
    print(f"\n=== summary ===")
    print(f"  passed: {n_pass}/{len(results)}")
    print(f"  first_pass: {n_first_pass}/{len(results)}")
    if n_retry_attempted:
        print(f"  retry_attempted: {n_retry_attempted}/{len(results)}")
    if n_retry_pass:
        print(f"  after_retry: {n_first_pass + n_retry_pass}/{len(results)} ({n_retry_pass} case(s) passed on retry)")
    print(f"  total time: {total_ms / 1000:.1f}s")
    return 0 if n_fail == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
