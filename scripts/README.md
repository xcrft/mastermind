# Scripts

Run commands from the repository root. The [justfile](../justfile) provides the
main developer interface:

| Purpose | Command |
|---|---|
| Full deterministic gate | `just check` |
| Repository contracts | `just validate` |
| Evidence and eval harnesses, without a build or model | `just eval-harness` |
| Native npm tarball smoke | `just npm-smoke-native` |
| Index benchmark | `just benchmark-index` |

## Repository validator

`validate.py` checks:

- artifact metadata, links, template mirrors and workflow bundle staging.
- subagent model/tool/turn contracts and MCP grants.
- public tool documentation, schemas and report ownership.
- skill adapters and their evaluation cases.
- Action pins, required-check routing and publication workflow structure.
- npm versions, platform packages and README badges.

```bash
just bootstrap  # install pinned Python dependencies once
just validate
```

Exit `0` means no errors. Warnings do not fail the run. The validator does not
compile Rust, run npm, invoke models or establish the runtime behavior of an
agent prompt. Those checks have separate suites.

Artifact discovery excludes template placeholders and build/local-state
paths. New checks belong in `validate.py` and report `Issue` values with an
`error` or `warning` level. Fix newly detected repository violations in the same
change.

## Document evidence

`test_document_graph.py` tests the portable history helper in temporary Git
repositories: schema and path validation, bounded reads, changed or deleted
sources and snapshot invalidation. It uses Python's standard library and Git.

The runtime helper and its contract live in
[the project-history skill](../skills/workflow/mastermind-project-history/SKILL.md).
These tests establish source identity and freshness behavior. They do not judge
the meaning of a declared relation.

## GitHub release controls

`configure-github-protections.sh` previews settings by default. Applying them
requires an admin-authenticated `gh` session:

```bash
scripts/configure-github-protections.sh
scripts/configure-github-protections.sh --apply
```

It configures the `main` ruleset, `npm-v*` tag boundary and `npm-prod` environment
reviewer. It does not replace environment secrets. Self-review prevention is
disabled by default. Enable it with a distinct eligible reviewer:

```bash
scripts/configure-github-protections.sh \
  --reviewer another-maintainer --prevent-self-review --apply
```

## Registry smoke tests

Publish workflows run these against public packages after publication:

| Script | What it checks |
|---|---|
| `smoke-installed-npm-release.sh` | Exact npm version, platform binary, indexing, facts adaptation/signing/import and a two-repository team map |
| `smoke-installed-crate-release.sh` | Exact crates.io version, binary version and the shipped facts/team/review command surfaces |

Both install into temporary locations, retry registry propagation for a bounded
period and fail when the package cannot be installed or exercised. They do not
publish packages. The local `npm-smoke-native` recipe instead uses workspace
tarballs and does not establish public-registry availability.
