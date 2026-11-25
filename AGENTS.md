# Engineering rules

- Keep code in packages by responsibility. Put shared code in `shared` only when multiple packages use the same contract.
- Use SOLID, DRY, KISS and YAGNI to guide changes. Prefer a small function or module over an interface with one implementation.
- Fix the owning implementation. Update callers, commands, CI and documentation together when moving code.
- Put Python tests in `tests/`, mirroring the runtime packages. Keep reusable fixtures in `tests/evals/support/`; do not instantiate another test class for setup.
- Test observable behavior and failure boundaries. Avoid duplicate cases and assertions that only mirror implementation details.
- Synchronize subprocess tests on readiness. Use controlled clocks for deadline logic and state polling for cleanup. Do not assert machine-dependent execution speed.
- `just check` is the gate before every push. It runs fmt, clippy, cargo tests, the validator, the npm and Lens tests, the Python suite with the corpus check, and cargo deny. For Python-only changes, `python3 -m unittest discover -s tests -t .` and `python3 scripts/validate.py` are enough while iterating. Model-backed experiments are separate from deterministic tests.
- Run the gate outside the Bash sandbox. Lens and invocation tests bind local ports and manage process groups, so the sandbox reports false failures. Give each parallel stream of work its own `CARGO_TARGET_DIR`.
- Local clippy covers only the host target. Windows and Linux lints run in CI, so code under `#[cfg(unix)]` or `#[cfg(windows)]` needs a look at the imports it shares with other platforms.
- Write READMEs and runbooks as instructions: purpose, prerequisites, commands, inputs, outputs and recovery. Use tables for choices and contracts; keep historical measurements in retained reports.
- Comments explain a constraint or decision. Do not put task labels or ticket identifiers in comments.
- Preserve source bindings, failure denominators, unknown measurements and immutable result records. Passing transport or harness checks does not establish semantic quality.
