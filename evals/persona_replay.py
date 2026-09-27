#!/usr/bin/env python3
"""Replay a pinned local Git corpus through isolated persona stores.

This tests mechanical contracts, not the truth of a person's inferred habits.
Reports contain private repository metadata and must stay outside source control.
No model, network request, checkout hook or real user profile is used.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import sys

if __package__:
    from .benchmark_process import run_bounded
else:
    from benchmark_process import run_bounded


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def encoded(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True).encode()


def environment(home: Path) -> dict[str, str]:
    result = {k: v for k, v in os.environ.items()
              if not k.startswith(("GIT_", "MMCG_"))}
    result.update(HOME=str(home), USERPROFILE=str(home),
                  XDG_CONFIG_HOME=str(home / "config"),
                  CODEX_HOME=str(home / "codex"),
                  GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                  GIT_NO_REPLACE_OBJECTS="1", GIT_TERMINAL_PROMPT="0",
                  MMCG_PROFILE_CLIENT="persona-replay")
    return result


def run(command: list[str], cwd: Path, home: Path, stdin: bytes = b"") -> bytes:
    result = run_bounded(command, cwd=cwd, env=environment(home), stdin=stdin,
                         timeout=180, stdout_limit=32 * 1024 * 1024,
                         stderr_limit=256 * 1024)
    if result.stop_reason or result.returncode != 0:
        # Keep subprocess diagnostics in the explicitly selected local report.
        raise RuntimeError(f"{command[0]} failed: {result.stop_reason or result.returncode}; "
                           f"{result.stderr.decode(errors='replace')[:3000]}")
    return result.stdout


def git(cwd: Path, home: Path, *args: str, stdin: bytes = b"") -> bytes:
    return run(["git", "-c", "core.hooksPath=", "-c", "commit.gpgsign=false",
                *args], cwd, home, stdin)


def metadata(repo: Path, home: Path, ref: str) -> list[dict]:
    raw = git(repo, home, "log", "--no-mailmap", "-z",
              "--format=%H%x00%P%x00%an%x00%ae%x00%cn%x00%ce%x00%aI%x00%B", ref)
    fields = raw.decode("utf-8").split("\0")
    if fields[-1] == "":
        fields.pop()
    if len(fields) % 8:
        raise ValueError("malformed NUL-framed Git metadata")
    rows = []
    for i in range(0, len(fields), 8):
        sha, parents, name, email, committer, committer_email, date, message = fields[i:i+8]
        rows.append(dict(sha=sha, parents=parents.split(), name=name, email=email,
                         committer=committer, committer_email=committer_email,
                         date=date, coauthor_trailers=sum(
                             line.lower().startswith("co-authored-by:")
                             for line in message.splitlines())))
    return rows


def store_snapshot(home: Path) -> dict:
    db = home / ".mastermind/style.db"
    with sqlite3.connect(db.as_uri() + "?mode=ro", uri=True) as conn:
        commits = conn.execute("SELECT sha, authored_at FROM sampled_commit ORDER BY sha").fetchall()
        counters = conn.execute("SELECT sha, key, value FROM commit_counter ORDER BY sha, key").fetchall()
        identities = [r[0] for r in conn.execute("SELECT DISTINCT email FROM identity ORDER BY email")]
        provenance = conn.execute("SELECT commits_total, commits_sampled, added_lines_sampled, extractor FROM repo").fetchall()
    return dict(commits=commits, counters=counters, identities=identities, provenance=provenance)


def view(binary: Path, repo: Path, home: Path) -> dict:
    run([str(binary), "miner", "access", "grant", "--client", "persona-replay"], repo, home)
    requests = [
        dict(jsonrpc="2.0", id=0, method="initialize", params=dict(
            protocolVersion="2025-11-25", capabilities={},
            clientInfo=dict(name="persona-replay", version="1"))),
        dict(jsonrpc="2.0", method="notifications/initialized"),
        dict(jsonrpc="2.0", id=1, method="tools/call", params=dict(
            name="mmcg_profile", arguments=dict(paths=[], role="auditor",
                                                workflow="verified", budget_tokens=8000))),
    ]
    output = run([str(binary), "serve"], repo, home,
                 b"\n".join(encoded(r) for r in requests) + b"\n")
    response = next(json.loads(line) for line in output.splitlines()
                    if json.loads(line).get("id") == 1)
    if "error" in response or response["result"].get("isError"):
        raise ValueError("profile MCP request failed")
    return json.loads(response["result"]["content"][0]["text"])


def corpus(source: Path, output: Path, home: Path, anchor: str,
           frozen_manifest: Path | None = None) -> tuple[Path, list[dict], dict]:
    if frozen_manifest:
        frozen = json.loads(frozen_manifest.read_text())
        if frozen["source"] != str(source):
            raise ValueError("frozen manifest belongs to a different source")
        refs, head, anchor_sha = frozen["refs"], frozen["head"], frozen["anchor"]
    else:
        refs = git(source, home, "for-each-ref", "--format=%(refname) %(objectname)").decode().splitlines()
        head = git(source, home, "rev-parse", "HEAD^{commit}").decode().strip()
        anchor_sha = git(source, home, "rev-parse", "--verify", anchor + "^{commit}").decode().strip()
    for oid in [head, anchor_sha, *(line.split(" ", 1)[1] for line in refs)]:
        if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", oid):
            raise ValueError("manifest contains a non-OID history selector")
    # Only confirmed non-commit objects may be skipped. Missing/corrupt objects
    # and transport failures abort instead of silently shrinking the corpus.
    tips, skipped_refs = {head}, []
    for record in refs:
        name, oid = record.split(" ", 1)
        kind = git(source, home, "cat-file", "-t", oid).decode().strip()
        if kind == "tag":
            oid = git(source, home, "rev-parse", "--verify", oid + "^{}").decode().strip()
            kind = git(source, home, "cat-file", "-t", oid).decode().strip()
        if kind == "commit":
            tips.add(oid)
        elif kind in ("tree", "blob"):
            skipped_refs.append(name)
        else:
            raise ValueError(f"unexpected Git object type: {kind}")
    members = set(git(source, home, "rev-list", "--stdin",
                      stdin=("\n".join(sorted(tips)) + "\n").encode()).decode().splitlines())
    if anchor_sha not in members:
        raise ValueError("anchor must belong to the frozen corpus")
    common = Path(git(source, home, "rev-parse", "--path-format=absolute", "--git-common-dir").decode().strip())
    repo = output / "corpus.git"
    object_format = git(source, home, "rev-parse", "--show-object-format").decode().strip()
    git(output, home, "init", "--bare", "--object-format=" + object_format, str(repo))
    # Objects stay local and immutable; a missing source object fails the run.
    (repo / "objects/info/alternates").write_text(str(common / "objects") + "\n")
    tree = git(source, home, "rev-parse", anchor_sha + "^{tree}").decode().strip()
    args = ["-c", "user.name=Persona Replay", "-c", "user.email=replay@example.invalid",
            "commit-tree", tree]
    parents = sorted(tips | {anchor_sha})
    for tip in parents:
        args.extend(["-p", tip])
    synthetic = len(parents) > 1
    snapshot = (git(repo, home, *args, stdin=b"Local replay corpus union; excluded merge\n").decode().strip()
                if synthetic else parents[0])
    git(repo, home, "update-ref", "--no-deref", "HEAD", snapshot)
    excluded = {snapshot} if synthetic else set()
    observed_members = set(git(repo, home, "rev-list", "HEAD").decode().splitlines()) - excluded
    if observed_members != members:
        raise ValueError("synthetic union does not preserve the frozen commit set")
    rows = [r for r in metadata(repo, home, snapshot) if r["sha"] not in excluded]
    regular = [r for r in rows if len(r["parents"]) < 2]
    ancestry = git(repo, home, "rev-list", "--first-parent", anchor_sha).decode().splitlines()
    past = ancestry[len(ancestry) // 2]
    manifest = dict(source=str(source), refs=refs, head=head, anchor=anchor_sha,
                    snapshot=snapshot, synthetic_merge=synthetic,
                    temporal_seed=past, skipped_refs=skipped_refs,
                    commit_set_sha256=digest(encoded(sorted(members))),
                    commits=len(rows),
                    non_merge_commits=len(regular), merge_commits=len(rows)-len(regular),
                    raw_author_emails=len({r["email"] for r in regular}),
                    commits_with_coauthor_trailers=sum(r["coauthor_trailers"] > 0 for r in regular))
    if frozen_manifest and manifest["commit_set_sha256"] != frozen["commit_set_sha256"]:
        raise ValueError("frozen commit membership changed")
    return repo, rows, manifest


def replay(source: Path, binary: Path, output: Path, anchor: str,
           frozen_manifest: Path | None = None) -> dict:
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    home = output / "git-home"
    home.mkdir(mode=0o700)
    pinned_binary = output / "mmcg"
    shutil.copy2(binary, pinned_binary)
    binary = pinned_binary
    repo, rows, manifest = corpus(source, output, home, anchor, frozen_manifest)
    manifest["binary_sha256"] = digest(binary.read_bytes())
    (output / "manifest.json").write_bytes(encoded(manifest))
    (output / "commit-metadata.json").write_bytes(encoded(rows))
    regular = [r for r in rows if len(r["parents"]) < 2]
    snapshot, past = manifest["snapshot"], manifest["temporal_seed"]
    report = dict(manifest=manifest, identities=[], failures=[], limitations=[
        "Git identities and coauthor trailers do not prove human ownership or absence of AI assistance.",
        "The union includes local and remote refs, including unmerged work; merges are excluded by the miner.",
        "The anchor tree supplies the miner's current tooling context for the whole historical union.",
        "No human habits, attribution accuracy or statistical population coverage are certified.",
    ])
    for email in sorted({r["email"] for r in regular}):
        identity_id = digest(email.encode())[:16]
        person_home = output / "identities" / identity_id
        person_home.mkdir(mode=0o700, parents=True)
        expected = {r["sha"] for r in regular if r["email"] == email}
        entry = dict(id=identity_id, email=email,
                     names=sorted({r["name"] for r in regular if r["email"] == email}),
                     expected_commits=len(expected))
        try:
            command = [str(binary), "miner", "profile", str(repo), "--author", f"<{email}>"]
            run(command, repo, person_home)
            first = store_snapshot(person_home)
            run(command, repo, person_home)
            second = store_snapshot(person_home)
            profile = view(binary, repo, person_home)
            temporal_home = output / "temporal" / identity_id
            temporal_home.mkdir(mode=0o700, parents=True)
            git(repo, home, "update-ref", "--no-deref", "HEAD", past)
            try:
                run(command, repo, temporal_home)
                seed = (store_snapshot(temporal_home)
                        if (temporal_home / ".mastermind/style.db").is_file()
                        else dict(commits=[], counters=[]))
            finally:
                git(repo, home, "update-ref", "--no-deref", "HEAD", snapshot)
            run(command, repo, temporal_home)
            temporal = store_snapshot(temporal_home)
            measured = {sha for sha, key, value in first["counters"] if key == "diff.sampled" and value > 0}
            seed_measured = {sha for sha, key, value in seed["counters"] if key == "diff.sampled" and value > 0}
            shared_measurements = seed_measured & measured
            same_context = bool(seed_measured) and (
                [r[3] for r in seed["provenance"]] == [r[3] for r in first["provenance"]])
            temporal_coverage = dict(seed_listed=len(seed["commits"]), seed_measured=len(seed_measured),
                                     measured_intersection=len(shared_measurements),
                                     cache_context_equal=same_context,
                                     reusable_measured_intersection=len(shared_measurements) if same_context else 0,
                                     status="exercised" if shared_measurements else "not_exercised")
            observed = {sha for sha, _ in first["commits"]}
            checks = dict(exact_selection=observed == expected if len(expected) <= 2000 else observed <= expected and len(observed) == 2000,
                          exact_identity=first["identities"] == [email],
                          repeat_invariant=first == second,
                          cold_equals_temporal=first == temporal,
                          diff_count=profile["evidence"]["diff_sampled"] == len(measured),
                          no_personal_claims=not profile["feedback"] and not profile["habits"])
            periods = {}
            for row in regular:
                if row["email"] != email:
                    continue
                period = periods.setdefault(row["date"][:7], dict(authored_nonmerge=0, listed=0, measured=0))
                period["authored_nonmerge"] += 1
                period["listed"] += row["sha"] in observed
                period["measured"] += row["sha"] in measured
            # Ambiguity is predicate-specific, not one global 'confidence'.
            counts = {}
            for sha, key, value in first["counters"]:
                counts.setdefault(sha, {})[key] = value
            ambiguities = {}
            for yes, no in [("indent.space", "indent.tab"), ("quotes.single", "quotes.double"),
                            ("brace.same", "brace.own"), ("decl.const", "decl.let"),
                            ("string.template", "string.concat")]:
                ambiguities[yes.split(".")[0]] = sum(c.get(yes, 0) == c.get(no, 0) > 0 for c in counts.values())
            entry.update(listed=len(observed), measured=len(measured), periods=periods,
                         temporal_coverage=temporal_coverage,
                         ambiguous_commits_by_predicate=ambiguities,
                         reported_diff_sampled=profile["evidence"]["diff_sampled"], checks=checks,
                         evidence_sha256=digest(encoded(first)))
            (person_home / "profile-view.json").write_bytes(encoded(profile))
            for check, passed in checks.items():
                if not passed:
                    report["failures"].append(dict(identity=identity_id, check=check))
        except (RuntimeError, ValueError, KeyError, sqlite3.Error) as error:
            entry["error"] = str(error)
            report["failures"].append(dict(identity=identity_id, check="replay_error"))
        report["identities"].append(entry)
        (output / "report.json").write_bytes(encoded(report))
        print(json.dumps({k: entry.get(k) for k in ("id", "expected_commits", "measured", "checks", "error")}), flush=True)
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--anchor", default="origin/main")
    parser.add_argument("--manifest", type=Path, help="Reuse the exact refs from an earlier local replay")
    args = parser.parse_args()
    report = replay(args.repo.resolve(), args.binary.resolve(), args.output.resolve(), args.anchor, args.manifest)
    print(json.dumps(dict(identities=len(report["identities"]), failures=len(report["failures"]))))
    return int(bool(report["failures"]))


if __name__ == "__main__":
    sys.exit(main())
