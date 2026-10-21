# BUG-2169: give each contained seat a private clone (architecture sketch)

Status: APPROVED 2026-10-16 (option A, slices S1-S4). S1 landed in 2177eaad8f, S2 in b66e4d93b8.
S3 (host-safe lifecycle) is tracked on TASK-2053; see "S2 as built" and "S3 as built" at the end.
Owning spec: BUG-2169. Supersedes the PR #2694 approach, which the round-2 ruling rejected.
Code references are to `main` at fc39d8f0bc. Paths are relative to `aida-cli-lib/src/` unless they
start with `aida-core/`.

## 0. Problem and ruling in one paragraph

A contained seat runs in a linked worktree of the main checkout. To run git it needs writes into
the shared common gitdir: `objects/`, `refs/`, `logs/` and `packed-refs`. PR #2694 granted those
writes, then denied single files (`refs/heads/main` and the others) with read-only binds. Under
bwrap 0.9.0 a read-only bind on a file inside a writable directory is not a boundary:

- **Rename aside:** `mv refs/heads h.x; mkdir refs/heads` gets around the bind.
- **Host detach:** a host-side lock-then-rename of `main`, or `pack-refs --prune`, replaces the
  bind source, which detaches the mount.

The ruling is that no seat may get write access to the shared `refs/`, `logs/` or `objects/` by
allowlisting with file-level carve-outs. **Recommendation: option A, a private clone per contained
seat.** Option B is compared in §9.

## 1. Where the clone lives and who creates it

**Location.** A sibling of the pool trees under `[worktree_pool] worktree_parent` (today
`~/ai/aida-worktrees`): `aida-seat-<slug>-<N>/`. It is a full `git clone`, so its `.git/` is a
real directory owned by the seat, not a `gitdir:` pointer into `/home/joe/ai/aida/.git`.

**Creation command.** The host-side launcher runs this before any sandbox exists:

```
git clone --reference <main> --no-checkout <origin-url> <seat-dir>
git -C <seat-dir> checkout -b <branch> <base-sha>
```

