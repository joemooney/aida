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
command copies its script into its private state directory, creates one user
systemd service, and registers one existing AIDA substrate health check with its
advisor failure route. Use Python 3.11 or newer for installation. The AIDA binary
must also contain `doctor check codex-mail-wake`; do not install an older candidate.
The service uses an explicit AIDA executable and Codex thread.

```bash
python3 /path/to/reviewed/codex-mail-wake.py once \
  --project /path/to/project --thread <advisor-thread-uuid> \
  --aida /path/to/reviewed/aida --dry-run

python3 /path/to/reviewed/codex-mail-wake.py install \
  --project /path/to/project --thread <advisor-thread-uuid> \
  --aida /path/to/reviewed/aida
```

Use the main checkout as `--project` for installation; linked-checkout installs
refuse before deployment because scheduled doctor checks resolve the main project.
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
The `health` object reports `accepted_unacknowledged_count`,
`accepted_unknown_age_count`, `oldest_known_notification_age_seconds` (null when
no age is known), and `recovery_required`. These describe persisted evidence as
of the last successful scan; status itself does not scan or acknowledge inboxes.
Notification age starts at the recorded queue submission attempt, including an
attempt later confirmed with `recover --accepted`. Legacy accepted IDs without
per-ID timestamps retain unknown ages; the last batch's time is never assigned
to unrelated IDs. The installed substrate health check consumes a bounded counts-only snapshot
written alongside the ledger. It does not automatically requeue mail.

After a complete successful scan, accepted IDs and their timestamps are pruned
only when AIDA supplies acknowledgement evidence for the same logical advisor.
Accepted unread IDs remain suppressed across restarts, missing inventories and
removed worktrees. Failed scans retain them. Read-and-accept cycles therefore
do not accumulate notification history indefinitely; genuinely unread accepted
mail remains visible until acknowledgement evidence arrives.

Acknowledgement reconciliation uses evidence available from current mailbox
roots. If the only root holding a read receipt disappears after pruning, a stale
unread replica in another root can become eligible again. The adapter does not
retain permanent read tombstones or write shared acknowledgement state; durable
logical-inbox reconciliation belongs to AIDA's mailbox store. Preserve receipt
roots when retiring worktrees until that shared reconciliation is available.

## Owned scheduled health check

`install` appends an exact project/thread-owned job block to `.aida/config.toml`
and writes private `.aida/codex-mail-health.json` registration data. It registers
`doctor check codex-mail-wake --fail-on-findings` every 15 minutes plus an advisor
seat route keyed only to that job's `CronJobFailed` event. The existing scheduler
marks the advisor job due; it does not launch a model or submit a Codex wake.
Already-due failures stay one due job. Successful recovery is visible in the
substrate job's normal schedule status and in the doctor check.

Both derived job names must be free in the effective global registry as well as
the project registry. The installer reads `$AIDA_HOME/.aida/schedule.toml` when
`AIDA_HOME` is nonempty, otherwise `~/.aida/schedule.toml`, accepting native
`[schedule]` and bare `jobs`/`tasks` shapes. Even disabled global name collisions
refuse installation, idempotent installation and removal before either local
registration file changes. Unsafe, unreadable, oversized or malformed global
registries also refuse; global configuration is never modified. Captured global
bytes are validated with the selected AIDA binary's read-only `schedule list` in
a disposable project and HOME, preserving native cadence/event/predicate checks
without running jobs or rereading the live global path.

An existing AIDA scheduler driver must be running with a binary containing this
health category for unattended checks. `--aida` pins the watcher subprocess; it
does not repoint an existing scheduler driver. The advisor must verify the reviewed
driver binary before enabling the job, using the ordinary driver deployment path.
Verify with `aida schedule status`; if none is installed, the operator uses the existing
`aida schedule install-systemd` or `install-cron` workflow. The watcher never
installs another timer or replaces an existing driver.

The check fails visibly for uncertain delivery, any accepted legacy IDs with
unknown ages, accepted unread IDs aged at least one hour, or an unavailable/stale
snapshot. `--health-max-age <seconds>` at install configures the accepted age
limit (60–604800 seconds; default 3600). Snapshot freshness allows the larger of
15 minutes or three scan intervals. Neither clock nor health failures infer a
mail acknowledgement. Diagnostics contain counts, ages and recovery state;
no message IDs, subjects or bodies are included.

```bash
aida doctor check codex-mail-wake --json --fail-on-findings
aida schedule due --seat advisor
```

Installation is idempotent only for the exact owned registration and unchanged
job block. A user job with either owned name, another target registration, or an
edited owned block refuses before replacement. To change the configured age,
uninstall the owned setup first, then reinstall with the desired threshold.
Repeat the original `--state-dir`, `--interval`, `--batch-seconds`, `--aida`,
`--codex` and `--health-max-age` settings, using the same Python executable,
when uninstalling; edited or mismatched registrations refuse rather than guessing.
`uninstall` removes only that exact block and registration, retaining other jobs,
notification state and historical scheduler evidence. The permanent health
install lock is retained. `stop` leaves the health check armed, so a stopped
watcher eventually reports unavailable; use `uninstall` to retire its monitoring.
Runtime registration and snapshots belong in ignored per-clone state, as in
AIDA's default `.aida/*` ignore policy. No live check is installed by fixture tests.


### Install failures and tracked configuration

Here `.aida/config.toml` is tracked. Installation adds only the exact owned job
and route block; it never stages, commits or publishes that diff. Before rollout,
review `git diff -- .aida/config.toml` and the exact planned block. Deployment still
requires the ordinary reviewed health-capable scheduler driver; selecting watcher
`--aida` does not upgrade that driver.

