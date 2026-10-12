#!/bin/bash
# BUG-1933: the pre-commit `///`-provenance gate fails CLOSED when its awk scan
# breaks. An awk that exits without output, or whose output is cut short before
# the `#AIDA-PROV-END` sentinel, must refuse the commit, not read as "no
# offenders". The healthy scan still allows a clean commit and refuses debt.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in a throwaway repo, via real `git commit`.
# Only the provenance scan's awk (the one given `-v spec_re=...`) is shadowed;
# every other awk in the hook runs the real binary.
#
# trace:BUG-1933 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"
REAL_AWK=$(command -v awk)

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
STDERR_LOG="$FAKEHOME/stderr.txt"
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

# A no-op `cargo` keeps the hook's rustfmt step out of the way.
SHIMS="$FAKEHOME/shims"
mkdir -p "$SHIMS"
printf '#!/bin/sh\nexit 0\n' >"$SHIMS/cargo"
chmod +x "$SHIMS/cargo"

# awk shim: AWK_MODE=crash exits 2 with no output from the provenance scan;
# AWK_MODE=truncate runs the real scan and drops its last line (the sentinel).
cat >"$SHIMS/awk" <<EOF
#!/bin/bash
for a in "\$@"; do
    case "\$a" in
        spec_re=*)
            case "\${AWK_MODE:-}" in
                crash) cat >/dev/null; exit 2 ;;
                truncate) "$REAL_AWK" "\$@" | sed '\$d'; exit 0 ;;
            esac
            ;;
    esac
done
exec "$REAL_AWK" "\$@"
EOF
chmod +x "$SHIMS/awk"

cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
mkdir -p .git/hooks src
cp "$HOOK_TEMPLATE" .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit
echo 'pub fn seed() {}' >src/seed.rs
git add -A
git commit --no-verify -qm "chore: seed"

# commit <awk-mode> <msg> — a real hermetic `git commit`; echoes the exit code.
commit() {
    set +e
    env -i PATH="$SHIMS:$PATH" HOME="$FAKEHOME" AWK_MODE="$1" \
        git -c user.email=t@t.t -c user.name=t commit -qm "$2" \
        >/dev/null 2>"$STDERR_LOG"
    local rc=$?
    set -e
    echo "$rc"
}

assert_exit() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then
        echo "ok   - $desc (exit $actual)"
    else
        echo "FAIL - $desc (expected $expected, got $actual)"
        [ -s "$STDERR_LOG" ] && head -n 20 "$STDERR_LOG" | sed 's/^/       | /'
        fail=1
    fi
}

assert_stderr_contains() {
    local desc="$1" needle="$2"
    if grep -q -- "$needle" "$STDERR_LOG"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (stderr lacks '$needle')"
        fail=1
    fi
}

# A clean staged change: no provenance debt anywhere.
echo 'pub fn clean() {}' >src/clean.rs
git add -A
base=$(git rev-parse HEAD)

# Each case starts from the same staged change: undo a commit a regressed hook
# let through, so the next case still has something to commit.
restore() {
    git reset -q --soft "$base"
}

assert_exit "a crashed provenance awk refuses the commit" 1 "$(commit crash "feat: clean")"
assert_stderr_contains "the refusal says the scan did not complete" "provenance scan did not complete"
restore
assert_exit "a provenance scan cut short before its sentinel refuses the commit" 1 \
    "$(commit truncate "feat: clean")"
assert_stderr_contains "the truncated scan is reported as incomplete" "provenance scan did not complete"
restore

# The healthy scan: debt is still refused, and the clean change goes through.
cat >src/debt.rs <<'EOF'
/// trace:BUG-4243 | ai:claude
pub fn debt() {}
EOF
git add -A
assert_exit "the healthy scan still refuses new debt" 1 "$(commit "" "feat: debt")"
assert_stderr_contains "the refusal names the debt line" "BUG-4243"
git rm -q --cached src/debt.rs
rm src/debt.rs
assert_exit "the healthy scan allows a clean commit" 0 "$(commit "" "feat: clean")"

if [ "$fail" -ne 0 ]; then
    echo "PRE-COMMIT PROVENANCE AWK FAILURE (BUG-1933): FAILURES"
    exit 1
fi
echo "PRE-COMMIT PROVENANCE AWK FAILURE (BUG-1933): all checks passed"
