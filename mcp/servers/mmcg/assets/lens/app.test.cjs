"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const APP_SOURCE = fs.readFileSync(path.join(__dirname, "app.js"), "utf8");
const HTML_SOURCE = fs.readFileSync(path.join(__dirname, "index.html"), "utf8");
const CSS_SOURCE = fs.readFileSync(path.join(__dirname, "styles.css"), "utf8");
const FACT_SOURCE_ID = "facts:sha256:" + "f".repeat(64);

class TokenList {
  constructor() {
    this.values = new Set();
  }

  set(value) {
    this.values = new Set(String(value).split(/\s+/).filter(Boolean));
  }

  add(...values) {
    values.forEach((value) => this.values.add(value));
  }

  remove(...values) {
    values.forEach((value) => this.values.delete(value));
  }

  contains(value) {
    return this.values.has(value);
  }

  toggle(value, force) {
    const enabled = force === undefined ? !this.contains(value) : Boolean(force);
    if (enabled) {
      this.add(value);
    } else {
      this.remove(value);
    }
    return enabled;
  }

  toString() {
    return Array.from(this.values).join(" ");
  }
}

class MockElement {
  constructor(tagName) {
    this.tagName = tagName;
    this.children = [];
    this.parentNode = null;
    this.attributes = new Map();
    this.dataset = {};
    this.classList = new TokenList();
    this.listeners = {};
    this.hidden = false;
    this.disabled = false;
    this.value = "";
    this.clientWidth = 375;
    this.ownText = "";
  }

  set className(value) {
    this.classList.set(value);
  }

  get className() {
    return this.classList.toString();
  }

  set textContent(value) {
    this.ownText = String(value ?? "");
    this.children = [];
  }

  get textContent() {
    return this.ownText + this.children.map((child) => child.textContent || "").join("");
  }

  set innerHTML(_value) {
    throw new Error("Unsafe innerHTML write");
  }

  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  replaceChildren(...children) {
    this.children = [];
    this.ownText = "";
    children.forEach((child) => this.appendChild(child));
  }

  get childElementCount() {
    return this.children.filter((child) => child instanceof MockElement).length;
  }

  setAttribute(name, value) {
    const stringValue = String(value);
    this.attributes.set(name, stringValue);
    if (name === "class") {
      this.className = stringValue;
    }
    if (name.startsWith("data-")) {
      const key = name.slice(5).replace(/-([a-z])/g, (_match, character) => character.toUpperCase());
      this.dataset[key] = stringValue;
    }
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  removeAttribute(name) {
    this.attributes.delete(name);
  }

  addEventListener(type, handler) {
    if (!this.listeners[type]) {
      this.listeners[type] = [];
    }
    this.listeners[type].push(handler);
  }

  dispatch(type, properties) {
    const event = Object.assign({
      type: type,
      target: this,
      preventDefault() {},
    }, properties || {});
    (this.listeners[type] || []).forEach((handler) => handler(event));
  }

  focus() {
    this.focused = true;
  }

  scrollTo() {}

  closest(selector) {
    let current = this;
    while (current) {
      if (selector.startsWith(".") && current.classList.contains(selector.slice(1))) {
        return current;
      }
      current = current.parentNode;
    }
    return null;
  }

  querySelectorAll(selector) {
    const matches = (element) => {
      if (!(element instanceof MockElement)) {
        return false;
      }
      if (selector.startsWith(".")) {
        return element.classList.contains(selector.slice(1));
      }
      const attribute = selector.match(/^\[([^\]]+)\]$/);
      if (attribute) {
        return element.attributes.has(attribute[1]);
      }
      return element.tagName === selector.toLowerCase();
    };
    const results = [];
    const visit = (element) => {
      element.children.forEach((child) => {
        if (matches(child)) {
          results.push(child);
        }
        if (child instanceof MockElement) {
          visit(child);
        }
      });
    };
    visit(this);
    return results;
  }
}

class MockText {
  constructor(value) {
    this.textContent = String(value);
    this.parentNode = null;
  }
}

const ELEMENT_IDS = [
  "repository-name", "repository-root", "baseline-ref", "baseline-oid", "head-oid",
  "refresh-button", "snapshot-age", "notice-stack", "instrument-summary", "completeness-status", "trace-search",
  "clear-search", "fit-button", "review-workspace", "component-list", "graph-frame", "trace-graph", "graph-state",
  "trace-context", "mobile-trace-list", "trace-count", "inspector-body", "precision-list",
  "precision-count", "limits-list", "limits-count", "schema-label", "status-region",
  "evidence-summary", "evidence-source-list",
  "temporal-summary", "temporal-events", "temporal-components", "temporal-boundaries",
  "temporal-cycles", "temporal-centrality", "temporal-ownership", "temporal-history",
  "metric-files", "metric-files-note", "metric-symbols", "metric-symbols-note", "metric-impact",
  "metric-impact-note", "metric-crossings", "metric-crossings-note", "metric-tests",
  "metric-tests-note", "method-ledger", "method-ledger-disclosure", "method-ledger-toggle",
  "audit-board", "audit-summary", "audit-explain", "audit-structural", "audit-health", "audit-security",
  "audit-change", "audit-production",
  "audit-verdict", "audit-verdict-word", "audit-lede", "audit-pillars", "audit-treemap", "audit-map-mobile",
  "audit-map-note",
  "audit-explain-sev", "audit-structural-sev", "audit-health-sev", "audit-change-sev", "audit-security-sev",
  "audit-bugs", "audit-bus", "audit-bugs-sev", "audit-bus-sev",
  "audit-domain-card", "audit-domain", "audit-domain-sev", "audit-redteam", "audit-redteam-body",
];

function createDocument() {
  const nodes = new Map(ELEMENT_IDS.map((id) => [id, new MockElement("div")]));
  nodes.forEach((node, id) => {
    node.id = id;
  });
  ["files", "symbols", "impact", "crossings", "tests"].forEach((name) => {
    const metric = new MockElement("article");
    metric.className = "metric";
    metric.appendChild(nodes.get("metric-" + name));
    metric.appendChild(nodes.get("metric-" + name + "-note"));
  });
  const scopeButtons = ["all", "changed", "impacted", "test"].map((scope) => {
    const button = new MockElement("button");
    button.dataset.scope = scope;
    button.setAttribute("data-scope", scope);
    return button;
  });
  const overlayButtons = ["findings", "coverage", "ownership", "churn", "tests", "semantic", "runtime", "facts", "knowledge"].map((overlay) => {
    const button = new MockElement("button");
    button.dataset.overlay = overlay;
    button.setAttribute("data-overlay", overlay);
    button.setAttribute("aria-pressed", "true");
    return button;
  });
  const modeButtons = ["review", "audit"].map((mode) => {
    const button = new MockElement("button");
    button.dataset.mode = mode;
    button.setAttribute("data-mode", mode);
    button.setAttribute("aria-pressed", mode === "review" ? "true" : "false");
    return button;
  });
  nodes.set("mode-review", modeButtons[0]);
  nodes.set("mode-audit", modeButtons[1]);
  return {
    nodes: nodes,
    scopeButtons: scopeButtons,
    overlayButtons: overlayButtons,
    modeButtons: modeButtons,
    document: {
      body: new MockElement("body"),
      getElementById(id) {
        return nodes.get(id);
      },
      querySelectorAll(selector) {
        if (selector === "[data-scope]") {
          return scopeButtons;
        }
        if (selector === "[data-mode]") {
          return modeButtons;
        }
        return selector === "[data-overlay]" ? overlayButtons : [];
      },
      createElement(tagName) {
        return new MockElement(tagName);
      },
      createElementNS(_namespace, tagName) {
        return new MockElement(tagName);
      },
      createTextNode(value) {
        return new MockText(value);
      },
    },
  };
}

