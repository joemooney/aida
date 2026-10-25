#!/usr/bin/env python3
"""Real CLI + shipped hook commands, with process fixtures for both vendors.

The fixtures model client lifecycle/payloads, not model output or a second
mail transport. Antigravity CLI 1.4.2 hooks.md pins the camelCase payload and
injectSteps shape. No API access or vendor credentials are used.
trace:STORY-1843.ac0eef56 | ai:codex
"""
import argparse
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time

WORKER = r'''
import json, os, subprocess, sys
binary, hook, wiring, vendor = sys.argv[1:]
wiring = json.loads(wiring)
def run(args, payload=None):
    p = subprocess.run(args, input=payload, text=True, capture_output=True, timeout=25)
    if p.returncode:
        raise RuntimeError((args, p.returncode, p.stdout, p.stderr))
    return p.stdout
def notice(event):
    if vendor == 'agy':
        body = {'conversationId':'agy-native-session', 'workspacePaths':[os.getcwd()],
                'modelName':'auto', 'invocationNum':2, 'initialNumSteps':3}
        name = 'aida-mail-notice' if event == 'SessionStart' else 'aida-mail-notice-turn'
        command = wiring[name][event][0]['command']
        return run(['sh', '-c', command], json.dumps(body))
    body = {'session_id':'claude-native-session', 'hook_event_name':event, 'cwd':os.getcwd()}
    return run(['sh', hook], json.dumps(body))
print(json.dumps(notice('SessionStart')), flush=True)
for line in sys.stdin:
    task = json.loads(line)
    if task['op'] == 'notice': out = notice('PreInvocation' if vendor == 'agy' else 'UserPromptSubmit')
    elif task['op'] == 'startup': out = notice('SessionStart')
    elif task['op'] == 'send': out = run([binary, 'mailbox', 'send', '--to', task['handle'], '--subject', 'cross-vendor-proof', task.get('body', 'hello agy from Claude')])
    elif task['op'] == 'inbox': out = run([binary, 'mailbox', 'inbox', '--format', 'human'])
    elif task['op'] == 'user':
        os.environ['AIDA_USER'] = task['identity']
        out = 'user updated'
    elif task['op'] == 'clear-role':
        os.environ.pop('AIDA_SESSION_ROLE', None)
        out = 'role cleared'
    else: raise RuntimeError(task)
    print(json.dumps(out), flush=True)
'''


def checked(args, root, env):
    p = subprocess.run([str(a) for a in args], cwd=root, env=env,
                       capture_output=True, text=True, timeout=30)
    assert p.returncode == 0, (args, p.stdout, p.stderr)
    return p.stdout


def list_agents(binary, root, env):
    # agent ls intentionally has no public JSON contract. In this CLI fixture
    # inspect its documented HANDLE/PID columns without reading registry files.
    table = checked([binary, 'agent', 'ls', '--format', 'human'], root, env)
    rows = []
    for line in table.splitlines():
        cells = line.split()
        if len(cells) >= 2 and cells[1].isdigit():
            rows.append({'name': cells[0], 'pid': int(cells[1])})
    return rows


def response(worker):
    assert select.select([worker.stdout], [], [], 30)[0], 'client fixture timed out'
    line = worker.stdout.readline()
    assert line, ('client exited', worker.poll())
    return json.loads(line)


def ask(worker, **task):
    worker.stdin.write(json.dumps(task) + '\n')
    worker.stdin.flush()
    return response(worker)


def notice_text(raw):
    obj = json.loads(raw)
    return '\n'.join(step.get('systemMessage', {}).get('systemMessage', '')
                     for step in obj.get('injectSteps', []))


