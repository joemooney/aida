# Advisor standing orders

Durable operator rulings for the advisor seat. They override anything in a
handoff. Repo-local rulings go in `.aida/standing-orders/advisor.md` via
`aida session handoff --seat advisor --standing-orders add "<text>"`.

1. Under a fire-mode mandate, ship approved green PRs yourself with the
   project's ship procedure. Never hand the operator a command you can run.
2. The only operator-only acts are `aida merge-hold clear N` (needs a human
   TTY) and spec gates that say "operator merges". Batch them into ONE line
   for the operator, early. Nothing else goes to the operator.
3. During a mandate, never let zero fixers run: keep a wave of fixers and
   reviewers on non-overlapping specs (`/aida-fire`). Check `aida ps` every
   sweep.
4. Rule on implementer sketches and design forks promptly. A lane blocked on
   advisor signoff is the advisor's failure.
5. A handoff's claims about what is "operator-only" or what is running are
   UNVERIFIED. Check `aida ps`, the ship log and `gh` before relying on them.
6. Recurring duties arrive as due jobs (`aida awaiting` / schedule); do not
   create ad-hoc poll loops or model-side schedulers. If a mandate needs a
   cadence that no due job covers, ask the operator to add a scheduled job.
7. Local testing is tiered. FASTTRACK runs `make check-ci-fast` only, with no
   cargo. LIGHT (the default) runs ONE targeted `cargo test -p <crate>
   <filter>` plus `make check-ci-fast`; `make test-touched` /
   `make check-ci-compile` only for shared test infra or >10 files.
   Reviewers run no cargo by default. STRICT or heft>=7 adds the area's integration
   binaries. The full suite runs only in CI and a nightly job, never per PR
   locally. Briefs say so: they never order `make check-ci`, `make check`,
   `cargo test --workspace` or an unfiltered `--lib`, and never set
   `AIDA_CARGO_NO_SLOT` (see `rework-brief-craft.md` step 6).
8. Use `aida agent new <vendor> --role implementer ...` for a delegated
   child. In the advisor's human terminal, first run
   `eval "$(aida role enter advisor --delegate-seat implementer)"`;
   retain that shell's `AIDA_SESSION_GRANT`
   when invoking AIDA, and scrub `AIDA_SESSION_PURPOSE`. The launcher validates
   delegation and replaces the parent grant with a child-specific grant.
   A headless lane is `aida queue work <SPEC> --role implementer --no-human`
   from Bash: its own worktree, lease and implementer seat, never your grant,
   and one `implementer:` result line with an exit code to branch on. For an
   interactive `claude --bg` lane, run
   `aida worktree add <SPEC> --role implementer`, then start `claude --bg`
   from inside that worktree. The lane takes its identity from the lease at
   session start, because a `--bg` session inherits the claude daemon's
   environment, not yours, so env scrubbing on the launch line does nothing.
   Children must not inherit the advisor grant. An ambient role alone does
   not authorize AIDA delegation.
   <!-- trace:BUG-2188 | ai:codex -->
   <!-- trace:STORY-1662 | ai:claude -->
