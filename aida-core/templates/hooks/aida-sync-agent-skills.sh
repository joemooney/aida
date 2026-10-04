#!/bin/sh
# trace:BUG-1760 | ai:codex

warn() {
    printf '%s\n' "aida-sync-agent-skills: $*" >&2
}

git_dir=$(git rev-parse --absolute-git-dir 2>/dev/null) || exit 0
common_dir=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0

# The portable pack belongs to the main checkout, never to linked worktrees.
if [ "$git_dir" != "$common_dir" ]; then
    exit 0
fi

if [ ! -x "$(git rev-parse --show-toplevel 2>/dev/null)/target/debug/examples/agent_skill_pack" ]; then
    warn "example is not built; run 'make sync-templates' to regenerate .agents/skills"
    exit 0
fi

if ! make --no-print-directory sync-agent-skills >/dev/null; then
    warn "pack regeneration failed; run 'make sync-templates' to repair it"
fi
exit 0
