#!/usr/bin/python3
"""Unprivileged proposal only; controller re-fetches and authorizes independently.
trace:STORY-2695 | ai:codex
"""
import json
import subprocess
import sys

# Stable disjoint proposals avoid both foreground loops racing for one queued
# job while its first slot prepares source/cache. Cache affinity is per shard.
FAMILIES = {'s0': ('lib-0', 'cli-1'), 's1': ('lib-1', 'cli-0')}


def api(route):
    return json.loads(subprocess.check_output(['gh', 'api', 'repos/joemooney/aida/' + route], timeout=30))


def propose(slot, read=api):
    candidates = []
    for status in ('queued', 'in_progress'):
        runs = read(f'actions/runs?status={status}&per_page=100')['workflow_runs']
        if len(runs) >= 100:
            raise RuntimeError('Queue truncated; inspect manually')
        for run in runs:
            if run['event'] == 'pull_request' and run['head_branch'].startswith('train/'):
                kind = 'TRAIN'
            elif run['event'] == 'push' and run['head_branch'] == 'main':
                kind = 'MAIN'
            else:
                continue
            jobs = read(f'actions/runs/{run["id"]}/jobs?per_page=100')['jobs']
            if len(jobs) >= 100:
                raise RuntimeError('Jobs truncated; inspect manually')
            for job in jobs:
                if job['status'] == 'queued' and job['name'] in [f'CI ({s})' for s in FAMILIES[slot]]:
                    candidates.append(dict(run=run['id'], job=job['id'], **{'class': kind}))
    candidates.sort(key=lambda item: (item['class'] != 'TRAIN', item['run'], item['job']))
    return candidates[0] if candidates else None


if __name__ == '__main__':
    if len(sys.argv) != 2 or sys.argv[1] not in FAMILIES:
        raise SystemExit('Usage: select-job.py s0|s1')
    print(json.dumps(propose(sys.argv[1])))
