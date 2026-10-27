# STORY-1742 conformance ledger

Final ledger for the clone-local WORK transport profile. Every acceptance
item, constraint and binding advisor ruling (SKETCH v2 R1–R3, signed
2026-10-24/25; scope ruling 2026-10-26 11:42) maps to the code that implements
it and the tests that prove it. Out-of-scope rows are listed so nothing is
silently dropped.

Test locations:
- `T:` `aida-cli/tests/story_1742_transport_profile.rs` (end-to-end, real binary, isolated bare remotes)
- `S:` `aida-core/src/transport_sanitization.rs` unit tests
- `W:` `aida-core/src/db/work_intake.rs` unit tests
- `P:` `aida-core/src/transport_policy.rs` unit tests
- `G:` `aida-cli-lib/src/transport_scan.rs` unit tests
- `V:` `aida-server/src/service.rs` tests

Trace check: `aida criteria STORY-1742` lists every acceptance criterion
below with at least one traced test (`Untested criteria: none`).

Slices: 1 = #2904, 2 = #2910, S3a = #2929, S3b = #2938, S3c = #2967, S3d = this ledger's PR.

## Acceptance

| Criterion | Slice | Proof |
|---|---|---|
| `acda93d1` Enable, inspect and disable a named profile for one clone without touching tracked files or store objects | 1 | T `profile_is_clone_local_shared_by_worktrees_and_disable_restores_default`, `legacy_main_created_profile_and_lock_stay_usable_and_disable_clears_them` |
| `ac80f8ef` Profile is clone-local and follows the common Git directory | 1 | T `profile_is_clone_local_shared_by_worktrees_and_disable_restores_default` (sibling worktree reads and disables it) |
| `ac37eb1b` Bare `aida push` runs every permitted leg: one ref-only push of local `aida-store` to `origin/aida-store`; code push disabled | 2 | T `executor_pushes_one_exact_store_ref_with_identity_hook_and_no_code_hooks`, `generic_defect_travels_work_to_hub_to_public_moving_only_the_store_ref` |
| `acf9e200` Push refuses a dirty store or an in-progress Git operation | 2 | T `dirty_in_progress_and_tampered_store_hooks_refuse_without_receiving`, `busy_store_writer_is_reported_separately_from_dirty_or_active_store` |
| `ac804464` Push makes no store commit, mailbox publication, reconciliation, ID assignment or fan-out | 2 | T `executor_pushes_one_exact_store_ref_with_identity_hook_and_no_code_hooks`, `generic_defect_travels_work_to_hub_to_public_moving_only_the_store_ref` (product refs, store HEAD, mailbox and every private counter file byte-identical across the push; hub holds only `aida-store`) |
| `ac58fd1a` `--store-only` accepted and reported redundant | 1, 2 | T `plans_are_read_only_and_execution_cannot_fall_through`, `pull_fast_forwards_only_the_store_after_scanning_and_names_moved_refs` |
| `acf57804` `--code-only` refuses with an actionable explanation | 1 | T `plans_are_read_only_and_execution_cannot_fall_through` |
| `aca124ca` Pulls run only the configured legs, skip lifecycle mutations, and name every ref that can move before execution | S3c | T `pull_fast_forwards_only_the_store_after_scanning_and_names_moved_refs`, `pull_tracks_origin_without_moving_a_store_that_is_already_ahead`, `unsafe_foreign_or_divergent_incoming_history_moves_no_accepted_ref`, `dirty_store_or_missing_policy_refuses_before_fetching`, `interrupted_pull_journal_blocks_push_and_recovers_only_a_clean_known_state`, `generic_defect_travels_work_to_hub_to_public_moving_only_the_store_ref` |
| `acc84d5d` Human and JSON status report profile, permitted legs, exact refspecs, and flag disposition | 1, 2 | T `plans_are_read_only_and_execution_cannot_fall_through`, `pull_fast_forwards_only_the_store_after_scanning_and_names_moved_refs` |
| `ac7443a3` Existing clones keep current behavior until the profile is enabled | 1 | T `profile_is_clone_local_shared_by_worktrees_and_disable_restores_default`, `ordinary_shared_clones_remain_ordinary_but_enrolled_shared_metadata_refuses` |

