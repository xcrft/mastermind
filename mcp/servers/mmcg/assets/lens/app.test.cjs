"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const APP_SOURCE = fs.readFileSync(path.join(__dirname, "app.js"), "utf8");
const HTML_SOURCE = fs.readFileSync(path.join(__dirname, "index.html"), "utf8");
const CSS_SOURCE = fs.readFileSync(path.join(__dirname, "styles.css"), "utf8");

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
  "refresh-button", "snapshot-age", "notice-stack", "instrument-summary", "trace-search",
  "clear-search", "fit-button", "component-list", "graph-frame", "trace-graph", "graph-state",
  "trace-context", "mobile-trace-list", "trace-count", "inspector-body", "precision-list",
  "precision-count", "limits-list", "limits-count", "schema-label", "status-region",
  "evidence-summary", "evidence-source-list",
  "metric-files", "metric-files-note", "metric-symbols", "metric-symbols-note", "metric-impact",
  "metric-impact-note", "metric-crossings", "metric-crossings-note", "metric-tests",
  "metric-tests-note",
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
  const overlayButtons = ["findings", "coverage", "ownership", "churn"].map((overlay) => {
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
    evidence: {
      schema_version: 1,
      partial: false,
      sources: {
        total: 4,
        returned: 4,
        truncated: false,
        items: [
          { id: "sarif:0", kind: "sarif", label: "semgrep.sarif", status: "loaded", facts_total: 1, facts_returned: 1, files_matched: 1 },
          { id: "coverage:0", kind: "coverage", label: "lcov.info", status: "loaded", facts_total: 2, facts_returned: 2, files_matched: 1 },
          { id: "codeowners", kind: "codeowners", label: ".github/CODEOWNERS", status: "loaded", facts_total: 3, facts_returned: 3, files_matched: 3 },
          { id: "git-history", kind: "git_history", label: "Git · last 200 commits", status: "loaded", facts_total: 3, facts_returned: 3, files_matched: 2 },
        ],
      },
      files: {
        total: 3,
        returned: 3,
        truncated: false,
        items: [
          {
            path: "src/auth.rs",
            findings: [{ source_id: "sarif:0", tool: "Semgrep", rule_id: "auth.bypass", level: "error", message: "Authorization result is ignored", line: 42, column: 3 }],
            coverage: { source_ids: ["coverage:0"], lines_found: 2, lines_hit: 1 },
            ownership: { codeowners_source_id: "codeowners", codeowners: ["@security"], contributors: [{ name: "Alice", commits: 2 }] },
            churn: { commits: 2, lines_added: 7, lines_deleted: 3 },
          },
          {
            path: "src/target.rs",
            findings: [],
            ownership: { codeowners_source_id: "codeowners", codeowners: [], contributors: [] },
          },
          {
            path: "tests/auth.rs",
            findings: [],
            ownership: { codeowners_source_id: "codeowners", codeowners: [], contributors: [{ name: "Bob", commits: 1 }] },
            churn: { commits: 1, lines_added: 4, lines_deleted: 0 },
          },
        ],
      },
      diagnostics: { total: 0, returned: 0, truncated: false, items: [] },
      limits: { artifact_bytes: 33554432, artifact_sources: 64, relevant_files: 1000, findings: 5000, findings_per_file: 100, coverage_lines: 500000, codeowner_rules: 50000, codeowners_bytes: 3145728, owners_per_rule: 50, contributors_per_file: 5, diagnostics: 100, git_commits: 200 },
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

async function renderFixture() {
  const harness = createDocument();
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
    fetch: async () => ({ ok: true, status: 200, json: async () => fixture() }),
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
  await new Promise((resolve) => setImmediate(resolve));
  await new Promise((resolve) => setImmediate(resolve));
  return harness;
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
  const mobileCss = CSS_SOURCE.slice(CSS_SOURCE.indexOf("@media (max-width: 720px)"));
  assert.match(
    mobileCss,
    /\.evidence-source-list\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*repeat\(2,/s,
    "Mobile evidence sources must wrap into a visible two-column register"
  );

  const harness = await renderFixture();
  assert.match(harness.nodes.get("evidence-summary").textContent, /4 sources · 3 matched trace files/i);
  assert.equal(harness.nodes.get("evidence-source-list").querySelectorAll(".evidence-source").length, 4);
  const changedCandidate = harness.nodes.get("mobile-trace-list").querySelectorAll(".mobile-candidate")[0];
  assert.ok(changedCandidate, "Changed claim with overlays must remain selectable on mobile");
  assert.equal(
    harness.nodes.get("trace-graph").getAttribute("hidden"),
    "",
    "Mobile index must remove the dormant SVG from layout"
  );
  changedCandidate.dispatch("click");
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
  const changedInspector = harness.nodes.get("inspector-body").textContent;
  assert.match(changedInspector, /SARIF findings · file-level/i);
  assert.match(changedInspector, /Semgrep \/ auth\.bypass:42:3/i);
  assert.match(changedInspector, /50% reported lines covered \(1\/2\)/i);
  assert.match(changedInspector, /CODEOWNERS · @security/i);
  assert.match(changedInspector, /Git contributor · Alice · 2 commits/i);
  const nodeCountBeforeToggle = harness.nodes.get("trace-graph").querySelectorAll("[data-node-id]").length;
  const findings = harness.overlayButtons.find((button) => button.dataset.overlay === "findings");
  findings.dispatch("click");
  assert.equal(findings.getAttribute("aria-pressed"), "false");
  assert.doesNotMatch(harness.nodes.get("inspector-body").textContent, /SARIF findings/i);
  assert.equal(
    harness.nodes.get("trace-graph").querySelectorAll("[data-node-id]").length,
    nodeCountBeforeToggle,
    "Overlay visibility must not alter graph topology"
  );
  findings.dispatch("click");
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
