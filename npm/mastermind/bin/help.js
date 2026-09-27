// This module deliberately imports no installer, resolver or package metadata.
// Showing wrapper help needs only the argument list and an output writer.

const TOP_LEVEL = `Mastermind — project setup and evidence-backed AI workflows

Usage: mastermind <command> [options]

Start here:
  init       Configure this project and its AI client
  update     Update the npm package and installed workflows
  status     Inspect project readiness and the next step

Project tools:
  index      Build or refresh the local code and documentation index
  miner      Collect and review profile evidence
  ui         Open the local read-only Lens interface
  doctor     Diagnose project setup and index problems

Advanced:
  install             Install workflows and register MCP directly
  list                List bundled skills and Claude subagents
  doctor --workflow   Check installed workflow files against the bundle
  mmcg --help         Show the full native command catalog

Use mastermind <command> --help for command options.
`;

const INSTALL = `Usage: mastermind install [options]

Install bundled workflow files and register MCP for the selected client.
Use mastermind init for project onboarding.

Options:
  --client claude|codex|all                 Client selection (default: claude)
  --profile core|frontend|security|full     Preserve the installed profile by default
  -h, --help                              Show this help

A new client installation uses the core profile unless selected explicitly.
`;

const UPDATE = `Usage: mastermind update [options]

Update the npm package, then refresh installed workflow files.
Uses the detected global or project npm scope. Other installs receive manual instructions.

Options:
  --client claude|codex|all                 Client selection (default: installed clients)
  --profile core|frontend|security|full     Preserve each installed profile by default
  --dry-run                              Show a local plan without changes or network calls
  --workflow-only                        Refresh workflows from the current package
  --json                                 Print structured results
  -h, --help                              Show this help

Workflow-only mode leaves the npm package and native binary unchanged.
Locally modified workflow files block overwrite.
`;

const LIST = `Usage: mastermind list [options]

List the bundled skills and Claude subagents without installing them.

Options:
  --profile core|frontend|security|full     Bundle selection (default: core)
  -h, --help                              Show this help
`;

const WORKFLOW_DOCTOR = `Usage: mastermind doctor --workflow [options]

Read-only comparison of installed workflow files with the current package bundle.
Use mastermind doctor --help for project and index diagnostics.

Options:
  --client claude|codex|all                 Client selection (default: claude)
  --profile core|frontend|security|full     Compare the installed profile by default
  --json                                 Print structured results
  -h, --help                              Show this help

The check exits with status 1 when an installation is missing, invalid or drifted.
`;

/**
 * Render wrapper-owned help from argv without the node/script prefix.
 * Return null for execution or native command help so the caller can delegate.
 * This function performs no filesystem, environment, process or network I/O.
 */
export function renderHelp(argv) {
  if (argv.length === 0 || (argv.length === 1 && ["--help", "-h", "help"].includes(argv[0]))) {
    return TOP_LEVEL;
  }

  const helpVerb = argv[0] === "help";
  const [command, ...rest] = helpVerb ? argv.slice(1) : argv;
  const separator = rest.indexOf("--");
  const options = separator === -1 ? rest : rest.slice(0, separator);
  if (!helpVerb && !options.some((arg) => arg === "--help" || arg === "-h")) return null;

  switch (command) {
    case "install": return INSTALL;
    case "update": return UPDATE;
    case "list": return LIST;
    case "doctor": return options.includes("--workflow") ? WORKFLOW_DOCTOR : null;
    default: return null;
  }
}

/**
 * Call before binary resolution or installer dispatch. True means help was
 * printed and the wrapper should exit successfully. False means continue.
 * A custom writer receives one complete string including the final newline.
 */
export function printHelp(argv, write = (text) => process.stdout.write(text)) {
  const help = renderHelp(argv);
  if (help === null) return false;
  write(help);
  return true;
}
