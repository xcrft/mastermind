# Documentation Best Practices

Detailed rules for technical Markdown and MDX. Editorial preference is not a
review defect; report only documentation that changes a contract, causes an
incorrect action, hides risk, or breaks navigation or automation.

## Enforceable rules

### documentation-commands-match-current-code

**Name:** Commands are current and copyable

**Rule:** Commands must use current paths, tools, flags, branches, variables,
and prerequisites from the owning repository and state their working directory.

**Why:** A plausible but stale command can deploy the wrong target, mutate data,
or prevent a supported setup from working.

**Detect:** A diff adds/changes a command that references a missing path/script,
wrong package target/branch/port, or omitted required variable demonstrably
needed by the current code or CI.

### documentation-destructive-actions-are-explicit

**Name:** Expose destructive scope and recovery

**Rule:** Deletion, overwrite, migration, credential, production, and bulk
operations must identify exact targets, permissions, irreversible effects,
confirmation/dry-run behaviour, and recovery where available.

**Why:** Readers can execute dangerous instructions against the wrong data or
environment without understanding the impact.

**Detect:** A diff documents a destructive or privileged command as a normal
copy/paste step but omits a target/scope check or known irreversible/rollback
constraint present in the implementation.

### documentation-no-real-secrets-or-personal-data

**Name:** Examples contain no live sensitive data

**Rule:** Use unmistakable placeholders and synthetic identifiers; do not paste
tokens, keys, internal credentials, signed URLs, private endpoints, or real
personal/tenant records.

**Why:** Documentation is indexed, copied, published, and retained outside the
runtime secret boundary.

**Detect:** A diff adds a credential-shaped value, private-key block, live
connection string/signed URL, or identifiable real user/tenant data rather than
a clear placeholder.

### documentation-links-and-anchors-resolve

**Name:** Navigation reaches the intended source

**Rule:** Changed relative links, anchors, includes, images, and navigation
entries must resolve from the rendered location and use current heading names.

**Why:** Broken links hide the source of truth or make required procedures
unreachable.

**Detect:** A changed link points to a missing tracked file/anchor, uses a path
relative to the wrong document, or leaves navigation targeting a renamed/deleted
page.

### documentation-evidence-claims-match-proof

**Name:** State only the evidence actually established

**Rule:** Distinguish install, lint, compile, unit, integration, startup smoke,
functional, live E2E, migration, and rollout evidence.

**Why:** Calling CI or a mocked test end-to-end proof can release behaviour that
was never exercised at its real boundary.

**Detect:** A diff claims a stronger verification/deployment result than the
referenced current workflow or commands perform, or says failures cannot skip
coverage while the implementation truncates/fails open.

## Advisory rules

### documentation-link-to-the-source-of-truth

**Name:** Do not duplicate volatile configuration

**Rule:** Explain intent and link to the owning manifest/schema/configuration
instead of copying long version, repository, environment, or rule tables.

**Why:** Multiple normative copies drift and give maintainers conflicting
instructions.

**Detect:** A diff adds a second manually maintained normative list already
owned by a machine-readable file, without generation or a clear snapshot label.

### documentation-preserve-version-and-rollout-context

**Name:** Bound instructions to supported versions

**Rule:** Version-sensitive behaviour must name the supported runtime/tool and
rolling-upgrade or compatibility assumptions.

**Why:** Readers apply current-only syntax or atomic-rollout assumptions to
older deployed consumers.

**Detect:** A diff prescribes a version-specific flag/API/schema while current
manifests or consumers include an incompatible version and no conditional path
is documented.

### documentation-diagrams-match-the-contract

**Name:** Diagrams and prose agree

**Rule:** Changed diagrams must render and show the same ownership, direction,
sequence, failure path, and boundaries described by current code and prose.

**Why:** A visually convincing stale diagram can drive an incorrect integration
or operational decision.

**Detect:** A diff changes adjacent prose/code references but leaves a diagram
with a contradictory owner/direction/state, or adds Mermaid syntax that cannot
render under the repository's supported renderer.
