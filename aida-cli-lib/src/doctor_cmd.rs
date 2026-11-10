//! `aida doctor` command cluster (EPIC-19) — the heavy diagnostics + migration
//! command. `handle_doctor_command` dispatches the read-only multi-agent drift
//! report (`doctor_multi_agent`), the `--heal` remediation path
//! (`heal_doctor_*`), the report renderers, and the migration/repair
//! subcommands (`doctor_fsck`, `doctor_verify_relationships`,
//! `doctor_validate_trace_comments`, `doctor_repair_stale_blocks`,
//! `doctor_scrub_collisions`, `doctor_migrate_counter_scope`, `doctor_fix_sandbox`).
//!
//! Split by concern under `doctor/` (TASK-3781). The dispatcher, collection
//! orchestration and heal routing live here; diagnostics and repairs live in
//! the concern modules. Shared library helpers live with their owning commands.

#![allow(clippy::too_many_arguments)]

use crate::{
    ack_agent_brief, agent_registry, alias, cli, collect_doctor_findings, commit, contradictions,
    events, external_import_bleed, find_project_root, findings, git_arg_guard, glyph, history,
    lane_host, leases, load_store_for_lookup, main_worktree_root_from, manual, nightly_red_status,
    normalize_doctor_category, orchestrator, permissions, process_probe,
    resolve_completed_since_cutoff, resolve_default_branch_ref, role, run, runaway_seats,
    scan_completed_without_commit_with_options, scan_store_mirror_fanout_failures, schedule_ledger,
    stale_lock_findings, toon, worktree, QueueCommand, RoleCommand,
};

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};
use colored::Colorize;

// trace:STORY-1728 | ai:claude
#[path = "doctor_cmd_args.rs"]
pub(crate) mod doctor_cmd_args;
pub(crate) use doctor_cmd_args::*;

use crate::cli::Command;
use crate::db_block::{
    doctor_migrate_counter_scope_in_store, doctor_repair_dangling_relationships_in_store,
    doctor_tombstone_stale_blocks_in_store, update_config_counter_scope,
};
use crate::doctor_root::{
    completed_without_commit_opted_out, detect_legacy_store_cruft, detect_legacy_store_in_root,
    detect_store_tracked_runtime, git_config_value, id_collision_findings,
    is_legacy_store_cruft_path, is_store_tracked_runtime_path, legacy_store_in_root_action,
    legacy_store_in_root_summary, scan_completed_without_commit, which_binary, LegacyStoreInRoot,
    DOCTOR_CATEGORY_ALIASES,
};
use crate::kernel::current_user_id;
use crate::leases::{force_cleanup_lease, list_leases};
use crate::maintenance_schedule::{disk_headroom_fail_reason, FLOOR_RECLAIM_BUDGET};
use crate::pr_lookup::{detect_merged_pr_for_branch_via_forge, PrLookup};
use crate::rel_cards::{hostname, load_dispenser, load_node_id};
use crate::remote_create::same_remote;
use crate::requirements_cli::{local_twin_note, truncate};
use crate::scaffold_cmd::{installed_codex_version, parse_codex_semver};
use crate::status_awaiting::print_status_coordination_section;
use crate::status_prs::{collect_open_prs, list_worktrees};
use crate::status_sections::{
    collect_remote_branch_commits, collect_user_context, print_status_agents_section,
    print_status_claude_code_section, print_status_codex_section, print_status_open_prs_section,
    print_status_pr_section, print_status_presence_line, print_status_queue_section,
    print_status_worktrees_section, worktree_dirty_entries,
};
use crate::store_push_pull::commit_count;
use crate::store_sync::{detect_distributed_store_from, read_id_counter_scope};
use crate::trace::{parse_trace_id_token, walk_source_for_traces};
use crate::worktree_gc::{
    bwrap_status_line, classify_worktree_gc, is_agent_managed_worktree,
    requirement_missing_acceptance, salvage_worktree_patch, sanitize_salvage_component,
    worktree_is_active, worktree_pool_global_hooks, WorktreeGcFacts, WorktreeGcVerdict,
};
use crate::worktree_gc::{run_merged_agent_worktree_gc_quiet, scan_merged_agent_worktrees};
use crate::zen_drive_root::{extract_spec_ids_from_commit, is_plan_commit_subject};
use crate::DoctorFinding;
use aida_core::{
    check_scaffold_status, RelationshipType, Requirement, RequirementStatus, RequirementType,
    RequirementsStore, ScaffoldConfig, Storage,
};
use clap::Parser;
use std::collections::HashSet;
use std::io::IsTerminal;

pub(crate) fn handle_doctor_command(
    heal: bool,
    yes: bool,
    category: Option<&str>,
    json: bool,
    force: bool,
    all: bool,
    since: Option<&str>,
    cmd: Option<&cli::DoctorCommand>,
) -> Result<()> {
    let Some(cmd) = cmd else {
        return doctor_multi_agent(DoctorRunOptions {
            heal,
            yes,
            category: category.map(str::to_string),
            json,
            force,
            all,
            since: since.map(str::to_string),
            fail_on_findings: false,
            quiet_output: false,
        });
    };
    match cmd {
        // trace:TASK-2324 | ai:codex
        cli::DoctorCommand::RustcCache { fix } => {
            let root = find_project_root()?;
            crate::rustc_cache::run(&root, *fix)
        }
        cli::DoctorCommand::Check {
            category,
            all: sub_all,
            json,
            fail_on_findings,
        } => {
            // TASK-1473: `doctor check runaway-seats` is what the
            // maintenance scheduler dispatches every 15 minutes (the
            // watchdog job) — and the watchdog rule never touches the AIDA
            // store. The full path below always pays for loading the whole
            // store plus `collect_doctor_findings` (parent-tag-drift,
            // id-collisions, lease listing, live-session probing, …) even
            // though `--category runaway-seats` filters every one of those
            // findings back out. Take a lighter entry path straight to the
            // watchdog scan for exactly this shape (single category, no
            // `--all`); anything else falls through to the full path
            // unchanged. trace:TASK-1473 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::runaway_seats::CATEGORY)
            {
                return doctor_check_runaway_seats_light(*json, *fail_on_findings);
            }
            // BUG-1675: the disk-headroom guard is a single statvfs on the
            // project root, scheduled every 30 minutes. Same shape, same
            // reasoning: skip the store load + collect_doctor_findings pass
            // the category filter would discard anyway (measured 44 s under
            // load, past the 120 s scheduler kill on a busy host — a false
            // CronJobFailed disk trip). trace:BUG-1675 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(DISK_HEADROOM_CATEGORY)
            {
                return doctor_check_disk_headroom_light(*json, *fail_on_findings);
            }
            // STORY-1699: the relay stall guard reads only
            // `.aida/relay/*/relay.status`; same store-free light path.
            // trace:STORY-1699 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::relay_stall::CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::relay_stall::CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // trace:TASK-2347 | ai:codex — data-only host health, no store/model load.
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::codex_mail_health::CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::codex_mail_health::CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // STORY-HOME-128: the mail-backlog guard reads the mailbox and the
            // agent registry only. trace:STORY-HOME-128 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::mail_backlog::CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::mail_backlog::CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // STORY-1688: lane liveness substrate check reads `.aida/` state
            // without loading the spec store.
            // trace:STORY-1688 | ai:antigravity
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::lane_liveness::CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::lane_liveness::CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // STORY-1700: the drive-mandate check reads the AIDA home and the
            // schedule driver only. trace:STORY-1700 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::mandate_drive::DOCTOR_CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::mandate_drive::DOCTOR_CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // STORY-HOME-100: the advisor handshake reads hello records and
            // the agent registry only.
            // trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::advisor_handshake::DOCTOR_CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::advisor_handshake::DOCTOR_CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // BUG-1934: the seat-grant check reads the grant store and this
            // process's ancestry only. trace:BUG-1934 | ai:claude
            if !all
                && !*sub_all
                && normalize_doctor_category(category).ok().as_deref()
                    == Some(crate::gate::seat_authority::DOCTOR_CATEGORY)
            {
                return doctor_check_store_free_light(
                    crate::gate::seat_authority::DOCTOR_CATEGORY,
                    *json,
                    *fail_on_findings,
                );
            }
            // trace:TASK-1544 | ai:codex
            if !all && !*sub_all {
                match normalize_doctor_category(category).ok().as_deref() {
                    // trace:TASK-2197 | ai:codex
                    Some(crate::loose_objects::CATEGORY) => {
                        return doctor_check_store_free_light(
                            crate::loose_objects::CATEGORY,
                            *json,
                            *fail_on_findings,
                        );
                    }
                    // trace:STORY-2630 | ai:codex
                    Some(crate::codegraph::CATEGORY) => {
                        return doctor_check_store_free_light(
                            crate::codegraph::CATEGORY,
                            *json,
                            *fail_on_findings,
                        );
                    }
                    Some("performance") => {
                        return doctor_check_store_free_light(
                            "performance",
                            *json,
                            *fail_on_findings,
                        );
                    }
                    Some("remote-drift") => {
                        return doctor_check_store_free_light(
                            "remote-drift",
                            *json,
                            *fail_on_findings,
                        );
                    }
                    _ => {}
                }
            }
            doctor_multi_agent(DoctorRunOptions {
                heal: false,
                yes,
                category: Some(category.clone()),
                json: *json,
                force,
                all: all || *sub_all,
                since: since.map(str::to_string),
                fail_on_findings: *fail_on_findings,
                quiet_output: false,
            })
        }
        cli::DoctorCommand::Heal {
            category,
            yes,
            force,
            all: sub_all,
            json,
        } => doctor_multi_agent(DoctorRunOptions {
            heal: true,
            yes: *yes,
            category: Some(category.clone()),
            json: *json,
            force: *force,
            all: all || *sub_all,
            since: since.map(str::to_string),
            fail_on_findings: false,
            quiet_output: false,
        }),
        cli::DoctorCommand::MigrateCounterScope {
            to,
            dry_run,
            yes,
            size,
        } => doctor_migrate_counter_scope(to, *dry_run, *yes, *size),
        cli::DoctorCommand::RepairStaleBlocks { dry_run, yes } => {
            doctor_repair_stale_blocks(*dry_run, *yes)
        }
        cli::DoctorCommand::ScrubCollisions => doctor_scrub_collisions(),
        cli::DoctorCommand::VerifyRelationships { repair, yes } => {
            doctor_verify_relationships(*repair, *yes)
        }
        cli::DoctorCommand::ValidateTraceComments {
            strip_dangling,
            dry_run,
            yes,
        } => doctor_validate_trace_comments(*strip_dangling, *dry_run, *yes),
        cli::DoctorCommand::Fsck => doctor_fsck(),
        cli::DoctorCommand::ConventionCheck { quiet } => doctor_convention_check(*quiet),
        cli::DoctorCommand::ShellSubstitutionHoles { exclude } => {
            doctor_shell_substitution_holes(exclude)
        }
        // trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
        cli::DoctorCommand::BareIdTwins => doctor_bare_id_twins(),
    }
}

#[cfg(test)]
#[path = "tests/doctor_cmd_task_190_tests.rs"]
mod task_190_tests;

// ----------------------------------------------------------------------------
// STORY-462 — `aida doctor`: multi-agent state drift diagnostics + healing.
// ----------------------------------------------------------------------------

// `Default` is every field's zero: a plain `aida doctor` run with no flags. Added
// so a test can name only the option it cares about and stay compiling when a
// new flag lands, rather than re-listing every field.
// trace:BUG-1734 | ai:claude
#[derive(Debug, Clone, Default)]
pub(crate) struct DoctorRunOptions {
    pub(crate) heal: bool,
    pub(crate) yes: bool,
    pub(crate) category: Option<String>,
    pub(crate) json: bool,
    pub(crate) force: bool,
    pub(crate) all: bool,
    /// TASK-673: cutoff for the completed-without-commit integrity check —
    /// specs completed before this ref/date are exempt (legacy history).
    pub(crate) since: Option<String>,
    /// Exit non-zero when the selected category has any finding, so a scheduled
    /// job can GATE on it rather than only report. Opt-in: `doctor check` stays
    /// report-only unless the caller asks, which keeps this a caller's choice
    /// rather than a contract change to a shared surface.
    // trace:STORY-1422 | ai:claude
    pub(crate) fail_on_findings: bool,
    // trace:BUG-1718 | ai:codex
    pub(crate) quiet_output: bool,
}

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_2490_tests.rs"]
mod bug_2490_tests;

