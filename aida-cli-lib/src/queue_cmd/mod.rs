// trace:TASK-3429 | ai:codex
//! `aida queue` command cluster — `handle_queue_command` and the
//! advance/progress/rework/work/recover/integrate handlers plus their
//! queue-local helpers, extracted from `lib.rs` (SPIKE-78 / STORY-771;
//! pure movement, no behavior change). Shared queue/store helpers stay in
//! `lib.rs`, reached via `crate::`.
//! Clap types live in `queue_cmd_args.rs`, re-exported from `cli.rs`; their
//! owning command module is now `queue_cmd/mod.rs`.
// trace:TASK-3426 | ai:codex
// trace:STORY-771 | ai:claude

use crate::process_retry::RetryEtxtbsy;
use crate::{
    active_lease_for_cwd, active_role_scope, agent_output_mode, all_open_prs_for_spec_via_forge,
    apply_calibration_tags, apply_effort_tag, apply_session_env_to_process, auto_complete, backlog,
    base_behind_indicator, batch_tag_of, behind_origin_warning, branch_commits_ahead_default,
    branch_commits_ahead_main, brief, build_integrate_rebase_args, build_queue_warn_subset,
    burndown, canonical_review_story, canonical_role_name, capture_interface_changes,
    capture_review_calibration_for_spec, capture_test_plan, change_lookup_for_branch, ci_failures,
    classify_queue_move_target, cli, clone_seat_trees, collect_open_facts, collect_open_prs,
    commits_behind_origin_main, completion, complexity_calibration, config_path_for_project,
    confirm, current_role_instance_is_companion, current_user_id,
    default_queue_collision_fingerprint, derive_parent_epic_label, detect_default_branch_ref,
    detect_distributed_store_from, detect_open_pr_for_spec_via_forge, discover_plan_context,
    drain_caps, drain_cmd, drain_lock, drain_running_banner, drain_state, drivable_queued_count,
    drive_robustness, dup_pickup_recheck, effective_role, effort_calibration, enforce_team_gate,
    entry_matches_role_filter, entry_scope_session_match, epic_descendant_uuid_set, event_wait,
    extract_plan_followups, extract_referenced_spec_ids_from_commit, extract_spec_ids_from_commit,
    fetch_pr_ship_metadata_via_gh, filter_entries_by_descendant_set, find_lease_by_id_prefix,
    find_lease_by_spec, find_main_worktree_root, find_project_root, findings, first_run, focus,
    focus_scope_guard_for_spec, for_target_requires_dispatch_authority, forecast_rebase_onto,
    forge, format_review_label, format_review_story_display, format_tag_chip, get_default_author,
    git_log_messages, global_queue, glyph, handle_auto_complete, handle_auto_complete_batch,
    handle_auto_complete_batches, handle_auto_complete_next_n, handle_auto_complete_single_branch,
    handle_drain_resume, handle_from_pr, handle_review_spec, headless_tee, hostname,
    identity_guard, in_flight_lease_role_map, integrate, integrate_checkout, is_next_keyword_id,
    is_terminal_status, is_terminal_status_str, launch, lease_covers_cwd, lease_owning_spec,
    leases, list_leases, load_agents_bypass, load_agents_contained, load_dispenser,
    main_worktree_root_from, maintenance_schedule, maybe_hint_advisor_seat,
    maybe_hint_after_queue_drain, maybe_show_faithful_launcher_notice, merge_hold,
    move_to_absolute_position, network_retry, no_human_kickoff_gate, non_interactive_confirm,
    non_tty_interactive_implementer_preflight, normalize_batch_name, not_found, notify,
    orchestrator, orphan_store_behind_count, output_format_is_json, output_format_override,
    pane_host, parse_batch_chain, parse_days_arg, parse_next_keyword, parse_pr_arg,
    parse_review_scope, parse_review_story_pr_number, parse_status, permissions,
    pickup_worktree_path, plan_review_warning, position_after, pr_base_head,
    pr_is_merged_with_sink, pr_rebase, pr_ship, preflight_intermediate_only_check,
    preflight_stale_base_check, presence, print_effort_load_for_requirements, print_scope_sessions,
    print_ultraplan_suggestion_hint, probe_branch_on_origin, probe_resume_facts, prompt_yes_no,
    publish_orchestrated_lease_receipt_from_env, punt, queue_add_hidden_view_reason, queue_recover,
    queue_role_fallback, queued_review_story_for_pr, read_behavior_permission_mode,
    record_role_activity, recover_open_change, remote_branch_gone, render_in_flight_grouped,
    resolve_autonomy_mode, resolve_batch_members, resolve_default_branch_ref, resolve_drain_alias,
    resolve_pr_to_spec, resolve_queue_role_filter, resolve_queue_work_batch,
    resolve_queue_work_launch, resolve_queue_work_permission_mode, resolve_session_branch,
    resolve_stack_base, review_title_matches, reviewer_row_actionability, reviewer_summary, role,
    roleless_recovery_sentence, run, run_aida_db_sync_pull, run_client_trailer_guard,
    scope_fallback_pick, session, session_end, session_enforcement, session_manifest,
    shell_join_display, shell_quote, shelve_spec_on_failure, slugify, spec_arg, split_last_char,
    stacks, standalone_implementer, status_advance_requires_advisor_authority, status_display,
    strip_batch_prefix, tag_matches_exact, tag_matches_prefix, toon_status_token, truncate,
    update_manifest_for_status, verdict_tip_relation, wait_for_ci_terminal,
    warn_if_queued_ahead_of_blocker, warn_pending_briefs_for_running_agent, watch, workflow_hints,
    working_tree_clean, worktree, worktree_dirty_entries, worktree_lease,
    write_queue_work_manifest, zen, AutonomyMode, BranchOriginProbe, CiProbe, DrainOverlay,
    DupPickupDecision, NextKeyword, OpenPrSnapshot, OutputFormat, PrLookup, QueueWorkLaunch,
    ReviewForge, ReviewStoryLookup, SessionEnforcement, SessionLease, SoftSignpostShown,
};
use crate::{maybe_sync_pull, QueueCommand};
use aida_core::DatabaseBackend;
use aida_core::Storage;
use aida_core::{Requirement, RequirementStatus, RequirementType};
use anyhow::Context;
use anyhow::Result;
use colored::Colorize;
use std::collections::HashSet;
use std::io::IsTerminal;
use uuid::Uuid;
// trace:STORY-1889 | ai:claude
use crate::gate::review_verdict::evaluate_review_verdict_gate;
use std::path::Path;

