//! Local hook observations. The worktree log is writable even when the seat
//! has only read access to the main checkout and its lease registry.
// trace:STORY-2748 | ai:codex

use crate::events::{self, Event, EventKind};
use crate::SessionLease;
use aida_core::liveness::LeaseState;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

const INPUT_LIMIT: usize = 256 * 1024;
const LOG_WINDOW: u64 = 1024 * 1024;
const INTERVAL: Duration = Duration::from_secs(15);
const STALE_SECS: i64 = 10 * 60;

#[derive(Deserialize)]
struct HookInput {
    hook_event_name: HookEvent,
    tool_name: Option<String>,
    tool_input: Option<serde_json::Value>,
}

#[derive(Deserialize)]
enum HookEvent {
    PostToolUse,
    SessionStart,
    Stop,
    SubagentStop,
}

pub(crate) fn handle(vendor: &str, id: Option<&std::ffi::OsStr>) {
    if events::events_disabled() {
        return;
    }
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let _ = record_hook(vendor, io::stdin().lock(), &cwd, id, &crate::hostname());
}

fn record_hook(
    vendor: &str,
    input: impl Read,
    cwd: &Path,
    id: Option<&std::ffi::OsStr>,
    host: &str,
) -> Option<()> {
    let input = parse_input(input)?;
    let cwd = cwd.canonicalize().ok()?;
    let worktree = crate::find_project_root_from(&cwd).ok()?;
    // Use the linked registry where available to keep Git off the hook path.
    let registry = if worktree.join(".aida/sessions").is_dir() {
        worktree.clone()
    } else {
        crate::main_worktree_root_from(&worktree)
    };
    let lease = if let Some(id) = id {
        // The launcher's ID disambiguates overlapping roles in one worktree.
        crate::leases::read_exact_lease(&registry, id.to_str()?).ok()??
    } else {
        let mut matches = crate::list_leases(&registry)
            .into_iter()
            .filter(|lease| lease.hostname == host && lease.worktree_path == worktree);
        let lease = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        lease
    };
    if host.is_empty()
        || host == "unknown"
        || lease.hostname != host
        || lease.worktree_path != worktree
    {
        return None;
    }
    let event = make_event(&lease, vendor, input);
    append(&worktree, &lease.id, &event).ok()
}

fn parse_input(input: impl Read) -> Option<HookInput> {
    let mut bytes = Vec::new();
    input
        .take((INPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > INPUT_LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

fn make_event(lease: &SessionLease, vendor: &str, input: HookInput) -> Event {
    let tool = input.tool_name.as_deref().and_then(tool_label);
    let path = input.tool_input.as_ref().and_then(|params| {
        let raw = params
            .get("file_path")
            .or_else(|| params.get("path"))?
            .as_str()?;
        let raw = Path::new(raw);
        let relative = if raw.is_absolute() {
            raw.strip_prefix(&lease.worktree_path).ok()?
        } else {
            raw
        };
        let path = relative.to_str()?;
        if path.is_empty()
            || path.len() > 256
            || path.chars().any(char::is_control)
            || relative
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return None;
        }
        Some(path.to_owned())
    });
    let summary = match input.hook_event_name {
        HookEvent::PostToolUse => tool.as_ref().map_or_else(
            || "Tool finished".to_string(),
            |name| format!("{name} finished"),
        ),
        HookEvent::SessionStart => "Session started".to_string(),
        HookEvent::Stop => "Turn finished".to_string(),
        HookEvent::SubagentStop => "Subagent finished".to_string(),
    };
    let mut event = Event::new(
        bounded_label(&lease.scope),
        "",
        EventKind::ActivityHeartbeat {
            lease_id: lease.id.clone(),
            host: lease.hostname.clone(),
            vendor: vendor.into(),
            tool,
            path,
            summary,
        },
    );
    event.seat = lease.role.as_deref().and_then(bounded_label);
    event
}

fn bounded_label(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.to_owned())
}

fn tool_label(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte)))
    .then(|| value.to_owned())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

