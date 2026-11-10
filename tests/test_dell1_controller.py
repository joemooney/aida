"""Host-independent S2 controller evidence; real unit isolation is an S3 gate.
trace:STORY-2695 | ai:codex
"""
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('controller', ROOT / 'scripts/runner/controller.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
SHA = 'a' * 40
WORKFLOW = b'reviewed workflow with exact checkout and local caches only'
POLICY = dict(schema=1, workflow_sha256=hashlib.sha256(WORKFLOW).hexdigest(),
              rustc_vv='rustc pinned\nhost: x86_64-unknown-linux-gnu', target='x86_64-unknown-linux-gnu',
              profile_features='test default features', capacity_accepted=True,
              local_cache_only=True, exact_checkout=True)
RECORD = dict(repository=c.REPO, pr=42, sha=SHA, branch='train/example')


class Forge:
    def __init__(self, kind='TRAIN'):
        self.run = dict(id=10, repository=dict(full_name=c.REPO), head_repository=dict(full_name=c.REPO),
                        head_sha=SHA, head_branch='train/example' if kind == 'TRAIN' else 'main',
                        run_attempt=1, path='.github/workflows/ci.yml',
                        event='pull_request' if kind == 'TRAIN' else 'push',
                        pull_requests=[dict(number=42)] if kind == 'TRAIN' else [])
        self.job = dict(id=20, run_id=10, head_sha=SHA, run_attempt=1,
                        name='CI (lib-0)', status='queued', runner_name='runner', conclusion=None)
        self.pr = dict(state='open', head=dict(repo=dict(full_name=c.REPO), ref='train/example', sha=SHA),
                       base=dict(repo=dict(full_name=c.REPO), ref='main'))
        self.workflow = WORKFLOW
        self.runs = []
        self.jobs = []

    def get(self, route):
        if route == 'actions/runs/10':
            return self.run
        if route == 'actions/jobs/20':
            return self.job
        if route == 'pulls/42':
            return self.pr
        if route == 'commits/main':
            return dict(sha='b' * 40)
        if route.startswith('actions/runs?'):
            return dict(workflow_runs=self.runs)
        if route.startswith('actions/runs/10/jobs?'):
            return dict(jobs=self.jobs)
        raise AssertionError(route)

    def file(self, path, sha):
        return self.workflow


class Admission(unittest.TestCase):
    def test_train_and_main(self):
        for kind in ('TRAIN', 'MAIN'):
            self.assertEqual(c.admit(Forge(kind), 10, 20, POLICY, [RECORD])['class'], kind)

    def test_every_record_binding_required(self):
        for field, wrong in [('repository', 'fork/aida'), ('pr', 43), ('sha', 'c' * 40), ('branch', 'train/other')]:
            records = [dict(RECORD, **{field: wrong})]
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, 'host train record'):
                c.admit(Forge(), 10, 20, POLICY, records)
        for records in ([], [RECORD, RECORD]):
            with self.assertRaises(RuntimeError):
                c.admit(Forge(), 10, 20, POLICY, records)

    def test_forged_labels_branch_only_merge_group_and_ordinary_pr_refuse(self):
        for event, branch in [('merge_group', 'train/example'), ('pull_request_target', 'train/example'),
                              ('pull_request', 'feature'), ('push', 'train/example'), ('push', 'refs/heads/main')]:
            forge = Forge()
            forge.run.update(event=event, head_branch=branch)
            forge.job['labels'] = ['MAIN', 'TRAIN', 'aida-dell1']
            with self.subTest(event=event, branch=branch), self.assertRaises(RuntimeError):
                c.admit(forge, 10, 20, POLICY, [RECORD])

    def test_fork_and_foreign_pr_refuse(self):
        for obj, field in [('repository', 'full_name'), ('head_repository', 'full_name')]:
            forge = Forge()
            forge.run[obj][field] = 'fork/aida'
            with self.assertRaisesRegex(RuntimeError, 'foreign repository'):
                c.admit(forge, 10, 20, POLICY, [RECORD])
        forge = Forge()
        forge.pr['head']['repo']['full_name'] = 'fork/aida'
        with self.assertRaises(RuntimeError):
            c.admit(forge, 10, 20, POLICY, [RECORD])

    def test_mismatched_and_ambiguous_forge_evidence(self):
        mutations = [lambda f: f.run.update(id=11), lambda f: f.job.update(run_id=11),
                     lambda f: f.job.update(head_sha='c'*40), lambda f: f.job.update(run_attempt=2),
                     lambda f: f.run.update(pull_requests=[]), lambda f: f.run.update(pull_requests=[{}, {}]),
                     lambda f: f.pr['head'].update(sha='c'*40), lambda f: f.pr.update(state='closed'),
                     lambda f: f.run.update(path='evil.yml'), lambda f: f.job.update(name='CI (rest)')]
        for mutate in mutations:
            forge = Forge()
            mutate(forge)
            with self.assertRaises(RuntimeError):
                c.admit(forge, 10, 20, POLICY, [RECORD])

    def test_workflow_edits_and_policy_drift_refuse(self):
        forge = Forge()
        forge.workflow = b'PR edits routing or cache keys'
        with self.assertRaisesRegex(RuntimeError, 'policy drift'):
            c.admit(forge, 10, 20, POLICY, [RECORD])
        forge.file = lambda path, sha: WORKFLOW if sha == 'b'*40 else b'edited workflow'
        with self.assertRaisesRegex(RuntimeError, 'head workflow'):
            c.admit(forge, 10, 20, POLICY, [RECORD])

    def test_job_gate_binds_actual_runner_and_attempt(self):
        forge = Forge()
        forge.job['status'] = 'in_progress'
        self.assertEqual(c.admit(forge, 10, 20, POLICY, [RECORD], runner='runner')['sha'], SHA)
        with self.assertRaisesRegex(RuntimeError, 'another runner'):
            c.admit(forge, 10, 20, POLICY, [RECORD], runner='wrong')
        forge.job['run_attempt'] = 2
        with self.assertRaises(RuntimeError):
            c.admit(forge, 10, 20, POLICY, [RECORD], runner='runner')

    def test_publish_requires_completed_forge_success(self):
        forge = Forge('MAIN')
        for state, conclusion in [('in_progress', None), ('completed', 'failure'), ('completed', 'cancelled')]:
            forge.job.update(status=state, conclusion=conclusion)
            with self.assertRaises(RuntimeError):
                c.admit(forge, 10, 20, POLICY, [], runner='runner', completed=True)
        forge.job.update(status='completed', conclusion='success')
        self.assertEqual(c.admit(forge, 10, 20, POLICY, [], runner='runner', completed=True)['class'], 'MAIN')

    def test_train_priority_and_truncated_queue(self):
        forge = Forge()
        self.assertFalse(c.train_waiting(forge, [RECORD]))
        forge.runs = [forge.run]
        forge.jobs = [forge.job]
        self.assertTrue(c.train_waiting(forge, [RECORD]))
        forge.runs *= 100
        with self.assertRaisesRegex(RuntimeError, 'truncated'):
            c.train_waiting(forge, [RECORD])

    def test_api_error_is_refusal(self):
        forge = Forge()
        forge.get = lambda route: (_ for _ in ()).throw(OSError('unreachable forge'))
        with self.assertRaises(OSError):
            c.admit(forge, 10, 20, POLICY, [RECORD])


