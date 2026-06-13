# Fasttrack a trivial change

File, implement, and merge a genuinely trivial change (doc tweak, one-line
papercut, string fix) with no human-review ceremony — CI is the only gate.

## Instructions

Follow the workflow in `.claude/skills/aida-fasttrack.md`:

1. Read `$ARGUMENTS` as the change description.
2. File + queue it tagged for the lane (capture the SPEC-ID):
   `aida add "<desc>" --status approved --queue --batch fasttrack --tags lifecycle:no-review --type <task|bug>`.
3. Implement on a fresh branch off `origin/main`; **reduced gate** —
   build + `fmt --check` + `clippy -D correctness` + a quick smoke (CI runs the
   full suite).
4. Commit with the `(SPEC-ID)` trailer, open a PR, and merge **only on green
   CI** (never merge red); `aida pull` to auto-bump.

The one hard rule: fasttrack skips human review, **not** CI. If the work turns
out non-trivial, punt out of the lane to normal review.

ARGUMENTS: $ARGUMENTS
