import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { copyAll, readManifest } from "../bin/install.js";

const PACKAGE = "@xcraftmind/mastermind";
const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = path.resolve(PACKAGE_ROOT, "../..");
const ENTRY = "import { pathToFileURL } from 'node:url'; const { main } = await import(pathToFileURL(process.argv[2]).href); process.exit(await main(JSON.parse(process.argv[3]), JSON.parse(process.argv[4])));";

function skillNames(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    if (!entry.isDirectory()) return [];
    const child = path.join(directory, entry.name);
    return fs.existsSync(path.join(child, "SKILL.md")) ? [entry.name] : skillNames(child);
  });
}

function fixture(t, mode = "global") {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-update-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const home = path.join(root, "home");
  const prefix = path.join(root, "scope with spaces $literal");
  const modules = mode === "global" && process.platform !== "win32"
    ? path.join(prefix, "lib", "node_modules") : path.join(prefix, "node_modules");
  const packageRoot = path.join(modules, ...PACKAGE.split("/"));
  const next = path.join(root, "next-package");
  const calls = path.join(root, "npm-calls.jsonl");
  fs.mkdirSync(home, { recursive: true });
  function packageFixture(directory, version) {
    fs.mkdirSync(path.join(directory, "bin"), { recursive: true });
    fs.writeFileSync(path.join(directory, "package.json"), JSON.stringify({ name: PACKAGE, version, type: "module" }));
    for (const name of ["install.js", "update.js", "help.js"]) {
      const source = path.join(PACKAGE_ROOT, "bin", name);
      if (fs.existsSync(source)) fs.copyFileSync(source, path.join(directory, "bin", name));
    }
    fs.writeFileSync(path.join(directory, "bin", "mmcg.js"), `console.log('mmcg ${version}');\n`);
    fs.mkdirSync(path.join(directory, "share", "agents"), { recursive: true });
    fs.writeFileSync(path.join(directory, "share", "agents", "mastermind-example.md"), `agent ${version}\n`);
    for (const skill of skillNames(path.join(REPO_ROOT, "skills"))) {
      const target = path.join(directory, "share", "skills", skill);
      fs.mkdirSync(target, { recursive: true });
      fs.writeFileSync(path.join(target, "SKILL.md"), `${skill} ${version}\n`);
    }
  }
  packageFixture(packageRoot, "1.0.0");
  packageFixture(next, "2.0.0");
  if (mode === "project") {
    fs.writeFileSync(path.join(prefix, "package.json"), JSON.stringify({
      name: "synthetic-project", private: true, devDependencies: { [PACKAGE]: "^1.0.0" },
    }));
  }
  const npm = path.join(root, "npm-cli.js");
  fs.writeFileSync(npm, `
const fs = require('node:fs');
const path = require('node:path');
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FIXTURE_CALLS, JSON.stringify({args,cwd:process.cwd()})+'\\n');
if (args[0] === 'root') {
  console.log(process.env.FIXTURE_NPM_ROOT || process.env.FIXTURE_MODULES);
} else if (args[0] === 'install') {
  if (process.env.FIXTURE_NPM_MODE === 'fail') process.exit(7);
  fs.cpSync(process.env.FIXTURE_NEXT, process.env.FIXTURE_PACKAGE, {recursive:true,force:true});
  if (process.env.FIXTURE_USER_EDIT) fs.writeFileSync(process.env.FIXTURE_USER_EDIT, 'edited while npm was running\\n');
  if (process.env.FIXTURE_NPM_MODE === 'partial') process.exit(7);
} else {
  throw new Error('unexpected npm call '+JSON.stringify(args));
}
`);
  const env = {
    ...process.env, HOME: home, USERPROFILE: home, MASTERMIND_WORKFLOW_HOME: home,
    npm_execpath: npm, FIXTURE_CALLS: calls, FIXTURE_MODULES: modules,
    FIXTURE_NEXT: next, FIXTURE_PACKAGE: packageRoot, NO_COLOR: "1",
  };
  return {
    root, home, prefix, modules, packageRoot, next, env,
    install(client, profile = "full") {
      return copyAll({ home, share: path.join(packageRoot, "share"), version: "1.0.0", client, profile });
    },
    manifest(client) {
      return readManifest(path.join(home, `.${client}`, ".mastermind-workflow.json"));
    },
    calls() {
      return fs.existsSync(calls) ? fs.readFileSync(calls, "utf8").trim().split("\n").map(JSON.parse) : [];
    },
    run(args = [], overrides = {}) {
      const output = spawnSync(process.execPath, ["--input-type=module", "--eval", ENTRY,
        "mastermind-update-test", path.join(packageRoot, "bin", "update.js"), JSON.stringify(["update", "--json", ...args]),
        JSON.stringify({ packageRoot, installMode: mode, packageName: PACKAGE, version: "1.0.0", nodePath: process.execPath, ...overrides }),
      ], { cwd: root, env, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
      return { ...output, report: output.stdout.trim() ? JSON.parse(output.stdout) : null };
    },
  };
}

test("dry-run plans package and installed profiles without invoking npm or writing client files", (t) => {
  const f = fixture(t);
  f.install("codex", "frontend");
  const before = JSON.stringify(f.manifest("codex"));
  const output = f.run(["--dry-run"]);
  assert.equal(output.status, 0, output.stderr);
  assert.equal(output.report.status, "planned");
  assert.equal(output.report.package.status, "planned");
  assert.deepEqual(output.report.workflows.clients.map(({ client, profile }) => ({ client, profile })),
    [{ client: "codex", profile: "frontend" }]);
  assert.deepEqual(f.calls(), []);
  assert.equal(JSON.stringify(f.manifest("codex")), before);
  assert.equal(fs.existsSync(path.join(f.home, ".claude")), false);
});

test("global update verifies the new binary and refreshes only installed integrations with a fresh installer", (t) => {
  const f = fixture(t);
  f.install("claude", "frontend");
  f.install("codex", "security");
  const user = path.join(f.home, ".claude", "agents", "user-owned.md");
  const settings = path.join(f.home, ".codex", "config.toml");
  fs.writeFileSync(user, "user content\n");
  fs.writeFileSync(settings, "# user settings\n");
  const output = f.run();
  assert.equal(output.status, 0, JSON.stringify(output.report));
  assert.equal(output.report.status, "complete");
  assert.equal(output.report.package.status, "updated");
  assert.deepEqual(output.report.package.binary, { status: "verified", version: "2.0.0" });
  assert.deepEqual(output.report.workflows.clients.map(({ client, profile, version, parity }) => ({ client, profile, version, parity })), [
    { client: "claude", profile: "frontend", version: "2.0.0", parity: true },
    { client: "codex", profile: "security", version: "2.0.0", parity: true },
  ]);
  assert.equal(f.manifest("claude").version, "2.0.0");
  assert.equal(fs.readFileSync(user, "utf8"), "user content\n");
  assert.equal(fs.readFileSync(settings, "utf8"), "# user settings\n");
  const command = f.calls().find((row) => row.args[0] === "install");
  assert.equal(command.args.includes("--global"), true);
  assert.equal(command.args[command.args.indexOf("--prefix") + 1], fs.realpathSync(f.prefix));
  assert.equal(command.args.at(-1), `${PACKAGE}@latest`);
  assert.equal(command.cwd, fs.realpathSync(f.prefix));
  assert.equal(fs.readFileSync(path.join(f.home, ".claude", "skills", "mastermind-task-planning", "SKILL.md"), "utf8"), "mastermind-task-planning 2.0.0\n");
});

test("project update preserves dependency kind and uses the owning project instead of the caller directory", (t) => {
  const f = fixture(t, "project");
  const output = f.run();
  assert.equal(output.status, 0, JSON.stringify(output.report));
  assert.equal(output.report.workflows.status, "not_installed");
  const command = f.calls().find((row) => row.args[0] === "install");
  assert.equal(command.args.includes("--global"), false);
  assert.equal(command.args.includes("--save-dev"), true);
  assert.equal(command.args[command.args.indexOf("--prefix") + 1], fs.realpathSync(f.prefix));
  assert.equal(command.cwd, fs.realpathSync(f.prefix));
  assert.equal(fs.existsSync(path.join(f.home, ".claude")), false);
  assert.equal(fs.existsSync(path.join(f.home, ".codex")), false);
});

test("workflow-only supports manual installations and explicit profile changes without npm or binary claims", (t) => {
  const f = fixture(t, "manual");
  f.install("codex", "full");
  fs.writeFileSync(path.join(f.packageRoot, "bin", "mmcg.js"), "throw new Error('native must not launch');\n");
  const output = f.run(["--workflow-only", "--client", "codex", "--profile", "core"]);
  assert.equal(output.status, 0, JSON.stringify(output.report));
  assert.equal(output.report.package.status, "not_requested");
  assert.equal(output.report.package.binary.status, "not_checked");
  assert.equal(f.manifest("codex").profile, "core");
  assert.equal(f.manifest("codex").version, "1.0.0");
  assert.deepEqual(f.calls(), []);
});

test("npx unknown and ambiguous project scopes require manual action without a guessed global install", (t) => {
  for (const mode of ["npx", "manual", "unknown", "project", "project_missing", "project_git"]) {
    const f = fixture(t, mode.startsWith("project") ? "project" : mode);
    if (mode === "project") fs.writeFileSync(path.join(f.prefix, "package.json"), '{"private":true}\n');
    if (mode === "project_missing") fs.rmSync(path.join(f.prefix, "package.json"));
    if (mode === "project_git") fs.writeFileSync(path.join(f.prefix, "package.json"), JSON.stringify({ dependencies: { [PACKAGE]: "example/mastermind" } }));
    const output = f.run();
    assert.equal(output.status, 1, mode);
    assert.equal(output.report.status, "manual_required", JSON.stringify(output.report));
    assert.equal(output.report.package.status, "manual_required");
    assert.equal(output.report.recovery.length, 1);
    assert.deepEqual(f.calls(), []);
  }
});

test("edited ownership and unowned collisions block before npm and leave the user files intact", (t) => {
  for (const kind of ["edited", "unowned", "invalid_manifest"]) {
    const f = fixture(t);
    f.install("codex", "full");
    const skill = path.join(f.home, ".codex", "skills", "mastermind-task-planning", "SKILL.md");
    if (kind === "edited") fs.writeFileSync(skill, "user modification\n");
    if (kind === "unowned") {
      const manifest = path.join(f.home, ".codex", ".mastermind-workflow.json");
      fs.rmSync(manifest);
    }
    if (kind === "invalid_manifest") fs.writeFileSync(path.join(f.home, ".codex", ".mastermind-workflow.json"), "{invalid");
    const before = fs.readFileSync(skill, "utf8");
    const output = f.run(["--client", "codex"]);
    assert.equal(output.status, 1, kind);
    assert.equal(output.report.status, "failed");
    const reason = { edited: /edited owned workflow artifact/, unowned: /unowned workflow artifact conflicts/, invalid_manifest: /invalid workflow manifest/ }[kind];
    assert.match(output.report.reason, reason);
    assert.deepEqual(f.calls(), []);
    assert.equal(fs.readFileSync(skill, "utf8"), before);
  }
});

test("new bundle conflicts and edits during npm stop adapter updates after a successful package step", (t) => {
  for (const mode of ["new_collision", "concurrent_edit"]) {
    const f = fixture(t);
    f.install("codex", "full");
    const user = path.join(f.home, ".codex", "skills", "local-skill", "SKILL.md");
    fs.mkdirSync(path.dirname(user), { recursive: true });
    fs.writeFileSync(user, "user skill\n");
    if (mode === "new_collision") {
      const bundled = path.join(f.next, "share", "skills", "local-skill", "SKILL.md");
      fs.mkdirSync(path.dirname(bundled), { recursive: true });
      fs.writeFileSync(bundled, "new package skill\n");
    } else {
      f.env.FIXTURE_USER_EDIT = path.join(f.home, ".codex", "skills", "mastermind-task-planning", "SKILL.md");
    }
    const output = f.run();
    assert.equal(output.status, 1, mode);
    assert.equal(output.report.status, "partial");
    assert.equal(output.report.package.status, "updated");
    assert.equal(output.report.package.binary.status, "verified");
    assert.match(output.report.reason, mode === "new_collision"
      ? /unowned workflow artifact conflicts/ : /edited owned workflow artifact/);
    assert.equal(f.manifest("codex").version, "1.0.0");
    assert.equal(fs.readFileSync(user, "utf8"), "user skill\n");
    if (mode === "concurrent_edit") assert.equal(fs.readFileSync(f.env.FIXTURE_USER_EDIT, "utf8"), "edited while npm was running\n");
  }
});

test("npm failure package identity drift and mismatched native versions never become complete", (t) => {
  for (const mode of ["npm_fail", "npm_partial", "package_identity", "binary_version", "scope_mismatch"]) {
    const f = fixture(t);
    f.install("codex", "core");
    if (mode === "npm_fail") f.env.FIXTURE_NPM_MODE = "fail";
    if (mode === "npm_partial") f.env.FIXTURE_NPM_MODE = "partial";
    if (mode === "package_identity") fs.writeFileSync(path.join(f.next, "package.json"), '{"name":"other-package","version":"2.0.0","type":"module"}');
    if (mode === "binary_version") fs.writeFileSync(path.join(f.next, "bin", "mmcg.js"), "console.log('mmcg 1.0.0');\n");
    if (mode === "scope_mismatch") f.env.FIXTURE_NPM_ROOT = f.home;
    const output = f.run();
    assert.equal(output.status, 1, mode);
    assert.equal(output.report.status, mode === "scope_mismatch" ? "failed" : "partial");
    const reason = {
      npm_fail: /exit 7/,
      npm_partial: /exit 7/,
      package_identity: /package identity or version/,
      binary_version: /native binary version/,
      scope_mismatch: /different installation scope/,
    }[mode];
    assert.match(output.report.reason, reason);
    assert.equal(f.manifest("codex").version, "1.0.0");
    if (mode === "scope_mismatch") assert.equal(f.calls().some((row) => row.args[0] === "install"), false);
    if (mode === "binary_version") assert.equal(output.report.package.binary.status, "failed");
    if (mode === "npm_partial") assert.equal(output.report.package.status, "unverified_after_attempt");
  }
});