// trace:STORY-1728 | ai:claude
#[path = "../queue_cmd_args.rs"]
pub(crate) mod queue_cmd_args;
pub(crate) use queue_cmd_args::*;

// trace:TASK-3426 | ai:codex
mod add;
pub(crate) use add::{
    print_queue_mutation_destination, queue_destination_details, queue_mutation_destination_human,
    queue_mutation_destination_json, role_is_stakeholder_only, shell_arg,
    stakeholder_persona_guidance, QueueDestinationDetails,
};
mod advance;
pub(crate) use advance::{
    advance_backend, advance_dispatch, advance_dispatch_close, advance_report_status,
    handle_queue_advance, queue_dead_target_summary_filter,
};
mod done;
pub(crate) use done::{
    cheap_outstanding_refusal_candidate, done_spec_outstanding_refusal, queue_done_commit_evidence,
    resolve_done_rework_ids, select_done_rework_rows, RefusalCheck, MAX_REWORK_TIP_CHECKS,
};
#[path = "integrate.rs"]
mod integrate_command;
pub(crate) use integrate_command::{
    build_focus_display_subtree, ci_state_from_terminal_probe, handle_queue_integrate,
    probe_pr_integration_state, wait_for_integrate_ci,
};
mod list;
pub(crate) use list::{
    queue_json_rows, queue_list_json_requested, queue_list_matches_requested_filters,
    render_all_users_queue, reviewer_state_chip,
};
mod next;
mod progress;
pub(crate) use progress::{
    classify_progress_bucket, handle_queue_progress, status_is_shelved, ProgressBucket,
};
mod prune_gc;
pub(crate) use prune_gc::{
    dead_queue_entries, merged_pr_review_entries, opportunistic_queue_gc,
    queue_prune_no_predicate_message, queue_target_still_dead, revalidate_dead_queue_entries,
    review_pr_is_merged_or_closed, review_summaries_pending_merge_check, unsatisfied_blocker_facts,
};
mod recover;
pub(crate) use recover::{handle_queue_recover, recover_action_label, recover_push_branch};
mod remove_move;
mod rework;
pub(crate) use rework::{
    current_queue_route, drain_running, drain_running_notice, handle_queue_rework,
    handle_rework_entry, loop_guard_park, needs_attention_parks, park_reason_line,
    record_rework_ruling, rework_is_review_shelf, rework_smart_target, rework_triage_loop,
    run_aida_interactive, run_rework, DrainRunning, ReworkArgs, ReworkFlags, REWORK,
};
mod time_args;
pub use rework::{requeue_project_root, rework_target_for_mode};
pub use time_args::parse_lookback;
pub(crate) use time_args::{
    is_definitive_time_bound_error, parse_lookback_at, parse_since_arg, parse_since_arg_at,
    parse_time_bound_at, AmbiguousLocalTime, TimeBoundOutOfRange, NAIVE_DATETIME_FORMATS,
};
mod work;
#[cfg(test)]
pub(crate) use work::resolve_rework_review;
pub(crate) use work::ResumePublicationFailure;
pub(crate) use work::{
    append_reviewer_prompt_suffixes, append_reviewer_prompt_suffixes_with,
    append_standalone_reviewer_prompt_suffixes, append_untraced_criteria_prompt_block,
    apply_queue_work_contained_env, apply_rework_conflict_route,
    auto_complete_dispatch_authority_ok, auto_complete_head_candidates,
    auto_complete_head_candidates_with_blocked, auto_complete_head_candidates_with_roles,
    auto_complete_head_drivable, auto_complete_sibling_role_hint, build_queue_work_claude_command,
    build_resolved_entry, change_verdict_bound_to, check_exited_branch_clone,
    claude_posture_display, clear_standalone_reviewer_verdict, deferred_headless_launch_hint,
    deferred_interactive_launch_hint, derive_queue_work_prompt_with_round,
    derive_rework_queue_work_prompt, derive_scope_from_entry, derive_scope_from_req_id,
    drain_preview_head_members, effective_auto_complete_role, end_finished_review,
    findings_recur_consecutively, format_queue_work_not_queued_error, handle_queue_work,
    headless_baseline_notice, headless_launch_hint, headless_waiting_rule_suffix, held_resume_plan,
    held_resume_reentry_allowed, honour_auto_complete_sandbox, infer_queue_work_role,
    is_ruling_comment, is_standalone_reviewer, latest_findings_block, launch_refused_before_agent,
    leased_implementer_queue_head, leave_setup_tree, merge_current_spec_queue_entries,
    orchestrator_authorizes_explicit_queue_pickup,
    orchestrator_authorizes_explicit_queue_pickup_with_token, orchestrator_review_envelope_for_pr,
    pick_auto_complete_head, pick_auto_complete_head_for_role,
    pickup_failure_offers_manual_continuation, pickup_refusal_gate, preclaim_branch_state,
    preclaim_collision_check, preflight_queue_work_branch, preflight_queue_work_branch_adopting,
    preflight_queue_work_branch_reclaiming, prepend_due_jobs, preview_queue_work_drain,
    print_drain_preview_members, print_drain_preview_skipped, queue_add_for_scope_routing,
    queue_drain_pickup_policy, queue_fresh_pickup_policy, queue_fresh_pickup_reason_label,
    queue_work_delegation_refusal, queue_work_is_fresh_headless, queue_work_issues_child_grant,
    queue_work_manual_continuation, queue_work_plan_wants_pr_head_branch, queue_work_role_mismatch,
    queue_work_self_claim, queued_supervised_mode, reclaim_exited_branch_clone,
    record_pickup_refusal, refuse_retired_steal, refuse_unpresentable_contained_reviewer,
    refuse_waiting_pickup, refused_review_launch_releases_lease, release_refused_review_lease,
    resolve_auto_complete_head, resolve_queue_work_plan, resolve_queue_work_posture,
    resolve_rework_findings, resume_autocomplete_conflict_message, retire_idle_review_worktree,
    review_calibration_spec_ids, review_calibration_spec_ids_in, review_envelope_targets_pr,
    review_ref_from_verdict_filename, review_round_from_comments, reviewer_from_pr_contract_suffix,
    reviewer_pickup_keeps_pr_scope, reviewer_preflight_pr_n, reviewer_prompt_for_round,
    reviewer_spawn_instant, reviewer_vendor_exit_code, reviewer_verdict_anchor_suffix,
    rework_conflict_route, rework_findings_block_for_spec, rework_pickup_prompt,
    rework_pr_head_branch_override, rework_pr_head_matches_spec, rework_role_input,
    rework_round_from_comments, ruling_comment_text, run_pickup_launch, run_queue_resume_to_exit,
    run_standalone_reviewer, scope_conflict_resume_hint, spec_matches, standalone_review_plan,
    standalone_review_pr_head, standalone_reviewer_outcome, standalone_reviewer_record_anchor,
    standalone_summary_verdict_body, verdict_recorded_time, AgentLaunchMark,
    AutoCompleteHeadCandidate, AutoCompleteHeadPick, LeasePeek, NoReviewVerdict,
    PreclaimBranchState, PreclaimDecision, QueueFreshPickup, QueueWorkEntry, QueueWorkMode,
    QueueWorkPlan, QueueWorkPosture, QueueWorkRoleInput, ReviewerVendorExit, ReworkConflictLaunch,
    StandaloneVerdictChecks, NO_REVIEW_VERDICT_EXIT_CODE, RULING_TAG,
};
#[cfg(test)]
pub(crate) use work::{
    derive_queue_work_prompt, latest_ruling_comment, prepend_ruling, resolve_pickup_attempt,
    rework_pr_head_branch_from_lookup,
};
pub use work::{refuse_waiting_review, spec_open_change_key};

