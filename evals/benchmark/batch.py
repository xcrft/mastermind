"""Bound batch plans, one-shot execution order and snapshot checks."""

from __future__ import annotations

from contextlib import contextmanager
from pathlib import Path
import hashlib
import os
import re
import stat

from . import artifacts as artifact_io
from . import conditions as condition_contract


def _check_batch_root(root: Path, descriptor: int, expected: os.stat_result) -> None:
    """Ensure a held batch-directory descriptor still names the batch root."""
    try:
        observed = root.stat(follow_symlinks=False)
        canonical_root = root.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        raise artifact_io.BenchmarkError("batch_lock", "batch root changed while holding its execution lock") from error
    if (not stat.S_ISDIR(observed.st_mode) or not artifact_io._same_node(observed, expected)
            or not artifact_io._same_node(os.fstat(descriptor), expected) or canonical_root != root):
        raise artifact_io.BenchmarkError("batch_lock", "batch root changed while holding its execution lock")


def validate_batch_binding(value: object) -> dict:
    if (not isinstance(value, dict)
            or set(value) != {"batch_id", "plan_sha256", "position"}
            or not isinstance(value.get("batch_id"), str)
            or not re.fullmatch(r"batch-[0-9a-f]{32}", value["batch_id"])
            or not isinstance(value.get("plan_sha256"), str)
            or not re.fullmatch(r"[0-9a-f]{64}", value["plan_sha256"])
            or type(value.get("position")) is not int or value["position"] < 0):
        raise artifact_io.BenchmarkError("invalid_batch_binding", "invalid trial batch binding")
    return value


def batch_plan_identity(batch: dict) -> dict:
    identity = {"batch_id": batch["batch_id"], "task_id": batch["task_id"],
            "repetitions": batch["repetitions"],
            "trials": [{key: item[key] for key in ("directory", "condition", "repetition")}
                       for item in batch["trials"]]}
    if batch.get("schema_version") == 3:
        identity["conditions"] = batch["conditions"]
        if "calibration" in batch:
            identity["calibration"] = batch["calibration"]
    return identity


def validate_batch_summary(batch: dict, *, allow_legacy: bool = False,
                           code: str = "batch_changed", identity_code: str | None = None) -> list[dict]:
    identity_code = identity_code or code
    version = batch.get("schema_version") if isinstance(batch, dict) else None
    required = {"kind", "schema_version", "task_id",
                "repetitions", "trials", "quality_uplift", "comparison_accepted"}
    if version in (2, 3):
        required |= {"batch_id", "plan_sha256"}
    if version == 3:
        required.add("conditions")
    if (not isinstance(batch, dict) or not required <= set(batch)
            or set(batch) - required - {"corpus_case", "calibration"}
            or batch.get("kind") != "mastermind-research-batch"
            or type(version) is not int or version not in ((1, 2, 3) if allow_legacy else (2, 3))
            or not isinstance(batch.get("task_id"), str) or not batch["task_id"]
            or type(batch.get("repetitions")) is not int or not 1 <= batch["repetitions"] <= 20
            or batch.get("quality_uplift") is not None or batch.get("comparison_accepted") is not False
            or not isinstance(batch.get("trials"), list)):
        raise artifact_io.BenchmarkError(code, "invalid batch plan")
    try:
        names = condition_contract.condition_names(batch)
        if "calibration" in batch:
            condition_contract.validate_calibration(batch["calibration"], batch.get("conditions"))
    except (artifact_io.BenchmarkError, KeyError) as error:
        raise artifact_io.BenchmarkError(code, "invalid batch conditions") from error
    if not len(batch["trials"]) == len(names) * batch["repetitions"] <= condition_contract.TRIAL_LIMIT:
        raise artifact_io.BenchmarkError(code, "batch must retain every planned condition and repetition within 60 trials")
    seen = set()
    for position, item in enumerate(batch["trials"]):
        common = item.get("common_sha256") if isinstance(item, dict) else None
        invalid_common = (common is not None
                          and (not isinstance(common, str) or not re.fullmatch(r"[0-9a-f]{64}", common)))
        if (not isinstance(item, dict)
                or set(item) != {"directory", "condition", "repetition", "common_sha256", "status"}
                or not isinstance(item.get("directory"), str)
                or not re.fullmatch(r"trial-[0-9a-f]{32}", item["directory"])
                or item["directory"] in seen
                or type(item.get("repetition")) is not int
                or item.get("status") not in {"prepared", "setup_failed"}
                or invalid_common):
            raise artifact_io.BenchmarkError(code, "invalid batch slot")
        seen.add(item["directory"])
        repetition, offset = divmod(position, len(names))
        if (item.get("repetition") != repetition
                or item.get("condition") != names[(repetition + offset) % len(names)]):
            raise artifact_io.BenchmarkError(code, "batch order differs from its planned matrix")
    if version != 1 and (not isinstance(batch.get("batch_id"), str)
            or not re.fullmatch(r"batch-[0-9a-f]{32}", batch["batch_id"])
            or not isinstance(batch.get("plan_sha256"), str)
            or artifact_io.digest(batch_plan_identity(batch)) != batch["plan_sha256"]):
        raise artifact_io.BenchmarkError(identity_code, "bound batch plan identity changed")
    return batch["trials"]


