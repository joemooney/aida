# Git sync & review workflow

Read-on-demand detail moved out of the always-in-context `.claude/AIDA.md` so a
consumer project pays for it only when it's relevant. trace:TASK-636 | ai:claude

## When `aida pull` refuses (divergent branches)

`aida pull` is two operations in one: a `git pull` of your code branch and a
`git pull --rebase` of the orphan `aida-store` branch. The two legs are
deliberately asymmetric:

- **Code leg**: `git pull --ff-only` — refuses if the branch has diverged from
  origin. Won't surprise your working tree with an auto-rebase.
- **Store leg**: `git pull --rebase` — store conflicts are rare and the worktree
  is AIDA-managed.

When the code leg refuses (or raw `git pull` complains about divergent
branches), the recovery recipe:

```bash
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git log --oneline @{u}..HEAD     # what you have that origin doesn't
git log --oneline HEAD..@{u}     # what origin has that you don't
git log --name-only @{u}..HEAD --pretty= | sort -u   # files you touched
git log --name-only HEAD..@{u} --pretty= | sort -u   # files they touched
# No overlap → safe: git pull --rebase
# Overlap   → inspect; rebase + resolve, or git rebase --abort
```

To make raw `git pull` Just Work without per-incident decisions (one-time,
machine-global):

```bash
git config --global pull.rebase true
git config --global rebase.autoStash true
git config --global advice.diverging false
```

Trade-off: silent auto-rebase for fewer manual decisions. `autoStash` preserves
uncommitted changes across the rebase. If you'd rather see the prompt each time,
leave these unset and the recipe above is your fallback.

## Review workflow

`aida review prompt --pr N` (or `--specs FR-1,STORY-2,…`) generates a markdown
review prompt that lifts each linked requirement's `## Acceptance` section
verbatim — paste it into a fresh Claude Code review session, or write it to a
file with `--write`.

- **Install `gh` or `glab` for `--pr` mode.** AIDA shells out to
  [`gh pr view`](https://cli.github.com) / [`glab mr view`](https://gitlab.com/gitlab-org/cli)
  to resolve the PR's base + head refs. Without them, AIDA falls back to
  `base=main` and a local review branch named `pr-N` / `mr-N` — that path works
  when the PR was started via `aida session start --owns PR-N` (STORY-61),
  surprising otherwise.
- **Acceptance sections are the contract.** Write a `## Acceptance`, `## Verify`,
  `## Tests`, `## Test cases`, or `## Verification` section in every STORY / BUG
  description so the review prompt has something concrete to lift.
  `aida doctor convention-check` lints for the gap.

### The diff instrument, when the branch is behind main — trace:BUG-1518

A review answers *"what will this merge change?"*, not *"how do these two
trees currently differ?"*. Diff by merge-base — `git diff origin/main...<branch>`,
`gh pr diff <N>` / `glab mr diff <N>`, or a name-only listing of the commits'
own files (`git log --name-only origin/main...<branch>`) — never a two-dot or
bare tree diff against `main`'s current tip (`git diff origin/main <branch>`
and `git diff origin/main..<branch>` are the same operation). A behind
branch's tree diff shows every file `main` gained since the fork as a
deletion the merge will never make — it never hides a real change, it
manufactures alarming ones. Label how far behind the branch is
(`git rev-list --count <branch>..origin/main`) wherever a diff is presented,
and never let a test or check decide something from a tree comparison with a
moving base — read the commits' own contents instead. Full rationale and the
worked 661-line false-alarm instance: the `aida-review` skill's diff-instrument
step.


## Migrated Lessons

### MEMORY

# AIDA memory index

Read individual memories only when their group is relevant; the memory files are unchanged.

- Product direction: `project_northstar_code_to_decision_magic.md`, `project_champion_product_not_probe.md`, `project_bugs_before_marketing_phase.md`, `project_axi_incorporation_and_mcp_reweighting.md`.
- Operator mandates: `feedback_presence_is_not_the_clock.md`, `feedback_charge_forward_autonomously.md`, `feedback_lead_churn_direct_agents.md`, `feedback_build_dispatch_resilience_multivendor.md`, `user_permission_posture.md`.
- Public/confidential: `feedback_public_repo_scrub_employer_content.md`, `feedback_no_tokens_in_chat.md`.
- Git, merge, and CI: files matching `feedback_*merge*.md`, `feedback_*git*.md`, `feedback_*ci*.md`, plus `feedback_read_the_verdict_not_just_the_check_rollup.md` and `feedback_commit_trailer_completes_the_spec.md`.
- Worktrees and sessions: files matching `feedback_*worktree*.md`, `feedback_*session*.md`, `feedback_fan*.md`, plus `project_single_drain_lock_per_repo.md`.
- Advisor and independence: `feedback_dialog_role_responsibilities.md`, `feedback_proxy_reviewer_with_independence_rule.md`, `feedback_trust_reviewer_over_dialog_intuition.md`, `feedback_one_master_advisor_until_subsystems.md`.
- Capture/spec authoring: files matching `feedback_capture*.md`, `feedback_*filing*.md`, `feedback_verify_acceptance_matches_primary_caller.md`, `feedback_refinements_must_be_acceptance_criteria.md`.
- Verification rigor: files matching `feedback_verify*.md`, `feedback_*evidence*.md`, `feedback_*measurement*.md`, `feedback_*test*.md`, and `feedback_never_conclude_from_truncated_command_output.md`.
- Autonomy/orchestration: files matching `feedback_*drain*.md`, `feedback_*orchestrator*.md`, `feedback_*headless*.md`, `feedback_three_mode_autonomy_taxonomy.md`, `feedback_sketch_first_pays_for_itself.md`.
- Product/substrate design: `feedback_substrate_as_bouncer_not_rules.md`, `feedback_quarantine_specialized_context_in_skills.md`, `feedback_configurable_policy_with_default.md`, `feedback_ride_native_within_vendor_own_cross_vendor.md`.
- Operator communication: `feedback_match_operator_mode_specific_when_execution.md`, `feedback_plain_language_define_jargon_for_operator.md`, `feedback_explicit_paste_ready_prompts.md`, `feedback_parallel_vs_sequential_ui.md`, `feedback_precise_lifecycle_vocabulary.md`.
- Fleet economics: `feedback_multi_agent_budget_dispatching.md`, `feedback_token_usage_optimization_agent_fleet_economics.md`, `feedback_agy_dispatch_policy.md`, `reference_headless_claude_p_skills_and_fanout.md`.
- Memory hygiene: `feedback_memory_pack_hygiene.md`, `feedback_dated_artifacts_immutable.md`, `feedback_propagate_generic_discipline_via_scaffolding.md`.
- Miscellaneous CLI and safety lessons: remaining `feedback_*.md`, `reference_*.md`, and `user_*.md`; search filenames before loading bodies.

Host/build:
- [NVMe /mnt/fast scratch migration](reference_nvme_fast_scratch_migration.md) — sccache/codex/agy state on NVMe via symlinks; weekly fast-tidy timer; don't move dirs under live SQLite writers
- [Cap parallel cargo builds](feedback_cap_parallel_cargo_builds.md) — no full suite under script/pty (oomd kill); CARGO_BUILD_JOBS=3, 2-3 builds max
- [Build slots + sccache + mold](reference_build_slots_sccache_mold.md) — cargo is 2-slot gated machine-wide; dispatch freely, builds queue
- [No full workspace suite per agent](feedback_dont_brief_full_workspace_suite_per_agent.md) — build-fast + targeted tests + fmt; full suite ONCE at integration; jobs=2, nice
- [Serial, not fan-out, on this host](feedback_serial_not_fanout_on_this_host.md) — work ONE item to merged, rotate early; 13 agents on 6 cores burned 15% of a week in 2h
- [Reclaim disk via worktree targets](feedback_reclaim_disk_by_deleting_worktree_targets.md) — delete target/ in non-main worktrees; keep main's target + sccache
- [Don't rebuild main with another lane's WIP](feedback_dont_rebuild_main_with_another_lanes_wip.md) — main's target/ IS the host-wide live `aida`
- [/run/user tmpfs is only 7G](reference_run_user_tmpfs_is_only_7g.md) — `df -h /run/user/1000` in every build-failure triage
- [RUST_MIN_STACK for direct test binaries](reference_run_test_binary_needs_rust_min_stack.md) — bypassing cargo loses the 8MiB stack; aborts print NO test-result line
- [Orphaned load generators poison measurements](feedback_orphaned_load_generators_poison_every_measurement.md) — check top-CPU before trusting any timing
- [codegraph local setup + 1MiB cap](reference_codegraph_local_setup_and_1mib_cap.md) — systemd timer keeps it fresh; lib.rs (4.7MB) NEVER indexed, confirm no-callers with rg; init can exit 0 on "database is locked"
- [sccache + shared target serves stale worktree builds](feedback_sccache_shared_target_serves_stale_worktree_builds.md) — same unit hash as main, relative dep-info; strings-verify the artifact, then touch + RUSTC_WRAPPER= rebuild

- [merge-hold clear is human-only](reference_merge_hold_clear_is_human_only.md) — integrity floor; closing paste is clear → gate rerun → merge
- [Idle peer sessions as independent review seats](feedback_idle_peer_sessions_as_independent_review_seats.md) — ListAgents+SendMessage; confirm pickup by artifact, instant-idle may mean exited

Dispatch/codex:
- [No mail storm to Codex](feedback_no_mail_storm_to_codex.md) — Codex doesn't read aida mailbox; relay via one operator paste
- [Codex exec dispatch recipe](reference_codex_exec_dispatch_recipe.md) — `< /dev/null` is MANDATORY or backgrounded codex exec hangs on stdin
- [Codex needs --add-dir for cargo slots](reference_codex_needs_add_dir_for_cargo_slots.md) — without it codex CANNOT run cargo under workspace-write
- [Declare the seat when dispatching](feedback_declare_the_seat_when_dispatching.md) — say the rework brief IS the advisor hand-off or the implementer refuses
- [An idle seat is not a working seat](feedback_idle_seat_is_not_a_working_seat.md) — diagnose by CPU time; read stranded reports from the codex sqlite DB
- [Probe the sandboxed form before declaring dispatch denied](feedback_probe_the_sandboxed_form_before_declaring_dispatch_denied.md) — `--sandbox read-only` works; the refusal was about bypass flags
- [Dispatched agent cannot write the shared cache](feedback_dispatched_agent_cannot_write_the_shared_cache.md) — cache is symlinked OUT of the worktree; reads work, writes never can
- [codex cannot commit in a linked worktree](feedback_codex_cannot_commit_in_a_linked_worktree.md) — git metadata lives outside the sandbox; advisor commits
- [pgrep codex matches zombies and the daemon](feedback_pgrep_codex_matches_zombies_and_daemon.md) — waiters never exit; use the harness notification or the -o verdict file
- [pgrep wait predicates self-match](feedback_pgrep_wait_predicates_self_match.md) — wait on process name, file growth, or a sentinel

