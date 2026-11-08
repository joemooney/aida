#!/usr/bin/env bash
# trace:STORY-2672 | ai:codex
set -euo pipefail
[ "$(id -u)" -ne 0 ]
for tool in cc git curl python3 pkg-config protoc zsh rg bwrap; do
    command -v "$tool" >/dev/null
done
pkg-config --exists openssl gtk+-3.0 xcb-render xcb-shape xcb-xfixes xkbcommon
# A runner must never inherit access to the operator's home or privileges.
[ ! -r /home/dirtlab/.codex/auth.json ]
[ ! -x /home/dirtlab ]
if sudo -n true 2>/dev/null; then
    echo 'Runner unexpectedly has sudo access' >&2
    exit 1
fi
bwrap --unshare-all --cap-drop ALL --ro-bind / / -- /usr/bin/python3 -I -S -c 'import ctypes as C; lib=C.CDLL(None,use_errno=True); rc=lib.unshare(0x10000000|0x40000000); assert rc==0,"NESTED_USERNS_UNAVAILABLE: errno=%d" % C.get_errno()'
