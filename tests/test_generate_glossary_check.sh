#!/usr/bin/env bash
set -euo pipefail

# Fixture test for `docs/cli/generate-glossary.sh --check`, the drift guard
# for the generated AIDA Book glossary page (docs/cli/12-glossary.md).
#
# A stub AIDA_BIN stands in for `aida docs glossary`; it reproduces
# render_discipline_glossary: the selected embedded discipline templates
# (in product, machinery, lifecycle order; trailing newline trimmed, as
# build.rs embeds them) joined by a blank line. The real binary is exercised
# by CI's "Book glossary drift guard" step.
# trace:BUG-1950 | ai:claude
# trace:STORY-2219 | ai:claude
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GEN="$ROOT/docs/cli/generate-glossary.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

cat >"$TMP/aida" <<'SH'
#!/usr/bin/env bash
D="${STUB_TEMPLATE_DIR:?}"
[ "${1:-} ${2:-}" = "docs glossary" ] || { echo "stub: unexpected args: $*" >&2; exit 64; }
shift 2
p=0 m=0 l=0 fmt=""
while [ $# -gt 0 ]; do
  case "$1" in
    --product) p=1 ;;
    --machinery) m=1 ;;
    --lifecycle) l=1 ;;
    --format) fmt="${2:-}"; shift ;;
    *) echo "stub: unexpected arg: $1" >&2; exit 64 ;;
  esac
  shift
done
[ "$fmt" = human ] || { echo "stub: --format human is required" >&2; exit 64; }
[ $((p + m + l)) -eq 0 ] && p=1 m=1 l=1
out=""
for pair in "$p:product-glossary" "$m:machinery-glossary" "$l:lifecycle-vocabulary"; do
  [ "${pair%%:*}" = 1 ] || continue
  part="$(cat "$D/${pair#*:}.md")"
  if [ -z "$out" ]; then out="$part"; else out="$out"$'\n\n'"$part"; fi
done
printf '%s' "$out"
SH
chmod +x "$TMP/aida"

# The fixture has the collisions the real templates have: product "Role" and
# machinery "role" share an id, both sections have a "Terms" heading, and the
# templates link each other by file name.
mkdir -p "$TMP/tpl"
cat >"$TMP/tpl/product-glossary.md" <<'MD'
# Product glossary

<!-- trace:STORY-1 | ai:claude -->

Developer terms are in the [machinery glossary](machinery-glossary.md), and
lifecycle states in the [lifecycle vocabulary](lifecycle-vocabulary.md).

## Terms

### Hub

A Git server. See [the guide](../../../../docs/air-gapped-development.md#the-parts).

### Role

A job. Held by a [hub](#hub) user; see [Terms](#terms).
MD
cat >"$TMP/tpl/machinery-glossary.md" <<'MD'
# Machinery glossary

## Terms

### drain

Works the ready set. trace:STORY-1 | ai:claude

### role

Capability profile. Not the [role](#role) of a person; see [Terms](#terms),
[`lifecycle-vocabulary.md`](lifecycle-vocabulary.md#merged) and the literal
`[x](lifecycle-vocabulary.md)`.
MD
cat >"$TMP/tpl/lifecycle-vocabulary.md" <<'MD'
# Lifecycle vocabulary

Say "PR opened for TASK-12".

### merged

On main. See [drain](machinery-glossary.md#drain).
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
grep -Fx "## Product glossary" "$PAGE" >/dev/null
grep -Fx "A Git server. See [the guide](../air-gapped-development.md#the-parts)." "$PAGE" >/dev/null
grep -Fx "Works the ready set." "$PAGE" >/dev/null
grep -F 'Say "PR opened for `<spec-id>`".' "$PAGE" >/dev/null
if grep -F "trace:" "$PAGE" >/dev/null; then
  echo "FAIL: trace provenance leaked into the generated page" >&2
  exit 1
fi

# (a2) The book prints machinery, lifecycle, then product, so the developer
#      headings keep their bare ids and the product repeats get `-1`. Links
#      between templates become in-page anchors; bare anchors name the heading
#      in their own section; inline code is left as written.
order="$(grep -E '^## (Machinery glossary|Lifecycle vocabulary|Product glossary)$' "$PAGE" | tr '\n' '|')"
[ "$order" = "## Machinery glossary|## Lifecycle vocabulary|## Product glossary|" ] \
  || { echo "FAIL: book section order is '$order'" >&2; exit 1; }
for want in \
  'Developer terms are in the [machinery glossary](#machinery-glossary), and' \
  'lifecycle states in the [lifecycle vocabulary](#lifecycle-vocabulary).' \
  'A job. Held by a [hub](#hub) user; see [Terms](#terms-1).' \
  'Capability profile. Not the [role](#role) of a person; see [Terms](#terms),' \
  '[`lifecycle-vocabulary.md`](#merged) and the literal' \
  '`[x](lifecycle-vocabulary.md)`.' \
  'On main. See [drain](#drain).'; do
  grep -Fx -- "$want" "$PAGE" >/dev/null \
    || { echo "FAIL: generated page lacks: $want" >&2; cat "$PAGE" >&2; exit 1; }
done
if grep -E '\]\((machinery-glossary|lifecycle-vocabulary|product-glossary)\.md' "$PAGE" \
  | grep -vF '`[x](lifecycle-vocabulary.md)`' >/dev/null; then
  echo "FAIL: a link to an unpublished template page survived" >&2
  exit 1
fi
python3 "$ROOT/docs/cli/glossary-anchors.py" --verify "$PAGE"

# (a3) The anchor check catches a link that lands in the wrong section and a
#      link with no heading.
sed 's/see \[Terms\](#terms-1)/see [Terms](#terms)/' "$PAGE" >"$TMP/wrong.md"
if python3 "$ROOT/docs/cli/glossary-anchors.py" --verify "$TMP/wrong.md" 2>"$TMP/out"; then
  echo "FAIL: a product link landing on the machinery heading was not caught" >&2
  exit 1
fi
grep -F "has its own #terms-1" "$TMP/out" >/dev/null
printf '\nSee [nothing](#no-such-term).\n' >>"$TMP/tpl/product-glossary.md"
if gen; then
  echo "FAIL: an unresolved anchor did not fail generation" >&2
  exit 1
fi
grep -F "no heading for #no-such-term" "$TMP/out" >/dev/null
sed -i '/no-such-term/d' "$TMP/tpl/product-glossary.md"
gen

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

# (g) Every in-page anchor in the committed chapter resolves to the heading
#     in its own section.
python3 "$ROOT/docs/cli/glossary-anchors.py" --verify "$ROOT/docs/cli/12-glossary.md"

echo "ok: generate-glossary.sh --check"