function fixture() {
  const changed = {
    file: "src/auth.rs",
    name: "authorize",
    kind: "function",
    line: 42,
    change: "body_changed",
  };
  return {
    schema_version: 1,
    repository: { name: "example", root_label: "." },
    options: { since: "main", path: ".", depth: 3, top: 100, production_only: false },
    temporal: {
      status: "available",
      data: {
        schema_version: 1,
        baseline: { requested_ref: "main" },
        summary: {
          architecture_changed: true,
          components_added: 1, components_removed: 0,
          boundaries_added: 1, boundaries_removed: 0, boundaries_changed: 0,
          cycles_introduced: 1, cycles_resolved: 0,
          centrality_increases: 1, hotspot_entries: 1, hotspot_exits: 0,
          cycles_changed: 0,
          ownership_changes: 1, history_review_candidates: 1,
        },
        components: {
          added: { total: 1, returned: 1, truncated: false, items: [{ path: "payments", file_count: 1, languages: [{ language: "rust", file_count: 1 }] }] },
          removed: { total: 1, returned: 1, truncated: false, items: [{ path: "legacy", file_count: 1, languages: [{ language: "python", file_count: 1 }] }] },
          changed: { total: 0, returned: 0, truncated: false, items: [] },
        },
        boundaries: {
          added: { total: 1, returned: 1, truncated: false, items: [{ component: "payments", file: "src/auth.rs", name: "charge</span>", kind: "function", line: 42 }] },
          removed: { total: 0, returned: 0, truncated: false, items: [] },
          changed: { total: 0, returned: 0, truncated: false, items: [] },
        },
        cycles: {
          added: { total: 1, returned: 1, truncated: false, items: [["payments/api.rs", "ledger/api.rs"]] },
          removed: { total: 1, returned: 1, truncated: false, items: [["legacy/a.py", "legacy/b.py"]] },
          changed: { total: 0, returned: 0, truncated: false, items: [] },
        },
        centrality: { total: 1, returned: 1, truncated: false, items: [{ file: "src/auth.rs", name: "authorize", kind: "function", base_in_degree: 2, head_in_degree: 5, in_degree_delta: 3 }] },
        hotspots: {
          entered: { total: 1, returned: 1, truncated: false, items: [{ file: "src/auth.rs", name: "authorize", kind: "function", rank: 1, in_degree: 5 }] },
          exited: { total: 1, returned: 1, truncated: false, items: [{ file: "legacy/auth.py", name: "legacy_auth", kind: "function", rank: 2, in_degree: 3 }] },
          moved: { total: 1, returned: 1, truncated: false, items: [] },
        },
        ownership: {
          changes: { total: 1, returned: 1, truncated: false, items: [{ path: "src/auth.rs", base_owners: ["@platform"], head_owners: ["@security"] }] },
        },
        history_review_candidates: { total: 1, returned: 1, truncated: false, items: [{ artifact_path: "docs/adr/004-auth.md", referenced_path: "src/legacy.rs", trigger: "path_deleted" }] },
        limits: { ownership_paths: 500 },
        diagnostics: [],
        partial: false,
      },
    },
    semantic: {
      schema_version: 1,
      available: true,
      partial: false,
      fallback_active: false,
      source: {
        format: "scip", tool_name: "rust-analyzer", tool_version: "test",
        documents: 2, definitions: 2, edges: 1, text_verified_documents: 2,
        repository_verified: true, revision_verified: true,
      },
      definitions: { total: 0, returned: 0, truncated: false, items: [] },
      edges: {
        total: 1,
        returned: 1,
        truncated: false,
        items: [{
          from_symbol: "rust-analyzer . example . authorize_target().",
          from_display_name: "authorize_target",
          from_file: "src/target.rs", from_line: 7, from_character: 4,
          occurrence_line: 9, occurrence_character: 6,
          to_symbol: "rust-analyzer . example . authorize().",
          to_display_name: "authorize",
          to_file: "src/auth.rs", to_line: 42, to_character: 4,
          kind: "reference", provenance: "scip", confidence: "high",
        }],
      },
      diagnostics: [],
      resolution: { default_graph: "tree-sitter", static_precedence: ["scip", "tree-sitter"], runtime_confidence: "observed", fallback_without_scip: "tree-sitter" },
    },
    evidence: {
      schema_version: 1,
      partial: false,
      sources: {
        total: 8,
        returned: 8,
        truncated: false,
        items: [
          { id: "sarif:0", kind: "sarif", label: "semgrep.sarif", status: "loaded", facts_total: 1, facts_returned: 1, files_matched: 1 },
          { id: "coverage:0", kind: "coverage", label: "lcov.info", status: "loaded", facts_total: 2, facts_returned: 2, files_matched: 1 },
          { id: "codeowners", kind: "codeowners", label: ".github/CODEOWNERS", status: "loaded", facts_total: 3, facts_returned: 3, files_matched: 3 },
          { id: "git-history", kind: "git_history", label: "Git · last 200 commits", status: "loaded", facts_total: 3, facts_returned: 3, files_matched: 2 },
          { id: "junit:0", kind: "junit", label: "junit.xml", status: "loaded", facts_total: 1, facts_returned: 1, files_matched: 1 },
          { id: "otel:0", kind: "otel", label: "traces.json", status: "loaded", facts_total: 2, facts_returned: 2, files_matched: 2 },
          { id: "project-knowledge", kind: "project_knowledge", label: "Indexed project knowledge", status: "loaded", facts_total: 1, facts_returned: 1, files_matched: 1 },
          { id: FACT_SOURCE_ID, kind: "facts", label: "com.example.arch-lint 1.4.0 · default", status: "loaded", facts_total: 3, facts_returned: 3, files_matched: 2, artifact_sha256: "a".repeat(64), artifact_bytes: 1024 },
        ],
      },
      files: {
        total: 3,
        returned: 3,
        truncated: false,
        items: [
          {
            path: "src/auth.rs",
            production: true,
            findings: [
              { source_id: "sarif:0", tool: "Semgrep", rule_id: "auth.bypass", level: "error", message: "Authorization result is ignored", line: 42, column: 3 },
              { source_id: FACT_SOURCE_ID, tool: "com.example.arch-lint", rule_id: "architecture.boundary", level: "warning", message: "Payment boundary crossed", line: 42, column: 3 },
            ],
            coverage: { source_ids: ["coverage:0"], lines_found: 2, lines_hit: 1 },
            ownership: { codeowners_source_id: "codeowners", codeowners: ["@security"], contributors: [{ name: "Alice", commits: 2 }] },
            churn: { commits: 2, lines_added: 7, lines_deleted: 3 },
            test_results: {
              source_ids: ["junit:0"], total: 1, passed: 0, failed: 1, errors: 0, skipped: 0,
              duration_ms: 11, failures_truncated: false,
              failures: [{ source_id: "junit:0", name: "authorization rejects", class_name: "AuthTest", status: "failed", message: "expected denial" }],
            },
            runtime: { source_ids: ["otel:0"], spans: 1, traces: 1 },
            knowledge: [{
              source_id: "project-knowledge", artifact_path: "docs/adr/004-auth.md",
              kind: "architecture_decision", title: "Auth boundary", match_kind: "exact_path",
              excerpt: "Keep src/auth.rs behind the security boundary.",
            }],
          },
          {
            path: "src/target.rs",
            production: true,
            findings: [],
            ownership: { codeowners_source_id: "codeowners", codeowners: [], contributors: [] },
            runtime: { source_ids: ["otel:0"], spans: 1, traces: 1 },
            knowledge: [],
          },
          {
            path: "tests/auth.rs",
            production: false,
            findings: [],
            ownership: { codeowners_source_id: "codeowners", codeowners: [], contributors: [{ name: "Bob", commits: 1 }] },
            churn: { commits: 1, lines_added: 4, lines_deleted: 0 },
            knowledge: [],
          },
        ],
      },
      runtime_edges: {
        total: 1,
        returned: 1,
        truncated: false,
        items: [{
          source_ids: ["otel:0"], parent_file: "src/auth.rs", child_file: "src/target.rs",
          spans: 1, traces: 1, span_names: ["authorize_target"], names_truncated: false,
        }],
      },
      fact_artifacts: {
        total: 1,
        returned: 1,
        truncated: false,
        items: [{
          source_id: FACT_SOURCE_ID,
          id: "arch-lint-output",
          path: "reports/arch-lint.json",
          sha256: "b".repeat(64),
          bytes: 4312,
        }],
      },
      fact_relationships: {
        total: 2,
        returned: 2,
        truncated: false,
        items: [
          {
            source_id: FACT_SOURCE_ID,
            fact_id: "architecture.auth-to-target",
            relation: "calls",
            from_path: "src/auth.rs",
            from_line: 42,
            from_column: 3,
            to_path: "src/target.rs",
            to_line: 7,
            to_column: 1,
            confidence: "high",
            label: "Compiler-resolved auth to target call",
          },
          {
            source_id: FACT_SOURCE_ID,
            fact_id: "architecture.unrelated-lines",
            relation: "calls",
            from_path: "src/auth.rs",
            from_line: 999,
            to_path: "src/target.rs",
            to_line: 998,
            confidence: "high",
            label: "Wrong-line relationship must not decorate the edge",
          },
        ],
      },
      diagnostics: { total: 0, returned: 0, truncated: false, items: [] },
      limits: { artifact_bytes: 33554432, artifact_sources: 64, relevant_files: 1000, findings: 5000, findings_per_file: 100, coverage_lines: 500000, test_cases: 100000, runtime_spans: 100000, runtime_edges: 1000, normalized_fact_sources: 64, normalized_fact_artifacts: 64, normalized_fact_relationships: 200, knowledge_matches: 500, codeowner_rules: 50000, codeowners_bytes: 3145728, owners_per_rule: 50, contributors_per_file: 5, diagnostics: 100, git_commits: 200 },
    },
    map: {
      scope: { aggregation_paths_truncated: false },
      files: { total: 4, returned: 4, truncated: false, items: [] },
      languages: { total: 1, returned: 1, truncated: false, items: [{ language: "rust", file_count: 3 }] },
      components: {
        total: 2,
        returned: 2,
        truncated: false,
        items: [
          { path: "src", file_count: 3, languages: [], boundaries: { total: 0, returned: 0, truncated: false, items: [] } },
          { path: "tests", file_count: 1, languages: [], boundaries: { total: 0, returned: 0, truncated: false, items: [] } },
        ],
      },
      entry_points: { total: 1, returned: 1, truncated: false, items: [{ file: "src/main.rs", classification: "direct", evidence: { kind: "filename", matched: "main.rs" } }] },
      hotspots: { total: 1, returned: 1, truncated: false, items: [{ name: "authorize", kind: "function", file: "src/auth.rs", line: 42, in_degree: 5, name_collision: 1, edge_precision: [] }] },
      cycles: { total: 0, returned: 0, truncated: false, items: [] },
      limits: {},
      precision_notes: [],
    },
    impact: {
      baseline: {
        requested_ref: "main",
        baseline_oid: "1111111111111111111111111111111111111111",
        head_oid: "2222222222222222222222222222222222222222",
        includes_worktree: true,
        includes_untracked: true,
      },
      changes: {
        files: { total: 1, returned: 1, truncated: false, items: [{ path: changed.file, status: "modified" }] },
        symbols: { total: 1, returned: 1, truncated: false, items: [changed] },
      },
      affected_components: {
        total: 2,
        returned: 2,
        truncated: false,
        items: [
          { component: "src", changed_symbols: 1, impacted_symbols: 0, candidate_tests: 0 },
          { component: "tests", changed_symbols: 0, impacted_symbols: 0, candidate_tests: 1 },
        ],
      },
      impact: {
        total: 1,
        returned: 1,
        truncated: false,
        items: [{
          symbol: { file: "src/target.rs", name: "authorize_target", kind: "function", line: 7 },
          minimum_depth: 1,
          edge_precision: ["syntactic"],
          name_collision_count: 0,
          seeds: [changed],
        }],
      },
      api_crossings: { total: 0, returned: 0, truncated: false, items: [] },
      tests: {
        total: 1,
        returned: 1,
        truncated: false,
        items: [{
          symbol: { file: "tests/auth.rs", name: "login_heuristic", kind: "function", line: 12 },
          classification: "heuristic",
          minimum_depth: 1,
          confidence: "medium",
          evidence: [{ kind: "heuristic", seed: null, component: "tests" }],
        }],
      },
      limits: {},
      precision_notes: [],
    },
    audit: {
      dead_code: {
        total: 3,
        returned: 3,
        truncated: true,
        items: [
          { name: "unused_helper", kind: "function", file: "src/dead.rs", line: 9 },
          { name: "OldWidget", kind: "class", file: "src/old.rs", line: 2 },
          { name: "unused_fixture", kind: "function", file: "tests/fixtures/dead.rs", line: 4 },
        ],
      },
      change_hotspots: {
        status: "available",
        window_commits: 500,
        returned: 2,
        truncated: false,
        items: [
          { file: "src/auth.rs", commits: 9, in_degree: 5, score: 45 },
          { file: "tests/auth.rs", commits: 4, in_degree: 2, score: 8 },
        ],
      },
      largest_files: {
        status: "available",
        returned: 4,
        truncated: false,
        items: [
          { file: "frontend/scripts/highstock.min.js", lines: 60000 },
          { file: "src/report.rs", lines: 4200 },
          { file: "src/auth.rs", lines: 900 },
          { file: "tests/fixtures/big.rs", lines: 5000 },
        ],
      },
      bus_factor: {
        status: "available",
        window_commits: 2000,
        returned: 3,
        truncated: false,
        items: [
          { component: "src/core", authors: 1, touches: 40, top_author_pct: 100 },
          { component: "src", authors: 6, touches: 50, top_author_pct: 62 },
          { component: "tests/e2e", authors: 1, touches: 30, top_author_pct: 100 },
        ],
      },
    },
  };
}

