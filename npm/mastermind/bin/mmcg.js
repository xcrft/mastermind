#!/usr/bin/env node
// `mmcg` — compatibility command. Resolves the platform-specific binary from
// the optional dependency installed by npm and execs it with argv pass-through.
//
// Most users should run `mastermind` (the public command). This file exists so
// scripts written against the cargo-installed `mmcg` keep working after npm
// adoption — same binary, same subcommands, same exit codes.

import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import process from "node:process";

const require = createRequire(import.meta.url);
const pkg = require("../package.json");

/**
 * Detect glibc vs musl on Linux. `process.report.getReport()` includes
 * `glibcVersionRuntime` when the host libc is glibc; absent on musl/Alpine.
 * Node 18+ ships `process.report` enabled by default.
 *
 * Returns "gnu" on glibc hosts, "musl" otherwise. Conservative fallback to
 * musl when detection is ambiguous (better to fail at require.resolve than
 * silently grab the wrong binary).
 */
function detectLibc() {
  if (process.platform !== "linux") return null;
  const report = process.report?.getReport?.();
  if (report?.header?.glibcVersionRuntime) return "gnu";
  return "musl";
}

/** Map (platform, arch[, libc]) → the npm scoped package name. */
function packageName() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "arm64") return "@xcrft/mmcg-darwin-arm64";
  if (platform === "darwin" && arch === "x64") return "@xcrft/mmcg-darwin-x64";
  if (platform === "win32" && arch === "x64") return "@xcrft/mmcg-win32-x64-msvc";

  if (platform === "linux") {
    const libc = detectLibc();
    if (arch === "x64") return `@xcrft/mmcg-linux-x64-${libc}`;
    if (arch === "arm64") return `@xcrft/mmcg-linux-arm64-${libc}`;
  }

  throw new Error(`unsupported platform: ${platform}-${arch}`);
}

function resolveBinary() {
  const pkgName = packageName();
  const exe = process.platform === "win32" ? "mmcg.exe" : "mmcg";
  try {
    return require.resolve(`${pkgName}/bin/${exe}`);
  } catch (err) {
    const lines = [
      `Could not locate the native mmcg binary for ${process.platform}-${process.arch}.`,
      "",
      "The platform-specific package was not installed. This usually means:",
      `  - npm skipped \`${pkgName}\` because optional-dependency install failed`,
      "  - your platform is not in the supported set (see README)",
      "",
      "Fixes:",
      "  - npm install --include=optional @xcrft/mastermind     # force optional install",
      "  - cargo install mmcg                                   # build from source (needs Rust)",
      "",
      `(underlying error: ${String(err?.message ?? err)})`,
    ];
    console.error(lines.join("\n"));
    process.exit(1);
  }
}

const bin = resolveBinary();

// `spawn` (not `spawnSync`) so signals (SIGINT / SIGTERM) propagate to the
// child correctly — important for `mmcg watch` and `mmcg serve` which are
// long-running.
const child = spawn(bin, process.argv.slice(2), {
  stdio: "inherit",
  // Propagate environment unchanged. `mastermind.js` injects extra env vars
  // before spawning this script when needed (install-mode detection).
});

child.on("error", (err) => {
  console.error(`failed to launch mmcg binary at ${bin}: ${err.message}`);
  process.exit(1);
});

child.on("exit", (code, signal) => {
  if (signal) {
    // Re-raise the signal so the parent shell sees the canonical exit code.
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 1);
  }
});

// Re-export the version for diagnostics: `mmcg --version` is the binary's
// version; this is the wrapper / package version. Equal when shipped together.
export const wrapperVersion = pkg.version;
