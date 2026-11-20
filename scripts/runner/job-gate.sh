#!/bin/sh
# Runner-supported .sh hook; isolated Python ignores job/user module paths.
# trace:STORY-2695 | ai:codex
exec /usr/bin/python3 -I /usr/local/libexec/aida-ci-job-gate.py
