import copy
import hashlib
import json
import os
import pathlib
import re
import stat
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile

import yaml

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import validate as validator
from validate import audit_pr_contract_errors, audit_publication_contract_errors


ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOW_PATH = ROOT / "docs/examples/mastermind-audit-publish.yml"
PR_WORKFLOW_PATH = ROOT / "docs/examples/mastermind-audit-pr.yml"


def load_workflow():
    text = WORKFLOW_PATH.read_text(encoding="utf-8")
    return text, yaml.safe_load(text)


def load_pr_workflow():
    text = PR_WORKFLOW_PATH.read_text(encoding="utf-8")
    return text, yaml.safe_load(text)


def step_script(workflow, job, name):
    for step in workflow["jobs"][job]["steps"]:
        if step.get("name") == name:
            return step["run"]
    raise AssertionError(f"missing step: {job}/{name}")


def python_body(script):
    matches = re.findall(r"python3 - <<'PY'\n(.*?)\nPY", script, flags=re.DOTALL)
    if len(matches) != 1:
        raise AssertionError("expected exactly one Python heredoc")
    return matches[0]


def write_zip(path, members):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", UserWarning)
        with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, data, mode, extra in members:
                info = zipfile.ZipInfo(name)
                info.create_system = 3
                info.external_attr = mode << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                info.extra = extra
                archive.writestr(info, data)


class WorkflowContractTests(unittest.TestCase):
    def setUp(self):
        self.text, self.workflow = load_workflow()
        self.pr_text, self.pr_workflow = load_pr_workflow()

    def assert_mutation_rejected(self, text=None, workflow=None):
        errors = audit_publication_contract_errors(
            self.text if text is None else text,
            self.workflow if workflow is None else workflow,
        )
        self.assertTrue(errors)

    def assert_pr_mutation_rejected(self, text=None, workflow=None):
        errors = audit_pr_contract_errors(
            self.pr_text if text is None else text,
            self.pr_workflow if workflow is None else workflow,
        )
        self.assertTrue(errors)

    def test_current_workflow_contract_passes(self):
        self.assertEqual(audit_publication_contract_errors(self.text, self.workflow), [])
        self.assertEqual(audit_pr_contract_errors(self.pr_text, self.pr_workflow), [])

    def test_attempt_specific_artifact_identity_is_mandatory(self):
        changed = copy.deepcopy(self.pr_workflow)
        changed["jobs"]["audit"]["steps"][-1]["with"]["name"] = "mastermind-pr-audit"
        self.assert_pr_mutation_rejected(workflow=changed)
        changed = copy.deepcopy(self.pr_workflow)
        changed["jobs"]["audit"]["steps"][-1]["with"]["name"] = "mastermind-pr-audit-attempt-${{ github.run_number }}"
        self.assert_pr_mutation_rejected(workflow=changed)
        self.assert_mutation_rejected(
            text=self.text.replace("artifacts?name=$expected_artifact_name", "artifacts", 1)
        )
        self.assert_mutation_rejected(
            text=self.text.replace(
                '| if length == 1 then .[0] else error("exactly one attempt-specific artifact required") end',
                '| if length >= 1 then .[0] else error("attempt-specific artifact required") end',
                1,
            )
        )
        self.assert_mutation_rejected(
            text=self.text.replace(
                'expected_artifact_name="mastermind-pr-audit-attempt-$source_run_attempt"',
                'expected_artifact_name="mastermind-pr-audit"\nartifact_created=now\nrun_started=then',
                1,
            )
        )
        self.assert_mutation_rejected(
            text=self.text.replace(
                'expected_artifact_name="mastermind-pr-audit-attempt-$source_run_attempt"',
                'expected_artifact_name="mastermind-pr-audit-attempt-$SOURCE_RUN_ID"',
                1,
            )
        )

    def test_wrong_attempt_id_digest_and_size_are_rejected(self):
        self.assert_mutation_rejected(text=self.text.replace("?attempt_number=$SOURCE_RUN_ATTEMPT", "", 1))
        self.assert_mutation_rejected(text=self.text.replace("actions/artifacts/$ARTIFACT_ID/zip", "actions/artifacts/123/zip", 1))
        self.assert_mutation_rejected(text=self.text.replace('test "$actual" = "$ARTIFACT_DIGEST"', "true", 1))
        self.assert_mutation_rejected(text=self.text.replace("wc -c <", "printf 0 <", 1))
        self.assert_mutation_rejected(text=self.text.replace('test "$size" = "$ARTIFACT_SIZE"', "true", 1))
        self.assert_mutation_rejected(text=self.text.replace('test "$(jq -r .id <<<"$server")" = "$ARTIFACT_ID"', "true", 1))

    def test_archive_and_redownload_guards_are_required(self):
        self.assert_mutation_rejected(text=self.text.replace("name in seen", "False", 1))
        self.assert_mutation_rejected(text=self.text.replace("not stat.S_ISREG(mode)", "False", 1))
        self.assert_mutation_rejected(text=self.text.replace("info.extra", "False", 1))
        self.assert_mutation_rejected(text=self.text.replace("16 * 1024 * 1024", "2**63", 1))
        index = self.text.rfind('test "$actual" = "$ARTIFACT_DIGEST"')
        self.assertGreaterEqual(index, 0)
        self.assert_mutation_rejected(text=self.text[:index] + "true" + self.text[index + len('test "$actual" = "$ARTIFACT_DIGEST"'):])

    def test_attestation_publish_and_verifier_guards_are_required(self):
        changed = copy.deepcopy(self.workflow)
        changed["jobs"]["attest"]["steps"][-1]["with"]["subject-path"] = "unverified.tar"
        self.assert_mutation_rejected(workflow=changed)
        changed = copy.deepcopy(self.workflow)
        changed["jobs"]["publish"]["needs"] = ["verify"]
        self.assert_mutation_rejected(workflow=changed)
        self.assert_mutation_rejected(text=self.text + "\n# REPLACE_WITH_ALLOWED_VERIFIER\n")


class ArchiveReaderTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        _, workflow = load_workflow()
        cls.readers = [
            (
                python_body(step_script(workflow, "verify", "Download exact source artifact ID and hard-check server digest")),
                "source-download/source.zip",
                "source",
                [("result.json", b"{}", stat.S_IFREG | 0o600, b"")],
            ),
            (
                python_body(step_script(workflow, "attest", "Download exact verified artifact bytes and safely extract")),
                "verified-subject.zip",
                "verified-subject",
                [
                    ("verified.tar", b"tar", stat.S_IFREG | 0o600, b""),
                    ("verified-statement.json", b"{}", stat.S_IFREG | 0o600, b""),
                ],
            ),
            (
                python_body(step_script(workflow, "publish", "Download exact verified bytes, safely extract, and post one bot-owned marker comment")),
                "verified-subject.zip",
                "verified-subject",
                [
                    ("verified.tar", b"tar", stat.S_IFREG | 0o600, b""),
                    ("verified-statement.json", b"{}", stat.S_IFREG | 0o600, b""),
                ],
            ),
        ]

    def run_reader(self, reader, archive_name, target_name, members):
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            archive_path = root / archive_name
            archive_path.parent.mkdir(parents=True, exist_ok=True)
            target = root / target_name
            target.mkdir(parents=True, exist_ok=True)
            write_zip(archive_path, members)
            result = subprocess.run(
                [sys.executable, "-c", reader],
                cwd=root,
                text=True,
                capture_output=True,
                check=False,
            )
            materialized = sorted(path.name for path in target.iterdir())
            return result, materialized

    def test_valid_regular_archives_pass(self):
        for reader, archive_name, target_name, valid in self.readers:
            with self.subTest(target=target_name):
                result, materialized = self.run_reader(reader, archive_name, target_name, valid)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(materialized, sorted(member[0] for member in valid))

    def test_duplicate_link_device_fifo_hardlink_and_bomb_fail_before_materialization(self):
        special_modes = [stat.S_IFLNK, stat.S_IFCHR, stat.S_IFBLK, stat.S_IFIFO, stat.S_IFSOCK]
        for reader, archive_name, target_name, valid in self.readers:
            name = valid[0][0]
            cases = [valid + [(name, b"duplicate", stat.S_IFREG | 0o600, b"")]]
            cases.extend(
                [(name, b"target", mode | 0o600, b"")] + valid[1:]
                for mode in special_modes
            )
            cases.append([(name, b"hardlink", stat.S_IFREG | 0o600, b"nu\x00\x00")] + valid[1:])
            cases.append([(name, b"0" * (17 * 1024 * 1024), stat.S_IFREG | 0o600, b"")] + valid[1:])
            for index, members in enumerate(cases):
                with self.subTest(target=target_name, case=index):
                    result, materialized = self.run_reader(reader, archive_name, target_name, members)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(materialized, [])


class InlineVerifierSchemaTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        _, workflow = load_workflow()
        cls.verifier = python_body(
            step_script(workflow, "verify", "Verify source claims and envelopes with workflow-bound implementation")
        )

    def envelope(self):
        manifest = {
            "repository": {
                "identity": "owner/repo",
                "baseline_oid": "1" * 40,
                "head_oid": "2" * 40,
                "worktree_clean": True,
            },
            "inputs": {
                "spec_path": "spec.md",
                "spec_sha256": "sha256:" + "3" * 64,
                "executor_report_path": None,
                "executor_report_present": False,
                "executor_report_sha256": None,
            },
            "diff": {
                "name_status": [{"status": "M", "path": "src/lib.py", "old_path": None}],
                "binary_diff_sha256": "sha256:" + "4" * 64,
            },
            "tool": {"name": "mastermind", "version": "0.37.0", "bundle_schema": 3},
            "audit_configuration": {"baseline_input": "main", "require_clean_worktree": True},
            "index_metadata": {"source": "mmcg"},
            "verdict": "drift",
            "declared_files": ["src/lib.py"],
            "changed_files": ["src/lib.py"],
            "verified_claims": [],
            "failed_claims": [],
            "discrepancies": [{"kind": "planned_test_not_added", "test": "test_change"}],
            "snapshot_drift": [
                {"kind": "snapshot_caller_drift", "symbol": "run", "spec_says": 1, "index_says": 2}
            ],
            "snapshot_changed": False,
            "mmcg_queries": [],
            "verify_commands": ["cargo test"],
            "human_summary": "drift",
        }
        envelope = {
            "schema_version": 3,
            "manifest": manifest,
            "integrity": {
                "algorithm": "sha256",
                "canonicalization": "mastermind-cjson-v1",
                "manifest_digest": "",
            },
        }
        return self.reseal(envelope)

    def reseal(self, envelope):
        canonical = json.dumps(
            envelope["manifest"], ensure_ascii=False, sort_keys=True, separators=(",", ":")
        ).encode()
        envelope["integrity"]["manifest_digest"] = "sha256:" + hashlib.sha256(canonical).hexdigest()
        return envelope

    def run_verifier(self, value=None, raw=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            source = root / "source"
            source.mkdir()
            claims = {
                "run-id": "100",
                "run-attempt": "2",
                "pr-number": "7",
                "base-sha": "1" * 40,
                "head-sha": "2" * 40,
                "workflow-sha": "5" * 40,
                "workflow-ref": "owner/repo/.github/workflows/mastermind-audit-pr.yml@refs/heads/main",
            }
            for name, claim in claims.items():
                (source / name).write_text(claim, encoding="utf-8")
            if raw is None:
                raw = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
            (source / "audit.bundle.json").write_text(raw, encoding="utf-8")
            output = root / "output"
            env = os.environ.copy()
            env.update(
                {
                    "EXPECTED_REPOSITORY": "owner/repo",
                    "EXPECTED_RUN_ID": "100",
                    "EXPECTED_RUN_ATTEMPT": "2",
                    "EXPECTED_PR": "7",
                    "EXPECTED_BASE": "1" * 40,
                    "EXPECTED_HEAD": "2" * 40,
                    "EXPECTED_WORKFLOW_SHA": "5" * 40,
                    "EXPECTED_WORKFLOW_REF": claims["workflow-ref"],
                    "GITHUB_OUTPUT": str(output),
                }
            )
            result = subprocess.run(
                [sys.executable, "-c", self.verifier],
                cwd=root,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            return result, output.read_text() if output.exists() else "", (root / "verified").exists()

    def assert_rejected(self, envelope):
        if type(envelope.get("manifest")) is dict and type(envelope.get("integrity")) is dict:
            self.reseal(envelope)
        result, output, verified = self.run_verifier(value=envelope)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, "")
        self.assertFalse(verified)

    def test_exact_schema_valid_envelope_passes(self):
        result, output, verified = self.run_verifier(value=self.envelope())
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, "verifier-identity=mastermind-inline-schema-v3-verifier-v1\n")
        self.assertFalse(verified)

    def test_every_nested_family_rejects_added_deleted_and_wrong_type(self):
        family_paths = [
            ("manifest",),
            ("manifest", "repository"),
            ("manifest", "inputs"),
            ("manifest", "diff"),
            ("manifest", "diff", "name_status", 0),
            ("manifest", "tool"),
            ("manifest", "audit_configuration"),
            ("manifest", "index_metadata"),
            ("manifest", "discrepancies", 0),
            ("manifest", "snapshot_drift", 0),
            ("integrity",),
        ]
        for path in family_paths:
            with self.subTest(path=path, mutation="add"):
                value = self.envelope()
                target = value
                for component in path:
                    target = target[component]
                target["unknown"] = True
                self.assert_rejected(value)
            with self.subTest(path=path, mutation="delete"):
                value = self.envelope()
                target = value
                for component in path:
                    target = target[component]
                target.pop(next(iter(target)))
                self.assert_rejected(value)
            with self.subTest(path=path, mutation="wrong-type"):
                value = self.envelope()
                parent = value
                for component in path[:-1]:
                    parent = parent[component]
                parent[path[-1]] = []
                self.assert_rejected(value)

    def test_ranges_paths_digests_oids_order_floats_booleans_and_duplicates_reject(self):
        mutations = []
        value = self.envelope(); value["schema_version"] = True; mutations.append(value)
        value = self.envelope(); value["manifest"]["tool"]["bundle_schema"] = True; mutations.append(value)
        value = self.envelope(); value["manifest"]["repository"]["baseline_oid"] = "1" * 39; mutations.append(value)
        value = self.envelope(); value["manifest"]["inputs"]["spec_sha256"] = "sha256:bad"; mutations.append(value)
        value = self.envelope(); value["manifest"]["inputs"]["spec_path"] = "../spec.md"; mutations.append(value)
        value = self.envelope(); value["manifest"]["discrepancies"][0] = {"kind": "observed_exit_code_non_zero", "cmd": "x", "exit_code": 2147483648}; mutations.append(value)
        value = self.envelope(); value["manifest"]["declared_files"] = ["z", "a"]; mutations.append(value)
        value = self.envelope(); value["manifest"]["diff"]["name_status"] = list(reversed(value["manifest"]["diff"]["name_status"] * 2)); mutations.append(value)
        value = self.envelope(); value["manifest"]["snapshot_changed"] = True; mutations.append(value)
        value = self.envelope(); value["manifest"]["discrepancies"][0]["test"] = 3.5; mutations.append(value)
        for index, value in enumerate(mutations):
            with self.subTest(index=index):
                self.assert_rejected(value)
        value = self.envelope()
        raw = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        raw = raw.replace('"identity":"owner/repo"', '"identity":"owner/repo","identity":"owner/repo"', 1)
        result, output, verified = self.run_verifier(raw=raw)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, "")
        self.assertFalse(verified)


class EntrypointPathGrammarTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        text = (ROOT / "scripts/audit-action-entrypoint.sh").read_text(encoding="utf-8")
        cls.functions = text[text.index("relative_path()") : text.index("workspace=")]

    def check(self, function, value):
        script = self.functions + f'\n{function} "$1"'
        return subprocess.run(
            ["sh", "-c", script, "test", value],
            env={**os.environ, "LC_ALL": "C"},
            check=False,
        ).returncode == 0

    def test_root_dot_and_safe_paths_pass(self):
        self.assertTrue(self.check("root_path", "."))
        self.assertTrue(self.check("root_path", "safe/root"))
        self.assertTrue(self.check("relative_path", "safe/output"))

    def test_unsafe_root_and_bundle_paths_fail(self):
        unsafe = ["", "/absolute", ".", "..", "a//b", "/a", "a/", "./a", "a/./b", "a/../b", "a\\b", "a\tb", "a\nb", "a\x7fb"]
        for value in unsafe:
            with self.subTest(value=repr(value)):
                self.assertFalse(self.check("relative_path", value))
        for value in unsafe:
            if value != ".":
                with self.subTest(root=repr(value)):
                    self.assertFalse(self.check("root_path", value))


class RepositoryDeliveryContractTests(unittest.TestCase):
    def test_docker_action_entrypoint_is_executable(self):
        entrypoint = ROOT / "scripts/audit-action-entrypoint.sh"
        mode = stat.S_IMODE(entrypoint.stat().st_mode)
        self.assertNotEqual(mode & stat.S_IXUSR, 0, f"entrypoint mode is {mode:o}")

    def test_docker_action_uses_default_root_user_for_workspace_access(self):
        dockerfile = (ROOT / "Dockerfile.audit-action").read_text(encoding="utf-8")
        self.assertIn("COPY mcp/servers/mmcg/benches ./mcp/servers/mmcg/benches", dockerfile)
        self.assertIsNone(
            re.search(r"^USER\s+", dockerfile, flags=re.MULTILINE),
            "GitHub Docker Actions must use the default root user for GITHUB_WORKSPACE",
        )

    def test_docker_from_detector_fails_closed_on_every_stage_form(self):
        dockerfile = (ROOT / "Dockerfile.audit-action").read_text(encoding="utf-8")
        approved = validator.dockerfile_from_images(dockerfile)
        self.assertEqual(len(approved), 2)
        self.assertEqual(
            validator.dockerfile_from_images(dockerfile.replace("FROM ", "from ", 1)),
            approved,
            "Docker instruction names are case-insensitive",
        )

        unapproved = "debian:bookworm@sha256:" + "0" * 64
        changed_digest = approved[1][:-1] + ("0" if approved[1][-1] != "0" else "1")
        missing_digest = approved[1].split("@", 1)[0]
        mutations = {
            "changed digest": dockerfile.replace(approved[1], changed_digest, 1),
            "missing digest": dockerfile.replace(approved[1], missing_digest, 1),
            "third lowercase stage": dockerfile + f"\nfrom {unapproved} AS escape\n",
            "third mixed-case indented stage": dockerfile + f"\n  FrOm {unapproved} AS escape\n",
            "third platform stage": dockerfile
            + f"\n\tFROM --platform=linux/amd64 {unapproved} AS escape\n",
            "duplicate approved stage": dockerfile + f"\nFROM {approved[0]} AS duplicate\n",
            "bare stage": dockerfile + "\nFROM\n",
            "bare mixed-case indented stage": dockerfile + "\n  FrOm\n",
            "incomplete platform stage": dockerfile + "\nFROM --platform=linux/amd64\n",
        }
        for name, mutated in mutations.items():
            with self.subTest(mutation=name):
                self.assertNotEqual(validator.dockerfile_from_images(mutated), approved)

    def test_required_workflows_are_not_suppressed_by_path_filters(self):
        workflows = [
            ROOT / ".github/workflows/ci-mmcg.yml",
            ROOT / ".github/workflows/ci-npm.yml",
            ROOT / ".github/workflows/supply-chain.yml",
            ROOT / ".github/workflows/validate.yml",
        ]
        for path in workflows:
            with self.subTest(path=path.name):
                workflow = yaml.load(path.read_text(encoding="utf-8"), Loader=yaml.BaseLoader)
                self.assertFalse(
                    validator.required_workflow_has_path_filter(workflow),
                    f"{path.name} can strand a required status by filtering pull_request paths",
                )

    def test_validator_workflow_smokes_the_docker_action_image(self):
        workflow = (ROOT / ".github/workflows/validate.yml").read_text(encoding="utf-8")
        self.assertIn("docker build", workflow)
        self.assertIn("test \"$(id -u)\" = 0", workflow)
        self.assertIn("test -x /usr/local/bin/audit-action-entrypoint", workflow)
        self.assertIn("touch /github/workspace/.mastermind-action-write-smoke", workflow)

    def test_required_workflow_filter_detector_rejects_both_filter_forms(self):
        detector = getattr(validator, "required_workflow_has_path_filter", None)
        self.assertIsNotNone(detector, "validator needs a semantic required-workflow filter guard")
        for key in ("paths", "paths-ignore"):
            with self.subTest(filter=key):
                workflow = {"on": {"pull_request": {key: ["docs/**"]}}}
                self.assertTrue(detector(workflow))

    def test_release_workflows_require_tag_commit_on_main(self):
        for name in ["publish-mmcg.yml", "publish-npm.yml"]:
            with self.subTest(workflow=name):
                text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
                self.assertIn("fetch-depth: 0", text)
                self.assertIn("merge-base --is-ancestor", text)

    def test_validator_dependency_is_hash_locked_everywhere(self):
        requirements = (ROOT / "scripts/requirements.txt").read_text(encoding="utf-8")
        self.assertRegex(requirements, r"(?m)^PyYAML==[0-9]+\.[0-9]+\.[0-9]+")
        self.assertIn("--hash=sha256:", requirements)
        installers = [
            ROOT / "justfile",
            ROOT / ".github/workflows/validate.yml",
            ROOT / ".github/workflows/publish-mmcg.yml",
        ]
        for path in installers:
            with self.subTest(path=path.name):
                self.assertIn(
                    "--require-hashes -r scripts/requirements.txt",
                    path.read_text(encoding="utf-8"),
                )

    def test_admin_protection_script_covers_live_release_controls(self):
        path = ROOT / "scripts/configure-github-protections.sh"
        self.assertTrue(path.is_file(), "missing reproducible GitHub protection setup")
        text = path.read_text(encoding="utf-8")
        for token in (
            "npm-prod",
            "npm-v*",
            "required_reviewers",
            "wrapper check + linux-x64 install smoke",
            "advisories, licenses, bans, sources",
            "viewerPermission",
            "--apply",
        ):
            self.assertIn(token, text)
        self.assertNotEqual(stat.S_IMODE(path.stat().st_mode) & stat.S_IXUSR, 0)


if __name__ == "__main__":
    unittest.main()
