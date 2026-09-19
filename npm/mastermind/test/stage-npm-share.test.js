import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const stageScript = path.join(repoRoot, "scripts", "stage-npm-share.sh");

test("staging workflow artifacts excludes Python bytecode caches", () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-npm-stage-"));
  try {
    const script = path.join(root, "scripts", "stage-npm-share.sh");
    const skill = path.join(root, "skills", "alpha");
    fs.mkdirSync(path.join(root, "agents", "subagents"), { recursive: true });
    fs.mkdirSync(path.dirname(script), { recursive: true });
    fs.mkdirSync(path.join(skill, "scripts", "__pycache__"), { recursive: true });
    fs.copyFileSync(stageScript, script);
    fs.writeFileSync(path.join(root, "agents", "subagents", "worker.md"), "worker\n");
    fs.writeFileSync(path.join(skill, "SKILL.md"), "---\nname: alpha\n---\n");
    fs.writeFileSync(path.join(skill, "scripts", "helper.py"), "print('ok')\n");
    fs.writeFileSync(path.join(skill, "scripts", "__pycache__", "helper.cpython-313.pyc"), "cache");
    fs.writeFileSync(path.join(skill, "scripts", "legacy.pyo"), "cache");

    const result = spawnSync("bash", [script], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stdout + result.stderr);

    const staged = path.join(root, "npm", "mastermind", "share", "skills", "alpha", "scripts");
    assert.equal(fs.readFileSync(path.join(staged, "helper.py"), "utf8"), "print('ok')\n");
    assert.equal(fs.existsSync(path.join(staged, "__pycache__")), false);
    assert.equal(fs.existsSync(path.join(staged, "legacy.pyo")), false);
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
});
