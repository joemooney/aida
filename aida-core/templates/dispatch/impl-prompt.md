You are already in your worktree {{WORKTREE}} (branch {{BRANCH}}, off origin/main). NEVER call the EnterWorktree tool.
Implementer brief (advisor {{TIMESTAMP}}): {{SPEC}}. Operator goal: open list to ZERO, no deferrals — every item in scope gets a fix and a test, or evidence it no longer reproduces.
1. Claim {{SPEC}} from this worktree. Read `aida show {{SPEC}} --full --comments` and every superseded/folded member (`aida show <id> --full`).
2. {{BRIEF}}
3. Tests: use `crate::test_git::git_command()` in any test you add or touch; never raise `scripts/git-maintenance-baseline.txt` or widen an allowlist. Add `// trace:{{SPEC}} | ai:{{VENDOR}}` beside implementation code.
**Sandboxed headless lane override** (`AIDA_HEADLESS=1` in a native or
AIDA-contained lane): do not run `make check-ci-fast` directly inside the seat.
Its containment fixtures need sockets, its ratchets invoke Cargo, and the
aggregate can exceed the 60-second tool-call limit. Route the gate by who
publishes:

- **Native, host-published seat:** leave edits uncommitted and report
  `check_ci_fast=deferred(host)` with the reason above. This is not a passing
  gate. The host launcher runs the gate after exit and before publication.
- **Contained, self-publishing lane:** before publication, run
  `aida lane job run -- make check-ci-fast` through the host-side runner.
  On exit 75, poll `aida lane job wait <id>` in bounded calls until it finishes.
  Require exit 0 for the final tree before pushing or opening the PR; rerun
  after further edits. If the runner is unavailable, refuses, times out, or
  fails, stop publication and report the blocker. Deferral is not permission
  to publish, and a later post-exit gate is not a pre-publication pass.

Run targeted Cargo tests through `aida lane job run -- cargo test --profile
agent -p <crate> --lib -- <filter>` and poll `aida lane job wait <id>` on exit
75. If the runner refuses or is unavailable, report `untested=true`. A fast-gate
job does not count as an executed targeted Cargo test. This override applies to
the FASTTRACK/LIGHT/STRICT instructions below as well as CLI golden tests
(route their Cargo invocation through the lane runner).
<!-- trace:TASK-3123 | ai:codex -->

4. Gates (LIGHT): ONE targeted `cargo test -p <crate> <module filter>` run, then `make check-ci-fast`, then push; CI runs the full shards and is the gate. Run `make test-touched`/`make check-ci-compile` ONLY if you touched shared test infra (`test_env`, `test_git`, ratchet baselines) or >10 files. NEVER the full suite, `cargo test --workspace`, or `AIDA_CARGO_NO_SLOT`; no outer timeout on cargo. Added or changed a CLI command or flag? Regenerate the help golden (`AIDA_UPDATE_SNAPSHOTS=1 cargo test --profile agent -p aida-cli-lib help_surface_matches_golden_snapshot`) and commit `aida-cli-lib/src/tests/golden/cli_help_snapshot.txt`.
5. Commit `[AI:{{VENDOR}}] type(scope): summary ({{SPEC}})` (use `Refs {{SPEC}}` only for a non-completing slice), push, open the PR with `gh pr create`, then `aida mailbox send --to advisor --subject "READY: PR-<n> ({{SPEC}})" "<SHA + one line per item>"`. Watch CI and fix red jobs yourself. Never merge, never self-approve, never clear holds. If blocked, mail the advisor the exact error and stop.
