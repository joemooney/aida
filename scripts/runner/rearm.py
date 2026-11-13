#!/usr/bin/python3
"""Credentialed JIT registration only after root admission; no tokens on argv.
trace:STORY-2695 | ai:codex
"""
import json
import subprocess
import sys
import time
import uuid


def once(slot, proposal):
    name = f'aida-ci-{slot}-{uuid.uuid4().hex}'
    child = subprocess.Popen(['sudo', '-n', '/usr/local/sbin/aida-ci-one-job', slot],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    try:
        child.stdin.write(json.dumps(dict(run=proposal['run'], job=proposal['job'], runner=name)) + '\n')
        child.stdin.flush()
        line = child.stdout.readline()
        if not line:
            return child.wait()
        if json.loads(line) != {'ready': True}:
            raise RuntimeError('invalid admission handshake')
        jit = subprocess.check_output([
            'gh', 'api', '--method', 'POST', 'repos/joemooney/aida/actions/runners/generate-jitconfig',
            '-f', f'name={name}', '-F', 'runner_group_id=1',
            '-f', 'labels[]=self-hosted', '-f', 'labels[]=Linux', '-f', 'labels[]=X64',
            '-f', 'labels[]=aida-dell1', '-f', 'labels[]=' + proposal['class'],
            '-f', 'labels[]=' + name, '-f', 'work_folder=_work', '--jq', '.encoded_jit_config'],
            text=True, timeout=30).strip()
        child.stdin.write(jit + '\n')
        child.stdin.close()
        for line in child.stdout:
            print(line, end='', flush=True)
        return child.wait()
    finally:
        if child.poll() is None:
            child.terminate()
            child.wait(timeout=60)


def main(slot):
    while True:
        proposal = json.loads(subprocess.check_output(
            ['/usr/bin/python3', '-I', '/usr/local/libexec/aida-ci-select-job.py', slot], text=True))
        if proposal is None:
            time.sleep(15)
            continue
        status = once(slot, proposal)
        if status == 75:
            print(f'{slot}: TRAIN priority; retrying', flush=True)
            time.sleep(15)
        elif status:
            raise SystemExit(status)


if __name__ == '__main__':
    if len(sys.argv) != 2 or sys.argv[1] not in ('s0', 's1'):
        raise SystemExit('Usage: rearm.py s0|s1')
    main(sys.argv[1])
