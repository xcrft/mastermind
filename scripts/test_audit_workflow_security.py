import copy
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import pathlib
import re
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import warnings
import zipfile
from unittest import mock

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


class EntrypointHomeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        text = (ROOT / "scripts/audit-action-entrypoint.sh").read_text(encoding="utf-8")
        start = text.index("prepare_private_home()")
        cls.function = text[start : text.index("workspace=", start)]

    def prepare(self, path):
        return subprocess.run(
            ["sh", "-c", self.function + '\nprepare_private_home "$1"', "test", str(path)],
            text=True,
            capture_output=True,
            check=False,
        )

    def test_existing_home_is_accepted_and_missing_home_is_private(self):
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            existing = root / "existing home"
            existing.mkdir()
            self.assertEqual(self.prepare(existing).returncode, 0)

            missing = root / "new home"
            result = self.prepare(missing)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue(missing.is_dir())
            self.assertEqual(stat.S_IMODE(missing.stat().st_mode), 0o700)

    @unittest.skipIf(os.name == "nt", "symlink semantics differ on Windows")
    def test_symlink_home_is_rejected(self):
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            target = root / "target"
            target.mkdir()
            link = root / "home-link"
            link.symlink_to(target, target_is_directory=True)
            self.assertNotEqual(self.prepare(link).returncode, 0)


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

    def test_action_outputs_are_repository_relative_and_consumed_by_a_real_job(self):
        action = yaml.safe_load((ROOT / "action.yml").read_text(encoding="utf-8"))
        for name in ("bundle-dir", "result-json"):
            self.assertIn("repository-relative", action["outputs"][name]["description"])

        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/validate.yml").read_text(encoding="utf-8")
        )
        job = workflow["jobs"].get("audit-action-self-test")
        self.assertIsNotNone(job, "validate.yml must execute and consume the local Action")
        steps = job["steps"]
        producer = next(step for step in steps if step.get("uses") == "./")
        self.assertEqual(producer.get("id"), "audit")
        consumer = next(step for step in steps if step.get("name") == "Consume repository-relative Action outputs")
        self.assertEqual(consumer["env"]["BUNDLE_DIR"], "${{ steps.audit.outputs.bundle-dir }}")
        self.assertEqual(consumer["env"]["RESULT_JSON"], "${{ steps.audit.outputs.result-json }}")
        self.assertIn('case "$BUNDLE_DIR" in /*)', consumer["run"])
        self.assertIn('case "$RESULT_JSON" in /*)', consumer["run"])
        self.assertIn('test -f "$RESULT_JSON"', consumer["run"])

    def test_existing_required_context_aggregates_contracts_and_real_action(self):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/validate.yml").read_text(encoding="utf-8")
        )
        required = next(
            job
            for job in workflow["jobs"].values()
            if job.get("name") == "Frontmatter & cross-references"
        )
        self.assertEqual(
            set(required.get("needs", [])),
            {"artifact-contracts", "audit-action-self-test"},
        )
        self.assertIn("always()", str(required.get("if", "")))
        script = "\n".join(step.get("run", "") for step in required["steps"])
        self.assertIn("needs.artifact-contracts.result", str(required))
        self.assertIn("needs.audit-action-self-test.result", str(required))
        self.assertGreaterEqual(script.count('= "success"'), 2)

    def test_action_output_path_conversion_is_behaviorally_covered(self):
        text = (ROOT / "scripts/audit-action-entrypoint.sh").read_text(encoding="utf-8")
        start = text.index("workspace_relative_path()")
        functions = text[start : text.index("workspace=", start)]

        def convert(workspace, path):
            result = subprocess.run(
                ["sh", "-c", functions + '\nworkspace_relative_path "$1" "$2"', "test", workspace, path],
                text=True,
                capture_output=True,
                check=False,
            )
            return result.returncode, result.stdout.strip()

        self.assertEqual(convert("/github/workspace", "/github/workspace/.mastermind/out"), (0, ".mastermind/out"))
        self.assertEqual(convert("/github/workspace", "/github/workspace/sub dir/out"), (0, "sub dir/out"))
        self.assertNotEqual(convert("/github/workspace", "/tmp/out")[0], 0)

    def test_native_npm_smoke_has_a_documented_local_recipe(self):
        justfile = (ROOT / "justfile").read_text(encoding="utf-8")
        recipe = justfile[justfile.index("npm-smoke-native:") :]
        for token in (
            "cargo build --release",
            "build-npm-packages.sh",
            "npm pack",
            "npm install --no-save",
            "node_modules/.bin/mastermind --version",
        ):
            self.assertIn(token, recipe)
        contributing = (ROOT / "CONTRIBUTING.md").read_text(encoding="utf-8")
        self.assertIn("just npm-smoke-native", contributing)

    def test_npm_publish_job_uses_resumable_root_last_helper(self):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/publish-npm.yml").read_text(encoding="utf-8")
        )
        publish_steps = workflow["jobs"]["publish"]["steps"]
        publish = next(step for step in publish_steps if step.get("name") == "Publish or verify all 8 packages")
        self.assertIn("scripts/publish-npm-tarballs.sh", publish["run"])
        self.assertNotIn("npm publish", publish["run"])

    def test_crate_publish_uploads_the_verified_artifact_without_repackaging(self):
        workflow = yaml.safe_load(
            (ROOT / ".github/workflows/publish-mmcg.yml").read_text(encoding="utf-8")
        )
        verify_runs = "\n".join(
            step.get("run", "") for step in workflow["jobs"]["verify"]["steps"]
        )
        publish_runs = "\n".join(
            step.get("run", "") for step in workflow["jobs"]["publish"]["steps"]
        )
        self.assertIn("publish-crate-artifact.py prepare", verify_runs)
        self.assertIn("sha256sum", verify_runs)
        self.assertIn("sha256sum --check", publish_runs)
        self.assertRegex(publish_runs, r'publish-crate-artifact\.py"? publish')
        self.assertNotRegex(publish_runs, r"\bcargo\s+publish\b")

    def test_distributed_package_surfaces_advertise_vue(self):
        missing = validator.distributed_vue_metadata_errors()
        self.assertEqual(missing, [])

        surfaces = validator.distributed_vue_metadata_contents()
        for relative, marker in validator.DISTRIBUTED_VUE_MARKERS.items():
            with self.subTest(path=relative):
                changed = dict(surfaces)
                self.assertIn(marker, changed[relative])
                changed[relative] = changed[relative].replace(marker, "")
                self.assertIn(relative, validator.distributed_vue_metadata_errors(changed))

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
            "--prevent-self-review",
            "prevent_self_review=false",
            "eligible reviewer different from the workflow initiator",
            "wrapper check + linux-x64 install smoke",
            "advisories, licenses, bans, sources",
            "viewerPermission",
            "--apply",
        ):
            self.assertIn(token, text)
        self.assertIn('current_actor=$(gh api user --jq .login)', text)
        self.assertIn('"$reviewer" = "$current_actor"', text)
        self.assertNotEqual(stat.S_IMODE(path.stat().st_mode) & stat.S_IXUSR, 0)

    def test_self_review_prevention_rejects_the_authenticated_reviewer(self):
        script = ROOT / "scripts/configure-github-protections.sh"
        with tempfile.TemporaryDirectory() as raw:
            fake_bin = pathlib.Path(raw)
            gh = fake_bin / "gh"
            gh.write_text(
                "#!/bin/sh\n"
                "if [ \"$1\" = repo ]; then printf 'ADMIN\\n'; exit 0; fi\n"
                "if [ \"$1\" = api ] && [ \"$2\" = user ]; then printf 'aglumova\\n'; exit 0; fi\n"
                "exit 99\n",
                encoding="utf-8",
            )
            gh.chmod(0o755)
            result = subprocess.run(
                [
                    str(script),
                    "--repository",
                    "xcrft/mastermind",
                    "--reviewer",
                    "aglumova",
                    "--prevent-self-review",
                    "--apply",
                ],
                env={**os.environ, "PATH": f"{fake_bin}{os.pathsep}{os.environ['PATH']}"},
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("different from the workflow initiator", result.stderr)


class CratesIoArtifactPublisherTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.path = ROOT / "scripts/publish-crate-artifact.py"
        if not cls.path.is_file():
            raise AssertionError("missing exact-artifact crates.io publisher")
        spec = importlib.util.spec_from_file_location("publish_crate_artifact", cls.path)
        cls.module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(cls.module)

    def test_publish_body_contains_the_exact_verified_crate_bytes(self):
        self.assertEqual(
            self.module.CRATES_IO_PUBLISH_URL,
            "https://crates.io/api/v1/crates/new",
        )
        cargo_manifest = (ROOT / "mcp/servers/mmcg/Cargo.toml").read_text(encoding="utf-8")
        self.assertIn('publish = ["crates-io"]', cargo_manifest)
        metadata = {"name": "mmcg", "vers": "1.2.1", "deps": [], "features": {}}
        crate = b"verified-crate-bytes\x00\xff"
        body = self.module.build_publish_body(metadata, crate)
        metadata_len = struct.unpack("<I", body[:4])[0]
        encoded = body[4 : 4 + metadata_len]
        crate_len_at = 4 + metadata_len
        crate_len = struct.unpack("<I", body[crate_len_at : crate_len_at + 4])[0]
        uploaded = body[crate_len_at + 4 :]
        self.assertEqual(json.loads(encoded), metadata)
        self.assertEqual(crate_len, len(crate))
        self.assertEqual(uploaded, crate)

    def test_crate_binding_rejects_metadata_for_a_different_version(self):
        with tempfile.TemporaryDirectory() as raw:
            crate_path = pathlib.Path(raw) / "mmcg-1.2.1.crate"
            with tarfile.open(crate_path, "w:gz") as archive:
                cargo = b'[package]\nname = "mmcg"\nversion = "1.2.1"\n'
                info = tarfile.TarInfo("mmcg-1.2.1/Cargo.toml")
                info.size = len(cargo)
                archive.addfile(info, io.BytesIO(cargo))
            with self.assertRaisesRegex(ValueError, "metadata.*crate"):
                self.module.validate_crate_binding(
                    crate_path,
                    {"name": "mmcg", "vers": "9.9.9"},
                )

    def test_publish_checks_sha_and_puts_the_exact_crate_bytes(self):
        class Response:
            def __enter__(self):
                return self

            def __exit__(self, *_):
                return False

            def read(self, _limit):
                return b"{}"

        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            crate_path = root / "mmcg-1.2.1.crate"
            with tarfile.open(crate_path, "w:gz") as archive:
                cargo = b'[package]\nname = "mmcg"\nversion = "1.2.1"\n'
                info = tarfile.TarInfo("mmcg-1.2.1/Cargo.toml")
                info.size = len(cargo)
                archive.addfile(info, io.BytesIO(cargo))
            crate_bytes = crate_path.read_bytes()
            metadata_path = root / "publish.json"
            metadata_path.write_text(
                json.dumps({"name": "mmcg", "vers": "1.2.1"}),
                encoding="utf-8",
            )
            expected = hashlib.sha256(crate_bytes).hexdigest()
            captured = []

            def urlopen(request, timeout):
                captured.append((request, timeout))
                return Response()

            with mock.patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "secret-token"}), mock.patch.object(
                self.module.urllib.request, "urlopen", side_effect=urlopen
            ):
                with contextlib.redirect_stdout(io.StringIO()):
                    self.module.publish(crate_path, metadata_path, expected)

            self.assertEqual(len(captured), 1)
            request, timeout = captured[0]
            self.assertEqual(request.full_url, self.module.CRATES_IO_PUBLISH_URL)
            self.assertEqual(request.method, "PUT")
            self.assertEqual(timeout, 120)
            self.assertEqual(request.get_header("Authorization"), "secret-token")
            self.assertTrue(request.data.endswith(crate_bytes))

            with mock.patch.object(self.module.urllib.request, "urlopen") as unopened:
                with self.assertRaisesRegex(ValueError, "SHA-256"):
                    self.module.publish(crate_path, metadata_path, "0" * 64)
                unopened.assert_not_called()


if __name__ == "__main__":
    unittest.main()
