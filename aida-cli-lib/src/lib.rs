// trace:TASK-3429 | ai:codex
#![allow(warnings)]
#![allow(clippy::all)]
#![allow(clippy::doc_lazy_continuation)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::useless_format)]
#![allow(clippy::question_mark)]
#![allow(clippy::type_complexity)]
#![allow(clippy::redundant_closure)]
// The MCP `tool_descriptors()` json! macro expands deeply; the default
// recursion limit (128) is exceeded once new tool properties are added.
// trace:STORY-639 | ai:claude
#![recursion_limit = "256"]

// Crate-local `print!` / `println!` shadow the prelude macros for every module
// declared below (textual macro scope), so each stdout write through them is
// tracked: the dispatcher's agent-mode `cache:` footer then starts on its own
// line even when a command body ended mid-line. Both delegate to std's own
// macros, so locking, line buffering, test capture and the panic on a failed
// write are unchanged. trace:BUG-1975 | ai:claude
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::cache_output::print_tracked(::std::format_args!($($arg)*))
    };
}
// trace:BUG-1975 | ai:claude
macro_rules! println {
    () => {
        $crate::cache_output::println_tracked(::std::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::cache_output::println_tracked(::std::format_args!($($arg)*))
    };
}

// trace:STORY-1741 | ai:claude
mod adoption_cmd;
mod advisor;
mod advisor_cmd_root;
mod advisor_code_gate;
mod advisor_watch;
mod agent_config;
mod agent_launch;
mod agent_launch_prompt;
pub mod agent_registry;
mod agent_registry_root;
mod aida_bin;
mod alias;
mod approvals_cmd;
mod archive_cmd;
mod assign_cmd;
mod auto_complete;
mod auto_complete_telemetry;
mod autonomy_cmd;
mod autopilot;
mod launch_permission_profile;
// Disabled host-only transport; no command or consumer is registered.
// trace:TASK-3368 | ai:codex
#[cfg(target_os = "linux")]
mod proc_broker;
// trace:TASK-1018 | ai:claude — durable execution audit + one-command reversal.
mod autopilot_audit;
mod autopilot_intake;
mod awaiting_you;
mod backlog;
mod brief;
mod brief_cmd;
mod burndown;
mod idle_eligible;
// trace:TASK-1500 | ai:codex
mod burndown_run;
mod bypass_confirm;
mod cache_cmd;
pub mod cache_output;
mod calibration;
mod changelog;
mod changelog_cmd;
mod ci_idle_timeout;
mod ci_probe;
pub mod claim;
mod claude_agents;
mod cli;
#[cfg(feature = "remote")]
mod client;
// trace:TASK-3425 | ai:codex
// Direct framework imports are also used by the MCP transport crate.
// trace:TASK-3448 | ai:codex
pub mod cmdkit;
pub mod command_registry;

// Inject the production command tree and decoder at the composition root.
// Consumers use the leaf helpers without importing the registry table.
// trace:TASK-3448 | ai:codex
static CLI_SUPPORT: cmdkit::CliSupport<cli::Cli> = cmdkit::CliSupport::new(
    command_registry::built_command,
    command_registry::cli_from_matches,
);

#[cfg(test)]
use CLI_SUPPORT as TEST_CLI;

mod comment_cmd;
mod commit;
mod compete;
mod compete_cmd;
mod complexity_calibration;
// trace:STORY-2402 | ai:claude
mod condition_cmd;
mod config_cmd;
mod spec_arg;
// trace:BUG-1938 | ai:claude
mod config_commit;
mod config_edit;
mod config_init;
// trace:BUG-1928 | ai:claude — contained-posture write boundary (absolute allowed roots).
mod contained_boundary;
// trace:TASK-2712 | ai:claude — native Codex launches' shared write set.
mod codex_native_sandbox;
// trace:TASK-2705 | ai:codex
mod containment;
mod context_prompt;
mod coordination;
mod criteria;
// trace:STORY-1487 | ai:claude
mod criteria_coverage;
mod criteria_gate;
mod criteria_red_run;
// trace:BUG-2380 | ai:claude
mod ci_failures;
mod db_audit_ids;
mod db_block;
mod db_cmd;
mod db_reconcile;
mod decide_cmd;
mod deep_link;
mod defer_cmd;
// trace:STORY-2402 | ai:codex
mod defer_condition_migration;
mod defer_resurface;
mod deps_cmd;
// Derived per-spec driver views + `aida driver`. trace:STORY-1653 | ai:claude
mod derisk_cmd;
mod drain_next_n;
mod driver_cmd;
mod driver_view;
// Idle in-progress specs in `aida awaiting`. trace:STORY-1680 | ai:claude
mod idle_in_progress;
// trace:STORY-1646 | ai:claude
mod dev_build;
// trace:STORY-1689 | ai:claude
mod dev_autopromote;
mod dev_cmd;
mod digest;
mod digest_cmd;
mod dispatch;
// trace:TASK-1564 | ai:claude — render/re-read a drain phase-failure note.
mod doctor_root;
mod drain_batch;
mod drain_failure_note;
mod drain_gate;
mod findings_gate;
// trace:STORY-1889 | ai:claude — gate authority, all on the mandate floor.
pub mod gate;
// trace:STORY-B6B1278890774EF993D2EBC327862470-099 | ai:claude
mod deputy;
mod deputy_cmd;
// trace:STORY-2338 | ai:claude
mod deputy_shell;
mod deputy_shell_cmd;
mod gitlab_mirror_link;
mod graph_cmd;
mod help;
mod human;
mod kernel;
// trace:STORY-1645 | ai:claude
mod faq_topics;
// trace:STORY-1642 | ai:claude
mod knowledge_list;
// trace:BUG-1944 | ai:claude — live-work evidence + start-of-work status bump.
// trace:STORY-1662 | ai:claude
pub mod lane_env;
// trace:BUG-2383 | ai:claude — the lane job runner and `aida lane job`.
mod lane_cmd;
mod lane_job;
// trace:STORY-2640 | ai:codex
mod lane_fast_gate;
mod project_checks;
// One agent launcher. trace:STORY-2326 | ai:claude
mod launch;
pub mod leases;
mod live_work;
#[allow(dead_code)] // Foundation only; launch/ownership integration is a later slice.
mod park_runtime;
pub(crate) use live_work::try_auto_queue_pr_review;
mod agent_handle;
pub mod mailbox_resolver;
mod mailbox_root;
// trace:STORY-1652 | ai:claude — terminal markdown rendering for human output.
mod markdown_render;
mod memory_doc;
mod merge_hold_cmd;
// trace:BUG-2015 | ai:claude — attested mirror-sync allowance for a project's pre-push main guard.
mod mirror_guard;
mod orchestrator_leases;
mod output_format;
mod phase_driver;
mod phase_driver_trait;
mod phase_gates;
mod plan_followups;
mod plan_intent;
mod plan_report;
// trace:STORY-2325 | ai:claude — one ownership record and one pure liveness judge.
mod ownership;
// trace:STORY-2325 | ai:claude
mod ownership_cmd;
// trace:STORY-2315 | ai:claude
mod pr_lease_takeover;
mod pr_lookup;
mod pr_merge_gate;
mod project_capabilities;
mod protocol_gate;
// trace:TASK-1090 | ai:claude — per-row dispatch-health classifier for `aida ps`.
mod dispatch_health_ps;
// trace:TASK-1092 | ai:claude — [dispatch.routing] config loader (additive, not yet wired in).
mod dispatch_routing_config;
// trace:STORY-1916 | ai:claude — disk guard emergency tier, stuck escalation, status.
mod disk_emergency;
// trace:STORY-776 | ai:claude — pure decision logic for `aida do` mode dispatch.
mod autoprogress;
mod do_dispatch;
mod doc_cmd;
mod docs;
// trace:STORY-2220 | ai:claude
mod docs_sweep;
// trace:STORY-B6B1278890774EF993D2EBC327862470-105 | ai:claude
mod doctor_backfill_agreed;
mod doctor_cmd;
mod lane_host;
// trace:BUG-B6B1278890774EF993D2EBC327862470-046 | ai:claude
mod doctor_renumber;
// trace:STORY-1462 | ai:claude — runaway-seat watchdog doctor category.
mod drain_caps;
mod drain_cmd;
mod drain_lock;
// trace:TASK-1518 | ai:claude
mod drain_signal;
mod freshness_gate;
// trace:BUG-1622 | ai:claude — keeps user-supplied refs from reading as git options.
mod git_arg_guard;
pub mod git_backend_cmd;
pub(crate) mod intent_capture;
mod machine_readiness;
mod mcp_cmd;
mod orchestrator_cmd;
mod pr_cmd;
// trace:STORY-2732 | ai:codex
mod pr_emergency_land;
pub(crate) mod pr_list;
mod pr_state;
mod protocol_cmd;
mod ps_integrate;
mod punt_done;
mod questions;
pub mod queue_cmd;
mod queue_misc;
// trace:STORY-1688 | ai:antigravity
mod lane_liveness;
// trace:TASK-2053 | ai:claude
mod loose_objects;
mod rel_cards;
mod relay_mail;
mod relay_stall;
// trace:TASK-2347 | ai:codex
mod codex_mail_health;
// trace:STORY-2630 | ai:codex
mod codegraph;
// trace:BUG-2386 | ai:claude
pub(crate) mod codex_resume;
// trace:STORY-HOME-128 | ai:claude
mod mail_backlog;
// trace:STORY-B6B1278890774EF993D2EBC327862470-100 | ai:claude
pub mod advisor_handshake;
mod remote_cmd;
pub mod requirements_cli;
mod review_cmd;
mod review_gates;
mod review_waiting;
// trace:STORY-1742 | ai:claude
#[cfg(unix)]
mod sanitized_intake;
// trace:TASK-1993 | ai:codex
mod review_implementer_gate;
mod review_watch;
mod role;
mod runaway_seats;
mod scaffold_init;
mod session_end;
mod session_misc;
mod session_start;
mod skill_cmd;
mod solo_cmd;
mod stakeholder_role;
// trace:STORY-1662 | ai:claude
mod lane_result; // trace:STORY-2690 | ai:codex
mod standalone_implementer;
mod status_awaiting;
mod status_cmd;
mod status_prs;
mod status_sections;
mod statusline_root;
// trace:STORY-B6B1278890774EF993D2EBC327862470-097 | ai:claude
mod store_local;
// trace:TASK-3223 | ai:codex
pub mod store_mutation_client;
mod store_mutation_entry;
mod store_push_pull;
pub mod store_sync;
mod supervise_cmd;
mod supervisor;
mod transport_profile;
// trace:STORY-1218 | ai:claude
mod schedule_driver;
mod shift;
// trace:STORY-1649 | ai:claude
mod shipped_linkage;
// trace:BUG-2221 | ai:claude — `aida show` negative-linkage ancestry proof.
mod show_negative;
// trace:TASK-1471 | ai:claude — linked Docs section in human `aida show`.
mod show_docs;
// trace:BUG-2221 | ai:claude — `aida show` incremental spec-id -> commit index.
mod linkage_index;
mod terminal_cmd;
// trace:TASK-1427 | ai:codex
mod token_ledger;
mod trace;
// trace:TASK-1783 | ai:claude — whole-tree `aida trace coverage`.
// trace:TASK-1783 | ai:claude — region rule shared by lint constants and tree coverage.
mod trace_regions;
mod trace_tree_coverage;
mod tui_entry;
mod upgrade_release;
mod usage_mcp_helpers;
mod worktree_gc;
mod write_provenance;
mod zen_cmd;
use advisor_cmd_root::*;
use agent_config::*;
use agent_launch::*;
use agent_registry_root::*;
use autopilot_intake::*;
use brief::*;
use burndown_run::*;
use ci_probe::*;
use claim::*;
use config_init::*;
pub use config_init::{validate_priority_input, validate_status_input};
use db_block::*;
use db_reconcile::*;
use doctor_root::*;
use drain_batch::*;
use drain_cmd::*;
use drain_gate::*;
use drain_next_n::*;
use findings_gate::*;
use help::*;
use human::*;
pub use kernel::is_terminal_status_str;
use kernel::*;
use leases::*;
use mailbox_root::*;
use mcp_cmd::*;
use memory_doc::*;
use merge_hold_cmd::*;
use orchestrator_cmd::*;
use orchestrator_leases::*;
use output_format::*;
use phase_driver::*;
use phase_driver_trait::*;
use phase_gates::*;
use plan_followups::*;
use plan_intent::*;
use plan_report::*;
use pr_cmd::*;
use pr_list::*;
use pr_lookup::*;
pub use pr_lookup::{parse_pr_arg, resolve_pr_to_spec};
use pr_merge_gate::*;
use ps_integrate::*;
use punt_done::*;
use questions::*;
use queue_cmd::*;
use queue_misc::*;
use rel_cards::*;
use requirements_cli::*;
use review_gates::*;
pub use role::AGENT_ROLES;
use role::*;
use scaffold_init::*;
use session_end::*;
use session_misc::*;
use session_start::*;
use solo_cmd::*;
use stakeholder_role::*;
use status_awaiting::*;
use status_cmd::*;
use status_prs::*;
use status_sections::*;
use statusline_root::*;
use store_push_pull::*;
use store_sync::*;
use trace::*;
use tui_entry::*;
use upgrade_release::*;
use usage_mcp_helpers::*;
use worktree_gc::*;
use zen_cmd::*;
use zen_drive_root::*;
// trace:TASK-967 | ai:claude
mod drain_summary;
// trace:STORY-1415 | ai:claude
mod field_study;
mod field_study_cmd;
mod mass_change;
// trace:SPIKE-67 | ai:claude
mod rule_violation;
// trace:STORY-656 | ai:claude
pub mod completion; // trace:STORY-1418 | ai:claude
mod drain_resume;
mod drain_state;
mod drive_robustness;
mod dryrun;
// trace:TASK-1117 | ai:claude
mod edit_buffer;
mod edit_rebase;
mod effort_calibration;
// trace:ADR-55 | ai:antigravity
pub mod evaluator;
// trace:STORY-1426 | ai:antigravity
pub mod contradictions;
// trace:EPIC-72 trace:TASK-1435 trace:TASK-1436 trace:TASK-1438 | ai:antigravity
pub mod exposition;
// trace:EPIC-72 trace:TASK-1439 | ai:antigravity
pub mod wiki;
// trace:STORY-1424 | ai:antigravity
mod event_wait;
mod events;
mod exit_signal;
mod external_import_bleed;
mod feature_cmd;
pub mod findings;
#[cfg(unix)]
mod forwarded_signals;
pub mod graded_review;
mod identity_gate;
mod implementer_preflight;
// trace:STORY-700 | ai:claude — passive first-run hint chain through the core loop.
mod first_run;
mod focus;
mod focus_cmd;
pub mod forge;
mod forge_confirmation;
mod forge_observed;

// trace:STORY-2401 | ai:codex
mod ci_digest;

mod forge_profiles;
mod gate_cmd;
// trace:STORY-1656 | ai:claude — GitLab create-if-missing + multi-hub agreement.
mod gitlab_project_create;
mod global_queue;
mod last_drain;
mod lifecycle_cmd;
mod lint_cmd;
// trace:TASK-1785 | ai:claude — `aida lint constants`.
mod lint_constants;
mod load_cmd;
mod lock_cmd;
// trace:TASK-1140 | ai:claude — STORY-711 slice 2 automatic advisor-lock gate.
mod locking_gate;
// trace:STORY-1352 | ai:codex — versioned read-only monitor API.
mod monitor_contract;
// trace:TASK-974 | ai:claude — AXI #9 lifecycle-aware next-step help block.
mod help_next;
// trace:TASK-1098 | ai:claude — clap-derived `aida help commands` catalog.
mod help_catalog;
// trace:STORY-1641 | ai:claude — git-style external `aida-<name>` subcommands.
mod external_cmd;
mod import_export_cmd;
mod import_plan_cmd;
mod interview;
mod regen; // trace:TASK-1729 | ai:codex
           // trace:STORY-633 | ai:claude — toml_edit writer for `aida config glyph`.
mod glyph_config;
pub mod glyphs;
pub mod goal_cmd;
// trace:EPIC-36 | ai:claude — session-vs-drain misclassification-gap metric.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude — resume a lane parked on its own background run.
mod headless_background;
mod headless_linger;
// trace:TASK-2838 | ai:codex
mod agy_outcome;
mod headless_tail;
mod headless_tee;
mod vendor_activity;
// trace:BUG-2396 | ai:claude — vendor-aware next-step hints.
mod vendor_hints;
// trace:STORY-2539 | ai:codex
mod vendors;
// trace:TASK-990 | ai:claude — `aida watch` streaming event classifier.
mod watch;
// trace:STORY-658 | ai:claude — `aida health` at-a-glance vital-signs read.
mod health;
mod health_cmd;
mod health_metrics;
mod health_vitals_cmd;
pub mod history;
pub mod history_layout;
// trace:TASK-1507 | ai:claude
mod history_cache;
mod history_indexer;
// trace:STORY-1650 | ai:claude — bare `aida history`: the status flow.
mod history_flow;
// trace:STORY-1478 | ai:claude — per-spec work/wait/unknown timeline.
mod history_timeline;
// trace:STORY-1664 | ai:claude — opened/completed/rejected/approved/closed list views.
mod history_views;
// trace:STORY-1479 | ai:claude — aggregate cycle-time breakdown over completed specs.
mod human_audit;
mod human_cmd;
mod metrics_cycle_time;
// trace:STORY-1480 | ai:claude — per-spec timing records published to the store.
mod timing_record;
// trace:BUG-2180 | ai:claude — commit a contained child session's store writes.
mod child_store_commit;
// trace:TASK-1150 | ai:claude — distinct-user identity guard (queue/lease mixups).
mod identity_guard;
mod init_bootstrap;
mod init_cmd;
pub mod intake;
mod integrate;
// trace:TASK-1050 | ai:claude — own-checkout guard for `aida integrate` (BUG-650).
mod ci_gate;
mod ci_inventory;
mod ci_receipt;
mod ci_relay;
mod harvest;
mod integrate_checkout;
mod integrate_view;
mod intent;
mod intent_intake;
mod internal_cmd;
mod observation_envelope;
mod review_envelope;
// Retire a tree's mail before it is removed or reused. trace:STORY-1843 | ai:claude
mod mail_retention;
mod mailbox_cmd;
pub mod mailbox_store;
// trace:STORY-1843 | ai:claude
// trace:TASK-3074 | ai:codex
mod mailbox_action;
pub mod mailbox_view;
// trace:TASK-2347 | ai:codex
mod mail_wake;
mod mailbox_wake;
mod mailbox_wake_run;
// trace:STORY-1648 | ai:claude — read-only primary-checkout drift probe.
mod main_checkout_drift;
mod maintenance_schedule;
mod manual;
#[cfg(test)]
mod mcp;
pub mod mcp_serve;
#[cfg(test)]
extern crate self as aida_cli_lib;
// Shared with the extracted MCP transport; CLI callers stay below that boundary.
// trace:TASK-3429 | ai:codex
pub mod mcp_claim;
pub mod mcp_schema;
mod mcp_translate;
mod memories_cmd;
mod merge_hold;
// trace:STORY-1698 | ai:claude — the fire-fighting mandate.
mod mandate;
mod mandate_drive;
mod mandate_gates;
mod merge_lock;
mod metrics;
mod metrics_cmd;
// trace:TASK-1951 trace:TASK-2003 | ai:claude
mod needs_attention_lens;
mod network_retry;
mod node_cmd;
mod not_found;
mod pr_claim_surface;
mod reconcile_evidence;
mod reconstitute;
// trace:STORY-1029 | ai:codex — rule-gated operator notifications.
mod notify;
// ADR-7/ADR-9 guardrail registry — consumed only by its own tests (the
// substrate-as-bouncer CI gate for the one-engine invariant), so it compiles
// in test builds only. trace:ADR-7 trace:ADR-9
#[cfg(test)]
mod orchestration_routing;
mod orchestrator;
// trace:TASK-1120 | ai:claude — opt-in `--panes` tmux hosting for fanned implementers.
mod pane_host;
// trace:STORY-647 | ai:claude — team RBAC slice 2: gated-op permission map +
// protected specs + strict mode (guardrail, not security).
mod permissions;
// TASK-1454: local runtime marker recording a seat blocked on an unanswered
// permission prompt — the Notification(permission_prompt) hook writes it,
// `aida ps` / `aida awaiting` read it.
mod pending_approval;
mod plan_cmd;
mod pr_rebase;
mod pr_ship;
mod pr_ship_batch;
mod pr_train;
mod presence;
mod presence_cmd;
mod process_probe;
pub mod process_retry;
// BUG-HOME-048: title trailers complete only the specs a PR covers.
mod trailer_coverage;
use crate::process_retry::RetryEtxtbsy;
// BUG-677: the /proc probe + lease/spec liveness classifiers moved to
// aida-core so aida-tui can compute liveness in-process. Re-export the shared
// classifier types + fns at the crate root so `crate::LeaseState` /
// `classify_lease_state` etc. keep resolving for every existing call site.
// trace:BUG-677 | ai:claude
pub(crate) use aida_core::liveness::{
    classify_lease_state, classify_spec_liveness, classify_stale_lease_recovery, LeaseState,
    SpecLiveness, StaleLeaseRecovery,
};
mod native_publication;
// trace:TASK-3325 | ai:codex
mod prompt_select;
mod prompts;
mod ship;
mod ship_from_seat;
// trace:STORY-384 | ai:claude — pure recovery-action decision for `queue recover`.
pub mod punt;
mod queue_recover;
// NeedsAttention -> back-in-flight: shelve-marker clearing + requeue hint.
// trace:TASK-1311 | ai:claude
pub mod requeue;
// Read-side fallback so a `--for <role>` routing written into one user's queue
// file is visible to whoever actually wears that role. trace:BUG-774 | ai:claude
mod queue_role_fallback;
mod rebase_cmd;
mod record_cmd;
mod rel_def_cmd;
mod related_edge_migration;
mod relationship_cmd;
// trace:STORY-452 | ai:claude — inferred recent-remote-agent-activity for `aida status`.
mod remote_activity;
// trace:STORY-537 | ai:claude — guided origin bootstrap for `aida remote create`/`attach`.
mod remote_create;
mod report_cmd;
// trace:STORY-568 | ai:claude — pure core of the research/spike dispatch lane.
mod research;
// trace:BUG-b6b1278890774ef993d2ebc327862470-014 | ai:claude
mod reviewer_preflight;
mod reviewer_summary;
// trace:STORY-1417 | ai:claude
mod review_classes;
// trace:STORY-2698 | ai:codex
mod review_risk;
// trace:STORY-1889 | ai:claude — verdict rendering, outside the gate.
mod review_verdict_display;
// trace:BUG-B6B1278890774EF993D2EBC327862470-057 | ai:claude — what a rework pickup prompt leads with.
mod rework_direction;
mod role_cmd;
mod rules_cmd;
mod rules_sync;
mod sandbox_cmd;
// trace:BUG-2211 trace:TASK-2270 | ai:claude — sandbox-left config.lock doctor check.
mod sandbox_config_lock;
// trace:TASK-1595 | ai:claude
mod stranded_commits;
// trace:BUG-b6b1278890774ef993d2ebc327862470-001 | ai:claude
mod allocation_health;
// trace:TASK-2324 | ai:codex
mod rustc_cache;
mod scaffold_cmd;
mod scaffold_refresh;
// trace:TASK-1307 | ai:claude — the pre-BUG-1452 stranded-refusal sweep.
mod stranded_sweep;
mod sweep_test_hook;
// trace:STORY-262 | ai:claude
mod schedule;
mod schedule_cmd;
mod schedule_ledger;
mod schedule_predicate;
pub mod schema;
// trace:BUG-1934 | ai:claude
mod codex_tool_env;
mod grant_keyring;
// trace:BUG-2169 | ai:claude — a contained seat's private clone (the only host accessor).
mod seat_clone;
// trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude — pre-trust seat worktrees in ~/.claude.json.
mod claude_trust;
mod seat_occupancy;
// trace:BUG-2014 | ai:codex
mod grant_expiry_notice;
mod seat_rotation;
// trace:BUG-2383 | ai:claude
mod seat_target_seed;
// trace:BUG-2383 | ai:claude
#[cfg(unix)]
mod seat_target_overlay;
// trace:STORY-B6B1278890774EF993D2EBC327862470-101 | ai:claude
mod agent_rotation;
mod seats;
mod server_cmd;
mod session;
// trace:STORY-993 | ai:claude
mod session_liveness;
pub mod session_manifest;
mod session_misc_cmd;
// trace:STORY-B6B1278890774EF993D2EBC327862470-098 | ai:claude
mod whoami_view;
// trace:TASK-1177 | ai:claude — the substrate-state reap pass for sessions
// whose spec is finished, branch merged, and process exited.
mod session_reap;
// trace:STORY-2351 | ai:claude — the hourly reap + agent gc + idle-cache reclaim pass.
mod session_hygiene;
// trace:TASK-1171 | ai:claude — the dedicated marker-delimited channel the
// `aida()` wrapper evals, so ordinary stdout is never an eval candidate.
mod shell_eval;
// trace:STORY-627 | ai:claude — per-repo solo-loop lock + liveness sentinel.
mod solo_lock;
mod stacks;
mod state_snapshot;
mod status_cleanup;
mod status_display;
// trace:STORY-715 | ai:claude
mod statusbar_cmd;
// trace:TASK-1479 | ai:claude — shared statusline contract (stable AIDA
// fields vs client-live fields) + one formatter every client adapter calls.
mod statusline_contract;
// trace:TASK-1479 | ai:claude — thin client adapters: stdin JSON -> live fields.
mod statusline_agy_adapter;
mod statusline_claude_adapter;
// trace:TASK-1167
mod statusline_cmd;
mod store_cmd;
mod tail_cmd;
// trace:STORY-640 | ai:claude — team roster + distinct-identity guard + onboarding.
mod team;
mod team_cmd;
#[cfg(test)]
mod test_env;
#[cfg(test)]
mod test_exec;
// trace:BUG-2191 | ai:claude — fixture git without detached auto-maintenance.
#[cfg(test)]
mod test_git;
// trace:BUG-1642 | ai:claude — lib tests run under a temp HOME, never the real ~/.aida.
#[cfg(test)]
mod test_home;
// trace:BUG-1730 | ai:claude — wall-clock budgets judged against host load.
#[cfg(test)]
mod test_timing;
// trace:TASK-964 | ai:claude — TOON agent-output encoder (token-efficient).
mod toon;
mod trace_cmd;
mod tracker_cmd;
mod triage_cmd;
// trace:TASK-661 | ai:claude
mod triage_lease;
// trace:TASK-969 | ai:claude — trust boundary for code-executing project config.
mod trusted_config;
mod type_cmd;
mod ultraplan_cmd;
mod upgrade_cmd;
pub mod usage;
mod usage_cmd;
// trace:STORY-1673 | ai:codex
mod verify_cmd;
// trace:TASK-877 | ai:claude — user/project-defined `aida` command aliases.
mod user_alias;
pub mod worker;
mod workflow_hints;
// trace:STORY-716 | ai:claude — `aida worktree` namespace (EPIC-55 workspace layer).
mod worktree;
// trace:TASK-634 | ai:claude — pure WorktreeCreate/Remove payload → lease record.
mod worktree_lease;
// trace:TASK-1222 | ai:codex — dry-run-first seat/worktree lifecycle GC.
mod worktree_lifecycle_gc;
// trace:TASK-1562 | ai:claude — `aida worktree reclaim`: stale target/ cache reclaim.
mod worktree_reclaim;
// trace:TASK-2325 | ai:claude — agent scratch outside target/ (job tmp, session scratch, build caches).
mod scratch_reclaim;
// trace:STORY-711 | ai:claude — advisor-directed worktree lock (`aida lock`), slice 1.
mod worktree_lock;
// trace:TASK-1178 | ai:claude — warn-only commit-boundary worktree-scope guard.
mod worktree_scope_gate;
mod zen;
// trace:STORY-721 | ai:claude — `aida zen <spec>` autonomous implement+ship drive.
mod zen_drive;
mod zen_drive_root;

// Named modules retain the existing crate-root helper paths.
// trace:TASK-3428 | ai:codex
mod retire_legacy;
pub(crate) use retire_legacy::handle_retire_legacy_ids;
mod glossary_render;
pub(crate) use glossary_render::{find_uncovered_completed_specs, render_discipline_glossary};
mod role_file_helpers;
pub(crate) use role_file_helpers::{is_toml_kv_line, picker_terminal_width, ACTIVITY_MAX};
mod human_route;
mod mailbox_errors;
pub(crate) use human_route::{
    human_route_is_open, human_routed_spec_ids, is_human_route, HUMAN_ROUTE,
};
mod store_drift;
pub(crate) use store_drift::{
    current_store_head_sha, paired_store_sha_for_head, print_status_store_drift_section, short_sha,
    store_drift_verdict, store_sha_is_ancestor, StoreDriftVerdict,
};
mod launch_defaults;
pub(crate) use launch_defaults::{
    apply_agent_default_flags, gate_agent_bypass, refuse_uncontainable_agent_tool,
    ResolvedClaudeLaunch,
};
mod merge_hold_output;
pub(crate) use merge_hold_output::print_merge_hold_no_forge;
mod history_reporting;
pub(crate) use history_reporting::{
    ambiguous_id_in_chain, history_kind_report, record_ambiguous_id_refusal,
};
mod pull_lines;
pub(crate) use pull_lines::{pull_code_start_line, pull_skip_warning, pull_store_start_line};
mod text_truncate;
#[cfg(test)]
pub(crate) use launch_defaults::write_global_agents_posture;
pub(crate) use text_truncate::truncate_str;
#[cfg(test)]
mod regression_tests;
#[cfg(test)]
mod role_picker_tests;
#[cfg(test)]
mod shell_quote_tests;
pub mod worktree_dismiss;

mod scoped_lease;
pub(crate) use scoped_lease::spec_scoped_lease;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::IsTerminal;
use uuid::Uuid;

// Named modules retain the existing crate-root helper paths.
// trace:TASK-3428 | ai:codex
mod glyph_render;
pub(crate) use glyph_render::glyph;
mod cli_entry;
pub use cli_entry::main_entry;
pub(crate) use cli_entry::{install_sigpipe_handler, run, SoftSignpostShown, TypedPayloadEmitted};
mod history_filter;
pub(crate) use history_filter::history_should_exclude_meta;
mod output_policy;
pub(crate) use output_policy::{
    apply_color_mode, enforce_json_format_capability, json_capability_gate,
    non_interactive_confirm, non_interactive_confirm_from, resolve_agent_mode,
    review_may_launch_reviewer,
};
mod list_render;
pub use list_render::format_weight;
pub(crate) use list_render::{
    agent_list_default_cap, agent_list_truncation_header, build_list_fields_lines,
    build_list_fields_lines_with, empty_list_hint_line, list_field_header, list_fields_help,
    list_hidden_hint_lines, render_search_fields, resolve_agent_list_limit, resolve_list_sort,
    search_fields_help, toon_status_token, TOON_LIST_DEFAULT_FIELDS, TOON_LIST_FIELD_ALIASES,
    TOON_LIST_KNOWN_FIELDS,
};
mod agent_errors;
pub(crate) use agent_errors::{
    agent_error_block, agent_error_summary_help, extract_aida_suggestion,
};
mod cast_recording;
pub(crate) use cast_recording::{
    asciinema_available, asciinema_command_slug, default_cast_title, derive_semantic_title,
    derive_spec_aware_slug, maybe_run_asciinema_wrapper, resolve_cast_output_path,
    shell_command_for_asciinema, shell_quote, slugify_str, strip_asciinema_wrapper_args,
    truncate_asciinema_slug,
};
mod argv_rewrite;
pub(crate) use argv_rewrite::{
    bare_aida_action, bare_aida_picker_argv, builtin_alias_rewrites,
    cwd_in_initialized_aida_project, parse_help_commands_args, project_is_aida_initialized,
    rewrite_advisor_assess, rewrite_agent_default_new, rewrite_groom_alias, rewrite_list_alias,
    rewrite_personal_view_alias, rewrite_queue_default_list, rewrite_type_alias, BareAidaAction,
    ListLensAlias, LIST_LENS_ALIASES,
};
mod review_helpers;
pub(crate) use review_helpers::{parse_review_mode, pr_head_sha_best_effort, read_review_mode};
mod finding_queue;
#[cfg(test)]
pub(crate) use finding_queue::queue_promoted_finding;
mod gitignore_entries;
pub(crate) use gitignore_entries::add_aida_gitignore_entries;
mod store_write_policy;
pub(crate) use store_write_policy::command_triggers_per_write_auto_push;
mod status_policy;
pub(crate) use status_policy::{
    approval_forbidden_for_type, manual_epic_status_edit_forbidden, queue_at_filing_refusal,
    resolve_human_only, status_requires_advisor_authority, validate_status_input_for_type,
    zen_auto_approve_authorized, QueueAtFilingRefusal, VALID_STATUS_INPUTS,
};
pub use status_policy::{
    queue_done_lifecycle_refusal, queue_done_lifecycle_refusal_message,
    status_advance_requires_advisor_authority, QueueDoneLifecycleRefusal,
};
mod ci_staleness;
pub(crate) use ci_staleness::{
    classify_changed_files, is_script_path, is_test_path, is_workflow_path, pr_stale_check,
    pr_stale_check_warning, pr_triggered_workflow_files, script_referenced_by_pr_workflows,
    stale_check_branch_ref, workflow_is_pr_triggered, StaleCheckWarning,
};
mod ci_watch;
pub(crate) use ci_watch::watch_ci_terminal;
mod bounded_process;
pub(crate) use bounded_process::{
    command_output_with_timeout_capped, command_output_with_timeout_detail,
    command_output_with_timeout_forwarding, kill_process_group, BoundedCommandOutput,
    FORGE_CLI_CALL_TIMEOUT,
};
mod forge_lookup;
pub(crate) use forge_lookup::{
    forge_lookup_output, resolve_glab_binary, with_forge_lookup_timeout, FORGE_LOOKUP_TIMED_OUT,
    FORGE_LOOKUP_TIMEOUT,
};
mod store_lookup;
pub(crate) use store_lookup::load_store_for_lookup;
mod statusline_counts;
pub(crate) use statusline_counts::{
    cross_user_role_routed_depth, read_draft_backlog_depth, read_queue_depth,
    read_urgent_unread_count, read_urgent_unread_count_for,
};
mod session_warnings;
pub(crate) use session_warnings::{
    active_reviewer_unmerged_pr, cross_role_queue_waiting, local_main_already_at_origin,
    pr_number_from_scope, pull_noop_warning, read_queue_role_counts, release_unmerged_pr_warning,
    session_end_cross_role_warning,
};
mod default_branch;
mod orchestrator_badge;
pub(crate) use default_branch::{
    resolve_default_branch_ref, resolve_default_branch_ref_until, DefaultBranchResolveTimedOut,
};
mod plan_structure;
pub(crate) use plan_structure::ULTRAPLAN_STRUCTURE;
mod worktree_dirty;
pub(crate) use worktree_dirty::working_tree_is_dirty;

use aida_core::{
    check_migration_status,
    check_scaffold_status,
    determine_requirements_path,
    forbidden_attention_transition,
    merge_agents_md_aida_block,
    seed_meta_requirements,
    // trace:TASK-331 — shared atomic-write util, promoted from BUG-228's local copy
    write_atomic,
    AgentsMdBlockMerge,
    AttentionReason,
    Comment,
    DatabaseBackend,
    FieldChange,
    FileStatus,
    // GitLab integration
    GitLabClient,
    GitLabConfig,
    IdFormat,
    IssueFilter,
    IssueState,
    MigrationCheck,
    NumberingStrategy,
    RelationshipType,
    Requirement,
    RequirementPriority,
    RequirementStatus,
    RequirementType,
    RequirementsStore,
    ScaffoldConfig,
    Scaffolder,
    Storage,
};

use crate::cli::{
    AdvisorCommand, AgentCommand, AgentNewCommand, ApprovalCommand, BacklogCommand, BlockCommand,
    BriefCommand, CacheCommand, Cli, Command, CommentCommand, ConfigCommand, DbCommand,
    DepsCommand, DevCommand, DrainCommand, FindingsCommand, FocusCommand, GitHubCommand,
    GitLabCommand, GlyphCommand, GraphCommand, HeadlessCommand, HistoryCommand, IdentityCommand,
    JiraCommand, LoadCommand, MailboxCommand, McpCommand, MemoriesCommand, NodeCommand,
    OrchestratorCommand, OutputFormat, PlanCommand, PrCommand, PuntsCommand, QuestionsCommand,
    QueueCommand, RelationshipCommand, ReleaseCommand, ReviewCommand, RoleCommand,
    RolePromptCommand, RoleScopeCommand, ScaffoldCommand, SessionCommand, SessionManifestCommand,
    SkillCommand, SoloAction, SpecCommand, StackCommand, TeamCommand, TerminalCommand,
    TraceCommand, UpgradeCommand, UsageCommand, WorkerCommand, WorktreeCommand,
    WorktreePoolCommand, ZenCommand,
};

// trace:BUG-1745 | ai:codex
#[cfg(test)]
#[path = "tests/bug_1745_error_channel_tests.rs"]
mod bug_1745_error_channel_tests;
#[cfg(test)]
#[path = "tests/bug_1749_doctor_toon_tests.rs"]
mod bug_1749_doctor_toon_tests;
#[cfg(test)]
#[path = "tests/story_737_delight_tests.rs"]
mod story_737_delight_tests;
// trace:TASK-1555 | ai:claude
#[cfg(test)]
#[path = "tests/task_1555_ci_gate_tiers_tests.rs"]
mod task_1555_ci_gate_tiers_tests;
// trace:TASK-2238 | ai:claude
#[cfg(test)]
#[path = "tests/task_2238_review_sweep_tests.rs"]
mod task_2238_review_sweep_tests;
// trace:TASK-1562 | ai:claude
#[cfg(test)]
#[path = "tests/task_1562_worktree_reclaim_tests.rs"]
mod task_1562_worktree_reclaim_tests;
// trace:BUG-1723 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1723_config_trust_tests.rs"]
mod bug_1723_config_trust_tests;
// trace:TASK-1472 | ai:claude
#[cfg(test)]
#[path = "tests/task_1472_bare_aida_picker_tests.rs"]
mod task_1472_bare_aida_picker_tests;

#[cfg(test)]
#[path = "tests/bug_783_list_hidden_hints_tests.rs"]
mod bug_783_list_hidden_hints_tests;

#[cfg(test)]
#[path = "tests/story_1644_list_sort_cap_tests.rs"]
mod story_1644_list_sort_cap_tests;

#[cfg(test)]
#[path = "tests/task_1486_store_refusal_tests.rs"]
mod task_1486_store_refusal_tests;

#[cfg(test)]
#[path = "tests/task_1487_store_polish_tests.rs"]
mod task_1487_store_polish_tests;

#[cfg(test)]
#[path = "tests/story_423_asciinema_tests.rs"]
mod story_423_asciinema_tests;

// trace:BUG-588 | ai:claude
#[cfg(test)]
#[path = "tests/bug_588_history_id_resolves_uuid_tests.rs"]
mod bug_588_history_id_resolves_uuid_tests;

// trace:TASK-1480 | ai:claude
#[cfg(test)]
#[path = "tests/task_1480_history_id_alias_tests.rs"]
mod task_1480_history_id_alias_tests;

// trace:TASK-1507 | ai:claude
#[cfg(test)]
#[path = "tests/task_1507_history_cache_tests.rs"]
mod task_1507_history_cache_tests;

// trace:TASK-1508 | ai:claude
#[cfg(test)]
#[path = "tests/task_1508_history_source_tests.rs"]
mod task_1508_history_source_tests;

// trace:STORY-1650 | ai:claude
#[cfg(test)]
#[path = "tests/story_1650_history_flow_tests.rs"]
mod story_1650_history_flow_tests;

// trace:BUG-1631 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1631_history_spec_id_tests.rs"]
mod bug_1631_history_spec_id_tests;

// trace:BUG-1635 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1635_history_event_modes_tests.rs"]
mod bug_1635_history_event_modes_tests;

// trace:TASK-1512 | ai:claude
#[cfg(test)]
#[path = "tests/task_1512_history_transition_filters_tests.rs"]
mod task_1512_history_transition_filters_tests;

/// Classification of an `AIDA_STORE` value: either it resolves to a usable
/// store, or it's set-but-unusable with a specific reason. Lets the
/// resolution core stay pure/unit-testable while the env wrapper decides
/// whether to print the BUG-567 fall-through notice. Re-exported from
/// `aida-core::store_locate`, the canonical resolver both this CLI and
/// `aida-tui` route through.
// trace:BUG-567 trace:TASK-1141 | ai:claude
use aida_core::store_locate::StoreOverride;

#[cfg(test)]
#[path = "tests/delegated_reviewer_tests.rs"]
mod delegated_reviewer_tests;

#[cfg(test)]
#[path = "tests/story_569_review_brief_tests.rs"]
mod story_569_review_brief_tests;

#[cfg(test)]
#[path = "tests/task_760_intake_config_section_tests.rs"]
mod task_760_intake_config_section_tests;

#[cfg(test)]
#[path = "tests/task_1021_autopilot_config_section_tests.rs"]
mod task_1021_autopilot_config_section_tests;

#[cfg(test)]
#[path = "tests/task_304_ultraplan_cadence_tests.rs"]
mod task_304_ultraplan_cadence_tests;

#[cfg(test)]
#[path = "tests/story_760_store_mirror_tests.rs"]
mod story_760_store_mirror_tests;

#[cfg(test)]
#[path = "tests/story_441_archive_config_tests.rs"]
mod story_441_archive_config_tests;

#[cfg(test)]
#[path = "tests/story_284_store_sync_tests.rs"]
mod story_284_store_sync_tests;

#[cfg(test)]
#[path = "tests/bug_1468_stale_check_tests.rs"]
mod bug_1468_stale_check_tests;

#[cfg(test)]
#[path = "tests/forge_binary_resolution_tests.rs"]
mod forge_binary_resolution_tests;

// ───────────────────────────────────────────────────────────────────
// STORY-79 — background fetch from statusline (auto-freshness)
// ───────────────────────────────────────────────────────────────────

/// Pure source-label resolver for `aida whoami`'s user-id line. Mirrors
/// `current_user_id`'s resolution order (`AIDA_USER` → `USER` → `USERNAME` →
/// default) over already-read env values, so the printed source label can never
/// drift from the value `current_user_id` actually returned. Each arg is the
/// env var's value as `Option`, where an empty string counts as unset (matching
/// the `is_ok_and(|v| !v.is_empty())` checks the resolver uses).
/// trace:TASK-784
#[cfg(test)]
#[path = "tests/statusline_tests.rs"]
mod statusline_tests;

// trace:TASK-1479 | ai:claude
#[cfg(test)]
#[path = "tests/statusline_client_adapters_integration_tests.rs"]
mod statusline_client_adapters_integration_tests;

#[cfg(test)]
#[path = "tests/bug_88_pr_lookup_parse_tests.rs"]
mod bug_88_pr_lookup_parse_tests;

/// BUG-257: `gh_stderr_is_network_error` classifies a `gh` stderr line as a
/// transient network error vs. an auth/parse/other failure. The split powers
/// the `PrLookup::GhUnreachable` variant so the orchestrator can report
/// *Inconclusive* (drain pauses) instead of conflating "no PR" with "can't
/// reach the API". trace:BUG-257 | ai:claude
#[cfg(test)]
#[path = "tests/bug_257_gh_stderr_network_classifier_tests.rs"]
mod bug_257_gh_stderr_network_classifier_tests;

/// BUG-266: `claude_log_indicates_api_outage` classifies the headless
/// implementer's JSONL log as evidence the upstream Anthropic API took the
/// session out (a transient outage), rather than the implementer attempting
/// the work and failing. The orchestrator flips that phase from *failed* to
/// *Inconclusive* (the BUG-257 path), so a 529 / 5xx / stream-timeout no
/// longer marks a spec failed-by-implementer. trace:BUG-266 | ai:claude
#[cfg(test)]
#[path = "tests/bug_266_anthropic_api_outage_classifier_tests.rs"]
mod bug_266_anthropic_api_outage_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_354_text_question_classifier_tests.rs"]
mod bug_354_text_question_classifier_tests;

#[cfg(test)]
#[path = "tests/bug_87_queue_filter_tests.rs"]
mod bug_87_queue_filter_tests;

/// BUG-231: `aida findings promote` flipped status to Approved and printed
/// "joins the work queue" but never called `queue add` — the finding ended
/// up Approved and in no queue (a silent half-success). These cover the
/// fixed `queue_promoted_finding` helper: it routes to a real queue, honors
/// `--for`, and surfaces a queue-add failure as an error.
#[cfg(test)]
#[path = "tests/bug_231_findings_promote_tests.rs"]
mod bug_231_findings_promote_tests;
#[cfg(test)]
#[path = "tests/cr_8_filing_provenance_tests.rs"]
mod cr_8_filing_provenance_tests;
#[cfg(test)]
#[path = "tests/story_1428_gate_promote_tests.rs"]
mod story_1428_gate_promote_tests;

/// BUG-574: reliability papercuts — spurious non-zero exits on the idempotent
/// re-entry paths of `aida session start` (re-running for a scope this clone
/// already leases) and `aida pr ship` (re-running on an already-merged PR).
/// Both are benign no-ops that completed, so they must exit 0 — reserving
/// non-zero for real failures — exactly like the BUG-573 read-only contract.
/// These tests pin the pure decision points that drive the exit-0 behavior.
/// trace:BUG-574 | ai:claude
#[cfg(test)]
#[path = "tests/bug_574_spurious_exit_tests.rs"]
mod bug_574_spurious_exit_tests;

#[cfg(test)]
#[path = "tests/lease_enforcement_tests.rs"]
mod lease_enforcement_tests;

#[cfg(test)]
#[path = "tests/scope_fallback_tests.rs"]
mod scope_fallback_tests;

#[cfg(test)]
#[path = "tests/store_walkup_tests.rs"]
mod store_walkup_tests;

#[cfg(test)]
#[path = "tests/session_end_resolution_tests.rs"]
mod session_end_resolution_tests;

#[cfg(test)]
#[path = "tests/terminal_status_guard_tests.rs"]
mod terminal_status_guard_tests;

/// TASK-292 — `aida queue work --auto-complete` with no positional SPEC id
/// inherits the no-arg "pick the queue head" semantics. These cover the pure
/// pickup-order resolver: drivable-status classification, skipping in-flight
/// heads, and the empty-vs-all-in-flight error split.
#[cfg(test)]
#[path = "tests/auto_complete_head_tests.rs"]
mod auto_complete_head_tests;

/// TASK-293 — the `next` / `nextN` keyword parser. Covers every named form
/// in the acceptance criteria (`next`, `next1`, `next 1`, `next3`, `next 3`)
/// plus the malformed-input edge cases. The drain machinery itself
/// (empty-queue, N > queue-length) is the already-tested `drain_batch` /
/// `pick_auto_complete_head` — this module only covers the pure parse.
/// trace:TASK-293 | ai:claude
#[cfg(test)]
#[path = "tests/next_keyword_tests.rs"]
mod next_keyword_tests;

/// STORY-86 — env-flag opt-out + colorize_status coverage for the new
/// `Done` variant. Integration coverage (set up git+store, exercise the
/// helper end-to-end) lives in the verification script in the plan; the
/// unit slice here protects the bits that are pure functions.
/// trace:STORY-86 | ai:claude
#[cfg(test)]
#[path = "tests/auto_bump_done_tests.rs"]
mod auto_bump_done_tests;

/// BUG-95: regression coverage for the claim that `aida pull --code-only`
/// skips the Done → Completed auto-bump. The bug-as-filed asserted the
/// conditional was nested in the wrong branch; the current structure has
/// the auto-bump correctly inside the code-pull block (gated by
/// `!store_only`), so `--code-only` (code_only=true, store_only=false)
/// reaches it. This test exercises that path end-to-end via
/// `handle_pull_command` and verifies the bump fires.
/// trace:BUG-95 | ai:claude
#[cfg(test)]
#[path = "tests/handle_pull_command_tests.rs"]
mod handle_pull_command_tests;

/// TASK-106: coverage for the `aida push` / `aida pull` scope flags —
/// all four combinations (default / --code-only / --store-only / both)
/// at the clap-parse layer, the `AIDA_PUSH_DEFAULT` env-var override,
/// and the mutual-exclusion guard. trace:TASK-106 | ai:claude
#[cfg(test)]
#[path = "tests/push_pull_scope_tests.rs"]
mod push_pull_scope_tests;

/// TASK-234 / TASK-241: integration coverage for the git-state
/// classifiers behind `aida queue list`'s "Done — awaiting merge"
/// section and `aida show`'s git-linkage section. Both extracted
/// helpers (`classify_in_flight_specs`, `collect_git_linkage`) are
/// gh-free — they only run `git` — so a temp repo with hand-built
/// commits exercises every branch without a GitHub round-trip.
/// trace:TASK-234 TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/in_flight_linkage_integration_tests.rs"]
mod in_flight_linkage_integration_tests;

/// TASK-241: coverage for the squash-merge PR-number parser that
/// `aida show`'s git-linkage section uses to point shipped specs at
/// their merged PR. trace:TASK-241 | ai:claude
#[cfg(test)]
#[path = "tests/git_linkage_tests.rs"]
mod git_linkage_tests;

/// STORY-511: coverage for the forge-aware change-request (PR/MR) linkage
/// formatter that `aida show`'s git-linkage section renders. Fully
/// isolated — no git repo, no `gh`/`glab` on PATH, no I/O — the rendering
/// is a pure function of (ForgeKind, ChangeLinkageState). EPIC-35 slice 5.
/// trace:STORY-511 | ai:claude
#[cfg(test)]
#[path = "tests/change_linkage_format_tests.rs"]
mod change_linkage_format_tests;

/// TASK-1475 (CR-8 acceptance 6 follow-up): coverage for `filing_drift_hint`
/// — the filing-provenance staleness signal `aida show` / `aida why`
/// render. Pure git, no `gh`/`glab` on PATH.
// trace:TASK-1475 | ai:claude
#[cfg(test)]
#[path = "tests/task_1475_filing_drift_tests.rs"]
mod task_1475_filing_drift_tests;

/// TASK-238: coverage for the queue-list tag surfacing — the inline
/// chip formatter and the `--by-batch` group key. trace:TASK-238
#[cfg(test)]
#[path = "tests/queue_tag_tests.rs"]
mod queue_tag_tests;

/// TASK-107: regression coverage for `aida fetch`. The behaviors we
/// pin: (1) code-leg fetch updates origin/<branch> without touching the
/// worktree, (2) cache-invalidation hook fires after the store leg
/// (best-effort, off the real `~/.aida` since `AIDA_HOME` overrides it
/// for tests), (3) `--code-only` / `--store-only` route correctly,
/// (4) the auto-bump does NOT fire on fetch — that's a pull-only
/// behavior. trace:TASK-107 | ai:claude
#[cfg(test)]
#[path = "tests/handle_fetch_command_tests.rs"]
mod handle_fetch_command_tests;

#[cfg(test)]
#[path = "tests/derive_parent_epic_label_tests.rs"]
mod derive_parent_epic_label_tests;

#[cfg(test)]
#[path = "tests/format_review_story_display_tests.rs"]
mod format_review_story_display_tests;

#[cfg(test)]
#[path = "tests/auto_branch_tests.rs"]
mod auto_branch_tests;

#[cfg(test)]
#[path = "tests/worktree_dirty_entries_tests.rs"]
mod worktree_dirty_entries_tests;

#[cfg(test)]
#[path = "tests/add_aida_gitignore_entries_tests.rs"]
mod add_aida_gitignore_entries_tests;

#[cfg(test)]
#[path = "tests/recent_files_for_branch_tests.rs"]
mod recent_files_for_branch_tests;

#[cfg(test)]
#[path = "tests/branch_behind_main_tests.rs"]
mod branch_behind_main_tests;

#[cfg(test)]
#[path = "tests/session_root_resolution_tests.rs"]
mod session_root_resolution_tests;

include!("lib_part5.rs");

#[cfg(test)]
// trace:STORY-2539 | ai:codex
#[path = "tests/story_2539_vendor_literal_ratchet_tests.rs"]
mod story_2539_vendor_literal_ratchet_tests;

// MCP application boundary; retain the established helper paths.
// trace:TASK-3429 | ai:codex
pub use brief::clear_pending_brief;
pub use burndown_run::is_standing_artifact_type;
pub use config_init::read_ultraplan_config;
pub use config_init::UltraplanMode;
pub use findings_gate::record_caller_status_transition;
pub use kernel::canonical_role_name;
pub use kernel::current_user_id;
pub use kernel::find_project_root;
pub use kernel::get_default_author;
pub use kernel::home_dir;
pub use kernel::is_terminal_status;
pub use kernel::parse_days_arg;
pub use kernel::resolve_aida_exe;
pub use kernel::resolve_current_session_id;
pub use leases::format_queue_membership;
pub use leases::queue_memberships_for;
pub use leases::requeue_lease_gate;
pub use plan_report::assemble_ultraplan_prompt;
pub use plan_report::build_reusable_helpers_section;
pub use plan_report::compute_plan_report;
pub use plan_report::plan_repo_root;
pub use plan_report::read_reserved_paths;
pub use plan_report::render_plan_report_string;
pub use queue_cmd::rework_target_for_mode;
pub use queue_misc::entry_matches_role_filter;
pub use queue_misc::resolve_mail_sender_identity;
pub use queue_misc::resolve_mail_sender_role;
pub use queue_misc::resolve_queue_role_filter;
pub use rel_cards::reject_cross_store_spec_ref;
pub use rel_cards::relationship_terminal_ambiguity_warning;
pub use requirements_cli::enforce_structural_tag_replacement;
pub use requirements_cli::validate_tag_value;
pub use requirements_cli::TagRepair;
pub use role::record_role_activity;
pub use session_misc::update_manifest_for_status;
pub use stakeholder_role::active_stakeholder_role;
pub use stakeholder_role::requester_intake_type_allowed;
pub use stakeholder_role::stakeholder_action_allowed;
pub use stakeholder_role::stakeholder_action_label;
pub use stakeholder_role::stakeholder_mcp_read_allowed;
pub use stakeholder_role::stakeholder_refusal_message;
pub use statusline_root::statusline_project_root;
pub use trace::collect_git_linkage;
pub use trace::collect_git_linkage_until;
pub use trace::SHOW_LINKAGE_DEADLINE;
pub use upgrade_release::REQUESTER_INTAKE_FORBIDDEN_FIELDS;
pub use usage_mcp_helpers::aggregate_events;
pub use worktree_gc::for_target_requires_dispatch_authority;

// Build identity belongs to the CLI image, including the extracted MCP transport.
// trace:TASK-3429 | ai:codex
pub const BUILD_GIT_SHA: &str = env!("AIDA_BUILD_GIT_SHA");
pub const BUILD_GIT_DIRTY: &str = env!("AIDA_BUILD_GIT_DIRTY");
pub use cmdkit::StakeholderAction;

// trace:TASK-3429 | ai:codex
pub use rel_cards::load_dispenser;

// trace:STORY-2699 | ai:codex
mod flow_cmd;

mod flow_runtime;

// BUG-1638: race seams for the self-reading status writers, the phase-1
// restore outcome, zen's deleted-spec message, and the atomic findings
// promote. trace:BUG-1638 | ai:claude
#[cfg(test)]
#[path = "tests/bug_1638_race_seam_tests.rs"]
mod bug_1638_race_seam_tests;

// Shared stale-cache fixture for the BUG-1664 / BUG-1670 / BUG-1606 suites.
// trace:TASK-1526 | ai:claude
#[cfg(test)]
#[path = "tests/stale_cache_fixture.rs"]
mod stale_cache_fixture;
