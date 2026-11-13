"""Exercise CI's actual tier expression, scheduling and required aggregate.
trace:STORY-2735 | ai:codex
"""
import itertools
import json
import tempfile
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
        self.assertTrue(any(step.get('run') == 'make check-ci-fast' for step in JOBS['fast-gates']['steps']))
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

    def test_actual_fast_invocation_refuses_matching_source_and_ledger_growth(self):
        # Keep the real Make target, planner and guard bodies. Limit the fixture
        # inventory to the two affected guards; their unit suites run separately.
        # trace:STORY-2735 | ai:codex
        step = next(s for s in JOBS['fast-gates']['steps'] if s.get('run') == 'make check-ci-fast')
        guards = {'ExecContext environment ratchet': 'EXEC_CONTEXT_BASE',
                  'Crate glob import ratchet': 'CRATE_GLOBS_BASE'}
        for name, variable in guards.items():
            with self.subTest(guard=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                def write(path, content):
                    target = root / path
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(content)
                def git(*args):
                    return subprocess.check_output(['git', '-c', 'maintenance.auto=false',
                        '-c', 'gc.auto=0', '-c', 'user.name=Fixture',
                        '-c', 'user.email=fixture@example.test', *args], cwd=root, stderr=subprocess.PIPE).decode().strip()
                for path in ('Makefile', 'scripts/check-ci-gates.py',
                             'scripts/check-exec-context.py', 'scripts/check-crate-globs.py'):
                    write(path, (ROOT / path).read_text())
                guard = next(s for s in JOBS['build']['steps'] if s.get('name') == name)
                write('.github/workflows/ci.yml', yaml.safe_dump({'jobs': {'build': {
                    'defaults': JOBS['build']['defaults'], 'steps': [guard]}}}))
                write('scripts/ci-gate-tiers.toml', '[tiers]\n' + json.dumps(name) + ' = "fast"\n')
                for test in ('test_exec_context_ratchet.py', 'test_crate_split_tools.py'):
                    write('tests/' + test, 'import unittest\nclass Fixture(unittest.TestCase):\n def test_fixture(self): pass\n')
                write('tests/__init__.py', '')
                write('Cargo.toml', '[workspace]\nmembers = ["aida-core"]\n')
                write('aida-core/src/legacy.rs', '')
                write('aida-core/src/exec_context.rs', 'fn capture() { std::env::vars_os(); }')
                empty = {'version': 1, 'owner': 'STORY-2538', 'ceiling': 0, 'sites': {}}
                write('scripts/exec-context-legacy.json', json.dumps(empty))
                write('scripts/crate-globs-legacy.json', '{}')
                git('init')
                git('add', '.')
                git('commit', '-m', 'trusted base')
                base = git('rev-parse', 'HEAD')
                git('remote', 'add', 'origin', str(root))
                env = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'RUSTC_WRAPPER': ''}
                # Resolve exactly the environment supplied by the FAST workflow.
                expected = '${{ github.event.pull_request.base.sha || github.event.merge_group.base_sha || github.event.before }}'
                self.assertEqual(step['env'][variable], expected)
                env.update({key: base for key, value in step['env'].items() if value == expected})
                def run():
                    return subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', step['run']],
                        cwd=root, env=env, capture_output=True, text=True, timeout=25)
                clean = run()
                self.assertEqual(clean.returncode, 0, clean.stdout + clean.stderr)
                if variable == 'EXEC_CONTEXT_BASE':
                    write('aida-core/src/legacy.rs', 'fn f() { std::env::var("NEW"); }')
                    # Capture a matching expanded ledger without a base check.
                    subprocess.run(['python3', 'scripts/check-exec-context.py', '--write-ledger'],
                                   cwd=root, check=True, capture_output=True)
                else:
                    write('aida-core/src/legacy.rs', 'use crate::*;')
                    write('scripts/crate-globs-legacy.json', json.dumps({'aida-core/src/legacy.rs': 1}))
                result = run()
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('may only shrink',
                              result.stdout + result.stderr)
