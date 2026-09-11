use std::path::Path;

pub enum Mode {
    Lite,
    Standard,
    Verified,
    Strict,
}

impl Mode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "lite" => Ok(Mode::Lite),
            "standard" => Ok(Mode::Standard),
            "verified" => Ok(Mode::Verified),
            "direct" => Err(
                "direct mode does not create a task spec — use map/impact/tests and implement directly"
                    .into(),
            ),
            "strict" => Ok(Mode::Strict),
            other => Err(format!(
                "unknown mode {other:?} — use `verified` or `strict` (`lite`/`standard` remain legacy-compatible)"
            )),
        }
    }
}

pub fn run(description: &str, mode: Mode, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let slug = slugify(description);
    let spec_path = mmcg::task_scaffold::create_numbered_spec(root, &slug, |number| {
        render_spec(description, number, &mode)
    })?;

    println!("Created {}", spec_path.display());
    Ok(())
}

fn slugify(s: &str) -> String {
    let slug: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(40).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        "task".to_string()
    } else {
        slug
    }
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn render_spec(description: &str, n: u32, mode: &Mode) -> String {
    let id = format!("{:03}", n);
    match mode {
        Mode::Lite => render_lite(description, &id),
        Mode::Standard => render_standard(description, &id),
        Mode::Verified => render_verified(description, &id),
        Mode::Strict => render_strict(description, &id),
    }
}

fn render_verified(description: &str, id: &str) -> String {
    let title_yaml = yaml_quote(description);
    format!(
        "\
---
id: \"{id}\"
title: {title_yaml}
mode: verified
risk: medium

touches:
  - file: <path/to/file.ext>
    symbols: []

verify:
  - cmd: \"<focused test command>\"
  - cmd: \"<repository-required gate>\"

creates: []
expected_docs: []
---

# Task {id}: {description}

## Goals

- {description}
- <observable definition of done>

## Scope

- Change: `<path/to/file.ext>` — <intended outcome>
- Do not change: <boundary>

## Acceptance Criteria

- [ ] <behavior that can be observed or asserted>
- [ ] Existing relevant behavior remains compatible

## Pre-edit Snapshot

<!-- Delete for docs/config-only tasks. Record only symbols this task changes. -->
- `<symbol>` — <N> callers; signature `<signature>`

## Implementation Plan

1. <outcome-oriented change; exact FIND/CHANGE blocks are optional>
2. Add or update <test coverage>

## Tests Plan

- `<test name or command>` — proves <acceptance criterion>

## Final Verification

```bash
<focused test command>
<repository-required gate>
```

## Notes

- Assumptions: <only load-bearing assumptions, or none>
- Alternatives: <include only when a real design choice existed>
- Observability/performance/docs: <material impact, or n/a>
"
    )
}

fn render_lite(description: &str, id: &str) -> String {
    let title_yaml = yaml_quote(description);
    format!(
        "\
---
id: \"{id}\"
title: {title_yaml}
mode: lite
risk: low
creates: []
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

- [ ] Existing **File:** paths exist; additions are declared in `creates`
- [ ] FIND: blocks match current file contents
- [ ] VERIFY: commands are runnable
"
    )
}

fn render_standard(description: &str, id: &str) -> String {
    let title_yaml = yaml_quote(description);
    format!(
        "\
---
id: \"{id}\"
title: {title_yaml}
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

creates: []
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

## Alternatives Considered

Include only plausible alternatives for a real design choice. Remove unused
placeholders and explain when no meaningful alternative exists.

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

- [ ] Existing **File:** paths exist; additions are declared in `creates`
- [ ] All named symbols verified via `mmcg_search`
- [ ] All FIND: blocks match current file contents (whitespace-sensitive)
- [ ] `mmcg_impact` on each changed symbol agrees with stated scope
- [ ] VERIFY: commands are runnable
- [ ] **Alternatives Considered covers the real choice without filler**
- [ ] **Codeflow diagrams** present, nodes mmcg-verified or marked `[NEW]`
- [ ] **Decision Matrix** filled — exactly one row is `chosen`
- [ ] **Pre-edit snapshot** filled via mmcg (or deleted if no code symbols)
- [ ] **Tests Plan** is concrete
- [ ] **Documentation Plan** lists every doc touched
- [ ] **Observability Plan** addressed or marked n/a
- [ ] **Performance Considerations** addressed or marked n/a

### Design-time critic verdict

- **Spawn:** <YYYY-MM-DD> — brief: <summary>
- **Aggregate verdict:** `<ship it | ship with caveats | revise | rethink | insufficient evidence>`
- **Dimension scores:** <paste 7-row table>
"
    )
}

