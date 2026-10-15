#!/bin/bash
# TASK-144: the pre-commit `///`-provenance gate must not false-positive on
# PURE MOVES. The gate (TASK-135/BUG-624/BUG-629/TASK-903) scans added diff
# lines only, so a mechanical file-split used to re-add pre-existing
# `/// trace:` debt and refuse the commit (~85 false-flagged lines during the
# STORY-771 extraction) even though BUG-624's design note says debt a commit
# did not introduce must not block it. The fix credits `-`-removed provenance
# lines in the SAME staged diff against added candidates (trim-insensitive,
# counted), so a move is excused while genuinely NEW debt is still refused.
#
# Drives the REAL template (aida-core/templates/hooks/aida-pre-commit.sh)
# installed as .git/hooks/pre-commit in a throwaway repo, via real `git commit`.
#
# trace:TASK-144 | ai:claude
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_TEMPLATE="$PROJECT_ROOT/aida-core/templates/hooks/aida-pre-commit.sh"

TMP=$(mktemp -d)
FAKEHOME=$(mktemp -d)
STDERR_LOG="$FAKEHOME/stderr.txt"
STDOUT_LOG="$FAKEHOME/stdout.txt"
trap 'rm -rf "$TMP" "$FAKEHOME"' EXIT
fail=0

cd "$TMP"
git init -q -b main
git config user.email t@t.t
git config user.name t
mkdir -p .git/hooks src
cp "$HOOK_TEMPLATE" .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

# commit <msg> — run a real hermetic `git commit` (FAKEHOME so no user config /
# aida state leaks in; no AIDA_SESSION_ROLE so the advisor-code-gate, if an
# aida binary is even resolvable, allows). Echoes the exit code; stdout and
# stderr are captured OUTSIDE the repo so `git add -A` never stages them.
# HOOK_PATH, when set, replaces PATH for the commit so a test can put a fake
# `cargo` in front of the real one (BUG-1661).
commit() {
    set +e
    env -i PATH="${HOOK_PATH:-$PATH}" HOME="$FAKEHOME" \
        git -c user.email=t@t.t -c user.name=t commit -qm "$1" \
        >"$STDOUT_LOG" 2>"$STDERR_LOG"
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
        [ -s "$STDERR_LOG" ] && sed 's/^/       | /' "$STDERR_LOG"
        fail=1
    fi
}

assert_stderr_contains() {
    local desc="$1" needle="$2"
    if grep -q "$needle" "$STDERR_LOG"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (stderr lacks '$needle')"
        fail=1
    fi
}

