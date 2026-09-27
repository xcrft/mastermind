import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

test("public help works without a native package or a client installation", () => {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-help-"));
  try {
    const packageRoot = path.join(temporary, "package");
    const home = path.join(temporary, "home");
    fs.mkdirSync(home);
    fs.mkdirSync(path.join(packageRoot, "bin"), { recursive: true });
    const source = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
    fs.copyFileSync(path.join(source, "package.json"), path.join(packageRoot, "package.json"));
    for (const file of ["mastermind.js", "install.js", "help.js", "resolve.js"]) {
      fs.copyFileSync(path.join(source, "bin", file), path.join(packageRoot, "bin", file));
    }
    for (const [entry, args, expected] of [
      ["mastermind.js", [], "Start here:"],
      ["mastermind.js", ["install", "--help"], "Usage: mastermind install"],
      ["mastermind.js", ["update", "--help"], "--workflow-only"],
      ["mastermind.js", ["doctor", "--workflow", "--help"], "Read-only comparison"],
      ["mastermind.js", ["help", "list"], "Usage: mastermind list"],
      ["install.js", ["--help"], "Usage: mastermind install"],
    ]) {
      const result = spawnSync(process.execPath, [path.join(packageRoot, "bin", entry), ...args], {
        cwd: home, env: { HOME: home, USERPROFILE: home, PATH: "", NO_COLOR: "1", SystemRoot: process.env.SystemRoot ?? "" },
        encoding: "utf8", timeout: 5000,
      });
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.stdout.includes(expected), `${entry} ${args.join(" ")}: ${JSON.stringify(result.stdout)}`);
      assert.equal(result.stderr, "");
      assert.deepEqual(fs.readdirSync(home), []);
    }
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
});
