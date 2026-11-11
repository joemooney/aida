#!/usr/bin/python3
"""Operator-installed dell1 admission, job gate and cache publisher.

No registration credentials, shell evaluation or workflow-supplied cache keys.
trace:STORY-2695 | ai:codex
"""
import base64
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tarfile
import threading
import time
import urllib.request
import uuid

REPO = 'joemooney/aida'
SLOTS = ('s0', 's1')
SHARDS = ('lib-0', 'lib-1', 'cli-0', 'cli-1')
FAMILIES = {'s0': ('lib-0', 'cli-1'), 's1': ('lib-1', 'cli-0')}


class Deferred(RuntimeError):
    pass
JOBS = Path('/var/lib/aida-ci-jobs')
CACHE = Path('/var/lib/aida-ci-cache')
CONFIG = Path('/etc/aida-ci')
DIST = Path('/opt/aida-actions-runner')
HOOK = Path('/usr/local/libexec/aida-ci-job-gate.sh')
RUNTIME = Path('/run')


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def trusted(path, directory=False):
    """Validate each ancestor as well as the final root-owned policy object."""
    path = Path(path)
    for entry in reversed((path, *path.parents)):
        info = entry.lstat()
        require(info.st_uid == 0 and not info.st_mode & 0o022,
                f'untrusted owner/mode: {entry}')
        require(stat.S_ISDIR(info.st_mode) if entry != path or directory
                else stat.S_ISREG(info.st_mode) and info.st_nlink == 1,
                f'non-regular policy/path: {entry}')
    return path


def traversal_directory(path, mode):
    """Create/validate host-owned traversal roots independently of sudo umask."""
    path.mkdir(mode=mode, exist_ok=True)
    trusted(path, directory=True)
    path.chmod(mode)
    return path


def host_json(path):
    trusted(path)
    return json.loads(path.read_text())


class Forge:
    def __init__(self):
        self.headers = {'Accept': 'application/vnd.github+json', 'User-Agent': 'aida-dell1-admission'}
        token = CONFIG / 'forge-read-token'
        if token.exists():
            trusted(token)
            require(stat.S_IMODE(token.stat().st_mode) == 0o600, 'forge read token must be 0600')
            value = token.read_text().strip()
            require(value and '\n' not in value, 'invalid forge read token')
            self.headers['Authorization'] = 'Bearer ' + value

    def get(self, route):
        # Read-only host evidence; optional dedicated read token never enters a unit.
        request = urllib.request.Request('https://api.github.com/repos/' + REPO + '/' + route,
                                         headers=self.headers)
        with urllib.request.urlopen(request, timeout=15) as response:
            data = response.read(16 * 1024 * 1024 + 1)
        require(len(data) <= 16 * 1024 * 1024, 'forge response too large')
        return json.loads(data)

    def file(self, path, sha):
        result = self.get(f'contents/{path}?ref={sha}')
        require(result['type'] == 'file' and result['encoding'] == 'base64', 'invalid forge file')
        return base64.b64decode(result['content'], validate=False)


