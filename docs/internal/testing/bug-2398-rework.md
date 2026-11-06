# Codex child-context rework evidence

<!-- trace:BUG-2398 | ai:codex -->

The 2026-11-01 advisor ruling and final reviewer13 bundle at
`f05e1b9e6a22c508f60c614a6848423367165465` govern this rework on PR #3207.
The preliminary exec-companion finding was withdrawn. The queue-work caller's
companion refusal and existing companion/judge restrictions remain intact.

The adapter now compares the effective tool environment with a digest of the
explicit intended child's fixed nonsecret whitelist, before the existing grant
validation. The selector read stays in `seat_authority::current_grant_id`;
subject, roster, process/key possession, delegation, expiry and revocation still
use `grant_check`. This checksum provides transport integrity, no authority.
It does not change ADR-66's deliberate same-uid replay residual. Only fixed
cause labels leave the unlinked probe capture, including when Codex reports an
invalid custom config containing private values.

Real installed-Codex fixtures cover later exact-key, set-table and full-policy
conflicts, each with and without a profile, plus profile-only and unrelated
custom configuration controls. A second independently valid same-seat child
is accepted under its own context and refused when substituted for the
intended child. Empty-baseline launches also refuse later policy replacements
that could unmask another selector. Existing user config and KEEP_ME checks,
companion preservation, actual recorder matrices and invalid-grant refusals
remain in the named native and full-access fixtures.

## Exact reviewed CI failure inventory

Run `41301174931`, retained in `/tmp/aida-review3207-ci-failures.log`, contains
21 unique failed tests. All are introduced regressions addressed here:

| Count | Tests | Correction |
|---:|---|---|
| 1 | `only_seat_authority_names_or_reads_the_grant_env_var` | Use `seat_authority::GRANT_ENV`, without exemptions. |
| 1 | `every_gate_module_is_a_dispatch_module_a_kernel_or_an_orchestrator` | Inventory `review_approval.rs` as a protected decision kernel. The whole gate directory remains on the floor. |
| 1 | `no_cli_source_outside_the_resolver_uses_raw_current_executable_lookup` | Use the existing executable resolver for the sibling matching CLI. |
| 1 | `task_2143_no_new_raw_git_where_a_lease_tree_is_read` | Tighten `pr_cmd.rs` from 48 to 47 after the relocation. |
| 1 | `bug_2398_dispatch_flow_ungranted_artifact_requires_fresh_authorized_review` | Invoke the built matching CLI for the real fresh recording, retaining `--no-ship`. |
| 1 | `spawn_and_resume_children_carry_their_own_vendor_harness_version` | Fake Codex implements the sandbox protocol by executing the real probe. |
| 2 | Both BUG-1607 Codex reviewer launch tests | Same protocol; check metadata prefix and retain exact vendor argv comparison. |
| 1 | `bug_2185_standalone_reviewer_commits_child_store_writes_on_its_error_exit` | Same protocol; retain missing-verdict error and child-store commit assertions. |
| 12 | BUG-2149/2150 collection/lifecycle tests | Same protocol; retain own-child positives, sibling/foreign/launcher/stale/wrong-head/freshness/FIFO negatives and cleanup assertions. |

The real probe reads its roster from cwd. These synthetic launch fixtures now
enter their existing scratch project in an exact isolated test child. The mock
writes the review-worktree artifact by absolute path; invoking the probe from
the real project preserves both roster and lease lookup. No roster seats,
grants or existing fixture configuration are changed to obtain a passing probe. The initial
checkpoint exposed `RosterDenied` here; the fixture correction retains that
production refusal and the complete original assertion set.

The four broader STORY-1917 train-policy fixture failures overlapping BUG-2401
are a separate retained evidence obligation. BUG-2401 implementation is
serialized after this rework; no concurrent fixture edits or assertion waivers
are assumed. BUG-2403 projection/release-grant work remains separate and
blocked downstream. No ledger edits or older-approval searches are used.

## Validation

This round used the normal Cargo slots and `AIDA_CARGO_TIMEOUT=10m`, with
long commands in the background. No full suite, unfiltered library run, slot
bypass, outer timeout or PID-namespace trust waiver was used.

| Run | Actual result |
|---|---|
| `make check-ci-fast` final rerun | 36 gates passed. |
| `make check-ci-compile` | All workspace test targets compiled; none executed (1649.1s cold build). |
| Focused `codex_tool_env` group | 8 passed, 1 failed (native socket boundary), 5.94s. |
| Final focused launch/authority/ratchet group | 50 passed, 2 failed, 16.84s. All 21 original CI failures selected: 20 passed; cleanup reaches its unchanged assertion and is refused by the existing process-visibility guard below. |
| Actual `make test-touched` | 2777 passed, 46 failed, 1 ignored, 106.69s. This broad run is **not green**. Its reported cleanup case was investigated further in the final focused run. |

