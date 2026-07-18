import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { copyAll, readManifest, workflowStatus } from "../bin/install.js";

function fixture() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-installer-"));
  const home = path.join(root, "home");
  const share = path.join(root, "share");
  fs.mkdirSync(path.join(share, "agents"), { recursive: true });
  fs.mkdirSync(path.join(share, "skills", "alpha"), { recursive: true });
  fs.writeFileSync(path.join(share, "agents", "mastermind-one.md"), "one\n");
  fs.writeFileSync(path.join(share, "skills", "alpha", "SKILL.md"), "alpha\n");
  return {
    root,
    home,
    share,
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

test("update reconciles owned artifacts and preserves unrelated client files", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "claude", version: "1.0.0" });
    const claude = path.join(f.home, ".claude");
    fs.writeFileSync(path.join(claude, "skills", "alpha", "retired.txt"), "stale\n");
    fs.mkdirSync(path.join(claude, "skills", "user-skill"), { recursive: true });
    fs.writeFileSync(path.join(claude, "skills", "user-skill", "SKILL.md"), "user\n");
    fs.writeFileSync(path.join(claude, "agents", "user-agent.md"), "user\n");

    fs.rmSync(path.join(f.share, "agents", "mastermind-one.md"));
    fs.writeFileSync(path.join(f.share, "agents", "mastermind-two.md"), "two\n");
    fs.writeFileSync(path.join(f.share, "skills", "alpha", "SKILL.md"), "alpha v2\n");
    copyAll({ home: f.home, share: f.share, client: "claude", version: "2.0.0" });

    assert.equal(fs.existsSync(path.join(claude, "agents", "mastermind-one.md")), false);
    assert.equal(fs.readFileSync(path.join(claude, "agents", "mastermind-two.md"), "utf8"), "two\n");
    assert.equal(fs.existsSync(path.join(claude, "skills", "alpha", "retired.txt")), false);
    assert.equal(fs.readFileSync(path.join(claude, "skills", "alpha", "SKILL.md"), "utf8"), "alpha v2\n");
    assert.equal(fs.readFileSync(path.join(claude, "skills", "user-skill", "SKILL.md"), "utf8"), "user\n");
    assert.equal(fs.readFileSync(path.join(claude, "agents", "user-agent.md"), "utf8"), "user\n");

    const manifest = readManifest(path.join(claude, ".mastermind-workflow.json"));
    assert.deepEqual(manifest.artifacts.agents, ["mastermind-two.md"]);
    assert.deepEqual(manifest.artifacts.skills, ["alpha"]);
    assert.match(manifest.digests["skills/alpha"], /^[0-9a-f]{64}$/);
    assert.equal(workflowStatus({ home: f.home, share: f.share, client: "claude", version: "2.0.0" })[0].parity, true);
  } finally {
    f.cleanup();
  }
});

test("Codex adapter installs skills without Claude subagents", () => {
  const f = fixture();
  try {
    const [result] = copyAll({ home: f.home, share: f.share, client: "codex", version: "1.0.0" });
    assert.equal(result.subagents, 0);
    assert.equal(fs.existsSync(path.join(f.home, ".codex", "skills", "alpha", "SKILL.md")), true);
    assert.equal(fs.existsSync(path.join(f.home, ".codex", "agents")), false);
    assert.equal(workflowStatus({ home: f.home, share: f.share, client: "codex", version: "1.0.0" })[0].parity, true);
  } finally {
    f.cleanup();
  }
});

test("invalid ownership manifest fails before replacing installed files", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "claude", version: "1.0.0" });
    const claude = path.join(f.home, ".claude");
    fs.writeFileSync(path.join(claude, ".mastermind-workflow.json"), "not json\n");
    fs.writeFileSync(path.join(f.share, "agents", "mastermind-one.md"), "replacement\n");
    assert.throws(
      () => copyAll({ home: f.home, share: f.share, client: "claude", version: "2.0.0" }),
      /invalid workflow manifest/,
    );
    assert.equal(fs.readFileSync(path.join(claude, "agents", "mastermind-one.md"), "utf8"), "one\n");
  } finally {
    f.cleanup();
  }
});

test("doctor detects installed content tampering", () => {
  const f = fixture();
  try {
    copyAll({ home: f.home, share: f.share, client: "codex", version: "1.0.0" });
    fs.writeFileSync(
      path.join(f.home, ".codex", "skills", "alpha", "SKILL.md"),
      "tampered\n",
    );
    const [status] = workflowStatus({
      home: f.home,
      share: f.share,
      client: "codex",
      version: "1.0.0",
    });
    assert.equal(status.parity, false);
    assert.deepEqual(status.drifted, ["skills/alpha"]);
  } finally {
    f.cleanup();
  }
});