def admit(forge, run_id, job_id, policy, trains, runner=None, completed=False):
    """Only independently fetched evidence and root-owned records grant a class."""
    require(type(run_id) is int and run_id > 0 and type(job_id) is int and job_id > 0,
            'invalid run/job identity')
    run = forge.get(f'actions/runs/{run_id}')
    job = forge.get(f'actions/jobs/{job_id}')
    require(run['id'] == run_id and job['id'] == job_id and job['run_id'] == run_id,
            'run/job mismatch')
    require(run['repository']['full_name'] == REPO and run['head_repository']['full_name'] == REPO,
            'foreign repository')
    sha, branch = run['head_sha'], run['head_branch']
    require(re.fullmatch('[0-9a-f]{40}', sha) is not None, 'invalid SHA')
    require(job['head_sha'] == sha and job['run_attempt'] == run['run_attempt'], 'head/attempt mismatch')
    require(run['path'] == '.github/workflows/ci.yml', 'unreviewed workflow path')
    require(job['name'] in [f'CI ({shard})' for shard in SHARDS], 'not a heavy shard')
    require(job['status'] == ('completed' if completed else 'in_progress' if runner else 'queued'),
            'unexpected job state')
    if runner:
        require(job['runner_name'] == runner, 'job assigned to another runner')
    if completed:
        require(job['conclusion'] == 'success', 'unsuccessful job cannot publish')
    if run['event'] == 'push':
        require(branch == 'main' and not run['pull_requests'], 'MAIN requires main push')
        kind = 'MAIN'
    else:
        require(run['event'] == 'pull_request' and branch.startswith('train/'), 'not TRAIN or MAIN')
        prs = run['pull_requests']
        require(len(prs) == 1, 'ambiguous train PR')
        number = prs[0]['number']
        pr = forge.get(f'pulls/{number}')
        require(pr['state'] == 'open' or completed, 'train PR is not open')
        require(pr['head']['repo']['full_name'] == REPO and pr['base']['repo']['full_name'] == REPO
                and pr['base']['ref'] == 'main' and pr['head']['ref'] == branch
                and pr['head']['sha'] == sha, 'PR head mismatch')
        wanted = {'repository': REPO, 'pr': number, 'sha': sha, 'branch': branch}
        require(sum(record == wanted for record in trains) == 1, 'missing/ambiguous host train record')
        kind = 'TRAIN'
    # A reviewed default-branch policy pins the *entire* workflow, not a YAML label.
    default = forge.get('commits/main')['sha']
    require(hashlib.sha256(forge.file('.github/workflows/ci.yml', default)).hexdigest()
            == policy['workflow_sha256'], 'default workflow policy drift')
    require(hashlib.sha256(forge.file('.github/workflows/ci.yml', sha)).hexdigest()
            == policy['workflow_sha256'], 'head workflow differs from reviewed policy')
    return {'class': kind, 'sha': sha, 'shard': job['name'][4:-1],
            'attempt': run['run_attempt'], 'run': run_id, 'job': job_id}


