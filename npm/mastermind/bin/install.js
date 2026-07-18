#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_SHARE = path.join(HERE, "..", "share");
const PACKAGE_JSON = path.join(HERE, "..", "package.json");
const PKG = "@xcraftmind/mastermind";
const REPO = "github.com/xcrft/mastermind";
const MANIFEST = ".mastermind-workflow.json";
const MANIFEST_SCHEMA = 1;

const tty = process.stdout.isTTY && !process.env.NO_COLOR;
const paint = (code, value) => (tty ? `\x1b[${code}m${value}\x1b[0m` : value);
const green = (value) => paint("32", value);
const yellow = (value) => paint("33", value);
const bold = (value) => paint("1", value);
const dim = (value) => paint("2", value);

const CLIENTS = {
  claude: {
    homeDir: ".claude",
    agentsDir: "agents",
    skillsDir: "skills",
    agents: true,
    mcpClient: "claude",
  },
  codex: {
    homeDir: ".codex",
    skillsDir: "skills",
    agents: false,
    mcpClient: "codex",
  },
};

function packageVersion() {
  return JSON.parse(fs.readFileSync(PACKAGE_JSON, "utf8")).version;
}

function safeName(name, kind) {
  if (!name || name === "." || name === ".." || path.basename(name) !== name) {
    throw new Error(`unsafe ${kind} name in workflow bundle: ${JSON.stringify(name)}`);
  }
  return name;
}

function artifactDigest(artifact) {
  const hash = createHash("sha256");
  const visit = (current, relative) => {
    const stat = fs.lstatSync(current);
    if (stat.isSymbolicLink()) {
      throw new Error(`workflow bundle contains a symbolic link: ${current}`);
    }
    if (stat.isFile()) {
      hash.update("file\0");
      hash.update(relative.split(path.sep).join("/"));
      hash.update("\0");
      hash.update(fs.readFileSync(current));
      hash.update("\0");
      return;
    }
    if (!stat.isDirectory()) {
      throw new Error(`workflow bundle contains an unsupported artifact: ${current}`);
    }
    const entries = fs.readdirSync(current).sort();
    for (const entry of entries) visit(path.join(current, entry), path.join(relative, entry));
  };
  const stat = fs.lstatSync(artifact);
  visit(artifact, stat.isDirectory() ? "" : path.basename(artifact));
  return hash.digest("hex");
}

