# Fasttrack a trivial change

The low-ceremony lane for genuinely trivial work — a doc tweak, a one-line UX
papercut, a string fix. File it, do it, and let CI be the only gate. No
human-review round-trip. trace:STORY-587

## When to use

`/aida-fasttrack <description>` is for changes that are **cosmetic, doc-only, or
a single obvious line** — where a full review ceremony is overkill. If the work
turns out to be more than trivial (touches behavior, needs a judgment call),
**punt out of the lane**: stop, drop the fasttrack tags, and route it through
the normal review flow. The lane is a convenience, not a way to dodge review on
real changes.

## The one hard rule

Fasttrack skips the **human-review ceremony** and batches — it does **NOT** skip
CI. CI still runs and must be **green before merge** (never merge red). The lane
uses `lifecycle:no-review`, never `lifecycle:no-ci-wait` / `lifecycle:trivial`
(those merge optimistically before CI is green). "Without much ado" = no human
gating, still integrity-gated.

## Instructions

1. Read `$ARGUMENTS` as the change description. Pick a `--type` that fits
   (`bug` for a papercut/defect, `task` for a chore/doc).
2. File + queue it in one shot, and capture the SPEC-ID — the `aida fasttrack`
   verb owns the lane's filing convention (Approved + queued + `batch:fasttrack`
   + `lifecycle:no-review`), so the tags live in one place, not in this prose:
   ```
   aida fasttrack "<description>" --type <type>
   ```
3. Implement the change in a fresh worktree/branch off latest `origin/main`.
   Add a `// trace:<SPEC-ID>` comment if it's code.
4. **Reduced gate** (the point of the lane): `cargo build -p aida-cli`,
   `cargo fmt --all -- --check`, `cargo clippy -p aida-cli -- -D
   clippy::correctness`, and a quick smoke of the change. Skip the exhaustive
   local test run — CI runs the full suite.
5. Commit with the `(SPEC-ID)` trailer; open a PR.
6. **CI is the gate**: read the required check = SUCCESS, *then* merge (squash).
   If CI is red, fix-forward or punt out of the lane — never merge red.
7. `aida pull` to auto-bump the spec to Completed.

## Batch variant

Several fasttrack items at once: file each with `--batch fasttrack`, then drain
the whole bucket with `aida queue work --batch fasttrack --auto-complete` — one
implement→CI→merge lifecycle per item, CI gating each.

Pairs with the normal review flow (for anything non-trivial) and `/aida-commit`
(the trailer convention).

ARGUMENTS: $ARGUMENTS
