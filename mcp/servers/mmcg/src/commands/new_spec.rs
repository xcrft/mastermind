use std::fs;
use std::path::Path;

pub enum Mode {
    Lite,
    Standard,
    Strict,
}

impl Mode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "lite" => Ok(Mode::Lite),
            "standard" => Ok(Mode::Standard),
            "strict" => Ok(Mode::Strict),
            other => Err(format!("unknown mode {other:?} — use `lite`, `standard`, or `strict`")),
        }
    }
}

pub fn run(description: &str, mode: Mode, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tasks_dir = root.join(".mastermind").join("tasks");
    if !tasks_dir.exists() {
        fs::create_dir_all(&tasks_dir)
            .map_err(|e| format!("create .mastermind/tasks/: {e}"))?;
    }

    let next_n = next_task_number(&tasks_dir)?;
    let slug = slugify(description);
    let dir_name = format!("{:03}-{}", next_n, slug);
    let task_dir = tasks_dir.join(&dir_name);
    fs::create_dir_all(&task_dir)
        .map_err(|e| format!("create {}: {e}", task_dir.display()))?;

    let spec_path = task_dir.join("spec.md");
    let content = render_spec(description, next_n, &mode);
    fs::write(&spec_path, &content)
        .map_err(|e| format!("write {}: {e}", spec_path.display()))?;

    println!("Created {}", spec_path.display());
    Ok(())
}

fn next_task_number(tasks_dir: &Path) -> Result<u32, Box<dyn std::error::Error>> {
    let mut max: u32 = 0;
    if let Ok(entries) = fs::read_dir(tasks_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if let Some(prefix) = s.split('-').next() {
                if let Ok(n) = prefix.parse::<u32>() {
                    if n > max {
                        max = n;
                    }
                }
            }
        }
    }
    Ok(max + 1)
}

fn slugify(s: &str) -> String {
    let slug: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.len() > 40 {
        slug[..40].trim_end_matches('-').to_string()
    } else {
        slug
    }
}

fn render_spec(description: &str, n: u32, mode: &Mode) -> String {
    let id = format!("{:03}", n);
    match mode {
        Mode::Lite => render_lite(description, &id),
        Mode::Standard => render_standard(description, &id),
        Mode::Strict => render_strict(description, &id),
    }
}

fn render_lite(description: &str, id: &str) -> String {
    format!(
        "\
---
id: \"{id}\"
title: {description}
mode: lite
risk: low
---

# Task {id}: {description}

## Goals

{description}

## Scope

- **File:** `<path/to/file.ext>`

## Pre-edit snapshot

<!-- delete if no code symbols touched -->
- `<symbol>` — <N> callers (mmcg_callers), signature `<sig>` (mmcg_search)

## Phase 1: <outcome>

### 1.1 <action>

**File:** `<path/to/file.ext>`

FIND:
```
<exact existing code>
```

CHANGE TO:
```
<new code>
```

VERIFY: `<command>`

## Notes

### Pre-flight validation

- [ ] All **File:** paths exist
- [ ] FIND: blocks match current file contents
- [ ] VERIFY: commands are runnable
"
    )
}

