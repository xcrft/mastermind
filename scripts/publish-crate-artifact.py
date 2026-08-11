#!/usr/bin/env python3
"""Prepare and upload one verified .crate through Cargo's registry Web API.

Cargo's CLI always packages again during ``cargo publish``. This helper uses
the documented ``/api/v1/crates/new`` wire format so the bytes uploaded by the
release job are exactly the artifact built and verified in the previous job.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.request


CRATES_IO_PUBLISH_URL = "https://crates.io/api/v1/crates/new"
MAX_CRATE_BYTES = 32 * 1024 * 1024
MAX_UNPACKED_BYTES = 256 * 1024 * 1024
MAX_MEMBERS = 4096
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def _json_without_duplicates(raw: str) -> dict:
    def pairs(items):
        value = {}
        for key, item in items:
            if key in value:
                raise ValueError(f"duplicate JSON key: {key}")
            value[key] = item
        return value

    parsed = json.loads(raw, object_pairs_hook=pairs)
    if not isinstance(parsed, dict):
        raise ValueError("publish metadata must be a JSON object")
    return parsed


def _safe_members(archive: tarfile.TarFile) -> tuple[str, list[tarfile.TarInfo]]:
    members = archive.getmembers()
    if not members or len(members) > MAX_MEMBERS:
        raise ValueError("crate archive has an invalid member count")
    roots: set[str] = set()
    seen: set[str] = set()
    total = 0
    for member in members:
        name = member.name
        parts = pathlib.PurePosixPath(name).parts
        if (
            not parts
            or pathlib.PurePosixPath(name).is_absolute()
            or any(part in {"", ".", ".."} for part in parts)
            or "\\" in name
            or name in seen
        ):
            raise ValueError(f"unsafe crate archive member: {name!r}")
        if not (member.isdir() or member.isfile()):
            raise ValueError(f"non-regular crate archive member: {name!r}")
        seen.add(name)
        roots.add(parts[0])
        if member.isfile():
            total += member.size
            if total > MAX_UNPACKED_BYTES:
                raise ValueError("crate archive expands beyond the safety limit")
    if len(roots) != 1:
        raise ValueError("crate archive must have exactly one package root")
    return roots.pop(), members


def _read_archive_file(
    archive: tarfile.TarFile, members: list[tarfile.TarInfo], name: str
) -> bytes:
    match = next((member for member in members if member.name == name), None)
    if match is None or not match.isfile():
        raise ValueError(f"crate archive is missing {name}")
    source = archive.extractfile(match)
    if source is None:
        raise ValueError(f"cannot read {name} from crate archive")
    data = source.read(match.size + 1)
    if len(data) != match.size:
        raise ValueError(f"crate archive member size changed while reading {name}")
    return data


def inspect_crate_identity(crate_path: pathlib.Path) -> tuple[str, str, str]:
    if not crate_path.is_file() or crate_path.is_symlink():
        raise ValueError("crate artifact must be a regular file")
    size = crate_path.stat().st_size
    if size <= 0 or size > MAX_CRATE_BYTES:
        raise ValueError("crate artifact has an invalid size")
    with tarfile.open(crate_path, "r:gz") as archive:
        root, members = _safe_members(archive)
        manifest_raw = _read_archive_file(archive, members, f"{root}/Cargo.toml")
    manifest = tomllib.loads(manifest_raw.decode("utf-8"))
    package = manifest.get("package")
    if not isinstance(package, dict):
        raise ValueError("packaged Cargo.toml lacks [package]")
    name = package.get("name")
    version = package.get("version")
    if not isinstance(name, str) or not isinstance(version, str):
        raise ValueError("packaged Cargo.toml lacks package name/version")
    if root != f"{name}-{version}" or crate_path.name != f"{root}.crate":
        raise ValueError("crate filename, archive root, and package identity differ")
    return name, version, root


def _extract_crate(crate_path: pathlib.Path, destination: pathlib.Path) -> pathlib.Path:
    with tarfile.open(crate_path, "r:gz") as archive:
        root, members = _safe_members(archive)
        for member in members:
            target = destination.joinpath(*pathlib.PurePosixPath(member.name).parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            source = archive.extractfile(member)
            if source is None:
                raise ValueError(f"cannot read {member.name} from crate archive")
            with target.open("xb") as output:
                shutil.copyfileobj(source, output)
            if target.stat().st_size != member.size:
                raise ValueError(f"crate archive member size changed: {member.name}")
    return destination / root


def _relative_optional_path(package_root: pathlib.Path, value: object) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError("Cargo metadata path must be a string or null")
    path = pathlib.Path(value)
    if not path.is_absolute():
        path = package_root / path
    path = path.resolve()
    try:
        return path.relative_to(package_root.resolve()).as_posix()
    except ValueError as error:
        raise ValueError("Cargo metadata path escapes the packaged crate") from error


def prepare_metadata(crate_path: pathlib.Path) -> dict:
    name, version, _ = inspect_crate_identity(crate_path)
    with tempfile.TemporaryDirectory(prefix="mastermind-crate-metadata-") as raw:
        package_root = _extract_crate(crate_path, pathlib.Path(raw))
        manifest_path = package_root / "Cargo.toml"
        result = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--no-deps",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest_path),
            ],
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode != 0:
            raise RuntimeError(f"cargo metadata failed for packaged crate:\n{result.stderr}")
        cargo_metadata = _json_without_duplicates(result.stdout)
        packages = cargo_metadata.get("packages")
        if not isinstance(packages, list) or len(packages) != 1:
            raise ValueError("packaged crate metadata must contain exactly one package")
        package = packages[0]
        if package.get("name") != name or package.get("version") != version:
            raise ValueError("cargo metadata identity differs from the crate artifact")

        dependencies = []
        for dependency in package.get("dependencies", []):
            dependencies.append(
                {
                    "name": dependency["name"],
                    "version_req": dependency["req"],
                    "features": dependency.get("features", []),
                    "optional": dependency.get("optional", False),
                    "default_features": dependency.get("uses_default_features", True),
                    "target": dependency.get("target"),
                    "kind": dependency.get("kind") or "normal",
                    "registry": dependency.get("registry"),
                    "explicit_name_in_toml": dependency.get("rename"),
                }
            )
        dependencies.sort(
            key=lambda item: (
                item["target"] or "",
                item["kind"],
                item["explicit_name_in_toml"] or item["name"],
            )
        )

        readme_file = _relative_optional_path(package_root, package.get("readme"))
        readme = None
        if readme_file is not None:
            readme = (package_root / readme_file).read_text(encoding="utf-8")
        license_file = _relative_optional_path(package_root, package.get("license_file"))

        metadata = {
            "name": name,
            "vers": version,
            "deps": dependencies,
            "features": package.get("features", {}),
            "authors": package.get("authors", []),
            "description": package.get("description"),
            "documentation": package.get("documentation"),
            "homepage": package.get("homepage"),
            "readme": readme,
            "readme_file": readme_file,
            "keywords": package.get("keywords", []),
            "categories": package.get("categories", []),
            "license": package.get("license"),
            "license_file": license_file,
            "repository": package.get("repository"),
            "badges": {},
            "links": package.get("links"),
            "rust_version": package.get("rust_version"),
        }
    validate_crate_binding(crate_path, metadata)
    return metadata


def validate_crate_binding(crate_path: pathlib.Path, metadata: dict) -> None:
    name, version, _ = inspect_crate_identity(crate_path)
    if metadata.get("name") != name or metadata.get("vers") != version:
        raise ValueError("publish metadata identity differs from the crate artifact")


def build_publish_body(metadata: dict, crate_bytes: bytes) -> bytes:
    encoded = json.dumps(
        metadata, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    if len(encoded) >= 2**32 or len(crate_bytes) >= 2**32:
        raise ValueError("publish request component exceeds Cargo protocol limits")
    return (
        struct.pack("<I", len(encoded))
        + encoded
        + struct.pack("<I", len(crate_bytes))
        + crate_bytes
    )


def publish(crate_path: pathlib.Path, metadata_path: pathlib.Path, expected_sha256: str) -> None:
    if not SHA256_RE.fullmatch(expected_sha256):
        raise ValueError("expected SHA-256 must be 64 lowercase hexadecimal characters")
    crate_bytes = crate_path.read_bytes()
    actual_sha256 = hashlib.sha256(crate_bytes).hexdigest()
    if actual_sha256 != expected_sha256:
        raise ValueError("crate artifact SHA-256 differs from the verified checksum")
    metadata = _json_without_duplicates(metadata_path.read_text(encoding="utf-8"))
    validate_crate_binding(crate_path, metadata)
    token = os.environ.get("CARGO_REGISTRY_TOKEN", "")
    if not token or "\n" in token or "\r" in token:
        raise ValueError("CARGO_REGISTRY_TOKEN is required and must be one line")

    request = urllib.request.Request(
        CRATES_IO_PUBLISH_URL,
        data=build_publish_body(metadata, crate_bytes),
        headers={
            "Accept": "application/json",
            "Authorization": token,
            "Content-Type": "application/octet-stream",
            "User-Agent": "mastermind-exact-crate-publisher/1",
        },
        method="PUT",
    )
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            raw = response.read(1024 * 1024 + 1)
            if len(raw) > 1024 * 1024:
                raise RuntimeError("crates.io response exceeded the safety limit")
    except urllib.error.HTTPError as error:
        detail = error.read(64 * 1024).decode("utf-8", errors="replace")
        raise RuntimeError(f"crates.io publish failed with HTTP {error.code}: {detail}") from error
    except urllib.error.URLError as error:
        raise RuntimeError(f"crates.io publish transport failed: {error.reason}") from error

    response_value = _json_without_duplicates(raw.decode("utf-8"))
    if response_value.get("errors"):
        raise RuntimeError(f"crates.io rejected the publish: {response_value['errors']}")
    print(f"published {metadata['name']}@{metadata['vers']} from sha256:{actual_sha256}")


def _write_metadata(path: pathlib.Path, metadata: dict) -> None:
    if path.exists():
        raise ValueError(f"refusing to overwrite metadata output: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(
        json.dumps(metadata, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    os.replace(temporary, path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    prepare_parser = commands.add_parser("prepare")
    prepare_parser.add_argument("--crate", required=True, type=pathlib.Path)
    prepare_parser.add_argument("--output", required=True, type=pathlib.Path)
    publish_parser = commands.add_parser("publish")
    publish_parser.add_argument("--crate", required=True, type=pathlib.Path)
    publish_parser.add_argument("--metadata", required=True, type=pathlib.Path)
    publish_parser.add_argument("--expected-sha256", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "prepare":
            _write_metadata(args.output, prepare_metadata(args.crate))
        else:
            publish(args.crate, args.metadata, args.expected_sha256)
    except (OSError, ValueError, RuntimeError, tarfile.TarError, tomllib.TOMLDecodeError) as error:
        print(f"exact crate publisher: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
