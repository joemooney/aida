#!/bin/sh
# AIDA store pre-push hook: preserve the identity gate without project hooks.
# Only policy data from the owning project's config is inspected; nothing in
# the writable store is sourced or resolved as an executable.
# trace:BUG-1929 | ai:codex
refs=$(cat)
top=$(git rev-parse --show-toplevel 2>/dev/null) || top=
gitdir=$(git rev-parse --absolute-git-dir 2>/dev/null) || gitdir=
common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || common=
cfg=
if [ -n "$gitdir" ] && [ -n "$common" ] && [ "$gitdir" != "$common" ]; then
    case "$common" in */.git) cfg="${common%/.git}/.aida/config.toml" ;; esac
else
    case "$top" in */.aida-store) cfg="${top%/.aida-store}/.aida/config.toml" ;; esac
fi
# Standalone override clones may have no owning project and no installed
# identity hook. Preserve that opt-in policy: an existing identity hook still
# fails closed when its owner is unknown. Only inspect its marker as data;
# project hook contents are never executed.
if [ -z "$cfg" ]; then
    project_hooks=$(git config --local --type=path --get core.hooksPath 2>/dev/null) || project_hooks=
    case "$project_hooks" in
        '') project_hooks="$common/hooks" ;;
        /*) ;;
        *) project_hooks="$top/$project_hooks" ;;
    esac
    if ! grep -qF 'aida identity check-push' -- "$project_hooks/pre-push" 2>/dev/null; then
        exit 0
    fi
fi
if command -v aida >/dev/null 2>&1 && aida identity check-push --help >/dev/null 2>&1; then
    printf '%s\n' "$refs" | aida identity check-push "$1" || exit 1
elif [ -z "$cfg" ]; then
    printf '%s\n' 'identity gate: cannot determine the owning project and aida identity check-push is unavailable; refusing store push.' >&2
    exit 1
elif [ -f "$cfg" ] && grep -q allowed_emails "$cfg" 2>/dev/null; then
    printf '%s\n' 'identity gate: allowed_emails is configured but aida identity check-push is unavailable; refusing store push.' >&2
    exit 1
fi
exit 0
