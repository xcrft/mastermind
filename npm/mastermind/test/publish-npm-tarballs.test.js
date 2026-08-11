import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
const publisher = path.join(repoRoot, "scripts", "publish-npm-tarballs.sh");

function integrity(file) {
  return `sha512-${crypto.createHash("sha512").update(fs.readFileSync(file)).digest("base64")}`;
}

function fixture({ published = {}, lookupErrors = {} } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-npm-publish-"));
  const packed = path.join(root, "packed");
  const bin = path.join(root, "bin");
  fs.mkdirSync(packed);
  fs.mkdirSync(bin);

  const manifest = path.join(root, "package.json");
  fs.writeFileSync(
    manifest,
    `${JSON.stringify({
      name: "@scope/root",
      version: "1.2.3",
      optionalDependencies: {
        "@scope/platform-a": "1.2.3",
        "@scope/platform-b": "1.2.3",
      },
    })}\n`,
  );

  const files = {
    "@scope/platform-a@1.2.3": "scope-platform-a-1.2.3.tgz",
    "@scope/platform-b@1.2.3": "scope-platform-b-1.2.3.tgz",
    "@scope/root@1.2.3": "scope-root-1.2.3.tgz",
  };
  for (const [spec, name] of Object.entries(files)) {
    fs.writeFileSync(path.join(packed, name), `tarball:${spec}\n`);
  }

  const statePath = path.join(root, "state.json");
  fs.writeFileSync(
    statePath,
    JSON.stringify({ published, lookupErrors, files, events: [] }),
  );
  const fakeNpm = path.join(bin, "npm");
  fs.writeFileSync(
    fakeNpm,
    `#!/usr/bin/env node
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const statePath = process.env.NPM_RESUME_STATE;
const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
const args = process.argv.slice(2);
if (args[0] === "view") {
  const spec = args[1];
  if (state.lookupErrors[spec]) {
    console.error(state.lookupErrors[spec]);
    process.exit(1);
  }
  if (!state.published[spec]) {
    console.error("npm error code E404");
    process.exit(1);
  }
  console.log(JSON.stringify(state.published[spec]));
  process.exit(0);
}
if (args[0] === "publish") {
  const tarball = args.at(-1);
  const base = path.basename(tarball);
  const spec = Object.entries(state.files).find(([, file]) => file === base)?.[0];
  if (!spec) throw new Error("unexpected tarball " + base);
  const value = "sha512-" + crypto.createHash("sha512").update(fs.readFileSync(tarball)).digest("base64");
  state.published[spec] = value;
  state.events.push({ spec, args });
  fs.writeFileSync(statePath, JSON.stringify(state));
  process.exit(0);
}
throw new Error("unexpected npm invocation: " + args.join(" "));
`,
  );
  fs.chmodSync(fakeNpm, 0o755);

  return {
    root,
    packed,
    manifest,
    statePath,
    files,
    run() {
      return spawnSync("bash", [publisher, packed, manifest], {
        encoding: "utf8",
        env: {
          ...process.env,
          PATH: `${bin}${path.delimiter}${process.env.PATH}`,
          NPM_RESUME_STATE: statePath,
          NPM_PUBLISH_VERIFY_ATTEMPTS: "1",
        },
      });
    },
    state() {
      return JSON.parse(fs.readFileSync(statePath, "utf8"));
    },
    cleanup() {
      fs.rmSync(root, { recursive: true, force: true });
    },
  };
}

test("a partial npm release resumes, verifies existing bytes, and publishes root last", () => {
  const f = fixture();
  try {
    const state = f.state();
    const first = "@scope/platform-a@1.2.3";
    state.published[first] = integrity(path.join(f.packed, f.files[first]));
    fs.writeFileSync(f.statePath, JSON.stringify(state));

    const result = f.run();
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stdout, /already published with matching integrity/);
    const events = f.state().events;
    assert.deepEqual(events.map(({ spec }) => spec), [
      "@scope/platform-b@1.2.3",
      "@scope/root@1.2.3",
    ]);
    for (const { args } of events) {
      assert.ok(args.includes("--provenance"), args.join(" "));
      assert.ok(args.includes("--access"), args.join(" "));
      assert.ok(args.includes("public"), args.join(" "));
    }
  } finally {
    f.cleanup();
  }
});

test("an existing npm version with different bytes fails before any publish", () => {
  const spec = "@scope/platform-a@1.2.3";
  const f = fixture({ published: { [spec]: "sha512-not-the-local-tarball" } });
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /integrity mismatch/);
    assert.deepEqual(f.state().events, []);
  } finally {
    f.cleanup();
  }
});

test("a mismatched existing root aborts before filling missing platforms", () => {
  const rootSpec = "@scope/root@1.2.3";
  const f = fixture({ published: { [rootSpec]: "sha512-not-the-local-root" } });
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /integrity mismatch/);
    assert.deepEqual(f.state().events, []);
  } finally {
    f.cleanup();
  }
});

test("a registry lookup failure is not mistaken for an unpublished version", () => {
  const spec = "@scope/platform-a@1.2.3";
  const f = fixture({ lookupErrors: { [spec]: "npm error code EAI_AGAIN" } });
  try {
    const result = f.run();
    assert.notEqual(result.status, 0, result.stdout + result.stderr);
    assert.match(result.stderr, /registry lookup failed/);
    assert.deepEqual(f.state().events, []);
  } finally {
    f.cleanup();
  }
});
