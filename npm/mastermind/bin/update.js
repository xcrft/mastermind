#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { PROFILE_NAMES, artifactDigest, bundled, profileBundle, readManifest } from "./install.js";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const CLIENTS = ["claude", "codex"];
const MANIFEST = ".mastermind-workflow.json";
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const PACKAGE = /^(?:@[a-z0-9_.-]+\/)?[a-z0-9][a-z0-9_.-]*$/;
const CHILD = "import { pathToFileURL } from 'node:url'; const { main } = await import(pathToFileURL(process.argv[2]).href); process.exit(await main(JSON.parse(process.argv[3]), JSON.parse(process.argv[4])));";

function parseArgs(argv) {
  const values = argv[0] === "update" ? argv.slice(1) : argv;
  const result = { client: null, profile: null, dryRun: false, workflowOnly: false, json: false };
  const seen = new Set();
  for (let index = 0; index < values.length; index += 1) {
    const [flag, ...inline] = values[index].split("=");
    if (seen.has(flag)) throw new Error(`duplicate update argument: ${flag}`);
    seen.add(flag);
    if (flag === "--client" || flag === "--profile") {
      const value = inline.length ? inline.join("=") : values[++index];
      const choices = flag === "--client" ? [...CLIENTS, "all"] : PROFILE_NAMES;
      if (!choices.includes(value)) throw new Error(`${flag} requires ${choices.join(", ")}`);
      result[flag.slice(2)] = value;
    } else {
      const key = { "--dry-run": "dryRun", "--workflow-only": "workflowOnly", "--json": "json" }[flag];
      if (!key || inline.length) throw new Error(`unknown update argument: ${values[index]}`);
      result[key] = true;
    }
  }
  return result;
}

function entry(target) {
  try { return fs.lstatSync(target); }
  catch (error) { if (error.code === "ENOENT") return null; throw error; }
}

function regular(target, label, limit = 1024 * 1024) {
  const stat = entry(target);
  if (!stat?.isFile() || stat.isSymbolicLink() || stat.size > limit) {
    throw new Error(`${label} must be a bounded regular file: ${target}`);
  }
  return fs.readFileSync(target, "utf8");
}

function directory(target, label, required = false) {
  const stat = entry(target);
  if (!stat && !required) return false;
  if (!stat?.isDirectory() || stat.isSymbolicLink()) throw new Error(`${label} must be a regular directory: ${target}`);
  return true;
}

function metadata(root, expectedName, expectedVersion = null) {
  directory(root, "package root", true);
  const value = JSON.parse(regular(path.join(root, "package.json"), "package metadata"));
  if (!PACKAGE.test(value.name) || !VERSION.test(value.version)
      || (expectedName && value.name !== expectedName)
      || (expectedVersion && value.version !== expectedVersion)) {
    throw new Error("package identity or version does not match the running updater");
  }
  return value;
}

function treeDigest(target, budget, depth = 0) {
  const stat = entry(target);
  if (!stat || stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) {
    throw new Error(`workflow artifact is unavailable or linked: ${target}`);
  }
  budget.entries += 1;
  budget.bytes += stat.isFile() ? stat.size : 0;
  if (depth > 32 || budget.entries > 8192 || budget.bytes > 64 * 1024 * 1024) {
    throw new Error("workflow preflight exceeds its file or byte limit");
  }
  if (stat.isDirectory()) {
    for (const child of fs.readdirSync(target)) treeDigest(path.join(target, child), budget, depth + 1);
  }
  return depth === 0 ? artifactDigest(target) : null;
}

function selectedClients(home, client) {
  if (client) return client === "all" ? CLIENTS : [client];
  return CLIENTS.filter((name) => {
    const root = path.join(home, `.${name}`);
    if (!directory(root, "client root")) return false;
    const manifest = path.join(root, MANIFEST);
    if (!entry(manifest)) return false;
    regular(manifest, "ownership manifest");
    const value = readManifest(manifest);
    if (value.client !== name) throw new Error(`ownership manifest client mismatch: ${manifest}`);
    return true;
  });
}

