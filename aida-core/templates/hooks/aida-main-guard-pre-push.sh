#!/bin/sh
# AIDA main guard pre-push hook: refuse any push to the default branch.
#
# For repos with no server-side branch protection, where the default branch
# must advance only through a reviewed PR (`aida pr ship`). Opt-in, never
# scaffolded. Install per clone (chains in front of any existing pre-push
# hook, such as the mirror fan-out hook `aida remote mirror` installs):
#
#     aida remote mirror-guard-check --install-guard
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

# The protected branch is ORIGIN's default branch, whatever remote this push
# targets: a mirror hub usually has no HEAD symref of its own. Resolved
# locally (no network). When it cannot be determined, refuse rather than
# guard a guessed branch.
default_branch=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null)
default_branch="${default_branch#origin/}"
if [ -z "$default_branch" ]; then
    printf '\npre-push: REFUSING: cannot tell origin'"'"'s default branch (refs/remotes/origin/HEAD is unset).\n' >&2
    printf 'pre-push: set it once, then push again:\n' >&2
    printf 'pre-push:     git remote set-head origin --auto\n\n' >&2
    exit 1
fi
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
