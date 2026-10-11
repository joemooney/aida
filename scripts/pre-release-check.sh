#!/usr/bin/env bash
# scripts/pre-release-check.sh — retired cross-platform release gate.
#
# ADR-72: AIDA is Linux-only. Windows and macOS are unsupported, have no CI,
# and releases do NOT require any cross-platform run. This script used to
# block a release until .github/workflows/cross-platform.yml was green within
# 24h; that gate re-forced Windows work on every release, so it is now a no-op
# that always exits 0. scripts/release.sh no longer calls it. It is kept only
# so any external caller keeps working.
#
# trace:TASK-1723 | ai:claude

echo "pre-release-check: no cross-platform gate (AIDA is Linux-only); nothing to check."
exit 0
