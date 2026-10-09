"""Campaign read accounting preserves failure denominators and source bindings."""

import copy
import unittest

from evals.benchmark import artifacts, campaign, retrieval_analysis, trials
from evals.benchmark.retrieval import ReadLedger
from tests.evals.support.corpus import CorpusFixture


class RetrievalAnalysisTests(unittest.TestCase):
    def test_complete_campaign_ledger_is_bound_to_retained_result_and_source(self):
        case = CorpusFixture()
        self.addCleanup(case.close)
        fixture = case.fixture
        auth = fixture.root / "auth"
        auth.mkdir()
        cli = fixture.executable("fake-codex", '''
import json, pathlib, sys
if sys.argv[1:] == ['--version']:
    print('codex-cli 0.160.1')
    sys.exit(0)
sys.stdin.read()
body = {'path':'src/service.py', 'start_line':1, 'end_line':2, 'total_lines':2,
        'lines':[{'line':i+1, 'text':text} for i,text in enumerate(pathlib.Path('src/service.py').read_text().splitlines())]}
item = {'type':'mcp_tool_call', 'server':'research', 'tool':'source_read', 'id':'read-1',
        'arguments':{'path':'src/service.py', 'start_line':1, 'end_line':2},
        'result':{'structured_content':body}, 'status':'completed', 'error':None}
for event in [{'type':'thread.started'}, {'type':'turn.started'},
              {'type':'item.completed','item':item},
              {'type':'item.completed','item':{'type':'agent_message','text':'Observed src/service.py:2.'}},
              {'type':'turn.completed','usage':{'input_tokens':10,'cached_input_tokens':0,'cache_write_input_tokens':0,'output_tokens':2}}]:
    print(json.dumps(event), flush=True)
''')
        cli["version"] = "0.160.1"
        fixture.config["adapter"] = {"kind": "codex_cli", "cli": cli, "auth_home": str(auth), "reasoning_effort": "max"}
        output = fixture.root / "campaign"
        campaign.prepare(corpus_path=case.path, config=fixture.config,
            source_repo=fixture.repo, tool_repo=fixture.repo, output=output, repetitions=1)
        campaign.run(output, progress=lambda *args, **kwargs: None)
        result = retrieval_analysis.summarize_campaign(output)
        self.assertEqual(result["planned_attempts"], 3)
        for condition in result["by_condition"].values():
            metric = condition["metrics"]["returned_lines"]
            self.assertEqual((metric["total"], metric["unknown_attempts"]), (2, 0))
        self.assertEqual([item["source_integrity"] for item in result["attempts"]], ["verified"] * 3)
        self.assertTrue(all(item["result_sha256"] and item["manifest_sha256"] for item in result["attempts"]))

    def test_unrun_and_legacy_attempts_remain_unknown_in_campaign_totals(self):
        case = CorpusFixture()
        self.addCleanup(case.close)
        fixture = case.fixture
        output = fixture.root / "campaign"
        campaign.prepare(corpus_path=case.path, config=fixture.config,
            source_repo=fixture.repo, tool_repo=fixture.repo, output=output, repetitions=1)
        _, batches = campaign.load(output)
        _, batch, plan = batches[0]
        trials.run_trial(batch / plan["trials"][0]["directory"])
        result = retrieval_analysis.summarize_campaign(output)
        self.assertEqual(result["planned_attempts"], 3)
        self.assertEqual([row["status"] for row in result["attempts"]], ["completed", "not_run", "not_run"])
        for condition in result["by_condition"].values():
            metric = condition["metrics"]["returned_lines"]
            self.assertEqual((metric["total"], metric["observed_total"], metric["unknown_attempts"]), (None, None, 1))
        self.assertTrue(result["attempts"][0]["result_sha256"])
        self.assertIsNone(result["quality_uplift"])

    def test_failed_and_incomplete_reads_keep_partial_counts_without_claiming_complete_ranges(self):
        complete = ReadLedger([]).report()
        partial = dict(complete, read_calls=2, failed_reads=1, unverifiable_reads=1,
                       range_accounting_complete=False)
        rows = [{"condition": "routed", "status": status, "source_integrity": "verified",
                 "answer_sha256": None, "read_ledger": ledger}
                for status, ledger in (("completed", complete), ("completed", partial),
                                       ("invocation_error", complete))]
        value = retrieval_analysis.summarize(rows, ["routed"])["routed"]
        self.assertEqual(value["attempts"]["failed"], 1)
        self.assertEqual(value["metrics"]["read_calls"]["observed_total"], 2)
        self.assertIsNone(value["metrics"]["read_calls"]["total"])
        self.assertEqual(value["metrics"]["read_calls"]["unknown_attempts"], 1)
        self.assertEqual(value["metrics"]["returned_lines"]["unknown_attempts"], 2)
        known = retrieval_analysis.summarize(rows[:1], ["routed"])["routed"]["metrics"]
        self.assertEqual(known["returned_lines"]["total"], 0)

    def test_invalid_range_counts_and_changed_source_binding_are_rejected(self):
        sources = [{"path": "src/value.py", "sha256": "a" * 64, "lines": 10}]
        ledger = ReadLedger(sources).report()
        ledger.update(read_calls=1, returned_lines=5, unique_returned_lines=5,
                      sources=[{"path": "src/value.py", "source_sha256": "a" * 64, "ranges": [[1, 5]]}])
        self.assertEqual(retrieval_analysis.validate_ledger(ledger, sources)["unique_returned_lines"], 5)
        mutations = [dict(schema_version=True), dict(returned_lines=True), dict(repeated_lines=1),
                     dict(range_accounting_complete=False), dict(failed_reads=2),
                     dict(read_calls=0), dict(returned_lines=201, repeated_lines=196),
                     dict(unique_returned_lines=4, repeated_lines=1)]
        for changes in mutations:
            with self.subTest(changes=changes), self.assertRaises(artifacts.BenchmarkError):
                retrieval_analysis.validate_ledger(dict(ledger, **changes), sources)
        for field, value in (("source_sha256", "b" * 64), ("path", "private.py"),
                             ("ranges", [[1, 11]]), ("ranges", [[1, 3], [3, 4]])):
            modified = copy.deepcopy(ledger)
            modified["sources"][0][field] = value
            with self.subTest(field=field, value=value), self.assertRaises(artifacts.BenchmarkError):
                retrieval_analysis.validate_ledger(modified, sources)
