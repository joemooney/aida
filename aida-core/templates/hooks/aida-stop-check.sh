#!/bin/bash
# AIDA Stop Check Hook
# Warns about implementation files modified without trace comments
# Hook type: Stop (runs when Claude Code session ends)

modified=$(git diff --name-only HEAD 2>/dev/null | grep -E '\.(rs|py|ts|js|tsx|jsx)$' || true)
untraced=""

for f in $modified; do
    if [ -f "$f" ] && ! grep -q "trace:" "$f" 2>/dev/null; then
        untraced="$untraced\n  - $f"
    fi
done

if [ -n "$untraced" ]; then
    echo "⚠ AIDA: Modified files without trace comments:" >&2
    echo -e "$untraced" >&2
    echo "" >&2
    echo "Consider running /aida-capture to link changes to requirements." >&2
fi

exit 0
