"""Effort routing stays request-bound and cannot hide runtime differences."""

import copy
import unittest

from evals.benchmark import artifacts, campaign, conditions, effort, protocol
from tests.evals.support.corpus import CorpusFixture


class EffortTests(unittest.TestCase):
    def test_unknown_scope_and_risk_fall_back_without_using_the_key(self):
        task = {"kind": "research", "question": "Explain the returned result.",
                "source_allowlist": ["src/service.py"],
                "output_contract": "Read source only; do not execute the researched code."}
        self.assertEqual(effort.select(task)["reasoning_effort"], "high")
        for change in ({"question": "Explain state changes."}, {"question": "Review removal evidence."},
                       {"question": "Check permission handling."}, {"kind": "fix"},
                       {"output_contract": "Run and change the code."},
                       {"source_allowlist": ["a", "b", "c", "d"]}):
            with self.subTest(change=change):
                self.assertEqual(effort.select(dict(task, **change))["reasoning_effort"], "max")

    def test_invalid_or_partial_effort_declarations_are_rejected(self):
        matrix = [{"id": name, "tools": "source", "instruction_paths": [], "reasoning_effort": "max"}
                  for name in ("current", "adaptive")]
        config = {"adapter": {"kind": "codex_cli"}, "conditions": matrix,
                  "effort_policy": {"condition": "adaptive", "policy": effort.POLICY}}
        for mutation in ("missing", "invalid", "unknown_policy", "unknown_condition", "other_adapter"):
            value = copy.deepcopy(config)
            if mutation == "missing":
                del value["conditions"][1]["reasoning_effort"]
            elif mutation == "invalid":
                value["conditions"][1]["reasoning_effort"] = "automatic"
            elif mutation == "unknown_policy":
                value["effort_policy"]["policy"] = "unfrozen"
            elif mutation == "unknown_condition":
                value["effort_policy"]["condition"] = "absent"
            else:
                value["adapter"]["kind"] = "claude_cli"
            with self.subTest(mutation=mutation), self.assertRaises(artifacts.BenchmarkError):
                effort.configure({}, value)
        self.assertEqual(conditions.validate_conditions(matrix), matrix)

    def test_campaign_delivers_declared_effort_and_preserves_other_identity_guards(self):
        case = CorpusFixture()
        self.addCleanup(case.close)
        fixture = case.fixture
        case.task["output_contract"] = "Read source only; do not execute the researched code."
        case.save()
        auth = fixture.root / "auth"
        auth.mkdir()
        cli = fixture.executable("codex", '''
import json, sys
if sys.argv[1:] == ['--version']:
    print('codex-cli fixture')
    sys.exit(0)
effort = next(value.split('=', 1)[1] for value in sys.argv if value.startswith('model_reasoning_effort='))
assert 'FIXTURE_PRIVATE_KEY_CANARY' not in sys.stdin.read()
for event in [{'type':'thread.started'}, {'type':'turn.started'},
    {'type':'item.completed','item':{'type':'agent_message','text':'Observed src/service.py:2. Effort '+effort}},
    {'type':'turn.completed','usage':{'input_tokens':10,'cached_input_tokens':0,'cache_write_input_tokens':0,'output_tokens':2}}]:
    print(json.dumps(event), flush=True)
''')
        cli['version'] = 'fixture'
        fixture.config['adapter'] = {'kind': 'codex_cli', 'cli': cli, 'auth_home': str(auth), 'reasoning_effort': 'max'}
        fixture.config['limits'].update(trace_bytes=16777216)
        fixture.config['conditions'] = [{"id": name, "tools": "source", "instruction_paths": [], "role": "researcher", "reasoning_effort": "max"}
                                        for name in ('current', 'adaptive')]
        fixture.config['calibration'] = {'axis': 'effort'}
        fixture.config['effort_policy'] = {'condition': 'adaptive', 'policy': effort.POLICY}
        original = copy.deepcopy(fixture.config)
        output = fixture.root / 'campaign'
        plan = campaign.prepare(corpus_path=case.path, config=fixture.config, source_repo=fixture.repo,
                                tool_repo=fixture.repo, output=output, repetitions=1)
        self.assertEqual(fixture.config, original)
        self.assertEqual(plan['cases'][0]['effort_routing']['task_sha256'], artifacts.digest(case.task))
        self.assertEqual(plan['cases'][0]['effort_routing']['reasoning_effort'], 'high')
        self.assertEqual(plan['calibration'], {'axis': 'effort'})
        _, batches = campaign.load(output)
        _, batch, inventory = batches[0]
        self.assertEqual(len({row['common_sha256'] for row in inventory['trials']}), 1)
        campaign.run(output, progress=lambda *args, **kwargs: None)
        for slot, expected in zip(inventory['trials'], ('max', 'high')):
            trial = batch / slot['directory']
            manifest = artifacts.load_json(trial / 'manifest.json')
            self.assertIn('Effort "'+expected+'"', (trial / 'answer.md').read_text())
            changed = copy.deepcopy(manifest)
            changed['adapter']['settings']['reasoning_effort'] = 'low'
            with self.assertRaises(artifacts.BenchmarkError) as caught:
                protocol.common_identity(changed)
            self.assertEqual(caught.exception.code, 'condition_runtime_mismatch')
            changed = copy.deepcopy(manifest)
            changed['adapter']['settings']['auth_home'] += '-changed'
            self.assertNotEqual(artifacts.digest(protocol.common_identity(changed)), manifest['common_sha256'])
        exported = fixture.root / 'reviews'
        campaign.export(output, exported)
        compared = campaign.compare(exported, 'current', 'adaptive')
        self.assertEqual(artifacts.load_json(exported / 'review-set.json')['campaign']['calibration'], {'axis': 'effort'})
        self.assertEqual(compared['planned_attempts'], 2)
        self.assertEqual(compared['cases'][0]['attempts']['completed'], 2)
        self.assertEqual(compared['efficiency'][0]['by_condition']['adaptive']['successful_outcomes'],
                         {'lower': 0, 'upper': 1})
