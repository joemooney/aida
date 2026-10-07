#!/usr/bin/env python3
"""Deterministic workflow contracts and shell behavior; no GitHub side effects.

trace:TASK-1588 | ai:codex
"""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

import yaml

ROOT = Path(__file__).resolve().parents[1]
workflow = yaml.safe_load((ROOT / '.github/workflows/cross-platform.yml').read_text())
# PyYAML's YAML 1.1 resolver reads the Actions 'on' key as True.
triggers = workflow.get('on', workflow.get(True))
jobs = workflow['jobs']
assert triggers['schedule'] == [{'cron': '0 6 * * 0'}]
assert jobs['build']['strategy']['matrix'] == {'os': ['windows-latest']}
assert triggers['pull_request']['branches'] == ['main']
assert 'aida-core/src/db/git_backend.rs' in triggers['pull_request']['paths']
assert '.github/workflows/**' in triggers['pull_request']['paths']
for name in ('build', 'gitlab-forge-smoke', 'history-index-sweep'):
    assert jobs[name]['needs'] == 'validate-main'
    checkout = jobs[name]['steps'][0]
    ref = checkout['with']['ref']
    if name == 'build':
        assert ref == "${{ github.event_name == 'pull_request' && github.ref || 'main' }}"
    else:
        assert ref == 'main'
guard = jobs['validate-main']['steps'][0]['run']
for event, ref, code in [('workflow_dispatch', 'refs/heads/main', 0),
                         ('workflow_dispatch', 'refs/heads/topic', 1),
                         ('workflow_dispatch', 'refs/tags/main', 1),
                         ('pull_request', 'refs/pull/1/merge', 0),
                         ('schedule', 'refs/heads/main', 0)]:
    result = subprocess.run(['bash', '-e', '-c', guard], text=True, capture_output=True,
                            env={**os.environ, 'GITHUB_EVENT_NAME': event, 'GITHUB_REF': ref})
    assert result.returncode == code, result
    if code:
        assert '::error::' in result.stdout and '--ref main' in result.stdout
failure = jobs['notify-failure']
assert failure['needs'] == 'build'
assert failure['if'] == "failure() && needs.build.result == 'failure' && (github.event_name == 'schedule' || (github.event_name == 'workflow_dispatch' && inputs.exercise_tracking_issue))"
assert jobs['notify-recovery']['if'] == "success() && github.event_name == 'schedule'"
assert jobs['notify-recovery']['needs'] == 'build'
with (ROOT / '.aida/config.toml').open('rb') as f:
    config = tomllib.load(f)
assert config['ci']['informational_workflows'] == ['Cross-platform*']
reminder = next(j for j in config['schedule']['jobs'] if j['name'] == 'weekly-windows-triage')
assert reminder['enabled'] and reminder['every'] == '24h' and reminder['seats'] == ['product']
assert 'command' not in reminder and 'on' not in reminder
for text in ('latest scheduled Windows', 'cross-platform-red', 'issue number', 'no open issue',
             'aida schedule done weekly-windows-triage', 'Do not queue implementation', 'Linux work'):
    assert text in reminder['prompt'], text
assert 'MAX_AGE_HOURS=24' in (ROOT / 'scripts/pre-release-check.sh').read_text()
ci = yaml.safe_load((ROOT / '.github/workflows/ci.yml').read_text())
assert ci['jobs']['build']['strategy']['matrix'] == {'os': ['ubuntu-latest']}
assert (ROOT / '.github/workflows/merge-hold-gate.yml').exists()

# Execute the notifier scripts with a logging gh stub, including migration of
# an existing nightly issue and recovery of every open tracking issue.
with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    gh = tmp / 'gh'
    gh.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ['GH_LOG'], 'a') as f:
    f.write(json.dumps(args) + '\\n')
if args[:2] == ['issue', 'list']:
    print(os.environ.get('OPEN_ISSUES', ''))
''')
    gh.chmod(0o755)
    log = tmp / 'log'
    env = {**os.environ, 'PATH': f'{tmp}:{os.environ["PATH"]}',
           'GH_LOG': str(log), 'RUN_URL': 'https://example.test/run/42'}
    def run_notifier(name, issues):
        log.write_text('')
        subprocess.run(['bash', '-e', '-o', 'pipefail', '-c', jobs[name]['steps'][-1]['run']],
                       env={**env, 'OPEN_ISSUES': issues}, check=True)
        return [json.loads(line) for line in log.read_text().splitlines()]
    for issues, action in [('', 'create'), ('123', 'edit')]:
        calls = run_notifier('notify-failure', issues)
        mutation = next(c for c in calls if c[:2] == ['issue', action])
        assert mutation[mutation.index('--title') + 1] == 'Weekly cross-platform CI is red'
        body = mutation[mutation.index('--body') + 1]
        assert 'weekly Sunday 06:00 UTC' in body and env['RUN_URL'] in body
        assert 'nightly' not in body.lower()
        if issues:
            assert mutation[2] == '123'
            assert any(c[:3] == ['issue', 'comment', '123'] for c in calls)
        else:
            assert mutation[mutation.index('--label') + 1] == 'cross-platform-red'
    calls = run_notifier('notify-recovery', '123\n456')
    assert [c[2] for c in calls if c[:2] == ['issue', 'close']] == ['123', '456']
    assert not any(c[:2] == ['issue', 'close'] for c in run_notifier('notify-recovery', ''))

    # An old green must dispatch main and wait for a new green, not reuse it.
    gh.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
log = Path(os.environ['GH_LOG'])
prior = log.read_text() if log.exists() else ''
with log.open('a') as f:
    f.write(json.dumps(args) + '\\n')
if args[:2] == ['run', 'list']:
    if '--event' in args:
        print('42' if '"workflow", "run"' in prior else '41')
    else:
        print('completed\\tsuccess\\t2000-01-01T00:00:00Z\\t41\\thttps://example.test/run/41')
''')
    sleep = tmp / 'sleep'
    sleep.write_text('#!/bin/sh\nexit 0\n')
    sleep.chmod(0o755)
    log.write_text('')
    result = subprocess.run(['bash', str(ROOT / 'scripts/pre-release-check.sh')],
                            env=env, check=True, capture_output=True, text=True)
    calls = [json.loads(line) for line in log.read_text().splitlines()]
    assert ['workflow', 'run', 'cross-platform.yml', '--ref', 'main'] in calls
    assert ['run', 'watch', '42', '--exit-status'] in calls
    assert 'green but stale' in result.stdout
print('PASS: weekly Windows policy, dispatch refusal, issue lifecycle, Product reminder, stale release refresh')