assert_stderr_lacks() {
    local desc="$1" needle="$2"
    if grep -q "$needle" "$STDERR_LOG"; then
        echo "FAIL - $desc (stderr unexpectedly contains '$needle')"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# The hook prints its fmt progress on stdout and its failures on stderr; these
# two look at BOTH streams so a claim cannot hide on either. trace:BUG-1661
assert_output_contains() {
    local desc="$1" needle="$2"
    if cat "$STDOUT_LOG" "$STDERR_LOG" | grep -q "$needle"; then
        echo "ok   - $desc"
    else
        echo "FAIL - $desc (neither stdout nor stderr has '$needle')"
        sed 's/^/       | /' "$STDOUT_LOG" "$STDERR_LOG"
        fail=1
    fi
}

assert_output_lacks() {
    local desc="$1" needle="$2"
    if cat "$STDOUT_LOG" "$STDERR_LOG" | grep -q "$needle"; then
        echo "FAIL - $desc (output unexpectedly contains '$needle')"
        sed 's/^/       | /' "$STDOUT_LOG" "$STDERR_LOG"
        fail=1
    else
        echo "ok   - $desc"
    fi
}

# Seed: a file already carrying `///` provenance debt (committed with
# --no-verify, exactly how such debt predates the gate).
cat >src/lib.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn beta() {}
pub fn gamma() {}
EOF
git add src/lib.rs
git commit --no-verify -qm "chore: seed pre-existing debt"

# 1. PURE MOVE (the STORY-771 false-positive shape): both debt lines leave
#    lib.rs and reappear verbatim in a new split.rs, same staged diff → ALLOWED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn beta() {}
EOF
git add -A
assert_exit "pure file-split move of /// trace debt is allowed" 0 "$(commit "refactor: split")"

# 2. GENUINELY NEW debt (no matching staged removal) → still REFUSED.
cat >>src/split.rs <<'EOF'
/// trace:BUG-999 | ai:claude
pub fn delta() {}
EOF
git add -A
assert_exit "genuinely new /// trace debt is refused" 1 "$(commit "feat: new debt")"
assert_stderr_contains "refusal names the new debt line" "BUG-999"
git reset -q --hard

# 3. MOVE + NEW debt in one commit → REFUSED, but ONLY the new line is flagged;
#    the moved line is excused.
cat >src/lib.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
/// trace:BUG-777 | ai:claude
pub fn epsilon() {}
pub fn gamma() {}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-102 | ai:claude
pub fn beta() {}
EOF
git add -A
assert_exit "move mixed with new debt is refused" 1 "$(commit "feat: mixed")"
assert_stderr_contains "refusal flags the new line" "BUG-777"
assert_stderr_lacks "refusal does NOT flag the moved line" "TASK-101"
git reset -q --hard

# 4. RE-INDENTED move: the line moves into an indented context (indentation
#    changes, content identical after trimming) → still a move → ALLOWED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
pub mod inner {
    /// trace:TASK-102 | ai:claude
    pub fn beta() {}
}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-101 | ai:claude
pub fn alpha() {}
EOF
git add -A
assert_exit "re-indented move is still recognized as a move" 0 "$(commit "refactor: reindent move")"

# 5. CREDIT CONSUMPTION: one removal cannot excuse TWO identical added copies —
#    the duplicate beyond the removal count is NEW debt → REFUSED.
cat >src/lib.rs <<'EOF'
pub fn gamma() {}
pub mod inner {
    pub fn beta() {}
}
EOF
cat >src/split.rs <<'EOF'
/// trace:TASK-102 | ai:claude
pub fn alpha() {}
/// trace:TASK-102 | ai:claude
pub fn zeta() {}
EOF
git add -A
assert_exit "duplicating a moved line beyond its removal count is refused" 1 "$(commit "feat: dup")"
git reset -q --hard

# 6. PROSE MENTIONS in ordinary rustdoc (TASK-1516): the hook ships to every
#    scaffolded project and scans every staged *.rs file, so a descriptive
#    mention of a project's own id on a `///` line in a non-CLI file must NOT
#    block, and an id-shaped substring of a longer word (DEBUG-2 -> BUG-2,
#    SCR-4 -> CR-4) is not an id at all (leading word boundary).
cat >src/prose.rs <<'EOF'
/// See ADR-12 for the rationale.
pub fn adr() {}
/// Implements FR-1-042 from the product requirements.
pub fn fr() {}
/// Log at DEBUG-2 verbosity.
pub fn debug() {}
/// Handles the SCR-4 screen.
pub fn scr() {}
/// DEBUG-2
pub fn bare_debug() {}
/// SCR-4
pub fn bare_scr() {}
EOF
git add -A
assert_exit "prose id mentions on /// in a non-CLI .rs file are allowed" 0 "$(commit "docs: prose ids")"

# 7. A bare `trace:` marker on a `///` line is still provenance -> REFUSED.
cat >>src/prose.rs <<'EOF'
/// trace:BUG-1 | ai:x
pub fn traced() {}
EOF
git add -A
assert_exit "bare /// trace marker is still refused" 1 "$(commit "feat: trace on doc")"
assert_stderr_contains "refusal names the trace line" "trace:BUG-1"
git reset -q --hard

# 8. A bare id (no prose) on a `///` line is still refused, DOC prefix included.
cat >>src/prose.rs <<'EOF'
/// DOC-3
pub fn doc_bare() {}
EOF
git add -A
assert_exit "bare /// DOC id is still refused" 1 "$(commit "feat: bare doc id")"
git reset -q --hard

# 8b. BUG-2008: A wrapped prose sentence whose bare id lands alone on the next line
#     continues the prose sentence from the previous /// line and is ALLOWED.
#     trace:BUG-2008 | ai:antigravity
cat >>src/prose.rs <<'EOF'
/// Reuses the usage log introduced by
/// STORY-30.
pub fn wrapped_prose() {}
EOF
git add -A
assert_exit "wrapped prose sentence ending with bare id on next line is allowed" 0 "$(commit "docs: wrapped prose id")"
git reset -q --hard

# But a standalone one-line /// STORY-30 is still refused.
cat >>src/prose.rs <<'EOF'
/// STORY-30
pub fn bare_story() {}
EOF
git add -A
assert_exit "standalone bare /// STORY-30 is still refused" 1 "$(commit "feat: standalone bare story")"
assert_stderr_contains "refusal names the bare id line" "STORY-30"
git reset -q --hard

# And a /// trace:... line following a prose line is still refused.
cat >>src/prose.rs <<'EOF'
/// Reuses the usage log introduced by
/// trace:STORY-30 | ai:claude
pub fn wrapped_trace() {}
EOF
git add -A
assert_exit "wrapped /// trace marker is still refused" 1 "$(commit "feat: wrapped trace on doc")"
assert_stderr_contains "refusal names the wrapped trace line" "trace:STORY-30"
git reset -q --hard

# Refuse case 1: previous line ends in period
cat >>src/prose.rs <<'EOF'
/// Reuses the usage log.
/// STORY-30.
pub fn sentence_ended_dot() {}
EOF
git add -A
assert_exit "refuses bare id when previous line ends in period" 1 "$(commit "feat: period ended")"
assert_stderr_contains "refusal names the bare id line after period" "STORY-30"
git reset -q --hard

# Refuse case 2: previous line ends in colon
cat >>src/prose.rs <<'EOF'
/// Reuses the usage log introduced by:
/// STORY-30.
pub fn sentence_ended_colon() {}
EOF
git add -A
assert_exit "refuses bare id when previous line ends in colon" 1 "$(commit "feat: colon ended")"
assert_stderr_contains "refusal names the bare id line after colon" "STORY-30"
git reset -q --hard

# Refuse case 3: empty-/// separator (standalone ID paragraph)
cat >>src/prose.rs <<'EOF'
/// Does a thing.
///
/// STORY-30
pub fn empty_doc_separator() {}
EOF
git add -A
assert_exit "refuses bare id separated by empty doc comment line" 1 "$(commit "feat: empty doc separator")"
assert_stderr_contains "refusal names the bare id line after empty line" "STORY-30"
git reset -q --hard

# Refuse case 4: code fence preceding
cat >>src/prose.rs <<'EOF'
/// ```rust
/// STORY-30
pub fn code_fence_preceding() {}
EOF
git add -A
assert_exit "refuses bare id following code fence" 1 "$(commit "feat: code fence preceding")"
assert_stderr_contains "refusal names the bare id line after code fence" "STORY-30"
git reset -q --hard

# Refuse case 5: heading preceding
cat >>src/prose.rs <<'EOF'
/// # Provenance
/// STORY-30
pub fn heading_preceding() {}
EOF
git add -A
assert_exit "refuses bare id following heading" 1 "$(commit "feat: heading preceding")"
assert_stderr_contains "refusal names the bare id line after heading" "STORY-30"
git reset -q --hard

# Refuse case 6: bullet preceding
cat >>src/prose.rs <<'EOF'
/// - see
/// STORY-30
pub fn bullet_preceding() {}
EOF
git add -A
assert_exit "refuses bare id following bullet" 1 "$(commit "feat: bullet preceding")"
assert_stderr_contains "refusal names the bare id line after bullet" "STORY-30"
git reset -q --hard

# Refuse case 7: chained bare IDs
cat >>src/prose.rs <<'EOF'
/// Implemented for
/// STORY-30
/// STORY-31
pub fn chained_bare_ids() {}
EOF
git add -A
assert_exit "refuses chained bare ids" 1 "$(commit "feat: chained bare ids")"
assert_stderr_contains "refusal names second bare id line" "STORY-31"
git reset -q --hard

# Refuse case 8: prose in one hunk with bare ID in another hunk
cat >src/prose.rs <<'EOF'
/// existing line 1
/// existing line 2
/// existing line 3
/// existing line 4
/// existing line 5
/// existing line 6
/// existing line 7
/// existing line 8
/// existing line 9
/// existing line 10
pub fn multi_hunk() {}
EOF
git add src/prose.rs
commit "feat: base for multi hunk" >/dev/null

cat >src/prose.rs <<'EOF'
/// Reuses the usage log introduced by
/// existing line 1
/// existing line 2
/// existing line 3
/// existing line 4
/// existing line 5
/// existing line 6
/// existing line 7
/// existing line 8
/// existing line 9
/// existing line 10
/// STORY-30
pub fn multi_hunk() {}
EOF
git add src/prose.rs
assert_exit "refuses bare id when prose is in earlier hunk" 1 "$(commit "feat: separate hunks")"
assert_stderr_contains "refusal names bare id in separate hunk" "STORY-30"
git reset -q --hard HEAD~1

# 8c. TASK-1369: a code line inside a fenced block is not prose, so a bare id
#     after it is still refused; so is one after a markdown table row.
#     trace:TASK-1369 | ai:claude
cat >>src/prose.rs <<'EOF'
/// ```
/// let x = foo();
/// STORY-30
pub fn fenced_code_line() {}
EOF
git add -A
assert_exit "refuses bare id after a code line inside a fence" 1 "$(commit "feat: fenced code line")"
assert_stderr_contains "refusal names the bare id after fenced code" "STORY-30"
git reset -q --hard

cat >>src/prose.rs <<'EOF'
/// | name | value |
/// STORY-30
pub fn table_row() {}
EOF
git add -A
assert_exit "refuses bare id after a markdown table row" 1 "$(commit "feat: table row")"
assert_stderr_contains "refusal names the bare id after table row" "STORY-30"
git reset -q --hard

# After the fence closes, a wrapped prose sentence still continues normally.
cat >>src/prose.rs <<'EOF'
/// ```
/// let x = foo();
/// ```
/// Reuses the usage log introduced by
/// STORY-30.
pub fn fence_then_prose() {}
EOF
git add -A
assert_exit "wrapped prose after a closed fence is allowed" 0 "$(commit "docs: fence then prose")"
git reset -q --hard HEAD~1

# 8d. TASK-1371: the abbreviation guard is word-anchored. "e.g." does not end
#     a sentence, so the wrapped id is allowed; "devs." does, so it is refused.
#     U+2026 and U+3002 end a sentence too.
#     trace:TASK-1371 trace:TASK-1370 | ai:claude
cat >>src/prose.rs <<'EOF'
/// Works with any cache, e.g.
/// STORY-30.
pub fn abbreviation() {}
EOF
git add -A
assert_exit "bare id after an e.g. continuation is allowed" 0 "$(commit "docs: abbreviation")"
git reset -q --hard HEAD~1

cat >>src/prose.rs <<'EOF'
/// Written for the devs.
/// STORY-30
pub fn devs_period() {}
EOF
git add -A
assert_exit "refuses bare id after a word that merely ends in vs." 1 "$(commit "feat: devs period")"
assert_stderr_contains "refusal names the bare id after devs." "STORY-30"
git reset -q --hard

for ender in '…' '。'; do
    cat >>src/prose.rs <<EOF
/// The work is done${ender}
/// STORY-30
pub fn unicode_ender() {}
EOF
    git add -A
    assert_exit "refuses bare id after unicode sentence end ${ender}" 1 "$(commit "feat: unicode ender")"
    git reset -q --hard
done

# 8e. TASK-1370: removal-side context decides whether a removed bare id is a
#     move credit. trace:TASK-1370 | ai:claude
#     (i) Removed wrapped prose: the removed id continues a sentence, so it is
#     NOT debt and earns no credit; a new standalone id is still refused.
cat >src/ctx.rs <<'EOF'
/// Reuses the usage log introduced by
/// STORY-30
pub fn wrapped_old() {}
EOF
git add src/ctx.rs
git commit --no-verify -qm "chore: seed wrapped prose"
rm src/ctx.rs
cat >src/fresh.rs <<'EOF'
/// STORY-30
pub fn standalone_new() {}
EOF
git add -A
assert_exit "removed wrapped-prose id earns no credit for a new standalone id" 1 "$(commit "feat: no credit")"
assert_stderr_contains "refusal names the uncredited id" "STORY-30"
git reset -q --hard HEAD~1

#     (ii) Removed id after a heading IS debt; moving it is credited.
cat >src/ctx.rs <<'EOF'
/// # Provenance
/// STORY-30
pub fn heading_old() {}
EOF
git add src/ctx.rs
git commit --no-verify -qm "chore: seed heading debt"
rm src/ctx.rs
cat >src/moved.rs <<'EOF'
/// STORY-30
pub fn moved() {}
EOF
git add -A
assert_exit "removed id after a heading credits its move" 0 "$(commit "refactor: move after heading")"
git reset -q --hard HEAD~2

#     (iii) Add side: after a credited leak line the prose context resets, so
#     a following bare id is new debt even though the line before the leak
#     was unfinished prose.
cat >src/ctx.rs <<'EOF'
/// trace:STORY-30 | ai:claude
pub fn seeded() {}
EOF
git add src/ctx.rs
git commit --no-verify -qm "chore: seed trace debt"
rm src/ctx.rs
cat >src/moved.rs <<'EOF'
/// Reuses the usage log introduced by
/// trace:STORY-30 | ai:claude
/// STORY-31
pub fn seeded() {}
EOF
git add -A
assert_exit "bare id after a credited leak line is still refused" 1 "$(commit "feat: after credited leak")"
assert_stderr_contains "refusal names the id after the credited leak" "STORY-31"
assert_stderr_lacks "the credited leak line itself is excused" "trace:STORY-30"
git reset -q --hard HEAD~1

# 9. `////` is a PLAIN comment to rustc (four or more slashes are never
#    rustdoc, so never `--help` text). Provenance on such a line is fine and
#    must not be refused; the three-slash form right next to it still is.
#    trace:BUG-1661
cat >src/plain.rs <<'EOF'
//// trace:TASK-555 | ai:claude
pub fn four_slashes() {}
    ///// TASK-556
pub fn five_slashes() {}
EOF
git add -A
assert_exit "//// (4+ slashes) provenance is a plain comment and is allowed" 0 "$(commit "feat: four slashes")"
cat >>src/plain.rs <<'EOF'
/// trace:TASK-557 | ai:claude
pub fn three_slashes() {}
EOF
git add -A
assert_exit "/// provenance beside a //// line is still refused" 1 "$(commit "feat: three slashes")"
assert_stderr_contains "refusal names the /// line" "TASK-557"
assert_stderr_lacks "refusal does NOT name the //// line" "TASK-555"
git reset -q --hard

# 10. The cargo-fmt step must not claim "drift fixed" when cargo itself fails
#     (e.g. no rustup default toolchain). Fake `cargo` on PATH: every
#     invocation fails the way rustup does. The commit still proceeds (the step
#     is a convenience; CI is the fmt gate) but the output reports the failure
#     and never claims a fix. trace:BUG-1661
SHIMS="$FAKEHOME/shims"
mkdir -p "$SHIMS/broken" "$SHIMS/drifty"
cat >"$SHIMS/broken/cargo" <<'EOF'
#!/bin/sh
echo "error: rustup could not choose a version of cargo to run, because one wasn't specified explicitly, and no default is configured." >&2
exit 1
EOF
# `fmt --check` reports drift (exit 1); plain `fmt` succeeds — the honest
# "drift fixed" path.
cat >"$SHIMS/drifty/cargo" <<'EOF'
#!/bin/sh
case " $* " in
    *" --check "*) echo "Diff in src/fmt.rs at line 1:" >&2; exit 1 ;;
