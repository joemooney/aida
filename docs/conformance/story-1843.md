# STORY-1843 acceptance conformance matrix

<!-- trace:STORY-1843 | ai:claude -->

The completing record for STORY-1843 (addressable agent handles, and the
shared mailbox and relay work it grew into). It has one row per acceptance
criterion: the criteria in the spec description, plus the obligations from the
signed sketches and advisor rulings on the spec (comment numbers below are
`aida show STORY-1843 --comments` indexes). Comment 90 is the inventory this
matrix completes.

Evidence was checked against `origin/main` at `c832ed71e03` (2026-10-28):
each SHA is the merge commit on main (`git log origin/main`), and each test was
found by name with `rg 'fn <name>'`. Paths are relative to the repository root.

Status values:

- **MET**: implemented on main, and the named tests prove it.
- **POST-DEPLOYMENT**: an operator step after release. Advisor ruling
  2026-10-23 12:27 (comment 33) puts it outside the code obligations.
- **KNOWN LIMITATION**: a gap the advisor accepted on the record. It is
  documented, and no follow-up spec is filed.
- **PENDING-D3**: relay-side (aida-relay STORY-122). The evidence is in
  joemooney/aida-relay#98, which has not merged yet.
- **MISSING**: no implementation on main. This blocks completion.

## Slice commits on main

| Slice | Merge commit | PR |
|---|---|---|
| PR1 shared resolver, unroutable recipients refused | `597ea98e771` | #2808 line |
| PR2 migration dry-run, layout manifest refusal | `b6aa5e87013` | |
| PR2b handles, statusline, `agent ls` | `f19b73debfc` | #2864 |
| PR3 child handles, agy next-turn mail | `94124569c9b`, `84f0642f339` | #2909 |
| B1 logical inbox snapshot | `06fa4fb01ee` | #2944 |
| B2a monotone retraction | `f34fd143233` | |
| B2b identity policy, own-audience readers | `ab8c97933da` | #2952 |
| B2c post-flush ACK, common-Git receipt anchor | `b010bc35f63` | #3015 |
| C1 lifecycle anchor, retention barrier | `8ae30fc1685` | #2949 |
| C2 retire mail before destructive and reuse entries | `a35bc40b892` | #2973 |
| C3x canonical publication permit | `4e961089c95` | #3002 |
| D1 role-cohort delivery ownership | `10443faf8fd`, `481ccc8609e` | #3042, #3061 |
| D2a relay run lifetime | `27937702c65` | #3040 |
| D2b relay outbox, label reuse, handover | `b00dbdfa8ba`, `ada00a6def4` | #3049, #3068 |
| Lifecycle write fallback chain | `872aae84a7a`, `8767e9accd8` | #3069, #3072 |
| D3 aida-relay STORY-122 | not merged | aida-relay#98 (head `9d614b6`) |

## A. Description acceptance criteria

`aida criteria STORY-1843` reports no untested criteria.

