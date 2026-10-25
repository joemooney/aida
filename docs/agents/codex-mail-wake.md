# Wake an existing Codex advisor for new mail

The host adapter `scripts/codex-mail-wake.py` scans AIDA mailbox envelopes and
queues one notification to an explicitly configured existing Codex thread. Quiet
scans do not run a model. This is opt-in; it does not start another advisor or
issue a seat grant. Codex 0.164.0's native `codex queue` is required.

<!-- trace:TASK-2347 | ai:codex -->

## Install and verify

Use an AIDA binary containing `mailbox wake-snapshot`. Before merging a candidate,
keep both that binary and the reviewed adapter in a private release directory
keyed by the reviewed commit; do not promote an unmerged binary globally. After
merging, the ordinary installed AIDA binary is suitable. The adapter's `install`
command copies its script into its private state directory and creates one user
systemd service, with an explicit AIDA executable and Codex thread.

```bash
python3 /path/to/reviewed/codex-mail-wake.py once \
  --project /path/to/project --thread <advisor-thread-uuid> \
  --aida /path/to/reviewed/aida --dry-run

python3 /path/to/reviewed/codex-mail-wake.py install \
  --project /path/to/project --thread <advisor-thread-uuid> \
  --aida /path/to/reviewed/aida
```

Use the exact thread UUID; a role name, `--last`, or a vendor name is not a
conversation identifier. `--codex /absolute/path/to/codex` can pin a non-default
Codex executable. For a systemd service, prefer the native Codex executable,
not a Node shim whose `#!/usr/bin/env node` depends on an interactive shell's
PATH. A locally managed daemon installation may provide the native executable at
`~/.codex/packages/app-server-daemon/current/bin/codex`; verify it locally and
pass its absolute path. Keep candidate executables in the reviewed release
snapshot when testing an unmerged change. This command does not override the target's model, sandbox,
approval policy, grants or mandate. Human-only actions remain human-only.

`install` enables and starts the service. `start`, `stop` and `uninstall` use the
same project/thread arguments. `stop` disables automatic startup too. Uninstall
removes the unit but retains the notification ledger for audit and safe restart.
No cron or model-side recurring wake is installed. A user systemd manager must be
running; normal user-service login/logout lifetime applies unless the operator
separately enables lingering.

The default private state directory is under
`$XDG_STATE_HOME/aida/codex-mail-wake` (or `~/.local/state/aida/codex-mail-wake`).
Use `--state-dir` consistently to override it. State is specific to the canonical
project path and thread UUID. Never delete its permanent lock sidecar while a
watcher may run.

```bash
python3 /path/to/reviewed/codex-mail-wake.py status \
  --project /path/to/project --thread <advisor-thread-uuid>
```

Status reports the ledger and whether another process holds the watcher lock;
`last_acceptance` means **Codex accepted a queue command**, not that the model
ran or mail was acknowledged. The advisor must explicitly read/ack messages.

## Delivery and recovery

The first scan includes existing unread mail. It is never silently baselined as
already delivered. Mail discovered within the batch window (default five
seconds) is combined, up to 64 IDs per notification. The service checks every 30
seconds; `--interval` and `--batch-seconds` change these bounds. Empty or unchanged
unread inventories produce no new Codex turn. A first `once` can persist a
pending batch without submitting until a later scan; use `--batch-seconds 0` for
an immediate one-shot submission.

Git's NUL-delimited worktree inventory identifies sibling checkouts. Each local
mailbox is inspected using the version-1 AIDA `mailbox wake-snapshot` interface
with the main project as the AIDA subprocess working directory. The envelope
contains only unread message IDs and does not move read watermarks. The adapter
keeps all source checkout paths for each ID so the advisor can find mail that
has not yet been published from a sibling. Replicated IDs cause one notification.
New or removed checkouts are discovered at each scan. Failed scans do not record
acceptance and retry at the next interval. Scans have per-command and total time,
output, entry and message bounds.

Successful queue submissions are committed atomically under a permanent exclusive
lock. Failure to create the queue process is known not to have submitted anything
and is retried with a delay capped at five minutes. A failed or timed-out process,
or a crash between queue acceptance and ledger commit, has an **uncertain result**:
the adapter stops rather than silently duplicating an expensive model turn. Inspect
the target conversation, stop the service if necessary, then choose explicitly:

```bash
# The notification is already present in the target conversation:
python3 /path/to/reviewed/codex-mail-wake.py recover \
  --project /path/to/project --thread <advisor-thread-uuid> --accepted

# It is absent; retrying is appropriate (a duplicate remains possible):
python3 /path/to/reviewed/codex-mail-wake.py recover \
  --project /path/to/project --thread <advisor-thread-uuid> --retry
```

Then start the service again. Recovery changes the adapter's delivery ledger,
never AIDA's inbox read state. Queueing and local persistence are not a single
transaction, so this adapter does not claim exactly-once delivery. A ledger that
fills its bound refuses further submissions rather than forgetting delivered IDs.

## End-to-end evidence

Fixture tests use temporary repositories, a fake AIDA envelope command and a
fake Codex queue executable. They prove discovery, deduplication, bounded process
handling, locking, batching, persistence and recovery without invoking a model or
reading real mail.

The live acceptance test is separate and requires the operator's authorization:
create a synthetic AIDA advisor message, allow the target thread to become idle,
and verify that its next actual turn reports the message ID in that same thread.
Record both the queue acceptance receipt and the observed turn receipt. Subsequent
scans must not queue that ID again, even if the advisor has not acknowledged it.
A successful `codex queue` exit alone proves only queue acceptance. Existing
unread backlog should be handled before isolating this test's new ID.
