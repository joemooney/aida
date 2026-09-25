//! Product-role supervision helpers.
//!
//! `aida supervise --nudge` is the stop-gap loop for STORY-1052: it reads the
//! local event stream, finds transiently parked specs, and sends one deduped
//! mailbox nudge to a live advisor. It never leases work, drives a queue item,
//! opens a PR, or merges.
//!
//! trace:STORY-1052 | ai:codex

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use aida_core::DatabaseBackend;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::agent_registry::{self, AgentClassifyContext, AgentStatus, Availability};
use crate::cli::SuperviseCommand;
use crate::events::{self, Event, EventKind};
use crate::notify;

const DEFAULT_MIN_INTERVAL_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StuckItem {
    pub(crate) spec: String,
    pub(crate) title: String,
    pub(crate) phase: String,
    pub(crate) kind: String,
    pub(crate) fingerprint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NudgeState {
    #[serde(default)]
    specs: BTreeMap<String, SpecNudgeState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpecNudgeState {
    fingerprint: String,
    last_nudged_at_ms: i64,
}

pub(crate) fn handle_supervise_command(
    cmd: &SuperviseCommand,
    backend: &aida_core::CachedGitBackend,
    store_path: &Path,
) -> Result<()> {
    match cmd {
        SuperviseCommand::Nudge => handle_supervise_nudge(backend, store_path),
        // trace:STORY-1051 | ai:claude
        SuperviseCommand::Redrive {
            execute,
            max_attempts,
            max,
            json,
        } => {
            let project_root = store_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
            let opts = crate::supervisor::SuperviseOpts {
                execute: *execute,
                max_attempts: max_attempts.unwrap_or(crate::supervisor::DEFAULT_MAX_ATTEMPTS),
                backoff: crate::supervisor::DEFAULT_BACKOFF.to_vec(),
                max: *max,
                json: *json,
                floors: None,
            };
            crate::supervisor::handle_supervise_command(backend, project_root, opts)
        }
        // trace:STORY-1096 | ai:claude
        SuperviseCommand::Watch {
            objective,
            execute,
            interval,
            json,
        } => handle_supervise_watch(
            backend,
            store_path,
            objective.as_deref(),
            *execute,
            *interval,
            *json,
        ),
    }
}

fn handle_supervise_nudge(backend: &aida_core::CachedGitBackend, store_path: &Path) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
    let store = backend.load()?;
    let titles = store
        .requirements
        .iter()
        .filter_map(|r| {
            let id = display_id(r)?;
            Some((id.to_ascii_uppercase(), r.title.clone()))
        })
        .collect::<HashMap<_, _>>();
    let body = std::fs::read_to_string(events::events_path(project_root)).unwrap_or_default();
    let stuck = stuck_items_from_events(&body, &titles);
    if stuck.is_empty() {
        return Ok(());
    }

    let state_path = nudge_state_path(project_root);
    let mut state = read_nudge_state(&state_path);
    let now_ms = Utc::now().timestamp_millis();
    let due = due_stuck_items(&stuck, &state, now_ms, DEFAULT_MIN_INTERVAL_MS);
    if due.is_empty() {
        return Ok(());
    }

    if let Some(advisor) = live_advisor_recipient(project_root) {
        send_advisor_nudge(project_root, &advisor, &due)?;
        record_nudged(&mut state, &due, now_ms);
        write_nudge_state(&state_path, &state)?;
        println!(
            "nudged {advisor} about {} transiently parked spec(s)",
            due.len()
        );
    } else {
        let message = format_operator_notification(&due);
        let outcome = notify::send_direct(
            project_root,
            "supervise_nudge",
            "aida: stuck work needs advisor",
            &message,
        )?;
        if outcome.configured {
            record_nudged(&mut state, &due, now_ms);
            write_nudge_state(&state_path, &state)?;
            println!(
                "operator notified about {} transiently parked spec(s) (sent {}, suppressed {}, pending {})",
                due.len(),
                outcome.sent,
                outcome.suppressed,
                outcome.pending
            );
        } else {
            println!(
                "no live advisor and [notify].command is unset; {} transiently parked spec(s) need attention",
                due.len()
            );
        }
    }
    Ok(())
}

fn display_id(req: &aida_core::Requirement) -> Option<String> {
    req.agreed_id
        .clone()
        .or_else(|| req.spec_id.clone())
        .filter(|s| !s.trim().is_empty())
}

fn nudge_state_path(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("supervise-nudges.json")
}

fn read_nudge_state(path: &Path) -> NudgeState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str(&body).ok())
        .unwrap_or_default()
}

