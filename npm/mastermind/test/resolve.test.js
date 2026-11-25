import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import test from "node:test";

const RESOLVE = pathToFileURL(path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "bin", "resolve.js"));

// The child prints its pid once its handlers are installed, then waits. With a
// handler it exits with the given code, without one it dies by the signal.
function startWrapper(t, signal, exitCode) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "mastermind-resolve-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const child = path.join(directory, "child.cjs");
  fs.writeFileSync(child, `
${exitCode === null ? "" : `process.on(${JSON.stringify(signal)}, () => process.exit(${exitCode}));`}
process.stdout.write(process.pid + "\\n");
setInterval(() => {}, 1000);
`);
  const wrapper = spawn(process.execPath, [
    "--input-type=module", "-e",
    `import(${JSON.stringify(RESOLVE.href)}).then((m) => m.runBinary(process.execPath, [${JSON.stringify(child)}]));`,
  ], { stdio: ["ignore", "pipe", "inherit"] });
  let childPid = null;
  t.after(() => {
    wrapper.kill("SIGKILL");
    if (childPid !== null) {
      try { process.kill(childPid, "SIGKILL"); } catch {}
    }
  });
  const ready = new Promise((resolve, reject) => {
    let output = "";
    wrapper.stdout.on("data", (chunk) => {
      output += chunk;
      if (output.includes("\n")) {
        childPid = Number(output.trim());
        resolve();
      }
    });
    wrapper.on("exit", () => reject(new Error("wrapper exited before the child was ready")));
  });
  const exited = new Promise((resolve) => {
    wrapper.on("exit", (code, received) => resolve({ code, signal: received }));
  });
  return { wrapper, ready, exited, childPid: () => childPid };
}

function childAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

for (const [index, signal] of ["SIGTERM", "SIGINT", "SIGHUP"].entries()) {
  test(`the wrapper forwards ${signal} and exits with the child's status`,
    { skip: process.platform === "win32" }, async (t) => {
      const exitCode = 40 + index;
      const run = startWrapper(t, signal, exitCode);
      await run.ready;
      run.wrapper.kill(signal);
      assert.deepEqual(await run.exited, { code: exitCode, signal: null });
    });
}

test("the wrapper dies by the signal that killed the child",
  { skip: process.platform === "win32" }, async (t) => {
    const run = startWrapper(t, "SIGTERM", null);
    await run.ready;
    run.wrapper.kill("SIGTERM");
    assert.deepEqual(await run.exited, { code: null, signal: "SIGTERM" });
    assert.equal(childAlive(run.childPid()), false, "the child must not outlive the wrapper");
  });
