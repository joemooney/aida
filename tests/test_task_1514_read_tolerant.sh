#!/bin/bash
set -euo pipefail

# trace:TASK-1514 | ai:antigravity
# A test: stale cache + live foreign writer, aida graph tree <EPIC> completes in < 2 s and exits 0

# Pre-build to avoid compile times skewing the result
cargo build --bin aida

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
AIDA="$PWD/target/debug/aida"

cd "$TMP_DIR"
"$AIDA" init --name test_proj > /dev/null
"$AIDA" add --type epic --title "Root Epic" --prefix EPIC > /dev/null

# Get the epic ID
EPIC_ID=$("$AIDA" list --type epic --json | jq -r '.[0].id')

# Simulate a foreign writer taking the refresh flock
echo "Taking refresh flock..."
# Actually we also need the cache to be "stale". Let's modify the git store manually.
# Create a new commit in the git store.
git -C .aida-store commit --allow-empty -m "Foreign writer" > /dev/null

# Take the lock
mkfifo flock_in flock_out
python3 -c "import fcntl,sys; f=open(sys.argv[1],'w'); fcntl.flock(f,fcntl.LOCK_EX); print('ready',flush=True); sys.stdin.readline()" .aida/cache.db.refresh.lock < flock_in > flock_out &
PY_PID=$!
exec 3> flock_in
read -r line < flock_out

echo "Running aida graph tree $EPIC_ID..."
START=$(date +%s.%N)
"$AIDA" graph tree "$EPIC_ID" > /dev/null
END=$(date +%s.%N)

# Release the flock
echo >&3
wait $PY_PID

DURATION=$(echo "$END - $START" | bc)
echo "Graph tree took $DURATION seconds."

if (( $(echo "$DURATION >= 2.0" | bc -l) )); then
    echo "FAILED: expected < 2 seconds, got $DURATION"
    exit 1
fi
echo "OK"
