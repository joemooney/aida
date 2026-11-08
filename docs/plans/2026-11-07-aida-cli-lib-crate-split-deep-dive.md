# aida-cli-lib crate split: deep dive (SPIKE-713)
<!-- trace:SPIKE-713 | ai:claude -->

Date: 2026-11-07. Commit measured: `79603aff6b0` (main == origin/main). Read-only
spike; no code moves. Spec: `aida show SPIKE-713 --full`; parent: EPIC-78
(deferred until STORY-2326 completes, operator ruling 7, 2026-11-05).

All line counts are from `wc -l`/scripts run on 2026-11-07 against
`aida-cli-lib/src`. "non-test-dir files" means `.rs` files outside any `tests/`
directory; those still contain inline `#[cfg(test)] mod` blocks, which were
measured separately with brace matching. Module-graph numbers come from a
script that parses `use crate::X` / `crate::X::item` in non-test-dir files
(scratch script, not committed). `codegraph callers glyphs::get` returned "No
callers found" while `rg` finds 26 call sites in 16 files, so `rg` is the
primary evidence for coupling throughout.

## 1. Summary

- aida-cli-lib is 997k lines: 580k production, 129k inline `#[cfg(test)]`, 288k
  in `src/tests/`. It compiles as ONE rustc unit of 121.9 s (frontend 73.1 s,
  codegen 48.9 s) that is the sole active process for the last 100 s of a
  162.6 s clean build on a 20-core host. Only sibling crates fix that; a chain
  of crates (kernel -> app -> cli) gives ~0% because rustc's stable front end is
  single-threaded and pipelining only overlaps codegen.
- The module graph is one strongly connected component of 337 modules / 593k
  lines. Cutting the framework hubs (command_registry types, cli enums, test
  helpers) shrinks it only to 221 modules / 489k lines; the rest is command
  modules using each other's helpers (session.rs containment items in 20
  modules, pr_cmd ship helpers in 15, doctor helpers in 12). No file-move crate
  split exists today; the EPIC-78 family extraction IS the prerequisite.
- Recommended crate map (dependency direction top to bottom):
  `aida-cli` (bin) and `aida-mcp` -> `aida-cli-lib` (clap args, REGISTRY,
  rendering) -> `aida-app-{queue,review,session,launch,ops}` (sibling use-case
  crates) -> `aida-kernel` (gate/, events, forge, grant_keyring, merge_hold,
  ownership+leases types) -> `aida-cmdkit`, `aida-term` (leaves) -> `aida-core`;
  `aida-testkit` as a dev-dependency beside the leaves.
- First three PRs, all startable today without touching the in-flight files:
  (1) relocate inline tests out of doctor_cmd.rs, mcp.rs, auto_complete.rs,
  maintenance_schedule.rs, awaiting_you.rs, burndown.rs (30k test lines) into
  `src/tests/`; (2) move the 14 command-framework types out of
  `command_registry.rs` into `cmdkit.rs` with a re-export (cuts the SCC by 29
  modules, zero import churn); (3) split queue_cmd.rs into a `queue_cmd/`
  directory by subcommand, leaving the `Work` region (lines 11,095-16,470, where
  all STORY-2326 hunks land) in place until STORY-2326 PR-D merges.
- The first real crate (`aida-term` + `aida-cmdkit`, ~6k lines, 64 items widened
  to `pub`) buys layering and API hygiene, not build time; say so in its spec.
  The first build-time win is the sibling family cut, which cannot start before
  STORY-2326 PR-D, STORY-2538 slice B, STORY-2539 step 3a and STORY-2325 land.

## 2. Coupling evidence

### 2.1 Size (2026-11-07)

| Crate | Lines | Files | Note |
|---|---|---|---|
| aida-cli-lib non-test-dir files | 708,725 | 692 | `find -not -path '*/tests/*' \| xargs wc -l` |
| of which inline `#[cfg(test)] mod` blocks | 128,571 (18%) | 291 files have them | brace-matched scan |
| aida-cli-lib production | 580,154 | | 708,725 - 128,571 |
| aida-cli-lib `src/tests/` and `*/tests/` | 288,459 | 583 | `#[path]` test modules |
| aida-core | 173,583 | 195 | ~13.7k in `*_tests.rs` |
| aida-tui | 28,991 | | |
| aida-server | 14,792 | | |
| aida-cli (bin) | 6 | 1 | |

`lib.rs` is 782 lines with 453 `mod` declarations and still `include!`s
`lib_part1..6.rs` (lib.rs:772-777), 12,308 lines in total; lib_part5.rs holds
12 `handle_*` functions (4,850 lines). The 2026-10-10 advisor ruling on
STORY-1488 said the final state has no `lib_part*.rs`; that condition is unmet.

### 2.2 Top modules by production lines, with coupling

Columns: non-test-dir lines, inline-test lines, production lines, fan-in
(distinct modules that reference it), fan-out, `pub(crate)` occurrences.

| Module | Lines | Inline test | Prod | Fan-in | Fan-out | pub(crate) |
|---|---|---|---|---|---|---|
| gate/ (29 files) | 39,771 | 0 | 39,771 | 121 | 52 | 1,203 |
| queue_cmd.rs | 18,827 | 364 | 18,463 | 25 | 62 | 254 |
| session.rs | 14,992 | 1,277 | 13,715 | 56 | 47 | 222 |
| regen/ | 11,546 | | | 4 | 13 | 15 |
| requirements_cli.rs | 10,410 | 269 | 10,141 | 8 | 29 | 102 |
| doctor_cmd.rs | 16,406 | 6,663 | 9,743 | 12 | 55 | 101 |
| mcp.rs | 15,426 | 6,026 | 9,400 | 1 | 36 | 7 |
| pr_cmd.rs | 10,460 | 1,491 | 8,969 | 15 | 41 | 165 |
| agent_launch.rs | 8,111 | 211 | 7,900 | 16 | 34 | 250 |
| auto_complete.rs | 14,774 | 8,299 | 6,475 | 5 | 14 | 160 |
| forge.rs | 7,440 | 1,840 | 5,600 | 68 | 15 | 72 |
| init_cmd.rs | 7,408 | 1,906 | 5,502 | 14 | 17 | 44 |
| leases.rs | 5,279 | 0 | 5,279 | 34 | 19 | 199 |
| ownership/ | 5,793 | | | 15 | 11 | 125 |
| config_cmd.rs | 5,972 | 1,180 | 4,792 | 5 | 21 | 32 |
| db_reconcile.rs | 4,680 | 0 | 4,680 | 5 | 7 | 88 |
| maintenance_schedule.rs | 7,954 | 3,574 | 4,380 | 8 | 36 | 58 |
| phase_driver_trait.rs | 4,277 | 81 | 4,196 | 0 | 21 | 2 |
| pr_train.rs | 4,102 | 0 | 4,102 | 14 | 20 | 64 |
| trace.rs | 4,112 | 44 | 4,068 | 4 | 7 | 147 |
| status_awaiting.rs | 4,078 | 102 | 3,976 | 2 | 28 | 108 |
| ps_integrate.rs | 3,900 | 0 | 3,900 | 2 | 8 | 141 |
| shift.rs | 3,894 | 0 | 3,894 | 6 | 23 | 81 |
| session_start.rs | 3,932 | 54 | 3,878 | 6 | 23 | 67 |
| worktree_gc.rs | 3,807 | 40 | 3,767 | 16 | 19 | 135 |
| store_push_pull.rs | 4,195 | 505 | 3,690 | 3 | 8 | 77 |
| drain_state.rs | 3,617 | | | 18 | 11 | 106 |
| seat_clone.rs | 4,110 | | | 39 | 9 | 118 |
| launch/ | 4,010 | | | 15 | 10 | 129 |
| agent_registry.rs | 5,406 | 1,913 | 3,493 | 40 | 12 | 216 |
| awaiting_you.rs | 6,509 | 2,901 | 3,608 | 7 | 12 | 82 |
| history.rs | 5,776 | 1,693 | 4,083 | 10 | 16 | 74 |
| burndown.rs | 5,431 | 2,920 | 2,511 | 3 | 2 | 94 |
| deputy_shell/ | 5,254 | | | 4 | 9 | 121 |
| deputy/ | 4,362 | | | 6 | 11 | 123 |
| remote_create.rs | 5,150 | 1,966 | 3,184 | 7 | 13 | 18 |
| status_prs.rs | 4,728 | 3,477 | 1,251 | 8 | 11 | 98 |
| seat_occupancy/ | 4,377 | | | 2 | 5 | 60 |
| dispatch.rs | 3,242 | 0 | 3,242 | 7 | 16 | 61 |
| phase_driver.rs | 3,214 | 0 | 3,214 | | | |

