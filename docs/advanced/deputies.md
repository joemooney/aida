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
[CLI reference](../cli/06-roles-sessions.md#aida-deputy). It runs no shell
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

`submit` and `cancel` talk to the running deputy over its socket,
`~/.aida/deputy-shell/<name>/submit.sock`. With no deputy running, `submit`
refuses and tells you to start `aida deputy shell run` first; nothing is left
waiting in a queue.

`submit` refuses unless the session holds a valid seat grant, refuses inside a
script job (a job cannot queue its successor), and refuses a seat the runner
does not accept. Only the advisor seat may submit unless you start the runner
with `--allow-seat`; no setting in the project's tracked `.aida/config.toml`
can add a seat, because any merged change could edit that file.

### What happens to a job

Jobs run strictly one at a time, oldest first. For each one the runner:

1. moves it from `accepted/` (where the deputy itself filed it when the
   submit came in over the socket) into its own `jobs/<id>/` directory, never
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

At submit, the job records the submitter from its **validated** grant record:
session id, seat, subject, principal, grant id, the binding token it
presented, the project and this machine's hostname and boot id. Nothing is
taken from free text.

The deputy then checks the submitting process itself. When `submit` connects,
the kernel tells the deputy which process is on the other end (its pid and
user id). The deputy accepts the job only when:

- that process runs as you;
- that process, or one of its parents, is a process the named grant is bound
  to: the shell or agent launcher it was issued to, or that session's leader.
  A binding recorded in another pid namespace (inside a container) is never
  matched against a process here;
- the process is still the same one when the check ends, so a reused pid
  cannot pass. The deputy holds the kernel's own handle on the process that
  connected (`SO_PEERPIDFD`, Linux 6.5 or later), never one looked up by pid;
  on an older kernel every submit is refused as `refused:unattested`.

The deputy handles each connection separately and gives a client 5 seconds to
send its whole request, so a client that connects and stalls holds up nobody
else. With 16 requests already in progress, the next is refused as
`refused:busy`; try it again.

It records that process on the job as the attested submitter, replacing
anything the client claimed about itself. A submit that fails these checks is
refused at once with its reason (`refused:foreign-uid`,
`refused:not-under-grant`, `refused:grant`), recorded in `audit.jsonl` as
`submit-refused`, and never queued.

Before a job runs, the runner refuses it when:

| Reason | When |
| --- | --- |
| `refused:unattributed` | the job names no grant or no presenter token |
| `refused:unattested` | the job did not come in over the deputy's socket, or the binding it was attested under is not one of the grant's |
| `refused:foreign-host` | it was submitted on another machine, or before this boot |
| `refused:grant` | the grant is missing, expired, revoked, outside the roster, or does not match the job's seat, subject or principal |
| `refused:seat` | the seat may not submit to this deputy |
| `refused:not-live` | no process the grant is bound to is alive, and (for a sandboxed session whose grant is bound only to a session key) the submitting agent session has had no heartbeat in the last 10 minutes from a registry row that holds that grant's session key |
| `refused:schema` | the record has an unknown field, another schema version, a sha256 that is not its script's, or an id already in use |

A refused job is recorded with exit code 126.

### What this protects against, and what it does not

The queue lives in a 0700 directory under your AIDA home, never in a
world-writable temp directory, so another user, or any process that can only
write to a shared temp directory, cannot inject a script. Stale and
unattributed jobs are refused, and every job records who asked in a form you
can check against the grant store.

No session writes into the queue. A job reaches it only through the socket,
and only from a process the kernel reports as running under the grant the job
names. A process that merely knows a live session's grant id (your grant
records are readable by you) cannot submit as that session.

A process running as you, outside any sandbox, can still edit the queue's
files directly, as it could edit your shell profile; no queue can stop that.
For the same reason a cancel over the socket checks only that the caller runs
as you: `cancel` itself refuses another session's job unless you run it at
your terminal, but any unsandboxed process running as you could cancel a job
directly over the socket, or by writing its cancel file. A cancel stops a job;
it never runs anything.

Jobs written into the old `incoming/` directory by a release before the socket
are never claimed. When it starts, the deputy lists any it finds on your
terminal and in `audit.jsonl` (`legacy-incoming-ignored`); `status` and `wait`
report them as unknown. Resubmit them, then delete the directory.

No seat's sandbox is given access to unix sockets, so a contained lane cannot
reach the deputy at all.

### Files

```text
~/.aida/deputy-shell/<name>/
  submit.sock              the running deputy's socket (submit and cancel)
  accepted/<id>.job        a job the deputy accepted, waiting to run
  accepted/<id>.cancel     a cancel request the deputy accepted
  jobs/<id>/job.json       the submitted record, immutable once claimed
  jobs/<id>/script.sh      the exact script (sha256sum it against job.json)
  jobs/<id>/state.json     state, timestamps, exit code, reason, checks
  jobs/<id>/transcript.log everything the job printed
  acks/<id>                a failed job you acknowledged
  audit.jsonl              every event, append-only
  runner.json              the live runner
  paused                   present while paused
```

`audit.jsonl` records every state change (`submit-refused`, `queued`,
`claimed`, `refused`,
`started`, `finished`, `cancel-requested`, `cancelled`, `orphaned`, `acked`,
`pruned`, `legacy-incoming-ignored`), the runner starting and stopping, pauses,
and each grant refresh.
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

A sandboxed session cannot reach the deputy: on Linux its sandbox blocks unix
sockets outright, and the only setting that would allow this one would also
open every other socket on the machine (Docker, the session bus, ssh-agent) to
that session. `submit` from a sandboxed session fails and says so.

An advisor session that runs unsandboxed (started from your own shell) needs
nothing. For a sandboxed advisor session, you may exempt just the two
commands that use the socket by adding them to that session's sandbox
`excludedCommands`:

```json
{
  "sandbox": {
    "excludedCommands": ["aida deputy shell submit", "aida deputy shell cancel"]
  }
}
```

The exempted command runs outside the sandbox but still as a child of the
agent's process, so the deputy's check still finds the grant's process among
its parents. Start the agent from the terminal where you ran
`aida role enter advisor`, so its process runs under that grant. Never add this
exemption for a contained lane, and do not add the queue directory to any
sandbox's write allowlist: nothing needs to write there except the deputy.
