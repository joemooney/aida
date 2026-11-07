# Recovering a quarantined canonical transaction

<!-- trace:TASK-3223 | ai:codex -->

The redo journal is local to the **store worktree**, at `.aida/redo/`.
A durable `.pending` decision is replayed on the next outermost store-lock
acquisition. A failed precondition, corrupt record or unsupported format stops
writers with `OutcomeUnknown`. The diagnostic names the affected record
(`.unknown` for a quarantined decision, `.pending` or `.done` for uncertain
I/O/reply failures), or the journal directory for a format/directory failure,
and this guide.
This fence applies to ordinary CLI writers too: they must not overwrite an
accepted transaction whose outcome is uncertain.

`comment_add` and `spec_edit` have staged typed schemas and core transaction
tests, but the live service and host refuse them with `UnsupportedOperation`.
They remain disabled until a publication slice durably commits the installed
objects and records the corresponding events before acknowledging success.
`own_claim` and `queue_done` also remain unsupported. Do not enable any of these
verbs to work around a quarantine.

## Inspect without changing the store

Stop launchers and store writers. Use the exact store root and diagnostic path;
these are not the code checkout's `.aida/` directory. Copy the journal and
`objects/` tree to a private backup outside the store. Preserve pending, done,
and unknown records, the `format` file, Git HEAD and `git status --short` output.
Do not reset, rebase, pull or remove the journal to bypass the fence.

A version-1 record is JSON with `checksum` and `decision`. The checksum is the
SHA-256 of Rust's compact serialized decision. Each decision identifies its
binding, launch epoch, request ID, payload digest, exact result and images.
Each image contains a spec ID, a SHA-256 `before` hash and complete YAML `after`
bytes. Compare canonical bytes with these values and independent committed
history/backups. A partial install can have some before images and some after
images. Recovery installs the recorded after images; it never repeats comment
or edit logic. A valid `.done` record proves installation, not Git publication.

## Resolve as the host operator

Hold the existing advisory lock for the **entire** inspection, repair and
journal transition. One way to obtain an interactive maintenance shell is:

```bash
flock -x /absolute/store/.aida/store-write.lock bash --noprofile --norc
```

Keep that shell open while repairing; do not run an AIDA writer inside it,
because that separate process would wait for your lock. Never unlink the lock
sidecar. Work on the backed-up evidence first.

* For a valid version-1 decision whose canonical precondition was disturbed,
  restore each affected object to independently verified bytes matching either
  its `before` hash or exact `after` bytes. Check **all** images, not just the
  first refused object. Restore safe regular-file destinations if a link caused
  the refusal. Rename only that decision from `.unknown` to `.pending`, keeping
  its original `<epoch>-<request-id>` basename. Fsync repaired files and their
  parent directories, and fsync `.aida/redo` after the rename.
* For a damaged record, recover its exact original bytes from a trusted backup,
  then follow the preceding procedure. Never invent a checksum, result or image.
  An unsupported format needs the matching AIDA version or restoration of the
  original journal, not an edited version marker.
* If the original decision cannot be recovered, explicitly reconcile every
  affected object and caller outcome using independent history/backups. Record
  the operator's decision and preserve the original evidence outside the active
  journal. Only after that reconciliation may the operator move the quarantined
  record into a private recovery archive outside `.aida/redo`, fsyncing both
  directories. This is a manual disposition, not a successful replay; the old
  request must never be retried or reported as completed from that archive.
  If the affected objects or outcome cannot be established, keep the fence.

Exit the maintenance shell to release the lock. The next ordinary authorized
store write performs recovery before its own operation; a valid pending record
becomes `.done`. Confirm object bytes and the durable result, rebuild the cache
with `aida cache rebuild`, and separately reconcile/commit any installed bytes
through the host's supported store-publication workflow before syncing. If
another record is refused, repeat inspection for the newly named file. Keep
the backup and operator disposition as the recovery audit trail.

Completed retry evidence is retained. Admission quotas apply separately to
each launch epoch, so an ended epoch filling its 4096-record quota does not
block a fresh launch. Records are not automatically deleted to recover capacity.
The cache records the redo generation separately from the real Git HEAD; a
changed generation forces a full refresh once, while subsequent HEAD changes
can use incremental refresh and ordinary writes can restamp the cache.