The real full-access/core fixture passes the exact-key/table/full-table and
profile combinations, valid-other-child refusal, actual independent recorder
positives and ungranted/invalid-grant negatives. It is a model-free policy
proof, not a substitute for the required native host run.

Both retained failures were actually executed (neither skipped):

- `bug_2398_real_native_codex_core_delegated_roles_and_reviewer_recording`:
  `app-server socket directory must be a user-owned directory with mode 0700`.
  This tool environment exposes `/tmp/codex-daemon-1000` as mode 000.
- `standalone_reviewer_reaps_its_finished_review_once_the_verdict_is_collected`:
  the original lease/tree/branch cleanup assertions stay intact. Added read-only
  diagnostics report `dirty=false`, no visible holders, and an unverified scan:
  `this process runs in a separate pid namespace (a container?), so processes
  outside it are hidden`. The existing removal guard retains the lease. No
  production reaper, process visibility or cleanup authority was changed.

Run those two exact tests on the host against the current artifact before
claiming those completion gates passed. The final test artifact is
`target/agent/deps/aida_cli_lib-40e6b7411ae37973`, SHA-256
`e07e340f1633e5ff1f601f3809febabf882af7c4b64aab2cc558dab4cfa45981`.
The final matching CLI build passed; `target/agent/aida` SHA-256 is
`d2d97c761eba8efd39696c33c90d341e8c99d797949b64aa08c3ebb9468a0253`.
A direct current-CLI recheck of both real fixtures with `TMPDIR=/tmp` again
passes full-access/core and retains the exact native socket refusal; it does
not provide a host-native pass. Log:
`/tmp/bug2398-round2-current-cli-real-fixtures.log`.
For example, from this checkout on the host:

```sh
TMPDIR=/tmp target/agent/deps/aida_cli_lib-40e6b7411ae37973 --exact codex_tool_env::tests::bug_2398_real_native_codex_core_delegated_roles_and_reviewer_recording --nocapture --test-threads=1
TMPDIR=/tmp target/agent/deps/aida_cli_lib-40e6b7411ae37973 --exact status_sections::bug_2149_2150_standalone_reviewer_tests::standalone_reviewer_reaps_its_finished_review_once_the_verdict_is_collected --nocapture --test-threads=1
```

Retained local logs: `/tmp/bug2398-round2-focused.log`,
`/tmp/bug2398-round2-retained-evidence.log`, `/tmp/bug2398-round2-touched.log`,
`/tmp/bug2398-round2-fast-final.log`, `/tmp/bug2398-compile.log`,
`/tmp/bug2398-round2-ci-regression-results.json`, and
`/tmp/bug2398-round2-touched-failures.json`. The source/publication byte and
stable-patch identity manifest is
`/tmp/bug2398-round2-publication-identity.json`.

The 19 STORY-1917 flow/schedule tests passed on this rework's f05 baseline.
Only the matching recorder CLI path changed in their shared file. No BUG-2401
required-policy implementation or fixture amendment was included; its different
baseline and four overlapping failures must be verified separately, in the
serialized order prescribed by the advisor.

The previous host native pass (1/0, 6.08 seconds) is evidence only for the old
source. Earlier nested socket-mask and cache/IO timeout failures remain valid
failed attempts. Model-free host proof does not establish first model-backed
tool metadata or full native parity. Full CI and fresh independent STRICT
review remain required; this implementer does not record review or merge.

## Broad touched-run failures retained

These are the actual 46 failing tests from the broad run. Containment/private-
clone topology, missing parent grants, default-branch/forge fixture expectations,
synthetic-HOME setup and PID visibility failures remain unresolved evidence;
this rework does not certify them green or waive their assertions. The native
and cleanup failures are explained above. Full CI remains the ordinary gate.

