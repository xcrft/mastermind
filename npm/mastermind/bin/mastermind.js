#!/usr/bin/env node
// `mastermind` — public command. Same native binary as `mmcg`, but injects
// install-mode hints into the environment so `setup claude` writes the right
// MCP `command` form for each installation method (npx vs global npm vs
// project-local npm vs cargo).

import { createRequire } from "node:module";
import path from "node:path";
import process from "node:process";
import { resolveBinary, runBinary } from "./resolve.js";

const require = createRequire(import.meta.url);
const pkg = require("../package.json");
// Package root (…/npm/mastermind). Its bundled `share/` tree holds the workflow
// subagents + skills that `init` installs into ~/.claude/.
const pkgRoot = path.dirname(require.resolve("../package.json"));

// Workflow bundle management is handled in JS; no native binary is needed.
// `doctor --workflow` checks package↔installed manifest parity without touching
// the repository-local codegraph doctor.
if (
  ["install", "update", "list"].includes(process.argv[2]) ||
  (process.argv[2] === "doctor" && process.argv.includes("--workflow"))
) {
  const installer = await import("./install.js");
  process.exit(await installer.main(process.argv.slice(2)));
}

/**
 * Detect how this wrapper was invoked so `setup claude` can write a stable
 * MCP `command` form per install mode.
 *
 *   npx    — process.argv[1] lives under an npx cache (`/_npx/` or `\_npx\`)
 *   project — wrapper lives under a `node_modules` of cwd or any ancestor
 *             (monorepos hoist deps to the workspace root)
 *   global  — wrapper lives under a known npm global prefix
 *   cargo   — never reached here (cargo users invoke `mmcg` directly, no JS)
 *
 * Best-effort: if none match cleanly, returns "unknown" — the Rust side
 * falls back to `command: mastermind` which works for any PATH-installed case.
 */
function detectInstallMode() {
  const self = process.argv[1] || "";
  // npx caches keep packages under `_npx` directories on every platform.
  if (self.includes(`${path.sep}_npx${path.sep}`) || self.includes("/_npx/")) {
    return "npx";
  }
  // Project install: the wrapper resolves from a `node_modules` belonging to
  // cwd OR any ancestor. Monorepos hoist dependencies to the workspace root, so
  // a command run in `packages/foo` loads its bin from `<repo-root>/node_modules`
  // — still a project install, not global. Walk up the tree to catch that.
  let dir = process.cwd();
  for (;;) {
    const nm = path.join(dir, "node_modules") + path.sep;
    if (self.startsWith(nm)) {
      return "project";
    }
    const parent = path.dirname(dir);
    if (parent === dir) break; // reached the filesystem root
    dir = parent;
  }
  // Heuristic for global install: bin path lands under a directory whose
  // name suggests global npm (e.g. /usr/local/lib/node_modules, ~/.npm-global,
  // C:\\Program Files\\nodejs\\node_modules). Any node_modules path that
  // isn't under cwd qualifies.
  if (self.includes(`${path.sep}node_modules${path.sep}`)) {
    return "global";
  }
  return "unknown";
}

const bin = resolveBinary();
const installMode = detectInstallMode();

// Inject hints for `setup claude` and any other subcommand that benefits from
// knowing how it was invoked. The Rust side reads these env vars; absence is
// treated as "cargo-style install" and the existing behavior is preserved.
const env = {
  ...process.env,
  MASTERMIND_INSTALL_MODE: installMode,
  MASTERMIND_VERSION: pkg.version,
  MASTERMIND_PACKAGE: pkg.name,
  MASTERMIND_SHARE_DIR: path.join(pkgRoot, "share"),
  MASTERMIND_INSTALLER_JS: path.join(pkgRoot, "bin", "install.js"),
  MASTERMIND_NODE: process.execPath,
};

runBinary(bin, process.argv.slice(2), env);
