"""Exercise the actual workflow expression across trust and capacity boundaries.
trace:STORY-2672 | ai:codex
"""
import itertools
import json
from pathlib import Path
from types import SimpleNamespace
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]


class Routing(unittest.TestCase):
    def test_actual_expression_routes_only_opted_in_trusted_heavy_shards(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = workflow['jobs']['build']
        expression = build['runs-on'].removeprefix('${{').removesuffix('}}').strip()
        # Translate the closed expression subset used here to Python. Strings and
        # non-empty arrays in this expression have the same Actions truthiness.
        cases = []
        for enabled, event, same, shard in itertools.product(
                ('', 'false', 'true'),
                ('push', 'pull_request', 'merge_group', 'workflow_dispatch', 'pull_request_target'),
                (False, True), build['strategy']['matrix']['shard']):
            context = dict(vars=dict(AIDA_DELL1_CI=enabled),
                           github=dict(event_name=event, repository='joemooney/aida',
                                       event=dict(pull_request=dict(head=dict(repo=dict(
                                           full_name='joemooney/aida' if same else 'fork/aida'))))),
                           matrix=dict(shard=shard, os='ubuntu-latest'))
            expected = (enabled == 'true' and
                        (event == 'push' or (event == 'pull_request' and same)) and
                        shard in ('lib-0', 'lib-1', 'cli-0', 'cli-1'))
            cases.append([context, expected])
        def namespace(value):
            return SimpleNamespace(**{key: namespace(item) if isinstance(item, dict) else item
                                      for key, item in value.items()})
        expression = expression.replace('&&', ' and ').replace('||', ' or ')
        for context, expected in cases:
            actual = eval(expression, {'__builtins__': {}, 'fromJSON': json.loads,
                                       'contains': lambda array, value: value in array},
                          vars(namespace(context)))
            wanted = ['self-hosted', 'Linux', 'X64', 'aida-dell1'] if expected else 'ubuntu-latest'
            self.assertEqual(actual, wanted, context)

    def test_host_mutating_setup_excludes_self_hosted(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = workflow['jobs']['build']
        for step in build['steps']:
            if 'sudo ' in step.get('run', ''):
                # gates never route to dell1; shared setup must explicitly refuse it.
                gate_only = "matrix.shard == 'gates'" in step.get('if', '')
                self.assertTrue(gate_only or "runner.environment != 'self-hosted'" in step['if'], step['name'])
        self.assertEqual(workflow['jobs']['build-result']['runs-on'], 'ubuntu-latest')
