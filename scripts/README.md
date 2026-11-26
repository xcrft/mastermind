# Scripts

Run commands from the repository root. The [justfile](../justfile) provides the
main developer interface:

| Purpose | Command |
|---|---|
| Full deterministic gate | `just check` |
| Release gate: `check`, native npm smoke, crates.io publish dry run | `just release-check` |
| Rust tests on Linux in Docker, from committed `HEAD` | `just test-linux` |
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
- behavior fixture source trees for embedded answer clues, with an error if
  the fixture inventory is missing or has no source files. Fixture READMEs are excluded.
- Action pins, required-check routing and publication workflow structure.
- one Rust version: workflows install from `rust-toolchain.toml` and name no
  toolchain, and the Docker Action builds with the same channel.
- npm versions, platform packages and README badges.

```bash
just bootstrap  # install pinned Python dependencies once
just validate
```

Exit `0` means no errors. Warnings do not fail the run. The validator does not
compile Rust, run npm, invoke models or establish the runtime behavior of an
agent prompt. Those checks have separate suites.

Artifact discovery excludes template placeholders and build/local-state
paths, including `.mastermind` research archives. Exclusions are relative to the
checked repository, so an ancestor directory named `target` does not hide source
documentation. New checks belong in `validate.py` and report `Issue` values with an
`error` or `warning` level. Fix newly detected repository violations in the same
change.

## Python harness checks

Run workflow contracts and process ownership checks from the repository root:

```bash
python3 -m unittest discover -s tests -t .
```

`tests/validation/` covers release and audit workflow contracts. `tests/evals/`
covers benchmark process supervision. Run `just eval-harness` for these checks,
document snapshots and the complete deterministic eval harness suite.

## Document evidence

```sh
python3 -m unittest tests.validation.test_document_graph
```

`tests/validation/test_document_graph.py` tests the portable history helper in temporary Git
repositories: schema and path validation, bounded reads, changed or deleted
sources and snapshot invalidation. It uses Python's standard library and Git.

The runtime helper and its contract live in
[the project-history skill](../skills/workflow/mastermind-project-history/SKILL.md).
These tests establish source identity and freshness behavior. They do not judge
the meaning of a declared relation.

## Cutting a release

Use this when `[Unreleased]` in `CHANGELOG.md` is ready to ship. You need push
access to `main`, an eligible reviewer for the `prod` and `npm-prod`
environments, `gh` authenticated for the repository and Docker for
`just test-linux`. Run the commands from the repository root.

A release goes out in three pushes, each after the previous one is green:

| Push | What | Wait for |
|---|---|---|
| 1 | Code changes, without a version bump | Green push CI on `main` |
| 2 | The release commit from `just bump` | `just release-tags`, which waits for CI and the publish dry runs |
| 3 | Both tags in one `git push` | Approvals, registry smokes and the GitHub Release |

Keep the code and the bump apart. A failure after push 1 is then a normal fix,
not a half-made release.

1. Start from a `main` commit whose push CI is already green. Read
   `[Unreleased]` again. Mark incompatible changes under **Changed** and pick
   the version by SemVer: an incompatible CLI, schema, file or workflow change
   is a major version.
2. Set the version everywhere and move the notes:

   ```bash
   just bump 4.0.0
   ```

   This rewrites every file listed in `LOCATIONS` in
   `scripts/release_version.py`, adds the `## [4.0.0] - <date>` heading and the
   CHANGELOG links. It refuses to run if the files already disagree.
   `python3 scripts/validate.py` fails a pull request whose versions drift.
3. Commit, then run both gates outside the Bash sandbox. They can run in
   parallel in two terminals. `test-linux` tests the committed `HEAD` on a
   case-sensitive Linux file system, which macOS does not have.

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
   `publish-mmcg.yml` and `publish-npm.yml` on that commit. Then it waits for
   the push CI and both dry runs and refuses to tag if any of them is red or
   unfinished. Running it again on the same commit waits for the same runs and
   does not start new ones. A long wait is normal. The macOS jobs are the
   slowest and least predictable part. The recipe waits up to two hours for
   each stage. Set `CI_WAIT_SECONDS` to change that.
