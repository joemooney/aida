#!/usr/bin/env bash
# scripts/docs-gate.sh — release-time documentation gate.
#
# Cherry-picked from EPIC-25's one load-bearing idea: verify documentation
# is current at RELEASE time, not on every PR — so PRs ship fast (no
# per-PR doc tax) and docs are checked once, where staleness actually
# ships to users: a tagged release. The composite-spec / documenter-role /
# release-master machinery from EPIC-25 is deliberately NOT built here.
#
# Blocking check:
#   CHANGELOG currency + determinism — the changelog scripts/release.sh
#   regenerated for this release must (a) contain a section for the version
#   being tagged and (b) be idempotent: a second `changelog refresh`
#   produces byte-identical output (no uncommitted drift). This turns the
#   "docs are current" promise into a real gate — today a changelog
#   generation failure is only a warning and a release can silently ship a
#   stale or half-generated CHANGELOG.md.
#
# Advisory (non-blocking):
#   Plan-ref integrity — every docs/plans/*.md touched since the previous
#   tag is run through `aida plan verify`; any drifted `path:line` refs /
#   missing files / absent sections are summarised as a hint. This never
#   changes the exit code: preexisting plan drift is real tech debt but
#   should not perpetually block releases. Fix it with `aida plan verify
#   <file> --fix` / `aida skill lint`.
#
# Exit 0     = docs are current; safe to tag.
# Exit non-0 = the blocking CHANGELOG check failed; do not tag (or opt out).
#
# Called by scripts/release.sh before the tag step. Opt out there with
# --skip-docs-check or AIDA_SKIP_DOCS_CHECK=1 (mirrors --skip-xplat-check /
# AIDA_SKIP_XPLAT_CHECK). Not recommended for a published release.
#
# Usage:
#   scripts/docs-gate.sh <version>      # e.g. scripts/docs-gate.sh 0.15.0
#
# The `aida` invocation defaults to `cargo run -q -p aida-cli --` (matches
# release.sh — guarantees the CURRENT branch's binary generates the
# changelog, not a stale one on PATH). Override with AIDA_BIN for tests
# (e.g. a prebuilt release binary):  AIDA_BIN=./target/release/aida ...
#
# trace:TASK-1149 | ai:claude

# Build the aida invocation into a global array, word-splitting AIDA_BIN
# safely. Kept as a function so tests can `source` this file and exercise
# the checks without running the main flow.
docs_gate_resolve_aida() {
    if [ -n "${AIDA_BIN:-}" ]; then
        # shellcheck disable=SC2206  # deliberate word-split of the override
        DOCS_GATE_AIDA=($AIDA_BIN)
    else
        DOCS_GATE_AIDA=(cargo run -q -p aida-cli --)
    fi
}

# docs_gate_changelog <version> — blocking. Returns 0 when CHANGELOG.md is
# current for the release and deterministic, non-zero otherwise.
docs_gate_changelog() {
    local version="$1"
    local changelog="CHANGELOG.md"

    echo "  → regenerating CHANGELOG.md for v$version ..."
    if ! "${DOCS_GATE_AIDA[@]}" changelog refresh --released-as "v$version" >/dev/null 2>&1; then
        echo "  ✗ 'aida changelog refresh' failed — cannot verify docs currency." >&2
        echo "    A release must ship a current changelog; fix generation or pass --skip-docs-check." >&2
        return 1
    fi

    if [ ! -f "$changelog" ]; then
        echo "  ✗ CHANGELOG.md not found after regeneration." >&2
        return 1
    fi

    # (a) the released version must appear as its own section header.
    if ! grep -q "^## \[v$version\]" "$changelog"; then
        echo "  ✗ CHANGELOG.md has no '## [v$version]' section after regeneration." >&2
        echo "    The changelog was not updated for this release." >&2
        return 1
    fi

    # (b) idempotency — a second refresh must not change the file. Same git
    #     state + spec store => byte-identical output; drift here means the
    #     changelog generation is non-deterministic, which would leave an
    #     uncommitted diff after the release commit.
    local before after
    before=$(git hash-object "$changelog")
    "${DOCS_GATE_AIDA[@]}" changelog refresh --released-as "v$version" >/dev/null 2>&1 || true
    after=$(git hash-object "$changelog")
    if [ "$before" != "$after" ]; then
        echo "  ✗ CHANGELOG.md is not idempotent — a re-run produced uncommitted drift." >&2
        echo "    Changelog generation is non-deterministic; investigate before tagging." >&2
        return 1
    fi

    echo "  ✓ CHANGELOG.md is current for v$version (section present; regeneration idempotent)."
    return 0
}

# docs_gate_plan_advisory — non-blocking. Reports drifted plan files touched
# since the previous tag. Never returns non-zero; purely informational.
docs_gate_plan_advisory() {
    local prev
    prev=$(git describe --tags --abbrev=0 2>/dev/null || true)
    if [ -z "$prev" ]; then
        echo "  · plan-ref advisory skipped (no previous tag to diff against)."
        return 0
    fi

    local changed
    changed=$(git diff --name-only "$prev"..HEAD -- 'docs/plans/*.md' 2>/dev/null || true)
    if [ -z "$changed" ]; then
        echo "  · plan-ref advisory: no plan files changed since $prev."
        return 0
    fi

    local drifted=0 total=0 plan
    while IFS= read -r plan; do
        [ -z "$plan" ] && continue
        [ -f "$plan" ] || continue
        total=$((total + 1))
        if ! "${DOCS_GATE_AIDA[@]}" plan verify "$plan" -q >/dev/null 2>&1; then
            drifted=$((drifted + 1))
        fi
    done <<<"$changed"

    if [ "$drifted" -eq 0 ]; then
        echo "  ✓ plan-ref advisory: all $total plan file(s) changed since $prev verify clean."
    else
        echo "  ⚠ plan-ref advisory: $drifted of $total plan file(s) changed since $prev have drifted refs."
        echo "    (non-blocking) inspect with 'aida skill lint' or 'aida plan verify <file> --fix'."
    fi
    return 0
}

# Allow `source docs-gate.sh` from tests without running the main flow.
if [ "${BASH_SOURCE[0]}" != "${0}" ]; then
    return 0
fi

set -euo pipefail

version="${1:-}"
if [ -z "$version" ]; then
    echo "usage: $0 <version>   # e.g. $0 0.15.0" >&2
    exit 1
fi
# Accept a leading 'v' for convenience; the checks want the bare version.
version="${version#v}"

docs_gate_resolve_aida

echo "─── Release documentation gate (v$version) ───"
gate_ok=0
docs_gate_changelog "$version" || gate_ok=1
docs_gate_plan_advisory || true

if [ "$gate_ok" -ne 0 ]; then
    echo "✗ documentation gate failed for v$version — not safe to tag." >&2
    exit 1
fi
echo "✓ documentation gate passed for v$version."
exit 0
