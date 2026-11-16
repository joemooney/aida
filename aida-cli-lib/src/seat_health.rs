//! Model-free operator escalation for confirmed input blocks and failed handshakes.
//! trace:BUG-2631 | ai:codex
use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
struct Block {
    since: DateTime<Utc>,
    description: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Alert {
    description: String,
    next: Option<DateTime<Utc>>,
    repeats: u32,
}

type Ledger = BTreeMap<String, Alert>;

#[derive(Default, Serialize, Deserialize)]
struct State {
    alerts: Ledger,
    waits: BTreeMap<String, DateTime<Utc>>,
}

fn threshold(root: &Path) -> Duration {
    let config = crate::read_project_config_value(root);
    let minutes = config
        .as_ref()
        .and_then(|c| c.get("seat_health"))
        .and_then(|c| c.get("blocked_after_mins"))
        .and_then(toml::Value::as_integer)
        .filter(|n| (1..=10080).contains(n))
        .unwrap_or(15);
    Duration::minutes(minutes)
}

/// Read the same subject-bound unread evidence as the inbox, including role mail.
fn unread(root: &Path, store: &Path, id: &str) -> Option<usize> {
    use aida_core::mailbox_logical::{inspect_mail, InspectionState, MailboxContext, ReadLimits};
    let home = crate::mailbox_store::mailbox_home(root);
    let directory = crate::mailbox_resolver::RecipientDirectory::load(home.root(), store).ok()?;
    let ctx = MailboxContext::resolve_or_local(home.root(), Some(store)).ok()?;
    for (entry, subject) in directory.inspection_readers() {
        if entry.id != id {
            continue;
        }
        let inspection = inspect_mail(&ctx, &subject.ok()?, &ReadLimits::explicit_read());
        if !inspection.complete || !inspection.unknown_ids.is_empty() {
            return None;
        }
        return Some(
            inspection
                .records
                .iter()
                .filter(|r| r.state == InspectionState::Unread)
                .count(),
        );
    }
    None
}

#[cfg(test)]
fn inspect(root: &Path, now: DateTime<Utc>) -> BTreeMap<String, Block> {
    inspect_with_sessions(root, now, None, &mut BTreeMap::new())
}

fn inspect_with_sessions(
    root: &Path,
    now: DateTime<Utc>,
    sessions: Option<&[crate::claude_agents::ClaudeAgentEntry]>,
    waits: &mut BTreeMap<String, DateTime<Utc>>,
) -> BTreeMap<String, Block> {
    let live = crate::agent_registry::load_live_identity_entries(root);
    let store =
        crate::detect_distributed_store_from(root).unwrap_or_else(|| root.join(".aida-store"));
    let census = crate::advisor_handshake::Census::new(
        root,
        store.clone(),
        live.clone(),
        crate::advisor_handshake::Policy::load(root),
        now,
    );
    let failed_since = live
        .iter()
        .filter_map(|e| census.unreachable(e.name.as_deref(), e.started_at))
        .map(|(record, _)| record.sent_at)
        .min();
    let mut blocks = BTreeMap::new();
    let mut waiting = std::collections::BTreeSet::new();
    for entry in &live {
        let Some(role) = entry.role.as_deref() else {
            continue;
        };
        let session = entry
            .native_session_id
            .as_deref()
            .or(entry.claude_session_id.as_deref());
        // Markers are local to the session checkout; also check the shared primary.
        let marker = session.and_then(|session| {
            crate::pending_approval::for_live_session(&entry.worktree_path, session)
                .into_iter()
                .chain(crate::pending_approval::for_live_session(root, session))
                .filter(|m| m.since >= entry.started_at)
                .min_by_key(|m| m.since)
        });
        let wait_key = format!("{}:{}", entry.id, entry.started_at.timestamp());
        let cross = sessions.and_then(|rows| {
            rows.iter()
                .find(|row| row.pid == entry.pid && session == Some(row.session_id.as_str()))
        });
        let status = cross.and_then(|row| row.status.as_deref());
        let cross_waiting = matches!(status, Some("waiting" | "blocked"));
        // Unknown cross-session evidence retains an observed wait instead of clearing it.
        let observed_wait = if cross_waiting || (status.is_none() && waits.contains_key(&wait_key))
        {
            waiting.insert(wait_key.clone());
            Some(*waits.entry(wait_key.clone()).or_insert(now))
        } else {
            None
        };
        let (since, reason, prompt) = if let Some(marker) = marker {
            (
                marker.since,
                "blocked on operator input",
                marker.message.unwrap_or_else(|| "unknown".into()),
            )
        } else if let Some(since) = observed_wait {
            let prompt = cross
                .and_then(|row| row.prompt.as_ref())
                .and_then(|p| p.as_str())
                .map(|p| p.chars().take(200).collect())
                .unwrap_or_else(|| "unknown".into());
            (
                since,
                "waiting for operator input (cross-session status)",
                prompt,
            )
        } else if role == "advisor" {
            let Some(since) = failed_since else {
                continue;
            };
            (
                since.max(entry.started_at),
                "advisor handshake failed; operator attention required",
                "unknown".into(),
            )
        } else {
            continue;
        };
        let handle = crate::mail_backlog::handle(entry);
        let count = unread(root, &store, &entry.id)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into());
        let reviews = crate::review_watch::pending_verdict_count(root)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into());
        blocks.insert(wait_key, Block {
            since,
            description: format!("{handle} ({role}), terminal {}: {reason} for {}. Unread mail: {count}; project watched PRs awaiting verdict: {reviews}. Prompt: {prompt}. Stop re-pinging; escalate to the operator.",
                entry.tty.as_deref().unwrap_or("unknown"), crate::mail_backlog::format_age(now - since)),
        });
    }
    waits.retain(|key, _| waiting.contains(key));
    blocks
}

