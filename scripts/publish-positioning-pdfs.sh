#!/usr/bin/env bash
# Batch-render docs/positioning/*.md to PDF and land them in the artifacts repo.
# trace:TASK-1127 | ai:claude
#
# Positioning rots fast (see docs/positioning/README.md "Maintenance rhythm"),
# so the PDF set is a re-runnable render, not a one-off. Reuses the same
# DejaVu/xelatex stack as scripts/paper-pdf.sh, but tuned for these docs: the
# leading `# H1` becomes the PDF title block and the `##` sections are promoted
# to top-level sections.
#
# Usage:
#   scripts/publish-positioning-pdfs.sh                # render ALL -> stage in artifacts repo, NO push
#   scripts/publish-positioning-pdfs.sh --only vs-kiro # render a single doc (basename, .md optional)
#   scripts/publish-positioning-pdfs.sh --publish      # also commit + push to the artifacts origin (OUTWARD-FACING)
#
# Env:
#   ARTIFACTS_DIR   output dir, must live inside the artifacts git repo
#                   (default: ~/artifacts/positioning)
#
# Requires: pandoc, xelatex (texlive-xetex), DejaVu fonts, pdfinfo (poppler-utils).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_DIR="$REPO_ROOT/docs/positioning"
LUA_TABLE_WIDTHS="$REPO_ROOT/scripts/pandoc-table-widths.lua"
LUA_BREAK_CODE="$REPO_ROOT/scripts/pandoc-break-long-code.lua"
ARTIFACTS_DIR="${ARTIFACTS_DIR:-$HOME/artifacts/positioning}"
SUBTITLE="AIDA positioning · best-effort comparison — live source: github.com/joemooney/aida"

ONLY=""
PUBLISH=0
while [ $# -gt 0 ]; do
  case "$1" in
    --publish)  PUBLISH=1 ;;
    --only)     ONLY="${2:-}"; shift ;;
    --only=*)   ONLY="${1#--only=}" ;;
    -h|--help)  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)          echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

for t in pandoc xelatex pdfinfo; do
  command -v "$t" >/dev/null 2>&1 || { echo "error: '$t' not found on PATH" >&2; exit 1; }
done
[ -d "$SRC_DIR" ] || { echo "error: no positioning source dir at $SRC_DIR" >&2; exit 1; }

mkdir -p "$ARTIFACTS_DIR"

# DejaVu Serif lacks the emoji-presentation verdict glyphs the docs use in
# tables (✅ ❌ ⚠ ⭐). Remap them to Noto Sans Symbols2's monochrome cousins
# (colored, so pass/fail stays legible) at render time — the source markdown is
# untouched, so terminal/GitHub keep the real emoji.
HEADER="$(mktemp --suffix=.tex)"
cat > "$HEADER" <<'LATEX'
\usepackage{newunicodechar}
\usepackage{xcolor}
\newfontfamily\aidasym{Noto Sans Symbols2}
\definecolor{aidaok}{RGB}{22,138,73}
\definecolor{aidabad}{RGB}{197,48,48}
\definecolor{aidawarn}{RGB}{183,121,31}
\definecolor{aidastar}{RGB}{202,138,4}
\newunicodechar{✅}{{\color{aidaok}\aidasym ✔}}
\newunicodechar{❌}{{\color{aidabad}\aidasym ✘}}
\newunicodechar{⚠}{{\color{aidawarn}\aidasym ⚠}}
\newunicodechar{⭐}{{\color{aidastar}\aidasym ⭐}}
LATEX
trap 'rm -f "$HEADER"' EXIT

