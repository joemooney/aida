#!/usr/bin/env bash
# trace:STORY-2672 | ai:codex
set -euo pipefail
refuse() { echo "Runner prerequisite refused: $*" >&2; exit 1; }
[ "$(id -u)" -ne 0 ] || refuse "runner must be unprivileged"
for tool in cc git curl python3 pkg-config protoc zsh rg bwrap zstd jq ssh-keygen; do
    command -v "$tool" >/dev/null || refuse "missing tool: $tool"
done
pkg-config --exists openssl gtk+-3.0 xcb-render xcb-shape xcb-xfixes xkbcommon || refuse "missing build libraries"
# A runner must never inherit access to the operator's home or privileges.
[ ! -r /home/dirtlab/.codex/auth.json ] || refuse "operator Codex auth is readable"
[ ! -x /home/dirtlab ] || refuse "operator home is traversable"
if sudo -n true 2>/dev/null; then
    echo 'Runner unexpectedly has sudo access' >&2
    exit 1
fi
bwrap --unshare-all --cap-drop ALL --ro-bind / / --dev /dev --proc /proc -- /usr/bin/python3 -I -S -c 'import ctypes as C; lib=C.CDLL(None,use_errno=True); rc=lib.unshare(0x10000000|0x40000000); assert rc==0,"NESTED_USERNS_UNAVAILABLE: errno=%d" % C.get_errno()'
