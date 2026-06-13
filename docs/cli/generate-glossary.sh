#!/usr/bin/env bash
# Generate the AIDA Book's Glossary chapter from `aida docs glossary`.
#
# trace:STORY-608 — the Glossary page is GENERATED, never hand-maintained
# (ADR-5: generated-not-hand-maintained). The source of truth is the binary's
# embedded discipline glossary (machinery terms + lifecycle vocabulary, from
# `aida-core/templates/docs/aida/discipline/{machinery-glossary,lifecycle-vocabulary}.md`).
# Edit those templates, rebuild the binary, re-run this script — the page follows.
#
# Build:  bash docs/cli/generate-glossary.sh
# Output: docs/cli/12-glossary.md  (overwritten in place; do not edit by hand)
#
# Wired into the book build via `make book-glossary` / `make book`.
set -euo pipefail

CLI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$CLI_DIR/12-glossary.md"

# Resolve the aida binary: honor an activated dev build, else fall back to PATH.
AIDA="${AIDA_BIN:-aida}"
if ! command -v "$AIDA" >/dev/null 2>&1; then
  echo "error: '$AIDA' not found on PATH (run 'aida-on' for the dev build, or set AIDA_BIN)" >&2
  exit 1
fi

# `aida docs glossary` (no flags) emits both sections, machinery first.
GLOSSARY="$("$AIDA" docs glossary)"

if [ -z "$GLOSSARY" ]; then
  echo "error: 'aida docs glossary' produced no output" >&2
  exit 1
fi

{
  echo "# Glossary"
  echo
  echo "<!-- GENERATED FILE — do not edit by hand."
  echo "     Source: \`aida docs glossary\` (embedded machinery + lifecycle vocabulary)."
  echo "     Regenerate: \`bash docs/cli/generate-glossary.sh\` (or \`make book-glossary\`)."
  echo "     Edit the term definitions in"
  echo "     aida-core/templates/docs/aida/discipline/{machinery-glossary,lifecycle-vocabulary}.md,"
  echo "     rebuild the binary, then re-run. -->"
  echo
  echo "> The shared vocabulary AIDA's docs, error messages, and agent-to-agent"
  echo "> handoffs lean on — the **machinery** terms (orchestrator, phase, drain,"
  echo "> lease, role, session, scope, worktree, sentinel, batch, autonomy mode)"
  echo "> and the **lifecycle** verbs (committed / pushed / merged / completed /"
  echo "> released). This page is generated from \`aida docs glossary\`, so it can"
  echo "> never drift from the definitions the binary ships."
  echo
  # Two transforms on the embedded glossary before it lands in the
  # user-facing book:
  #
  #  1. Demote the two embedded H1s ("# Machinery glossary", "# Lifecycle
  #     vocabulary") to H2 so the chapter has a single H1 ("# Glossary").
  #
  #  2. Strip SPEC-ID noise. The discipline-pack templates carry trailing
  #     `trace:...` provenance markers and illustrative `TASK-12` examples;
  #     the book is user-facing, where the convention (and the manual's
  #     drift-guard, docs/cli/verify-manual.py) is NO SPEC-IDs. Drop the
  #     trace provenance and rewrite illustrative SPEC-IDs to `<spec-id>`.
  #     This keeps the page generated *and* drift-clean.
  printf '%s\n' "$GLOSSARY" \
    | sed -E \
        -e 's/^# (Machinery glossary|Lifecycle vocabulary)$/## \1/' \
        -e 's/[[:space:]]*trace:[A-Za-z0-9_| :+-]*$//' \
        -e 's/\b(STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|PRIN|VIS|CON|TERM|CR)-[0-9]+/<spec-id>/g'
} >"$OUT"

echo "wrote $OUT ($(wc -l <"$OUT") lines) from 'aida docs glossary'"
