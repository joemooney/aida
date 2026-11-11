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
                ('', 'false', 'true', 'TRUE', 'True'),
                ('push', 'pull_request', 'merge_group', 'workflow_dispatch', 'pull_request_target'),
                (False, True), build['strategy']['matrix']['shard']):
            context = dict(vars=dict(AIDA_DELL1_CI=enabled.lower()),
                           github=dict(event_name=event, repository='joemooney/aida',
                                       event=dict(pull_request=dict(head=dict(repo=dict(
                                           full_name='joemooney/aida' if same else 'fork/aida'))))),
                           matrix=dict(shard=shard, os='ubuntu-latest'))
            expected = (enabled.lower() == 'true' and
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

    def test_live_bwrap_export_requires_successful_self_hosted_probe(self):
        import subprocess
        import tempfile
        build = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']
        self.assertNotIn('AIDA_REQUIRE_BWRAP_LIVE', build['env'])
        step = next(s for s in build['steps'] if s.get('name') == 'Verify self-hosted prerequisites')
        self.assertEqual(step['if'], "runner.environment == 'self-hosted'")
        for status in (0, 1):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                probe = root / 'probe.sh'
                probe.write_text('exit ' + str(status))
                exported = root / 'env'
                command = step['run'].replace('scripts/runner/check-prerequisites.sh', str(probe))
                result = subprocess.run(['bash', '-e', '-c', command],
                                        env={'PATH': '/usr/bin:/bin', 'GITHUB_ENV': str(exported)}, timeout=5)
                self.assertEqual(result.returncode, status)
                self.assertEqual(exported.read_text() if exported.exists() else '',
                                 'AIDA_REQUIRE_BWRAP_LIVE=1\n' if status == 0 else '')

    def test_host_mutating_setup_excludes_self_hosted(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = workflow['jobs']['build']
        for step in build['steps']:
            if 'sudo ' in step.get('run', ''):
                # gates never route to dell1; shared setup must explicitly refuse it.
                gate_only = "matrix.shard == 'gates'" in step.get('if', '')
                self.assertTrue(gate_only or "runner.environment != 'self-hosted'" in step['if'], step['name'])
        self.assertEqual(workflow['jobs']['build-result']['runs-on'], 'ubuntu-latest')


class LauncherRefusals(unittest.TestCase):
    """Execute real shell guards with isolated paths and mocked host tools."""
    def test_prerequisite_refusals(self):
        import subprocess
        import tempfile
        for failure in ('root', 'sudo', 'home', 'auth', 'zstd', 'jq', 'ssh-keygen', 'dbus-run-session'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                home = root / 'operator'
                home.mkdir(mode=0o700)
                auth = home / 'auth.json'
                auth.write_text('fixture')
                script = (ROOT / 'scripts/runner/check-prerequisites.sh').read_text()
                script = script.replace('/home/dirtlab/.codex/auth.json', str(auth) if failure == 'auth' else str(root / 'absent'))
                script = script.replace('/home/dirtlab', str(home) if failure == 'home' else str(root / 'absent'))
                for name in ('cc', 'git', 'curl', 'python3', 'pkg-config', 'protoc', 'zsh', 'rg', 'bwrap', 'zstd', 'jq', 'ssh-keygen', 'dbus-run-session', 'sudo', 'id'):
                    body = 'exit 0'
                    if name == 'id':
                        body = 'echo ' + ('0' if failure == 'root' else '1001')
                    if name == 'sudo':
                        body = 'exit ' + ('0' if failure == 'sudo' else '1')
                    if name == failure and failure in ('zstd', 'jq', 'ssh-keygen', 'dbus-run-session'):
                        continue
                    path = root / name
                    path.write_text('#!/bin/sh\n' + body + '\n')
                    path.chmod(0o755)
                result = subprocess.run(['/bin/bash', '-c', script], capture_output=True,
                                        text=True, timeout=5, env={'PATH': str(root)})
                self.assertNotEqual(result.returncode, 0)
                expected = {'root': 'unprivileged', 'sudo': 'sudo access',
                            'home': 'home is traversable', 'auth': 'auth is readable',
                            'zstd': 'missing tool: zstd', 'jq': 'missing tool: jq',
                            'ssh-keygen': 'missing tool: ssh-keygen',
                            'dbus-run-session': 'missing tool: dbus-run-session'}[failure]
                self.assertIn(expected, result.stderr)


# Keep the existing CI fixture entry point; S2 adds no workflow/routing edit.
# trace:STORY-2695 | ai:codex
from tests.test_dell1_controller import Admission, Cache, GateAndShell, LaunchIntegration, Selection, Rearm, Source, RoundThree  # noqa: E402,F401
