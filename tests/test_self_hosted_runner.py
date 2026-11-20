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
    def test_ci_entry_point_discovers_all_controller_regressions(self):
        import sys
        from tests import test_dell1_controller
        loader = unittest.TestLoader()
        def ids(suite):
            for test in suite:
                if isinstance(test, unittest.TestSuite):
                    yield from ids(test)
                else:
                    yield test.id()
        expected = set(ids(loader.loadTestsFromModule(test_dell1_controller)))
        actual = set(ids(loader.loadTestsFromModule(sys.modules[__name__])))
        self.assertTrue(expected)
        self.assertEqual(expected - actual, set(), 'controller regressions missing from CI entry point')

    def test_actual_expression_routes_only_opted_in_trusted_heavy_shards(self):
        # Only the classes the controller admits (MAIN: main push; TRAIN:
        # same-repository train/ PR into main) may wait for dell1.
        # trace:STORY-2695 | ai:claude
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        build = workflow['jobs']['build']
        expression = build['runs-on'].removeprefix('${{').removesuffix('}}').strip()
        # Translate the closed expression subset used here to Python. Strings and
        # non-empty arrays in this expression have the same Actions truthiness.
        cases = []
        routed = 0
        for enabled, event, same, ref, base, head, shard in itertools.product(
                ('', 'false', 'true', 'TRUE', 'True'),
                ('push', 'pull_request', 'merge_group', 'workflow_dispatch', 'pull_request_target'),
                (False, True), ('refs/heads/main', 'refs/heads/train/x', 'refs/pull/7/merge'),
                ('main', 'integration', ''),
                ('train/x', 'story-1', 'xtrain/x', 'Train/x', 'train', ''),
                build['strategy']['matrix']['shard']):
            context = dict(vars=dict(AIDA_DELL1_CI=enabled.lower()),
                           github=dict(event_name=event, repository='joemooney/aida',
                                       ref=ref, base_ref=base, head_ref=head,
                                       event=dict(pull_request=dict(head=dict(repo=dict(
                                           full_name='joemooney/aida' if same else 'fork/aida'))))),
                           matrix=dict(shard=shard, os='ubuntu-latest'))
            main_push = event == 'push' and ref == 'refs/heads/main'
            train = (event == 'pull_request' and same and base == 'main'
                     and head.lower().startswith('train/'))
            expected = (enabled.lower() == 'true' and (main_push or train) and
                        shard in ('lib-0', 'lib-1', 'cli-0', 'cli-1'))
            routed += expected
            cases.append([context, expected])
        self.assertGreater(routed, 0)
        def namespace(value):
            return SimpleNamespace(**{key: namespace(item) if isinstance(item, dict) else item
                                      for key, item in value.items()})
        expression = expression.replace('&&', ' and ').replace('||', ' or ')
        for context, expected in cases:
            # Actions' startsWith ignores case, so a Train/x head is routed
            # too; the controller's case-sensitive check refuses it on the host.
            actual = eval(expression, {'__builtins__': {}, 'fromJSON': json.loads,
                                       'contains': lambda array, value: value in array,
                                       'startsWith': lambda text, prefix: text.lower().startswith(prefix.lower())},
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

    def heavy_self_hosted_steps(self):
        build = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']
        for step in build['steps']:
            condition = step.get('if', '')
            hosted_only = "runner.environment != 'self-hosted'" in condition
            other_shard = any(f"matrix.shard == '{shard}'" in condition and '||' not in condition
                              for shard in ('gates', 'contracts', 'rest'))
            if not hosted_only and not other_shard:
                yield step

    def test_self_hosted_runs_no_checkout_toolchain_or_remote_cache_action(self):
        # Exact source, the pinned toolchain and the local-only cache all come
        # from the host; an action that replaced any of them would break the
        # reviewed contract the controller's policy digest pins.
        # trace:STORY-2695 | ai:claude
        actions = {step['uses'].split('@')[0] for step in self.heavy_self_hosted_steps() if 'uses' in step}
        self.assertEqual(actions, {'taiki-e/install-action', 'actions/upload-artifact'})
        for step in self.heavy_self_hosted_steps():
            for word in ('rustup ', 'git checkout', 'git fetch --no-tags --depth', 'actions/cache'):
                self.assertNotIn(word, step.get('run', ''), step.get('name'))

    def test_self_hosted_source_step_precedes_every_other_self_hosted_step(self):
        steps = list(self.heavy_self_hosted_steps())
        self.assertEqual(steps[0].get('name'), 'Adopt host-prepared exact source')
        self.assertEqual(steps[0]['if'], "runner.environment == 'self-hosted'")
        self.assertEqual(steps[0]['env']['EXPECTED_SHA'],
                         '${{ github.event.pull_request.head.sha || github.sha }}')
        selected = next(s for s in steps if s.get('id') == 'changes')
        self.assertEqual(selected['env']['FULL_CI'],
                         "${{ runner.environment == 'self-hosted' && 'true' || steps.paths.outputs.full_ci }}")

    def adopt(self, root, mutate=lambda paths: None, sha=None, env=None):
        """Run the real step body against a real Git tree and a fake toolchain."""
        import subprocess
        build = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']
        step = next(s for s in build['steps'] if s.get('name') == 'Adopt host-prepared exact source')
        paths = SimpleNamespace(work=root / 'work/workspace', target=root / 'work/target',
                                cargo=root / 'work/cargo', runner=root / 'runner/_work/aida/aida',
                                tools=root / 'toolchain/bin', path_file=root / 'github-path')
        for directory in (paths.work, paths.target, paths.cargo, paths.runner, paths.tools):
            directory.mkdir(parents=True)
        for tool in ('cargo', 'rustc'):
            (paths.tools / tool).write_text('#!/bin/sh\necho "cargo 1.0.0"\n')
            (paths.tools / tool).chmod(0o755)
        (paths.work / 'Cargo.lock').write_text('fixture\n')
        (paths.work / '.gitignore').write_text('/target/\n')
        git = ['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid',
               '-c', 'init.defaultBranch=main', '-C', str(paths.work)]
        for args in (['init', '--quiet'], ['add', '-A'], ['commit', '--quiet', '-m', 'fixture']):
            subprocess.run(git + args, check=True, capture_output=True, timeout=10)
        head = subprocess.check_output(git + ['rev-parse', 'HEAD'], text=True, timeout=10).strip()
        overrides = mutate(paths) or {}
        values = {'PATH': f'{paths.tools}:/usr/bin:/bin', 'HOME': str(root),
                  'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1',
                  'AIDA_CI_WORKSPACE': str(paths.work), 'CARGO_TARGET_DIR': str(paths.target),
                  'CARGO_HOME': str(paths.cargo), 'GITHUB_WORKSPACE': str(paths.runner),
                  'GITHUB_PATH': str(paths.path_file), 'EXPECTED_SHA': sha or head}
        values.update(overrides)
        values.update(env or {})
        values = {key: value for key, value in values.items() if value is not None}
        body = step['run'].replace('/opt/aida-ci-toolchain/bin', str(paths.tools))
        self.assertNotEqual(body, step['run'])
        result = subprocess.run(['bash', '--noprofile', '--norc', '-e', '-o', 'pipefail', '-c', body],
                                env=values, cwd=paths.runner, capture_output=True, text=True, timeout=20)
        return paths, result

    def test_adopted_source_is_the_host_tree_with_the_seeded_target(self):
        import subprocess
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            paths, result = self.adopt(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            # Later steps start in the runner workspace: it is the host tree.
            self.assertTrue(paths.runner.is_symlink())
            self.assertEqual(paths.runner.resolve(), paths.work)
            self.assertEqual((paths.work / 'target').resolve(), paths.target)
            self.assertEqual(paths.path_file.read_text(), f'{paths.cargo}/bin\n')
            # Neither the link nor a later build output dirties the tree or
            # trips the shard root-hygiene check.
            (paths.runner / 'target/test-run.log').write_text('log\n')
            self.assertEqual((paths.target / 'test-run.log').read_text(), 'log\n')
            listed = subprocess.check_output(
                ['git', '-C', str(paths.runner), 'ls-files', '--others', '--exclude-standard', '--directory'],
                text=True, timeout=10)
            self.assertEqual(listed, '')

    def test_adopt_refuses_every_broken_source_contract(self):
        import os
        import tempfile
        def dirty(paths):
            (paths.work / 'Cargo.lock').write_text('changed\n')
        def untracked(paths):
            (paths.work / 'extra.rs').write_text('fn main() {}\n')
        def populated(paths):
            (paths.runner / 'stale').write_text('previous job\n')
        def target_present(paths):
            (paths.work / 'target').mkdir()
        def target_link(paths):
            os.symlink('/nonexistent', paths.work / 'target')
        def linked_target_dir(paths):
            paths.target.rmdir()
            os.symlink(paths.cargo, paths.target)
        def no_git(paths):
            import shutil
            shutil.rmtree(paths.work / '.git')
        def nested(paths):
            # A prepared tree that is only a subdirectory of some other repository.
            import shutil
            import subprocess
            shutil.rmtree(paths.work / '.git')
            git = ['git', '-c', 'user.name=t', '-c', 'user.email=t@example.invalid', '-C', str(paths.work.parent)]
            for args in (['init', '--quiet'], ['add', '-A'], ['commit', '--quiet', '-m', 'outer']):
                subprocess.run(git + args, check=True, capture_output=True, timeout=10)
            return {'EXPECTED_SHA': subprocess.check_output(git + ['rev-parse', 'HEAD'], text=True,
                                                            timeout=10).strip()}
        def foreign_cargo(paths):
            (paths.tools / 'cargo').unlink()
            other = paths.tools.parent / 'other'
            other.mkdir()
            (other / 'cargo').write_text('#!/bin/sh\necho "cargo 1.0.0"\n')
            (other / 'cargo').chmod(0o755)
            return {'PATH': f'{other}:{paths.tools}:/usr/bin:/bin'}
        cases = {
            'wrong sha': (dict(sha='0' * 40), 'this job is for'),
            'merge ref sha': (dict(sha='1' * 40), 'this job is for'),
            'missing workspace': (dict(env={'AIDA_CI_WORKSPACE': None}), 'AIDA_CI_WORKSPACE is not an absolute'),
            'relative workspace': (dict(env={'AIDA_CI_WORKSPACE': 'work/workspace'}), 'AIDA_CI_WORKSPACE is not an absolute'),
            'empty target': (dict(env={'CARGO_TARGET_DIR': ''}), 'CARGO_TARGET_DIR is not an absolute'),
            'absent cargo home': (dict(env={'CARGO_HOME': '/nonexistent/cargo'}), 'CARGO_HOME is not a host-prepared'),
            'linked target dir': (dict(mutate=linked_target_dir), 'CARGO_TARGET_DIR is not a host-prepared'),
            'no git': (dict(mutate=no_git), 'prepared source has no Git HEAD'),
            'nested': (dict(mutate=nested), 'not its own Git root'),
            'dirty': (dict(mutate=dirty), 'differs from the admitted commit'),
            'untracked': (dict(mutate=untracked), 'differs from the admitted commit'),
            'populated runner workspace': (dict(mutate=populated), 'runner workspace is not empty'),
            'target present': (dict(mutate=target_present), 'already has target'),
            'target link': (dict(mutate=target_link), 'already has target'),
            'foreign cargo': (dict(mutate=foreign_cargo), 'cargo is not the installed pinned toolchain'),
        }
        for name, (arguments, message) in cases.items():
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                paths, result = self.adopt(root, **arguments)
                self.assertNotEqual(result.returncode, 0, name)
                self.assertIn('dell1 source contract refused', result.stderr, name)
                self.assertIn(message, result.stderr, name)
                # A refusal never leaves the runner workspace pointing at source.
                self.assertFalse(paths.runner.is_symlink(), name)
                self.assertFalse((paths.work / 'target').is_symlink() and name != 'target link', name)

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


class AptBounds(unittest.TestCase):
    """Bound hosted APT steps and preserve short mirror/DNS fetch timeouts.
    trace:BUG-2526 BUG-2668 | ai:codex
    """

    def steps(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        for job_name, job in workflow['jobs'].items():
            for step in job.get('steps', []):
                if 'apt-get' in step.get('run', ''):
                    yield job_name, step

    def test_every_apt_step_has_its_expected_timeout_bound(self):
        steps = list(self.steps())
        self.assertTrue(steps, 'expected apt steps in ci.yml')
        for job_name, step in steps:
            limit = step.get('timeout-minutes')
            self.assertIsNotNone(limit, f'{job_name}: {step["name"]} has no step timeout')
            if (job_name, step['name']) == ('build', 'Install Linux dependencies'):
                self.assertEqual(int(limit), 12)
            else:
                self.assertLessEqual(int(limit), 5, f'{job_name}: {step["name"]}')

    def test_every_apt_call_sets_fetch_timeouts(self):
        for job_name, step in self.steps():
            for line in step['run'].replace('\\\n', '').splitlines():
                if 'apt-get' in line and not line.lstrip().startswith('#'):
                    for option in ('Acquire::http::Timeout=', 'Acquire::https::Timeout=',
                                   'Acquire::Retries='):
                        self.assertIn(option, line, f'{job_name}: {step["name"]}: {line.strip()}')
