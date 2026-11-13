# TASK-3428 partial batch 5 publication scope

PR #3829 is a partial slice. Its PR title and eventual squash subject must be:

`[AI:codex] fix(imports): correct partial batch 5 ownership sites (Refs TASK-3428)`

This follows advisor correction 68 and supersedes the completion trailer in
implementation commit 86443475a37. TASK-3428 remains In Progress: 45 crate-glob
imports across 40 ledger files and the six `lib_part1..6.rs` includes remain.
AC1 and AC3 are unfinished.

The production scope is exactly eight modules in `aida-cli-lib/src/`:
`ci_probe.rs`, `config_cmd.rs`, `db_reconcile.rs`, `merge_hold_cmd.rs`,
`pr_list.rs`, `status_prs.rs`, `status_sections.rs`, and `upgrade_release.rs`.
These replace crate-glob imports with explicit imports. Dependencies exposed
by removing the globs are imported locally in the affected test files.
Only these eight module entries are pruned from `scripts/crate-globs-legacy.json`;
all surviving debt is retained.

The two authorized qualified-call rewrites both call
`crate::leases::lease_state_for`:

- `status_prs::collect_unshipped_work_items_bounded`, `status_prs.rs:2569`.
- `status_sections::lease_agent_view`, `status_sections.rs:1933`.

The corresponding module-level imports were removed to avoid adding
ownership-liveness sites. The ownership-liveness baseline is unchanged.

Validation recorded in spec comment 71 and the host publication receipt for
15aa7e80b9fa:

- Launcher job `j20261111T053254502-2-0` passed
  `cargo check --profile agent -p aida-cli-lib --tests`. This is compilation
  only, not executed Rust tests.
- Ownership-liveness ratchet passed two executed Python tests.
- `rustfmt --check` and `git diff --check` passed.
- `make check-ci-fast` passed all 47 fast-tier gates with umask 077; the host
  receipt records `fast_gate=pass` for 15aa7e80b9fa. The crate-glob ratchet
  reported 45 remaining imports and zero additions. The initial attempt under
  the inherited shared-write umask stopped on mail-wake fixture mode checks.
- CI for 15aa7e80b9fa was pending at the round 19 metadata correction.
  These earlier checks do not attest a subsequent publication head.

Round 19 changes publication metadata and this durable scope note only.
The native seat leaves this file uncommitted for the host launcher to create
the required new commit and publish it using the Refs subject above.

<!-- trace:TASK-3428 | ai:codex -->
