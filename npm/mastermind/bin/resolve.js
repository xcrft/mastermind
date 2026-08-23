import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import process from "node:process";

const require = createRequire(import.meta.url);

// Prefer a failed musl package lookup to silently selecting an incompatible binary.
function detectLibc() {
  if (process.platform !== "linux") return null;
  const report = process.report?.getReport?.();
  if (report?.header?.glibcVersionRuntime) return "gnu";
  return "musl";
}

function packageName() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === "darwin" && arch === "arm64") return "@xcraftmind/mmcg-darwin-arm64";
  if (platform === "darwin" && arch === "x64") return "@xcraftmind/mmcg-darwin-x64";
  if (platform === "win32" && arch === "x64") return "@xcraftmind/mmcg-win32-x64-msvc";

  if (platform === "linux") {
    const libc = detectLibc();
    if (arch === "x64") return `@xcraftmind/mmcg-linux-x64-${libc}`;
    if (arch === "arm64") return `@xcraftmind/mmcg-linux-arm64-${libc}`;
  }

  throw new Error(`unsupported platform: ${platform}-${arch}`);
}

export function resolveBinary() {
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
      "  - npm install --include=optional @xcraftmind/mastermind   # force optional install",
      "  - cargo install mmcg                                      # build from source (needs Rust)",
      "",
      `(underlying error: ${String(err?.message ?? err)})`,
    ];
    console.error(lines.join("\n"));
    process.exit(1);
  }
}

// Long-running watch/serve commands need asynchronous signal propagation.
export function runBinary(bin, args, env) {
  const child = spawn(bin, args, env ? { stdio: "inherit", env } : { stdio: "inherit" });

  child.on("error", (err) => {
    console.error(`failed to launch mmcg binary at ${bin}: ${err.message}`);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exit(code ?? 1);
    }
  });
}
