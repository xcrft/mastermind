"""Entry point copied into the private, hash-checked adapter directory."""

from pathlib import Path
import runpy
import sys


sys.path.insert(0, str(Path(__file__).resolve().parent))
module = "evals.benchmark.adapters.claude"
if sys.argv[1:2] == ["codex"]:
    del sys.argv[1]
    module = "evals.benchmark.adapters.codex"
if sys.argv[1:2] == ["tools"]:
    del sys.argv[1]
    module = "evals.benchmark.tools"
runpy.run_module(module, run_name="__main__")
