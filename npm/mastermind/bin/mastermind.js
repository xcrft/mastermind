#!/usr/bin/env node
// `mastermind` — public command. Same native binary as `mmcg`, but injects
// install-mode hints into the environment so `setup claude` writes the right
// MCP `command` form for each installation method (npx vs global npm vs
// project-local npm vs cargo).

import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { resolveBinary, runBinary } from "./resolve.js";
import { printHelp } from "./help.js";

if (printHelp(process.argv.slice(2))) process.exit(0);

const require = createRequire(import.meta.url);
const pkg = require("../package.json");
// Package root (…/npm/mastermind). Its bundled `share/` tree holds the workflow
// subagents + skills that `init` installs into ~/.claude/.
const pkgRoot = fs.realpathSync(path.dirname(require.resolve("../package.json")));
const installMode = detectInstallMode();

if (process.argv[2] === "update") {
  const updater = await import("./update.js");
  process.exit(await updater.main(process.argv.slice(2), {
    packageRoot: pkgRoot,
    installMode,
    packageName: pkg.name,
    version: pkg.version,
    nodePath: process.execPath,
  }));
}

// Workflow bundle management is handled in JS; no native binary is needed.
// `doctor --workflow` checks package↔installed manifest parity without touching
// the repository-local codegraph doctor.
if (
  ["install", "list"].includes(process.argv[2]) ||
  (process.argv[2] === "doctor" && process.argv.includes("--workflow"))
) {
  const installer = await import("./install.js");
  process.exit(await installer.main(process.argv.slice(2)));
}

/**
 * Use the resolved package layout, not the bin symlink or caller directory.
 * Update verifies the inferred npm module root again before writing.
 * Ambiguous owners remain unknown rather than becoming global installations.
 */
function detectInstallMode() {
  if ([pkgRoot, process.argv[1] ?? ""].some((value) => /(?:^|[\\/])_npx(?:[\\/]|$)/.test(value))) {
    return "npx";
  }
  const samePath = (left, right) => process.platform === "win32"
    ? left.toLowerCase() === right.toLowerCase() : left === right;
  const parts = pkg.name.split("/");
  const modules = path.resolve(pkgRoot, ...parts.map(() => ".."));
  if (!samePath(path.basename(modules), "node_modules")
      || !samePath(path.join(modules, ...parts), pkgRoot)) return "unknown";
  const owner = path.dirname(modules);
  const project = projectOwner(owner);
  if (project !== null) return project;

  if (process.platform !== "win32") return path.basename(owner) === "lib" ? "global" : "unknown";
  const prefixes = [
    process.env.npm_config_prefix,
    process.env.NPM_CONFIG_PREFIX,
    process.env.APPDATA && path.join(process.env.APPDATA, "npm"),
    path.dirname(process.execPath),
  ];
  for (const prefix of prefixes) {
    if (!prefix || !path.isAbsolute(prefix)) continue;
    try {
      if (samePath(fs.realpathSync(prefix), owner)) return "global";
    } catch {
      // Unavailable prefix evidence cannot establish an installation scope.
    }
  }
  return "unknown";
}

function projectOwner(owner) {
  const metadata = path.join(owner, "package.json");
  let stat;
  try { stat = fs.lstatSync(metadata); }
  catch (error) { return error.code === "ENOENT" ? null : "unknown"; }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) return "unknown";
  try {
    const project = JSON.parse(fs.readFileSync(metadata, "utf8"));
    if ((project.packageManager && !/^npm@/.test(project.packageManager))
        || fs.existsSync(path.join(owner, "pnpm-lock.yaml"))
        || fs.existsSync(path.join(owner, "yarn.lock"))) return "unknown";
    const kinds = ["dependencies", "devDependencies", "optionalDependencies"]
      .filter((kind) => Object.hasOwn(project[kind] ?? {}, pkg.name));
    if (kinds.length === 1) {
      const source = project[kinds[0]][pkg.name];
      if (typeof source === "string" && source.trim() && !/[/:#\\]/.test(source)) return "project";
    }
  } catch {
    // Invalid ownership metadata is handled by the explicit update workflow.
  }
  return "unknown";
}

const bin = resolveBinary();

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
  MASTERMIND_LAUNCHER_JS: path.join(pkgRoot, "bin", "mastermind.js"),
  MASTERMIND_NODE: process.execPath,
};

runBinary(bin, process.argv.slice(2), env);