| Criterion | Status | Evidence | Notes |
|---|---|---|---|
| `aca8ca0d` Every live session (claude, codex, agy) gets one short stable handle at session start, registered by each client's SessionStart hook | MET | `f19b73debfc`, `94124569c9b`. `aida-cli-lib/src/agent_handle.rs`: `spelling_bounds_refuse_with_a_suggestion_and_reserve_aliases`, `generated_handles_use_vendor_role_and_two_digit_suffix_that_grows`, `allocation_skips_live_holders_and_the_same_session_keeps_its_recipient`, `concurrent_allocators_never_share_a_handle`, `session_start_registers_a_generated_handle_or_keeps_the_launch_name`, `explicit_live_collision_refuses_and_a_launch_reservation_binds_to_its_session`, `story_1843_pr3_session_start_binds_child_handle_env` | Fourteen tests are traced to this criterion. The launch `-n` name is kept when it is given. |
| `aceb5213` `aida statusline` renders the handle for all three clients | MET | `f19b73debfc`. `agent_handle.rs`: `statusline_session_evidence_covers_all_three_clients`, `statusline_handle_resolves_from_a_tmux_pane_and_from_the_spawning_client`, `inherited_claude_session_env_does_not_identify_a_codex_or_agy_child`, `current_session_handle_matches_by_session_never_borrows_another` | |
| `ac97b025` `aida agent ls` lists live agents of every vendor with handle, role, spec and liveness; dead entries pruned or marked | MET | `f19b73debfc`, `b6aa5e87013`. `agent_handle.rs`: `agent_ls_views_list_every_vendor_with_handle_role_spec_and_liveness`; `aida-cli-lib/src/agent_registry.rs`: `gc_dead_agents_marks_dead_resumable_rows_ended_and_keeps_them` | |
| `acfeb946` `aida mailbox send <handle>` delivers to that agent's inbox and its notice hook surfaces it; a handle with no live agent is refused with a hint and no phantom inbox | MET | `597ea98e771`, `b6aa5e87013`. `aida-cli-lib/src/mailbox_resolver.rs`: `unknown_dead_and_zero_reader_recipients_refuse` (asserts no mailbox directory is created), `cli_send_refuses_without_write_then_delivers_to_canonical_name`; `agent_handle.rs`: `mail_to_a_handle_reaches_the_session_holding_it_and_nothing_else` | |
| `ac0eef56` A Claude session addresses an agy session by handle, and the agy session sees it on its next turn (black-box, real agy hook shape) | MET | `94124569c9b`, `84f0642f339`. `aida-cli/tests/story_1843_pr3_blackbox.rs`: `story_1843_pr3_claude_to_agy_next_turn` (drives `tests/test_story_1843_pr3.py` against the built binary); `aida-core/src/scaffolding/codex_hooks.rs`: `antigravity_scaffolding_adds_missing_mail_notice_key_without_force`; `agent_registry.rs`: `story_1843_pr3_hook_arguments_do_not_identify_a_client` | The hook shape is agy 1.4.2's real SessionStart/PreInvocation payload (comments 3 and 4). |
| `acf32e9f` The operator runs the C3 write-mode mailbox migration against the approved dry-run report | POST-DEPLOYMENT | Dry-run only on main: `b6aa5e87013`, `aida-cli-lib/src/mailbox_migration.rs`: `preview_preserves_legacy_watermarks_receipts_and_tombstones_without_writes`, `preview_refuses_malformed_envelopes_and_read_state_instead_of_skipping_them`, `preview_refuses_links_and_observed_membership_changes` | Rulings 18 #1b-#1d: run by the operator, never by an agent. Write mode refuses (`mailbox migration requires --dry-run`). |

## B. Signed obligations

