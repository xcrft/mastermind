"""python -m evals.selection [--write] [--verbose]"""

from __future__ import annotations

import argparse
import sys

from evals import selection


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(prog="python -m evals.selection", description=__doc__)
    parser.add_argument("--write", action="store_true", help="regenerate the catalogue from the skill descriptions")
    parser.add_argument("--verbose", action="store_true", help="list declared ties and trigger overlaps")
    args = parser.parse_args(argv)

    if args.write:
        selection.CATALOGUE.write_text(selection.render_catalogue(selection.load_skills()), encoding="utf-8")

    report = selection.check()
    cases = selection.load_cases()
    if args.verbose:
        for score, left, right, shared in report.overlaps:
            print(f"overlap {score:.2f} {left} / {right}: {shared}")
        for case in report.ties:
            print(f"tie {case.id}: {', '.join(sorted(case.accept))}")
    for note in report.notes:
        print(f"note: {note}")
    for problem in report.problems:
        print(f"error: {problem}")
    print(
        f"Checked {len(cases)} selection prompts: {len(report.problems)} error(s), "
        f"{len(report.ties)} declared tie(s) the descriptions do not settle."
    )
    return 1 if report.problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
