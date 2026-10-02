#!/usr/bin/env bash
# scripts/install-agent-skill-hooks.sh — install the main-checkout git hooks
# (post-merge / post-checkout / post-rewrite) that regenerate .agents/skills
# after a skill-master change lands (BUG-1760's regeneration half).
#
# Two modes:
#   default        Loud, behind `make install-agent-skill-hooks`: refuses from
#                  a linked worktree, and refuses to overwrite a hook it does
#                  not own (anything that is not already the
#                  aida-sync-agent-skills symlink) rather than clobbering it.
#   --best-effort  Quiet, behind the `make build` / `make build-fast`
#                  prerequisite: exits 0 where the loud mode refuses (linked
#                  worktree, non-repo), and warns-and-skips a foreign hook, so
#                  a routine build never fails over hook installation.
#
# Both modes are idempotent: a hook that already points at the master is left
# untouched, so re-running changes nothing and exits 0.
#
# trace:BUG-1762 | ai:claude
set -euo pipefail

best_effort=0
if [ "${1:-}" = "--best-effort" ]; then
    best_effort=1
fi

refuse() {
    if [ "$best_effort" -eq 1 ]; then
        exit 0
    fi
    echo "$*" >&2
    exit 1
}

root=$(git rev-parse --show-toplevel 2>/dev/null) ||
    refuse "install-agent-skill-hooks: not inside a git repository"
git_dir=$(git rev-parse --absolute-git-dir)
common_dir=$(git rev-parse --path-format=absolute --git-common-dir)

# The portable pack belongs to the main checkout, never to linked worktrees
# (same ownership rule as the hook master itself).
if [ "$git_dir" != "$common_dir" ]; then
    refuse "Refusing to install portable-pack hooks from a linked worktree"
fi

master="$root/aida-core/templates/hooks/aida-sync-agent-skills.sh"
if [ ! -f "$master" ]; then
    refuse "install-agent-skill-hooks: hook master not found: $master"
fi

hooks="$common_dir/hooks"
mkdir -p "$hooks"

status=0
for name in post-merge post-checkout post-rewrite; do
    hook="$hooks/$name"
    if [ -L "$hook" ] && [ "$(readlink "$hook")" = "$master" ]; then
        continue
    fi
    if [ -e "$hook" ] || [ -L "$hook" ]; then
        if [ "$best_effort" -eq 1 ]; then
            echo "install-agent-skill-hooks: skipping $name — a hook that is not ours already exists at $hook" >&2
            continue
        fi
        echo "Refusing to overwrite existing hook: $hook" >&2
        echo "  It is not the aida-sync-agent-skills symlink. Move it aside, or chain" >&2
        echo "  to $master from it yourself." >&2
        status=1
        continue
    fi
    ln -s "$master" "$hook"
    echo "  Linked: $hook"
done
exit "$status"