export function bundled(share = DEFAULT_SHARE) {
  const agentsDir = path.join(share, "agents");
  const skillsDir = path.join(share, "skills");
  const subagents = fs.existsSync(agentsDir)
    ? fs
        .readdirSync(agentsDir, { withFileTypes: true })
        .filter((entry) => entry.isFile() && entry.name.endsWith(".md"))
        .map((entry) => safeName(entry.name, "agent"))
        .sort()
    : [];
  const skills = fs.existsSync(skillsDir)
    ? fs
        .readdirSync(skillsDir, { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => safeName(entry.name, "skill"))
        .sort()
    : [];
  if (subagents.length === 0 || skills.length === 0) {
    throw new Error("workflow bundle is incomplete: expected at least one agent and one skill");
  }
  return { subagents, skills };
}

function expandClients(client) {
  if (client === "all") return Object.keys(CLIENTS);
  if (!Object.hasOwn(CLIENTS, client)) {
    throw new Error(`unsupported workflow client ${JSON.stringify(client)}; use claude, codex, or all`);
  }
  return [client];
}

function targetFor(home, client) {
  if (typeof home !== "string" || !path.isAbsolute(home)) {
    throw new Error("workflow home must be an absolute path");
  }
  const config = CLIENTS[client];
  const root = path.join(home, config.homeDir);
  return {
    client,
    config,
    root,
    manifestPath: path.join(root, MANIFEST),
  };
}

export function readManifest(manifestPath) {
  if (!fs.existsSync(manifestPath)) return null;
  let value;
  try {
    value = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  } catch (error) {
    throw new Error(`invalid workflow manifest at ${manifestPath}: ${error.message}`);
  }
  const valid =
    value?.schema_version === MANIFEST_SCHEMA &&
    value?.package === PKG &&
    typeof value?.version === "string" &&
    Object.hasOwn(CLIENTS, value?.client) &&
    Array.isArray(value?.artifacts?.agents) &&
    Array.isArray(value?.artifacts?.skills) &&
    value?.digests &&
    typeof value.digests === "object" &&
    !Array.isArray(value.digests);
  if (!valid) throw new Error(`unsupported workflow manifest at ${manifestPath}`);
  if (
    JSON.stringify(Object.keys(value).sort()) !==
      JSON.stringify(["artifacts", "client", "digests", "package", "schema_version", "version"]) ||
    JSON.stringify(Object.keys(value.artifacts).sort()) !==
      JSON.stringify(["agents", "skills"])
  ) {
    throw new Error(`unsupported workflow manifest fields at ${manifestPath}`);
  }
  for (const name of value.artifacts.agents) safeName(name, "manifest agent");
  for (const name of value.artifacts.skills) safeName(name, "manifest skill");
  const owned = [
    ...value.artifacts.agents.map((name) => `agents/${name}`),
    ...value.artifacts.skills.map((name) => `skills/${name}`),
  ].sort();
  if (
    JSON.stringify(Object.keys(value.digests).sort()) !== JSON.stringify(owned) ||
    Object.values(value.digests).some((digest) => !/^[0-9a-f]{64}$/.test(digest))
  ) {
    throw new Error(`unsupported workflow manifest digests at ${manifestPath}`);
  }
  return value;
}

function artifactPath(target, kind, name) {
  safeName(name, kind);
  const directory = kind === "agents" ? target.config.agentsDir : target.config.skillsDir;
  if (!directory) throw new Error(`${target.client} does not support ${kind}`);
  return path.join(target.root, directory, name);
}

function removePath(target) {
  fs.rmSync(target, { recursive: true, force: true });
}

function installClient({ home, share, client, version }) {
  const bundle = bundled(share);
  const target = targetFor(home, client);
  fs.mkdirSync(target.root, { recursive: true });
  const previous = readManifest(target.manifestPath);
  if (previous && previous.client !== client) {
    throw new Error(`workflow manifest client mismatch at ${target.manifestPath}`);
  }

  const desired = {
    agents: target.config.agents ? bundle.subagents : [],
    skills: bundle.skills,
  };
  const digests = Object.fromEntries([
    ...desired.agents.map((name) => [
      `agents/${name}`,
      artifactDigest(path.join(share, "agents", name)),
    ]),
    ...desired.skills.map((name) => [
      `skills/${name}`,
      artifactDigest(path.join(share, "skills", name)),
    ]),
  ]);
  const manifest = {
    schema_version: MANIFEST_SCHEMA,
    package: PKG,
    version,
    client,
    artifacts: desired,
    digests,
  };

  const stage = fs.mkdtempSync(path.join(target.root, ".mastermind-stage-"));
  const stagedArtifacts = path.join(stage, "new");
  const backupRoot = path.join(stage, "backup");
  const stagedManifest = path.join(stage, MANIFEST);
  const installed = [];
  const backups = [];

  try {
    if (target.config.agents) {
      for (const name of desired.agents) {
        const destination = path.join(stagedArtifacts, target.config.agentsDir, name);
        fs.mkdirSync(path.dirname(destination), { recursive: true });
        fs.copyFileSync(path.join(share, "agents", name), destination);
      }
    }
    for (const name of desired.skills) {
      const destination = path.join(stagedArtifacts, target.config.skillsDir, name);
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      fs.cpSync(path.join(share, "skills", name), destination, {
        recursive: true,
        force: false,
        errorOnExist: true,
      });
    }
    fs.writeFileSync(stagedManifest, `${JSON.stringify(manifest, null, 2)}\n`, {
      encoding: "utf8",
      mode: 0o600,
    });

    const moveAside = (destination, label) => {
      if (!fs.existsSync(destination)) return;
      const backup = path.join(backupRoot, label);
      fs.mkdirSync(path.dirname(backup), { recursive: true });
      fs.renameSync(destination, backup);
      backups.push({ destination, backup });
    };
    const replace = (source, destination, label) => {
      fs.mkdirSync(path.dirname(destination), { recursive: true });
      moveAside(destination, label);
      fs.renameSync(source, destination);
      installed.push(destination);
    };

    for (const kind of ["agents", "skills"]) {
      if (kind === "agents" && !target.config.agents) continue;
      for (const name of desired[kind]) {
        replace(
          path.join(stagedArtifacts, target.config[`${kind}Dir`], name),
          artifactPath(target, kind, name),
          path.join(kind, name),
        );
      }
    }

    if (previous) {
      for (const kind of ["agents", "skills"]) {
        if (kind === "agents" && !target.config.agents) continue;
        const retired = previous.artifacts[kind].filter((name) => !desired[kind].includes(name));
        for (const name of retired) {
          moveAside(artifactPath(target, kind, name), path.join("retired", kind, name));
        }
      }
    }

    replace(stagedManifest, target.manifestPath, "manifest.json");
    removePath(stage);
    return {
      client,
      root: target.root,
      skills: desired.skills.length,
      subagents: desired.agents.length,
      version,
    };
  } catch (error) {
    for (const destination of installed.reverse()) removePath(destination);
    for (const { destination, backup } of backups.reverse()) {
      if (fs.existsSync(destination)) removePath(destination);
      if (fs.existsSync(backup)) {
        fs.mkdirSync(path.dirname(destination), { recursive: true });
        fs.renameSync(backup, destination);
      }
    }
    removePath(stage);
    throw error;
  }
}

export function copyAll({
  home = os.homedir(),
  share = DEFAULT_SHARE,
  client = "claude",
  version = packageVersion(),
} = {}) {
  const names = expandClients(client);
  const bundle = bundled(share);
  for (const name of names) readManifest(targetFor(home, name).manifestPath);
  for (const agent of bundle.subagents) artifactDigest(path.join(share, "agents", agent));
  for (const skill of bundle.skills) artifactDigest(path.join(share, "skills", skill));
  return names.map((name) => installClient({ home, share, client: name, version }));
}

export function workflowStatus({
  home = os.homedir(),
  share = DEFAULT_SHARE,
  client = "claude",
  version = packageVersion(),
} = {}) {
  const bundle = bundled(share);
  return expandClients(client).map((name) => {
    const target = targetFor(home, name);
    let manifest;
    let manifestError = null;
    try {
      manifest = readManifest(target.manifestPath);
    } catch (error) {
      manifestError = error.message;
    }
    const expected = {
      agents: target.config.agents ? bundle.subagents : [],
      skills: bundle.skills,
    };
    const missing = [];
    const drifted = [];
    const expectedDigests = {};
    for (const kind of ["agents", "skills"]) {
      if (kind === "agents" && !target.config.agents) continue;
      for (const artifact of expected[kind]) {
        const key = `${kind}/${artifact}`;
        const bundledPath = path.join(share, kind, artifact);
        expectedDigests[key] = artifactDigest(bundledPath);
        const installedPath = artifactPath(target, kind, artifact);
        if (!fs.existsSync(installedPath)) {
          missing.push(key);
        } else {
          try {
            if (artifactDigest(installedPath) !== expectedDigests[key]) drifted.push(key);
          } catch {
            drifted.push(key);
          }
        }
      }
    }
    const manifestMatches =
      manifest?.version === version &&
      manifest?.client === name &&
      JSON.stringify(manifest?.artifacts) === JSON.stringify(expected) &&
      JSON.stringify(manifest?.digests) === JSON.stringify(expectedDigests);
    return {
      client: name,
      root: target.root,
      version,
      installed_version: manifest?.version ?? null,
      manifest: manifestError ? "invalid" : manifest ? "present" : "missing",
      manifest_error: manifestError,
      missing,
      drifted,
      parity:
        !manifestError &&
        manifestMatches &&
        missing.length === 0 &&
        drifted.length === 0,
      expected,
    };
  });
}

function registerMcp(client) {
  const wrapper = path.join(HERE, "mastermind.js");
  const result = spawnSync(
    process.execPath,
    [wrapper, "setup", CLIENTS[client].mcpClient, "--scope", "user", "--write"],
    { stdio: "inherit" },
  );
  return result.status === 0;
}

function completionBox(results, { updated, mcp }) {
  const indent = " ".repeat(10);
  const sep = "    " + dim("─".repeat(60));
  const lines = [
    "",
    `${indent}${green("✓")} ${bold(updated ? "Workflow Update Complete" : "Workflow Installation Complete")}`,
    "",
  ];
  for (const result of results) {
    lines.push(
      `${indent}${result.client}: ${green(`${result.skills} skills`)} + ${green(`${result.subagents} subagents`)} → ${result.root}`,
    );
    if (!updated && mcp) {
      lines.push(
        `${indent}${mcp[result.client] ? green("✓") : yellow("⚠")} ${result.client} MCP ${mcp[result.client] ? "registered" : "registration failed"}`,
      );
    }
  }
  lines.push(
    "",
    sep,
    "",
    `${indent}${dim("Check parity:")} ${bold("mastermind doctor --workflow --client all")}`,
    `${indent}${dim(REPO)}`,
    "",
  );
  console.log(lines.join("\n"));
}

function listBundled(share = DEFAULT_SHARE) {
  const { subagents, skills } = bundled(share);
  console.log(bold(`\n  ${PKG} — ${skills.length} skills + ${subagents.length} Claude subagents\n`));
  console.log(dim("  skills (Claude + Codex)"));
  for (const skill of skills) console.log(`    ${skill}`);
  console.log(dim("\n  subagents (Claude adapter)"));
  for (const agent of subagents) console.log(`    ${agent.replace(/\.md$/, "")}`);
  console.log("");
}

async function confirm(question) {
  if (!process.stdin.isTTY) return true;
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = await new Promise((resolve) => rl.question(question, resolve));
  rl.close();
  return !/^n/i.test(answer.trim());
}

export function parseArgs(argv) {
  const command = argv[0] ?? "install";
  let client = "claude";
  let json = false;
  for (let index = 1; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--client") {
      client = argv[index + 1];
      if (!client) throw new Error("--client requires claude, codex, or all");
      index += 1;
    } else if (arg === "--json") {
      json = true;
    } else if (arg !== "--workflow") {
      throw new Error(`unknown workflow installer argument: ${arg}`);
    }
  }
  expandClients(client);
  return { command, client, json };
}