fn write_nudge_state(path: &Path, state: &NudgeState) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating supervise state dir {}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(state).context("serialising supervise nudge state")?;
    aida_core::write_atomic(path, &body)
        .with_context(|| format!("writing supervise nudge state {}", path.display()))
}

pub(crate) fn stuck_items_from_events(
    body: &str,
    titles: &HashMap<String, String>,
) -> Vec<StuckItem> {
    let mut by_spec = BTreeMap::<String, StuckItem>::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Ok(event) = serde_json::from_str::<Event>(line) else {
            continue;
        };
        let EventKind::SpecShelved { phase, kind, .. } = event.kind else {
            continue;
        };
        if !is_transient_shelve_kind(&kind) {
            continue;
        }
        let Some(spec) = event.spec else {
            continue;
        };
        let spec_key = spec.to_ascii_uppercase();
        let title = titles
            .get(&spec_key)
            .cloned()
            .unwrap_or_else(|| "(title unavailable)".to_string());
        let fingerprint = format!(
            "{}:{}:{}:{}",
            spec_key,
            event.ts.timestamp_millis(),
            phase,
            kind
        );
        by_spec.insert(
            spec_key.clone(),
            StuckItem {
                spec,
                title,
                phase,
                kind,
                fingerprint,
            },
        );
    }
    by_spec.into_values().collect()
}

fn is_transient_shelve_kind(kind: &str) -> bool {
    let k = kind.trim().to_ascii_lowercase();
    [
        "transient",
        "watchdog",
        "timeout",
        "timed-out",
        "network",
        "api",
        "rate-limit",
        "environmental",
    ]
    .iter()
    .any(|needle| k.contains(needle))
}

fn due_stuck_items(
    stuck: &[StuckItem],
    state: &NudgeState,
    now_ms: i64,
    min_interval_ms: i64,
) -> Vec<StuckItem> {
    stuck
        .iter()
        .filter(|item| {
            let key = item.spec.to_ascii_uppercase();
            let Some(prior) = state.specs.get(&key) else {
                return true;
            };
            prior.fingerprint != item.fingerprint
                || now_ms.saturating_sub(prior.last_nudged_at_ms) >= min_interval_ms
        })
        .cloned()
        .collect()
}

fn record_nudged(state: &mut NudgeState, items: &[StuckItem], now_ms: i64) {
    for item in items {
        state.specs.insert(
            item.spec.to_ascii_uppercase(),
            SpecNudgeState {
                fingerprint: item.fingerprint.clone(),
                last_nudged_at_ms: now_ms,
            },
        );
    }
}

fn live_advisor_recipient(project_root: &Path) -> Option<String> {
    let cfg = agent_registry::Config::load(project_root);
    let ctx = AgentClassifyContext::new(Utc::now(), cfg.busy_threshold_secs, Vec::new());
    agent_registry::list_agent_views(project_root, &ctx)
        .into_iter()
        .filter(|a| {
            a.role
                .as_deref()
                .map(|r| r.eq_ignore_ascii_case("advisor"))
                .unwrap_or(false)
                && a.status != AgentStatus::Stale
                && a.availability == Availability::Available
        })
        .map(|a| a.name.unwrap_or_else(|| "advisor".to_string()))
        .next()
}

fn send_advisor_nudge(project_root: &Path, advisor: &str, items: &[StuckItem]) -> Result<()> {
    use aida_core::mailbox::{Intent, Message, Recipient};

    let id = uuid::Uuid::new_v4().to_string();
    let body = format_nudge_body(items);
    // trace:BUG-1533 | ai:claude
    let (from, from_source) = crate::resolve_mail_sender_identity(None);
    // trace:BUG-1592 | ai:claude
    let from_role = crate::resolve_mail_sender_role();
    let msg = Message {
        subject: None,
        id: id.clone(),
        thread_id: id,
        from,
        to: Recipient::Agent(advisor.to_string()),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: true,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
        from_source,
        from_role,
        relayed_from: None,
    };
    crate::mailbox_store::write_message(project_root, &msg)?;
    Ok(())
}

