#!/usr/bin/env bash
set -euo pipefail

# Fixture test for `docs/cli/generate-glossary.sh --check`, the drift guard
# for the generated AIDA Book glossary page (docs/cli/12-glossary.md).
#
# A stub AIDA_BIN stands in for `aida docs glossary`; it reproduces
# render_discipline_glossary: the two embedded discipline templates (trailing
# newline trimmed, as build.rs embeds them) joined by a blank line. The real
# binary is exercised by CI's "Book glossary drift guard" step.
# trace:BUG-1950 | ai:claude
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN="$ROOT/docs/cli/generate-glossary.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/aida" <<'SH'
#!/usr/bin/env bash
[ "$*" = "docs glossary --format human" ] || { echo "stub: unexpected args: $*" >&2; exit 64; }
D="${STUB_TEMPLATE_DIR:?}"
printf '%s\n\n%s' "$(cat "$D/machinery-glossary.md")" "$(cat "$D/lifecycle-vocabulary.md")"
SH
chmod +x "$TMP/aida"

mkdir -p "$TMP/tpl"
cat >"$TMP/tpl/machinery-glossary.md" <<'MD'
# Machinery glossary

### drain

Works the ready set. trace:STORY-1 | ai:claude
MD
cat >"$TMP/tpl/lifecycle-vocabulary.md" <<'MD'
# Lifecycle vocabulary

Say "PR opened for TASK-12".
MD

PAGE="$TMP/12-glossary.md"
gen() {
  AIDA_BIN="$TMP/aida" STUB_TEMPLATE_DIR="$TMP/tpl" GLOSSARY_OUT="$PAGE" \
    bash "$GEN" "$@" >"$TMP/out" 2>&1
}

# (a) A freshly generated page passes --check, and the generator's transforms
#     ran: H1s demoted, trace provenance stripped, SPEC-IDs code-quoted.
gen
if ! gen --check; then
  echo "FAIL: freshly generated page should pass --check" >&2
  cat "$TMP/out" >&2
  exit 1
fi
grep -Fx "## Machinery glossary" "$PAGE" >/dev/null
grep -Fx "Works the ready set." "$PAGE" >/dev/null
grep -F 'Say "PR opened for `<spec-id>`".' "$PAGE" >/dev/null
if grep -F "trace:" "$PAGE" >/dev/null; then
  echo "FAIL: trace provenance leaked into the generated page" >&2
  exit 1
fi

# (b) A hand edit of the page fails --check and leaves the page untouched.
sed 's/Works the ready set\./Works the ready set, by hand./' "$PAGE" >"$PAGE.tmp" && mv "$PAGE.tmp" "$PAGE"
cp "$PAGE" "$TMP/edited"
if gen --check; then
  echo "FAIL: hand-edited page was not caught by --check" >&2
  exit 1
fi
grep -F "is stale" "$TMP/out" >/dev/null
cmp "$PAGE" "$TMP/edited"

# (c) A template edit without regeneration fails --check.
gen
printf '\n### lease\n\nA claim.\n' >>"$TMP/tpl/machinery-glossary.md"
if gen --check; then
  echo "FAIL: template edit without regeneration was not caught" >&2
  exit 1
fi

# (d) A missing binary is an error, not a silent pass.
if AIDA_BIN="$TMP/no-such-aida" GLOSSARY_OUT="$PAGE" bash "$GEN" --check >"$TMP/out" 2>&1; then
  echo "FAIL: --check passed without an aida binary" >&2
  exit 1
fi

# (e) An unknown argument is refused.
if gen --bogus; then
  echo "FAIL: unknown argument was accepted" >&2
  exit 1
fi

# (f) The committed page matches the committed template masters. This is the
#     no-build slice of CI's real-binary drift guard.
if ! AIDA_BIN="$TMP/aida" STUB_TEMPLATE_DIR="$ROOT/aida-core/templates/.aida/discipline" \
  bash "$GEN" --check >"$TMP/out" 2>&1; then
  echo "FAIL: docs/cli/12-glossary.md is stale against the template masters;" >&2
  echo "      run 'make book-glossary' with a binary built from this tree" >&2
  cat "$TMP/out" >&2
  exit 1
fi

echo "ok: generate-glossary.sh --check"