Graph totals: 580 modules, 2,882 module-to-module edges, 12,986 `crate::`
references.

### 2.3 Fan-in hubs (what everything depends on)

| Hub | Fan-in | Refs | Lines | Distinct items used by others | Fan-out | Production deps |
|---|---|---|---|---|---|---|
| command_registry | 146 | 1,834 | 598 | 14 (CommandReg, Handler, DispatchCtx, StoreCtx, Invocation, Flow, CmdPolicy, try_parse_from, ...) | 135 | REGISTRY table imports every command module (command_registry.rs:10-...) |
| glyphs | 146 | 1,512 | 1,088 | 6 (Glyph, GlyphProfile, active_profile, active_theme, get) | 0 in production (only doc links at glyphs.rs:84,547) | pure leaf |
| stakeholder_role | 135 | 170 | 410 | 3 | requirements_cli::AddArgs (stakeholder_role.rs:210-270) | near-leaf |
| gate/ | 121 | 1,369 | 39,771 | 40 submodule paths | 52 | reaches UP: forge 59, events 55, pr_cmd 25, grant_keyring 23, merge_hold 20, session 15, forge_observed 14, seat_clone 13, ownership 13, pr_ship 11, orchestrator 10, auto_complete 7, awaiting_you 7, queue_cmd 6 |
| cli | 111 | 338 | 2,635 | 108 (legacy clap enums; `Command` enum at cli.rs:373 has 22 variants left) | 94 | |
| git_backend_cmd | 69 | 106 | 1,624 | 3 (exit_unsupported_on_git_backend, handle_git_backend_command) | 11 | |
| forge | 68 | 824 | 7,440 | 56 (`pub trait Forge` at forge.rs:855, Change* types) | 15 | reaches up: pr_train 14, ci_gate 9, pr_ship 7, usage 9 |
| test_env | 67 (inline tests) | 408 | 1,564 | 18 | 6 | gate 9, test_home 3, agent_registry 2, session 1, agent_handle 1, aida_bin 1 |
| session | 56 | 261 | 14,992 | 62 | 47 | |
| spec_arg | 53 | 119 | 60 | 4 | 0 | pure leaf |
| test_git | 46 | 165 | 423 | | 0 | pure leaf |
| agent_registry | 40 | 134 | 5,406 | 39 | 12 | |
| process_retry | 40 | 51 | 166 | 3 | test_exec (cfg(test)) | leaf |
| seat_clone | 39 | 328 | 4,110 | 62 | 9 | |
| events | 36 | 278 | 2,757 | 26 | 3 | schedule_ledger, gate |
| leases | 34 | 92 | 5,279 | 25 | 19 | |
| queue_cmd | 25 | 50 | 18,827 | 29 (parse_lookback 4 modules, parse_since_arg_at 3, parse_time_bound_at 3, QueueArgs 3, ...) | 62 | |
| mailbox_store | 25 | 101 | 842 | | 1 | |
| toon | 24 | 137 | 516 | 3 | 0 | pure leaf |
| process_probe | 24 | 68 | 21 | 10 (shim over aida_core::process_probe) | 0 | |
| cache_output | 19 | 36 | 260 | 3 | 0 | pure leaf |

Session's shared items are containment, not sessions: HeadlessVendor (20
modules), resolve_headless_vendor (9), process_contained (6),
ContainmentPosture (5), BwrapAvailability (5), bwrap_availability (5),
revoke_lease_child_grant (5), with_lease_write_lock (4). pr_cmd's shared items
are ship plumbing: ship_gate_spec_records_for_ids, fetch_change_info_via_resolved_forge,
pr_ship_target_branch, append_ship_activity_line (3 modules each). doctor_cmd
is used as a library by 12 modules (db_block: 3 store repairers; init_cmd:
ensure_store_tracked_runtime_gitignore; maintenance_schedule:
FLOOR_RECLAIM_BUDGET, disk_headroom_fail_reason; remote_create: same_remote;
sandbox_config_lock; scaffold_cmd: installed_codex_version; stranded_commits;
worktree_dismiss: scan_merged_agent_worktrees).

### 2.4 Strongly connected clusters

Tarjan SCC over the module graph: one component of 337 modules (593k lines),
one of 3. What-if cuts (edges removed, then SCC recomputed):

| Cut applied cumulatively | Largest SCC | Lines |
|---|---|---|
| baseline | 337 | 593k |
| 1. ignore `#[cfg(test)]` helper edges (test_env, test_git, test_home, test_exec, test_timing) | 323 | 576k |
| 2. command_registry types moved to a leaf (drop edges into command_registry) | 294 | 544k |
| 3. legacy cli enums moved to `<module>_args` (drop edges into cli) | 221 | 489k |
| 4. lib dispatch shell ignored | 221 | 489k |
| 5. gate stops reaching into 13 command modules | 221 | 489k |
| 6. forge stops reaching into pr_train/pr_ship/ci_gate/... | 221 | 489k |
| 7. shared items of session, queue_cmd, pr_cmd, doctor_cmd moved to service leaves | 190 | 406k |
| 8. ...plus agent_launch, seat_clone, leases, agent_registry, gate, forge | 145 | 291k |