// Preflight preserves edited owned files and collisions with unowned names.
// Repeat it in the fresh package process after npm changes the bundle.
function workflowPlan(home, packageRoot, names, profile) {
  if (!names.length) return [];
  const share = path.join(packageRoot, "share");
  const budget = { entries: 0, bytes: 0 };
  treeDigest(share, budget);
  const bundle = bundled(share);
  return names.map((client) => {
    const root = path.join(home, `.${client}`);
    directory(root, "client root");
    const manifestPath = path.join(root, MANIFEST);
    if (entry(manifestPath)) regular(manifestPath, "ownership manifest");
    const previous = readManifest(manifestPath);
    if (previous && previous.client !== client) throw new Error(`ownership manifest client mismatch: ${manifestPath}`);
    const selected = profile ?? (previous?.schema_version === 1 ? "full" : previous?.profile ?? "core");
    const desired = profileBundle(bundle, selected, share);
    for (const kind of ["agents", "skills"]) {
      if (kind === "agents" && client === "codex") continue;
      directory(path.join(root, kind), "workflow artifact directory");
      const owned = previous?.artifacts[kind] ?? [];
      for (const name of owned) {
        const installed = path.join(root, kind, name);
        if (entry(installed) && treeDigest(installed, budget) !== previous.digests[`${kind}/${name}`]) {
          throw new Error(`edited owned workflow artifact, preserve or restore it before updating: ${installed}`);
        }
      }
      for (const name of desired[kind === "agents" ? "subagents" : "skills"]) {
        const installed = path.join(root, kind, name);
        if (!owned.includes(name) && entry(installed)) {
          throw new Error(`unowned workflow artifact conflicts with the new bundle: ${installed}`);
        }
      }
    }
    return { client, profile: selected, installed_version: previous?.version ?? null, root };
  });
}

function scopeFor(packageRoot, installMode, packageName) {
  const parts = packageName.split("/");
  const modules = path.resolve(packageRoot, ...parts.map(() => ".."));
  if (path.basename(modules) !== "node_modules" || path.join(modules, ...parts) !== packageRoot) {
    return { reason: "package layout does not establish an npm installation scope" };
  }
  if (installMode === "global") {
    const parent = path.dirname(modules);
    if (process.platform !== "win32" && path.basename(parent) !== "lib") {
      return { reason: "package layout does not establish the global npm prefix" };
    }
    const prefix = process.platform === "win32" ? parent : path.dirname(parent);
    return { kind: "global", prefix, modules, cwd: prefix,
      args: ["install", "--global", "--prefix", prefix, "--include=optional", "--no-audit", "--no-fund", `${packageName}@latest`] };
  }
  if (installMode !== "project") return { reason: `installation mode ${installMode} is not a persistent npm scope` };
  const prefix = path.dirname(modules);
  const projectPath = path.join(prefix, "package.json");
  if (!entry(projectPath)) return { reason: "the owning npm project has no package metadata" };
  const project = JSON.parse(regular(projectPath, "project package metadata"));
  if ((project.packageManager && !/^npm@/.test(project.packageManager))
      || entry(path.join(prefix, "pnpm-lock.yaml")) || entry(path.join(prefix, "yarn.lock"))) {
    return { reason: "the project declares another package manager" };
  }
  const kinds = ["dependencies", "devDependencies", "optionalDependencies"]
    .filter((kind) => Object.hasOwn(project[kind] ?? {}, packageName));
  if (kinds.length !== 1 || typeof project[kinds[0]][packageName] !== "string"
      || !project[kinds[0]][packageName].trim() || /[/:#\\]/.test(project[kinds[0]][packageName])) {
    return { reason: "the package has no unambiguous direct registry dependency in the owning project" };
  }
  const save = { dependencies: "--save-prod", devDependencies: "--save-dev", optionalDependencies: "--save-optional" }[kinds[0]];
  return { kind: "project", prefix, modules, cwd: prefix,
    args: ["install", "--prefix", prefix, save, "--include=optional", "--no-audit", "--no-fund", `${packageName}@latest`] };
}

function npmEntry(nodePath) {
  const candidates = [];
  if (process.env.npm_execpath && path.basename(process.env.npm_execpath) === "npm-cli.js") candidates.push(process.env.npm_execpath);
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    if (!dir) continue;
    const command = path.join(dir, process.platform === "win32" ? "npm.cmd" : "npm");
    if (!entry(command)) continue;
    let actual;
    try { actual = fs.realpathSync(command); }
    catch (error) { if (error.code === "ENOENT") continue; throw error; }
    if (path.basename(actual) === "npm-cli.js") candidates.push(actual);
    candidates.push(path.join(path.dirname(actual), "node_modules", "npm", "bin", "npm-cli.js"));
  }
  candidates.push(path.join(path.dirname(nodePath), "node_modules", "npm", "bin", "npm-cli.js"));
  candidates.push(path.resolve(path.dirname(nodePath), "..", "lib", "node_modules", "npm", "bin", "npm-cli.js"));
  const found = candidates.find((candidate) => path.isAbsolute(candidate) && entry(candidate)?.isFile());
  if (!found) throw new Error("npm CLI was not found. Run the displayed npm command with your package manager");
  return fs.realpathSync(found);
}