fn runtime_dir(worktree: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(worktree.join(".aida"))
}

fn open_leaf(dir: &File, name: &str, mut options: OpenOptions) -> io::Result<File> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd())).join(name);
    let file = options
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(io::Error::other(
            "activity path is not a singly linked regular file",
        ));
    }
    Ok(file)
}

fn append(worktree: &Path, id: &str, event: &Event) -> io::Result<()> {
    if !valid_id(id) {
        return Err(io::Error::other("invalid activity lease ID"));
    }
    let dir = runtime_dir(worktree)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600);
    let mut stamp = open_leaf(&dir, &format!("activity-{id}.stamp"), options)?;
    fs2::FileExt::try_lock_exclusive(&stamp)?;
    let meta = stamp.metadata()?;
    let now = SystemTime::from(event.ts);
    if meta.len() != 0 && now.duration_since(meta.modified()?).unwrap_or_default() < INTERVAL {
        return Ok(());
    }

    let mut options = OpenOptions::new();
    options.append(true).create(true).mode(0o600);
    let log = open_leaf(&dir, "events.jsonl", options)?;
    events::append_event(log, event)?;
    if meta.len() == 0 {
        stamp.write_all(b"1")?;
    }
    stamp.set_modified(now)?;
    Ok(())
}

// Snapshots are bounded per worktree and read once even when multiple rows
// share the worktree. Heartbeats never change how ps classifies a lease.
pub(crate) fn read_for_ps(
    rows: &[crate::ps_integrate::PsRow],
    host: &str,
    now: DateTime<Utc>,
) -> HashMap<String, Activity> {
    let mut logs = HashMap::new();
    rows.iter()
        .filter(|row| {
            row.state == LeaseState::Live
                && row.pid.is_some()
                && !host.is_empty()
                && host != "unknown"
                && row.lease.hostname == host
                && row.lease.worktree_path.is_absolute()
        })
        .map(|row| {
            let log = logs
                .entry(&row.lease.worktree_path)
                .or_insert_with(|| read_latest(&row.lease.worktree_path, host, now));
            let since = row
                .pid_started_at
                .unwrap_or(row.lease.started_at)
                .max(row.lease.started_at);
            (
                row.lease.id.clone(),
                activity(log.get(&row.lease.id), since, now),
            )
        })
        .collect()
}

fn read_latest(
    worktree: &Path,
    host: &str,
    now: DateTime<Utc>,
) -> HashMap<String, (DateTime<Utc>, String)> {
    let mut latest = HashMap::new();
    let Ok(dir) = runtime_dir(worktree) else {
        return latest;
    };
    for name in ["events.jsonl.1", "events.jsonl"] {
        let mut options = OpenOptions::new();
        options.read(true);
        let Ok(mut file) = open_leaf(&dir, name, options) else {
            continue;
        };
        let Ok(end) = file.seek(SeekFrom::End(0)) else {
            continue;
        };
        let start = end.saturating_sub(LOG_WINDOW);
        let mut data = Vec::new();
        if file.seek(SeekFrom::Start(start)).is_err()
            || file.take(end - start).read_to_end(&mut data).is_err()
        {
            continue;
        }
        let lines = data
            .split_inclusive(|byte| *byte == b'\n')
            .skip(usize::from(start != 0));
        for line in lines.filter(|line| line.ends_with(b"\n") && line.len() <= 4096) {
            let Ok(Event {
                ts,
                kind:
                    EventKind::ActivityHeartbeat {
                        lease_id,
                        host: source,
                        summary,
                        ..
                    },
                ..
            }) = serde_json::from_slice(line)
            else {
                continue;
            };
            if source == host
                && ts <= now
                && valid_id(&lease_id)
                && summary.len() <= 112
                && summary.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
                && !summary.trim().is_empty()
                && latest
                    .get(&lease_id)
                    .is_none_or(|(previous, _)| ts > *previous)
            {
                latest.insert(lease_id, (ts, summary));
            }
        }
    }
    latest
}

