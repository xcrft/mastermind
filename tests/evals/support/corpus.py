"""Synthetic public task and private review key over a disposable repository."""

import copy
import subprocess
import sys

from .benchmark import BenchmarkFixture
from evals.benchmark import artifacts as artifact_io
from evals.benchmark import corpus


class CorpusFixture:
    def __init__(self):
        self.fixture = BenchmarkFixture()
        self.root = self.fixture.root / "corpus"
        (self.root / "tasks").mkdir(parents=True)
        (self.root / "rubrics").mkdir()
        self.path = self.root / "corpus.json"
        self.task = copy.deepcopy(self.fixture.task)
        self.key = {"task_id": self.task["id"], "source_revision": self.task["revision"],
                    "status": "calibration_key_source_reviewed",
                    "required_knowns": [{"claim": "The return value is fixed.", "anchors": ["src/service.py:1-2"]}],
                    "expected_unknowns": ["Whether callers rely on the value."],
                    "materially_false_claims": ["The function performs network I/O."],
                    "reviewer_notes": ["FIXTURE_PRIVATE_KEY_CANARY"],
                    "review_dimensions": ["claim_to_source_support", "critical_evidence_coverage",
                                          "material_false_conclusions", "appropriate_unknowns"],
                    "decision_quality": "not_applicable_to_facts_only_research_task",
                    "automatic_quality_score": None}
        self.entry = {"id": self.task["id"], "role": "calibration", "coverage": ["fixed_return"],
                      "task": "tasks/service-01.json", "rubric": "rubrics/service-01.json",
                      "indexed_files": ["src/service.py"]}
        self.registry = {"kind": "mastermind-research-corpus", "schema_version": 1, "cases": [self.entry]}
        self.save()


    def save(self):
        for name, value in (("corpus.json", self.registry), (self.entry["task"], self.task), (self.entry["rubric"], self.key)):
            path = self.root / name
            path.write_bytes(artifact_io.canonical(value))


    def check(self):
        return corpus.check_corpus(self.path, self.fixture.repo)


    def prepare_cli(self, selection, config=None, module=True):
        config_path = self.fixture.root / "config.json"
        config_path.write_bytes(artifact_io.canonical(self.fixture.config if config is None else config))
        command = [sys.executable, "-m", "evals.benchmark"] if module else [sys.executable, "-m", "evals.benchmark.trials"]
        return subprocess.run([*command, "prepare", *selection, "--config", str(config_path),
            "--source-repo", str(self.fixture.repo), "--tool-repo", str(self.fixture.repo),
            "--output", str(self.fixture.root / "selected"), "--repetitions", "1"],
            capture_output=True, text=True, timeout=20)


    def close(self):
        self.fixture.close()