def copy_tree(source, destination):
    """Copy bytes, never hardlink or follow a job-controlled link (unit is dead)."""
    # The job can replace even target/cargo or their parent with a symlink.
    # Every source ancestor must be a real directory before root traverses it.
    for ancestor in (source, *source.parents):
        require(stat.S_ISDIR(ancestor.lstat().st_mode), f'cache source is not a directory: {ancestor}')
    require(not destination.exists(), 'copy destination exists')
    destination.mkdir(mode=0o700)
    for item in source.iterdir():
        info = item.lstat()
        target = destination / item.name
        if stat.S_ISDIR(info.st_mode):
            copy_tree(item, target)
        else:
            require(stat.S_ISREG(info.st_mode),
                    f'cache contains symlink/special file: {item}')
            require(info.st_uid == source.stat().st_uid, 'cache file has foreign owner')
            # O_NOFOLLOW closes the final-component symlink race as well.
            fd = os.open(item, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            with os.fdopen(fd, 'rb') as reader:
                opened = os.fstat(reader.fileno())
                require(stat.S_ISREG(opened.st_mode) and opened.st_uid == info.st_uid,
                        'cache file changed type/owner')
                with target.open('xb') as writer:
                    shutil.copyfileobj(reader, writer)
            target.chmod(0o700 if info.st_mode & 0o111 else 0o600)
            # Never make old compiler outputs newer than the fresh source
            # checkout: that could make Cargo reuse a stale workspace build.
            # Clamp future timestamps from a job; preserve ordinary fingerprints.
            now = time.time_ns()
            os.utime(target, ns=(min(info.st_atime_ns, now), min(info.st_mtime_ns, now)))


def key_for(slot, admission, policy, source):
    inputs = {}
    for path in sorted(source.rglob('*')):
        if stat.S_ISREG(path.lstat().st_mode) and (path.name in ('Cargo.lock', 'Cargo.toml', 'build.rs', 'rust-toolchain.toml')
                               or any(part in path.parts for part in ('.cargo', '.github', '.config', 'scripts'))
                               or path.name in ('Makefile', 'rust-toolchain')):
            inputs[str(path.relative_to(source))] = hashlib.sha256(path.read_bytes()).hexdigest()
    require('Cargo.lock' in inputs, 'missing Cargo.lock')
    value = {'slot': slot, 'shard': admission['shard'], 'rustc': policy['rustc_vv'],
             'target': policy['target'], 'profile_features': policy['profile_features'],
             'build_inputs': inputs, 'schema': 1}
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def seed(cache, key, work):
    manifest = cache / 'manifest.json'
    if not manifest.exists():
        return None
    record = host_json(manifest)
    if record['key'] != key:
        return None
    require(re.fullmatch('[0-9a-f]{32}', record['generation']) is not None, 'invalid generation')
    generation = trusted(cache / record['generation'], directory=True)
    for name in ('target', 'cargo'):
        copy_tree(generation / name, work / name)
    return record


def publish(cache, key, work, admission):
    require(admission['class'] == 'MAIN', 'TRAIN may never publish')
    generation = uuid.uuid4().hex
    staging = cache / ('.new-' + generation)
    staging.mkdir(mode=0o700)
    try:
        copy_tree(work / 'target', staging / 'target')
        require(stat.S_ISDIR((work / 'cargo').lstat().st_mode), 'Cargo home replaced by link')
        (staging / 'cargo').mkdir(mode=0o700)
        # Cargo config, credentials, binaries and rustup state never persist.
        for name in ('registry', 'git'):
            if (work / 'cargo' / name).exists():
                copy_tree(work / 'cargo' / name, staging / 'cargo' / name)
        staging.rename(cache / generation)
        record = {'generation': generation, 'key': key, 'sha': admission['sha'], 'time': time.time()}
        temporary = cache / '.manifest.new'
        temporary.write_text(json.dumps(record) + '\n')
        temporary.chmod(0o600)
        os.replace(temporary, cache / 'manifest.json')
        # The slot lock excludes seed readers, including other compatibility keys.
        for entry in cache.iterdir():
            if entry.name != generation and re.fullmatch(r'(?:[0-9a-f]{32}|\.new-[0-9a-f]{32})', entry.name):
                shutil.rmtree(entry)
        # Retain one published compatibility key per slot/shard. Preparation,
        # TRAIN and unsuccessful MAIN runs never retire trusted generations.
        for entry in cache.parent.iterdir():
            if entry != cache and re.fullmatch('[0-9a-f]{64}', entry.name):
                shutil.rmtree(entry)
        return record
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def source_tree(sha, destination):
    # Public archive fetched by the host, never from a cached .git/config.
    url = f'https://codeload.github.com/{REPO}/tar.gz/{sha}'
    with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': 'aida-ci'}), timeout=30) as response:
        data = response.read(256 * 1024 * 1024 + 1)
    require(len(data) <= 256 * 1024 * 1024, 'source archive too large')
    destination.mkdir(mode=0o700)
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
        total = 0
        links = []
        for member in archive:
            parts = Path(member.name).parts[1:]
            require('..' not in parts and not Path(member.name).is_absolute(), 'unsafe archive path')
            require(member.isdir() or member.isfile() or member.issym(), 'source archive link/special file')
            if not parts:
                continue
            target = destination.joinpath(*parts)
            require('.git' not in parts, 'source archive Git metadata')
            if member.issym():
                require(not Path(member.linkname).is_absolute(), 'absolute source link')
                resolved = (target.parent / member.linkname).resolve()
                require(resolved.is_relative_to(destination.resolve()), 'escaping source link')
                links.append((target, member.linkname))
            elif member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                total += member.size
                require(total <= 512 * 1024 * 1024, 'expanded source too large')
                target.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as reader, target.open('xb') as writer:
                    shutil.copyfileobj(reader, writer)
                target.chmod(0o755 if member.mode & 0o111 else 0o644)

        # Install links last: archive writes never traverse a link. Resolve again
        # after each insertion to reject chained escapes.
        for target, link in links:
            target.parent.mkdir(parents=True, exist_ok=True)
            require(target.parent.resolve().is_relative_to(destination.resolve()), 'escaping link parent')
            require((target.parent / link).resolve().is_relative_to(destination.resolve()), 'escaping source link')
            os.symlink(link, target)

    # Recreate Git identity independently; never reuse a cached repository/config.
    # No checkout filters or hooks run on the host: the archive supplies files,
    # fetch/reset only establish the exact object/index metadata needed by tests.
    git_env = {'PATH': '/usr/bin:/bin', 'HOME': '/nonexistent', 'GIT_CONFIG_NOSYSTEM': '1',
               'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_TERMINAL_PROMPT': '0'}
    for args in (['init', '--quiet'],
                 ['fetch', '--quiet', '--no-tags', '--depth=1', f'https://github.com/{REPO}.git', sha],
                 ['reset', '--quiet', '--mixed', sha]):
        subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', '-C', str(destination), *args],
                       env=git_env, check=True, timeout=45)
    actual = subprocess.check_output(['git', '-C', str(destination), 'rev-parse', 'HEAD'],
                                     env=git_env, timeout=5, text=True).strip()
    require(actual == sha, 'source identity mismatch')


