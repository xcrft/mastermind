"""Freeze and verify the implementation shared by native CLI adapters."""

from pathlib import Path
import hashlib
import os
import sys

from evals.benchmark import artifacts as artifact_io
from evals.benchmark import runtime as runtime_identity


COMMON_BUNDLE = {
    "launch.py": "evals/benchmark/adapters/launch.py",
    **{path: path for path in (
        "evals/__init__.py", "evals/shared/__init__.py", "evals/shared/process.py",
        "evals/benchmark/__init__.py", "evals/benchmark/artifacts.py",
        "evals/benchmark/conditions.py", "evals/benchmark/protocol.py",
        "evals/benchmark/runtime.py", "evals/benchmark/source.py",
        "evals/benchmark/mcp.py", "evals/benchmark/tools.py",
        "evals/benchmark/adapters/__init__.py", "evals/benchmark/adapters/bundle.py",
    )},
}


def prepare_runtime(trial: Path, spec: dict, *, kind: str, version: str,
                    bundle_sources: dict, settings: dict | None = None) -> dict:
    cli = runtime_identity.runtime_pin(spec["cli"], kind)
    python_path = Path(sys.executable).resolve(strict=True)
    python = runtime_identity.runtime_pin({"path": str(python_path),
        "sha256": artifact_io.hash_file(python_path, artifact_io.BINARY_BYTE_LIMIT)["sha256"],
        "version": sys.version.split()[0], "origin": "preparation interpreter; dependencies not attested"}, "python")
    runtime = trial / "runtime"
    runtime.mkdir(mode=0o700)
    bundle = {}
    repository = Path(__file__).resolve().parents[3]
    for name, source in bundle_sources.items():
        body = artifact_io.read_file(repository / source)
        destination = runtime / name
        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        artifact_io.write_new_bytes(destination, body, mode=0o555)
        bundle[name] = hashlib.sha256(body).hexdigest()
    adapter = {"kind": kind, "path": str(runtime / "launch.py"),
               "sha256": bundle["launch.py"], "version": version,
               "origin": "frozen benchmark implementation bytes", "bundle": bundle,
               "cli": cli, "python": python}
    adapter.update(settings or {})
    artifact_io.write_new(trial / "adapter-runtime.json", adapter, mode=0o444)
    adapter["runtime_sha256"] = artifact_io.digest(adapter)
    return adapter



def verify_runtime(trial: Path, adapter: dict, bundle_sources: dict) -> None:
    expected = {key: value for key, value in adapter.items() if key != "runtime_sha256"}
    if artifact_io.load_json(trial / "adapter-runtime.json") != expected or artifact_io.digest(expected) != adapter["runtime_sha256"]:
        raise artifact_io.BenchmarkError("adapter_runtime_changed", "adapter runtime descriptor changed")
    runtime = trial / "runtime"
    allowed_directories = {parent.as_posix() for path in bundle_sources
                           for parent in Path(path).parents if parent != Path(".")}
    observed = set()
    if runtime.is_symlink() or runtime.resolve() != runtime:
        raise artifact_io.BenchmarkError("adapter_bundle_changed", "adapter bundle inventory changed")
    for directory, directories, files in os.walk(runtime, followlinks=False):
        for name in directories:
            path = Path(directory) / name
            if path.is_symlink() or path.relative_to(runtime).as_posix() not in allowed_directories:
                raise artifact_io.BenchmarkError("adapter_bundle_changed", "adapter bundle directory changed")
        observed.update((Path(directory) / name).relative_to(runtime).as_posix() for name in files)
    if observed != set(bundle_sources):
        raise artifact_io.BenchmarkError("adapter_bundle_changed", "adapter bundle inventory changed")
    if adapter["path"] != str(runtime / "launch.py") or set(adapter["bundle"]) != set(bundle_sources):
        raise artifact_io.BenchmarkError("adapter_bundle_changed", "adapter entry point or bundle changed")
    for name in bundle_sources:
        if hashlib.sha256(artifact_io.read_file(runtime / name)).hexdigest() != adapter["bundle"][name]:
            raise artifact_io.BenchmarkError("adapter_bundle_changed", "adapter implementation bytes changed")
    runtime_identity.runtime_pin(adapter["cli"], adapter["kind"])
    runtime_identity.runtime_pin(adapter["python"], "python")