fn render_standard(description: &str, id: &str) -> String {
    format!(
        "\
---
id: \"{id}\"
title: {description}
mode: standard
risk: medium

touches:
  - file: <path/to/file.ext>
    language: <python|typescript|rust|csharp|go|java|php|cpp>
    symbols:
      - name: <symbol>
        callers: 0

verify:
  - cmd: \"<typecheck command>\"
  - cmd: \"<test command>\"

expected_docs: []
---

# Task {id}: {description}

## LLM Agent Directives

You are implementing {description}.

**Goals:**
1. <primary goal — what counts as done>

**Rules (global):**
- DO NOT add features beyond what this spec lists (YAGNI)
- DO NOT refactor unrelated code (KISS)
- RUN `<typecheck command>` after each phase — must exit 0

**Critic findings baked into rules** *(paste concern/fail items from critic here; delete if no critic spawned):*
- <caveat>

---

## Alternatives Considered *(MANDATORY — at least 2 entries)*

The planner must enumerate ≥ 2 plausible approaches and explain why each was rejected.

### Alternative A — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`
- **Tradeoff:** <concrete>
- **Rejected because:** <reason>

### Alternative B — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`
- **Tradeoff:** <concrete>
- **Rejected because:** <reason>

### Picked approach — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** <mmcg evidence>
- **Chosen because:** <concrete reason>

---

## Decision Matrix

| Option | Correctness | Complexity | Blast radius | Migration risk | Observability | Reversibility | Verdict |
|---|---|---|---|---|---|---|---|
| A — <name> | pass | low | low | none | good | easy | reject |
| B — <name> | concern | medium | high | medium | weak | hard | reject |
| C — <name> | pass | medium | low | none | good | easy | **chosen** |

Column values: `pass / concern / fail` for Correctness; `low / medium / high` for complexity/blast/migration; `good / weak / none` for observability; `easy / medium / hard` for reversibility. Exactly one row is `chosen`.

---

## Pre-edit snapshot *(filled by planner via mmcg)*

<!-- delete if no code symbols touched -->
- `<symbol>` — <N> callers (mmcg_callers), signature `<sig>` (mmcg_search)

---

## Phase 1: <outcome>

### 1.1 <action>

**File:** `<path/to/file.ext>`

FIND:
```
<exact existing code>
```

CHANGE TO:
```
<new code>
```

VERIFY: `<command>`

---

## Phase N: Final verification

```bash
<typecheck command>
<test command>
```

---

## Tests Plan *(MANDATORY)*

- **<test name>** in `<test file>` — covers <case>. Asserts <expected>.

---

## Documentation Plan *(MANDATORY)*

- [ ] **CHANGELOG** — new entry under `[Unreleased]`
- [ ] **No external doc changes needed** — <reason>

---

## Observability Plan *(MANDATORY)*

- **On success:** <log line / metric>
- **On failure:** <error log>
- n/a — no production runtime

---

## Performance Considerations *(MANDATORY)*

- **Call frequency:** <per-request / one-time / etc.>
- n/a — not hot path

---

## Notes

### Pre-flight validation

- [ ] All **File:** paths exist in the working tree
- [ ] All named symbols verified via `mmcg_search`
- [ ] All FIND: blocks match current file contents (whitespace-sensitive)
- [ ] `mmcg_impact` on each changed symbol agrees with stated scope
- [ ] VERIFY: commands are runnable
- [ ] **Alternatives Considered has ≥ 2 entries**
- [ ] **Codeflow diagrams** present, nodes mmcg-verified or marked `[NEW]`
- [ ] **Decision Matrix** filled — exactly one row is `chosen`
- [ ] **Pre-edit snapshot** filled via mmcg (or deleted if no code symbols)
- [ ] **Tests Plan** is concrete
- [ ] **Documentation Plan** lists every doc touched
- [ ] **Observability Plan** addressed or marked n/a
- [ ] **Performance Considerations** addressed or marked n/a

### Design-time critic verdict

- **Spawn:** <YYYY-MM-DD> — brief: <summary>
- **Aggregate verdict:** `<ship it | ship with caveats | revise | rethink>`
- **Dimension scores:** <paste 7-row table>
"
    )
}

