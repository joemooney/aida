"""Exercise CI's actual tier expression, scheduling and required aggregate.
trace:STORY-2735 | ai:codex
"""
import itertools
import json
import re
import tempfile
from pathlib import Path
from types import SimpleNamespace
import subprocess
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
JOBS = WORKFLOW['jobs']
# A label event for any label but ci:full: it must not restart or cancel CI.
# trace:BUG-2578 | ai:claude
LABEL_NOOP = ("(github.event.action == 'labeled' || github.event.action == 'unlabeled')"
              " && github.event.label.name != 'ci:full'")


def render(template, event='pull_request', action='synchronize', label=None,
           labels=(), head='feature/x', variable='', number=7, run_id=99):
    """Evaluate a workflow string's ${{ }} expressions for one event."""
    pull_request = event == 'pull_request'
    github = SimpleNamespace(
        workflow='CI', event_name=event, run_id=run_id, head_ref=head if pull_request else '',
        event=SimpleNamespace(
            action=action if pull_request else None,
            label=SimpleNamespace(name=label),
            pull_request=SimpleNamespace(number=number if pull_request else None)))
    scope = {'github': github, 'vars': SimpleNamespace(AIDA_PR_CI_TIER=variable),
             'labels': list(labels), 'contains': lambda a, b: b in a,
             'startsWith': lambda a, b: a.startswith(b), 'always': lambda: True,
             'format': lambda text, *args: text.format(*args)}

    def evaluate(match):
        expression = match.group(1).strip()
        expression = expression.replace('github.event.pull_request.labels.*.name', 'labels')
        expression = expression.replace('||', ' or ').replace('&&', ' and ')
        expression = re.sub(r'!(?!=)', ' not ', expression)
        return str(eval(expression, {'__builtins__': {}}, scope))
    return re.sub(r'\$\{\{(.*?)\}\}', evaluate, str(template))


LABEL_EVENTS = [(action, label) for action in ('labeled', 'unlabeled')
                for label in ('aida:merge-hold', 'aida:merge-hold-cleared',
                              'aida:merge-hold-recorded', 'ci:fast', 'bug')]
