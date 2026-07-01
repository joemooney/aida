#!/usr/bin/env bash
# Glyph-literal lint — the forward-guard for EPIC-45 / TASK-835.
#
# AIDA routes its status/marker glyphs through a central registry
# (aida-cli/src/glyphs.rs) so an `[ui] glyphs = "ascii"` profile, the
# `AIDA_GLYPHS` env, or a custom `[glyphs]` override table can re-render them for
# terminals that can't draw the unicode/emoji defaults. A raw glyph literal
# typed inline anywhere else SILENTLY bypasses that machinery — the override has
# no effect on it. This script is the substrate-as-bouncer that stops NEW raw
# literals from landing: it greps the curated registry glyph set across
# aida-cli/src and fails when one appears OUTSIDE the registry module and the
# explicit allow-list below.
#
# SCOPE — the curated set is the registry's OWN glyphs only (status badges,
# work-routing markers, the little emoji markers). It deliberately does NOT flag
# em-dashes, box-drawing, prose arrows, ellipsis, the `·` separator dot, math
# symbols, accented letters or CJK — those are ordinary text, not registry
# glyphs, and gating them would be noise.
#
# MODE — report-only by default (annotate the count, exit 0) because the long
# tail of pre-existing literals is migrated incrementally (TASK-835). The
# ALLOW_LIST below names every file that still legitimately contains raw
# literals; that list IS the tracked remaining-work inventory for the migration.
# Pass `--block` (or set GLYPH_LINT_BLOCK=1) to fail on ANY hit outside the
# allow-list — flip CI to that once a file is fully migrated and removed from
# the list. trace:TASK-835 | ai:claude
set -euo pipefail

cd "$(dirname "$0")/.."

SRC_DIR="aida-cli/src"

# The curated registry glyph set. Keep in sync with the `Glyph` enum in
# aida-cli/src/glyphs.rs — when you add a registry glyph whose literal could
# show up inline, add the codepoint here so the guard covers it.
# trace:TASK-1071 — info/notice glyphs (ⓘ ℹ ⦿ 📨) added so raw usages are flagged.
GLYPHS='✓✗◯◐⚠▷▸↳✉•⏳🏠🚶🤖◉⊘↑▶ⓘℹ⦿📨'

# Files exempt from the guard:
#  - glyphs.rs        — the registry itself (defines the literals).
#  - status_display.rs — holds the Unicode source-of-truth fallback map that the
#                        registry's defaults are validated against (status_glyph_literal).
#
# ALLOW_LIST — the long-tail files that STILL contain raw glyph literals not yet
# migrated to the registry. This is the live remaining-work list for TASK-835:
# as each file is migrated, delete its line here; once the list is empty the
# guard can be flipped to --block unconditionally in CI.
EXEMPT_REGISTRY=(
  "$SRC_DIR/glyphs.rs"
  "$SRC_DIR/status_display.rs"
)
ALLOW_LIST=(
  # cli.rs — remaining literals are all in `///` clap doc/help text (compile-time
  # strings rendered in `--help`), which cannot route through the runtime glyph
  # registry. Kept on the allow-list by design. (TASK-840)
  "$SRC_DIR/cli.rs"
)

block_mode=0
explicit_files=()
# Args: `--block` plus an OPTIONAL list of file paths to scan. With no file
# paths the guard scans the whole aida-cli/src tree (CI default). With paths it
# scans ONLY those — the pre-commit hook passes the staged aida-cli/src/*.rs
# files so a new raw literal is caught at commit time, scoped to what changed,
# without flagging the pre-existing long-tail (TASK-984). The exempt/allow-list
# logic still applies to the passed files. trace:TASK-984
for arg in "$@"; do
  case "$arg" in
    --block) block_mode=1 ;;
    *) explicit_files+=("$arg") ;;
  esac
done
if [[ "${GLYPH_LINT_BLOCK:-0}" == "1" ]]; then
  block_mode=1
fi

is_exempt() {
  local f="$1"
  local e
  for e in "${EXEMPT_REGISTRY[@]}" "${ALLOW_LIST[@]}"; do
    [[ "$f" == "$e" ]] && return 0
  done
  return 1
}

violations=0
allowed_hits=0
offending_files=()

# Scan target: the explicitly-passed files (staged-files mode), else the whole
# aida-cli/src tree (CI default). trace:TASK-984
scan_files=()
if [[ ${#explicit_files[@]} -gt 0 ]]; then
  for f in "${explicit_files[@]}"; do
    [[ -f "$f" ]] && scan_files+=("$f")
  done
else
  while IFS= read -r f; do
    scan_files+=("$f")
  done < <(find "$SRC_DIR" -name '*.rs' | sort)
fi

for f in "${scan_files[@]}"; do
  count=$(grep -oP "[$GLYPHS]" "$f" 2>/dev/null | wc -l || true)
  [[ "$count" -eq 0 ]] && continue
  if is_exempt "$f"; then
    allowed_hits=$((allowed_hits + count))
  else
    violations=$((violations + count))
    offending_files+=("$f ($count)")
  fi
done

echo "glyph-lint: registry glyph set = $GLYPHS"
echo "glyph-lint: $allowed_hits literal(s) in exempt/allow-listed files (pre-existing, tracked by TASK-835)"

if [[ "$violations" -gt 0 ]]; then
  echo ""
  echo "glyph-lint: ${violations} RAW glyph literal(s) found OUTSIDE the registry and allow-list:"
  for entry in "${offending_files[@]}"; do
    echo "  - $entry"
  done
  echo ""
  echo "Route these through aida-cli/src/glyphs.rs (Glyph::render / get / get_custom)"
  echo "so the ASCII profile and [glyphs] overrides apply. If a file is a known"
  echo "long-tail not yet migrated, add it to ALLOW_LIST in scripts/glyph-lint.sh."
  if [[ "$block_mode" -eq 1 ]]; then
    echo "::error::glyph-lint failed (--block): ${violations} new raw glyph literal(s)"
    exit 1
  fi
  echo "::warning::glyph-lint (report-only): ${violations} raw glyph literal(s) outside allow-list"
fi

echo "glyph-lint: OK"
exit 0
