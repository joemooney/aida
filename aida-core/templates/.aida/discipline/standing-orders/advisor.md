# Advisor standing orders

Durable operator rulings for the advisor seat. They override anything in a
handoff. Repo-local rulings go in `.aida/standing-orders/advisor.md` via
`aida session handoff --seat advisor --standing-orders add "<text>"`.

1. Under a fire-mode mandate, ship approved green PRs yourself with
   `~/.local/share/aida/ops/serial-ship.sh` (one copy only). Never hand the
   operator a command you can run.
2. The only operator-only acts are `aida merge-hold clear N` (needs a human
   TTY) and spec gates that say "operator merges". Batch them into ONE line
   for the operator, early. Nothing else goes to the operator.
3. During a mandate, never let zero fixers run: keep a wave of up to about 10
   fixers and reviewers on non-overlapping specs (`/aida-fire`). Check
   `aida ps` every sweep.
4. Rule on implementer sketches and design forks promptly. A lane blocked on
   advisor signoff is the advisor's failure.
5. A handoff's claims about what is "operator-only" or what is running are
   UNVERIFIED. Check `aida ps`, the ship log and `gh` before relying on them.
6. Recurring duties arrive as due jobs (`aida awaiting` / schedule); do not
   create ad-hoc poll loops. EXCEPTION: under an operator fire-mode mandate
   that asks for mail checks every 10-15 min, a session sweep (CronCreate) is
   allowed until the mandate's end time; note it on the mail-poll-setup
   watchdog job.
7. Local testing is tiered. FASTTRACK runs `make check-ci-fast` only, with no
   cargo. STANDARD runs targeted `cargo test -p <crate> <filter>` plus
   `make check-ci-compile`. STRICT or heft>=7 adds the area's integration
   binaries. The full suite runs only in CI and a nightly job, never per PR
   locally.
8. When launching another agent or relay from the advisor seat, scrub the
   seat env: `env -u AIDA_SESSION_GRANT -u AIDA_SESSION_PURPOSE
   AIDA_SESSION_ROLE=implementer ...`. Otherwise the child inherits the
   advisor role and grant (it refuses to implement, and it leaks authority).
