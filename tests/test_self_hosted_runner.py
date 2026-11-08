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

    def test_live_bwrap_export_requires_successful_self_hosted_probe(self):
        import os
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
                                        env={**os.environ, 'GITHUB_ENV': str(exported)}, timeout=5)
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
    def run_launcher(self, *, uid='1001', groups='1001', writable=False,
                     symlink=False, jit='config\n', invoking_uid='0'):
        import os
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            dist = root / 'distribution'
            dist.mkdir(mode=0o755)
            (dist / 'run.sh').write_text('#!/bin/sh\nexit 0\n')
            (dist / 'run.sh').chmod(0o755)
            if writable:
                (dist / 'run.sh').chmod(0o777)
            if symlink:
                (dist / 'npm').symlink_to('run.sh')
            commands = root / 'bin'
            commands.mkdir()
            mocks = {
                'id': f'case "$1" in -u) echo {uid};; -G) echo "{groups}";; esac',
                'flock': 'exit 0',
                # Stop before any system mutation, after all input guards.
                'install': 'echo REACHED_INSTALL >&2; exit 91',
            }
            # The fixture user owns its temp tree; substitute the owner predicate
            # only, preserving find's real type/permission behavior for symlinks.
            script = (ROOT / 'scripts/runner/one-job.sh').read_text()
            script = script.replace('"$EUID"', f'"{invoking_uid}"')
            script = script.replace('/run/aida-ci-runner.lock', str(root / 'lock'))
            script = script.replace('/opt/aida-actions-runner', str(dist))
            script = script.replace('! -user root', f'! -user {os.getuid()}')
            for name, body in mocks.items():
                path = commands / name
                path.write_text('#!/bin/sh\n' + body + '\n')
                path.chmod(0o755)
            return subprocess.run(['bash', '-c', script], input=jit, text=True,
                                  capture_output=True, timeout=5,
                                  env={**os.environ, 'PATH': f'{commands}:/usr/bin:/bin'})

    def test_refuses_nonroot_operator(self):
        result = self.run_launcher(invoking_uid='1001')
        self.assertEqual(result.returncode, 2)
        self.assertIn('Usage:', result.stderr)

    def test_refuses_root_runner(self):
        self.assertIn('must be non-root', self.run_launcher(uid='0').stderr)

    def test_refuses_supplementary_groups(self):
        self.assertIn('supplementary groups', self.run_launcher(groups='1001 27').stderr)

    def test_refuses_writable_distribution(self):
        self.assertIn('not writable', self.run_launcher(writable=True).stderr)

    def test_symlink_distribution_passes_mode_guard(self):
        result = self.run_launcher(symlink=True)
        self.assertEqual(result.returncode, 91, result.stderr)
        self.assertIn('REACHED_INSTALL', result.stderr)

    def test_refuses_empty_or_missing_jit(self):
        for jit in ('\n', ''):
            result = self.run_launcher(jit=jit)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('JIT configuration', result.stderr)
            self.assertNotIn('REACHED_INSTALL', result.stderr)

    def test_prerequisite_refusals(self):
        import os
        import subprocess
        import tempfile
        for failure in ('root', 'sudo', 'home', 'auth', 'zstd', 'jq', 'ssh-keygen'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                home = root / 'operator'
                home.mkdir(mode=0o700)
                auth = home / 'auth.json'
                auth.write_text('fixture')
                script = (ROOT / 'scripts/runner/check-prerequisites.sh').read_text()
                script = script.replace('/home/dirtlab/.codex/auth.json', str(auth) if failure == 'auth' else str(root / 'absent'))
                script = script.replace('/home/dirtlab', str(home) if failure == 'home' else str(root / 'absent'))
                for name in ('cc', 'git', 'curl', 'python3', 'pkg-config', 'protoc', 'zsh', 'rg', 'bwrap', 'zstd', 'jq', 'ssh-keygen', 'sudo', 'id'):
                    body = 'exit 0'
                    if name == 'id':
                        body = 'echo ' + ('0' if failure == 'root' else '1001')
                    if name == 'sudo':
                        body = 'exit ' + ('0' if failure == 'sudo' else '1')
                    if name == failure and failure in ('zstd', 'jq', 'ssh-keygen'):
                        continue
                    path = root / name
                    path.write_text('#!/bin/sh\n' + body + '\n')
                    path.chmod(0o755)
                result = subprocess.run(['/bin/bash', '-c', script], capture_output=True,
                                        text=True, timeout=5, env={**os.environ, 'PATH': str(root)})
                self.assertNotEqual(result.returncode, 0)
                expected = {'root': 'unprivileged', 'sudo': 'sudo access',
                            'home': 'home is traversable', 'auth': 'auth is readable',
                            'zstd': 'missing tool: zstd', 'jq': 'missing tool: jq',
                            'ssh-keygen': 'missing tool: ssh-keygen'}[failure]
                self.assertIn(expected, result.stderr)

    def test_rearm_generates_fresh_configs_and_stops_on_failure(self):
        import os
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            count = root / 'count'
            received = root / 'received'
            commands = {
                'gh': f'echo fresh-jit >> "{count}"; echo fresh-jit',
                'sudo': f'[ "$1" = -n ] || exit 97; cat >> "{received}"; '
                        f'[ "$(wc -l < "{received}")" -lt 2 ]',
            }
            for name, body in commands.items():
                path = root / name
                path.write_text('#!/bin/sh\n' + body + '\n')
                path.chmod(0o755)
            script = (ROOT / 'scripts/runner/rearm.sh').read_text().replace('"$EUID"', '"1001"')
            result = subprocess.run(['bash', '-c', script], capture_output=True,
                                    text=True, timeout=5,
                                    env={**os.environ, 'PATH': f'{root}:/usr/bin:/bin'})
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(count.read_text().splitlines(), ['fresh-jit'] * 2)
            self.assertEqual(received.read_text().splitlines(), ['fresh-jit'] * 2)

    def test_job_paths_are_stable_and_wiped_under_lock(self):
        launcher = (ROOT / 'scripts/runner/one-job.sh').read_text()
        self.assertIn('state=/var/lib/aida-ci-jobs/current', launcher)
        self.assertNotIn('mktemp', launcher)
        self.assertIn('HOME="$state/home"', launcher)
        self.assertIn('WorkingDirectory="$state"', launcher)
        lock = launcher.index('flock -n 9')
        prepare = launcher.index('systemctl stop "$unit.service" || {')
        wipe = launcher.index('rm -rf -- "$state"', prepare)
        create = launcher.index('install -d -m 0700 "$state"')
        copy = launcher.index('cp -a "$runner_source/." "$state/"')
        self.assertLess(lock, prepare)
        self.assertLess(prepare, launcher.index('trap cleanup EXIT'))
        self.assertLess(launcher.index('trap cleanup EXIT'), wipe)
        self.assertLess(wipe, create)
        self.assertLess(create, copy)
        self.assertIn('refuse "cannot stop previous job unit', launcher)

    def test_idle_wait_does_not_consume_job_execution_budget(self):
        launcher = (ROOT / 'scripts/runner/one-job.sh').read_text()
        self.assertNotIn('RuntimeMaxSec=', launcher)
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        self.assertEqual(workflow['jobs']['build']['timeout-minutes'], 40)
        supervisor = (ROOT / 'scripts/runner/rearm.sh').read_text()
        self.assertNotIn('sudo -v', supervisor)
        self.assertIn('sudo -n /usr/local/sbin/aida-ci-one-job', supervisor)