class Cache(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.work = self.root / 'work'
        for name in ('target/incremental', 'cargo/registry', 'cargo/git'):
            (self.work / name).mkdir(parents=True)
        (self.work / 'target/incremental/object').write_text('main object')
        (self.work / 'cargo/registry/download').write_text('crate')
        (self.work / 'cargo/credentials.toml').write_text('never cache')
        (self.work / 'cargo/config.toml').write_text('never cache')
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.admission = dict(**{'class': 'MAIN'}, sha=SHA)

    def test_private_copy_preserves_main_when_train_mutates_and_never_publishes(self):
        record = c.publish(self.cache, 'key', self.work, self.admission)
        train = self.root / 'train'
        train.mkdir()
        # Owner validation is tested separately; fixtures run without root.
        with patch.object(c, 'trusted', side_effect=lambda p, **kw: p):
            self.assertEqual(c.seed(self.cache, 'key', train), record)
        self.assertFalse((train / 'cargo/credentials.toml').exists())
        self.assertFalse((train / 'cargo/config.toml').exists())
        original = self.cache / record['generation'] / 'target/incremental/object'
        self.assertNotEqual(original.stat().st_ino, (train / 'target/incremental/object').stat().st_ino)
        (train / 'target/incremental/object').write_text('poison')
        self.assertEqual(original.read_text(), 'main object')
        with self.assertRaisesRegex(RuntimeError, 'TRAIN'):
            c.publish(self.cache, 'key', train, dict(self.admission, **{'class': 'TRAIN'}))
        self.assertEqual(json.loads((self.cache / 'manifest.json').read_text()), record)

    def test_target_timestamps_and_exec_bits_preserved_without_future_fingerprints(self):
        artifact = self.work / 'target/incremental/object'
        artifact.chmod(0o755)
        os.utime(artifact, ns=(1_000_000_000, 1_000_000_000))
        record = c.publish(self.cache, 'key', self.work, self.admission)
        cached = self.cache / record['generation'] / 'target/incremental/object'
        self.assertEqual(cached.stat().st_mtime_ns, 1_000_000_000)
        self.assertEqual(cached.stat().st_mode & 0o777, 0o700)
        future = time.time_ns() + 10**15
        os.utime(artifact, ns=(future, future))
        record = c.publish(self.cache, 'key', self.work, self.admission)
        cached = self.cache / record['generation'] / 'target/incremental/object'
        self.assertLess(cached.stat().st_mtime_ns, future)

    def test_cold_miss_and_key_mismatch(self):
        self.assertIsNone(c.seed(self.cache, 'key', self.work))
        c.publish(self.cache, 'key', self.work, self.admission)
        with patch.object(c, 'trusted', side_effect=lambda p, **kw: p):
            self.assertIsNone(c.seed(self.cache, 'wrong', self.work))

    def test_symlink_fifo_and_alias_refuse_without_changing_generation(self):
        record = c.publish(self.cache, 'key', self.work, self.admission)
        victim = self.work / 'target/unsafe'
        for kind in ('symlink', 'fifo'):
            if kind == 'symlink':
                victim.symlink_to(self.work / 'cargo')
            else:
                os.mkfifo(victim)
            with self.subTest(kind=kind), self.assertRaises(RuntimeError):
                c.publish(self.cache, 'key', self.work, self.admission)
            victim.unlink()
            self.assertEqual(json.loads((self.cache / 'manifest.json').read_text()), record)
        alias = self.root / 'alias'
        alias.symlink_to(self.work, target_is_directory=True)
        with self.assertRaises(RuntimeError):
            c.publish(self.cache, 'key', alias, self.admission)

    def test_cargo_hardlinked_artifacts_are_copied_to_independent_inodes(self):
        artifact = self.work / 'target/incremental/object'
        alias = self.work / 'target/hardlink'
        os.link(artifact, alias)
        record = c.publish(self.cache, 'key', self.work, self.admission)
        cached = self.cache / record['generation'] / 'target/hardlink'
        self.assertNotEqual(cached.stat().st_ino, artifact.stat().st_ino)
        artifact.write_text('changed')
        self.assertEqual(cached.read_text(), 'main object')

    def test_top_level_target_and_cargo_links_refuse(self):
        import shutil
        for name in ('target', 'cargo'):
            with self.subTest(name=name):
                source = self.work / name
                relocated = self.root / (name + '-elsewhere')
                source.rename(relocated)
                source.symlink_to(relocated, target_is_directory=True)
                with self.assertRaises(RuntimeError):
                    c.publish(self.cache, 'key', self.work, self.admission)
                source.unlink()
                relocated.rename(source)

    def test_all_compatibility_dimensions_cold_start(self):
        source = self.root / 'source'
        source.mkdir()
        (source / 'Cargo.lock').write_text('lock')
        (source / 'Cargo.toml').write_text('manifest')
        admission = dict(shard='lib-0')
        initial = c.key_for('s0', admission, POLICY, source)
        self.assertNotEqual(initial, c.key_for('s1', admission, POLICY, source))
        self.assertNotEqual(initial, c.key_for('s0', dict(shard='cli-0'), POLICY, source))
        for field in ('rustc_vv', 'target', 'profile_features'):
            self.assertNotEqual(initial, c.key_for('s0', admission, dict(POLICY, **{field: 'changed'}), source))
        (source / 'Cargo.lock').write_text('changed')
        self.assertNotEqual(initial, c.key_for('s0', admission, POLICY, source))

    def test_owner_mode_and_symlink_policy_refuse(self):
        path = self.root / 'policy.json'
        path.write_text('{}')
        path.chmod(0o666)
        with self.assertRaises(RuntimeError):
            c.trusted(path)
        alias = self.root / 'policy-alias'
        alias.symlink_to(path)
        with self.assertRaises(RuntimeError):
            c.trusted(alias)


class GateAndShell(unittest.TestCase):
    def test_actual_socket_gate_is_one_shot_and_hook_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            endpoint = Path(directory) / 'gate'
            with patch.object(c.os, 'chown'):
                gate = c.Gate(endpoint, os.getuid(), lambda: None)
            self.addCleanup(gate.close)
            hook = ROOT / 'scripts/runner/job-gate.py'
            env = dict(os.environ, AIDA_CI_GATE=str(endpoint))
            wrapper = (ROOT / 'scripts/runner/job-gate.sh').read_text().replace(
                '/usr/local/libexec/aida-ci-job-gate.py', str(hook))
            self.assertEqual(subprocess.run(['bash', '-c', wrapper], env=env, timeout=5).returncode, 0)
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(5)
                client.connect(str(endpoint))
                self.assertEqual(client.recv(32), b'REFUSE\n')
            # A hook transport failure must not return control to always steps.
            failed_env = dict(env, AIDA_CI_GATE=str(endpoint)+'-missing')
            child = subprocess.Popen(['python3', hook], env=failed_env, stderr=subprocess.DEVNULL)
            try:
                with self.assertRaises(subprocess.TimeoutExpired):
                    child.wait(timeout=0.2)
            finally:
                child.kill()
                child.wait(timeout=5)
            self.assertTrue(gate.accepted)

    def test_socket_gate_refuses_wrong_uid_and_failed_evidence(self):
        for uid, check in [(os.getuid()+1, lambda: None),
                           (os.getuid(), lambda: c.require(False, 'forge mismatch'))]:
            with tempfile.TemporaryDirectory() as directory:
                endpoint = Path(directory) / 'gate'
                with patch.object(c.os, 'chown'):
                    gate = c.Gate(endpoint, uid, check)
                try:
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(5)
                        client.connect(str(endpoint))
                        self.assertEqual(client.recv(32), b'REFUSE\n')
                    self.assertFalse(gate.accepted)
                finally:
                    gate.close()

    def test_launcher_rejects_extra_args_and_unenumerated_slot(self):
        script = (ROOT / 'scripts/runner/one-job.sh').read_text().replace('"$EUID"', '"0"')
        for args in ([], ['s2'], ['../s0'], ['s0', 'command']):
            result = subprocess.run(['bash', '-c', script, 'launcher', *args], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 2)
        script = script.replace('"0"', '"1001"')
        self.assertEqual(subprocess.run(['bash', '-c', script, 'launcher', 's0'],
                                       capture_output=True, timeout=5).returncode, 2)

    def test_stop_failure_and_populated_cgroup_refuse(self):
        with patch.object(c.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'stop failed')):
            with self.assertRaisesRegex(RuntimeError, 'stop failed'):
                c.stop('aida-ci-s0')
        results = [subprocess.CompletedProcess([], 0, '', ''),
                   subprocess.CompletedProcess([], 0, '/aida-ci-test\n', '')]
        with patch.object(c.subprocess, 'run', side_effect=results), patch.object(Path, 'exists', return_value=True), \
                patch.object(Path, 'read_text', return_value='populated 1\n'):
            with self.assertRaisesRegex(RuntimeError, 'populated'):
                c.stop('aida-ci-s0')

    def test_shell_syntax(self):
        subprocess.run(['bash', '-n', ROOT / 'scripts/runner/one-job.sh', ROOT / 'scripts/runner/rearm.sh', ROOT / 'scripts/runner/job-gate.sh'], check=True)