- `<origin-url>` is main's `remote.origin.url`.
- `<base-sha>` is resolved in MAIN, by the same `furthest_ahead_default_ref` logic as today.
- If the clone's fetch does not have `<base-sha>`, the launcher runs `git -C <seat-dir> fetch
  <main> <base-sha>`. That is a host fetch FROM main, which is trusted, into a clone no seat has
  touched yet.

Measured here: 1.7 s and 57 MB total; `.git/objects` is 16 KB plus an alternates line pointing at
`/home/joe/ai/aida/.git/objects`.

**Owner.** Host-side launcher code, before the sandbox starts:

- `session_start` (session_start.rs:1109). In place of `acquire_session_pool_worktree`
  (session_start.rs:1021) or the `git worktree add` fallback (session_start.rs:~1883-1905), when
  the launch posture is contained, it calls a new `seat_clone::create(project_root, branch,
  base_sha) -> SeatClone`.
- This holds for every contained entry point:
  - `queue work`: session_start at queue_cmd.rs:12483, then chdir at queue_cmd.rs:12762.
  - `agent new`: `prepare_agent_launch_with_cwd`, agent_launch.rs:3384, reached from both
    agent_launch.rs:2557 (foreground) and agent_launch.rs:2796 (`--bg`).
  - Drains and phase children: phase_driver.rs:2276/2291.
- Posture is known before session_start runs: `resolve_claude_new_posture` (agent_launch.rs:2033)
  and `resolve_queue_work_posture` (queue_cmd.rs:11480). Pass a `TreeKind::{Worktree, Clone}` into
  session_start instead of re-deriving it there.

**Host-side runtime links.** `link_worktree_runtime_state` (worktree_gc.rs:1030) links `.aida/`
into the new tree exactly as it does today: `cache.db*`, `sessions/`, `agents/` and `pgdata` point
at main. `.aida-store` points at main's store, as in the pool tree today. The launcher also
writes a host-owned marker `.aida/seat-clone.toml` with `main = <main path>`, `branch`,
`base_sha`, `origin_url` and `created_by_pid`. See §2 for why it is a hint and not authority.

**Base pin.** The host writes `refs/aida/seats/<seat-name> = <base_sha>` in MAIN, so that main's
`gc` can never prune objects the clone borrows through alternates. See §6, row "alternates".

**Hard precondition: the launcher must be unsandboxed.** The launching `aida` process may itself
be running inside a contained parent's Bash sandbox, for example a contained advisor relay
running `aida agent new --bg`. That process cannot create a directory under `worktree_parent`. A
`TreeKind::Clone` creation that hits a read-only file system refuses with a clear message, and
never falls back to a worktree. **Open question O1 for the advisor:** in the 2026-10-16 incident,
was the relay's `aida agent new` running unsandboxed? It must have been, since it wrote the pool
lease and ran `git checkout -b` in the shared refs. If yes, nothing more is needed. If not, slice
S5 adds a host-provisioned clone pool, refilled by the uncontained maintenance schedule
(STORY-1699 heartbeat), so the contained launcher only leases.

**Pre-existing defects to fix in the same lane** (found while tracing):

- `tool_contained_flags("claude")` (agent_launch.rs:1646, used at :2063) resolves write roots from
  the LAUNCHER's cwd (`contained_session_git_root`, session.rs:1463). It does this before the lease
  tree exists. A seat launched from the main checkout therefore gets the main checkout as its
  `--settings` boundary. With clones it must be `claude_contained_flags_for(&clone)`, which is what
  resume already does (agent_launch.rs:1289).
- `build_agent_new_bg_command` (agent_launch.rs:4685) and the interactive `exec_claude`
  (session.rs:1025-1056) never apply `os_wrapped_program_and_args`. Only foreground `agent new`
  (agent_launch.rs:4340/4350) and headless paths (session.rs:1283, :3163) wrap.

**New sandbox boundary for a clone seat:**

| Path | Seat access | Mechanism |
|---|---|---|
| `<seat-dir>/**`, including its own `.git/` | rw | `contained_write_roots(clone)` (contained_boundary.rs:48), unchanged |
| `<main>/.aida-store`, `<main>/.aida/cache.db*` | rw | BUG-1929 `contained_store_write_paths`, unchanged; main comes from the lease, not from git (§2) |
| `<main>/.git/**`, the whole common gitdir | **ro**, read-only root, no allowWrite entry | nothing to carve out, so nothing to bypass |
| `<main>/target/worktrees/<seat-dir-name>` | rw | new allowWrite and bwrap `--bind` (§3) |
| `$TMPDIR`, `~/.cargo`, `~/.claude*` | rw | unchanged (session.rs:3201-3214) |

Round 1's config/hooks/info/pointer denies (`contained_store_deny_write_paths`,
contained_boundary.rs:162) are kept. They still bind roots the seat cannot write.

## 2. Every path that assumes "worktree of main", and how it changes

Legend: **C** = breaks or does the wrong thing with a clone; **S** = silently skips (fails safe,
but the clone becomes invisible). Every S becomes a positive handling, because invisible seats
are how unshipped work gets lost.

### 2a. Resolving "the main checkout" (the root problem)

- worktree_gc.rs:725 `main_worktree_root_from`. The first record of `git worktree list` inside a
  clone is the clone itself, and it has about 78 call sites. **C.**
  - Change: resolution order becomes (1) `git worktree list`, if the first record differs from
    `start`; then (2) `.aida/seat-clone.toml` `main`, accepted only if that path's
    `.git/objects` equals the clone's `objects/info/alternates` line AND `<main>/.aida/sessions`
    has a lease whose `worktree_path` is the clone; then (3) `start`.
  - Inside the sandbox this is a convenience. A forged marker only misdirects the seat's own
    writes, which the sandbox bounds anyway.
  - Host-side callers that act on a seat (reap, end, ship-from-host) never ask the tree. They
    take `parent_project_root` from the lease (`SessionLease`, leases.rs:8, already has
    `parent_project_root`).
- `--git-common-dir`-parent helpers, which fail the same way. Each switches to the shared
  resolver above:
  - worktree_scope_gate.rs:261-266 (S)
  - worktree_reclaim.rs:438-443 (C)
  - identity_gate.rs:134/162
  - internal_cmd.rs:156-161
  - session_reap.rs:1108
  - dev_build.rs:323
  - rebase_cmd.rs:206-244 / :234 (C: `aida rebase` would target the wrong checkout)
- session_end.rs:966-977 `resolve_session_end_root` treats a `.git` directory as the project root.
  **C.** It should use the lease's `parent_project_root`.

### 2b. Creating, re-entering and resetting trees

- session_start.rs:1021 (pool acquire), session_start.rs:~1883-1905 (`worktree add -b`),
  worktree_gc.rs:1318/1349/1372 (idempotent add). For `TreeKind::Clone` all of these route to
  `seat_clone::create`; worktree seats are unchanged.
- worktree_gc.rs:1609 `is_linked_worktree` (the BUG-2143 lease re-entry gate) never treats a clone
  as linked. **C.** Accept `TreeKind::Clone` when the lease records it and the alternates line
  matches main.
- session_start.rs:1222 `reuse_existing_worktree_path`. `--path` on an existing clone bails.
  **C.** Same fix as the re-entry gate.
- aida-core/src/worktree_pool.rs:515/756 and :606/:787. `furthest_ahead_default_ref` returns a
  ref NAME, and `reset_worktree_to` (aida-core/src/git_ops.rs:2581) resolves it inside the tree.
  In a clone that is the clone's own stale `origin/main`. **C, silent.**
  - Clones are not pooled in S1-S4, so they are never reset.
  - If S5 adds a clone pool, reset passes a SHA resolved in main and fetches it into the clone
    before `checkout --detach --force <sha>`.
  - The worktree pool also improves by passing a SHA rather than a name.

### 2c. Reap, stop and end

- session_end.rs:1675 `git worktree remove --force` fails on a clone: it warns, the directory is
  left, and the lease is released anyway. **C.** It becomes `seat_clone::destroy` (§4).
- session_end.rs:1036-1037 commits-ahead and `classify_session_end_unshipped_work`
  (session_end.rs:720) read main's `refs/heads/<b>`, find nothing and return 0, so the
  unshipped-work warning is suppressed. **S, dangerous.** It becomes the clone unshipped probe
  (§4).
- session_reap.rs:569 `branch_checked_out_elsewhere` (S), session_reap.rs:654-700
  `resolve_local_branch_tip` (S: never provably merged, never reaped), and session_reap.rs:1225
  `update-ref -d refs/heads/<b>` (S: no-op). For a clone lease, reap judges merged-ness against
  `refs/remotes/origin/<b>` plus the forge PR state, and its dirty/unpushed check is the probe in
  §4.
- worktree_gc.rs:31/155/241/354 (gc candidates, locks), worktree_gc.rs:3212 (`aida worktree list`),
  worktree_reclaim.rs:570 (target reclaim), worktree_dismiss.rs:49-115 (dismiss),
  aida-core/src/git_ops.rs:~2400 `list_worktree_paths` (runaway_seats.rs:216, doctor_root.rs:639,
  worktree_pool_adopt.rs:319), and status_prs.rs:696 `list_worktrees` (brief.rs:435,
  doctor_root.rs:1018, doctor_cmd.rs:6107/6402, status_sections.rs:343, status_awaiting.rs:41).
  All are **S**: clones are invisible.
  - Change: add one enumerator, `seat_trees(project_root) -> Vec<SeatTree{path, branch, kind}>`.
    It merges `git worktree list` with the clone leases in `.aida/sessions/*.toml` (`list_leases`,
    leases.rs:567) and with an orphan scan of `aida-seat-*` directories whose marker names this
    main.
  - Each listed caller switches to it.
  - Operations that need git state inside a clone use the probe (§4) instead of `git -C`.
- phase_driver.rs:2449 `unique_worktree_on_branch` (pr_lookup.rs:616) and phase_driver.rs:2493
  `remove_worktree_at` (aida-core/src/git_ops.rs:2845). Failed-attempt cleanup skips the clone.
  **S.** Both route to `seat_trees` and `seat_clone::destroy`.
- compete_cmd.rs:648/670/687/857/870 does a local merge of the winner branch plus worktree
  add/remove. **C** only if compete lanes become contained. Out of scope: compete keeps worktrees,
  and if contained it refuses with a clear message until a follow-up.

### 2d. Ship, PR, CI-fix and status

- pr_cmd.rs:1209/2236/3535/4597: post-merge `aida pull` and `checkout main` in `main_worktree`.
  From inside a clone that is the clone, so the real main checkout is never synced. **C, silent.**
  Fixed by §2a. In a contained seat the main checkout is read-only anyway, so after ship the pull
  is skipped with a note, and the host (reap, the heartbeat, or the operator's next command)
  syncs main.
- pr_cmd.rs:3774 `pr_ship_hold_root` reads merge-hold markers from the clone. **C.** Fixed by §2a,
  because the resolver returns the real main.
- No change needed (they key off origin):
  - pr_cmd.rs:2431/4659-4690 `branch_in_sibling_worktree`: `--delete-branch` is fine.
  - pr_cmd.rs:241/318 `pr_gc_branch_checked_out`.
  - pr_cmd.rs:5005/5127.
  - orchestrator_leases.rs:1687/1734: graded-review scratch checkout at an origin SHA.
  - CI-fix, phase_driver_trait.rs:3626-3675: uses the recorded `implementer_worktree` plus
    `remote_head_sha`. The path is the clone, so CI-fix relaunches into the clone with the same
    `TreeKind`.
- status_prs.rs:1967-2105 unshipped detector: `for-each-ref refs/heads/` in main. **S.** It adds
  `seat_trees` clone entries and the §4 probe result.
- queue_cmd.rs:10845 `preflight_queue_work_branch` and trace.rs:991 do branch-ownership and
  worktree lookup. **S.** They switch to `seat_trees`.

## 3. Cargo target dir and disk cost

- **Objects.** `--reference` keeps the clone's `objects/` at about 16 KB plus the seat's own new
  commits. Main's 1.6 GB of objects are read through alternates, read-only (the seat cannot write
  `<main>/.git/objects`). Do NOT `--dissociate`, which copies 1.6 GB per seat.
- **Working tree.** About 57 MB per clone. Pool worktrees cost the same.
- **Target dir.** Keep today's model: `CARGO_TARGET_DIR=<main>/target/worktrees/<seat-dir-name>`
  (session_end.rs:402 `isolate_cargo_target_dir`, exported through `.aida/session-env.sh`). The
  launcher creates that directory, and the seat's `target` symlink points at it (pool trees do
  this already: `aida-pool-aida-1/target -> /home/joe/ai/aida/target/worktrees/aida-pool-aida-1`).
  It is added to the sandbox allowWrite and the bwrap `--bind`.
  - This is a directory the seat owns outright. No other seat or host process binds files inside
    it, so the file-level-carve-out problem does not apply.
  - **Cost:** per-seat target dirs today are 6.7-19 GB on `/mnt/build` (103 GB free). Clones change
    nothing here.
  - **Cold-build cost:** the same as a fresh pool tree today. Pool trees currently have no target
    dir either (`target/worktrees/aida-pool-aida-1` does not exist).
  - **sccache:** `~/.cargo/config.toml` sets `rustc-wrapper = sccache`, with the cache at
    `/mnt/build/sccache` (32 GB). That cache is what keeps a cold build cheap, so it needs rw (add
    `/mnt/build/sccache` to the write paths, or run with `SCCACHE_*` pointing at a server reached
    over localhost).
  - Contained seats today can't write the symlinked target or the sccache dir, so they probably
    cannot build at all. S2 verifies this; it is not a regression.
- **Reclaim.** worktree_reclaim.rs adds clone target dirs through `seat_trees`. `seat_clone::destroy`
  removes `target/worktrees/<seat-dir-name>`, as session end does for worktrees.

## 4. Getting the seat's branch to origin and local main, and cleaning up

**Push only.** Inside the sandbox the seat commits in its own `.git/` and runs `git push -u origin
<branch>`. Credentials come from the global git config, which is readable, and the gh credential
helper. `github.com` is already in `[contained] allowed_hosts`, and tmp-clone seats pushed this
way all night.

- Local main never receives a direct ref write from the seat.
- Landing is unchanged: forge squash-merge (`aida pr ship`, or the advisor), then the host's normal
  `git fetch` / `aida pull` in main.
- The only local-merge code is compete_cmd.rs:648, which is out of scope (§2c).

**Rule: the host never runs git with a seat clone as the repository.** The clone's `.git/config`,
`hooks/` and `info/` are seat-writable. Host git there would run seat-planted `core.fsmonitor`,
`core.sshCommand`, hooks, `uploadpack.*` or filter drivers outside the sandbox. That is the same
escape round 1 closed for the shared common dir. Host code that needs facts from a clone uses:

- **`seat_clone::probe(clone) -> ProbeResult`.** The host runs git inside a throwaway bwrap with
  the clone read-only: `--ro-bind / / --tmpfs /tmp --unshare-all --die-with-parent`, plus
  `GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null -c core.hooksPath=/dev/null -c
  core.fsmonitor=false`.
  - It runs `status --porcelain=v2 --branch` and `rev-list --count @{u}..HEAD`, or `rev-parse HEAD`
    with an origin compare.
  - Planted config can run, but only in a read-only, network-less namespace. Its stdout is parsed
    as untrusted data.
  - If bwrap is unavailable, the probe reports "unknown". Reap and end then treat the clone as
    unshipped and keep it (fail closed).
- **`seat_clone::destroy(lease)`.**
  1. Run the probe. If it is dirty, or HEAD is not reachable from `origin/<b>`, salvage first.
     Salvage does not fetch from the clone, because that would run upload-pack in a repository
     the seat controls. Instead, a probe run writes the clone's unpushed commits with
     `format-patch`, plus the working-tree diff, into `<main>/.aida/salvage/<seat>/`. This mirrors
     the pool's dirty-tree salvage.
  2. `remove_dir_all(<seat-dir>)`. Rust's implementation does not follow symlinks.
  3. Remove `target/worktrees/<seat-dir-name>`.
  4. `git -C <main> update-ref -d refs/aida/seats/<seat>`.
  5. Release the lease.
- **Callers.** Stop, end and reap call it. These are the session_end.rs:1675 site, the
  session_reap.rs clone branch, and worktree_dismiss and gc via `seat_trees`. Merged-ness for reap
  is "origin/<b> merged or PR merged on the forge" plus "probe: nothing beyond origin/<b>".

## 5. Migration and back-compat

- **Uncontained seats keep linked worktrees and the warm pool unchanged.** `TreeKind::Worktree`
  is the default, and every changed function keeps its current branch for it.
- **Contained seats.** Only a launch whose resolved posture is contained gets `TreeKind::Clone`.
  The knob `[contained] seat_tree = "clone"` defaults to `clone` once S4 lands. Until then it
  defaults to `worktree`, which keeps today's (broken, but safe) worktree-only roots. There is no
  `"worktree-gitdir"` mode: PR #2694's widening is never shipped.
- **In-flight contained seats in pool worktrees** keep running with worktree-only roots: they
  cannot commit, which is today's behaviour. They are never retrofitted. They end normally, and
  the next contained launch gets a clone.
- **Leases.** `SessionLease` gains `#[serde(default)] tree_kind: TreeKind` (default `Worktree`),
  so old lease files parse unchanged. Readers that ignore the field see a clone lease as a
  worktree lease with a path that `git worktree list` does not know, which is the existing S
  behaviour (fail safe).
- **PR #2694** is closed without merging. Its config/hooks/info/pointer deny work is already the
  round-1 boundary on main, so nothing is lost. Its tests become the S4 regression suite.

## 6. Residual-risk table

| Item | Under PR #2694 | Under option A (clone) | Residual and mitigation |
|---|---|---|---|
| Rename-aside of `refs/heads` in the shared gitdir | bypass (reproduced) | `<main>/.git` has no write grant, so `mv` gives EROFS | none; covered by adversarial test T1 |
| Host lock-then-rename or `pack-refs --prune` detaching a bind | bypass (reproduced) | no binds inside the shared gitdir to detach | none; T2 |
| `refs/remotes/origin/HEAD` in main (operator `reset --hard origin/main` hazard) | writable | read-only; the seat can only forge its OWN clone's `origin/HEAD`, which no host command reads | none locally; T5 |
| `refs/tags` in main (release tags) | writable | read-only locally | **residual at origin:** the seat can `git push origin v9.9.9`, and release.yml fires on `v*` tags. This is NOT new: contained seats already have token plus egress to github.com. Mitigation: operator adds a GitHub tag ruleset on `v*` (release actor only), which is outside the code. The `main` branch already forbids force-push and deletion and requires 2 checks. Note: the "main" ruleset is `enforcement: disabled`, and classic protection is what holds |
| Stale `.lock` causing a silent worktree-only fallback | silent downgrade | no lock or materialize step at all. Clone creation either succeeds or the launch refuses with an error naming the step (no silent fallback). A stale lock inside the seat's OWN clone is the seat's problem | none; T6 asserts refusal is loud |
| Alternates write | `objects/info/alternates` in the shared dir writable | the seat can edit its OWN `objects/info/alternates`. That only changes what the seat can read, and reads are broad already. Host never execs git there (probe sandbox) | low; T4 |
| Alternates source pruned by main's `gc` | n/a | objects the clone borrows could vanish if main drops the refs that reach them | host pins `refs/aida/seats/<seat> = base_sha` in main at creation and deletes it at destroy; the seat's own new objects are local to the clone |
| Seat-planted config or hooks in its clone executed by host git | n/a | the main new hazard | rule "host never runs git in a clone"; the probe is a read-only, network-less bwrap; a grep gate in S3 flags `Command::new("git")` with `-C <lease.worktree_path>` when `tree_kind == Clone` |
| Seat forges `.aida/seat-clone.toml` to point host code at another dir | n/a | host-side actions take main and paths from the LEASE (in `<main>/.aida/sessions`) | lease writes go through the store paths the seat can write (BUG-1929 store is rw). Host destroy refuses unless `lease.worktree_path` is under `worktree_parent` and matches `aida-seat-*` |
| Seat pushes to another seat's branch at origin | n/a | possible, same as any pusher | the forge's PR review plus the merge gates. Not a local-repo integrity issue |

## 7. Real-bwrap adversarial test plan

Location: `aida-cli-lib/src/tests/bug_2169_contained_clone_bwrap_tests.rs`.

**Harness.** Build bwrap argv from the generated `--settings` sandbox.filesystem with Claude Code's
semantics: `--ro-bind / /`, then `--bind` for each existing allowWrite entry, then `--ro-bind` for
each existing denyWrite entry, skipping missing paths (fixing the bug_1929 model, as round 1
asked). Then also run through `bwrap_confinement_args_inner` for the os_wrap layer. Use real
`bwrap` 0.9.0 and a fixture main repo with a bare file:// "origin":

- `main` packed and `refs/remotes/origin/HEAD` set
- tag `v0.0.1`
- one other linked worktree on branch `other`

**Gating.** Run when `bwrap_preflight` passes. CI runs it as a gate in ci.yml with
`sysctl kernel.apparmor_restrict_unprivileged_userns=0`, the Ubuntu 24.04 runner remedy from
SPIKE-61 §7.7. Locally it is in `make check-ci-fast` when bwrap works. A test that cannot get a
userns prints `SKIP (bwrap unavailable)` and the CI gate fails on skip. Each attack runs against
two layouts:

- **Legacy layout** (worktree plus PR #2694 grants and denies, rebuilt from 8bc07d82e8's
  `contained_write_roots` output as a frozen fixture). The attack must SUCCEED, which proves the
  test detects the bypass.
- **Clone layout** (the new launcher's real output). The attack must FAIL, and host state must be
  unchanged.

| Test | Attack (run inside bwrap) | Legacy expectation | Clone expectation |
|---|---|---|---|
| T1 rename-aside | `cd $MAIN/.git; mv refs/heads h.x; mkdir refs/heads; mv h.x/* refs/heads/; echo $EVIL > refs/heads/main` | host `git -C $MAIN rev-parse main` == $EVIL | first `mv` gives EROFS; host main unchanged |
| T2 host detach | seat waits on a fifo; host runs `git -C $MAIN update-ref refs/heads/main <new>`, then a second variant with `git pack-refs --all --prune`; seat then writes `refs/heads/main` | write succeeds and host sees the forged SHA | EROFS in both variants |
| T3 positive | in the clone: commit, `fetch origin`, `merge origin/main`, `push -u origin <b>`; host `git -C $MAIN fetch origin` | n/a | all succeed; `$MAIN` has `origin/<b>` == the seat's HEAD; `$MAIN` local refs unchanged |
| T4 objects and alternates | `touch $MAIN/.git/objects/pack/x`; `rm $MAIN/.git/objects/pack/*.pack`; edit own `objects/info/alternates` to `/` | pack delete succeeds (the shared objects dir is rw) | EROFS on main; own alternates edit allowed; host `seat_clone::probe` still runs, read-only |
| T5 origin/HEAD and tags | `git -C $MAIN update-ref refs/remotes/origin/HEAD $EVIL`; `git -C $MAIN tag -f v0.0.1 $EVIL` | succeeds | EROFS; fixture origin tags are unchanged except when the seat pushes, which T5b records as the documented residual (a push of a new tag to the file:// origin succeeds, and the test pins it so a future server-side guard flips it) |
| T6 no silent fallback | make `worktree_parent` read-only, or leave a stale `.lock` in the clone's target path; run the launcher | (legacy: silently falls back to worktree-only roots) | launch refuses, non-zero exit, message names the clone step |
| T7 config and hooks escape via host | seat writes `core.fsmonitor=/tmp/pwn.sh` and a `post-checkout` hook in its clone; host then runs reap, end and destroy | n/a | `/tmp/pwn.sh` on the HOST never runs (marker file absent outside the sandbox); the probe ran under bwrap |
| T8 config.worktree / hooks of main | round-1 denies, re-run under the clone layout | holds | holds |

## 8. Vertical slices

Each slice is mergeable on its own, behind `[contained] seat_tree` (default `worktree` until S4).

1. **S1: seat-clone creation and resolver.**
   - `seat_clone::{create, marker}`
   - `TreeKind` on `SessionLease`
   - the `main_worktree_root_from` clone branch, plus the `--git-common-dir` helpers routed to it
   - `refs/aida/seats/*` pin
   - session_start route under the knob
   - fix `tool_contained_flags` so it scopes to the lease tree
   - tests: unit tests plus T3 positive (no sandbox)
2. **S2: sandbox boundary for clones.**
   - write roots = clone, store, cargo target dir and sccache
   - bg and interactive launches go through `os_wrapped_program_and_args`
   - real-bwrap harness, with T1, T2, T4, T5 and T8 against both layouts
   - verify that contained cargo builds work
3. **S3: host-safe lifecycle.**
   - `seat_clone::probe` and `seat_clone::destroy`
   - session end, reap and dismiss branches
   - grep gate against host git in clones
   - T6 and T7
4. **S4: visibility.**
   - `seat_trees` enumerator replaces the listed `list_worktrees` / `git worktree list` consumers
     (status, doctor, brief, runaway, gc, reclaim, preflight, trace)
   - unshipped detector and CI-fix relaunch
   - post-ship pull note
   - flip the knob default to `clone`; close PR #2694
5. **S5 (conditional on O1): host-provisioned clone pool**, refilled by the uncontained heartbeat.
   Pool reset passes SHAs, not ref names.

## 9. Option B (no ref writes; host moves the branch), briefly

B keeps the linked worktree. The seat commits on a detached HEAD, and host-side `aida` moves
`refs/heads/<b>` at stop or ship.

- **Against:**
  - A commit still writes objects and an index. In a linked worktree, objects go to the SHARED
    `objects/`, so B still needs a writable shared directory. The rename-aside class then becomes
    "rename `objects/pack` aside or delete packs", which corrupts main for everyone. T4 legacy
    shows it.
  - Avoiding that means a per-seat object dir plus alternates, which is option A without the
    clean `.git`.
  - B also needs a host-side trusted ref mover and push agent, a new privileged verb. The seat
    cannot `fetch`, `merge origin/main` or `push`, which is the basic job BUG-2169 says the lane
    needs.
- **For:** existing worktree tooling (§2) stays unchanged.

**Recommendation: option A.** It removes every shared-gitdir write, and it formalizes the
workaround that already worked all night. Its cost is the §2 tooling work, which splits into
slices S1-S4. Most of the change is turning silent-skip paths into explicit clone handling.

## Open questions for the advisor

- **O1:** is the `aida` process that launches a contained seat always unsandboxed? This decides
  whether S5 is needed.
- **O2:** add `/mnt/build/sccache` rw for contained seats, or require an sccache server? This
  affects build cost, not containment.
- **O3:** a GitHub tag ruleset for `v*` is an operator action. Please confirm it is acceptable to
  record it as the origin-side residual rather than block on it.

## S2 as built

- **Trust for the clone's own `.git`.** BUG-2012 denies a real `.git` under any write root as a
  main checkout's. `seat_clone::host_recorded_clone` is the only exemption: the root must be the
  clone root itself, named `aida-seat-*`, verified by `main_for`, and named by a `clone` lease in
  `<main>/.aida/sessions`. That host-written lease sits outside every granted root. The tool rules
  also deny `.aida/sessions` under the clone (a symlink into main) and main's own copy. In practice
  the BUG-2012 deny reached only the Edit/Write tool rules, not `sandbox.filesystem.denyWrite`, so
  Bash git in a clone worked either way; the exemption fixes the tool layer.
- **Write set.** The clone, `<main>/.aida-store` and `cache.db*` (BUG-1929), and
  `<main>/target/worktrees/<clone dir>` from the lease. `session_start` creates the target directory
  host-side, since a sandbox skips a path missing at launch. There is no sccache path (ruling O2).
  `RUSTC_WRAPPER=""` and `CARGO_TARGET_DIR` go into the settings `env` and the os_wrap
  `--setenv`. Measured here: with sccache configured and the network shared, a sandboxed build
  otherwise reaches the host's sccache server, which writes the shared cache for it.
- **os_wrap.** `os_wrap_rw_paths` binds main's store and cache files plus the target directory for a
  clone, never `<main>/.git`.
- **Pre-existing defects.** `agent new` rescopes `--settings` to `plan.launch_cwd`
  (`rescope_contained_settings`), so the boundary is no longer the launcher's git root. The
  interactive `claude` launch and the interactive resume go through `os_wrapped_program_and_args`. `--bg` cannot be wrapped,
  because `claude --bg` hands the session to Claude Code's background daemon, which the dispatcher
  never parents, so a contained `--bg` launch under os_wrap is refused before any claim, lease, clone or pin.
- **Harness.** `tests/bug_2169_contained_clone_bwrap_tests.rs` covers T1, T2 (update-ref and
  pack-refs variants), T3, T4, T5/T5b and T8, plus a cargo build. Each runs against both the
  modelled Claude Code sandbox and the os_wrap argv. The attack tests also run against the frozen
  #2694 layout, where the attacks must succeed. The tests join the GitHub live-bwrap gate, where
  `AIDA_REQUIRE_BWRAP_LIVE=1` turns a skip into a failure. T4's probe check and T6/T7 belong to S3.

## S3 as built

Owning spec: TASK-2053 (the remaining BUG-2169 slices moved there unchanged).

- **`seat_clone::probe(main, clone)`.** Two git runs (`status --porcelain=v2 --branch
  --untracked-files=all`, then `rev-list --count HEAD --not --remotes=origin <base>`) inside
  `bwrap --unshare-all --die-with-parent --new-session` with a private `/tmp` and no network. The
  read view is never wider than the seat's own: with `[contained] read_allowlist` (read from
  MAIN's config) only the essential system paths and the allowlist, otherwise `--ro-bind / /`. The
  clone and `<main>/.git` are bound read-only after the `/tmp` mount. A host-written repo config
  (repository format and object format taken from MAIN) is bound over the clone's `.git/config`,
  and `GIT_DIR`/`GIT_COMMON_DIR` are set, so no seat-defined filter, diff, textconv or fsmonitor
  driver or include exists in the probe, and a planted `.git/commondir` cannot point at another
  config. The environment is cleared; `GIT_CONFIG_NOSYSTEM=1`, `GIT_CONFIG_GLOBAL=/dev/null`,
  `GIT_OPTIONAL_LOCKS=0`, and command-scope `core.hooksPath=/dev/null` and
  `core.fsmonitor=false`. Each run is killed after 120 s, and its captured output is capped
  (16 MiB for a probe; the reader kills the run past the cap). `<base>` is the commit the pin in
  MAIN names, never read from the clone. No bwrap, a timeout, a non-zero exit, a cut-off or an
  unparsable answer is `Probe::Unknown`, and every caller keeps the clone.
- **"Unpushed"** means commits reachable from HEAD that no `refs/remotes/origin/*` in the clone
  holds. Those refs are seat-writable, so a seat can make its own work look pushed; that only
  loses its own work (no host or cross-seat effect), and session end still requires `--force` for
  uncommitted files.
- **`seat_clone::salvage`.** One fixed `sh -c` script in the same sandbox prints `format-patch
  --stdout --binary HEAD --not --remotes=origin <base>`, `diff --binary HEAD`, and a new-file diff
  per untracked file (BUG-696 shape). The host writes stdout to
  `<main>/.aida/salvage/<clone dir>/<UTC stamp>.patch`. No fetch from the clone. A salvage past
  64 MiB is written cut off with a closing `# TRUNCATED` line and is an error, so `destroy` keeps
  the clone.
- **`seat_clone::destroy(main, clone, mode, expect_head)`.** Refuses unless the path is an
  `aida-seat-*` directory directly in main's worktree parent, neither inside main nor containing it
  (the lease is seat-writable through the store). Then: probe (unknown keeps; a HEAD other than
  `expect_head` keeps), salvage when dirty or unpushed (`KeepUnshipped` stops there, for reap;
  a failed or cut-off salvage keeps the clone in either mode), for reap a fresh live-process
  check (`check_worktree_removal`, BUG-1947: in use or unverified keeps), `remove_dir_all`, remove `<main>/target/worktrees/<clone dir>`, release the pin. A clone
  directory that is already gone goes straight to the target and pin steps.
- **Session end.** Probes first (unknown refuses, nothing changed). Unpushed commits count as dirty
  entries, so they need `--force` like uncommitted files. Teardown calls
  `destroy(SalvageUnshipped)` in place of `git worktree remove`; the lease is deleted at the usual
  point.
- **Reap.** A `clone` lease is classified by `classify_clone_reap` (process alive and spec not
  finished first, so a live or unfinished seat is never probed; then probe unknown, dirty, off
  its branch, unpushed; then no merged PR on the forge: each keeps it). The
  row's `branch_tip` is the probed HEAD; `reap_one` calls `destroy(KeepUnshipped, Some(tip))` and
  releases the lease only when the clone is gone. No branch in main is touched.
- **Orphan pins (condition 4).** `create` claims the empty clone directory before it writes the
  pin, so a pin never exists without its directory while a launch is in flight (the lease is
  written after `create`). `seat_clone::orphan_pins` lists `refs/aida/seats/*` that no clone
  lease names. `aida session reap` (and the quiet night-shift reap) release those whose clone
  directory is gone and report the ones whose clone still exists.
- **Placement.** An explicit `--path` for a clone seat must be the seat-clone location (an
  `aida-seat-*` directory directly in the worktree parent); anything else is refused, so no clone
  is ever made where `destroy` cannot remove it.
- **Dismiss.** `aida worktree dismiss <clone>` runs the holder check, `destroy(SalvageUnshipped)`,
  releases any clone lease naming the directory, and writes a `seat_clone_dismiss` ledger line. The
  salvage patch is the archive (the clone's commits are not in main, so there is no
  `refs/archive/*`).
- **Tests.** `tests/task_2053_seat_clone_lifecycle_tests.rs`. T7 plants `core.fsmonitor`, a clean
  filter and hooks, runs probe, the reap scan, salvage and destroy with a logging `git` first on
  PATH, and asserts no canary fired on the host and no untagged git ran in the clone; a negative
  control shows the plant fires under plain host git. T6 is S1's refusal tests plus a stale
  pin-lock variant. T4's probe check points the seat's alternates at `/`. The `task_2053_bwrap_`
  tests join the live-bwrap CI gate.
- **Still gated.** `seat_tree = "clone"` stays a development switch until S4 moves status and the
  other listing paths off plain git.

### S3 review round 2 fixes

- **Unpushed means any ref.** The probe counts commits reachable from every branch, tag, HEAD and
  stash entry (the `refs/stash` reflog) that are not on `origin/*` or the pinned base, so a side
  branch, a tag or a stash makes the clone unshipped.
- **Salvage writes a bundle.** Beside the readable patch, salvage writes `<stamp>.bundle` holding
  every one of those commits with all of the clone's refs (thin against origin; `git fetch` it into
  a checkout of main). A bundle that cannot be made fails the salvage, and the clone is kept.
- **Untracked names are NUL-separated** (`ls-files -z` into `xargs -0`), so no quoted name is
  dropped, and a file that cannot be diffed fails the whole salvage.
- **Orphan pins are compare-and-delete.** `release_orphan_pin` re-checks that the clone directory
  is absent and that no clone lease names it, then runs `update-ref -d <pin> <listed oid>`.
## S4a as built (host git off seat clones)

S4 ships in three PRs: S4a (this: no host git in a clone from routine paths), S4b (`seat_trees`
visibility: status rows, unshipped detector, runaway sources, preflight, trace, gc/reclaim), and
S4c (flip `seat_tree` to `clone`, retire `AIDA_DEV_SEAT_CLONE`, close PR #2694).

- **Raw git routed through `host_git_command`**, which refuses a seat clone without spawning git:
  `dispatch_health_ps::git_stdout` (the `aida status` / `aida ps` / statusbar running-work probe,
  `current_branch_at`, the untracked-parent `ls-files`), `worktree_gc::{current_branch_at,
  detect_default_branch_ref, git_output_lossy}`, `burndown_run::worktree_last_activity` (its mtime
  leg still answers), `lane_liveness::branch_latest_commit_ts`, and
  `orchestrator_leases::git_output_checked` plus the PR-title `git log`. A refused read is "no
  answer", which each caller already handles.
- **`git_ops::is_git_repo`** is false for a seat clone, and the TUI's main-root resolver reads a
  clone as its own directory.
- **Clone-aware cleanup.** `force_cleanup_lease` (doctor's lease heal, pickup and review
  stale-lease release, drain-retry predecessor, escalated cleanup) calls
  `seat_clone::destroy(SalvageUnshipped)` for a clone lease after its holder check and releases
  the lease only once the clone is gone. `salvage_worktree_patch` probes a clone and salvages it
  through the sandbox; an unreadable clone is an error, so nothing is torn down.
- **Implementer push.** `push_branch_from_implementer_worktree` refuses a clone: a host push would
  run its seat-written pre-push hook. A clone seat pushes from its own sandbox.
- **`agent new` re-entry** checks a clone lease's branch through `seat_clone::probe`.
- **Tests.** `tests/task_2053_s4_host_git_tests.rs` runs every path above against a clone with a
  logging `git` first on PATH and requires that no untagged git ran in the clone (negative
  control included); `task_2053_bwrap_bulk_lease_cleanup_destroys_a_seat_clone_salvage_first`
  covers heal/cleanup.

## S4b as built (seat clones visible to the host)

- **`seat_trees` enumerator** (`status_prs.rs`): the linked worktrees from `git worktree list`
  (now run through `host_git_command`), then one record per `clone` lease whose directory exists
  (`clone_seat_trees`), carrying the lease branch and `clone: true`. Destructive sweeps (gc, doctor
  orphan/merged heals, reclaim, dismiss, reap) keep enumerating linked worktrees only, so a clone
  is never their candidate; its removal stays `seat_clone::destroy`.
- **Visibility.** `aida status`'s worktree pane and `aida awaiting`'s uncommitted-WIP detector list
  clones, with branch, dirty count and unpushed count from the sandboxed probe (an unreadable clone
  reads dirty). The unshipped-work detector adds `clone_unshipped_work_items`: a clone with commits
  on no `origin/*` ref, attributed to its lease's spec, or an unreadable clone with an unknown
  count; never dropped. Pushed work flows through the existing `origin/` path.
- **Branch ownership.** `aida brief` resolves a spec branch held by a clone to a new
  `SeatClone` target ("continue it in that seat"), `aida show` linkage falls back to a clone lease
  for the spec, and `aida queue work` preflight refuses a branch a clone holds without suggesting
  `git worktree remove`.
- **Other sources.** Runaway-seat transcripts, `aida merge-hold list`'s stray markers (only through
  real directories, never a symlinked `.aida`) and `aida worktree list` (kind `clone`, its focus
  marker unread) include clones; doctor's trust-breadth count skips `aida-seat-*`.
- **Post-ship pull.** `aida pr ship` from a seat clone skips step 4 with a note: main is read-only
  to the seat and is synced from outside it.
- **Hardening.** `teardown_worktree_path` refuses a clone before any `pre_destroy` hook runs; the
  phase driver's failed-attempt teardown keeps a clone (its session end or reap removes it);
  `local_commits_not_on_branch`, `worktree_reclaim`'s git and review-watch's root probe no longer
  run raw git in a clone.
- **Not changed (by design §2d):** `branch_in_sibling_worktree`, `pr_gc_branch_checked_out` and
  `session_reap`'s checked-out-elsewhere check key off main's or origin's refs.
- **Tests.** `tests/task_2053_s4b_seat_trees_tests.rs`: enumerator, brief, show, pickup,
  runaway, teardown refusal with no untagged git in the clone; stray markers with a symlinked
  `.aida`; under live bwrap, a status row and an unshipped item for a clone with a commit and a
  dirty file.