/// Persist successful delivery only; repeat after 15, 30, 60, then 120 minutes.
fn reconcile(
    ledger: &mut Ledger,
    blocks: &BTreeMap<String, Block>,
    now: DateTime<Utc>,
    threshold: Duration,
    mut send: impl FnMut(&str, &str) -> Result<bool>,
) -> Result<()> {
    let mut errors = Vec::new();
    let recovered: Vec<_> = ledger
        .keys()
        .filter(|id| !blocks.contains_key(*id))
        .cloned()
        .collect();
    for id in recovered {
        if ledger[&id].repeats == 0 {
            ledger.remove(&id);
            continue;
        }
        let message = format!(
            "Seat health cleared: the prior block is no longer observed. {}",
            ledger[&id].description
        );
        match send("aida seat health cleared", &message) {
            Ok(true) => {
                ledger.remove(&id);
            }
            Ok(false) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    for (id, block) in blocks {
        if !ledger.contains_key(id) && now - block.since < threshold {
            continue;
        }
        let alert = ledger.entry(id.clone()).or_default();
        alert.description.clone_from(&block.description);
        if alert.next.is_some_and(|next| now < next) {
            continue;
        }
        match send("aida seat needs operator input", &block.description) {
            Ok(true) => {
                let minutes = 15 * (1_i64 << alert.repeats.min(3));
                alert.next = Some(now + Duration::minutes(minutes));
                alert.repeats = alert.repeats.saturating_add(1);
            }
            Ok(false) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }
    anyhow::ensure!(
        errors.is_empty(),
        "seat health delivery failed: {}",
        errors.join("; ")
    );
    Ok(())
}

/// Called by scheduled `notify check`, independently of the blocked model's turns.
pub(crate) fn check(checkout: &Path) -> Result<()> {
    let home = crate::mailbox_store::mailbox_home(checkout);
    let root = crate::main_worktree_root_from(home.root());
    let dir = root.join(".aida/seat-health");
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("lock"))?;
    if let Err(error) = lock.try_lock_exclusive() {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(());
        }
        return Err(error.into());
    }
    let path = dir.join("alerts.json");
    let mut state: State = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid seat health ledger")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(e.into()),
    };
    // Older ledgers included the evidence timestamp. Retain the most advanced
    // delivery state when upgrading to one episode per live session.
    let mut alerts = Ledger::new();
    for (key, alert) in std::mem::take(&mut state.alerts) {
        let key = key.split(':').take(2).collect::<Vec<_>>().join(":");
        let prior = alerts.entry(key).or_insert_with(Alert::default);
        if (alert.repeats, alert.next) >= (prior.repeats, prior.next) {
            *prior = alert;
        }
    }
    state.alerts = alerts;
    let now = Utc::now();
    let has_sessions = crate::agent_registry::load_live_identity_entries(&root)
        .iter()
        .any(|entry| {
            entry.role.is_some()
                && (entry.native_session_id.is_some() || entry.claude_session_id.is_some())
        });
    let sessions = if has_sessions {
        crate::claude_agents::list_agents()
    } else {
        Some(Vec::new())
    };
    let blocks = inspect_with_sessions(&root, now, sessions.as_deref(), &mut state.waits);
    let result = reconcile(
        &mut state.alerts,
        &blocks,
        now,
        threshold(&root),
        |title, body| crate::notify::send_health_alert(&root, title, body),
    );
    // Preserve successful earlier sends even when a later delivery failed.
    std::fs::write(dir.join("alerts.tmp"), serde_json::to_vec_pretty(&state)?)?;
    std::fs::rename(dir.join("alerts.tmp"), path)?;
    result
}

