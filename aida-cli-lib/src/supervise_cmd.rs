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
        let EventKind::SpecShelved { phase, kind } = event.kind else {
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
    let msg = Message {
        id: id.clone(),
        thread_id: id,
        from: crate::current_user_id(None),
        to: Recipient::Agent(advisor.to_string()),
        timestamp: Utc::now().timestamp_millis(),
        in_reply_to: None,
        body,
        urgent: true,
        intent: Intent::Request,
        retracted: false,
        deleted: false,
        archived: false,
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

#[cfg(test)]
#[path = "tests/story_1052_supervise_nudge_tests.rs"]
mod story_1052_supervise_nudge_tests;
