#!/usr/bin/env python3
"""Strict local forge transport for the two-node command-entry test.

Git objects and pushes are real; only the remote forge's API/CI is simulated.
Every request is logged; unsupported operations fail instead of implying success.
"""
import json
import os
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent
state_path = root / 'forge.json'
state = json.loads(state_path.read_text())
a = sys.argv[1:]
# Git expands insteadOf in `remote get-url`. Keep the forge identity visible
# while delegating every actual Git operation to the real local transport.
if pathlib.Path(__file__).name == 'git':
    if 'remote' in a and 'get-url' in a:
        result = subprocess.run([state['git'], *a], capture_output=True)
        output = result.stdout
        if output.strip() == state['hub'].encode():
            output = b'https://github.com/fixture/review.git\n'
        sys.stdout.buffer.write(output)
        sys.stderr.buffer.write(result.stderr)
        sys.exit(result.returncode)
    routed = [state['hub'] if arg == 'https://github.com/fixture/review.git' else arg for arg in a]
    os.execv(state['git'], [state['git'], *routed])
with (root / 'forge.log').open('a') as log:
    log.write(json.dumps(a) + '\n')


def git(*args):
    return subprocess.check_output([state['git'], '--git-dir', state['hub'], *args], text=True).strip()


def save():
    state_path.write_text(json.dumps(state))


def emit(value):
    print(json.dumps(value))
    sys.exit(0)


def option(name, default=None):
    return a[a.index(name) + 1] if name in a else default


def row(n):
    r = dict(state['prs'][str(n)])
    r['headRefOid'] = git('rev-parse', r['headRefName'])
    subprocess.check_call([state['git'], '--git-dir', state['hub'], 'update-ref',
                           f'refs/pull/{n}/head', r['headRefOid']])
    r.update(number=int(n), url=f'https://github.com/fixture/review/pull/{n}',
             isDraft=False, isCrossRepository=False, headRepository={'name': 'review'},
             mergedAt=None, mergeable='MERGEABLE', mergeStateStatus='CLEAN',
             labels=[], author={'login': 'author-a'}, reviewDecision='REVIEW_REQUIRED',
             latestReviews=[], commits=[], statusCheckRollup=[{'name': 'Build',
             'status': 'COMPLETED', 'conclusion': 'SUCCESS'}])
    return r


if a[:2] == ['pr', 'list']:
    wanted = option('--state', 'open').upper()
    rows = [row(n) for n in state['prs'] if wanted == 'ALL' or state['prs'][n]['state'] == wanted]
    if '--base' in a:
        rows = [r for r in rows if r['baseRefName'] == option('--base')]
    if '--head' in a:
        rows = [r for r in rows if r['headRefName'] == option('--head')]
    if '--search' in a:
        rows = [r for r in rows if option('--search').split()[0] in r['title']]
    jq = option('--jq') or option('-q')
    if jq:
        if jq == '.[] | [.number, .url, .headRefName, .baseRefName] | @tsv':
            columns = ['number', 'url', 'headRefName', 'baseRefName']
        elif jq == r'.[] | "\(.number)\t\(.title)\t\(.url)\t\(.headRefName)"':
            columns = ['number', 'title', 'url', 'headRefName']
        else:
            raise RuntimeError(f'unsupported list jq {jq}')
        for r in rows: print('\t'.join(str(r[k]) for k in columns))
        sys.exit(0)
    emit(rows)
if a[:2] == ['pr', 'view']:
    target = a[2] if len(a) > 2 and not a[2].startswith('-') else '7'
    n = target if target.isdigit() else next(n for n in state['prs'] if state['prs'][n]['headRefName'] == target)
    r = row(n)
    jq = option('--jq') or option('-q')
    if jq:
        if jq == '.headRefOid': print(r['headRefOid']); sys.exit(0)
        if jq in ['.state', '.body', '.baseRefName', '.headRefName']:
            print(r[jq[1:]]); sys.exit(0)
        if jq == '[.state, .mergeable, .reviewDecision, .headRefOid] | @tsv':
            print('\t'.join(r[k] for k in ['state', 'mergeable', 'reviewDecision', 'headRefOid'])); sys.exit(0)
        if jq == '[.title, (.commits[].messageHeadline)] | @tsv':
            print(r['title'] + '\t' + git('log', '--reverse', '--format=%s', r['baseRefName'] + '..' + r['headRefOid']).replace('\n', '\t')); sys.exit(0)
        if jq == '.commits[].messageHeadline':
            print(git('log', '--reverse', '--format=%s', r['baseRefName'] + '..' + r['headRefOid'])); sys.exit(0)
        raise RuntimeError(f'unsupported jq {jq}')
    emit(r)
if a[:2] == ['pr', 'create']:
    n = '70' if option('--head', '').startswith('train/') else '7'
    state['prs'][n] = dict(headRefName=option('--head'), baseRefName=option('--base'),
                          title=option('--title'), body=option('--body', ''), state='OPEN')
    save()
    print(f'https://github.com/fixture/review/pull/{n}')
    sys.exit(0)
if a[:2] == ['pr', 'checks']:
    emit([dict(name='Build', state='SUCCESS', bucket='pass', workflow='Build', link='https://ci.invalid')])
if a[:2] == ['pr', 'merge']:
    n = a[2]
    r = row(n)
    assert n == '70', 'only the train may land the member'
    assert option('--match-head-commit') == r['headRefOid'], a
    assert '--rebase' in a, a
    base = git('rev-parse', r['baseRefName'])
    assert git('rev-parse', r['headRefOid'] + '^') == base
    subprocess.check_call([state['git'], '--git-dir', state['hub'], 'update-ref',
                           'refs/heads/' + r['baseRefName'], r['headRefOid'], base])
    state['prs'][n]['state'] = 'MERGED'
    save()
    sys.exit(0)
if a[:2] in [['pr', 'comment'], ['pr', 'close']]:
    if a[1] == 'close': state['prs'][a[2]]['state'] = 'CLOSED'; save()
    sys.exit(0)
if a[0] == 'api':
    endpoint = next(x for x in a[1:] if x.startswith(('repos/', 'user')))
    if endpoint == 'user':
        if option('--jq') == '.login': print('operator'); sys.exit(0)
        emit({'login': 'operator'})
    if '/check-runs' in endpoint:
        sha = endpoint.split('/commits/')[1].split('/')[0]
        emit({'total_count': 1, 'check_runs': [dict(name='Build', head_sha=sha, status='completed', conclusion='success', html_url='https://ci.invalid')]})
    if '/status' in endpoint:
        sha = endpoint.split('/commits/')[1].split('/')[0]
        emit({'sha': sha, 'total_count': 0, 'statuses': []})
    if endpoint.endswith('/protection'): emit({'required_status_checks': {'contexts': ['Build'], 'checks': []}})
    if '/protection/required_status_checks' in endpoint: emit({'contexts': ['Build'], 'checks': []})
    if '/rules/branches/' in endpoint: emit([])
    if endpoint.endswith('/pulls/7'): emit(dict(number=7, head={'sha': row(7)['headRefOid']}, labels=[]))
    if endpoint == 'repos/fixture/review': emit({'default_branch': 'main', 'full_name': 'fixture/review'})
raise RuntimeError(f'unsupported forge request: {a}')