TIER_EVENTS = [('opened', None), ('synchronize', None), ('reopened', None),
               ('labeled', 'ci:full'), ('unlabeled', 'ci:full')]


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
        checkout = next(step for step in JOBS['fast-gates']['steps']
                        if step.get('uses', '').startswith('actions/checkout@'))
        # All ratchets compare against the trusted PR base, including when
        # the base is older than the shallow checkout's immediate parent.
        self.assertEqual(checkout['with']['fetch-depth'], 0)
        self.assertTrue(any(step.get('run', '').strip().endswith('make check-ci-fast') for step in JOBS['fast-gates']['steps']))
        self.assertIn({'run': 'cargo check --profile dev --workspace --tests'},
                      JOBS['fast-compile']['steps'])

    # trace:BUG-2578 | ai:claude
    def test_only_tier_changing_events_join_the_cancelling_pr_group(self):
        concurrency = WORKFLOW['concurrency']
        self.assertEqual(concurrency['cancel-in-progress'], "${{ github.event_name == 'pull_request' }}")
        group = concurrency['group']
        # Pushes and ci:full label changes share the PR's group, so they
        # cancel the run they supersede.
        for action, label in TIER_EVENTS:
            self.assertEqual(render(group, action=action, label=label), 'CI-7', (action, label))
        # Every other label event is alone in its group: it can neither
        # cancel the PR's running CI nor be cancelled into restarting it.
        for action, label in LABEL_EVENTS:
            first = render(group, action=action, label=label, run_id=99)
            second = render(group, action=action, label=label, run_id=100)
            self.assertEqual((first, second), ('CI-label-99', 'CI-label-100'), (action, label))
        for head in ('feature/x', 'train/batch'):
            self.assertEqual(render(group, action='labeled', label='aida:merge-hold', head=head),
                             'CI-label-99')
            self.assertEqual(render(group, action='synchronize', head=head), 'CI-7')
        for event in ('push', 'merge_group', 'schedule', 'workflow_dispatch'):
            self.assertEqual(render(group, event=event), 'CI-99', event)

    # trace:BUG-2578 | ai:claude
    def test_other_label_events_skip_every_job_under_names_of_their_own(self):
        text = (ROOT / '.github/workflows/ci.yml').read_text()
        # One condition, copied; a divergent copy would split the behaviour.
        self.assertEqual(text.count(LABEL_NOOP), 7)
        self.assertEqual(text.count('github.event.label'), 7)
        tier, aggregate = JOBS['tier'], JOBS['build-result']
        real = {'tier': None, 'fast-gates': 'CI (fast gates)',
                'fast-compile': 'CI (fast compile)', 'build-result': 'Build (ubuntu-latest)'}
        for action, label in TIER_EVENTS:
            for ci_full in (False, True):
                event = dict(action=action, label=label, labels=['ci:full'] if ci_full else [])
                self.assertEqual(render(tier['if'], **event), 'True', event)
                self.assertEqual(render(aggregate['if'], **event), 'True', event)
                self.assertEqual(render(tier['name'], **event),
                                 'ci-tier: full' if ci_full else 'ci-tier: fast', event)
                for job, name in real.items():
                    if name:
                        self.assertEqual(render(JOBS[job]['name'], **event), name, event)
        for event in ('push', 'merge_group', 'schedule', 'workflow_dispatch'):
            self.assertEqual(render(tier['if'], event=event), 'True', event)
            self.assertEqual(render(aggregate['if'], event=event), 'True', event)
            self.assertEqual(render(tier['name'], event=event), 'ci-tier: full', event)
            self.assertEqual(render(aggregate['name'], event=event), 'Build (ubuntu-latest)', event)
        for action, label in LABEL_EVENTS:
            for ci_full in (False, True):
                event = dict(action=action, label=label, labels=['ci:full'] if ci_full else [])
                self.assertEqual(render(tier['if'], **event), 'False', event)
                # always() alone would run the aggregate after a skipped tier.
                self.assertEqual(render(aggregate['if'], **event), 'False', event)
                for job in real:
                    name = render(JOBS[job]['name'], **event)
                    # The ship gate reads `ci-tier: ` markers and branch
                    # protection reads Build; neither may see this run.
                    self.assertTrue(name.startswith('label event ('), (job, name))
                    self.assertNotIn(name.lower(), {n.lower() for n in real.values() if n})
        # A skipped tier skips its dependents: none overrides the implicit
        # success() with a status function.
        for job in ('build', 'fast-gates', 'fast-compile'):
            self.assertEqual(JOBS[job]['needs'], 'tier')
            self.assertNotRegex(JOBS[job]['if'], r'always\(|cancelled\(|failure\(')

    def test_aggregate_runs_and_rejects_failed_cancelled_or_missing_selected_jobs(self):
        aggregate = JOBS['build-result']
        self.assertEqual(render(aggregate['name']), 'Build (ubuntu-latest)')
        self.assertEqual(aggregate['if'], '${{ always() && !(%s) }}' % LABEL_NOOP)
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

    # trace:TASK-3791 | ai:codex
    def test_fast_inventory_only_compiles_lightweight_guards(self):
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
            if name == 'Rust structural ratchets':
                self.assertEqual(tiers[name], 'fast')
                self.assertIn('cargo test -p aida-ci-guards --lib', command)
            elif tiers.get(name) == 'fast':
                self.assertNotRegex(command, r'cargo (check|test|build)', name)

    def test_actual_fast_invocation_refuses_matching_source_and_ledger_growth(self):
        # Keep the real Make target, planner and guard bodies. Limit the fixture
        # inventory to the affected guards; their unit suites run separately.
        # trace:STORY-2735 TASK-3785 | ai:codex
        step = next(s for s in JOBS['fast-gates']['steps'] if s.get('run', '').strip().endswith('make check-ci-fast'))
        guards = {'ExecContext environment ratchet': 'EXEC_CONTEXT_BASE',
                  'Crate glob import ratchet': 'CRATE_GLOBS_BASE',
                  'Gate downward dependency ratchet': 'GATE_DEPENDENCIES_BASE'}
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
                             'scripts/check-exec-context.py', 'scripts/check-crate-globs.py',
                             'scripts/check-gate-dependencies.py'):
                    write(path, (ROOT / path).read_text())
                guard = next(s for s in JOBS['build']['steps'] if s.get('name') == name)
                write('.github/workflows/ci.yml', yaml.safe_dump({'jobs': {'build': {
                    'defaults': JOBS['build']['defaults'], 'steps': [guard]}}}))
                write('scripts/ci-gate-tiers.toml', '[tiers]\n' + json.dumps(name) + ' = "fast"\n')
                for test in ('test_exec_context_ratchet.py', 'test_crate_split_tools.py',
                             'test_gate_dependency_ratchet.py'):
                    write('tests/' + test, 'import unittest\nclass Fixture(unittest.TestCase):\n def test_fixture(self): pass\n')
                write('tests/__init__.py', '')
                write('Cargo.toml', '[workspace]\nmembers = ["aida-core"]\n')
                write('aida-core/src/legacy.rs', '')
                write('aida-cli-lib/src/gate/new.rs', 'fn f() {}')
                write('aida-core/src/exec_context.rs', 'fn capture() { std::env::vars_os(); }')
                empty = {'version': 1, 'owner': 'STORY-2538', 'ceiling': 0, 'sites': {}}
                write('scripts/exec-context-legacy.json', json.dumps(empty))
                write('scripts/crate-globs-legacy.json', '{}')
                write('scripts/gate-dependencies-legacy.json', json.dumps({
                    'version': 1, 'owner': 'TASK-3785', 'ceiling': 0, 'edges': {}}))
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
                def run(environment=None, command=None):
                    return subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', step['run'] if command is None else command],
                        cwd=root, env=env if environment is None else environment,
                        capture_output=True, text=True, timeout=25)
                clean = run()
                self.assertEqual(clean.returncode, 0, clean.stdout + clean.stderr)
                # Missing trusted evidence must refuse even on a clean tree.
                # Full-history checkout must already contain the trusted base.
                # Refuse before Make or any guard's optional no-base branch.
                unavailable = dict(env, **{variable: '1' * 40})
                missing = run(unavailable)
                self.assertNotEqual(missing.returncode, 0,
                                    missing.stdout + missing.stderr)
                self.assertIn('FAST requires a trusted base resolving to a checked-out commit', missing.stderr)
                self.assertNotIn('RUN   ', missing.stdout)
                # A real Git object is insufficient when it is not a commit.
                blob = git('rev-parse', 'HEAD:Makefile')
                noncommit = run(dict(env, **{variable: blob}))
                self.assertNotEqual(noncommit.returncode, 0,
                                    noncommit.stdout + noncommit.stderr)
                self.assertIn('FAST requires a trusted base resolving to a checked-out commit', noncommit.stderr)
                self.assertNotIn('RUN   ', noncommit.stdout)
                if variable == 'EXEC_CONTEXT_BASE':
                    write('aida-core/src/legacy.rs', 'fn f() { std::env::var("NEW"); }')
                    # Capture a matching expanded ledger without a base check.
                    subprocess.run(['python3', 'scripts/check-exec-context.py', '--write-ledger'],
                                   cwd=root, check=True, capture_output=True)
                elif variable == 'GATE_DEPENDENCIES_BASE':
                    write('aida-cli-lib/src/gate/new.rs', 'fn f() { crate::session::launch(); }')
                    subprocess.run(['python3', 'scripts/check-gate-dependencies.py', '--write-ledger'],
                                   cwd=root, check=True, capture_output=True)
                else:
                    write('aida-core/src/legacy.rs', 'use crate::*;')
                    write('scripts/crate-globs-legacy.json', json.dumps({'aida-core/src/legacy.rs': 1}))
                # Model the actual PR checkout: its HEAD already contains the
                # expanded ledger. Comparing against HEAD would silently pass;
                # FAST must retain the older event-supplied trusted base.
                git('add', '.')
                git('commit', '-m', 'PR source and ledger ceiling increase')
                self.assertNotEqual(git('rev-parse', 'HEAD'), base)
                # Positive control: matching source/ledger growth alone passes
                # this same FAST invocation. Its refusal below must come from
                # the trusted base comparison, not an inconsistent fixture.
                without_base = {key: value for key, value in env.items()
                                if key not in guards.values()}
                # Local Make remains usable without a PR event, but CI must
                # fail closed if event resolution supplied an empty or null
                # base. Guard bodies otherwise treat the null SHA as no base.
                for absent in ('', None, '0' * 40):
                    incomplete = dict(env)
                    if absent is None:
                        incomplete.pop(variable)
                    else:
                        incomplete[variable] = absent
                    refused = run(incomplete)
                    self.assertNotEqual(refused.returncode, 0,
                                        refused.stdout + refused.stderr)
                    self.assertIn('FAST requires a trusted base', refused.stderr)
                unchecked = run(without_base, command='make check-ci-fast')
                self.assertEqual(unchecked.returncode, 0,
                                 unchecked.stdout + unchecked.stderr)
                if variable == 'GATE_DEPENDENCIES_BASE':
                    for absent in ('', None, '0' * 40):
                        pr_env = dict(without_base, GITHUB_EVENT_NAME='pull_request')
                        if absent is not None:
                            pr_env[variable] = absent
                        refused = run(pr_env, command='make check-ci-fast')
                        self.assertNotEqual(refused.returncode, 0,
                                            refused.stdout + refused.stderr)
                        self.assertIn('PR CI requires a trusted gate-dependency base',
                                      refused.stdout + refused.stderr)
                result = run()
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn('may only decrease' if variable == 'GATE_DEPENDENCIES_BASE' else 'may only shrink',
                              result.stdout + result.stderr)
