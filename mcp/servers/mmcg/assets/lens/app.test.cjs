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

  focus() {}

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
      return attribute ? element.attributes.has(attribute[1]) : false;
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
  return {
    nodes: nodes,
    scopeButtons: scopeButtons,
    overlayButtons: overlayButtons,
    document: {
      body: new MockElement("body"),
      getElementById(id) {
        return nodes.get(id);
      },
      querySelectorAll(selector) {
        if (selector === "[data-scope]") {
          return scopeButtons;
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
            findings: [],
            ownership: { codeowners_source_id: "codeowners", codeowners: [], contributors: [] },
            runtime: { source_ids: ["otel:0"], spans: 1, traces: 1 },
            knowledge: [],
          },
          {
            path: "tests/auth.rs",
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
      files: { total: 2, returned: 2, truncated: false, items: [] },
      languages: { total: 1, returned: 1, truncated: false, items: [] },
      components: {
        total: 2,
        returned: 2,
        truncated: false,
        items: [
          { path: "src", file_count: 1, languages: [], boundaries: { total: 0, returned: 0, truncated: false, items: [] } },
          { path: "tests", file_count: 1, languages: [], boundaries: { total: 0, returned: 0, truncated: false, items: [] } },
        ],
      },
      entry_points: { total: 0, returned: 0, truncated: false, items: [] },
      hotspots: { total: 0, returned: 0, truncated: false, items: [] },
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
  const fetchGate = settings.deferFetch
    ? new Promise((resolve) => { releaseFetch = resolve; })
    : null;
  const window = {
    setTimeout(handler) {
      handler();
      return 1;
    },
    setInterval() {
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
    Date: Date,
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
  return harness;
}

function cloneFixture() {
  return JSON.parse(JSON.stringify(fixture()));
}

function cloneFixtureValue(value) {
  return JSON.parse(JSON.stringify(value));
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
    /body:has\(\.workspace\.has-selection\)[\s\S]*?\.trace-context[\s\S]*?position:\s*sticky/s,
    "Selected mobile traces must retain sticky context"
  );

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
  assert.match(harness.nodes.get("evidence-summary").textContent, /9 sources · 3 matched trace files/i);
  assert.equal(harness.nodes.get("evidence-source-list").querySelectorAll(".evidence-source").length, 9);
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

  const empty = cloneFixture();
  const emptyCollection = { total: 0, returned: 0, truncated: false, items: [] };
  empty.impact.changes.files = cloneFixtureValue(emptyCollection);
  empty.impact.changes.symbols = cloneFixtureValue(emptyCollection);
  empty.impact.impact = cloneFixtureValue(emptyCollection);
  empty.impact.api_crossings = cloneFixtureValue(emptyCollection);
  empty.impact.tests = cloneFixtureValue(emptyCollection);
  empty.impact.affected_components = cloneFixtureValue(emptyCollection);
  const emptyHarness = await renderFixture(empty, { width: 900 });
  assert.match(emptyHarness.nodes.get("instrument-summary").textContent, /No changes detected/i);
  assert.match(emptyHarness.nodes.get("graph-state").textContent, /No changes in scope/i);
  assert.match(emptyHarness.nodes.get("trace-context").textContent, /Baseline main · scope \./i);
  assert.ok(emptyHarness.nodes.get("review-workspace").classList.contains("is-zero-change"));
  assert.doesNotMatch(emptyHarness.nodes.get("graph-state").textContent, /Select a trace claim/i);

  const keyboardHarness = await renderFixture(fixture(), { width: 900 });
  const graphChildren = keyboardHarness.nodes.get("trace-graph").children;
  const clusterLayerIndex = graphChildren.findIndex((node) => node.classList && node.classList.contains("graph-clusters"));
  const edgeControlIndex = graphChildren.findIndex((node) => node.classList && node.classList.contains("graph-edge-controls"));
  assert.ok(clusterLayerIndex >= 0 && edgeControlIndex > clusterLayerIndex, "Keyboard order must reach graph claims before edge controls");
  const cluster = keyboardHarness.nodes.get("trace-graph").querySelectorAll("[data-cluster-id]")[0];
  assert.ok(cluster, "Desktop overview must expose keyboard-expandable clusters");
  cluster.dispatch("keydown", { key: "Enter" });
  const node = keyboardHarness.nodes.get("trace-graph").querySelectorAll("[data-node-id]")[0];
  assert.ok(node, "Expanded cluster must expose keyboard-selectable claims");
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
