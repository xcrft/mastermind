import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const assembler = path.join(repoRoot, "scripts", "build-npm-packages.sh");
const targets = [
  ["aarch64-apple-darwin", "darwin-arm64"],
  ["x86_64-apple-darwin", "darwin-x64"],
  ["x86_64-unknown-linux-gnu", "linux-x64-gnu"],
  ["aarch64-unknown-linux-gnu", "linux-arm64-gnu"],
  ["x86_64-unknown-linux-musl", "linux-x64-musl"],
  ["aarch64-unknown-linux-musl", "linux-arm64-musl"],
  ["x86_64-pc-windows-msvc", "win32-x64-msvc"],
];

function nativePlatform() {
  const key = `${process.platform}-${process.arch}`;
  return {
    "darwin-arm64": ["aarch64-apple-darwin", "darwin-arm64"],
    "darwin-x64": ["x86_64-apple-darwin", "darwin-x64"],
    "linux-x64": ["x86_64-unknown-linux-gnu", "linux-x64-gnu"],
    "linux-arm64": ["aarch64-unknown-linux-gnu", "linux-arm64-gnu"],
  }[key];
}

function fixture(target, variant, { binaryVersion = "1.2.1", embeddedVersion = "1.2.1" } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-npm-assembler-"));
  const script = path.join(root, "scripts", "build-npm-packages.sh");
  const platformDir = path.join(root, "npm", "platforms", variant);
  const binary = path.join(root, "fake-mmcg");

  fs.mkdirSync(path.dirname(script), { recursive: true });
  fs.copyFileSync(assembler, script);
  fs.mkdirSync(path.join(root, "mcp", "servers", "mmcg"), { recursive: true });
  fs.writeFileSync(
    path.join(root, "mcp", "servers", "mmcg", "Cargo.toml"),
    '[package]\nname = "mmcg"\nversion = "1.2.1"\n',
  );
  fs.mkdirSync(platformDir, { recursive: true });
  fs.writeFileSync(
    path.join(platformDir, "package.json"),
    `${JSON.stringify({ name: `@xcraftmind/mmcg-${variant}`, version: "1.2.1" }, null, 2)}\n`,
  );
  fs.writeFileSync(
    binary,
    `#!/bin/sh\n# MMCG_BUILD_VERSION=[${embeddedVersion}]\nprintf 'mastermind %s\\n' '${binaryVersion}'\n`,
  );

  return {
    root,
    run() {
      return spawnSync("bash", [script, target, binary], { encoding: "utf8" });
    },
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

const native = nativePlatform();

test("assembler rejects a native binary whose reported version is wrong", { skip: !native }, () => {
  const [target, variant] = native;
  const f = fixture(target, variant, { binaryVersion: "9.9.9" });
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /binary version mismatch/);
  } finally {
    f.cleanup();
  }
});

test("assembler rejects a longer version with the expected version as its prefix", () => {
  const [target, variant] = targets.find(([candidate]) => candidate !== native?.[0]);
  const f = fixture(target, variant, { embeddedVersion: "1.2.10" });
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /embedded binary version mismatch/);
  } finally {
    f.cleanup();
  }
});

for (const [target, variant] of targets) {
  test(`assembler accepts matching version evidence for ${target}`, () => {
    const f = fixture(target, variant);
    try {
      const result = f.run();
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.match(result.stdout, new RegExp(`assembled ${variant}`));
    } finally {
      f.cleanup();
    }
  });
}