#[derive(serde::Serialize)]
pub(crate) struct Activity {
    pub(crate) summary: Option<String>,
    pub(crate) age_secs: Option<u64>,
    pub(crate) stale: bool,
}

impl Activity {
    pub(crate) fn age_label(&self) -> String {
        let age = self
            .age_secs
            .map_or_else(|| "-".to_string(), crate::humanize_duration_secs);
        if self.stale {
            format!("{age} (stale)")
        } else {
            age
        }
    }
}

fn activity(
    heartbeat: Option<&(DateTime<Utc>, String)>,
    since: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Activity {
    let heartbeat = heartbeat.filter(|(ts, _)| *ts >= since);
    let age = now.signed_duration_since(heartbeat.map_or(since, |(ts, _)| *ts));
    Activity {
        summary: heartbeat.map(|(_, summary)| summary.clone()),
        age_secs: heartbeat.map(|_| age.num_seconds() as u64),
        stale: age > chrono::Duration::seconds(STALE_SECS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::leases::bug_1680_salvage_main_tests::mock_ps_row;

    fn row(root: &Path, id: &str, host: &str, now: DateTime<Utc>) -> crate::ps_integrate::PsRow {
        let mut row = mock_ps_row(id, "STORY-2748", root.to_path_buf());
        row.lease.hostname = host.into();
        row.lease.started_at = now - chrono::Duration::minutes(30);
        row.state = LeaseState::Live;
        row.pid = Some(1);
        row
    }

    // trace:STORY-2748 | ai:codex
    #[test]
    fn story2748_hook_events_are_bounded_and_throttled_per_lease() {
        use aida_core::vendors::PROFILES;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        let now = Utc::now();
        for (index, vendor) in PROFILES[..2].iter().enumerate() {
            let id = format!("lease-{index}");
            let row = row(root.path(), &id, "local", now);
            let payload = br#"{
                "hook_event_name":"PostToolUse","tool_name":"Edit",
                "tool_input":{"file_path":"src/main.rs","command":"secret command"},
                "tool_response":"secret output","prompt":"secret prompt"
            }"#;
            let mut event = make_event(
                &row.lease,
                crate::vendors::heartbeat_vendor_name(vendor.launch_name)
                    .unwrap()
                    .as_str(),
                parse_input(&payload[..]).unwrap(),
            );
            event.ts = now;
            append(root.path(), &id, &event).unwrap();
            event.ts += chrono::Duration::seconds(14);
            append(root.path(), &id, &event).unwrap();
            event.ts += chrono::Duration::seconds(1);
            append(root.path(), &id, &event).unwrap();
        }
        let observed = events::read_all(root.path());
        assert_eq!(observed.len(), 4);
        for (index, event) in observed.iter().enumerate() {
            assert_eq!(event.spec.as_deref(), Some("STORY-2748"));
            assert_eq!(event.seat.as_deref(), Some("implementer"));
            assert!(!event.kind.is_actionable());
            assert!(!crate::timing_record::lifecycle_publishable(&event.kind));
            assert!(matches!(&event.kind, EventKind::ActivityHeartbeat {
                lease_id, host, vendor, tool: Some(tool), path: Some(path), summary,
            } if lease_id == &format!("lease-{}", index / 2) && host == "local"
                && vendor == PROFILES[index / 2].launch_name && tool == "Edit"
                && path == "src/main.rs" && summary == "Edit finished"));
        }
        let raw = std::fs::read_to_string(events::events_path(root.path())).unwrap();
        assert!(!raw.contains("secret"));
        assert!(parse_input(&vec![b' '; INPUT_LIMIT + 1][..]).is_none());
        assert!(parse_input(&b"{\"hook_event_name\":\"Unknown\"}"[..]).is_none());
    }

    #[test]
    fn story2748_hook_attributes_only_the_selected_local_worktree_lease() {
        use std::ffi::OsStr;
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().canonicalize().unwrap();
        std::fs::create_dir(worktree.join(".git")).unwrap();
        std::fs::create_dir_all(worktree.join(".aida/sessions")).unwrap();
        let now = Utc::now();
        for (index, vendor) in aida_core::vendors::PROFILES[..2].iter().enumerate() {
            let id = format!("vendor-{index}");
            let row = row(&worktree, &id, "local", now);
            std::fs::write(
                crate::leases::lease_path(&worktree, &id),
                toml::to_string(&row.lease).unwrap(),
            )
            .unwrap();
            let payload =
                &br#"{"hook_event_name":"SubagentStop","tool_input":{"path":"../secret"}}"#[..];
            let record = |host| {
                record_hook(
                    vendor.launch_name,
                    payload,
                    &worktree,
                    Some(OsStr::new(&id)),
                    host,
                )
            };
            assert!(record("other-host").is_none());
            assert!(record("local").is_some());
            assert!(record("local").is_some());
        }
        let events = events::read_all(&worktree);
        assert_eq!(events.len(), 2);
        for event in events {
            assert!(matches!(event.kind, EventKind::ActivityHeartbeat {
                path: None, summary, ..
            } if summary == "Subagent finished"));
        }
    }

    #[test]
    fn story2748_hook_and_ps_refuse_linked_event_logs() {
        let root = tempfile::tempdir().unwrap();
        let runtime = root.path().join(".aida");
        std::fs::create_dir(&runtime).unwrap();
        let now = Utc::now();
        let row = row(root.path(), "local", "here", now);
        let mut event = make_event(
            &row.lease,
            aida_core::vendors::PROFILES[0].launch_name,
            HookInput {
                hook_event_name: HookEvent::SessionStart,
                tool_name: None,
                tool_input: None,
            },
        );
        event.ts = now;
        let decoy = root.path().join("decoy");
        std::fs::write(&decoy, serde_json::to_string(&event).unwrap() + "\n").unwrap();
        std::os::unix::fs::symlink(&decoy, events::events_path(root.path())).unwrap();
        assert!(append(root.path(), &row.lease.id, &event).is_err());
        assert!(read_latest(root.path(), "here", now).is_empty());
        assert_eq!(std::fs::read_to_string(&decoy).unwrap().lines().count(), 1);
    }

    #[test]
    fn story2748_ps_projects_age_for_live_local_leases_only() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".aida")).unwrap();
        let now = Utc::now();
        let local = row(root.path(), "local", "here", now);
        let foreign = row(root.path(), "foreign", "elsewhere", now);
        let mut adopted = row(root.path(), "adopted", "here", now);
        adopted.pid_started_at = Some(now - chrono::Duration::seconds(5));
        let mut dead = row(root.path(), "dead", "here", now);
        dead.state = LeaseState::Stale;
        let missing = row(root.path(), "missing", "here", now);
        for row in [&local, &foreign, &adopted, &dead] {
            let mut event = make_event(
                &row.lease,
                aida_core::vendors::PROFILES[0].launch_name,
                parse_input(&b"{\"hook_event_name\":\"Stop\"}"[..]).unwrap(),
            );
            event.ts = now - chrono::Duration::seconds(70);
            append(root.path(), &row.lease.id, &event).unwrap();
        }
        let state = read_for_ps(&[local, foreign, adopted, dead, missing], "here", now);
        assert_eq!(state["local"].age_secs, Some(70));
        assert_eq!(
            state["local"].age_label(),
            crate::humanize_duration_secs(70)
        );
        assert_eq!(state["local"].summary.as_deref(), Some("Turn finished"));
        assert!(!state["local"].stale);
        assert_eq!(state["adopted"].age_secs, None);
        assert!(!state["adopted"].stale);
        assert_eq!(state["missing"].age_secs, None);
        assert!(state["missing"].stale);
        assert!(!state.contains_key("foreign"));
        assert!(!state.contains_key("dead"));
    }
}