/// `store_path` is the orphan-store path; the `--json` fast path opens a
/// cache-backed backend from it to resolve titles via the SQLite cache rather
/// than the legacy full YAML load.
// trace:BUG-618 | ai:claude
pub(crate) fn handle_queue_command(
    cmd: &QueueCommand,
    storage: &Storage,
    store_path: &std::path::Path,
) -> Result<()> {
    match cmd {
        QueueCommand::List { .. } => list::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Load { .. } => list::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Add { .. } => add::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Remove { .. } => remove_move::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Move { .. } => remove_move::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Advance { .. } => advance::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Clear { .. } => remove_move::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Prune { .. } => prune_gc::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Gc { .. } => prune_gc::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Next { .. } => next::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Done { .. } => done::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Work { .. } => work::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Progress { .. } => progress::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Rework { .. } => rework::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Recover { .. } => recover::handle_queue_command(cmd, storage, store_path),
        QueueCommand::Integrate { .. } => {
            integrate_command::handle_queue_command(cmd, storage, store_path)
        }
    }
}

// trace:BUG-1586 | ai:codex
fn sequential_single_batch_concurrency_notice(project_root: Option<&std::path::Path>) -> String {
    let Some(project_root) = project_root else {
        return "Ordering only — concurrency for this single-batch drain follows \
                `[drain] pipeline_depth`; the resolved value is unavailable."
            .to_string();
    };
    let depth = crate::DrainTuning::resolve(project_root).pipeline_depth();
    if depth == 1 {
        format!(
            "Ordering only — concurrency for this single-batch drain follows \
             `[drain] pipeline_depth` (resolved {depth}, i.e. strictly one at a time)."
        )
    } else {
        format!(
            "Ordering only — concurrency for this single-batch drain follows \
             `[drain] pipeline_depth` (resolved {depth}, allowing up to {depth} members \
             in flight)."
        )
    }
}

