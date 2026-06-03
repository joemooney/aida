#!/usr/bin/env bash
#
# Render every AIDA presentation deck to HTML and generate an index.html that
# links them. Output goes to docs/presentation/build/ by default (gitignored).
#
#   ./docs/presentation/build.sh            # render to docs/presentation/build/
#   ./docs/presentation/build.sh /tmp/out   # render to a custom dir
#   PDF=1 ./docs/presentation/build.sh      # also emit a .pdf per deck
#
# Requires: npx (Node). marp-cli is fetched on demand via `npx --yes`.
# trace:TASK-637 | ai:claude
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="${1:-$HERE/build}"
MARP=(npx --yes @marp-team/marp-cli@latest)
mkdir -p "$OUT"

# Deck table: "<source-basename>|<Title>|<Audience / description>"
# (source files live next to this script).
DECKS=(
  "aida-executive-briefing|Executive Briefing|Executives & leadership — problem, wedge, moat, proof, ask"
  "aida-developer-deep-dive|Developer Deep Dive (under the hood)|Engineers — storage, distributed IDs, the orchestrator, MCP, traceability"
  "aida-administrator-guide|Administrator Guide|Operators — init, multi-node, config, maintenance, doctor, security"
  "aida-user-walkthrough|User Walkthrough|Daily users — the loop, queue, autonomous drain, traces, lifecycle, TUI"
  # The operator's live-demo deck, included automatically when present.
  "2026-06-management-demo|Management Live-Demo|Semi-technical leadership — the live terminal demo is the argument"
)

CARDS=""
RENDERED=0
for entry in "${DECKS[@]}"; do
  IFS='|' read -r slug title desc <<<"$entry"
  src="$HERE/$slug.md"
  if [[ ! -f "$src" ]]; then
    echo "skip (missing): $slug.md"
    continue
  fi
  echo "rendering: $title"
  "${MARP[@]}" "$src" -o "$OUT/$slug.html" >/dev/null
  if [[ "${PDF:-0}" == "1" ]]; then
    "${MARP[@]}" "$src" --pdf -o "$OUT/$slug.pdf" >/dev/null
    pdflink=" &nbsp;<a class=\"pdf\" href=\"$slug.pdf\">PDF</a>"
  else
    pdflink=""
  fi
  CARDS+="      <li><a class=\"deck\" href=\"$slug.html\">$title</a>$pdflink<p>$desc</p></li>"$'\n'
  RENDERED=$((RENDERED + 1))
done

# Version for the index footer — read from the workspace Cargo.toml (repo-local,
# deterministic; no dependence on whichever `aida` is on PATH).
stats=""
ver="$(grep -m1 '^version' "$HERE/../../Cargo.toml" 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
[[ -n "$ver" ]] && stats="AIDA v$ver"
gen_date="$(date -u +%Y-%m-%d)"

cat >"$OUT/index.html" <<HTML
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>AIDA — Presentations</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 16px/1.5 -apple-system, system-ui, "Segoe UI", Roboto, sans-serif;
         max-width: 760px; margin: 3rem auto; padding: 0 1.25rem; }
  h1 { margin-bottom: .15rem; }
  .sub { color: #888; margin-top: 0; }
  ul { list-style: none; padding: 0; }
  li { border: 1px solid #8884; border-radius: 10px; padding: 1rem 1.2rem; margin: .8rem 0; }
  li:hover { border-color: #888a; }
  a.deck { font-size: 1.15rem; font-weight: 600; text-decoration: none; }
  a.deck:hover { text-decoration: underline; }
  a.pdf { font-size: .8rem; color: #888; text-decoration: none; border: 1px solid #8884;
          border-radius: 5px; padding: 0 .4rem; }
  li p { margin: .35rem 0 0; color: #888; font-size: .92rem; }
  footer { margin-top: 2.5rem; color: #999; font-size: .82rem; }
</style>
</head>
<body>
  <h1>AIDA — Presentations</h1>
  <p class="sub">Audience-targeted slide decks. Pick your perspective.</p>
  <ul>
$CARDS  </ul>
  <footer>${stats:+$stats · }Generated $gen_date · regenerate with <code>docs/presentation/build.sh</code></footer>
</body>
</html>
HTML

echo
echo "Done: $RENDERED deck(s) + index.html → $OUT/index.html"
