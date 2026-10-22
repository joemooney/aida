#!/bin/sh
# AIDA main guard pre-push hook: refuse any push to the default branch.
#
# For repos with no server-side branch protection, where the default branch
# must advance only through a reviewed PR (`aida pr ship`). Opt-in, never
# scaffolded. Install per clone:
#
#     aida remote mirror-guard-check --print-guard \
#         > "$(git rev-parse --git-common-dir)/hooks/pre-push"
#     chmod 755 "$(git rev-parse --git-common-dir)/hooks/pre-push"
#
# The one exception: `aida remote mirror-sync` pushing origin's reviewed
# default-branch tip to a registered mirror hub. Before refusing, the guard
# pipes the ref lines to `aida remote mirror-guard-check`, which allows the
# push only when the remote is a registered mirror (never origin), no ref is
# a deletion, every default-branch sha equals origin's live tip, and
# mirror-sync attested exactly that push. When `aida` is missing or says no,
# the guard refuses.
#
# A project-local guard gets the same exception by capturing stdin first and
# adding, just before it refuses:
#
#     printf '%s\n' "$refs" | aida remote mirror-guard-check "$1" && exit 0
#
# trace:BUG-2015 | ai:claude

remote="${1:-origin}"

# Resolve the default branch locally (no network): origin's HEAD symref,
# else `main`.
default_branch=$(git symbolic-ref --quiet --short "refs/remotes/$remote/HEAD" 2>/dev/null)
default_branch="${default_branch#"$remote/"}"
[ -n "$default_branch" ] || default_branch=main
protected="refs/heads/$default_branch"

refs=$(cat)

# Only the remote ref decides: that is what changes on the far side. A
# deletion of the default branch names it too and is refused the same way.
offenders=$(printf '%s\n' "$refs" | while read -r local_ref local_sha remote_ref remote_sha; do
    [ "$remote_ref" = "$protected" ] && printf '  %s <- %s (%.12s)\n' "$remote_ref" "${local_ref:-?}" "$local_sha"
done)
[ -z "$offenders" ] && exit 0

if command -v aida >/dev/null 2>&1 \
    && printf '%s\n' "$refs" | aida remote mirror-guard-check "$remote"; then
    exit 0
fi

printf '\npre-push: REFUSING to push to %s, the default branch.\n' "$default_branch" >&2
printf '%s\n' "$offenders" | sed 's/^/pre-push:/' >&2
printf 'pre-push: land it through a PR instead:\n' >&2
printf 'pre-push:     git push -u origin <your-branch>   # then open a PR\n' >&2
printf 'pre-push:     aida pr ship <N>\n\n' >&2
exit 1
