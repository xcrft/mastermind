# Uncommitted audit scope

This case leaves `src/staged.py` staged, `src/unstaged.py` modified in the
working tree, and `src/added.py` untracked. The executor report declares all
three.

Comparing the baseline to HEAD alone misses them. The auditor must inspect
the working-tree diff and untracked files.