function child(nodePath, args, { cwd, timeout = 60_000 } = {}) {
  const result = spawnSync(nodePath, args, {
    cwd, env: process.env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    timeout, maxBuffer: 4 * 1024 * 1024, windowsHide: true, shell: false,
  });
  if (result.error || result.status !== 0) {
    const detail = (result.stderr || result.stdout || result.error?.message || `exit ${result.status}`)
      .replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, "").trim().slice(-2048);
    throw new Error(`child command failed${result.signal ? ` (${result.signal})` : ""}: ${detail}`);
  }
  return result.stdout;
}

function installWorkflows(context, flags, planned) {
  const names = planned.map((row) => row.client);
  if (!names.length) return { status: "not_installed", clients: [] };
  const clients = names.length === 2 ? "all" : names[0];
  const installer = path.join(context.packageRoot, "bin", "install.js");
  regular(installer, "fresh workflow installer");
  const args = [installer, "update", "--client", clients];
  if (flags.profile) args.push("--profile", flags.profile);
  child(context.nodePath, args, { cwd: context.packageRoot });
  const raw = child(context.nodePath, [installer, "doctor", "--workflow", "--client", clients, "--json"], { cwd: context.packageRoot });
  const result = JSON.parse(raw);
  if (result.schema_version !== 1 || !Array.isArray(result.clients)
      || result.clients.length !== names.length
      || result.clients.some((row, index) => row.client !== names[index]
        || row.profile !== planned[index].profile || row.version !== context.version
        || row.installed_version !== context.version || row.parity !== true)) {
    throw new Error("fresh workflow installer did not verify every selected integration");
  }
  return { status: "updated", clients: result.clients };
}

function display(report, json) {
  if (json) { console.log(JSON.stringify(report, null, 2)); return; }
  console.log(`Mastermind update: ${report.status}`);
  if (report.dry_run) console.log("Dry run. No update commands were executed.");
  console.log(`Package: ${report.package.status}${report.package.version_after ? ` (${report.package.version_after})` : ""}`);
  console.log(`Native binary: ${report.package.binary.status}`);
  if (report.package.command) console.log(`Package scope: ${report.package.install_mode} at ${report.package.command.cwd}`);
  console.log(`Workflows: ${report.workflows.status}`);
  for (const row of report.workflows.clients) console.log(`  ${row.client}: ${row.profile}`);
  if (report.reason) console.log(report.reason);
  for (const action of report.recovery) console.log(`Next: ${action}`);
}

