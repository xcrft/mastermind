import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const SOURCE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function updateFixture(t, mode) {
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-entry-"));
  t.after(() => fs.rmSync(temporary, { recursive: true, force: true }));
  const home = path.join(temporary, "home");
  const caller = path.join(temporary, "unrelated caller");
  const appData = path.join(temporary, "appdata");
  const prefix = mode === "npx" ? path.join(temporary, "_npx", "cache-entry")
    : mode === "global" && process.platform === "win32" ? path.join(appData, "npm")
      : path.join(temporary, `${mode} owner`);
  const modules = mode === "global" && process.platform !== "win32"
    ? path.join(prefix, "lib", "node_modules") : path.join(prefix, "node_modules");
  const packageRoot = path.join(modules, "@xcraftmind", "mastermind");
  fs.mkdirSync(home);
  fs.mkdirSync(caller);
  fs.mkdirSync(path.join(packageRoot, "bin"), { recursive: true });
  fs.copyFileSync(path.join(SOURCE, "package.json"), path.join(packageRoot, "package.json"));
  for (const file of ["mastermind.js", "install.js", "update.js", "help.js", "resolve.js"]) {
    fs.copyFileSync(path.join(SOURCE, "bin", file), path.join(packageRoot, "bin", file));
  }
  if (mode === "project") {
    const version = JSON.parse(fs.readFileSync(path.join(packageRoot, "package.json"), "utf8")).version;
    fs.writeFileSync(path.join(prefix, "package.json"), JSON.stringify({
      private: true, devDependencies: { "@xcraftmind/mastermind": `^${version}` },
    }));
  }
  let entry = path.join(packageRoot, "bin", "mastermind.js");
  if (["global", "npx"].includes(mode) && process.platform !== "win32") {
    const link = path.join(mode === "global" ? prefix : temporary, "bin", "mastermind");
    fs.mkdirSync(path.dirname(link));
    fs.symlinkSync(entry, link);
    entry = link;
  }
  const npm = path.join(temporary, "npm-cli.js");
  const npmMarker = path.join(home, "npm-was-run");
  fs.writeFileSync(npm, `require('node:fs').writeFileSync(${JSON.stringify(npmMarker)}, 'unexpected'); process.exit(19);\n`);
  const env = {
    HOME: home, USERPROFILE: home, MASTERMIND_WORKFLOW_HOME: home,
    APPDATA: process.platform === "win32" ? appData.toUpperCase() : appData,
    PATH: "", npm_execpath: npm, NO_COLOR: "1",
    SystemRoot: process.env.SystemRoot ?? "",
  };
  return {
    home, prefix, packageRoot, env,
    run(args = ["update", "--dry-run", "--json"]) {
      const result = spawnSync(process.execPath, [entry, ...args], {
        cwd: caller, env, encoding: "utf8", timeout: 5000,
      });
      assert.equal(result.error, undefined);
      assert.equal(result.stderr, "");
      assert.deepEqual(fs.readdirSync(home), [], "dry run must not invoke npm or write client files");
      return { ...result, report: JSON.parse(result.stdout) };
    },
  };
}

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

test("public update resolves the global package behind its bin link or known Windows prefix", (t) => {
  const f = updateFixture(t, "global");
  const { status, report } = f.run();
  assert.equal(status, 0, JSON.stringify(report));
  assert.equal(report.status, "planned");
  assert.equal(report.package.install_mode, "global");
  assert.equal(report.package.command.cwd, fs.realpathSync(f.prefix));
  assert.equal(report.package.command.argv.includes("--global"), true);
  assert.equal(report.package.binary.status, "not_checked");
  assert.equal(report.workflows.status, "not_installed");
});

test("public update finds its direct project owner when called outside that project", (t) => {
  const f = updateFixture(t, "project");
  const { status, report } = f.run();
  assert.equal(status, 0, JSON.stringify(report));
  assert.equal(report.package.install_mode, "project");
  assert.equal(report.package.command.cwd, fs.realpathSync(f.prefix));
  assert.equal(report.package.command.argv.includes("--save-dev"), true);
  assert.equal(report.package.command.argv.includes("--global"), false);
  assert.equal(report.package.binary.status, "not_checked");
});

test("public update leaves npx and unowned node_modules installations manual", (t) => {
  for (const mode of ["npx", "unknown"]) {
    const f = updateFixture(t, mode);
    const { status, report } = f.run();
    assert.equal(status, 1);
    assert.equal(report.status, "manual_required");
    assert.equal(report.package.install_mode, mode);
    assert.equal(report.package.command, undefined);
    assert.equal(report.package.binary.status, "not_checked");
  }
});

test("native routing receives the canonical launcher and project installation hints", (t) => {
  const f = updateFixture(t, "project");
  fs.writeFileSync(path.join(f.packageRoot, "bin", "resolve.js"), `
export function resolveBinary() { return "synthetic-native"; }
export function runBinary(binary, args, env) {
  console.log(JSON.stringify({ binary, args, hints: Object.fromEntries([
    "MASTERMIND_INSTALL_MODE", "MASTERMIND_VERSION", "MASTERMIND_PACKAGE",
    "MASTERMIND_SHARE_DIR", "MASTERMIND_INSTALLER_JS", "MASTERMIND_LAUNCHER_JS", "MASTERMIND_NODE",
  ].map((key) => [key, env[key]])) }));
}
`);
  const { status, report } = f.run(["setup", "codex"]);
  assert.equal(status, 0);
  assert.equal(report.binary, "synthetic-native");
  assert.deepEqual(report.args, ["setup", "codex"]);
  const root = fs.realpathSync(f.packageRoot);
  const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  assert.deepEqual(report.hints, {
    MASTERMIND_INSTALL_MODE: "project",
    MASTERMIND_VERSION: pkg.version,
    MASTERMIND_PACKAGE: pkg.name,
    MASTERMIND_SHARE_DIR: path.join(root, "share"),
    MASTERMIND_INSTALLER_JS: path.join(root, "bin", "install.js"),
    MASTERMIND_LAUNCHER_JS: path.join(root, "bin", "mastermind.js"),
    MASTERMIND_NODE: process.execPath,
  });
});
