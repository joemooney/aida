# STORY-2630: read-only codegraph client blocker

<!-- trace:STORY-2630 | ai:codex -->

Measured 2026-11-07 19:15 UTC from the native Codex implementer seat at
`34dc5896bbdcdd669f23cedef35888f8a29331dc`. Slice 1 is present and PR #3607's
round-3 review approved it. This investigation concerns the remaining Slice 2
acceptance items: read-only containment access (AC2) and a successful lane call
(AC5). Neither is claimed complete here.

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
trace is evidence that the current sandbox denies writes as intended; it is
not successful codegraph evidence. The untracked local trace is at
`/tmp/story-2630-codegraph-open.trace` for this process's environment.

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

`aida punt STORY-2630 --category design-fork` recorded the client decision and
set the spec to Needs Attention. That punt is a publication veto. This note is
left uncommitted; no commit, push, PR creation, queue completion, merge, or
review verdict was performed by this seat. Unit tests validate the existing
Slice 1 behavior; they do not satisfy AC2 or AC5.