export async function main(argv = process.argv.slice(2), options = {}) {
  let json = argv.includes("--json");
  const report = { schema_version: 1, kind: "update", status: "failed", dry_run: false,
    package: { status: "not_started", version_before: null, version_after: null, binary: { status: "not_checked" } },
    workflows: { status: "not_started", clients: [] }, recovery: [] };
  let packageAttempted = false;
  let workflowAttempted = false;
  try {
    const flags = parseArgs(argv);
    json = flags.json;
    report.dry_run = flags.dryRun;
    const suppliedRoot = path.resolve(options.packageRoot ?? path.join(HERE, ".."));
    directory(suppliedRoot, "package root", true);
    const packageRoot = fs.realpathSync(suppliedRoot);
    const pkg = metadata(packageRoot, options.packageName, options.version);
    const context = { packageRoot, installMode: options.installMode ?? "unknown",
      packageName: pkg.name, version: pkg.version, nodePath: options.nodePath ?? process.execPath };
    if (!path.isAbsolute(context.nodePath)) throw new Error("Node executable must be an absolute path");
    const home = process.env.MASTERMIND_WORKFLOW_HOME ?? os.homedir();
    if (!path.isAbsolute(home)) throw new Error("workflow home must be an absolute path");
    const names = selectedClients(home, flags.client);
    report.package = { ...report.package, name: pkg.name, install_mode: context.installMode, version_before: pkg.version };
    report.workflows.clients = workflowPlan(home, packageRoot, names, flags.profile);
    report.workflows.status = names.length ? "planned" : "not_installed";
    report.package.status = flags.workflowOnly ? "not_requested" : "planned";
    if (flags.workflowOnly) {
      if (flags.dryRun) report.status = "planned";
      else {
        workflowAttempted = names.length > 0;
        report.workflows = installWorkflows(context, flags, report.workflows.clients);
        report.status = "complete";
      }
    } else {
      const scope = scopeFor(packageRoot, context.installMode, pkg.name);
      if (scope.reason) {
        report.status = "manual_required";
        report.package.status = "manual_required";
        report.reason = scope.reason;
        report.recovery.push(context.installMode === "npx"
          ? `npx --yes ${pkg.name}@latest update --workflow-only`
          : `Update ${pkg.name} with the package manager that owns this installation, then run mastermind update --workflow-only`);
      } else {
        report.package.command = { executable: "npm", argv: scope.args, cwd: scope.cwd };
        report.package.scope_verification = "not_run";
        if (flags.dryRun) report.status = "planned";
        else {
          const npm = npmEntry(context.nodePath);
          const rootArgs = [npm, "root", "--prefix", scope.prefix];
          if (scope.kind === "global") rootArgs.push("--global");
          const actualRoot = child(context.nodePath, rootArgs, { cwd: scope.cwd }).trim();
          if (!path.isAbsolute(actualRoot) || fs.realpathSync(actualRoot) !== fs.realpathSync(scope.modules)) {
            throw new Error("npm reported a different installation scope. No update was started");
          }
          report.package.scope_verification = "matched";
          packageAttempted = true;
          report.package.status = "attempted";
          child(context.nodePath, [npm, ...scope.args], { cwd: scope.cwd, timeout: 15 * 60_000 });
          if (fs.realpathSync(packageRoot) !== packageRoot) throw new Error("package root changed during npm update");
          const updated = metadata(packageRoot, pkg.name);
          report.package.version_after = updated.version;
          report.package.status = updated.version === pkg.version ? "unchanged" : "updated";
          const wrapper = path.join(packageRoot, "bin", "mmcg.js");
          report.package.binary = { status: "failed", expected_version: updated.version };
          regular(wrapper, "fresh native binary wrapper");
          const nativeVersion = child(context.nodePath, [wrapper, "--version"], { cwd: packageRoot, timeout: 15_000 }).trim();
          if (!new RegExp(`^(?:mmcg|mastermind) ${updated.version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`).test(nativeVersion)) {
            throw new Error("native binary version does not match the updated package");
          }
          report.package.binary = { status: "verified", version: updated.version };
          if (names.length) {
            const updater = path.join(packageRoot, "bin", "update.js");
            regular(updater, "fresh update coordinator");
            const selected = names.length === 2 ? "all" : names[0];
            const nextArgs = ["update", "--workflow-only", "--client", selected, "--json"];
            if (flags.profile) nextArgs.push("--profile", flags.profile);
            const nextContext = { ...context, version: updated.version };
            workflowAttempted = true;
            const raw = child(context.nodePath, ["--input-type=module", "--eval", CHILD,
              "mastermind-update-bootstrap", updater, JSON.stringify(nextArgs), JSON.stringify(nextContext)], { cwd: packageRoot });
            const result = JSON.parse(raw);
            if (result.schema_version !== 1 || result.kind !== "update" || result.status !== "complete"
                || result.package.version_before !== updated.version || result.workflows.status !== "updated"
                || result.workflows.clients?.length !== names.length
                || result.workflows.clients.some((row, index) => row.client !== names[index]
                  || row.profile !== report.workflows.clients[index].profile || row.parity !== true)) {
              throw new Error("fresh update coordinator returned an incomplete workflow result");
            }
            report.workflows = result.workflows;
          }
          report.status = "complete";
        }
      }
    }
  } catch (error) {
    report.status = packageAttempted || workflowAttempted ? "partial" : "failed";
    report.reason = error instanceof Error ? error.message : String(error);
    if (report.package.status === "attempted") report.package.status = "unverified_after_attempt";
    if (report.workflows.status === "planned") report.workflows.status = workflowAttempted ? "unverified_after_attempt" : "not_updated";
    if (packageAttempted || workflowAttempted) {
      report.recovery.push("Inspect the reported package version and preserved workflow files, then run mastermind update --workflow-only");
    }
  }
  display(report, json);
  return ["complete", "planned"].includes(report.status) ? 0 : 1;
}

if (process.argv[1] && fs.existsSync(process.argv[1]) && fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) process.exit(await main());
