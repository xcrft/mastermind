# Dependency Manifest Best Practices

Detailed rules for package manifests, workspace definitions, dependency pins,
registries, and generated lockfiles. Use `mastermind-dependency-maintenance`
(security and full profiles) for full compatibility and verification depth.
Review manifest intent; do not emit line-by-line findings for generated
lockfile churn.

## Enforceable rules

### dependencies-manifest-and-lock-agree

**Name:** Update the owning manifest and lock together

**Rule:** Change dependencies through the repository package manager and commit
the required manifest/lock/resolution files without unrelated graph refresh.

**Why:** A stale or manually edited lock makes local, CI, and release builds
resolve different packages or integrity metadata.

**Detect:** A diff changes a direct dependency constraint without the required
lock/resolution update, changes only generated lock entries with no owning
manifest/tool action, or refreshes unrelated packages beyond the requested
dependency graph.

### dependencies-no-credentials-in-registry-config

**Name:** Keep registry credentials out of source

**Rule:** Reference approved CI/developer secrets; do not commit registry tokens,
passwords, private feed URLs containing credentials, or expanded auth files.

**Why:** Package configuration is copied into build contexts, caches, logs, and
developer machines.

**Detect:** A diff adds a credential-like literal or authenticated URL to npm,
NuGet, Cargo, Python, Maven/Gradle, Go, or other registry/package configuration.

### dependencies-install-scripts-are-reviewed-code

**Name:** Treat install and build hooks as executable supply chain

**Rule:** New lifecycle scripts, build scripts, plugins, git/path dependencies,
and binary downloads need a concrete purpose, pinned provenance, minimum
authority, and integrity controls.

**Why:** Dependency installation runs this code before application tests and
often with developer or CI credentials.

**Detect:** A diff adds a pre/post-install hook, remote script/binary, mutable git
dependency, package plugin, or local path outside the owned workspace without a
pinned revision/integrity and reviewed execution path.

### dependencies-preserve-supported-runtime-and-platforms

**Name:** Keep runtime and platform compatibility

**Rule:** A dependency update must support the repository's compiler/runtime,
module system, architecture, OS, native backend, and feature matrix.

**Why:** Resolution or one CI target can pass while another supported consumer
cannot compile, load native assets, or start.

**Detect:** A diff selects a package/version/feature whose declared minimum
runtime, target, peer, or native requirement conflicts with a current manifest,
CI matrix, or repository-local supported target.

### dependencies-preserve-public-package-contracts

**Name:** Keep published dependency boundaries compatible

**Rule:** Peer/runtime/dev dependency placement, optional features, package
exports, and bundled/external choices must match what downstream consumers need.

**Why:** Moving a dependency or feature can duplicate framework runtimes, remove
types/assets, or require consumers to install an undeclared package.

**Detect:** A diff changes peer/runtime/optional placement, feature defaults, or
bundle externalisation while published entry points still expose or require the
dependency and consumers are not migrated.

### dependencies-security-fix-reaches-the-shipped-graph

**Name:** Remediate the actual vulnerable path

**Rule:** A security update must move every shipped affected dependency path out
of the vulnerable range without introducing an incompatible override.

**Why:** Updating an unused direct declaration or one workspace can leave the
vulnerable transitive version in the released artifact.

**Detect:** A diff claims/remediates a vulnerability but the resulting lock or
workspace graph still contains the affected version on a shipped path, or uses
an override incompatible with a current consumer.

## Advisory rules

### dependencies-new-package-has-owned-purpose

**Name:** Add dependencies only for owned behaviour

**Rule:** A new package needs a concrete current use and review of maintenance,
security, licence, size, transitive, native, and platform impact.

**Why:** One convenience import adds long-lived supply-chain and update burden
to every consumer or shipped artifact.

**Detect:** A diff adds a direct dependency that is unused in changed code or
duplicates an established repository capability without a demonstrated need.

### dependencies-removal-proves-no-supported-consumer

**Name:** Prove compatibility before removing a package

**Rule:** Trace imports, reflection/plugins, optional features, scripts,
packaging, deployment, and downstream consumers before removing an apparently
unused dependency.

**Why:** Build-only, runtime-loaded, compatibility, or platform-specific use may
not appear in the primary source imports.

**Detect:** A diff removes a dependency still referenced by a supported manifest,
feature/target, script, packaging step, generated binding, dynamic load, or
downstream public contract.

### dependencies-preserve-repository-pin-policy

**Name:** Keep version and feature policy intentional

**Rule:** Preserve exact/range pin style, prerelease policy, feature selection,
and update grouping unless the change deliberately migrates that contract.

**Why:** Broadening constraints or defaults can resolve unreviewed code later;
narrowing can block compatible security updates or consumers.

**Detect:** A diff changes pin style, enables default/all features, or broadens a
group without repository-local compatibility evidence and matching validation.

### dependencies-do-not-claim-functional-proof-from-resolution

**Name:** Separate resolution from runtime evidence

**Rule:** Installation, restore, audit, or compilation proves only that layer;
runtime-loading and integration changes need their own evidence.

**Why:** Native assets, peer/runtime behaviour, startup configuration, and live
integrations fail after a clean dependency resolution.

**Detect:** A changed PR/workflow/doc treats restore/audit/build success as the
only proof for a dependency that affects runtime loading, transport, storage,
native code, or an external integration.