| Obligation | Status | Evidence | Notes |
|---|---|---|---|
| 2 C1: send to a role snapshots its live readers, prints their names, and refuses zero readers with a hint | MET | `597ea98e771`. `mailbox_resolver.rs`: `unknown_dead_and_zero_reader_recipients_refuse`, `multiple_readers_are_named_and_duplicate_names_refuse` | |
| 2 C1: `--queue-for-role` is assigned once, to the first eligible reader | MET | `10443faf8fd`. `aida-core/src/mailbox_delivery_tests.rs`: `story1843_d1_queued_delivery_binds_exactly_once_under_a_two_reader_race` (real threads), `story1843_d1_local_only_project_binds_once_with_the_same_rule`, `story1843_d1_bind_rechecks_ownership_and_writes_nothing_when_lost`; `aida-cli-lib/src/tests/story_1843_mailbox_view_tests.rs`: `story1843_d1_queued_role_mail_is_taken_only_by_its_live_owner`, `story1843_d1_owning_reader_takes_queued_mail_on_explicit_read_and_notice` | Ruling 91 Q2 and ruling 94 F3. |
| 2 C1 / 48 #4: each cohort reader gets its own per-UUID receipt | MET | `10443faf8fd`. `mailbox_delivery_tests.rs`: `story1843_d1_cohort_is_visible_only_to_its_snapshot_and_legacy_is_unchanged`, `story1843_d1_cohort_receipts_are_independent_and_ignore_role_watermarks`, `recipient_uuid_accepts_only_the_canonical_form`, `sidecar_parse_validates_its_message_and_records` | The sidecar per message (ruling 91 Q1) leaves the message JSON immutable. |
| 94 F1: a reservation whose ids match neither the registry id nor the native session id yields no UUID | MET | `10443faf8fd`. `aida-core/src/mailbox_identity_tests.rs`: `reservation_recipient_requires_the_row_or_its_session` | |
| 94 F4: the per-turn notice bind never waits on the Lifecycle lock | MET | `10443faf8fd`. `story_1843_mailbox_view_tests.rs`: `story1843_d1_notice_never_waits_on_a_held_lifecycle_lock` | |
| 97: a source is unsighted only when it is live and its read failed; a missing or prunable sibling and a refused store never hide legacy role mail | MET | `10443faf8fd`, `481ccc8609e`. `mailbox_delivery_tests.rs`: `story1843_d1_stale_worktrees_and_a_refused_store_keep_legacy_role_mail`, `story1843_d1_an_unreadable_sending_source_fails_closed`, `story1843_d1_an_incomplete_inventory_fails_closed`, `story1843_d1_an_unresolvable_worktree_fails_closed`, `story1843_d1_an_inaccessible_worktree_fails_closed` | |
| 18 #1 / 2 C3: `aida mailbox migrate --dry-run` reconciliation report | MET | `b6aa5e87013`. `mailbox_migration.rs`: the three `preview_*` tests listed in row `acf32e9f` | |
| 18 #2: an old-layout writer hits the manifest and exits nonzero without writing; the upgrade order is documented | MET | `b6aa5e87013`. `aida-core/src/mailbox.rs`: `layout_manifest_refuses_legacy_writers_and_readers_without_writing`; `mailbox_resolver.rs`: `cli_send_refuses_at_a_layout_manifest_without_writing`; `docs/cli/06-roles-sessions.md` "Mailbox layout upgrade order" | |
| 41 (3): a child that inherits `AIDA_USER` cannot read parent-private mail; handles confer no authority | MET | `94124569c9b`, `ab8c97933da`. `agent_handle.rs`: `an_unregistered_payload_session_never_borrows_its_parents_handle`, `a_handle_spelling_a_seat_or_launch_name_confers_no_authority`; `mailbox_identity_tests.rs`: `bound_child_excludes_inherited_parent_aliases`; `story_1843_mailbox_view_tests.rs`: `story1843_bound_child_never_sees_inherited_parent_mail` | |
| 41 (1): one cross-root logical inbox resolver | MET | `06fa4fb01ee`, `f34fd143233`, `ab8c97933da`. `aida-core/src/mailbox_logical_tests.rs`: `story1843_logical_inbox_conformance_sibling_only_mail_is_found_everywhere` | |
| 41 (2): removing a worktree cannot strand sibling mail | MET | `8ae30fc1685`, `a35bc40b892`, `4e961089c95`. `aida-core/src/mailbox_lifecycle/guard_tests.rs`: `a_mail_bearing_worktree_is_kept_and_the_refusal_names_activation`, `an_activated_worktree_preserves_its_mail_then_goes`, `a_failed_destructive_step_reopens_the_source_with_its_mail`, `a_reuse_hands_the_next_occupant_an_empty_mailbox`; `aida-cli-lib/src/tests/story_1843_c2_refusal_tests.rs`: `a_refused_force_cleanup_keeps_the_session_and_its_grant_stays_revoked`, `a_refused_reap_keeps_the_session_ending_retained`, `a_refused_doctor_heal_keeps_the_orphan_tree`, `a_lane_reuse_keeps_a_refused_tree_and_reopens_after_a_failed_checkout`, `an_integrator_reuse_keeps_a_refused_tree_and_reopens_after_a_failed_reset`; `aida-core/src/mailbox_lifecycle/canonical_tests.rs`: `untracked_canonical_mail_keeps_the_store_worktree`, `a_live_canonical_publication_keeps_the_store_worktree` | |
| 48 #1 / 60 #5 / 87: private anchor, safe IO, per-key convergent publication | MET | `8ae30fc1685`, `b010bc35f63`. `aida-core/src/mailbox_lifecycle/ack_tests.rs`: `story1843_b2c_commit_anchors_receipts_that_outlive_the_source`, `story1843_b2c_partial_publish_converges_on_retry`, `story1843_b2c_full_ledger_refuses_without_pruning`, `story1843_b2c_conflicting_replicas_plan_nothing`, `story1843_b2c_receipt_inspector_honours_deadline_and_byte_budget` | Ruling 87 amends "one atomic publication" to "idempotent convergent publication". |
| 48 #2 / 60 #7: writer, destructive-entry and lock-order inventory | MET | Comments 61-63; advisor disposition in 63 | This is a design artifact and has no test. |
| 60 #1: typed context and snapshot; audience is resolved before projection | MET | `06fa4fb01ee`, `ab8c97933da`. `mailbox_identity_tests.rs`: `audience_from_facts_classes_each_key`, `audience_drops_unbounded_keys`; `story_1843_mailbox_view_tests.rs`: `story1843_selector_only_narrows` | |
| 60 #2: receipt evidence and legacy watermark rules | MET | `06fa4fb01ee`, `f34fd143233`, `b010bc35f63`. `ack_tests.rs`: `story1843_b2c_explicit_ack_resolves_a_legacy_watermark`; `story_1843_mailbox_view_tests.rs`: `story1843_legacy_watermark_is_not_new_and_is_labelled`, `story1843_a_late_old_arrival_after_a_read_is_still_new`, `story1843_unreadable_watermarks_fail_closed` | |
| 60 #3 / #4: post-flush ACK, MCP response-scoped plan, honest outcome | MET | `b010bc35f63`. `aida-cli-lib/src/mcp.rs`: `mcp_read_inbox_ack_commits_only_after_its_own_response_is_flushed`; `aida-cli-lib/src/mailbox_cmd.rs`: `piped_inbox_closed_early_acknowledges_nothing_undelivered`; `story_1843_mailbox_view_tests.rs`: `story1843_b2c_inbox_acknowledges_after_flush_into_the_anchor`, `story1843_b2c_inbox_commit_failure_exits_non_zero_and_a_retry_records` | |
| 63 #1-#6 / 76 QC1-QC4: retention barrier, permits, clone binding, Lifecycle-then-Receipt lock order | MET | `8ae30fc1685`, `a35bc40b892`, `4e961089c95`, `b010bc35f63`. `ack_tests.rs`: `story1843_b2c_lock_order_is_lifecycle_then_receipt`; `canonical_tests.rs`: `admission_under_a_store_lock_refuses_as_a_lock_order_violation`, `a_fenced_source_refuses_canonical_admission`; plus the guard and C2 refusal tests in row 41 (2) | |
| 81 D1: activation record command (operator only, human TTY) | MET | `a35bc40b892`. `mailbox_cmd.rs`: `retention_activate_refuses_a_caller_without_a_terminal`; `aida-cli-lib/src/mail_retention.rs`: `the_refusal_names_the_activation_command_only_when_it_is_missing`; `story_1843_c2_refusal_tests.rs`: `r3_f4_the_report_says_an_activation_was_recorded_by_another_build` | |
| 81 / 33: an operator records retention activation | POST-DEPLOYMENT | `aida mailbox retention activate --dry-run`, then `aida mailbox retention activate` at a human TTY once every writer host admits | Until then, mail-bearing trees are kept. That is the intended safe default (ruling 81). |
| 0 / 2 relay, AIDA side: one run UUID across rotations; gap mail accepted; terminal generation recorded once; capability gate | MET | `27937702c65`. `aida-cli-lib/src/tests/story_1843_relay_lifetime_tests.rs`: `rotations_keep_one_run_uuid`, `gap_mail_is_accepted_until_the_run_ends`, `end_is_recorded_once`, `a_protocol_mismatch_refuses_before_anything_is_reserved`, `bind_prints_the_capability_once_and_stores_only_its_digest`, `child_bind_and_end_refuse_a_caller_without_the_capability_and_write_nothing`, `the_capability_file_must_be_private_and_not_a_symlink`, `reconcile_ends_a_dead_controller_and_never_an_unknown_one`, `a_reused_controller_pid_is_dead`; `aida-cli/tests/story_1843_relay_mail.rs`: `story_1843_relay_mail_lifetime_through_the_cli` | Ruling 94 D2 Q1-Q4. |
| 0 / 2 / 48 Q3 relay, AIDA side: end/crash notice outbox, one notice per sender, never bounces a bounce, advisor deduplicated, pending and retryable | MET | `b00dbdfa8ba`. `aida-cli-lib/src/tests/story_1843_relay_outbox_tests.rs`: `mail_to_a_relay_label_is_recorded_as_a_delivery_to_its_run`, `ending_a_run_tells_each_sender_once_and_never_bounces_a_bounce` (also asserts F6: no namespace lock held across a notice write), `an_advisor_who_sent_the_mail_is_told_once`, `a_crashed_run_gets_its_notices_at_reconciliation_and_they_deliver_at_maintenance`, `a_corrupt_outbox_entry_is_skipped_and_reported_never_fatal` | Ruling 96 F4-F6. |
| 0 / 2 relay, AIDA side: label reuse reports the exact unread count; headless needs a choice; handover is provenance-linked and never a rename or auto-ACK | MET | `b00dbdfa8ba`, `ada00a6def4`. `aida-cli-lib/src/tests/story_1843_relay_reuse_tests.rs`: `reuse_without_a_choice_refuses_with_the_exact_unread_count_and_reserves_nothing`, `leave_with_old_keeps_the_mail_with_the_old_run`, `handover_lets_the_new_run_read_the_mail_and_keeps_the_old_runs_ownership`, `legacy_label_mail_refuses_reuse_whatever_the_choice`, `unreadable_read_receipts_make_the_count_unknown_and_record_no_notice`; `aida-core/src/mailbox_relay_handover_tests.rs`: `handover_adds_an_owner_and_keeps_the_old_runs_receipts`, `relay_mail_is_owned_by_its_run_and_acknowledged_under_the_run_uuid` | Ruling 96 F2-F3. |