esac
exit 0
EOF
chmod +x "$SHIMS/broken/cargo" "$SHIMS/drifty/cargo"

cat >src/fmt.rs <<'EOF'
pub fn needs_fmt(  ) {}
EOF
git add -A
HOOK_PATH="$SHIMS/broken:$PATH"
assert_exit "commit proceeds when cargo itself fails" 0 "$(HOOK_PATH="$HOOK_PATH" commit "feat: broken cargo")"
assert_output_lacks "broken cargo: no 'drift fixed' claim" "drift fixed"
assert_stderr_contains "broken cargo: failure is reported" "cargo fmt --all FAILED"
assert_stderr_contains "broken cargo: the cargo error is surfaced" "no default is configured"

# 11. Real drift with a working cargo: the fix claim is still made, so the
#     honest-failure path did not silence the success path.
cat >>src/fmt.rs <<'EOF'
pub fn more(  ) {}
EOF
git add -A
HOOK_PATH="$SHIMS/drifty:$PATH"
assert_exit "commit proceeds when fmt fixes drift" 0 "$(HOOK_PATH="$HOOK_PATH" commit "feat: drifty cargo")"
assert_output_contains "drifty cargo: 'drift fixed and re-staged' is claimed" "drift fixed and re-staged"
assert_stderr_lacks "drifty cargo: no failure reported" "cargo fmt --all FAILED"
unset HOOK_PATH

if [ "$fail" -ne 0 ]; then
    echo "PRE-COMMIT PROVENANCE MOVE GATE (TASK-144): FAILURES"
    exit 1
fi
echo "PRE-COMMIT PROVENANCE MOVE GATE (TASK-144): all checks passed"
