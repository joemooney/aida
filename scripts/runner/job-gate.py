#!/usr/bin/python3
"""Installed immutable runner job-start hook; fail before any workflow step.
trace:STORY-2695 | ai:codex
"""
import os
import signal
import socket
import sys

try:
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(120)
        client.connect(os.environ['AIDA_CI_GATE'])
        if client.recv(32) != b'ALLOW\n':
            raise RuntimeError('Host admission refused this job')
except Exception as error:
    print(f'Host job gate blocked: {error}', file=sys.stderr, flush=True)
    # Never return a failed step and let an untrusted if: always() step run.
    # The host gate stops this slot's cgroup; a transport failure stays blocked
    # for operator reconciliation instead of giving execution back to the job.
    while True:
        signal.pause()