/// Peer listing is read-only and shows current evidence, without sending alerts.
pub(crate) fn notes_by_id(root: &Path) -> BTreeMap<String, String> {
    let home = crate::mailbox_store::mailbox_home(root);
    let root = home.root();
    let mut state: State = std::fs::read(root.join(".aida/seat-health/alerts.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    inspect_with_sessions(root, Utc::now(), None, &mut state.waits)
        .into_iter()
        .map(|(key, block)| {
            (
                key.split(':').next().unwrap_or(&key).to_owned(),
                block.description,
            )
        })
        .collect()
}

pub(crate) fn notes(root: &Path) -> Vec<String> {
    notes_by_id(root).into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercise the real registry/marker join, including the old 30-minute expiry.
    #[test]
    fn live_advisor_prompt_survives_five_hours_and_clears() {
        use crate::agent_registry::{register_spawned_agent_with_row, write_entry, SpawnedRow};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let now = Utc::now();
        let mut entry = register_spawned_agent_with_row(
            root,
            "claude",
            std::process::id(),
            SpawnedRow {
                role: Some("advisor".into()),
                current_spec: None,
                worktree_path: root.into(),
                binary: None,
                name: Some("claude-advisor-2631".into()),
                native_session_id: Some("blocked-advisor-session".into()),
                resumed_from: None,
                description: None,
                delegate_seats: Vec::new(),
                seat: Some("advisor".into()),
            },
        )
        .unwrap();
        entry.started_at = now - Duration::hours(6);
        entry.tty = Some("/dev/pts/7".into());
        write_entry(root, &entry).unwrap();
        crate::pending_approval::write_marker(
            root,
            "blocked-advisor-session",
            Some("Bash"),
            Some("Approve this command?"),
            now - Duration::hours(5),
        )
        .unwrap();
        assert_eq!(crate::pending_approval::list_active(root, now).len(), 1);
        let blocks = inspect(root, now);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        let block = blocks.values().next().unwrap();
        assert!(block.description.contains("/dev/pts/7"));
        assert!(block.description.contains("Approve this command?"));
        assert!(block.description.contains("blocked on operator input"));
        crate::pending_approval::clear_marker(root, "blocked-advisor-session").unwrap();
        assert!(inspect(root, now).is_empty());

        let mut sessions: Vec<crate::claude_agents::ClaudeAgentEntry> = serde_json::from_value(serde_json::json!([{
            "pid": std::process::id(), "cwd": root, "kind": "interactive",
            "startedAt": entry.started_at.timestamp_millis(), "sessionId": "blocked-advisor-session",
            "status": "waiting", "prompt": "Choose an option"
        }])).unwrap();
        let mut waits = BTreeMap::new();
        let first = inspect_with_sessions(root, now, Some(&sessions), &mut waits);
        assert_eq!(first.values().next().unwrap().since, now);
        let later = inspect_with_sessions(
            root,
            now + Duration::minutes(16),
            Some(&sessions),
            &mut waits,
        );
        assert_eq!(later.values().next().unwrap().since, now);
        assert!(later
            .values()
            .next()
            .unwrap()
            .description
            .contains("Choose an option"));
        assert!(
            !inspect_with_sessions(root, now, None, &mut waits).is_empty(),
            "unknown observation must retain a prior wait"
        );
        // Missing live rows and optional status are unknown, even on success.
        let mut ledger = Ledger::new();
        reconcile(
            &mut ledger,
            &later,
            now + Duration::minutes(16),
            Duration::minutes(15),
            |_, _| Ok(true),
        )
        .unwrap();
        let next = ledger.values().next().unwrap().next;
        for missing_status in [false, true] {
            sessions[0].status = None;
            let rows = if missing_status {
                sessions.as_slice()
            } else {
                &[]
            };
            let unknown =
                inspect_with_sessions(root, now + Duration::minutes(17), Some(rows), &mut waits);
            assert_eq!(unknown.values().next().unwrap().since, now);
            reconcile(
                &mut ledger,
                &unknown,
                now + Duration::minutes(17),
                Duration::minutes(15),
                |_, _| panic!("unknown evidence must not clear or re-alert"),
            )
            .unwrap();
            assert_eq!(ledger.values().next().unwrap().next, next);
        }
        // A marker replacing a wait stays in the same session episode.
        crate::pending_approval::write_marker(
            root,
            "blocked-advisor-session",
            None,
            Some("new prompt"),
            now + Duration::minutes(17),
        )
        .unwrap();
        let marker = inspect_with_sessions(
            root,
            now + Duration::minutes(18),
            Some(&sessions),
            &mut waits,
        );
        assert_eq!(
            marker.keys().collect::<Vec<_>>(),
            later.keys().collect::<Vec<_>>()
        );
        reconcile(
            &mut ledger,
            &marker,
            now + Duration::minutes(18),
            Duration::minutes(15),
            |_, _| panic!("source change must preserve backoff"),
        )
        .unwrap();
        crate::pending_approval::clear_marker(root, "blocked-advisor-session").unwrap();
        sessions[0].status = Some("busy".into());
        assert!(inspect_with_sessions(root, now, Some(&sessions), &mut waits).is_empty());
        assert!(waits.is_empty());

        let sender = register_spawned_agent_with_row(
            root,
            "codex",
            std::process::id(),
            SpawnedRow {
                role: Some("implementer".into()),
                current_spec: None,
                worktree_path: root.into(),
                binary: None,
                name: Some("codex-implementer-2631".into()),
                native_session_id: None,
                resumed_from: None,
                description: None,
                delegate_seats: Vec::new(),
                seat: None,
            },
        )
        .unwrap();
        let mut hello = crate::advisor_handshake::HelloRecord {
            handle: sender.name.clone().unwrap(),
            role: "implementer".into(),
            spec: None,
            path: root.display().to_string(),
            hello_id: "health-test-hello".into(),
            sent_at: sender.started_at,
            mailbox: root.display().to_string(),
            home: crate::advisor_handshake::HomeKind::Checkout,
            advisor_live: true,
            acked_at: None,
            acked_by: None,
            ack_reported: false,
            rotation: None,
        };
        crate::advisor_handshake::save_record(root, &hello).unwrap();
        let failed = inspect(root, now + Duration::minutes(16));
        assert_eq!(failed.len(), 1);
        assert!(failed
            .values()
            .next()
            .unwrap()
            .description
            .contains("advisor handshake failed"));
        let mut ledger = Ledger::new();
        reconcile(
            &mut ledger,
            &failed,
            now + Duration::minutes(16),
            Duration::minutes(15),
            |_, _| Ok(true),
        )
        .unwrap();
        // A second outstanding hello remains after the oldest is acknowledged.
        let mut second = hello.clone();
        second.handle = entry.name.clone().unwrap();
        second.hello_id = "health-second-hello".into();
        second.sent_at = now + Duration::minutes(1);
        crate::advisor_handshake::save_record(root, &second).unwrap();
        hello.acked_at = Some(now + Duration::minutes(17));
        crate::advisor_handshake::save_record(root, &hello).unwrap();
        let remaining = inspect(root, now + Duration::minutes(18));
        assert_eq!(remaining.len(), 1);
        assert_ne!(
            failed.values().next().unwrap().since,
            remaining.values().next().unwrap().since
        );
        reconcile(
            &mut ledger,
            &remaining,
            now + Duration::minutes(18),
            Duration::minutes(15),
            |_, _| panic!("acking oldest hello must not reset episode"),
        )
        .unwrap();
        // Handshake -> marker -> wait also preserves the alert episode.
        crate::pending_approval::write_marker(root, "blocked-advisor-session", None, None, now)
            .unwrap();
        let marked = inspect(root, now + Duration::minutes(19));
        reconcile(
            &mut ledger,
            &marked,
            now + Duration::minutes(19),
            Duration::minutes(15),
            |_, _| panic!("handshake to marker reset"),
        )
        .unwrap();
        crate::pending_approval::clear_marker(root, "blocked-advisor-session").unwrap();
        sessions[0].status = Some("waiting".into());
        let waited = inspect_with_sessions(
            root,
            now + Duration::minutes(20),
            Some(&sessions),
            &mut waits,
        );
        reconcile(
            &mut ledger,
            &waited,
            now + Duration::minutes(20),
            Duration::minutes(15),
            |_, _| panic!("marker to wait reset"),
        )
        .unwrap();
        second.acked_at = Some(now + Duration::minutes(21));
        crate::advisor_handshake::save_record(root, &second).unwrap();
        assert!(inspect(root, now + Duration::minutes(22)).is_empty());
    }

    #[test]
    fn blocked_advisor_backoff_and_recovery() {
        let now = Utc::now();
        let mut ledger = Ledger::new();
        let mut blocks = BTreeMap::from([(
            "advisor".into(),
            Block {
                since: now - Duration::minutes(16),
                description: "advisor pts/7 blocked: approve Bash".into(),
            },
        )]);
        let mut messages = Vec::new();
        let mut tick = |ledger: &mut Ledger, blocks: &BTreeMap<String, Block>, at| {
            reconcile(ledger, blocks, at, Duration::minutes(15), |title, body| {
                messages.push(format!("{title}: {body}"));
                Ok(true)
            })
            .unwrap();
        };
        tick(&mut ledger, &blocks, now);
        tick(&mut ledger, &blocks, now + Duration::minutes(14));
        tick(&mut ledger, &blocks, now + Duration::minutes(15));
        tick(&mut ledger, &blocks, now + Duration::minutes(44));
        tick(&mut ledger, &blocks, now + Duration::minutes(45));
        blocks.clear();
        tick(&mut ledger, &blocks, now + Duration::minutes(46));
        tick(&mut ledger, &blocks, now + Duration::minutes(47));
        assert_eq!(messages.len(), 4);
        assert!(messages[3].contains("cleared"));
        assert!(messages[0].contains("pts/7"));
        assert!(ledger.is_empty());
    }

    #[test]
    fn below_threshold_and_failed_delivery_retry() {
        let now = Utc::now();
        let mut ledger = Ledger::new();
        let blocks = BTreeMap::from([(
            "advisor".into(),
            Block {
                since: now,
                description: "blocked".into(),
            },
        )]);
        reconcile(&mut ledger, &blocks, now, Duration::minutes(15), |_, _| {
            panic!("too early")
        })
        .unwrap();
        assert!(reconcile(
            &mut ledger,
            &blocks,
            now + Duration::minutes(15),
            Duration::minutes(15),
            |_, _| anyhow::bail!("offline")
        )
        .is_err());
        assert!(ledger["advisor"].next.is_none());
        reconcile(
            &mut ledger,
            &blocks,
            now + Duration::minutes(16),
            Duration::minutes(15),
            |_, _| Ok(true),
        )
        .unwrap();
        assert_eq!(ledger["advisor"].repeats, 1);
    }
}