class LaunchIntegration(unittest.TestCase):
    """Run orchestration with a real gate/cache and fake systemd/public forge."""
    def exercise(self, kind, *, unit_failure=False, stop_failure=False, gate_failure=False):
        from contextlib import ExitStack
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            root = Path(directory)
            for name in ('jobs', 'cache', 'run', 'config', 'dist'):
                (root / name).mkdir()
            (root / 'dist/run.sh').write_text('fixture runner')
            (root / 'config/policy.json').write_text(json.dumps(POLICY))
            (root / 'config/trains.json').write_text(json.dumps([RECORD]))
            forge = Forge(kind)
            runner = 'aida-ci-s0-' + '1'*32
            forge.job['runner_name'] = runner
            accounts = {f'aida-ci-{slot}-{cls}': SimpleNamespace(pw_name=f'aida-ci-{slot}-{cls}',
                        pw_uid=10000+i*2+j, pw_gid=10000+i*2+j)
                        for i, slot in enumerate(c.SLOTS) for j, cls in enumerate(('main', 'train'))}
            account = accounts[f'aida-ci-s0-{kind.lower()}']
            commands = []
            stops = []

            def source(sha, path):
                self.assertEqual(sha, SHA)
                path.mkdir()
                (path / 'Cargo.lock').write_text('controlled lock')

            def stop(unit):
                stops.append(unit)
                if stop_failure and len(stops) >= 2:
                    raise RuntimeError('cannot stop previous unit')

            def system(command, **kwargs):
                commands.append(command)
                if command[0] == 'systemd-run':
                    forge.job['status'] = 'in_progress'
                    if gate_failure:
                        forge.job['runner_name'] = 'unrelated-pickup'
                    endpoint = root / 'run/aida-ci-s0/gate.sock'
                    with socket.socket(socket.AF_UNIX) as client:
                        client.settimeout(5)
                        client.connect(str(endpoint))
                        self.assertEqual(client.recv(32), b'REFUSE\n' if gate_failure else b'ALLOW\n')
                    if gate_failure:
                        self.assertGreaterEqual(len(stops), 2, 'refusal stops unit before replying')
                        return subprocess.CompletedProcess(command, 1)
                    work = root / f'jobs/s0/current/{kind.lower()}'
                    (work / 'target/incremental').mkdir()
                    (work / 'target/incremental/object').write_text('compiled delta')
                    (work / 'cargo/registry').mkdir()
                    (work / 'cargo/registry/crate').write_text('download')
                    forge.job.update(status='completed', conclusion='failure' if unit_failure else 'success')
                    return subprocess.CompletedProcess(command, 1 if unit_failure else 0)
                return subprocess.CompletedProcess(command, 0)

            for attribute, value in [('JOBS', root / 'jobs'), ('CACHE', root / 'cache'),
                                     ('RUNTIME', root / 'run'), ('CONFIG', root / 'config'), ('DIST', root / 'dist')]:
                stack.enter_context(patch.object(c, attribute, value))
            stack.enter_context(patch.object(c, 'trusted', side_effect=lambda p, **kw: p))
            stack.enter_context(patch.object(c.os, 'geteuid', return_value=0))
            stack.enter_context(patch.object(c.os, 'chown'))
            stack.enter_context(patch.object(c.os, 'getgrouplist', return_value=[account.pw_gid]))
            stack.enter_context(patch.object(c.pwd, 'getpwnam', side_effect=accounts.__getitem__))
            stack.enter_context(patch.object(c, 'Forge', return_value=forge))
            stack.enter_context(patch.object(c, 'source_tree', side_effect=source))
            stack.enter_context(patch.object(c, 'stop', side_effect=stop))
            stack.enter_context(patch.object(c.subprocess, 'check_output', return_value=POLICY['rustc_vv']))
            stack.enter_context(patch.object(c.subprocess, 'run', side_effect=system))
            # Kernel peer credentials are the fixture user's; production binds to
            # the actual distinct service uid. UID refusal has real socket tests.
            original_gate = c.Gate
            stack.enter_context(patch.object(c, 'Gate', side_effect=lambda p, uid, check, **kw: original_gate(p, os.getuid(), check, **kw)))
            clock = iter([0, 11])
            stack.enter_context(patch.object(c.time, 'monotonic', side_effect=lambda: next(clock)))
            if unit_failure or stop_failure or gate_failure:
                with self.assertRaises(RuntimeError):
                    c.launch('s0', dict(run=10, job=20, runner=runner, jit='secret-jit'))
            else:
                c.launch('s0', dict(run=10, job=20, runner=runner, jit='secret-jit'))
            manifests = list((root / 'cache').rglob('manifest.json'))
            self.assertEqual(len(manifests), int(kind == 'MAIN' and not unit_failure and not stop_failure and not gate_failure))
            self.assertEqual((root / 'jobs/s0/current').exists(), stop_failure)
            self.assertGreaterEqual(len(stops), 2)
            unit = next(command for command in commands if command[0] == 'systemd-run')
            for setting in ('CPUQuota=400%', 'CPUWeight=100', 'MemoryHigh=10G', 'MemoryMax=12G',
                            'MemorySwapMax=0', 'KillMode=control-group', 'ProtectHome=yes',
                            'PrivateIPC=yes', 'NoNewPrivileges=yes', 'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4'):
                self.assertIn(setting, unit)
            self.assertIn('User=' + account.pw_name, unit)
            hidden = next(arg for arg in unit if arg.startswith('InaccessiblePaths='))
            self.assertIn(str(root / 'jobs/s1'), hidden)
            self.assertIn(str(root / 'cache'), hidden)
            self.assertNotIn('secret-jit', ' '.join(unit))
            self.assertFalse(any('RuntimeMaxSec=' in arg for arg in unit))

    def test_successful_main_publishes_only_after_stop(self):
        self.exercise('MAIN')

    def test_successful_train_never_publishes(self):
        self.exercise('TRAIN')

    def test_unexpected_job_pickup_stops_unit_before_denying_hook(self):
        self.exercise('MAIN', gate_failure=True)

    def test_failed_main_never_publishes(self):
        self.exercise('MAIN', unit_failure=True)

    def test_stop_failure_retains_state_and_never_publishes(self):
        self.exercise('MAIN', stop_failure=True)


