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

## Status: Unreproduced

The underlying mechanism of the CI failure has **not** been reproduced or identified. `BUG-2118` remains open as unreproduced.

## What Was Checked and Ruled Out

1. **Where the error text originates in Git 2.43.0:**
   - `"unable to create temporary file"` is formatted in `start_loose_object_common` (`object-file.c`), which calls `git_mkstemp_mode(".git/objects/<aa>/tmp_obj_XXXXXX", 0444)` and, if that fails with `ENOENT` and `mkdir(".git/objects/<aa>", 0777) == 0`, retries `git_mkstemp_mode` once.
   - `"fatal: failed to write commit object"` is emitted by `cmd_commit` (`builtin/commit.c`) after `setup_git_directory`, `prepare_to_commit` (tree building), and the root-tree lookup in `commit_tree_extended` have completed.
2. **Process-wide `cwd` removal is ruled out as the cause of `git -C` failing:**
   - `fixture_git` passes `-C <dir>` where `<dir>` is an absolute `TempDir` path (`fx.store`).
   - Running `git -C /abs/repo commit --allow-empty` after `rmdir`ing the caller's current working directory still succeeds (`exit 0`) on Git 2.43.0 because `git -C` `chdir`s to the absolute target path directly without resolving relative to the caller's deleted `cwd`.
3. **Self-planted fanout faults are not a reproduction:**
   - Planting a dangling symlink at `.git/objects/<aa>` or concurrently calling `rmdir` on `.git/objects/<aa>` in a tight loop can force `start_loose_object_common` to emit the same two-line stderr, but nothing in `Fixture`, `build_merged`, or `task_1507_history_cache_tests.rs` creates symlinks in `.git/objects` or spawns a concurrent pruner (`fixture_git` already passes `-c maintenance.auto=false -c gc.auto=0`).

## What Remains Unknown

- Why `start_loose_object_common` received `ENOENT` when creating `.git/objects/<aa>/tmp_obj_XXXXXX` during `ship FR-1` in CI run `38123466347` after earlier commits and tree writes in the same `TempDir` succeeded.
- Whether `.git/objects` or the `TempDir` was affected by an external cleaner, filesystem anomaly, or another process during that CI run.
- `git_in` now logs `status`, `dir`, `dir_exists`, `objects_exists`, and `cwd` on failure so that if the failure recurs in CI, the state of the fixture directory at the moment of failure is captured.

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