#[cfg(test)]
mod sequential_notice_tests {
    use super::sequential_single_batch_concurrency_notice;

    fn project_with_depth(depth: usize) -> tempfile::TempDir {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".aida")).unwrap();
        std::fs::write(
            project.path().join(".aida/config.toml"),
            format!("[drain]\npipeline_depth = {depth}\n"),
        )
        .unwrap();
        project
    }

    // trace:BUG-1586 | ai:codex
    #[test]
    fn sequential_notice_reports_resolved_pipeline_depth_two() {
        let project = project_with_depth(2);
        let notice = sequential_single_batch_concurrency_notice(Some(project.path()));

        assert!(notice.contains("resolved 2"), "notice was: {notice}");
        assert!(
            !notice.contains("strictly one at a time"),
            "notice was: {notice}"
        );
    }

    // trace:BUG-1586 | ai:codex
    #[test]
    fn sequential_notice_keeps_one_at_a_time_clause_for_default_depth() {
        let project = project_with_depth(crate::drain_state::default_pipeline_depth());
        let notice = sequential_single_batch_concurrency_notice(Some(project.path()));

        assert!(notice.contains("resolved 1"), "notice was: {notice}");
        assert!(notice.contains("strictly one at a time"));
    }

    // trace:BUG-1586 | ai:codex
    #[test]
    fn sequential_notice_degrades_gracefully_without_a_project_root() {
        let notice = sequential_single_batch_concurrency_notice(None);

        assert!(notice.contains("`[drain] pipeline_depth`"));
        assert!(notice.contains("resolved value is unavailable"));
        assert!(!notice.contains("strictly one at a time"));
    }
}