Inside the residual 221-module SCC the densest sources are queue_cmd (44
in-SCC targets), doctor_cmd (41), session (35), pr_cmd (31), agent_launch (27),
maintenance_schedule (27), gate (26), status_awaiting (22). Conclusion: after
the two cheap framework cuts, the cycle is command modules calling each
other's helpers. That is exactly the EPIC-78 "use-case layer" extraction.

Caveat: 104 non-test-dir files start with `use crate::*;` (37 of them `_cmd`/
`_args` files, including queue_cmd.rs, doctor_cmd.rs, pr_cmd.rs,
requirements_cli.rs, agent_launch.rs, init_cmd.rs, config_cmd.rs, leases.rs).
Their dependencies on lib_part items are invisible to this matrix, so every
edge count above is a lower bound.

### 2.5 Candidate boundaries and widening cost

"Inbound items" = distinct `module::item` paths referenced from outside the set
= the minimum number of `pub(crate)` items that become `pub`. "Outbound" = what
the set would drag with it (or must invert) to become a lower crate.

| Candidate set | Modules | Lines | Dependents outside | Inbound items to widen | Outbound modules | Outbound items | Blocking outbound targets |
|---|---|---|---|---|---|---|---|
| A. terminal/helper leaves: glyphs, glyph_config, toon, output_format, cache_output, git_arg_guard, process_retry, network_retry, process_probe, spec_arg, usage, stakeholder_role | 12 | 4,898 | 285 | 64 | 7 | 9 | requirements_cli (stakeholder_role), cli (usage), kernel::find_project_root (glyph_config), rest cfg(test) |
| B. testkit: test_env, test_git (+test_home, test_exec, test_timing) | 2 (5) | 1,987 (3,376) | 95 | 18 | 6 | 7 | gate 9, test_home 3, agent_registry 2, session 1, agent_handle 1, aida_bin 1 |
| C. gate/ (authority kernel) | 1 dir | 39,771 | 121 | 40 | 52 | 144 | forge 59, events 55, pr_cmd 25, grant_keyring 23, merge_hold 20, session 15, seat_clone 13, ownership 13, pr_ship 11, orchestrator 10 |
| D. forge + git_backend_cmd + retries | 4 | 9,777 | 154 | 70 | 24 | 49 | events 19, forge_observed 16, pr_train 14, git_arg_guard 12, gate 11, ci_gate 9, pr_ship 7 |
| E. launch + vendors + agent_launch (+prompt, vendor_hints, codex_tool_env, agent_codex_background) | 7 | 15,790 | 32 | 44 | 40 | 119 | gate 58, agent_rotation 25, session 24, command_registry 22, seat_clone 19, containment 18 |
| F. ownership + leases + presence | 4 | 13,217 | 75 | 74 | 26 | 59 | seat_clone 16, glyphs 16, command_registry 16, mail_retention 14, gate 12, session 8 |
| G. mcp.rs | 1 | 15,426 | 1 (lib) | 1 | 36 | 99 | mailbox_store 30, mailbox_view 15, command_registry 13, agent_registry 10, worker 9, session_manifest 9, requeue 8 |
| H. cli + command_registry + lib (delivery shell) | 3 | 15,541 | 158 | 119 | 166 | 351 | everything (gate 131, glyphs 58, requirements_cli 49, ...) |
| I. events + mailbox_* + mail_* + coordination + notify | 8 | 13,408 | 81 | 108 | 26 | 67 | command_registry 30, agent_registry 18, glyphs 18 |
| J. queue family: queue_cmd, drain_state, drain_lock, dispatch | 4 | 27,088 | 50 | 59 | 73 | 233 | glyphs 188, gate 102, command_registry 34, events 33, orchestrator 32, integrate 30, session 24 |
| K. session family: session, session_start, session_end, session_reap, seat_clone, seat_occupancy | 6 | 32,740 | 74 | 131 | 68 | 209 | launch 56, test_env 55, gate 50, glyphs 47, contained_boundary 35, forge 27 |