render_one() {
  local md="$1" base title out body
  base="$(basename "$md" .md)"
  title="$(sed -n 's/^# //p' "$md" | head -1)"
  [ -n "$title" ] || title="$base"
  out="$ARTIFACTS_DIR/$base.pdf"
  # Title/subtitle/date go through a YAML metadata block (not -V) so pandoc
  # parses them as markdown — escaping LaTeX specials and rendering any code
  # span (e.g. a title like "... (`SOME_ENV_FLAG`)"). Passing them via -V
  # injects them into \title{} literally, and a raw `_` breaks xelatex.
  # The leading H1 is dropped so it isn't rendered twice under the title block.
  body="$(mktemp)"
  {
    printf -- '---\ntitle: |\n  %s\nsubtitle: |\n  %s\ndate: |\n  %s\n---\n\n' \
      "$title" "$SUBTITLE" "rendered $(date +%Y-%m-%d)"
    awk 'BEGIN{dropped=0} /^# /&&!dropped{dropped=1;next} {print}' "$md"
  } > "$body"
  # Reader: pandoc 'markdown' (not 'gfm') so wide pipe tables get wrappable p{}
  # columns instead of natural-width 'l' columns that clip off the page. Disable
  # tex_math_dollars so '$' in prices stays literal; keep bare-URL autolinks.
  pandoc "$body" -o "$out" \
    -f markdown-tex_math_dollars+autolink_bare_uris --pdf-engine=xelatex \
    --shift-heading-level-by=-1 -H "$HEADER" \
    --lua-filter "$LUA_BREAK_CODE" \
    --lua-filter "$LUA_TABLE_WIDTHS" \
    -V mainfont="DejaVu Serif" -V sansfont="DejaVu Sans" -V monofont="DejaVu Sans Mono" \
    -V fontsize=11pt -V geometry:margin=2.2cm \
    -V colorlinks=true -V linkcolor=Mahogany -V urlcolor=Mahogany
  rm -f "$body"
  printf '  %-36s %s pages\n' "$base.pdf" "$(pdfinfo "$out" 2>/dev/null | awk '/^Pages/{print $2}')"
}

gen_index() {
  local idx="$ARTIFACTS_DIR/README.md" pdf b
  {
    echo "# AIDA positioning — PDF renders"
    echo
    echo "Reader-facing *\"should I use AIDA or X?\"* comparisons, rendered from"
    echo "[\`docs/positioning/\`](https://github.com/joemooney/aida/tree/main/docs/positioning)"
    echo "in \`joemooney/aida\`. Rendered **$(date +%Y-%m-%d)**."
    echo
    echo "These PDFs are point-in-time; the markdown in the repo is the live source"
    echo "and may be newer. Positioning is best-effort calibration, not an audited"
    echo "comparison — see each doc's \`Last updated\` line."
    echo
    for pdf in "$ARTIFACTS_DIR"/*.pdf; do
      [ -e "$pdf" ] || continue
      b="$(basename "$pdf")"
      echo "- [$b]($b)"
    done
  } > "$idx"
}

if [ -n "$ONLY" ]; then
  md="$SRC_DIR/${ONLY%.md}.md"
  [ -f "$md" ] || { echo "error: no such positioning doc: $md" >&2; exit 1; }
  echo "Rendering 1 doc -> $ARTIFACTS_DIR"
  render_one "$md"
  gen_index
else
  echo "Rendering all positioning docs -> $ARTIFACTS_DIR"
  rm -f "$ARTIFACTS_DIR"/*.pdf   # clear stale renders (renamed/removed docs)
  n=0
  for md in "$SRC_DIR"/*.md; do render_one "$md"; n=$((n+1)); done
  gen_index
  echo "rendered $n docs + index README.md"
fi

# --- stage / publish in the artifacts repo ------------------------------------
REPO="$(git -C "$ARTIFACTS_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
if [ -z "$REPO" ]; then
  echo
  echo "note: $ARTIFACTS_DIR is not inside a git repo — skipping stage/publish."
  exit 0
fi

git -C "$REPO" add "$ARTIFACTS_DIR"
if [ "$PUBLISH" -eq 1 ]; then
  if git -C "$REPO" diff --cached --quiet; then
    echo "nothing changed — not committing."
  else
    git -C "$REPO" commit -q -m "positioning: refresh PDF renders ($(date +%Y-%m-%d))"
    git -C "$REPO" push origin HEAD
    echo "published to artifacts origin."
  fi
else
  echo
  git -C "$REPO" status --short "$ARTIFACTS_DIR"
  echo
  echo "Staged in $REPO. Review, then re-run with --publish to commit + push."
fi
