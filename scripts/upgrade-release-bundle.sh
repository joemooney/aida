#!/bin/sh
# Stage every member before replacing any installed file. Renames avoid ETXTBSY
# for running binaries; a failed replacement restores the previous set.
# trace:STORY-2704 | ai:codex
set -eu
src=$1
dest=$2
target=$3
members='aida aida-server aida-porcelain aida-flow'
legacy=0
if [ ! -f "$src/aida" ] && [ -f "$src/aida-$target" ]; then
    legacy=1
    members=aida
else
    [ -f "$src/aida" ] || { echo "release bundle missing aida" >&2; exit 1; }
    if [ -f "$src/aida-porcelain" ]; then
        [ -f "$src/aida-server" ] || { echo "release bundle missing aida-server" >&2; exit 1; }
    else
        # Older releases still carry built-in flow; preserve pinned upgrades.
        members=aida
        if [ -f "$src/aida-server" ]; then
            members='aida aida-server'
        fi
    fi
fi
stage=$(mktemp -d "$dest/.aida-upgrade-XXXXXX")
changed=''
cleanup() {
    result=$?
    trap - EXIT HUP INT TERM
    recovery_failed=0
    if [ "$result" -ne 0 ]; then
        for name in $changed; do
            if [ -e "$stage/old/$name" ] || [ -L "$stage/old/$name" ]; then
                mv -Tf "$stage/old/$name" "$dest/$name" || recovery_failed=1
            else
                rm -f "$dest/$name" || recovery_failed=1
            fi
        done
    fi
    if [ "$recovery_failed" = 1 ]; then
        echo "upgrade rollback failed; recovery files retained at $stage" >&2
        exit 1
    fi
    rm -rf "$stage"
    exit "$result"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir "$stage/new" "$stage/old"
for name in $members; do
    if [ "$name" = aida-flow ]; then
        ln -s aida-porcelain "$stage/new/$name"
    elif [ "$legacy" = 1 ]; then
        install -m 755 "$src/aida-$target" "$stage/new/aida"
    else
        install -m 755 "$src/$name" "$stage/new/$name"
    fi
    # Refuse directories before changing anything. Preserve symlinks as links.
    [ ! -d "$dest/$name" ] || { echo "install destination is a directory: $name" >&2; exit 1; }
    if [ -e "$dest/$name" ] || [ -L "$dest/$name" ]; then
        cp -Pp "$dest/$name" "$stage/old/$name"
    fi
done
for name in $members; do
    changed="$name $changed"
    mv -Tf "$stage/new/$name" "$dest/$name"
done
