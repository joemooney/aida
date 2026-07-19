#!/usr/bin/env bash
# trace:TASK-1121 | ai:claude
# Render a research/plan markdown doc to a PDF sibling via pandoc + xelatex.
#
# Usage:
#   scripts/paper-pdf.sh                          # builds the coordination paper
#   scripts/paper-pdf.sh docs/research/foo.md     # builds any markdown doc
#   PAPER_SUBTITLE="..." scripts/paper-pdf.sh     # override the subtitle line
#
# Requires: pandoc, xelatex (texlive-xetex), DejaVu fonts — all stock Ubuntu
# packages. DejaVu is deliberate: it covers the box-drawing diagrams and the
# arrow/math glyphs (→ × ≥ ⊥) the research docs use; prettier serif faces drop
# them silently.
set -euo pipefail

MD="${1:-docs/research/2026-07-08-coordinating-multi-vendor-agent-fleets.md}"
OUT="${MD%.md}.pdf"
SUBTITLE="${PAPER_SUBTITLE:-Design-science findings from a home-grown probe into multi-vendor agent coordination — working draft}"

[ -f "$MD" ] || { echo "error: no such file: $MD" >&2; exit 1; }

pandoc "$MD" -o "$OUT" \
  --pdf-engine=xelatex \
  --shift-heading-level-by=-1 \
  --toc --toc-depth=2 \
  -V mainfont="DejaVu Serif" -V sansfont="DejaVu Sans" -V monofont="DejaVu Sans Mono" \
  -V fontsize=10pt -V geometry:margin=2.4cm \
  -V colorlinks=true -V linkcolor=Mahogany -V urlcolor=Mahogany \
  -V subtitle="$SUBTITLE" \
  -V date="rendered $(date +%Y-%m-%d)"

echo "wrote $OUT ($(pdfinfo "$OUT" 2>/dev/null | awk '/^Pages/{print $2}') pages)"
