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

mod advisor;
mod advisor_code_gate;
mod advisor_watch;
mod agent_launch_prompt;
mod agent_registry;
mod aida_bin;
mod alias;
mod archive_cmd;
mod assign_cmd;
mod auto_complete;
mod auto_complete_telemetry;
mod autonomy_cmd;
mod autopilot;
// trace:TASK-1018 | ai:claude — durable execution audit + one-command reversal.
mod autopilot_audit;
mod awaiting_you;
mod backlog;
mod brief_cmd;
mod burndown;
// trace:TASK-1500 | ai:codex
mod bypass_confirm;
mod cache_cmd;
mod cache_output;
mod calibration;
mod changelog;
mod changelog_cmd;
mod ci_idle_timeout;
mod claim;
mod claude_agents;
mod cli;
#[cfg(feature = "remote")]
mod client;
mod comment_cmd;
mod commit;
mod compete;
mod compete_cmd;
mod complexity_calibration;
mod config_cmd;
mod config_edit;
mod context_prompt;
mod coordination;
mod criteria;
// trace:STORY-1487 | ai:claude
mod criteria_coverage;
mod criteria_gate;
mod criteria_red_run;
mod db_cmd;
mod decide_cmd;
mod deep_link;
mod defer_cmd;
mod deps_cmd;
mod dev_cmd;
mod digest;
mod digest_cmd;
// trace:TASK-1564 | ai:claude — render/re-read a drain phase-failure note.
mod drain_failure_note;
mod gitlab_mirror_link;
mod graph_cmd;
mod project_capabilities;
mod protocol_gate;
// trace:TASK-1090 | ai:claude — per-row dispatch-health classifier for `aida ps`.
mod dispatch_health_ps;
// trace:TASK-1092 | ai:claude — [dispatch.routing] config loader (additive, not yet wired in).
mod dispatch_routing_config;
// trace:STORY-776 | ai:claude — pure decision logic for `aida do` mode dispatch.
mod autoprogress;
mod do_dispatch;
mod doc_cmd;
mod docs;
mod doctor_cmd;
// trace:STORY-1462 | ai:claude — runaway-seat watchdog doctor category.
mod drain_caps;
mod drain_cmd;
mod drain_lock;
// trace:TASK-1518 | ai:claude
mod drain_signal;
mod freshness_gate;
// trace:BUG-1622 | ai:claude — keeps user-supplied refs from reading as git options.
mod git_arg_guard;
mod git_backend_cmd;
// trace:TASK-1522 | ai:antigravity
pub(crate) mod intent_capture;
mod machine_readiness;
mod mcp_cmd;
mod orchestrator_cmd;
mod pr_cmd;
pub(crate) mod pr_list;
mod protocol_cmd;
mod queue_cmd;
mod runaway_seats;
mod solo_cmd;
mod status_cmd;
mod supervise_cmd;
mod supervisor;
// trace:STORY-1218 | ai:claude
mod schedule_driver;
mod shift;
mod terminal_cmd;
// trace:TASK-1427 | ai:codex
mod token_ledger;
mod tui_entry;
mod zen_cmd;
use claim::*;
use drain_cmd::*;
use mcp_cmd::*;
use orchestrator_cmd::*;
use pr_cmd::*;
use pr_list::*;
use queue_cmd::*;
use solo_cmd::*;
use status_cmd::*;
use tui_entry::*;
use zen_cmd::*;
// trace:TASK-967 | ai:claude
mod drain_summary;
// trace:STORY-1415 | ai:claude
mod field_study;
mod field_study_cmd;
mod mass_change;
// trace:SPIKE-67 | ai:claude
mod rule_violation;
// trace:STORY-656 | ai:claude
mod completion; // trace:STORY-1418 | ai:claude
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
mod findings;
pub mod graded_review;
mod identity_gate;
mod implementer_preflight;
// trace:STORY-700 | ai:claude — passive first-run hint chain through the core loop.
mod first_run;
mod focus;
mod focus_cmd;
mod forge;
mod forge_profiles;
mod gate_cmd;
mod global_queue;
mod last_drain;
mod lifecycle_cmd;
mod lint_cmd;
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
mod import_export_cmd;
mod import_plan_cmd;
mod interview;
// trace:STORY-633 | ai:claude — toml_edit writer for `aida config glyph`.
mod glyph_config;
mod glyphs;
mod goal_cmd;
// trace:EPIC-36 | ai:claude — session-vs-drain misclassification-gap metric.
mod headless_tail;
mod headless_tee;
mod vendor_activity;
// trace:TASK-990 | ai:claude — `aida watch` streaming event classifier.
mod watch;
// trace:STORY-658 | ai:claude — `aida health` at-a-glance vital-signs read.
mod health;
mod health_cmd;
mod health_metrics;
mod health_vitals_cmd;
mod history;
mod history_layout;
// trace:TASK-1507 | ai:claude
mod history_cache;
// trace:STORY-1478 | ai:claude — per-spec work/wait/unknown timeline.
mod history_timeline;
// trace:STORY-1479 | ai:claude — aggregate cycle-time breakdown over completed specs.
mod human_audit;
mod human_cmd;
mod metrics_cycle_time;
// trace:STORY-1480 | ai:claude — per-spec timing records published to the store.
mod timing_record;
// trace:TASK-1150 | ai:claude — distinct-user identity guard (queue/lease mixups).
mod identity_guard;
mod init_bootstrap;
mod init_cmd;
mod intake;
mod integrate;
// trace:TASK-1050 | ai:claude — own-checkout guard for `aida integrate` (BUG-650).
mod ci_gate;
mod harvest;
mod integrate_checkout;
mod integrate_view;
mod intent;
mod internal_cmd;
mod mailbox_cmd;
mod mailbox_store;
mod maintenance_schedule;
mod manual;
mod mcp;
mod mcp_translate;
mod memories_cmd;
mod merge_hold;
mod merge_lock;
mod metrics;
mod metrics_cmd;
mod network_retry;
mod node_cmd;
mod not_found;
mod pr_claim_surface;
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
mod presence;
mod presence_cmd;
mod process_probe;
mod process_retry;
use crate::process_retry::RetryEtxtbsy;
// BUG-677: the /proc probe + lease/spec liveness classifiers moved to
// aida-core so aida-tui can compute liveness in-process. Re-export the shared
// classifier types + fns at the crate root so `crate::LeaseState` /
// `classify_lease_state` etc. keep resolving for every existing call site.
// trace:BUG-677 | ai:claude
pub(crate) use aida_core::liveness::{
    classify_lease_state, classify_spec_liveness, classify_stale_lease_recovery,
    lease_owner_process_gone, LeaseState, SpecLiveness, StaleLeaseRecovery,
};
mod prompts;
mod ship;
// trace:STORY-384 | ai:claude — pure recovery-action decision for `queue recover`.
mod punt;
mod queue_recover;
// NeedsAttention -> back-in-flight: shelve-marker clearing + requeue hint.
// trace:TASK-1311 | ai:claude
mod requeue;
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
mod reviewer_summary;
// trace:STORY-1405 | ai:claude — review-in-progress marker consulted by merge surfaces.
mod review_marker;
// trace:STORY-1417 | ai:claude
mod review_classes;
// trace:BUG-775 | ai:claude — review verdicts as first-class gate-readable state.
mod review_verdict;
mod role_cmd;
mod rules_cmd;
mod rules_sync;
// trace:TASK-1307 | ai:claude — the pre-BUG-1452 stranded-refusal sweep.
mod sandbox_cmd;
mod scaffold_cmd;
mod scaffold_refresh;
mod stranded_sweep;
mod sweep_test_hook;
// trace:STORY-262 | ai:claude
mod schedule;
mod schedule_cmd;
mod schedule_ledger;
mod schedule_predicate;
mod schema;
// trace:STORY-1473 | ai:codex
mod seat_authority;
mod seat_rotation;
mod seats;
mod server_cmd;
mod session;
// trace:STORY-993 | ai:claude
mod session_liveness;
mod session_manifest;
mod session_misc_cmd;
// trace:TASK-1177 | ai:claude — the substrate-state reap pass for sessions
// whose spec is finished, branch merged, and process exited.
mod session_reap;
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
mod usage;
mod usage_cmd;
// trace:TASK-877 | ai:claude — user/project-defined `aida` command aliases.
mod user_alias;
mod worker;
mod workflow_hints;
// trace:STORY-716 | ai:claude — `aida worktree` namespace (EPIC-55 workspace layer).
mod worktree;
// trace:TASK-634 | ai:claude — pure WorktreeCreate/Remove payload → lease record.
mod worktree_lease;
// trace:TASK-1562 | ai:claude — `aida worktree reclaim`: stale target/ cache reclaim.
mod worktree_reclaim;
// trace:STORY-711 | ai:claude — advisor-directed worktree lock (`aida lock`), slice 1.
mod worktree_lock;
// trace:TASK-1178 | ai:claude — warn-only commit-boundary worktree-scope guard.
mod worktree_scope_gate;
mod zen;
// trace:STORY-721 | ai:claude — `aida zen <spec>` autonomous implement+ship drive.
mod zen_drive;

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::IsTerminal;
use uuid::Uuid;

// trace:TASK-840 | ai:claude
/// Render a registry [`crate::glyphs::Glyph`] honoring the active glyph profile
/// (`AIDA_GLYPHS` env / `[ui] glyphs` config / default unicode). The crate-root
/// twin of the module-local `glyph()` helpers in `auto_complete.rs`,
/// `status_cleanup.rs`, etc. Called fully-qualified (`crate::glyph(...)`) from
/// the migrated literal sites so it resolves uniformly from main-code and
/// inline test-module scopes alike. Default (unicode) output is byte-for-byte
/// identical to the raw literals it replaced.

// SPLIT BY AI
// trace:TASK-1538 | ai:antigravity
include!("lib_part1.rs");
include!("lib_part2.rs");
include!("lib_part3.rs");
include!("lib_part4.rs");
include!("lib_part5.rs");
include!("lib_part6.rs");
