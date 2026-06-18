#!/usr/bin/env node
// `mastermind install / update / list` — set up the Mastermind workflow in
// Claude Code without the per-project codegraph index.
//
//   install — copy subagents + skills into ~/.claude AND register the mmcg MCP
//             server (so the agents can actually query the codegraph)
//   update  — re-copy subagents + skills only (MCP already registered)
//   list    — show what ships
//
// The codegraph INDEX is per-project: run `mastermind init` inside a repo to
// build it. Invoked by bin/mastermind.js when argv[2] is install/update/list.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SHARE = path.join(HERE, "..", "share");
const PKG = "@xcraftmind/mastermind";
const REPO = "github.com/xcrft/mastermind";

// ANSI — off when piped or NO_COLOR is set, so logs stay clean.
const tty = process.stdout.isTTY && !process.env.NO_COLOR;
const paint = (code, s) => (tty ? `\x1b[${code}m${s}\x1b[0m` : s);
const green = (s) => paint("32", s);
const yellow = (s) => paint("33", s);
const bold = (s) => paint("1", s);
const dim = (s) => paint("2", s);

// What the package ships, read straight off the bundled tree.
function bundled() {
  const agentsDir = path.join(SHARE, "agents");
  const skillsDir = path.join(SHARE, "skills");
  const subagents = fs.existsSync(agentsDir)
    ? fs.readdirSync(agentsDir).filter((f) => f.endsWith(".md"))
    : [];
  const skills = fs.existsSync(skillsDir)
    ? fs
        .readdirSync(skillsDir, { withFileTypes: true })
        .filter((e) => e.isDirectory())
        .map((e) => e.name)
    : [];
  return { subagents, skills };
}

function copyAll() {
  const { subagents, skills } = bundled();
  const claude = path.join(os.homedir(), ".claude");
  const agentsOut = path.join(claude, "agents");
  const skillsOut = path.join(claude, "skills");
  fs.mkdirSync(agentsOut, { recursive: true });
  fs.mkdirSync(skillsOut, { recursive: true });
  for (const f of subagents) {
    fs.copyFileSync(path.join(SHARE, "agents", f), path.join(agentsOut, f));
  }
  for (const s of skills) {
    // Overwrite the whole skill dir (SKILL.md + references) from the bundle.
    fs.cpSync(path.join(SHARE, "skills", s), path.join(skillsOut, s), {
      recursive: true,
      force: true,
    });
  }
  return { skills: skills.length, subagents: subagents.length };
}

// Register the mmcg MCP server by re-invoking this same wrapper's `setup claude`
// — that path already resolves the native binary and picks the right `command`
// form for the install mode (npx vs global vs project). Returns true on success.
function registerMcp() {
  const wrapper = path.join(HERE, "mastermind.js");
  const r = spawnSync(
    process.execPath,
    [wrapper, "setup", "claude", "--write-mcp"],
    { stdio: "inherit" },
  );
  return r.status === 0;
}

function completionBox({ skills, subagents }, { updated, mcp }) {
  const indent = " ".repeat(14);
  const sep = "    " + dim("─".repeat(60));
  const lines = [
    "",
    " ".repeat(26) + `${green("✓")}  ${bold(updated ? "Update Complete" : "Installation Complete")}`,
    "",
    `${indent}${updated ? "Updated" : "Installed"} ${green(`${skills} skills`)} + ${green(`${subagents} subagents`)} ${updated ? "in" : "to"} Claude Code`,
  ];
  if (!updated) {
    lines.push(
      mcp
        ? `${indent}${green("✓")} mmcg MCP server registered.`
        : `${indent}${yellow("⚠")} MCP not registered — install globally (${dim("npm i -g " + PKG)}) then ${dim("mastermind setup claude --write-mcp")}.`,
    );
    lines.push("");
    lines.push(`${indent}${dim("Next: run")} ${bold("mastermind init")} ${dim("in a project to build the codegraph index.")}`);
  }
  lines.push(
    "",
    sep,
    "",
    `${indent}Examples:`,
    "",
    `${indent}    →  ${dim('"who calls parseConfig?"')}`,
    `${indent}    →  ${dim('"plan a task to add per-tenant rate limiting"')}`,
    `${indent}    →  ${dim('"audit this executor report against the diff"')}`,
    "",
    sep,
    "",
    `${indent}Commands:`,
    "",
    `${indent}$ mastermind install`,
    `${indent}$ mastermind update`,
    `${indent}$ mastermind list`,
    "",
    sep,
    "",
    `${indent}${dim(REPO)}`,
    "",
  );
  console.log(lines.join("\n"));
}

function listBundled() {
  const { subagents, skills } = bundled();
  console.log(
    bold(`\n  ${PKG} — ${skills.length} skills + ${subagents.length} subagents\n`),
  );
  console.log(dim("  skills"));
  for (const s of skills) console.log(`    ${s}`);
  console.log(dim("\n  subagents"));
  for (const a of subagents) console.log(`    ${a.replace(/\.md$/, "")}`);
  console.log("");
}

async function confirm(question) {
  if (!process.stdin.isTTY) return true; // non-interactive (CI / piped) → proceed
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = await new Promise((resolve) => rl.question(question, resolve));
  rl.close();
  return !/^n/i.test(answer.trim());
}

const cmd = process.argv[2]; // install | update | list

if (cmd === "list") {
  listBundled();
} else if (cmd === "update") {
  completionBox(copyAll(), { updated: true });
} else {
  // install
  const { subagents, skills } = bundled();
  const ok = await confirm(
    `\n  Install ${green(`${skills.length} skills`)} + ${green(`${subagents.length} subagents`)} into Claude Code (${dim("~/.claude")}) and register the mmcg MCP? ${dim("[Y/n]")} `,
  );
  if (!ok) {
    console.log(dim("  Aborted — nothing written."));
    process.exit(0);
  }
  const counts = copyAll();
  const mcp = registerMcp();
  completionBox(counts, { updated: false, mcp });
}
