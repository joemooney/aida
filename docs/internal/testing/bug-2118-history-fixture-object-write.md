# BUG-2118 Investigation Note: History Fixture `failed to write commit object`

<!-- trace:TASK-1970 | ai:antigravity -->
<!-- trace:BUG-2118 | ai:antigravity -->

## Observed Failure

In CI run `38123466347` (Ubuntu Build, commit `36c906f453`), `history_cache::task_1507_history_cache_tests::history_cache_merge_commit_decodes_like_git_show_cc` failed at `aida-cli-lib/src/tests/task_1507_history_cache_tests.rs:270` during the final commit of `build_merged(&mut fx)`:

```text
git ["commit", "-q", "--allow-empty", "-m", "ship FR-1"] failed:
error: unable to create temporary file: No such file or directory
fatal: failed to write commit object
```

## Status: Mechanism identified and reproduced

<!-- trace:BUG-2118 | ai:claude -->

The fixture's own `git` commands started **detached auto-maintenance**, and
it raced the fixture's next object write.

1. CI ran **git 2.55.0** (`actions/checkout` "Getting Git version info" step
   of run `38123466347`). Git 2.54 made the **geometric** maintenance strategy
   the default (`Documentation/RelNotes/2.54.0.adoc`: "git maintenance starts
   using the geometric strategy by default").
2. Every `git commit` / `git merge` calls `run_auto_maintenance()`, which runs
   `git maintenance run --auto --quiet --detach` (`run-command.c`,
   `prepare_auto_maintenance`: `--detach` unless `maintenance.autoDetach` or
   `gc.autoDetach` is false). The child daemonizes, and the fixture's `git`
   returns while maintenance still runs.
3. The geometric-repack auto condition is `too_many_loose_objects(100)`. Its
   approximate count is `entries in objects/17 * 256 > 256`, so **two** loose
   objects in `objects/17` trigger it. The fixture's dates and contents are
   fixed, so the trigger is deterministic for this fixture.
4. The task runs `git repack -d -l --cruft ...`. With `-d` it deletes the now
   packed loose objects and `rmdir`s every emptied `.git/objects/<xx>` fanout
   dir.
5. Meanwhile the fixture's next `git` call writes an object.
   `create_tmpfile()` (`object-file.c`) tries `mkstemp(objects/<xx>/tmp_obj_*)`.
   On ENOENT it runs `mkdir(objects/<xx>)` and retries once. When the
   background prune removes the fanout dir between that `mkdir` and the
   retry, git reports `error: unable to create temporary file: No such file
   or directory`. The caller then reports `fatal: failed to write commit
   object` (commit) or `failed to insert into database` (add).

Git 2.43 (the local default on Ubuntu 24.04) uses the `gc` strategy. Its
threshold (`gc.auto`=6700) is never reached by this fixture, which is why
local traces never saw the race.

### Raw evidence

- **trace2 timeline** (git 2.55, fixture without the guard): the detached
  `repack -d` started by `commit -m "side2: add STORY-4"` ran
  13.882–13.934. During that window the fixture ran `rev-parse HEAD`
  (13.904), `checkout -q aida-store` (13.910) and
  `merge --no-ff --no-edit side2` (13.918–13.937), and the merge writes a
  commit object.
- **strace** (`-f -e execve,mkdir,rmdir,exit_group`): the fixture's commit,
  pid 76051, called `exit_group` at 05.991950. Its daemonized `repack -d`,
  pid 76115, ran `rmdir(".git/objects/0e") = 0` … 49 fanout dirs at
  06.115–06.122, about 124 ms after the fixture's `git` had returned.
- **Panic with the window widened** (see below): `git ["add", "-A"] failed
  (status=Some(128), dir_exists=true, objects_exists=true …): error: unable
  to create temporary file: No such file or directory`. The store and
  `.git/objects` survive; only a fanout dir disappears. This matches CI,
  where earlier commits in the same repo had succeeded.

### Deterministic reproduction

An `LD_PRELOAD` shim adds a pause after a successful
`mkdir("<…>/objects/<xx>")`. That widens git's mkdir→mkstemp window without
changing any logic:

```c
#define _GNU_SOURCE
#include <dlfcn.h>
#include <string.h>
#include <unistd.h>
#include <ctype.h>
#include <sys/stat.h>
int mkdir(const char *p, mode_t m) {
    static int (*real)(const char *, mode_t);
    if (!real) real = dlsym(RTLD_NEXT, "mkdir");
    int r = real(p, m);
    size_t n = strlen(p);
    if (r == 0 && n >= 10 && isxdigit(p[n-1]) && isxdigit(p[n-2]) &&
        !strncmp(p + n - 10, "objects/", 8))
        usleep(300 * 1000);
    return r;
}
```

Build it with `gcc -shared -fPIC -o libwiden.so widen.c -ldl`. Then put a
`git` wrapper first on `PATH` that runs
`LD_PRELOAD=libwiden.so exec <git-2.55>/bin/git "$@"`, and run
`cargo test -p aida-cli-lib --lib history_cache_merge_commit_decodes_like_git_show_cc`.

| fixture `git` | git 2.55 + widened window |
|---|---|
| without `-c maintenance.auto=false -c gc.auto=0` (as on `36c906f453`) | **5/5 FAILED**, "unable to create temporary file" |
| with the guard (current `fixture_git`) | 5/5 passed |
| git 2.43 + widened window, shell loop, no guard | 20/20 passed (no trigger) |

### Fix and regression

`fixture_git` passes `NO_AUTO_MAINTENANCE`
(`-c maintenance.auto=false -c gc.auto=0`), which makes
`prepare_auto_maintenance` return before spawning anything. This guard
landed in #2560 as hardening, before anyone knew why it mattered; this
investigation shows it is the actual fix.
`fixture_git_never_starts_auto_maintenance` keeps it in place on every git
version. It sets the repo up so that any maintenance run packs in the
foreground (`maintenance.loose-objects.enabled=true`,
`maintenance.loose-objects.auto=-1`, both `autoDetach` false). It then
checks that `build_merged` leaves zero packs, with an unguarded control that
must pack. Dropping the guard fails the test on git 2.43 (11 packs) and on
git 2.55 (3 packs).

Ruled out along the way: PATH shims (they exec the real git), removal of
the caller's cwd (`git -C /abs` still succeeds), inherited `GIT_DIR` /
`GIT_OBJECT_DIRECTORY` (no process-wide `GIT_*` writer existed on
`36c906f453`), and early `TempDir` drop (`Fixture` owns it for the whole
test).

Other repository test fixtures that ran bare `git commit` without this
guard were exposed to the same race on git 2.54+ (BUG-2191).

### All fixtures: `test_git` helper and ratchet (BUG-2191)

<!-- trace:BUG-2191 | ai:claude -->

The guard is now shared. `aida_core::git_ops::NO_AUTO_MAINTENANCE` holds the
`-c` pair; production's `transaction_git_config` returns it under the store
write lock. Test code spawns git through a per-crate helper that applies it:

- `crate::test_git::git_command()` in aida-core and aida-cli-lib unit tests;
- `test_git::git_command()` in aida-cli integration tests, after
  `#[path = "support/test_git.rs"] mod test_git;`;
- inline `git -c maintenance.auto=false -c gc.auto=0 <cmd>` in shell scripts.

A push is different: the receiving repository's `receive-pack` runs
auto-maintenance (`receive.autogc`) under its own config, and local transport
clears `GIT_CONFIG_PARAMETERS`, so the client's `-c` pair never reaches it.
The helper therefore also passes `-c init.templateDir=<dir>`. That template
has the default layout plus a `config` with `receive.autogc=false`,
`gc.auto=0` and `maintenance.auto=false`, so every repository the helper
creates (`init`, `init --bare`, `clone --bare`) carries the settings itself.

It is deliberately not a process-wide `GIT_CONFIG_COUNT` env: production adds
its own `GIT_CONFIG_*` pairs, and many fixtures set `GIT_CONFIG_GLOBAL` for
isolation.

`tests/test_git_maintenance_ratchet.py` (CI gate "Test git auto-maintenance
ratchet", in `make check-ci-fast`) counts test-source sites that run a
write-class subcommand (commit, commit-tree, merge, am, rebase, cherry-pick,
revert, pull, fetch, push) without the guard. The per-file counts in
`scripts/git-maintenance-baseline.txt` may only go down. The remaining sites
mostly commit through production helpers (`host_git_command`, regen's `git`)
or a generic `run(dir, "git", ..)`.

Reproduction on a migrated fixture (git 2.55.0 built from source, the
`libwiden.so` wrapper above, `cargo test -p aida-cli-lib --lib
auto_bump_done`, 97 tests, 5 runs each):

| `test_git::git_command()` | result |
|---|---|
| with the `-c` pair | 5/5 passed |
| `-c` pair removed (control) | 1/5 failed: `git ["commit", "--allow-empty", ...] failed: error: unable to create temporary file: No such file or directory` |

More converted fixtures, same setup, guarded only (the control above already
shows the wrapper reaches fixture git; an aida-cli control would cost about
an hour under the widened window):

| converted fixture | runs | result |
|---|---|---|
| aida-cli integration test `task_1717_store_writer_lock` (43 tests, `support/test_git.rs`) | 5 | 5/5 passed |
| aida-core `git_ops::tests` + `worktree_pool` (171 tests) | 5 | 5/5 passed |

<!-- trace:BUG-2191.ac490e38 | ai:claude -->

Shapes the ratchet covers since TASK-1955 (follow-ups TASK-2010/TASK-2018):

- **Split statements and variable argv.** `let mut c = Command::new("git");
  c.arg("commit")` counts unless the binding is the `test_git` helper or a
  statement on it applies the `-c` pair; `let a = vec!["commit", ..];
  git.args(&a)` counts when a later unguarded git spawn receives `a`.
- **Shell and Python test files.** `tests/*.sh`, `tests/lib/*.sh` and
  `tests/*.py` are scanned too. A shell fixture exports the environment form
  of the pair near its top
  (`export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=maintenance.auto
  GIT_CONFIG_VALUE_0=false GIT_CONFIG_KEY_1=gc.auto GIT_CONFIG_VALUE_1=0`);
  a site behind `env -i` loses that environment and passes the `-c` pair
  inline. A Python fixture passes the pair in its argv list.

Known exclusions of the ratchet:

- The exported pair does not reach a bare remote's `receive-pack` on a local
  `git push` (local transport clears the config environment); the Rust
  `test_git` helper covers that through `init.templateDir`, shell fixtures do
  not.
- Python argv built across several lines, or a shell command assembled in a
  variable, is not parsed.

<!-- trace:TASK-1955 | ai:claude (TASK-2010) -->

## General Test-Isolation Hygiene Landed in This PR (TASK-1970, Refs BUG-2118)

While auditing `aida-cli-lib` tests for shared process-wide state, unrelated working-directory hygiene issues were found and fixed under `TASK-1970`:

1. **Isolate direct `std::env::set_current_dir` calls in multi-threaded lib tests (`run_path_isolated_test()`):**
   - `queue_rework_tests::rework_writes_blocking_review_findings_comment` (`aida-cli-lib/src/tests/queue_rework_tests.rs`)
   - `bug_2010_factory_lane_ps_tests::test_queue_work_respects_no_launch_for_calling_lane_attribution` (`aida-cli-lib/src/tests/bug_2010_factory_lane_ps_tests.rs`)
   - `init_cmd::init_roster_tests::init_at_non_tty_writes_no_team_roster` (`aida-cli-lib/src/init_cmd.rs`)
   - `user_alias::tests::project_overrides_personal_on_name_clash` (`aida-cli-lib/src/user_alias.rs`)
2. **`fixture_git` subprocess hygiene (`aida-cli-lib/src/tests/task_1507_history_cache_tests.rs`):**
   - Sets `.current_dir(dir)` and `GIT_CONFIG_GLOBAL=/dev/null` on `fixture_git`, and reports `status`, `dir`, `dir_exists`, `objects_exists`, and `cwd` if `git_in` fails.
3. **Source scanner for unisolated direct `set_current_dir(` in test code (`aida-cli-lib/src/test_env.rs`):**
   - `no_unisolated_set_current_dir_in_test_code` scans test modules/files for direct `set_current_dir(` calls that do not call `run_path_isolated_test()`.
   - `scan_block_for_unisolated_cwd` resets guard state on any `fn` item (`fn`, `pub fn`, `pub(crate) fn`, `async fn`, etc.), verified by `scan_block_for_unisolated_cwd_flags_seeded_offenders_across_fn_visibilities`.
   - **Scanner limitation:** Only literal `set_current_dir(` calls in test code are scanned; indirect `cwd` mutations through production functions (`queue_cmd.rs:12794`, `session_start.rs:2516`, ...) are not detected.
