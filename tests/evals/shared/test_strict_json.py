"""Strict JSON parsing shared by the benchmark and hook-intake harnesses."""

import unittest

from evals.benchmark import artifacts
from evals.intake import harness as intake
from evals.shared.strict_json import strict_json


class StrictJsonTests(unittest.TestCase):
    def test_both_harnesses_refuse_ambiguous_or_nonfinite_documents(self):
        for parse in (strict_json, artifacts.parse_json, intake.strict_json):
            for body in (b'{"a":1,"a":2}', b'{"a":NaN}', b'{"a":-Infinity}', b"[" * 100_000):
                with self.subTest(parse=parse.__qualname__, body=body[:16]):
                    with self.assertRaises(ValueError):
                        parse(body)

    def test_intake_harness_refuses_a_byte_order_mark(self):
        with self.assertRaises(ValueError):
            intake.strict_json('﻿{"a":1}'.encode("utf-8"))


if __name__ == "__main__":
    unittest.main()
