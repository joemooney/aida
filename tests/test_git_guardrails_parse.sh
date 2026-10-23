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
assert_stderr "unterminated heredoc states its reason on stderr" "BLOCKED: could not parse" "$(run_stderr '{"tool_input":{"command":"cat <<EOF\ngit push --force origin main"}}')"
# Fail-closed applies ONLY to git-bearing commands: an unparseable non-git
# command must never be blocked by the tokenizer. trace:BUG-1936 | ai:claude
assert_exit "unterminated quote in a non-git command is allowed" 0 "$(run '{"tool_input":{"command":"echo \"unterminated"}}')"
assert_exit "unterminated non-git heredoc is allowed" 0 "$(run '{"tool_input":{"command":"cat <<EOF\nhello"}}')"
assert_exit "unterminated quote in python3 -c is allowed" 0 "$(run '{"tool_input":{"command":"python3 -c \"print(1)"}}')"
assert_exit "unterminated quote with rm -rf .git fails closed" 2 "$(run '{"tool_input":{"command":"rm -rf .git \"oops"}}')"

# Agents may not bypass the cargo-slot limiter; mentions and unsets are fine.
# trace:STORY-1853 | ai:claude
assert_exit "AIDA_CARGO_NO_SLOT=1 prefix is blocked" 2 "$(run '{"tool_input":{"command":"AIDA_CARGO_NO_SLOT=1 cargo test --workspace"}}')"
assert_exit "exported AIDA_CARGO_NO_SLOT is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_SLOT=1 && make check"}}')"
assert_exit "env AIDA_CARGO_NO_SLOT=\"1\" is blocked" 2 "$(run '{"tool_input":{"command":"cd x && env AIDA_CARGO_NO_SLOT=\"1\" cargo build"}}')"
assert_stderr "the NO_SLOT block names the limiter" "cargo build-slot limiter" "$(run_stderr '{"tool_input":{"command":"AIDA_CARGO_NO_SLOT=1 cargo test"}}')"
assert_exit "unsetting AIDA_CARGO_NO_SLOT is allowed" 0 "$(run '{"tool_input":{"command":"env -u AIDA_CARGO_NO_SLOT cargo test -p aida-core x; unset AIDA_CARGO_NO_SLOT"}}')"
assert_exit "bash -c wrapping AIDA_CARGO_NO_SLOT=1 is blocked" 2 "$(run '{"tool_input":{"command":"timeout 3000 bash -c \"AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "sh -c wrapping AIDA_CARGO_NO_SLOT=1 is blocked" 2 "$(run '{"tool_input":{"command":"sh -c '"'"'AIDA_CARGO_NO_SLOT=1 cargo test'"'"'"}}')"
assert_exit "AIDA_CARGO_NO_SLOT+=1 is blocked" 2 "$(run '{"tool_input":{"command":"AIDA_CARGO_NO_SLOT+=1; export AIDA_CARGO_NO_SLOT; make check"}}')"
assert_exit "a bare export AIDA_CARGO_NO_SLOT=1 is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_SLOT=1"}}')"
assert_exit "env -u AIDA_CARGO_NO_SLOT alone is allowed" 0 "$(run '{"tool_input":{"command":"env -u AIDA_CARGO_NO_SLOT make check-ci-fast"}}')"
# TASK-1643: quoted text nothing executes is prose; a payload a shell runs is not.
# trace:TASK-1952 | trace:TASK-1643 | ai:claude
assert_exit "a mailbox heredoc spelling out AIDA_CARGO_NO_SLOT=1 is allowed" 0 "$(run '{"tool_input":{"command":"aida mailbox send --to advisor --stdin <<EOF\nfix-2725 ran with AIDA_CARGO_NO_SLOT=1\nEOF"}}')"
assert_exit "an rg search for the assignment is allowed" 0 "$(run '{"tool_input":{"command":"rg -n '"'"'AIDA_CARGO_NO_SLOT='"'"' aida-core"}}')"
assert_exit "a commit message quoting it is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"docs: never set AIDA_CARGO_NO_SLOT=1\""}}')"
assert_exit "an aida comment quoting it is allowed" 0 "$(run '{"tool_input":{"command":"aida comment add TASK-1 \"ran with AIDA_CARGO_NO_SLOT=1 by mistake\""}}')"
assert_exit "a shell heredoc payload is blocked" 2 "$(run '{"tool_input":{"command":"bash <<EOF\nAIDA_CARGO_NO_SLOT=1 make check\nEOF"}}')"
assert_exit "a payload piped into sh is blocked" 2 "$(run '{"tool_input":{"command":"echo '"'"'AIDA_CARGO_NO_SLOT=1 make check'"'"' | sh"}}')"
assert_exit "an eval payload is blocked" 2 "$(run '{"tool_input":{"command":"eval \"AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "a command substitution in a quoted arg is blocked" 2 "$(run '{"tool_input":{"command":"echo \"$(AIDA_CARGO_NO_SLOT=1 make check)\""}}')"
assert_exit "an absolute-path shell payload is blocked" 2 "$(run '{"tool_input":{"command":"/bin/bash -c '"'"'AIDA_CARGO_NO_SLOT=1 cargo test'"'"'"}}')"
assert_exit "an env-prefixed shell payload is blocked" 2 "$(run '{"tool_input":{"command":"/usr/bin/env bash -c \"AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "a payload piped into /bin/sh is blocked" 2 "$(run '{"tool_input":{"command":"echo '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' | /bin/sh"}}')"
assert_exit "a shell named in quoted prose is allowed" 0 "$(run '{"tool_input":{"command":"aida comment add TASK-1 \"never run bash -c with AIDA_CARGO_NO_SLOT=1\""}}')"
assert_exit "a heredoc body line ending in a shell name is allowed" 0 "$(run '{"tool_input":{"command":"aida mailbox send --to advisor --stdin <<EOF\nAIDA_CARGO_NO_SLOT=1 was set by a script run with bash\nEOF"}}')"
assert_exit "a -c payload with ; before the assignment is blocked" 2 "$(run '{"tool_input":{"command":"bash -c \"echo; AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "a -c payload with && before the assignment is blocked" 2 "$(run '{"tool_input":{"command":"bash -c '"'"'cd x && AIDA_CARGO_NO_SLOT=1 make check'"'"'"}}')"
assert_exit "an eval payload with | before the assignment is blocked" 2 "$(run '{"tool_input":{"command":"eval \"true | AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "env with a quoted assignment word is blocked" 2 "$(run '{"tool_input":{"command":"env \"AIDA_CARGO_NO_SLOT=1\" make check"}}')"
assert_exit "export with a quoted assignment word is blocked" 2 "$(run '{"tool_input":{"command":"export '"'"'AIDA_CARGO_NO_SLOT=1'"'"'"}}')"
assert_exit "declare -x with a quoted assignment word is blocked" 2 "$(run '{"tool_input":{"command":"declare -x \"AIDA_CARGO_NO_SLOT=1\"; make check"}}')"
assert_exit "a process substitution is blocked" 2 "$(run '{"tool_input":{"command":"source <(echo \"AIDA_CARGO_NO_SLOT=1\")"}}')"
assert_exit "a quoted export in a commit message is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"docs: never export AIDA_CARGO_NO_SLOT=1\""}}')"
assert_exit "a similarly named variable is allowed" 0 "$(run '{"tool_input":{"command":"MY_AIDA_CARGO_NO_SLOT=1 true"}}')"
# TASK-2139: make reads a quoted VAR=1 argument as a variable it exports to
# recipes; fish runs a -c payload; env -S / --split-string runs its string;
# a file written then sourced runs its body.
# trace:TASK-1952 | trace:TASK-2139 | ai:claude
assert_exit "make with a quoted assignment argument is blocked" 2 "$(run '{"tool_input":{"command":"make check '"'"'AIDA_CARGO_NO_SLOT=1'"'"'"}}')"
assert_exit "gmake with a quoted assignment argument is blocked" 2 "$(run '{"tool_input":{"command":"gmake \"AIDA_CARGO_NO_SLOT=1\" check"}}')"
assert_exit "a fish -c payload is blocked" 2 "$(run '{"tool_input":{"command":"fish -c '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"'"}}')"
assert_exit "env --split-string= is blocked" 2 "$(run '{"tool_input":{"command":"env --split-string='"'"'AIDA_CARGO_NO_SLOT=1 make'"'"'"}}')"
assert_exit "env -S is blocked" 2 "$(run '{"tool_input":{"command":"env -S \"AIDA_CARGO_NO_SLOT=1 make check\""}}')"
assert_exit "write then source is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh && source x.sh"}}')"
assert_exit "write then dot-source is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; . ./x.sh"}}')"
assert_exit "a path containing sh- is not a shell" 0 "$(run '{"tool_input":{"command":"ls scripts/sh-helper; rg '"'"'AIDA_CARGO_NO_SLOT=1'"'"'"}}')"
assert_exit "a dot argument is not a dot-source" 0 "$(run '{"tool_input":{"command":"ls . && rg '"'"'AIDA_CARGO_NO_SLOT=1'"'"' ."}}')"
assert_exit "make named in a commit message is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"docs: never make check AIDA_CARGO_NO_SLOT=1\""}}')"
# TASK-2140: a quoted name with an unquoted = is still one assignment word
# after quote removal.
# trace:TASK-1952 | trace:TASK-2140 | ai:claude
assert_exit "export with a quoted name is blocked" 2 "$(run '{"tool_input":{"command":"export \"AIDA_CARGO_NO_SLOT\"=1"}}')"
assert_exit "env with a single-quoted name is blocked" 2 "$(run '{"tool_input":{"command":"env '"'"'AIDA_CARGO_NO_SLOT'"'"'=1 make check"}}')"
assert_exit "declare -x with a quoted name is blocked" 2 "$(run '{"tool_input":{"command":"declare -x \"AIDA_CARGO_NO_SLOT\"=1"}}')"
assert_exit "export with an empty quote after the name is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_SLOT\"\"=1"}}')"
assert_exit "make with a quoted name is blocked" 2 "$(run '{"tool_input":{"command":"make \"AIDA_CARGO_NO_SLOT\"=1 check"}}')"
# PR-2837 review: a quote inside the name is joined too (TASK-2140), and a
# sourced or executed file after then/do/else/command/builtin/! (TASK-2139).
# trace:TASK-1952 | trace:TASK-2139 | trace:TASK-2140 | ai:claude
assert_exit "a quoted tail of the name is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_\"SLOT\"=1"}}')"
assert_exit "a quoted head of the name is blocked" 2 "$(run '{"tool_input":{"command":"export \"AIDA_CARGO\"_NO_SLOT=1"}}')"
assert_exit "a single-quoted middle of the name is blocked" 2 "$(run '{"tool_input":{"command":"env AIDA_'"'"'CARGO_NO'"'"'_SLOT=1 make check"}}')"
assert_exit "a dot-source after then is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; if true; then . ./x.sh; fi"}}')"
assert_exit "a source after do is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; for f in x.sh; do source $f; done"}}')"
assert_exit "a dot-source after else is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; if false; then true; else . ./x.sh; fi"}}')"
assert_exit "command dot-source is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; command . ./x.sh"}}')"
assert_exit "builtin source is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; builtin source x.sh"}}')"
assert_exit "a negated source is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; ! source x.sh"}}')"
assert_exit "write then execute is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; ./x.sh"}}')"
assert_exit "a commit message naming source is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"docs: never source a file that sets AIDA_CARGO_NO_SLOT=1\""}}')"
assert_exit "a ./script without the variable is allowed" 0 "$(run '{"tool_input":{"command":"chmod +x x.sh; ./x.sh"}}')"
# TASK-2246: a time prefix before the dot-source, env fed through xargs, and
# setters with no `=` (printf -v, read, a bare export).
# trace:TASK-1952 | trace:TASK-2246 | ai:claude
assert_exit "a dot-source after time is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; time . ./x.sh"}}')"
assert_exit "a source after time -p is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; time -p source x.sh"}}')"
assert_exit "env fed through xargs is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; xargs -a x.sh env"}}')"
assert_exit "printf -v of the name is blocked" 2 "$(run '{"tool_input":{"command":"printf -v AIDA_CARGO_NO_SLOT 1; make check"}}')"
assert_exit "read of the name is blocked" 2 "$(run '{"tool_input":{"command":"read AIDA_CARGO_NO_SLOT <<< 1; make check"}}')"
assert_exit "a bare export of the name is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_SLOT"}}')"
assert_exit "export of another name is allowed" 0 "$(run '{"tool_input":{"command":"export FOO; time make check"}}')"
assert_exit "unset of the name is allowed" 0 "$(run '{"tool_input":{"command":"unset AIDA_CARGO_NO_SLOT; make check"}}')"
assert_exit "a search for the name is allowed" 0 "$(run '{"tool_input":{"command":"rg -n AIDA_CARGO_NO_SLOT aida-core; git ls-files | xargs wc -l"}}')"
# PR-2837 review round 4: quoted names after a bare setter, write-then-run
# through exec prefixes, and reads or prose that only name the variable.
# trace:TASK-1952 | trace:TASK-2246 | ai:claude
assert_exit "a bare export of a double-quoted name is blocked" 2 "$(run '{"tool_input":{"command":"export \"AIDA_CARGO_NO_SLOT\""}}')"
assert_exit "a bare export of a single-quoted name is blocked" 2 "$(run '{"tool_input":{"command":"export '"'"'AIDA_CARGO_NO_SLOT'"'"'"}}')"
assert_exit "a bare export of a partly quoted name is blocked" 2 "$(run '{"tool_input":{"command":"export AIDA_CARGO_NO_\"SLOT\""}}')"
assert_exit "printf -v with a quoted name is blocked" 2 "$(run '{"tool_input":{"command":"printf -v \"AIDA_CARGO_NO_SLOT\" 1; make check"}}')"
assert_exit "write then run under nohup is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; nohup ./x.sh"}}')"
assert_exit "write then run under nice -n is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; nice -n 5 ./x.sh"}}')"
assert_exit "write then run under timeout is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; timeout 10 ./x.sh"}}')"
assert_exit "write then run under setsid is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; setsid ./x.sh"}}')"
assert_exit "write then run under stdbuf is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; stdbuf -oL ./x.sh"}}')"
assert_exit "write then run under env -i is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; env -i ./x.sh"}}')"
assert_exit "declare -p of the name is allowed" 0 "$(run '{"tool_input":{"command":"declare -p AIDA_CARGO_NO_SLOT; make check"}}')"
assert_exit "printf of the name is allowed" 0 "$(run '{"tool_input":{"command":"printf '"'"'%s\\n'"'"' AIDA_CARGO_NO_SLOT"}}')"
assert_exit "a commit message naming a bare export is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"docs: never export AIDA_CARGO_NO_SLOT; queue for a slot\""}}')"
assert_exit "a commit message naming printf -v is allowed" 0 "$(run '{"tool_input":{"command":"git commit -m \"guard: block printf -v AIDA_CARGO_NO_SLOT\""}}')"
assert_exit "nohup without the variable is allowed" 0 "$(run '{"tool_input":{"command":"nohup make check > log 2>&1"}}')"
# PR-2837 review round 5: every bare-setter form blocked at 893de128 stays
# blocked (export --, declare -x --, export -p NAME, a quoted ; or | in an
# earlier word), setters inside shell/eval payloads, and more exec prefixes.
# trace:TASK-1952 | trace:TASK-2246 | ai:claude
assert_exit "export -- of the name is blocked" 2 "$(run '{"tool_input":{"command":"export -- AIDA_CARGO_NO_SLOT; make check"}}')"
assert_exit "declare -x -- of the name is blocked" 2 "$(run '{"tool_input":{"command":"declare -x -- AIDA_CARGO_NO_SLOT"}}')"
assert_exit "export -p of the name is blocked" 2 "$(run '{"tool_input":{"command":"export -p AIDA_CARGO_NO_SLOT"}}')"
assert_exit "declare -px of the name is blocked" 2 "$(run '{"tool_input":{"command":"declare -px AIDA_CARGO_NO_SLOT"}}')"
assert_exit "declare -g of the name is blocked" 2 "$(run '{"tool_input":{"command":"declare -g AIDA_CARGO_NO_SLOT"}}')"
assert_exit "a quoted ; in an earlier export word is blocked" 2 "$(run '{"tool_input":{"command":"export FOO=\"x;y\" AIDA_CARGO_NO_SLOT; make"}}')"
assert_exit "a quoted | in an earlier export word is blocked" 2 "$(run '{"tool_input":{"command":"export FOO='"'"'a|b'"'"' AIDA_CARGO_NO_SLOT; make"}}')"
assert_exit "readonly of the name is blocked" 2 "$(run '{"tool_input":{"command":"readonly AIDA_CARGO_NO_SLOT"}}')"
assert_exit "local of the name is blocked" 2 "$(run '{"tool_input":{"command":"f() { local AIDA_CARGO_NO_SLOT; }"}}')"
assert_exit "readarray of the name is blocked" 2 "$(run '{"tool_input":{"command":"readarray AIDA_CARGO_NO_SLOT < x"}}')"
assert_exit "mapfile of the name is blocked" 2 "$(run '{"tool_input":{"command":"mapfile -t AIDA_CARGO_NO_SLOT < x"}}')"
assert_exit "read -r of the name is blocked" 2 "$(run '{"tool_input":{"command":"read -r AIDA_CARGO_NO_SLOT < x"}}')"
assert_exit "a bare export inside bash -c is blocked" 2 "$(run '{"tool_input":{"command":"bash -c '"'"'export AIDA_CARGO_NO_SLOT; make'"'"'"}}')"
assert_exit "a bare export inside sh -c is blocked" 2 "$(run '{"tool_input":{"command":"sh -c \"export AIDA_CARGO_NO_SLOT\""}}')"
assert_exit "a bare export inside eval is blocked" 2 "$(run '{"tool_input":{"command":"eval '"'"'export AIDA_CARGO_NO_SLOT'"'"'"}}')"
assert_exit "write then exec is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; exec ./x.sh"}}')"
assert_exit "write then run under env -u is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; env -u FOO ./x.sh"}}')"
assert_exit "write then run under nohup env with an assignment is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; nohup env FOO=1 ./x.sh"}}')"
assert_exit "write then run under timeout -s is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; timeout -s KILL 5 ./x.sh"}}')"
assert_exit "write then run under ionice is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; ionice -c3 ./x.sh"}}')"
assert_exit "write then run under taskset is blocked" 2 "$(run '{"tool_input":{"command":"printf '"'"'AIDA_CARGO_NO_SLOT=1 make'"'"' > x.sh; chmod +x x.sh; taskset -c 0 ./x.sh"}}')"
assert_exit "typeset -p of the name is allowed" 0 "$(run '{"tool_input":{"command":"typeset -p AIDA_CARGO_NO_SLOT"}}')"
assert_exit "export -p alone is allowed" 0 "$(run '{"tool_input":{"command":"export -p | grep CARGO"}}')"
rm -rf "$TMP"
[ "$fail" -eq 0 ] || exit 1
echo "GIT GUARDRAILS PARSE: all checks passed"