#[cfg(test)]
#[path = "../tests/queue_progress_tests.rs"]
mod queue_progress_tests;

// trace:TASK-1509 | ai:claude
#[cfg(test)]
#[path = "../tests/time_bound_flags_tests.rs"]
mod time_bound_flags_tests;

/// trace:BUG-225 | ai:claude
#[cfg(test)]
#[path = "../tests/headless_hint_tests.rs"]
mod headless_hint_tests;

// trace:BUG-1808 | ai:claude
#[cfg(test)]
#[path = "../tests/bug_1808_delegation_preflight_tests.rs"]
mod bug_1808_delegation_preflight_tests;

/// Personal work queue commands
#[derive(clap::Parser, Debug)]
#[clap(name = "queue")]
#[clap(hide = true)]
pub(crate) struct QueueArgs {
    #[clap(subcommand)]
    pub inner: crate::cli::QueueCommand,
}

fn run_queue(
    matches: &clap::ArgMatches,
    ctx: &crate::cmdkit::DispatchCtx,
) -> anyhow::Result<crate::cmdkit::Flow> {
    let args = <QueueArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    dispatch_queue_args(&args, ctx)
}

/// The `aida queue` store-phase execution path, shared by `run_queue` and
/// `aida pr review` (which builds `queue work <change> --auto-complete` and
/// runs it here, so every queue-work gate and refusal stays in one place).
// trace:TASK-1715 | ai:claude
pub(crate) fn dispatch_queue_args(
    args: &QueueArgs,
    ctx: &crate::cmdkit::DispatchCtx,
) -> anyhow::Result<crate::cmdkit::Flow> {
    use crate::{handle_queue_command, maybe_sync_pull, QueueArgs, QueueCommand};
    use aida_core::Storage;
    match &ctx.store {
        None => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Distributed { .. }) => Ok(crate::cmdkit::Flow::Continue),
        Some(crate::cmdkit::StoreCtx::Git {
            store_path,
            backend,
        }) => {
            use crate::git_backend_cmd::*;
            let (store_path, backend) = (*store_path, *backend);
            match args {
                QueueArgs { inner: queue_cmd } => {
                    // STORY-78: `aida queue list --sync` pulls before listing.
                    // Done here at dispatch (not inside handle_queue_command) so
                    // the helper has direct store_path access. Other queue
                    // subcommands have no --sync. trace:STORY-78 | ai:claude
                    if let QueueCommand::List { sync: true, .. } = queue_cmd {
                        maybe_sync_pull(store_path)?;
                    }
                    // Reuse the legacy Storage handler — it works against any
                    // DatabaseBackend via Storage trait shims, and our GitBackend
                    // already implements queue_list/add/remove/reorder/clear.
                    // Wrap our backend in a Storage façade pointing at the store path.
                    let storage = Storage::new(store_path);
                    handle_queue_command(queue_cmd, &storage, store_path)?;
                }
            }
            Ok(crate::cmdkit::Flow::Continue)
        }
        Some(crate::cmdkit::StoreCtx::Legacy {
            storage,
            requirements_path,
        }) => {
            let (storage, requirements_path) = (*storage, *requirements_path);
            match args {
                QueueArgs { inner: queue_cmd } => {
                    handle_queue_command(queue_cmd, &storage, &requirements_path)?;
                }
            }
            Ok(crate::cmdkit::Flow::Continue)
        }
    }
}