Reading: A and B are real leaves once three small dependencies are cut
(stakeholder_role -> requirements_cli::AddArgs, usage -> cli, glyph_config ->
kernel::find_project_root; and test_env's six upward refs). C, D, I are the
kernel but today reach upward into command modules (C: 144 items). E, J, K
cannot be cut without the family extraction. G (mcp) is the one large module
that is a pure top: fan-in 1, so it can move ABOVE aida-cli-lib at the cost of
99 items made `pub`.

### 2.6 Churn and merge-conflict pressure (2026-10-01 to 2026-11-07)

`git log --since=2026-10-01 --name-only -- aida-cli-lib/src`: 857 commits
touched 648 non-test-dir files. Pressure = lines x distinct PRs.

| File | Lines | Commits | Distinct PRs | Pressure | Tools |
|---|---|---|---|---|---|
| queue_cmd.rs | 18,827 | 106 | 54 | 1,017k | agy/antigravity/claude/codex/human |
| session.rs | 14,992 | 102 | 52 | 780k | all |
| doctor_cmd.rs | 16,382 | 71 | 32 | 524k | all |
| pr_cmd.rs | 10,460 | 98 | 47 | 492k | 4 tools |
| agent_launch.rs | 8,111 | 84 | 38 | 308k | all |
| mcp.rs | 15,426 | 39 | 14 | 216k | all |
| forge.rs | 7,440 | 37 | 24 | 179k | 4 tools |
| requirements_cli.rs | 10,410 | 33 | 15 | 156k | all |
| maintenance_schedule.rs | 7,954 | 31 | 18 | 143k | all |
| init_cmd.rs | 7,408 | 36 | 18 | 133k | all |
| cli.rs | 2,635 | 78 | 42 | 111k | all |
| lib.rs | 782 | 264 | 123 | 96k | all |
| seat_clone.rs | 4,110 | 38 | 23 | 95k | claude/codex |
| auto_complete.rs | 14,774 | 14 | 6 | 89k | 4 tools |
| session_start.rs | 3,932 | 45 | 22 | 87k | all |

lib.rs is touched by 123 of the PRs in the window (one `mod` line each): the
command-registry convention removed the enum churn but every new module still
lands a line in lib.rs. auto_complete.rs is large but quiet (6 PRs); doctor_cmd
and queue_cmd are both large and hot.

## 3. Build baseline (dell1, 2026-11-07 14:38 MST)

Command: `cargo build --profile agent -p aida-cli-lib --timings` under
`/usr/bin/time -v`, clean target dir, commit 79603aff6b. Host: 20 cores, rustc
1.99.1 (2026-10-13), `rustc-wrapper = sccache` in `~/.cargo/config.toml`
(third-party deps may be sccache hits; workspace crates are `incremental =
true` in `[profile.agent]`, Cargo.toml:81-93, which sccache does not cache).
Load average at finish 0.41/1.52/2.99, so the other lanes had mostly drained;
treat the dependency phase as sccache-assisted and the two aida units as real.

| Measurement | Value |
|---|---|
| Wall time (`Finished` / `Total time`) | 162.6 s (2m 42.6s) |
| CPU | user 1,425.8 s, sys 35.6 s, 898% average, max RSS 7.48 GB |
| Units | 381 dirty, max concurrency 22 (jobs=20) |
| aida-cli-lib | start 40.6 s, duration 121.9 s: frontend 73.1 s, codegen 48.9 s |
| aida-core | start 22.9 s, duration 30.1 s: frontend 15.3 s, codegen 14.8 s |
| aida-tui | start 38.2 s, 4.5 s (frontend 2.3 s); cli-lib waited for its rmeta (`default = ["tui"]`, aida-cli-lib/Cargo.toml) |
| Head of critical path | libsqlite3-sys build script 3.5 -> 22.5 s (19.0 s, compiles bundled SQLite), rusqlite 22.5 -> 23.1 s |
| Critical path | libsqlite3-sys build.rs (19.0) -> rusqlite (0.6) -> aida-core frontend (15.3) -> aida-tui rmeta (2.4) -> aida-cli-lib (121.9) = 162.5 s of 162.6 |
| Concurrency | 337 units active in 0-10 s, 59 in 10-20 s, 4 in 20-30 s, 2 in 30-60 s, then exactly 1 unit (aida-cli-lib) from 60 s to 162.6 s |

The single aida-cli-lib unit is 75% of wall time and 100 s of it runs alone on
a 20-core machine. Per compiled line, aida-core is 0.19 ms (160k compiled
lines, 30.1 s) and aida-cli-lib 0.21 ms (580k, 121.9 s): roughly linear, so
crate size predicts unit time.

Prior baseline for comparison (TASK-1537, 2026-10-04, local box, 406k lines,
sccache off, dev profile; `docs/plans/2026-09-26-story-1488-timings.md:30-39`):
clean dev 153.4 s; one-line edit rebuild 17.1 s (leaf) to 20.0 s (lib.rs);
`cargo test -p aida-cli-lib --no-run` clean 116.5 s, after an edit 30.3-34.4 s;
`cargo check` after an edit 11.8 s. The doc's reading (lines 50-61): the ~17 s
floor is crate-wide front-end plus link that a module split does not remove.

CI (run 42415012131 on main, 2026-11-07 21:15Z, green): gates 18m10s (live
bwrap gate 6.8 min, build all 2.5 min, protobuf drift 1.5, clippy 1.2),
lib-1 16m43s (Run tests 13.0 min, build aida bin 2.8 min), lib-0 12m35s,
contracts 10m24s, cli-0 9m00s, rest 7m40s, cli-1 6m40s. The last eight green
main runs took 11.8 to 24.8 min (`gh run list`). The lib shards are the
long pole after the bwrap gate, and the lib test binary compiles all 997k lines.

### 3.1 Estimated speedup per split

Model: rustc's stable front end is one thread per crate (the parallel front end
is still nightly `-Z threads`); codegen is parallel across codegen units
(release-derived profile, 16 CGUs; 256 when incremental) and cargo pipelines
dependents to start at the dependency's rmeta (end of frontend). So the
critical path = sum of frontends along the longest crate chain + codegen of the
last crate; sibling crates overlap. Frontend cost ~0.126 ms/line (73.1 s /
580k), codegen ~0.084 ms/line at 20 cores.

| Scenario | Crate shape | Clean critical path (cli-lib portion) | Clean wall est. | One-line edit rebuild | Targeted `cargo test -p <crate>` build |
|---|---|---|---|---|---|
| Today | 1 x 580k | 73 + 49 = 122 s | 162.6 s measured | ~17-25 s (floor is whole-crate frontend) | compiles 997k lines in one binary |
| S1: leaves only (aida-term + aida-cmdkit ~6k, aida-testkit ~3k) | 1 x 574k + leaves | 122 s | ~162 s (0%) | unchanged for edits in the big crate; an edit in a leaf rebuilds everything above | unchanged |
| S2: chain kernel 100k -> app 430k -> cli 50k | chain | 12.6 + 54 + 6.3 + codegen(cli) ~4 = 77 s + app codegen overlaps | ~120 s (-25%, codegen of app overlaps cli frontend only partly) | edit in app: ~54 s frontend + codegen + cli 6 s: WORSE than today for most edits; edit in cli: ~8 s | app test binary still ~750k lines |
| S3: kernel 100k -> 5 sibling families ~85k each -> cli 50k | DAG | 12.6 + 10.7 + 6.3 + ~4 = 34 s (+ family codegens overlap each other) | ~95-105 s (-35-40%) | edit in one family: ~11 s frontend + 2-5 s incremental codegen + cli 6 s + link ~= 20-25 s; edit in kernel: ~45 s (rebuilds all families) | family test binary ~150-200k lines: ~5x smaller; CI lib shards become per-crate jobs |
| G: aida-mcp above aida-cli-lib | adds a 9.4k-line crate on top | +1.5 s frontend on the chain | ~164 s | mcp edit: ~2 s + link (today: whole crate) | mcp tests (157) compile apart from the 997k binary |

The honest reading: a chained layering (S2) is a build-time loss for edits in
the big middle crate and a wash on clean builds. The only shape that pays is
S3 (several sibling family crates under a kernel), and it requires the family
extraction first. The incremental-build win of S3 is modest (floor drops from
~25 s to ~20 s because the cli crate above still recompiles); the test-build
and CI-shard wins are large (per-crate test binaries). The leaf crates (S1)
are justified by layering (EPIC-78 ports, `aida-server`/`aida-tui` reuse of
glyphs/toon output) and by shrinking the SCC, not by time; the STORY-1488
"keep only if faster" gate would reject them and should be replaced with "no
slower than run-to-run spread" for leaves.

A 19 s clean-build item unrelated to the split: `libsqlite3-sys` compiles the
bundled SQLite amalgamation at the head of the critical path (3.5-22.5 s);
build scripts are not sccache-cached. Using the system SQLite in the agent/dev
profile would cut every clean build by ~19 s. Out of scope here; listed in
section 9.

## 4. Crate map (EPIC-78 layers reconciled with the build cut)

Dependency direction: each crate depends only on crates below it.

