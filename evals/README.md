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
| `researcher.jsonl` | Codegraph researcher | Cited facts, explicit unknowns, or planner handoff |
| `auditor.jsonl` | Post-flight auditor | `held`, `drift`, or `broken` |
| `intake.jsonl` | Prompt intake | `refined`, `passthrough`, or `ask` |
| `workflow.jsonl` | Planner, executor, and portable skills | Required and forbidden signals |
| `fixtures/` | Real Git histories for researcher/auditor cases | Exact planted change |
| `scorecard.md` | Dated full-suite results | Environment and trust notes |

`runner.py` invokes `claude -p`. Researcher and auditor cases load the shipped
agents through Claude's `--agents` / `--agent` runtime contract, so frontmatter
tool scoping is part of the eval instead of a separate handwritten allowlist.
`test_runner.py` tests the deterministic parser, isolation, runtime contract,
allowlist, report gate, and fixture machinery without calling a model.

Every case and suite summary reports turns, input/output tokens, prompt-cache
creation/read tokens, API time, and Claude CLI reported cost. Retries aggregate
both attempts, so a recovered flaky case does not hide its token spend. Reports
also retain the resolved model IDs and tool identities. Tool inputs are not
persisted.

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
./evals/runner.py --suite critic --model opus \
  --report /tmp/mastermind-critic.json
./evals/runner.py --suite critic --model opus \
  --report /tmp/mastermind-critic-current.json \
  --baseline-report evals/baselines/critic-opus-pre-lean.json
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

## Reports and token gates

`--report` atomically writes a `mastermind-eval-report` schema-v1 JSON file.
Every case retains quality, retry state, duration, API duration, turns, input
and output tokens, prompt-cache creation/read tokens, reported cost, and
telemetry completeness. Suite summaries use the nearest-rank rule for p50 and
p95; raw cases stay in the same report so each aggregate is auditable. Gate
inputs are recomputed from those raw cases, and an inconsistent summary fails
closed.

Context tokens are `input_tokens + cache_creation_input_tokens +
cache_read_input_tokens`. Output tokens remain separately reported because
response length varies with generation. A missing, malformed, negative, or
non-finite required telemetry field fails the case and cannot become a zero-cost
improvement.

`--baseline-report` is intentionally strict. Current and baseline evidence must
have the same requested model, resolved model IDs, Claude CLI version,
suite/case filters, selected suites, case order, and SHA-256 digest of the
selected JSONL definitions plus referenced fixture trees. For every suite, the
pass rate cannot fall, every baseline-passing case must still pass, and both p50
and p95 context tokens must be strictly lower. Malformed or incomparable
evidence exits non-zero. A case filter that matches nothing is also an error.

The checked-in critic baseline predates report emission and was transcribed from
the runner's console output. Its capture metadata records that only aggregate
API duration was observable; the token and quality fields used by the gate were
recorded per case. Its capture metadata also states how the resolved Opus model
ID was verified immediately afterward with the same alias and CLI. Claude CLI
reported cost is retained as telemetry, but these runs use the maintainer's
existing Claude subscription rather than per-token API billing.

## Researcher and auditor fixture lifecycle

Each researcher or auditor case names `fixtures/<name>/`, a baseline tag, and an
after tag. The runner:

1. creates a temporary Git repository;
2. commits the fixture baseline and tags it;
3. replaces the tree with the named after-state, commits, and tags it;
4. indexes the after-state with `mmcg`;
5. gives the shipped custom agent the temporary repository and a live stdio MCP
   server;
6. checks the suite's deterministic verdict, phrase, tool identity, and
   tool-turn signals.

The auditor reads the real Git diff and codegraph. The researcher queries the
same graph and reads source before reporting a fact. JSONL cases do not provide
synthetic diffs or structural answers. The runner prefers the in-tree release
binary at `mcp/servers/mmcg/target/release/mmcg`, then falls back to `mmcg` on
`PATH`.

Build the matching binary before a model-backed researcher or auditor run:

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
- Use `min_turns`, `max_turns`, or `max_output_tokens` only when the behavior
  has a real tool-use or response budget. Claude's reported output includes
  intermediate tool-call turns, not only the final prose.
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
