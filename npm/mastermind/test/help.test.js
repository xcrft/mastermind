import assert from "node:assert/strict";
import test from "node:test";

import { printHelp, renderHelp } from "../bin/help.js";

test("top-level help puts the onboarding commands first and exposes native help", () => {
  const help = renderHelp([]);
  for (const argv of [["--help"], ["-h"], ["help"]]) {
    assert.equal(renderHelp(argv), help);
  }
  assert.match(help, /Usage: mastermind <command> \[options\]/);
  for (const command of ["init", "update", "status", "index", "miner", "ui", "doctor"]) {
    assert.match(help, new RegExp(`^  ${command}\\s`, "m"));
  }
  const primary = ["init", "update", "status"].map((command) => help.indexOf(`  ${command} `));
  assert.ok(primary[0] >= 0 && primary[0] < primary[1] && primary[1] < primary[2]);
  assert.ok(primary[2] < help.indexOf("  index "));
  assert.match(help, /mastermind <command> --help/);
  assert.match(help, /mmcg --help/);
});

test("wrapper commands accept both help spellings and the help verb", () => {
  for (const command of ["install", "update", "list"]) {
    const help = renderHelp([command, "--help"]);
    assert.match(help, new RegExp(`Usage: mastermind ${command}\\b`));
    assert.equal(renderHelp([command, "-h"]), help);
    assert.equal(renderHelp(["help", command]), help);
  }
  const workflow = renderHelp(["doctor", "--workflow", "--help"]);
  assert.match(workflow, /Usage: mastermind doctor --workflow/);
  assert.equal(renderHelp(["doctor", "-h", "--workflow"]), workflow);
  assert.equal(renderHelp(["help", "doctor", "--workflow"]), workflow);
});

test("command help describes supported selection and read-only controls", () => {
  const install = renderHelp(["install", "--help"]);
  assert.match(install, /--client.*claude\|codex\|all/);
  assert.match(install, /--profile.*core\|frontend\|security\|full/);
  assert.match(install, /register MCP/);
  const list = renderHelp(["list", "--help"]);
  assert.match(list, /--profile/);
  assert.doesNotMatch(list, /--json/);
  const doctor = renderHelp(["doctor", "--workflow", "--help"]);
  assert.match(doctor, /--json/);
  assert.match(doctor, /[Rr]ead.only/);
  const update = renderHelp(["update", "--help"]);
  for (const option of ["--client", "--profile", "--dry-run", "--workflow-only", "--json"]) {
    assert.ok(update.includes(option), `${option} must be discoverable`);
  }
});

test("native command help and non-help execution keep their original route", () => {
  for (const argv of [
    ["init", "--help"],
    ["status", "-h"],
    ["miner", "start", "--help"],
    ["doctor", "--help"],
    ["setup", "claude", "--help"],
    ["help", "init"],
    ["help", "miner"],
    ["--index", "other.db", "--help"],
    ["--version"],
    ["unknown-command", "--help"],
    ["install"],
    ["update", "--dry-run"],
    ["list", "--profile", "core"],
    ["doctor", "--workflow", "--json"],
  ]) {
    assert.equal(renderHelp(argv), null, JSON.stringify(argv));
    assert.equal(printHelp(argv, () => assert.fail("unhandled help must not print")), false);
  }
});

test("a literal help token after the option terminator is not a help request", () => {
  for (const argv of [
    ["install", "--", "--help"],
    ["update", "--", "-h"],
    ["doctor", "--help", "--", "--workflow"],
  ]) {
    assert.equal(renderHelp(argv), null, JSON.stringify(argv));
  }
});

test("help does not mutate or echo caller arguments and writes exactly one complete result", () => {
  const argv = Object.freeze(["install", "--profile", "PRIVATE_ARGUMENT", "--help"]);
  const chunks = [];
  assert.equal(printHelp(argv, (text) => chunks.push(text)), true);
  assert.equal(chunks.length, 1);
  assert.equal(chunks[0], renderHelp(["install", "--help"]));
  assert.ok(chunks[0].endsWith("\n"));
  assert.doesNotMatch(chunks[0], /PRIVATE_ARGUMENT/);
  assert.deepEqual(argv, ["install", "--profile", "PRIVATE_ARGUMENT", "--help"]);
});

test("output errors remain visible to the wrapper", () => {
  const error = new Error("output closed");
  assert.throws(() => printHelp(["--help"], () => { throw error; }), (actual) => actual === error);
});