fn format_nudge_body(items: &[StuckItem]) -> String {
    let mut out = String::from("Transiently parked specs need a move:\n");
    for item in items {
        out.push_str(&format!(
            "\n- {}: {}\n  parked: {} / {}\n  move: aida queue work {} --resume\n",
            item.spec, item.title, item.phase, item.kind, item.spec
        ));
    }
    out
}

fn format_operator_notification(items: &[StuckItem]) -> String {
    let mut out = format!(
        "{} item(s) stuck, no advisor running -- start one: aida agent new claude --role advisor\n",
        items.len()
    );
    out.push_str("\nStuck specs:\n");
    for item in items {
        out.push_str(&format!(
            "- {}: {}\n  parked: {} / {}\n  move: aida queue work {} --resume\n",
            item.spec, item.title, item.phase, item.kind, item.spec
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// STORY-1096 slice 1 — the oversight watch loop.
//
// `aida supervise watch --objective <EPIC>` is the substrate-first oversight
// pass from ADR-29. It COMPOSES the two shipped reflexes (redrive + nudge) and
// adds the one new capability — objective-drift detection + realign — then
// surfaces the human-decision items. It reads the substrate (store + queue +
// events) and never drives or merges. Default is a dry-run report; --execute
// realigns the queue. --interval repeats the pass on a sleep loop.
// trace:STORY-1096 | ai:claude
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct WatchReport {
    objective: String,
    total_children: usize,
    done_children: usize,
    /// Approved, actionable children not yet queued — the drift to realign.
    drift: Vec<String>,
    /// Children queued this pass (only populated under --execute).
    realigned: Vec<String>,
    /// The re-drive reflex's one-line outcome (the night shift's step).
    // trace:BUG-1621 | ai:claude
    redrive: String,
    /// Parks re-queued this pass (only under --execute with re-drive on).
    redriven: Vec<String>,
    /// Parks reclassified to needs-human at the ADR-26 cap this pass.
    reclassified: Vec<String>,
    /// Per-park decisions, including every floor refusal.
    redrive_plan: Vec<crate::supervisor::SuperviseDecision>,
}

fn handle_supervise_watch(
    backend: &aida_core::CachedGitBackend,
    store_path: &Path,
    objective: Option<&str>,
    execute: bool,
    interval: Option<u64>,
    json: bool,
) -> Result<()> {
    let project_root = store_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot derive project root from store path"))?;
    let objective = resolve_objective(objective, project_root)?;
    loop {
        run_watch_pass(backend, store_path, project_root, &objective, execute, json)?;
        match interval {
            Some(secs) if secs > 0 => std::thread::sleep(std::time::Duration::from_secs(secs)),
            _ => break,
        }
    }
    Ok(())
}

/// Resolve the objective from the flag, else the `[oversight] objective` config
/// value. Errors with guidance when neither is set.
fn resolve_objective(flag: Option<&str>, project_root: &Path) -> Result<String> {
    if let Some(o) = flag.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(o.to_string());
    }
    let cfg_path = project_root.join(".aida").join("config.toml");
    if let Ok(body) = std::fs::read_to_string(&cfg_path) {
        if let Ok(value) = body.parse::<toml::Value>() {
            if let Some(obj) = value
                .get("oversight")
                .and_then(|t| t.get("objective"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Ok(obj.to_string());
            }
        }
    }
    anyhow::bail!(
        "no objective given — pass --objective <EPIC> or set `[oversight] objective` in .aida/config.toml"
    )
}

fn run_watch_pass(
    backend: &aida_core::CachedGitBackend,
    store_path: &Path,
    project_root: &Path,
    objective: &str,
    execute: bool,
    json: bool,
) -> Result<()> {
    let queued = queued_spec_ids();
    let report = watch_pass_core(
        backend,
        project_root,
        objective,
        execute,
        &queued,
        &mut queue_add_implementer,
    )?;

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print_watch_report(&report, execute);
    }

    // Compose the other shipped reflex: nudge advisor stalls.
    // Nudge sends a real mailbox message / notification, so it only fires under
    // --execute; a dry-run pass has no side effects.
    if execute {
        let _ = handle_supervise_nudge(backend, store_path);
    }

    // Surface only human-decision items.
    if !json {
        print_awaiting_surface();
    }
    Ok(())
}

/// One watch pass without its subprocess reads and its output: realign
/// (through `queue_add`) and the re-drive reflex. `execute = false` is a
/// dry run that writes nothing. Split out so tests drive the real wiring.
// trace:BUG-1621 | ai:claude
fn watch_pass_core(
    backend: &aida_core::CachedGitBackend,
    project_root: &Path,
    objective: &str,
    execute: bool,
    queued: &std::collections::HashSet<String>,
    queue_add: &mut dyn FnMut(&str) -> bool,
) -> Result<WatchReport> {
    use aida_core::models::{RelationshipType, RequirementStatus};

    let store = backend.load()?;
    let obj_req = store
        .requirements
        .iter()
        .find(|r| crate::queue_cmd::spec_matches(r, objective))
        .ok_or_else(|| anyhow::anyhow!("no requirement matches objective `{objective}`"))?;
    let obj_id = obj_req.id;
    let obj_display = display_id(obj_req).unwrap_or_else(|| objective.to_string());
    let parent_tag = format!("parent:{}", obj_display.to_ascii_lowercase());

    // Children of the objective: a Parent relationship OR a `parent:<obj>` tag.
    let children: Vec<&aida_core::Requirement> = store
        .requirements
        .iter()
        .filter(|r| {
            r.relationships
                .iter()
                .any(|rel| rel.rel_type == RelationshipType::Parent && rel.target_id == obj_id)
                || r.tags.iter().any(|t| t.to_ascii_lowercase() == parent_tag)
        })
        .collect();

    let total = children.len();
    let done = children
        .iter()
        .filter(|r| {
            matches!(
                r.status,
                RequirementStatus::Done | RequirementStatus::Completed
            )
        })
        .count();

    // Drift = Approved, not archived, not deferred, not already queued.
    let drift = compute_drift(&children, queued);

    // Realign: queue each drifted child for the implementer (idempotent — the
    // Approved filter + queue-add dupe tolerance keep it safe to re-run).
    let mut realigned: Vec<String> = Vec::new();
    if execute {
        for spec in &drift {
            if queue_add(spec) {
                realigned.push(spec.clone());
            }
        }
    }

    // The re-drive reflex runs through the night shift's own re-drive step
    // (BUG-1621): the same per-clone opt-in (ADR-26 default off), guards,
    // floors, attempt-recorded-first requeue and cap branch as the tick. It
    // only re-queues; the next drain wave picks the specs up, so this pass
    // never launches a drive and never forces a claim. A failure is reported
    // and never stops the rest of the pass.
    // trace:BUG-1621 | ai:claude
    let redrive = crate::shift::run_redrive_pass(project_root, backend, !execute);
    let (redrive_line, redriven, reclassified, redrive_plan) = match redrive {
        Ok(r) => {
            let mut line = r.redrive;
            let failing: Vec<&str> = r
                .redrive_guards
                .iter()
                .filter(|g| !g.pass)
                .map(|g| g.detail.as_str())
                .collect();
            if !failing.is_empty() {
                line.push_str(&format!(" ({})", failing.join("; ")));
            }
            (line, r.redriven, r.reclassified, r.redrive_plan)
        }
        Err(e) => (format!("error: {e:#}"), Vec::new(), Vec::new(), Vec::new()),
    };

    Ok(WatchReport {
        objective: obj_display,
        total_children: total,
        done_children: done,
        drift,
        realigned,
        redrive: redrive_line,
        redriven,
        reclassified,
        redrive_plan,
    })
}

fn print_watch_report(report: &WatchReport, execute: bool) {
    println!("▸ oversight · objective {}", report.objective);
    println!(
        "  progress: {}/{} children done",
        report.done_children, report.total_children
    );
    if report.drift.is_empty() {
        println!("  drift: none — objective work is aligned");
    } else if execute {
        println!(
            "  realigned: queued {} ready child(ren) → {}",
            report.realigned.len(),
            report.realigned.join(", ")
        );
    } else {
        println!(
            "  drift: {} ready child(ren) unqueued → {} (dry-run; --execute to realign)",
            report.drift.len(),
            report.drift.join(", ")
        );
    }
    // trace:BUG-1621 | ai:claude
    println!("  re-drive: {}", report.redrive);
}

/// Whether a child type is realignable — i.e. buildable work an implementer
/// can pick up. Realign QUEUES the spec for the implementer under `--execute`,
/// so it must never target a knowledge-class / structural record: an accepted
/// ADR carries status Approved (BUG-1130), a Constraint/Vision/Principle/Term
/// is authored not implemented, an Epic's status is a read-only rollup, and
/// Folder/Meta/Doc are organizational. This is intentionally STRICTER than the
/// candidate-view filter `aida_core::lifecycle::is_work_item_type` (which a
/// triage view uses): realign also excludes Epic/Folder/Meta/Constraint/Doc
/// because none is queueable implementer work.
// trace:BUG-1130 | ai:claude
fn is_realignable_type(req_type: &aida_core::models::RequirementType) -> bool {
    use aida_core::models::RequirementType as T;
    !matches!(
        req_type,
        T::Decision
            | T::Epic
            | T::Folder
            | T::Meta
            | T::Principle
            | T::Vision
            | T::Constraint
            | T::Term
            | T::Doc
    )
}

/// Objective children that are ready but unqueued: an implementable type with
/// status Approved, not archived, not deferred, and not already in a queue.
/// Pure, so it is unit tested directly.
fn compute_drift(
    children: &[&aida_core::Requirement],
    queued: &std::collections::HashSet<String>,
) -> Vec<String> {
    use aida_core::models::RequirementStatus;
    let mut drift = Vec::new();
    for c in children {
        if is_realignable_type(&c.req_type)
            && c.status == RequirementStatus::Approved
            && !c.archived
            && !c.deferred
        {
            if let Some(id) = display_id(c) {
                if !queued.contains(&id.to_ascii_uppercase()) {
                    drift.push(id);
                }
            }
        }
    }
    drift
}

/// Best-effort set of spec ids currently in a queue (uppercased). Reads only
/// per-entry identity fields from `aida queue list --json`; an empty set on any
/// failure just means the --execute path relies on queue-add dupe tolerance.
fn queued_spec_ids() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let exe = crate::aida_exe_path();
    let Ok(out) = std::process::Command::new(exe)
        .args(["queue", "list", "--json"])
        .output()
    else {
        return set;
    };
    if !out.status.success() {
        return set;
    }
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
        collect_queue_identity_spec_ids(&value, &mut set);
    }
    set
}

// trace:TASK-1219 | ai:codex
fn collect_queue_identity_spec_ids(
    value: &serde_json::Value,
    set: &mut std::collections::HashSet<String>,
) {
    let Some(entries) = value.as_array() else {
        return;
    };
    for entry in entries {
        let Some(map) = entry.as_object() else {
            continue;
        };
        for key in ["spec_id", "agreed_id", "requirement_id", "id"] {
            let Some(s) = map.get(key).and_then(|v| v.as_str()) else {
                continue;
            };
            if is_spec_id(s) {
                set.insert(s.to_ascii_uppercase());
                break;
            }
        }
    }
}

/// Loosely: `LETTERS-DIGITS` (e.g. STORY-1094, BUG-12, EPIC-1-001).
fn is_spec_id(s: &str) -> bool {
    let s = s.trim();
    let Some((prefix, rest)) = s.split_once('-') else {
        return false;
    };
    !prefix.is_empty()
        && prefix.chars().all(|c| c.is_ascii_alphabetic())
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Queue one spec for the implementer. Tolerates the "already queued" outcome
/// (returns false so it is not reported as a fresh realign).
fn queue_add_implementer(spec: &str) -> bool {
    let exe = crate::aida_exe_path();
    std::process::Command::new(exe)
        .args(["queue", "add", spec, "--for", "implementer", "--no-scope"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Print the `aida awaiting` human-decision surface (best-effort).
fn print_awaiting_surface() {
    let exe = crate::aida_exe_path();
    if let Ok(out) = std::process::Command::new(exe).args(["awaiting"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        if !text.trim().is_empty() {
            print!("{text}");
        }
    }
}

#[cfg(test)]
#[path = "tests/story_1052_supervise_nudge_tests.rs"]
mod story_1052_supervise_nudge_tests;

#[cfg(test)]
#[path = "tests/story_1096_supervise_watch_tests.rs"]
mod story_1096_supervise_watch_tests;

#[cfg(test)]
#[path = "tests/bug_1621_watch_redrive_tests.rs"]
mod bug_1621_watch_redrive_tests;
