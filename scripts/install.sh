#!/usr/bin/env bash
# scripts/install.sh — install a released aida binary.
#
# This is the end-user install path (no Rust toolchain needed). For the
# AIDA-developer install path that uses your in-repo build instead, run
# `make install` from the AIDA repo.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash
#   curl -sSL https://raw.githubusercontent.com/joemooney/aida/main/scripts/install.sh | bash -s -- --version v0.4.0
#   ./scripts/install.sh                          # latest release, install to ~/.local/bin
#   ./scripts/install.sh --version v0.4.0         # pin a specific release
#   ./scripts/install.sh --prefix /usr/local/bin  # install to a different directory (may need sudo)
#
# Auto-detects platform via `uname -sm`. Supported targets:
#   linux  x86_64  aarch64
#   darwin x86_64  arm64
#
# trace:EPIC-1-001 | ai:claude

set -euo pipefail

# ---- Defaults --------------------------------------------------------------

VERSION="latest"
PREFIX="$HOME/.local/bin"
REPO="joemooney/aida"

# ---- Arg parsing -----------------------------------------------------------

usage() {
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            shift
            [ $# -gt 0 ] || { echo "error: --version requires an argument (e.g. v0.4.0)" >&2; exit 1; }
            VERSION=$1
            shift
            ;;
        --prefix)
            shift
            [ $# -gt 0 ] || { echo "error: --prefix requires a directory" >&2; exit 1; }
            PREFIX=$1
            shift
            ;;
        -h|--help) usage 0 ;;
        *)
            echo "error: unrecognized argument: $1" >&2
            usage 1
            ;;
    esac
done

# ---- Platform detection ----------------------------------------------------

uname_s=$(uname -s)
uname_m=$(uname -m)
case "$uname_s" in
    Linux)  os=linux  ;;
    Darwin) os=darwin ;;
    *) echo "error: unsupported OS: $uname_s (expected Linux or Darwin)" >&2; exit 1 ;;
esac
case "$uname_m" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) echo "error: unsupported architecture: $uname_m" >&2; exit 1 ;;
esac
target="${os}-${arch}"

# ---- URL resolution --------------------------------------------------------

if [ "$VERSION" = "latest" ]; then
    asset_url="https://github.com/${REPO}/releases/latest/download/aida-${target}.tar.gz"
else
    # Strip any leading "v" the user might have already included to avoid v vv.
    tag="v${VERSION#v}"
    asset_url="https://github.com/${REPO}/releases/download/${tag}/aida-${target}.tar.gz"
fi

# ---- Download + extract ---------------------------------------------------

tmpdir=$(mktemp -d -t aida-install-XXXXXX)
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading $asset_url"
if ! curl -fSL -o "$tmpdir/aida.tar.gz" "$asset_url"; then
    echo "error: download failed. Verify the release exists at $asset_url" >&2
    exit 1
fi

echo "Extracting..."
tar xzf "$tmpdir/aida.tar.gz" -C "$tmpdir"

# ---- Install ---------------------------------------------------------------

mkdir -p "$PREFIX"

# If the destination is not writable, escalate via sudo.
if [ -w "$PREFIX" ]; then
    install_cmd=(install -m 755)
else
    if ! command -v sudo >/dev/null 2>&1; then
        echo "error: $PREFIX is not writable and sudo is not available" >&2
        exit 1
    fi
    echo "Note: $PREFIX is not user-writable; using sudo for install."
    install_cmd=(sudo install -m 755)
fi

for bin in aida aida-server; do
    src="$tmpdir/$bin"
    if [ -f "$src" ]; then
        "${install_cmd[@]}" "$src" "$PREFIX/$bin"
        echo "  installed $PREFIX/$bin"
    fi
done

# ---- Post-install --------------------------------------------------------

case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *)
        echo
        echo "Note: $PREFIX is not on your PATH."
        echo "      Add it to your shell rc, e.g.:"
        echo "        export PATH=\"$PREFIX:\$PATH\""
        ;;
esac

echo
"$PREFIX/aida" --version 2>/dev/null || echo "(installed; run 'aida --version' to verify)"
