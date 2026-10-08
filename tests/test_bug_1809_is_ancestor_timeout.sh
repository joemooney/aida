#!/bin/bash
# trace:BUG-1809 | ai:antigravity
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TEST_DIR=$(mktemp -d)

cargo build -p aida-cli
AIDA_BIN="$PROJECT_ROOT/target/debug/aida"

cd "$TEST_DIR"
git init -q
git config user.email "test@example.com"
git config user.name "Test User"
"$AIDA_BIN" init .
"$AIDA_BIN" add --type bug --title "Target" --status approved
spec_id=$("$AIDA_BIN" list --format json | jq -r '.requirements[0].id')
"$AIDA_BIN" cache rebuild

# Commit the cache state
git add -A
git commit -q -m "commit 1"

# Move HEAD forward so a read attempts an incremental update
echo "change" > some_file
git add some_file
git commit -q -m "commit 2"

mkdir -p bin
real_git=$(command -v git)
cat << GITSCRIPT > bin/git
#!/bin/bash
if [[ "\$*" == *"merge-base --is-ancestor"* ]]; then
    sleep 2
    exit 0
fi
exec "$real_git" "\$@"
GITSCRIPT
chmod +x bin/git

export PATH="$PWD/bin:$PATH"
export AIDA_CACHE_READ_WAIT_MS=50

start=$(date +%s)
"$AIDA_BIN" show "$spec_id" >/dev/null
end=$(date +%s)

duration=$((end - start))
if [ "$duration" -ge 2 ]; then
    echo "FAIL: aida show hung on git merge-base --is-ancestor ($duration seconds)"
    exit 1
fi

echo "OK"
