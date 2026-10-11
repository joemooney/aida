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

# Prose that merely MENTIONS a destructive git operation — in a heredoc body, a
# quoted argument spanning lines, or a comment — is not a git invocation and
# must be allowed; real invocations still block, with the reason on STDERR.
# trace:BUG-1936 | ai:claude
run_stderr() { printf '%s' "$1" | PATH="$TMP/no-pcre:$PATH" "$HOOK" 2>&1 >/dev/null; }
assert_stderr() {
    local desc="$1" needle="$2" stderr="$3"
    if [ -n "$stderr" ] && printf '%s' "$stderr" | grep -qF -- "$needle"; then echo "ok   - $desc (stderr has '$needle')"
    else echo "FAIL - $desc (stderr missing '$needle'; got: ${stderr:-<empty>})"; fail=1; fi
}
assert_exit "heredoc body mentioning a force-push to main is allowed" 0 "$(run '{"tool_input":{"command":"cat > /tmp/x.md <<EOF\nnever git push --force origin main\nEOF"}}')"
assert_exit "quoted <<-'EOF' heredoc body with reset --hard is allowed" 0 "$(run '{"tool_input":{"command":"cat > /tmp/x.md <<-'"'"'EOF'"'"'\n\tdo not git reset --hard\n\tEOF\necho done"}}')"
assert_exit "double-quoted arg spanning lines mentioning a force-push is allowed" 0 "$(run '{"tool_input":{"command":"aida add --description \"line1\ngit push --force origin main\nline3\""}}')"
assert_exit "single-line quoted description mentioning a force push is allowed" 0 "$(run '{"tool_input":{"command":"aida add --description \"mentions a force push: git push --force origin main\""}}')"
assert_exit "comment mentioning a force-push is allowed" 0 "$(run '{"tool_input":{"command":"ls # don'"'"'t git push --force origin main"}}')"
assert_exit "arithmetic shift is not a heredoc" 0 "$(run '{"tool_input":{"command":"echo $((1<<2))"}}')"
assert_exit "bare force-push to main is blocked" 2 "$(run '{"tool_input":{"command":"git push --force origin main"}}')"
assert_stderr "bare force-push to main states its reason on stderr" "BLOCKED: force-push to a protected branch" "$(run_stderr '{"tool_input":{"command":"git push --force origin main"}}')"
assert_stderr "reset --hard states its reason on stderr" "BLOCKED: 'git reset --hard'" "$(run_stderr '{"tool_input":{"command":"git reset --hard"}}')"
assert_exit "real force-push after a heredoc is still blocked" 2 "$(run '{"tool_input":{"command":"cat > /tmp/x.md <<EOF\nnotes\nEOF\ngit push --force origin main"}}')"
assert_exit "force-push split by a line continuation is blocked" 2 "$(run '{"tool_input":{"command":"git push \\\n  --force origin main"}}')"
assert_exit "unterminated quote fails closed" 2 "$(run '{"tool_input":{"command":"git push origin main \"oops"}}')"
assert_stderr "unterminated quote states its reason on stderr" "BLOCKED: could not parse" "$(run_stderr '{"tool_input":{"command":"git push origin main \"oops"}}')"
assert_exit "unterminated heredoc fails closed" 2 "$(run '{"tool_input":{"command":"cat <<EOF\ngit push --force origin main"}}')"
assert_stderr "unterminated heredoc states its reason on stderr" "BLOCKED: could not parse" "$(run_stderr '{"tool_input":{"command":"cat <<EOF\nbody"}}')"
rm -rf "$TMP"
[ "$fail" -eq 0 ] || exit 1
echo "GIT GUARDRAILS PARSE: all checks passed"
