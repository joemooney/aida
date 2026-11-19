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
    #[serde(default)]
    recovered: bool,
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
        // Unknown cross-session evidence retains an observed wait instead of clearing it.
        let observed_wait = match status {
            Some("waiting" | "blocked") => Some(*waits.entry(wait_key.clone()).or_insert(now)),
            Some("busy" | "idle") => None,
            _ => waits.get(&wait_key).copied(),
        };
        if observed_wait.is_some() {
            waiting.insert(wait_key.clone());
        }
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
        } else if crate::advisor_handshake::is_advisor_session(entry) {
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
        let reviews = crate::review_watch::pending_verdict_count(root, entry)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "unknown".into());
        blocks.insert(wait_key, Block {
            since,
            description: format!("{handle} ({role}), terminal {}: {reason} for {}. Unread mail: {count}; Watched PRs awaiting this seat's verdict: {reviews}. Prompt: {prompt}. Stop re-pinging; escalate to the operator.",
                entry.tty.as_deref().unwrap_or("unknown"), crate::mail_backlog::format_age(now - since)),
        });
    }
    // A stopped relay has no live child: inspect durable faults independently.
    // trace:BUG-2639 | ai:codex
    let runs = crate::relay_mail::list_runs(root);
    if census.presence() == crate::advisor_handshake::AdvisorPresence::Unknown {
        let outage = crate::advisor_handshake::advisor_outage(root, &live, now, &runs);
        if let Some((key, since)) = outage.rotation_exit {
            blocks.insert(
                key,
                Block {
                    since,
                    description: format!(
                        "No advisor running after rotation. {} Relaunch the advisor rotate relay.",
                        outage.detail
                    ),
                },
            );
        }
    }
    match runs {
        Ok(runs) => {
            for run in runs {
                if let Some(detail) = crate::relay_mail::unresolved_fault(&run) {
                    blocks.insert(format!("rotation:{}", run.run_id), Block {
                        since: run.terminal.as_ref().unwrap().at,
                        description: format!("{} rotation stopped: {detail}. Resolve the refusal and relaunch {}.", run.label, run.label),
                    });
                }
            }
        }
        Err(error) => {
            blocks.insert(
                "rotation:unreadable".into(),
                Block {
                    since: now - threshold(root),
                    description: format!(
                        "Cannot read rotation health: {error:#}; inspect relay run records."
                    ),
                },
            );
        }
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
        let alert = ledger.get_mut(&id).expect("recovered alert exists");
        if alert.repeats == 0 {
            ledger.remove(&id);
            continue;
        }
        alert.recovered = true;
        let message = format!(
            "Seat health cleared: the prior block is no longer observed. {}",
            alert.description
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
        // A failed clear can be retried while recovered, but it must not
        // carry backoff into a new block or send a clear during that block.
        if ledger.get(id).is_some_and(|alert| alert.recovered) {
            ledger.remove(id);
        }
        if !id.starts_with("rotation:advisor-")
            && !ledger.contains_key(id)
            && now - block.since < threshold
        {
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
        if alerts.get(&key).is_none_or(|prior| {
            (!alert.recovered, alert.repeats, alert.next)
                >= (!prior.recovered, prior.repeats, prior.next)
        }) {
            alerts.insert(key, alert);
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
                if key.starts_with("rotation:") {
                    key
                } else {
                    key.split(':').next().unwrap_or(&key).to_owned()
                },
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

    #[test]
    fn blocked_seat_alert_counts_only_its_pending_reviews() {
        use crate::agent_registry::{register_spawned_agent_with_row, SpawnedRow};
        use serde_json::json;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let entry = register_spawned_agent_with_row(
            root,
            "claude",
            std::process::id(),
            SpawnedRow {
                role: Some("advisor".into()),
                current_spec: None,
                worktree_path: root.into(),
                binary: None,
                name: Some("blocked-advisor".into()),
                native_session_id: Some("blocked-review-session".into()),
                resumed_from: None,
                description: None,
                delegate_seats: Vec::new(),
                seat: Some("advisor".into()),
            },
        )
        .unwrap();
        let now = entry.started_at + Duration::minutes(30);
        crate::pending_approval::write_marker(
            root,
            "blocked-review-session",
            Some("Bash"),
            Some("Approve this command?"),
            entry.started_at + Duration::minutes(1),
        )
        .unwrap();

        let identity = entry.process_identity().unwrap();
        let reviewer = json!({
            "pid": identity.pid,
            "start": format!("linux-starttime:{}", identity.starttime),
            "boot": identity.boot_id,
            "pid_namespace": identity.pidns_inode,
        });
        let mut other = reviewer.clone();
        // A reused PID must not be attributed to the current process.
        other["start"] = json!(format!("linux-starttime:{}", identity.starttime + 1));
        let head = "a".repeat(40);
        let other_dispatcher = other.clone();
        let review = |phase, reviewer, dispatcher| {
            json!({
                "attempts": [{
                    "head": head, "phase": phase, "token": uuid::Uuid::nil().to_string(),
                    "identity": other_dispatcher, "reviewer": reviewer, "dispatcher": dispatcher,
                    "supersede": (phase == "superseding").then(|| json!({
                        "by": "b".repeat(40), "requested_at": now,
                    })),
                }],
                "notices": {},
            })
        };
        let state_path = root.join(".aida/review-watch/state.json");
        std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        for (key, kind) in [("PR-4", "APPROVED"), ("PR-6", "WAITING")] {
            let verdict = crate::gate::review_verdict::verdict_path(root, key);
            std::fs::create_dir_all(verdict.parent().unwrap()).unwrap();
            std::fs::write(
                &verdict,
                json!({"reviewed_sha": head, "verdict": kind}).to_string(),
            )
            .unwrap();
        }

        let mut state = json!({
            "version": 1, "repository": "github:github.com/owner/repo", "max_concurrent": 6,
            "entries": {
                "PR-1": review("running", reviewer.clone(), serde_json::Value::Null),
                "PR-2": review("running", other, serde_json::Value::Null),
                "PR-3": review("running", serde_json::Value::Null, reviewer.clone()),
                "PR-4": review("running", reviewer.clone(), serde_json::Value::Null),
                "PR-5": review("superseding", reviewer.clone(), serde_json::Value::Null),
                "PR-6": review("running", reviewer, serde_json::Value::Null),
                "PR-7": {
                    "attempts": [], "notices": {},
                    "skipped": {"head": head, "line": "parked", "checked_at": now},
                },
            },
        });
        let assert_alert = |expected| {
            let mut sent = Vec::new();
            reconcile(
                &mut Ledger::new(),
                &inspect(root, now),
                now,
                Duration::minutes(15),
                |_, body| {
                    sent.push(body.to_owned());
                    Ok(true)
                },
            )
            .unwrap();
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(
                sent[0].contains(&format!(
                    "Watched PRs awaiting this seat's verdict: {expected}."
                )),
                "{sent:?}"
            );
        };
        assert_alert("unknown");
        std::fs::write(&state_path, state.to_string()).unwrap();
        std::fs::set_permissions(&state_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_alert("2");
        let entries = state["entries"].as_object_mut().unwrap();
        entries.remove("PR-1");
        entries.remove("PR-6");
        std::fs::write(&state_path, state.to_string()).unwrap();
        assert_alert("0");
    }

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
        for status in [None, Some(""), Some("unknown")] {
            sessions[0].status = status.map(str::to_owned);
            let rows = sessions.as_slice();
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
        let missing_row =
            inspect_with_sessions(root, now + Duration::minutes(17), Some(&[]), &mut waits);
        assert_eq!(missing_row.values().next().unwrap().since, now);
        reconcile(
            &mut ledger,
            &missing_row,
            now + Duration::minutes(17),
            Duration::minutes(15),
            |_, _| panic!("missing row must not clear or re-alert"),
        )
        .unwrap();
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
        assert!(
            !inspect_with_sessions(root, now + Duration::minutes(22), None, &mut waits).is_empty()
        );
        entry.ended_at = Some(now + Duration::minutes(23));
        write_entry(root, &entry).unwrap();
        let ended = inspect_with_sessions(root, now + Duration::minutes(23), None, &mut waits);
        assert!(ended.is_empty());
        assert!(waits.is_empty());
        reconcile(
            &mut ledger,
            &ended,
            now + Duration::minutes(23),
            Duration::minutes(15),
            |title, _| {
                assert_eq!(title, "aida seat health cleared");
                Ok(true)
            },
        )
        .unwrap();
        assert!(ledger.is_empty());
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
    fn failed_clear_does_not_carry_backoff_into_next_block() {
        let now = Utc::now();
        let threshold = Duration::minutes(15);
        let mut blocks = BTreeMap::from([(
            "advisor".into(),
            Block {
                since: now - threshold,
                description: "first prompt".into(),
            },
        )]);
        let mut ledger = Ledger::new();
        for minutes in [0, 15, 45] {
            reconcile(
                &mut ledger,
                &blocks,
                now + Duration::minutes(minutes),
                threshold,
                |_, _| Ok(true),
            )
            .unwrap();
        }
        let recovery = now + Duration::minutes(46);
        for minutes in [0, 1] {
            assert!(reconcile(
                &mut ledger,
                &BTreeMap::new(),
                recovery + Duration::minutes(minutes),
                threshold,
                |title, _| {
                    assert_eq!(title, "aida seat health cleared");
                    anyhow::bail!("offline")
                },
            )
            .is_err());
            // A clear failure survives the scheduled check's ledger reload.
            ledger = serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        }

        let second = recovery + Duration::minutes(2);
        let block = blocks.get_mut("advisor").unwrap();
        block.since = second;
        block.description = "second prompt".into();
        reconcile(&mut ledger, &blocks, second, threshold, |_, _| {
            panic!("new block is below threshold")
        })
        .unwrap();
        assert!(ledger.is_empty());
        reconcile(
            &mut ledger,
            &blocks,
            second + threshold,
            threshold,
            |title, body| {
                assert_eq!(title, "aida seat needs operator input");
                assert_eq!(body, "second prompt");
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(ledger["advisor"].repeats, 1);
        assert_eq!(ledger["advisor"].next, Some(second + threshold * 2));
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

#[cfg(test)]
mod rotation_fault_tests {
    use super::*;
    // trace:BUG-2638 | ai:codex
    #[test]
    fn bug_2638_rotation_without_relay_alerts_once_and_relay_suppresses_it() {
        use crate::agent_registry::{register_spawned_agent_with_row, write_entry, SpawnedRow};
        use crate::seat_rotation::{write_handoff_by, HandoffWriter};

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let handle = "claude-advisor-2638";
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let entry = register_spawned_agent_with_row(
            root,
            "claude",
            child.id(),
            SpawnedRow {
                role: Some("advisor".into()),
                current_spec: None,
                worktree_path: root.into(),
                binary: None,
                name: Some(handle.into()),
                native_session_id: Some("advisor-session-2638".into()),
                resumed_from: None,
                description: None,
                delegate_seats: Vec::new(),
                seat: Some("advisor".into()),
            },
        );
        let presence = crate::advisor_handshake::Census::load(root).presence();
        child.kill().unwrap();
        child.wait().unwrap();
        let mut entry = entry.unwrap();
        assert_eq!(
            presence,
            crate::advisor_handshake::AdvisorPresence::Registered
        );
        let now = entry.started_at + Duration::seconds(4);
        assert!(inspect(root, now).is_empty());

        let writer = HandoffWriter {
            handle: Some(handle.into()),
            native_session_id: entry.native_session_id.clone(),
            ..Default::default()
        };
        for (timestamp, other_writer) in [
            (
                now - Duration::seconds(3),
                HandoffWriter {
                    native_session_id: Some("previous-session".into()),
                    ..writer.clone()
                },
            ),
            (
                now - Duration::seconds(2),
                HandoffWriter {
                    final_: true,
                    ..writer.clone()
                },
            ),
        ] {
            write_handoff_by(
                root,
                "advisor",
                "previous handoff",
                timestamp,
                &other_writer,
            )
            .unwrap();
            assert!(inspect(root, now).is_empty());
        }

        write_handoff_by(
            root,
            "advisor",
            "ready for rotation",
            now - Duration::seconds(1),
            &writer,
        )
        .unwrap();
        let without_receipt = inspect(root, now);
        assert_eq!(without_receipt.len(), 1, "{without_receipt:?}");
        assert!(without_receipt
            .values()
            .next()
            .unwrap()
            .description
            .contains("exit time unknown"));
        entry.ended_at = Some(entry.last_active_at);
        write_entry(root, &entry).unwrap();
        assert_eq!(
            inspect(root, now).keys().collect::<Vec<_>>(),
            without_receipt.keys().collect::<Vec<_>>()
        );
        entry.ended_at = Some(now);
        write_entry(root, &entry).unwrap();
        let mut unrelated = crate::agent_rotation::RotationRun::bind_for_role(
            root,
            "product-relay",
            Some("product"),
        )
        .unwrap();
        let blocks = inspect(root, now);
        assert_eq!(
            blocks.keys().collect::<Vec<_>>(),
            without_receipt.keys().collect::<Vec<_>>()
        );
        let report = || {
            let census = crate::advisor_handshake::Census::load(root);
            crate::advisor_handshake::report_findings(&census, &[], false)
        };
        assert!(report().iter().any(|row| {
            row.summary.starts_with("no advisor running.")
                && row.summary.contains(handle)
                && row.summary.contains(&now.to_rfc3339())
                && row.summary.contains("no advisor rotate relay running")
        }));
        let mut ledger = Ledger::new();
        let mut sent = Vec::new();
        for at in [now, now + Duration::seconds(1)] {
            reconcile(
                &mut ledger,
                &blocks,
                at,
                Duration::minutes(15),
                |_, body| {
                    sent.push(body.to_owned());
                    Ok(true)
                },
            )
            .unwrap();
        }
        assert_eq!(sent.len(), 1);
        assert!(sent[0].contains(handle));
        assert!(sent[0].contains(&now.to_rfc3339()));
        assert!(sent[0].contains("no advisor rotate relay running"));

        let mut relay = crate::agent_rotation::RotationRun::bind_for_role(
            root,
            "new-advisor-handle",
            Some("advisor"),
        )
        .unwrap();
        assert!(inspect(root, now).is_empty());
        assert!(report()
            .iter()
            .any(|row| row.summary.contains("; advisor rotate relay running")));
        relay.end(crate::relay_mail::TerminalReason::Exit);
        unrelated.end(crate::relay_mail::TerminalReason::Exit);
    }

    // trace:BUG-2639 | ai:codex
    #[test]
    fn bug_2639_stopped_relay_alert_delivery_backoff_and_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut run = crate::agent_rotation::RotationRun::bind(root, "claude-advisor-08").unwrap();
        run.block(&anyhow::anyhow!("allocation retry failed"))
            .unwrap();
        drop(run);
        let now = Utc::now() + Duration::minutes(16);
        let blocks = inspect(root, now);
        assert_eq!(blocks.len(), 1);
        assert!(blocks
            .values()
            .next()
            .unwrap()
            .description
            .contains("allocation retry failed"));
        let mut ledger = Ledger::new();
        let mut sent = Vec::new();
        // Failed or unavailable delivery must not consume the initial alert.
        assert!(reconcile(
            &mut ledger,
            &blocks,
            now,
            Duration::minutes(15),
            |_, _| anyhow::bail!("notification transport unavailable"),
        )
        .is_err());
        assert!(ledger.values().all(|alert| alert.next.is_none()));
        reconcile(&mut ledger, &blocks, now, Duration::minutes(15), |_, _| {
            Ok(false)
        })
        .unwrap();
        assert!(ledger.values().all(|alert| alert.next.is_none()));
        reconcile(
            &mut ledger,
            &blocks,
            now,
            Duration::minutes(15),
            |title, body| {
                sent.push((title.to_owned(), body.to_owned()));
                Ok(true)
            },
        )
        .unwrap();
        assert_eq!(sent.len(), 1);
        reconcile(
            &mut ledger,
            &blocks,
            now + Duration::minutes(1),
            Duration::minutes(15),
            |_, _| panic!("backoff"),
        )
        .unwrap();
        reconcile(
            &mut ledger,
            &blocks,
            now + Duration::minutes(15),
            Duration::minutes(15),
            |_, _| Ok(true),
        )
        .unwrap();
        assert_eq!(ledger.values().next().unwrap().repeats, 2);
        let mut successor =
            crate::agent_rotation::RotationRun::bind(root, "claude-advisor-08").unwrap();
        assert!(inspect(root, now).is_empty());
        reconcile(
            &mut ledger,
            &inspect(root, now),
            now,
            Duration::minutes(15),
            |title, _| {
                assert_eq!(title, "aida seat health cleared");
                Ok(true)
            },
        )
        .unwrap();
        assert!(ledger.is_empty());
        successor.end(crate::relay_mail::TerminalReason::Exit);
    }
}