## Constraints

| Constraint | Slice | Proof |
|---|---|---|
| Store push runs the verified store hook profile including the identity gate; project code/mirror hooks do not run | 2, S3a | T `executor_pushes_one_exact_store_ref_with_identity_hook_and_no_code_hooks`, `identity_refusal_and_remote_rejection_never_retry_or_echo_private_diagnostics`, `reviewed_non_email_identity_passes_real_store_hook_and_receipt_pins_exact_safe_history` |
| No non-fast-forward or forced push | 2 | T `non_fast_forward_is_not_reconciled_or_force_pushed`; pull divergence in `unsafe_foreign_or_divergent_incoming_history_moves_no_accepted_ref` |
| Profile cannot silently broaden remotes or refspecs | 1, 2 | T `origin_is_pinned_and_diagnostics_do_not_expose_endpoints_or_credentials`, `malformed_or_future_profiles_and_mirror_origins_fail_closed`, `chained_url_rewrite_is_refused_without_contacting_either_destination`, `symlink_then_parent_pushes_only_to_gits_actual_origin`, `changing_a_symlink_target_after_enrollment_refuses_both_remotes`, `an_old_lexical_destination_pin_cannot_confirm_the_wrong_repository`, `unresolved_filesystem_components_refuse_without_lexical_parent_collapse`, `relative_origin_pin_uses_worktree_root_and_refuses_a_different_store_endpoint`, `explicit_store_overrides_cannot_bypass_a_profile_in_either_clone` |
| No credentials in configuration or diagnostics | 2 | T `credentialed_pinned_push_authenticates_without_exposing_or_redirecting_the_endpoint`, `endpoint_whitespace_is_preserved_and_multiline_output_is_refused`, `origin_is_pinned_and_diagnostics_do_not_expose_endpoints_or_credentials` |

## Binding rulings (SKETCH v2)

