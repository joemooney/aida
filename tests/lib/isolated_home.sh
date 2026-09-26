# shellcheck shell=bash
# Shared HOME isolation + real-~/.aida guard for shell tests.
#
# Tests must never touch the operator's real ~/.aida (or ~/.config, ~/.claude).
# Source this file near the top of a test script and call `aida_isolate_home`.
# It:
#   1. saves the real HOME as AIDA_TEST_REAL_HOME,
#   2. pins CARGO_HOME / RUSTUP_HOME to the real toolchain locations so cargo
#      and rustup keep working,
#   3. snapshots the real ~/.aida listing + mtimes,
#   4. exports a fresh temporary HOME (and AIDA_HOME / XDG_CONFIG_HOME, which
#      the CLI also consults) and removes it on EXIT,
#   5. unsets the ambient session/role env (AIDA_SESSION_*, AIDA_ROLE*, ...)
#      so tests don't inherit the operator's seat,
#   6. on EXIT re-snapshots the real ~/.aida and fails the script loudly if
#      anything the test could plausibly have written changed.
#
# EXIT traps: the guard owns the EXIT trap. An EXIT trap that existed before
# `aida_isolate_home`, and any `trap ... EXIT` the script sets afterwards, is
# chained (run first, in a subshell, with the script's exit status in $?)
# rather than replacing the guard: `trap` is wrapped by a shell function for
# the top-level shell. Subshells still get the plain builtin. Defining
# `aida_test_cleanup` is the other supported hook.
#
# "Plausibly written" = every path the run created under the temporary
# ~/.aida (i.e. what the CLI wrote to its home dir) plus roles/, and in every
# mode any top-level entry of ~/.aida appearing or disappearing (transient
# atomic-write names like .tmp*/*.tmp/*.lock excepted). Other live
# agent sessions on the same machine legitimately touch unrelated files
# (presence.toml, usage.jsonl, ...) mid-run, so those are ignored by default.
# Shared append-only logs (*.jsonl, *.log) are checked for existence only in
# this mode, for the same reason.
# Set AIDA_HOME_GUARD_STRICT=1 (automatic when CI=true) to compare the whole
# snapshot instead.
#
# trace:BUG-1634 | ai:claude

# Emits "<relative path>\t<mtime>" per entry below $1 (the root itself is
# excluded). Never fails: another session may remove a file between readdir and
# stat, and that must not abort a `set -euo pipefail` caller.
_aida_home_snapshot() {
    local dir="$1"
    if [ -d "$dir" ]; then
        { find "$dir" -mindepth 1 -maxdepth 2 -printf '%P\t%T@\n' 2>/dev/null || true; } | LC_ALL=C sort
    fi
    return 0
}

aida_isolate_home() {
    AIDA_TEST_REAL_HOME="${HOME:?HOME must be set}"
    export AIDA_TEST_REAL_HOME
    export CARGO_HOME="${CARGO_HOME:-$AIDA_TEST_REAL_HOME/.cargo}"
    export RUSTUP_HOME="${RUSTUP_HOME:-$AIDA_TEST_REAL_HOME/.rustup}"

    AIDA_TEST_GUARD_DIR=$(mktemp -d "${TMPDIR:-/tmp}/aida-home-guard.XXXXXX")
    _aida_home_snapshot "$AIDA_TEST_REAL_HOME/.aida" >"$AIDA_TEST_GUARD_DIR/before"

    AIDA_TEST_FAKE_HOME=$(mktemp -d "${TMPDIR:-/tmp}/aida-test-home.XXXXXX")
    export HOME="$AIDA_TEST_FAKE_HOME"
    export AIDA_HOME="$AIDA_TEST_FAKE_HOME"
    export XDG_CONFIG_HOME="$AIDA_TEST_FAKE_HOME/.config"
    # A fresh HOME has no ~/.gitconfig; give git an identity so any commit the
    # CLI makes in a scratch repo still works.
    export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-aida-test}"
    export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-aida-test@example.invalid}"
    export GIT_COMMITTER_NAME="${GIT_COMMITTER_NAME:-aida-test}"
    export GIT_COMMITTER_EMAIL="${GIT_COMMITTER_EMAIL:-aida-test@example.invalid}"

    # Don't inherit the operator's seat: an ambient AIDA_SESSION_ROLE=advisor
    # (etc.) changes gate outcomes the tests assert. Scripts that need a role
    # set it per command. AIDA_DEV_* and build paths are left alone.
    local v
    # AIDA_STORE could point the CLI at a real store; GIT_DIR /
    # GIT_CONFIG_GLOBAL would leak in when a test runs from a git hook.
    unset AIDA_SESSION_ROLE AIDA_SESSION_PURPOSE AIDA_ROLE_INSTANCE \
        AIDA_PERMISSION_MODE AIDA_SESSION_PROJECT AIDA_USER \
        AIDA_AUTO_COMPLETE AIDA_AUTO_COMPLETE_TOKEN AIDA_AGENT_OUTPUT \
        AIDA_STORE AIDA_AGENT_TYPE AIDA_AGENT_NAME AIDA_HEADLESS \
        GIT_DIR GIT_CONFIG_GLOBAL
    for v in $(compgen -e); do
        case "$v" in
            AIDA_SESSION_* | AIDA_ROLE*) unset "$v" ;;
        esac
    done

    # Chain, don't replace, a pre-existing EXIT trap.
    _AIDA_USER_EXIT_TRAP=""
    local existing
    existing=$(builtin trap -p EXIT)
    if [ -n "$existing" ]; then
        eval "set -- $existing"
        _AIDA_USER_EXIT_TRAP="$3"
    fi
    _AIDA_GUARD_SUBSHELL="$BASH_SUBSHELL"
    builtin trap 'aida_home_guard_exit' EXIT
}