| Layer | Crate | Contents (modules) | Build role |
|---|---|---|---|
| Delivery | `aida-cli` (bin, exists) | 6-line main | unchanged |
| Delivery | `aida-mcp` (new) | mcp.rs, schema, McpProfile; registered into the CLI through a function pointer that `aida-cli-lib` exposes (`REGISTRY` is a `static` in command_registry.rs:10, so the bin supplies the MCP handler) | top-of-graph; isolates 157 tests and 15.4k lines |
| Delivery | `aida-cli-lib` (shrinks) | cli.rs, command_registry REGISTRY, the 93 `*_args.rs` (12,702 lines, deps outside cli/command_registry are only spec_arg 76 refs), help, rendering of Outcome in human/toon/json, lib dispatch (lib_part1-6 dissolved) | thin top: target 40-60k lines |
| Delivery (exist) | `aida-server`, `aida-tui` | unchanged, gain a dependency on `aida-app-*` to reach queue/review/session/merge-hold | `aida-tui/src/cmd_palette.rs:75-78` already notes it needs `command_registry::built_command()` from aida-cli-lib |
| Application | `aida-app-queue` | queue_cmd core (List/Add/Remove/Move/Prune/Gc/Next/Done/Work/Progress/Rework/Recover/Integrate as use cases), drain_state, drain_lock, dispatch, auto_complete, orchestrator, burndown, requeue | sibling |
| Application | `aida-app-review` | review_*, pr_cmd ship core, pr_ship, pr_train, pr_lookup, merge_hold ops, awaiting_you, status_prs | sibling; owned by STORY-2325 until it lands |
| Application | `aida-app-session` | session listing and leases ops, session_start/end/reap, seat_clone, seat_occupancy, worktree_gc, worktree_lock, runaway_seats | sibling |
| Application | `aida-app-launch` | launch/, vendors/, containment/, session.rs containment + headless-vendor halves, agent_launch core, phase_driver, compete | sibling; owned by STORY-2326/2538/2539 until they land |
| Application | `aida-app-ops` | doctor checks, maintenance_schedule, db_reconcile, init, config, regen/, store_push_pull, remote_create | sibling |
| Kernel | `aida-kernel` | gate/ (29 files), events, forge (trait + gh/glab adapters until EPIC-78 separates adapters), forge_observed, kernel.rs, trusted_config, grant_keyring, merge_hold core, mailbox_store, mail_retention, ownership/ + leases types (post STORY-2325), process_probe shim | one unit ~100k; the one crate every family depends on |
| Leaf | `aida-cmdkit` | CommandReg, Handler, DispatchCtx, StoreCtx, Invocation, Flow, CmdPolicy, GlobalArgs, spec_arg, stakeholder_role (minus the AddArgs coupling) | clap-dependent leaf so families can register commands without depending on the top |
| Leaf | `aida-term` | glyphs, glyph_config, toon, output_format, cache_output, git_arg_guard, process_retry, network_retry, usage (telemetry) | no clap; usable from aida-server/aida-tui |
| Dev leaf | `aida-testkit` | test_env, test_git, test_home, test_exec, test_timing (minus six upward refs) | dev-dependency of every crate above it |
| Domain | `aida-core` (exists) | domain + store + exec_context (STORY-2538) + launch_policy + vendors posture (STORY-2539) + proc_identity/liveness (STORY-2537) | unchanged |

Which split comes first and why: the leaves (`aida-cmdkit`, `aida-term`,
`aida-testkit`) come first because they are the only cuts with zero
in-flight collision, they collapse the SCC by 29+ modules (cut 1-2 in section
2.4), and every later crate needs them below it. They buy no build time; the
spec must say so. The kernel cut follows once gate's 144 upward items are
inverted (section 7, PR 8). The family cuts come last and are sequenced behind
STORY-2326 PR-D (launch), STORY-2538 slice B and STORY-2539 step 3a (both
ruled "after PR-D" on 2026-11-07), and STORY-2325 (review/PR trail). The MCP
delivery crate is the one large cut available early: fan-in 1, 99 items to
widen, and it is EPIC-78's first "delivery calls the same function" proof.

EPIC-78 metric refresh (2026-11-07, non-test-dir files): `println!/eprintln!`
7,648 (was 6,653 on 2026-10-12), `Command::new` 740 (was 780), files using
clap 263 of 692 (was 22 of 355; the `<module>_args.rs` convention put clap
derive types next to every command). The "aida-app never depends on clap"
guard therefore means the `_args.rs` files stay in the delivery crate and the
use-case functions take plain structs.

## 5. Leaf-crate plan for the shared helper web

Items are the exact `module::item` paths referenced from other modules today
(the widening list from section 2.5, set A plus the command framework types).

### 5.1 `aida-term` (no clap)

| Module | Lines | Items that become `pub` | Production deps to cut |
|---|---|---|---|
| glyphs.rs | 1,088 | Glyph, GlyphProfile, active_profile, active_theme, get (+ the module) | none (glyphs.rs:84 and :547 are doc links only) |
| glyph_config.rs | 265 | Scope, config_path_for | `crate::find_project_root` (kernel.rs:163) x2: pass the project root as a parameter |
| toon.rs | 516 | parse_table, scalar, table_raw | none |
| output_format.rs | | agent_output_mode, agent_output_mode_from, parse_output_format, toon_list_cell, with_local_id_column | check for cli::GlobalArgs use |
| cache_output.rs | 260 | json_pretty, print_tracked, println_tracked | none |
| git_arg_guard.rs | 155 | END_OF_OPTIONS, is_hex_sha, is_option_like, is_shell_safe_branch_name, reject_option_like | gate (1 ref): invert |
| process_retry.rs | 166 | RetryEtxtbsy, command_output_retrying_etxtbsy, command_status_retrying_etxtbsy | test_exec (cfg(test)) |
| network_retry.rs | 547 | NoopSink, RetryConfig, RetryEvent, RetrySink, StderrSink, classify_transient, default_transient_patterns, run_with_retry, run_with_retry_checked | process_retry (same crate) |
| usage.rs | 974 | UsageEvent, append_value, arm_phases, is_enabled, parse_telemetry_enabled, phase, read_events, record_phase | cli (1 ref) and test_env: invert the cli ref |

Keep every existing path valid with `pub(crate) use aida_term::glyphs;` etc. in
aida-cli-lib's lib.rs so the 156 files / 1,513 `crate::glyph*` references do
not change. Ledger impact is small: glyphs appears twice in
scripts/exec-context-legacy.json and once in scripts/config-trust.toml,
glyph_config once each in store-writers.toml, config-trust.toml and
exec-context-legacy.json, output_format 4 and 2, network_retry 1 and 1; all
other leaf files have zero ledger entries (grep of the four ledgers).

### 5.2 `aida-cmdkit` (clap allowed)

Move from command_registry.rs: CommandReg, Handler, DispatchCtx, StoreCtx,
Invocation, Flow, CmdPolicy, try_parse_from's types (14 distinct items used by
146 modules). Move from cli.rs: GlobalArgs. Add spec_arg.rs (60 lines, 4
items, 53 dependents, 76 refs from `_args.rs` files) and stakeholder_role.rs
(410 lines, 3 items, 135 dependents) after breaking its
`requirements_cli::AddArgs` coupling (stakeholder_role.rs:210-270: make
`apply_requester_intake` generic over a small trait or move the two functions
to requirements_cli). `REGISTRY` itself (the table at command_registry.rs:10,
fan-out 135) stays in aida-cli-lib. Do this as an in-crate module first (PR 2
in section 7) with `pub(crate) use cmdkit::*;` inside command_registry.rs so
no import line changes.

### 5.3 `aida-testkit` (dev-dependency)

