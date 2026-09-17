#!/usr/bin/env bash
set -euo pipefail

# TUI seam guard: reads in aida-tui go through aida-core, while mutations cross
# the CLI subprocess boundary. The compile graph must never grow an aida-tui ->
# aida-cli-lib edge. trace:TASK-1252 | ai:codex

cd "$(dirname "$0")/.."

echo "==> cargo check -p aida-tui --all-targets"
cargo check -p aida-tui --all-targets

echo "==> verifying aida-tui has no aida-cli-lib dependency"
python3 - <<'PY'
import json
import subprocess
import sys

metadata = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        text=True,
    )
)
packages = {pkg["name"]: pkg for pkg in metadata["packages"]}
tui = packages.get("aida-tui")
if tui is None:
    print("error: cargo metadata did not include aida-tui", file=sys.stderr)
    sys.exit(1)

direct = [dep["name"] for dep in tui["dependencies"]]
if "aida-cli-lib" in direct:
    print("error: aida-tui must not directly depend on aida-cli-lib", file=sys.stderr)
    sys.exit(1)
PY

tree="$(cargo tree -p aida-tui --edges normal,build --prefix none)"
if printf '%s\n' "$tree" | awk '{print $1}' | grep -qx 'aida-cli-lib'; then
  echo "error: aida-tui dependency tree includes aida-cli-lib" >&2
  exit 1
fi

echo "ok: aida-tui checks standalone and has no aida-cli-lib edge"
