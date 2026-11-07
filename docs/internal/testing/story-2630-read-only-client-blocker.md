# STORY-2630: read-only codegraph client blocker

<!-- trace:STORY-2630 | ai:codex -->

Measured 2026-11-07 19:15 UTC from the native Codex implementer seat at
`34dc5896bbdcdd669f23cedef35888f8a29331dc`. Slice 1 is present; subsequent
round-4 review requested a fix for its CI failure. This investigation concerns
the remaining Slice 2 acceptance items: read-only containment access (AC2)
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

The investigating seat recorded a design-fork punt for STORY-2630 and left
this note uncommitted for handoff. The host subsequently committed it at
`ae96e54ce1a4` and published it on Slice 1's PR #3607. That publication does
not resolve the Slice 2 client decision or clear any punt or merge hold.
The advisor must confirm whether this historical investigation belongs on
the Slice 1 PR or should move to the Slice 2 change; the implementer grants
no publication exception. Unit tests validate Slice 1 behavior; they do not
satisfy AC2 or AC5.

## Round 4: Slice 1 CI regression

The lib-1 log for job `128506720072` identifies the failing test as
`seat_clone::tests::codegraph_runtime_link_is_clean_on_an_older_clone_base`.
Its diagnostic is:

```text
warning: skipping primary codegraph index link: git rev-parse --git-path info/exclude failed: refusing to run git in /tmp/.tmpB762Rz/seats/aida-seat-aida-codegraph: it is a contained seat's private clone, and its git configuration and hooks are writable by the seat
assertion failed: dest.join(".codegraph").is_symlink()
```

This is a deterministic setup defect, not a claimed flake. Runtime-exclude
repair must use the detected clone's real `.git/info/exclude` directly,
without invoking host Git in the clone. Linked worktrees still resolve the
common exclude through Git. The regression now removes the modern clone
exclude to model an older seat, asserts that host Git remains refused, and
checks linking, clean status, preserved ignore rules and idempotent repair.

Round-4 native validation: formatting, `git diff --check`, template-hook
checks, the store-writer inventory and all eight Git-maintenance guard tests
passed. The exact clone regression lane job
`j20261107T195741173-2146033-0` timed out after 900 seconds during compilation;
it executed no tests. At handoff, the broader `codegraph` run
`j20261107T195919096-2173288-0` was running and the core linked-worktree
exclude run `j20261107T195920264-2173350-0` was queued. No Rust test pass is
claimed for this round (`untested=true`). Full `make check-templates` and
`make check-ci-fast` stopped when they invoked local Cargo; host reruns are
needed under this lane's runner-only Cargo rule. The installed doctor still
lacks the codegraph category.

The native seat recorded the scoped host subject
`[AI:codex] fix(codegraph): repair clone excludes and correct blocker handoff (Refs STORY-2630)`
and left all three changed files uncommitted. It punted round-4 finding 3's
note-placement decision to the advisor; that punt remains a publication
veto until resolved. AC2/AC5 and the existing merge hold remain outside this
Slice 1 rework.