5. Push both tags:

   ```bash
   git push origin mmcg-v4.0.0 npm-v4.0.0
   ```

6. Approve the `prod` (crates.io) and `npm-prod` deployments. Each publish job
   checks again that CI is green on the tagged commit, so approval does not need
   a manual CI check. The tag run of `publish-mmcg.yml` does not repeat the
   tests, it relies on that green CI.
7. Wait for both registry smokes. The npm workflow then creates the GitHub
   Release `npm-v4.0.0` with the CHANGELOG section as its notes.

### Where to look first

Do not wait for the whole run after a push. Linux and Windows finish in about
17 minutes, macOS often takes 40 minutes or more. Open the jobs that are
already done:

```bash
run_id=$(gh run list --commit "$(git rev-parse HEAD)" --workflow ci-mmcg.yml \
  --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$run_id" --json jobs --jq '.jobs[] | [.name, .status, .conclusion] | @tsv'
gh run view "$run_id" --log-failed
```

A macOS failure is a real failure, even when the job is slow. Fix the test or
the code. Do not add retries or longer timeouts to make it pass.

### When something fails

| Symptom | What to do |
|---|---|
| `release-tags` says HEAD is not origin/main | Push the release commit, or check out `main` |
| `require-green-ci.sh` reports a failed push CI workflow before tagging | Fix forward: commit the fix on `main`, bump nothing, push, wait for green CI, run `just release-tags` again. It tags the new commit and starts new dry runs |
| A publish dry run failed before tagging | Same fix forward. A fix to a workflow file only takes effect on a new commit |
| A dry run failed for an infrastructure reason, with no code problem | `gh run rerun <id> --failed`, then run `just release-tags` again. It waits for the rerun |
| A publish job fails with HTTP 401 or 403 | The registry token in the environment expired. Rotate `CARGO_REGISTRY_TOKEN` in `prod` or `NPM_TOKEN` in `npm-prod`, then `gh run rerun <id> --failed`. The rerun reads the new secret and reuses the verified artifact, which is kept 7 days. Do not move the tags |
| The publish workflow itself is broken on the tag and nothing was published | The tag run uses the workflow file of the tagged commit, so the tags must move. Fix `main` and push it, then move the tags as shown below |
| npm publish failed partway | Follow [npm publication and recovery](#npm-publication-and-recovery) |
| A tag points at the wrong commit and nothing was published | Move the tags as shown below |

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

`configure-github-protections.sh` previews settings by default. Applying them
requires an admin-authenticated `gh` session:

```bash
scripts/configure-github-protections.sh
scripts/configure-github-protections.sh --apply
```

It configures the `main` ruleset and two release boundaries. `npm-prod` deploys
only from `npm-v*` tags and `prod` only from `mmcg-v*` tags, each with its
reviewer and an immutable tag ruleset. Branch deployment policies on those
environments are removed. It does not replace environment secrets. Self-review prevention is
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
the failed job to appear in npm. Then dispatch the recovery workflow from the
release tag so that it satisfies `npm-prod`'s tag deployment policy. It checks
out the current helper from `main`. Set `source_run_id` to the failed tag run's
ID and `release_tag` to its immutable tag:

```bash
gh workflow run recover-publish-npm.yml --ref "$release_tag" \
  -f release_tag="$release_tag" -f source_run_id="$source_run_id"
```

The workflow requires an eligible `npm-prod` reviewer and the original
`npm-tarballs` artifact. It verifies existing versions, publishes only missing
packages and runs the public npm installation smoke. Keep the original tag and
tarballs intact. If the artifact has expired or a registry integrity differs,
stop recovery and investigate the source release.

For older tags whose recovery workflow does not yet check out the helper from
`main`, rerun only the failed tag jobs after accepted packages become visible:

```bash
gh run rerun "$source_run_id" --failed
```

This reuses the successful assembly job's original tarballs without rebuilding
the platform binaries.

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