class Selection(unittest.TestCase):
    def test_slots_propose_disjoint_jobs_with_train_priority(self):
        spec = importlib.util.spec_from_file_location('selector', ROOT / 'scripts/runner/select-job.py')
        selector = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(selector)
        runs = [dict(id=1, event='push', head_branch='main'),
                dict(id=2, event='pull_request', head_branch='train/example'),
                dict(id=3, event='pull_request', head_branch='feature')]
        jobs = [dict(id=i+10, name=f'CI ({shard})', status='queued') for i, shard in enumerate(c.SHARDS)]
        def read(route):
            if 'status=queued' in route:
                return dict(workflow_runs=runs)
            if 'status=in_progress' in route:
                return dict(workflow_runs=[])
            return dict(jobs=jobs)
        first = selector.propose('s0', read)
        second = selector.propose('s1', read)
        self.assertEqual(first['run'], 2)
        self.assertEqual(second['run'], 2)
        self.assertNotEqual(first['job'], second['job'])
        self.assertEqual(first['class'], 'TRAIN')
        self.assertIsNone(selector.propose('s0', lambda route: dict(workflow_runs=[])))
        with self.assertRaisesRegex(RuntimeError, 'truncated'):
            selector.propose('s0', lambda route: dict(workflow_runs=runs*100))