| Ruling | Slice | Proof |
|---|---|---|
| R1 fixed categories (email, IPv4, IPv6, hostname, host-bearing URL) plus reviewed private deny list; no value echo | S3a | S `story_1742_categories_and_decoding_refuse_without_values`, `story_1742_html_character_references_refuse_in_every_format`; T `scanner_checks_decoded_scalars_commit_names_paths_binary_and_signature_encodings`, `scanner_refuses_signatures_identity_mismatch_structured_tokens_trailers_addresses_entities_and_limits` |
| R1 closed structural grammar; ambiguous or unsupported content refuses | S3a | S `story_1742_closed_path_grammar_matches_the_real_store_layout`, `story_1742_unknown_and_ambiguous_encodings_refuse`, `story_1742_declared_opaque_payloads_are_not_plain_decoded_text`, `story_1742_native_oplog_tags_are_closed_and_all_values_are_scanned`; `transport_yaml.rs` `story_1742_yaml_grammar_refuses_structure_not_prose_bytes`; G `story_1742_tree_entries_parse_closed_binary_framing`; T `real_store_layout_passes_at_scale_beyond_the_old_object_and_tree_caps` |
| R1 public-safe pseudonymous non-email identity through the real identity hook | S3a, S3b | T `reviewed_non_email_identity_passes_real_store_hook_and_receipt_pins_exact_safe_history`, `sanitized_intake_write_files_one_generic_draft_only_when_enrolled` |
| Full-history scan from the trust root: deleted content, merge side-parents, reused blobs at new paths, metadata | S3a | T `scanner_refuses_historical_deleted_content_and_merge_side_metadata_before_receive`, `reused_baseline_blob_under_unsafe_path_is_refused` |
| Missing/corrupt policy, forged tracking, unsafe private files, incomplete/replaced/grafted graphs fail closed with no remote objects | S3a | T `missing_policy_forged_tracking_and_unsafe_private_files_never_create_remote_objects`, `incomplete_replaced_grafted_and_alternate_graphs_refuse_before_receive`; P `story_1742_bounded_lock_wait_refuses_contention_quickly` |
| R2 private successful-push receipt; no receipt on uncertain success; stricter policy rescans from the original root | S3a | T `reviewed_non_email_identity_passes_real_store_hook_and_receipt_pins_exact_safe_history`, `policy_change_after_remote_acceptance_reports_partial_success_without_receipt`, `writer_lock_is_released_before_receive_and_later_commits_stay_local` |
| R2 actual store-only FF pull: staging with no accepted ref or `FETCH_HEAD` movement before scan; old-value-checked transition; recoverable partial state | S3c | T `pull_fast_forwards_only_the_store_after_scanning_and_names_moved_refs`, `unsafe_foreign_or_divergent_incoming_history_moves_no_accepted_ref`, `interrupted_pull_journal_blocks_push_and_recovers_only_a_clean_known_state` |
| R2 incoming history under strict WORK policy; no trust inferred from the remote | S3c | T `unsafe_foreign_or_divergent_incoming_history_moves_no_accepted_ref` |
| R3 sanitized intake: read-only preview/template, write refuses before any object/counter/oplog/ref effect, honest provenance omission | S3b | T `sanitized_intake_template_and_preview_are_read_only_and_never_echo_refusals`, `sanitized_intake_write_files_one_generic_draft_only_when_enrolled`; W `story_1742_native_bulk_paths_admit_or_refuse_whole_batches`, `story_1742_native_full_save_keeps_conflict_rules_and_policy`, `story_1742_concurrent_work_writers_serialize_instead_of_failing`, `story_1742_umask_002_work_writes_never_brick_the_next_write` |
| R3 every enabled-WORK adapter uses the prepared boundary or refuses before side effects (CLI, MCP, server) | S3b | P `story_1742_unwired_writers_refuse_before_any_effect_in_enabled_work`; V `story_1742_refused_work_saves_never_keep_refused_rows`; T `sanitized_intake_write_files_one_generic_draft_only_when_enrolled` (MCP route) |
| 2026-10-26 ruling: intake and push gate scan only what a write adds (oplog/metadata deltas) | S3b | W `story_1742_policy_scans_what_a_write_adds_never_history`, `story_1742_work_writes_succeed_on_a_realistic_shared_store`, `story_1742_work_write_appends_to_an_oplog_past_one_mib`; S `story_1742_oplog_and_metadata_scan_only_what_a_write_adds`; T `work_intake_and_push_scan_agree_on_what_a_write_adds` |
| End-to-end chain: safe generic defect WORK → hub → PUBLIC, exact inspected SHA, only `aida-store` moves | S3d | T `generic_defect_travels_work_to_hub_to_public_moving_only_the_store_ref` |
| Show and docs state the boundary, unsafe-history refusal and enrollment duty | S3d | `transport show` limitations (`BYPASS_LIMIT`, `TRUST_LIMIT`, `SANITIZATION_LIMIT`, `PULL_LIMIT`) asserted in T `profile_is_clone_local_shared_by_worktrees_and_disable_restores_default`; `docs/cli/07-project-setup.md` "Sanitization is refusal-only" and "The end-to-end path" |

## Out of scope for v1 (signed exclusions)

- **Receiving-side ingress enforcement.** The hub does not scan what it
  receives; raw Git or another client can push unscanned history that is then
  forwarded publicly. Stated in `transport show` and the setup chapter; not a
  passing enforcement claim. The end-to-end test forwards hub → PUBLIC with raw
  Git to make that explicit.
- **Clean-history export or rewriting.** Unsafe committed history refuses; the
  profile never rewrites or launders it.
- **Live hub hooks, public forwarding automation, real work data, new
  services.** None were added.
