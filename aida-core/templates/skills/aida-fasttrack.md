---
name: aida-fasttrack
description: "The low-ceremony lane for genuinely trivial work: cosmetic, documentation-only, or one obvious line."
---
# Fasttrack a trivial change

Fasttrack is a single lane for work that is genuinely too small to justify a
human-review round trip. It files an Approved requirement, queues it, and adds
`batch:fasttrack` + `lifecycle:no-review`. CI still runs and must pass.

<!-- trace:TASK-1267 | ai:codex -->

## Eligibility

A request belongs in fasttrack only when all of these are true:

1. It is cosmetic, documentation-only, or one obvious line.
2. It is bounded, single-purpose, low blast radius, and easy to revert.
3. It changes no public API, data format, architecture, security boundary,
   autonomy machinery, queue/lease behavior, or cross-cutting lifecycle rule.
4. Its acceptance condition is unambiguous.

If any condition fails, file the work normally. Use `execution_mode=drain` when
the desired behavior is full autonomous execution with the normal gates.

## Instructions

1. Read `$ARGUMENTS` and apply the eligibility test above.
2. Pick `bug` for a papercut/defect or `task` for a chore/doc tweak.
3. File and queue eligible work in one shot:

   ```text
   aida fasttrack "<description>" --type <task|bug>
   ```

4. Implement on a fresh branch from `origin/main` and add a
   `// trace:<SPEC-ID>` comment for code changes.
5. Run the relevant build, `cargo fmt --all -- --check`, correctness clippy,
   and a focused smoke test.
6. Commit with the `(SPEC-ID)` trailer. Merge only after CI succeeds, then run
   `aida pull` so the spec auto-completes.

`aida fasttrack --express` remains as a deprecated compatibility alias for one
release. It files Approved + queued work with `execution_mode=drain` and no
fasttrack lane or lifecycle tag. Prefer normal intake with `--mode drain`.

## Punt-out rule

If implementation reveals that the work is not trivial, punt it to
NeedsAttention, file a finding, and remove `batch:fasttrack` and
`lifecycle:no-review`. Never silently continue larger work under the reduced
review gate.

ARGUMENTS: $ARGUMENTS
