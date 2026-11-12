"""Exercise CI's actual tier expression, scheduling and required aggregate.
trace:STORY-2735 | ai:codex
"""
import itertools
from pathlib import Path
from types import SimpleNamespace
import subprocess
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
JOBS = WORKFLOW['jobs']


class CiTiers(unittest.TestCase):
    def test_selection_and_check_name_agree_for_all_events(self):
        select = JOBS['tier']['steps'][0]
        expression = select['env']['FULL'].removeprefix('${{').removesuffix('}}').strip()
        name = JOBS['tier']['name']
        self.assertIn(expression, name)
        expression = expression.replace('github.event.pull_request.labels.*.name', 'labels')
        expression = expression.replace('||', ' or ')
        for event, label, variable, head in itertools.product(
            ('pull_request', 'push', 'merge_group', 'schedule', 'workflow_dispatch'),
            (False, True), ('', 'fast', 'full', 'FULL', 'invalid'), ('feature/x', 'train/batch'),
        ):
            actual = eval(expression, {'__builtins__': {}, 'contains': lambda a, b: b in a, 'startsWith': lambda a, b: a.startswith(b)}, {
                'github': SimpleNamespace(event_name=event, head_ref=head),
                'vars': SimpleNamespace(AIDA_PR_CI_TIER=variable.lower()),
                'labels': ['ci:full'] if label else [],
            })
            self.assertEqual(actual, event != 'pull_request' or head.startswith('train/') or label or variable.lower() == 'full',
                             (event, label, variable))
        self.assertEqual(JOBS['tier']['outputs']['full'], '${{ steps.select.outputs.full }}')
        self.assertEqual(select['run'], 'echo "full=$FULL" >> "$GITHUB_OUTPUT"')

    def test_event_and_job_wiring(self):
        triggers = WORKFLOW.get('on', WORKFLOW.get(True))
        self.assertEqual(triggers['push']['branches'], ['main', 'train/**'])
        self.assertTrue({'labeled', 'unlabeled', 'synchronize'} <= set(triggers['pull_request']['types']))
        for event in ('merge_group', 'schedule', 'workflow_dispatch'):
            self.assertIn(event, triggers)
        self.assertEqual(JOBS['build']['needs'], 'tier')
        self.assertEqual(JOBS['build']['if'], "needs.tier.outputs.full == 'true'")
        self.assertEqual(JOBS['build']['strategy']['matrix']['shard'],
                         ['lib-0', 'lib-1', 'cli-0', 'cli-1', 'rest', 'gates', 'contracts'])
        for job in ('fast-gates', 'fast-compile'):
            self.assertEqual(JOBS[job]['needs'], 'tier')
            self.assertEqual(JOBS[job]['if'], "needs.tier.outputs.full != 'true'")
        self.assertIn({'run': 'make check-ci-fast'}, JOBS['fast-gates']['steps'])
        self.assertIn({'run': 'cargo check --profile dev --workspace --tests'},
                      JOBS['fast-compile']['steps'])

    def test_aggregate_runs_and_rejects_failed_cancelled_or_missing_selected_jobs(self):
        aggregate = JOBS['build-result']
        self.assertEqual(aggregate['name'], 'Build (ubuntu-latest)')
        self.assertEqual(aggregate['if'], '${{ always() }}')
        self.assertEqual(set(aggregate['needs']), {'tier', 'build', 'fast-gates', 'fast-compile'})
        command = aggregate['steps'][0]['run']
        # Execute the workflow shell, including its -e contract: a failed first
        # assertion must not be masked by a successful last assertion.
        for full, tier, build, gates, compile_result in itertools.product(
            ('true', 'false', ''), ('success', 'failure', 'cancelled', 'skipped'),
            ('success', 'failure', 'cancelled', 'skipped'),
            ('success', 'failure', 'cancelled', 'skipped'),
            ('success', 'failure', 'cancelled', 'skipped'),
        ):
            expected = tier == 'success' and (
                (full == 'true' and build == 'success' and gates == compile_result == 'skipped') or
                (full == 'false' and build == 'skipped' and gates == compile_result == 'success'))
            result = subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', command],
                                    env={'FULL': full, 'TIER': tier, 'BUILD': build,
                                         'GATES': gates, 'COMPILE': compile_result}, timeout=2)
            self.assertEqual(result.returncode == 0, expected,
                             (full, tier, build, gates, compile_result))

    def test_fast_inventory_does_not_compile(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location('gates', ROOT / 'scripts/check-ci-gates.py')
        gates = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gates)
        tiers, skips, _ = gates.load_tiers(ROOT)
        steps = gates.load_ci_steps(ROOT)
        self.assertEqual(gates.audit(steps, tiers, skips), [])
        self.assertEqual(tiers['Check aida-cli without default features'], 'full')
        self.assertEqual(tiers['TUI seam guard'], 'full')
        for name, command in steps:
            if tiers.get(name) == 'fast':
                self.assertNotRegex(command, r'cargo (check|test|build)', name)
