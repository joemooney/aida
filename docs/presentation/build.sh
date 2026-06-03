#!/usr/bin/env bash
#
# Render every AIDA presentation deck to HTML, embed an in-browser player for
# every asciinema .cast (docs/casts/ + docs/presentation/), and generate an
# index.html that links them all. Output → docs/presentation/build/ (gitignored).
#
#   ./docs/presentation/build.sh            # render to docs/presentation/build/
#   ./docs/presentation/build.sh /tmp/out   # render to a custom dir
#   PDF=1 ./docs/presentation/build.sh      # also emit a .pdf per deck
#
# Requires: npx (Node) for the decks; marp-cli is fetched on demand via `npx
# --yes`. Casts use the vendored asciinema-player in vendor/ (offline-capable),
# falling back to CDN when those assets are absent.
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

# --- asciinema casts -------------------------------------------------------
# Embed an in-browser player for every .cast found in docs/casts/ (curated,
# tracked) and docs/presentation/ (the live-demo cast). Player assets are
# vendored (docs/presentation/vendor/) so casts play offline / air-gapped;
# falls back to CDN with a warning when the vendored assets are absent.
ASCIINEMA_VER="3.8.0"
shopt -s nullglob
CASTS=("$HERE/../casts"/*.cast "$HERE"/*.cast)
shopt -u nullglob
CAST_COUNT=0
if [[ ${#CASTS[@]} -gt 0 ]]; then
  if [[ -f "$HERE/vendor/asciinema-player.min.js" && -f "$HERE/vendor/asciinema-player.css" ]]; then
    cp "$HERE/vendor/asciinema-player.min.js" "$HERE/vendor/asciinema-player.css" "$OUT/"
    PLAYER_JS="asciinema-player.min.js"
    PLAYER_CSS="asciinema-player.css"
  else
    echo "warning: vendored asciinema-player assets not found in vendor/ — using CDN (needs network to view)"
    PLAYER_JS="https://cdn.jsdelivr.net/npm/asciinema-player@${ASCIINEMA_VER}/dist/bundle/asciinema-player.min.js"
    PLAYER_CSS="https://cdn.jsdelivr.net/npm/asciinema-player@${ASCIINEMA_VER}/dist/bundle/asciinema-player.css"
  fi
  PLAYERS_HTML=""
  INIT_JS=""
  for cast in "${CASTS[@]}"; do
    base="$(basename "$cast")"
    # Skip a second cast with the same basename (e.g. the same file in both
    # docs/casts/ and docs/presentation/) — tracked per-run, not via $OUT, so a
    # re-run into an existing build/ doesn't false-skip. (bash-3 portable.)
    case " ${SEEN_CASTS:-} " in *" $base "*) echo "skip (dup cast name): $base"; continue ;; esac
    SEEN_CASTS="${SEEN_CASTS:-} $base"
    cp "$cast" "$OUT/$base"   # keep a copy too (for http serving / download)
    # Title: drop .cast + any leading ISO timestamp (2026-05-24T025421Z-foo → foo).
    title="$(echo "${base%.cast}" | sed -E 's/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{6}Z-//')"
    id="cap$CAST_COUNT"
    echo "embedding cast: $title"
    # Inline the cast as base64 and decode it UTF-8-safe in the browser, so the
    # player never has to fetch() the file — that fetch is blocked by the
    # Same-Origin Policy when the page is opened from file:// (the "CORS request
    # not http" error). Inlining makes casts.html self-contained: it plays
    # double-clicked AND over http. trace:TASK-637 | ai:claude
    b64="$(base64 -w0 "$cast" 2>/dev/null || base64 "$cast" | tr -d '\n')"
    PLAYERS_HTML+="    <div class=\"cast\"><h2>$title</h2><div id=\"$id\"></div></div>"$'\n'
    INIT_JS+="    (function(){var d=new TextDecoder().decode(Uint8Array.from(atob('$b64'),function(c){return c.charCodeAt(0)}));AsciinemaPlayer.create({data:d},document.getElementById('$id'),{fit:'width',terminalFontSize:'14px'});})();"$'\n'
    CAST_COUNT=$((CAST_COUNT + 1))
  done

  if [[ $CAST_COUNT -gt 0 ]]; then
    cat >"$OUT/casts.html" <<HTML
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>AIDA — Casts</title>
<link rel="stylesheet" href="$PLAYER_CSS">
<style>
  body { font: 16px/1.5 -apple-system, system-ui, "Segoe UI", Roboto, sans-serif;
         max-width: 900px; margin: 2.5rem auto; padding: 0 1.25rem; color-scheme: light dark; }
  a.back { color: #888; text-decoration: none; font-size: .9rem; }
  .cast { margin: 2rem 0; }
  .cast h2 { font-size: 1rem; margin: 0 0 .5rem; }
</style>
</head>
<body>
  <a class="back" href="index.html">&larr; index</a>
  <h1>AIDA — Casts</h1>
$PLAYERS_HTML
  <script src="$PLAYER_JS"></script>
  <script>
$INIT_JS  </script>
</body>
</html>
HTML
    CARDS+="      <li><a class=\"deck\" href=\"casts.html\">&#9654; Casts ($CAST_COUNT)</a><p>Recorded terminal sessions — play inline in the browser, no install.</p></li>"$'\n'
  fi
fi

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
echo "Done: $RENDERED deck(s)${CAST_COUNT:+ + $CAST_COUNT cast(s)} + index.html → $OUT/index.html"
