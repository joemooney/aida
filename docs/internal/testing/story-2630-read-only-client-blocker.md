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

## Acceptance status and remaining ownership

- AC1: primary-index routing is present on main in
  `codegraph::primary_index_link` and `worktree_gc::link_worktree_runtime_state`.
  Setup links the existing index without copying it. A successful runtime query
  from a seat remains unverified; routing alone does not establish AC2 or AC5.
- AC2: not met. No codegraph-specific containment read grant is implemented.
  The supported read-only client and WAL-layout decision above remains required;
  the routing symlink grants no read-only boundary.
- AC3: main already carries the navigation guidance in the AGENTS.md template
  and shared lane prompt, with template-equality and assembled-prompt coverage.
- AC4: doctor probes missing tools, indexes older than one hour and unreachable
  indexes. Diagnostics require an explicit `.codegraph` ignore rule in the
  invoking or primary checkout's `.gitignore`, so unrelated projects stay silent.
  Missing tool plus absent index produces one setup finding; broken links and
  access errors retain reachability diagnostics.
- AC5: open because no successful read-only seat query has been demonstrated.
  The installed client's writable-open/WAL behavior described above blocks that
  evidence even when the executable and routing link are available.

STORY-2630 owns the further slice for AC2 and AC5, including host provisioning,
the supported read-only client decision, containment read admission and a
successful seat-call log. This PR uses `Refs STORY-2630`; it does not complete
those acceptance items. Per-round command output, validation and publication
handoff belong in the PR body and STORY-2630 comments.

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