test_env.rs 1,564, test_git.rs 423, test_home.rs 953, test_exec.rs 126,
test_timing.rs 310, declared `#[cfg(test)]` at lib.rs:704-715; used by 186
files under src/tests and 306 files overall; exec-context-legacy.json has 52
entries for test_env.rs. Six upward references must leave first: test_env ->
gate (9 refs), test_home (3), agent_registry (2), session (1), agent_handle
(1), aida_bin (1); test_git already uses only `aida_core::git_ops`
(test_git.rs:19, :238). The rule that makes this work: a dev-dependency may
not depend on the crate under test, so kernel-specific fixtures (gate grant
fixtures, agent_registry entries) stay in the kernel crate as its own
`#[cfg(test)]` helpers, and testkit holds only env/home/git/exec/timing
fixtures over aida-core.

## 6. Split plan per hotspot file (merge-conflict relief before any crate split)

Production/test line counts from the brace-matched scan; `#[test]` counts from
`grep -c '#\[test\]'`.

| File | Prod / inline test | Structure found | Proposed sub-modules (by concern) | In-flight collision |
|---|---|---|---|---|
| queue_cmd.rs | 18,463 / 364 (13 tests) | `handle_queue_command` is one match from 1,521 to 6,965 (5,444 lines): List 1,529-3,285 (1,756), Add 3,308-3,713, Remove 3,713-3,883, Move 3,883-4,154, Prune 4,177-4,328, Gc 4,328-4,507, Next 4,507-5,052, Done 5,052-5,676, Work 5,676-6,825, Progress/Rework/Recover/Integrate 6,825-6,965; then handle_queue_progress 7,337, handle_queue_rework 8,212, handle_queue_work 12,075-16,470, handle_queue_recover 16,470, handle_queue_integrate 17,270. 161 pub(crate) fns, 27 private | `queue_cmd/mod.rs` (dispatcher, ~300 lines), `list.rs`, `add.rs`, `remove_move.rs`, `prune_gc.rs`, `next.rs`, `done.rs`, `work.rs`, `progress.rs`, `rework.rs`, `recover.rs`, `integrate.rs`; `time_args.rs` leaf for parse_lookback / parse_since_arg_at / parse_time_bound_at (used by 4 other modules) | All STORY-2326 hunks are in 11,095-15,096 (handle_queue_work). Move everything except the Work arm and handle_queue_work now; move those after PR-D |
| doctor_cmd.rs | 9,743 / 6,663 (224 tests in 29 cfg(test) blocks interleaved from line 450) | `handle_doctor` 247 lines; 133 private fns; 12 external dependents use it as a library (section 2.3) | Step 1: lift the 29 inline test blocks to `src/tests/doctor_cmd_*_tests.rs` (needs `pub(crate)` on the private items they touch). Step 2: `doctor/` directory: `store_repair.rs` (doctor_migrate_counter_scope_in_store, repair_dangling_relationships, tombstone_stale_blocks -> used by db_block), `worktrees.rs` (scan_merged_agent_worktrees, run_merged_agent_worktree_gc at :530), `disk.rs` (FLOOR_RECLAIM_BUDGET, disk_headroom_fail_reason), `remote.rs` (same_remote), `sandbox.rs` (heal_doctor_sandbox_config_lock), `vendors.rs` (installed_codex_version), `report.rs` | STORY-2326 PR-B touched lines 2,030, 2,576 and 16,283 only; PR-D may touch again. Low |
| session.rs | 13,715 / 1,277 (144 tests) | 200 pub(crate) fns, 89 private; shared surface is containment and headless spawning (section 2.3) | `session/containment.rs` (ContainmentPosture, BwrapAvailability, bwrap_availability, process_contained, drain_contained, contained_* , aida_contained_env), `session/headless.rs` (HeadlessVendor, resolve_headless_vendor, spawn_vendor_headless_with_seat, spawn_analysis_headless, assert_headless_posture), `session/lease_lock.rs` (with_lease_write_lock, revoke_lease_child_grant, revoke_headless_child_grant), `session/list.rs` (list, print_json, print_leases, print_group) | HIGH: 6 of 6 STORY-2326 commits and 2 STORY-2539 commits touch it; STORY-2538 slice B and STORY-2539 3a are both sequenced after PR-D. Do not touch before those land |
| mcp.rs | 9,400 / 6,026 (157 tests in one block 9,316-15,304) | 354 fns; tool_* 5 fns / 1,250 lines; handle_request :1,114, tools/call :1,180, resources :1,387-1,497; binary identity/respawn via `Command::new(exe)` at :259 and :297 | Step 1: lift the test block to `src/tests/mcp_tests.rs` (6k lines out). Step 2: `mcp/{transport.rs, tools.rs, resources.rs, profile.rs, identity.rs}` | 14 PRs since 10-01, none in-flight. Low |
| auto_complete.rs | 6,475 / 8,299 (256 tests in one block 6,476-14,774) | groups: drain 7 fns / 1,074 lines, orchestrate 3 / 976, finish 10 / 829, recovery 2 / 338, watchdog 3 / 109 | Step 1: lift the test block (8.3k lines). Step 2: `auto_complete/{drain.rs, orchestrate.rs, finish.rs, recovery.rs, watchdog.rs}`; `PhaseDriver` trait stays (phase_driver_trait.rs implements it, STORY-1488 Q9 split) | 6 PRs since 10-01; phase_driver*.rs are in STORY-2326's path, auto_complete.rs is not. Low |
| pr_cmd.rs | 8,969 / 1,491 (37 tests) | pr 20 fns / 3,194 lines, ship 16 / 558, run_human 268, fetch 5 / 187, derive 5 / 157, rebase 4 / 133; 43 items used by 15 modules | `pr_cmd/{ship.rs, ship_gate.rs (ship_gate_spec_records_for_ids, probe_ship_drive_block, pr_ship_hold_root), fetch.rs (fetch_change_info_via_resolved_forge), rebase.rs, human.rs}` | STORY-2325 touched pr_cmd.rs in 2 of 5 commits; #3619 open. Wait for 2325 |
| agent_launch.rs | 7,900 / 211 | handle_session 309 lines, dispatch 6 fns / 145, 151 pub(crate) fns | `agent_launch/{dispatch.rs, session_handler.rs, codex.rs}` | STORY-2326 touched it in 5 of 6 commits. Wait for PR-D |
| requirements_cli.rs | 10,141 / 269 | 63 top-level fns; clap AddArgs and the add/del/edit handlers | `requirements_cli/{add.rs, edit.rs, del.rs, intake.rs}` | 15 PRs; stakeholder_role depends on AddArgs. Low |
| maintenance_schedule.rs | 4,380 / 3,574 (one block 4,307-7,880) | | lift the test block | 18 PRs. Low |
| awaiting_you.rs, burndown.rs, init_cmd.rs, agent_registry.rs, history.rs, forge.rs | 3,608/2,901; 2,511/2,920; 5,502/1,906; 3,493/1,913; 4,083/1,693; 5,600/1,840 | | lift inline test blocks | forge.rs 24 PRs; others low |

