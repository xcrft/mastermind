import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { setTimeout as pause } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const SOURCE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const NATIVE = process.env.MMCG_TEST_BINARY;
const CORE = [
  "mastermind-architecture-review", "mastermind-change-impact", "mastermind-codegraph-research",
  "mastermind-comment-audit", "mastermind-critical-review", "mastermind-investigation-ledger",
  "mastermind-project-history", "mastermind-project-map", "mastermind-product-intake",
  "mastermind-structured-report-contract", "mastermind-task-executor", "mastermind-task-planning",
  "mastermind-test-audit", "mastermind-test-impact", "no-ai-slop-comments",
];
const CODEX = `
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const argv = process.argv.slice(2);
if (argv[0] === 'exec') {
  assert.equal(process.env.MASTERMIND_MINER, '1');
  const model = argv[argv.indexOf('--model')+1];
  const request = JSON.parse(fs.readFileSync(0, 'utf8'));
  const episode = request.episode;
  assert.ok(episode);
  fs.appendFileSync(path.join(process.env.FIXTURE_ACCOUNT, 'provider-calls.jsonl'),
    JSON.stringify({model, episode: episode.id})+'\\n');
  console.log(JSON.stringify({type:'item.completed', item:{type:'agent_message',
    text:JSON.stringify({schema:1, episode_id:episode.id, episode_revision:episode.revision, drafts:[]})}}));
  console.log(JSON.stringify({type:'turn.completed'}));
  process.exit(0);
}
fs.appendFileSync(path.join(process.env.HOME, 'native-calls.jsonl'), JSON.stringify(argv)+'\\n');
if (process.env.FIXTURE_FORBID_NATIVE === '1') process.exit(91);
assert.equal(argv[0], 'mcp', 'model calls are forbidden');
const receipt = path.join(process.env.HOME, 'native-registration.json');
if (argv[1] === 'list') {
  assert.deepEqual(argv, ['mcp', 'list', '--json']);
  console.log(fs.existsSync(receipt) ? fs.readFileSync(receipt, 'utf8') : '[]');
} else {
  assert.deepEqual(argv.slice(0, 6), ['mcp', 'add', 'mmcg', '--env', 'MMCG_PROFILE_CLIENT=codex', '--']);
  const [command, ...args] = argv.slice(6);
  assert.equal(command, process.env.FIXTURE_NODE);
  assert.deepEqual(args, [process.env.FIXTURE_LAUNCHER, 'serve']);
  const env = {MMCG_PROFILE_CLIENT: 'codex'};
  fs.mkdirSync(process.env.CODEX_HOME, {recursive: true});
  fs.writeFileSync(path.join(process.env.CODEX_HOME, 'config.toml'),
    '[mcp_servers.mmcg]\\ncommand = '+JSON.stringify(command)+'\\nargs = '+JSON.stringify(args)+
    '\\n[mcp_servers.mmcg.env]\\nMMCG_PROFILE_CLIENT = "codex"\\n');
  fs.writeFileSync(receipt, JSON.stringify([{name: 'mmcg', enabled: true,
    transport: {type: 'stdio', command, args, env, env_vars: [], cwd: null}}]));
}
`;

function assertCurrentIndex(index) {
  assert.equal(index.db_exists, true);
  assert.ok(index.symbol_count > 0);
  assert.equal(index.stale_count, 0);
  assert.equal(index.extractor_contract_current, true);
  assert.equal(index.concept_contract_current, true);
  assert.equal(index.history_freshness, "fresh");
  for (const field of ["database_error", "root_error", "freshness_error", "history_freshness_error"]) {
    assert.equal(index[field], null, field);
  }
}

