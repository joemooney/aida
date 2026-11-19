# Reviewer completion regression coverage

<!-- trace:BUG-2546 | ai:codex -->

The completion watcher must inspect the selected verdict's raw `sweep_pending`
barrier before accepting its normalized disposition. A pending approval is
normalized to REQUEST_CHANGES for gates, but does not establish completed
delivery and must not start the reviewer's two-second shutdown grace period.

`reviewer_completion::tests::bug2546_wait_loop_preserves_pending_child_and_reaps_completed_twin`
runs a child that publishes completed delivery only after three seconds. The
test covers APPROVED, REQUEST_CHANGES, REJECTED and WAITING. Early shutdown leaves
the pending barrier in place and fails the test; successful completion must terminate the lingering
child and retain the verdict timestamp for runtime reporting.

The adjacent delayed-delivery test checks that pending records retain neither
the shutdown latch nor the runtime timestamp. The WAITING test checks that a
completed, attested WAITING record stops the reviewer and supplies its runtime
timestamp while its parsed verdict remains non-approving. Collector approval
semantics are unchanged.

`bug2546_completed_delivery_still_requires_current_launch_identity` also runs
each of those four dispositions through real verdict discovery with a stale
timestamp, a different grant, and a mismatched reviewed SHA. None may latch
shutdown or runtime evidence. Replacing each with a current, correctly bound
completed twin must start the grace and retain the runtime timestamp.

The `bug2546_interactive_codex_resume_receives_current_reviewer_policy` and
`bug2546_interactive_claude_resume_receives_current_reviewer_policy` tests exercise
the real interactive resume launch arms with captured vendor arguments. Both
must receive the newly composed Cargo restriction and exit instruction, even
when the saved session predates that policy. These are argument-contract tests;
they do not run live vendor sessions.

Run through the lane runner, building the matching CLI first because the resume
fixtures execute its model-free authority probe:

```sh
aida lane job run -- cargo build --profile agent -p aida-cli --bin aida
aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib -- bug2546
```

If a job exits 75, continue with `aida lane job wait <job-id>` until it settles.

Validation on 2026-11-13: the CLI prerequisite build passed, then the focused
suite executed eight tests with eight passes (lane job
`j20261113T165702397-2-0`; prerequisite build `j20261113T165548184-2-0`).
The resume fixture explicitly
points its reviewer worktree at the launcher's scratch store so the real Codex
seat preflight can validate the same roster before argument capture.
