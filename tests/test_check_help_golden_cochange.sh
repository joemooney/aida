#!/usr/bin/env bash
set -euo pipefail

# Fixture test for scripts/check-help-golden-cochange.sh: a branch that edits
# an *_args.rs or command_registry.rs file must also update the golden help
# snapshot, or carry a `Help-Golden: unchanged` trailer.
# trace:TASK-B6B1278890774EF993D2EBC327862470-125 | ai:claude
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
CHECK="$ROOT/scripts/check-help-golden-cochange.sh"
GOLDEN="aida-cli-lib/src/tests/golden/cli_help_snapshot.txt"

g() {
  git -C "$TMP" -c maintenance.auto=false -c gc.auto=0 -c user.name=fixture -c user.email=fixture@example.com \
    -c commit.gpgsign=false -c core.hooksPath=/dev/null "$@"
}
commit() {
  g add -A
  g commit -q -m "$1"
}
run_check() {
  (cd "$TMP" && bash "$CHECK" main </dev/null) >"$TMP/.out" 2>&1
}
expect_pass() {
  if ! run_check; then
    echo "FAIL: $1" >&2
    cat "$TMP/.out" >&2
    exit 1
  fi
}
expect_fail() {
  if run_check; then
    echo "FAIL: $1" >&2
    exit 1
  fi
}
branch() {
  g checkout -q main
  g checkout -q -B "$1"
}

g init -q -b main
printf '/.out\n' >"$TMP/.gitignore"
mkdir -p "$TMP/aida-cli-lib/src/tests/golden" "$TMP/aida-cli-lib/src/sub"
printf 'pub struct A;\n' >"$TMP/aida-cli-lib/src/queue_args.rs"
printf 'pub static REGISTRY: () = ();\n' >"$TMP/aida-cli-lib/src/command_registry.rs"
printf 'pub fn f() {}\n' >"$TMP/aida-cli-lib/src/queue.rs"
printf 'golden v1\n' >"$TMP/$GOLDEN"
commit "base"

# (a) A branch that touches no surface file passes.
branch a
printf 'pub fn f() { () }\n' >"$TMP/aida-cli-lib/src/queue.rs"
commit "handler edit"
expect_pass "non-surface change was flagged"

# (b) An *_args.rs edit without a golden update fails and names the command.
branch b
printf 'pub struct A { flag: bool }\n' >"$TMP/aida-cli-lib/src/queue_args.rs"
commit "add a flag"
expect_fail "args change without golden update was not caught"
grep -F "aida-cli-lib/src/queue_args.rs" "$TMP/.out" >/dev/null
grep -F "AIDA_UPDATE_SNAPSHOTS=1 cargo test --profile agent -p aida-cli-lib help_surface_matches_golden_snapshot" "$TMP/.out" >/dev/null

# (c) The same edit plus a golden update passes, even in a later commit.
printf 'golden v2\n' >"$TMP/$GOLDEN"
commit "regenerate golden"
expect_pass "args change with golden update was flagged"

# (d) A command_registry.rs edit without a golden update fails.
branch d
printf 'pub static REGISTRY: ((),) = ((),);\n' >"$TMP/aida-cli-lib/src/command_registry.rs"
commit "register a command"
expect_fail "command_registry change without golden update was not caught"

# (e) A `Help-Golden: unchanged` trailer opts out.
branch e
printf 'pub struct A; // reorder\n' >"$TMP/aida-cli-lib/src/queue_args.rs"
commit $'refactor args\n\nHelp-Golden: unchanged'
expect_pass "Help-Golden trailer did not opt out"

# (f) The trailer must be its own line, not prose that mentions it.
branch f
printf 'pub struct A; // again\n' >"$TMP/aida-cli-lib/src/queue_args.rs"
commit "mention Help-Golden: unchanged inline"
expect_fail "inline trailer mention was accepted"

# (g) Only top-level aida-cli-lib/src *_args.rs files count.
branch g
printf 'x\n' >"$TMP/aida-cli-lib/src/sub/other_args.rs"
commit "nested args-like file"
expect_pass "nested *_args.rs was treated as surface"

# (h) An unresolvable base fails loudly instead of passing.
if (cd "$TMP" && bash "$CHECK" no-such-ref) >"$TMP/.out" 2>&1; then
  echo "FAIL: unresolvable base passed" >&2
  exit 1
fi

echo "check-help-golden-cochange fixtures: ok"
