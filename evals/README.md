# Behavioral evaluations

Can the shipped instruction still produce the behavior it promises under one
focused adversarial scenario? These suites make that question replayable for
Mastermind agents and workflow skills.

One case probes one expected behavior. A pass rate is not coverage, product
correctness, or evidence that the behavior survives a long real-world task.

## Suite map

| File | Target | Expected result |
|---|---|---|
| `critic.jsonl` | Design critic | `rethink`, `revise`, or `ship` |
| `auditor.jsonl` | Post-flight auditor | `held`, `drift`, or `broken` |
| `intake.jsonl` | Prompt intake | `refined`, `passthrough`, or `ask` |
| `workflow.jsonl` | Planner, executor, and portable skills | Required and forbidden signals |
| `fixtures/` | Real Git histories for auditor cases | Exact planted change |
| `scorecard.md` | Dated full-suite results | Environment and trust notes |

`runner.py` invokes `claude -p`. `test_runner.py` tests the deterministic
parser, isolation, allowlist, and fixture machinery without calling a model.

## Run the right layer

Model-backed runs require authenticated `claude` and `git` executables:

```bash
./evals/runner.py
./evals/runner.py --suite critic
./evals/runner.py --suite workflow
./evals/runner.py --case c-001-slop-rethink
./evals/runner.py --model sonnet
./evals/runner.py --keep-fixtures
./evals/runner.py --verbose-failures
```

Run deterministic repository gates before every model-backed suite:

```bash
bash evals/run-verified.sh --model sonnet
```

Model-backed evals are hand-run, not ordinary CI. CI runs the deterministic
harness contract through:

```bash
.venv/bin/python -m unittest evals/test_runner.py
```

## Auditor fixture lifecycle

Each auditor case names `fixtures/<name>/`, a baseline tag, and an after tag.
The runner:

1. creates a temporary Git repository;
2. commits the fixture baseline and tags it;
3. replaces the tree with the named after-state, commits, and tags it;
4. indexes the after-state with `mmcg`;
5. gives the auditor the temporary repository and a live stdio MCP server;
6. checks the structured audit verdict and required reasoning signals.

The auditor reads the real Git diff and codegraph. The JSONL case does not
provide a synthetic diff. The runner prefers the in-tree release binary at
`mcp/servers/mmcg/target/release/mmcg`, then falls back to `mmcg` on `PATH`.

Build the matching binary before a model-backed auditor run:

```bash
cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml --locked
```

## Add a critic case

```jsonc
{
  "id": "c-NNN-short-name",
  "why": "single regression scenario",
  "input": {},
  "expect": {
    "verdict": "rethink",
    "contains": ["required phrase"],
    "not_contains": ["forbidden phrase"]
  }
}
```

## Add an auditor case

```jsonc
{
  "id": "a-NNN-short-name",
  "why": "single planted defect",
  "fixture": "fake-session",
  "baseline_ref": "baseline",
  "after_ref": "scope-creep",
  "allow_no_mmcg": false,
  "input": {
    "spec_summary": "...",
    "executor_report": "..."
  },
  "expect": {
    "verdict": ["drift", "broken"],
    "contains": ["config", "scope"],
    "not_contains": ["contract held"]
  }
}
```

Verdict assertions read the YAML block between
`<!-- mastermind:audit-begin -->` and `<!-- mastermind:audit-end -->`. Missing
or malformed structured output fails the case. Add a full after-tree under
`fixtures/<name>/changes/<after_ref>/`; files absent from that tree are deleted
in the generated commit.

## Add a workflow case

```jsonc
{
  "id": "w-NNN-short-name",
  "artifact": "skills/workflow/example/SKILL.md",
  "input": {"prompt": "self-contained scenario"},
  "expect": {
    "contains": ["required signal"],
    "contains_any": [["equivalent A", "equivalent B"]],
    "not_contains": ["forbidden claim"],
    "code_comments": {"prefixes": ["//", "/*"], "min": 0, "max": 0}
  }
}
```

Workflow artifacts are allowlisted and loaded from the repository. Cases run
from the system temporary directory in Claude safe mode with no tools, so the
prompt under evaluation cannot operate on the maintainer checkout.

## Keep cases honest

- One adversarial or golden behavior per case.
- Explain the regression in `why`; do not leak the expected answer into source
  fixture files.
- Use deterministic verdict and phrase assertions. The harness does not use an
  LLM judge.
- Use `code_comments` only when generated code, rather than prose advice, is
  under test.
- Run the focused case before the full suite and record full-suite results in
  `scorecard.md`.

## Ablation

`ablation.py` compares planted-defect detection under two conditions on the
same generated Git repository:

- `vanilla`: a neutral reviewer with shell access, but no mmcg or Mastermind
  auditor contract;
- `mastermind`: the shipped auditor with the live codegraph.

Golden `held` cases are excluded because there is no defect to catch.

```bash
python evals/ablation.py
python evals/ablation.py --with-mastermind
```

The score is phrase-based. It can miss subtly incorrect reasoning that happens
to contain the expected signals; record that limitation with every result.