def check(binary):
    source = Path(__file__).resolve().parents[1]
    wiring = json.loads((source / 'aida-core/templates/antigravity-hooks.json').read_text())
    with tempfile.TemporaryDirectory(prefix='story1843-pr3-') as temporary:
        base = Path(temporary)
        root, home, bin_dir = (base / p for p in ('project', 'home', 'bin'))
        for p in (root, home, bin_dir): p.mkdir()
        env = {k:v for k,v in os.environ.items()
               if not k.startswith(('AIDA_', 'GIT_', 'CLAUDE', 'CODEX', 'ANTIGRAVITY', 'GEMINI', 'XDG_'))}
        env.update(HOME=str(home), AIDA_HOME=str(home), XDG_CONFIG_HOME=str(home / '.config'),
                   XDG_CACHE_HOME=str(home / '.cache'), AIDA_TELEMETRY='0', AIDA_OS_WRAP='0',
                   GIT_AUTHOR_NAME='Fixture', GIT_AUTHOR_EMAIL='fixture@example.invalid',
                   GIT_COMMITTER_NAME='Fixture', GIT_COMMITTER_EMAIL='fixture@example.invalid',
                   PATH=str(bin_dir) + os.pathsep + env['PATH'])
        (bin_dir / 'aida').symlink_to(binary)
        checked(['git', 'init', '-q', '-b', 'main'], root, env)
        # trace:BUG-2017 | ai:codex — positive cross-vendor global-ID fixture.
        git_dir = root / '.git'
        assert git_dir.is_dir() and not git_dir.is_symlink()
        assert not (git_dir / 'commondir').exists()
        with (git_dir / 'aida-allocation.toml').open('x', encoding='utf-8') as setting:
            setting.write('version=1\nauthoritative=true\n')
        checked([binary, 'init', '--no-skills', '--no-hooks'], root, env)
        hook = root / '.agents/hooks/aida-mail-notice.sh'
        hook.parent.mkdir(parents=True, exist_ok=True)
        hook.write_text((source / 'aida-core/templates/hooks/aida-mail-notice.sh').read_text())
        hook.chmod(0o755)
        workers = []
        try:
            for vendor, agent_type in [('claude', 'claude'), ('agy', 'antigravity')]:
                script = base / vendor
                script.write_text(WORKER)
                client_env = dict(env, AIDA_AGENT_TYPE=agent_type, AIDA_SESSION_ROLE='implementer',
                                  AIDA_USER='fixture-' + vendor)
                if vendor == 'agy':
                    # A raw Agy launched by Claude keeps stale parent variables.
                    # Its named hook must register Agy, and subsequent CLI
                    # commands must use its own nearest process, not the parent.
                    parent_rows = list_agents(binary, root, env)
                    parent_handle = next(row['name'] for row in parent_rows if row['pid'] == workers[0].pid)
                    ask(workers[0], op='user', identity=parent_handle)
                    client_env.update(AIDA_AGENT_TYPE='claude', CLAUDE_PID=str(workers[0].pid),
                                      CLAUDE_CODE_SESSION_ID='claude-native-session',
                                      AIDA_HANDLE=parent_handle, AIDA_AGENT_NAME=parent_handle,
                                      AIDA_USER=parent_handle)
                worker = subprocess.Popen([sys.executable, str(script), binary, str(hook), json.dumps(wiring), vendor],
                                          cwd=root, env=client_env, text=True, stdin=subprocess.PIPE,
                                          stdout=subprocess.PIPE)
                workers.append(worker)
                response(worker)
            claude, agy = workers
            rows = list_agents(binary, root, env)
            by_pid = {row['pid']:row for row in rows}
            assert claude.pid in by_pid and agy.pid in by_pid, (rows, claude.pid, agy.pid)
            sender, recipient = by_pid[claude.pid]['name'], by_pid[agy.pid]['name']
            assert sender.startswith('claude-implementer-'), rows
            assert recipient.startswith('agy-implementer-'), rows
            assert 'mail' not in notice_text(ask(agy, op='notice')).lower()
            # Self-sent mail is excluded from inboxes, so the privacy control
            # comes from a third fixture identity through the real send CLI.
            checked([binary, 'mailbox', 'send', '--to', sender, '--from', 'fixture-operator',
                     '--subject', 'private-parent-control', 'parent-private-only'], root, env)
            parent_read = root / '.aida/mailbox/.read' / (sender + '.txt')
            assert not parent_read.exists(), 'fixture parent already acknowledged mail'
            assert 'unread mailbox' not in notice_text(ask(agy, op='notice')), 'Agy inherited parent private inbox'
            child_inbox = ask(agy, op='inbox')
            assert 'parent-private-only' not in child_inbox, child_inbox
            assert not parent_read.exists(), 'child acknowledged parent private mail'
            assert '1 mail' in ask(claude, op='notice'), 'parent mail stopped being unread'
            parent_inbox = ask(claude, op='inbox')
            assert 'parent-private-only' in parent_inbox, parent_inbox
            assert parent_read.exists(), 'parent inbox did not acknowledge its own mail'
            assert '1 mail' not in ask(claude, op='notice')
            sent = ask(claude, op='send', handle=recipient)
            assert 'sent' in sent, sent
            # The actual configured PreInvocation command and camelCase payload
            # must discover its own persisted handle despite stale parent aliases.
            first = notice_text(ask(agy, op='notice'))
            assert '1 unread mailbox message' in first and 'cross-vendor-proof' in first and sender in first, first
            assert '1 unread mailbox message' in notice_text(ask(agy, op='notice')), 'notice acknowledged private mail'
            assert '1 mail' not in ask(claude, op='notice'), 'sender read recipient private mail'
            inbox = ask(agy, op='inbox')
            assert 'hello agy from Claude' in inbox and sender in inbox, inbox
            assert 'unread mailbox' not in notice_text(ask(agy, op='notice')), 'explicit inbox did not acknowledge'
            # Shared role and vendor audiences remain visible even while the
            # child carries the parent's private AIDA_USER.
            for audience in ('implementer', 'antigravity'):
                checked([binary, 'mailbox', 'send', '--to', audience, '--from', 'fixture-operator',
                         '--subject', 'shared-' + audience, 'shared-body-' + audience], root, env)
            shared = notice_text(ask(agy, op='notice'))
            assert 'shared-implementer' in shared and 'shared-antigravity' in shared, shared
            shared_inbox = ask(agy, op='inbox')
            assert 'shared-body-implementer' in shared_inbox and 'shared-body-antigravity' in shared_inbox, shared_inbox
            assert 'unread mailbox' not in notice_text(ask(agy, op='notice'))

            # A known human AIDA_USER remains an intentional additional inbox.
            roster = root / '.aida-store/registry/team.toml'
            roster.parent.mkdir(parents=True, exist_ok=True)
            roster.write_text('[members]\nfixture-human = ["operator"]\n')
            ask(agy, op='user', identity='fixture-human')
            checked([binary, 'mailbox', 'send', '--to', 'fixture-human', '--from', 'fixture-operator',
                     '--subject', 'human-control', 'human-body'], root, env)
            assert 'human-control' in notice_text(ask(agy, op='notice'))
            assert 'human-body' in ask(agy, op='inbox')
            assert 'human-control' not in notice_text(ask(agy, op='notice'))

            # Resume the same native session with the actual inherited parent
            # aliases again; its handle and unread delivery must survive.
            ask(claude, op='send', handle=recipient, body='resume-private-body')
            checked([binary, 'mailbox', 'send', '--to', sender, '--from', 'fixture-operator',
                     '--subject', 'parent-after-resume', 'parent-resume-private'], root, env)
            parent_watermark = parent_read.read_text()
            agy.stdin.close()
            agy.wait(timeout=5)
            resumed = subprocess.Popen([sys.executable, str(script), binary, str(hook), json.dumps(wiring), 'agy'],
                                       cwd=root, env=client_env, text=True, stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE)
            workers.append(resumed)
            startup = notice_text(response(resumed))
            assert '1 mail' in startup and '2 mail' not in startup, startup
            resumed_rows = list_agents(binary, root, env)
            assert next(row['name'] for row in resumed_rows if row['pid'] == resumed.pid) == recipient, resumed_rows
            resumed_inbox = ask(resumed, op='inbox')
            assert 'resume-private-body' in resumed_inbox, resumed_inbox
            assert 'parent-resume-private' not in resumed_inbox, resumed_inbox
            assert parent_read.read_text() == parent_watermark, 'resumed child acknowledged parent mail'
            # With only parent-private mail pending, a second startup must
            # emit neither a private-mail count nor a shared-mail count.
            resumed.stdin.close()
            resumed.wait(timeout=5)
            resumed_again = subprocess.Popen([sys.executable, str(script), binary, str(hook), json.dumps(wiring), 'agy'],
                                             cwd=root, env=client_env, text=True, stdin=subprocess.PIPE,
                                             stdout=subprocess.PIPE)
            workers.append(resumed_again)
            parent_only_startup = notice_text(response(resumed_again))
            assert 'mail' not in parent_only_startup.lower(), parent_only_startup
            assert parent_read.read_text() == parent_watermark
            # The aged-mail warning is another startup notice path. Without
            # an explicit role it must not fall back to the parent's user.
            config = root / '.aida/config.toml'
            with config.open('a') as f:
                f.write('\n[mailbox]\nunread_warn_after = "1s"\n')
            time.sleep(1.1)
            ask(resumed_again, op='clear-role')
            no_role_startup = notice_text(ask(resumed_again, op='startup'))
            assert 'mail' not in no_role_startup.lower(), no_role_startup
            assert parent_read.read_text() == parent_watermark
            assert '1 mail' in ask(claude, op='notice')
            assert 'parent-resume-private' in ask(claude, op='inbox')
            assert 'unread mailbox' not in notice_text(ask(resumed_again, op='notice'))
            print('PASS STORY-1843.ac0eef56: inherited parent AIDA_USER isolation, child delivery/ack, shared/human audiences and resume')
        finally:
            for worker in workers:
                worker.stdin.close()
                try: worker.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    worker.terminate()
                    worker.wait(timeout=5)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--aida', required=True)
    check(str(Path(parser.parse_args().aida).resolve()))
