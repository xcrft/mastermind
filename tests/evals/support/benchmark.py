"""Disposable Git, source and adapter fixtures shared by benchmark tests."""

from pathlib import Path
import hashlib
import shutil
import subprocess
import sys
import tempfile
import textwrap

from evals.benchmark import runtime as runtime_identity
from evals.benchmark import trials as trial_runner


CONTRACT = {"schema_version": "8", "extractor_contract_version": "mmcg-extractors-v6",
            "concept_normalization_version": "mmcg-concepts-v2"}

ADAPTER_HEADER = """import hashlib, json, os, pathlib, sys, time
request = json.load(sys.stdin)
root = pathlib.Path(request['source_root'])
(root.parent / 'adapter-called').write_text('yes')
def emit(event):
    print(json.dumps(event), flush=True)
def init(model=None):
    emit({'type': 'init', 'model': model or request['model'], 'adapter_version': 'fake-v1'})
def final(answer='Observed src/service.py:2.', **extra):
    result = {'type': 'result', 'answer': answer, 'model_error': False, 'turns': 1,
              'usage': {'input_tokens': 25, 'output_tokens': 12, 'cache_read_tokens': 0,
                        'cache_write_tokens': 0}, 'cost_usd': 0}
    result.update(extra)
    emit(result)
"""

INDEXER_BODY = """import hashlib, json, pathlib, sqlite3, sys
assert len(sys.argv) == 5 and sys.argv[1] == '--index' and sys.argv[3] == 'index'
index = pathlib.Path(sys.argv[2])
root = pathlib.Path(sys.argv[4]).resolve()
(root.parent / 'indexer-called').write_text('yes')
with sqlite3.connect(index) as db:
    db.executescript('CREATE TABLE symbols(id); CREATE TABLE edges(id); '
                     'CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT); '
                     'CREATE TABLE files(path TEXT PRIMARY KEY, content_sha256 TEXT);')
    meta = dict(CONTRACT, index_root=str(root))
    if MODE == 'wrong_root':
        meta['index_root'] = str(root.parent)
    if MODE == 'wrong_contract':
        meta['schema_version'] = '7'
    db.executemany('INSERT INTO meta VALUES (?, ?)', meta.items())
    if MODE != 'partial':
        for path in INDEXED_FILES:
            sha = hashlib.sha256((root / path).read_bytes()).hexdigest()
            db.execute('INSERT INTO files VALUES (?, ?)',
                       (path, '0' * 64 if MODE == 'wrong_hash' else sha))
if MODE == 'wal':
    index.with_name(index.name + '-wal').write_bytes(b'uncheckpointed')
if MODE == 'exit_failure':
    sys.exit(7)
"""



class BenchmarkFixture:
    def __init__(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name).resolve()
        self.repo = self.root / "original"
        self.repo.mkdir()
        self.env = runtime_identity.clean_environment(self.root)
        self.git("init", "-q", "-b", "fixture")
        self.git("config", "core.autocrlf", "false")
        self.instruction = "# Research\nRead source; cite evidence and unknowns.\n"
        for name, body in {
            "src/service.py": "def value():\n    return 7\n",
            "docs/guide.md": "The service returns a fixed value.\n",
            "evals/answer.json": '{"hidden": "ANSWER_KEY_CANARY"}\n',
            ".claude/settings.json": '{"private": "SETTINGS_CANARY"}\n',
            "AGENTS.md": "AGENT_CONFIG_CANARY\n",
            "skills/research/SKILL.md": self.instruction,
        }.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        self.git("add", "--all")
        self.git("commit", "-qm", "fixture with private evaluation data")
        self.revision = self.git("rev-parse", "HEAD").strip()
        self.task = {"id": "service-01", "revision": self.revision, "kind": "research",
                     "source_allowlist": ["src/service.py", "docs/guide.md"],
                     "question": "What does value return?", "output_contract": "Cite path:line."}
        self.rubric = {"task_id": self.task["id"], "source_revision": self.revision,
                       "required_fact": "HIDDEN_RUBRIC_CANARY"}
        self.config = {"model": "fixed-test-model-20260907", "tool_revision": self.revision,
                       "instruction_path": "skills/research/SKILL.md",
                       "limits": {"timeout_seconds": 30}}
        self.adapter()
        self.indexer()


    def git(self, *args, cwd=None):
        result = subprocess.run(
            [shutil.which("git"), "--no-replace-objects", "-c", "core.hooksPath=/dev/null",
             "-c", "commit.gpgsign=false", *args], cwd=cwd or self.repo, env=self.env,
            check=True, capture_output=True, text=True, timeout=10,
        )
        return result.stdout


    def executable(self, name, body):
        path = self.root / name
        path.write_text(f"#!{sys.executable}\n" + textwrap.dedent(body))
        path.chmod(0o755)
        return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                "version": "fake-v1", "origin": "deterministic test fixture"}


    def adapter(self, body="init()\nfinal()\n"):
        self.config["adapter"] = self.executable("fake-adapter", ADAPTER_HEADER + textwrap.dedent(body))


    def indexer(self, mode="ok", paths=None):
        paths = ["src/service.py"] if paths is None else paths
        body = f"CONTRACT = {CONTRACT!r}\nMODE = {mode!r}\nINDEXED_FILES = {paths!r}\n" + INDEXER_BODY
        pin = self.executable("fake-mmcg", body)
        self.config["mmcg"] = dict(pin, source_revision=self.revision,
                                   index_contract=CONTRACT, indexed_files=paths)


    def prepare(self, condition="source", **kwargs):
        return trial_runner.prepare_trial(task=kwargs.get("task", self.task), rubric=self.rubric,
            config=kwargs.get("config", self.config), source_repo=self.repo, tool_repo=self.repo,
            output=self.root / "trials", condition=condition)


    def condition_matrix(self):
        instructions = {"prompts/refined.md": "Уточнение: cite the return value and caller uncertainty.\n",
                        "prompts/profile.md": "PROFILE_CONTROL_CANARY: keep the answer concise.\n"}
        for name, body in instructions.items():
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        self.git("add", "prompts")
        self.git("commit", "-qm", "instruction controls")
        revision = self.git("rev-parse", "HEAD").strip()
        self.config["tool_revision"] = revision
        self.config["mmcg"]["source_revision"] = revision
        self.config["conditions"] = [
            {"id": "raw", "tools": "source", "instruction_paths": []},
            {"id": "refined", "tools": "source", "instruction_paths": ["prompts/refined.md"]},
            {"id": "profile", "tools": "source", "instruction_paths": ["prompts/profile.md"]},
            {"id": "refined_profile", "tools": "source", "instruction_paths": list(instructions)},
        ]
        return instructions


    def close(self):
        self.temp.cleanup()