## C. Adapter conformance (48 #3, 60 #8, sketch E)

Every reader of mail must use the one logical view (48 #3: "all
CLI/MCP/hooks/notices/status/watcher readers use the one API rather than
permanent Python union logic").

| Adapter | Status | Evidence | Notes |
|---|---|---|---|
| CLI (`mailbox inbox`, follow-up verbs) | MET | `ab8c97933da`, `b010bc35f63`. `story_1843_mailbox_view_tests.rs`: `story1843_adapters_share_one_logical_view_and_one_read_state`, `story1843_every_listed_id_resolves_in_the_follow_up_verbs`, `story1843_partial_view_is_never_an_empty_success` | |
| MCP (`read_inbox`) | MET | `b010bc35f63`. `mcp.rs`: `mcp_read_inbox_ack_commits_only_after_its_own_response_is_flushed` | |
| TUI cockpit | MET | `ab8c97933da`. `aida-tui/src/redesign/mail.rs`: `mail_items_for_reads_the_logical_view_and_flags_a_partial_one`, `mail_items_for_reads_sibling_worktrees`, `mail_items_for_a_bound_session_ignores_the_inherited_parent_identity` | |
| Per-turn notice hook (claude, codex, agy) | MET | `ab8c97933da`, `10443faf8fd`. `story1843_adapters_share_one_logical_view_and_one_read_state` (`notice_text`), `story1843_d1_notice_never_waits_on_a_held_lifecycle_lock`; agy: `story_1843_pr3_claude_to_agy_next_turn` | |
| Status, statusline, awaiting, latency | MET | `ab8c97933da`. `story1843_adapters_share_one_logical_view_and_one_read_state` (`read_urgent_unread_count_for`, `mailbox_latency_warning_for`), `story1843_awaiting_mail_channel_shares_the_view`, `story1843_latency_rows_share_the_view`, `story1843_inbox_read_clears_every_fleet_wide_unread_reader` | |
| Relay (AIDA side) | MET | `b00dbdfa8ba`. `relay_mail_outbox.rs` and `relay_mail_reuse.rs` read the view; tests are in the relay rows of section B | |
| Mail watcher (`aida mailbox wake-snapshot` + `scripts/codex-mail-wake.py`, TASK-2347) | **MISSING** | `a01a6989984` (#2915) is the only commit to `aida-cli-lib/src/mailbox_wake.rs`. It predates B2c and D1 and does not use the logical view. | `mailbox_wake::snapshot` reads one root's local `.read`/`.read-receipts` and the canonical store, and the Python script unions the roots. It never reads the B2c ledger (`<git-common-dir>/aida/mail-lifecycle/receipts/`) or D1 `.deliveries/` sidecars. So the watcher can wake for mail the view counts as read: acknowledged in a since-removed worktree, or a cohort message outside this reader's snapshot. It can also miss mail a refused or partial source would flag. The watcher only wakes and never acknowledges or exposes bodies, so it loses no mail, but the obligation is unmet. |
| Relay (aida-relay side) | PENDING-D3 | aida-relay#98 | See section D. |

## D. Relay side, aida-relay STORY-122 (D3)

Ruling 98 scope. The tests are the cases in `tests/shell/story122_relay_mail.sh`
on aida-relay#98 (head `9d614b6`, not merged). The aida-relay merge SHA is
filled in when #98 merges.

| Criterion | Status | Evidence (aida-relay#98) | Notes |
|---|---|---|---|
| Probe before reserving; refuse on a protocol mismatch | PENDING-D3 | case 4: a protocol mismatch refuses (exit 9) before the run directory or the label is reserved, and no session starts | |
| Bind before advertising the label; child-bind on every rotation; one run UUID | PENDING-D3 | case 1: two sessions keep one run UUID (one run record, two child bindings, fresh launch token per session); mid-run mail to the label is accepted | |
| The capability never goes on argv or into a session's environment | PENDING-D3 | case 1: the capability never appears on any command line or in a session's environment | |
| End on trap and normal exit, recorded once | PENDING-D3 | case 1 (ends once, reason `exit`); case 2: SIGTERM mid-session, the EXIT trap records the end once (reason `stop`) | |
| A crash leaves reconciliation to AIDA (`aida agent gc`) | PENDING-D3 | case 3: after `kill -9`, no end is recorded until `aida agent gc` reconciles the run as a crash | |
| Reuse needs `--leave-with-old` or `--handover`; headless with unread mail exits nonzero | PENDING-D3 | case 5: headless without a choice exits 9 with no session; `--leave-with-old` binds a new run UUID | |

## E. Known limitations

| Limitation | Status | Evidence | Notes |
|---|---|---|---|
| F5: a canonical-only clone sees a cohort message as legacy role mail | KNOWN LIMITATION | Ruling 94 F5; `docs/mailbox.md` "Known limitation: a canonical-only clone", and the relay note under it | Reader UUIDs are never published to the public orphan store, and the same holds for relay delivery records. No follow-up spec, per the ruling. |
| The no-rename/no-link filesystem fallback race in `write_new_synced` | KNOWN LIMITATION | Ruling on TASK-082 (2026-10-28 15:56). `872aae84a7a`, `8767e9accd8`. `aida-core/src/mailbox_lifecycle/leftovers_tests.rs`: `without_rename_or_links_the_write_falls_back_to_create_new`, `a_failed_fallback_write_leaves_no_partial_file` | The race exists only on filesystems without `RENAME_NOREPLACE` or hard links (some CIFS/SMB mounts). ADR-72 scopes AIDA to Linux local filesystems. The race fails closed: the reader sees `Unsafe`, which is retriable, and never an alias. |

## F. Completion

Not complete. One row is MISSING (the mail watcher, section C) and six rows
are PENDING-D3 (section D). The completing commit carries the bare
`(STORY-1843)` trailer only when every row is MET, POST-DEPLOYMENT or KNOWN
LIMITATION. A separate STRICT review of this PR is still required
(48 #5 / 60 #8).
