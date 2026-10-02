#!/bin/bash
# Focused JSON parsing coverage for the git-guardrails hook. trace:BUG-1751
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK="$ROOT/aida-core/templates/hooks/aida-git-guardrails.sh"
TMP=$(mktemp -d)
fail=0
REAL_GREP=$(command -v grep)
mkdir -p "$TMP/no-pcre"
cat >"$TMP/no-pcre/grep" <<EOF
#!/bin/sh
for arg do case "\$arg" in --perl-regexp|-*P*) echo 'PCRE unavailable' >&2; exit 2;; esac; done
exec "$REAL_GREP" "\$@"
EOF
chmod +x "$TMP/no-pcre/grep"
run() { printf '%s' "$1" | PATH="$TMP/no-pcre:$PATH" "$HOOK" >/dev/null 2>&1; echo $?; }
assert_exit() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then echo "ok   - $desc (exit $actual)"
    else echo "FAIL - $desc (expected $expected, got $actual)"; fail=1; fi
}
assert_exit "PCRE unavailable: destructive reset is blocked" 2 "$(run '{"tool_name":"Bash","tool_input":{"command":"git reset --hard"}}')"
assert_exit "PCRE unavailable: harmless status is allowed" 0 "$(run '{"tool_name":"Bash","tool_input":{"command":"git status --short"}}')"
assert_exit "escaped quote does not truncate destructive command" 2 "$(run '{"tool_name":"Bash","tool_input":{"command":"echo \\\"quoted\\\"; git reset --hard"}}')"
assert_exit "non-Bash payload without command is allowed" 0 "$(run '{"tool_name":"Write","tool_input":{"file_path":"x.rs"}}')"
rm -rf "$TMP"
[ "$fail" -eq 0 ] || exit 1
echo "GIT GUARDRAILS PARSE: all checks passed"
