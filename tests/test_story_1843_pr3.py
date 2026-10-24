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
    elif task['op'] == 'send': out = run([binary, 'mailbox', 'send', '--to', task['handle'], '--subject', 'cross-vendor-proof', task.get('body', 'hello agy from Claude')])
    elif task['op'] == 'inbox': out = run([binary, 'mailbox', 'inbox', '--format', 'human'])
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
                    client_env.update(AIDA_AGENT_TYPE='claude', CLAUDE_PID=str(workers[0].pid),
                                      CLAUDE_CODE_SESSION_ID='claude-native-session',
                                      AIDA_HANDLE=parent_handle, AIDA_AGENT_NAME=parent_handle)
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
            assert 'unread mailbox' not in notice_text(ask(agy, op='notice')), 'Agy inherited parent private inbox'
            parent_inbox = ask(claude, op='inbox')
            assert 'parent-private-only' in parent_inbox, parent_inbox
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
            print('PASS STORY-1843.ac0eef56: Claude handle -> agy native PreInvocation -> private inbox/ack')
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
