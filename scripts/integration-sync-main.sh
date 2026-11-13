#!/usr/bin/env bash
# Bring origin/main into the integration branch the one safe way (STORY-2737).
#
#   scripts/integration-sync-main.sh [--dry-run] [--no-push]
#
# Run from a checkout on `integration`. It never rebases, never force-pushes,
# and picks the merge kind from the content, not from a message:
#   - main already in integration      -> nothing to do
#   - main's tree equals an integration
#     first-parent commit (a promotion) -> `git merge -s ours origin/main`
#   - anything else (main moved on its
#     own: a train, a direct PR)        -> real `git merge origin/main`
# Then `make check-ci-fast`, push, and print the line to send the advisor.
# trace:STORY-2737 | ai:claude
set -euo pipefail

dry_run=0 push=1
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    --no-push) push=0 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

die() { echo "integration-sync-main: $*" >&2; exit 1; }

branch=$(git symbolic-ref --short -q HEAD || true)
[ "$branch" = integration ] || die "on '${branch:-detached HEAD}', not 'integration'"
[ -z "$(git status --porcelain --untracked-files=no)" ] || die "tracked changes present; commit or discard them first"

git fetch -q origin main integration
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/integration)" ] \
  || die "local integration ($(git rev-parse --short HEAD)) != origin/integration ($(git rev-parse --short origin/integration)); pull --ff-only or push first"

main=$(git rev-parse origin/main)
short_main=$(git rev-parse --short "$main")

if git merge-base --is-ancestor "$main" HEAD; then
  echo "origin/main ($short_main) is already in integration; nothing to do."
  exit 0
fi

# A promotion squashes an integration head into main, so main's tree equals
# that head's tree. Only then may -s ours record main without its content.
main_tree=$(git rev-parse "$main^{tree}")
promoted=""
while read -r c; do
  if [ "$(git rev-parse "$c^{tree}")" = "$main_tree" ]; then promoted=$c; break; fi
done < <(git rev-list --first-parent -n 500 HEAD)

subjects=""
if [ -n "$promoted" ]; then
  kind="promotion sync (-s ours): main $short_main has the tree of integration $(git rev-parse --short "$promoted")"
  merge=(git merge -s ours --no-edit -m "[AI:claude] chore(integration): record main $short_main (promotion of $(git rev-parse --short "$promoted")) as merged (STORY-2737)" "$main")
else
  subjects=$(git log --format='  %h %s' "HEAD..$main" | head -20)
  kind="real merge: main $short_main moved independently of integration"
  merge=(git merge --no-ff -m "[AI:claude] chore(integration): merge main $short_main (STORY-2737)" "$main")
fi

echo "$kind"
[ -n "$subjects" ] && printf 'main commits not in integration:\n%s\n' "$subjects"
if [ "$dry_run" = 1 ]; then
  echo "(dry run) would run: ${merge[*]}"
  exit 0
fi

if ! "${merge[@]}"; then
  echo >&2
  die "merge stopped with conflicts. Resolve them and 'git commit', then run 'make check-ci-fast' and push; or 'git merge --abort'. Never rebase integration."
fi
if [ -n "$promoted" ]; then
  git diff --quiet HEAD^1 HEAD || die "BUG: -s ours changed the tree; inspect before pushing"
fi

make check-ci-fast || die "check-ci-fast failed on the merge commit $(git rev-parse --short HEAD); NOT pushed. Fix forward, or 'git reset --hard origin/integration' to drop the local merge."

if [ "$push" = 1 ]; then
  git push origin integration
  echo
  echo "Send the advisor: integration pushed at $(git rev-parse --short HEAD) (${kind%%:*} of main $short_main)"
else
  echo "Merged locally at $(git rev-parse --short HEAD); not pushed (--no-push)."
fi