fn render_strict(description: &str, id: &str) -> String {
    let title_yaml = yaml_quote(description);
    format!(
        "\
---
id: \"{id}\"
title: {title_yaml}
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

creates: []
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

**Critic findings baked into rules** *(paste evidenced concern/fail items here):*
- <security caveat>
- <performance caveat>
- <simplicity caveat>

---

## Alternatives Considered

Include only plausible alternatives for a real design choice. Remove unused
placeholders and explain when no meaningful alternative exists.

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
- [ ] **Project history** — update `CONTEXT.md` only for durable knowledge;
      otherwise resolve `history-review.md` as `not applicable`

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

- [ ] Existing **File:** paths exist; additions are declared in `creates`
- [ ] All named symbols verified via `mmcg_search`
- [ ] All FIND: blocks match current file contents (whitespace-sensitive)
- [ ] `mmcg_impact` on each changed symbol agrees with stated scope
- [ ] VERIFY: commands are runnable
- [ ] **Alternatives Considered covers the real choice without filler**
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

### Design-time critic verdict (independent review — required for strict)

Use one critic by default. Add separate security/performance/simplicity lenses
only when their questions are independent; omit unused lens sections.

**Primary review:**
- **Spawn:** <YYYY-MM-DD> — brief: <summary>
- **Aggregate verdict:** `<ship it | ship with caveats | revise | rethink | insufficient evidence>`
- **Dimension scores:** <paste 7-row table>

**Additional independent lenses:** <results if needed, otherwise remove>

**Combined verdict:** `<ship it | ship with caveats | revise | rethink | insufficient evidence>`
Do not average away blocking findings. Resolve evidenced failures first, then
material unknowns. `insufficient evidence` returns to research before acceptance.
**Planner's disagreements (if any):** <if planner overrode any critic finding, document why>
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_ascii_basic() {
        assert_eq!(slugify("fix the context doctor"), "fix-the-context-doctor");
    }

    #[test]
    fn slugify_unicode_no_panic() {
        let s = slugify("починить проверку контекста и аудит");
        assert!(!s.is_empty());
        assert!(s.is_ascii(), "slug must be ASCII-only, got: {s:?}");
    }

    #[test]
    fn slugify_all_unicode_falls_back_to_task() {
        assert_eq!(slugify("проверка"), "task");
    }

    #[test]
    fn slugify_long_unicode_does_not_panic() {
        let long = "абвгдеёжзийклмнопрстуфхцчшщъыьэюяabcde";
        let s = slugify(long);
        assert!(s.len() <= 40);
        assert!(s.is_ascii());
    }

    #[test]
    fn slugify_truncates_at_40_chars_cleanly() {
        let long = "a".repeat(60);
        let s = slugify(&long);
        assert!(s.len() <= 40);
    }

    #[test]
    fn yaml_quote_plain() {
        assert_eq!(yaml_quote("hello world"), "\"hello world\"");
    }

    #[test]
    fn yaml_quote_colon_in_title() {
        let q = yaml_quote("fix: context doctor");
        assert_eq!(q, "\"fix: context doctor\"");
    }

    #[test]
    fn yaml_quote_inner_double_quote() {
        let q = yaml_quote(r#"fix "broken" audit"#);
        assert_eq!(q, r#""fix \"broken\" audit""#);
    }

    #[test]
    fn lite_spec_with_colon_title_keeps_lite_mode() {
        let content = render_spec("fix: context doctor", 1, &Mode::Lite);
        assert!(
            content.contains("mode: lite"),
            "mode: lite must survive colon-in-title; got:\n{content}"
        );
        assert!(
            content.contains("title: \"fix: context doctor\""),
            "title must be quoted; got:\n{content}"
        );
    }

    #[test]
    fn new_spec_frontmatter_quotes_colon_title() {
        for mode in [Mode::Lite, Mode::Standard, Mode::Verified, Mode::Strict] {
            let content = render_spec("feat: add new thing", 1, &mode);
            assert!(
                content.contains("title: \"feat: add new thing\""),
                "title must be quoted in all modes; got:\n{content}"
            );
            assert!(content.contains("creates: []"), "{content}");
            let parsed = mmcg::spec::parse_str("spec.md", &content);
            assert!(parsed.frontmatter_error.is_none());
            assert!(parsed.frontmatter.unwrap().creates.is_empty());
        }
    }

    #[test]
    fn verified_is_the_compact_default_contract() {
        let content = render_spec("change behavior", 7, &Mode::Verified);
        assert!(content.contains("mode: verified"));
        assert!(content.contains("## Acceptance Criteria"));
        assert!(content.contains("## Final Verification"));
        assert_eq!(
            mmcg::spec::parse_str("spec.md", &content).declared_verify_commands(),
            ["<focused test command>", "<repository-required gate>"]
        );
        assert!(!content.contains("Decision Matrix"));
        assert!(!content.contains("Risk Register"));
    }

    #[test]
    fn direct_mode_does_not_create_a_spec() {
        let error = Mode::from_str("direct").err().expect("direct is spec-free");
        assert!(error.contains("does not create a task spec"));
    }
}