def stop(unit):
    result = subprocess.run(['systemctl', 'stop', unit + '.service'], capture_output=True, text=True, timeout=45)
    require(result.returncode in (0, 5), f'cannot stop {unit}: {result.stderr}')
    if result.returncode == 0:
        result = subprocess.run(['systemctl', 'show', unit + '.service', '-p', 'ControlGroup', '--value'],
                                capture_output=True, text=True, check=True, timeout=5)
        group = result.stdout.strip()
        if group:
            events = Path('/sys/fs/cgroup' + group) / 'cgroup.events'
            require(not events.exists() or 'populated 1' not in events.read_text(), 'cgroup still populated')


class Gate:
    """Host socket is only reachable by this slot/class uid; no client claims trusted."""
    def __init__(self, path, uid, check, refuse=lambda: None):
        self.server = socket.socket(socket.AF_UNIX)
        self.server.bind(str(path))
        os.chown(path, uid, -1)
        os.chmod(path, 0o600)
        self.server.listen(1)
        self.server.settimeout(1)
        self.check = check
        self.refuse = refuse
        self.uid = uid
        self.accepted = False
        self.done = False
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while not self.done:
            try:
                connection, _ = self.server.accept()
            except socket.timeout:
                continue
            with connection:
                try:
                    _, uid, _ = struct.unpack('3i', connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    require(uid == self.uid and not self.accepted, 'invalid/repeated gate caller')
                    self.check()
                    self.accepted = True
                    connection.sendall(b'ALLOW\n')
                except Exception as error:
                    print(f'job gate refused: {error}', file=sys.stderr, flush=True)
                    # A hook's failed exit alone does not fence if: always()
                    # steps. Stop the whole unit before releasing the hook.
                    try:
                        self.refuse()
                    finally:
                        try:
                            connection.sendall(b'REFUSE\n')
                        except OSError:
                            pass

    def close(self):
        self.done = True
        self.thread.join(timeout=20)
        self.server.close()


def train_waiting(forge, trains, slot=None):
    for status in ('queued', 'in_progress'):
        runs = forge.get(f'actions/runs?event=pull_request&status={status}&per_page=100')['workflow_runs']
        require(len(runs) < 100, 'train queue truncated; refuse MAIN')
        for run in runs:
            if any(r['repository'] == REPO and r['sha'] == run['head_sha']
                   and r['branch'] == run['head_branch'] for r in trains):
                jobs = forge.get(f'actions/runs/{run["id"]}/jobs?per_page=100')['jobs']
                require(len(jobs) < 100, 'job queue truncated')
                if any(j['status'] == 'queued' and j['name'] in [f'CI ({s})' for s in (FAMILIES[slot] if slot else SHARDS)] for j in jobs):
                    return True
    return False


def assigned_job(forge, name, policy, trains):
    matches = []
    runs = forge.get('actions/runs?status=in_progress&per_page=100')['workflow_runs']
    require(len(runs) < 100, 'assigned run discovery truncated')
    for run in runs:
        jobs = forge.get(f'actions/runs/{run["id"]}/jobs?per_page=100')['jobs']
        require(len(jobs) < 100, 'assigned job discovery truncated')
        matches.extend((run['id'], job['id']) for job in jobs
                       if job.get('runner_name') == name and job['status'] == 'in_progress')
    require(len(matches) == 1, 'missing/ambiguous runner assignment')
    return admit(forge, *matches[0], policy, trains, runner=name)


def own_tree(path, account):
    for root, dirs, files in os.walk(path, topdown=False, followlinks=False):
        # Transfer each enclosing directory last: the live service cannot
        # replace an ancestor while root still visits its descendants.
        for entry in [*(Path(root) / name for name in dirs + files), Path(root)]:
            os.chown(entry, account.pw_uid, account.pw_gid, follow_symlinks=False)


def launch(slot, envelope):
    require(os.geteuid() == 0 and slot in SLOTS, 'root and enumerated slot required')
    # Root-owned /run lock cannot be replaced by either job uid.
    lock = os.open(RUNTIME / f'aida-ci-{slot}.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    policy = host_json(CONFIG / 'policy.json')
    require(policy['schema'] == 1 and policy['capacity_accepted'] is True,
            'operator combined-load capacity acceptance required')
    # The installed policy is an explicit S3 prerequisite, never inferred from YAML.
    require(policy['local_cache_only'] is True and policy['exact_checkout'] is True,
            'reviewed remote-cache disable and exact checkout contract required')
    trusted(Path('/opt/aida-ci-toolchain/bin/rustc'))
    version = subprocess.check_output(['/opt/aida-ci-toolchain/bin/rustc', '-Vv'], text=True,
                                      env={'PATH': '/usr/bin:/bin'}, timeout=10).strip()
    require(version == policy['rustc_vv'].strip(), 'installed rustc differs from reviewed policy')
    trains = host_json(CONFIG / 'trains.json')
    forge = Forge()
    admission = admit(forge, envelope['run'], envelope['job'], policy, trains)
    # Serialize admission/reservations across both slots, not execution.
    admission_lock = os.open(RUNTIME / 'aida-ci-admission.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    fcntl.flock(admission_lock, fcntl.LOCK_EX)
    reservation = RUNTIME / f'aida-ci-{slot}.json'
    require(not reservation.exists(), 'stale slot reservation; operator reconciliation required')
    for other in SLOTS:
        path = RUNTIME / f'aida-ci-{other}.json'
        if path.exists():
            require(host_json(path)['job'] != admission['job'], 'job already reserved')
    if admission['class'] == 'MAIN':
        if train_waiting(forge, trains, slot):
            os.close(admission_lock)
            os.close(lock)
            raise Deferred('TRAIN priority: queued train work exists')
    reservation.write_text(json.dumps(admission))
    reservation.chmod(0o600)
    os.close(admission_lock)
    state = gate = None
    try:
        name = envelope['runner']
        require(re.fullmatch(f'aida-ci-{slot}-[0-9a-f]{{32}}', name) is not None, 'invalid runner name')
        account = pwd.getpwnam(f'aida-ci-{slot}-{admission["class"].lower()}')
        accounts = [pwd.getpwnam(f'aida-ci-{s}-{c}') for s in SLOTS for c in ('main', 'train')]
        require(len({a.pw_uid for a in accounts}) == 4 and all(a.pw_uid != 0 for a in accounts),
                'four distinct non-root uids required')
        require(os.getgrouplist(account.pw_name, account.pw_gid) == [account.pw_gid], 'supplementary groups')
        unit = 'aida-ci-' + slot
        stop(unit)  # Must succeed before deleting or changing class.
        trusted(DIST, directory=True)
        for entry in DIST.rglob('*'):
            trusted(entry, directory=entry.is_dir())
        trusted(HOOK)
        trusted(Path('/usr/local/libexec/aida-ci-job-gate.py'))
        toolchain = Path('/opt/aida-ci-toolchain')
        for entry in toolchain.rglob('*'):
            trusted(entry, directory=entry.is_dir())
        traversal_directory(JOBS, 0o755)
        # Mandatory InaccessiblePaths must exist even on the first single-slot
        # launch. Keep both parents stable so later sibling jobs stay hidden.
        for sibling in SLOTS:
            traversal_directory(JOBS / sibling, 0o755)
        parent = JOBS / slot
        state = parent / 'current'
        if state.exists():
            shutil.rmtree(state)
        # Runner.Listener enumerates every ancestor before parsing arguments.
        # Root-owned current must permit service-uid reads as well as traversal.
        traversal_directory(state, 0o755)
        gate = None
        try:
            # Each class has a stable private view. Only these three trees persist.
            work = state / admission['class'].lower()
            work.mkdir(mode=0o700)
            shutil.copytree(DIST, state / 'runner', symlinks=False)
            (state / 'runner/home').mkdir(mode=0o700)
            own_tree(state / 'runner', account)
            cache = key = None
            runtime = RUNTIME / unit
            traversal_directory(runtime, 0o755)
            endpoint = runtime / 'gate.sock'
            endpoint.unlink(missing_ok=True)
            def verify_job():
                nonlocal admission, cache, key
                current_policy = host_json(CONFIG / 'policy.json')
                require(current_policy == policy, 'host policy changed during setup')
                current = assigned_job(forge, name, current_policy, host_json(CONFIG / 'trains.json'))
                require(current['class'] == admission['class'], 'assigned class changed')
                admission = current
                reservation.write_text(json.dumps(admission))
                source_tree(admission['sha'], work / 'workspace')
                key = key_for(slot, admission, policy, work / 'workspace')
                cache = CACHE / slot / admission['shard'] / key
                cache.mkdir(mode=0o700, parents=True, exist_ok=True)
                trusted(cache, directory=True)
                record = seed(cache, key, work)
                print(json.dumps({'slot': slot, 'admission': admission, 'seed': record}), flush=True)
                for directory in ('target', 'cargo', 'home'):
                    (work / directory).mkdir(mode=0o700, exist_ok=True)
                own_tree(work, account)
            gate = Gate(endpoint, account.pw_uid, verify_job, refuse=lambda: stop(unit))
            subprocess.run(['systemctl', 'start', 'aida-ci.slice'], check=True, timeout=15)
            subprocess.run(['systemctl', 'set-property', '--runtime', 'aida-ci.slice', 'CPUQuota=800%',
                            'AllowedCPUs=0-7', 'MemoryHigh=20G', 'MemoryMax=24G', 'MemorySwapMax=0'],
                           check=True, timeout=15)
            properties = {
                'User': account.pw_name, 'Group': account.pw_name, 'WorkingDirectory': str(state / 'runner'),
                'ProtectHome': 'yes', 'ProtectSystem': 'strict', 'ReadWritePaths': str(state),
                'PrivateTmp': 'yes', 'PrivateIPC': 'yes', 'PrivateDevices': 'yes', 'NoNewPrivileges': 'yes',
                'RestrictSUIDSGID': 'yes', 'ProtectKernelModules': 'yes', 'ProtectControlGroups': 'yes',
                'RemoveIPC': 'yes', 'KillMode': 'control-group', 'TimeoutStopSec': '30',
                'CPUAccounting': 'yes', 'MemoryAccounting': 'yes', 'CPUQuota': '400%', 'CPUWeight': '100', 'AllowedCPUs': '0-7',
                'MemoryHigh': '10G', 'MemoryMax': '12G', 'MemorySwapMax': '0',
                'InaccessiblePaths': f'-/run/docker.sock -/run/podman -/run/dbus {CACHE} {CONFIG} '
                                     + ' '.join(str(JOBS / s) for s in SLOTS if s != slot),
            }
            command = ['systemd-run', '--quiet', '--wait', '--pipe', '--collect',
                       '--unit=' + unit, '--slice=aida-ci.slice']
            for prop, value in properties.items():
                command += ['-p', f'{prop}={value}']
            command += ['/usr/bin/env', '-i', f'HOME={state}/runner/home', 'PATH=/opt/aida-ci-toolchain/bin:/usr/local/bin:/usr/bin:/bin',
                        f'CARGO_HOME={work}/cargo', f'CARGO_TARGET_DIR={work}/target',
                        f'AIDA_CI_WORKSPACE={work}/workspace',
                        f'AIDA_CI_GATE={endpoint}', f'ACTIONS_RUNNER_HOOK_JOB_STARTED={HOOK}',
                        'CARGO_BUILD_JOBS=4', 'RUST_TEST_THREADS=4', 'AIDA_REQUIRE_BWRAP_LIVE=1',
                        '/bin/bash', '-c', 'set -eu; IFS= read -r config; exec ./run.sh --jitconfig "$config"']
            started = time.monotonic()
            jit = envelope['jit']() if callable(envelope['jit']) else envelope['jit']
            result = subprocess.run(command, input=jit + '\n', text=True)
            stop(unit)  # No publisher traverses a tree until every descendant is dead.
            require(result.returncode == 0 and gate.accepted and time.monotonic() - started >= 10,
                    'runner failed, gate not accepted or short service lifetime')
            if admission['class'] == 'MAIN':
                current_policy = host_json(CONFIG / 'policy.json')
                require(current_policy == policy, 'host policy changed before publication')
                job = forge.get(f'actions/jobs/{admission["job"]}')
                if job['status'] != 'completed' or job['conclusion'] != 'success':
                    print('MAIN not successful/completed; cache unchanged', flush=True)
                    return
                final = admit(forge, admission['run'], admission['job'], current_policy,
                              host_json(CONFIG / 'trains.json'), runner=name, completed=True)
                require(final == admission, 'publication evidence changed')
                publish(cache, key, work, admission)
        finally:
            if gate:
                gate.close()
            stop(unit)  # If stop fails, retain all state for operator recovery.
            shutil.rmtree(state)
    finally:
        try:
            stop('aida-ci-' + slot)
        except Exception:
            raise  # Preserve reservation when cgroup death cannot be established.
        else:
            reservation.unlink(missing_ok=True)
            os.close(lock)


def main():
    require(len(sys.argv) == 2 and sys.argv[1] in SLOTS, 'Usage: controller.py s0|s1 < envelope.json')
    data = sys.stdin.buffer.readline(1024 * 1024 + 1)
    require(len(data) <= 1024 * 1024, 'envelope too large')
    envelope = json.loads(data)
    def registration():
        print(json.dumps({'ready': True}), flush=True)
        jit = sys.stdin.buffer.readline(1024 * 1024).decode().strip()
        require(jit, 'missing JIT')
        return jit
    envelope['jit'] = registration
    def terminate(signum, frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, terminate)
    launch(sys.argv[1], envelope)


if __name__ == '__main__':
    try:
        main()
    except Deferred as error:
        print(str(error), file=sys.stderr)
        sys.exit(75)
    except Exception as error:
        print(f'Runner refused: {error}', file=sys.stderr)
        sys.exit(1)