pub(crate) fn doctor_multi_agent(opts: DoctorRunOptions) -> Result<()> {
    // The post-ship GC (the quiet run) shares one deadline between the scan
    // and the heal that removes what the scan found.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
    let gc_deadline = opts
        .quiet_output
        .then(|| std::time::Instant::now() + POST_SHIP_GC_BUDGET);
    // trace:STORY-2630 | ai:codex
    let invoking_root = find_project_root()?;
    let project_root = main_worktree_root_from(&invoking_root);
    // A bad `--since` fails the run with an error naming the flag rather than
    // silently applying no cutoff. trace:BUG-1622 | ai:claude
    if let Some(raw) = opts.since.as_deref() {
        resolve_completed_since_cutoff(&project_root, raw)?;
    }
    // The stale-lock healer must not need the store or cache it heals, and
    // must not hang on a saturated disk. trace:BUG-HOME-041 | ai:claude
    if !opts.all
        && !opts.quiet_output
        && opts
            .category
            .as_deref()
            .and_then(|c| normalize_doctor_category(c).ok())
            .as_deref()
            == Some("stale-locks")
    {
        return doctor_stale_locks_bounded(&project_root, &opts);
    }
    let store_path = project_root.join(".aida-store");
    let store = Storage::new(&store_path)
        .load()
        .with_context(|| format!("loading AIDA store at {}", store_path.display()))?;
    let mut findings = collect_doctor_findings(&project_root, &store, opts.category.as_deref())?;
    // The tolerant loader leaves bad objects out; report each one separately
    // while retaining the other categories and safe heals.
    // trace:BUG-2490 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "store-objects")? {
        findings.extend(scan_store_objects(&store_path));
    }
    // Main owns the store, but navigation must be reachable from this seat.
    // trace:STORY-2630 | ai:codex
    if invoking_root != project_root
        && doctor_category_selected(opts.category.as_deref(), crate::codegraph::CATEGORY)?
    {
        findings.retain(|finding| finding.category != crate::codegraph::CATEGORY);
        findings.extend(crate::codegraph::scan(&invoking_root));
    }
    // trace:BUG-1719 | ai:codex
    let mut merged_agent_worktrees_reclaimable = 0;
    let mut merged_agent_worktrees_skipped = 0;
    let mut merged_agent_worktrees_deferred = 0;
    let mut hidden_completed_without_commit = 0;
    let mut performance_audits = Vec::new();

    // trace:BUG-1693 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "merge-hold-integrity")? {
        findings.extend(
            crate::gate::merge_hold_core::unrecorded_marker_removals(&project_root)
                .into_iter()
                .map(|pr| DoctorFinding {
                    category: "merge-hold-integrity".to_string(),
                    id: format!("PR-{pr}-marker-missing"),
                    summary: format!("PR #{pr} has a recorded hold placement, but its marker disappeared without a clearance record."),
                    action: format!("inspect the PR and local .aida/events.jsonl; keep or restore the hold unless a human clearance was recorded"),
                    safe_heal: false,
                }),
        );
        // trace:BUG-1747 | ai:codex
        findings.extend(merge_hold_label_definition_findings(
            crate::merge_hold::label_definitions(&project_root),
        ));
        // A recorded Unsynced state is the durable observation from the last
        // sync attempt. Re-syncing with `list --fix` records Synced only after
        // the forge confirms it; avoid one forge read per hold here.
        // trace:BUG-1747 | ai:codex
        findings.extend(unsynced_merge_hold_findings(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-673: the completed-without-commit integrity check runs git scans
    // (a default-branch `git log` + `git grep`) so it is kept OUT of the hot
    // `collect_doctor_findings` path (which `aida status` calls on every
    // invocation) and appended here, where only `aida doctor` reaches it.
    // It honours the same `--category` filter as the built-in categories.
    // trace:TASK-673 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "completed-without-commit")? {
        let env_since = std::env::var("AIDA_DOCTOR_COMPLETED_SINCE")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if let (None, Some(raw)) = (opts.since.as_deref(), env_since.as_deref()) {
            // trace:BUG-1622 | ai:claude
            resolve_completed_since_cutoff(&project_root, raw)
                .context("AIDA_DOCTOR_COMPLETED_SINCE is set to an invalid value")?;
        }
        let since = opts
            .since
            .clone()
            .or(env_since)
            .or_else(default_completed_without_commit_recent_cutoff)
            .filter(|s| !s.trim().is_empty());
        let scan = scan_completed_without_commit_with_options(
            &project_root,
            &store,
            since.as_deref(),
            opts.all,
        );
        hidden_completed_without_commit = scan.hidden_older;
        findings.extend(scan.findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-717: stale-remote-branch scan. Like completed-without-commit it runs
    // git ancestry probes + a `gh pr list`, so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here where only `aida doctor`
    // reaches it. Honours the same `--category` filter. trace:TASK-717
    if doctor_category_selected(opts.category.as_deref(), "stale-remote-branches")? {
        findings.extend(scan_stale_remote_branches(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-878: merged Agent-tool worktree GC. Runs git ancestry probes + a forge
    // merged-PR lookup, so it is kept OUT of the hot `collect_doctor_findings`
    // path and appended here where only `aida doctor` reaches it. Honours the
    // same `--category` filter. trace:TASK-878 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "merged-agent-worktrees")? {
        // trace:BUG-1719 | ai:codex
        // The quiet run is the post-ship GC: it examines merged worktrees
        // only, within a time budget, so it never holds up a serial ship.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
        let limits = if opts.quiet_output {
            AgentWorktreeScanLimits {
                merged_only: true,
                deadline: gc_deadline,
            }
        } else {
            AgentWorktreeScanLimits::default()
        };
        let scan = scan_merged_agent_worktrees_limited(&project_root, limits);
        merged_agent_worktrees_reclaimable = scan.reclaimable_count;
        merged_agent_worktrees_skipped = scan.skipped_unmerged;
        merged_agent_worktrees_deferred = scan.deferred;
        findings.extend(scan.findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-781: project-manifest check. Reads one small file (plus one
    // `git remote get-url` when a repository is recorded), so it is cheap, but
    // it is appended here with the other opt-in categories rather than added
    // to the hot `collect_doctor_findings` path. Honours `--category`.
    // trace:STORY-781 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "project-manifest")? {
        findings.extend(scan_project_manifest(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1648: primary-checkout drift — one `git status` in the main
    // worktree, plus lease reads for best-effort attribution. Report-only:
    // it lists paths and never deletes. Kept off the hot
    // `collect_doctor_findings` path like the other git-probing categories.
    // trace:STORY-1648 | ai:claude
    if doctor_category_selected(
        opts.category.as_deref(),
        crate::main_checkout_drift::CATEGORY,
    )? {
        findings.extend(crate::main_checkout_drift::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // BUG-2211: the empty read-only config.lock a sandboxed shell leaves in
    // the shared git dir. One `git rev-parse` + one stat.
    // trace:BUG-2211 | ai:claude
    if doctor_category_selected(
        opts.category.as_deref(),
        crate::sandbox_config_lock::CATEGORY,
    )? {
        findings.extend(crate::sandbox_config_lock::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1595: commits that exist in one agent worktree only — a branch on
    // no remote, or a spec commit a reset left reachable from the reflog
    // alone. Reflogs only, bounded; no fsck. trace:TASK-1595 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), crate::stranded_commits::CATEGORY)? {
        let is_terminal = |spec: &str| {
            store
                .get_requirement_by_spec_id(spec)
                .is_some_and(|r| crate::kernel::is_terminal_status(&r.status))
        };
        findings.extend(crate::stranded_commits::scan(&project_root, &is_terminal));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // A clone whose node-local allocation would refuse: unsafe allocation
    // state or a dispenser on another node. A few stats + two TOML reads.
    // trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), crate::allocation_health::CATEGORY)? {
        findings.extend(crate::allocation_health::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // Report only: general --heal never removes compiler caches.
    // trace:TASK-2324 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), crate::rustc_cache::CATEGORY)? {
        // Coordination scans use main; build artifacts belong to the invoked
        // worktree, matching the dedicated rustc-cache command.
        let build_root = find_project_root()?;
        findings.extend(crate::rustc_cache::scan(&build_root));
    }

    // TASK-1095: remote-drift scan — do the shared branches (trunk + store) hold
    // the same tip on every configured remote? Uses cheap `ls-remote` (refs
    // only, no object transfer), so it is kept OUT of the hot
    // `collect_doctor_findings` path and appended here. Honours the same
    // `--category` filter. trace:TASK-1095 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "remote-drift")? {
        findings.extend(scan_remote_drift(&project_root));
        findings.extend(scan_store_mirror_fanout_failures(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1127: read-only permission posture check. Kept in the opt-in doctor
    // append path because it inspects user-global and native Codex config files
    // that `aida status` should not need to read on every run.
    // trace:STORY-1127 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "permission-posture")? {
        findings.extend(crate::config_cmd::scan_permission_posture_findings(
            &project_root,
        ));
        findings.extend(contained_store_boundary_findings(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // trace:STORY-1043 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "ci")? {
        if let Some(red) = nightly_red_status(&project_root) {
            findings.push(DoctorFinding {
                category: "ci".to_string(),
                id: "cross-platform-nightly-red".to_string(),
                summary: red.summary,
                action: red
                    .run_id
                    .map(|id| format!("inspect `gh run view {id}` and fix-forward until green"))
                    .unwrap_or_else(|| {
                        "inspect `gh run list --workflow cross-platform.yml`".to_string()
                    }),
                safe_heal: false,
            });
            findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        }
    }

    // TASK-1124: rule-delivery-rot — hash the DEPLOYED vendor prompts/skills
    // (project `.claude/`+`.codex/` and the machine-global `~/.codex/prompts`)
    // against the binary's embedded source templates and flag drift. Turns the
    // invisible "stale scaffolding at the seams" failure class (a contributing
    // cause of the TASK-1123 reviewer-bypass incident) into a checkable one.
    // Detection only — the fix is the re-scaffold command, not a heal.
    // trace:TASK-1124 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "scaffold-drift")? {
        findings.extend(scan_scaffold_drift(&project_root, &store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1122: store-scrub — when identity redaction is configured, has the
    // raw system identity already leaked into the store (a leak that landed
    // before redaction was turned on)? Reads store files + commit authors.
    // trace:TASK-1122 | ai:claude
    // BUG-1505: non-canonical review-verdict files. Report-only — the verdict
    // store is gitignored, so a bulk rewrite would have no history to undo it.
    // trace:BUG-1505 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "review-verdicts")? {
        findings.extend(scan_review_verdicts(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    if doctor_category_selected(opts.category.as_deref(), "store-scrub")? {
        findings.extend(scan_store_scrub(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1313: round-trip artifacts — a swallowed TOON row header or a
    // UTF-8-as-Latin-1 mojibake sequence left in stored spec text by a
    // read-modify-write through rendered `aida show` output. Pure text scan
    // over the already-loaded store (no extra git/process calls), but kept
    // off the hot `collect_doctor_findings` path per the same convention as
    // the other opt-in categories above: `aida status` should not pay for a
    // full-store text scan on every invocation. Report-only. Honours
    // `--category`. trace:TASK-1313 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "round-trip-artifacts")? {
        findings.extend(scan_round_trip_artifacts(&store));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // The performance gate: guarded command shapes that exceed their budget on
    // TOO LARGE A FRACTION of recent calls, plus guarded shapes with no recorded
    // calls at all. A proportion over a window, not a median — the distribution
    // this judges is bimodal, and a median sits at ~665ms and never trips even
    // with the regression live. Opt-in by config: a project with no
    // `[performance.budgets]` has nothing guarded and nothing to report.
    // trace:STORY-1422 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "performance")? {
        let cfg = crate::read_project_config_value(&project_root);
        let budgets = performance_budgets(cfg.as_ref());
        let policy = performance_policy(cfg.as_ref());
        if !budgets.is_empty() {
            let events = crate::usage::read_events();
            let now = chrono::Utc::now();
            // BUG-1572: `~/.aida/usage.jsonl` is machine-global, so scope the
            // population to binaries in THIS HEAD's lineage before judging.
            // trace:BUG-1572 | ai:claude
            let lineage = resolve_binary_lineage(&project_root, &events, &budgets, &policy, now);
            findings.extend(performance_findings(
                &events, &budgets, &policy, now, &lineage,
            ));
            performance_audits =
                performance_audit_records(&events, &budgets, &policy, now, &lineage);
            findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        }
    }

    // STORY-1367: free disk headroom on the filesystem holding the project
    // root, checked against a configurable floor. Silent when healthy — a
    // job that reports every run trains people to ignore it, which is how
    // the 2026-09-19 hub-drift warnings were ignored in the first place.
    // Kept in this opt-in append path (not the hot `collect_doctor_findings`
    // path) for the same reason as `remote-drift`: cheap, but not free, and
    // `aida status` should not pay for a disk probe on every invocation.
    // trace:STORY-1367 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "disk-headroom")? {
        let cfg = crate::read_project_config_value(&project_root);
        let min_free_gib = disk_headroom_min_free_gib(cfg.as_ref());
        findings.extend(scan_disk_headroom(&project_root, min_free_gib));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-2053 S5: loose objects that slow seat-clone creation (opt-in
    // append path, like disk-headroom: it runs git).
    // trace:TASK-2053 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), crate::loose_objects::CATEGORY)? {
        findings.extend(crate::loose_objects::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }
    // STORY-1699: stalled relay runs (opt-in append path, like disk-headroom).
    // trace:STORY-1699 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), crate::relay_stall::CATEGORY)? {
        findings.extend(crate::relay_stall::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }
    // trace:STORY-1700 | ai:claude
    if doctor_category_selected(
        opts.category.as_deref(),
        crate::mandate_drive::DOCTOR_CATEGORY,
    )? {
        findings.extend(crate::mandate_drive::doctor_scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }
    // trace:BUG-1934 | ai:claude
    if doctor_category_selected(
        opts.category.as_deref(),
        crate::gate::seat_authority::DOCTOR_CATEGORY,
    )? {
        findings.extend(crate::gate::seat_authority::doctor_scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }
    // trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
    if doctor_category_selected(
        opts.category.as_deref(),
        crate::advisor_handshake::DOCTOR_CATEGORY,
    )? {
        findings.extend(crate::advisor_handshake::doctor_scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-2256: live Codex agents with no mail-wake service installed. The
    // default run reads only the registry and one unit file; the full
    // `check codex-mail-wake` scan stays on its own light path.
    // trace:STORY-2256 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), crate::codex_mail_health::CATEGORY)? {
        findings.extend(crate::codex_mail_health::service_findings(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-HOME-128: mail backlog (opt-in append path: only when --category
    // is explicitly passed, like lane-liveness). trace:STORY-HOME-128 | ai:claude
    if opts.category.is_some()
        && doctor_category_selected(opts.category.as_deref(), crate::mail_backlog::CATEGORY)?
    {
        findings.extend(crate::mail_backlog::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1688: lane liveness (opt-in append path: only when --category is explicitly passed,
    // since it inspects open PRs via gh and session leases via git).
    // trace:STORY-1688 | ai:claude
    if opts.category.is_some()
        && doctor_category_selected(opts.category.as_deref(), crate::lane_liveness::CATEGORY)?
    {
        findings.extend(crate::lane_liveness::scan(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // BUG-1700: the folder-trust gate that silently strands an interactive
    // `aida agent new claude --spec` launch. Read-only (it never writes the
    // operator's ~/.claude.json) and silent unless the worktree parent really
    // lacks a covering trust record. Opt-in path for the same reason as
    // disk-headroom: it reads a file outside the project.
    // trace:BUG-1700 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "agent-launch")? {
        findings.extend(scan_agent_worktree_trust(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // BUG-1738: the pre-push mirror hook is generated once, at
    // `aida remote mirror` time, and nothing re-generates it afterwards — a
    // generator fix ships and every already-installed hook keeps running the
    // old script with no signal (BUG-1706's --dry-run forwarding never
    // reached hooks installed before it merged). Compare the installed hook
    // against the current generator and report drift. One `git rev-parse` +
    // one file read; kept in the opt-in append path with its neighbours.
    // trace:BUG-1738 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "mirror-hook-drift")? {
        findings.extend(scan_mirror_hook_drift(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // BUG-2009: an AIDA git hook without an executable bit is skipped by git
    // with no signal (the TASK-917 bypass detector sat off for 9 days), and
    // a stale generated one blocks store writes. One `git rev-parse` + a
    // read per hook; the same dry run `aida scaffold upgrade --only
    // git-hooks` prints. trace:BUG-2009 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "git-hooks")? {
        findings.extend(scan_aida_git_hooks(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1330: identity hygiene. 2026-10-05: an employer-identifying email
    // reached public history because a work machine's git config used it and
    // nothing refused it. When `[identity] allowed_emails` is configured,
    // report a git identity outside the allowlist and a pre-push hook that
    // does not run the gate. Silent when the project opted out (no
    // allowlist). One git config read + one file read; kept in the opt-in
    // append path with its neighbours. trace:TASK-1330 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "identity")? {
        findings.extend(scan_identity_hygiene(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // TASK-1527 (amendment A7): the detached-refresh health report. Silent
    // when no request is pending or a fresh request is simply in flight;
    // reports a crash-looped worker (readers are falling back to strict
    // inline reads) and a request old enough that the schedule tick should
    // have retired it. Reads one sidecar file — cheap, but kept in the
    // opt-in append path with its neighbours.
    // trace:TASK-1527 | ai:claude
    if doctor_category_selected(opts.category.as_deref(), "cache-refresh")? {
        findings.extend(scan_cache_refresh(&project_root));
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    }

    // STORY-1462: the runaway-seat watchdog. Reads the trailing day of this
    // project's session transcripts (incrementally, via a per-file watermark)
    // and trips on per-session wake/token/repeated-prompt/idle anomalies and
    // the project's daily token spend. Zero-token and bounded; opt-in path
    // for the same reason as disk-headroom. The coverage block travels with
    // the report so unavailable evidence reads as unknown, never as ok.
    // trace:STORY-1462 | ai:claude
    let mut runaway_seats = None;
    if doctor_category_selected(opts.category.as_deref(), "runaway-seats")? {
        let cfg = crate::read_project_config_value(&project_root);
        let policy = crate::runaway_seats::policy(cfg.as_ref());
        let label = project_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| project_root.display().to_string());
        let (seat_findings, coverage) = crate::runaway_seats::scan(
            &label,
            &crate::runaway_seats::default_sources(&project_root),
            &policy,
            chrono::Utc::now(),
            &crate::runaway_seats::registry_attribution(&project_root),
        );
        findings.extend(seat_findings);
        findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
        runaway_seats = Some(coverage);
    }

    let mut report = DoctorReport::from_findings(findings);
    report.codex_token_lifetime = codex_token_lifetime(
        &aida_core::exec_context::Environment::capture(),
        opts.category.as_deref(),
    )?;
    // trace:TASK-2631 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "permission-posture")? {
        report.permission_postures = crate::config_cmd::permission_posture_report(&project_root)
            .agents
            .into_iter()
            .map(|row| format!("{}: {}", row.agent, row.net_effect))
            .collect();
    }
    // Discovery and hashing only: never invoke the wrapper on the host.
    // trace:BUG-2484 | ai:codex
    if doctor_category_selected(opts.category.as_deref(), "vendor-binary")? {
        if let Some(diagnostic) = crate::vendors::installed_codex_binary_diagnostic() {
            report.vendor_binaries.push(diagnostic);
        }
    }
    report.performance_audits = performance_audits;
    report.runaway_seats = runaway_seats;
    report.hidden_completed_without_commit = hidden_completed_without_commit;

    if opts.heal {
        report.healed = heal_doctor_findings(&project_root, &report.findings, &opts, gc_deadline)?;
    }

    // trace:BUG-1749 | ai:codex
    let reason = if !opts.heal
        && !report.findings.is_empty()
        && opts.category.as_deref().is_some()
        && doctor_category_selected(opts.category.as_deref(), "permission-posture")?
    {
        Some("permission-posture finding(s) detected — see the report above".to_string())
    } else if !opts.heal && opts.fail_on_findings && !report.findings.is_empty() {
        Some(format!(
            "{} finding(s) in {} — failing because --fail-on-findings was requested",
            report.findings.len(),
            opts.category.as_deref().unwrap_or("all categories")
        ))
    } else {
        let failed = report
            .healed
            .iter()
            .filter(|r| r.status == "failed")
            .count();
        (failed > 0).then(|| format!("{failed} finding(s) failed to heal — see the report above"))
    };

    if opts.quiet_output {
        eprintln!(
            "{}",
            post_ship_gc_summary(
                merged_agent_worktrees_reclaimable,
                merged_agent_worktrees_skipped,
                merged_agent_worktrees_deferred,
                report
                    .healed
                    .iter()
                    .filter(|r| r.status == HEAL_DEFERRED)
                    .count(),
            )
        );
    } else {
        return emit_doctor_report(
            report,
            reason,
            doctor_render_mode(opts.json),
            opts.heal,
            |report| {
                if let Some(coverage) = &report.runaway_seats {
                    print!("{}", crate::runaway_seats::render_coverage(coverage));
                }
                // STORY-707: full human doctor includes orientation diagnostics.
                if opts.category.is_none() {
                    print_doctor_status_diagnostics(&project_root, &store);
                    // STORY-1655: surface that the project runs solo review.
                    // trace:STORY-1655 | ai:claude
                    if crate::gate::solo_review::effective(&project_root) {
                        println!("{}", crate::gate::solo_review::MODE_ON_LINE);
                    }
                }
            },
        );
    }

    // trace:BUG-1718 | ai:codex
    if opts.quiet_output {
        return Ok(());
    }

    Ok(())
}

fn merge_hold_label_definition_findings(
    definitions: crate::merge_hold::LabelDefinitions,
) -> Vec<DoctorFinding> {
    use crate::merge_hold::LabelDefinitions;
    match definitions {
        LabelDefinitions::Read { missing, .. } => missing
            .into_iter()
            .map(|name| DoctorFinding {
                category: "merge-hold-integrity".into(),
                id: format!("merge-hold-label-{name}-undefined"),
                summary: format!("Required merge-hold label `{name}` is undefined; the required `merge-hold-gate` check cannot enforce a hold without it."),
                action: "aida merge-hold labels --create-missing".into(),
                safe_heal: false,
            })
            .collect(),
        LabelDefinitions::Unknown(why) => vec![DoctorFinding {
            category: "merge-hold-integrity".into(),
            id: "merge-hold-labels-undetermined".into(),
            summary: format!("Could not determine merge-hold label definitions: {why}"),
            action: "aida merge-hold labels".into(),
            safe_heal: false,
        }],
        LabelDefinitions::NoForge => Vec::new(),
    }
}

fn unsynced_merge_hold_findings(root: &std::path::Path) -> Vec<DoctorFinding> {
    crate::gate::merge_hold_core::list_holds(root)
        .into_iter()
        .filter_map(|(pr, _)| {
            let crate::gate::merge_hold_core::LabelState::Unsynced(err) =
                crate::gate::merge_hold_core::read_label_state(root, pr)
            else {
                return None;
            };
            let mut action = "aida merge-hold list --fix".to_string();
            if err.contains("missing label definition") {
                action.push_str("; aida merge-hold labels --create-missing");
            }
            Some(DoctorFinding {
                category: "merge-hold-integrity".into(),
                id: format!("PR-{pr}-hold-label-unsynced"),
                summary: format!("PR #{pr} has its Layer 1 hold marker armed, but the forge label never landed; the required `merge-hold-gate` check does not enforce this hold."),
                action,
                safe_heal: false,
            })
        })
        .collect()
}

/// BUG-1929: with contained mode on, report each way the contained write set
/// would miss the store or cache this checkout resolves. Not auto-healed: the
/// fix is to create the missing file or move the store/cache back under the
/// main checkout.
// trace:BUG-1929 | ai:claude
fn contained_store_boundary_findings(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    if !crate::load_agents_contained(project_root).unwrap_or(false) {
        return Vec::new();
    }
    let main = crate::main_worktree_root_from(project_root);
    let store = aida_core::store_locate::detect_distributed_store_from(project_root);
    let cache = aida_core::db::project_cache_path(project_root);
    crate::contained_boundary::contained_store_boundary_gaps(&main, store.as_deref(), &cache)
        .into_iter()
        .enumerate()
        .map(|(n, gap)| DoctorFinding {
            category: "permission-posture".to_string(),
            id: format!("contained-store-boundary-{}", n + 1),
            summary: format!("contained sessions: {gap}"),
            action: "run any `aida` command from the main checkout, or keep the store and \
                     cache under the main checkout"
                .to_string(),
            safe_heal: false,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1747_tests.rs"]
mod bug_1747_tests;

/// TASK-1473: the light entry path for `aida doctor check runaway-seats`.
/// Reads `[watchdog]` config off the project root and runs the watchdog scan
/// directly — no `Storage::load()` of the whole AIDA store, no
/// `collect_doctor_findings` pass (parent-tag-drift, id-collisions, lease
/// listing, live-session probing, …), no other opt-in doctor category. The
/// watchdog itself reads only on-disk transcripts/logs (see
/// `runaway_seats.rs` module docs), so none of that machinery was ever
/// needed for this one category; skipping it is what turns the scheduled
/// tick's ~8s `aida doctor` startup into a sub-second run. Output shape
/// (report fields, exit-code gating on `--fail-on-findings`) matches the
/// full path exactly, so a script or the scheduler cannot tell which path
/// ran.
// trace:TASK-1473 | ai:claude
fn doctor_check_runaway_seats_light(json: bool, fail_on_findings: bool) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    let cfg = crate::read_project_config_value(&project_root);
    let policy = crate::runaway_seats::policy(cfg.as_ref());
    let label = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_root.display().to_string());
    let (findings, coverage) = crate::runaway_seats::scan(
        &label,
        &crate::runaway_seats::default_sources(&project_root),
        &policy,
        chrono::Utc::now(),
        &crate::runaway_seats::registry_attribution(&project_root),
    );

    let mut report = DoctorReport::from_findings(findings);
    report.runaway_seats = Some(coverage);

    let reason = (fail_on_findings && !report.findings.is_empty()).then(|| {
        format!(
            "{} finding(s) in runaway-seats — failing because --fail-on-findings was requested",
            report.findings.len()
        )
    });
    emit_doctor_report(report, reason, doctor_render_mode(json), false, |report| {
        if let Some(coverage) = &report.runaway_seats {
            print!("{}", crate::runaway_seats::render_coverage(coverage));
        }
    })
}

/// BUG-1675: the light entry path for `aida doctor check disk-headroom`,
/// mirroring [`doctor_check_runaway_seats_light`]. Reads
/// `[doctor.disk_headroom]` off the project root and runs the disk probe —
/// no `Storage::load()`, no `collect_doctor_findings`, no other category.
/// Output shape (report fields, `--fail-on-findings` exit gating) matches
/// the full path so the scheduler cannot tell which path ran.
// trace:BUG-1675 | ai:claude
fn doctor_check_disk_headroom_light(json: bool, fail_on_findings: bool) -> Result<()> {
    let project_root = main_worktree_root_from(&find_project_root()?);
    let report = disk_headroom_light_report(&project_root);

    let reason = fail_on_findings
        .then(|| disk_headroom_fail_reason(&report.findings))
        .flatten();
    let mode = doctor_render_mode(json);
    // STORY-1916 AC3: the routed CronJobFailed item carries only the child's
    // last stderr line, and a typed (JSON/TOON) failure prints nothing to
    // stderr. Put the reason there explicitly so the route says why.
    // trace:STORY-1916.ac641c6e | ai:claude
    if let (Some(reason), true) = (&reason, mode != DoctorRenderMode::Human) {
        eprintln!("{reason}");
    }
    emit_doctor_report(report, reason, mode, false, |report| {
        // trace:STORY-1916 | ai:claude
        if let Some(status) = &report.disk_status {
            print!("{}", crate::disk_emergency::render_status(status));
        }
    })
}

/// Default bound on `aida doctor [heal] stale-locks`; overridable with
/// `AIDA_STALE_LOCKS_DEADLINE_SECS`.
// trace:BUG-HOME-041 | ai:claude
const STALE_LOCKS_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);

/// The stale-locks bound: `AIDA_STALE_LOCKS_DEADLINE_SECS` when it parses,
/// else [`STALE_LOCKS_DEADLINE`].
// trace:BUG-HOME-041 | ai:claude
fn stale_locks_budget(raw: Option<&str>) -> std::time::Duration {
    raw.and_then(|v| v.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(STALE_LOCKS_DEADLINE)
}

/// Run `work` on a worker thread and wait for it until `deadline` (measured
/// from `now`). `None` when the deadline passes first: the worker is left to
/// finish or park on its own, and the caller returns. Filesystem reads and
/// removals parked on a saturated disk cannot be interrupted, only abandoned.
// trace:BUG-HOME-041 | ai:claude
fn within_deadline<T: Send + 'static>(
    deadline: std::time::Instant,
    now: std::time::Instant,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.recv_timeout(deadline.saturating_duration_since(now))
        .ok()
}

/// The error for a stale-locks step that outlived its deadline.
// trace:BUG-HOME-041 | ai:claude
fn stale_locks_timed_out(
    project_root: &std::path::Path,
    step: &str,
    budget: std::time::Duration,
) -> anyhow::Error {
    let cause = aida_core::io_pressure::observe_disk_saturation(None)
        .map(|disk| disk.describe())
        .unwrap_or_else(|| "the filesystem did not answer".to_string());
    anyhow::anyhow!(
        "timed out after {}s {step} the cache lock-info under {}: {cause}. \
         Re-run when the disk is responsive.",
        budget.as_secs(),
        project_root.join(".aida").display()
    )
}

/// The stale-locks scan and (with `--heal`) its heal, each run on a worker
/// thread bounded by `deadline`, so a read or a removal parked on a
/// saturated disk cannot hang the command. An interactive confirmation runs
/// on this thread between them, so the bound never cuts off a human.
// trace:BUG-HOME-041 | ai:claude
fn doctor_stale_locks_report(
    project_root: &std::path::Path,
    opts: &DoctorRunOptions,
    budget: std::time::Duration,
    deadline: std::time::Instant,
) -> Result<DoctorReport> {
    let root = project_root.to_path_buf();
    let findings = within_deadline(deadline, std::time::Instant::now(), move || {
        stale_lock_findings(&root, chrono::Utc::now())
    })
    .ok_or_else(|| stale_locks_timed_out(project_root, "reading", budget))??;
    let mut report = DoctorReport::from_findings(findings);
    if !opts.heal || report.findings.is_empty() {
        return Ok(report);
    }
    let safe = report.findings.iter().all(|f| f.safe_heal);
    if safe && !opts.yes && !confirm_doctor_category("stale-locks", report.findings.len())? {
        report.healed = vec![DoctorHealResult {
            category: "stale-locks".to_string(),
            id: "stale-locks".to_string(),
            action: "operator declined".to_string(),
            status: "skipped".to_string(),
            detail: None,
        }];
        return Ok(report);
    }
    let root = project_root.to_path_buf();
    let findings = report.findings.clone();
    let heal_opts = DoctorRunOptions {
        yes: opts.yes || safe,
        ..opts.clone()
    };
    report.healed = within_deadline(deadline, std::time::Instant::now(), move || {
        heal_doctor_findings(&root, &findings, &heal_opts, Some(deadline))
    })
    .ok_or_else(|| stale_locks_timed_out(project_root, "healing", budget))??;
    Ok(report)
}

/// `aida doctor [heal] --category stale-locks`: scan only the lock-info files
/// (no store load, no cache open) and heal them, failing with the holder
/// diagnosis when either step outlives the deadline (a saturated disk can
/// park even a two-file read).
// trace:BUG-HOME-041 | ai:claude
fn doctor_stale_locks_bounded(
    project_root: &std::path::Path,
    opts: &DoctorRunOptions,
) -> Result<()> {
    let budget = stale_locks_budget(
        std::env::var("AIDA_STALE_LOCKS_DEADLINE_SECS")
            .ok()
            .as_deref(),
    );
    let report = doctor_stale_locks_report(
        project_root,
        opts,
        budget,
        std::time::Instant::now() + budget,
    )?;
    let failed = report
        .healed
        .iter()
        .filter(|r| r.status == "failed")
        .count();
    let reason = if !opts.heal && opts.fail_on_findings && !report.findings.is_empty() {
        Some(format!(
            "{} finding(s) in stale-locks — failing because --fail-on-findings was requested",
            report.findings.len()
        ))
    } else {
        (failed > 0).then(|| format!("{failed} finding(s) failed to heal — see the report above"))
    };
    emit_doctor_report(
        report,
        reason,
        doctor_render_mode(opts.json),
        opts.heal,
        |_| {},
    )
}

// trace:TASK-1544 | ai:codex
fn doctor_check_store_free_light(category: &str, json: bool, fail_on_findings: bool) -> Result<()> {
    // Keep the invoking seat for codegraph reachability; other scans use main.
    // trace:STORY-2630 | ai:codex
    let invoking_root = find_project_root()?;
    let project_root = main_worktree_root_from(&invoking_root);
    let report = match category {
        crate::codegraph::CATEGORY => {
            DoctorReport::from_findings(crate::codegraph::scan(&invoking_root))
        }
        // trace:TASK-2197 | ai:codex
        crate::loose_objects::CATEGORY => {
            DoctorReport::from_findings(crate::loose_objects::scan(&project_root))
        }
        crate::codex_mail_health::CATEGORY => {
            DoctorReport::from_findings(crate::codex_mail_health::scan(&project_root))
        }
        "performance" => performance_light_report(&project_root),
        "remote-drift" => remote_drift_light_report(&project_root),
        // trace:STORY-1699 | ai:claude
        crate::relay_stall::CATEGORY => {
            DoctorReport::from_findings(crate::relay_stall::scan(&project_root))
        }
        // Only the scheduled tick routes; findings it routed are not a job
        // failure, so the tick's command passes no --fail-on-findings.
        // trace:STORY-1688 | ai:claude
        crate::lane_liveness::CATEGORY => {
            let route = std::env::var_os("AIDA_SCHEDULE_CHILD").is_some_and(|v| v == "1");
            DoctorReport::from_findings(crate::lane_liveness::scan_and_route(&project_root, route))
        }
        // trace:STORY-1700 | ai:claude
        crate::mandate_drive::DOCTOR_CATEGORY => {
            DoctorReport::from_findings(crate::mandate_drive::doctor_scan(&project_root))
        }
        // trace:BUG-1934 | ai:claude
        crate::gate::seat_authority::DOCTOR_CATEGORY => {
            DoctorReport::from_findings(crate::gate::seat_authority::doctor_scan(&project_root))
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
        crate::advisor_handshake::DOCTOR_CATEGORY => {
            // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
            DoctorReport::from_findings(crate::advisor_handshake::doctor_report(&project_root))
        }
        // Routes like lane-liveness: only the scheduled tick nudges, and a
        // nudged backlog is not a job failure. trace:STORY-HOME-128 | ai:claude
        crate::mail_backlog::CATEGORY => {
            let route = std::env::var_os("AIDA_SCHEDULE_CHILD").is_some_and(|v| v == "1");
            DoctorReport::from_findings(crate::mail_backlog::scan_and_route(&project_root, route))
        }
        _ => unreachable!("only store-free categories dispatch here"),
    };
    let failing = fail_on_findings && !report.findings.is_empty();
    // trace:TASK-2347 | ai:codex — scheduled JSON errors otherwise suppress
    // stderr, but CronJobFailed routes only its final line. This summary is
    // built solely from the bounded counts/ages reader, never mailbox text.
    if failing && category == crate::codex_mail_health::CATEGORY {
        eprintln!("Codex mail health: {}", report.findings[0].summary);
    }
    let reason = failing.then(|| {
        format!(
            "{} finding(s) in {category} — failing because --fail-on-findings was requested",
            report.findings.len()
        )
    });
    emit_doctor_report(report, reason, doctor_render_mode(json), false, |_| {})
}

// trace:TASK-1544 | ai:codex
fn performance_light_report(project_root: &std::path::Path) -> DoctorReport {
    let cfg = crate::read_project_config_value(project_root);
    let budgets = performance_budgets(cfg.as_ref());
    let policy = performance_policy(cfg.as_ref());
    if budgets.is_empty() {
        return DoctorReport::from_findings(Vec::new());
    }
    let events = crate::usage::read_events();
    let now = chrono::Utc::now();
    let lineage = resolve_binary_lineage(project_root, &events, &budgets, &policy, now);
    performance_light_report_with(&events, &budgets, &policy, now, &lineage)
}

// Same inputs and report fields as the full append path, with telemetry and
// lineage supplied by the caller for deterministic tests.
// trace:TASK-1544 | ai:codex
fn performance_light_report_with(
    events: &[crate::usage::UsageEvent],
    budgets: &[PerformanceBudget],
    policy: &PerformancePolicy,
    now: chrono::DateTime<chrono::Utc>,
    lineage: &BinaryLineage,
) -> DoctorReport {
    let mut findings = performance_findings(events, budgets, policy, now, lineage);
    findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    let mut report = DoctorReport::from_findings(findings);
    report.performance_audits = performance_audit_records(events, budgets, policy, now, lineage);
    report
}

// trace:TASK-1544 | ai:codex
fn remote_drift_light_report(project_root: &std::path::Path) -> DoctorReport {
    remote_drift_light_report_with(
        project_root,
        scan_remote_drift,
        scan_store_mirror_fanout_failures,
    )
}

fn remote_drift_light_report_with(
    project_root: &std::path::Path,
    drift: impl Fn(&std::path::Path) -> Vec<DoctorFinding>,
    fanout: impl Fn(&std::path::Path) -> Vec<DoctorFinding>,
) -> DoctorReport {
    let mut findings = drift(project_root);
    findings.extend(fanout(project_root));
    findings.sort_by(|a, b| a.category.cmp(&b.category).then(a.id.cmp(&b.id)));
    DoctorReport::from_findings(findings)
}

#[cfg(test)]
#[path = "tests/task_1544_light_doctor_tests.rs"]
mod task_1544_light_doctor_tests;

#[cfg(test)]
#[path = "tests/bug_1675_disk_headroom_light_tests.rs"]
mod bug_1675_disk_headroom_light_tests;

#[cfg(test)]
#[path = "tests/task_1534_worktree_gc_batch_landed_tests.rs"]
mod task_1534_worktree_gc_batch_landed_tests;

// trace:BUG-1719 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1719_doctor_prose_tests.rs"]
mod bug_1719_doctor_prose_tests;

// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
#[cfg(test)]
#[path = "tests/bug_home_033_post_ship_gc_tests.rs"]
mod bug_home_033_post_ship_gc_tests;

/// `git-hooks` doctor findings: one per AIDA git hook git would not run as
/// this build ships it, from the dry run of the hook-only refresh. Silent
/// outside a git repository.
// trace:BUG-2009 | ai:claude
fn scan_aida_git_hooks(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    if !aida_core::git_ops::is_git_repo(project_root) {
        return Vec::new();
    }
    match aida_core::git_ops::refresh_stale_aida_hooks(project_root, true) {
        Ok(report) => aida_git_hook_findings(&report, project_root),
        Err(err) => vec![DoctorFinding {
            category: "git-hooks".to_string(),
            id: "git-hooks-unreadable".to_string(),
            summary: format!("could not inspect the repository's git hooks: {err:#}"),
            action: "check the hooks directory's permissions (`git rev-parse --git-path hooks`)"
                .to_string(),
            safe_heal: false,
        }],
    }
}

/// The pure core of [`scan_aida_git_hooks`].
// trace:BUG-2009 | ai:claude
fn aida_git_hook_findings(
    report: &aida_core::git_ops::StaleHookReport,
    project_root: &std::path::Path,
) -> Vec<DoctorFinding> {
    let remedy = format!(
        "run `aida scaffold upgrade --only git-hooks --project-root {}` (add --dry-run to \
         preview); it touches only AIDA's own hooks",
        project_root.display()
    );
    let mut findings: Vec<DoctorFinding> = report
        .refreshed
        .iter()
        .map(|hook| {
            let (id, summary) = if hook.backup.is_none() {
                (
                    format!("aida-hook-not-executable-{}", hook.name),
                    format!(
                        "the AIDA `{}` git hook ({}) is not executable, so git silently skips \
                         it on every commit",
                        hook.name,
                        hook.path.display()
                    ),
                )
            } else {
                let mode = if hook.not_executable {
                    " and is not executable, so git silently skips it"
                } else {
                    "; store writes refuse it"
                };
                (
                    format!("aida-hook-stale-{}", hook.name),
                    format!(
                        "the AIDA-generated `{}` git hook ({}) is not this build's hook{mode}",
                        hook.name,
                        hook.path.display()
                    ),
                )
            };
            DoctorFinding {
                category: "git-hooks".to_string(),
                id,
                summary,
                action: remedy.clone(),
                safe_heal: false,
            }
        })
        .collect();
    findings.extend(report.manual.iter().map(|hook| DoctorFinding {
        category: "git-hooks".to_string(),
        id: format!("aida-hook-manual-{}", hook.name),
        summary: format!(
            "the AIDA `{}` git hook ({}) needs manual action",
            hook.name,
            hook.path.display()
        ),
        action: hook.reason.clone(),
        safe_heal: false,
    }));
    findings
}

/// Compare the installed pre-push mirror hook against the current generator
/// (`mirror_pre_push_hook_script`) and report when a re-install is needed.
/// Resolves the hooks directory exactly as the installer does
/// (`repo_hooks_dir`), so the file inspected is the file `aida remote mirror`
/// would refresh.
// trace:BUG-1738 | ai:claude
fn scan_mirror_hook_drift(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    mirror_hook_drift_finding(&crate::remote_create::repo_hooks_dir(project_root))
        .into_iter()
        .collect()
}

/// The tempdir-testable core of the `mirror-hook-drift` scan.
///
/// Recognition: the generated banner line (`MIRROR_HOOK_HEADER`), not
/// `MIRROR_HOOK_MARKER` — the installer tells owners of a custom pre-push
/// hook to paste the marker line into their hook, so a marker match can be
/// someone else's hook. An absent hook, a foreign hook, or a project that
/// never configured mirroring produces no finding.
///
/// Comparison: `generated_text_matches` — the same newline and
/// trailing-whitespace tolerance the installer's own refresh gate uses, so
/// this never reports a hook that `aida remote mirror` would decline to
/// rewrite. Any other byte difference is real drift: the hook is a generated
/// artifact nobody is supposed to hand-edit.
///
/// Report-only (`safe_heal: false`): the remedy rewrites a file in the
/// operator's `.git/hooks/`, a write outside the store, and the installer
/// already refreshes its own hook in place — consistent with
/// `heal_doctor_finding`'s diagnostic-only default arm.
// trace:BUG-1738 | ai:claude
fn mirror_hook_drift_finding(hooks_dir: &std::path::Path) -> Option<DoctorFinding> {
    let installed = std::fs::read_to_string(hooks_dir.join("pre-push")).ok()?;
    if !installed.contains(crate::remote_create::MIRROR_HOOK_HEADER) {
        return None;
    }
    let current = crate::remote_create::mirror_pre_push_hook_script();
    if aida_core::scaffolding::generated_text_matches(&installed, &current) {
        return None;
    }
    Some(DoctorFinding {
        category: "mirror-hook-drift".to_string(),
        id: "pre-push-mirror-hook-stale".to_string(),
        summary: "the installed pre-push mirror hook was generated by an older aida and \
                  differs from the current script — hook fixes shipped since it was \
                  installed (such as --dry-run forwarding) are not active in this repository"
            .to_string(),
        action: "run `aida remote mirror <name>` (any configured mirror hub) to refresh \
                 AIDA's own hook in place"
            .to_string(),
        safe_heal: false,
    })
}

/// TASK-1330: identity hygiene scan. Gathers the live inputs (allowlist,
/// effective `git config user.email`, installed pre-push hook body) and
/// hands them to the pure `identity_hygiene_findings` core.
// trace:TASK-1330 | ai:claude
fn scan_identity_hygiene(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let allowlist = match crate::identity_gate::read_allowed_emails(project_root) {
        Ok(list) => list,
        Err(err) => {
            return vec![DoctorFinding {
                category: "identity".to_string(),
                id: "identity-config-unreadable".to_string(),
                summary: format!(
                    "could not read [identity] allowed_emails from .aida/config.toml ({err:#}) — \
                     the push-time identity gate refuses pushes while the config is unreadable"
                ),
                action: "fix the TOML syntax in .aida/config.toml so the allowlist loads"
                    .to_string(),
                safe_heal: false,
            }];
        }
    };
    let user_email = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["config", "user.email"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let hook_body = std::fs::read_to_string(
        crate::remote_create::repo_hooks_dir(project_root).join("pre-push"),
    )
    .ok();
    identity_hygiene_findings(&allowlist, user_email.as_deref(), hook_body.as_deref())
}

/// The tempdir-testable core of the `identity` scan.
///
/// Silent when no allowlist is configured — the gate is opt-in per project.
/// Once configured, two findings cover the two ways the 2026-10-05 incident
/// could recur: the commit identity itself is wrong (`git config user.email`
/// outside the allowlist), and the recurrence guard is not wired (no
/// pre-push hook running `aida identity check-push`). Report-only: both
/// remedies touch the operator's git config / `.git/hooks/`, writes outside
/// the store.
// trace:TASK-1330 | ai:claude
fn identity_hygiene_findings(
    allowlist: &[String],
    user_email: Option<&str>,
    hook_body: Option<&str>,
) -> Vec<DoctorFinding> {
    if allowlist.is_empty() {
        return Vec::new();
    }
    let mut findings = Vec::new();
    match user_email {
        Some(email) if crate::identity_gate::email_allowed(email, allowlist) => {}
        other => {
            let observed = match other {
                Some(email) => format!("`{email}`"),
                None => "unset".to_string(),
            };
            findings.push(DoctorFinding {
                category: "identity".to_string(),
                id: "git-user-email-not-allowlisted".to_string(),
                summary: format!(
                    "git user.email is {observed}, which is not in [identity] allowed_emails \
                     ({}) — commits made here would carry a non-allowlisted identity",
                    allowlist.join(", ")
                ),
                action: "run `git config user.email <allowed-address>` (and fix any \
                         GIT_AUTHOR_EMAIL/GIT_COMMITTER_EMAIL overrides)"
                    .to_string(),
                safe_heal: false,
            });
        }
    }
    let gate_wired = hook_body
        .map(|body| body.contains(crate::identity_gate::IDENTITY_HOOK_MARKER))
        .unwrap_or(false);
    if !gate_wired {
        findings.push(DoctorFinding {
            category: "identity".to_string(),
            id: "identity-pre-push-hook-missing".to_string(),
            summary: "[identity] allowed_emails is configured but the repository's pre-push \
                      hook does not run `aida identity check-push` — a push from a \
                      misconfigured machine would publish non-allowlisted emails unchallenged"
                .to_string(),
            action: "run `aida identity install-hook` (refreshes AIDA's own hooks in place; \
                     prints the lines to add when the hook is custom)"
                .to_string(),
            safe_heal: false,
        });
    }
    findings
}

#[cfg(test)]
#[path = "tests/doctor_cmd_task_1330_identity_hygiene_tests.rs"]
mod task_1330_identity_hygiene_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_2009_git_hook_tests.rs"]
mod bug_2009_git_hook_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1738_mirror_hook_drift_tests.rs"]
mod bug_1738_mirror_hook_drift_tests;

// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
#[cfg(test)]
#[path = "tests/doctor_cmd_bug_b6b_049_seat_trust_doctor_tests.rs"]
mod bug_b6b_049_seat_trust_doctor_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1700_worktree_trust_tests.rs"]
mod bug_1700_worktree_trust_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1744_trust_breadth_tests.rs"]
mod bug_1744_trust_breadth_tests;

/// STORY-1367: free space on the filesystem holding `project_root`, via
/// `sysinfo` (same disk-enumeration approach as `machine_readiness`, which
/// this check does not reuse directly because that module answers a
/// different question — "is this host big enough for a planned drain of N
/// lanes/specs/hours" — while this one is an ambient, fixed-floor cadence
/// check with no drain-sizing inputs). Silent (empty) when the filesystem
/// can't be resolved rather than risk a false positive.
// trace:STORY-1367 | ai:claude
/// How long a pending refresh request may sit before doctor calls it stuck:
/// two 15-minute tick cadences plus slack, so a single missed tick stays
/// silent and a dead tick does not.
// trace:TASK-1527 | ai:claude
const CACHE_REFRESH_REQUEST_STUCK_MINS: i64 = 35;

/// STORY-1484 slice C: surface a crash-looped detached refresh worker and a
/// refresh request nothing is retiring (amendment A7's "`cache status` and
/// doctor show the failure").
// trace:TASK-1527 | ai:claude
fn scan_cache_refresh(project_root: &std::path::Path) -> Vec<DoctorFinding> {
    let cache = aida_core::CachedGitBackend::default_cache_path(&project_root.join(".aida-store"));
    let Some(request) = aida_core::db::refresh_request::load(&cache) else {
        return Vec::new();
    };
    let now = chrono::Utc::now();
    let mut findings = Vec::new();
    if request.spawning_suppressed(now) {
        let failures = request
            .attempts
            .iter()
            .filter(|a| a.error.is_some())
            .count();
        findings.push(DoctorFinding {
            category: "cache-refresh".to_string(),
            id: "cache-refresh-crash-loop".to_string(),
            summary: format!(
                "the detached cache refresh worker failed {failures} time(s); spawning is \
                 suppressed and readers fall back to strict inline refresh{}",
                request
                    .last_error()
                    .map(|e| format!(" (last error: {e})"))
                    .unwrap_or_default()
            ),
            action: format!(
                "inspect {} and run `aida cache refresh` to refresh inline and clear the loop",
                aida_core::db::refresh_request::refresh_log_path(&cache).display()
            ),
            safe_heal: false,
        });
        return findings;
    }
    let age_mins = chrono::DateTime::parse_from_rfc3339(&request.requested_at)
        .map(|at| (now - at.with_timezone(&chrono::Utc)).num_minutes())
        .unwrap_or(i64::MAX);
    if age_mins >= CACHE_REFRESH_REQUEST_STUCK_MINS {
        findings.push(DoctorFinding {
            category: "cache-refresh".to_string(),
            id: "cache-refresh-request-stuck".to_string(),
            summary: format!(
                "a cache refresh request for {} has been pending for {age_mins} minute(s); \
                 the `cache-refresh-tick` schedule job should have retired it",
                request.target_head
            ),
            action: "run `aida cache refresh`, and check the tick is registered with \
                     `aida schedule list`"
                .to_string(),
            safe_heal: false,
        });
    }
    findings
}

#[cfg(test)]
#[path = "tests/doctor_cmd_story_1422_performance_gate_tests.rs"]
mod story_1422_performance_gate_tests;

/// STORY-1367: the disk-headroom job. `disk_headroom_finding` is pure so the
/// floor logic is verified without manufacturing a full filesystem;
/// `scan_disk_headroom` is verified against the real filesystem with a floor
/// forced to each side of "always healthy" / "never healthy" so the wiring
/// itself (path → mount → available_space) is proven too.
#[cfg(test)]
#[path = "tests/doctor_cmd_story_1367_disk_headroom_tests.rs"]
mod story_1367_disk_headroom_tests;

/// BUG-1572: the gate reads a MACHINE-GLOBAL log, so its population must be
/// scoped to one binary lineage and its finding must show the per-binary split.
#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1572_binary_lineage_tests.rs"]
mod bug_1572_binary_lineage_tests;

/// Whether `category` (a normalized doctor category) is in scope given the
/// user's `--category` filter. `None` filter selects everything. Errors only
// if the filter itself is an unknown category. trace:TASK-673 | ai:claude
fn doctor_category_selected(filter: Option<&str>, category: &str) -> Result<bool> {
    match filter {
        None => Ok(true),
        Some(raw) => Ok(normalize_doctor_category(raw)? == category),
    }
}

#[cfg(test)]
#[path = "tests/doctor_cmd_task_1313_round_trip_artifact_tests.rs"]
mod task_1313_round_trip_artifact_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_story_762_vendor_binary_tests.rs"]
mod story_762_vendor_binary_tests;

/// TASK-1089: the git-canonical storage migration cutoff. Completions BEFORE
/// this date predate the reliable `(SPEC-ID)` trailer + trace convention, and
/// the bulk YAML import stamped every pre-migration spec's `created_at` at
/// import time — so flagging them as "completed without commit" is pure noise
/// (the observed 353→212 import-cohort false positives). Completions on/after it
/// are expected to carry git corroboration. A FIXED migration date, not a
/// rolling window: a window would slide forward and eventually hide genuinely
/// stranded recent completions. Override per-run with `--since` /
/// `AIDA_DOCTOR_COMPLETED_SINCE`. Pinned to UTC midnight so the count does
/// not depend on the machine's timezone.
// trace:TASK-1089 | ai:claude
// trace:TASK-1509 | ai:claude
const GIT_CANONICAL_MIGRATION_CUTOFF: &str = "2026-06-01T00:00:00Z";

fn default_completed_without_commit_recent_cutoff() -> Option<String> {
    Some(GIT_CANONICAL_MIGRATION_CUTOFF.to_string())
}

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1796_orphan_store_doctor_tests.rs"]
mod bug_1796_orphan_store_doctor_tests;

/// STORY-666: is THIS process an autonomous / unattended context for the
/// purposes of the destructive-heal gate? Pure so it is unit-testable without
/// touching the real terminal or environment — `doctor_running_autonomously`
/// (below) feeds it the live signals.
///
/// **Autonomous = no interactive TTY OR a corroborated live orchestrator run.**
///
/// Deliberately NOT keyed on `--yes` alone: a human at a keyboard typing
/// `aida doctor --heal --force --yes` IS the explicit sign-off, and that
/// interactive path must stay exactly as it is today (STORY-666 req #3). The
/// `--yes` flag only ever *reaches* a destructive heal together with `--force`,
/// and at a TTY that combination is a deliberate human decision; what we must
/// fail-closed against is the genuinely unattended case — piped/CI stdin (no
// TTY) or an `--auto-complete` drain (orchestrator token live). trace:STORY-666
fn doctor_context_is_autonomous(stdin_is_tty: bool, orchestrated: bool) -> bool {
    !stdin_is_tty || orchestrated
}

/// STORY-666: live-signal wrapper for [`doctor_context_is_autonomous`] — reads
/// the real TTY state + the corroborated orchestrator verdict for `project_root`.
// trace:STORY-666 | ai:claude
pub(crate) fn doctor_running_autonomously(project_root: &std::path::Path) -> bool {
    let orchestrated = orchestrator::detect(project_root).is_orchestrated();
    doctor_context_is_autonomous(std::io::stdin().is_terminal(), orchestrated)
}

/// STORY-666: the heal disposition for one finding-category, decided by its
/// safe/destructive classification, the requested flags, and whether we are in
/// an autonomous context. Pure → unit-testable. The single place the
// fail-closed invariant lives. trace:STORY-666 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HealDisposition {
    /// Apply the category's heals (safe always; destructive only with explicit
    /// interactive sign-off).
    Proceed,
    /// A destructive category was requested via `--force --yes` but we are in a
    /// non-interactive/autonomous context — refuse and report (fail-closed).
    GateAutonomous,
    /// A destructive category WITHOUT the `--force --yes` opt-in — the
    /// pre-existing "needs a manual decision" skip (unchanged).
    SkipNeedsForce,
}

fn doctor_heal_disposition(
    safe: bool,
    force: bool,
    yes: bool,
    autonomous: bool,
) -> HealDisposition {
    if safe {
        // Safe, reversible heals proceed in every context — the safe
        // classification IS the bouncer. Never over-gate routine fixes.
        return HealDisposition::Proceed;
    }
    // Destructive from here down.
    if !(force && yes) {
        return HealDisposition::SkipNeedsForce;
    }
    if autonomous {
        // Fail-closed: a destructive heal must not execute silently with no one
        // to make the check-before-delete judgment.
        return HealDisposition::GateAutonomous;
    }
    // Destructive + --force --yes + interactive TTY = explicit human sign-off.
    HealDisposition::Proceed
}

/// Heal status of a finding left alone because the run's deadline passed.
// trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
pub(crate) const HEAL_DEFERRED: &str = "deferred";

/// Heals `findings`. With a `deadline`, each finding reached after it passes
/// is recorded as [`HEAL_DEFERRED`] instead of healed. The deadline is checked
/// between findings, so a heal already running finishes.
fn heal_doctor_findings(
    project_root: &std::path::Path,
    findings: &[DoctorFinding],
    opts: &DoctorRunOptions,
    deadline: Option<std::time::Instant>,
) -> Result<Vec<DoctorHealResult>> {
    // STORY-666: detect the autonomous/unattended context ONCE for the whole
    // run — the destructive-heal gate keys off it. trace:STORY-666 | ai:claude
    let autonomous = doctor_running_autonomously(project_root);
    let mut out = Vec::new();
    let mut by_category: std::collections::BTreeMap<String, Vec<&DoctorFinding>> =
        std::collections::BTreeMap::new();
    for finding in findings {
        by_category
            .entry(finding.category.clone())
            .or_default()
            .push(finding);
    }
    let mut categories: Vec<_> = by_category.into_iter().collect();
    categories.sort_by_key(|(category, _)| doctor_heal_category_order(category));
    for (category, items) in categories {
        let safe = items.iter().all(|item| item.safe_heal);
        match doctor_heal_disposition(safe, opts.force, opts.yes, autonomous) {
            HealDisposition::Proceed => {}
            HealDisposition::SkipNeedsForce => {
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: "skipped category requiring manual decision".to_string(),
                    status: "skipped".to_string(),
                    detail: Some(
                        "pass --yes --force only when you want destructive branch cleanup".into(),
                    ),
                });
                continue;
            }
            HealDisposition::GateAutonomous => {
                // STORY-666: fail-closed. A destructive heal was requested
                // (--force --yes) but we are unattended (no TTY / live drain).
                // Refuse it, name exactly what was skipped, and print the precise
                // interactive command to run it under human sign-off. The
                // `skipped` status keeps the audit trail legible and the heal
                // never executes. trace:STORY-666 | ai:claude
                out.push(DoctorHealResult {
                    category: category.clone(),
                    id: category.clone(),
                    action: format!(
                        "gated — destructive heal of {} finding(s) withheld (unattended context)",
                        items.len()
                    ),
                    status: "skipped".to_string(),
                    detail: Some(format!(
                        "destructive fixes require sign-off and were NOT applied in this \
                         unattended context. Re-run it at an interactive terminal: \
                         `aida doctor --heal --force --yes --category {category}`"
                    )),
                });
                continue;
            }
        }
        if !opts.yes && !confirm_doctor_category(&category, items.len())? {
            out.push(DoctorHealResult {
                category: category.clone(),
                id: category.clone(),
                action: "operator declined".to_string(),
                status: "skipped".to_string(),
                detail: None,
            });
            continue;
        }
        for (index, finding) in items.iter().enumerate() {
            // trace:BUG-B6B1278890774EF993D2EBC327862470-033 | ai:claude
            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                out.extend(items[index..].iter().map(|finding| DoctorHealResult {
                    category: category.clone(),
                    id: finding.id.clone(),
                    action: finding.action.clone(),
                    status: HEAL_DEFERRED.to_string(),
                    detail: Some(format!(
                        "time budget spent; run `aida doctor --category {category}`"
                    )),
                }));
                break;
            }
            // BUG-471: a single finding's heal failure must not abort the whole
            // run (the resilient-drain discipline). Record it as a `failed`
            // result and continue; the caller surfaces failures + exits non-zero.
            // trace:BUG-471 | ai:claude
            match heal_doctor_finding(project_root, finding, opts) {
                Ok(result) => out.push(result),
                Err(e) => out.push(DoctorHealResult {
                    category: category.clone(),
                    id: finding.id.clone(),
                    action: finding.action.clone(),
                    status: "failed".to_string(),
                    detail: Some(e.to_string()),
                }),
            }
        }
    }
    Ok(out)
}

fn doctor_heal_category_order(category: &str) -> u8 {
    match category {
        // trace:BUG-1136 | ai:codex
        // Lease reaping has to precede status derivation in an all-category
        // heal. Otherwise an Approved spec with only a stale lease can be
        // bumped to In Progress, then lose the lease later in the same pass.
        "stale-leases" | "abandoned-leases" | "stale-reviewer-leases" => 0,
        "spec-status-drift" => 1,
        _ => 2,
    }
}

fn confirm_doctor_category(category: &str, count: usize) -> Result<bool> {
    // BUG-407: never block on a prompt nobody can answer. In a non-interactive
    // shell (no TTY — a background task, CI, or piped stdin) `stdin.read_line`
    // blocks forever on an open-but-empty socket (the observed `aida doctor
    // --heal` hang: 7.5min, 0 progress, WCHAN unix_stream_read_generic).
    // Decline fast with guidance instead; `--heal --yes` skips this prompt
    // entirely (the caller only calls us when !opts.yes). trace:BUG-407
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "  {} non-interactive shell — skipping '{}' ({} finding(s)). Re-run \
             `aida doctor --heal --yes` (add --force for destructive categories) \
             to apply without prompting.",
            "Note:".yellow().bold(),
            category,
            count
        );
        return Ok(false);
    }
    // trace:STORY-809 | ai:claude
    let card = crate::context_prompt::ContextCard {
        decision: format!("whether doctor may heal the '{category}' findings"),
        provenance: vec![format!(
            "`aida doctor` detected {count} finding(s) in category '{category}'"
        )],
        answers: vec![
            format!("y: apply the '{category}' heal to all {count} finding(s) now"),
            "n: skip this category (findings remain; re-run doctor any time)".to_string(),
        ],
        recommended_default:
            "y for hygiene categories (stale leases, dead registrations); read the findings first for destructive ones"
                .to_string(),
    };
    crate::context_prompt::confirm_with_context(
        &format!("Heal {count} finding(s) in {category}?"),
        false,
        &card,
    )
}

fn heal_doctor_finding(
    project_root: &std::path::Path,
    finding: &DoctorFinding,
    opts: &DoctorRunOptions,
) -> Result<DoctorHealResult> {
    match finding.category.as_str() {
        "stale-leases" | "abandoned-leases" | "stale-reviewer-leases" => {
            heal_doctor_lease(project_root, finding)
        }
        "brief-spec-drift" | "OBE-briefs" => heal_doctor_brief(finding),
        "spec-status-drift" if finding.action.starts_with("confirm and end lease") => {
            heal_doctor_lease(project_root, finding)
        }
        "spec-status-drift" => heal_doctor_spec_status(project_root, finding),
        "orphan-worktrees" => heal_doctor_orphan_worktree(project_root, finding),
        // TASK-752: git-rm the tracked legacy-store artifact + gitignore it —
        // exactly PR-651's resolution. safe_heal (the detector self-gates on
        // git-canonical mode, so this never touches an active centralized
        // backend). trace:TASK-752 | ai:claude
        "legacy-store-cruft" => heal_doctor_legacy_store_cruft(project_root, finding),
        // BUG-563: git-rm --cached the per-clone runtime file in the STORE
        // worktree + gitignore it + commit on the store worktree. safe_heal (the
        // detector self-gates on git-canonical mode + an attached store
        // worktree). trace:BUG-563 | ai:claude
        "store-tracked-runtime" => heal_doctor_store_tracked_runtime(project_root, finding),
        // trace:TASK-1547 | ai:codex
        "store-staging-ignore" => heal_doctor_store_staging_ignore(project_root, finding),
        "orphan-queue-entries" => heal_doctor_orphan_queue_entry(project_root, finding),
        "stale-locks" => heal_doctor_stale_lock(finding),
        // trace:BUG-2211 | ai:claude
        "sandbox-config-lock" => heal_doctor_sandbox_config_lock(project_root, finding),
        // trace:TASK-1595 | ai:claude
        "stranded-commits" => {
            let outcome = crate::stranded_commits::heal(project_root, &finding.id);
            Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: outcome.action,
                status: if outcome.healed { "healed" } else { "skipped" }.to_string(),
                detail: outcome.detail,
            })
        }
        "dead-agents" => heal_doctor_dead_agent(project_root, finding),
        // STORY-1128: permission-posture heal applies the contained profile
        // through the same merge-preserving writer as `config permissions set`.
        // trace:STORY-1128 | ai:codex
        // BUG-1929: the contained store-boundary findings are report-only.
        // trace:BUG-1929 | ai:claude
        "permission-posture" if finding.id.starts_with("contained-store-boundary") => {
            Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: finding.action.clone(),
                status: "skipped".to_string(),
                detail: Some("not auto-healed; follow the action".to_string()),
            })
        }
        "permission-posture" => heal_doctor_permission_posture(project_root, finding),
        "parent-tag-drift" => heal_doctor_parent_tag_drift(project_root, finding),
        "orphan-branches" if opts.force && opts.yes => {
            heal_doctor_orphan_branch(project_root, finding)
        }
        // TASK-717: prune a verified-stale REMOTE branch. DESTRUCTIVE (deletes
        // origin/*) so force+yes gated like local orphan-branch deletion.
        // trace:TASK-717
        "stale-remote-branches" if opts.force && opts.yes => {
            heal_doctor_stale_remote_branch(project_root, finding)
        }
        // TASK-878: GC a merged Agent-tool worktree + its branch. DESTRUCTIVE
        // (worktree removal + branch deletion) so force+yes gated like the
        // stale-remote-branch deletion above, AND routed through the STORY-666
        // autonomous-context refusal in heal_doctor_findings (safe_heal=false).
        // trace:TASK-878
        "merged-agent-worktrees" if opts.force && opts.yes => {
            heal_doctor_merged_agent_worktree(project_root, finding)
        }
        // TASK-673 / BUG-1637: the heal reports and refuses; it never reopens
        // the Completed spec (see heal_doctor_completed_without_commit).
        // Still force-gated (see heal_doctor_findings).
        "completed-without-commit" if opts.force && opts.yes => {
            heal_doctor_completed_without_commit(project_root, finding)
        }
        // TASK-699: remove a stray ancestor instruction file whose @-imports
        // escape the project. DESTRUCTIVE — it deletes a file OUTSIDE the repo —
        // so it's force+yes gated (see heal_doctor_findings's safe-heal gate).
        // trace:TASK-699
        "external-import-bleed" if opts.force && opts.yes => {
            heal_doctor_external_import_bleed(project_root, finding)
        }
        _ => Ok(DoctorHealResult {
            category: finding.category.clone(),
            id: finding.id.clone(),
            action: finding.action.clone(),
            status: "skipped".to_string(),
            detail: Some("diagnostic-only category; follow the printed action".to_string()),
        }),
    }
}

fn heal_doctor_brief(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    if !path.exists() {
        let acked = path.with_file_name(format!(
            "{}.acked",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
        ));
        if acked.exists() {
            return Ok(DoctorHealResult {
                category: finding.category.clone(),
                id: finding.id.clone(),
                action: "brief already acked".to_string(),
                status: "skipped".to_string(),
                detail: Some(acked.display().to_string()),
            });
        }
    }
    ack_agent_brief(path)?;
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "acked obsolete brief".to_string(),
        status: "healed".to_string(),
        detail: None,
    })
}

/// Remove a cache lock-info file only when its recorded owner is provably dead
/// (PID gone, or PID reused by a different process). Re-verified at heal time
/// with a compare-and-delete, so a finding that went stale between detection
/// and heal (or a live owner past its expected duration) is never deleted.
// trace:TASK-1484 | ai:claude
fn heal_doctor_stale_lock(finding: &DoctorFinding) -> Result<DoctorHealResult> {
    let path = std::path::Path::new(&finding.id);
    let outcome = aida_core::reclaim_dead_lock_info(path)?;
    let (status, detail) = match outcome {
        aida_core::LockInfoReclaim::Removed { .. } => ("healed", None),
        aida_core::LockInfoReclaim::Absent => (
            "skipped",
            Some("lock-info file was already gone".to_string()),
        ),
        // trace:BUG-HOME-041 | ai:claude
        aida_core::LockInfoReclaim::HolderOnDisk { pid } => (
            "skipped",
            Some(format!(
                "pid {pid} is waiting on disk (state D), not stuck; left in place. \
                 Retry when IO pressure drops"
            )),
        ),
        aida_core::LockInfoReclaim::Kept(Some(owner)) if owner.presumed_alive() => (
            "skipped",
            Some("lock owner is still alive (or cannot be verified); left in place".to_string()),
        ),
        aida_core::LockInfoReclaim::Kept(_) => (
            "skipped",
            Some("lock-info changed or could not be parsed; left in place".to_string()),
        ),
    };
    Ok(DoctorHealResult {
        category: finding.category.clone(),
        id: finding.id.clone(),
        action: "removed stale cache lock-info file".to_string(),
        status: status.to_string(),
        detail,
    })
}

#[cfg(test)]
#[path = "tests/doctor_cmd_story_462_doctor_tests.rs"]
mod story_462_doctor_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_bug_1505_review_verdict_doctor_tests.rs"]
mod bug_1505_review_verdict_doctor_tests;

/// Diagnose and heal AIDA multi-agent state drift. Report-only by
/// design: exit 0 means the scan RAN, not that it found nothing —
/// findings print as text/JSON either way. A caller that needs a
/// pass/fail signal (e.g. a CI gate or scheduled job) opts in with
/// `doctor check <category> --fail-on-findings`, which exits non-zero
/// only when that one category has findings.
// trace:EPIC-19 trace:STORY-462
#[derive(clap::Parser, Debug)]
#[clap(name = "doctor")]
pub(crate) struct DoctorArgs {
    /// Check this machine before running automated lanes.
    // trace:STORY-2642 | ai:codex
    #[clap(long, conflicts_with_all = ["lane_capacity", "heal", "fix_sandbox", "contradictions", "yes", "category", "force", "all", "since", "dry_run", "mapping", "tree_only", "preserve", "contradictions_limit", "contradictions_offset"])]
    pub lane_host: bool,

    /// Report the host codegraph snapshot age as seen from this lane.
    // trace:STORY-2630 | ai:claude
    #[clap(long, conflicts_with_all = ["lane_host", "heal", "fix", "fix_sandbox", "contradictions"])]
    pub lane_codegraph: bool,

    /// Apply safe fixes after scanning. Without this, doctor is read-only.
    #[clap(long)]
    pub heal: bool,

    /// Skip per-category confirmation prompts while healing.
    #[clap(long, short = 'y')]
    pub yes: bool,

    /// Restrict scan/heal to one category.
    #[clap(long, value_name = "CATEGORY")]
    pub category: Option<String>,

    /// Emit machine-readable JSON.
    #[clap(long)]
    pub json: bool,

    /// Read-only typed local lane capacity and reservations (not readiness admission).
    // trace:STORY-2660 | ai:codex
    #[clap(long, conflicts_with_all = ["heal", "fix", "fix_sandbox", "contradictions", "category"])]
    pub lane_capacity: bool,

    /// Permit riskier destructive fixes such as branch deletion.
    #[clap(long)]
    pub force: bool,

    /// Show older completed-without-commit findings individually.
    #[clap(long)]
    pub all: bool,

    /// Exempt specs completed before this point from the
    /// completed-without-commit integrity check: a relative duration
    /// (`30d`, `2w`, `24 hours ago`), an ISO date (local midnight), a
    /// zone-less ISO datetime (local time), RFC3339, or else a git
    /// ref/tag whose commit date is the cutoff. An invalid value is an
    /// error. Quiets noise on legacy history predating trace
    /// conventions. Falls back to the AIDA_DOCTOR_COMPLETED_SINCE env var.
    // trace:TASK-673 | ai:claude
    // trace:TASK-1509 | ai:claude
    // trace:BUG-1622 | ai:claude
    #[clap(long, value_name = "DATE_OR_REF")]
    pub since: Option<String>,

    /// Print the guided, copy-pasteable steps to bring the OS sandbox
    /// (bubblewrap write-confinement) up on this host: detect the current
    /// state, the exact sysctl to persist if the kernel blocks unprivileged
    /// user namespaces, how to opt in, and the verify command. Prints the
    /// sequence — sudo steps are clearly marked "run this yourself"; it
    /// never silently runs sudo. Runs the non-sudo availability re-probe as
    /// a smoke check.
    // trace:STORY-665 | ai:claude
    #[clap(long)]
    pub fix_sandbox: bool,

    /// Repair one category of store damage that needs a data migration.
    /// `id-collisions`: spec IDs that differ only by leading zeros are the
    /// same ID, so each collision keeps one spec on the number and gives the
    /// others a fresh ID, rewriting their mentions in the store, docs and
    /// code comments. `agreed-ids`: give every node-local spec that has no
    /// agreed ID the next agreed number of its type; local IDs keep
    /// resolving. Needs --dry-run (preview) or --yes (apply).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
    #[clap(long, value_name = "CATEGORY", num_args = 0..=1, default_missing_value = "lane-host", value_parser = ["id-collisions", "agreed-ids", "lane-host"])]
    pub fix: Option<String>,

    /// With --fix: show the plan and write nothing to the store.
    #[clap(long, requires = "fix")]
    pub dry_run: bool,

    /// With --fix id-collisions: the old -> new mapping file. With --dry-run
    /// a missing file is created from the plan; otherwise the file's mapping
    /// is applied exactly as written.
    #[clap(long, value_name = "FILE", requires = "fix")]
    pub mapping: Option<std::path::PathBuf>,

    /// With --fix id-collisions --mapping: rewrite only this checkout's
    /// tracked files (docs, changelog, code comments), not the store.
    #[clap(long, requires = "mapping")]
    pub tree_only: bool,

    /// With --fix id-collisions: leave this spec's own text (title,
    /// description, comments) as written when applying to the store, e.g. the
    /// spec recording the migration's old -> new table. Repeatable.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    #[clap(long, value_name = "SPEC", requires = "fix", value_parser = crate::spec_arg::spec_id)]
    pub preserve: Vec<String>,

    /// Run a store-wide semantic contradiction sweep combining mechanical joins
    /// in Rust with Jev System One semantic choice queries.
    // trace:STORY-1426 | ai:antigravity
    #[clap(long)]
    pub contradictions: bool,

    /// Maximum contradiction findings to print per page. The default is
    /// deliberately bounded; use --contradictions-offset to continue or
    /// --all to request the complete scan.
    // trace:STORY-1426 | ai:codex
    #[clap(long, value_name = "N", default_value_t = 25)]
    pub contradictions_limit: usize,

    /// Zero-based offset into the deterministically ordered contradiction
    /// findings. Ignored with --all.
    // trace:STORY-1426 | ai:codex
    #[clap(long, value_name = "N", default_value_t = 0)]
    pub contradictions_offset: usize,

    /// Legacy maintenance subcommand or focused doctor action.
    #[clap(subcommand)]
    pub cmd: Option<crate::cli::DoctorCommand>,
}

// Doctor commands run before storage init — they may need to operate
// on broken or partially-migrated stores. trace:EPIC-19 | ai:claude
fn run_doctor(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> anyhow::Result<()> {
    let args = <DoctorArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let DoctorArgs {
        lane_host,
        lane_codegraph,
        heal,
        yes,
        category,
        json,
        force,
        all,
        since,
        fix_sandbox,
        fix,
        dry_run,
        mapping,
        tree_only,
        preserve,
        contradictions,
        contradictions_limit,
        contradictions_offset,
        lane_capacity,
        cmd,
    } = &args;
    // trace:STORY-2642 | ai:codex
    if *lane_host {
        if fix.as_deref().is_some_and(|v| v != "lane-host") || *dry_run || cmd.is_some() {
            anyhow::bail!("use `aida doctor --lane-host [--fix]` for host setup");
        }
        let root = crate::main_worktree_root_from(&crate::find_project_root()?);
        return crate::lane_host::run(&root, fix.is_some(), *json);
    }
    if fix.as_deref() == Some("lane-host") {
        anyhow::bail!("bare --fix requires --lane-host; store repairs require a category");
    }
    // No store refresh, host mutation or arbitrary subprocess for this report.
    // trace:STORY-2630 | ai:codex
    if *lane_codegraph {
        anyhow::ensure!(
            cmd.is_none() && category.is_none(),
            "--lane-codegraph is a standalone report"
        );
        let root = crate::find_project_root()?;
        let report = crate::codegraph::lane_host_report(&root)?;
        if *json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "codegraph host snapshot: {}s old; captured {}s ago; host observation {}s ago; {}",
                report["snapshot_age_seconds"],
                report["snapshot_captured_age_seconds"],
                report["host_observation_age_seconds"],
                report["index_access"].as_str().unwrap_or("unknown")
            );
            if !report["refresh_error"].is_null() {
                println!("snapshot refresh error: {}", report["refresh_error"]);
            }
            if !report["query_error"].is_null() {
                println!("query service unreachable: {}", report["query_error"]);
            }
        }
        return Ok(());
    }
    // trace:STORY-2660 | ai:codex
    if *lane_capacity {
        anyhow::ensure!(
            cmd.is_none(),
            "--lane-capacity cannot be combined with a doctor subcommand"
        );
        return crate::standalone_implementer::doctor_host_status(&find_project_root()?, *json);
    }
    // trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
    if let Some(category) = fix.as_deref() {
        if *json {
            anyhow::bail!("`aida doctor --fix {category}` has no JSON projection");
        }
        // trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
        if category == "agreed-ids" {
            if *tree_only || !preserve.is_empty() {
                anyhow::bail!("--tree-only and --preserve apply only to --fix id-collisions");
            }
            return self::doctor_fix_agreed_ids(&crate::doctor_backfill_agreed::BackfillOptions {
                dry_run: *dry_run,
                yes: *yes,
                mapping: mapping.clone(),
            });
        }
        return self::doctor_fix_id_collisions(&crate::doctor_renumber::FixOptions {
            dry_run: *dry_run,
            yes: *yes,
            mapping: mapping.clone(),
            tree_only: *tree_only,
            preserve: preserve.clone(),
        });
    }
    // STORY-665: `aida doctor --fix-sandbox` is a guided printer for bringing
    // bwrap OS-confinement up on a new machine. Short-circuit before the
    // multi-agent drift scan — it's a standalone setup helper, not a check.
    // trace:STORY-665 | ai:claude
    if *fix_sandbox {
        return self::doctor_fix_sandbox();
    }
    // STORY-1426: `aida doctor --contradictions` runs the store-wide semantic
    // contradiction sweep combining mechanical joins with Jev choice queries.
    // trace:STORY-1426 | ai:antigravity
    if *contradictions {
        return self::doctor_contradictions(
            *json,
            *contradictions_limit,
            *contradictions_offset,
            *all,
        );
    }
    return self::handle_doctor_command(
        *heal,
        *yes,
        category.as_deref(),
        *json,
        *force,
        *all,
        since.as_deref(),
        cmd.as_ref(),
    );
}

// trace:STORY-1728 | ai:claude
pub(crate) static DOCTOR: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "doctor",
    help_after: "driver",
    command: <DoctorArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_doctor),
};

#[cfg(test)]
#[path = "tests/bug_1985_destructive_gate_tests.rs"]
mod bug_1985_destructive_gate_tests;

#[cfg(test)]
#[path = "tests/task_1981_writer_tests.rs"]
mod task_1981_writer_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_loose_objects_followup_tests.rs"]
mod loose_objects_followup_tests;

// trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
#[cfg(test)]
#[path = "tests/bug_home_036_bare_id_tests.rs"]
mod bug_home_036_bare_id_tests;

#[cfg(test)]
#[path = "tests/doctor_cmd_story_2326_doctor_tests.rs"]
mod story_2326_doctor_tests;

// trace:BUG-2486 | ai:codex
#[cfg(test)]
#[path = "tests/doctor_cmd_bug_2486_tests.rs"]
mod bug_2486_tests;

// trace:BUG-2484 | ai:codex
#[test]
fn bug_2484_doctor_renders_resolved_binary_diagnostics() {
    let mut report = DoctorReport::from_findings(Vec::new());
    let diagnostic =
        "Codex resolved binary: /fixture/vendor/bin/codex (ChatGPT SHA-256 pin matches)";
    report.vendor_binaries.push(diagnostic.into());
    assert_eq!(
        serde_json::to_value(&report).unwrap()["vendor_binaries"][0],
        diagnostic
    );
    let toon = render_doctor_report_toon(&report);
    assert!(toon.contains("vendor_binaries"));
    assert!(toon.contains(diagnostic));
    assert_eq!(report.total, 0);
}

#[path = "doctor/store_repair.rs"]
mod store_repair;
use store_repair::*;

#[path = "doctor/worktrees.rs"]
mod worktrees;
use worktrees::*;

#[path = "doctor/disk.rs"]
mod disk;
use disk::*;

#[path = "doctor/remote.rs"]
mod remote;
use remote::*;

#[path = "doctor/sandbox.rs"]
mod sandbox;
use sandbox::*;

#[path = "doctor/vendors.rs"]
mod vendors;
use vendors::*;

#[path = "doctor/report.rs"]
mod report;
use report::*;

pub(crate) use remote::scan_project_manifest;

pub(crate) use store_repair::{
    ambiguous_bare_id_lines, doctor_contradictions, ensure_store_tracked_runtime_gitignore,
    heal_doctor_parent_tag_drift, heal_doctor_spec_status, history_index_doctor_line,
};

pub(crate) use worktrees::{
    branch_content_fully_landed, branch_paths_match_default, classify_agent_worktree,
    claude_launch_active, heal_doctor_orphan_worktree, post_ship_gc_summary, AgentWorktreeFacts,
    AgentWorktreeScanLimits, AgentWorktreeVerdict, POST_SHIP_GC_BUDGET,
};

pub(crate) use sandbox::{doctor_fix_sandbox, heal_doctor_sandbox_config_lock};

pub(crate) use disk::{
    disk_headroom_light_report, disk_headroom_min_free_gib, DISK_HEADROOM_CATEGORY,
};

pub(crate) use worktrees::scan_merged_agent_worktrees_with_count;

#[cfg(test)]
pub(crate) use report::{bug_1745_test_envelope, bug_1745_test_report};

pub(crate) use report::{
    binary_breakdown_phrase, doctor_lane_tests_note, doctor_render_mode_from,
    perf_gate_scope_disabled_by, performance_breaches, performance_budgets, performance_findings,
    performance_policy, performance_unobserved, render_doctor_report_toon, resolve_binary_lineage,
    tally_by_binary, BinaryLineage, BinaryTally, DoctorHealResult, DoctorRenderMode, DoctorReport,
    PerformanceBreach, PerformanceBudget, PerformancePolicy, UnobservedBudget,
    PERF_GATE_ALL_BINARIES_ENV, UNKNOWN_BINARY_LABEL,
};
