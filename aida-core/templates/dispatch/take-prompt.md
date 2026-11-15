You are already in your worktree {{WORKTREE}} (branch {{BRANCH}}, the PR #{{PR}} head branch). NEVER call the EnterWorktree tool.
Takeover fixer brief (advisor {{TIMESTAMP}}): PR #{{PR}} ({{SPEC}}). The original author is idle; you continue its branch.
1. Claim {{SPEC}} from this worktree. Read `aida show {{SPEC}} --full --comments`, the current review verdict `aida review verdict PR-{{PR}} --json` (it reads the PR's gate ledger), and `gh pr view {{PR}} --comments`.
2. `git merge origin/main` (resolve conflicts preserving both sides' intent; no force-push ever).
3. {{BRIEF}}
4. Main's git auto-maintenance ratchet (`tests/test_git_maintenance_ratchet.py`) flags bare git spawns in tests: use `crate::test_git::git_command()` in any test you add or touch; never raise `scripts/git-maintenance-baseline.txt`.
5. Add `// trace:{{SPEC}} | ai:{{VENDOR}}` beside implementation code.
**Sandboxed headless lane override** (`AIDA_HEADLESS=1` in a native or
AIDA-contained lane): do not run `make check-ci-fast` inside the seat. Its
containment fixtures need sockets, its ratchets invoke Cargo, and the aggregate
can exceed the 60-second tool-call limit. Report
`check_ci_fast=deferred(host)` with that reason in the handoff; this is not a
passing gate. The host launcher runs the unchanged gate before publication.
Run targeted Cargo tests through `aida lane job run -- cargo test --profile
agent -p <crate> --lib -- <filter>` and poll `aida lane job wait <id>` on exit
75. If the runner refuses or is unavailable, report `untested=true`. Native
seats leave edits uncommitted for host publication. This override applies to
the FASTTRACK/LIGHT/STRICT instructions below as well as CLI golden tests
(route their Cargo invocation through the lane runner).
<!-- trace:TASK-3123 | ai:codex -->

6. Gates (LIGHT): ONE targeted `cargo test -p <crate> <module filter>` run, then `make check-ci-fast`, then push; CI runs the full shards and is the gate. Run `make test-touched`/`make check-ci-compile` ONLY if you touched shared test infra (`test_env`, `test_git`, ratchet baselines) or >10 files. NEVER the full suite, `cargo test --workspace`, or `AIDA_CARGO_NO_SLOT`; no outer timeout on cargo.
7. Commit with the PR's existing type/scope and the PR's existing trailer IDs (keep `Refs` iff the PR is a non-completing slice), push (no force), comment on the PR one line per fix, then `aida mailbox send --to advisor --subject "PR-{{PR}} pushed ({{SPEC}})" "<SHA + one line per fix>"`. Never merge, never self-approve, never clear holds. If blocked, mail the advisor the exact error and stop.
