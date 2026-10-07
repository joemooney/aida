#!/usr/bin/env python3
"""Ship refuses before touching CI; isolated fake gh never watches or merges.

Run: python3 tests/test_pr_ship_preflight.py --aida /path/to/aida
trace:TASK-1606 | ai:codex
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

FAKE_GH = r'''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['SHIP_CALLS'], 'a') as f: f.write(json.dumps(args) + '\n')
case = os.environ['SHIP_CASE']
head = os.environ['SHIP_HEAD']
if args[:2] == ['pr', 'merge']:
    sys.exit('unexpected merge')
if args[:2] == ['pr', 'checks']:
    if case not in ('timeout', 'registration', 'red', 'postflight', 'definition_override'):
        sys.exit('unexpected CI read before refusal')
    if '--watch' in args: sys.exit('unexpected unbounded watch')
    if '--json' in args:
        bucket = 'fail' if case == 'red' else 'pass' if case == 'postflight' else 'pending'
        print(json.dumps([{'name':'Build (ubuntu-latest)', 'workflow':'CI', 'bucket':bucket}]))
        if case == 'postflight' and '--required' not in args:
            Path('.aida/review-verdicts').mkdir(parents=True, exist_ok=True)
            Path('.aida/review-verdicts/PR-7.json').write_text(json.dumps({'verdict':'approved', 'reviewed_sha':'b'*40, 'recorded_by':'reviewer'}))
    elif case == 'registration':
        print('no checks reported', file=sys.stderr); sys.exit(1)
    else: print('Build pending')
    sys.exit(0)
if args[:2] == ['pr', 'list']:
    print('[]' if '--base' in args else json.dumps([{'number':7, 'url':'https://github.com/test/repo/pull/7', 'headRefName':'topic', 'baseRefName':'main', 'title':'Fixture'}]))
elif args[:2] == ['pr', 'view']:
    fields = args[args.index('--json')+1] if '--json' in args else ''
    if fields == 'state,mergeable,reviewDecision,headRefOid':
        print('OPEN\t' + ('CONFLICTING' if case == 'conflict' else 'MERGEABLE') + '\t\t' + head)
    elif fields == 'baseRefName': print('main')
    elif fields == 'headRefOid': print(head)
    else:
        labels = [{'name':'aida:merge-hold'}] if case == 'hold' else []
        print(json.dumps({'number':7, 'url':'https://github.com/test/repo/pull/7', 'state':'OPEN', 'headRefName':'topic', 'baseRefName':'main', 'headRefOid':head, 'title':'Fixture', 'isDraft':False, 'labels':labels}))
else: print('{}')
'''


def check(binary):
    cases = {'conflict':22, 'approval':23, 'corpus':23, 'definition':24,
             'hold':25, 'marker':25, 'definition_override':21, 'timeout':21, 'registration':21, 'red':20, 'postflight':23}
    for case, expected in cases.items():
        with tempfile.TemporaryDirectory(prefix='ship-preflight-') as temp:
            root = Path(temp)
            home = root / 'home'
            home.mkdir()
            bins = root / 'bin'
            bins.mkdir()
            gh = bins / 'gh'
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            env = {k:v for k,v in os.environ.items() if not k.startswith(('AIDA_', 'GIT_'))}
            env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
                       PATH=str(bins) + ':' + env['PATH'], SHIP_CASE=case, SHIP_CALLS=str(root/'calls'),
                       GIT_AUTHOR_NAME='Test', GIT_AUTHOR_EMAIL='test@example.invalid',
                       GIT_COMMITTER_NAME='Test', GIT_COMMITTER_EMAIL='test@example.invalid', AIDA_TELEMETRY='0')
            def git(*args):
                return subprocess.check_output(['git', *args], cwd=root, env=env, stderr=subprocess.DEVNULL).decode().strip()
            git('init', '-q', '-b', 'main')
            (root/'.aida').mkdir()
            (root/'.aida/config.toml').write_text('[forge]\nprovider = "github"\n')
            (root/'.github/workflows').mkdir(parents=True)
            workflow = root/'.github/workflows/ci.yml'
            workflow.write_text('on: [pull_request]\njobs: {}\n')
            git('add', '.'); git('commit', '-qm', 'fixture')
            env['SHIP_HEAD'] = git('rev-parse', 'HEAD')
            git('branch', 'topic')
            if case in ('definition', 'definition_override'):
                workflow.write_text('on: [pull_request]\njobs: {build: {}}\n')
                git('add', '.'); git('commit', '-qm', 'stricter workflow')
            git('update-ref', 'refs/remotes/origin/main', git('rev-parse', 'HEAD'))
            git('checkout', '-q', 'topic')
            git('remote', 'add', 'origin', 'https://github.com/test/repo.git')
            if case in ('approval', 'corpus'):
                directory = root/'.aida/review-verdicts'
                directory.mkdir()
                verdict = {'verdict':'approved', 'reviewed_sha':env['SHIP_HEAD'] if case == 'corpus' else 'b'*40,
                           'recorded_by':'reviewer', 'recorded_at':'2026-10-06T00:00:00Z'}
                if case == 'corpus':
                    verdict['rounds'] = [{'verdict':'approved', 'reviewed_sha':env['SHIP_HEAD']}]
                (directory/'PR-7.json').write_text(json.dumps(verdict))
            if case == 'marker':
                directory = root/'.aida/merge-holds'
                directory.mkdir()
                (directory/'PR-7').write_text('operator hold')
            started = time.monotonic()
            flags = ['--override-stale-check'] if case == 'definition_override' else []
            result = subprocess.run([str(binary), 'pr', 'ship', '7', '--no-trailer-check', '--no-pull',
                                     '--no-cleanup', '--wait', '1', *flags], cwd=root, env=env,
                                    text=True, capture_output=True, timeout=12)
            elapsed = time.monotonic() - started
            calls = [json.loads(line) for line in (root/'calls').read_text().splitlines()]
            assert result.returncode == expected, (case, result.returncode, result.stdout, result.stderr, calls)
            assert not any(c[:2] == ['pr','merge'] for c in calls), calls
            if case in ('conflict','approval','corpus','definition','hold','marker'):
                assert not any(c[:2] == ['pr','checks'] for c in calls), (case, calls)
                assert 'step 2: watching' not in result.stderr
            if case in ('timeout', 'registration'):
                assert 1 <= elapsed < 4, elapsed
            print(f'PASS {case}: exit {expected}, {elapsed:.2f}s', flush=True)
    help_text = subprocess.check_output([str(binary), 'pr','ship','--help'], text=True)
    for code in cases.values(): assert str(code) in help_text
    assert '--wait [<SECS>]' in help_text and '300 seconds' in help_text
    print('PASS help exit-code table', flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--aida', type=Path, required=True)
    check(parser.parse_args().aida.resolve())
