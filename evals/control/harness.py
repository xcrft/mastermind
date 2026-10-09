"""Check a bounded completion model and sampled CLI and UI boundaries.

Run with ``python3 -m evals.control --output <new-directory>``.
This is a local regression report, not a proof of implementation refinement.
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass, replace
from pathlib import Path
import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time

from evals.shared.process import run_bounded


ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "mcp/servers/mmcg"
OBLIGATIONS = ("authorized", "executed", "verified", "audited", "reviewed", "history")


@dataclass(frozen=True)
class State:
    authorized: bool = False
    executed: bool = False
    verified: bool = False
    audited: bool = False
    reviewed: bool = False
    history: bool = False
    completed: bool = False


def successors(state: State, omitted_guard: str | None = None):
    """Abstract persisted evidence plus current validity of each obligation.

    Invalidations may leave other records intact. Publication must recheck
    every obligation even when earlier gates once passed.
    """
    if state.completed:
        # Completion is a historical observation. A new preflight starts a
        # different iteration; later source changes do not rewrite the old one.
        yield "new_preflight", State(authorized=True)
        return
    yield "preflight", State(authorized=True)
    if state.authorized:
        yield "executor_succeeded", replace(state, executed=True)
        yield "checks_passed", replace(state, verified=True)
    if state.authorized and state.executed and state.verified:
        yield "audit_held", replace(state, audited=True)
    if state.audited:
        yield "review_satisfied", replace(state, reviewed=True)
        yield "history_resolved", replace(state, history=True)
    for action, field in (
        ("contract_changed", "authorized"),
        ("invocation_invalidated", "executed"),
        ("check_failed_or_inputs_changed", "verified"),
        ("audit_invalidated", "audited"),
        ("review_negative_unknown_or_publication_failed", "reviewed"),
        ("canonical_history_changed_or_update_required", "history"),
    ):
        yield action, replace(state, **{field: False})
    if all(getattr(state, field) for field in OBLIGATIONS if field != omitted_guard):
        yield "publish", replace(state, completed=True)


def publication_valid(before: State) -> bool:
    # Independent oracle: the complete contract, never the selected guard set.
    return (before.authorized and before.executed and before.verified
            and before.audited and before.reviewed and before.history)


def explore(omitted_guard: str | None = None) -> dict:
    initial = State()
    paths = {initial: ()}
    queue = deque([initial])
    edges = publications = 0
    violations = []
    while queue:
        before = queue.popleft()
        for action, after in successors(before, omitted_guard):
            edges += 1
            trace = (*paths[before], action)
            if action == "publish":
                publications += 1
                if not publication_valid(before):
                    violations.append({"trace": list(trace), "before": vars(before)})
            if after not in paths:
                paths[after] = trace
                queue.append(after)
    # Existential recovery under a stable environment and successful producers.
    # This does not assume that an arbitrary real task can be fixed or approved.
    recovery = ("preflight", "executor_succeeded", "checks_passed", "audit_held",
                "review_satisfied", "history_resolved", "publish")
    recoverable = 0
    for state in paths:
        if state.completed:
            continue
        current = state
        for action in recovery:
            candidates = dict(successors(current))
            if action not in candidates:
                break
            current = candidates[action]
        recoverable += int(current.completed)
    return {
        "states": len(paths), "transitions": edges, "publication_edges": publications,
        "violations": violations, "open_states": sum(not s.completed for s in paths),
        "recoverable_open_states": recoverable, "recovery_steps_bound": len(recovery),
    }


def model_report() -> dict:
    baseline = explore()
    mutants = []
    for field in OBLIGATIONS:
        result = explore(field)
        mutants.append({"omitted_guard": field, "detected": bool(result["violations"]),
                        "counterexample": next(iter(result["violations"]), None)})
    passed = (baseline["publication_edges"] > 0 and not baseline["violations"]
              and baseline["open_states"] == baseline["recoverable_open_states"]
              and all(m["detected"] for m in mutants))
    return {"status": "passed" if passed else "failed", **baseline, "mutants": mutants}


# Exact selectors are part of the evidence contract. Missing/ignored tests fail.
CASES = {
    "profile_budget_cli": (
        "dry_run_changes_only_the_profile_budget_key",
        "out_of_range_profile_budget_values_are_rejected",
        "the_default_profile_budget_is_not_persisted_and_a_set_value_survives_reinit",
        "doctor_reports_profile_budget_as_not_configured_by_default",
    ),
    "onboarding_cli": (
        "dry_run_and_unconfigured_status_leave_project_and_home_unchanged",
        "local_init_indexes_source_and_context_and_preserves_existing_documents",
        "explicit_capture_registers_audience_without_claiming_activation_or_profile_access",
        "partial_setup_retries_saved_client_selection_after_client_installation",
        "repeated_init_preserves_a_stopped_worker_budget_and_explicit_start_renews_it",
        "failed_client_removal_stays_pending_and_reader_access_is_revoked_independently",
        "status_reads_registrations_without_executing_a_native_client_or_server",
        "failed_file_commits_do_not_report_a_successful_index_build",
    ),
    "acceptance_cli": (
        "unix::every_check_is_required_and_executor_pass_cannot_supply_missing_proof",
    ),
    "verification_receipts_cli": (
        "unix::observed_failure_overrules_reported_pass_and_revokes_a_previous_success",
        "unix::late_external_executable_change_cannot_close_legacy_repeated_postflight",
        "unix::a_receipt_from_another_repository_cannot_satisfy_the_same_task_name",
    ),
    "task_review_cli": (
        "prepare_is_read_only_and_unknown_until_typed_positive_review_closes_across_processes",
        "unknown_or_negative_judgments_are_stored_and_block_completion_despite_green_checks",
        "compare_and_swap_rejects_a_queued_positive_after_a_newer_negative",
        "accepted_semantics_with_unknown_or_required_history_updates_cannot_close_via_markdown",
        "repeated_postflight_preserves_current_review_and_keeps_ignored_lesson_changes_reviewable",
        "follow_up_returns_exact_bound_history_work_and_conditional_steps_without_side_effects",
    ),
    "review_invocation_cli": (
        "review_resume_completes_manual_and_native_held_tasks_without_execution",
        "review_holds_the_controller_lock_until_native_completion",
        "review_resume_revokes_stale_pending_approval_before_any_native_or_check_launch",
        "auto_follow_up_retries_one_current_criterion_with_bound_feedback_and_fresh_review",
        "auto_follow_up_stops_on_unknown_history_scope_and_native_permission_failures",
        "auto_follow_up_shares_the_iteration_budget_and_never_repeats_semantic_rejection",
        "auto_follow_up_revalidates_review_and_hidden_sources_after_iteration_advance",
        "auto_follow_up_requires_exec_review_and_a_finite_unforced_budget",
        "auto_follow_up_does_not_approve_or_retry_new_failed_checks",
    ),
    "invocation_cli": (
        "auto_repair_respects_iteration_budget_and_plain_exec_does_not_retry",
        "native_denial_and_terminal_failures_cannot_enter_postflight_with_complete_report",
        "successful_invocation_binds_exact_stdin_context_and_keeps_private_payloads_out_of_receipt",
        "native_context_retrieves_documents_from_literal_task_title_terms",
        "intake_binding_reaches_native_checks_review_and_historical_completion",
        "context_preview_withholds_mismatched_or_malformed_invocation_metadata",
    ),
    "invocation_guard_cli": (
        "guarded_executor_reconciles_exact_calls_and_keeps_payloads_private",
        "guarded_executor_denies_protected_outside_alias_and_unknown_actions_before_effects",
        "guarded_executor_refuses_missing_mismatched_and_conflicting_mediation",
        "guarded_executor_rejects_receipt_replay_and_changed_decision_artifacts",
        "guarded_executor_requires_explicit_scope_and_observed_shell_declarations",
    ),
    "context_cli": (
        "context_withholds_changed_markdown_until_explicit_reindex",
        "context_person_layer_requires_configured_audience_scope_and_current_source",
        "context_review_queue_requires_exact_root_access_and_withholds_candidate_content",
        "context_review_queue_preserves_stale_and_malformed_source_boundaries",
        "context_delivery_revalidates_selected_document_evidence_before_native_use",
    ),
    "persona_transcripts_cli": (
        "collection_preview_is_read_only_and_collection_does_not_publish_candidates",
        "preference_acceptance_binds_review_and_live_sources_through_mcp_and_refresh",
    ),
    "persona_hooks_refiner_cli": (
        "malformed_binding_and_timeout_failures_preserve_original_without_workflow_handoff",
        "replay_closed_turn_and_conflicting_native_identity_never_repeat_the_processor",
        "revocation_or_a_new_prompt_withholds_a_blocked_processors_old_result",
        "durable_capture_fences_block_refinement_before_processing_and_before_publication",
        "disabling_refiner_revokes_processing_even_when_native_configuration_is_malformed",
        "bound_intake_survives_tool_events_and_routes_only_its_session",
        "refiner_exposure_is_event_scoped_and_recovery_cannot_erase_it",
        "marker_loss_or_a_conflicting_event_blocks_the_first_preflight",
        "binding_rejects_ordinary_stale_and_cross_project_sources",
        "prepared_handoff_blocks_run_and_allows_exact_cas_recovery",
        "missing_intake_marker_exact_recovery_requires_latest_cas_without_changing_session",
        "missing_intake_marker_replacement_requires_cas_and_rejects_superseded_intakes",
    ),
    "persona_hooks_cli": (
        "semantic_hook_habit_requires_attestation_independent_sources_review_and_current_mcp_receipts",
        "offered_profile_preserves_original_observation_and_restricts_later_echo_promotion",
        "ordinary_mcp_profile_tool_read_marks_following_echo_as_influenced",
        "unreviewed_draft_history_does_not_hide_a_later_selected_attestation",
    ),
    "persona_hooks_worker_cli": (
        "status_is_read_only_and_capture_setup_does_not_start_a_worker",
        "restart_preserves_settings_and_completed_checkpoints_but_observes_revised_episodes",
        "independent_client_slots_charge_only_their_own_eligible_episodes",
        "concurrent_start_has_one_owner_and_stop_cancels_the_processor_and_releases_its_lease",
        "failed_and_timed_out_attempts_stop_without_retry_until_explicit_restart",
        "capture_revocation_and_processor_drift_withhold_inflight_results",
        "stop_cancels_a_pending_start_without_an_owner_and_allows_a_fresh_run",
        "interrupted_idle_worker_can_restart_without_replaying_completed_work_or_an_old_stop",
        "status_preserves_files_and_rejects_replaced_worker_storage",
    ),
    "persona_hooks_readiness_cli": (
        "absent_optional_hooks_are_not_failures_and_status_does_not_install_anything",
        "installed_capture_and_observed_current_sessions_are_separate_boundaries",
        "native_drift_disabled_hooks_and_malformed_config_do_not_hide_the_capture_grant",
        "configured_refiner_is_not_a_provider_test_or_a_background_miner",
        "unavailable_capture_journal_preserves_other_readiness_observations",
    ),
}

UI_SUITE = "mcp/servers/mmcg/assets/lens/app.test.cjs"
UI_COMPLETION = "Lens focused DOM/static regressions passed"


def test_outcome(output: bytes, expected: tuple[str, ...]) -> dict:
    decoded = output.decode("utf-8", errors="replace")
    rows = re.findall(r"^test (\S+) \.\.\. (ok|FAILED|ignored[^\n]*)$", decoded, re.M)
    passed = [name for name, status in rows if status == "ok"]
    summary = re.findall(r"^test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", decoded, re.M)
    valid = (sorted(passed) == sorted(expected) and len(rows) == len(expected)
             and summary == [(str(len(expected)), "0", "0")])
    return {"status": "passed" if valid else "failed", "expected": list(expected),
            "passed": passed, "missing": sorted(set(expected) - set(passed))}


def ui_outcome(output: bytes, source_root: Path = ROOT) -> dict:
    """Count the existing Lens harness as one aggregate, never its helpers."""
    decoded = output.decode("utf-8", errors="replace")
    rows = re.findall(r"^(?:not )?ok \d+ - .+$", decoded, re.M)
    summary = {name: re.findall(rf"^# {name} (\d+)$", decoded, re.M)
               for name in ("tests", "pass", "fail", "cancelled", "skipped", "todo")}
    expected_summary = {name: ["1" if name in ("tests", "pass") else "0"]
                        for name in summary}
    valid = (rows in ([f"ok 1 - {UI_SUITE}"], [f"ok 1 - {source_root / UI_SUITE}"])
             and re.findall(r"^\d+\.\.\d+$", decoded, re.M) == ["1..1"]
             and summary == expected_summary
             and decoded.splitlines().count(f"# {UI_COMPLETION}") == 1)
    return {"status": "passed" if valid else "failed", "suite": UI_SUITE,
            "aggregation": "one_file_suite", "expected_suites": 1,
            "passed_suites": int(valid)}


def source_identity(source_root: Path = ROOT) -> dict:
    crate = source_root / "mcp/servers/mmcg"
    paths = [crate / "Cargo.toml", crate / "Cargo.lock", crate / "build.rs",
             source_root / "schemas/invocation-receipt-v1.schema.json",
             source_root / "schemas/invocation-receipt-v2.schema.json"]
    # Include embedded assets, templates and integration fixture helpers.
    for directory in ("src", "tests", "templates", "assets"):
        paths += sorted(p for p in (crate / directory).rglob("*") if p.is_file())
    items = {}
    for path in sorted(set(paths)):
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 4 * 1024 * 1024:
            raise ValueError(f"unsupported evidence source: {path.relative_to(source_root)}")
        items[str(path.relative_to(source_root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    for name in ("evals/control/harness.py", "tests/evals/control/test_harness.py", "evals/shared/process.py"):
        items[name] = hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
    digest = hashlib.sha256(json.dumps(items, sort_keys=True).encode()).hexdigest()
    return {"sha256": digest, "files": items}


PROFILE_LIB_CASES = (
    "miner::profile::tests::feedback_delivers_a_ranked_prefix_instead_of_all_or_nothing",
    "miner::profile::tests::feedback_nothing_fits_omits_the_whole_list",
    "miner::profile::tests::feedback_priority_orders_scope_then_recency_then_key",
    "miner::profile::tests::feedback_view_ranks_by_scope_kind_with_key_as_the_tiebreak",
    "miner::profile::tests::feedback_cap_and_eligibility_spend_slots_only_on_eligible_rows",
    "miner::profile::tests::delivery_summary_reports_eligible_deliverable_fitting_and_tokens_needed",
    "context::tests::fit_person_layer_shrinks_to_the_views_own_size_across_a_budget_range",
    "context::tests::fit_person_layer_fixes_the_shrink_from_initial_and_whole_layer_regressions",
    "context::tests::fit_person_layer_keeps_the_first_view_when_a_shrink_retry_errors",
    "context::tests::fit_person_layer_does_not_shrink_person_to_make_room_for_documentation",
    "mcp::tests::resolve_profile_budget_uses_argument_then_project_setting_then_default",
    "onboarding::tests::old_setup_file_without_the_profile_budget_key_loads_as_the_default_and_omits_it",
)


def run_cli(output: Path, source_root: Path = ROOT) -> list[dict]:
    cargo = shutil.which("cargo")
    if not cargo:
        raise ValueError("cargo is required for CLI conformance")
    results = []
    env = dict(os.environ, CARGO_TERM_COLOR="never")
    targets = [(name, ("--test", name), selectors) for name, selectors in CASES.items()]
    targets.append(("profile_delivery_lib", ("--lib",), PROFILE_LIB_CASES))
    for target, selection, selectors in targets:
        command = [cargo, "test", "--locked", *selection, "--", "--exact",
                   "--test-threads=2", *selectors]
        result = run_bounded(command, cwd=source_root / "mcp/servers/mmcg", env=env, timeout=600,
                             stdout_limit=4 * 1024 * 1024, stderr_limit=1024 * 1024)
        (output / f"{target}.stdout").write_bytes(result.stdout)
        (output / f"{target}.stderr").write_bytes(result.stderr)
        row = test_outcome(result.stdout, selectors)
        row.update(target=target, elapsed_seconds=result.elapsed_seconds,
                   exit_code=result.returncode, stop_reason=result.stop_reason)
        if result.returncode != 0 or result.stop_reason:
            row["status"] = "failed"
        results.append(row)
        print(f"{target}: {row['status']} ({len(row['passed'])}/{len(selectors)})", flush=True)
    return results


def run_ui(output: Path, source_root: Path = ROOT) -> dict:
    node = shutil.which("node")
    if not node:
        raise ValueError("node is required for UI boundary conformance")
    result = run_bounded([node, "--test", "--test-reporter=tap", UI_SUITE],
                         cwd=source_root, env=dict(os.environ), timeout=60,
                         stdout_limit=1024 * 1024, stderr_limit=1024 * 1024)
    (output / "lens.stdout").write_bytes(result.stdout)
    (output / "lens.stderr").write_bytes(result.stderr)
    row = ui_outcome(result.stdout, source_root)
    row.update(elapsed_seconds=result.elapsed_seconds, exit_code=result.returncode,
               stop_reason=result.stop_reason)
    if result.returncode != 0 or result.stop_reason:
        row["status"] = "failed"
    print(f"Lens UI: {row['status']} (1 aggregate suite)", flush=True)
    return row


def report_status(report: dict) -> str:
    statuses = (report["bounded_model_safety"]["status"],
                report["sampled_cli_conformance"], report["sampled_ui_conformance"])
    if "failed" in statuses or not report["source_unchanged"]:
        return "failed"
    return "passed" if all(status == "passed" for status in statuses) else "incomplete"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new private report directory")
    parser.add_argument("--model-only", action="store_true", help="skip CLI and UI, report both as not_run")
    parser.add_argument("--source-repo", type=Path, default=ROOT, help="source checkout to test; defaults to this repository")
    args = parser.parse_args(argv)
    source_root = args.source_repo.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    started = time.monotonic()
    report = {"schema_version": 1, "kind": "mastermind-control-loop-eval",
              "runtime": {"python": platform.python_version(), "platform": platform.platform()},
              "bounded_model_safety": model_report(), "sampled_cli_conformance": "not_run",
              "sampled_ui_conformance": "not_run", "cli_cases": [], "ui_suites": [],
              "semantic_goal_success": None, "token_savings": None,
              "cost_savings": None, "limitations": [
                  "Finite abstraction, no refinement proof of all Rust executions.",
                  "CLI cases sample local behavior with synthetic native clients, without model inference.",
                  "Persona, worker, readiness, intake and UI checks are adjacent boundaries, not extra model obligations.",
                  "The UI harness is one aggregate DOM/static suite, not a browser or model-use measurement.",
                  "Recovery assumes a stable environment and successful checks and judgments.",
                  "Owner-writable local evidence, no sandbox or independent reviewer attestation.",
                  "No semantic quality, long-task convergence, token or cost improvement is measured."]}
    try:
        report["source_repo"] = str(source_root)
        report["source"] = source_identity(source_root)
        report["revision"] = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=source_root, text=True, timeout=10).strip()
        report["working_tree_dirty"] = bool(subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=source_root, timeout=10))
        if not args.model_only:
            report["cli_cases"] = run_cli(output, source_root)
            report["sampled_cli_conformance"] = (
                "passed" if report["cli_cases"]
                and all(r["status"] == "passed" for r in report["cli_cases"]) else "failed")
            report["ui_suites"] = [run_ui(output, source_root)]
            report["sampled_ui_conformance"] = report["ui_suites"][0]["status"]
        report["source_unchanged"] = source_identity(source_root) == report["source"]
        report["status"] = report_status(report)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        report.update(status="failed", error=str(error))
    report["elapsed_seconds"] = time.monotonic() - started
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Control loop eval: {report['status']}; report: {output / 'report.json'}")
    return 0 if report["status"] == "passed" or (args.model_only and report["status"] == "incomplete") else 1


if __name__ == "__main__":
    sys.exit(main())
