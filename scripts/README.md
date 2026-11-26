# Scripts

These scripts validate the repository, package Mastermind and publish releases.
Run every command from the repository root. The [justfile](../justfile) is the
main entry point:

| Purpose | Command |
|---|---|
| Full deterministic gate | `just check` |
| Release gate: `check`, native npm smoke, crates.io publish dry run | `just release-check` |
| Rust tests on Linux in Docker, from committed `HEAD` | `just test-linux` |
| Repository contracts | `just validate` |
| Python tests and the eval corpus check, see [tests](../tests/README.md) | `just eval-harness` |
| Native npm tarball smoke | `just npm-smoke-native` |
| Index benchmark | `just benchmark-index` |

## Repository validator

```bash
just bootstrap  # install pinned Python dependencies once
just validate
```

`validate.py` checks:

- artifact metadata, links, template mirrors and workflow bundle staging
- subagent model, tool and turn contracts and MCP grants
- public tool documentation, schemas and report ownership
- skill adapters and their evaluation cases
- skill descriptions: each is at most 400 characters, says `Use when`,
  `before`, `after` or `for` within its first 250 characters, and has a
  `Not for` clause that names the skill to use instead
- the untrusted-content block: each file in `UNTRUSTED_CONTENT_COVERED` carries
  [the canonical paragraph](../agents/blocks/untrusted-content.md) exactly once
  and unchanged, as its own paragraph outside code fences and HTML comments,
  and no other skill or agent carries it
- behavior fixture source trees for embedded answer clues. A missing fixture
  inventory or one with no source files is an error. Fixture READMEs are
  excluded
- Action pins, required-check routing and publication workflow structure
- one Rust version: workflows install from `rust-toolchain.toml` and name no
  toolchain, and the Docker Action builds with the same channel
- npm versions, platform packages and README badges

Exit `0` means no errors, and warnings don't fail the run. The validator
doesn't compile Rust, run npm or call models, and it skips template
placeholders and build or local-state paths such as `.mastermind`. A new check
reports `Issue` values with an `error` or `warning` level and lands with the
fixes for the violations it finds.

## Cutting a release

Use this when `[Unreleased]` in `CHANGELOG.md` is ready to ship. You need push
access to `main`, an eligible reviewer for the `prod` and `npm-prod`
environments, `gh` authenticated for the repository and Docker for
`just test-linux`.

A release goes out in three pushes, each after the previous one is green. Code
changes and the version bump stay apart, so a failure after the first push is a
normal fix and leaves no half-made release.

| Push | What | Wait for |
|---|---|---|
| 1 | Code changes, without a version bump | Green push CI on `main` |
| 2 | The release commit from `just bump` | `just release-tags`, which waits for CI and the publish dry runs |
| 3 | Both tags in one `git push` | Approvals, registry smokes and the GitHub Release |

1. Start from a `main` commit whose push CI is green. Read `[Unreleased]` again,
   mark incompatible changes under **Changed** and pick the version by SemVer.
   An incompatible CLI, schema, file or workflow change is a major version.
2. Set the version everywhere and move the notes:

   ```bash
   just bump 4.0.0
   ```

   This rewrites every file in `LOCATIONS` in `scripts/release_version.py` and
   adds the `## [4.0.0] - <date>` heading and the CHANGELOG links. It refuses to
   run if the files already disagree, and `python3 scripts/validate.py` fails a
   pull request whose versions drift.
3. Commit, then run both gates outside the Bash sandbox, in two terminals if you
   like. `test-linux` tests the committed `HEAD` on a case-sensitive Linux file
   system, which macOS doesn't have.

   ```bash
   git commit -am "Release 4.0.0"
   just release-check
   just test-linux
   git push origin main
   ```

4. Tag the pushed commit:

   ```bash
   just release-tags 4.0.0
   ```

   The recipe starts one verify-only `workflow_dispatch` run of
   `publish-mmcg.yml` and `publish-npm.yml` on that commit, waits for the push
   CI and both dry runs, and refuses to tag if any of them is red or
   unfinished. Running it again on the same commit waits for the same runs. It
   waits up to two hours for each stage, and `CI_WAIT_SECONDS` changes that.
   The macOS jobs are the slowest part.
5. Push both tags:

   ```bash
   git push origin mmcg-v4.0.0 npm-v4.0.0
   ```

6. Approve the `prod` (crates.io) and `npm-prod` deployments. Each publish job
   checks again that CI is green on the tagged commit, so you don't need to
   check CI by hand. The tag run of `publish-mmcg.yml` relies on that green CI
   and doesn't repeat the tests.
7. Wait for both registry smokes. The npm workflow then creates the GitHub
   Release `npm-v4.0.0` with the CHANGELOG section as its notes.

### Where to look first

Linux and Windows finish in about 17 minutes, and macOS often takes 40 minutes
or more, so open the jobs that are already done:

```bash
run_id=$(gh run list --commit "$(git rev-parse HEAD)" --workflow ci-mmcg.yml \
  --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$run_id" --json jobs --jq '.jobs[] | [.name, .status, .conclusion] | @tsv'
gh run view "$run_id" --log-failed
```

A macOS failure is a real failure, even when the job is slow. Fix the test or
the code, and don't add retries or longer timeouts to make it pass.

### When something fails

| Symptom | What to do |
|---|---|
| `release-tags` says HEAD is not origin/main | Push the release commit, or check out `main` |
| `require-green-ci.sh` reports a failed push CI workflow before tagging | Fix forward: commit the fix on `main`, bump nothing, push, wait for green CI and run `just release-tags` again. It tags the new commit and starts new dry runs |
| A publish dry run failed before tagging | Same fix forward. A workflow file fix only takes effect on a new commit |
| A dry run failed for an infrastructure reason | `gh run rerun <id> --failed`, then `just release-tags` again. It waits for the rerun |
| A publish job fails with HTTP 401 or 403 | The registry token expired. Rotate `CARGO_REGISTRY_TOKEN` in `prod` or `NPM_TOKEN` in `npm-prod`, then `gh run rerun <id> --failed`. The rerun reads the new secret and reuses the verified artifact, which is kept 7 days. Don't move the tags |
| The publish workflow is broken on the tag and nothing was published | The tag run uses the workflow file of the tagged commit, so the tags must move. Fix `main`, push it, then move the tags as below |
| npm publish failed partway | Follow [npm publication and recovery](#npm-publication-and-recovery) |
| A tag points at the wrong commit and nothing was published | Move the tags as below |

To move the tags, first check on crates.io and npm that no package with this
version exists. The release tag rulesets block deletion for everyone except an
organization admin, so an admin runs:

```bash
git push origin --delete mmcg-v4.0.0 npm-v4.0.0
git tag -d mmcg-v4.0.0 npm-v4.0.0
just release-tags 4.0.0
git push origin mmcg-v4.0.0 npm-v4.0.0
```

Never move a tag after a package with that version was published. Ship a new
patch version instead.

## GitHub release controls

`configure-github-protections.sh` sets the `main` ruleset and two release
boundaries: `npm-prod` deploys only from `npm-v*` tags and `prod` only from
`mmcg-v*` tags, each with its reviewer and an immutable tag ruleset. It removes
branch deployment policies on those environments and leaves environment
secrets alone. It previews by default, and `--apply` needs an
admin-authenticated `gh` session:

```bash
scripts/configure-github-protections.sh
scripts/configure-github-protections.sh --apply
```

Self-review prevention is off by default. Turn it on with a second eligible
reviewer:

```bash
scripts/configure-github-protections.sh \
  --reviewer another-maintainer --prevent-self-review --apply
```

## npm publication and recovery

`publish-npm-tarballs.sh` publishes the verified platform tarballs before the
root package. An existing version must match the original tarball's SHA-512
integrity, and each upload must show up in the registry before the next one.

| Input | Default | Purpose |
|---|---|---|
| `NPM_PUBLISH_VERIFY_ATTEMPTS` | `180` | Maximum integrity lookups after an accepted upload |
| `NPM_PUBLISH_VERIFY_DELAY_SECONDS` | `5` | Delay between lookups of a version that is not visible yet |

The defaults allow about fifteen minutes per package. When that runs out, the
job fails and leaves the later packages unpublished, without repeating the
accepted upload or skipping integrity checks.

To recover a partial release:

1. Wait until every package the failed job reported as accepted appears in npm.
2. Dispatch the recovery workflow from the release tag, so it satisfies the tag
   policy of `npm-prod`. Set `source_run_id` to the failed tag run's ID and
   `release_tag` to its tag:

   ```bash
   gh workflow run recover-publish-npm.yml --ref "$release_tag" \
     -f release_tag="$release_tag" -f source_run_id="$source_run_id"
   ```

   It needs an eligible `npm-prod` reviewer and the original `npm-tarballs`
   artifact. It checks out the current helper from `main`, verifies existing
   versions, publishes only the missing packages and runs the public install
   smoke.
3. If the artifact expired or a registry integrity differs, stop and
   investigate the source release. Keep the original tag and tarballs intact.

## Package smoke tests

| Script | Runs | What it checks |
|---|---|---|
| `smoke-packed-npm-release.sh` | Before publication in CI, and locally through `just npm-smoke-native` | Candidate tarballs installed offline in a temporary project: setup, index, npm update scope and all four workflow profiles |
| `smoke-installed-npm-release.sh` | After publication | Exact npm version, platform binary, indexing, facts adaptation, signing and import, and a two-repository team map |
| `smoke-installed-crate-release.sh` | After publication | Exact crates.io version, binary version and the shipped facts, team and review commands |

None of these scripts publish. The two registry checks install into temporary
locations and retry for registry propagation for a bounded time.
