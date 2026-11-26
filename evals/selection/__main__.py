"""python -m evals.selection [--write] [--verbose]

Checks the selection case list and catalogue without a model. It does not
measure which skill a model picks; the `selection` behaviour suite does that.
"""

from __future__ import annotations

import argparse
import sys

from evals import selection


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="python -m evals.selection", description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the catalogue from the skill and subagent descriptions")
    parser.add_argument("--verbose", action="store_true", help="list trigger overlaps, ties and cases that share rare words with their label")
    args = parser.parse_args(argv)

    if args.write:
        selection.CATALOGUE.write_text(selection.render_catalogue(selection.load_choices()), encoding="utf-8")

    report = selection.check()
    cases = selection.load_cases()
    if args.verbose:
        for score, left, right, shared in report.overlaps:
            print(f"overlap {score:.2f} {left} / {right}: {shared}")
        for case in report.ties:
            print(f"tie {case.id}: {', '.join(sorted(case.accept))}")
        for case_id, words in report.leaks:
            print(f"rare words shared only with the label {case_id}: {', '.join(words)}")
    for note in report.notes:
        print(f"note: {note}")
    for problem in report.problems:
        print(f"error: {problem}")
    exceptions = sum(bool(case.lexical_exception) for case in cases)
    print(
        f"Checked {len(cases)} selection cases ({report.splits.get('train', 0)} train, "
        f"{report.splits.get('held-out', 0)} held-out, {exceptions} with a reviewed lexical exception): "
        f"{len(report.problems)} error(s). No model was run."
    )
    return 1 if report.problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