// trace:STORY-1728 | ai:claude
pub(crate) static QUEUE: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "queue",
    help_after: "grep",
    command: <QueueArgs as clap::CommandFactory>::command,
    policy: queue_cli_policy,
    run: crate::cmdkit::Handler::Store(run_queue),
};

// Per-verb policy, carried over from the legacy stakeholder and per-write
// auto-push tables. An unparseable invocation gets the fail-closed answer.
// trace:STORY-1728 | ai:claude
pub(crate) fn queue_cli_policy(matches: &clap::ArgMatches) -> crate::cmdkit::CmdPolicy {
    use crate::cli::QueueCommand;
    use crate::stakeholder_role::StakeholderAction as SA;
    let args = <QueueArgs as clap::FromArgMatches>::from_arg_matches(matches).ok();
    let inner = args.as_ref().map(|args| &args.inner);
    crate::cmdkit::CmdPolicy {
        stakeholder: if matches!(
            inner,
            Some(
                QueueCommand::List { .. }
                    | QueueCommand::Next { .. }
                    | QueueCommand::Progress { .. }
            )
        ) {
            SA::Read
        } else {
            SA::Queue
        },
        per_write_auto_push: matches!(
            inner,
            Some(
                QueueCommand::Add { .. }
                    | QueueCommand::Remove { .. }
                    | QueueCommand::Move { .. }
                    | QueueCommand::Clear { .. }
                    | QueueCommand::Done { .. }
                    | QueueCommand::Rework { .. }
                    | QueueCommand::Prune { .. }
                    | QueueCommand::Gc { .. }
            )
        ),
        skip_role_restore_prompt: false,
        team_identity_write: true,
    }
}

#[cfg(test)]
#[path = "../tests/bug_home_045_reviewer_verdict_tests.rs"]
mod bug_home_045_reviewer_verdict_tests;

#[cfg(test)]
#[path = "../tests/bug_2428_launch_rollback_tests.rs"]
mod bug_2428_launch_rollback_tests;

// trace:TASK-3426 | ai:codex
#[cfg(test)]
pub(crate) fn production_sources() -> Vec<(String, String)> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/queue_cmd");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = format!("queue_cmd/{}", path.file_name().unwrap().to_str().unwrap());
            (name, std::fs::read_to_string(path).unwrap())
        })
        .collect()
}
