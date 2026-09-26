#!/usr/bin/env bash
set -euo pipefail

# Fixture test for scripts/check-removed-flags.sh: release history may quote
# removed flag spellings, but live docs and templates may not.
# trace:BUG-1665 | ai:claude
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

git -C "$TMP" init -q
mkdir -p "$TMP/scripts" "$TMP/docs" "$TMP/aida-core/templates/skills"
cp "$ROOT/scripts/check-removed-flags.sh" "$TMP/scripts/"

run_check() {
  (cd "$TMP" && bash scripts/check-removed-flags.sh </dev/null) >"$TMP/out" 2>&1
}

# (a) Removed flags quoted in root CHANGELOG history lines pass.
cat >"$TMP/CHANGELOG.md" <<'MD'
- aida graph --tree renders a flat list (#852)
- aida usage --slowest was replaced by a subcommand
MD
printf 'aida graph tree EPIC-1\n' >"$TMP/docs/guide.md"
if ! run_check; then
  echo "FAIL: CHANGELOG history should be exempt" >&2
  cat "$TMP/out" >&2
  exit 1
fi

# (b) The same flag in a live doc fails.
printf 'Run aida graph --tree EPIC-1\n' >"$TMP/docs/guide.md"
if run_check; then
  echo "FAIL: removed flag in a live doc was not caught" >&2
  exit 1
fi
grep -F "docs/guide.md" "$TMP/out" >/dev/null

# (b2) Piped stdin must not replace the tree scan.
if (cd "$TMP" && printf 'unrelated text\n' | bash scripts/check-removed-flags.sh) >"$TMP/out" 2>&1; then
  echo "FAIL: piped stdin hid a removed flag in a live doc" >&2
  exit 1
fi
grep -F "docs/guide.md" "$TMP/out" >/dev/null

# (c) A removed flag in a skill template fails.
printf 'aida graph tree EPIC-1\n' >"$TMP/docs/guide.md"
printf 'aida usage --slowest\n' >"$TMP/aida-core/templates/skills/SKILL.md"
if run_check; then
  echo "FAIL: removed flag in a skill template was not caught" >&2
  exit 1
fi
grep -F "aida-core/templates/skills/SKILL.md" "$TMP/out" >/dev/null

# (d) A nested CHANGELOG.md is not exempt; only the root release history is.
rm "$TMP/aida-core/templates/skills/SKILL.md"
printf 'aida graph --impact X\n' >"$TMP/docs/CHANGELOG.md"
if run_check; then
  echo "FAIL: nested CHANGELOG.md should still be scanned" >&2
  exit 1
fi

echo "check-removed-flags fixture tests passed"