class Rearm(unittest.TestCase):
    def test_fresh_jit_stdin_and_failures_stop_supervision(self):
        for status in (0, 1):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                selector = root / 'selector.py'
                selector.write_text('import json,sys; assert sys.argv[1] == "s0"; '
                                    'print(json.dumps(dict(run=10,job=20,**{"class":"TRAIN"})))')
                received = root / 'received'
                args = root / 'args'
                count = root / 'count'
                commands = {
                    'gh': f'echo register >> "{count}"; echo secret-jit',
                    'jq': 'cat >/dev/null; echo TRAIN',
                    'sudo': f'printf "%s\\n" "$@" > "{args}"; cat > "{received}"; exit {status}',
                }
                for name, body in commands.items():
                    path = root / name
                    path.write_text('#!/bin/sh\n' + body + '\n')
                    path.chmod(0o755)
                script = (ROOT / 'scripts/runner/rearm.sh').read_text()
                script = script.replace('"$EUID"', '"1001"').replace('/usr/local/libexec/aida-ci-select-job.py', str(selector))
                result = subprocess.run(['bash', '-c', script, 'rearm', 's0'],
                                        env={'PATH': f'{root}:/usr/bin:/bin'}, capture_output=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(count.read_text().splitlines(), ['register'])
                envelope = json.loads(received.read_text())
                self.assertEqual(envelope['jit'], 'secret-jit')
                self.assertEqual(envelope['job'], 20)
                self.assertRegex(envelope['runner'], '^aida-ci-s0-[0-9a-f]{32}$')
                self.assertEqual(args.read_text().splitlines(), ['-n', '/usr/local/sbin/aida-ci-one-job', 's0'])


class Source(unittest.TestCase):
    def test_archive_identity_is_independent_and_traversal_or_links_refuse(self):
        import io
        import tarfile
        from contextlib import ExitStack
        for kind in ('regular', 'traversal', 'symlink', 'fifo'):
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode='w:gz') as archive:
                info = tarfile.TarInfo('prefix/Cargo.lock' if kind != 'traversal' else 'prefix/../escape')
                if kind == 'symlink':
                    info.type = tarfile.SYMTYPE
                    info.linkname = '/etc/passwd'
                elif kind == 'fifo':
                    info.type = tarfile.FIFOTYPE
                archive.addfile(info, io.BytesIO(b''))
            with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
                stack.enter_context(patch.object(c.urllib.request, 'urlopen', return_value=io.BytesIO(stream.getvalue())))
                runner = stack.enter_context(patch.object(c.subprocess, 'run'))
                stack.enter_context(patch.object(c.subprocess, 'check_output', return_value=SHA+'\n'))
                source = Path(directory) / 'workspace'
                if kind == 'regular':
                    c.source_tree(SHA, source)
                    self.assertTrue((source / 'Cargo.lock').exists())
                    calls = [call.args[0] for call in runner.call_args_list]
                    self.assertEqual(len(calls), 3)
                    self.assertIn(SHA, calls[1])
                    self.assertNotIn('checkout', str(calls))
                    self.assertIn('core.hooksPath=/dev/null', calls[0])
                else:
                    with self.assertRaises(RuntimeError):
                        c.source_tree(SHA, source)
                    runner.assert_not_called()


if __name__ == '__main__':
    unittest.main()
