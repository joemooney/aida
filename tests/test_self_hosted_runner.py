"""Exercise the actual workflow expression across trust and capacity boundaries.
trace:STORY-2672 | ai:codex
"""
import functools
import itertools
import json
from pathlib import Path
from types import SimpleNamespace
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]


@functools.lru_cache(maxsize=None)
def build_job():
    """The parsed build job, for read-only use."""
    return yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']


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

    # Routing has two parts: the tier job's shell step decides the class
    # (case-sensitively, before the matrix exists) and the build job's
    # expression turns that class into the labels it requests. Both run here
    # from the workflow's own text. trace:STORY-2695 | ai:claude
    ROUTE_INPUTS = {
        'ENABLED': '${{ vars.AIDA_DELL1_CI }}', 'EVENT': '${{ github.event_name }}',
        'REF': '${{ github.ref }}', 'REPOSITORY': '${{ github.repository }}',
        'HEAD_REPOSITORY': '${{ github.event.pull_request.head.repo.full_name }}',
        'BASE_REF': '${{ github.base_ref }}', 'HEAD_REF': '${{ github.head_ref }}',
    }
    DELL1 = ['self-hosted', 'Linux', 'X64', 'aida-dell1']
    HEAVY = ('lib-0', 'lib-1', 'cli-0', 'cli-1')

    def route_classes(self, cases):
        """Run the real route step body once per case; return each case's output."""
        import shlex
        import subprocess
        import tempfile
        tier = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['tier']
        step = next(s for s in tier['steps'] if s.get('id') == 'route')
        self.assertEqual(step['env'], self.ROUTE_INPUTS)
        self.assertNotIn('${{', step['run'])  # No untrusted text is spliced into the shell.
        self.assertEqual(tier['outputs']['dell1'], '${{ steps.route.outputs.dell1 }}')
        self.assertEqual(tier['runs-on'], 'ubuntu-latest')
        with tempfile.TemporaryDirectory() as directory:
            outputs = Path(directory) / 'outputs'
            outputs.mkdir()
            lines = ['route() {', step['run'], '}']
            for index, case in enumerate(cases):
                self.assertEqual(set(case), set(self.ROUTE_INPUTS))
                assignments = ' '.join(f'{key}={shlex.quote(value)}' for key, value in case.items())
                lines.append(f'{assignments} GITHUB_OUTPUT={shlex.quote(str(outputs / str(index)))} route')
            # The tier job has no shell default: GitHub runs `bash -e {0}`.
            script = Path(directory) / 'route.sh'
            script.write_text('\n'.join(lines) + '\n')
            result = subprocess.run(['bash', '--noprofile', '--norc', '-e', str(script)],
                                    env={'PATH': '/usr/bin:/bin'}, capture_output=True, text=True, timeout=50)
            self.assertEqual(result.returncode, 0, result.stderr)
            classes = []
            for index in range(len(cases)):
                text = (outputs / str(index)).read_text()
                self.assertRegex(text, r'\Adell1=(MAIN|TRAIN)?\n\Z')
                classes.append(text.strip().partition('=')[2])
            return classes

    def requested_runner(self, route, shard):
        """Evaluate the build job's real runs-on expression for one route output."""
        class Folded(str):
            # Workflow expressions compare strings without regard to case.
            def __eq__(self, other):
                return str(self).lower() == str(other).lower() if isinstance(other, str) else NotImplemented
            def __ne__(self, other):
                return not self == other
            __hash__ = str.__hash__
        build = build_job()
        self.assertEqual(build['needs'], 'tier')
        expression = build['runs-on'].removeprefix('${{').removesuffix('}}').strip()
        # Translate the closed expression subset used here to Python. Strings and
        # non-empty arrays in this expression have the same Actions truthiness.
        expression = expression.replace('&&', ' and ').replace('||', ' or ')
        names = dict(needs=SimpleNamespace(tier=SimpleNamespace(outputs=SimpleNamespace(dell1=Folded(route)))),
                     matrix=SimpleNamespace(shard=Folded(shard), os='ubuntu-latest'))
        return eval(expression, {'__builtins__': {}, 'fromJSON': json.loads,
                                 'contains': lambda array, value: value in [Folded(item) for item in array]},
                    names)

    def test_route_step_admits_only_exact_main_pushes_and_train_pull_requests(self):
        cases, expected = [], []
        for enabled, event, same, ref, base, head in itertools.product(
                ('', 'false', 'true', 'TRUE', 'True', 'true '),
                ('push', 'pull_request', 'merge_group', 'workflow_dispatch', 'pull_request_target', 'Push'),
                (False, True),
                ('refs/heads/main', 'refs/heads/Main', 'refs/heads/train/x', 'refs/pull/7/merge'),
                ('main', 'Main', 'integration', ''),
                ('train/x', 'Train/x', 'TRAIN/x', 'story-1', 'xtrain/x', 'train', 'main', '')):
            cases.append({'ENABLED': enabled, 'EVENT': event, 'REF': ref, 'REPOSITORY': 'joemooney/aida',
                          'HEAD_REPOSITORY': 'joemooney/aida' if same else 'fork/aida',
                          'BASE_REF': base, 'HEAD_REF': head})
            if enabled != 'true':
                expected.append('')
            elif event == 'push' and ref == 'refs/heads/main':
                expected.append('MAIN')
            elif event == 'pull_request' and same and base == 'main' and head.startswith('train/'):
                expected.append('TRAIN')
            else:
                expected.append('')
        actual = self.route_classes(cases)
        self.assertEqual({'', 'MAIN', 'TRAIN'}, set(expected))
        for case, wanted, got in zip(cases, expected, actual):
            if wanted != got:
                self.fail(f'{case}: expected {wanted!r}, got {got!r}')

    def test_route_step_fails_closed_on_missing_or_hostile_context(self):
        train = {'ENABLED': 'true', 'EVENT': 'pull_request', 'REF': 'refs/pull/7/merge',
                 'REPOSITORY': 'joemooney/aida', 'HEAD_REPOSITORY': 'joemooney/aida',
                 'BASE_REF': 'main', 'HEAD_REF': 'train/x'}
        hostile = [
            dict(train, REPOSITORY='', HEAD_REPOSITORY=''),            # no repository identity at all
            dict(train, HEAD_REPOSITORY='JoeMooney/aida'),             # a differently-cased owner
            dict(train, HEAD_REF='Train/x'),
            dict(train, HEAD_REF='x/train/x'),
            dict(train, HEAD_REF='$(echo train/x)'),
            dict(train, HEAD_REF='story-1\ndell1=TRAIN'),              # output injection
            dict(train, BASE_REF='main\ndell1=TRAIN', HEAD_REF='story-1'),
            dict(train, ENABLED='true\n'),
            dict(train, EVENT='push'),                                 # a push never has a PR merge ref as main
            dict(train, EVENT='push', REF='refs/heads/main\n'),
            dict(train, EVENT='push', REF='refs/tags/main'),
        ]
        self.assertEqual(self.route_classes([train] + hostile), ['TRAIN'] + [''] * len(hostile))

    def test_runs_on_requests_the_class_label_only_for_heavy_shards(self):
        build = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']
        shards = build['strategy']['matrix']['shard']
        self.assertLess(set(self.HEAVY), set(shards))
        for route, shard in itertools.product(('', 'MAIN', 'TRAIN', 'true', 'aida-dell1', 'MAINTRAIN'), shards):
            wanted = (self.DELL1 + [route] if route in ('MAIN', 'TRAIN') and shard in self.HEAVY
                      else 'ubuntu-latest')
            self.assertEqual(self.requested_runner(route, shard), wanted, (route, shard))

    def test_a_job_can_only_reach_a_runner_registered_for_its_own_class(self):
        # GitHub dispatches a job only to a runner holding every requested
        # label. rearm.py registers the four fixed labels, the admitted class
        # and the runner's own name; nothing else.
        rearm = (ROOT / 'scripts/runner/rearm.py').read_text()
        for label in self.DELL1:
            self.assertIn(f"'labels[]={label}'", rearm)
        self.assertIn("'labels[]=' + proposal['class']", rearm)
        self.assertEqual(rearm.count('labels[]='), len(self.DELL1) + 2)
        def registered(kind):
            return {label.lower() for label in self.DELL1 + [kind, 'aida-ci-s0-0123']}
        for route, other in (('MAIN', 'TRAIN'), ('TRAIN', 'MAIN')):
            for shard in self.HEAVY:
                requested = {label.lower() for label in self.requested_runner(route, shard)}
                self.assertLessEqual(requested, registered(route))
                self.assertFalse(requested <= registered(other), (route, shard))

    def test_mixed_case_train_branch_stays_hosted_end_to_end(self):
        pull = {'ENABLED': 'true', 'EVENT': 'pull_request', 'REF': 'refs/pull/7/merge',
                'REPOSITORY': 'joemooney/aida', 'HEAD_REPOSITORY': 'joemooney/aida',
                'BASE_REF': 'main', 'HEAD_REF': 'train/x'}
        push = dict(pull, EVENT='push', REF='refs/heads/main', BASE_REF='', HEAD_REF='')
        cases = [pull, dict(pull, HEAD_REF='Train/x'), dict(pull, HEAD_REF='TRAIN/X'),
                 dict(pull, BASE_REF='Main'), push, dict(push, REF='refs/heads/Main'),
                 dict(pull, ENABLED='TRUE'), dict(pull, ENABLED='')]
        routes = self.route_classes(cases)
        wanted = ['TRAIN', '', '', '', 'MAIN', '', '', '']
        self.assertEqual(routes, wanted)
        for route, shard in itertools.product(routes, self.HEAVY):
            runner = self.requested_runner(route, shard)
            self.assertEqual(runner, self.DELL1 + [route] if route else 'ubuntu-latest')

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

    def build_steps(self):
        return yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())['jobs']['build']['steps']

    def heavy_self_hosted_steps(self, steps=None):
        """Steps a dell1 heavy shard can run, by evaluating each whole condition.

        A substring test would call `hosted-only || lib-0` hosted-only. This
        evaluates the complete expression with the GitLab adapter's evaluator
        (case-insensitive comparison, the status functions) for all four
        shards, both admitted events and both prior-step outcomes; an
        expression it cannot evaluate raises instead of being assumed safe.
        trace:STORY-2695 | ai:claude
        """
        import importlib.util
        spec = importlib.util.spec_from_file_location('gdma_ci', ROOT / 'ci/gdma/run-upstream.py')
        adapter = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(adapter)
        contexts = []
        for shard, event, failed, cancelled in itertools.product(
                self.HEAVY, ('push', 'pull_request'), (False, True), (False, True)):
            context = adapter.context(shard, True, failed)
            # dell1 always selects full CI; it has no cache-action outputs.
            context.update({'runner.environment': 'self-hosted', 'github.event_name': event,
                            'github.ref': 'refs/heads/main' if event == 'push' else 'refs/pull/7/merge',
                            'cancelled': cancelled})
            self.assertEqual(context['steps.changes.outputs.full_ci'], 'true')
            contexts.append(context)
        for step in self.build_steps() if steps is None else steps:
            if any(adapter.condition(step.get('if', 'true'), context) for context in contexts):
                yield step

    def test_self_hosted_step_selection_evaluates_the_whole_condition(self):
        import copy
        original = self.build_steps()
        baseline = [step.get('name', step.get('uses')) for step in self.heavy_self_hosted_steps(original)]
        self.assertNotIn('actions/checkout@v5', baseline)
        self.assertNotIn('Install Linux dependencies', baseline)
        self.assertIn('Run tests', baseline)
        def selected(name, condition):
            steps = copy.deepcopy(original)
            step = next(s for s in steps if s.get('name', s.get('uses')) == name)
            step['if'] = condition
            return name in [s.get('name', s.get('uses')) for s in self.heavy_self_hosted_steps(steps)]
        hosted = "runner.environment != 'self-hosted'"
        # An OR branch re-admits dell1 although the hosted-only text is still there.
        for condition in (f"{hosted} || matrix.shard == 'lib-0'",
                          f"{hosted} || startsWith(matrix.shard, 'cli-')",
                          f"${{{{ {hosted} || !cancelled() }}}}",
                          f"({hosted} && matrix.shard == 'gates') || matrix.shard == 'LIB-1'",
                          f"!({hosted})", "always()"):
            self.assertTrue(selected('actions/checkout@v5', condition), condition)
        for condition in (hosted, f"{hosted} && matrix.shard == 'lib-0'",
                          f"{hosted} || matrix.shard == 'gates'",
                          "matrix.shard == 'rest' || matrix.shard == 'contracts'",
                          "runner.environment != 'SELF-HOSTED'"):
            self.assertFalse(selected('actions/checkout@v5', condition), condition)
        # A gates-only sudo step that gains a heavy-shard branch is caught too.
        self.assertFalse(selected('Live bwrap OS-wrap confinement gate', "matrix.shard == 'gates'"))
        self.assertTrue(selected('Live bwrap OS-wrap confinement gate',
                                 "matrix.shard == 'gates' || matrix.shard == 'cli-0'"))
        # An expression outside the evaluated subset is refused, never assumed hosted.
        with self.assertRaises(ValueError):
            selected('actions/checkout@v5', f"{hosted} || hashFiles('Cargo.lock') != ''")

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

    # What a previous run leaves in a seeded build directory.
    STALE_REPORTS = ('test-run.log', 'test-timings.json', 'changed-files.txt',
                     'nextest/ci/junit-lib-0.xml', 'nextest/ci/junit-doc.xml')

    def published_reports(self, workspace):
        """Files the real timing steps would read or upload from this workspace."""
        steps = self.build_steps()
        upload = next(s for s in steps if s.get('name') == 'Upload test timings')
        patterns = upload['with']['path'].split()
        self.assertEqual(patterns, ['target/test-timings.json', 'target/test-run.log',
                                    'target/nextest/ci/junit-*.xml'])
        budget = next(s for s in steps if s.get('name') == 'Test time budget')['run']
        for pattern in patterns + ['target/changed-files.txt']:
            self.assertIn(pattern, budget)
        return sorted(str(path.relative_to(workspace))
                      for pattern in patterns + ['target/changed-files.txt']
                      for path in workspace.glob(pattern))

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
        (paths.work / '.gitignore').write_text('/target/\n.env\n/build-cache/\n')
        # Every seeded target is warm: it holds the publishing run's reports
        # beside a build artifact that must survive.
        for name in self.STALE_REPORTS + ('debug/aida',):
            (paths.target / name).parent.mkdir(parents=True, exist_ok=True)
            (paths.target / name).write_text('previous run\n')
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
            # The step adds nothing to PATH: install-action owns that (see
            # the cargo-nextest location test).
            self.assertFalse(paths.path_file.exists())
            self.assertFalse((paths.cargo / 'bin').exists())
            # The previous run's reports are gone before the target is
            # visible; the build cache itself is intact.
            self.assertEqual(self.published_reports(paths.runner), [])
            self.assertFalse((paths.target / 'nextest').exists())
            self.assertEqual((paths.target / 'debug/aida').read_text(), 'previous run\n')
            # Neither the link nor a later build output dirties the tree or
            # trips the shard root-hygiene check.
            (paths.runner / 'target/test-run.log').write_text('log\n')
            self.assertEqual(self.published_reports(paths.runner), ['target/test-run.log'])
            self.assertEqual((paths.target / 'test-run.log').read_text(), 'log\n')
            listed = subprocess.check_output(
                ['git', '-C', str(paths.runner), 'ls-files', '--others', '--exclude-standard', '--directory'],
                text=True, timeout=10)
            self.assertEqual(listed, '')

    def nextest_location(self, root, installed, path):
        """Run the real location check with HOME and PATH under `root`."""
        import subprocess
        step = next(s for s in self.build_steps()
                    if s.get('name') == 'Verify self-hosted cargo-nextest location')
        self.assertEqual(step['if'], "runner.environment == 'self-hosted'")
        for directory in installed:
            (root / directory).mkdir(parents=True)
            (root / directory / 'cargo-nextest').write_text('#!/bin/sh\n')
            (root / directory / 'cargo-nextest').chmod(0o755)
        return subprocess.run(
            ['bash', '--noprofile', '--norc', '-e', '-o', 'pipefail', '-c', step['run']],
            env={'HOME': str(root / 'home'), 'CARGO_HOME': str(root / 'cargo'),
                 'PATH': ':'.join([str(root / d) for d in path] + ['/usr/bin', '/bin'])},
            capture_output=True, text=True, timeout=10)

    def test_cargo_nextest_must_be_the_install_action_copy_in_the_job_home(self):
        # install-action (main.sh at the pinned commit) installs into
        # CARGO_HOME/bin only when `cargo` itself resolves there; otherwise it
        # uses $HOME/.install-action/bin and appends that to GITHUB_PATH. The
        # controller puts the pinned /opt toolchain first on PATH and points
        # CARGO_HOME at the class work tree, so dell1 always takes the second
        # branch. trace:STORY-2695 | ai:claude
        import tempfile
        controller = (ROOT / 'scripts/runner/controller.py').read_text()
        self.assertIn("'PATH=/opt/aida-ci-toolchain/bin:/usr/local/bin:/usr/bin:/bin'", controller)
        self.assertIn("f'CARGO_HOME={work}/cargo'", controller)
        self.assertIn("f'HOME={state}/runner/home'", controller)
        names = [step.get('name') for step in self.heavy_self_hosted_steps()]
        install = names.index('Install cargo-nextest')
        self.assertEqual(names[install + 1], 'Verify self-hosted cargo-nextest location')
        self.assertEqual(names[install + 2], 'Run tests')
        action, cargo, host = 'home/.install-action/bin', 'cargo/bin', 'usr-local/bin'
        cases = {
            'install-action copy': ([action], [action], None),
            'install-action copy wins over a later one': ([action, host], [action, host], None),
            'not installed': ([], [action], 'cargo-nextest is not on PATH'),
            'only in CARGO_HOME/bin': ([cargo], [action, cargo], 'cargo nextest resolves to'),
            'seeded CARGO_HOME copy shadows it': ([action, cargo], [cargo, action], 'cargo nextest resolves to'),
            # cargo runs the CARGO_HOME/bin copy before it searches PATH.
            'seeded CARGO_HOME copy behind it on PATH': ([action, cargo], [action, cargo], 'cargo nextest resolves to'),
            'seeded CARGO_HOME copy off PATH': ([action, cargo], [action], 'cargo nextest resolves to'),
            'only in CARGO_HOME/bin, off PATH': ([cargo], [], 'cargo nextest resolves to'),
            'host copy shadows it': ([action, host], [host, action], 'cargo nextest resolves to'),
            'installed but not on PATH': ([action], [], 'cargo-nextest is not on PATH'),
        }
        for name, (installed, path, message) in cases.items():
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                result = self.nextest_location(Path(directory).resolve(), installed, path)
                if message is None:
                    self.assertEqual(result.returncode, 0, result.stderr)
                else:
                    self.assertNotEqual(result.returncode, 0, name)
                    self.assertIn(message, result.stderr, name)

    def test_adopt_clears_linked_reports_without_following_them(self):
        import os
        import tempfile
        def linked(paths):
            import shutil
            kept = paths.work.parent / 'kept'
            kept.mkdir()
            (kept / 'junit-old.xml').write_text('elsewhere\n')
            (paths.target / 'test-run.log').unlink()
            os.symlink(kept / 'junit-old.xml', paths.target / 'test-run.log')
            shutil.rmtree(paths.target / 'nextest')
            os.symlink(kept, paths.target / 'nextest')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            paths, result = self.adopt(root, mutate=linked)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.published_reports(paths.runner), [])
            self.assertFalse(os.path.lexists(paths.target / 'test-run.log'))
            self.assertFalse(os.path.lexists(paths.target / 'nextest'))
            self.assertEqual((root / 'work/kept/junit-old.xml').read_text(), 'elsewhere\n')

    def test_adopt_refuses_every_broken_source_contract(self):
        import os
        import tempfile
        def dirty(paths):
            (paths.work / 'Cargo.lock').write_text('changed\n')
        def untracked(paths):
            (paths.work / 'extra.rs').write_text('fn main() {}\n')
        def ignored_env(paths):
            (paths.work / '.env').write_text('TOKEN=from-the-host\n')
        def ignored_directory(paths):
            (paths.work / 'build-cache').mkdir()
            (paths.work / 'build-cache/artifact').write_text('unreviewed input\n')
        def locally_excluded(paths):
            (paths.work / '.git/info').mkdir(exist_ok=True)
            (paths.work / '.git/info/exclude').write_text('extra.rs\n')
            (paths.work / 'extra.rs').write_text('fn main() {}\n')
        def unclearable(paths):
            (paths.target / 'nextest/ci').chmod(0o500)
        def populated(paths):
            (paths.runner / 'stale').write_text('previous job\n')
        def target_present(paths):
            (paths.work / 'target').mkdir()
        def target_link(paths):
            os.symlink('/nonexistent', paths.work / 'target')
        def linked_target_dir(paths):
            import shutil
            shutil.rmtree(paths.target)
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
        def foreign_rustc(paths):
            (paths.tools / 'rustc').unlink()
            other = paths.tools.parent / 'other'
            other.mkdir()
            (other / 'rustc').write_text('#!/bin/sh\necho "rustc 1.0.0"\n')
            (other / 'rustc').chmod(0o755)
            return {'PATH': f'{paths.tools}:{other}:/usr/bin:/bin'}
        def silent_cargo(paths):
            (paths.tools / 'cargo').write_text('#!/bin/sh\necho "not a version"\n')
        def failing_cargo(paths):
            (paths.tools / 'cargo').write_text('#!/bin/sh\nexit 1\n')
        def unreadable_index(paths):
            # HEAD still resolves; only the comparison with it cannot run.
            (paths.work / '.git/index').write_bytes(b'not an index\n')
        def surviving_report(paths):
            # An `rm` that reports success without removing anything.
            other = paths.tools.parent / 'other'
            other.mkdir()
            (other / 'rm').write_text('#!/bin/sh\nexit 0\n')
            (other / 'rm').chmod(0o755)
            return {'PATH': f'{other}:{paths.tools}:/usr/bin:/bin'}
        cases = {
            'wrong sha': (dict(sha='0' * 40), 'this job is for'),
            'merge ref sha': (dict(sha='1' * 40), 'this job is for'),
            'missing workspace': (dict(env={'AIDA_CI_WORKSPACE': None}), 'AIDA_CI_WORKSPACE is not an absolute host-prepared path'),
            'relative workspace': (dict(env={'AIDA_CI_WORKSPACE': 'work/workspace'}), 'AIDA_CI_WORKSPACE is not an absolute host-prepared path'),
            'empty target': (dict(env={'CARGO_TARGET_DIR': ''}), 'CARGO_TARGET_DIR is not an absolute host-prepared path'),
            'absent cargo home': (dict(env={'CARGO_HOME': '/nonexistent/cargo'}), 'CARGO_HOME is not a host-prepared directory'),
            'linked target dir': (dict(mutate=linked_target_dir), 'CARGO_TARGET_DIR is not a host-prepared directory'),
            'no git': (dict(mutate=no_git), 'prepared source has no Git HEAD'),
            'nested': (dict(mutate=nested), 'not its own Git root'),
            'dirty': (dict(mutate=dirty), 'differs from the admitted commit'),
            'untracked': (dict(mutate=untracked), 'differs from the admitted commit'),
            'ignored .env': (dict(mutate=ignored_env), 'differs from the admitted commit'),
            'ignored directory': (dict(mutate=ignored_directory), 'differs from the admitted commit'),
            'locally excluded': (dict(mutate=locally_excluded), 'differs from the admitted commit'),
            'populated runner workspace': (dict(mutate=populated), 'runner workspace is not empty'),
            'target present': (dict(mutate=target_present), 'already has target'),
            'target link': (dict(mutate=target_link), 'already has target'),
            'foreign cargo': (dict(mutate=foreign_cargo), 'cargo is not the installed pinned toolchain'),
            'foreign rustc': (dict(mutate=foreign_rustc), 'rustc is not the installed pinned toolchain'),
            'cargo prints no version': (dict(mutate=silent_cargo), 'cargo did not report a version'),
            'cargo fails': (dict(mutate=failing_cargo), 'cargo did not report a version'),
            'unreadable index': (dict(mutate=unreadable_index), 'cannot compare prepared source'),
            'report survives clearing': (dict(mutate=surviving_report), 'previous report test-run.log survives'),
        }
        # Every refusal the step can raise has a case here.
        import re
        body = next(s for s in self.build_steps()
                    if s.get('name') == 'Adopt host-prepared exact source')['run']
        raised = re.findall(r'refuse "([^"]+)"', body)
        self.assertEqual(len(raised), 14, raised)
        covered = [message for _, message in cases.values()] + ['cannot clear previous reports']
        for refusal in raised:
            self.assertTrue(any(c in refusal or literal.strip() in c for c in covered
                                for literal in re.split(r'\$\w+', refusal) if literal.strip()), refusal)
        if os.geteuid() != 0:  # Root ignores the directory mode this case relies on.
            cases['unclearable reports'] = (dict(mutate=unclearable), 'cannot clear previous reports')
        for name, (arguments, message) in cases.items():
            with self.subTest(case=name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                paths, result = self.adopt(root, **arguments)
                if name == 'unclearable reports':
                    (paths.target / 'nextest/ci').chmod(0o700)
                # A refused (failed) warm job exposes none of the previous
                # run's reports to the timing and upload steps.
                self.assertEqual(self.published_reports(paths.runner), [], name)
                self.assertNotEqual(result.returncode, 0, name)
                self.assertIn('dell1 source contract refused', result.stderr, name)
                self.assertIn(message, result.stderr, name)
                # A refusal never leaves the runner workspace pointing at source.
                self.assertFalse(paths.runner.is_symlink(), name)
                self.assertFalse((paths.work / 'target').is_symlink() and name != 'target link', name)

    def test_host_mutating_setup_excludes_self_hosted(self):
        workflow = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
        sudo = [step['name'] for step in workflow['jobs']['build']['steps'] if 'sudo ' in step.get('run', '')]
        self.assertTrue(sudo)
        # Whole-condition evaluation: no step that can run on dell1 uses sudo.
        for step in self.heavy_self_hosted_steps():
            self.assertNotIn('sudo ', step.get('run', ''), step.get('name'))
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
