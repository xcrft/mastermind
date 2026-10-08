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

## npm publication and recovery

`publish-npm-tarballs.sh` publishes verified platform tarballs before the root
package. It requires Node.js, npm, registry credentials and the packed artifacts
from the release workflow. Existing versions must have the same SHA-512
integrity as the original tarballs. A successful upload is followed by public
registry checks before the next package is published.

| Input | Default | Purpose |
|---|---|---|
| `NPM_PUBLISH_VERIFY_ATTEMPTS` | `180` | Maximum integrity lookups after an accepted upload |
| `NPM_PUBLISH_VERIFY_DELAY_SECONDS` | `5` | Delay between unavailable-version lookups |

The default allows about fifteen minutes of registry processing per package.
An exhausted window fails the job and leaves later packages unpublished. It
does not repeat the accepted upload or bypass integrity checks.

To recover a partial release, first wait for any accepted package reported by
the failed job to appear in npm. Then run the recovery workflow from `main`
using the failed tag run's ID and its immutable release tag. Set
`source_run_id` and `release_tag` to those values:

```bash
gh workflow run recover-publish-npm.yml --ref main \
  -f release_tag="$release_tag" -f source_run_id="$source_run_id"
```

The workflow requires an eligible `npm-prod` reviewer and the original
`npm-tarballs` artifact. It verifies existing versions, publishes only missing
packages and runs the public npm installation smoke. Keep the original tag and
tarballs intact. If the artifact has expired or a registry integrity differs,
stop recovery and investigate the source release.

## Package smoke tests

Release workflows check candidate tarballs before publication and public
packages afterward:

| Script | What it checks |
|---|---|
| `smoke-packed-npm-release.sh` | Candidate tarballs, setup, index, npm update scope and all four workflow profiles, shared by CI and the local recipe |
| `smoke-installed-npm-release.sh` | Exact npm version, platform binary, indexing, facts adaptation/signing/import and a two-repository team map |
| `smoke-installed-crate-release.sh` | Exact crates.io version, binary version and the shipped facts/team/review command surfaces |

The two public-registry checks install into temporary locations, retry registry
propagation for a bounded period and fail when the package cannot be installed
or exercised. The shared tarball check installs offline in a temporary project.
The local `npm-smoke-native` recipe calls it with workspace tarballs. These
scripts do not publish packages. A tarball smoke does not establish
public-registry availability.