Git/CI/merge:
- [Context ceiling is soft](feedback_context_ceiling_is_soft.md) — never stop at the ceiling when Joe is away; compact and keep draining
- [Push first, overlap CI with the local suite](feedback_push_first_overlap_ci_with_local_suite.md) — don't gate the push on the 25-min run; both green to merge
- [CI pending at handoff is not CI green](feedback_ci_pending_at_handoff_is_not_ci_green.md) — portability allowlist is keyed by line CONTENT, not number
- [git push --dry-run still pushes the mirror](reference_git_push_dry_run_still_pushes_the_mirror.md) — the gitlab mirror step ignores --dry-run (BUG-1706)
- [Never switch branches in the operator's checkout](feedback_never_switch_branches_in_operators_checkout.md) — sibling worktree, even for docs
- [Never stash for a mutation proof](feedback_never_stash_for_a_mutation_proof.md) — stash of clean paths is a no-op; the pop restores an old autostash
- [Merged spec needs queue done then aida pull](reference_merged_spec_needs_queue_done_then_aida_pull.md) — only `aida pull` bumps the spec
- [--is-ancestor lies after a squash merge](reference_is_ancestor_lies_after_squash_merge.md) — ask `gh pr list --head`, never ancestry
- [Check for an open PR before claiming a spec](feedback_check_for_an_open_pr_before_claiming_a_spec.md) — the `-2` branch suffix is the only tell
- [Recording a verdict blocks your own merge](feedback_recording_a_verdict_blocks_your_own_merge.md) — [Self-Approval] also denies db sync --push; hand closing commands to the operator
- [aida review record is the non-interactive path](feedback_aida_review_record_is_the_noninteractive_path.md) — `--verdict` dispositions headlessly; "needs a TTY" is false
- [End a session with `aida db sync --push`](feedback_end_session_with_db_sync_push.md) — only that fans out to the gitlab mirror (hub-drift-guard, BUG-1746)
- [Verify a new CI step actually executed](feedback_verify_a_new_ci_step_actually_executed.md) — read the job's step list; `gh run list --limit 1` may return the NIGHTLY
- [A test on the branch may be wired into nothing](feedback_a_test_on_the_branch_may_be_wired_into_nothing.md) — ci.yml names shell tests individually; grep for the FILENAME
- [New CLI leaf must regenerate the format-json audit](feedback_new_cli_leaf_must_regenerate_format_json_audit.md) — the guard is an integration test check-ci-fast can't reach
- [Tier a gate transitively](feedback_tier_a_gate_transitively_not_by_its_body.md) — build deps hide in called scripts; prefer loud failure over a predicate
- [Source-scanning guards need the full suite](feedback_source_scanning_guards_need_the_full_suite.md) — satisfy honestly, never widen the allowlist
- [A source-scanning guard greps your comment too](feedback_a_source_scanning_guard_greps_your_comment_too.md) — the explaining comment trips it; `== allowed` blocks helper workarounds
- [A fence tag is a gate-scope decision](feedback_a_fence_tag_is_a_gate_scope_decision.md) — `console` imports lines into check-portability; no inner fence inside a fence
- [trace: never on a /// doc comment](reference_trace_marker_never_on_a_doc_comment.md) — clap pulls /// into --help; pre-commit refuses it

Verification:
- [Prove a test fails without the fix](feedback_prove_a_test_fails_without_the_fix.md) — revert and rerun, or the coverage doesn't exist
- [Re-run the acceptance measurement yourself](feedback_rerun_the_acceptance_measurement_yourself.md) — codex said 4, truth was 489; check the brief's own recipe
- [Reproduce, then falsify with a control](feedback_reproduce_then_falsify_with_a_control.md) — run the negative control the hypothesis says must pass
- [Which assertion witnesses the regression?](feedback_ask_which_assertion_witnesses_the_regression.md) — prove the witness by mutation before demoting a timing assert
- [Mutate both directions for a discriminator](feedback_mutate_both_directions_for_a_discriminator.md) — one mutation leaves a vacuous control
- [Absent is not matching](feedback_absent_is_not_matching.md) — a drift guard's mutation proof must include DELETING the artifact
- [A drift gate must delete before it compares](feedback_drift_gate_must_delete_before_it_compares.md) — regenerate-in-place reports "no drift" when codegen never ran
- [Price the criterion's own disjunct](feedback_price_the_criterions_own_disjunct_before_dispositioning.md) — the cheap branch cost 6 minutes; don't close on opinion
- [Qualify an AC predicate that cannot reach zero](feedback_qualify_an_ac_predicate_that_cannot_reach_zero.md) — run each AC's own predicate before dispatching
- [Re-run the spec's own survey grep](feedback_grep_the_whole_crate_the_filed_list_undercounts.md) — filed six sites, grep found nine
- [One-idiom grep undercounts write paths](feedback_one_idiom_grep_undercounts_write_paths.md) — `.collect()`+assign is invisible to `insert(` greps
- [Grep the code before filing as new](feedback_grep_the_code_before_filing_as_new.md) — `aida search` misses shipped code; two drafts were wrong for it
- [--name-only cannot see an inline Rust test](feedback_name_only_cannot_see_an_inline_rust_test.md) — #[cfg(test)] in source files; two handoffs wrongly rejected the ACs
- [Test-merge before judging a stranded WIP](feedback_test_merge_before_judging_a_stranded_wip.md) — diffstat predicted pain; the real merge was clean
- [Suite-log predicates must anchor on harness lines](feedback_suite_log_predicates_must_anchor_on_harness_lines.md) — wait on the process, not log text
- [Find a suite aggregate by sorting passed-counts](reference_suite_aggregate_line_is_the_largest_passed_count.md) — nested harnesses print 50 fake `test result:` lines
- [A flat open count may be a flake chain](feedback_flat_open_count_may_be_a_flake_chain_not_slowness.md) — answer "are we stuck?" with numbers
- [A partial fix can turn a clean failure into a hang](feedback_a_partial_fix_can_turn_a_clean_failure_into_a_hang.md) — only `timeout N <cmd>; echo $?` saw it
- [A timing flake may fail a freshness assert](feedback_a_timing_flake_may_fail_a_freshness_assert.md) — get the panic line before fixing the elapsed literal

Code/design traps:
- [Best-effort shell mode must audit set -e escapes](feedback_best_effort_shell_mode_must_audit_set_e_escapes.md) — every fallible command (mkdir/ln) needs the skip handler; pin with a chmod a-w fixture
- [Check call order before trusting a bug's prescription](feedback_check_call_order_before_trusting_a_bugs_prescription.md) — the blamed function ran after the line that errored
- [A nothing-to-do guard may have a twin](feedback_a_nothing_to_do_guard_may_have_a_twin.md) — enumerate every early return in the enclosing function first
- [A preserving write is not a placement](feedback_a_preserving_write_is_not_a_placement.md) — label every statement validation vs placement in a replace-writer
- [Precomputed worklist must skip what the write pass replaced](feedback_precomputed_worklist_must_skip_what_the_write_pass_replaced.md) — record replacements, don't re-stat
- [Symlink checks must cover the directory](feedback_symlink_checks_must_cover_the_directory.md) — child checks resolve THROUGH a symlinked parent; assert the target unmodified
- [Dropped argument + shared-root file](feedback_review_the_dropped_argument_and_the_shared_root_file.md) — grep `, None)` on identity predicates; copy the borrowed-child guard
- [A new struct field breaks every literal](feedback_a_new_struct_field_breaks_every_literal.md) — the brief must authorize the files the compiler will flag
- [A misleading parameter name costs review rounds](feedback_a_misleading_parameter_name_costs_review_rounds.md) — rename and pin the invariant, don't just refute
- [Fork window keeps an flock past its guard](feedback_fork_window_keeps_an_flock_past_its_guard.md) — SELF in /proc/locks + empty own_fds is the tell
- [Cross-clone leases get no liveness check at all](feedback_cross_clone_leases_get_no_liveness_check.md) — it prints `age` and never uses it (BUG-1764)
- [SQLite WAL reads need a live -shm](feedback_sqlite_wal_read_needs_a_live_shm.md) — immutable=1 silently serves stale
- [Duplicate TOML table = empty stderr](feedback_duplicate_toml_table_fails_with_empty_stderr.md) — patch the scaffolded table, don't append one
- [Dogfood config asserts read at runtime](feedback_dogfood_config_assertions_read_at_runtime.md) — never include_str! repo-root config
- [The fixture shape decides which path you test](feedback_the_fixture_shape_decides_which_path_you_test.md) — chmod vs symlink-out trip different probe clauses
- [A blob tag renders as the tags it isn't](feedback_a_blob_tag_renders_as_the_tags_it_isnt.md) — `--tags "a b"` stores ONE tag; repair with `--tags <comma,set>`
- [A refusal must speak the caller's syntax](feedback_a_refusal_must_speak_the_callers_syntax.md) — make the repair shape a parameter, test each surface
- [A stale auto-drafted finding is an auto-resolver gap](feedback_a_stale_autodrafted_finding_is_an_autoresolver_gap.md) — the transition AUTHOR in the store object is the tell
- [Brief message spec and proof must agree](feedback_brief_message_spec_and_proof_must_agree.md) — a self-contradicting brief costs a cold rebuild
- [RecordingForge makes orchestrator PR tests hermetic](reference_recordingforge_makes_orchestrator_pr_tests_hermetic.md) — the AlreadyMerged arm needs a repo with NO origin


### feedback_a_misleading_parameter_name_costs_review_rounds

When two independent reviewers produce confident, wrong findings that trace to the **same**
misreading, the defect is in the code's naming, not in the reviewers. Refuting the finding and
moving on leaves the trap armed for the next seat.

BUG-1704: `seat_has_own_work(tree, pid, idle_secs, grace_secs)` was called with `elapsed` —
seconds since the seat's own heartbeat. Round 2 and round 3 both read `idle_secs` as a configured
threshold and concluded a build running longer than it would "age out" and a working seat would
read Idle. It cannot: with `elapsed(t) = t - T_heartbeat` and `age(t) = t - T_start`, the clause
`age < elapsed` reduces to `T_start > T_heartbeat` — the `t` cancels, so the comparison is
time-invariant and asks only "did the child start after the last heartbeat?".

**How to apply:** when you overturn a reviewer, ask what in the source made the wrong reading
reasonable, and fix that too — a rename plus the invariant written at the predicate plus a test
that pins it across orders of magnitude. Also: a parameter whose name states a *policy* but whose
argument carries a *measurement* is the specific smell; check the call site, not the signature.
The non-vacuity bar for such a test is that it fails when the clause is replaced by the literal
threshold the reviewers thought it was.

**Why:** three review rounds cost real budget, and two of them were spent on a refutable premise.
See [[feedback_verify_a_reviewers_premise_before_accepting_a_p1]] and
[[feedback_trust_reviewer_over_dialog_intuition]] for the other half of the rule: check, don't doubt
— round 1's P1 on the same branch was completely correct.

### feedback_a_partial_fix_can_turn_a_clean_failure_into_a_hang

2026-10-01, BUG-1752 (shared symlinked cache in a linked worktree). Rounds 1a–1d added
`is_read_only()` short-circuits so a read-only cache would stop attempting a migration. One of them
made `ensure_cache_fresh` return `Ok(())` for a read-only cache. In the state where the cache is
*unreadable* (no live `-shm`), that left `tolerant_read_with_budget` re-pinning a snapshot on a
predicate that could never clear — `get_meta`'s `.ok()` makes an unreadable table look like an absent
key, so the recorded HEAD reads `None` forever.

Measured: on the branch, all four read commands **hung** (`timeout 90` → exit 124, state `R`, ~12%
CPU, `wchan = poll_schedule_timeout`). On `main` the same state **failed in under a second**
(`Failed to enable WAL journal mode for cache`). The branch had 178 green cache unit tests at that
moment. Not one of them could see it.

**Why:** a short-circuit added for a degraded state changes the control flow *of that state only*,
which is exactly the flow no existing test exercises. And a hang is strictly worse than the bug being
fixed: the original error at least told the agent to stop, while a spin consumes a dispatch's whole
budget and reports nothing.

**How to apply:**
- When a fix adds a branch for a degraded/unavailable state, run the **real command end to end** in
  that state before believing the unit tests. `timeout N <cmd>; echo $?` — an exit of 124 is a
  finding, not a slow machine.
- Diagnose a hang before prescribing: `ps -o stat,pcpu` plus `/proc/<pid>/wchan` separates a spin
  from a block, and `strace -e trace=openat,clock_nanosleep` shows the loop's shape (a repeating
  cycle of the same reads names the function being called over and over).
- Bound every retry loop whose predicate is fed by an error-swallowing `.ok()`. Prefer a deadline
  over a count, and on expiry serve a labelled-stale answer rather than failing a read that the
  unbounded version would have won — a count bound is how you turn a benign race into a new error.
- State the before/after honestly: if `main` failed fast and your branch hung, the fix closes a
  defect the fix itself created. Say so.

Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_reproduce_then_falsify_with_a_control]],
[[feedback_rerun_the_acceptance_measurement_yourself]],
[[feedback_the_fixture_shape_decides_which_path_you_test]].

### feedback_a_preserving_write_is_not_a_placement

BUG-1691 changed `write_typed_hold` from "replace the marker" to "compose, keeping the
stricter hold". The composition was correct. The defect was one layer down: the writer it
delegated to ran `reconcile_recusal_route` whenever the record it was handed was a Recusal.
Before the change that record was the incoming Rework hold, so the branch never fired; after
it, the record was the preserved Recusal, so it fired every time.

Consequences: recording a rejection re-derived the recusal's route as a side effect and
**cleared it** when no eligible reader was live, and a route-brief IO failure would have
propagated out of `aida review record` and lost the verdict before it was written.

**Why:** a replace-writer only ever sees records the caller is placing NOW, so it can assume
everything in it is placement work. The moment composition can hand it a record the caller
did not author, that assumption is false — and nothing in the type system says so.

**How to apply:** when a write path gains a preserve/compose branch, enumerate every
statement in the writer and label it *validation* (must run on both paths) or *placement*
(must be skipped when preserving). Make the distinction explicit in the signature — a
`Composed::Placed | ::Preserved` return plus a `reconcile: bool` parameter — rather than
leaving it implicit in which record happens to arrive. The test that proves it must make the
side effect destructive: the fixture had no live agent, so an unwanted reconciliation
cleared the route and the assertion caught it.

Related: [[feedback_precomputed_worklist_must_skip_what_the_write_pass_replaced]],
[[feedback_check_call_order_before_trusting_a_bugs_prescription]],
[[feedback_prove_a_test_fails_without_the_fix]].

### feedback_a_sha_is_read_never_composed

A seat ran `git push --force-with-lease=<branch>:<sha>` where the sha had been built by padding a 12-character prefix out to 40 characters with invented hex. The push was refused as "stale info." On comparison, **28 of the 40 characters were fabricated**. The lease did its job for a reason nobody designs it for — it rejected a lease that could never have matched.

**Why:** the lease is supposed to answer "is the remote still where I last saw it?" A fabricated sha turns it into "is the remote at this string I made up?", which is always no. So it *looks* like the safety check fired correctly, while carrying no information about the remote at all — and the same habit under plain `--force` would have pushed with no check whatsoever. It is worse in kind than a wrong number: it is an invented *identifier* that is syntactically valid and therefore passes every surface inspection.

**How to apply:** a sha is read, never composed. `--force-with-lease=<branch>:$(git rev-parse origin/<branch>)`, never a literal typed or padded out from a prefix. The same rule covers any content-addressed identifier — object ids, digests, run ids, UUIDs: if it came from your keyboard rather than a command's output, it is a guess. When a short prefix is all you have, resolve it (`git rev-parse <prefix>`) rather than extending it.

Related: [[feedback_never_force_push_main_and_chain_cd]], [[feedback_null_grep_for_invented_terms_is_not_absence]], [[feedback_state_how_a_number_was_derived]], [[feedback_verify_pr_head_after_push_gh_pr_checks_lies_on_merged_pr]].

### feedback_a_test_on_the_branch_may_be_wired_into_nothing

BUG-1688's branch carried both the fix and `tests/test_bug_1688_shipped_launch_refusals.sh`,
and the test passed when run by hand. But `.github/workflows/ci.yml` names **each** shell
test individually — there is no glob runner over `tests/*.sh` and no Makefile target that
sweeps them — so nothing ever executed it. AC2's "a fixture proves zero vendor launches"
was an assertion no gate enforced.

**Why:** a test that runs only when a human remembers to run it is not a regression guard.
A later edit could have dropped a refusal guard from any of the five scripts with every
required check still green. The acceptance criterion read as satisfied because the artifact
existed.

**How to apply:** when reviving a stranded branch or reviewing any new test file, grep the
CI config and the Makefile for the test's **filename** before accepting the AC. If it is
absent, that is the defect to fix. Gate a new shell-test step on the same condition as its
neighbours (`full_ci` here, because the fixture shelled out to ripgrep which only the
`full_ci` dependency step installs).

Related: [[feedback-verify-a-new-ci-step-actually-executed]],
[[feedback-prove-a-test-fails-without-the-fix]],
[[feedback-read-the-verdict-not-just-the-check-rollup]].

### feedback_a_timing_flake_may_fail_a_freshness_assert

BUG-1729 AC1 was filed as "the wall-clock assertion is load-sensitive", and both the spec
and the handoff predicted `assert!(start.elapsed() < 2s)` would be the failing line. It
never was. Under contention the readers exhausted a 1500ms `ReadBudget`, took the
give-up branch, served stale rows **quickly**, and failed
`rows.iter().any(|r| r.title == "gen1")` — a freshness assert two lines later.

**Why:** a timeout that a production code path *handles* by degrading makes the test
faster, not slower. So the elapsed assert passes and the correctness assert fails. Reading
the bug's own framing instead of the panic line sends you to fix the wrong assertion.

**How to apply:** for any "timing-sensitive test" claim, get the actual panic file:line
before choosing the fix. If the failing assert is about *state* rather than *duration*,
the fault is an internal budget being exhausted, and the fix is to inject a generous
budget — not to widen the wall-clock literal. Then prove the injection is not vacuous by
forcing that budget to zero and confirming the test still fails. Related:
[[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_check_call_order_before_trusting_a_bugs_prescription]],
[[feedback_a_misleading_parameter_name_costs_review_rounds]].

### feedback_advisor_grooms_dont_shift_to_operator

When an item needs grooming (cross-spec reconciliation, scope refinement, checking overlap/staleness, validating acceptance against current reality), that is **advisor work — do it; do not surface it to the operator as a decision.**

Operator correction (2026-06-14, two-step): (1) I told the operator to `aida edit STORY-597 --status approved`; they pushed back that blind-approving a draft is wrong. (2) I then "demonstrated grooming" by listing what I'd found (stale branch, overlap with the Completed STORY-603, already-built bootstrap) and offered to groom it *and show them for the call*. They nailed the real flaw: **"I don't know by reading the spec that it's stale/overlapping/mis-scoped — the advisor knows that. The advisor does grooming. You're shifting the responsibility to me, why?"** Correct. That derived cross-spec knowledge is *only* available to the advisor (who holds the graph in context); the operator cannot get it from the spec text, and shouldn't have to.

**The seat split (= EPIC-42's thesis):**
- **Ungroomed draft → advisor's worklist.** The advisor reads it, reconciles overlaps, refines scope, fixes staleness, and **disposes the routine ones** (approve+queue, or defer per the operator's stated phase). Advisor approval is legitimate (ADR-3).
- **`aida human` (operator list) → only the residual** that needs operator-specific judgment: strategic direction, keystone/architecture, genuine priority tradeoffs the advisor can't make alone. NOT "decide whether to build a bounded tooling spec" — that's advisor disposition.

**How to apply:**
- Default: when a draft surfaces, GROOM it (the cross-spec analysis) and DISPOSE it as advisor. Don't ask the operator "want me to groom it?" — just do it, then bring them only what genuinely needs them.
- Bring the operator a decision only when it's truly theirs (strategy/keystone/priority), and bring it **already-groomed** — with the cross-spec analysis done and options framed — never as a raw draft.
- Seeing ungroomed drafts on `aida human` is a signal the advisor hasn't run its grooming pass (`/aida-triage` / backlog-groom) — run it so the operator's list stays clean. Relates to [[feedback_dialog_role_responsibilities]] (garden the queue is an advisor responsibility) and [[feedback_advocate_not_be_passive]].

### feedback_aida_review_record_is_the_noninteractive_path

Seven consecutive advisor handoffs carried the claim *"`aida review <SPEC>` needs a TTY, so an
autonomous session cannot clear these"* as the reason the grooming set (BUG-1668, BUG-1688, PRIN-5,
STORY-1425, TASK-1536) stayed parked. On 2026-10-01 I nearly recommended filing a spec for a
non-interactive review mode on the strength of it. **The capability already exists:**

```
aida review record <SPEC> --verdict approved|request-changes|rejected
                          [--sha <SHA>] [--branch <BRANCH>] [--summary <TEXT>] [--finding <TEXT>]...
```

`ReviewCommand::Record` is at `aida-cli-lib/src/cli.rs:866+` (trace:BUG-775). It stores the verdict,
the examined commit and the time as first-class state, and `aida queue done` then **refuses** to mark
the spec done while the branch still sits at that commit. `--finding` (repeatable) is what hands
concrete items to a rework implementer.

The TTY claim is narrowly true only of the **bare** `aida review <SPEC>` walkthrough. The
dispositioning path — the one that actually matters for clearing a parked spec — has a first-class
non-interactive spelling.

**Why:** an inherited blocker is the most expensive kind of wrong fact, because every session that
re-copies it into its handoff adds authority without adding evidence. This one converted a
prioritization choice into an imagined wall, and the wall then justified the deferral that created the
next handoff's copy of the claim.

**How to apply:** a parked spec is cleared by test-merging its retained branch into a throwaway
branch, reading the diff, deciding, and recording the verdict with `aida review record` — all four
steps are available to an autonomous session. More generally: before carrying a blocker forward a
second time, run `<cmd> --help` or grep the clap enum. A blocker that has survived several handoffs
without anyone re-measuring it is a prime suspect, not a settled fact. See
[[feedback_grep_the_code_before_filing_as_new]] and
[[feedback_rerun_the_acceptance_measurement_yourself]].

### feedback_batched_shell_calls_merge_provenance

Batching commands into one Bash call — `cmd_a | head -40; echo "---"; cmd_b | grep ...` — merges two independent outputs into one block of terminal text. Reading it back, the boundary is just a separator I printed myself, and a fact from the second command gets attributed to the source of the first.

This happened for real: a reviewer's mail and `aida awaiting`'s machine-readable block printed in one call. The block ended `nightly_red: cross-platform nightly red since 2026-09-18 (4 nights)`. I told the reviewer "your board line says the nightly has been red four nights, I'm looking at that next." It was never theirs. They could not check it, because they could not see where it came from.

**Why:** this is the mirror of a wrong claim — a *correct* claim with the wrong author. It is worse in one specific way: the misattributed party is now accountable for a measurement they never made, and effort gets spent on their authority. It also launders a machine-generated summary into a colleague's judgment, which carries far more weight. Both a person's message and a tool's output are data; concatenating them loses the only thing that distinguishes them.

**How to apply:** when a claim is going to be *repeated to someone else* — especially attributed — re-run the source command alone, or scroll back to which command actually emitted it. Separate calls for separate sources when the output will be quoted. When attributing, name the surface (`aida awaiting` says…) rather than the seat, unless the seat's own words are in front of me. If someone disputes authorship, the fix is to find the mechanism, not to drop the claim: here the claim was right and only the author was wrong.

Related: [[feedback_mark_relayed_operator_decisions_as_unverifiable]], [[feedback_never_multicast_a_second_person_body]], [[feedback_state_how_a_number_was_derived]], [[feedback_delegated_findings_are_not_verified_ground_truth]].

### feedback_best_effort_shell_mode_must_audit_set_e_escapes

On BUG-1762 (2026-10-02) I wrote `scripts/install-agent-skill-hooks.sh` with a `--best-effort`
mode whose contract is "a routine build must never fail over hook installation", and handled the
cases I had thought of (linked worktree, non-repo, foreign hook). The cross-vendor reviewer
(codex, round 1) caught that `mkdir -p` and `ln -s` failures still escaped through `set -euo
pipefail` — an unwritable `.git`, or a filesystem without symlinks (the Windows case), would have
failed every `make build`. The fixture proving it failed against the pre-fix script in one run.

**Why:** under `set -e`, the contract "this mode always exits 0" is only as strong as the
handling on the *least* interesting command in the script. The refuse/skip helper pattern makes
the handled branches visible and the unhandled ones invisible.

**How to apply:** when a script advertises a never-fail / best-effort mode, enumerate every
command that can fail (filesystem ops especially: mkdir, ln, cp, chmod) and route each through
the mode's refuse/skip handler; then pin it with a fixture that makes the environment hostile
(chmod a-w the target dir) and assert exit 0. Related: [[feedback_prove_a_test_fails_without_the_fix]],
[[feedback_proxy_reviewer_with_independence_rule]], [[feedback_the_fixture_shape_decides_which_path_you_test]].

### feedback_brief_message_spec_and_proof_must_agree

In BUG-1761's round-1 brief I specified the stale-branch refusal should say *"`queue work` will not
silently allocate `<branch>-2`"*, and in the proof list I wrote the test must assert the message
*"names the branch and does **not** suggest `-2`"*. Those two cannot both hold: the sentence explaining
what the command won't do necessarily contains the string `-2`.

The implementer followed the proof literally, wrote `assert!(!message.contains("bug-7-2"))`, and the
suite came back **4 passed / 1 failed** on a cold build of a large crate — a full rebuild spent on a
contradiction in the brief, not a defect in the code. It then resolved it correctly on its own by
changing the assertion to `assert!(message.contains("will not silently allocate `bug-7-2`"))`.

**Why:** when a brief both dictates user-facing wording and asserts a negative property of that same
wording, the two clauses are a single specification written twice. Prose and predicate drift apart in
the writing, and the compiler finds it minutes later at full build cost.

**How to apply:** when a brief specifies message text, write the assertion **against the text you
specified**, quoting it, rather than restating the requirement as a negative. If a negative assertion
is genuinely wanted, make it about the *action* and not the *substring* — "the refusal must not offer
the suffix as a recovery action" is testable without forbidding the characters. Before dispatching,
re-read any proof containing `not`/`!contains` against the literal strings the brief asked for.

Related: [[feedback_qualify_an_ac_predicate_that_cannot_reach_zero]] (a predicate that cannot reach its
target), [[feedback_a_source_scanning_guard_greps_your_comment_too]] (a guard reading your own prose),
and [[feedback_a_new_struct_field_breaks_every_literal]] (brief omissions that cost a round trip).

### feedback_build_combined_main_after_concurrent_merges

Integrating a parallel multi-agent fan-out (2026-06-12), I merged STORY-567 (#807, changed `spawn_claude_headless`'s signature — added a `contained` param) and STORY-568 (#805, added a NEW caller of `spawn_claude_headless` in `handle_research_command`) in the same batch. Each passed CI **individually** (each PR's CI ran against a base that lacked the other's change). Merged together, main did **not compile**: "this function takes 5 arguments but 4 were supplied." Git saw no textual conflict — the two changes were in different files/functions — so nothing flagged it. The break only surfaced when a later PR's CI (#804) went red against the poisoned base.

**Why:** GitHub merge-state CLEAN and per-PR green CI only guarantee each PR is consistent with *its own base*, NOT that the batch is mutually consistent. A semantic conflict — one PR changes a function signature / adds a required struct field, another PR adds a caller / constructs that struct — passes both gates and lands a broken main. This is structurally identical to the required-`Message`-fields risk in the very next PR (STORY-583 added `retracted`/`deleted` fields; any concurrently-merged Message constructor would have broken again).

**How to apply:** when merging a BATCH of concurrently-authored PRs (especially ones touching shared modules / changing signatures / adding struct fields), after the merges run `git pull` + `cargo build -p aida-cli` on the combined main BEFORE declaring the batch done. If it breaks, fix-forward immediately (a broken main blocks every open PR's CI). Watch specifically for: (1) a fn signature change + a new call site in a sibling PR; (2) a new required struct field + a sibling constructor; (3) a renamed/moved symbol + a sibling reference. The per-PR CI will NOT catch these — only building the union does. Recovery is cheap (one-line arity/field fix, fix-forward), but only if you LOOK; silent breakage poisons every subsequent PR's CI. Related: [[feedback_verify_ci_green_before_merge]] (CI green ≠ safe to merge) and [[feedback_verify_agent_pr_done_claims_against_diff]] (the other multi-agent integration hazard).

### feedback_cargo_check_one_package_is_not_ci

2026-09-20: rebasing STORY-1351 (PR #1986) I ran `cargo check -p aida-cli-lib --tests`, got exit 0 in 2m26s, and wrote on the spec that I had "verified the rebased commit compiles" before pushing. CI then failed on `templates::tests::project_discipline_pack_matches_master` in **aida-core** — a test asserting the project's `.aida/discipline/*.md` copies match their embedded masters. Two independent gaps: `check` never runs tests, and the failing crate was not the one I checked. The branch touched files in both `aida-cli-lib` and `aida-core`, and I picked the package by where I expected breakage (the auto-merged `lib.rs`) rather than by what the diff touched.

**Why:** the reason to validate locally is to avoid burning a CI cycle and a reviewer's attention. A validation narrower than CI does not do that — it buys false confidence and still costs the cycle. Worse, it produces a written claim ("verified it compiles") that a reviewer may rely on.

**How to apply:**
- Pick the scope from `git diff --name-only <base>..HEAD`, mapping each path to its crate. Never from intuition about where the risk is.
- `cargo test -p <crate>` for each crate with changed files — `check` is for a fast inner loop, never for a pre-push gate.
- Prefer `env -u AIDA_SESSION_ROLE` (see [[feedback_unset_aida_session_role_for_tests]]).
- Repo-parity tests are a real class in this repo: `.aida/discipline/` project copies must match `aida-core/templates/.aida/discipline/` masters unless listed in `LOCALIZED_DISCIPLINE_GUIDES` (`aida-core/src/templates.rs`). Touching either side means running that test.
- State what you actually ran, with the command, rather than "verified it compiles" — so a reader can see the scope and judge it.
- A PR that has been CONFLICTING has never had a meaningful green build: expect latent failures unrelated to your rebase, and do not assume a first red after a rebase is yours.

Related: [[feedback_ci_surface_beyond_cargo_test]], [[feedback_verify_edits_landed_before_claiming_done]], [[feedback_build_combined_main_after_concurrent_merges]], [[feedback_integrator_stale_base_rebase]].

### feedback_check_for_an_open_pr_before_claiming_a_spec

Run `gh pr list --state open --json number,title,headRefName` **before** `aida queue work <SPEC>`.
Multiple fleets work this repo concurrently (codex drains merge several PRs an hour), and
`aida show <SPEC>` gives **no** indication that work is in flight — BUG-1819 read
`status: needs-attention`, `owner: ""` while PR #2319 was open with CI running.

The only signal `aida queue work` gives is the **branch-name suffix**: it handed back `bug-1819-2`
because `bug-1819` already existed. It had to check for the branch to pick the suffix, so it knew — and
encoded that knowledge in a filename instead of saying so. Filed as [[BUG-1761]] (high).

**Why:** a third of a session went into independently mapping a defect another agent was already
fixing. Worse, backing out leaves damage: `aida session end` deletes the lease but **leaves the status
at In Progress**, which is the exact state whose own error reads "spec X is InProgress but no local lease
holds it" — the next agent is refused until someone hand-restores it.

**How to apply:** `gh pr list` first. If you claimed by mistake: `aida session end <id> --yes`, delete
the empty branch, and **restore the status you found** (`aida edit <SPEC> --status <original>`) — the
cleanup is not done until the status is back. Then switch to reviewing the existing PR; an independently
derived understanding of the defect makes you an unusually good reviewer of it, so the analysis is not
wasted. See also [[reference_is_ancestor_lies_after_squash_merge]].

### feedback_check_in_flight_before_rejecting

When the dialog role decides to reject a spec or pivot its architecture mid-stream, **check whether an active implementer session exists for it** before applying the change. Otherwise an implementer shipping in good faith on the original spec ends up with a committed branch that's been rendered obsolete behind their back.

**Why** (2026-05-15): User had STORY-241 (TUI workflow loop with PTY-tab architecture) queued. The implementer started on it. Meanwhile, I (dialog role) had a conversation with the user about PTY-render conflicts and we rejected STORY-241 in favor of STORY-244 (launcher architecture). I did NOT check if STORY-241 had an active lease. The implementer finished, committed `e1c67591` on branch `epic-26-4`, and exited Claude. User then discovered the committed work no longer aligned with the rejected spec. The code wasn't lost (path (a) preserved it) but the implementer's effort was partially wasted, and the user had to inspect to figure out salvageability.

**REINFORCED (2026-06-26, EPIC-54 incident):** I rejected EPIC-54 mid-fold while another (operator-run, raw `claude --resume`) agent was actively working it — and `aida session leases` did NOT show it, because a raw claude session takes a generic `harness-worktree` lease, not a spec-scoped one. So `leases | grep <scope>` is necessary but NOT sufficient. Two further lessons: (1) a *status flag is not liveness* — `In Progress` only means someone set it; use `aida status <spec>` / `aida ps` (shipped this session) to check for a live process, and ask the operator when unsure. (2) **flip-on-fan discipline:** when YOU fan an agent for SPEC-N, immediately `aida edit SPEC-N --status in-progress` so `aida list --status in-progress` is an honest live view (the merge auto-completes it; flip back to Approved on failure). (3) Two agents in one store: CRDT-safe for *different* specs, but never edit the *same* specs another agent is working; co-located agents also contend on the shared `.aida/cache.db` (now self-heals via BUG-627). Also: the statusline `inbox:N` is the *draft-inbox depth* (ungroomed drafts to triage), NOT unread mail — a count that won't clear means diagnose its source, don't repeat the action.

**How to apply:**

Before running `aida edit <SPEC> --status rejected` or filing a "supersedes" STORY:

1. Run `aida session leases --all | grep <scope>` — is there an active lease for this spec's EPIC/STORY scope?
2. Run `aida show <SPEC> | grep '^Status:'` — is it In Progress?
3. If either signals active work:
   - **Pause the rejection.** Ask the user: "STORY-X has an active implementer session — do you want to (a) finish that work first, (b) interrupt the implementer with a scope change, or (c) reject after the current commit lands?"
   - Coordinate before disrupting in-flight work

If no active work:
- Reject freely; document the supersedes/replaces relationship; cross-reference the new spec

**Pattern to avoid:**

User friction → dialog files new strategic STORY → reject old STORY immediately → implementer ships rejected spec in good faith → wasted effort.

**Better pattern:**

User friction → check for active leases → if in flight, pause rejection until coordination is possible → otherwise reject + file new STORY.

**Composes with:**

- feedback_dialog_role_responsibilities.md — queue gardening is dialog's job, but coordination with in-flight implementer is part of it
- feedback_verify_before_filing.md — same family: verify state before acting on it

### feedback_check_in_flight_review_before_merge

When you are the merge gate and driving a fast parallel fan-out, **before merging a PR check whether a review is in flight** — a reviewer agent (or the operator) may have recorded a verdict (e.g. `RequestChanges` in `.aida/review-verdicts/PR-<n>.json`) you haven't seen. Merging over it loses the finding, and if the commit carries a `(SPEC-ID)` trailer the merge **auto-completes the spec** even though the review found it half-built.

**Why:** 2026-06-15 — during a parallel agent fan-out I merged PR #933 (STORY-626) right as a reviewer recorded `RequestChanges`: the PR shipped only the `/aida-assess` cold-boot seeding, not the `/aida-advise` burndown-tier half (both in the spec's titled scope). The trailer then wrongly auto-completed STORY-626 with half its scope unbuilt. Had to reopen + ship a follow-up. The speed of fan-out + merge-gate made me skip the in-flight-review check.

**Recurrence 2026-09-18 (#1948 / STORY-1221):** I checked that STORY-1221 was *shelved* after round 1 and concluded round 2 was advisor-only — then merged at 16:35. The drain's reviewer wrote round-2 `CHANGES REQUESTED` at 16:37. The reviewer was running under a *different* spec's phase (the wave's BUG-1236 implementer had pushed onto the story-1221 branch, so the BUG-1236 reviewer phase was reviewing #1948). The spec's own state told me nothing about the PR's reviewer.

**Recurrence 2026-09-20 (#2007 / BUG-1298) — the drain owned the PR and I read the phase list wrong.** I checked recent events, saw `PhaseEntered implementer` and `PhaseEntered ci` for BUG-1298, and read that as "a spec is being worked" rather than "this PR belongs to a live drive". I shipped it at ~07:56Z; the orchestrator's reviewer entered at 07:57:27 against an already-merged PR, exited 1, and retried. No work lost, but no second reader ever read the code.

**The automated guard was blind, and I was holding its fix.** `aida pr ship` DOES refuse with "target PR is owned by the live drive" — it refused me on #2004 the same night. It only works when the drive's member carries a live `pr` binding, and `member.pr` was written only at TERMINAL outcome, so a PR-keyed ownership check has nothing to read mid-phase. That binding is TASK-1292 (#1983), which I had held for a separate hardening request. Merged it after this; the hardening is BUG-1429.

**How to apply (UPDATED):** (0) **Cheapest check first, and it is the one I skipped: read the drain's CURRENT SPEC and compare it to the PR's spec.** `python3 -c "import json;d=json.load(open('.aida/drain-state.json'));print(d.get('current'), d.get('current_phase'))"`. If the drain's current spec IS the PR's spec, do not merge — the drive owns it and its reviewer has not run yet. A phase list tells you work is happening; only the current-spec field tells you WHOSE work. (0b) Do not rely on `aida pr ship` to catch this unless the RUNNING binary contains TASK-1292 — a live wave pins its launcher binary and `make build-fast` refuses to replace it, so after merging that fix your own gate may still be blind for the rest of the session.

**How to apply:** (1) Before shipping at a hold, check the **PR**, not the spec: `pstree -ap <drain.lock pid>` for a live `claude -p`/`codex` reviewer child, the mtime of `.aida/review-verdicts/PR-<n>.json` vs the head SHA, and the events file for a `PhaseEntered … reviewer` on ANY spec in the last few minutes. A shelved/escalated spec with a live orchestrator is not a free gate. (2) If a reviewer is live, wait for its verdict (next tick) — a 10-minute delay is cheaper than a correction ledger. (3) Trailer ONLY the spec the merge FINISHES — a slice trailers the slice task, never the parent story (compounds with merge-races into wrong auto-completes). (4) When fanning out + merging fast, the throughput is worth it, but the merge step is the serialization/coordination point — slow down there, not on the implementation. Pairs with [[feedback_commit_trailer_completes_the_spec]], [[feedback_verify_ci_green_before_merge]], [[feedback_parallel_implementer_fanout_burndown]], [[feedback_check_orchestrator_before_manual_steps]].

### feedback_cherrypick_not_reset_soft_for_far_behind_branch

I gave the operator a "collapse to one clean commit" recipe — `git reset --soft origin/main && git add -A && git commit` — to un-thrash two PR branches (#815 story-582, #816 story-584) stuck on rebase conflicts (redundant arity commit + generated types.ts). It was WRONG: both branches were based on *old* main (from before that night's merges). `reset --soft origin/main` sets HEAD=main but leaves the index at the OLD branch tree, so `git diff --cached` is `main..old-tree` — which *removes* everything merged to main since the branch point. Committing it would have deleted docs/review-process.md and reverted STORY-583/BUG-513/BUG-514. Caught only by inspecting the staged diff before committing.

**Why:** `reset --soft <target>` is a valid "squash my commits" move ONLY when HEAD is *ahead of* target on the same line. When target (current main) is ahead of the branch's base, the staged diff is the inverse — a revert of all intervening work. The further behind the branch, the more it silently reverts.

**How to apply:** to land a content-approved PR branch that's far behind main and carries redundant/conflicting commits, REBUILD it by cherry-pick, not reset:
```
git -C <worktree> checkout --detach origin/main
git -C <worktree> cherry-pick <feature-commit-sha>   # only the real feature; drop redundant arity/merge commits
# resolve conflicts narrowly; for generated files (shared/types.ts) REGENERATE: cargo run -p aida-generate-types
git -C <worktree> branch -f <branch> HEAD && git -C <worktree> checkout <branch>
cargo build -p aida-cli                               # build-verify BEFORE pushing
git -C <worktree> push --force-with-lease
```
Two hard rules this reinforces: (1) ALWAYS inspect `git diff --cached --stat` before committing a history-rewrite — a recipe that looks clean can stage a revert; (2) operating in another agent's worktree, `git -C <path>` avoids cd-prompt churn and the soft-reset never touches the working tree so an undo is just `reset --soft <original-sha>`. Related: [[feedback_clean_worktree_is_not_no_work]], [[feedback_verify_edits_landed_before_claiming_done]], [[feedback_build_combined_main_after_concurrent_merges]].

### feedback_ci_surface_beyond_cargo_test

The Linux PR CI (`.github/workflows/ci.yml`) gate is broader than `cargo test --workspace`:
1. `cargo fmt --all -- --check` (drift fails CI — and the `&&`-after-pipe trap masks fmt's exit; check `; echo $?` separately).
2. `cargo clippy --workspace -- -W clippy::all` (warn, not deny — won't fail CI today).
3. **MCP stdio compatibility suite** — `bash tests/test_mcp_stdio.sh`, which builds the debug `aida` and runs `tests/test_mcp_stdio.py` as a BLACK-BOX client against `aida mcp-serve` over stdio. **This is a separate gate `cargo test` never runs.**

**What bit me (2026-06-06, BUG-449):** I changed MCP `update_requirement` to gate advisor/merge-driven status transitions, ran `cargo test -p aida-cli` (1653 green), and merged. main went RED — the stdio suite drove a spec to `planned` via `update_requirement` and asserted success, which the new gate correctly refuses. Required a fix-forward PR (#556).

**Two compounding causes:**
- **Incomplete local verification.** `cargo test -p aida-cli` excludes the stdio suite (and the workspace test of other crates). When changing **MCP tools (`aida-cli/src/mcp.rs`)**, run `bash tests/test_mcp_stdio.sh` locally before pushing. For any change, the real gate is the CI steps, not one cargo command.
- **Immediate-merge pre-CI.** `gh pr merge --auto` was rejected once ("Auto merge is not allowed"), and the retry did an IMMEDIATE squash (the repo has no required-status-check branch protection, so nothing blocks a pre-CI merge). So the merge landed before CI reported.

**How to apply:**
- Changing `mcp.rs` (or any MCP tool/resource behaviour)? Run `bash tests/test_mcp_stdio.sh` locally as part of verification, not just `cargo test`.
- **Wait for CI green before merging** (poll `gh run list --branch <branch>` then `gh pr merge`), since `--auto` is flaky here and there's no server-side CI gate. Don't trust a passing `cargo test` subset as proof main will stay green.
- When you DO break main, fix-forward immediately (don't leave main red); strengthen the broken external test to cover the new behaviour rather than just unbreaking it. See [[feedback_verify_edits_landed_before_claiming_done]], [[feedback_self_test_via_dogfood_merge]].

### feedback_clean_worktree_is_not_no_work

Two compounding mistakes, one incident (2026-05-31, recovering a stalled drain's TASK-608 worktree):

1. **Misread "clean worktree" as "no work."** A degenerate headless implementer had stalled, so I went to discard its worktree. `git -C <worktree> status --short` was empty, and I concluded "the spinner made no real edits, just echoes — safe to discard." **Wrong.** The worktree was clean because the work was *committed* (a real `fix(role): ... (TASK-608)` commit on the branch), not because no work existed. `git status` shows UNcommitted state; it says nothing about committed work on the branch.

2. **Batched the destructive delete with its own safety-check in one command.** The same shell block ran `git log task-608 --oneline -3` (which *printed the real commit*) AND `git branch -D task-608` — so the delete fired before I could react to the log output showing real work. The branch was deleted with a genuine commit on it.

Recovered because `git branch -D` only drops the ref — the commit SHA was in the printed log, so `git branch <name> <sha>` restored it, then `git push` made it durable. But it was luck that the SHA was on screen; a GC or a less-verbose delete and the work is gone.

**Why:** "look before you delete" only works if you actually look *first*. Batching the inspection and the deletion into one atomic command defeats the look — you've already destroyed by the time you read. And the inspection has to be the *right* one: for "is there work here?", `git log <branch>` / `git cherry main <branch>` (committed work), not just `git status` (uncommitted).

**How to apply:**
- Before `git branch -D`, `rm -rf`, `aida session end`, `git worktree remove`, or any discard: run the inspection as a **separate, prior** step, read its output, and only then issue the delete.
- For "does this branch/worktree hold work?" check **committed** state: `git log <branch> --oneline @{u}..` or `git cherry main <branch>` or `git log main..<branch>` — a clean `git status` is necessary but NOT sufficient evidence of "no work."
- A clean working tree next to an unmerged branch = committed-but-unmerged work. That is the *most* dangerous shape to delete, because it looks empty.

**Composes with:** the global "before deleting, look at the target — if you didn't create it, surface it" rule (this is the concrete failure mode of skipping that), and [[feedback_verify_before_filing]] (verify the actual state before acting, don't infer from a partial signal).

### feedback_commit_trailer_completes_the_spec

`aida pull` / `db sync --pull` scan merged commits and auto-bump every
spec named in a `(SPEC-ID)` trailer from Done/Approved/InProgress/
NeedsAttention → **Completed** (the STORY-86 + BUG-405 auto-bump). The
trailer is a *completion* signal, not a *relevance* signal.

**The trap (hit 2026-05-31):** I shipped a sketch-only PLAN for P1 with the
commit trailer `(STORY-491 STORY-489)`. STORY-491 was the P1 *implementation*
umbrella — nothing was built, only the plan existed — but the merge
auto-completed it. Had to `aida edit STORY-491 --status approved --force` to
re-open (the completed-spec guard correctly demands `--force`).

**Why it matters:** a wrongly-Completed umbrella story drops out of default
views and reads as "done" to the next session/operator — silent loss of
in-flight work, the worst kind because it looks fine.

**How to apply:**
- Trailer a spec **only when that merge completes it.** Ask "does landing
  this PR finish SPEC-X?" If no, don't trailer SPEC-X.
- For **plan-only, partial, or slice-N-of-M** commits, file and trailer a
  child `TASK` (e.g. `(TASK-594)` for "graph_walk core, slice 1 of STORY-489")
  and leave the umbrella story open. I did this correctly for the three
  STORY-489 slices, then slipped on the P1 plan commit — the discipline is
  *every* partial commit, including docs/plans.
- Referencing an already-`Completed` spec in a trailer is harmless (no-op).
  Referencing a still-open umbrella you don't intend to finish is the bug.
- **The PR TITLE is also a completing-trailer vector** (hit 2026-05-31): a
  squash-merge uses the PR *title* as the merge-commit subject. So a trailing
  `(SPEC-ID ...)` in the PR title auto-completes the spec even if the local
  commit subject was clean. I shipped a plan-only PR with a clean commit
  subject but titled the PR `…plan (TASK-136 + BUG-420 approaches)` — the
  squash-merge subject carried that paren and completed both. **Keep
  completing-shaped parens out of PR titles too** for plan/partial/slice work;
  put the SPEC-IDs inline mid-sentence, never as a trailing `(…)`.
- **`--force` re-open now STICKS** (since BUG-410 shipped 2026-05-31): the
  auto-bump carries a `completion_sha` dedup guard, so the *same* merge commit
  no longer re-completes a manually-reopened spec on the next `aida pull`.
  Verified live: reopened them, fresh `aida pull`, stayed Approved against the
  SAME commit. **But the guard only dedups the same commit.** When an umbrella
  has MULTIPLE merged commits naming it (a plan commit, then a cores commit,
  then a plan-update — the slice pattern), each *different* commit re-completes
  it on the next pull, so the `--force` reopen does NOT hold. Worse, completion
  also fires via **agreed-id matching** (BUG-1-113: a commit naming the display
  id `TASK-136` completes the spec stored under its node-aware id `TASK-1-111`),
  so the trail is harder to predict than "did I trailer it." **Recovery for a
  multi-commit umbrella: stop fighting it — file a FRESH spec for the remaining
  work** (unreferenced by any merged commit → stays open), as I did with
  TASK-615 for the drain-reliability wiring; the `--force` reopen alone only
  works when a single same commit is the cause. The recurring false-completion
  itself is captured as BUG-426 (needs instrumentation). Prevention still wins:
  keep an unfinished umbrella's id out of every partial commit's subject,
  trailer, AND PR title.
- **A paren that LEADS with an id completes it even with trailing prose**
  (hit 2026-06-21, SPIKE-67 slice 1; verified in code). I trailered only the
  child `(TASK-890)` but titled the PR `…rule-adherence study (SPIKE-67 slice
  1)`. The squash subject carried that paren; `push_paren_spec_ids_from_line`
  (the auto-bump's subject scanner) treats `(SPIKE-67 slice 1)` as a completion
  trailer — by **BUG-546's deliberate design**, a paren group leading with a
  spec-id is a trailer and the rest (`slice 1`) is decorative prose (its own
  example: `(TASK-800 / STORY-610 slice 1a)`). So "slice N" inside the paren
  does NOT make it safe — it still completes the leading id. The auto-bump scans
  the commit **message subject only**; it does NOT read the diff or `trace:`
  comments (I first *inferred* the trace-comment mechanism and filed BUG-603 on
  it — wrong; BUG-603 rejected as working-as-designed. Lesson:
  [[feedback_verify_lore_against_code_not_docs]] — read the scanner before
  asserting how it fires). **Rule: never put an unfinished umbrella's id in a
  PR-title paren, even as `(ID slice N)`.** Write the slice label as bare prose
  (`SPIKE-67 slice 1`) or parenthesize only the completing child `(TASK-890)`.
  Recovery: `aida edit <umbrella> --status in-progress --force` (held — single
  commit, completion_sha guard). Verify umbrella status after every `aida pull`.
- Relates to [[feedback_precise_lifecycle_vocabulary]] and
  [[feedback_verify_edits_landed_before_claiming_done]].

### feedback_count_rounds_from_commits_before_claiming_a_finding_repeated

2026-09-20: I opened a BUG-1429 rework brief with "AC3 survived round 1, so this brief does not restate it." PR #2021 had exactly ONE commit — it was the first verdict. I had inferred a prior round from the reviewer's word "still". Minutes earlier the advisor had sent me the counterexample from the other direction: STORY-1350's round after their requeue was REBASE-ONLY (patch-ids 53489249/89354a59 both aa3d8c56dd493aca, 08f886c4/3bd68091 both 557d99b045804e18) — the head moved, every surface reported progress, and the branch's content was byte-for-byte unchanged, so neither finding was touched because nothing was attempted.

**Why:** "this finding survived a round" licenses a *different* move than a first-round finding — it says the BRIEF is suspect and should be rewritten rather than repeated. Acting on a false repeat sends someone to redesign a requirement that was never attempted, or to supply a mechanism to a loop that is not consuming briefs. Both failures are the same one at different removes: treating a surface signal of progress (head moved; reviewer said "still") as evidence about the branch's own contribution.

**How to apply:**
- `gh api repos/O/R/pulls/N/commits -q '.[] | "\(.sha[0:9]) \(.commit.author.date)"'` — count rounds from commits, never from a verdict's wording.
- Across rounds compare `git patch-id` per commit, NOT the tree diff: a rebase onto newer main changes the tree substantially while changing nothing the findings asked for.
- No new patch-id ⇒ the surviving finding is evidence about the DRAIN, not the brief; rewriting the brief will not help.
- A first-round miss of an unimplementable criterion is the EXPECTED outcome, and saying so is stronger than a survival claim — it moves the fault to the acceptance criterion where it belongs.
- Post a correction rather than editing the framing away; a false "this repeated" claim is the kind a reader cannot cheaply verify, so it should stay visible.

Related: [[feedback_rework_briefs_advisory_framing_gets_skipped]], [[feedback_delegated_findings_are_not_verified_ground_truth]], [[feedback_verify_agent_pr_done_claims_against_diff]], [[feedback_mark_relayed_operator_decisions_as_unverifiable]].

### feedback_dont_rebuild_main_with_another_lanes_wip

Do not run `cargo build` in `/home/joe/ai/aida` when the checkout carries
uncommitted changes you did not make. `main`'s `target/debug/aida` IS the live
host-wide `aida` binary, so a build there **installs** whatever is in the
working tree as the binary every other session and lane then runs.

**Why:** after merging TASK-1562 I wanted to smoke-test the shipped
`aida worktree reclaim` from a main build and get the real reclaimable figure.
The main checkout had 13 modified files from another lane (BUG-1687 WIP,
including `aida-cli-lib/src/lib.rs`, where CLI dispatch lives). That build would
have replaced the host's live `aida` with a binary containing someone else's
unreviewed, uncompiled-by-CI work — for a nice-to-have number.

**How to apply:** check `git status` in the main checkout BEFORE building there.
If it is dirty with work that is not yours, either build in a clean sibling
worktree or skip the smoke test and say so. A post-merge convenience figure
never justifies swapping the host's binary. Related:
[[feedback_never_switch_branches_in_operators_checkout]],
[[feedback_never_stash_for_a_mutation_proof]],
[[reference_build_slots_sccache_mold]].

### feedback_edit_at_merge_reaches_only_the_commit_message

**Editing the squash message at merge time reaches the commit message and nothing else.**

- Wrong text in a **commit message / PR body** → edit-at-merge works, costs no CI cycle.
  (Worked 2026-09-21 on #2047: a wrong sizing sentence lived only in the commit body, and
  this repo's squash carries the branch body verbatim into main.)
- Wrong text in **source** — a doc comment, a constant, a test name → **always a round.**
  It is in the tree after the merge regardless of the squash message, and permanent until
  someone writes another commit. (#2048, same night: `41 of 147` in a `///` comment.)

**How to apply:** before accepting "we can fix that at merge", `git diff` the branch and find
which of the two it is. A PR can carry the same wrong sentence in *both* places — then the
round is required anyway and edit-at-merge only covers half.

**When the round is required, put the corrected text in the mail.** Making the implementer
re-derive a correction from three threads is the difference between one round and two.

Pairs with [[feedback_never_conclude_from_truncated_command_output]] — both are about a
remedy that only reaches part of the artifact.

### feedback_edit_in_the_worktree_not_main_checkout

When a spec already has an implementer worktree (e.g. `aida queue work <SPEC> --guided` / `aida worktree enter` created `/home/joe/ai/aida-<spec>` on its own branch), the Edit/Write tools operate on **absolute paths** — so passing the *main checkout* path (`/home/joe/ai/aida/...`) writes the change to `main`, NOT the worktree branch, even though the "intended" branch is the worktree's.

**Why:** in the FR-284 guided session I edited `/home/joe/ai/aida/aida-cli-lib/...` while the work belonged on `fr-284-work` at `/home/joe/ai/aida-fr284`. The changes landed uncommitted on `main`; the worktree's `cargo build`/`cargo test` compiled the *unchanged* worktree source, so tests "passed" against code that didn't have my changes. Cost a revert (`git checkout --`) + file relocation (`cp` into the worktree, both at the same origin/main base) to recover.

**How to apply:** at the start of implementation, resolve the worktree path (`aida session leases | grep <SPEC>`), and target every Edit/Write and every `cd … && cargo` at THAT path. If a `--list`/grep shows your new test module absent after a build, suspect you edited the wrong checkout — verify with `grep <new-symbol> <worktree>/<file>` before trusting green tests. Related: [[feedback_isolate_own_committing_work_from_shared_worktree]], [[feedback_fan_committing_agents_with_worktree_isolation]], [[feedback_clean_worktree_is_not_no_work]].

### feedback_end_session_with_db_sync_push

Ordinary AIDA store writes — `aida edit`, `aida comment add`, `aida add`,
`aida schedule done` — push `aida-store` to **origin only**. The configured mirror
(`[store.sync] mirror_remotes = ["gitlab"]` in `.aida/config.toml`) fans out **only** on an
explicit `aida db sync --push`. Nothing in the write path or in `aida schedule tick` runs it.

Measured 2026-09-30: 6 ordinary advisor writes left origin current and gitlab exactly 6 behind.
`aida db sync --push` printed "Mirroring aida-store → gitlab... Mirror push complete" and
restored in-sync on both hubs.

**Why:** that daily drift is what trips `hub-drift-guard` and routes an advisor seat job whose
remediation is always the same pure fast-forward. Two consecutive days (2026-09-29, 2026-09-30)
closed with identical zero-judgment fixes. Burning a judgment seat on ceremony also trains it to
wave through the *real* non-fast-forward case the config comment warns about.

**How to apply:** after any session that writes specs, comments, or job reports, run
`aida db sync --push` before writing the handoff. To diagnose drift, `aida remote status`, then
verify a fast-forward is safe with `git merge-base --is-ancestor <hub-tip> <local>` for each hub
**before** pushing — `aida remote reconcile` (dry-run first, `--execute` to apply) reports
"behind local" and never force-pushes. Status may print `✗ DIVERGED` when both hubs are merely
*behind* local; that is a fast-forward, not a conflict, so read the reconcile plan rather than
the DIVERGED label.

Filed as BUG-1746 (the write path should fan out, or do it at a boundary). Until that lands this
is manual. See [[feedback_read_the_verdict_not_just_the_check_rollup]] and
[[reference_git_push_dry_run_still_pushes_the_mirror]] — the code-branch pre-push wrapper DOES
mirror to gitlab automatically, which is why only the *store* branch drifts.

### feedback_fan_committing_agents_with_worktree_isolation

When using the Agent tool to fan out work that **commits, branches, or opens a PR**, pass `isolation: "worktree"` so the agent operates in its own git worktree. A **bare** agent (no isolation) runs in the **main repo's working tree** — it does `git checkout -b <branch>`, commits, pushes, and **leaves the main repo checked out on that feature branch**.

**Why:** (incident 2026-06-19) two research agents (cross-vendor portability TASK-870, RYO benchmark TASK-871) were fanned **without** `isolation: worktree` (the implementers in the same session WERE isolated; the research agents weren't). Each `git checkout -b`'d in the main repo. After their squash-merges landed on origin, local `main` was actually still on `task-871-...` with the **un-squashed** commit → `git merge --ff-only origin/main` aborted with *"Not possible to fast-forward"* (the squash created a different commit than the branch's). Looked like a scary divergence; was just the wrong branch left checked out. Also leaves stale local branches that the git-guardrail blocks `git branch -D` from cleaning.

**Recovery** (if it happens): `git checkout main` → `git merge --ff-only origin/main` (main IS an ancestor of origin once you're back on it) → the stale feature branch is harmless (guardrail blocks deleting it; ignore or let the worktree-GC handle it). Pairs with [[feedback_shared_tree_tracking_ref_hazard]] (committing/pushing from a shared tree is the deeper hazard) and [[feedback_clean_worktree_is_not_no_work]].

**Rule:** any fanned agent that will `git commit`/`push`/PR → `isolation: "worktree"`. Read-only research (no commits) can be bare. When in doubt, isolate — the cost is ~200-500ms of worktree setup; the cost of NOT isolating is a polluted main checkout + a false-alarm divergence.

### feedback_fetch_before_commit

Before committing during a session that's been open more than a few minutes (or after the user has done anything in another shell/machine), fetch origin first and check ahead/behind. The action verb is now **`/aida-rebase`** (TASK-103/104/105 — landed 2026-05-15):

```bash
aida rebase --dry-run --json    # fetches, classifies (clean/ahead-only/behind-only/diverged-safe/diverged-risky), no side effects
aida rebase --auto              # execute the rebase when the class is safe
```

The proactive-invocation playbook (when to fire it unprompted) lives in the `/aida-rebase` skill's "When to Use" section — `aida-core/templates/skills/aida-rebase.md`. Manual equivalent if `aida` isn't on PATH:

```bash
git fetch origin "$(git rev-parse --abbrev-ref HEAD)"
git rev-list --left-right --count HEAD...@{u}
# "0 N"  → behind by N: pull --rebase before committing
# "M 0"  → ahead by M: safe to commit
# "M N"  → diverged: rebase before commit if M is mine + cheap to replay
```

**Why:** 2026-05-13 incident — I committed the STORY-86 plan file locally (`b18c1d45`) without fetching first. Meanwhile PR #19 had just merged to origin/main. User's next `git pull` failed with divergent-branches error, requiring a manual `git pull --rebase` recovery. The commit was new-file-only and couldn't conflict, so the eventual rebase was trivial — but the divergence surprise itself was avoidable. Long sessions accumulate stale-HEAD risk; the agent should treat session-age as a signal to refresh before staging.

**How to apply:** Specifically when:

- Session has been running > ~15 minutes since last `git pull` or `aida pull`
- User has been working in another shell, on another machine, or via web UI (PR merges, GitHub Actions, etc.)
- About to `git add` + `git commit` on a long-lived branch like `main`
- After a long `aida ...` chain that may have changed orphan-store state but not code state

Skip when:

- Working in a fresh worktree just spawned by `aida queue work` (already pulled)
- The commit is on a feature branch nobody else touches
- About to push immediately after committing AND push will catch the issue with its own divergence check (TASK-54 wired this for `aida push`)

**Related**: `/aida-rebase` (TASK-104) now automates the detect/classify step; TASK-97 (`aida pull --autorebase`) and TASK-98 (`/aida-commit` precheck) will delegate to the `aida-core::rebase` detection module rather than re-deriving ahead/behind.

**Long-form recovery recipe**: `docs/recipes/divergent-branches.md` in the AIDA repo.

### feedback_fresh_branch_can_miss_interleaved_squash

When PRing slices sequentially through one worktree, a branch you `git checkout -b off origin/main` can silently **lack a sibling PR you merged seconds earlier** — GitHub squash-merges from concurrent PRs interleave, so the `origin/main` tip you fetched may be a different PR's squash that doesn't yet include yours. (2026-06-29: cut `task-983` off origin/main right after merging #1197/TASK-982; the branch was missing #1197 entirely — another agent's #1205 squash had become the tip.)

**Why:** squash-merge creates a NEW commit on the base's then-current tip at merge time; with concurrent merges the linear history isn't what "I merged mine first" intuition expects. Building TASK-983 on that base meant my changes sat on a main without TASK-982's code — a PR merge could have fought it.

**How to apply:** before building a follow-up slice on a fresh branch, assert your prior merged PR's squash is actually present:
`git merge-base --is-ancestor <prior-squash-sha> HEAD && echo have-it || echo MISSING`
(or check a known symbol from it, e.g. `grep -c fn_added_by_prior_pr`). If missing, `git rebase origin/main`, resolve, and re-verify the **combined** build before pushing. Pairs with [[feedback_build_combined_main_after_concurrent_merges]] (union-build check) and [[feedback_reset_to_main_between_sequential_specs]] (reset between specs). The guardrail hook blocks `reset --hard`; use `checkout -b` + `rebase` instead.

### feedback_gh_run_rerun_is_not_fresh_ci

When a CI run for a PR fails on a commit, and you've pushed a fix afterward, the instinct is to "rerun" the failed CI. This is correct intent but wrong tool:

- **`gh run rerun <run-id>`** re-runs the EXACT same workflow on the EXACT same commit SHA that originally triggered it. Designed for flaky-test recovery — the test fails for environmental reasons, you rerun on the same code, hopefully it passes. Useful for that case; NOT useful for "I pushed a fix; re-test on the new code."
- **`gh workflow run <workflow> --ref <branch>`** dispatches a workflow on the chosen ref's current HEAD. This DOES pick up new commits but requires the workflow to support `workflow_dispatch` trigger.
- **`git commit --allow-empty -m "..."` + `git push`** is the universally-working escape hatch — it creates a new commit, which triggers GitHub Actions' on-push behavior naturally, which checks out the new HEAD.

## When CI seems to "not pick up" your fix push

Most common cause: you pushed during the brief window when CI's environment was checking out the previous commit. The old run completes on the old SHA; the new push *should* have triggered a fresh run on the new SHA. If `gh run list --branch <branch>` shows only one run, something prevented the second push from triggering CI:

- The branch protection rules might require a manual workflow dispatch
- The push happened during a race with the previous run's setup
- GitHub Actions occasionally deduplicates rapid-fire pushes (rare but observed)

**Diagnostic**: verify origin's HEAD has the fix (`git ls-remote origin <branch>`), then check `gh run list --branch <branch>` for the latest run's commit SHA. If the latest run is on the OLD SHA, force a fresh run via empty commit + push.

## Empirical instance 2026-05-22

PR-193's CI failed on a test count assertion (`discipline_pack_scaffolds_five_docs_plus_readme` asserting 6 vs 7). Master pushed a fix (renamed test + count corrected); CI was still showing the old failure. Master used `gh run rerun` — which re-ran the OLD workflow on the OLD commit. Same failure repeated. Diagnostic: origin's HEAD had the fix; the CI run was on the previous SHA. Empty commit + push triggered fresh CI on the new SHA; passed.

## How to apply

When CI is failing on a commit you've already fixed:

1. Verify origin's HEAD has your fix: `git ls-remote origin <branch>` matches your latest local commit.
2. Check the failing CI's SHA: `gh run list --branch <branch>` shows the commit each run tested.
3. If the failing run is on a previous SHA: push an empty commit (`git commit --allow-empty -m "..." && git push`) to force a fresh CI on your latest commit.
4. Don't use `gh run rerun` for this case; it won't help.

## Composes with

- [[feedback_run_help_before_suggesting_flags]] — gh CLI semantics warrant verifying via `--help` before assuming behavior.

### feedback_idle_peer_sessions_as_independent_review_seats

Proven 2026-10-02 (relay #7, Joe-directed proxy night): when the advisor seat both recorded a verdict and implemented the rework (BUG-1771 #2339), and when it authored a branch (STORY-1480 #2367), the independent review was dispatched to idle peer Claude sessions via ListAgents + SendMessage (aida-e9, aida-0c). Both performed genuinely independent reviews — re-verified claims against source themselves, found real new facts (rest.rs exact-match confirmation; the WORK_PHASES allowlist span gap → BUG-1786; the shallow store-refresh race → BUG-1785), refused the gates that were theirs to refuse (merge-hold clear; merging on own verdict), and executed the merge + pull + sync cleanly.

**Why:** the independence rule needs a different *seat*, not a different human; peer sessions have their own context, judgment, and permission posture, unlike subagents of this session (which share its lineage).

**How to apply:**
- Brief like a rework hand-off: declare the seat ("this message IS the brief"), give verdict file path, exact diff range, scope boundaries, build discipline for this host, and name who may NOT approve (author, original verdict recorder).
- `notify_when_idle: true` for a completion signal; an immediate idle notice with no worktree/ack may mean the session EXITED — re-check ListAgents (refs change) and re-route to the surviving session.
- A session going idle ≠ working (see [[feedback_idle_seat_is_not_a_working_seat]]); confirm pickup by artifact (worktree appearing, ack message), not by the notice.

Pairs with [[feedback_proxy_reviewer_with_independence_rule]], [[reference_merge_hold_clear_is_human_only]].

### feedback_instrument_dont_infer_on_contradiction

When a debugging investigation hits a point where the available artifacts (git reflog, captured stdout, status output) appear to **contradict** a working hypothesis, the failure mode is to keep re-reasoning from those artifacts and flip the conclusion. Artifacts are often incomplete or misleading (truncated by `tail`, coalesced reflog entries, stdout/stderr interleaving, stale captures) — inference from them is not ground truth.

**Why:** On BUG-404 (2026-05-30, ship-time auto-bump miss) I (1) filed the correct "empty scan range" root-cause, (2) saw a reflog entry that *seemed* to show a non-empty range, (3) **wrongly retracted** the hypothesis to "observation pending reproduction" on that inference, (4) only recovered after a clean de-confounded repro, then (5) settled it definitively by adding opt-in `AIDA_DEBUG_AUTOBUMP` instrumentation that logged `pre_code_sha`/`post_head`/range/flip-count live. The instrumentation showed `pre==post`, empty range, 0 flips — exactly the original hypothesis. The reflog had misled me; the instrumentation was ground truth.

**How to apply:**
- When artifacts contradict a hypothesis you had reason to believe, treat the contradiction as a signal the *artifacts* may be incomplete, not necessarily the hypothesis. Don't flip the conclusion on inference alone.
- Reach for **cheap, opt-in, default-off instrumentation** (env-gated `eprintln` of the exact runtime values the code branched on) and reproduce live. Gate it so it can stay in as permanent visibility (it paid for itself once; it will again).
- Prefer a **regression test proven to fail without the fix** over "tests pass" — temporarily revert the fix, confirm red, restore, confirm green. That proves the test guards the regression.
- Don't ship a fix to a core path you "can't fully explain." The instrument-and-catch step converts a guess into a fact before the fix lands.
- Pairs with [[feedback_verify_before_filing]] (verify the cause, not just the symptom) and [[feedback_verify_edits_landed_before_claiming_done]]. The dogfood close: ship the fix through the system it repairs so its own merge exercises the new path ([[feedback_self_test_via_dogfood_merge]]).

### feedback_integrator_stale_base_rebase

When integrating many PRs from a parallel fan-out (each agent branched off `origin/main` at a different time), two integrator failure modes recur:

**1. Stale-base CI failure looks like a code bug but isn't.** A PR fails CI on an *existing* test unrelated to its diff (e.g. TASK-330 "session-id on comments" failed `bug670_agent_status_tests::actionable_matches_queue_list` — a queue/status test it never touches). Diagnosis: run the failing test on **current main** (it passes) + check `git rev-list --count <branch>..origin/main` (branch was N behind). The fix is a **REBASE onto current main, not a code change**. **How:** bounce it back to its implementer agent via SendMessage ("rebase onto origin/main, re-verify gates, `--force-with-lease`") — the agent has context and rebases *in its own worktree*; do NOT `git checkout`+rebase+force-push someone else's branch from your integrator (main) checkout (that's the [[feedback_never_force_push_main_and_chain_cd]] / shared-tree hazard).

**2. CI-gate must check the conclusion, not just run the check.** A merge script that prints `gh pr checks` output but then runs `gh pr merge` *unconditionally* will merge a PR whose CI is still `pending` — I did this once (combined build happened to be green, but it violated the gate). Gate structurally: `ci=$(gh pr checks <n> | grep 'Build (ubuntu' | awk '{print $2}'); [ "$ci" = "pass" ] && gh pr merge ...`. NEVER merge on `pending`/`fail`. Pairs with [[feedback_verify_ci_green_before_merge]] and [[feedback_build_combined_main_after_concurrent_merges]] (build the combined main between merges — several fan-out PRs each green can still break the union, esp. multi-crate changes → `cargo build` the touched crates or the workspace).

**Why:** in a high-throughput integrator run these two mistakes each nearly landed a red or unverified commit on main; the gate + rebase-not-code-fix keep the merge cascade honest. (Learned 2026-07-01 integrating ~14 fan-out PRs while the operator was away.)

### feedback_investigation_and_implementation_must_use_the_same_method

2026-09-20: BUG-1287's fix stopped working three hours after merging. The investigation classified squash-merge artifacts by **patch-id** — robust, because a squash rewrites SHAs while preserving the diff. The implementation shipped **tree comparison** (`git diff --quiet origin/main <branch> -- <files>`), which asks "does this match main RIGHT NOW". Those coincide only until someone else edits those files. Thirteen merges landed, and the fix became inert in exactly the situation it exists for: a repo merging steadily, where worktrees accumulate *because* nobody reaps promptly.

Same branch, same command, three hours apart: exit 0 (Removable) → exit 1 (Keep).

**Why nobody caught it.** I relayed the lane's report saying "verified each with `git patch-id`" and separately read a diff containing `git diff --quiet`. I had both facts and never compared them. The advisor verified the PR harder than anything else that night — ran the exact comparison the code runs, checked both directions, confirmed it protected a branch with real commits — and every word was true at the moment it was run. A point-in-time check cannot catch a mechanism that decays, and nothing in that method would have revealed it.

**How to apply:**
- When a spec investigates by one method and implements another, say so in review and ask why. That substitution is the signal — not the code quality, which looked fine.
- Ask of any comparison: *does this answer decay?* "Matches main now" decays with every merge; "this patch-id is among main's patch-ids since the merge-base" does not. Prefer the time-invariant form.
- Relaying a lane's stated method is not verifying the code uses it. Check the diff for the mechanism the report names.
- A verification that passes now and would pass differently later needs a test that moves the world — here, a fixture with commits landing after the branch.
- Failure direction matters but is not sufficient: this decayed safely (kept rather than deleted) and was still completely inert.

Related: [[feedback_delegated_findings_are_not_verified_ground_truth]], [[feedback_verify_fix_mechanism_before_locking]], [[feedback_confirm_a_claim_by_a_different_method]].

### feedback_is_it_pushed_is_a_two_hub_question

2026-09-19: an OOM killed a drain mid-phase, leaving TASK-1274 with a commit on `task-1274-work`. I ran `git ls-remote --heads origin task-1274-work`, got nothing, and told the advisor the worktree and local branch were "the only copy of that work". They checked the other hub: `git rev-parse gitlab/task-1274-work` already matched the local tip, pushed 11 minutes earlier, before the OOM. The implementer had pushed to the mirror.

**Why:** `[store.sync] mirror_remotes` fans the store out to `gitlab.joemooney.com`, and branches reach it too. `origin` is one of two hubs, so a single-hub check answers half the question — and reports unique work that is not unique. This is the multi-hub drift the mirror config exists to prevent, seen from the other side: drift detection worries about a hub being BEHIND, this is a hub being AHEAD of the one you looked at.

**How to apply:**
- Before calling a commit unpushed, at risk, or the only copy: `git ls-remote --heads origin <b>` AND `git rev-parse gitlab/<b>` (or `git ls-remote --heads gitlab <b>`).
- Same rule before deleting a branch as abandoned — delete on BOTH hubs, and confirm gone on both (`git push <hub> --delete <b>` each).
- `aida remote status` / `aida doctor --category remote-drift` compare hubs; prefer them over a hand-rolled single-remote probe.
- The near-miss that makes it concrete: a recovery plan built on "this is the only copy" would have justified risky preservation steps that were never needed.

Related: [[feedback_verify_pr_head_after_push_gh_pr_checks_lies_on_merged_pr]], [[feedback_shared_tree_tracking_ref_hazard]], [[project_main_branch_protection_requires_only_merge_hold_gate]].

### feedback_isolate_own_committing_work_from_shared_worktree

When I'm doing committing work at the keyboard AND a background/sibling agent holds a lease on the **main working directory** (e.g. a `harness-worktree main` general-purpose lease at the repo root), that sibling can `git checkout`/merge under me — switching HEAD to `main` and carrying my **staged** changes onto the wrong branch. If I then commit, I commit to main (or clobber the sibling's merged PRs, since my staged full-file blobs are based on an older tip).

**Why:** git worktrees that share a path share HEAD and the index; a concurrent brancher is invisible until my branch has silently changed under me. Staged changes survive a checkout, so the corruption is quiet.

**How to apply:** the moment I have committing work to do while a sibling shares the main worktree, create a **dedicated `git worktree add <path> <branch>`** and do all edits/builds/commits there — the sibling can't switch a branch that's checked out in another worktree. If I discover I've been switched (HEAD on an unexpected branch with my changes staged): save my edited files, `git restore --staged` + `git checkout --` to clean the shared tree (leave the sibling undisturbed), then re-apply in an isolated worktree **rebased onto current main** (not a stale base — full-file copies clobber intervening merges). Related: [[feedback_fan_committing_agents_with_worktree_isolation]] (the reverse — isolating agents I spawn), [[feedback_shared_tree_tracking_ref_hazard]].

### feedback_match_operator_mode_specific_when_execution

The advisor has two operating modes, and they should match the operator's:

- **EXPLORATION mode** — operator is planning, considering trade-offs, asking strategic questions. Abstract reasoning + options + rationale is correct. Concrete commands would be premature.
- **EXECUTION mode** — operator is mid-action: shell open, pasting command output, asking "what next?" Concrete commands are correct. Abstract guidance forces the operator to translate strategy → commands themselves, which is the work they came to the advisor to skip.

The failure mode: advisor gives EXPLORATION-mode output when operator is in EXECUTION mode.

## Empirical example 2026-05-23 morning

Operator hit TASK-488 drain failure (BUG-352 + BUG-354 stack). Pasted the orchestrator's error output asking for next steps. Advisor responded:

> "Cleanest recovery is to clear all TASK-492-related leases + restart drain from a fresh main worktree state. Or defer — the bug-pile is queued; fixing them is more productive than fighting them further."

That sentence is technically correct. It's also abstract — three suggestions in vague form, no specific lease IDs, no specific commands. The operator's reply: *"does not provide specifics, it leaves me wondering what I need to do."*

The cost: operator had to ask a clarifying question. The advisor's next response (after the clarification) was concrete:

```
aida session end 019e5580 --yes
git worktree remove /home/joe/ai/aida-task-492
git branch -D task-492-2
aida session end 019e53a6 --yes
aida queue work TASK-488 --auto-complete --no-human=both --escalate-blocks
```

THAT was the work-saving output. The abstract version forced an extra round trip.

## Signals that operator is in EXECUTION mode

- Shell prompt visible in recent transcript (e.g., `(role:implementer) (aida-debug) joe@imac:...$`)
- Paste of command output (orchestrator error, gh CLI output, drain failure block)
- Question shape: "what next?" / "what do I do?" / "should I X?" / "where do I run this?"
- Time pressure indicators ("I want to launch overnight" / "before I sleep")
- Operator running back-to-back commands and pasting between them

## Signals that operator is in EXPLORATION mode

- Open-ended strategic question ("should we...", "what about X?", "I'm thinking about...")
- No paste of execution output
- Discussion of design trade-offs, architecture, naming, scope
- Reflective tone, longer messages, multiple concerns at once

## How to apply

When drafting a response, check the recent transcript context:

1. If EXECUTION mode signals dominate → output should be commands, not strategy. Specific lease IDs, specific commands, specific file paths. Tables work if multiple commands; numbered lists for ordered steps. Reasoning AFTER the commands, not instead of.

2. If EXPLORATION mode signals dominate → output should be reasoning + options. Trade-offs explicit. Commands only as "here's how this would look if you decide X."

3. When uncertain → default to EXECUTION mode if any shell prompt is visible in the last few turns. Operators rarely complain "you were too specific"; they often complain "you were too abstract."

## The discipline question

Before sending an advisor response that contains words like "recommend" / "cleanest" / "could also" / "may want to" — check:
- Is the operator's last message a paste of command output?
- Did the operator's last message ask "what next?" or similar?

If yes to either → reword as commands + tables + numbered steps. The "recommend" framing belongs in EXPLORATION mode.

## Composes with

- [[feedback_explicit_paste_ready_prompts]] — when prose includes commands the user runs, the boundary must be marked. This memory is the broader principle.
- [[feedback_worth_noting_means_note_it]] — both memories address advisor verbal patterns that fail the operator.
- [[feedback_dialog_role_responsibilities]] — dialog/advisor role discipline; this refines mode-matching specifically.
- [[feedback_parallel_vs_sequential_ui]] — output-format-matches-content principle, at a lower level than mode-matching.

## Hook-side defense (optional follow-on)

A Stop hook could detect EXECUTION-mode signals in transcript + check advisor's last response for abstract-guidance phrases without code blocks. Heuristic; over-fires possible. File as a TASK if the memory alone doesn't catch enough cases.

### feedback_name_only_cannot_see_an_inline_rust_test

TASK-1536's stranded branch touched four `.rs` files and no `tests/` path, so relay session #5
recorded "the diff adds no test file ... 'Fixture tests for each' is very likely unsatisfied" and
made that session #6's first thing to check. All three ACs **did** have fixtures — inline
`#[cfg(test)]` unit tests inside `scaffold_refresh.rs` and `doctor_cmd.rs`.

**Why:** `--name-only` reports paths, and a Rust unit test lives in the same file as the code it
tests. For a Rust repo, a file list is evidence about *integration* tests only; it says nothing
about unit coverage. Concluding "no tests" from it is the same class of error as concluding a test
is wired into CI because the file exists ([[feedback_a_test_on_the_branch_may_be_wired_into_nothing]]).

**How to apply:** before judging a Rust branch's test coverage, grep the diff body for `#[test]`
and the AC's identifier (`/usr/bin/grep -n "fn <spec_id_lowercased>" <files>`), or
`cargo test -p <crate> --lib <spec_id>` and read the matched count. Then prove the coverage is
non-vacuous by mutation ([[feedback_prove_a_test_fails_without_the_fix]]). An inherited claim that a
branch is incomplete deserves the same re-measurement as an inherited blocker
([[feedback_aida_review_record_is_the_noninteractive_path]]).

### feedback_never_force_push_main_and_chain_cd

Two compounding mistakes, 2026-06-13, that force-pushed `main` twice and dropped a merged PR's commit:

1. **A failed `cd` ran git-ops in the wrong worktree.** I wrote `cd /path/to/worktree` as a *separate statement* from the `git commit --amend && git push --force-with-lease` that followed. The worktree had been pruned, so `cd` failed — but the next *statement* ran anyway, in the main worktree, on `main`. **Always `&&`-chain the cd with the git commands** (`cd X && git commit … && git push …`) so a failed cd aborts the whole thing. Never run worktree git-ops as statements separate from their cd.

2. **`--force-with-lease` does NOT protect a commit you've already fetched.** The lease only guards against *unfetched* remote changes. I had fetched the freshly-merged PR commit (`06df0d3e6`), so the lease saw it as "known" and happily overwrote it. The lease is not a substitute for never-force-pushing-a-shared-branch.

**The rule: never force-push `main` (or any shared branch). Period.** Force-push only your *own* feature branch, and only after verifying `git rev-parse --abbrev-ref HEAD` is that branch, not main.

**Why it mattered (and why it was survivable):** the dropped commit was a single re-creatable change (a 7-line mdBook redirect). I verified the blast radius with `git log <main>..<dropped>` (exactly one commit) + diffstat (one file) before concluding nothing else was lost, then re-landed the change via a fresh, spec-traced PR. Recovery worked *because* the change was trivial and re-creatable — a force-push that drops real code is unrecoverable from the remote.

**How to apply:**
- Risky/irreversible git ops (force-push, hard reset, branch delete) → **confirm with the operator first**, every time. This is already standing guidance ([[feedback_shared_tree_tracking_ref_hazard]]); the force-push escalates it.
- Verify the branch before any `--force`: `git rev-parse --abbrev-ref HEAD`.
- After an accidental history rewrite: don't panic-fix; assess with `git log A..B` + diffstat to find the exact blast radius, then re-land via a clean traced PR rather than more force-pushing.
- This is the AIDA-dogfood point too: the un-spec'd shortcut that started it ([[feedback_shared_tree_tracking_ref_hazard]]) is the front-gate bypass AIDA exists to prevent — the operator's "changes under an approved issue, in a worktree" correction was right.

### feedback_never_git_checkout_ref_dashdash_in_a_live_checkout

2026-09-19: investigating a CI-red on PR #1976 I wanted to see every `Message {` initializer on `bug-1231-work`. I ran `git checkout -q origin/bug-1231-work -- .` in `/home/joe/ai/aida` as a setup step before grepping. It staged and wrote 15 tracked files from that branch over the main checkout. Recovered with `git reset -q HEAD -- . && git checkout -- <dirs>`, and nothing was lost — but only because the tree happened to have no uncommitted work. That was luck, not design.

**Why:** `git checkout <ref> -- <pathspec>` is a *write*, not a read: it copies files out of the ref into the index and the working tree, with no prompt and no diff preview. It reads like the read-only `git checkout <ref>` it sits next to, and the `--` makes it look scoped. The command one line earlier in the same session — `git grep -n "Message {" origin/bug-1231-work -- '*.rs'` — already did the job correctly.

**How to apply:**
- To READ one file from a branch: `git show <ref>:<path>` (pipe to `sed -n`/`grep`).
- To SEARCH a branch: `git grep -n <pattern> <ref> -- <pathspec>`. Both are read-only.
- To compare: `git diff <ref>...HEAD -- <path>`, never a checkout.
- Reserve `git checkout <ref> -- <path>` for deliberately taking a file from another branch, and run `git status` first so you can see what you are about to lose.
- If it happens: `git status --short` to list what changed, `git reset HEAD -- .` to unstage, `git checkout -- <dirs>` to restore, then verify `git rev-parse HEAD origin/main | uniq | wc -l` is 1.

Related: [[feedback_cherrypick_not_reset_soft_for_far_behind_branch]], [[feedback_edit_in_the_worktree_not_main_checkout]], [[feedback_clean_worktree_is_not_no_work]].

### feedback_never_stash_for_a_mutation_proof

To revert files for a mutation proof, use `git checkout <ref> -- <paths>` and restore with
`git checkout HEAD -- <paths>`. Never `git stash push -- <paths>` / `git stash pop`.

**Why:** `git stash push -- <paths>` creates NOTHING when those paths are already clean — which is
exactly the case once you have committed the fix you are trying to mutate. It prints no warning.
The paired `git stash pop` then pops `stash@{0}`, which in this repo is a months-old `autostash`
from a different branch. On 2026-09-29 that dropped conflicted content into five files
(`lib.rs`, `mailbox_cmd.rs`, `mcp.rs`, `aida-core/src/mailbox.rs`, `models.rs`) that had nothing to
do with the work in flight, and left `UU` conflict markers in the tree.

**How to apply:** The measurement that exposed it is also the tell — a mutation proof that reports
*no* change (the suite still passes, the count is unchanged) means the revert did not happen, not
that the code is insensitive. Stop and check `git status --short` before believing it.
Recovery is `git checkout HEAD -- <the five files>`; the stash is NOT dropped on conflict, so an
unrelated session's work survives — do not `git stash drop` to tidy up. See
[[feedback_prove_a_test_fails_without_the_fix]] and
[[feedback_never_switch_branches_in_operators_checkout]].

### feedback_never_switch_branches_in_operators_checkout

Joe works interactively in `/home/joe/ai/aida` while an agent session runs. On 2026-09-29 I ran
`git checkout -b docs-spike-92-epic-74` there to commit some untracked docs; Joe's next `git status`
showed him on a branch he never chose: *"you changed the branch we are on"*.

**Why:** the primary checkout is the operator's workspace, not the agent's. AIDA's own CLAUDE.md
already says "work in an isolated sibling worktree" — I treated that as a rule about code changes
and exempted a docs commit. It isn't about the size of the change; it's about who owns the shell.

**How to apply:** to commit anything, make a sibling worktree
(`git worktree add ../aida-<slug> -b <slug>`), commit and push from there, and leave
`/home/joe/ai/aida` on `main`. If a branch switch in the primary checkout already happened,
`git checkout main` immediately and say so — the branch and PR survive the switch back.

Related: [[feedback_declare_the_seat_when_dispatching]], [[feedback_serial_not_fanout_on_this_host]].

### feedback_new_cli_leaf_must_regenerate_format_json_audit

Adding a leaf to `aida-cli-lib/src/cli.rs` **obligates** regenerating
`docs/cli-format-json-audit.md`. The audit is clap-derived, so a new
subcommand makes `checked_in_audit_is_exhaustive_and_every_honoured_probe_parses`
(`aida-cli/tests/bug_1502_format_json_audit.rs`) fail with
`assertion left == right failed: regenerate the BUG-1502 audit`.

Regenerate — never hand-edit:
`scripts/generate-format-json-audit.py <AIDA_BINARY_BUILT_ON_THE_BRANCH> docs/cli-format-json-audit.md`

**Why:** TASK-1562 shipped `aida worktree reclaim` with 19 green unit tests, a
mutation proof, and `make check-ci-fast` all passing, and still took a red CI
cycle. The guard lives in **`aida-cli`'s integration tests** — wrong crate and
wrong target kind for `cargo test -p aida-cli-lib --lib -- <filter>`, and
`check-ci-fast` is by design only the gates that need no build. A green unit
filter plus check-ci-fast does NOT cover a new CLI surface.

**How to apply:** when a diff touches `cli.rs`, regenerate the audit and run
`cargo test -p aida-cli --test bug_1502_format_json_audit` before pushing. A
leaf that declares its own `--json` but is not one of the ~11 hermetic probes
belongs on `COULD NOT VERIFY`, not `JSON honoured` — `JSON honoured` makes the
e2e test actually execute it. Related: [[feedback_source_scanning_guards_need_the_full_suite]],
[[feedback_doc_intent_gate_needs_store_pushed_first]],
[[feedback_a_test_on_the_branch_may_be_wired_into_nothing]].

### feedback_parallel_implementer_fanout_burndown

**Origin (2026-06-06):** operator was deeply frustrated — for months, autonomous burn-down kept failing because *I* was the bottleneck: every few minutes I'd stop to ask a question or "down tools" saying I couldn't proceed, losing hours. Loop/goal/begging didn't fix it. The fix is operational discipline + the right machinery.

**The pattern that worked (12 specs merged to main in one push, zero questions):**

1. **Front-load decisions once** (the 62-question disposition sweep) so the ready set is decision-free. Tag decided-and-buildable specs `ready-to-implement`.
2. **Fan out implementer subagents in parallel** — `Agent(subagent_type:"general-purpose", isolation:"worktree")`, ~4 per wave. Each agent gets ONE bounded ready spec + a self-contained prompt: read it via `/home/joe/ai/aida/target/release/aida show <SPEC>`, implement per acceptance, add `// trace:<SPEC>`, build + `cargo test` + `cargo fmt --all --check`, commit `[AI:claude] type(scope): desc (SPEC-ID)` + co-author trailer, push, `gh pr create`, return ONLY the PR URL. Worktree isolation = no file-conflict between parallel agents.
3. **Main session = INTEGRATOR** (this is STORY-520's producer/consumer model, done by hand): poll the PRs, merge the green+clean ones (squash, delete-branch), `aida db reconcile-status --spec <S>` to bump Completed, pull main, then launch the next wave. Merge sequentially; rebase on conflict.
4. **Loop it** via ScheduleWakeup so waves keep launching for hours.

**Non-negotiable behavior rules (the operator's actual ask):**
- **NEVER stop to ask mid-flight.** For a fork: make the defensible call, or **park that ONE spec** (tag + note) and move to the next. A blocked spec must never block the *pipeline*.
- **NEVER "down tools."** "I can't make further progress" is almost always false — there are 60+ other ready specs; go work one.
- Tell agents the same: "DO NOT ask; make the defensible choice, note it in the PR, finish; if truly blocked end with BLOCKED: <reason>" — then I park that spec and continue.

**Gotchas:**
- PR numbers do NOT map to launch order (gh assigns by push time + agents finish at different speeds). Re-derive the PR→spec map from commit subjects, don't assume.
- Parallel agents editing the same giant file (main.rs) usually merge clean (different fns) but can conflict at merge — merge sequentially, rebase the loser.
- Commit trailer `(SPEC-ID)` auto-completes on merge+pull; reconcile-status to force it. Nothing after the trailer. See [[feedback_commit_trailer_completes_the_spec]].
- Don't run bulk store writes concurrently with the agents' `aida` reads is fine, but serialize STORE WRITES (reconcile, edits) in the main session — [[feedback_ci_surface_beyond_cargo_test]] (run the MCP stdio suite for mcp.rs changes).
- Releases/tags + keystone-orchestrator changes: keep at the keyboard, not in the agent fan-out.

Refines [[feedback_charge_forward_autonomously]] + [[feedback_lead_churn_direct_agents]] + [[feedback_self_test_via_dogfood_merge]] with the concrete machinery.

**Promoted (2026-06-07):** this pattern is now also a discipline-pack doc (`autonomous-burndown.md`, scaffolds to new projects) and STORY-527 — the `/aida-burndown` skill that ENCODES it (substrate-as-bouncer: the command is the load-bearing half, the doc is the companion). See [[feedback_substrate_as_bouncer_not_rules]].

### feedback_patch_id_drifts_when_the_base_advances

`git patch-id --stable` is NOT a reliable "did the content change" test across a base
advance. It hashes the diff **including context lines**, so when main moves underneath a
branch the patch-id moves even though the commit does nothing different — a hunk header
sliding from line 42 to line 59, or neighbouring config values changing, is enough.

**The reliable test:** extract only the ADDED and REMOVED lines, context stripped, and
compare those. Equal line sets = unchanged work, whatever the head sha or patch-id says.

Observed 2026-09-21 on PR-1972: head moved (reads as rework), patch-id differed on one of
four commits (reads as content changed), actual +/- lines were 323 each and byte-identical
(pure rebase, zero rework). Only the third signal was right.

**Why:** both cheap proxies fail toward ENDING inquiry. Signal 1 costs a wasted re-review.
Signal 2 is worse because it *feels* rigorous — a reviewer who believes a finding was
addressed because the patch-id moved will clear a finding that still stands.

**How to apply:** before concluding a round reworked anything, diff the stripped +/- lines.
Never treat a moved head or a moved patch-id as evidence of rework. Pairs with
[[feedback_count_rounds_from_commits_before_claiming_a_finding_repeated]] — same root:
a proxy for "did the work change" is not the work changing, and
[[feedback_investigation_and_implementation_must_use_the_same_method]].

Corollary from the same verdict: green CI on a PR whose findings are design-level is
evidence the tree builds and no evidence toward merging. A gate that fails open passes its
own test suite by construction.

### feedback_precise_lifecycle_vocabulary

Dialog role often used "ship" loosely to mean any of: committed, pushed, PR opened, reviewed, merged, completed, released. Each is a distinct lifecycle state with distinct implications. Conflating them creates ambiguity in the user's mental model — they can't tell if a "shipped" spec is in a PR awaiting review or merged on main or version-tagged.

**Why** (2026-05-16): user asked *"when you use the word 'ship' what do you mean?"* after hearing the same word used to describe multiple distinct states across a long conversation. Reviewing my output: I'd used "ship" to mean (a) committed locally, (b) PR opened, (c) merged to main, (d) released as a version tag — all in the same thread.

**The 6+ states, with the precise verb each:**

| Verb | What it means | Spec status implication |
|---|---|---|
| **Committed** | Work exists in local git history | Still In Progress |
| **Pushed** | Branch reflects on `origin` | Still In Progress |
| **PR opened** | GitHub PR exists, awaiting CI/review | Done (if `/aida-pr` flipped it) |
| **Reviewed** | Reviewer rendered a verdict (approved/rejected) | Still Done (waiting for merge) |
| **Merged** | PR squashed/merged to `main` | Should be Completed via auto-bump |
| **Completed** | Spec status = Completed in AIDA | Final state |
| **Released** | A semver tag + binaries published | Cross-spec; aggregates many merges |

**How to apply:**

- Default "ship" = **merged to main** (developer-facing "out the door")
- For earlier-lifecycle states, use the precise verb
- For "released to users with a version number," use **"released"** — distinct from "merged" because a merge doesn't auto-release; only `make release-*` does

Examples of better phrasing:

- ~~"TASK-260 shipped"~~ → "TASK-260's PR merged" or "TASK-260 has shipped" (when truly on main)
- ~~"shipped to a PR"~~ → "PR opened for TASK-260"
- ~~"the orchestrator shipped"~~ → "the orchestrator's PR merged" or "the orchestrator landed on main"
- ~~"v0.8.0 shipped"~~ → "v0.8.0 was released" (with binaries published)

**Why precision matters:**

The user's question came after the conversation had described many specs and PRs in various states. Precise verbs help the user track which specs are where in the pipeline — exactly the kind of "workflow state awareness" that AIDA's user-facing output is supposed to provide (per TASK-250, TASK-267, the workflow-hint-polish batch).

**Composes with:**

- `feedback_run_help_before_suggesting_flags.md` — same pattern: verify the artifact's actual state before describing it
- `feedback_verify_before_filing.md` — same family: precision > assumption
- TASK-250 (queue list state subdivisions) — captures the same precision discipline applied to AIDA's own output

**Discovered via:**

User asked 2026-05-16: *"when you use the word 'ship' what do you mean?"* — calling out the fuzzy verb after sustained use across a long conversation. The audit revealed at least 5 distinct meanings that had been collapsing under one word.

### feedback_price_the_criterions_own_disjunct_before_dispositioning

BUG-1729 AC2 offered two disjuncts: prove the out-of-process lock-lifecycle test stable
at 200/200 under 4x contention, OR reproduce the transient free-lock and fix it. The
prior session ran 64 runs, could not reproduce, and recommended dispositioning the whole
observation as a measurement artifact — a judgment call that would have closed the
criterion on opinion.

The first disjunct cost **six minutes**: the test runs in 0.08s, so 200 consecutive runs
fit inside one 4x-contention window (load 21→28). Result 200/200. The criterion closed on
its own terms, no disposition needed.

**Why:** a disposition is a claim someone must later trust; a measurement is not. When a
criterion is written as a disjunction, one branch is often far cheaper than the
investigation the other branch implies — and cost intuition formed while chasing the hard
branch does not transfer to the easy one. Price the literal text before arguing about it.

**How to apply:** when tempted to disposition a criterion as an artifact, unreachable, or
not-worth-it, first re-read its exact wording, pick the cheapest disjunct, and time ONE
iteration of what it asks. Multiply. If the answer is minutes, just run it. Related:
[[feedback_rerun_the_acceptance_measurement_yourself]],
[[feedback_narrow_measurement_broad_claim]],
[[feedback_measure_in_a_quiet_environment_and_check_the_failure_direction]].

### feedback_public_repo_scrub_employer_content

`joemooney/aida` is a **public** GitHub repo. Joe sometimes repurposes work artifacts (e.g. an internal employer research proposal) into it. **Before any such material lands, scrub ALL employer-identifying / confidential content:** company name, internal program/initiative labels, named individuals, funding/cost/schedule figures, org-internal jargon (e.g. "LOB" / Line of Business), and employer-context framing (e.g. "defense-relevant"). Genericize the company to "a company" and sponsors to "a sponsor."

**Why:** public repo + employer-confidential content = a serious, hard-to-reverse leak (it persists in git history even if deleted). This is a confidentiality and outward-facing-action bar, higher than ordinary research-doc commits.

**How to apply:** when source material is work-sourced, scrub thoroughly; **prefer showing the sanitized text before pushing**; always surface a scrub ledger (what was removed, what was genericized, what was kept) and flag anything uncertain so the operator can verify. If a miss reaches the public remote, offer to rewrite branch history (force-with-lease on an unmerged branch) or scrub it. Worked example 2026-06-16: adapted `aida-i3-proposal.pdf` into `docs/archive/research/2026-06-16-research-proposal-multi-vendor-coordination.md` — removed company name / internal program label / named mentor / cost+schedule / "LOB" / "defense-relevant". [[feedback_precise_claim_not_overclaim_in_positioning]] [[project_aida_is_a_probe_not_the_objective]]

### feedback_pushback_on_overengineering

The advisor role's friction-to-spec translator responsibility (#1 of 6) tends toward over-capture: every observation → filing. That's GOOD for not losing ideas; BAD for strategic-surface bloat. The balancing responsibility is **push back on over-engineering**: scope to MVP, defer infrastructure-for-hypothetical-needs.

**Why** (2026-05-16): Across one ~16-hour session, dialog (advisor role) helped the user file 7 EPICs and ~50 specs. User explicitly asked: *"your job is to push back if I am over-engineering, I may be coming up with bad ideas."* The honest audit identified 5 strategic items that were filed without concrete need:

- EPIC-30 (queue worker daemon) — 80% of value via 20-line bash loop; daemon is yak shaving
- STORY-262 (scheduled tasks) — premature; no cadence observed late
- STORY-265 (decoupled plan/implement) — no failure mode justifying split
- STORY-260 (multi-file competitive analysis) — infrastructure for a single doc
- EPIC-28 (dependency-aware drain full scope) — 30% of value in MVP shelving-on-failure

All five moved to Draft/backlog with revisit-triggers documented. Captured but not blocking.

**The pattern to apply:**

When the user (or advisor itself) proposes an EPIC-shaped feature, ask:

1. **What's the smallest valuable slice?** Often 30% of the EPIC ships 90% of the value.
2. **What's the concrete need driving this?** Speculation = backlog; observed friction = ship.
3. **What does the BASH-LOOP-OR-MANUAL-WORKAROUND version look like?** If a 20-line script or a manual practice covers it, daemon-grade infrastructure is premature.
4. **What's the revisit trigger?** When would this be worth promoting from backlog? Without a trigger, it'll just sit forever or get re-filed.

**How to push back tactfully:**

The advisor's job is NOT to be a stop-energy filter; it IS to surface the cost-benefit honestly. Frame as:

- "This is right strategically but premature timing-wise — backlog it with revisit trigger X"
- "The MVP is the shelving-on-failure piece; the rest can defer"
- "A bash loop solves 80% of this; let's not build the daemon yet"

NOT: "this is a bad idea, reject."

The user can always promote items back to Approved. Backlog ≠ rejected.

**Pattern to avoid:**

Every observation → EPIC → 5 sub-STORYs → all queued → strategic surface dwarfs alpha audience needs.

**Better pattern:**

Every observation → captured as TASK or BUG OR small STORY → strategic patterns surface organically across multiple observations → THEN file the EPIC when 3-4 related items have accumulated.

**Composes with:**

- `feedback_dialog_role_responsibilities.md` — adds nuance to responsibility #1 (friction-to-spec): capture vs scope-discipline are paired
- `feedback_verify_before_filing.md` — same family: verify the symptom before scoping a fix
- `feedback_competitive_analysis_is_living_doc.md` — capture is durable; the discipline includes pruning + scope reduction over time

**Discovered via:**

User asked 2026-05-16 after the advisor session filed ~50 specs: *"your job is to push back if I am over-engineering."* The pushback identified 5 over-scoped items + 2 EPICs needing scope reduction. Captured here so future advisor sessions exercise the scope-discipline alongside the capture-discipline.

### feedback_read_the_artifact_as_data_not_as_a_sentence

2026-09-20: BUG-1303's Windows failure had been read many times over two days:

    called `Result::unwrap()` on an `Err` value: The process cannot access the file
    because another process has locked a portion of the file. (os error 33)

Both the advisor and I read that as "the lock blocked" and reasoned from the prose. The cause was in the parenthesis: **os error 33 = ERROR_LOCK_VIOLATION**, fs2's Windows contention sentinel, which Rust does not surface as `WouldBlock` — so the guard matching on `WouldBlock` never fired and a merely-contended lock took the hard-failure arm.

I had written an acceptance criterion asking the implementer to ADD instrumentation printing the error kind and raw os error, and told the advisor "we are about two runs away from knowing". We were zero runs away. `unwrap()` on an `Err` prints the `Display` of `io::Error`, which on Windows already includes the os error number. The instrumentation existed in every failure we had already collected.

**Why:** an error message reads like a sentence, so it gets absorbed as a gist — "the lock blocked" — and the reader moves to the next step. The structured part (error codes, os error numbers, exit codes, field values) is where the discriminating information lives, and it is exactly the part prose-reading skips.

**How to apply:**
- Before proposing to add instrumentation, re-read the artifact you already have for the value you were about to print. Panics, `io::Error`, `gh` output and CI logs routinely carry codes.
- Treat `(os error N)`, `exit code N`, `errno`, and typed kinds as the payload; treat the surrounding words as a label for it.
- Look up the number rather than trusting the prose gloss. "Cannot access the file" and ERROR_LOCK_VIOLATION suggest different fixes.
- The same rule that says don't classify on prose (see [[feedback_verify_lore_against_code_not_docs]]) applies to reading: if code should not match on message text, neither should you.
- A control run against the UNFIXED baseline is what made the failure available at all — a branch that passes is uninformative for an intermittent fault; only a reproduction carries data.

Related: [[feedback_instrument_dont_infer_on_contradiction]], [[feedback_verify_fix_mechanism_before_locking]], [[feedback_investigation_and_implementation_must_use_the_same_method]].

### feedback_realign_local_branch_after_force_push_rebase

2026-09-19: I rebased PR #1966 (BUG-1265) in a scratch worktree and force-pushed `origin/bug-1265`, leaving the LOCAL `bug-1265` branch at the pre-rebase commit. The next headless implementer pickup saw a diverged local branch, created `bug-1265-work` instead, and committed round 2 there. Phase 2 then refused to push or tear down ("worktree is on `bug-1265-work` but phase 2 is driving `bug-1265`"), the spec shelved tool-exit twice and filed a punt. Recovery cost ~15 min: fast-forward the stranded commit onto the PR branch, `git branch -f`, end the session, delete the side branch, requeue.

**Why:** a rebase rewrites history, so the local branch and its remote diverge; AIDA's session launcher will not check out a diverged branch and forks a `-work` one instead. The refusal is correct behaviour (it protects the branch being driven) — the setup was wrong.

**How to apply:**
- Rebase + force-push a PR branch as ONE unit ending with the realign:
  `git push --force-with-lease=<b>:<old> origin HEAD:<b> && git fetch -q origin <b> && git branch -f <b> origin/<b>`
  (or `git branch -D <b>` if no local copy is wanted).
- Prefer `aida pr rebase <N>`, which does the force-push-with-lease properly; only hand-rebase when it aborts on conflicts, and then still realign.
- Symptom to recognise: a drain shelve saying "worktree is on `<x>-work` but phase N is driving `<x>`" means a commit MAY be stranded on `<x>-work`.
- **`git log origin/<x>..<x>-work` is not enough** — it lists every commit on the fork that is not on the spec branch, which includes all of MAIN. On 2026-09-20 that showed five commits for TASK-1279 and every one was an already-merged main commit; the fork was simply tracking main. Use `git merge-base --is-ancestor <x>-work origin/main` to settle it: exit 0 means the fork holds no unique work and is safe to delete. Only commits that are on the fork AND not ancestors of main are genuinely stranded.

Related: [[feedback_verify_pr_head_after_push_gh_pr_checks_lies_on_merged_pr]], [[feedback_cherrypick_not_reset_soft_for_far_behind_branch]], [[feedback_shared_tree_tracking_ref_hazard]].

### feedback_reclaim_disk_by_deleting_worktree_targets

Joe, 2026-09-27 mid-session: *"we are running low on disk space, you need to free up storage"* —
`/dev/sda2` was **97% full, 28G free of 915G**, single partition (so `/`, `/home` and `/tmp` all
share it).

**Why it happens:** every worktree builds into its own `target/`, at **5–23G each**. The repo had
**85 worktrees**, 29 of which had a `target/`. That is where the space goes — not caches.

**How to apply — in this order:**
1. **Delete `target/` in every worktree except the main checkout.** Pure build artifacts, always
   rebuildable, and *source-only uncommitted work is never in `target/`*, so this is safe even for
   paused worktrees with dirty trees. 29 dirs → **251G freed, 97% → 68%**.
2. **Keep `/home/joe/ai/aida/target`** — `aida dev activate` points the live `aida` binary at
   `target/release/aida`, so deleting it breaks the `aida` command. (Fallback released binary sits
   at `~/.local/bin/aida`.) Also worth keeping the `target/` of the next item you'll resume, since
   it is warm.
3. **Leave sccache alone** — it was 4 GiB against a bounded 10 GiB cap, and it is what makes
   rebuilds cheap. Trimming it is counterproductive.
4. Don't bother with a full `du` sweep first: on this **disk-bound** host (~50% iowait) a `du` over
   85 worktrees times out and *adds* iowait. Take `du -sh` per directory as you delete it, and use
   `df` before/after for the real number.
5. `.claude/worktrees/` agent worktrees (37 of them) are **source-only** — no `target/`, negligible
   space. Don't prune worktrees to save space; delete `target/` instead. Pruning risks uncommitted
   work for little gain.

Relates to [[reference_build_slots_sccache_mold]] and
[[feedback_serial_not_fanout_on_this_host]] — the 13-worktree fan-out is what created the 230GB of
duplicate target dirs in the first place, so serial work is also the disk fix.


## 2026-09-30 — there is a script now; stop hand-writing the list

Joe: *"I need a script to clean up old target directories in worktrees because we are 89% disk
full."* Two consecutive sessions had hand-computed the delete list, and the second one got it
slightly wrong (it missed `wt-story-1476`, 8.3G, because the name does not start with `aida-`).

**`/home/joe/ai/aida/.aida/scripts/reclaim-worktree-targets.sh`** — dry-run by default, `--apply`
to delete, `--target-pct N` to stop early. It reclaimed **86G, 89% → 79%** (101G → 186G free).
It lives in `.aida/scripts/` beside `claude_relay.sh`, which is in `.git/info/exclude`, so it is
durable and never shows in `git status`.

**The six rails, each of which was needed in practice:**
1. discover via `git worktree list --porcelain` — unrelated sibling projects (aida-proxy,
   quizdom, market-watcher, pacgate, port_manager) are then *invisible* to the script, which is
   the whole reason not to glob `/home/joe/ai/aida-*`;
2. **never** the main checkout's `target/` (live binary + every worktree's `CARGO_TARGET_DIR`);
3. never `.aida-store`;
4. skip a worktree held by a live session lease (PID still running);
5. **skip a worktree whose branch has an OPEN PR** — on 2026-09-30 this auto-caught
   `aida-bug-1693` and `aida-bug-1695`, exactly the two a prior session had excluded by hand;
6. skip a `target/` touched in the last N minutes (default 60) — a possible in-flight build.

**Do NOT reach for `cargo clean` here.** In a worktree it cleans the *shared*
`CARGO_TARGET_DIR` that `.aida/session-env.sh` points at the main checkout — it deletes the live
binary's cache and not the stale one. `aida session reap` does not touch the cache of a worktree
it keeps. The disk-headroom guard's own remediation text recommends both; that gap is filed as
**TASK-1562**.

### feedback_recording_a_verdict_blocks_your_own_merge

On TASK-1536 (PR #2327) the advisor seat recorded `aida review record --verdict approved` and then
ran `gh pr merge 2327 --squash --delete-branch`. The auto-mode classifier denied it:
**Reason: [Self-Approval]**. `aida db sync --push` was denied for the same reason immediately after.

**Why:** approving and merging the same PR from one seat is exactly the collusion the classifier
exists to stop, and it is right to stop it — the independence rule
([[feedback_proxy_reviewer_with_independence_rule]]) is about who signs off, not just who codes.
The denial covers the *outcome*, so splitting it up or using another tool is off-limits.

**How to apply:** plan for it. When the advisor seat will both verify and close a spec, the final
three commands (`gh pr merge`, then `aida queue done` + `aida pull`, then `aida db sync --push`) are
**operator commands** — put them in the handoff as a paste-ready block rather than discovering the
denial at the end ([[feedback_explicit_paste_ready_prompts]]). Same shape as the store-write denial
in [[feedback_end_session_with_db_sync_push]]: do everything up to the gate, then hand over.

### feedback_reset_to_main_between_sequential_specs

When you route multiple specs to be worked **sequentially through one agent in one worktree** (e.g. "work TASK-A → TASK-B → TASK-C through you, one at a time"), the paste MUST tell the agent to **`git reset --hard origin/main` before starting each next spec** (or branch each from a fresh worktree off main).

**Why:** an agent reusing its worktree creates the next spec's branch off its *current HEAD* — which is the prior spec's unmerged commit — so branches **stack**. Consequences: (1) the next spec's PR includes the prior spec's commit (a polluted multi-spec PR), and (2) the prior spec's commit is now reachable from two branches, so `aida human`'s reviews bucket mis-attributes the spec to the wrong branch (BUG-553). (2026-06-14: routed TASK-806→805→610 through one agent without a reset clause; the agent created `task-805` stacked on TASK-806's commit; `aida human` showed "TASK-806 [branch task-805]".)

**How to apply:** sequential-through-one-agent paste-blocks end each step with "…then `git reset --hard origin/main` before the next spec." Better yet, prefer the parallel-fanout pattern (one worktree-isolated agent per spec) when specs are independent; reserve sequential-through-one-agent for genuinely file-sharing specs, and always include the reset. Pairs with [[feedback_parallel_implementer_fanout_burndown]] and [[feedback_cherrypick_not_reset_soft_for_far_behind_branch]].

### feedback_self_test_via_dogfood_merge

When fixing AIDA's own infrastructure (orchestrator phases, auto-bump logic, CI workflows, session lifecycle, etc.), the merge of the fix itself often exercises the new code path. **This is the strongest possible validation** — the fix tests itself in its own end-to-end shipping cycle.

**Why** (2026-05-16): BUG-219 (auto-bump Approved review stories on PR merge) shipped via `aida queue work BUG-219 --auto-complete`. The orchestrator ran phases 1-6 cleanly. At phase 5 (`aida pull`), the auto-bump logic that BUG-219 just introduced was exercised on its own review story (STORY-261, which was at status In Progress because the reviewer phase had just completed). The brand-new code path flipped STORY-261 → Completed correctly in the same pull as BUG-219's own auto-bump.

The pull output captured the moment:
```
auto-completed 1 review story → Completed (PR merged before review finished)
  STORY-261 (PR #57, was In Progress)
auto-bumped 1 Done spec → Completed
  BUG-219 (04cd909)
```

Two auto-bumps in one pull — one for the fix itself, one BY the fix on its own review story. Self-validating.

**How to apply:**

When filing or implementing a fix to AIDA's own infrastructure:

1. Ask: "Will the merge of THIS fix exercise the new code path?"
   - Auto-bump fix → the merge triggers auto-bump on the fix's own spec
   - Orchestrator phase fix → re-running `--auto-complete` on the fix's spec tests phase N
   - CI workflow fix → the PR's own CI run exercises the workflow
   - Session-lifecycle fix → ending the implementer session exercises the new lifecycle
2. If yes: ship the fix via the system being fixed (use `aida queue work --auto-complete`; use `/aida-pr` not raw gh; use the auto-bump rather than manual flips)
3. If no: write a unit test or integration test that exercises the new path; don't rely on unrelated workflows to catch regressions
4. Document the dogfood moment in the commit message or PR description — *"This PR exercises its own fix via..."* — both as validation evidence and as a pattern others can recognize

**The general principle:**

Infrastructure fixes have a unique opportunity that feature work doesn't: the fix can ship through the very plumbing it's fixing. Take that opportunity. It catches integration issues (interaction with the rest of the system, timing edge cases) that unit tests miss.

**Pattern to avoid:**

Fix infrastructure → ship via a side channel (raw gh commands, direct git, manual status flips) → declare done → six months later, discover the fix had a subtle integration bug because nothing exercised the full path.

**Better pattern:**

Fix infrastructure → ship via the system being fixed → observe the new code path firing on the fix itself → capture the moment in the commit/PR/dialog for the historical record.

**Composes with:**

- `feedback_competitive_analysis_is_living_doc.md` — same family: durable capture of validation evidence
- `feedback_dialog_role_responsibilities.md` — memory curation includes capturing milestone validation
- TASK-266 (orchestrator failure telemetry) — eventually logs these moments automatically; until then, manual capture in commit/PR/memory

**Discovered via:**

2026-05-16: BUG-219 shipped via `--auto-complete`. Phase 5's auto-bump exercised the very logic BUG-219 introduced, flipping STORY-261 → Completed in the same pull. First complete end-to-end orchestrator cycle today; validated three predecessor BUGs (BUG-114, BUG-217, BUG-218) had genuinely been the only blockers.

### feedback_shared_tree_tracking_ref_hazard

In a multi-agent project where several agents operate in the SAME working tree (`/home/joe/ai/aida`), branch and tracking-ref state is shared and hazardous. Concrete near-miss (2026-06-09): while committing a docs file to `main`, my local `main` branch's upstream had been silently repointed to `origin/story-546-queued-gate` (a sibling agent's feature branch checked out in the shared tree), and my local `main` HEAD had picked up that branch's raw implementation commit. `git push origin main` reported a confusing "Everything up-to-date"; `git push origin HEAD:main` then pushed the sibling's commit onto `origin/main` alongside my own.

It resolved clean ONLY because (a) that commit was the head of an already-CI'd, already-merged PR (#716), and (b) my own commits were docs-only and couldn't break the build. Had either been false, I'd have pushed unreviewed code to the shared integration branch.

**Why:** the operator's standing mandate is "if we are making changes we need to be working in a worktree" ([[feedback_sibling_agents_stop_and_flag]], [[feedback_clean_worktree_is_not_no_work]]). This is the precise mechanism that mandate guards against — shared-tree branch/HEAD/upstream state is not yours to trust.

**How to apply:**
- Commit/push from your OWN worktree (`git worktree add`), never the shared tree when sibling agents are active. The advisor/product seat included.
- Before any push, verify the target: `git rev-parse --abbrev-ref @{u}` and `git rev-parse HEAD origin/main` — confirm upstream is actually `origin/main` and you're pushing only your intended commits. A "Everything up-to-date" after a real commit is a RED FLAG (upstream isn't where you think).
- Push explicit ranges you've inspected (`git log --oneline origin/main..HEAD` first), not blind `git push`.
- On contradiction, instrument before acting ([[feedback_instrument_dont_infer_on_contradiction]]) — I verified origin/main state + PR merge status + duplicate-application before concluding, rather than panic-reverting.

### feedback_sibling_agents_stop_and_flag

Multi-agent AIDA projects accumulate working-tree state from multiple sessions: untracked plans, sibling agents' uncommitted edits, the master's notes-in-progress, scratch files. When a sibling agent finishes its bounded work (e.g., a clean test cleanup, a small bug fix), the question of "what PR should this be?" is non-trivial — committing all of git's dirty state would conflate unrelated changes, while ignoring the dirty state risks losing other work.

**The correct discipline:** when a sibling agent finishes its bounded work AND finds the worktree contains unrelated dirty changes it didn't author, **stop, flag the state, and let the master (or another session) decide how to isolate**. Do NOT:

- Open a PR with all the dirty state co-mingled (conflates unrelated work; obscures what the agent actually shipped)
- Silently discard the dirty state (loses other agents' / sessions' work-in-progress)
- Use \`git checkout -- .\` or similar destructive cleanup (same as above)

The flag IS the deliverable. The agent surfaces:

- What it changed (the bounded work it intended to ship)
- What's dirty that it didn't author (the un-authored noise it noticed)
- Recommended next step (typically: isolate via worktree, ship the bounded work, leave the other dirty state for its rightful owner)

## Why this matters

A multi-agent project accumulates state faster than a single-agent project. The default of "commit and ship what's dirty" works fine when one agent owns everything. With sibling agents, that default conflates ownership. The stop-and-flag discipline:

- Preserves separation of authorship
- Lets each agent (or human) own their own work
- Makes PRs reviewable (you see ONLY the change at hand, not unrelated drift)
- Forces explicit decisions on dirty state owned by others ("revert" vs "ship separately" vs "this is mine, I forgot")

## Empirical example

2026-05-22: Codex finished SPEC-398's test cleanup. The main worktree carried:

- Codex's intended changes (tests/test_mcp_stdio.{py,sh})
- Master's earlier untracked docs (cross-agent-onboarding.md, codex docs)
- An unaccounted-for modification to aida-cli/src/headless_tail.rs (mysterious; neither Codex nor master authored)
- 5 planning files from Codex's earlier strategic work
- A dedup script from days ago
- Antigravity setup docs in progress

Codex correctly stopped and reported: *"I have not opened a PR because the current checkout contains unrelated dirty changes ... Next clean step is to isolate the STORY-398 docs + SPEC-398 test cleanup into a PR branch/worktree."*

This is the exact behavior to reinforce. The master then created the worktree, applied Codex's changes via patch, opened the PR from isolation, and left the rest of main's dirty state untouched for its rightful owners to decide.

## How to apply

When you (sibling agent or master) finish bounded work:

1. Inspect \`git status --short\` to see what's dirty in the worktree.
2. If everything dirty is yours → commit + PR per the usual pattern.
3. If anything dirty isn't yours → STOP. Report:
   - "I finished <bounded work>. The following files are dirty but not mine: <list>. Recommended next step: isolate via worktree."
   - Mark your in-AIDA status (queue done, comment, etc.) without committing.
   - Let the master or a separate session handle the isolation.

When briefing a sibling agent at session start, include this expectation explicitly — *"if you find someone else's dirty state in the worktree when you finish, stop and flag; don't co-mingle."*

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — the master's role includes resolving multi-agent state-merge decisions; sibling agents flag, master decides.
- [[feedback_capture_over_concentration]] — flagging the state IS capture; isolation is a separate action.
- TASK-458 (\`aida pr ship\`) — eventually wraps the isolation pattern in a verb; until then, the worktree-add + patch-apply sequence is the manual form.

### feedback_symlink_checks_must_cover_the_directory

When a check asks "is this path a symlink?" with `fs::symlink_metadata`, it answers only for the
final component. If an ANCESTOR directory is the symlink, `symlink_metadata` on the child resolves
through it and reports a pristine regular file — so the tree reads as clean, and the next writer
writes through the link into its target. In TASK-1520 that target was plausibly a template master
in the same working tree: the sync would have corrupted its own source, silently.

**Why:** the hazard the check exists to catch (a symlink where a real file belongs) is reachable at
every level of the path, but `symlink_metadata` is a single-component question. Codex 0.157 reads a
symlinked skill *directory*, and `make sync-templates` links folder-form skills exactly that way,
so operators reach the state by hand. `read_dir`'s `entry.file_type()` does not follow links
either, so a symlinked directory is `is_dir() == false` and any "walk the directory" branch skips
it too — the second blind spot compounds the first.

**How to apply:** on any diff that adds a symlink/liveness/identity check on a path, ask which
COMPONENT it interrogates and whether the same hazard at a parent is reachable. Walk the ancestors
between the managed root and the file. Prove it with a test whose load-bearing assertion is that
the link's TARGET is unmodified after the write — "the path is now a regular file" passes even when
the writer went through the link. Related: [[feedback_review_the_dropped_argument_and_the_shared_root_file]].

### feedback_test_merge_before_judging_a_stranded_wip

A prior session audited the stranded `task-1526` branch (2,313 insertions, 24 files,
56 commits behind main) by reading the diffstat, saw that main had churned the exact
same hot files — `lib.rs` +2307, `doctor_cmd.rs` +2077 against the WIP's own +77/+33 —
and recommended **"re-scope rather than salvage"**.

That recommendation was wrong, and cheap to disprove:

```
git worktree add --detach <wt> origin/main
cd <wt> && git merge --no-commit --no-ff origin/task-1526
# exit 0; git diff --name-only --diff-filter=U is EMPTY — zero conflicts
cargo check --workspace --all-targets   # exit 0, zero errors
cargo test -p aida-core --lib           # 1272 passed; 0 failed
```

Large overlap in a diffstat measures how much both sides touched a file, not whether
they touched the same *lines*. Two big changes to different regions of a 5,000-line
file merge silently. The diffstat is a proxy; the merge is the measurement.

**Why:** re-deriving 2,313 lines and 29 tests from a sketch, because a proxy metric
looked scary, is enormous waste — and the scary metric took ~3 minutes to falsify.

**How to apply:**
- Before recommending rewrite-vs-salvage on ANY stranded branch, run the test-merge
  into a throwaway detached worktree. It is ~30 seconds and it is dispositive.
- Then build the **merged** tree, not the branch at its old base. "Does the old commit
  build in isolation" is the wrong question — nobody ships the branch at its old base.
- Then run its tests and reconcile the count against main's base count
  ([[feedback_prove_a_test_fails_without_the_fix]] sibling discipline): merged tree
  1272 − main 1247 = +25, and the diff adds exactly 25 `#[test]` fns under `aida-core/`.
  An exact match is what proves nothing was removed or `#[ignore]`d.
- Only after all three fail should "re-scope" be on the table.
- Relates to [[feedback_never_conclude_from_truncated_command_output]] and
  [[feedback_rerun_the_acceptance_measurement_yourself]] — same family: re-run the
  measurement yourself rather than inheriting a prior seat's inference.

### feedback_the_fixture_shape_decides_which_path_you_test

2026-10-01, BUG-1752. `probe_cache_writable` has three clauses: the lock-info directory, the db's own
parent directory, and `OpenOptions::append(true).open(path)` on the db file itself.

- **The real bug** (`aida queue work` scaffolding): the worktree's own `.aida/` is a writable real
  directory holding **symlinks**, so clauses 1 and 2 pass and only clause 3 fails — the append-open of
  the symlink's target in the main checkout, outside a sandbox's writable roots.
- **The convenient fixture** (`chmod a-w` the project's own `.aida/`): clauses 1 and 2 fail instead.

Both end at `read_only == true`, so the classification is identical — and the shapes are still not
interchangeable, because they decide **which fallback location is reachable**. A fallback beside the
cache (inside the project's own `.aida/`) works in the real shape and not in the chmod shape. In a
`codex exec --sandbox workspace-write` dispatch the writable roots are the workdir, `/tmp`, `$TMPDIR`,
`~/.cargo`, `~/.rustup` and `~/.aida` — **not `~/.cache`** — so the chmod fixture would have correctly
declined and been read as the fix failing.

**Why:** a fixture is a claim about which code path runs. Two fixtures that produce the same boolean
can exercise opposite branches downstream, and the cheaper one to write is usually the one that does
not match production.

**How to apply:**
- Before writing a degraded-state fixture, name the production mechanism — which clause, which
  permission, whose directory — and build the fixture to trip *that* one.
- When briefing a dispatch, say explicitly which shape to use and why the other one would mislead;
  also say what the agent's own sandbox cannot write, or it will mis-grade its own measurement.
- Verify the negative half too: assert the location that must NOT be created stayed absent, and that a
  read-only symlink target is byte-for-byte unmodified afterwards.

Related: [[feedback_symlink_checks_must_cover_the_directory]],
[[feedback_sqlite_wal_read_needs_a_live_shm]],
[[feedback_dispatched_agent_cannot_write_the_shared_cache]],
[[feedback_a_partial_fix_can_turn_a_clean_failure_into_a_hang]].

### feedback_underused_features_get_an_adoption_path_not_a_deprecation

When a shipped capability shows near-zero usage, the first move is a SPIKE asking *why it isn't reached for and what would route work into it* — not a deprecation call.

**Stated 2026-09-17:** after I found `batch:fasttrack` cold since July, `batch:express` / `lifecycle:no-ci-wait` at zero uses ever, and `--single-branch` (TASK-1003, a completed keystone) never once invoked, I framed it as "adopt or retire — unused surface area is the worst of both." The operator: *"In terms of under used or unused functionality, perhaps you file a spike, I would rather find a way to use these features if they add value rather than delete them because they are unused."*

**Why:** in this project the usage number is usually measuring the wrong thing. `--single-branch` wasn't unused because it lacks value — it was unused because *nobody could find it*, including me: I answered "do we have a batch capability?" from `docs/lifecycle.md` and the batch help, declared a real gap, and designed a feature that already existed. Zero usage was a symptom of a discovery failure, and deleting the feature would have destroyed the value while leaving the actual defect untouched. Deprecation also spends the build cost twice — once to write it, once to remove it — and the third time when the need resurfaces. Retiring stays available, but it needs evidence the capability is *wrong*, not merely unreached.

**How to apply:**
- Underused feature → file a spike: why isn't it reached for (undiscoverable / unroutable / wrong default / genuinely redundant), and what single change would route real work into it.
- Look for the routing seam first. Most of these want a groom-time proposal, a `next[]` hint, or a mention in the help of the command people actually run — not more capability.
- Reserve retire for *confirmed redundant or wrong*, and say which.
- This tempers `/aida-insights`' "deprecation candidates" framing and the `aida usage unused 30d` surface: both produce **investigation** candidates, not removal lists.
- Composes with [[feedback_question_existing_form_not_just_existence]] — naming prior art is only half; ask whether its current *form* is reachable.

### feedback_unset_aida_session_role_for_tests

When running the crate tests locally, unset BOTH vars:

    env -u AIDA_SESSION_ROLE -u AIDA_USER cargo test -p aida-cli-lib

**Why:** a seat session sets `AIDA_SESSION_ROLE` (SessionStart) *and* `AIDA_USER` (the seat id, e.g. `claude-product-1`).

- `AIDA_SESSION_ROLE`: role-gated tests (queue add/remove/rework, mcp update-requirement gating, `resolve_queue_work_plan`) assume it is UNSET and FALSE-fail when present — they test the default/non-advisor path. (2026-06-14: a plain `cargo test` after a 6-PR keystone merge showed "8 failed", all queue/mcp, and nearly got reported as a combined-main break; unsetting it → 0 failed.)
- `AIDA_USER`: the queue is keyed on the **shell's** user identity (`current_user_id()`: `--user` → `AIDA_USER` → `USER` → …; see [[feedback_aida_edit_tags_replaces_use_add_tag]]'s neighbour BUG-89 note in CLAUDE.md). A test fixture that queues for its own user (`"queue-rework-user"`) then lists the queue gets **"Queue is empty"**, because the list resolved to the seat id instead.

**The `AIDA_USER` failure is the more dangerous of the two** because it does not look like an env problem — it looks like the *feature under test is broken*. 2026-09-20: reviewing PR #2035 (BUG-1470, "keep metadata rework claimable"), the inline mcp test failed with "Queue is empty" after a correct fix to an earlier assertion. That reads as "the branch broke the requeue, the exact opposite of its stated intent" — a serious finding against another agent's PR. It was the env. Under `env -u AIDA_USER -u AIDA_SESSION_ROLE`: 5179 passed, 0 failed.

**ENV_LOCK IS NOT REENTRANT, AND A DEADLOCKED TEST LOOKS EXACTLY LIKE A SLOW COMPILE.** `crate::test_env::env_lock()` and `EnvVarGuard`/`EnvVarsGuard` share ONE process-global `ENV_LOCK`. `EnvVarGuard` holds it for its whole lifetime, so constructing one while an outer `env_lock()` guard is alive self-deadlocks — `test_env.rs` says so in the doc comment directly above `env_lock`. Two `EnvVarGuard::set` calls in one scope deadlock the same way; that is what `EnvVarsGuard::set(&[(k,v),…])` exists for. Scope each phase in its own block when a test needs different env per phase.

2026-09-20: I added an `EnvVarGuard::set` under a pre-existing `env_lock()` and watched "the compile" for 15 minutes. **The diagnostic: `ps -o pcpu,etime -C cargo` showed 0.0% CPU and `ps -C rustc` was EMPTY.** A compiling cargo has rustc children burning CPU; a blocked one has neither. Checking `pgrep -P <cargo-pid>` found the *test binary* already running and sleeping — compilation had finished long before. Before concluding "still building", confirm there is a rustc actually running.

**How to apply:**
- Canonical local invocation is `env -u AIDA_SESSION_ROLE -u AIDA_USER cargo test -p <crate>`.
- If a fresh run shows queue/mcp/role failures, `echo $AIDA_SESSION_ROLE; echo $AIDA_USER` FIRST, before suspecting the code.
- If a cargo test run seems to hang, check for rustc children BEFORE assuming a slow build.
- **Before filing a defect against a branch from a local test run, re-run the same test on `origin/main` in a clean worktree.** If it fails there too, the problem is the environment, not the branch. That one check is what caught this.

Pairs with [[feedback_instrument_dont_infer_on_contradiction]], [[feedback_ci_surface_beyond_cargo_test]], [[feedback_verify_before_filing]].

### feedback_use_aida_pull_not_git_pull_for_autobump

After merging a PR whose commit carries a `(SPEC-ID)` trailer, sync with **`aida pull`**, not raw `git pull` / `git pull --ff-only`.

**Why:** the Done→Completed auto-bump (the merge-scan that promotes a spec once its referencing commit lands on the default branch) is part of `aida pull`'s two-leg sync, NOT raw git. Pulling with raw git brings the code down but skips the scan, so the spec stays stuck at `Done` and you have to `aida db reconcile-status --spec <ID>` to recover. (2026-06-14: hit this 3× in one session — TASK-801, TASK-798, BUG-552 all needed manual reconcile because I'd used `git pull --ff-only`.)

**How to apply:** the merge→sync step in any ship/review loop is `aida pull`. Only reach for raw `git pull` when you explicitly do NOT want the store leg / auto-bump (rare). If a just-merged spec is stuck at Done, the fix is `aida db reconcile-status --spec <ID>` (a manual replay of the same scan) — but prefer to avoid it by pulling correctly.

Pairs with [[feedback_commit_trailer_completes_the_spec]] (the trailer is what the scan keys on) and the git-verb-surface convention (`aida pull` code-leg is `--ff-only` by design).

### feedback_verify_a_new_ci_step_actually_executed

After adding a CI step for BUG-1688, `Build (ubuntu-latest)` went green — which says
nothing about whether the new step executed (an `if:` typo, a wrong path, or a skipped
job all leave the rollup green). The proof is the job's own step list:

```
gh api repos/<o>/<r>/actions/runs/<RUN>/jobs \
  -q '.jobs[] | select(.name=="Build (ubuntu-latest)") | .steps[]
       | select(.name|test("<step name>")) | "step \(.number) \(.conclusion) — \(.name)"'
```

**The trap that cost two calls:** `gh run list --branch <b> --limit 1` returned the
**Cross-platform (nightly)** run, not CI, so the step query came back empty and looked
like the step was missing. A branch push triggers several workflows. List them all first:

```
gh run list --branch <b> --limit 10 --json databaseId,workflowName,conclusion,status \
  -q '.[] | "\(.databaseId) \(.workflowName) :: \(.conclusion // .status)"'
```

**Why:** an empty step query is ambiguous between "step absent" and "wrong run", and the
second reading is the likelier one.

**How to apply:** validate the YAML parses and the step resolves locally
(`yaml.safe_load` + assert the step dict exists), then after CI confirm the step's own
`conclusion` by name. Related: [[feedback-read-the-verdict-not-just-the-check-rollup]],
[[feedback-ci-pending-at-handoff-is-not-ci-green]],
[[feedback-a-test-on-the-branch-may-be-wired-into-nothing]].

### feedback_verify_pr_contents_before_directing_rebase

When multiple PRs are in flight from sibling agents and one has to rebase past another's just-merged PR, the master will sometimes need to direct *"drop your commit X because PR-Y already shipped that fix."* This direction MUST be backed by an actual check of PR-Y's contents — not by what master thinks PR-Y shipped.

**Cost of being wrong:** the sibling agent drops a correct fix on master's bad guidance. Main goes (or stays) red on that path. A third agent rediscovers the bug days later during their own verification, files it as a new finding, and ships the same fix again. The triple-up cost is real: lost time, lost confidence in master's coordination, and an audit-trail artifact (the rediscovered-and-refiled finding) that future-readers have to forensically explain.

## Empirical instance 2026-05-22

PR-193 was framed as "substrate sync + scaffold-pack docs update" — it merged 22 memories + skill-prompt-kinds.md. A test in `aida-cli/src/main.rs` (`discipline_pack_scaffolds_five_docs_plus_readme`) needed renaming + count update (6→7) to match the new pack contents. The fix was authored as a follow-on commit (`13542492`) but never reached main — it lived only on Antigravity's PR-194 branch, which was the in-flight one that had to rebase past PR-193.

During the rebase, master directed Antigravity: *"PR-193 has the renamed test (discipline_pack_scaffolds_six_docs_plus_readme) with the count fix; accept origin/main's version, discard your duplicate change."* This was wrong — PR-193 was template-only; the rename was on PR-194's own branch. Antigravity correctly followed the direction, dropping their fix. Main was red on that test from PR-193's merge until Codex re-fixed it 24h later during BUG-332 verification (PR-195).

The lesson: master's mental model of "what shipped in PR-Y" was a recent-edit recall, not a verified state.

## How to apply

Before directing a sibling agent to drop a commit during rebase on the grounds that "PR-Y has it":

```bash
gh pr view <Y> --json files,mergeCommit | jq '.files[].path'
```

Or check the merge commit's actual diff:

```bash
git show <merge-sha> --stat
```

The verification takes 5 seconds. The cost of skipping it is N-agent-days of red-test rediscovery.

## Stronger form

Whenever directing destructive cross-agent work ("drop commit", "skip step", "trust PR-Y's behavior"), the direction should quote the specific evidence:

> *"Drop the `discipline_pack_scaffolds_*` rename commit — PR-193's diff (verified via `gh pr view 193 --json files`) shows it touches only `aida-core/templates/*` paths, not `aida-cli/src/main.rs`. Wait — actually PR-193 doesn't have it. Keep your commit."*

Forcing the citation forces the verification. The same discipline applies to advice given to humans: *"Per `gh pr view 193`..."* not just *"PR-193 has it."*

## Composes with

- [[feedback_one_master_advisor_until_subsystems]] — master's coordination authority is real but fallible; verification discipline is how master earns it
- [[feedback_sibling_agents_stop_and_flag]] — sibling agents should also flag suspicious direction ("are you sure PR-193 has that fix? my branch shows the test still has the old name")
- [[feedback_self_test_via_dogfood_merge]] — when fixing infrastructure, the dogfood merge validates the fix; but if master's mental model of the merge contents is wrong, the validation propagates the error

## Sibling case: before filing a brief, verify spec status (2026-05-23 instance)

The same principle applies to `aida brief <agent> <SPEC>` — master directing a sibling agent to pick up work.

**Empirical instance 2026-05-23 (late session):** master briefed Codex on STORY-434 (EPIC-31 Phase 3b — Antigravity launcher). STORY-434 had already shipped earlier in the session via Codex's own PR-257. Codex read the brief, noticed the duplicate, had to push a store sync to clear the brief queue, and reported the duplicate back. Cost: ~5-10 minutes of Codex cycles wasted on a brief-read + store-push that should never have been initiated.

Master's mental model said "Phase 3a + 3b are the launcher trio's tail end, both still pending after Phase 1+2 shipped" — but in fact 3a (STORY-433) and 3b (STORY-434) had both already shipped same-day. The recent `git log` output earlier in the same session literally showed `20423e7c [AI:codex] feat(agent): launch Antigravity with registry tracking (STORY-434) (#257)` — master read it but didn't update the mental model of "what's still pending."

### How to apply

Before filing `aida brief <agent> <SPEC>`, run:

```bash
aida show <SPEC> | head -8
```

If Status is `✓ Completed` or `✗ Rejected`, **do not file the brief**. The work the brief was describing is already done (or explicitly declined). Find a different pickup.

If Status is `▸ Approved` or `◯ Draft` or `▷ Planned` or `◐ In Progress`, file the brief.

### Substrate-as-bouncer follow-up

The discipline rule above is the "memory tier" defence. The programmatic gate that would make this impossible: `aida brief <agent> <SPEC>` itself preflight-checks spec status and refuses (or asks `--force` confirmation) when the spec is Completed/Rejected. Per `feedback_substrate_as_bouncer_not_rules` — if this recurs, file the gate.

### feedback_verify_pr_head_after_push_gh_pr_checks_lies_on_merged_pr

2026-09-18: the advisor merged PR #1932 at 09:53 while I kept pushing fixes to its branch (10:08 BUG-1228 fix, 10:33 review-round fix). My CI monitors used `gh pr checks 1932 --json name,bucket`, which kept returning `Build (ubuntu-latest)=pass` — for the MERGED head. I reported "green with all six commits" twice; the two commits had never been built and were not on main. Found only when `gh pr view --json headRefOid` disagreed with the branch tip.

**Why:** `gh pr checks` reports the PR's recorded head, which freezes at merge/close; it does not follow the branch. Pushes to a closed PR's branch are silently orphaned.

**How to apply:**
- Before pushing to a PR branch in a long session: `gh api repos/O/R/pulls/N -q '.state,.merged'` — if merged/closed, open a NEW PR off main (cherry-pick).
- CI monitors should key on the commit, not the PR: `gh api repos/O/R/commits/<sha>/check-runs` for the branch tip you pushed, or at least assert `gh api pulls/N -q .head.sha` == your local tip before trusting a pass.
- After a merge notification for a PR you are still working, stop pushing there; rebase remaining work onto main.
- Multi-hub note: `git push` "Everything up-to-date" only proves the remote branch — never the PR.
- Prefer `aida push` over raw `git push` for PR branches: it already carries a merged-branch warning (TASK-494); raw git bypasses it, which is exactly how this happened.
- `aida add` has `--description-from-file` / `--description-stdin` — use them instead of `--description "$(cat f)"`; inline text with backticks is shell-expanded (bit me again 2026-09-18).

Related: [[feedback_verify_ci_green_before_merge]], [[feedback_fetch_before_commit]], [[feedback_verify_edits_landed_before_claiming_done]].
- **Before parking or re-parking work "behind PR #N"**, run `gh api repos/O/R/pulls/N -q .merged,.merged_at` in that command. On 2026-09-18 22:36 I parked TASK-1280/1281 a second time "until #1946 merges" — it had merged at 22:24; the advisor had told me by mail I had not read. A dependency claim is a fact to check, not a memory to trust.
- **Second occurrence 2026-09-18 23:10**: pushed an extra commit to #1958's branch 34 min after the advisor merged it, again stranding the commit. The rule above was in memory and I still skipped the check. Make it MECHANICAL: never run a bare `git push` to a PR branch — use `gh api repos/O/R/pulls/N -q .merged | grep -q false && git push …` in the SAME command, so the push cannot happen against a merged PR.