test("one npm init installs workflows, mines in the task and delivers the profile", {
  skip: process.platform === "win32" ? "native hooks require Unix"
    : !NATIVE ? "set MMCG_TEST_BINARY to the built native binary" : false,
  timeout: 120_000,
}, async (t) => {
  assert.equal(path.isAbsolute(NATIVE), true, "MMCG_TEST_BINARY must be absolute");
  const temporary = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-onboard-")));
  t.after(() => {
    if (fs.existsSync(path.join(root, ".mastermind", "setup.json"))) {
      spawnSync(process.execPath, [launcher, "miner", "stop", "--json"], {cwd:root, env, timeout:10_000});
    }
    fs.rmSync(temporary, { recursive: true, force: true });
  });
  const root = path.join(temporary, "project");
  const home = path.join(temporary, "home");
  const bin = path.join(temporary, "bin");
  for (const directory of [root, home, bin]) fs.mkdirSync(directory);
  const pkg = JSON.parse(fs.readFileSync(path.join(SOURCE, "package.json"), "utf8"));
  const modules = path.join(root, "node_modules");
  const packageRoot = path.join(modules, ...pkg.name.split("/"));
  fs.mkdirSync(packageRoot, { recursive: true });
  fs.cpSync(path.join(SOURCE, "bin"), path.join(packageRoot, "bin"), { recursive: true });
  fs.writeFileSync(path.join(packageRoot, "package.json"), JSON.stringify(pkg));
  fs.writeFileSync(path.join(root, "package.json"), JSON.stringify({
    private: true, devDependencies: { [pkg.name]: pkg.version },
  }));
  const platform = process.platform === "darwin" ? `darwin-${process.arch}`
    : `linux-${process.arch}-${process.report.getReport().header.glibcVersionRuntime ? "gnu" : "musl"}`;
  const nativePackage = path.join(modules, "@xcraftmind", `mmcg-${platform}`);
  fs.mkdirSync(path.join(nativePackage, "bin"), { recursive: true });
  fs.writeFileSync(path.join(nativePackage, "package.json"), JSON.stringify({ name: `@xcraftmind/mmcg-${platform}`, version: pkg.version }));
  fs.symlinkSync(fs.realpathSync(NATIVE), path.join(nativePackage, "bin", "mmcg"));
  const share = path.join(packageRoot, "share");
  fs.mkdirSync(path.join(share, "agents"), { recursive: true });
  fs.writeFileSync(path.join(share, "agents", "mastermind-fixture.md"), "Synthetic agent fixture.\n");
  for (const skill of CORE) {
    fs.mkdirSync(path.join(share, "skills", skill), { recursive: true });
    fs.writeFileSync(path.join(share, "skills", skill, "SKILL.md"), `# ${skill}\n\nSynthetic workflow fixture.\n`);
  }
  const launcher = path.join(packageRoot, "bin", "mastermind.js");
  const clientScript = path.join(bin, "codex.cjs");
  fs.writeFileSync(clientScript, CODEX);
  const quote = (value) => `'${value.replaceAll("'", "'\\''")}'`;
  fs.writeFileSync(path.join(bin, "codex"), `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(clientScript)} "$@"\n`, { mode: 0o700 });
  const env = {
    HOME: home, USERPROFILE: home, CODEX_HOME: path.join(home, ".codex"),
    MASTERMIND_WORKFLOW_HOME: home, PATH: `${bin}:/usr/bin:/bin`, NO_COLOR: "1",
    GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null",
    FIXTURE_NODE: process.execPath, FIXTURE_LAUNCHER: launcher,
    FIXTURE_ACCOUNT: home,
  };
  const run = (command, args, additions = {}) => {
    const output = spawnSync(command, args, { cwd: root, env: { ...env, ...additions },
      encoding: "utf8", timeout: 30_000, maxBuffer: 4 * 1024 * 1024 });
    assert.equal(output.error, undefined);
    assert.equal(output.status, 0, `${output.stdout}\n${output.stderr}`);
    return output;
  };
  const mastermind = (args, additions) => JSON.parse(run(process.execPath, [launcher, ...args], additions).stdout);
  fs.writeFileSync(path.join(root, "module.py"), "def welcome(name):\n    return name\n");
  fs.writeFileSync(path.join(root, ".gitignore"), "node_modules/\n.mastermind/\n.codex/\n");
  fs.writeFileSync(path.join(root, "CONTEXT.md"), "# Context\n\nKeep this user-owned project description.\n");
  run("/usr/bin/git", ["init", "-q", "--initial-branch=main"]);
  run("/usr/bin/git", ["config", "user.name", "Fixture User"]);
  run("/usr/bin/git", ["config", "user.email", "fixture@example.invalid"]);
  run("/usr/bin/git", ["add", "module.py", "CONTEXT.md", ".gitignore"]);
  for (let i = 0; i < 12; i++) run("/usr/bin/git", ["commit", "--allow-empty", "-qm", `Adjust sample ${i}`]);
  const first = mastermind(["init", "--json"]);
  assert.equal(first.status, "configured");
  assert.equal(first.settings.workflow, true);
  assert.equal(first.settings.mining, "task");
  assert.equal(first.settings.provider, null);
  assert.equal(first.settings.profile_access, true);
  assert.equal(fs.existsSync(path.join(home, "provider-calls.jsonl")), false);
  const client = first.observed.clients[0];
  assert.equal(client.client, "codex");
  assert.equal(client.mcp.status, "configured");
  assert.equal(client.workflow.clients[0].parity, true);
  assert.equal(client.hooks.capture.enabled, true);
  assert.equal(client.hooks.activation.status, "not_observed");
  assert.equal(client.hooks.mining.status, "configured");
  assert.equal(client.hooks.mining.execution, "in_session");
  assert.equal(client.profile_access.allowed, true);
  assert.equal(first.steps.find(step => step.component === "profile.git").detail.status, "refreshed");
  assertCurrentIndex(first.observed.project.index);
  const manifestPath = path.join(home, ".codex", ".mastermind-workflow.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  assert.equal(manifest.profile, "core");
  assert.deepEqual([...manifest.artifacts.skills].sort(), [...CORE].sort());
  for (const skill of CORE) assert.equal(fs.readFileSync(path.join(home, ".codex", "skills", skill, "SKILL.md"), "utf8"),
    fs.readFileSync(path.join(share, "skills", skill, "SKILL.md"), "utf8"));
  const keep = [path.join(root, ".mastermind", "setup.json"), path.join(root, ".codex", "hooks.json"),
    manifestPath, path.join(home, ".codex", "config.toml")];
  const before = keep.map((file) => fs.readFileSync(file));
  const userSkill = path.join(home, ".codex", "skills", "personal", "SKILL.md");
  fs.mkdirSync(path.dirname(userSkill));
  fs.writeFileSync(userSkill, "User-owned skill.\n");
  const context = "# Context\n\nUser changed the project description.\n";
  fs.writeFileSync(path.join(root, "CONTEXT.md"), context);
  const repeated = mastermind(["init", "--json"]);
  assert.equal(repeated.status, "configured");
  assert.deepEqual(repeated.settings, first.settings);
  assert.equal(repeated.observed.clients[0].hooks.capture.generation, client.hooks.capture.generation);
  assert.equal(fs.readFileSync(userSkill, "utf8"), "User-owned skill.\n");
  assert.equal(fs.readFileSync(path.join(root, "CONTEXT.md"), "utf8"), context);
  assert.deepEqual(keep.map((file) => fs.readFileSync(file)), before);
  const callsPath = path.join(home, "native-calls.jsonl");
  const calls = fs.readFileSync(callsPath, "utf8");
  const status = mastermind(["status", "--json"], { FIXTURE_FORBID_NATIVE: "1" });
  assert.equal(status.inspection, "read_only");
  assert.deepEqual(status.settings, first.settings);
  assert.equal(status.clients[0].mcp.status, "configured");
  assert.equal(status.clients[0].mcp.scope, "user_configuration_only");
  assert.equal(status.clients[0].workflow.clients[0].parity, true);
  assert.equal(status.clients[0].hooks.capture.generation, client.hooks.capture.generation);
  assert.equal(status.clients[0].profile_access.allowed, true);
  assertCurrentIndex(status.project.index);
  assert.equal(fs.readFileSync(callsPath, "utf8"), calls);
  assert.deepEqual(keep.map((file) => fs.readFileSync(file)), before);
  fs.writeFileSync(manifestPath, JSON.stringify({ ...manifest, version: "0.0.0" }));
  const oldManifest = fs.readFileSync(manifestPath);
  const outdated = mastermind(["status", "--json"], { FIXTURE_FORBID_NATIVE: "1" });
  assert.equal(outdated.clients[0].workflow.status, "missing_or_stale");
  assert.equal(outdated.clients[0].workflow.clients[0].installed_version, "0.0.0");
  assert.equal(outdated.clients[0].workflow.clients[0].parity, false);
  assert.equal(outdated.clients[0].workflow.next_action, "mastermind update --workflow-only --client codex");
  const outdatedText = run(process.execPath, [launcher, "status"], { FIXTURE_FORBID_NATIVE: "1" }).stdout;
  assert.ok(outdatedText.includes("workflow missing_or_stale"), outdatedText);
  assert.ok(outdatedText.includes("mastermind update --workflow-only --client codex"), outdatedText);
  assert.deepEqual(fs.readFileSync(manifestPath), oldManifest);
  assert.equal(fs.readFileSync(callsPath, "utf8"), calls);
  mastermind(["update", "--workflow-only", "--json"]);
  const currentWorkflow = mastermind(["status", "--json"], { FIXTURE_FORBID_NATIVE: "1" }).clients[0].workflow;
  assert.equal(currentWorkflow.status, "configured");
  assert.equal(currentWorkflow.clients[0].installed_version, pkg.version);
  assert.equal(currentWorkflow.clients[0].parity, true);
  assert.equal(currentWorkflow.next_action, undefined);
  const hooks = JSON.parse(fs.readFileSync(path.join(root, ".codex", "hooks.json"), "utf8"));
  assert.ok(hooks.hooks.SessionStart[0].hooks[0].command.includes(launcher));
  // Package replacement must reach existing hooks without another init.
  const nativePath = path.join(nativePackage, "bin", "mmcg");
  fs.unlinkSync(nativePath);
  fs.writeFileSync(nativePath, `#!/bin/sh\nprintf updated > ${quote(path.join(home, "hook-runtime"))}\nexec ${quote(fs.realpathSync(NATIVE))} "$@"\n`, {mode:0o700});
  const event = (kind, turn, extra = {}, project = root) => {
    const definitions = project === root ? hooks : JSON.parse(fs.readFileSync(path.join(project, ".codex", "hooks.json"), "utf8"));
    const command = definitions.hooks[kind][0].hooks[0].command;
    const output = spawnSync("/bin/sh", ["-c", command], { cwd: project, env,
      input: JSON.stringify({hook_event_name:kind, session_id:"native-session", cwd:project,
        ...(turn ? {turn_id:turn} : {}), ...extra}), encoding:"utf8", timeout:10_000 });
    assert.equal(output.status, 0, output.stderr);
    return JSON.parse(output.stdout);
  };
  event("SessionStart", null, {source:"startup", model:"gpt-task-model"});
  assert.equal(fs.readFileSync(path.join(home, "hook-runtime"), "utf8"), "updated");
  const firstPrompt = event("UserPromptSubmit", "one", {prompt:"Review module.py. I prefer short reviews only for simple changes."});
  assert.match(firstPrompt.hookSpecificOutput.additionalContext, /Mastermind task profile/);
  const ticket = firstPrompt.hookSpecificOutput.additionalContext.match(/ticket_id="([a-f0-9]{64})"/)[1];
  const messages = [
    {jsonrpc:"2.0", id:0, method:"initialize", params:{protocolVersion:"2025-11-25", capabilities:{}, clientInfo:{name:"fixture",version:"1"}}},
    {jsonrpc:"2.0", method:"notifications/initialized"},
    {jsonrpc:"2.0", id:1, method:"tools/call", params:{name:"mmcg_mining_submit", arguments:{ticket_id:ticket, candidates:[
      {when:"Reviewing simple changes", behavior:"Keep the review concise", exception:"Only for simple changes", evidence_kind:"review_preference", quote:"I prefer short reviews only for simple changes."}
    ]}}},
  ];
  const submitted = spawnSync(process.execPath, [launcher,"serve"], {cwd:root, env:{...env,MMCG_PROFILE_CLIENT:"codex"},
    input:messages.map(JSON.stringify).join("\n")+"\n", encoding:"utf8", timeout:10_000});
  assert.equal(submitted.status, 0, submitted.stderr);
  const receipt = submitted.stdout.trim().split("\n").map(JSON.parse).find(value => value.id === 1);
  assert.equal(receipt.error, undefined, JSON.stringify(receipt));
  assert.notEqual(receipt.result.isError, true, JSON.stringify(receipt));
  event("Stop", "one", {last_assistant_message:"Reviewed module.py."});
  const deadline = Date.now() + 15_000;
  let live;
  do {
    live = mastermind(["status", "--json"], {FIXTURE_FORBID_NATIVE:"1"});
    if (live.clients[0].hooks.pipeline.local_analysis.completed_revisions > 0) break;
    await pause(100);
  } while (Date.now() < deadline);
  assert.equal(live.clients[0].hooks.mining.execution, "in_session");
  assert.equal(fs.existsSync(path.join(home, "provider-calls.jsonl")), false);
  const episode = mastermind(["miner","hooks","episodes"]).episodes[0].id;
  const analysis = mastermind(["miner","hooks","show",episode]).analyses.find(value => value.processor.engine === "current_task_agent");
  assert.equal(analysis.current, true);
  assert.equal(analysis.processor.model, "gpt-task-model");
  assert.ok(live.clients[0].hooks.pipeline.local_analysis.completed_revisions > 0);
  const secondPrompt = event("UserPromptSubmit", "two", {prompt:"Check module.py again."});
  assert.match(secondPrompt.hookSpecificOutput.additionalContext, /Mastermind task profile/);
  const other = path.join(temporary, "second-project");
  fs.mkdirSync(other);
  fs.writeFileSync(path.join(other, "module.py"), "def other():\n    return True\n");
  const second = mastermind(["init", other, "--json"]);
  assert.equal(second.status, "configured");
  assert.equal(second.settings.profile_access, true);
  event("SessionStart", null, {source:"startup", model:"gpt-other-model"}, other);
  const crossRepo = event("UserPromptSubmit", "one", {prompt:"Check module.py."}, other);
  assert.match(crossRepo.hookSpecificOutput.additionalContext, /Mastermind task profile/);
  const stopped = spawnSync(process.execPath, [launcher, "miner", "stop", other, "--json"], {cwd:other, env, timeout:10_000});
  assert.equal(stopped.status, 0);
});
