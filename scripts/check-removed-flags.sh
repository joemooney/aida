#!/usr/bin/env bash
set -euo pipefail

# trace:STORY-1028 | ai:codex
root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$root"

common_rg=(
  rg -n
  --glob '!target/**'
  --glob '!bench/agent-surface/results/**'
  --glob '!docs/casts/**'
  --glob '!scripts/check-removed-flags.sh'
)

literal_patterns=(
  'aida usage --slowest'
  'aida usage --unused'
  'aida usage --errors'
  'aida usage --events'
  'aida usage --auto-complete'
  'aida usage --health'
  'aida release --patch'
  'aida release --minor'
  'aida release --major'
  'aida release --check'
  'aida focus --clear'
  'aida focus --show'
  'aida drain status --clear'
  'aida upgrade --check'
  'aida upgrade --diff'
  'aida statusline --title'
  'aida history --events'
)

failed=0
for pattern in "${literal_patterns[@]}"; do
  if "${common_rg[@]}" --fixed-strings "$pattern"; then
    failed=1
  fi
done

graph_regex='(\baida|\$AIDA_BIN|"\$AIDA_BIN") graph( +("[^"]+"|<[^>]+>|\$[A-Za-z_][A-Za-z0-9_]*|[A-Za-z0-9_{}./:-]+))? +--(blocked-by|blocks|tree|impact)\b|\bgraph +--(blocked-by|blocks|tree|impact)\b'
if "${common_rg[@]}" "$graph_regex"; then
  failed=1
fi

if (( failed )); then
  echo "old mode-selecting CLI flags found; use subcommands instead" >&2
  exit 1
fi