async function renderFixture(payload, options) {
  const harness = createDocument();
  const settings = options || {};
  if (settings.width) {
    harness.nodes.get("graph-frame").clientWidth = settings.width;
  }
  if (settings.embedded) {
    const embedded = new MockElement("script");
    embedded.textContent = JSON.stringify(payload || fixture());
    harness.nodes.set("lens-snapshot", embedded);
  }
  let fetchCalls = 0;
  let releaseFetch = null;
  let intervalHandler = null;
  let now = settings.now === undefined ? Date.now() : settings.now;
  class HarnessDate extends Date {
    constructor(...values) {
      super(...(values.length > 0 ? values : [now]));
    }

    static now() {
      return now;
    }
  }
  const fetchGate = settings.deferFetch
    ? new Promise((resolve) => { releaseFetch = resolve; })
    : null;
  const window = {
    setTimeout(handler) {
      handler();
      return 1;
    },
    setInterval(handler) {
      intervalHandler = handler;
      return 1;
    },
    addEventListener() {},
    ResizeObserver: class {
      observe() {}
    },
  };
  vm.runInNewContext(APP_SOURCE, {
    document: harness.document,
    window: window,
    ResizeObserver: window.ResizeObserver,
    fetch: async () => {
      fetchCalls += 1;
      if (settings.rejectFetch) {
        throw new Error("standalone package attempted a network request");
      }
      if (fetchGate) {
        await fetchGate;
      }
      const configured = Array.isArray(settings.responses)
        ? settings.responses[Math.min(fetchCalls - 1, settings.responses.length - 1)]
        : settings.response;
      if (configured && configured.reject) {
        throw new Error(configured.reject);
      }
      return {
        ok: configured ? configured.ok !== false : true,
        status: configured && configured.status ? configured.status : 200,
        json: async () => {
          if (configured && configured.jsonError) {
            throw new Error("invalid json");
          }
          return configured && Object.prototype.hasOwnProperty.call(configured, "payload")
            ? configured.payload
            : (payload || fixture());
        },
      };
    },
    Intl: Intl,
    Date: HarnessDate,
    Map: Map,
    Set: Set,
    Array: Array,
    Object: Object,
    String: String,
    Number: Number,
    Math: Math,
    JSON: JSON,
    console: console,
  }, { filename: "app.js" });
  const settle = async () => {
    await new Promise((resolve) => setImmediate(resolve));
    await new Promise((resolve) => setImmediate(resolve));
  };
  harness.settle = settle;
  if (settings.deferFetch) {
    harness.releaseFetch = releaseFetch;
  } else {
    await settle();
  }
  harness.fetchCalls = fetchCalls;
  harness.advanceTime = (milliseconds) => {
    now += milliseconds;
    if (intervalHandler) {
      intervalHandler();
    }
  };
  return harness;
}

function cloneFixture() {
  return JSON.parse(JSON.stringify(fixture()));
}

function cloneFixtureValue(value) {
  return JSON.parse(JSON.stringify(value));
}

function emptyFixture(source) {
  const payload = cloneFixtureValue(source || fixture());
  const empty = { total: 0, returned: 0, truncated: false, items: [] };
  payload.impact.changes.files = cloneFixtureValue(empty);
  payload.impact.changes.symbols = cloneFixtureValue(empty);
  payload.impact.impact = cloneFixtureValue(empty);
  payload.impact.api_crossings = cloneFixtureValue(empty);
  payload.impact.tests = cloneFixtureValue(empty);
  payload.impact.affected_components = cloneFixtureValue(empty);
  return payload;
}

function paginatedMobileFixture() {
  const payload = cloneFixture();
  const seed = payload.impact.changes.symbols.items[0];
  payload.impact.changes.symbols.items = Array.from({ length: 6 }, (_value, index) => ({
    ...seed,
    file: "src/change_" + (index + 1) + ".rs",
    name: "changed_" + (index + 1),
    line: index + 1,
  }));
  payload.impact.changes.symbols.total = 6;
  payload.impact.changes.symbols.returned = 6;
  return payload;
}

function relativeLuminance(hex) {
  const channels = hex.match(/[0-9a-f]{2}/gi).map((value) => parseInt(value, 16) / 255);
  return channels.reduce((result, value, index) => {
    const linear = value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
    return result + linear * [0.2126, 0.7152, 0.0722][index];
  }, 0);
}

function contrastRatio(foreground, background) {
  const light = Math.max(relativeLuminance(foreground), relativeLuminance(background));
  const dark = Math.min(relativeLuminance(foreground), relativeLuminance(background));
  return (light + 0.05) / (dark + 0.05);
}

