# Behavioral evaluations

Can the shipped instruction still produce the behavior it promises under one
focused adversarial scenario? These suites make that question replayable for
Mastermind agents and workflow skills.

One case probes one expected behavior. A pass rate is not coverage, product
correctness, or evidence that the behavior survives a long real-world task.

## Suite map

| File | Target | Expected result |
|---|---|---|
| `critic.jsonl` | Design critic | `rethink`, `revise`, `insufficient evidence`, `ship with caveats`, or `ship it` |
| `researcher.jsonl` | Codegraph researcher | Cited facts, explicit unknowns, or planner handoff |
| `auditor.jsonl` | Post-flight auditor | `held`, `drift`, or `broken` |
| `intake.jsonl` | Prompt intake | `refined`, `passthrough`, or `ask` |
| `workflow.jsonl` | Planner, executor, and portable skills | Required and forbidden signals |
| `fixtures/` | Real Git histories for researcher/auditor cases | Exact planted change |
| `scorecard.md` | Dated full-suite results | Environment and trust notes |
| `benchmark/` | Three-condition research trial preparation and adapter transport | Full answers retained for semantic review; no quality score yet |

`runner.py` invokes `claude -p`. Researcher and auditor cases load the shipped
agents through Claude's `--agents` / `--agent` runtime contract, so frontmatter
tool scoping is part of the eval instead of a separate handwritten allowlist.
`test_runner.py` and `test_evidence.py` test the deterministic parser, isolation,
runtime contract, allowlist, report gate, fixtures, and source citations without
calling a model.

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
python3 -m unittest evals/test_runner.py evals/test_evidence.py evals/test_benchmark.py
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
after-tree variant. The runner:

1. creates a temporary Git repository;
2. commits the fixture baseline and tags it;
3. replaces the tree with the named after-state, then commits and tags it by
   default; when `staged_paths` is present, leaves HEAD at baseline and stages
   only the listed paths;
4. indexes the after-state with `mmcg`;
5. gives the shipped custom agent the temporary repository and a live stdio MCP
   server;
6. checks the suite's deterministic verdict, phrase, tool identity, and
   tool-turn signals.

The auditor compares baseline to the current working tree and reads untracked
files separately, covering audits before commit. The researcher queries the
same graph and reads source before reporting a fact. JSONL cases do not provide
synthetic diffs or structural answers. The runner prefers the in-tree release
binary at `mcp/servers/mmcg/target/release/mmcg`, then falls back to `mmcg` on
`PATH`.

Build the matching binary before a model-backed researcher or auditor run:

```bash
cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml --locked
```

## Add a critic case

`expect.verdict` names one exact aggregate verdict or a list of acceptable
verdicts. The grader reads the single final `## Verdict` section; mentions in
prose, table rows, and quoted code examples cannot satisfy it. Missing or
conflicting final verdicts fail. `concern` and `fail` belong to dimension rows,
along with `pass` and `unknown`, so test those with phrase assertions when
needed. The portable critical-review workflow uses the same final-section
grader when its case sets `expect.verdict`.

Missing facts produce `insufficient evidence` unless an independently evidenced
failure already requires `revise` or `rethink`. A missing graph alone is not a
design failure when source evidence answers the claim. Verdict format checks
do not establish that dimension scores or their reasoning are correct.

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

## Source-backed research cases

`researcher.jsonl` includes a small regression corpus for ambiguous definitions,
callback registration, docs/code contradictions, superseded ADRs, dynamic
registration, and natural-language discovery. These are real disposable source
trees, not a measured product-quality baseline or a representative benchmark.
The researcher chooses the tool path; cases assert the facts and evidence the
answer must preserve.

Add source anchors to a case when a named file alone would allow a false pass:

```jsonc
"expect": {
  "contains": ["session_count"],
  "citations": [
    {"path": "src/session.rs", "anchor": "pub fn session_count("}
  ]
}
```

Each anchor must match exactly one line in the selected fixture tree. The answer
must cite that line with `path:line` or `path:start-end`, either directly, in
backticks, or in a Markdown link. Absolute paths inside the disposable fixture
are accepted. Ranges are limited to 40 lines so a whole-file citation cannot
satisfy every fact. Fenced/quoted examples do not count. Fabricated files,
out-of-range lines, and paths outside the fixture fail even when other required
citations are correct. Definitions and fixtures participate in the existing
case digest, so edited evidence cannot reuse an old baseline silently.

Case reports add optional `citation_checks` with `expected` and `matched`
anchors, `total` and `valid` unique citation locations, and failure `issues`.
`null` means no citation check was requested, not a perfect score. These counts
measure location validity and required anchor coverage. They do not establish
that a sentence follows from a source, that every dependency was found, or that
an architectural decision is correct. Phrase checks remain separate.

To establish a research-quality baseline, run this corpus with a fixed model,
CLI version, revision, and budgets, and review final reasoning for unsupported
claims and appropriate abstention. Compare prompt changes only on the same
case digest. Add unseen tasks and a source-search baseline before optimizing
skills against scores; do not treat passing parser tests as a model result.

The separate [research benchmark transport](benchmark/README.md) freezes one
public task, source allowlist, hidden rubric and budgets across source-only,
portable-instruction and portable-plus-mmcg conditions. It saves full bounded
answers and separates infrastructure and telemetry from semantic quality. Its
current trusted-adapter boundary is not an OS sandbox, so all comparison results
remain ineligible for an uplift claim.

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
from the generated working tree. To audit before commit, add
`"staged_paths": ["src/staged.py"]` to the case. Other modified tracked files
remain unstaged and new files remain untracked. Use `"staged_paths": []` to
leave every change unstaged.

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

`ablation.py` runs a diagnostic comparison under two conditions on equivalent
Git fixture trees, preserving staged, unstaged, and untracked changes:

- `vanilla`: a neutral reviewer with shell access, but no mmcg or Mastermind
  auditor contract;
- `mastermind`: the shipped auditor with the live codegraph.

Golden `held` cases are excluded because there is no defect to catch.

```bash
python evals/ablation.py
python evals/ablation.py --with-mastermind
```

The vanilla column uses phrase checks; the Mastermind column uses the full
auditor contract, including structured verdict and verification requirements.
The columns have different grading criteria, so the tool reports no quality
uplift and never inserts a historical score for a condition it did not run.

Phrase checks can miss subtly incorrect reasoning that happens
to contain the expected signals; record that limitation with every result.