- `agent_launch::bug_2002_launch_lease_tests::bug2188_checkout_created_destination_is_never_nested_or_removed`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_fresh_receipt_owns_exact_lease_and_rolls_back_requested_tree`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_failed_pool_checkout_rolls_back_the_pool_tree_then_falls_back`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_full_pool_falls_back_to_a_fresh_worktree_and_rolls_it_back`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_post_checkout_partial_materialization_rolls_back`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_prelease_failure_removes_materialized_tree_and_names_cause`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_private_clone_cleanup_removes_the_clone_pin_and_lease`
- `agent_launch::harness_version_tests::task1793_foreground_and_background_versions_reach_canonical_writes`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_production_failed_launches_rollback_setup_and_child_grant`
- `agent_launch::bug_2002_launch_lease_tests::bug2188_refused_launch_restores_approved_status_and_retry_succeeds`
- `agent_registry_root::agent_launcher_tests::apply_agent_default_flags_contained_injects_when_native`
- `agent_registry_root::agent_launcher_tests::apply_agent_default_flags_sets_contained_flag_for_claude`
- `agent_registry_root::agent_launcher_tests::interactive_claude_dontask_guard_refuses_unpromptable_mode`
- `agent_registry_root::agent_launcher_tests::tool_contained_flags_for_claude_keep_interactive_prompts`
- `agent_registry_root::agent_launcher_tests::tracked_fake_agent_receives_context_file_env_and_cleans_file`
- `agent_registry_root::agent_launcher_tests::tracked_fake_agent_receives_env_and_registry_is_removed`
- `codex_tool_env::tests::bug_2398_real_native_codex_core_delegated_roles_and_reviewer_recording`
- `compete_cmd::containment_tests::compete_children_inherit_containment_and_replace_bypass_argv`
- `drain_gate::bug_2139_headless_phase_seat_tests::unresolved_root_refuses_before_queue_or_phase_mutation`
- `gate::solo_review::story_1655_solo_review_tests::solo_self_approval_recorded_through_the_cli_carries_solo_and_clears_the_ship_gate`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1795_leftover_main_ref_is_not_default_branch_evidence`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1795_unknown_local_default_uses_forge_fallback`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1803_stripped_path_gh_resolution_finds_fallback_binary`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1803_gitlab_feature_branch_without_origin_head_proceeds_via_ls_remote`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1951_parse_ls_remote_symref_head_refuses_ambiguous_or_unsafe_answers`
- `pr_cmd::bug_2164_default_branch_ship_refusal_tests::task_1951_default_branch_of_prefers_local_origin_head_over_network_probe`
- `pr_cmd::story_1663_rebase_ship_tests::task_1951_already_merged_rerun_confirms_the_rebase_merge`
- `pr_cmd::story_1663_rebase_ship_tests::task_1951_already_merged_rerun_marks_only_the_merged_prs_spec`
- `pr_cmd::story_1663_rebase_ship_tests::task_2251_fast_forward_confirmation_requires_the_branch_to_author_its_tip`
- `pr_cmd::story_1663_rebase_ship_tests::task_2251_pure_git_already_merged_rerun_confirms_a_fast_forwarded_slice`
- `pr_cmd::story_1663_rebase_ship_tests::task_2251_pure_git_already_merged_rerun_fetches_a_stale_origin_base`
- `pr_cmd::story_1663_rebase_ship_tests::task_1951_pure_git_already_merged_rerun_uses_the_slice_branch`
- `regen::score::completeness::tests::one_helper_cannot_substitute_a_surrogate_for_the_next_help_check`
- `seat_clone::lifecycle_tests::task_2053_bwrap_session_end_and_dismiss_run_no_host_git_and_never_follow_a_clone_aida_link`
- `session::tests::build_claude_exec_command_exports_when_contained_and_strips_when_uncontained`
- `session::tests::build_claude_resume_exec_command_exports_when_contained_and_strips_when_uncontained`
- `session::tests::build_run_claude_session_command_exports_when_contained_and_strips_when_uncontained`
- `session::tests::contained_headless_args_use_strict_sandbox_settings`
- `session::tests::interactive_reviewer_and_resume_inherit_parent_before_spawn`
- `session::tests::spawn_reviewer_launch_plan_contains_or_refuses_stale_uncontained_plan`
- `session::tests::spawn_claude_resume_inherits_or_explicitly_applies_containment`
- `session_reap::bug_home_024_finished_lease_tests::finished_review_with_a_collected_verdict_is_reaped_at_exit`
- `session_reap::bug_2197_dead_lease_release_tests::dead_reviewer_of_a_merged_pr_is_reaped_and_its_hold_adopted`
- `session_reap::task_1543_reap_polish_tests::task_1543_teardown_does_not_prune_sibling_missing_worktree`
- `session_reap::bug_home_024_finished_lease_tests::review_kept_at_exit_keeps_its_lease_and_is_reaped_after_merge`
- `status_sections::bug_2149_2150_standalone_reviewer_tests::standalone_reviewer_reaps_its_finished_review_once_the_verdict_is_collected`