async function main() {
  assert.doesNotMatch(
    APP_SOURCE,
    /innerHTML|outerHTML|insertAdjacentHTML/,
    "Production code must keep repository strings on safe DOM text paths"
  );
  const refreshTag = HTML_SOURCE.match(/<button[^>]*id="refresh-button"[^>]*>/);
  assert.ok(refreshTag, "Refresh button must exist");
  assert.match(refreshTag[0], /aria-label="Refresh Lens snapshot"/, "Refresh needs an explicit mobile-safe accessible name");
  assert.match(
    HTML_SOURCE,
    /default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'/,
    "Lens must keep its strict same-origin CSP"
  );
  assert.doesNotMatch(HTML_SOURCE, /<(?:script|style)[^>]*>\s*[^<\s]/i, "Lens must not add inline executable content");
  assert.doesNotMatch(HTML_SOURCE, /https?:\/\//i, "Lens must remain offline and dependency-free");
  assert.match(HTML_SOURCE, /class="scope-control__hint"[^>]*>Emphasizes the graph · filters the mobile list</i);
  assert.match(HTML_SOURCE, /lane-key__serious[\s\S]*?Serious signal/, "Legend must name the serious tier");
  assert.match(HTML_SOURCE, /lane-key__warning[\s\S]*?Attention/, "Legend must name the attention tier");
  assert.match(HTML_SOURCE, /<summary role="button" aria-label="Evidence overlay filters"/i);
  assert.match(HTML_SOURCE, /<summary[^>]*role="button"[^>]*aria-label="Open the full precision and limits ledger"/i);
  assert.match(
    HTML_SOURCE,
    /class="evidence-canvas"[\s\S]*class="review-brief"[\s\S]*class="review-workbench"/i,
    "Review pulse and graph workbench must share one evidence canvas"
  );
  assert.match(APP_SOURCE, /Run mastermind temporal --since <baseline> --format json/);
  const mobileCss = CSS_SOURCE;
  assert.match(
    mobileCss,
    /\.evidence-source-list\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*repeat\(2,/s,
    "Mobile evidence sources must wrap into a visible two-column register"
  );
  assert.match(
    mobileCss,
    /\.lane-key__crossing\s*\{[^}]*display:\s*inline-flex/s,
    "Mobile must retain boundary-crossing context"
  );
  assert.match(
    CSS_SOURCE,
    /\.revision-strip__repo\s*\{[^}]*display:\s*block/s,
    "Narrow mobile must retain compact repository identity"
  );
  assert.match(CSS_SOURCE, /\.workspace\.is-zero-change[\s\S]*?\.workspace\.is-zero-change \.inspector[\s\S]*?display:\s*none/);
  assert.match(CSS_SOURCE, /\.scope-control button,[\s\S]*?min-height:\s*44px/);
  assert.match(CSS_SOURCE, /\.metric__note,[\s\S]*?font-size:\s*11px/);
  assert.match(CSS_SOURCE, /@media \(max-width: 360px\)[\s\S]*?\.wordmark__compact[\s\S]*?display:\s*inline/);
  assert.match(
    CSS_SOURCE,
    /body:has\(\.workspace\[data-trace-mode="mobile"\]\.has-selection\)[\s\S]*?\.trace-context[\s\S]*?position:\s*sticky/s,
    "Selected mobile traces must retain sticky context"
  );
  assert.match(
    CSS_SOURCE,
    /\.workspace\[data-trace-mode="mobile"\][\s\S]*?\.mobile-trace-list:not\(\[hidden\]\)[\s\S]*?display:\s*block/s,
    "The JS-selected trace mode must be the CSS representation authority"
  );
  assert.match(
    CSS_SOURCE,
    /\.workspace\[data-trace-mode="mobile"\] \.mobile-trace-list:not\(\[hidden\]\)\s*\{[^}]*padding:\s*4px 12px 16px/s,
    "Mobile-list appearance must follow the authoritative trace mode"
  );
  assert.match(
    CSS_SOURCE,
    /\.workspace\[data-trace-mode="mobile"\] \.mobile-candidate\s*\{[^}]*display:\s*grid;[^}]*min-height:\s*92px/s,
    "Mobile candidates must keep their representation styling above the page-shell breakpoint"
  );
  assert.match(
    CSS_SOURCE,
    /\.workspace\[data-trace-mode="mobile"\] \.mobile-lane-summary\s*\{[^}]*display:\s*grid/s,
    "Mobile summaries must follow the authoritative trace mode"
  );
  assert.match(
    CSS_SOURCE,
    /body:has\(\.workspace\[data-trace-mode="mobile"\]\.has-selection\) \.trace-context\s*\{[^}]*position:\s*sticky/s,
    "Selected-trace context must follow the authoritative trace mode"
  );
  assert.match(CSS_SOURCE, /\.search-control input::placeholder\s*\{[^}]*color:\s*var\(--ink-soft\)/s);
  assert.match(CSS_SOURCE, /\.search-control__field button\s*\{[^}]*color:\s*var\(--ink-soft\)/s);
  assert.match(CSS_SOURCE, /\.overlay-disclosure summary small\s*\{[^}]*color:\s*var\(--ink-soft\)/s);
  assert.match(CSS_SOURCE, /\.workspace\[data-trace-mode="mobile"\] \.mobile-candidate__meta\s*\{[^}]*color:\s*var\(--ink-soft\)/s);
  assert.match(CSS_SOURCE, /\.workspace\[data-trace-mode="mobile"\] \.mobile-lane-summary p\s*\{[^}]*color:\s*var\(--ink-soft\)/s);
  [
    /\.metric--risk\.is-zero \.metric__alert,[\s\S]*?\.metric--risk\.is-zero \.metric__note\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.component-filter__counts,[\s\S]*?\.component-filter__meta\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.evidence-deck__heading > p,[\s\S]*?\.method-ledger > header > p\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.evidence-source-list__empty\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.evidence-source__kind,[\s\S]*?\.evidence-source__facts\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.temporal-metrics span\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.temporal-events__empty\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.method-ledger__disclosure summary span:last-child\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /#limits-list dt\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
    /\.page-footer\s*\{[^}]*color:\s*var\(--ink-soft\)/s,
  ].forEach((selectorContract) => {
    assert.match(CSS_SOURCE, selectorContract, "Small normal text on canvas backgrounds must use the AA muted token");
  });
  assert.ok(contrastRatio("#475569", "#f5f7fb") >= 4.5, "Normal muted text token must meet WCAG AA on the canvas");
  assert.doesNotMatch(CSS_SOURCE, /content:\s*["']Local["']/, "Responsive CSS must not overwrite standalone runtime wording");

  const loadingHarness = await renderFixture(fixture(), { deferFetch: true });
  assert.match(loadingHarness.nodes.get("graph-state").textContent, /Calibrating blast trace/i);
  assert.equal(loadingHarness.nodes.get("graph-frame").getAttribute("aria-busy"), "true");
  assert.equal(loadingHarness.nodes.get("refresh-button").getAttribute("aria-busy"), "true");
  assert.match(loadingHarness.nodes.get("completeness-status").textContent, /Checking evidence completeness/i);
  loadingHarness.releaseFetch();
  await loadingHarness.settle();
  assert.equal(loadingHarness.nodes.get("graph-frame").getAttribute("aria-busy"), "false");

  const initialErrorHarness = await renderFixture(null, {
    response: {
      ok: false,
      status: 422,
      payload: { error: { code: "index_stale", message: "Reindex the repository before review." } },
    },
  });
  assert.equal(initialErrorHarness.nodes.get("notice-stack").hidden, false);
  assert.match(initialErrorHarness.nodes.get("notice-stack").textContent, /Snapshot unavailable · index_stale/i);
  assert.match(initialErrorHarness.nodes.get("notice-stack").textContent, /Use Refresh/i);
  assert.match(initialErrorHarness.nodes.get("completeness-status").textContent, /retry required/i);
  assert.match(initialErrorHarness.nodes.get("graph-state").textContent, /Retry local scan/i);

  const malformedHarness = await renderFixture(null, { response: { ok: true, status: 200, jsonError: true } });
  assert.match(malformedHarness.nodes.get("notice-stack").textContent, /invalid_json/i);

  const harness = await renderFixture();
  assert.match(harness.nodes.get("completeness-status").textContent, /No truncation reported/i);
  assert.ok(harness.nodes.get("completeness-status").classList.contains("is-complete"));
  assert.match(harness.nodes.get("status-region").textContent, /9 evidence sources were evaluated/i);
  assert.match(
    harness.nodes.get("instrument-summary").textContent,
    /Widest blast: authorize → 1 symbol across 1 component/i,
    "The headline must name the widest-blast changed symbol"
  );
  assert.match(
    harness.nodes.get("instrument-summary").textContent,
    /1 changed symbol reaches downstream code with no returned test path/i,
    "The headline must count changed symbols with no test path"
  );
  assert.match(harness.nodes.get("evidence-summary").textContent, /9 sources · 3 matched trace files/i);
  assert.equal(harness.nodes.get("evidence-source-list").querySelectorAll(".evidence-source").length, 9);

  const auditHarness = await renderFixture(fixture(), { width: 1200 });
  assert.match(auditHarness.nodes.get("audit-summary").textContent, /2 components across 1 language/i, "Audit summary must explain the codebase shape");
  assert.match(auditHarness.nodes.get("audit-summary").textContent, /3 dead-code candidates/i, "Audit summary must count dead-code candidates from the true total");
  assert.match(auditHarness.nodes.get("audit-explain").textContent, /Largest components/i);
  assert.match(auditHarness.nodes.get("audit-explain").textContent, /src[\s\S]*3 files/i, "Explain must rank components by file count");
  assert.match(auditHarness.nodes.get("audit-structural").textContent, /No dependency cycles/i, "Structural must show the acyclic clean state");
  assert.match(auditHarness.nodes.get("audit-structural").textContent, /Most depended-on symbols/i);
  assert.match(auditHarness.nodes.get("audit-structural").textContent, /authorize · src\/auth\.rs[\s\S]*5 in/i, "Structural must rank hotspots by in-degree");
  assert.match(auditHarness.nodes.get("audit-health").textContent, /Dead-code candidates/i);
  assert.match(auditHarness.nodes.get("audit-health").textContent, /unused_helper · function/i, "Health must list dead-code candidates");
  assert.match(auditHarness.nodes.get("audit-health").textContent, /Showing 3 of 3 candidates/i, "Health must be honest about truncation");

  assert.match(auditHarness.nodes.get("audit-change").textContent, /Churn × centrality/i);
  assert.match(auditHarness.nodes.get("audit-change").textContent, /src\/auth\.rs[\s\S]*9 commits × 5 in/i, "Change card must show both axes");
  assert.match(auditHarness.nodes.get("audit-change").textContent, /last 500 commits/i, "Change card must state the churn window");

  assert.match(auditHarness.nodes.get("audit-verdict-word").textContent, /Risk/i, "Fixture has an error-level finding -> Risk posture");
  assert.ok(auditHarness.nodes.get("audit-verdict").classList.contains("audit-verdict--risk"), "Verdict badge carries the posture class");
  assert.match(auditHarness.nodes.get("audit-lede").textContent, /2 components · 4 mapped files · 1 language/i, "Lede states the codebase shape from facts");
  assert.match(auditHarness.nodes.get("audit-lede").textContent, /No dependency cycles/i, "Lede states the acyclic structure");
  assert.equal(auditHarness.nodes.get("audit-pillars").querySelectorAll(".audit-pillar").length, 3, "Three posture pillars");
  assert.match(auditHarness.nodes.get("audit-pillars").textContent, /Structure[\s\S]*Healthy/i, "Structure pillar is healthy at 0 cycles");

  var tiles = auditHarness.nodes.get("audit-treemap").querySelectorAll(".tm-rect");
  assert.ok(tiles.length >= 2, "Treemap draws a tile per component");
  assert.ok(auditHarness.nodes.get("audit-treemap").querySelectorAll(".tm-rect--risk").length >= 1, "a component holding a change-hotspot is tinted risk");
  assert.ok(auditHarness.nodes.get("audit-map-mobile").querySelectorAll(".audit-row").length >= 2, "Mobile map falls back to ranked bars");

  assert.match(auditHarness.nodes.get("audit-structural-sev").textContent, /Acyclic/i, "Structural chip is Acyclic at 0 cycles");
  assert.match(auditHarness.nodes.get("audit-change-sev").textContent, /Watch/i, "Change chip is Watch with hotspots present");
  assert.match(auditHarness.nodes.get("audit-security-sev").textContent, /Findings|Review/i, "Security chip reflects loaded findings");

  assert.match(auditHarness.nodes.get("audit-bugs").textContent, /Largest files/i);
  assert.match(auditHarness.nodes.get("audit-bugs").textContent, /highstock\.min\.js[\s\S]*60,000 lines/i, "Largest-files output is not silently post-filtered after the backend cap");
  assert.match(auditHarness.nodes.get("audit-bugs-sev").textContent, /Very large/i, "A very large line-span proxy is surfaced without claiming a defect");

  assert.match(auditHarness.nodes.get("audit-bus").textContent, /knowledge-concentrated/i);
  assert.match(auditHarness.nodes.get("audit-bus").textContent, /src\/core[\s\S]*100% · 1 author/i, "Bus card shows single-owner concentration");
  assert.match(auditHarness.nodes.get("audit-bus-sev").textContent, /Concentrated/i, "Single-author history is reported as concentrated");
  assert.match(auditHarness.nodes.get("audit-map-note").textContent, /All 2 returned components/i, "Map states how much of the returned component set it represents");

  var auditUnavailablePayload = fixture();
  auditUnavailablePayload.evidence.files.items.forEach((file) => { file.findings = []; });
  auditUnavailablePayload.audit.change_hotspots = { status: "unavailable", window_commits: 500, returned: 0, items: [] };
  auditUnavailablePayload.audit.largest_files = { status: "unavailable", returned: 0, items: [] };
  auditUnavailablePayload.audit.bus_factor = { status: "unavailable", window_commits: 2000, returned: 0, items: [] };
  var auditUnavailableHarness = await renderFixture(auditUnavailablePayload, { width: 1200 });
  assert.equal(auditUnavailableHarness.nodes.get("audit-verdict-word").textContent, "Incomplete", "Missing audit inputs cannot produce a Healthy verdict");
  assert.equal(auditUnavailableHarness.nodes.get("audit-change-sev").textContent, "No data", "Unavailable churn is not labelled Clear");
  assert.equal(auditUnavailableHarness.nodes.get("audit-bugs-sev").textContent, "No data", "Unavailable size data is explicit");
  assert.equal(auditUnavailableHarness.nodes.get("audit-bus-sev").textContent, "No data", "Unavailable authorship data is explicit");

  var boundedPayload = fixture();
  boundedPayload.evidence.files.items.forEach((file) => { file.findings = []; });
  boundedPayload.audit.change_hotspots.items = boundedPayload.audit.change_hotspots.items.slice(0, 1);
  boundedPayload.audit.change_hotspots.returned = 1;
  boundedPayload.audit.change_hotspots.truncated = true;
  boundedPayload.audit.largest_files.truncated = true;
  boundedPayload.audit.bus_factor.truncated = true;
  boundedPayload.map.cycles = { total: null, returned: 0, truncated: true, truncation_reason: "cycle_limit", items: [] };
  var boundedHarness = await renderFixture(boundedPayload, { width: 1200 });
  assert.equal(boundedHarness.nodes.get("audit-verdict-word").textContent, "Incomplete", "Available but capped audit inputs cannot produce a Healthy verdict");
  assert.match(boundedHarness.nodes.get("audit-summary").textContent, /partial or unavailable/i);
  assert.match(boundedHarness.nodes.get("audit-change").textContent, /bounded subset/i);
  assert.match(boundedHarness.nodes.get("audit-bugs").textContent, /bounded ranking/i);
  assert.match(boundedHarness.nodes.get("audit-bus").textContent, /bounded subset/i);
  assert.match(boundedHarness.nodes.get("audit-structural").textContent, /cycle analysis is partial/i);
  assert.doesNotMatch(boundedHarness.nodes.get("audit-structural").textContent, /acyclic/i, "A partial zero-cycle window is not presented as acyclic");
  assert.equal(boundedHarness.nodes.get("audit-structural-sev").textContent, "Partial");

  var truncatedMapPayload = fixture();
  truncatedMapPayload.map.components = {
    total: 12,
    returned: 10,
    truncated: true,
    truncation_reason: "top_limit",
    items: Array.from({ length: 10 }, (_, index) => ({
      path: "component-" + index,
      file_count: 10 - index,
      languages: [],
      boundaries: { total: 0, returned: 0, truncated: false, items: [] },
    })),
  };
  truncatedMapPayload.map.files = { total: 55, returned: 55, truncated: false, items: [] };
  truncatedMapPayload.audit.narrative = {
    red_team: [
      { title: "Returned tail", vector: ["component-9"] },
      { title: "Unknown route", vector: ["ghost"] },
    ],
  };
  var truncatedMapHarness = await renderFixture(truncatedMapPayload, { width: 1200 });
  assert.match(truncatedMapHarness.nodes.get("audit-map-note").textContent, /Showing 10 of 12 components; omitted components are not represented/i);
  assert.match(truncatedMapHarness.nodes.get("audit-treemap").textContent, /Other returned components/i, "Visual tail grouping names only returned components");
  assert.doesNotMatch(truncatedMapHarness.nodes.get("audit-redteam-body").textContent, /Unknown route/i, "Unknown components cannot fall through to the visual tail tile");
  var returnedTail = truncatedMapHarness.nodes.get("audit-redteam-body").querySelectorAll(".audit-rt")[0];
  returnedTail.dispatch("click");
  assert.equal(truncatedMapHarness.nodes.get("audit-treemap").querySelectorAll(".tm-rect--traced").length, 1, "A known collapsed component traces to its returned-components tile");

  var nf = fixture();
  nf.audit.narrative = {
    summary: "AI executive summary of the codebase.",
    lenses: { bugs: "Review the largest indexed files.", security: "Guard the assistant's DB tools." },
    domains: [
      { name: "Auth & tenancy", severity: "risk", note: "Compliance-critical isolation.", components: ["src", "tests"] },
      { name: "Reporting", severity: "attention", note: "Large surface.", components: ["ghost"] }
    ],
    red_team: [
      { title: "AI tool reaches the DB", severity: "attention", scenario: "A user coerces the assistant into fetching another system.", evidence: "MultiAgentBot + GetSystemIDTool", vector: ["src", "tests"] },
      { title: "Partially unknown", severity: "attention", vector: ["src", "ghost"] },
      { title: "Ungrounded guess", severity: "risk", scenario: "unknown component", vector: ["ghost"] }
    ]
  };
  var narrHarness = await renderFixture(nf, { width: 1200 });
  assert.match(narrHarness.nodes.get("audit-lede").textContent, /AI executive summary of the codebase/i, "Narrative summary supersedes the factual lede");
  assert.ok(narrHarness.nodes.get("audit-lede").classList.contains("audit-lede--ai"), "Narrative lede is marked as AI interpretation");
  assert.match(narrHarness.nodes.get("audit-bugs").textContent, /Review the largest indexed files/i, "Per-lens AI reading renders on the bug card");
  assert.match(narrHarness.nodes.get("audit-bugs").textContent, /AI/i, "Per-lens reading carries an AI label");
  assert.equal(narrHarness.nodes.get("audit-domain-card").hidden, false, "Domain card shows when the narrative maps domains");
  assert.match(narrHarness.nodes.get("audit-domain").textContent, /Auth & tenancy[\s\S]*Compliance-critical/i, "Domain card renders domain + note");
  assert.match(narrHarness.nodes.get("audit-domain").textContent, /src · tests/i, "Domain card lists exact returned components");
  assert.equal(narrHarness.nodes.get("audit-redteam").hidden, false, "Red-team panel shows when hypotheses are present");
  assert.match(narrHarness.nodes.get("audit-redteam-body").textContent, /AI tool reaches the DB[\s\S]*coerces the assistant/i, "Red-team item renders title + scenario");
  assert.match(narrHarness.nodes.get("audit-redteam-body").textContent, /Claimed route[\s\S]*src → tests/i, "Red-team hypothesis labels its component route as a claim");
  assert.match(narrHarness.nodes.get("audit-redteam-body").textContent, /AI evidence note[\s\S]*MultiAgentBot \+ GetSystemIDTool/i, "AI prose is not presented as proof");
  assert.doesNotMatch(narrHarness.nodes.get("audit-redteam-body").textContent, /Ungrounded guess/i, "A hypothesis with an unknown component is never shown");
  assert.doesNotMatch(narrHarness.nodes.get("audit-redteam-body").textContent, /Partially unknown/i, "A mixed valid and unknown route is rejected as a whole");
  assert.match(narrHarness.nodes.get("audit-domain").textContent, /Bound components[\s\S]*src · tests/i, "Domain risk distinguishes component binding from a proven path");
  assert.doesNotMatch(narrHarness.nodes.get("audit-domain").textContent, /Reporting/i, "A domain with no returned component is dropped");

  var rtItem = narrHarness.nodes.get("audit-redteam-body").querySelectorAll(".audit-rt")[0];
  assert.equal(rtItem.getAttribute("role"), "button", "A hypothesis with a vector is traceable");
  assert.equal(rtItem.getAttribute("aria-pressed"), "false");
  rtItem.dispatch("click");
  assert.equal(rtItem.getAttribute("aria-pressed"), "true", "Activating a hypothesis marks it pressed");
  assert.ok(narrHarness.nodes.get("audit-treemap").querySelectorAll(".tm-rect--traced").length >= 1, "The vector highlights its component tiles on the map");
  rtItem.dispatch("click");
  assert.equal(rtItem.getAttribute("aria-pressed"), "false", "Clicking again clears the trace");
  assert.equal(narrHarness.nodes.get("audit-treemap").querySelectorAll(".tm-rect--traced").length, 0, "Clearing removes the map highlight");

  assert.equal(auditHarness.nodes.get("audit-domain-card").hidden, true, "Domain card is hidden without a narrative");
  assert.equal(auditHarness.nodes.get("audit-redteam").hidden, true, "Red-team panel is hidden without a narrative");
  assert.doesNotMatch(auditHarness.nodes.get("audit-lede").textContent, /AI executive/i, "Facts-only lede when no narrative");

  assert.match(HTML_SOURCE, /id="audit-redteam"/i, "Red-team section must exist");
  assert.match(HTML_SOURCE, /to verify/i, "Red-team panel is labelled as hypotheses to verify");
  assert.match(CSS_SOURCE, /\.tm-rect--traced/, "The map has a traced-tile style for vector highlighting");

  assert.match(HTML_SOURCE, /id="audit-treemap"/i, "Treemap element must exist");
  assert.match(HTML_SOURCE, /id="audit-verdict"/i, "Verdict badge must exist");
  assert.match(CSS_SOURCE, /\.tm-rect--risk\s*\{[^}]*fill:\s*var\(--coral\)/s, "Treemap risk fill is a theme token, not a hardcoded color");
  assert.match(CSS_SOURCE, /\.audit-lede--ai::before[\s\S]*?AI interpretation/s, "AI-sourced lede carries a visible AI label");
  assert.match(auditHarness.nodes.get("audit-security").textContent, /Static findings/i);
  assert.match(auditHarness.nodes.get("audit-security").textContent, /auth\.bypass/i, "Security must surface loaded static findings");
  assert.match(auditHarness.nodes.get("audit-security").textContent, /Attack surface/i);
  assert.match(auditHarness.nodes.get("audit-security").textContent, /src\/main\.rs/i, "Security must list entry points as attack surface");

  assert.equal(auditHarness.nodes.get("audit-board").hidden, true, "Audit board starts hidden in review mode");
  assert.equal(auditHarness.nodes.get("mode-review").getAttribute("aria-pressed"), "true");
  auditHarness.nodes.get("mode-audit").dispatch("click");
  assert.equal(auditHarness.nodes.get("audit-board").hidden, false, "Switching to audit reveals the board");
  assert.equal(auditHarness.nodes.get("mode-audit").getAttribute("aria-pressed"), "true", "Audit button becomes pressed");
  assert.equal(auditHarness.nodes.get("mode-review").getAttribute("aria-pressed"), "false", "Review button releases");
  assert.equal(auditHarness.document.body.getAttribute("data-mode"), "audit", "Body mode attribute drives the CSS show/hide");

  assert.equal(auditHarness.nodes.get("audit-production").textContent, "All indexed paths", "Audit reports the backend-selected path policy");
  assert.match(auditHarness.nodes.get("audit-change").textContent, /tests\/auth\.rs/i, "Unfiltered change card includes test paths");
  assert.match(auditHarness.nodes.get("audit-health").textContent, /unused_fixture/i, "Unfiltered health card includes fixture paths");

  var productionPayload = fixture();
  productionPayload.options.production_only = true;
  productionPayload.map.files = { total: 3, returned: 3, truncated: false, items: [] };
  productionPayload.map.components = { total: 1, returned: 1, truncated: false, items: [productionPayload.map.components.items[0]] };
  productionPayload.audit.dead_code = { total: 2, returned: 2, truncated: false, items: productionPayload.audit.dead_code.items.slice(0, 2) };
  productionPayload.audit.change_hotspots.items = productionPayload.audit.change_hotspots.items.slice(0, 1);
  productionPayload.audit.change_hotspots.returned = 1;
  productionPayload.audit.largest_files.items = productionPayload.audit.largest_files.items.filter((item) => item.file.startsWith("src/"));
  productionPayload.audit.largest_files.returned = productionPayload.audit.largest_files.items.length;
  productionPayload.audit.bus_factor.items = productionPayload.audit.bus_factor.items.filter((item) => item.component.startsWith("src"));
  productionPayload.audit.bus_factor.returned = productionPayload.audit.bus_factor.items.length;
  var productionHarness = await renderFixture(productionPayload, { width: 1200 });
  assert.equal(productionHarness.nodes.get("audit-production").textContent, "Production paths only");
  assert.doesNotMatch(productionHarness.nodes.get("audit-change").textContent, /tests\/auth\.rs/i, "Backend-filtered change data stays production-only");
  assert.doesNotMatch(productionHarness.nodes.get("audit-health").textContent, /unused_fixture/i, "Backend-filtered dead-code data stays production-only");
  assert.doesNotMatch(productionHarness.nodes.get("audit-bus").textContent, /tests\/e2e/i, "Backend-filtered authorship stays production-only");

  assert.match(HTML_SOURCE, /<span[^>]*id="audit-production"/i, "Selected audit scope must be visible and noninteractive");
  assert.doesNotMatch(APP_SOURCE, /NON_PRODUCTION_SEGMENTS|auditFilter\s*\(/, "The browser must not duplicate backend path classification");

  assert.match(HTML_SOURCE, /<button[^>]*id="mode-audit"[^>]*data-mode="audit"/i, "Audit mode button must exist");
  assert.match(HTML_SOURCE, /id="audit-board"[^>]*hidden/i, "Audit board must ship hidden");
  assert.match(CSS_SOURCE, /body\[data-mode="audit"\][\s\S]*?\.review-workbench[\s\S]*?display:\s*none/s, "Audit mode must hide the diff workbench");
  assert.match(harness.nodes.get("evidence-source-list").textContent, /repository verified/i);
  assert.match(harness.nodes.get("temporal-summary").textContent, /Architecture drift detected/i);
  assert.equal(harness.nodes.get("temporal-components").textContent, "+1 −1 ~0");
  assert.equal(harness.nodes.get("temporal-cycles").textContent, "+1 −1 ~0");
  assert.match(harness.nodes.get("temporal-events").textContent, /Cycle introduced/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /Component removed/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /Cycle resolved/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /Hotspot entered/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /Hotspot exited/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /charge<\/span>/i);
  assert.match(harness.nodes.get("temporal-events").textContent, /History needs review/i);

  for (const width of [390, 700, 768, 820, 900, 1440]) {
    const modeHarness = await renderFixture(fixture(), { width: width });
    const mobile = width < 700;
    assert.equal(
      modeHarness.nodes.get("review-workspace").getAttribute("data-trace-mode"),
      mobile ? "mobile" : "desktop",
      "Trace mode must be explicit at " + width + "px"
    );
    assert.equal(
      Number(modeHarness.nodes.get("trace-graph").getAttribute("hidden") === null)
        + Number(modeHarness.nodes.get("mobile-trace-list").hidden === false),
      1,
      "Exactly one trace representation must be active at " + width + "px"
    );
  }

  const ageHarness = await renderFixture(fixture(), { now: Date.UTC(2026, 7, 13, 12, 0, 0) });
  assert.match(ageHarness.nodes.get("snapshot-age").textContent, /Snapshot just now/i);
  ageHarness.advanceTime(2 * 60 * 60 * 1000);
  assert.equal(ageHarness.nodes.get("snapshot-age").textContent, "Snapshot 2h ago");
  assert.doesNotMatch(ageHarness.nodes.get("completeness-status").textContent, /current/i);

  const focusHarness = await renderFixture(fixture(), { width: 390 });
  const focusCandidate = focusHarness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  focusCandidate.dispatch("click");
  const backToCandidates = focusHarness.nodes.get("trace-context").querySelectorAll("button")[0];
  backToCandidates.dispatch("click");
  const restoredCandidate = focusHarness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.equal(restoredCandidate.focused, true, "Back to candidates must restore keyboard focus");

  const paginatedHarness = await renderFixture(paginatedMobileFixture(), { width: 390 });
  const pageActions = paginatedHarness.nodes.get("trace-context").querySelectorAll("button");
  pageActions[1].dispatch("click");
  assert.match(paginatedHarness.nodes.get("trace-context").textContent, /2\/2/);
  const pageTwoOrigin = paginatedHarness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.match(pageTwoOrigin.textContent, /changed_6/i);
  pageTwoOrigin.dispatch("click");
  paginatedHarness.nodes.get("trace-context").querySelectorAll("button")[0].dispatch("click");
  assert.match(paginatedHarness.nodes.get("trace-context").textContent, /2\/2/, "Back must preserve the originating mobile page");
  const restoredPageTwoOrigin = paginatedHarness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.match(restoredPageTwoOrigin.textContent, /changed_6/i);
  assert.equal(restoredPageTwoOrigin.focused, true, "Back must focus the page-two origin");

  const standaloneHarness = await renderFixture(fixture(), { embedded: true, rejectFetch: true });
  assert.equal(standaloneHarness.fetchCalls, 0, "Standalone Lens must render embedded JSON without fetching");
  assert.match(standaloneHarness.nodes.get("repository-name").textContent, /example/i);

  const staleHarness = await renderFixture(fixture(), {
    responses: [
      { ok: true, status: 200, payload: fixture() },
      { ok: false, status: 422, payload: { error: { code: "revision_changed", message: "The revision moved during refresh." } } },
    ],
  });
  staleHarness.nodes.get("refresh-button").dispatch("click");
  await staleHarness.settle();
  assert.match(staleHarness.nodes.get("repository-name").textContent, /example/i, "Failed refresh must retain the prior snapshot");
  assert.match(staleHarness.nodes.get("notice-stack").textContent, /Stale · revision_changed/i);
  assert.match(staleHarness.nodes.get("completeness-status").textContent, /Stale snapshot/i);
  assert.ok(staleHarness.nodes.get("completeness-status").classList.contains("is-error"));

  const schemaMismatch = cloneFixture();
  schemaMismatch.schema_version = 7;
  const schemaHarness = await renderFixture(schemaMismatch);
  assert.match(schemaHarness.nodes.get("notice-stack").textContent, /Schema mismatch/i);
  assert.match(schemaHarness.nodes.get("notice-stack").textContent, /received schema 7/i);
  assert.ok(schemaHarness.nodes.get("completeness-status").classList.contains("is-error"));
  assert.doesNotMatch(schemaHarness.nodes.get("completeness-status").textContent, /complete|current/i);

  const missingSchema = emptyFixture();
  delete missingSchema.schema_version;
  const missingSchemaHarness = await renderFixture(missingSchema);
  assert.match(missingSchemaHarness.nodes.get("notice-stack").textContent, /Schema unavailable/i);
  assert.ok(missingSchemaHarness.nodes.get("completeness-status").classList.contains("is-error"));
  assert.doesNotMatch(missingSchemaHarness.nodes.get("trace-context").textContent, /Review complete/i);

  const truncated = cloneFixture();
  truncated.impact.impact.total = 18;
  truncated.impact.impact.returned = 1;
  truncated.impact.impact.truncated = true;
  truncated.impact.impact.truncation_reason = "work_limit";
  const truncatedHarness = await renderFixture(truncated);
  assert.match(truncatedHarness.nodes.get("notice-stack").textContent, /1 bounded section/i);
  assert.match(truncatedHarness.nodes.get("notice-stack").textContent, /Review impacted symbols before approval/i);
  assert.doesNotMatch(truncatedHarness.nodes.get("notice-stack").textContent, /work_limit/i);
  assert.match(truncatedHarness.nodes.get("notice-stack").textContent, /Open precision & limits/i);
  assert.match(truncatedHarness.nodes.get("completeness-status").textContent, /Partial evidence/i);
  assert.ok(truncatedHarness.nodes.get("completeness-status").classList.contains("is-partial"));
  const limitsAction = truncatedHarness.nodes.get("notice-stack").querySelectorAll(".notice__action")[0];
  assert.ok(limitsAction, "Partial notice must provide a direct limits action");
  limitsAction.dispatch("click");
  assert.equal(truncatedHarness.nodes.get("method-ledger-disclosure").open, true);

  const empty = emptyFixture();
  const emptyHarness = await renderFixture(empty, { width: 900 });
  assert.match(emptyHarness.nodes.get("instrument-summary").textContent, /No changes were captured/i);
  assert.match(emptyHarness.nodes.get("graph-state").textContent, /No changes in captured scope/i);
  assert.match(emptyHarness.nodes.get("trace-context").textContent, /Baseline main · scope \./i);
  assert.doesNotMatch(emptyHarness.nodes.get("trace-context").textContent, /Review complete|current/i);
  assert.ok(emptyHarness.nodes.get("review-workspace").classList.contains("is-zero-change"));
  assert.doesNotMatch(emptyHarness.nodes.get("graph-state").textContent, /Select a trace claim/i);

  const keyboardHarness = await renderFixture(fixture(), { width: 900 });
  const graphChildren = keyboardHarness.nodes.get("trace-graph").children;
  const clusterLayerIndex = graphChildren.findIndex((node) => node.classList && node.classList.contains("graph-clusters"));
  const edgeControlIndex = graphChildren.findIndex((node) => node.classList && node.classList.contains("graph-edge-controls"));
  assert.ok(clusterLayerIndex >= 0 && edgeControlIndex > clusterLayerIndex, "Keyboard order must reach graph claims before edge controls");
  const cluster = keyboardHarness.nodes.get("trace-graph").querySelectorAll("[data-cluster-id]")[0];
  assert.ok(cluster, "Desktop overview must expose keyboard-expandable clusters");
  assert.ok(
    keyboardHarness.nodes.get("trace-graph").querySelectorAll(".graph-cluster--risk-serious").length >= 1,
    "Cluster overview must surface the risk tier before expanding"
  );
  assert.match(cluster.textContent, /1 serious/, "Cluster meta must count serious claims in text");
  cluster.dispatch("keydown", { key: "Enter" });
  const node = keyboardHarness.nodes.get("trace-graph").querySelectorAll("[data-node-id]")[0];
  assert.ok(node, "Expanded cluster must expose keyboard-selectable claims");
  assert.ok(
    node.classList.contains("graph-node--risk-serious"),
    "A changed claim with findings, a failing test, and no test path must carry the serious tier"
  );
  assert.match(node.textContent, /UNTESTED/, "A changed claim with no returned test path must be flagged in text, not color alone");
  assert.match(node.textContent, /1 fail · 2 findings · untested/i, "Evidence marks must be human-readable, risk first");
  assert.match(node.textContent, /→ 1 sym · 1 comp/, "Changed claims must state their blast reach");
  node.dispatch("keydown", { key: " " });
  assert.doesNotMatch(keyboardHarness.nodes.get("inspector-body").textContent, /Select a trace claim/i);

  const unavailable = fixture();
  unavailable.temporal = {
    status: "unavailable",
    diagnostic: { code: "snapshot_unavailable", message: "Temporal snapshot stayed bounded." },
  };
  const unavailableHarness = await renderFixture(unavailable);
  assert.equal(unavailableHarness.nodes.get("temporal-components").textContent, "—");
  assert.match(unavailableHarness.nodes.get("temporal-summary").textContent, /stayed bounded/i);
  assert.match(unavailableHarness.nodes.get("notice-stack").textContent, /Temporal · snapshot_unavailable/i);

  const partial = fixture();
  partial.temporal.data.partial = true;
  partial.temporal.data.diagnostics = [{ code: "bounded_map_projection", message: "Only the returned temporal window is comparable." }];
  const partialHarness = await renderFixture(partial);
  assert.match(partialHarness.nodes.get("temporal-summary").textContent, /bounded projection is partial/i);
  assert.match(partialHarness.nodes.get("precision-list").textContent, /returned temporal window/i);
  const changedCandidate = harness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.ok(changedCandidate, "Changed claim with overlays must remain selectable on mobile");
  assert.equal(
    harness.nodes.get("trace-graph").getAttribute("hidden"),
    "",
    "Mobile index must remove the dormant SVG from layout"
  );
  changedCandidate.dispatch("click");
  assert.ok(
    harness.nodes.get("review-workspace").classList.contains("has-selection"),
    "Selecting a mobile claim must switch to the focused trace journey"
  );
  assert.match(
    harness.nodes.get("trace-context").textContent,
    /Back to candidates/i,
    "Focused mobile traces must keep a persistent return to candidates"
  );
  const claimHeading = harness.nodes.get("inspector-body").querySelectorAll(".claim-heading")[0];
  assert.equal(claimHeading.children.find((child) => child.tagName === "h4")?.tagName, "h4");
  const claimSection = harness.nodes.get("inspector-body").querySelectorAll(".claim-section")[0];
  assert.equal(claimSection.children[0].tagName, "h5");
  assert.equal(
    harness.nodes.get("trace-graph").getAttribute("hidden"),
    null,
    "Selecting a mobile claim must reveal its compact SVG"
  );
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll(".graph-edge--ownership").length,
    1,
    "A named owner to explicit no-owner transition must remain visible as an ownership boundary"
  );
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll(".graph-edge--semantic").length,
    1,
    "An exact SCIP symbol, file, and definition-line pair must upgrade static provenance"
  );
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll(".graph-edge--facts").length,
    1,
    "A normalized relationship may decorate only the existing exact-endpoint edge"
  );
  const changedInspector = harness.nodes.get("inspector-body").textContent;
  assert.match(changedInspector, /Findings · file-level/i);
  assert.match(changedInspector, /Semgrep \/ auth\.bypass:42:3/i);
  assert.match(changedInspector, /com\.example\.arch-lint \/ architecture\.boundary:42:3/i);
  assert.match(changedInspector, /50% reported lines covered \(1\/2\)/i);
  assert.match(changedInspector, /CODEOWNERS · @security/i);
  assert.match(changedInspector, /Git contributor · Alice · 2 commits/i);
  assert.match(changedInspector, /JUnit · file-level/i);
  assert.match(changedInspector, /authorization rejects · failed · AuthTest · expected denial/i);
  assert.match(changedInspector, /Runtime spans · file-level/i);
  assert.match(changedInspector, /1 spans · 1 traces/i);
  assert.match(changedInspector, /Project knowledge · exact path/i);
  assert.match(changedInspector, /architecture_decision · Auth boundary/i);
  assert.match(
    harness.nodes.get("evidence-source-list").textContent,
    /manifest sha256 a{12}… · 1,?024 bytes/i
  );
  const nodeCountBeforeToggle = harness.nodes.get("trace-graph").querySelectorAll("[data-node-id]").length;
  const edgeCountBeforeToggle = harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length;
  const findings = harness.overlayButtons.find((button) => button.dataset.overlay === "findings");
  findings.dispatch("click");
  assert.equal(findings.getAttribute("aria-pressed"), "false");
  assert.doesNotMatch(harness.nodes.get("inspector-body").textContent, /Findings · file-level/i);
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll("[data-node-id]").length,
    nodeCountBeforeToggle,
    "Overlay visibility must not alter graph topology"
  );
  findings.dispatch("click");
  const runtime = harness.overlayButtons.find((button) => button.dataset.overlay === "runtime");
  runtime.dispatch("click");
  assert.equal(runtime.getAttribute("aria-pressed"), "false");
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length,
    edgeCountBeforeToggle,
    "Runtime overlay visibility must never create or remove graph topology"
  );
  runtime.dispatch("click");
  const runtimeEdge = harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]")[0];
  runtimeEdge.dispatch("click");
  assert.match(harness.nodes.get("inspector-body").textContent, /Static provenanceSCIP \(preferred\)/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /Compiler-resolved semantic evidence/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /reference at src\/target\.rs:9/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /Runtime trace corroboration/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /src\/auth\.rs → src\/target\.rs/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /Normalized relationship facts/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /Compiler-resolved auth to target call/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /facts:sha256:[0-9a-f]{64}/i);
  assert.match(harness.nodes.get("inspector-body").textContent, /arch-lint-output · reports\/arch-lint\.json · sha256 b{64} · 4,?312 bytes/i);
  assert.doesNotMatch(harness.nodes.get("inspector-body").textContent, /Wrong-line relationship/i);
  const facts = harness.overlayButtons.find((button) => button.dataset.overlay === "facts");
  const factEdgeCount = harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length;
  facts.dispatch("click");
  assert.equal(facts.getAttribute("aria-pressed"), "false");
  assert.equal(harness.nodes.get("trace-graph").querySelectorAll(".graph-edge--facts").length, 0);
  assert.equal(harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length, factEdgeCount);
  facts.dispatch("click");
  const semantic = harness.overlayButtons.find((button) => button.dataset.overlay === "semantic");
  const semanticEdgeCount = harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length;
  semantic.dispatch("click");
  assert.equal(semantic.getAttribute("aria-pressed"), "false");
  assert.equal(harness.nodes.get("trace-graph").querySelectorAll(".graph-edge--semantic").length, 0);
  assert.equal(harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length, semanticEdgeCount);
  assert.match(harness.nodes.get("inspector-body").textContent, /Static provenanceTree-sitter \(fallback\)/i);
  semantic.dispatch("click");
  harness.nodes.get("fit-button").dispatch("click");
  const testsScope = harness.scopeButtons.find((button) => button.dataset.scope === "test");
  testsScope.dispatch("click");
  const candidate = harness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.ok(candidate, "Heuristic test candidate must be selectable on mobile");
  candidate.dispatch("click");

  const inspectorText = harness.nodes.get("inspector-body").textContent;
  assert.match(inspectorText, /heuristic evidence/i);
  assert.match(inspectorText, /no graph seed returned/i);
  assert.match(inspectorText, /component tests/i);
  assert.match(inspectorText, /CODEOWNERS · explicitly unowned/i);
  assert.match(inspectorText, /Git contributor · Bob · 1 commits/i);
  assert.match(inspectorText, /1 commits · \+4 \/ −0 lines/i);
  assert.doesNotMatch(inspectorText, /unnamed seed|file unavailable/i, "Null seed must not invent an endpoint");
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll("[data-edge-id]").length,
    0,
    "Null seed must not create a graph edge"
  );

  const ownership = harness.overlayButtons.find((button) => button.dataset.overlay === "ownership");
  ownership.dispatch("click");
  assert.equal(ownership.getAttribute("aria-pressed"), "false");
  assert.doesNotMatch(harness.nodes.get("inspector-body").textContent, /CODEOWNERS/i);

  process.stdout.write("Lens focused DOM/static regressions passed\n");
}

main().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
