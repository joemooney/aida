#!/usr/bin/env bash
# trace:TASK-1538 | ai:claude
# V1 token-identity proof for STORY-1488 commit A: the old and new lib.rs
# must be token-identical once every `pub ( crate )` sequence is dropped.
#   usage: check_v1_tokens.sh <itemtool-binary> <old-lib.rs> <new-lib.rs>
set -euo pipefail
TOOL="$1"; OLD="$2"; NEW="$3"
strip_pubcrate() {
  python3 - "$1" <<'EOF'
import sys
toks = open(sys.argv[1], encoding="utf-8").read().splitlines()
out, i = [], 0
while i < len(toks):
    if (i + 3 < len(toks) and toks[i] == "pub" and toks[i+1] == "("
            and toks[i+2] == "crate" and toks[i+3] == ")"):
        i += 4
        continue
    out.append(toks[i]); i += 1
sys.stdout.write("\n".join(out) + "\n")
EOF
}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
"$TOOL" tokens "$OLD" > "$T/old.raw"
"$TOOL" tokens "$NEW" > "$T/new.raw"
strip_pubcrate "$T/old.raw" > "$T/old.tok"
strip_pubcrate "$T/new.raw" > "$T/new.tok"
if cmp -s "$T/old.tok" "$T/new.tok"; then
  echo "V1 OK: token-identical modulo pub(crate) ($(wc -l < "$T/old.tok") tokens)"
else
  echo "V1 FAIL: token streams differ:"
  diff "$T/old.tok" "$T/new.tok" | head -40
  exit 1
fi