Relocating the inline test blocks alone removes ~30k lines from the six
largest files and does not change any production line, which is the
STORY-770 pattern (`#[cfg(test)] #[path = "tests/NAME.rs"] mod NAME;`).
The cost is widening the private items those tests reach (doctor_cmd 133
private fns, mcp 70, maintenance_schedule 73, session 89) to `pub(crate)`,
which is invisible outside the crate.

## 7. Incremental PR sequence

Prerequisites name the in-flight slice that must merge first. "Today" means
no file in the PR is in any in-flight spec's path (section 2.6 and the
`git log --grep` file lists: STORY-2326 = session.rs, phase_driver.rs,
phase_driver_trait.rs, launch/*, compete_cmd.rs, agent_launch.rs,
queue_cmd.rs Work region, codex_tool_env.rs, autopilot_intake.rs,
agent_codex_background.rs, aida-core launch_policy.rs; STORY-2325 =
ownership/*, session_start.rs, pr_train.rs, pr_cmd.rs; STORY-2539 = vendors/*,
launch/profile.rs, vendor_hints.rs, mail_wake.rs, session.rs; STORY-2538 =
aida-core exec_context.rs then launch/context.rs, launch/resources.rs;
STORY-2537 S6 (#3622) = aida-core proc_identity, ownership/; STORY-2571 =
new host-service modules, #3627/#3628 touch containment/launch).

| # | PR | What moves | Prerequisite | Risk | Start |
|---|---|---|---|---|---|
| 1 | Lift inline tests (batch A) | doctor_cmd.rs (6,663), mcp.rs (6,026), auto_complete.rs (8,299), maintenance_schedule.rs (3,574), awaiting_you.rs (2,901), burndown.rs (2,920) -> `src/tests/<module>_inline_tests.rs`; private items they use become pub(crate) | none | Low. Pure moves; `scripts/split-lib/verify_pr.sh` V2 multiset check applies (it still reads lib_part*.rs, verify_pr.sh:24-29) | TODAY |
| 2 | `cmdkit.rs` in-crate | 14 framework types + GlobalArgs out of command_registry.rs/cli.rs; `pub(crate) use cmdkit::*` re-export keeps all 146 importers unchanged | none | Low. Zero import churn; SCC 323 -> 294 | TODAY |
| 3 | queue_cmd/ directory | Arms List..Done and Progress/Rework/Recover/Integrate plus time_args.rs; Work arm and handle_queue_work (11,095-16,470) stay in `queue_cmd/work.rs` as a verbatim move only after PR-D | partial: none; Work region: STORY-2326 PR-D | Medium. 54 PRs touched this file since 10-01; land in a quiet window with a rebase note | TODAY (non-Work arms) |
| 4 | Explicit imports | Replace `use crate::*;` in the 104 files with explicit imports, excluding the in-flight files; add a ratchet forbidding new glob imports | none | Low per file, broad. Makes the dependency matrix exact; required before any ratchet on layer direction | TODAY (about 85 of 104 files) |
| 5 | `aida-term` + `aida-cmdkit` crates | Section 5.1-5.2; re-exports keep paths; cut glyph_config -> kernel, usage -> cli, stakeholder_role -> AddArgs | PR 2, PR 4 | Low-medium: 64 items widened; four ledgers need ~14 path rewrites; cross-worktree cargo cache invalidation on new crate names (noted in the 2026-07-17 scoping doc) | after PR 2 |
| 6 | `aida-testkit` dev-dep crate | Section 5.3; the six upward refs move to owning modules' test helpers | PR 5 | Medium: 306 importers (re-export keeps paths); exec-context-legacy.json has 52 test_env entries | after PR 5 |
| 7 | Lift inline tests (batch B) + session.rs split | session.rs by concern (section 6); init_cmd, agent_registry, history, forge test blocks | STORY-2326 PR-D, STORY-2539 3a, STORY-2538 slice B | High until they land; then medium | after those merge |
| 8 | Gate downward-only ratchet | Python ratchet (same shape as scripts/check-exec-context.py) counting gate -> {pr_cmd, session, seat_clone, queue_cmd, auto_complete, awaiting_you, orchestrator, pr_ship, pr_train, drain_lock, drain_state, store_sync, merge_hold} edges; ceiling 144 items, burned down over 3-4 PRs by moving functions into gate or behind traits | PR 4 | Medium: gate/seat_authority.rs (9 PRs) and ownership judge are in STORY-2325's orbit | ratchet today; burn-down after 2325 |
| 9 | doctor/ directory + helper relocation | Section 6 row 2 step 2; the 9 library helpers go to db_block/worktree_gc/maintenance_schedule/remote_create/scaffold | PR 1 | Low-medium | after PR 1 |
| 10 | Review/PR service extraction | 43 pr_cmd items used by 15 modules into `pr_service.rs`; pr_cmd/ directory (section 6) | STORY-2325 completed | Medium | after 2325 |
| 11 | `aida-kernel` crate | gate/, events, forge, forge_observed, kernel.rs, trusted_config, grant_keyring, merge_hold core, mailbox_store, mail_retention, ownership+leases types; widen gate 40 submodule paths, forge 56, events 26, leases 25, ownership 16 | PR 8 at zero, PR 10, STORY-2325 and STORY-2537 S6 merged | High: first cut through the SCC; must be measured (clean + edit rebuild) before/after | later |
| 12 | `aida-mcp` crate (first delivery cut, EPIC-78 slice candidate) | mcp.rs + schema above aida-cli-lib; 99 cli-lib items widened; `aida mcp` handler injected into REGISTRY from the bin | PR 1 (tests lifted) | Medium: MCP surface (72 tool names in mcp.rs) must stay byte-identical; existing golden tests cover tools/list; the respawn identity check (mcp.rs:201-267) must keep reading the same binary | after PR 1; can run in parallel with 5-6 |

The family crates (`aida-app-*`) follow PR 11 and 12 and are not numbered
here: each is a measured slice under EPIC-78 once the owning in-flight story
is closed (launch last, per the operator ruling that STORY-2326 lands last).

## 8. Risks, and what NOT to split

- Do not touch launch/, vendors/, containment/, session.rs, agent_launch.rs,
  phase_driver*.rs, compete_cmd.rs before STORY-2326 PR-D, STORY-2538 slice B
  and STORY-2539 step 3a merge. The advisor rulings of 2026-11-07 (STORY-2538
  comment 40, STORY-2539 comment 17) already serialize those three on the same
  files; a split PR would be a fourth writer.
- Do not touch ownership/, pr_train.rs, pr_cmd.rs, session_start.rs before
  STORY-2325 closes (#3619 open) and STORY-2537 S6 (#3622) merges.
- Do not split aida-core, and do not split gate/ itself: it is 39.8k lines but
  the one coherent authority kernel; its problem is upward edges, not size.
- Do not do the chain-only layering (S2 in section 3.1): it makes most
  incremental builds slower.
- MCP surface stability: 72 tool names and the JSON-RPC shapes in mcp.rs must
  not change when the module moves; the `aida mcp` self-identity check
  (mcp.rs:201-267, :297) re-execs the installed binary and compares the version
  stamp, so `build.rs` stamps must survive any crate move (same gotcha as the
  2026-07-17 scoping doc).
- Path-keyed ratchets and inventories break on every file move:
  scripts/store-writers.toml (2,291 `aida-cli-lib/src/` entries),
  scripts/exec-context-legacy.json (855), scripts/portability-allowlist.txt
  (165), scripts/config-trust.toml (82), scripts/doc-path-exceptions.json (62),
  scripts/git-maintenance-baseline.txt (15), scripts/ci-gate-tiers.toml (1);
  14 scripts reference the crate path (check-removed-flags.sh,
  check-store-writers.py, test-timings.py, check-ci-gates.py,
  check-template-commands.py, test-touched.py, check-tui-seam.sh, publish.sh,
  check-trace-headers.py, analyze_code_growth.py, release.sh,
  check-generated-proto.sh, ci-test-shards.py, check-help-golden-cochange.sh);
  45 test files under src/tests hard-code `aida-cli-lib/src` or `src/`
  (including story_2326_launch_ratchet_tests.rs,
  story_2401_forge_read_ratchet_tests.rs, task_2032_launcher_main_ratchet_tests.rs,
  task_2143_clone_git_ratchet_tests.rs, task_1750_liveness_primitive_guard_tests.rs,
  node_alias_display_ratchet_tests.rs); 14 production files use
  CARGO_MANIFEST_DIR; two fixtures are path-keyed
  (src/tests/story_2539_vendor_literals.tsv,
  src/tests/fixtures/proc_identity_legacy_baseline.json). Every move PR must
  regenerate the ledgers (`--write-ledger`), which turns a pure move into a
  noisy diff. Mitigation: a one-time `scripts/relocate-ledger-paths.py`
  (old path -> new path map) applied in the same commit as each move, and
  ratchet tests that walk the workspace by `cargo metadata` rather than one
  crate directory before any crate is created.
- test_env sharing: a dev-dependency crate cannot depend on the crate it
  tests, so the kernel's and families' own fixtures must live inside those
  crates; today test_env reaches gate (9 refs) and session/agent_registry/
  agent_handle. Inline `#[cfg(test)]` modules in a lower crate cannot use
  helpers that live in a higher one; this is why batch-A test lifting (PR 1)
  must come before the kernel cut, not after.
- Glob imports: 104 files hide their edges; the matrix in section 2 is a lower
  bound and layer-direction ratchets are inaccurate until PR 4 lands.
- Cross-worktree cargo caches and sccache: new crate names invalidate every
  cached aida-cli-lib artifact fleet-wide once; `[profile.agent]` is
  incremental so sccache never caches the workspace crates anyway
  (Cargo.toml:85-91). Expect a one-time rebuild storm per new crate.
- Feature pass-through: `default = ["tui"]` and `remote` live in
  aida-cli-lib/Cargo.toml; each new crate above it must forward them or
  `aida-tui` silently drops out of the build (the clean build waited on
  aida-tui's rmeta at 40.6 s).
- `codegraph` skips files over 1 MiB; none exceed that today (largest source
  is queue_cmd.rs at 18.8k lines), but `codegraph callers` returned no result
  for a qualified path that `rg` resolves to 26 call sites, so use `rg` to
  confirm any "no callers" claim during the moves.
- Churn breadth: lib.rs is touched by 123 PRs in five weeks (one `mod` line
  each). Dissolving lib_part*.rs and moving `mod` declarations into directory
  `mod.rs` files reduces that only when modules move into directories (PR 3,
  7, 9), so expect lib.rs conflicts to continue until then.

## 9. Open questions for the advisor

1. Order of the first crate: leaves first (`aida-term`/`aida-cmdkit`, hygiene,
   0% time) or `aida-mcp` first (biggest single-file win, first EPIC-78
   delivery proof)? Both can run in parallel after PR 1 and PR 2.
2. Replace STORY-1488 amendment 5 ("keep only if measured faster") with "no
   slower than run-to-run spread" for leaf and delivery crates, keeping the
   faster-only gate for the kernel and family cuts?
3. Accept the ledger re-keying approach (one `relocate-ledger-paths.py` per
   move, applied in the move commit) instead of regenerating each ledger from
   scratch, so pure-move PRs stay reviewable?
4. gate's 144 upward items: invert through traits in gate (EPIC-78 ports) or
   move the called functions down into gate (which is already 39.8k lines)?
   A ruling sets the shape of PR 8.
5. Should the crate-split work be filed as children of EPIC-78 (so it stays
   deferred until STORY-2326 completes) or as a separate story for PRs 1-6,
   which do not touch the in-flight files and can land now?
6. The queue family: is `queue_cmd/work.rs` owned by STORY-2326 until PR-D,
   or may PR 3 move the Work region verbatim once PR-D is in review?
7. Dev-profile SQLite: allow a non-bundled `rusqlite` feature for the agent and
   dev profiles to take the 19 s `libsqlite3-sys` build script off the clean
   critical path?
8. Dissolve `lib_part1..6.rs` (lib.rs:772-777) as part of PR 4, closing the
   2026-10-10 TASK-1538 condition, or keep it as codegraph scaffolding?
9. Should `aida-server`/`aida-tui` wait for the family crates, or take a
   dependency on `aida-cli-lib` now (the TUI command palette already wants
   `command_registry::built_command()`), accepting the larger build for those
   two binaries?

## Appendix: commands behind the numbers

- Sizes: `find aida-cli-lib/src -name '*.rs' -not -path '*/tests/*' | xargs cat | wc -l` (708,320 before counting generated files; 708,725 with them), same with `-path '*/tests/*'` (288,459); brace-matched inline-test scan over the same set (128,571).
- Graph: parse of `crate::X` / `crate::{...}` / `use crate::X::item` in non-test-dir files, Tarjan SCC, what-if edge removal.
- Churn: `git log --since=2026-10-01 --format='COMMIT %h|%s' --name-only -- aida-cli-lib/src`; PR = trailing `(#N)`.
- In-flight files: `git log --since=2026-10-25 --format= --name-only --grep=<SPEC> -- aida-cli-lib/src aida-core/src`; STORY-2326 hunks in queue_cmd.rs via `git log -p --grep=STORY-2326 -- aida-cli-lib/src/queue_cmd.rs | grep '^@@'` (all between 11,095 and 15,096).
- Build: `cargo build --profile agent -p aida-cli-lib --timings` on dell1; `UNIT_DATA` parsed from `~/timing-target/cargo-timings/cargo-timing.html` (sections `frontend`/`codegen` per unit); `/usr/bin/time -v` from `~/lanes/timings-build.log`.
- CI: `gh run list --workflow ci.yml --branch main --status success --limit 8`; `gh run view 42415012131 --json jobs`.
- Ledgers: `grep -c 'aida-cli-lib/src/' scripts/*.toml scripts/*.json scripts/*.txt`; `rg -l 'aida-cli-lib/src|join("src")' aida-cli-lib/src/tests/*.rs`.
