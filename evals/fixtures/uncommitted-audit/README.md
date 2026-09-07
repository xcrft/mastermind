# Uncommitted audit scope

This fixture audits the state before an executor commit. `src/staged.py` changes
in the index, `src/unstaged.py` changes only in the working tree, and the new
`src/added.py` stays untracked. All three are declared in the executor report.

The `staged_paths` case field leaves HEAD at the baseline and stages only its
listed paths. Comparing baseline to HEAD alone finds none of these changes.
The auditor must combine the baseline-to-working-tree diff with the untracked
file inventory, then read the actual file contents.
