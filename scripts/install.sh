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
#   ./scripts/install.sh --host gitlab            # fetch from gitlab.joemooney.com
#   ./scripts/install.sh --prefix /usr/local/bin  # install to a different directory (may need sudo)
#
# Auto-detects platform via `uname -sm`. Supported targets:
#   linux  x86_64  aarch64
#   darwin x86_64  arm64
#   windows x86_64 (Git Bash / MSYS / Cygwin)
#
# trace:EPIC-1-001 | ai:claude

set -euo pipefail

# ---- Defaults --------------------------------------------------------------

VERSION="latest"
PREFIX="$HOME/.local/bin"
REPO="joemooney/aida"
HOST="${AIDA_RELEASE_HOST:-github}"
GITLAB_PROJECT_ID="${AIDA_GITLAB_PROJECT_ID:-joemooney%2Faida}"
GITLAB_BASE_URL="${AIDA_GITLAB_BASE_URL:-https://gitlab.joemooney.com}"

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
        --host)
            shift
            [ $# -gt 0 ] || { echo "error: --host requires github or gitlab" >&2; exit 1; }
            HOST=$1
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
    MINGW*|MSYS*|CYGWIN*) os=windows ;;
    *)
        echo "error: unsupported OS: $uname_s (expected Linux, Darwin, Git Bash, MSYS, or Cygwin)" >&2
        if [ "${OS:-}" = "Windows_NT" ]; then
            echo "Download the Windows zip manually:" >&2
            echo "  https://github.com/${REPO}/releases/latest/download/aida-windows-x86_64.zip" >&2
        fi
        exit 1
        ;;
esac
case "$uname_m" in
    x86_64|amd64) arch=x86_64 ;;
    arm64|aarch64) arch=arm64 ;;
    *) echo "error: unsupported architecture: $uname_m" >&2; exit 1 ;;
esac
target="${os}-${arch}"
archive_ext="tar.gz"
download_name="aida.tar.gz"

# WSL reports as Linux, so install the Linux binary there. Git Bash/MSYS/Cygwin
# report Windows-flavored uname values and can install the Windows zip.
# trace:TASK-1212 | ai:codex
if [ "$os" = "windows" ]; then
    if [ "$arch" != "x86_64" ]; then
        echo "error: no Windows release asset for architecture: $uname_m" >&2
        echo "Download the Windows zip manually when an asset is available:" >&2
        echo "  https://github.com/${REPO}/releases/latest/download/aida-windows-x86_64.zip" >&2
        exit 1
    fi
    archive_ext="zip"
    download_name="aida.zip"
fi

# ---- URL resolution --------------------------------------------------------

case "$HOST" in
    github|GitHub)
        host=github
        ;;
    gitlab|GitLab|gitlab.joemooney.com)
        host=gitlab
        ;;
    *)
        echo "error: unsupported release host: $HOST (expected github or gitlab)" >&2
        exit 1
        ;;
esac

if [ "$VERSION" = "latest" ]; then
    if [ "$host" = "github" ]; then
        asset_url="https://github.com/${REPO}/releases/latest/download/aida-${target}.${archive_ext}"
    else
        latest_json=$(curl -fsSL "${GITLAB_BASE_URL}/api/v4/projects/${GITLAB_PROJECT_ID}/releases/permalink/latest")
        tag=$(printf '%s\n' "$latest_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
        [ -n "$tag" ] || { echo "error: could not resolve latest GitLab release tag" >&2; exit 1; }
        asset_url="${GITLAB_BASE_URL}/api/v4/projects/${GITLAB_PROJECT_ID}/packages/generic/aida/${tag}/aida-${target}.${archive_ext}"
    fi
else
    # Strip any leading "v" the user might have already included to avoid v vv.
    tag="v${VERSION#v}"
    if [ "$host" = "github" ]; then
        asset_url="https://github.com/${REPO}/releases/download/${tag}/aida-${target}.${archive_ext}"
    else
        asset_url="${GITLAB_BASE_URL}/api/v4/projects/${GITLAB_PROJECT_ID}/packages/generic/aida/${tag}/aida-${target}.${archive_ext}"
    fi
fi

# ---- Download + extract ---------------------------------------------------

tmpdir=$(mktemp -d -t aida-install-XXXXXX)
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading $asset_url"
if ! curl -fSL -o "$tmpdir/$download_name" "$asset_url"; then
    echo "error: download failed. Verify the release exists at $asset_url" >&2
    exit 1
fi

echo "Extracting..."
if [ "$archive_ext" = "zip" ]; then
    if command -v unzip >/dev/null 2>&1; then
        unzip -q "$tmpdir/$download_name" -d "$tmpdir"
    elif command -v powershell.exe >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
        win_tmpdir=$(cygpath -w "$tmpdir")
        powershell.exe -NoLogo -NoProfile -Command "Expand-Archive -LiteralPath '$win_tmpdir\\$download_name' -DestinationPath '$win_tmpdir' -Force"
    else
        echo "error: unzip is required to extract the Windows release asset" >&2
        echo "Download and extract manually:" >&2
        echo "  $asset_url" >&2
        exit 1
    fi
else
    tar xzf "$tmpdir/$download_name" -C "$tmpdir"
fi

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

# The release tarball has shipped two layouts at different times:
#   v0.4.0 era: a single file named `aida-${target}` (renamed binary).
#   future:     two files `aida` and `aida-server` at top level.
# Handle both, fail loudly if neither matches (was: silent no-op).
installed_any=0

if [ -f "$tmpdir/aida-${target}" ]; then
    "${install_cmd[@]}" "$tmpdir/aida-${target}" "$PREFIX/aida"
    echo "  installed $PREFIX/aida"
    installed_any=1
fi
if [ -f "$tmpdir/aida" ]; then
    "${install_cmd[@]}" "$tmpdir/aida" "$PREFIX/aida"
    echo "  installed $PREFIX/aida"
    installed_any=1
fi
if [ -f "$tmpdir/aida-server" ]; then
    "${install_cmd[@]}" "$tmpdir/aida-server" "$PREFIX/aida-server"
    echo "  installed $PREFIX/aida-server"
    installed_any=1
fi
if [ -f "$tmpdir/aida.exe" ]; then
    "${install_cmd[@]}" "$tmpdir/aida.exe" "$PREFIX/aida.exe"
    echo "  installed $PREFIX/aida.exe"
    installed_any=1
fi
if [ -f "$tmpdir/aida-server.exe" ]; then
    "${install_cmd[@]}" "$tmpdir/aida-server.exe" "$PREFIX/aida-server.exe"
    echo "  installed $PREFIX/aida-server.exe"
    installed_any=1
fi

if [ "$installed_any" = "0" ]; then
    echo "error: extracted tarball at $tmpdir contains no aida binary I recognize." >&2
    echo "       expected one of: aida-${target}, aida, aida.exe" >&2
    echo "       tarball contents:" >&2
    ls -la "$tmpdir" >&2
    exit 1
fi

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
if [ -x "$PREFIX/aida" ]; then
    "$PREFIX/aida" --version 2>/dev/null || echo "(installed; run 'aida --version' to verify)"
elif [ -x "$PREFIX/aida.exe" ]; then
    "$PREFIX/aida.exe" --version 2>/dev/null || echo "(installed; run 'aida.exe --version' to verify)"
else
    echo "(installed; run 'aida --version' or 'aida.exe --version' to verify)"
fi
