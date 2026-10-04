#!/bin/bash
set -euxo pipefail

# trace:TASK-1566 | ai:claude
# Test for sweep-blob-tags.py

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

REPO_ROOT="$PWD"
AIDA="$REPO_ROOT/target/debug/aida"
SWEEP_SCRIPT="$REPO_ROOT/scripts/sweep-blob-tags.py"

cd "$TMP_DIR"
git init --initial-branch=main
echo 1 | "$AIDA" init --name test_proj

# Create a test object
"$AIDA" add --type task --title "Test task" --prefix TASK > /dev/null
TASK_ID=$("$AIDA" list | grep "TASK-" | head -1 | awk '{print $1}')
FILE_PATH=$(find .aida-store/objects -type f -name "*.yaml" | head -1)

# Remove any existing tags keys and items using awk or python. 
# It's easier to just use awk or python to ensure portability without relying on sed -i
python3 -c '
import sys, yaml
with open(sys.argv[1], "r") as f: data = yaml.safe_load(f)
data["tags"] = ["clean-tag", "blob1 blob2"]
with open(sys.argv[1], "w") as f: yaml.safe_dump(data, f, sort_keys=False)
' "$FILE_PATH"


# AC5: A control: an object with a colon-namespaced tag (severity:major) and no blob
"$AIDA" add --type task --title "Control task" --prefix TASK > /dev/null
CONTROL_ID=$("$AIDA" list | grep "TASK-" | grep -v "$TASK_ID" | head -1 | awk '{print $1}')
CONTROL_FILE=$(find .aida-store/objects -type f -name "*.yaml" | grep -v "$FILE_PATH" | head -1)

python3 -c '
import sys, yaml
with open(sys.argv[1], "r") as f: data = yaml.safe_load(f)
data["tags"] = ["severity:major", "clean-tag2"]
with open(sys.argv[1], "w") as f: yaml.safe_dump(data, f, sort_keys=False)
' "$CONTROL_FILE"

# Capture control file hash
CONTROL_HASH=$(sha256sum "$CONTROL_FILE" | awk '{print $1}')

# Run the sweep script with --apply
export PATH="$(dirname "$AIDA"):$PATH"
python3 "$SWEEP_SCRIPT" --apply

# Verify the test task
if grep -q "blob1 blob2" "$FILE_PATH"; then
    echo "Error: blob still exists in yaml"
    exit 1
fi

if ! grep -q "\- blob1" "$FILE_PATH"; then
    echo "Error: blob1 not found in yaml list"
    exit 1
fi
if ! grep -q "\- blob2" "$FILE_PATH"; then
    echo "Error: blob2 not found in yaml list"
    exit 1
fi
if ! grep -q "\- clean-tag" "$FILE_PATH"; then
    echo "Error: clean-tag not found in yaml list"
    exit 1
fi

# Verify control task is byte-identical
NEW_CONTROL_HASH=$(sha256sum "$CONTROL_FILE" | awk '{print $1}')
if [ "$CONTROL_HASH" != "$NEW_CONTROL_HASH" ]; then
    echo "Error: control file was modified"
    exit 1
fi

echo "Test passed."
