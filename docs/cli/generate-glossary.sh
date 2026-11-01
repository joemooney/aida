#!/usr/bin/env bash
# Generate the AIDA Book's Glossary chapter from `aida docs glossary`.
#
# trace:STORY-608 — the Glossary page is GENERATED, never hand-maintained
# (ADR-5: generated-not-hand-maintained). The source of truth is the binary's
# embedded discipline glossary (product terms + machinery terms + lifecycle
# vocabulary, from
# `aida-core/templates/.aida/discipline/{product-glossary,machinery-glossary,lifecycle-vocabulary}.md`).
# Edit those templates, rebuild the binary, re-run this script — the page follows.
#
# Build:  bash docs/cli/generate-glossary.sh
# Check:  bash docs/cli/generate-glossary.sh --check
# Output: docs/cli/12-glossary.md  (overwritten in place; do not edit by hand)
#
# Wired into the book build via `make book-glossary` / `make book`.
#
# trace:BUG-1950 | ai:claude — `--check` renders to a temp file and diffs it
# against the committed page instead of overwriting it, exiting non-zero on
# drift. CI runs it against the freshly built binary ("Book glossary drift
# guard"), so a template edit that skips regeneration, or a hand edit of the
# page, fails the build. `GLOSSARY_OUT` overrides the page path (tests only).
set -euo pipefail

CHECK=0
case "${1:-}" in
  "") ;;
  --check) CHECK=1 ;;
  *)
    echo "usage: $0 [--check]" >&2
    exit 2
    ;;
esac

CLI_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${GLOSSARY_OUT:-$CLI_DIR/12-glossary.md}"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

# Resolve the aida binary: honor an activated dev build, else fall back to PATH.
AIDA="${AIDA_BIN:-aida}"
if ! command -v "$AIDA" >/dev/null 2>&1; then
  echo "error: '$AIDA' not found on PATH (run 'aida-on' for the dev build, or set AIDA_BIN)" >&2
  exit 1
fi

# The book orders the sections machinery, lifecycle, product. mdBook gives a
# repeated heading a `-1` id, so the section printed first keeps the bare ids:
# this way links into 12-glossary.md#role or #seat still land on the machinery
# definitions they pointed at before the product glossary existed.
# `aida docs glossary` with no flags prints product first, so the book asks
# for the sections in two calls. trace:STORY-2219 | ai:claude
# Pin `--format human`: captured stdout otherwise defaults to TOON, which
# appends a `cache:` footer that is not part of the glossary. trace:BUG-1950 | ai:claude
DEVELOPER="$("$AIDA" docs glossary --machinery --lifecycle --format human)"
PRODUCT="$("$AIDA" docs glossary --product --format human)"
GLOSSARY="$(printf '%s\n\n%s' "$DEVELOPER" "$PRODUCT")"

if [ -z "$DEVELOPER" ] || [ -z "$PRODUCT" ]; then
  echo "error: 'aida docs glossary' produced no output" >&2
  exit 1
fi

{
  echo "# Glossary"
  echo
  echo "<!-- GENERATED FILE — do not edit by hand."
  echo "     Source: \`aida docs glossary\` (embedded product + machinery + lifecycle vocabulary)."
  echo "     Regenerate: \`bash docs/cli/generate-glossary.sh\` (or \`make book-glossary\`)."
  echo "     Edit the term definitions in"
  echo "     aida-core/templates/.aida/discipline/{product-glossary,machinery-glossary,lifecycle-vocabulary}.md,"
  echo "     rebuild the binary, then re-run. -->"
  echo
  echo "> The shared vocabulary AIDA's docs, error messages, and agent-to-agent"
  echo "> handoffs lean on — the **machinery** terms (orchestrator, phase, drain,"
  echo "> lease, role, session, scope, worktree, sentinel, batch, autonomy mode),"
  echo "> the **lifecycle** verbs (committed / pushed / merged / completed /"
  echo "> released) and the plain-language **product** terms (merge hold, mandate,"
  echo "> grant, hub, …). This page is generated from \`aida docs glossary\`, so it"
  echo "> can never drift from the definitions the binary ships."
  echo
  # Four transforms on the embedded glossary before it lands in the
  # user-facing book:
  #
  #  1. Demote the three embedded H1s ("# Product glossary", "# Machinery
  #     glossary", "# Lifecycle vocabulary") to H2 so the chapter has a single
  #     H1 ("# Glossary"). The product glossary's own H2/H3 headings are left
  #     as they are.
  #
  #  2. Strip SPEC-ID noise. The discipline-pack templates carry trailing
  #     `trace:...` provenance markers and illustrative `TASK-12` examples;
  #     the book is user-facing, where the convention (and the manual's
  #     drift-guard, docs/cli/verify-manual.py) is NO SPEC-IDs. Drop the
  #     trace provenance and rewrite illustrative SPEC-IDs to `<spec-id>`,
  #     wrapped in backticks so mdBook renders it as code instead of
  #     swallowing it as an unknown HTML tag (trace:BUG-1950).
  #     This keeps the page generated *and* drift-clean.
  #
  #  3. The product glossary links pages relative to its template master
  #     (`../../../../docs/<page>.md`); rewrite those to `../<page>.md`, which
  #     is the same page relative to docs/cli/. trace:STORY-2219 | ai:claude
  #
  #  4. (After assembly, below.) Point every in-page link at the right heading
  #     on the one combined page.
  printf '%s\n' "$GLOSSARY" \
    | sed -E \
        -e 's/^# (Product glossary|Machinery glossary|Lifecycle vocabulary)$/## \1/' \
        -e 's#\]\(\.\./\.\./\.\./\.\./docs/#](../#g' \
        -e '/^<!-- trace:[A-Za-z0-9_| :+-]* -->$/d' \
        -e 's/[[:space:]]*trace:[A-Za-z0-9_| :+-]*$//' \
        -e 's/\b(STORY|TASK|BUG|EPIC|SPIKE|ADR|FR|PRIN|VIS|CON|TERM|CR)-[0-9]+/`<spec-id>`/g'
} >"$TMP"

# Transform 4: links between the templates (`machinery-glossary.md`,
# `lifecycle-vocabulary.md#frag`) become in-page anchors, and a bare `#anchor`
# is pointed at the heading in its own section when two sections share a
# heading name. A link with no matching heading fails here, so a broken
# anchor fails --check instead of shipping. trace:STORY-2219 | ai:claude
python3 "$CLI_DIR/glossary-anchors.py" "$TMP"

if [ "$CHECK" -eq 1 ]; then
  if diff -u "$OUT" "$TMP"; then
    echo "OK: $OUT matches 'aida docs glossary'"
    exit 0
  fi
  echo "error: $OUT is stale — it differs from 'aida docs glossary' (diff above)." >&2
  echo "       The page is generated; edit the templates, then run 'make book-glossary'" >&2
  echo "       with a binary built from this tree and commit the result." >&2
  exit 1
fi

cp "$TMP" "$OUT"
echo "wrote $OUT ($(wc -l <"$OUT") lines) from 'aida docs glossary'"
