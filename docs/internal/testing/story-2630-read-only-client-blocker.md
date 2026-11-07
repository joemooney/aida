# STORY-2630: read-only codegraph client blocker

<!-- trace:STORY-2630 | ai:codex -->

Measured 2026-11-07 19:15 UTC from the native Codex implementer seat at
`34dc5896bbdcdd669f23cedef35888f8a29331dc`. Slice 1 is present. This
investigation concerns the remaining Slice 2 acceptance items: read-only
containment access (AC2)
and a successful lane call (AC5). Neither is claimed complete here.

## Reproduction and observations

The seat had no `.codegraph` entry on entry. Created only the ignored runtime
symlink to `/home/joe/ai/aida/.codegraph`; no index was built or copied, and no
Git metadata was changed. From the seat directory:

```sh
codegraph callers link_worktree_runtime_state --json
codegraph callers link_worktree_runtime_state -p /home/joe/ai/aida --json
```

Both calls fail with `callers failed: unable to open database file` using
installed codegraph 1.6.0. A plain file read succeeds and returns the SQLite
header. Header bytes 18 and 19 are both `2`, identifying WAL format. Both Python
SQLite `mode=ro` and `mode=rw` connections fail when querying the files table.
Thus readable database bytes alone do not establish a working SQLite query.

Tracing the first command with `strace -f -e trace=openat` shows:

```text
codegraph.db     O_RDWR|O_CREAT|O_NOFOLLOW|O_CLOEXEC = -1 EROFS
codegraph.db     O_RDONLY|O_NOFOLLOW|O_CLOEXEC      = 17
codegraph.db-wal O_RDWR|O_CREAT|O_NOFOLLOW|O_CLOEXEC = -1 EROFS
codegraph.db-wal O_RDONLY|O_NOFOLLOW|O_CLOEXEC      = -1 ENOENT
```

The primary directory contained only `codegraph.db` at inspection time. The
trace excerpt above is evidence that the current sandbox denies writes as
intended; it is not successful codegraph evidence. The relevant open calls
are included here so this note does not depend on a host-local trace file.

## Installed client behavior

Inspected the installed package under
`~/.codegraph/versions/v1.6.0/lib/dist/`. The `callers` action in
`bin/codegraph.js` uses `CodeGraph.open`. `db/index.js`'s
`DatabaseConnection.open` opens the default writable adapter, configures WAL,
checks/runs migrations, and invokes index/WAL healing. The adapter has an
internal `readOnly` option, but this public query path does not select it.
There is no read-only CLI option in `codegraph callers --help`.

## Decision required before Slice 2 continues

Prefer a supported, WAL-safe read-only codegraph client that never migrates or
heals the shared index during queries. Decide whether to obtain that upstream
or authorize a maintained AIDA query adapter and its compatibility contract.
The host must also supply a WAL layout that supports read-only readers while
its sync timer remains active. Merely selecting SQLite read-only mode does not
fix the observed missing-WAL case.

Do not grant writable DB/WAL/SHM access to make the existing client pass AC5:
that would contradict AC2. Do not mark an actively synced database immutable,
copy a per-lane index, patch the globally installed client from a lane, or
change host timer/index settings from this seat. A symlink is routing only;
non-contained seats still inherit the host's ordinary filesystem permissions.

No containment or launcher code was changed by this investigation. In
particular, `launch/spawn.rs` and `launch/profile.rs` remain untouched. The
installed `aida doctor check codegraph` still rejects the category because
that installed binary predates Slice 1; this is distinct from the client
failure reproduced above.

## Validation and handoff

- `cargo test --profile agent -p aida-cli-lib codegraph::tests -- --nocapture`:
  seven tests passed, including runtime linking, reused-seat excludes, prompt
  invocation preservation, template equality, and doctor probes.
- `make check-ci-fast`: all 43 fast gates passed, including formatting.
- `make check-templates`: passed; absent portable `.agents` pack is a clean
  no-op for this clone. No templates were changed in this investigation.
- `git diff --check`: passed.

The host published this investigation at `ae96e54ce1a4` on Slice 1's PR
#3607. That publication does not resolve the Slice 2 client decision or clear
any punt or merge hold. Unit tests validate Slice 1 behavior; they do not
satisfy AC2 or AC5.

## Slice 1 clone exclude regression

Runtime-exclude repair in a private clone must use its real `.git/info/exclude`
without invoking host Git. Linked worktrees resolve the common exclude through
Git. The regression test models an older seat by removing the modern clone
exclude, asserts that host Git remains refused, and checks linking, clean
status, preserved ignore rules and idempotent repair.

Clone metadata is seat-writable: the host pins real `.git` and `info`
directories, refuses non-regular excludes, reads with a bounded no-follow
handle, and replaces the exclude through an exclusively created no-follow temp
file in the pinned directory. Unsafe metadata skips optional navigation setup.
The refusal tests cover an exclude symlink to an outside secret and an info
symlink to an outside directory, checking that the secret is not copied and
that nothing is written outside the clone.

The prior exclude repair was published at `02d9c1304b46`; its durable CI
evidence is [run 42410096051](https://github.com/joemooney/aida/actions/runs/42410096051).
Per-round validation, launcher handoff and publication state belong in
STORY-2630 comments and the PR trail, rather than this investigation note.
