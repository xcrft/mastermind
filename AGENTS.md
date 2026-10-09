# Engineering rules

- Keep code in packages by responsibility. Put shared code in `shared` only when multiple packages use the same contract.
- Use SOLID, DRY, KISS and YAGNI to guide changes. Prefer a small function or module over an interface with one implementation.
- Fix the owning implementation. Update callers, commands, CI and documentation together when moving code.
- Put Python tests in `tests/`, mirroring the runtime packages. Keep reusable fixtures in `tests/evals/support/`; do not instantiate another test class for setup.
- Test observable behavior and failure boundaries. Avoid duplicate cases and assertions that only mirror implementation details.
- Synchronize subprocess tests on readiness. Use controlled clocks for deadline logic and state polling for cleanup. Do not assert machine-dependent execution speed.
- Run `python3 -m unittest discover -s tests -t .` and `python3 scripts/validate.py` for Python harness changes. Model-backed experiments are separate from deterministic tests.
- Write READMEs and runbooks as instructions: purpose, prerequisites, commands, inputs, outputs and recovery. Use tables for choices and contracts; keep historical measurements in retained reports.
- Comments explain a constraint or decision. Do not put task labels or ticket identifiers in comments.
- Preserve source bindings, failure denominators, unknown measurements and immutable result records. Passing transport or harness checks does not establish semantic quality.