def optional_artifact(path: Path, limit: int) -> tuple[bytes, tuple[int, ...]] | None:
    try:
        path.lstat()
    except FileNotFoundError:
        return None
    body = artifact_io.read_file(path, limit)
    return body, artifact_io.file_identity(path.lstat())


def load_bound_batch(trial: Path, manifest: dict, manifest_bytes: bytes) -> dict:
    binding = validate_batch_binding(manifest.get("batch"))
    batch_root = trial.parent
    try:
        root_before = artifact_io.file_identity(batch_root.lstat())
        if not stat.S_ISDIR(root_before[-1]):
            raise artifact_io.BenchmarkError("batch_changed", "batch root is not a directory")
        batch_path = batch_root / "batch.json"
        batch_bytes = artifact_io.read_file(batch_path)
        batch_identity = artifact_io.file_identity(batch_path.lstat())
        batch = artifact_io.parse_json(batch_bytes)
    except (OSError, ValueError) as error:
        raise artifact_io.BenchmarkError("batch_changed", "cannot read the bound batch plan") from error
    items = validate_batch_summary(batch)
    task = manifest.get("task")
    if (batch["batch_id"] != binding["batch_id"]
            or batch["plan_sha256"] != binding["plan_sha256"]
            or not isinstance(task, dict) or task.get("id") != batch["task_id"]
            or binding["position"] >= len(items)):
        raise artifact_io.BenchmarkError("batch_changed", "trial binding differs from its batch plan")
    manifest_records = []
    for position, item in enumerate(items):
        directory = batch_root / item["directory"]
        try:
            if directory.is_symlink() or not directory.is_dir() or directory.resolve(strict=True) != directory.absolute():
                raise artifact_io.BenchmarkError("batch_changed", "batch trial directory changed")
            body = artifact_io.read_file(directory / "manifest.json")
            identity = artifact_io.file_identity((directory / "manifest.json").lstat())
            value = artifact_io.parse_json(body)
        except (OSError, ValueError) as error:
            raise artifact_io.BenchmarkError("batch_changed", "batch trial manifest is unavailable") from error
        expected_binding = {"batch_id": batch["batch_id"], "plan_sha256": batch["plan_sha256"],
                            "position": position}
        if (value.get("kind") != "mastermind-research-trial"
                or value.get("schema_version") not in (1, 2, 3, 4)
                or value.get("batch") != expected_binding
                or value.get("trial_id") != item["directory"]
                or value.get("condition") != item["condition"]
                or value.get("repetition") != item["repetition"]
                or value.get("status") != item["status"]
                or value.get("common_sha256") != item["common_sha256"]):
            raise artifact_io.BenchmarkError("batch_changed", "trial manifest differs from its batch slot")
        expected_spec = next((spec for spec in batch.get("conditions", [])
                              if spec["id"] == item["condition"]), None)
        if condition_contract.condition_spec(value) != expected_spec:
            raise artifact_io.BenchmarkError("batch_changed", "trial condition differs from its batch specification")
        if value.get("calibration") != batch.get("calibration"):
            raise artifact_io.BenchmarkError("batch_changed", "trial calibration differs from its batch specification")
        if position == binding["position"] and (body != manifest_bytes or value != manifest):
            raise artifact_io.BenchmarkError("batch_changed", "selected trial manifest changed during batch validation")
        manifest_records.append({"bytes": body, "identity": identity,
                                 "sha256": hashlib.sha256(body).hexdigest()})
    if artifact_io.file_identity(batch_root.lstat()) != root_before:
        raise artifact_io.BenchmarkError("batch_changed", "batch root changed during validation")
    return {"root": batch_root, "root_identity": root_before, "batch_bytes": batch_bytes,
            "batch_identity": batch_identity, "batch": batch, "items": items,
            "manifests": manifest_records, "position": binding["position"]}