fn render_strict(description: &str, id: &str) -> String {
    format!(
        "\
---
id: \"{id}\"
title: {description}
mode: strict
risk: high

touches:
  - file: <path/to/file.ext>
    language: <python|typescript|rust|csharp|go|java|php|cpp>
    symbols:
      - name: <symbol>
        signature: \"<exact signature>\"
        callers: 0

verify:
  - cmd: \"<typecheck command>\"
  - cmd: \"<test command>\"

expected_docs: []

breaking_changes:
  removed_symbols: []
---

# Task {id}: {description}

## LLM Agent Directives

You are implementing {description}.

**Goals:**
1. <primary goal — what counts as done>

**Rules (global):**
- DO NOT add features beyond what this spec lists (YAGNI)
- DO NOT refactor unrelated code (KISS)
- DO NOT introduce breaking changes without explicit ack in frontmatter `breaking_changes`
- RUN `<typecheck command>` after each phase — must exit 0
- VERIFY `mmcg_callers` count stays consistent on touched symbols

**Critic findings baked into rules** *(paste concern/fail items from all 3 critic lenses here):*
- <security caveat>
- <performance caveat>
- <simplicity caveat>

---

## Alternatives Considered *(MANDATORY — at least 2 entries)*

The planner must enumerate ≥ 2 plausible approaches and explain why each was rejected.

### Alternative A — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`, `mmcg_callers <symbol>` → `<N> callers`
- **Tradeoff:** <concrete>
- **Rejected because:** <reason tied to mmcg findings or project constraint>

### Alternative B — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** `mmcg_search <symbol>` → `<file:line>`
- **Tradeoff:** <concrete>
- **Rejected because:** <reason>

### Picked approach — <name>

```mermaid
flowchart TD
  <real_symbol_or_file> --> <real_symbol_or_file>
```

- **Grounding:** <mmcg evidence>
- **Chosen because:** <concrete reason>

---

## Decision Matrix

| Option | Correctness | Complexity | Blast radius | Migration risk | Observability | Reversibility | Verdict |
|---|---|---|---|---|---|---|---|
| A — <name> | pass | low | low | none | good | easy | reject |
| B — <name> | concern | medium | high | medium | weak | hard | reject |
| C — <name> | pass | medium | low | none | good | easy | **chosen** |

Column values: `pass / concern / fail` for Correctness; `low / medium / high` for complexity/blast/migration; `good / weak / none` for observability; `easy / medium / hard` for reversibility. Exactly one row is `chosen`.

---

## Risk Register *(MANDATORY for strict)*

| Risk | Probability | Impact | Evidence | Mitigation | Owner phase |
|---|---|---|---|---|---|
| breaks existing callers | medium | high | `mmcg_callers X → N` | preserve signature, add compat wrapper | Phase 1 |
| <risk> | <low/medium/high> | <low/medium/high> | <evidence> | <mitigation> | Phase N |

---

## Pre-edit snapshot *(filled by planner via mmcg)*

- `<symbol>` — <N> callers (mmcg_callers), signature `<sig>` (mmcg_search)
- `<another_symbol>` — <N> callers, signature `<sig>`

---

## Evidence Ledger *(MANDATORY for strict)*

| Claim | Evidence type | Evidence | Confidence |
|---|---|---|---|
| `<symbol>` has N callers | mmcg | `mmcg_callers <symbol> → N` | high |
| `<file>` contains `<pattern>` | file | `grep '<pattern>' <file>` | high |
| <claim> | assumption | <what was assumed and why> | medium |

---

## Phase 1: <outcome>

### 1.1 <action>

**File:** `<path/to/file.ext>`

**Pre-edit check via mmcg** *(executor runs mmcg_callers before editing):*
- Expected callers: ≤ <N> (planner verified during pre-flight)

FIND:
```
<exact existing code>
```

CHANGE TO:
```
<new code>
```

VERIFY: `<command>`

---

## Phase N: Final verification

```bash
<typecheck command>
<test command>
<integration test or smoke command>
```

---

## Tests Plan *(MANDATORY)*

- **<test name>** in `<test file>` — covers <case>. Asserts <expected>.

---

## Documentation Plan *(MANDATORY)*

- [ ] **CHANGELOG** — new entry under `[Unreleased]`
- [ ] **API docs** — `<file:line>` for `<symbol>`
- [ ] **`CONTEXT.md`** — decision log entry

---

## Observability Plan *(MANDATORY)*

- **On success:** <log line / metric / span>
- **On failure:** <error log / alert>
- **Health probes affected:** <none / updated>

---

## Performance Considerations *(MANDATORY)*

- **Call frequency:** <per-request / per-second / etc.>
- **Time complexity:** <O(1) / O(n) / etc.>
- **Risks at scale:** <none / lock contention / etc.>

---

## Rollback / Migration

- **Rollback steps:** <ordered steps to revert if this goes wrong>
- **Migration required:** <yes/no — schema change, data backfill, etc.>
- **Rollback window:** <when rollback is still safe — e.g., before first deploy to prod>

---

## Notes

### Pre-flight validation

- [ ] All **File:** paths exist in the working tree
- [ ] All named symbols verified via `mmcg_search`
- [ ] All FIND: blocks match current file contents (whitespace-sensitive)
- [ ] `mmcg_impact` on each changed symbol agrees with stated scope
- [ ] VERIFY: commands are runnable
- [ ] **Alternatives Considered has ≥ 2 entries**
- [ ] **Codeflow diagrams** present, all nodes mmcg-verified or marked `[NEW]`
- [ ] **Decision Matrix** filled — exactly one row is `chosen`
- [ ] **Risk Register** filled — every high-impact risk has a mitigation
- [ ] **Evidence Ledger** filled — every non-trivial claim has a row, assumptions explicit
- [ ] **Pre-edit snapshot** filled via mmcg for every edited function/method
- [ ] **Tests Plan** is concrete (per-test what's covered)
- [ ] **Documentation Plan** lists every doc touched
- [ ] **Observability Plan** addressed
- [ ] **Performance Considerations** addressed
- [ ] **Rollback / Migration** section complete

### Design-time critic verdict (3-lens panel — MANDATORY for strict)

**Security lens:**
- **Spawn:** <YYYY-MM-DD> — brief: <summary>
- **Aggregate verdict:** `<ship it | ship with caveats | revise | rethink>`
- **Dimension scores:** <paste 7-row table>

**Performance lens:**
- **Spawn:** <YYYY-MM-DD>
- **Aggregate verdict:** `<verdict>`
- **Dimension scores:** <paste 7-row table>

**Simplicity lens:**
- **Spawn:** <YYYY-MM-DD>
- **Aggregate verdict:** `<verdict>`
- **Dimension scores:** <paste 7-row table>

**Combined verdict:** `<ship it | ship with caveats | revise | rethink>`
**Planner's disagreements (if any):** <if planner overrode any critic finding, document why>
"
    )
}
