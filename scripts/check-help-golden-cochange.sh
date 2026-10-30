#!/usr/bin/env bash
# Fail when a branch changes the CLI command/flag surface but not the golden
# help snapshot. The snapshot test (help_surface_matches_golden_snapshot) needs
# a cargo build, so the LIGHT gate never runs it and a stale golden used to
# surface only as a red CI run. This is the no-build proxy: it compares which
# files the branch touched, nothing more.
#
# Usage: scripts/check-help-golden-cochange.sh <base-ref>
#
# A surface file change that genuinely leaves `--help` unchanged (a refactor,
# a handler-only edit) opts out with a `Help-Golden: unchanged` line in any
# commit message on the branch.
# trace:TASK-B6B1278890774EF993D2EBC327862470-125 | ai:claude
set -euo pipefail

base="${1:?usage: check-help-golden-cochange.sh <base-ref>}"
golden="aida-cli-lib/src/tests/golden/cli_help_snapshot.txt"
update_cmd="AIDA_UPDATE_SNAPSHOTS=1 cargo test --profile agent -p aida-cli-lib help_surface_matches_golden_snapshot"

merge_base="$(git merge-base "$base" HEAD 2>/dev/null)" || {
  echo "check-help-golden-cochange: cannot resolve a merge base with '$base'; fetch it first" >&2
  exit 2
}

changed="$(git diff --name-only "$merge_base" HEAD)"
surface="$(printf '%s\n' "$changed" | grep -E '^aida-cli-lib/src/([^/]+_args\.rs|command_registry\.rs)$' || true)"

if [ -z "$surface" ]; then
  exit 0
fi
if printf '%s\n' "$changed" | grep -qxF "$golden"; then
  exit 0
fi
if git log --format=%B "$merge_base..HEAD" | grep -qE '^Help-Golden:[[:space:]]*unchanged[[:space:]]*$'; then
  echo "check-help-golden-cochange: surface changed without a golden update; allowed by a 'Help-Golden: unchanged' commit trailer"
  exit 0
fi

echo "check-help-golden-cochange: these CLI surface files changed but $golden did not:" >&2
printf '  %s\n' $surface >&2
cat >&2 <<EOF

Regenerate the golden help snapshot, review its diff, and commit it:

  $update_cmd

If --help output really is unchanged, add a 'Help-Golden: unchanged' line to
one of this branch's commit messages.
EOF
exit 1
