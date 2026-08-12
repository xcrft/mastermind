(function () {
  "use strict";

  const SVG_NS = "http:" + "//www.w3.org/2000/svg";
  const EXPECTED_SCHEMA = 1;
  const NODE_HEIGHT = 64;
  const MOBILE_NODE_HEIGHT = 68;
  const CLUSTERS_PER_LANE = 5;
  const CLAIMS_PER_LANE = 5;
  const CONNECTED_PER_LANE = 5;
  const MOBILE_CANDIDATES_PER_PAGE = 5;
  const MOBILE_CONNECTED_PER_LANE = 2;
  const OVERLAY_KEYS = ["findings", "coverage", "ownership", "churn", "tests", "semantic", "runtime", "knowledge"];

  const state = {
    raw: null,
    model: null,
    loading: true,
    refreshing: false,
    stale: false,
    error: null,
    fetchedAt: null,
    scope: "all",
    search: "",
    component: null,
    selectedId: null,
    focusedSeedId: null,
    layoutMode: null,
    disclosure: "overview",
    activeNodeIds: null,
    lanePages: { changed: 0, impacted: 0, test: 0 },
    mobilePage: 0,
    overlays: new Set(OVERLAY_KEYS),
  };

  const elements = {};

  class LensRequestError extends Error {
    constructor(code, message) {
      super(message);
      this.name = "LensRequestError";
      this.code = code;
    }
  }

  function byId(id) {
    return document.getElementById(id);
  }

  function isRecord(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  function record(value) {
    return isRecord(value) ? value : {};
  }

  function array(value) {
    return Array.isArray(value) ? value : [];
  }

  function text(value, fallback) {
    if (typeof value === "string" && value.length > 0) {
      return value;
    }
    if (typeof value === "number" || typeof value === "boolean") {
      return String(value);
    }
    return fallback === undefined ? "—" : fallback;
  }

  function finiteNumber(value) {
    return typeof value === "number" && Number.isFinite(value) ? value : null;
  }

  function collection(value) {
    const source = record(value);
    return {
      source: source,
      items: array(source.items),
      total: finiteNumber(source.total),
      totalUnknown: Object.prototype.hasOwnProperty.call(source, "total") && source.total === null,
      returned: finiteNumber(source.returned),
      truncated: source.truncated === true,
      reason: text(source.truncation_reason, ""),
    };
  }

  function returnedCount(value) {
    return value.returned === null ? value.items.length : value.returned;
  }

  function totalOrReturned(value) {
    if (value.total !== null) {
      return value.total;
    }
    return returnedCount(value);
  }

  function symbolIdentity(value) {
    const symbol = record(value);
    return JSON.stringify([
      text(symbol.file, ""),
      text(symbol.name, ""),
      text(symbol.kind, ""),
      finiteNumber(symbol.line),
    ]);
  }

  function symbolMatches(left, right) {
    return symbolIdentity(left) === symbolIdentity(right);
  }

  function semanticMatchesGraphEdge(value, edge) {
    const semantic = record(value);
    const fromFile = text(edge.from.symbol.file, "");
    const toFile = text(edge.to.symbol.file, "");
    const fromName = text(edge.from.symbol.name, "");
    const toName = text(edge.to.symbol.name, "");
    const semanticFromFile = text(semantic.from_file, "");
    const semanticToFile = text(semantic.to_file, "");
    const semanticFromName = text(semantic.from_display_name, "");
    const semanticToName = text(semantic.to_display_name, "");
    const semanticFromLine = finiteNumber(semantic.from_line);
    const semanticToLine = finiteNumber(semantic.to_line);
    const fromLine = finiteNumber(edge.from.symbol.line);
    const toLine = finiteNumber(edge.to.symbol.line);
    const direct = semanticFromFile === fromFile && semanticToFile === toFile
      && semanticFromName === fromName && semanticToName === toName
      && semanticFromLine === fromLine && semanticToLine === toLine;
    const reverse = semanticFromFile === toFile && semanticToFile === fromFile
      && semanticFromName === toName && semanticToName === fromName
      && semanticFromLine === toLine && semanticToLine === fromLine;
    return direct || reverse;
  }

  function shortOid(value) {
    const oid = text(value, "");
    return oid ? oid.slice(0, 8) : "unknown";
  }

  function compact(value, maximum) {
    const source = text(value, "");
    if (source.length <= maximum) {
      return source || "—";
    }
    if (maximum < 5) {
      return source.slice(0, maximum);
    }
    return source.slice(0, maximum - 1) + "…";
  }

  function displayNumber(value) {
    if (value === null) {
      return "—";
    }
    return new Intl.NumberFormat().format(value);
  }

  function createElement(tagName, className, content) {
    const node = document.createElement(tagName);
    if (className) {
      node.className = className;
    }
    if (content !== undefined) {
      node.textContent = content;
    }
    return node;
  }

  function createSvg(tagName, attributes, content) {
    const node = document.createElementNS(SVG_NS, tagName);
    Object.entries(attributes || {}).forEach(function (entry) {
      node.setAttribute(entry[0], String(entry[1]));
    });
    if (content !== undefined) {
      node.textContent = content;
    }
    return node;
  }

  function clearNode(node) {
    node.replaceChildren();
  }

  function setEnabled(enabled) {
    elements.search.disabled = !enabled;
    elements.clearSearch.disabled = !enabled;
    elements.fit.disabled = !enabled;
    elements.scopeButtons.forEach(function (button) {
      button.disabled = !enabled;
    });
    elements.overlayButtons.forEach(function (button) {
      button.disabled = !enabled;
    });
  }

  function announce(message) {
    elements.statusRegion.textContent = "";
    window.setTimeout(function () {
      elements.statusRegion.textContent = message;
    }, 10);
  }

  function crossingFor(apiCrossings, seed, target) {
    return apiCrossings.find(function (crossing) {
      return symbolMatches(crossing.seed, seed) && symbolMatches(crossing.impacted, target);
    }) || null;
  }

  function knownComponentPaths(mapComponents, affectedComponents) {
    const values = [];
    affectedComponents.forEach(function (item) {
      const name = text(record(item).component, "");
      if (name) {
        values.push(name);
      }
    });
    mapComponents.forEach(function (item) {
      const name = text(record(item).path, "");
      if (name) {
        values.push(name);
      }
    });
    return Array.from(new Set(values)).sort(function (left, right) {
      return right.length - left.length || left.localeCompare(right);
    });
  }

  function pathComponent(file, paths) {
    const fileName = text(file, "");
    if (!fileName) {
      return null;
    }
    const match = paths.find(function (path) {
      if (path === "." || path === "") {
        return true;
      }
      const normalized = path.endsWith("/") ? path.slice(0, -1) : path;
      return fileName === normalized || fileName.startsWith(normalized + "/");
    });
    return match || null;
  }

  function componentEvidence(symbol, type, item, apiCrossings, paths) {
    const exactCrossing = apiCrossings.find(function (crossing) {
      if (type === "changed") {
        return symbolMatches(crossing.seed, symbol) && text(crossing.changed_component, "") !== "";
      }
      return symbolMatches(crossing.impacted, symbol) && text(crossing.impacted_component, "") !== "";
    });

    if (exactCrossing) {
      const exactName = type === "changed" ? exactCrossing.changed_component : exactCrossing.impacted_component;
      return { name: text(exactName, "Unclassified"), basis: "API crossing" };
    }

    if (type === "test") {
      const explicitEvidence = array(record(item).evidence).find(function (evidence) {
        return text(record(evidence).component, "") !== "";
      });
      if (explicitEvidence) {
        return { name: text(record(explicitEvidence).component, "Unclassified"), basis: "Test evidence" };
      }
    }

    const pathMatch = pathComponent(record(symbol).file, paths);
    if (pathMatch) {
      return { name: pathMatch, basis: "Map path match" };
    }
    return { name: "Unclassified", basis: "No component evidence" };
  }

  function normalizeFileEvidence(value) {
    const source = record(value);
    const coverage = isRecord(source.coverage) ? record(source.coverage) : null;
    const ownership = isRecord(source.ownership) ? record(source.ownership) : null;
    const churn = isRecord(source.churn) ? record(source.churn) : null;
    const testResults = isRecord(source.test_results) ? record(source.test_results) : null;
    const runtime = isRecord(source.runtime) ? record(source.runtime) : null;
    return {
      path: text(source.path, ""),
      findings: array(source.findings).map(record),
      coverage: coverage,
      ownership: ownership,
      churn: churn,
      testResults: testResults,
      runtime: runtime,
      knowledge: array(source.knowledge).map(record),
    };
  }

  function ownerNames(evidence) {
    return evidence && evidence.ownership
      ? array(evidence.ownership.codeowners).map(function (value) { return text(value, ""); }).filter(Boolean)
      : [];
  }

  function hasCodeownersEvidence(evidence) {
    return evidence && evidence.ownership
      ? text(evidence.ownership.codeowners_source_id, "") !== "" || ownerNames(evidence).length > 0
      : false;
  }

  function ownerLabel(evidence) {
    const owners = ownerNames(evidence);
    return owners.length > 0 ? owners.join(", ") : "explicitly unowned";
  }

  function ownershipEndpointLabel(evidence) {
    return hasCodeownersEvidence(evidence) ? ownerLabel(evidence) : "no CODEOWNERS match returned";
  }

  function ownershipBoundary(left, right) {
    if (!hasCodeownersEvidence(left) || !hasCodeownersEvidence(right)) {
      return false;
    }
    const leftOwners = new Set(ownerNames(left).map(function (owner) { return owner.toLowerCase(); }));
    const rightOwners = new Set(ownerNames(right).map(function (owner) { return owner.toLowerCase(); }));
    return leftOwners.size !== rightOwners.size || Array.from(leftOwners).some(function (owner) {
      return !rightOwners.has(owner);
    });
  }

  function overlayEnabled(key) {
    return state.overlays.has(key);
  }

  function coverageLabel(coverage) {
    const found = finiteNumber(record(coverage).lines_found);
    const hit = finiteNumber(record(coverage).lines_hit);
    if (found === null || hit === null || found === 0) {
      return "Coverage count unavailable";
    }
    return Math.round((hit / found) * 100) + "% reported lines covered (" + hit + "/" + found + ")";
  }

  function evidenceSignals(node) {
    const evidence = node.evidence;
    if (!evidence) {
      return [];
    }
    const signals = [];
    if (overlayEnabled("findings") && evidence.findings.length > 0) {
      signals.push(evidence.findings.length + " file-level SARIF finding" + (evidence.findings.length === 1 ? "" : "s"));
    }
    if (overlayEnabled("coverage") && evidence.coverage) {
      signals.push(coverageLabel(evidence.coverage));
    }
    if (overlayEnabled("ownership") && hasCodeownersEvidence(evidence)) {
      signals.push("CODEOWNERS " + ownerLabel(evidence));
    }
    if (overlayEnabled("churn") && evidence.churn) {
      const commits = finiteNumber(evidence.churn.commits);
      signals.push(displayNumber(commits) + " recent commit" + (commits === 1 ? "" : "s"));
    }
    if (overlayEnabled("tests") && evidence.testResults) {
      const failed = (finiteNumber(evidence.testResults.failed) || 0) + (finiteNumber(evidence.testResults.errors) || 0);
      signals.push(displayNumber(failed) + " failed JUnit case" + (failed === 1 ? "" : "s"));
    }
    if (overlayEnabled("runtime") && evidence.runtime) {
      const spans = finiteNumber(evidence.runtime.spans);
      signals.push(displayNumber(spans) + " runtime span" + (spans === 1 ? "" : "s"));
    }
    if (overlayEnabled("knowledge") && evidence.knowledge.length > 0) {
      signals.push(evidence.knowledge.length + " exact project-knowledge match" + (evidence.knowledge.length === 1 ? "" : "es"));
    }
    return signals;
  }

  function evidenceMark(node) {
    const evidence = node.evidence;
    if (!evidence) {
      return "";
    }
    const marks = [];
    if (overlayEnabled("findings") && evidence.findings.length > 0) {
      marks.push("S" + evidence.findings.length);
    }
    if (overlayEnabled("coverage") && evidence.coverage) {
      const found = finiteNumber(evidence.coverage.lines_found);
      const hit = finiteNumber(evidence.coverage.lines_hit);
      marks.push(found && hit !== null ? "C" + Math.round((hit / found) * 100) : "C?");
    }
    if (overlayEnabled("ownership") && hasCodeownersEvidence(evidence)) {
      marks.push(ownerNames(evidence).length > 0 ? "O" : "O0");
    }
    if (overlayEnabled("churn") && evidence.churn) {
      marks.push("G" + displayNumber(finiteNumber(evidence.churn.commits)));
    }
    if (overlayEnabled("tests") && evidence.testResults) {
      const failed = (finiteNumber(evidence.testResults.failed) || 0) + (finiteNumber(evidence.testResults.errors) || 0);
      marks.push("T" + displayNumber(failed) + "F");
    }
    if (overlayEnabled("runtime") && evidence.runtime) {
      marks.push("R" + displayNumber(finiteNumber(evidence.runtime.spans)));
    }
    if (overlayEnabled("knowledge") && evidence.knowledge.length > 0) {
      marks.push("K" + evidence.knowledge.length);
    }
    return marks.join(" · ");
  }

  function normalizePayload(payload) {
    const raw = record(payload);
    const map = record(raw.map);
    const impact = record(raw.impact);
    const evidence = record(raw.evidence);
    const semantic = record(raw.semantic);
    const temporalEnvelope = record(raw.temporal);
    const temporal = record(temporalEnvelope.data);
    const changes = record(impact.changes);

    const files = collection(changes.files);
    const changedSymbols = collection(changes.symbols);
    const impactedSymbols = collection(impact.impact);
    const tests = collection(impact.tests);
    const affectedComponents = collection(impact.affected_components);
    const apiCrossings = collection(impact.api_crossings);
    const mapComponents = collection(map.components);
    const evidenceSources = collection(evidence.sources);
    const evidenceFiles = collection(evidence.files);
    const evidenceDiagnostics = collection(evidence.diagnostics);
    const runtimeEdges = collection(evidence.runtime_edges);
    const semanticEdges = collection(semantic.edges);
    const evidenceByPath = new Map();
    evidenceFiles.items.map(normalizeFileEvidence).forEach(function (item) {
      if (item.path) {
        evidenceByPath.set(item.path, item);
      }
    });
    const paths = knownComponentPaths(mapComponents.items, affectedComponents.items);

    const nodes = [];
    changedSymbols.items.forEach(function (value, index) {
      const symbol = record(value);
      nodes.push({
        id: "changed:" + index,
        type: "changed",
        symbol: symbol,
        item: symbol,
        component: componentEvidence(symbol, "changed", symbol, apiCrossings.items, paths),
        crossings: apiCrossings.items.filter(function (crossing) {
          return symbolMatches(record(crossing).seed, symbol);
        }),
      });
    });
    impactedSymbols.items.forEach(function (value, index) {
      const item = record(value);
      const symbol = record(item.symbol);
      nodes.push({
        id: "impacted:" + index,
        type: "impacted",
        symbol: symbol,
        item: item,
        component: componentEvidence(symbol, "impacted", item, apiCrossings.items, paths),
        crossings: apiCrossings.items.filter(function (crossing) {
          return symbolMatches(record(crossing).impacted, symbol);
        }),
      });
    });
    tests.items.forEach(function (value, index) {
      const item = record(value);
      const symbol = record(item.symbol);
      nodes.push({
        id: "test:" + index,
        type: "test",
        symbol: symbol,
        item: item,
        component: componentEvidence(symbol, "test", item, apiCrossings.items, paths),
        crossings: apiCrossings.items.filter(function (crossing) {
          return symbolMatches(record(crossing).impacted, symbol);
        }),
      });
    });
    nodes.forEach(function (node) {
      node.evidence = evidenceByPath.get(text(node.symbol.file, "")) || null;
    });

    const changedByIdentity = new Map();
    nodes.filter(function (node) {
      return node.type === "changed";
    }).forEach(function (node) {
      const key = symbolIdentity(node.symbol);
      const matches = changedByIdentity.get(key) || [];
      matches.push(node);
      changedByIdentity.set(key, matches);
    });
    const edges = [];
    nodes.filter(function (node) {
      return node.type === "impacted";
    }).forEach(function (node) {
      array(node.item.seeds).forEach(function (seedValue, seedIndex) {
        const seed = record(seedValue);
        const sources = changedByIdentity.get(symbolIdentity(seed)) || [];
        sources.forEach(function (source, sourceIndex) {
          const crossing = crossingFor(apiCrossings.items.map(record), seed, node.symbol);
          edges.push({
            id: "impact-edge:" + node.id + ":" + seedIndex + ":" + sourceIndex,
            type: "impact",
            from: source,
            to: node,
            seed: seed,
            evidence: null,
            crossing: crossing,
            minimumDepth: finiteNumber(node.item.minimum_depth),
            precision: array(node.item.edge_precision).map(function (value) { return text(value, ""); }).filter(Boolean),
            collisionCount: finiteNumber(node.item.name_collision_count),
          });
        });
      });
    });
    nodes.filter(function (node) {
      return node.type === "test";
    }).forEach(function (node) {
      array(node.item.evidence).forEach(function (evidenceValue, evidenceIndex) {
        const evidence = record(evidenceValue);
        const seed = record(evidence.seed);
        const sources = changedByIdentity.get(symbolIdentity(seed)) || [];
        sources.forEach(function (source, sourceIndex) {
          const crossing = crossingFor(apiCrossings.items.map(record), seed, node.symbol);
          edges.push({
            id: "test-edge:" + node.id + ":" + evidenceIndex + ":" + sourceIndex,
            type: "test",
            from: source,
            to: node,
            seed: seed,
            evidence: evidence,
            crossing: crossing,
            minimumDepth: finiteNumber(node.item.minimum_depth),
            precision: [],
            collisionCount: null,
          });
        });
      });
    });
    edges.forEach(function (edge) {
      edge.ownershipBoundary = ownershipBoundary(edge.from.evidence, edge.to.evidence);
      const fromFile = text(edge.from.symbol.file, "");
      const toFile = text(edge.to.symbol.file, "");
      edge.runtimeEvidence = runtimeEdges.items.map(record).filter(function (runtimeEdge) {
        const parent = text(runtimeEdge.parent_file, "");
        const child = text(runtimeEdge.child_file, "");
        return (parent === fromFile && child === toFile) || (parent === toFile && child === fromFile);
      });
      edge.semanticEvidence = semanticEdges.items.map(record).filter(function (semanticEdge) {
        return semanticMatchesGraphEdge(semanticEdge, edge);
      });
    });

    const componentRows = buildComponentRows(
      affectedComponents.items,
      mapComponents.items,
      nodes
    );
    const precisionNotes = collectPrecisionNotes(map, impact, evidence, semantic);
    array(temporal.diagnostics).forEach(function (value) {
      const item = record(value);
      precisionNotes.push({
        code: text(item.code, "Temporal precision"),
        message: text(item.message, ""),
        source: "Temporal",
      });
    });
    const temporalDiagnostic = record(temporalEnvelope.diagnostic);
    if (Object.keys(temporalDiagnostic).length > 0) {
      precisionNotes.push({
        code: text(temporalDiagnostic.code, "Temporal unavailable"),
        message: text(temporalDiagnostic.message, ""),
        source: "Temporal",
      });
    }
    const truncations = collectTruncations(map, impact, evidence, semantic);
    appendTemporalTruncations(truncations, temporal);
    const limits = collectLimits(raw, map, impact, evidence);
    flattenPrimitiveEntries("Temporal", temporal.limits, 0, limits);

    return {
      raw: raw,
      schemaVersion: finiteNumber(raw.schema_version),
      repository: record(raw.repository),
      options: record(raw.options),
      map: map,
      impact: impact,
      baseline: record(impact.baseline),
      files: files,
      changedSymbols: changedSymbols,
      impactedSymbols: impactedSymbols,
      tests: tests,
      affectedComponents: affectedComponents,
      apiCrossings: apiCrossings,
      mapComponents: mapComponents,
      evidence: evidence,
      semantic: semantic,
      temporalEnvelope: temporalEnvelope,
      temporal: temporal,
      evidenceSources: evidenceSources,
      evidenceFiles: evidenceFiles,
      evidenceDiagnostics: evidenceDiagnostics,
      runtimeEdges: runtimeEdges,
      semanticEdges: semanticEdges,
      evidenceByPath: evidenceByPath,
      nodes: nodes,
      edges: edges,
      components: componentRows,
      precisionNotes: precisionNotes,
      truncations: truncations,
      limits: limits,
    };
  }

  function appendTemporalTruncations(output, temporal) {
    const components = record(temporal.components);
    const boundaries = record(temporal.boundaries);
    const cycles = record(temporal.cycles);
    const hotspots = record(temporal.hotspots);
    const ownership = record(temporal.ownership);
    [
      ["Temporal components added", components.added],
      ["Temporal components removed", components.removed],
      ["Temporal components changed", components.changed],
      ["Temporal boundaries added", boundaries.added],
      ["Temporal boundaries removed", boundaries.removed],
      ["Temporal boundaries changed", boundaries.changed],
      ["Temporal cycles introduced", cycles.added],
      ["Temporal cycles resolved", cycles.removed],
      ["Temporal cycles changed", cycles.changed],
      ["Temporal centrality", temporal.centrality],
      ["Temporal hotspot entries", hotspots.entered],
      ["Temporal hotspot exits", hotspots.exited],
      ["Temporal ownership", ownership.changes],
      ["Temporal history review", temporal.history_review_candidates],
    ].forEach(function (entry) {
      const value = collection(entry[1]);
      if (value.truncated || value.totalUnknown) {
        output.push({ label: entry[0], reason: value.reason || "bounded projection" });
      }
    });
    if (temporal.partial === true && !output.some(function (item) { return item.label.startsWith("Temporal"); })) {
      output.push({ label: "Temporal architecture", reason: "bounded base/head map" });
    }
  }

  function buildComponentRows(affectedItems, mapItems, nodes) {
    const rows = new Map();
    affectedItems.forEach(function (value) {
      const item = record(value);
      const name = text(item.component, "");
      if (!name) {
        return;
      }
      rows.set(name, {
        name: name,
        changed: finiteNumber(item.changed_symbols),
        impacted: finiteNumber(item.impacted_symbols),
        tests: finiteNumber(item.candidate_tests),
        map: null,
      });
    });
    mapItems.forEach(function (value) {
      const item = record(value);
      const name = text(item.path, "");
      if (!name) {
        return;
      }
      if (rows.has(name)) {
        rows.get(name).map = item;
      } else if (nodes.some(function (node) { return node.component.name === name; })) {
        rows.set(name, {
          name: name,
          changed: null,
          impacted: null,
          tests: null,
          map: item,
        });
      }
    });
    if (nodes.some(function (node) { return node.component.name === "Unclassified"; })) {
      rows.set("Unclassified", {
        name: "Unclassified",
        changed: null,
        impacted: null,
        tests: null,
        map: null,
      });
    }
    return Array.from(rows.values()).sort(function (left, right) {
      return left.name.localeCompare(right.name);
    });
  }

  function collectPrecisionNotes(map, impact, evidence, semantic) {
    const notes = [];
    array(map.precision_notes).forEach(function (value) {
      const note = record(value);
      if (Object.keys(note).length > 0) {
        const code = text(note.code, "Map precision");
        const message = text(note.message, "No explanatory message returned.");
        notes.push({ code: code, message: message, source: "Map" });
      } else {
        notes.push({ code: text(value, "Map precision"), message: "", source: "Map" });
      }
    });
    array(impact.precision_notes).forEach(function (value) {
      const note = record(value);
      if (Object.keys(note).length > 0) {
        notes.push({
          code: text(note.code, "Impact precision"),
          message: text(note.message, ""),
          source: "Impact",
        });
      } else {
        notes.push({ code: text(value, "Impact precision"), message: "", source: "Impact" });
      }
    });
    array(evidence.precision_notes).forEach(function (value) {
      const note = record(value);
      notes.push({
        code: text(note.code, "Evidence precision"),
        message: text(note.message, ""),
        source: "Evidence · " + text(note.source_id, "unknown source"),
      });
    });
    collection(evidence.diagnostics).items.forEach(function (value) {
      const diagnostic = record(value);
      notes.push({
        code: text(diagnostic.code, "Evidence diagnostic"),
        message: text(diagnostic.message, ""),
        source: "Evidence · " + text(diagnostic.source_id, "unknown source"),
      });
    });
    array(semantic.diagnostics).forEach(function (value) {
      const diagnostic = record(value);
      notes.push({
        code: text(diagnostic.code, "Semantic diagnostic"),
        message: text(diagnostic.message, ""),
        source: "Semantic · SCIP",
      });
    });
    const seen = new Set();
    return notes.filter(function (note) {
      const key = note.source + "|" + note.code + "|" + note.message;
      if (seen.has(key)) {
        return false;
      }
      seen.add(key);
      return true;
    });
  }

  function collectTruncations(map, impact, evidence, semantic) {
    const results = [];
    const changes = record(impact.changes);
    const candidates = [
      ["Changed files", changes.files],
      ["Changed symbols", changes.symbols],
      ["Affected components", impact.affected_components],
      ["Impacted symbols", impact.impact],
      ["API crossings", impact.api_crossings],
      ["Candidate tests", impact.tests],
      ["Map files", map.files],
      ["Map languages", map.languages],
      ["Map components", map.components],
      ["Map entry points", map.entry_points],
      ["Map hotspots", map.hotspots],
      ["Map cycles", map.cycles],
      ["Evidence files", evidence.files],
      ["Runtime evidence edges", evidence.runtime_edges],
      ["Evidence diagnostics", evidence.diagnostics],
      ["SCIP semantic definitions", semantic.definitions],
      ["SCIP semantic edges", semantic.edges],
    ];
    candidates.forEach(function (candidate) {
      const value = collection(candidate[1]);
      if (value.truncated || value.totalUnknown) {
        results.push({
          label: candidate[0],
          reason: value.reason || (value.totalUnknown ? "incomplete count" : "unspecified limit"),
          returned: returnedCount(value),
          total: value.total,
        });
      }
    });
    collection(map.components).items.forEach(function (value) {
      const component = record(value);
      const boundaries = collection(component.boundaries);
      if (boundaries.truncated || boundaries.totalUnknown) {
        results.push({
          label: "Boundaries · " + text(component.path, "unnamed component"),
          reason: boundaries.reason || "incomplete count",
          returned: returnedCount(boundaries),
          total: boundaries.total,
        });
      }
    });
    if (record(map.scope).aggregation_paths_truncated === true) {
      results.push({
        label: "Aggregation paths",
        reason: "path work limit",
        returned: null,
        total: null,
      });
    }
    return results;
  }

  function flattenPrimitiveEntries(prefix, value, depth, output) {
    if (depth > 3) {
      return;
    }
    if (value === null || typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
      output.push([prefix, text(value, value === null ? "null" : "—")]);
      return;
    }
    if (Array.isArray(value)) {
      output.push([prefix, value.map(function (item) { return text(item, ""); }).filter(Boolean).join(", ") || "empty"]);
      return;
    }
    if (isRecord(value)) {
      Object.keys(value).sort().forEach(function (key) {
        const next = prefix ? prefix + " / " + key : key;
        flattenPrimitiveEntries(next, value[key], depth + 1, output);
      });
    }
  }

  function collectLimits(raw, map, impact, evidence) {
    const entries = [];
    flattenPrimitiveEntries("Map", map.limits, 0, entries);
    flattenPrimitiveEntries("Impact", impact.limits, 0, entries);
    flattenPrimitiveEntries("Evidence", evidence.limits, 0, entries);
    const baseline = record(impact.baseline);
    ["includes_worktree", "includes_untracked"].forEach(function (key) {
      if (Object.prototype.hasOwnProperty.call(baseline, key)) {
        entries.push(["Baseline / " + key, text(baseline[key], baseline[key] === null ? "null" : "—")]);
      }
    });
    const options = record(raw.options);
    ["path", "depth", "top", "production_only"].forEach(function (key) {
      if (Object.prototype.hasOwnProperty.call(options, key)) {
        entries.push(["Option / " + key, text(options[key], options[key] === null ? "null" : "—")]);
      }
    });
    return entries;
  }

  function initializeElements() {
    elements.repositoryName = byId("repository-name");
    elements.repositoryRoot = byId("repository-root");
    elements.baselineRef = byId("baseline-ref");
    elements.baselineOid = byId("baseline-oid");
    elements.headOid = byId("head-oid");
    elements.refresh = byId("refresh-button");
    elements.snapshotAge = byId("snapshot-age");
    elements.noticeStack = byId("notice-stack");
    elements.instrumentSummary = byId("instrument-summary");
    elements.search = byId("trace-search");
    elements.clearSearch = byId("clear-search");
    elements.scopeButtons = Array.from(document.querySelectorAll("[data-scope]"));
    elements.fit = byId("fit-button");
    elements.overlayButtons = Array.from(document.querySelectorAll("[data-overlay]"));
    elements.evidenceSummary = byId("evidence-summary");
    elements.evidenceSourceList = byId("evidence-source-list");
    elements.temporalSummary = byId("temporal-summary");
    elements.temporalEvents = byId("temporal-events");
    elements.temporalMetric = {
      components: byId("temporal-components"),
      boundaries: byId("temporal-boundaries"),
      cycles: byId("temporal-cycles"),
      centrality: byId("temporal-centrality"),
      ownership: byId("temporal-ownership"),
      history: byId("temporal-history"),
    };
    elements.componentList = byId("component-list");
    elements.graphFrame = byId("graph-frame");
    elements.graph = byId("trace-graph");
    elements.graphState = byId("graph-state");
    elements.traceContext = byId("trace-context");
    elements.mobileTraceList = byId("mobile-trace-list");
    elements.traceCount = byId("trace-count");
    elements.inspector = byId("inspector-body");
    elements.precisionList = byId("precision-list");
    elements.precisionCount = byId("precision-count");
    elements.limitsList = byId("limits-list");
    elements.limitsCount = byId("limits-count");
    elements.schema = byId("schema-label");
    elements.statusRegion = byId("status-region");
    elements.metric = {
      files: { value: byId("metric-files"), note: byId("metric-files-note") },
      symbols: { value: byId("metric-symbols"), note: byId("metric-symbols-note") },
      impact: { value: byId("metric-impact"), note: byId("metric-impact-note") },
      crossings: { value: byId("metric-crossings"), note: byId("metric-crossings-note") },
      tests: { value: byId("metric-tests"), note: byId("metric-tests-note") },
    };
  }

  function bindEvents() {
    elements.refresh.addEventListener("click", function () {
      loadSnapshot(false);
    });
    elements.search.addEventListener("input", function () {
      state.search = elements.search.value.trim().toLocaleLowerCase();
      state.selectedId = null;
      state.focusedSeedId = null;
      state.activeNodeIds = null;
      state.disclosure = state.search ? "claims" : (state.component ? "claims" : "overview");
      resetPages();
      renderGraph();
      renderInspector();
      announceVisibleClaims();
    });
    elements.clearSearch.addEventListener("click", function () {
      elements.search.value = "";
      state.search = "";
      state.selectedId = null;
      state.focusedSeedId = null;
      state.activeNodeIds = null;
      state.disclosure = state.component ? "claims" : "overview";
      resetPages();
      renderGraph();
      renderInspector();
      elements.search.focus();
      announceVisibleClaims();
    });
    elements.scopeButtons.forEach(function (button) {
      button.addEventListener("click", function () {
        state.scope = button.dataset.scope || "all";
        elements.scopeButtons.forEach(function (candidate) {
          candidate.setAttribute("aria-pressed", String(candidate === button));
        });
        state.mobilePage = 0;
        renderGraph();
        announceVisibleClaims();
      });
    });
    elements.overlayButtons.forEach(function (button) {
      button.addEventListener("click", function () {
        const overlay = button.dataset.overlay;
        if (!OVERLAY_KEYS.includes(overlay)) {
          return;
        }
        if (state.overlays.has(overlay)) {
          state.overlays.delete(overlay);
        } else {
          state.overlays.add(overlay);
        }
        button.setAttribute("aria-pressed", String(state.overlays.has(overlay)));
        renderGraph();
        renderInspector();
        announce(overlay + " overlay " + (state.overlays.has(overlay) ? "shown" : "hidden") + ". Topology remains unchanged.");
      });
    });
    elements.fit.addEventListener("click", resetAperture);

    if (typeof ResizeObserver === "function") {
      const observer = new ResizeObserver(function () {
        const nextMode = elements.graphFrame.clientWidth < 700 ? "mobile" : "desktop";
        if (state.model && nextMode !== state.layoutMode) {
          state.layoutMode = nextMode;
          renderGraph();
        }
      });
      observer.observe(elements.graphFrame);
    } else {
      window.addEventListener("resize", function () {
        const nextMode = elements.graphFrame.clientWidth < 700 ? "mobile" : "desktop";
        if (state.model && nextMode !== state.layoutMode) {
          state.layoutMode = nextMode;
          renderGraph();
        }
      });
    }
  }

  async function loadSnapshot(initial) {
    if (state.refreshing) {
      return;
    }
    state.refreshing = true;
    state.loading = state.model === null;
    state.error = null;
    document.body.classList.toggle("is-refreshing", !initial || state.model !== null);
    elements.refresh.disabled = true;
    elements.graphFrame.setAttribute("aria-busy", "true");

    if (state.loading) {
      renderLoadingState();
      setEnabled(false);
      announce("Loading Lens snapshot.");
    } else {
      renderNotices();
      announce("Refreshing Lens snapshot.");
    }

    try {
      const embedded = document.getElementById("lens-snapshot");
      let payload;
      if (embedded) {
        try {
          payload = JSON.parse(embedded.textContent);
        } catch (error) {
          throw new LensRequestError("invalid_json", "The review package contains an unreadable snapshot.");
        }
      } else {
        const response = await fetch("/api/lens", {
          method: "GET",
          headers: { Accept: "application/json" },
          cache: "no-store",
        });
        try {
          payload = await response.json();
        } catch (error) {
          throw new LensRequestError("invalid_json", "The local server returned an unreadable snapshot.");
        }
        const apiError = record(record(payload).error);
        if (!response.ok || Object.keys(apiError).length > 0) {
          throw new LensRequestError(
            text(apiError.code, "http_" + response.status),
            text(apiError.message, "The local Lens endpoint could not produce a snapshot.")
          );
        }
      }

      state.raw = payload;
      state.model = normalizePayload(payload);
      state.loading = false;
      state.stale = false;
      state.error = null;
      state.fetchedAt = new Date();
      state.selectedId = null;
      state.focusedSeedId = null;
      state.activeNodeIds = null;
      state.disclosure = state.search || state.component ? "claims" : "overview";
      resetPages();
      if (state.component && !state.model.components.some(function (row) { return row.name === state.component; })) {
        state.component = null;
      }
      renderAll();
      announce(snapshotAnnouncement());
    } catch (error) {
      const requestError = error instanceof LensRequestError
        ? error
        : new LensRequestError("request_failed", "Could not reach the local Lens endpoint. Check that the command is still running, then retry.");
      state.error = requestError;
      state.loading = false;
      state.stale = state.model !== null;
      if (state.model) {
        renderAll();
        announce("Refresh failed. Showing the previous snapshot. " + requestError.message);
      } else {
        renderInitialError(requestError);
        announce("Lens snapshot failed. " + requestError.message);
      }
    } finally {
      state.refreshing = false;
      document.body.classList.remove("is-refreshing");
      elements.refresh.disabled = Boolean(document.getElementById("lens-snapshot"));
      elements.graphFrame.setAttribute("aria-busy", "false");
      updateSnapshotAge();
    }
  }

  function renderAll() {
    if (!state.model) {
      return;
    }
    renderHeader();
    renderMetrics();
    renderNotices();
    renderEvidenceSources();
    renderComponents();
    renderTemporal();
    renderMethodLedger();
    setEnabled(true);
    renderGraph();
    renderInspector();
  }

  function renderHeader() {
    const model = state.model;
    elements.repositoryName.textContent = text(model.repository.name, "Unnamed repository");
    elements.repositoryRoot.textContent = text(model.repository.root_label, text(model.options.path, "."));
    elements.baselineRef.textContent = text(model.baseline.requested_ref, text(model.options.since, "unspecified"));
    elements.baselineOid.textContent = shortOid(model.baseline.baseline_oid);
    elements.headOid.textContent = shortOid(model.baseline.head_oid);
    elements.schema.textContent = "Schema " + (model.schemaVersion === null ? "unknown" : model.schemaVersion);
    updateSnapshotAge();
  }

  function metricPresentation(value) {
    const returned = returnedCount(value);
    const partial = value.truncated || value.totalUnknown;
    let metricValue;
    if (value.total !== null) {
      metricValue = displayNumber(value.total);
    } else if (value.totalUnknown) {
      metricValue = "≥" + displayNumber(returned);
    } else {
      metricValue = displayNumber(returned);
    }
    let note = displayNumber(returned) + " returned";
    if (value.truncated) {
      note = "Partial · " + (value.reason || "bounded result");
    } else if (value.totalUnknown) {
      note = "Count incomplete · " + displayNumber(returned) + " returned";
    }
    return { value: metricValue, note: note, partial: partial };
  }

  function applyMetric(name, value) {
    const presentation = metricPresentation(value);
    elements.metric[name].value.textContent = presentation.value;
    elements.metric[name].note.textContent = presentation.note;
    const article = elements.metric[name].value.closest(".metric");
    article.classList.toggle("is-partial", presentation.partial);
  }

  function renderMetrics() {
    const model = state.model;
    applyMetric("files", model.files);
    applyMetric("symbols", model.changedSymbols);
    applyMetric("impact", model.impactedSymbols);
    applyMetric("crossings", model.apiCrossings);
    applyMetric("tests", model.tests);

    const files = totalOrReturned(model.files);
    const symbols = totalOrReturned(model.changedSymbols);
    const impacts = totalOrReturned(model.impactedSymbols);
    const crossings = totalOrReturned(model.apiCrossings);
    const tests = totalOrReturned(model.tests);
    let summary;
    if (files === 0 && symbols === 0) {
      summary = model.truncations.length > 0
        ? "No returned change evidence. The snapshot is partial; inspect its limits."
        : "No changes detected against the requested baseline.";
    } else if (crossings > 0) {
      summary = crossings + " boundary crossing" + (crossings === 1 ? "" : "s") + " observed; " + tests + " test candidate" + (tests === 1 ? "" : "s") + " returned.";
    } else if (impacts > 0) {
      summary = "The diff reaches " + impacts + " impacted symbol" + (impacts === 1 ? "" : "s") + " without a returned API crossing.";
    } else {
      summary = "The returned change set is contained; no downstream symbol impact was reported.";
    }
    elements.instrumentSummary.textContent = summary;
  }

  function appendNotice(type, code, message) {
    const notice = createElement("article", "notice notice--" + type);
    notice.appendChild(createElement("p", "notice__code", code));
    notice.appendChild(createElement("p", "", message));
    elements.noticeStack.appendChild(notice);
  }

  function renderNotices() {
    clearNode(elements.noticeStack);
    if (state.stale && state.error) {
      appendNotice(
        "error",
        "Stale · " + text(state.error.code, "refresh failed"),
        "Refresh failed, so this view retains the previous snapshot. " + state.error.message
      );
    }
    if (state.model) {
      if (state.model.schemaVersion !== null && state.model.schemaVersion !== EXPECTED_SCHEMA) {
        appendNotice(
          "warning",
          "Schema mismatch",
          "This UI targets schema 1 but received schema " + state.model.schemaVersion + ". Claims may be incomplete or unavailable."
        );
      }
      if (state.model.truncations.length > 0) {
        const labels = state.model.truncations.slice(0, 4).map(function (item) {
          return item.label + " (" + item.reason + ")";
        });
        const remaining = state.model.truncations.length - labels.length;
        const suffix = remaining > 0 ? "; plus " + remaining + " more limit" + (remaining === 1 ? "" : "s") : "";
        appendNotice(
          "warning",
          "Partial result",
          "Do not read this trace as complete: " + labels.join("; ") + suffix + ". See the calibration record below."
        );
      }
      if (text(state.model.temporalEnvelope.status, "unavailable") !== "available") {
        const diagnostic = record(state.model.temporalEnvelope.diagnostic);
        appendNotice(
          "warning",
          "Temporal · " + text(diagnostic.code, "unavailable"),
          text(diagnostic.message, "The base/head architecture comparison was not returned; the ordinary diff trace remains available.")
        );
      }
    }
    elements.noticeStack.hidden = elements.noticeStack.childElementCount === 0;
  }

  function temporalPair(added, removed, changed) {
    const plus = totalOrReturned(collection(added));
    const minus = totalOrReturned(collection(removed));
    const drift = changed === undefined ? null : totalOrReturned(collection(changed));
    return "+" + displayNumber(plus) + " −" + displayNumber(minus) + (drift === null ? "" : " ~" + displayNumber(drift));
  }

  function renderTemporal() {
    clearNode(elements.temporalEvents);
    const envelope = state.model.temporalEnvelope;
    const temporal = state.model.temporal;
    if (text(envelope.status, "unavailable") !== "available" || Object.keys(temporal).length === 0) {
      Object.values(elements.temporalMetric).forEach(function (element) {
        element.textContent = "—";
      });
      const diagnostic = record(envelope.diagnostic);
      elements.temporalSummary.textContent = text(
        diagnostic.message,
        "Temporal architecture is unavailable for this snapshot; the current impact trace is still valid."
      );
      elements.temporalEvents.appendChild(createElement("p", "temporal-events__empty", "No base/head architecture delta was returned."));
      return;
    }

    const summary = record(temporal.summary);
    const components = record(temporal.components);
    const boundaries = record(temporal.boundaries);
    const cycles = record(temporal.cycles);
    const ownership = record(temporal.ownership);
    const hotspots = record(temporal.hotspots);
    elements.temporalMetric.components.textContent = temporalPair(components.added, components.removed, components.changed);
    elements.temporalMetric.boundaries.textContent = temporalPair(boundaries.added, boundaries.removed, boundaries.changed);
    elements.temporalMetric.cycles.textContent = temporalPair(cycles.added, cycles.removed, cycles.changed);
    elements.temporalMetric.centrality.textContent = "↑" + displayNumber(finiteNumber(summary.centrality_increases));
    elements.temporalMetric.ownership.textContent = displayNumber(finiteNumber(summary.ownership_changes));
    elements.temporalMetric.history.textContent = displayNumber(finiteNumber(summary.history_review_candidates));
    const changed = summary.architecture_changed === true;
    elements.temporalSummary.textContent = changed
      ? "Architecture drift detected between " + text(record(temporal.baseline).requested_ref, "the baseline") + " and the indexed working copy" + (temporal.partial === true ? "; bounded projection is partial." : ".")
      : "No architecture drift was observed in the returned base/head projection.";

    const events = [];
    collection(components.added).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "added", label: "Component added", detail: text(item.path, "component") + " · " + displayNumber(finiteNumber(item.file_count)) + " files" });
    });
    collection(components.removed).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "removed", label: "Component removed", detail: text(item.path, "component") + " · " + displayNumber(finiteNumber(item.file_count)) + " files at baseline" });
    });
    collection(components.changed).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "changed", label: "Component shape changed", detail: text(item.path, "component") + " · " + displayNumber(finiteNumber(item.base_file_count)) + " → " + displayNumber(finiteNumber(item.head_file_count)) + " files" });
    });
    collection(cycles.added).items.forEach(function (value) {
      events.push({ kind: "cycle", label: "Cycle introduced", detail: array(value).map(function (path) { return text(path, ""); }).filter(Boolean).join(" → ") });
    });
    collection(cycles.removed).items.forEach(function (value) {
      events.push({ kind: "removed", label: "Cycle resolved", detail: array(value).map(function (path) { return text(path, ""); }).filter(Boolean).join(" → ") });
    });
    collection(cycles.changed).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "changed", label: "Cycle membership changed", detail: "+ " + array(item.added_members).join(", ") + " · − " + array(item.removed_members).join(", ") });
    });
    collection(boundaries.added).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "added", label: "Public boundary added", detail: text(item.name, "symbol") + " · " + text(item.component, "component") + " · " + text(item.file, "unknown file") });
    });
    collection(boundaries.removed).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "removed", label: "Public boundary removed", detail: text(item.name, "symbol") + " · " + text(item.component, "component") + " · " + text(item.file, "unknown file") });
    });
    collection(boundaries.changed).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "changed", label: "Public API drift", detail: text(item.name, "symbol") + " · " + text(item.file, "unknown file") });
    });
    collection(temporal.centrality).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "gravity", label: "Centrality drift", detail: text(item.name, "symbol") + " · " + displayNumber(finiteNumber(item.base_in_degree)) + " → " + displayNumber(finiteNumber(item.head_in_degree)) });
    });
    collection(hotspots.entered).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "gravity", label: "Hotspot entered", detail: text(item.name, "symbol") + " · " + text(item.file, "unknown file") + " · rank " + displayNumber(finiteNumber(item.rank)) });
    });
    collection(hotspots.exited).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "removed", label: "Hotspot exited", detail: text(item.name, "symbol") + " · " + text(item.file, "unknown file") + " · baseline rank " + displayNumber(finiteNumber(item.rank)) });
    });
    collection(ownership.changes).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "owner", label: "Ownership changed", detail: text(item.path, "unknown path") + " · " + array(item.base_owners).join(", ") + " → " + array(item.head_owners).join(", ") });
    });
    collection(temporal.history_review_candidates).items.forEach(function (value) {
      const item = record(value);
      events.push({ kind: "history", label: "History needs review", detail: text(item.artifact_path, "artifact") + " mentions " + text(item.referenced_path, "changed path") });
    });
    if (events.length === 0) {
      elements.temporalEvents.appendChild(createElement("p", "temporal-events__empty", "No architecture events were observed in the returned projection."));
      return;
    }
    events.slice(0, 12).forEach(function (event) {
      const row = createElement("article", "temporal-event temporal-event--" + event.kind);
      row.appendChild(createElement("span", "temporal-event__kind", event.label));
      row.appendChild(createElement("span", "temporal-event__detail", event.detail));
      elements.temporalEvents.appendChild(row);
    });
    if (events.length > 12) {
      elements.temporalEvents.appendChild(createElement("p", "temporal-events__empty", "+" + (events.length - 12) + " more bounded events in the JSON snapshot."));
    }
  }

  function renderEvidenceSources() {
    clearNode(elements.evidenceSourceList);
    const sources = state.model.evidenceSources.items.map(record);
    const semantic = record(state.model.semantic);
    const semanticSource = isRecord(semantic.source) ? record(semantic.source) : null;
    const sourceCount = sources.length + (semanticSource ? 1 : 0);
    const matchedPaths = new Set();
    state.model.evidenceFiles.items.map(record).forEach(function (file) {
      const path = text(file.path, "");
      if (path) {
        matchedPaths.add(path);
      }
    });
    state.model.semanticEdges.items.map(record).forEach(function (edge) {
      const from = text(edge.from_file, "");
      const to = text(edge.to_file, "");
      if (from) {
        matchedPaths.add(from);
      }
      if (to) {
        matchedPaths.add(to);
      }
    });
    const matchedFiles = matchedPaths.size;
    elements.evidenceSummary.textContent = sourceCount === 0
      ? "No external evidence sources loaded; the static graph remains available."
      : sourceCount + " source" + (sourceCount === 1 ? "" : "s") + " · " + matchedFiles + " matched trace file" + (matchedFiles === 1 ? "" : "s") + ((record(state.model.evidence).partial === true || semantic.partial === true) ? " · partial" : "");
    if (sourceCount === 0) {
      elements.evidenceSourceList.appendChild(createElement("p", "evidence-source-list__empty", "Use mastermind enrich --scip index.scip or pass external evidence flags to add corroborating facts."));
      return;
    }
    if (semanticSource) {
      const status = semantic.partial === true ? "partial" : "loaded";
      const card = createElement("article", "evidence-source evidence-source--" + status);
      card.appendChild(createElement("span", "evidence-source__kind", "scip · " + status));
      card.appendChild(createElement("span", "evidence-source__label", text(semanticSource.tool_name, "SCIP producer") + (text(semanticSource.tool_version, "") ? " " + text(semanticSource.tool_version, "") : "")));
      const identity = semanticSource.repository_verified === true ? "repository verified" : "repository unverified";
      card.appendChild(createElement("span", "evidence-source__facts", displayNumber(finiteNumber(semanticSource.edges)) + " compiler-resolved edges · " + displayNumber(finiteNumber(semanticSource.documents)) + " documents · " + identity));
      elements.evidenceSourceList.appendChild(card);
    }
    sources.forEach(function (source) {
      const status = text(source.status, "unknown");
      const card = createElement("article", "evidence-source evidence-source--" + status);
      card.appendChild(createElement("span", "evidence-source__kind", text(source.kind, "source") + " · " + status));
      card.appendChild(createElement("span", "evidence-source__label", text(source.label, "Unnamed source")));
      const returned = finiteNumber(source.facts_returned);
      const total = finiteNumber(source.facts_total);
      const facts = displayNumber(returned) + " facts" + (total === null ? "" : " / " + displayNumber(total)) + " · " + displayNumber(finiteNumber(source.files_matched)) + " files";
      card.appendChild(createElement("span", "evidence-source__facts", facts));
      elements.evidenceSourceList.appendChild(card);
    });
  }

  function componentMeta(row) {
    if (!row.map) {
      return row.name === "Unclassified" ? "No component evidence returned" : "No matching map row returned";
    }
    const mapRow = record(row.map);
    const fileCount = finiteNumber(mapRow.file_count);
    const languageItems = Array.isArray(mapRow.languages)
      ? mapRow.languages
      : collection(mapRow.languages).items;
    const languages = languageItems.map(function (value) {
      return text(record(value).language, "");
    }).filter(Boolean).slice(0, 3);
    const segments = [];
    if (fileCount !== null) {
      segments.push(displayNumber(fileCount) + " mapped file" + (fileCount === 1 ? "" : "s"));
    }
    if (languages.length > 0) {
      segments.push(languages.join(" / "));
    }
    return segments.join(" · ") || "Map row returned";
  }

  function appendComponentButton(label, counts, meta, componentName) {
    const button = createElement("button", "component-filter");
    button.type = "button";
    button._lensComponentName = componentName;
    const active = !state.activeNodeIds
      && (componentName === state.component || (componentName === null && state.component === null));
    button.setAttribute("aria-pressed", String(active));
    button.setAttribute("aria-label", label + ". " + counts + ". " + meta);
    button.appendChild(createElement("span", "component-filter__name", label));
    button.appendChild(createElement("span", "component-filter__counts", counts));
    button.appendChild(createElement("span", "component-filter__meta", meta));
    button.addEventListener("click", function () {
      state.component = componentName;
      state.selectedId = null;
      state.focusedSeedId = null;
      state.activeNodeIds = null;
      state.disclosure = componentName === null && !state.search ? "overview" : "claims";
      resetPages();
      Array.from(elements.componentList.querySelectorAll(".component-filter")).forEach(function (candidate) {
        candidate.setAttribute("aria-pressed", String(candidate._lensComponentName === state.component));
      });
      renderGraph();
      renderInspector();
      announceVisibleClaims();
    });
    elements.componentList.appendChild(button);
  }

  function renderComponents() {
    clearNode(elements.componentList);
    const allCounts = "Δ " + state.model.changedSymbols.items.length
      + " / I " + state.model.impactedSymbols.items.length
      + " / T " + state.model.tests.items.length;
    appendComponentButton("All components", allCounts, "Returned trace arrays", null);

    state.model.components.forEach(function (row) {
      const counts = "Δ " + displayNumber(row.changed)
        + " / I " + displayNumber(row.impacted)
        + " / T " + displayNumber(row.tests);
      appendComponentButton(row.name, counts, componentMeta(row), row.name);
    });
  }

  function renderMethodLedger() {
    const notes = state.model.precisionNotes;
    clearNode(elements.precisionList);
    if (notes.length === 0) {
      elements.precisionList.appendChild(createElement("li", "", "No precision notes were returned."));
    } else {
      notes.forEach(function (note) {
        const detail = note.message ? note.code + " — " + note.message : note.code;
        elements.precisionList.appendChild(createElement("li", "", note.source + " · " + detail));
      });
    }
    elements.precisionCount.textContent = notes.length + " note" + (notes.length === 1 ? "" : "s");

    clearNode(elements.limitsList);
    const status = state.model.truncations.length > 0 ? "Partial" : "No truncation reported";
    appendLimitRow("Snapshot", status);
    state.model.limits.forEach(function (entry) {
      appendLimitRow(entry[0], entry[1]);
    });
    state.model.truncations.forEach(function (item) {
      const counts = item.returned === null
        ? item.reason
        : displayNumber(item.returned) + " returned" + (item.total === null ? "" : " / " + displayNumber(item.total) + " total") + " · " + item.reason;
      appendLimitRow("Limited / " + item.label, counts);
    });
    const rowCount = elements.limitsList.childElementCount;
    elements.limitsCount.textContent = rowCount + " record" + (rowCount === 1 ? "" : "s");
  }

  function appendLimitRow(label, value) {
    const row = document.createElement("div");
    row.appendChild(createElement("dt", "", label));
    row.appendChild(createElement("dd", "", value));
    elements.limitsList.appendChild(row);
  }

  function nodeSearchText(node) {
    return [
      text(node.symbol.name, ""),
      text(node.symbol.file, ""),
      text(node.symbol.kind, ""),
      node.type,
      node.component.name,
      evidenceSignals(node).join(" "),
    ].join(" ").toLocaleLowerCase();
  }

  function resetPages() {
    state.lanePages = { changed: 0, impacted: 0, test: 0 };
    state.mobilePage = 0;
  }

  function apertureNodes() {
    if (!state.model) {
      return [];
    }
    return state.model.nodes.filter(function (node) {
      if (state.activeNodeIds && !state.activeNodeIds.has(node.id)) {
        return false;
      }
      if (state.component && node.component.name !== state.component) {
        return false;
      }
      if (state.search && !nodeSearchText(node).includes(state.search)) {
        return false;
      }
      return true;
    });
  }

  function edgesWithin(nodes) {
    const nodeIds = new Set(nodes.map(function (node) { return node.id; }));
    return state.model.edges.filter(function (edge) {
      return nodeIds.has(edge.from.id) && nodeIds.has(edge.to.id);
    });
  }

  function resetAperture() {
    state.scope = "all";
    state.search = "";
    state.component = null;
    state.selectedId = null;
    state.focusedSeedId = null;
    state.activeNodeIds = null;
    state.disclosure = "overview";
    resetPages();
    elements.search.value = "";
    elements.scopeButtons.forEach(function (button) {
      button.setAttribute("aria-pressed", String(button.dataset.scope === "all"));
    });
    renderComponents();
    renderGraph();
    renderInspector();
    announce("Trace aperture reset. Component clusters summarize every returned claim.");
  }

  function renderGraph() {
    if (!state.model) {
      return;
    }
    clearNode(elements.graph);
    clearNode(elements.mobileTraceList);
    elements.graphFrame.classList.remove("has-graph-state");
    elements.mobileTraceList.hidden = true;
    setGraphVisible(true);

    const nodes = apertureNodes();
    const zeroChange = totalOrReturned(state.model.files) === 0 && totalOrReturned(state.model.changedSymbols) === 0;
    if (zeroChange) {
      renderTraceContext("Zero-change result", "No changed files or symbols were returned for this baseline.", []);
      elements.traceCount.textContent = "0 claims returned";
      showGraphState(
        "No changes in scope",
        state.model.truncations.length > 0
          ? "No change evidence was returned, but the snapshot is partial. Review the limits before treating the baseline as clean."
          : "The working copy matches the requested baseline within the analyzed scope.",
        null
      );
      setEmptySvg();
      return;
    }
    if (nodes.length === 0) {
      renderTraceContext("Empty aperture", "The current search or component boundary excludes every returned claim.", []);
      elements.traceCount.textContent = "0 displayed / " + state.model.nodes.length + " returned";
      showGraphState(
        "Nothing in this aperture",
        "The current search or component filter excludes every returned claim.",
        { label: "Reset aperture", handler: resetAperture }
      );
      setEmptySvg();
      return;
    }

    elements.graphState.hidden = true;
    const width = Math.max(320, Math.floor(elements.graphFrame.clientWidth || 900));
    const mobile = width < 700;
    state.layoutMode = mobile ? "mobile" : "desktop";
    const localTrace = resolveLocalTrace(nodes);

    if (localTrace) {
      renderLocalTrace(localTrace, nodes, width, mobile);
      return;
    }
    if (mobile) {
      renderMobileIndex(nodes);
      return;
    }
    if (state.disclosure === "overview" && !state.search && !state.component && !state.activeNodeIds) {
      renderClusterOverview(nodes, width);
      return;
    }
    renderClaimAperture(nodes, width);
  }

  function setEmptySvg() {
    elements.graph.setAttribute("viewBox", "0 0 1 1");
    elements.graph.setAttribute("width", "1");
    elements.graph.setAttribute("height", "1");
  }

  function setGraphVisible(visible) {
    if (visible) {
      elements.graph.removeAttribute("hidden");
    } else {
      elements.graph.setAttribute("hidden", "");
    }
  }

  function renderTraceContext(mode, summary, actions) {
    clearNode(elements.traceContext);
    const copy = document.createElement("div");
    copy.appendChild(createElement("p", "trace-context__mode", mode));
    copy.appendChild(createElement("p", "trace-context__summary", summary));
    elements.traceContext.appendChild(copy);
    if (actions.length === 0) {
      return;
    }
    const controls = createElement("div", "trace-context__actions");
    actions.forEach(function (action) {
      if (action.kind === "label") {
        controls.appendChild(createElement("span", "trace-context__page", action.label));
        return;
      }
      const button = createElement("button", "", action.label);
      button.type = "button";
      button.disabled = action.disabled === true;
      button.setAttribute("aria-label", action.ariaLabel || action.label);
      button.addEventListener("click", action.handler);
      controls.appendChild(button);
    });
    elements.traceContext.appendChild(controls);
  }

  function resolveLocalTrace(nodes) {
    let claim = selectedClaim();
    let automatic = false;
    if (!claim && state.search && nodes.length === 1) {
      claim = nodes[0];
      automatic = true;
    }
    if (!claim) {
      return null;
    }

    let root = null;
    let target = null;
    if (claim.symbol) {
      target = claim;
      if (claim.type === "changed") {
        root = claim;
      } else {
        const incoming = state.model.edges.filter(function (edge) { return edge.to.id === claim.id; });
        root = incoming.find(function (edge) { return edge.from.id === state.focusedSeedId; });
        root = root ? root.from : (incoming[0] ? incoming[0].from : null);
      }
    } else {
      root = claim.from;
      target = claim.to;
    }
    return { claim: claim, root: root, target: target, automatic: automatic };
  }

  function uniqueTargetEdges(edges, forcedEdgeId) {
    const groups = new Map();
    edges.forEach(function (edge) {
      const existing = groups.get(edge.to.id) || [];
      existing.push(edge);
      groups.set(edge.to.id, existing);
    });
    return Array.from(groups.values()).map(function (group) {
      return group.find(function (edge) { return edge.id === forcedEdgeId; }) || group[0];
    });
  }

  function pageConnectedEdges(edges, type, pageSize, trace) {
    const forcedEdgeId = trace.claim && !trace.claim.symbol ? trace.claim.id : null;
    const representatives = uniqueTargetEdges(edges, forcedEdgeId);
    if (trace.target && trace.target.type === type) {
      const forcedIndex = representatives.findIndex(function (edge) { return edge.to.id === trace.target.id; });
      if (forcedIndex >= 0) {
        state.lanePages[type] = Math.floor(forcedIndex / pageSize);
      }
    }
    const pages = Math.max(1, Math.ceil(representatives.length / pageSize));
    state.lanePages[type] = Math.min(Math.max(0, state.lanePages[type]), pages - 1);
    const start = state.lanePages[type] * pageSize;
    return {
      all: representatives,
      visible: representatives.slice(start, start + pageSize),
      page: state.lanePages[type],
      pages: pages,
    };
  }

  function renderLocalTrace(trace, aperture, width, mobile) {
    if (!trace.root) {
      const loneNode = trace.target ? [trace.target] : [];
      renderTraceContext(
        trace.automatic ? "Search result" : "Selected claim",
        "No returned changed-symbol seed connects to this claim. The inspector still shows its repository evidence.",
        [{ label: trace.automatic ? "Clear search" : "Back to aperture", handler: trace.automatic ? clearSearchAperture : clearSelection }]
      );
      elements.traceCount.textContent = loneNode.length + " displayed / " + aperture.length + " in aperture";
      renderClaimSvg(loneNode, [], width, mobile);
      return;
    }

    const pageSize = mobile ? MOBILE_CONNECTED_PER_LANE : CONNECTED_PER_LANE;
    const outgoing = state.model.edges.filter(function (edge) { return edge.from.id === trace.root.id; });
    const impact = pageConnectedEdges(outgoing.filter(function (edge) { return edge.type === "impact"; }), "impacted", pageSize, trace);
    const tests = pageConnectedEdges(outgoing.filter(function (edge) { return edge.type === "test"; }), "test", pageSize, trace);
    const visibleEdges = impact.visible.concat(tests.visible);
    const visibleNodes = [trace.root];
    visibleEdges.forEach(function (edge) {
      if (!visibleNodes.some(function (node) { return node.id === edge.to.id; })) {
        visibleNodes.push(edge.to);
      }
    });
    if (trace.target && !visibleNodes.some(function (node) { return node.id === trace.target.id; })) {
      visibleNodes.push(trace.target);
    }

    const actions = [{
      label: trace.automatic ? "Clear search" : "Back to aperture",
      handler: trace.automatic ? clearSearchAperture : clearSelection,
    }];
    appendLanePagerActions(actions, "impacted", "Impact", impact);
    appendLanePagerActions(actions, "test", "Tests", tests);
    const connectedClaims = impact.all.length + tests.all.length;
    const qualifier = trace.automatic ? "Search-resolved trace" : "Selected local trace";
    renderTraceContext(
      qualifier,
      text(trace.root.symbol.name, "Changed seed") + " has " + connectedClaims + " connected returned claim" + (connectedClaims === 1 ? "" : "s") + "; " + visibleEdges.length + " exact seed edge" + (visibleEdges.length === 1 ? "" : "s") + " displayed.",
      actions
    );
    elements.traceCount.textContent = visibleNodes.length + " claims displayed / " + aperture.length + " in aperture";
    renderClaimSvg(visibleNodes, visibleEdges, width, mobile);
  }

  function appendLanePagerActions(actions, type, label, page) {
    if (page.pages <= 1) {
      return;
    }
    actions.push({
      label: "← " + label,
      ariaLabel: "Previous " + label.toLocaleLowerCase() + " page",
      disabled: page.page === 0,
      handler: function () { changeLanePage(type, -1); },
    });
    actions.push({ kind: "label", label: (page.page + 1) + "/" + page.pages });
    actions.push({
      label: label + " →",
      ariaLabel: "Next " + label.toLocaleLowerCase() + " page",
      disabled: page.page === page.pages - 1,
      handler: function () { changeLanePage(type, 1); },
    });
  }

  function changeLanePage(type, delta) {
    state.lanePages[type] += delta;
    renderGraph();
    announce(laneTitle(type) + " page changed. Connected evidence remains in the local trace.");
  }

  function clearSelection() {
    state.selectedId = null;
    state.focusedSeedId = null;
    resetPages();
    renderGraph();
    renderInspector();
    announce("Returned to the bounded claim aperture.");
  }

  function clearSearchAperture() {
    state.search = "";
    state.selectedId = null;
    state.focusedSeedId = null;
    state.disclosure = state.component || state.activeNodeIds ? "claims" : "overview";
    resetPages();
    elements.search.value = "";
    renderGraph();
    renderInspector();
    announce("Search cleared. Returned to the bounded aperture.");
  }

  function renderClaimAperture(nodes, width) {
    const grouped = groupNodes(nodes);
    const displayed = [];
    const actions = [];
    ["changed", "impacted", "test"].forEach(function (type) {
      const pages = Math.max(1, Math.ceil(grouped[type].length / CLAIMS_PER_LANE));
      state.lanePages[type] = Math.min(Math.max(0, state.lanePages[type]), pages - 1);
      const start = state.lanePages[type] * CLAIMS_PER_LANE;
      displayed.push.apply(displayed, grouped[type].slice(start, start + CLAIMS_PER_LANE));
      appendLanePagerActions(actions, type, laneShortLabel(type), {
        page: state.lanePages[type],
        pages: pages,
      });
    });
    if (state.search || state.component || state.activeNodeIds) {
      actions.unshift({ label: "All clusters", handler: showAllClusters });
    }
    const displayedEdges = uniqueEndpointEdges(edgesWithin(displayed));
    const scopeNote = state.scope === "all" ? "All claim types share equal emphasis." : laneTitle(state.scope) + " is emphasized; connected lanes remain available.";
    renderTraceContext(
      "Paged claim aperture",
      displayed.length + " of " + nodes.length + " matching claims displayed. " + scopeNote + " Select a changed claim to resolve its exact local trace.",
      actions
    );
    elements.traceCount.textContent = displayed.length + " displayed / " + nodes.length + " in aperture / " + state.model.nodes.length + " returned";
    renderClaimSvg(displayed, displayedEdges, width, false);
  }

  function laneShortLabel(type) {
    if (type === "changed") {
      return "Changes";
    }
    if (type === "impacted") {
      return "Impact";
    }
    return "Tests";
  }

  function uniqueEndpointEdges(edges) {
    const unique = new Map();
    edges.forEach(function (edge) {
      const key = edge.from.id + "|" + edge.to.id + "|" + edge.type;
      if (!unique.has(key)) {
        unique.set(key, edge);
      }
    });
    return Array.from(unique.values());
  }

  function showAllClusters() {
    state.search = "";
    state.component = null;
    state.activeNodeIds = null;
    state.selectedId = null;
    state.focusedSeedId = null;
    state.disclosure = "overview";
    resetPages();
    elements.search.value = "";
    renderComponents();
    renderGraph();
    renderInspector();
    announce("Showing the bounded component-cluster overview.");
  }

  function renderMobileIndex(nodes) {
    setGraphVisible(false);
    elements.mobileTraceList.hidden = false;
    elements.graphState.hidden = true;
    const scoped = state.scope === "all"
      ? nodes.filter(function (node) { return node.type === "changed"; })
      : nodes.filter(function (node) { return node.type === state.scope; });
    const candidates = scoped.length > 0 ? scoped : nodes;
    const pages = Math.max(1, Math.ceil(candidates.length / MOBILE_CANDIDATES_PER_PAGE));
    state.mobilePage = Math.min(Math.max(0, state.mobilePage), pages - 1);
    const start = state.mobilePage * MOBILE_CANDIDATES_PER_PAGE;
    const visible = candidates.slice(start, start + MOBILE_CANDIDATES_PER_PAGE);

    const actions = [];
    if (state.search || state.component || state.activeNodeIds) {
      actions.push({ label: "All clusters", handler: showAllClusters });
    }
    if (pages > 1) {
      actions.push({
        label: "← Previous",
        disabled: state.mobilePage === 0,
        handler: function () { state.mobilePage -= 1; renderGraph(); },
      });
      actions.push({ kind: "label", label: (state.mobilePage + 1) + "/" + pages });
      actions.push({
        label: "Next →",
        disabled: state.mobilePage === pages - 1,
        handler: function () { state.mobilePage += 1; renderGraph(); },
      });
    }
    const scopeLabel = state.scope === "all" ? "changed" : state.scope;
    renderTraceContext(
      "Mobile " + scopeLabel + " index",
      visible.length + " of " + candidates.length + " candidate claims displayed. Select one to open its compact exact trace and inspector.",
      actions
    );

    const list = createElement("ol", "mobile-candidate-list");
    visible.forEach(function (node) {
      const item = document.createElement("li");
      const button = createElement("button", "mobile-candidate mobile-candidate--" + node.type);
      button.type = "button";
      button.setAttribute("aria-label", nodeLabel(node));
      button.appendChild(createElement("span", "mobile-candidate__kind", text(node.symbol.kind, "unknown") + " / " + node.type));
      button.appendChild(createElement("span", "mobile-candidate__name", text(node.symbol.name, "Unnamed symbol")));
      button.appendChild(createElement("span", "mobile-candidate__path", text(node.symbol.file, "File unavailable") + formatLine(node.symbol.line)));
      const mobileEvidence = evidenceMark(node);
      button.appendChild(createElement("span", "mobile-candidate__meta", nodeMetadata(node) + (mobileEvidence ? " · " + mobileEvidence : "")));
      button.addEventListener("click", function () { selectClaim(node); });
      item.appendChild(button);
      list.appendChild(item);
    });
    elements.mobileTraceList.appendChild(list);

    const grouped = groupNodes(nodes);
    const summaries = createElement("div", "mobile-lane-summary");
    ["impacted", "test"].forEach(function (type) {
      const summary = document.createElement("p");
      summary.appendChild(createElement("strong", "", String(grouped[type].length)));
      summary.appendChild(document.createTextNode(laneTitle(type) + " in aperture"));
      summaries.appendChild(summary);
    });
    elements.mobileTraceList.appendChild(summaries);
    elements.traceCount.textContent = visible.length + " displayed / " + candidates.length + " " + scopeLabel + " candidates / " + state.model.nodes.length + " returned";
    setEmptySvg();
  }

  function renderClaimSvg(nodes, edges, width, mobile) {
    setGraphVisible(true);
    elements.mobileTraceList.hidden = true;
    elements.graphState.hidden = true;
    const layout = mobile ? mobileLayout(nodes, width) : desktopLayout(nodes, width);
    elements.graph.setAttribute("viewBox", "0 0 " + layout.width + " " + layout.height);
    elements.graph.setAttribute("width", String(layout.width));
    elements.graph.setAttribute("height", String(layout.height));
    drawLayoutBackground(layout);

    const edgeLayer = createSvg("g", { class: "graph-edges" });
    edges.forEach(function (edge) {
      const from = layout.positions.get(edge.from.id);
      const to = layout.positions.get(edge.to.id);
      if (from && to) {
        drawEdge(edgeLayer, edge, from, to, mobile, width);
      }
    });
    elements.graph.appendChild(edgeLayer);

    const nodeLayer = createSvg("g", { class: "graph-nodes" });
    nodes.forEach(function (node) {
      const position = layout.positions.get(node.id);
      if (position) {
        drawNode(nodeLayer, node, position, layout.textWidth);
      }
    });
    elements.graph.appendChild(nodeLayer);
  }

  function drawLayoutBackground(layout) {
    const backgrounds = createSvg("g", { "aria-hidden": "true" });
    layout.bands.forEach(function (band) { drawBand(backgrounds, band); });
    layout.axisLines.forEach(function (line) {
      backgrounds.appendChild(createSvg("line", {
        x1: line.x1,
        y1: line.y1,
        x2: line.x2,
        y2: line.y2,
        class: "graph-axis-line",
      }));
    });
    elements.graph.appendChild(backgrounds);
  }

  function renderClusterOverview(nodes, width) {
    const overview = buildClusterOverview(nodes);
    const layout = clusterLayout(overview.clusters, width);
    setGraphVisible(true);
    elements.mobileTraceList.hidden = true;
    elements.graphState.hidden = true;
    elements.graph.setAttribute("viewBox", "0 0 " + layout.width + " " + layout.height);
    elements.graph.setAttribute("width", String(layout.width));
    elements.graph.setAttribute("height", String(layout.height));
    drawLayoutBackground(layout);

    const edgeLayer = createSvg("g", { class: "graph-edges" });
    overview.edges.forEach(function (edge) {
      const from = layout.positions.get(edge.from.id);
      const to = layout.positions.get(edge.to.id);
      if (from && to) {
        drawAggregateEdge(edgeLayer, edge, from, to, width);
      }
    });
    elements.graph.appendChild(edgeLayer);

    const clusterLayer = createSvg("g", { class: "graph-clusters" });
    overview.clusters.forEach(function (cluster) {
      const position = layout.positions.get(cluster.id);
      if (position) {
        drawCluster(clusterLayer, cluster, position, layout.textWidth);
      }
    });
    elements.graph.appendChild(clusterLayer);

    renderTraceContext(
      "Component-cluster overview",
      nodes.length + " returned claims are compressed into " + overview.clusters.length + " bounded clusters; " + overview.edges.length + " exact seed-link groups are shown. Activate a cluster to browse its claims.",
      [{
        label: "Browse claims",
        handler: function () {
          state.disclosure = "claims";
          resetPages();
          renderGraph();
          announce("Showing a bounded page of individual returned claims.");
        },
      }]
    );
    elements.traceCount.textContent = overview.clusters.length + " clusters displayed / " + nodes.length + " claims / " + state.model.nodes.length + " returned";
  }

  function buildClusterOverview(nodes) {
    const clusters = [];
    const clusterByNodeId = new Map();
    ["changed", "impacted", "test"].forEach(function (type) {
      const groups = new Map();
      nodes.filter(function (node) { return node.type === type; }).forEach(function (node) {
        const key = node.component.name;
        const grouped = groups.get(key) || [];
        grouped.push(node);
        groups.set(key, grouped);
      });
      const ordered = Array.from(groups.entries()).sort(function (left, right) {
        return right[1].length - left[1].length || left[0].localeCompare(right[0]);
      });
      const retained = ordered.length > CLUSTERS_PER_LANE
        ? ordered.slice(0, CLUSTERS_PER_LANE - 1).concat([["Other components", ordered.slice(CLUSTERS_PER_LANE - 1).flatMap(function (entry) { return entry[1]; })]])
        : ordered;
      retained.forEach(function (entry, index) {
        const label = entry[0];
        const groupedNodes = entry[1];
        const components = Array.from(new Set(groupedNodes.map(function (node) { return node.component.name; })));
        const files = new Set(groupedNodes.map(function (node) { return text(node.symbol.file, ""); }).filter(Boolean));
        const crossings = groupedNodes.reduce(function (sum, node) { return sum + node.crossings.length; }, 0);
        const evidenceCount = groupedNodes.reduce(function (sum, node) {
          return sum + (evidenceSignals(node).length > 0 ? 1 : 0);
        }, 0);
        const cluster = {
          id: "cluster:" + type + ":" + index,
          type: type,
          label: label,
          count: groupedNodes.length,
          nodes: groupedNodes,
          componentName: components.length === 1 ? components[0] : null,
          componentNames: components,
          fileCount: files.size,
          crossingCount: crossings,
          evidenceCount: evidenceCount,
        };
        clusters.push(cluster);
        groupedNodes.forEach(function (node) { clusterByNodeId.set(node.id, cluster); });
      });
    });

    const aggregate = new Map();
    edgesWithin(nodes).forEach(function (edge) {
      const from = clusterByNodeId.get(edge.from.id);
      const to = clusterByNodeId.get(edge.to.id);
      if (!from || !to) {
        return;
      }
      const key = from.id + "|" + to.id + "|" + edge.type;
      const existing = aggregate.get(key) || {
        id: "aggregate:" + aggregate.size,
        type: edge.type,
        from: from,
        to: to,
        count: 0,
        crossingCount: 0,
      };
      existing.count += 1;
      if (edge.crossing) {
        existing.crossingCount += 1;
      }
      aggregate.set(key, existing);
    });
    return { clusters: clusters, edges: Array.from(aggregate.values()) };
  }

  function clusterLayout(clusters, width) {
    const margin = 12;
    const gap = 12;
    const laneWidth = (width - margin * 2 - gap * 2) / 3;
    const clusterWidth = laneWidth - 18;
    const headerHeight = 64;
    const rowHeight = 78;
    const grouped = {
      changed: clusters.filter(function (cluster) { return cluster.type === "changed"; }),
      impacted: clusters.filter(function (cluster) { return cluster.type === "impacted"; }),
      test: clusters.filter(function (cluster) { return cluster.type === "test"; }),
    };
    const maximum = Math.max(grouped.changed.length, grouped.impacted.length, grouped.test.length, 1);
    const height = Math.max(500, headerHeight + maximum * rowHeight + 28);
    const positions = new Map();
    const bands = [];
    ["changed", "impacted", "test"].forEach(function (type, laneIndex) {
      const x = margin + laneIndex * (laneWidth + gap);
      bands.push({
        type: type,
        x: x,
        y: 12,
        width: laneWidth,
        height: height - 24,
        count: grouped[type].length,
        title: laneTitle(type),
        index: "0" + (laneIndex + 1),
        countLabel: grouped[type].length + " clusters",
      });
      grouped[type].forEach(function (cluster, index) {
        positions.set(cluster.id, {
          x: x + 9,
          y: headerHeight + index * rowHeight,
          width: clusterWidth,
          height: 66,
        });
      });
    });
    return {
      width: width,
      height: height,
      positions: positions,
      bands: bands,
      axisLines: [],
      textWidth: Math.max(14, Math.floor((clusterWidth - 30) / 7)),
    };
  }

  function drawCluster(layer, cluster, position, textWidth) {
    const classes = ["graph-cluster", "graph-cluster--" + cluster.type];
    if (state.scope !== "all" && state.scope !== cluster.type) {
      classes.push("is-scope-muted");
    }
    const label = cluster.count + " " + cluster.type + " claims in " + cluster.label + ", across " + cluster.fileCount + " files, " + cluster.evidenceCount + " with visible overlays. Activate to browse individual claims.";
    const group = createSvg("g", {
      class: classes.join(" "),
      transform: "translate(" + position.x + " " + position.y + ")",
      tabindex: "0",
      focusable: "true",
      role: "button",
      "aria-label": label,
      "data-cluster-id": cluster.id,
    });
    group.appendChild(createSvg("title", {}, label));
    group.appendChild(createSvg("rect", {
      x: -4,
      y: -4,
      width: position.width + 8,
      height: position.height + 8,
      class: "graph-node__focus",
    }));
    group.appendChild(createSvg("rect", {
      x: 0,
      y: 0,
      width: position.width,
      height: position.height,
      class: "graph-cluster__surface",
    }));
    group.appendChild(createSvg("text", {
      x: 11,
      y: 15,
      class: "graph-cluster__index",
    }, cluster.type + " cluster"));
    group.appendChild(createSvg("text", {
      x: 11,
      y: 35,
      class: "graph-cluster__name",
    }, compact(cluster.label, textWidth)));
    group.appendChild(createSvg("text", {
      x: position.width - 10,
      y: 35,
      class: "graph-cluster__count",
      "text-anchor": "end",
    }, String(cluster.count)));
    group.appendChild(createSvg("text", {
      x: 11,
      y: 54,
      class: "graph-cluster__meta",
    }, cluster.fileCount + " file" + (cluster.fileCount === 1 ? "" : "s") + (cluster.crossingCount > 0 ? " · " + cluster.crossingCount + " crossings" : "") + (cluster.evidenceCount > 0 ? " · E " + cluster.evidenceCount : "")));
    activateSvgCluster(group, cluster);
    layer.appendChild(group);
  }

  function activateSvgCluster(element, cluster) {
    const expand = function () {
      state.selectedId = null;
      state.focusedSeedId = null;
      state.disclosure = "claims";
      if (cluster.componentName && cluster.label !== "Other components") {
        state.component = cluster.componentName;
        state.activeNodeIds = null;
      } else {
        state.component = null;
        state.activeNodeIds = new Set(state.model.nodes.filter(function (node) {
          return cluster.componentNames.includes(node.component.name);
        }).map(function (node) { return node.id; }));
      }
      resetPages();
      renderComponents();
      renderGraph();
      renderInspector();
      announce("Expanded " + cluster.label + ". A bounded page of individual claims is displayed.");
    };
    element.addEventListener("click", expand);
    element.addEventListener("keydown", function (event) {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        expand();
      }
    });
  }

  function drawAggregateEdge(layer, edge, from, to, width) {
    const path = edgePath(from, to, false, width, edge.type);
    const label = edge.count + " exact returned " + edge.type + " seed link" + (edge.count === 1 ? "" : "s") + " from " + edge.from.label + " to " + edge.to.label + (edge.crossingCount > 0 ? ", including " + edge.crossingCount + " boundary crossing" + (edge.crossingCount === 1 ? "" : "s") : "") + ". Activate to browse the source cluster.";
    const hit = createSvg("path", {
      d: path,
      class: "graph-edge-hit",
      tabindex: "0",
      focusable: "true",
      role: "button",
      "aria-label": label,
    });
    hit.appendChild(createSvg("title", {}, label));
    activateSvgCluster(hit, edge.from);
    const classes = ["graph-edge", "graph-edge--" + edge.type, "graph-edge--aggregate"];
    if (edge.crossingCount > 0) {
      classes.push("graph-edge--crossing");
    }
    const visible = createSvg("path", {
      d: path,
      class: classes.join(" "),
      "aria-hidden": "true",
    });
    layer.appendChild(hit);
    layer.appendChild(visible);
  }

  function desktopLayout(nodes, width) {
    const margin = 12;
    const gap = 12;
    const laneWidth = (width - margin * 2 - gap * 2) / 3;
    const nodeWidth = laneWidth - 18;
    const headerHeight = 64;
    const rowHeight = 78;
    const grouped = groupNodes(nodes);
    const maximum = Math.max(grouped.changed.length, grouped.impacted.length, grouped.test.length, 1);
    const height = Math.max(500, headerHeight + maximum * rowHeight + 28);
    const positions = new Map();
    const bands = [];
    const types = ["changed", "impacted", "test"];
    types.forEach(function (type, laneIndex) {
      const x = margin + laneIndex * (laneWidth + gap);
      bands.push({
        type: type,
        x: x,
        y: 12,
        width: laneWidth,
        height: height - 24,
        count: grouped[type].length,
        title: laneTitle(type),
        index: "0" + (laneIndex + 1),
      });
      grouped[type].forEach(function (node, index) {
        positions.set(node.id, {
          x: x + 9,
          y: headerHeight + index * rowHeight,
          width: nodeWidth,
          height: NODE_HEIGHT,
        });
      });
    });
    const axisLines = [];
    for (let y = headerHeight + rowHeight; y < height - 20; y += rowHeight) {
      axisLines.push({ x1: margin, y1: y - 8, x2: width - margin, y2: y - 8 });
    }
    return {
      width: width,
      height: height,
      positions: positions,
      bands: bands,
      axisLines: axisLines,
      textWidth: Math.max(14, Math.floor((nodeWidth - 30) / 7)),
    };
  }

  function mobileLayout(nodes, width) {
    const grouped = groupNodes(nodes);
    const positions = new Map();
    const bands = [];
    const margin = 10;
    const bandWidth = width - margin * 2;
    const nodeWidth = bandWidth - 18;
    const rowHeight = 82;
    let cursor = 10;
    ["changed", "impacted", "test"].forEach(function (type, laneIndex) {
      const group = grouped[type];
      const bandHeight = Math.max(82, 55 + group.length * rowHeight);
      bands.push({
        type: type,
        x: margin,
        y: cursor,
        width: bandWidth,
        height: bandHeight,
        count: group.length,
        title: laneTitle(type),
        index: "0" + (laneIndex + 1),
      });
      group.forEach(function (node, index) {
        positions.set(node.id, {
          x: margin + 9,
          y: cursor + 48 + index * rowHeight,
          width: nodeWidth,
          height: MOBILE_NODE_HEIGHT,
        });
      });
      cursor += bandHeight + 12;
    });
    return {
      width: width,
      height: cursor,
      positions: positions,
      bands: bands,
      axisLines: [],
      textWidth: Math.max(18, Math.floor((nodeWidth - 30) / 7)),
    };
  }

  function groupNodes(nodes) {
    return {
      changed: nodes.filter(function (node) { return node.type === "changed"; }),
      impacted: nodes.filter(function (node) { return node.type === "impacted"; }),
      test: nodes.filter(function (node) { return node.type === "test"; }),
    };
  }

  function laneTitle(type) {
    if (type === "changed") {
      return "Observed change";
    }
    if (type === "impacted") {
      return "Downstream reach";
    }
    return "Test evidence";
  }

  function drawBand(layer, band) {
    layer.appendChild(createSvg("rect", {
      x: band.x,
      y: band.y,
      width: band.width,
      height: band.height,
      class: "graph-lane-band graph-lane-band--" + band.type,
    }));
    layer.appendChild(createSvg("text", {
      x: band.x + 11,
      y: band.y + 17,
      class: "graph-lane-index",
    }, band.index + " / " + band.type));
    layer.appendChild(createSvg("text", {
      x: band.x + 11,
      y: band.y + 39,
      class: "graph-lane-title",
    }, band.title));
    layer.appendChild(createSvg("text", {
      x: band.x + band.width - 11,
      y: band.y + 18,
      class: "graph-lane-count",
      "text-anchor": "end",
    }, band.countLabel || (band.count + " displayed")));
    if (band.count === 0) {
      layer.appendChild(createSvg("text", {
        x: band.x + 11,
        y: band.y + 65,
        class: "graph-axis-label",
      }, "No claims in current aperture"));
    }
  }

  function edgePath(from, to, mobile, width, type) {
    if (!mobile) {
      const startX = from.x + from.width;
      const startY = from.y + from.height / 2;
      const endX = to.x;
      const endY = to.y + to.height / 2;
      const distance = Math.max(30, endX - startX);
      return "M " + startX + " " + startY
        + " C " + (startX + distance * 0.43) + " " + startY
        + ", " + (endX - distance * 0.43) + " " + endY
        + ", " + endX + " " + endY;
    }
    const useRightRail = type === "impact";
    const startX = useRightRail ? from.x + from.width : from.x;
    const endX = useRightRail ? to.x + to.width : to.x;
    const startY = from.y + from.height / 2;
    const endY = to.y + to.height / 2;
    const rail = useRightRail ? width - 4 : 4;
    return "M " + startX + " " + startY
      + " C " + rail + " " + startY
      + ", " + rail + " " + endY
      + ", " + endX + " " + endY;
  }

  function edgeLabel(edge) {
    const relation = edge.type === "test" ? "Test evidence" : "Impact evidence";
    const boundary = edge.crossing ? ", boundary crossing" : "";
    const ownership = overlayEnabled("ownership") && edge.ownershipBoundary ? ", ownership boundary" : "";
    const runtime = overlayEnabled("runtime") && edge.runtimeEvidence.length > 0 ? ", runtime trace corroborated" : "";
    const semantic = overlayEnabled("semantic") && edge.semanticEvidence.length > 0 ? ", SCIP compiler-resolved, high confidence" : ", Tree-sitter syntactic, medium confidence";
    return relation + " from " + text(edge.from.symbol.name, "unnamed seed")
      + " to " + text(edge.to.symbol.name, "unnamed claim") + boundary + ownership + semantic + runtime + ". Select for details.";
  }

  function drawEdge(layer, edge, from, to, mobile, width) {
    const path = edgePath(from, to, mobile, width, edge.type);
    const hit = createSvg("path", {
      d: path,
      class: "graph-edge-hit",
      tabindex: "0",
      focusable: "true",
      role: "button",
      "aria-label": edgeLabel(edge),
      "data-edge-id": edge.id,
    });
    const classes = ["graph-edge", "graph-edge--" + edge.type];
    if (edge.crossing) {
      classes.push("graph-edge--crossing");
    }
    if (overlayEnabled("ownership") && edge.ownershipBoundary) {
      classes.push("graph-edge--ownership");
    }
    if (overlayEnabled("runtime") && edge.runtimeEvidence.length > 0) {
      classes.push("graph-edge--runtime");
    }
    if (overlayEnabled("semantic") && edge.semanticEvidence.length > 0) {
      classes.push("graph-edge--semantic");
    }
    if (state.selectedId === edge.id) {
      classes.push("is-selected");
    }
    const visible = createSvg("path", {
      d: path,
      class: classes.join(" "),
      "data-claim-id": edge.id,
      "aria-hidden": "true",
    });
    const title = createSvg("title", {}, edgeLabel(edge));
    hit.appendChild(title);
    activateSvgClaim(hit, edge);
    layer.appendChild(hit);
    layer.appendChild(visible);
  }

  function nodeMetadata(node) {
    if (node.type === "changed") {
      return text(node.item.change, "change not classified");
    }
    if (node.type === "impacted") {
      const depth = finiteNumber(node.item.minimum_depth);
      return depth === null ? "depth unknown" : "depth " + depth;
    }
    const confidence = text(node.item.confidence, "confidence unknown");
    const classification = text(node.item.classification, "unclassified");
    return classification + " · " + confidence;
  }

  function nodeLabel(node) {
    const line = finiteNumber(node.symbol.line);
    const location = text(node.symbol.file, "file unavailable") + (line === null ? "" : ":" + line);
    const signals = evidenceSignals(node);
    return node.type + " symbol " + text(node.symbol.name, "unnamed") + ", " + location + ", component " + node.component.name + (signals.length > 0 ? ", evidence: " + signals.join(", ") : "") + ". Select for evidence.";
  }

  function drawNode(layer, node, position, textWidth) {
    const classes = ["graph-node", "graph-node--" + node.type];
    if (state.scope !== "all" && state.scope !== node.type && !state.selectedId) {
      classes.push("is-scope-muted");
    }
    if (node.crossings.length > 0) {
      classes.push("graph-node--crossing");
    }
    const visibleFindings = node.evidence && overlayEnabled("findings") ? node.evidence.findings : [];
    if (visibleFindings.length > 0) {
      classes.push("graph-node--finding");
    }
    if (node.evidence && node.evidence.coverage && overlayEnabled("coverage")) {
      const found = finiteNumber(node.evidence.coverage.lines_found);
      const hit = finiteNumber(node.evidence.coverage.lines_hit);
      if (found !== null && found > 0 && hit !== null && hit < found) {
        classes.push("graph-node--coverage-gap");
      }
    }
    if (state.selectedId === node.id) {
      classes.push("is-selected");
    }
    const group = createSvg("g", {
      class: classes.join(" "),
      transform: "translate(" + position.x + " " + position.y + ")",
      tabindex: "0",
      focusable: "true",
      role: "button",
      "aria-label": nodeLabel(node),
      "data-node-id": node.id,
      "data-claim-id": node.id,
    });
    group.appendChild(createSvg("title", {}, nodeLabel(node)));
    group.appendChild(createSvg("rect", {
      x: -4,
      y: -4,
      width: position.width + 8,
      height: position.height + 8,
      class: "graph-node__focus",
    }));
    group.appendChild(createSvg("rect", {
      x: 0,
      y: 0,
      width: position.width,
      height: position.height,
      class: "graph-node__surface",
    }));
    group.appendChild(createSvg("rect", {
      x: 0,
      y: 0,
      width: 5,
      height: position.height,
      class: "graph-node__accent",
    }));
    group.appendChild(createSvg("text", {
      x: 14,
      y: 15,
      class: "graph-node__kind",
    }, compact(text(node.symbol.kind, "unknown kind") + " / " + node.type, textWidth)));
    if (node.crossings.length > 0) {
      group.appendChild(createSvg("text", {
        x: position.width - 11,
        y: 15,
        class: "graph-node__flag",
        "text-anchor": "end",
      }, "CROSSING"));
    }
    const mark = evidenceMark(node);
    if (mark) {
      group.appendChild(createSvg("text", {
        x: 14,
        y: position.height - 6,
        class: "graph-node__evidence-mark",
      }, compact(mark, Math.max(8, Math.floor(textWidth * 0.38)))));
    }
    group.appendChild(createSvg("text", {
      x: 14,
      y: 31,
      class: "graph-node__name",
    }, compact(text(node.symbol.name, "unnamed symbol"), textWidth)));
    const line = finiteNumber(node.symbol.line);
    const path = text(node.symbol.file, "file unavailable") + (line === null ? "" : ":" + line);
    group.appendChild(createSvg("text", {
      x: 14,
      y: 45,
      class: "graph-node__file",
    }, compact(path, textWidth)));
    group.appendChild(createSvg("text", {
      x: position.width - 11,
      y: position.height - 6,
      class: "graph-node__meta",
      "text-anchor": "end",
    }, compact(nodeMetadata(node), Math.max(8, Math.floor(textWidth * 0.45)))));
    activateSvgClaim(group, node);
    layer.appendChild(group);
  }

  function activateSvgClaim(element, claim) {
    element.addEventListener("click", function () {
      selectClaim(claim);
    });
    element.addEventListener("keydown", function (event) {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectClaim(claim);
      }
    });
  }

  function selectClaim(claim) {
    state.selectedId = claim.id;
    state.disclosure = "selected";
    resetPages();
    if (claim.type === "changed" && claim.symbol) {
      state.focusedSeedId = claim.id;
    } else if (claim.from) {
      state.focusedSeedId = claim.from.id;
    } else {
      const incoming = state.model.edges.find(function (edge) { return edge.to.id === claim.id; });
      state.focusedSeedId = incoming ? incoming.from.id : null;
    }
    renderGraph();
    renderInspector();
    const selector = claim.symbol ? "[data-node-id]" : "[data-edge-id]";
    const selected = Array.from(elements.graph.querySelectorAll(selector)).find(function (element) {
      const attribute = claim.symbol ? "data-node-id" : "data-edge-id";
      return element.getAttribute(attribute) === claim.id;
    });
    if (selected) {
      selected.focus({ preventScroll: true });
    }
    const label = claim.symbol ? text(claim.symbol.name, "claim") : edgeLabel(claim);
    announce("Selected " + label + ". Evidence is available in the claim inspector.");
  }

  function showGraphState(title, message, action) {
    elements.graphFrame.classList.add("has-graph-state");
    elements.mobileTraceList.hidden = true;
    clearNode(elements.graphState);
    elements.graphState.appendChild(createElement("p", "graph-state__eyebrow", "Blast trace aperture"));
    elements.graphState.appendChild(createElement("h3", "", title));
    elements.graphState.appendChild(createElement("p", "", message));
    if (action) {
      const button = createElement("button", "graph-state__action", action.label);
      button.type = "button";
      button.addEventListener("click", action.handler);
      elements.graphState.appendChild(button);
    }
    elements.graphState.hidden = false;
  }

  function renderLoadingState() {
    clearNode(elements.graph);
    clearNode(elements.mobileTraceList);
    elements.mobileTraceList.hidden = true;
    setGraphVisible(true);
    elements.graphFrame.classList.add("has-graph-state");
    renderTraceContext("Loading snapshot", "Resolving the bounded claim aperture from the local repository.", []);
    clearNode(elements.graphState);
    const calibration = createElement("div", "loading-calibration");
    calibration.setAttribute("aria-hidden", "true");
    calibration.appendChild(document.createElement("span"));
    calibration.appendChild(document.createElement("span"));
    calibration.appendChild(document.createElement("span"));
    elements.graphState.appendChild(calibration);
    elements.graphState.appendChild(createElement("p", "graph-state__eyebrow", "Resolving repository evidence"));
    elements.graphState.appendChild(createElement("h3", "", "Calibrating blast trace"));
    elements.graphState.appendChild(createElement("p", "", "Comparing the requested baseline with the current working copy."));
    elements.graphState.hidden = false;
  }

  function renderInitialError(error) {
    clearNode(elements.graph);
    renderTraceContext("Snapshot error", "No repository claims are available until the local endpoint succeeds.", []);
    showGraphState(
      "Snapshot unavailable",
      error.message + " Error code: " + text(error.code, "unknown") + ".",
      { label: "Retry local scan", handler: function () { loadSnapshot(false); } }
    );
    elements.noticeStack.hidden = true;
    elements.traceCount.textContent = "No snapshot loaded";
    elements.instrumentSummary.textContent = "The local endpoint did not return review evidence.";
    elements.schema.textContent = "Schema —";
    setEnabled(false);
  }

  function selectedClaim() {
    if (!state.model || !state.selectedId) {
      return null;
    }
    return state.model.nodes.find(function (node) { return node.id === state.selectedId; })
      || state.model.edges.find(function (edge) { return edge.id === state.selectedId; })
      || null;
  }

  function renderInspector() {
    clearNode(elements.inspector);
    const claim = selectedClaim();
    if (!claim) {
      const empty = createElement("div", "inspector-empty");
      empty.appendChild(createElement("span", "inspector-empty__crosshair", "+"));
      empty.appendChild(createElement("h3", "", "Select a trace claim"));
      empty.appendChild(createElement("p", "", "Choose a symbol or evidence line to inspect its source, depth, precision, and seeds."));
      elements.inspector.appendChild(empty);
      return;
    }
    if (claim.symbol) {
      renderNodeInspector(claim);
    } else {
      renderEdgeInspector(claim);
    }
  }

  function appendClaimHeading(type, name, file, line, variant) {
    const heading = createElement("section", "claim-heading");
    const typeClass = variant ? " claim-heading__type--" + variant : "";
    heading.appendChild(createElement("p", "claim-heading__type" + typeClass, type));
    heading.appendChild(createElement("h3", "", name));
    const location = createElement("p", "claim-heading__path");
    location.appendChild(document.createTextNode(file));
    if (line !== null) {
      const marker = createElement("span", "", " · line " + line);
      location.appendChild(marker);
    }
    heading.appendChild(location);
    elements.inspector.appendChild(heading);
  }

  function appendClaimGrid(rows) {
    const list = createElement("dl", "claim-grid");
    rows.forEach(function (row) {
      if (row[1] === null || row[1] === undefined || row[1] === "") {
        return;
      }
      const wrapper = document.createElement("div");
      wrapper.appendChild(createElement("dt", "", row[0]));
      wrapper.appendChild(createElement("dd", "", String(row[1])));
      list.appendChild(wrapper);
    });
    elements.inspector.appendChild(list);
  }

  function appendClaimList(title, values, variant, emptyMessage) {
    const section = createElement("section", "claim-section");
    section.appendChild(createElement("h4", "", title));
    const className = variant ? "claim-list claim-list--" + variant : "claim-list";
    const list = createElement("ul", className);
    if (values.length === 0) {
      list.appendChild(createElement("li", "", emptyMessage));
    } else {
      values.forEach(function (value) {
        list.appendChild(createElement("li", "", value));
      });
    }
    section.appendChild(list);
    elements.inspector.appendChild(section);
  }

  function renderNodeInspector(node) {
    const line = finiteNumber(node.symbol.line);
    const variant = node.crossings.length > 0 ? "risk" : (node.type === "test" ? "test" : "");
    appendClaimHeading(
      node.type === "changed" ? "Observed change" : (node.type === "impacted" ? "Impacted symbol" : "Candidate test"),
      text(node.symbol.name, "Unnamed symbol"),
      text(node.symbol.file, "File unavailable"),
      line,
      variant
    );

    const rows = [
      ["Kind", text(node.symbol.kind, "Unknown")],
      ["Change", node.type === "changed" ? text(node.item.change, "Not classified") : null],
      ["Classification", node.type === "test" ? text(node.item.classification, "Not classified") : null],
      ["Minimum depth", node.type === "changed" ? null : displayNumber(finiteNumber(node.item.minimum_depth))],
      ["Confidence", node.type === "test" ? text(node.item.confidence, "Not returned") : null],
      ["Component", node.component.name],
      ["Component basis", node.component.basis],
      ["Name collisions", node.type === "impacted" ? displayNumber(finiteNumber(node.item.name_collision_count)) : null],
      ["Edge precision", node.type === "impacted" ? array(node.item.edge_precision).map(function (value) { return text(value, ""); }).filter(Boolean).join(", ") || "Not returned" : null],
      ["Boundary", node.crossings.length > 0 ? node.crossings.length + " API crossing" + (node.crossings.length === 1 ? "" : "s") : "None returned"],
    ];
    appendClaimGrid(rows);

    if (node.type === "changed") {
      const reachable = state.model.edges.filter(function (edge) { return edge.from.id === node.id; });
      const reachableLabels = reachable.slice(0, 20).map(function (edge) {
        return (edge.type === "test" ? "TEST" : "IMPACT") + " · " + text(edge.to.symbol.name, "unnamed") + " · " + text(edge.to.symbol.file, "file unavailable");
      });
      if (reachable.length > reachableLabels.length) {
        reachableLabels.push("… " + (reachable.length - reachableLabels.length) + " additional returned evidence links are available through the trace pager.");
      }
      appendClaimList(
        "Reachable returned evidence",
        reachableLabels,
        "",
        "No returned impact or test evidence names this changed symbol as a seed."
      );
      elements.inspector.appendChild(createElement("p", "claim-note", "Selecting this change highlights only graph claims that explicitly reference it as a seed."));
    } else if (node.type === "impacted") {
      const seeds = array(node.item.seeds).map(function (value) {
        const seed = record(value);
        const seedLine = finiteNumber(seed.line);
        return text(seed.name, "unnamed seed") + " · " + text(seed.file, "file unavailable") + (seedLine === null ? "" : ":" + seedLine);
      });
      appendClaimList("Explicit seeds", seeds, "", "No seeds were returned for this impacted symbol.");
    } else {
      const evidence = array(node.item.evidence).map(function (value) {
        const item = record(value);
        const component = text(item.component, "");
        const kind = text(item.kind, "evidence");
        const seed = record(item.seed);
        const hasGraphSeed = isRecord(item.seed)
          && (text(seed.name, "") !== "" || text(seed.file, "") !== "");
        if (!hasGraphSeed) {
          return kind + " evidence · no graph seed returned" + (component ? " · component " + component : "");
        }
        return kind + " evidence · " + text(seed.name, "unnamed seed") + " · " + text(seed.file, "file unavailable") + (component ? " · component " + component : "");
      });
      appendClaimList("Test evidence", evidence, "test", "No evidence entries were returned for this test candidate.");
      elements.inspector.appendChild(createElement("p", "claim-note", "Focused tests are candidates only and do not replace the repository's full verification gate."));
    }

    if (node.crossings.length > 0) {
      appendClaimList(
        "Boundary evidence",
        node.crossings.map(function (value) {
          const crossing = record(value);
          return text(crossing.changed_component, "unknown source") + " → " + text(crossing.impacted_component, "unknown target") + " · depth " + displayNumber(finiteNumber(crossing.minimum_depth));
        }),
        "risk",
        "No boundary evidence returned."
      );
    }
    appendOverlayEvidence(node);
  }

  function appendOverlayEvidence(node) {
    const evidence = node.evidence;
    if (!evidence) {
      return;
    }
    if (overlayEnabled("findings")) {
      const findings = evidence.findings.map(function (value) {
        const finding = record(value);
        const line = finiteNumber(finding.line);
        const column = finiteNumber(finding.column);
        const location = line === null ? "" : ":" + line + (column === null ? "" : ":" + column);
        return text(finding.level, "warning").toUpperCase() + " · " + text(finding.tool, "SARIF") + " / " + text(finding.rule_id, "unclassified") + location + " · " + text(finding.message, "No message returned");
      });
      if (findings.length > 0) {
        appendClaimList("SARIF findings · file-level", findings, "risk", "No matching findings returned.");
      }
    }
    if (overlayEnabled("coverage") && evidence.coverage) {
      const sourceIds = array(evidence.coverage.source_ids)
        .map(function (value) { return text(value, ""); })
        .filter(Boolean);
      appendClaimList(
        "Coverage · file-level",
        [coverageLabel(evidence.coverage) + (sourceIds.length > 0 ? " · sources " + sourceIds.join(", ") : "")],
        "coverage",
        "No coverage facts returned."
      );
    }
    if (overlayEnabled("ownership") && evidence.ownership) {
      const ownership = [];
      const owners = ownerNames(evidence);
      if (hasCodeownersEvidence(evidence)) {
        ownership.push("CODEOWNERS · " + (owners.length > 0 ? owners.join(", ") : "explicitly unowned"));
      }
      array(evidence.ownership.contributors).forEach(function (value) {
        const contributor = record(value);
        ownership.push("Git contributor · " + text(contributor.name, "Unknown") + " · " + displayNumber(finiteNumber(contributor.commits)) + " commits");
      });
      if (ownership.length > 0) {
        appendClaimList("Ownership · file-level", ownership, "ownership", "No ownership facts returned.");
      }
    }
    if (overlayEnabled("churn") && evidence.churn) {
      appendClaimList(
        "Recent churn · file-level",
        [displayNumber(finiteNumber(evidence.churn.commits)) + " commits · +" + displayNumber(finiteNumber(evidence.churn.lines_added)) + " / −" + displayNumber(finiteNumber(evidence.churn.lines_deleted)) + " lines in the configured history window"],
        "churn",
        "No churn facts returned."
      );
    }
    if (overlayEnabled("tests") && evidence.testResults) {
      const tests = evidence.testResults;
      const sourceIds = array(tests.source_ids).map(function (value) { return text(value, ""); }).filter(Boolean);
      const summary = displayNumber(finiteNumber(tests.total)) + " total · "
        + displayNumber(finiteNumber(tests.passed)) + " passed · "
        + displayNumber(finiteNumber(tests.failed)) + " failed · "
        + displayNumber(finiteNumber(tests.errors)) + " errors · "
        + displayNumber(finiteNumber(tests.skipped)) + " skipped"
        + (sourceIds.length > 0 ? " · sources " + sourceIds.join(", ") : "")
        + (tests.failures_truncated === true ? " · failure details truncated" : "");
      const failures = array(tests.failures).map(function (value) {
        const failure = record(value);
        const className = text(failure.class_name, "");
        return text(failure.name, "Unnamed test") + " · " + text(failure.status, "failed")
          + (className ? " · " + className : "") + " · " + text(failure.message, "No failure detail returned");
      });
      appendClaimList(
        "JUnit · file-level",
        [summary].concat(failures),
        failures.length > 0 ? "risk" : "test",
        "No JUnit facts returned."
      );
    }
    if (overlayEnabled("runtime") && evidence.runtime) {
      appendClaimList(
        "Runtime spans · file-level",
        [displayNumber(finiteNumber(evidence.runtime.spans)) + " spans · "
          + displayNumber(finiteNumber(evidence.runtime.traces)) + " traces · sources "
          + array(evidence.runtime.source_ids).map(function (value) { return text(value, ""); }).filter(Boolean).join(", ")],
        "runtime",
        "No runtime facts returned."
      );
    }
    if (overlayEnabled("knowledge") && evidence.knowledge.length > 0) {
      appendClaimList(
        "Project knowledge · exact path",
        evidence.knowledge.map(function (value) {
          const item = record(value);
          return text(item.kind, "project record") + " · " + text(item.title, "Untitled")
            + " · " + text(item.artifact_path, "path unavailable") + " · " + text(item.excerpt, "No excerpt returned");
        }),
        "knowledge",
        "No exact project-knowledge matches returned."
      );
    }
  }

  function renderEdgeInspector(edge) {
    const isTest = edge.type === "test";
    const variant = edge.crossing ? "risk" : (isTest ? "test" : "");
    appendClaimHeading(
      edge.crossing ? "Boundary evidence line" : (isTest ? "Test evidence line" : "Impact evidence line"),
      text(edge.from.symbol.name, "Unnamed seed") + " → " + text(edge.to.symbol.name, "Unnamed target"),
      text(edge.to.symbol.file, "File unavailable"),
      finiteNumber(edge.to.symbol.line),
      variant
    );
    appendClaimGrid([
      ["Relation", isTest ? "Changed seed → candidate test" : "Changed seed → impacted symbol"],
      ["Minimum depth", displayNumber(edge.minimumDepth)],
      ["Precision", overlayEnabled("semantic") && edge.semanticEvidence.length > 0 ? "high" : (edge.precision.join(", ") || "medium")],
      ["Static provenance", overlayEnabled("semantic") && edge.semanticEvidence.length > 0 ? "SCIP (preferred)" : "Tree-sitter (fallback)"],
      ["Evidence kind", edge.evidence ? text(edge.evidence.kind, "Not classified") : "Impact seed"],
      ["Name collisions", displayNumber(edge.collisionCount)],
      ["Boundary crossing", edge.crossing ? "Observed" : "Not returned"],
      ["Ownership boundary", overlayEnabled("ownership") && edge.ownershipBoundary ? "Observed from CODEOWNERS" : "Not returned"],
      ["Runtime trace", overlayEnabled("runtime") && edge.runtimeEvidence.length > 0 ? "Corroborated" : "Not returned"],
    ]);
    appendClaimList("Evidence endpoints", [
      "FROM · " + text(edge.from.symbol.name, "unnamed") + " · " + text(edge.from.symbol.file, "file unavailable") + formatLine(edge.from.symbol.line),
      "TO · " + text(edge.to.symbol.name, "unnamed") + " · " + text(edge.to.symbol.file, "file unavailable") + formatLine(edge.to.symbol.line),
    ], isTest ? "test" : "", "No endpoints available.");
    if (edge.crossing) {
      appendClaimList("Component boundary", [
        text(record(edge.crossing).changed_component, "unknown source") + " → " + text(record(edge.crossing).impacted_component, "unknown target"),
      ], "risk", "No component names returned.");
    }
    if (overlayEnabled("ownership") && edge.ownershipBoundary) {
      appendClaimList(
        "Ownership boundary",
        [
          "FROM · " + ownershipEndpointLabel(edge.from.evidence),
          "TO · " + ownershipEndpointLabel(edge.to.evidence),
        ],
        "ownership",
        "No CODEOWNERS evidence returned."
      );
    }
    if (overlayEnabled("runtime") && edge.runtimeEvidence.length > 0) {
      appendClaimList(
        "Runtime trace corroboration",
        edge.runtimeEvidence.map(function (value) {
          const runtime = record(value);
          const names = array(runtime.span_names).map(function (name) { return text(name, ""); }).filter(Boolean);
          return text(runtime.parent_file, "file unavailable") + " → " + text(runtime.child_file, "file unavailable")
            + " · " + displayNumber(finiteNumber(runtime.spans)) + " spans · "
            + displayNumber(finiteNumber(runtime.traces)) + " traces"
            + (names.length > 0 ? " · " + names.join(", ") : "")
            + (runtime.names_truncated === true ? " · span names truncated" : "")
            + (array(runtime.source_ids).length > 0 ? " · sources " + array(runtime.source_ids).map(function (value) { return text(value, ""); }).filter(Boolean).join(", ") : "");
        }),
        "runtime",
        "No matching runtime evidence returned."
      );
    }
    if (overlayEnabled("semantic") && edge.semanticEvidence.length > 0) {
      appendClaimList(
        "Compiler-resolved semantic evidence",
        edge.semanticEvidence.map(function (value) {
          const semantic = record(value);
          return text(semantic.kind, "reference") + " · "
            + text(semantic.from_display_name, "file scope") + " · "
            + text(semantic.from_file, "file unavailable") + formatLine(semantic.from_line)
            + " → " + text(semantic.to_display_name, "symbol unavailable") + " · "
            + text(semantic.to_file, "external symbol") + formatLine(semantic.to_line)
            + " · reference at " + text(semantic.from_file, "file unavailable") + formatLine(semantic.occurrence_line)
            + " · SCIP / high";
        }),
        "semantic",
        "No matching SCIP edge returned."
      );
    }
    elements.inspector.appendChild(createElement(
      "p",
      "claim-note",
      isTest
        ? "This line exists only because the test evidence explicitly names the changed symbol as its seed."
        : (overlayEnabled("semantic") && edge.semanticEvidence.length > 0
          ? "SCIP is the preferred static provenance for this exact endpoint and symbol pair; Tree-sitter remains the fallback topology."
          : "This line exists only because the impacted symbol explicitly names the changed symbol in its seeds array.")
    ));
  }

  function formatLine(value) {
    const line = finiteNumber(value);
    return line === null ? "" : ":" + line;
  }

  function snapshotAnnouncement() {
    const model = state.model;
    return "Lens snapshot loaded. "
      + totalOrReturned(model.changedSymbols) + " changed symbols, "
      + totalOrReturned(model.impactedSymbols) + " impacted symbols, and "
      + totalOrReturned(model.tests) + " candidate tests. "
      + returnedCount(model.evidenceSources) + " evidence sources were evaluated. "
      + (model.truncations.length > 0 ? "The result is partial." : "No truncation was reported.");
  }

  function announceVisibleClaims() {
    if (!state.model) {
      return;
    }
    const count = apertureNodes().length;
    const scope = state.scope === "all" ? "All claim types are emphasized." : laneTitle(state.scope) + " is emphasized; connected evidence remains available.";
    announce(count + " of " + state.model.nodes.length + " returned claims are in the aperture. " + scope);
  }

  function updateSnapshotAge() {
    if (!state.fetchedAt) {
      elements.snapshotAge.textContent = state.error ? "No snapshot loaded" : "Awaiting first scan";
      return;
    }
    const elapsed = Math.max(0, Date.now() - state.fetchedAt.getTime());
    let label;
    if (elapsed < 60000) {
      label = "Snapshot just now";
    } else if (elapsed < 3600000) {
      const minutes = Math.floor(elapsed / 60000);
      label = "Snapshot " + minutes + "m ago";
    } else {
      const hours = Math.floor(elapsed / 3600000);
      label = "Snapshot " + hours + "h ago";
    }
    if (state.stale) {
      label += " · stale";
    }
    elements.snapshotAge.textContent = label;
  }

  function boot() {
    initializeElements();
    bindEvents();
    renderLoadingState();
    window.setInterval(updateSnapshotAge, 30000);
    loadSnapshot(true);
  }

  boot();
}());