export async function main(argv = process.argv.slice(2)) {
  try {
    const { command, client, json } = parseArgs(argv);
    if (command === "list") {
      listBundled();
      return 0;
    }
    if (command === "doctor") {
      const statuses = workflowStatus({ client, home: process.env.MASTERMIND_WORKFLOW_HOME });
      if (json) {
        console.log(JSON.stringify({ schema_version: 1, clients: statuses }, null, 2));
      } else {
        console.log("\nMastermind workflow parity\n");
        for (const status of statuses) {
          console.log(`  ${status.parity ? green("✓") : yellow("⚠")} ${status.client} — ${status.parity ? "current" : "drifted"}`);
          console.log(`    bundle ${status.version}; installed ${status.installed_version ?? "unknown"}; manifest ${status.manifest}`);
          for (const item of status.missing) console.log(`    missing ${item}`);
          for (const item of status.drifted) console.log(`    drifted ${item}`);
          if (status.manifest_error) console.log(`    ${status.manifest_error}`);
        }
        console.log("");
      }
      return statuses.every((status) => status.parity) ? 0 : 1;
    }
    if (!new Set(["install", "update"]).has(command)) {
      throw new Error(`unknown workflow command: ${command}`);
    }

    if (command === "install") {
      const names = expandClients(client).join(" + ");
      const ok = await confirm(
        `\n  Install the Mastermind workflow for ${green(names)} and register MCP? ${dim("[Y/n]")} `,
      );
      if (!ok) {
        console.log(dim("  Aborted — nothing written."));
        return 0;
      }
    }

    const results = copyAll({ client, home: process.env.MASTERMIND_WORKFLOW_HOME });
    const mcp = {};
    if (command === "install") {
      for (const result of results) mcp[result.client] = registerMcp(result.client);
    }
    completionBox(results, { updated: command === "update", mcp });
    return command === "install" && Object.values(mcp).some((registered) => !registered) ? 1 : 0;
  } catch (error) {
    console.error(`mastermind workflow: ${error.message}`);
    return 1;
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  process.exit(await main());
}