def batch_attempt_state(snapshot: dict, *, claimed: bool) -> tuple[dict, dict[str, tuple[int, ...] | None]]:
    previous_hash = None
    fingerprints = {}
    current = snapshot["position"]
    for position, item in enumerate(snapshot["items"]):
        directory = snapshot["root"] / item["directory"]
        lock = optional_artifact(directory / "run.lock", 0)
        result = optional_artifact(directory / "result.json", artifact_io.CONTROL_BYTE_LIMIT)
        if position < current:
            if lock is None or result is None:
                raise artifact_io.BenchmarkError("batch_order", "every earlier batch attempt must finish first")
            try:
                value = artifact_io.parse_json(result[0])
            except ValueError as error:
                raise artifact_io.BenchmarkError("batch_order", "an earlier batch result is invalid") from error
            expected = {"batch_id": snapshot["batch"]["batch_id"],
                        "plan_sha256": snapshot["batch"]["plan_sha256"], "position": position,
                        "previous_result_sha256": previous_hash}
            if (value.get("kind") != "mastermind-research-result"
                    or value.get("schema_version") != 2
                    or value.get("trial_id") != item["directory"]
                    or value.get("manifest_sha256") != snapshot["manifests"][position]["sha256"]
                    or value.get("batch_execution") != expected):
                raise artifact_io.BenchmarkError("batch_order", "an earlier result breaks the batch execution chain")
            previous_hash = hashlib.sha256(result[0]).hexdigest()
        elif position == current:
            if result is not None or (lock is None) == claimed:
                raise artifact_io.BenchmarkError("already_run" if not claimed else "batch_changed",
                                     "trial was already attempted; prepare a new balanced batch")
        elif lock is not None or result is not None:
            raise artifact_io.BenchmarkError("batch_order", "a later batch attempt was started out of order")
        if position != current:
            fingerprints[f"{item['directory']}/run.lock"] = lock[1] if lock else None
            fingerprints[f"{item['directory']}/result.json"] = result[1] if result else None
    receipt = {"batch_id": snapshot["batch"]["batch_id"],
               "plan_sha256": snapshot["batch"]["plan_sha256"], "position": current,
               "previous_result_sha256": previous_hash}
    return receipt, fingerprints


@contextmanager
def batch_execution_guard(trial: Path, manifest: dict, manifest_bytes: bytes):
    if "batch" not in manifest:
        yield None
        return
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW"):
        raise artifact_io.BenchmarkError("batch_platform", "batch execution requires POSIX no-follow file locking")
    import fcntl
    root = trial.parent
    path = root / "execution.lock"
    root_descriptor = None
    descriptor = None
    try:
        root_descriptor = os.open(
            root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_NONBLOCK)
        root_before = os.fstat(root_descriptor)
        if not stat.S_ISDIR(root_before.st_mode):
            raise artifact_io.BenchmarkError("batch_lock", "batch root is not a directory")
        _check_batch_root(root, root_descriptor, root_before)
        descriptor = os.open(
            "execution.lock", os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK,
            dir_fd=root_descriptor)
    except OSError as error:
        if root_descriptor is not None:
            os.close(root_descriptor)
        raise artifact_io.BenchmarkError("batch_lock", "cannot acquire the batch execution lock") from error
    except Exception:
        if root_descriptor is not None:
            os.close(root_descriptor)
        raise
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or before.st_size != 0:
            raise artifact_io.BenchmarkError("batch_lock", "batch execution lock is invalid")
        named = os.stat("execution.lock", dir_fd=root_descriptor, follow_symlinks=False)
        current = path.lstat()
        if artifact_io.file_identity(before) != artifact_io.file_identity(named) or artifact_io.file_identity(before) != artifact_io.file_identity(current):
            raise artifact_io.BenchmarkError("batch_lock", "batch execution lock changed while opening")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise artifact_io.BenchmarkError("batch_busy", "another batch attempt is still running") from error
        _check_batch_root(root, root_descriptor, root_before)
    except OSError as error:
        os.close(descriptor)
        os.close(root_descriptor)
        raise artifact_io.BenchmarkError("batch_lock", "cannot acquire the batch execution lock") from error
    except Exception:
        os.close(descriptor)
        os.close(root_descriptor)
        raise
    try:
        snapshot = load_bound_batch(trial, manifest, manifest_bytes)
        if snapshot["root_identity"][:2] != (root_before.st_dev, root_before.st_ino):
            raise artifact_io.BenchmarkError("batch_lock", "batch root changed while validating the execution lock")
        _check_batch_root(root, root_descriptor, root_before)
        receipt, fingerprints = batch_attempt_state(snapshot, claimed=False)
        snapshot.update(receipt=receipt, fingerprints=fingerprints,
                        lock_identity=artifact_io.file_identity(before))
        yield snapshot
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)
            os.close(root_descriptor)


def verify_batch_snapshot(trial: Path, manifest: dict, manifest_bytes: bytes,
                          execution: dict | None) -> None:
    if execution is None:
        return
    current = load_bound_batch(trial, manifest, manifest_bytes)
    receipt, fingerprints = batch_attempt_state(current, claimed=True)
    if (receipt != execution["receipt"] or fingerprints != execution["fingerprints"]
            or current["root_identity"] != execution["root_identity"]
            or current["batch_bytes"] != execution["batch_bytes"]
            or current["batch_identity"] != execution["batch_identity"]
            or current["manifests"] != execution["manifests"]
            or artifact_io.file_identity((trial.parent / "execution.lock").lstat()) != execution["lock_identity"]):
        raise artifact_io.BenchmarkError("batch_changed", "batch inputs changed during the attempt")
