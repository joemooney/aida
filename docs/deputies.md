# Deputies: command vs script

AIDA has two deputies. Both run work that other sessions hand them, under the
seat grant held by your own terminal. They differ in what they run and in how
much of your authority each job carries.

| | Command deputy | Script deputy |
| --- | --- | --- |
| Start it with | `aida deputy` | `aida deputy shell run` |
| Runs | one allow-listed command per job (`aida …`, or a program you named with `--allow-exec`) | one whole bash script per job |
| How many at once | several (`--workers`) | one at a time |
| Terminal | none, unless the command matches a terminal entry you set | always: your terminal, relayed to the job |
| Authority of a job | a job without a terminal is refused by every check that needs a person at the keyboard | **each job counts as you at the keyboard** |
| Who may submit | any session that can write its queue | the advisor seat by default (`--allow-seat` widens it) |
| Safe to leave running | yes | only while you are willing to have scripts run as you |

Use the command deputy for single commands. Use the script deputy only for
multi-step operator acts the command deputy cannot express: clearing a merge
hold and then shipping, a dev build, a live acceptance run.

## The command deputy

`aida deputy` is described in full in the
[CLI reference](cli/06-roles-sessions.md#aida-deputy). It runs no shell
scripts; a job that is a shell script belongs to the script deputy.

## The script deputy

### Starting it

Run `aida deputy shell run [--name N]` in your own terminal, through the
`aida()` shell function, holding an advisor grant (`aida role enter advisor`).
It refuses to start:

- inside a job of either deputy, in an agent session, or in a headless run;
- unless stdin, stdout and stderr are all your terminal;
- when run as a bare binary instead of through the shell function;
- without a valid advisor grant;
- when another script deputy of the same name is running.

The banner shows the grant and its expiry, the queue directory, who may
submit, the refresh interval, retention and the mandate it is tied to, and
says plainly that every job runs as you, at that terminal. Starting it there is
your consent to that.

### Submitting

An advisor session queues a script with:

```bash
aida deputy shell submit --name N --file job.sh --wait
```

`submit` prints the job id. With `--wait` it waits for the job, prints the
transcript and a result line, and exits with the script's exit code. Without
it, use `aida deputy shell status <id>`, `wait <id>`, `log <id> [--follow]`,
`list` and `cancel <id>`. A `wait` that gives up before the job ends exits 75;
that is not a job failure.

`submit` refuses unless the session holds a valid seat grant, refuses inside a
script job (a job cannot queue its successor), and refuses a seat the runner
does not accept. Only the advisor seat may submit unless you start the runner
with `--allow-seat`; no setting in the project's tracked `.aida/config.toml`
can add a seat, because any merged change could edit that file.

### What happens to a job

Jobs run strictly one at a time, oldest first. For each one the runner:

1. moves it from `incoming/` into its own `jobs/<id>/` directory, never
   replacing anything;
2. checks who submitted it (below) and refuses it with a named reason if any
   check fails. A refused job never runs, and its script is never written out;
3. shows you the job id, the submitter, the script's sha256 and the script;
4. runs `bash --noprofile --norc jobs/<id>/script.sh` as a child process in
   its own session on a fresh terminal (pty), with its working directory set
   to your home. An `exit`, `cd`, `set -e` or `trap` in the script affects
   only that child, never the runner or the next job;
5. relays your terminal to the job and copies every byte it prints to
   `transcript.log`;
6. records the exit code and mails the result to the submitting session.

Ctrl-C while a job runs goes to the job. Ctrl-C between jobs stops the
runner. A job that runs past its timeout (`submit --timeout`, default 2h) is
stopped and records exit code 124. A cancelled job records 130. A job left
running when the runner died is recorded as orphaned (130) and never re-run.

### Who submitted it

At submit, the job records the submitter from its **validated** grant record
and its own process: session id, seat, subject, principal, grant id, the
binding token it presented, its pid, the project and this machine's hostname
and boot id. Nothing is taken from free text.

Before a job runs, the runner refuses it when:

| Reason | When |
| --- | --- |
| `refused:unattributed` | the job names no grant or no presenter token |
| `refused:foreign-host` | it was submitted on another machine, or before this boot |
| `refused:grant` | the grant is missing, expired, revoked, outside the roster, or does not match the job's seat, subject or principal |
| `refused:seat` | the seat may not submit to this deputy |
| `refused:not-live` | no process the grant is bound to is alive, and (for a sandboxed session whose grant is bound only to a session key) the session the grant was issued to has had no heartbeat in the last 10 minutes |
| `refused:schema` | the record has an unknown field, another schema version, a sha256 that is not its script's, or an id already in use |

A refused job is recorded with exit code 126.

### What this protects against, and what it does not

The queue lives in a 0700 directory under your AIDA home, never in a
world-writable temp directory, so another user, or any process that can only
write to a shared temp directory, cannot inject a script. Stale and
unattributed jobs are refused, and every job records who asked in a form you
can check against the grant store.

It does **not** stop a process running as you, outside any sandbox, from
writing a job that names a live session's grant: your grant records are
readable by you. Such a process could already edit your shell profile, so a
file queue cannot do better. A sandboxed agent session can write only the
queue's `incoming/` directory, and only when its sandbox names it.

### Files

```text
~/.aida/deputy-shell/<name>/
  incoming/<id>.job        what submit writes (the only place it writes)
  incoming/<id>.cancel     a cancel request
  jobs/<id>/job.json       the submitted record, immutable once claimed
  jobs/<id>/script.sh      the exact script (sha256sum it against job.json)
  jobs/<id>/state.json     state, timestamps, exit code, reason, checks
  jobs/<id>/transcript.log everything the job printed
  acks/<id>                a failed job you acknowledged
  audit.jsonl              every event, append-only
  runner.json              the live runner
  paused                   present while paused
```

`audit.jsonl` records every state change (`queued`, `claimed`, `refused`,
`started`, `finished`, `cancel-requested`, `cancelled`, `orphaned`, `acked`,
`pruned`), the runner starting and stopping, pauses, and each grant refresh.
It is never truncated.

### Grant refresh, mandate, retention

Between jobs, never during one, the runner re-mints its advisor grant once
`--refresh` (default 12h) has passed or when the grant has less than an hour
left, the same way `aida role enter advisor` does. Later jobs run under the
new grant. If the refresh fails, the runner claims no more jobs and exits
when the grant expires; it never runs a job under an expired grant.

If a mandate was active when you started it, the runner stops claiming when
that mandate ends, unless you started it with `--keep-after-mandate`.

Finished jobs are kept for `--keep-days` (or `[deputy_shell] keep_days` in
`.aida/config.toml`; default 30), then removed. Each removal is recorded in
`audit.jsonl` with the script's sha256 and exit code.

### Pause, failed jobs

`aida deputy shell pause` stops the runner claiming new jobs; a running job
finishes. `aida deputy shell resume` works only at your terminal.

`aida awaiting` lists failed script jobs (refusals included, cancellations
not) beside failed command-deputy jobs, each labelled with its kind and the
command that acknowledges it: `aida deputy shell ack <id>` for a script job,
`aida deputy ack <id>` for a command job.

### Letting a sandboxed advisor submit

A sandboxed advisor session can submit only if its sandbox may write the
queue's `incoming/` directory. Add exactly `~/.aida/deputy-shell/*/incoming`
to that session's sandbox write allowlist. Never add the queue's other
directories. A contained lane is never given this path.