# Wrapper so a later `trap '...' EXIT` in the (top-level) script is chained in
# front of the guard instead of silently replacing it.
trap() {
    if [ -z "${_AIDA_GUARD_SUBSHELL+x}" ] || [ "$BASH_SUBSHELL" != "$_AIDA_GUARD_SUBSHELL" ]; then
        builtin trap "$@"
        return
    fi
    local args=("$@")
    [ "${args[0]:-}" = "--" ] && args=("${args[@]:1}")
    case "${args[0]:-}" in
        -p | -l | "") builtin trap "$@"; return ;;
    esac
    if [ "${#args[@]}" -lt 2 ]; then
        builtin trap "$@"
        return
    fi
    local action="${args[0]}" sig rest=()
    for sig in "${args[@]:1}"; do
        case "$sig" in
            EXIT | exit | SIGEXIT | 0)
                if [ "$action" = "-" ]; then _AIDA_USER_EXIT_TRAP=""; else _AIDA_USER_EXIT_TRAP="$action"; fi
                ;;
            *) rest+=("$sig") ;;
        esac
    done
    if [ "${#rest[@]}" -gt 0 ]; then
        builtin trap -- "$action" "${rest[@]}"
    fi
    return 0
}

# Compare the real ~/.aida before/after. Prints a diff and returns 1 on change.
aida_home_guard_check() {
    local before="$AIDA_TEST_GUARD_DIR/before"
    local after="$AIDA_TEST_GUARD_DIR/after"
    _aida_home_snapshot "$AIDA_TEST_REAL_HOME/.aida" >"$after"

    local strict="${AIDA_HOME_GUARD_STRICT:-}"
    [ "${CI:-}" = "true" ] && strict=1

    local b="$before" a="$after"
    if [ -z "$strict" ]; then
        # Paths the test could plausibly have written: whatever the CLI wrote
        # under the temporary ~/.aida, plus roles/ (the BUG-1634 regression).
        local watch="$AIDA_TEST_GUARD_DIR/watch"
        {
            echo "roles"
            _aida_home_snapshot "$AIDA_TEST_FAKE_HOME/.aida" | cut -f1
        } | awk 'NF' | sort -u >"$watch"
        # Shared append-only logs (usage.jsonl, *.log) are appended to every
        # few seconds by other live sessions, so their mtime is noise here: the
        # lenient guard watches their presence, not their mtime. Strict mode
        # (CI, no concurrent sessions) still compares their mtimes.
        _aida_filter() {
            # Every top-level entry is always listed (presence only), so a new
            # or removed direct child of ~/.aida is caught in this mode too.
            awk -F'\t' 'NR==FNR { w[$0]=1; next }
                 { p=$1; top=p; sub(/\/.*/, "", top)
                   if (p !~ /\// && p !~ /^\.tmp|\.tmp$|\.lock$/) print "entry\t" p
                   if (!((p in w) || (top in w))) next
                   if (p ~ /\.(jsonl|log)$/) { print p; next }
                   print }' "$watch" "$1" | LC_ALL=C sort -u
        }
        _aida_filter "$before" >"$AIDA_TEST_GUARD_DIR/before.f"
        _aida_filter "$after" >"$AIDA_TEST_GUARD_DIR/after.f"
        b="$AIDA_TEST_GUARD_DIR/before.f"
        a="$AIDA_TEST_GUARD_DIR/after.f"
    fi

    if ! diff -u "$b" "$a" >"$AIDA_TEST_GUARD_DIR/diff"; then
        echo "FAIL real ~/.aida changed during the test run (tests must never touch it):" >&2
        sed 's/^/    /' "$AIDA_TEST_GUARD_DIR/diff" >&2
        return 1
    fi
    echo "ok   real ~/.aida unchanged ($(wc -l <"$b") watched entries)"
}

aida_home_guard_exit() {
    local rc=$?
    # Chained script EXIT trap first, in a subshell so an `exit` inside it
    # cannot skip the guard; it sees the script's exit status in $?.
    if [ -n "${_AIDA_USER_EXIT_TRAP:-}" ]; then
        (
            builtin trap - EXIT
            (exit "$rc")
            eval "$_AIDA_USER_EXIT_TRAP"
        ) || true
    fi
    # Then any script-specific cleanup registered by the caller.
    if declare -F aida_test_cleanup >/dev/null; then
        aida_test_cleanup || true
    fi
    if ! aida_home_guard_check; then
        rc=1
    fi
    rm -rf "$AIDA_TEST_FAKE_HOME" "$AIDA_TEST_GUARD_DIR"
    exit "$rc"
}
