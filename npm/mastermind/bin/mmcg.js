#!/usr/bin/env node
// `mmcg` — compatibility command. Resolves the platform-specific binary from
// the optional dependency installed by npm and execs it with argv pass-through.
//
// Most users should run `mastermind` (the public command). This file exists so
// scripts written against the cargo-installed `mmcg` keep working after npm
// adoption — same binary, same subcommands, same exit codes. Environment is
// propagated unchanged; `mastermind.js` is the wrapper that injects hints.

import process from "node:process";
import { resolveBinary, runBinary } from "./resolve.js";

runBinary(resolveBinary(), process.argv.slice(2));
