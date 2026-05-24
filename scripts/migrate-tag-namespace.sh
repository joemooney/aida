#!/usr/bin/env bash
#
# Migrate flat hyphen-form tags (aida-*, queue-*, session-*) to the
# colon-namespaced aida:* convention.
#
# Idempotent: re-runs are safe — `aida edit --remove-tag X --add-tag Y`
# is a no-op when X is already absent and Y already present.
#
# Conservative on intent: only tags identifying a subcommand SURFACE
# migrate. Tags describing behaviors, patterns, concepts, crate names,
# or env vars stay flat (see CLAUDE.md "Tag conventions" + the spec
# body of TASK-511 for the rationale).
#
# Usage: scripts/migrate-tag-namespace.sh [--dry-run]
#
# trace:TASK-511 | ai:claude

set -euo pipefail

DRY_RUN=0
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN=1
fi

# Rename map: OLD_TAG NEW_TAG (one pair per line)
# Tags not listed here stay flat by design.
RENAMES=$(cat <<'EOF'
aida-brief        aida:brief
aida-capture      aida:capture
aida-dev          aida:dev
aida-digest       aida:digest
aida-docs         aida:docs
aida-init         aida:init
aida-mcp-serve    aida:mcp-serve
aida-pickup       aida:pickup
aida-pr           aida:pr
aida-pr-rebase    aida:pr:rebase
aida-pull         aida:pull
aida-punt         aida:punt
aida-push         aida:push
aida-queue        aida:queue
aida-queue-done   aida:queue:done
aida-review       aida:review
aida-role-show    aida:role:show
aida-session      aida:session
aida-show         aida:show
aida-status       aida:status
aida-tui          aida:tui
queue-display     aida:queue:list
queue-done        aida:queue:done
queue-list        aida:queue:list
queue-move        aida:queue:move
queue-progress    aida:queue:progress
queue-work        aida:queue:work
session-end       aida:session:end
session-forget    aida:session:forget
session-leases    aida:session:leases
session-list      aida:session:list
session-manifest  aida:session:manifest
session-prune     aida:session:prune
session-start     aida:session:start
EOF
)

CACHE="${AIDA_CACHE_PATH:-.aida/cache.db}"
if [[ ! -f "$CACHE" ]]; then
  echo "error: cache not found at $CACHE — run \`aida cache rebuild\` first" >&2
  exit 1
fi

total_edits=0
total_specs=0

# For each rename, find every spec carrying OLD and run the edit.
while read -r OLD NEW; do
  [[ -z "$OLD" ]] && continue
  # Cache stores tags as JSON arrays; match each value with a JSON-quoted string.
  mapfile -t SPECS < <(sqlite3 "$CACHE" \
    "SELECT DISTINCT spec_id FROM requirements_cache, json_each(requirements_cache.tags_json) WHERE json_each.value = '$OLD' ORDER BY spec_id")
  if [[ ${#SPECS[@]} -eq 0 ]]; then
    continue
  fi
  echo "::  $OLD  ->  $NEW  (${#SPECS[@]} specs)"
  for ID in "${SPECS[@]}"; do
    [[ -z "$ID" ]] && continue
    if [[ $DRY_RUN -eq 1 ]]; then
      echo "    [dry-run] aida edit $ID --remove-tag $OLD --add-tag $NEW"
    else
      aida edit "$ID" --remove-tag "$OLD" --add-tag "$NEW" >/dev/null
      echo "    $ID"
    fi
    total_edits=$((total_edits + 1))
  done
  total_specs=$((total_specs + ${#SPECS[@]}))
done <<<"$RENAMES"

echo
if [[ $DRY_RUN -eq 1 ]]; then
  echo "Dry-run complete: would touch $total_specs spec-edits across $(echo "$RENAMES" | grep -cv '^$') tag-renames"
else
  echo "Migration complete: $total_edits spec-edits applied"
fi