Install first performs read-only main-checkout, project/global ownership, selected
AIDA health-category, existing file identity and systemd state checks. A running
watcher/service must be stopped before reinstalling; its monitoring remains armed.
A conflict or capability refusal leaves original files unchanged. Publication
rechecks file identity/bytes and exact ownership, including the global namespace.

Before replacing the script or registering jobs, install saves a private
`install-recovery-<uuid>.json` under the mode-0700 state directory. Each mode-0600
bundle contains the original config, registration, unit and script bytes (base64),
their modes and SHA-256 hashes, original service state, the attempted stage, and
last safely observed file snapshots. **Treat the bundle as private**: config may
contain credentials. Do not paste it into mail, PRs or logs. Notification ledgers
are never rolled back or reset by this process.

Installation is not transactional. Failure at `script-deploy`,
`health-registration`, `unit-write`, `daemon-reload` or `enable` reports the exact
stage and recovery bundle path. Partial owned jobs/registration/unit may remain;
enable failure can leave service state uncertain. Original bytes stay in the
bundle, and unrelated/intervening changes are not automatically overwritten.
If saving the final observations fails, the original pre-change bundle remains
and the command reports the evidence failure.

After fixing the reported cause, retry the **same** install arguments and Python executable (including
state directory, interval, batch seconds, pinned AIDA/Codex paths and health max age). It accepts only the unchanged exact
owned registration/job block and exact service unit. If those were edited or a
name was reused, it refuses: inspect the diff and private evidence rather than
forcing replacement. The retry preserves unrelated jobs added after the failure.
To retire a partial installation instead, use the same target settings:

```bash
systemctl --user show "$SERVICE" --property=ActiveState --property=UnitFileState --property=LoadState
python3 /path/to/reviewed/codex-mail-wake.py uninstall \
  --project "$PROJECT" --thread "$THREAD" --state-dir "$STATE" \
  --interval 30 --batch-seconds 5 --health-max-age 3600 \
  --aida /path/to/reviewed/aida --codex /path/to/original/codex
git diff -- .aida/config.toml
```

`uninstall` validates ownership, runs `disable --now`, then independently checks
for inactive/failed service state **before** removing monitoring. Stop failure or
unknown/still-active state retains the health job, registration, unit and state;
repair service control, inspect `show`, and retry that same uninstall. A unit-write
failure can require repairing/retrying install first to create a controllable unit.
After confirmed stop, removal repeats exact ownership checks and removes only the
owned block, retaining intervening unrelated config and the recovery bundles.
A failure after removal (for example daemon-reload) is a stopped partial uninstall;
retry it after repairing the cause. Never use `git restore`, overwrite the whole
config from a backup, or auto-commit a tracked config to resolve partial install.
Any manual restoration requires comparing current hashes against the recorded
attempt and preserving unrelated edits; an unknown observation is not permission
to overwrite. Keep the private evidence until the operator accepts recovery.


## Delivery and recovery

The first scan includes existing unread mail. It is never silently baselined as
already delivered. Mail discovered within the batch window (default five
seconds) is combined into a notification bounded to 24 KB and at most 1,024 IDs.
The byte budget packs an ordinary historical backlog into one wake where possible,
rather than waking the model for many arbitrary small chunks. The service checks every 30
seconds; `--interval` and `--batch-seconds` change these bounds. Empty or unchanged
unread inventories produce no new Codex turn. A first `once` can persist a
pending batch without submitting until a later scan; use `--batch-seconds 0` for
an immediate one-shot submission.

Git's NUL-delimited worktree inventory identifies sibling checkouts. Each local
mailbox is inspected using the version-1 AIDA `mailbox wake-snapshot` interface
with the main project as the AIDA subprocess working directory. The envelope
contains unread message IDs and existing acknowledged IDs, without moving read
watermarks. Existing per-ID receipts are shared across these roots for the same
advisor. A root watermark acknowledges only messages actually present in that
root; it never suppresses unknown private mail from another checkout. The adapter
keeps all source checkout paths for each ID so the advisor can find mail that
has not yet been published from a sibling. Replicated IDs cause one notification.
New or removed checkouts are discovered at each scan. Failed scans do not record
acceptance and retry at the next interval. Scans have per-command and total time,
output, entry and message bounds. Receipt reads overlap file latency with at most
four guarded readers under the same two-second receipt deadline. Every explicit
receipt ID is retained, including IDs whose messages exist only in another root;
any unsafe, malformed, missing or late read refuses the complete snapshot. Tight
remaining byte budgets use sequential actual-byte accounting. No receipt cache,
baseline or read-watermark change is introduced.

Successful queue submissions are committed atomically under a permanent exclusive
lock. Failure to create the queue process is known not to have submitted anything
and is retried with a delay capped at five minutes. A failed or timed-out process,
or a crash between queue acceptance and ledger commit, has an **uncertain result**:
the adapter stops rather than silently duplicating an expensive model turn.
It remains paused until explicit recovery: systemd restarts and manual `run` or
`once` attempts refuse the persisted inflight submission without queueing it.
`status` remains available and reports `recovery_required: true`. Inspect
the target conversation, stop the service if necessary, then choose explicitly:

```bash
# The notification is already present in the target conversation:
python3 /path/to/reviewed/codex-mail-wake.py recover \
  --project /path/to/project --thread <advisor-thread-uuid> --accepted

# It is absent; retrying is appropriate (a duplicate remains possible):
python3 /path/to/reviewed/codex-mail-wake.py recover \
  --project /path/to/project --thread <advisor-thread-uuid> --retry
```

`recover --dry-run` is rejected before state-directory creation or locking; use
`status` for read-only inspection. Both normal recovery choices remain explicit.
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
