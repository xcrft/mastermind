# tests/ci-fixture/

Minimal committed fixture for CI smoke tests of `mmcg verify-spec` and
`mmcg audit-spec`. Used by `.github/workflows/ci-mmcg.yml` on every PR
across the 7-target build matrix.

## Layout

```
tests/ci-fixture/
├── README.md   ← this file
├── src/
│   └── lib.py  ← tiny Python source — small enough to index instantly
└── spec.md     ← spec with YAML frontmatter exercising the gate paths
```

## What the smoke does

The CI step copies this directory into a tmpdir, initializes a git repo,
commits the baseline, runs `mmcg init`, indexes, runs `verify-spec`,
introduces a change (adds `def new_helper`), commits HEAD, re-indexes,
runs `audit-spec --since baseline`.

The smoke proves on each target platform that:

1. The mmcg binary launches and parses CLI args
2. The indexer can write to SQLite + parse Python with tree-sitter
3. The YAML frontmatter parser works
4. `verify-spec` resolves frontmatter-scoped symbol checks against the index
5. `audit-spec` can shell out to `git`, parse old blobs via `git show`,
   and produce a verdict

A failure on any target = a real platform regression. Don't relax assertions
to make CI green — fix the regression.

## Don't extend this fixture casually

This file is platform-cross-product cost: every line of every file here
runs through the indexer on 7 OSes/architectures. Keep it minimal. If you
need a richer fixture for a specific test, put it in `evals/fixtures/`
where the eval runner uses it once per run.
