//! Local layer of the hybrid inter-agent mailbox (STORY-493 slice 2).
//!
//! The fast, immediate layer: one JSON file per message under
//! `.aida/mailbox/`, written atomically. This is the live exchange surface
//! (no commit-per-message latency); the git-canonical durable digest is a
//! later slice that mirrors these into the orphan store for replay/sharing.
//! `.aida/mailbox/` is runtime state under the existing `.aida/*`
//! deny-by-default gitignore — no new ignore line needed.
//!
//! The read/filter/merge logic lives in the pure `aida_core::mailbox` core;
//! this module is only the file I/O around it. trace:STORY-493 trace:TASK-603 | ai:claude

use aida_core::mailbox::{message_state_rank, Message};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The local mailbox directory: `<project_root>/.aida/mailbox/` (fast layer).
/// Delegates to the shared aida-core helper so the CLI and the REST server
/// agree on the on-disk layout. trace:STORY-650 | ai:claude
pub(crate) fn mailbox_dir(project_root: &Path) -> PathBuf {
    aida_core::mailbox::local_mailbox_dir(project_root)
}

/// The canonical mailbox directory in the orphan-store worktree:
/// `<store_root>/mailbox/`. Durable, replayable, shareable across clones — the
/// git-canonical half of the hybrid, digested from the local layer and
/// committed on the orphan branch (separate from `objects/`, so it never
/// touches the spec store or its cache). trace:TASK-605 | ai:claude
pub(crate) fn canonical_dir(store_root: &Path) -> PathBuf {
    store_root.join("mailbox")
}

/// Write one message as an atomically-written JSON file under `dir`, named by
/// id. Append-only: ids are unique, so this never clobbers an existing message.
fn write_message_in(dir: &Path, msg: &Message) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating mailbox dir {}", dir.display()))?;
    let path = dir.join(format!("{}.json", sanitize_id(&msg.id)));
    let json = serde_json::to_string_pretty(msg).context("serializing mailbox message")?;
    aida_core::write_atomic(&path, json.as_bytes())
        .with_context(|| format!("writing message {}", path.display()))?;
    Ok(())
}

/// Append a message to the LOCAL layer. Delegates to the shared aida-core
/// writer so the CLI and the REST server share one implementation.
/// trace:STORY-650 | ai:claude
pub(crate) fn write_message(project_root: &Path, msg: &Message) -> Result<()> {
    aida_core::mailbox::write_local_message(project_root, msg)
        .with_context(|| format!("writing message {}", msg.id))
}

/// Read every message under `dir`. Files that fail to parse are skipped (a
/// half-written or hand-mangled file must not sink the whole inbox) — matching
/// how the spec store treats an unreadable object defensively. Empty when the
/// dir is absent.
fn read_messages_in(dir: &Path) -> Result<Vec<Message>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(msg) = serde_json::from_slice::<Message>(&bytes) {
            out.push(msg);
        }
    }
    Ok(out)
}

/// Read every message in the LOCAL layer.
pub(crate) fn read_local_messages(project_root: &Path) -> Result<Vec<Message>> {
    read_messages_in(&mailbox_dir(project_root))
}

/// Read every message in the CANONICAL (orphan-store) layer.
pub(crate) fn read_canonical_messages(store_root: &Path) -> Result<Vec<Message>> {
    read_messages_in(&canonical_dir(store_root))
}

/// Digest the local layer into the canonical layer: write every local message
/// whose id is not already canonical into `<store_root>/mailbox/`. Append-only
/// + id-keyed, so it is idempotent (re-running digests nothing new) and two
///   agents digesting concurrently merge without edit conflict. Returns the count
///   newly written; the CALLER stages + commits the orphan-store change.
///   trace:TASK-605 | ai:claude
pub(crate) fn digest_local_to_canonical(store_root: &Path, project_root: &Path) -> Result<usize> {
    use std::collections::HashMap;
    let local = read_local_messages(project_root)?;
    let canonical_by_id: HashMap<String, Message> = read_canonical_messages(store_root)?
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let cdir = canonical_dir(store_root);
    let mut written = 0usize;
    for msg in &local {
        let should_write = match canonical_by_id.get(&msg.id) {
            None => true,
            Some(canonical) => message_state_rank(msg) > message_state_rank(canonical),
        };
        if !should_write {
            continue;
        }
        write_message_in(&cdir, msg)?;
        written += 1;
    }
    Ok(written)
}

// trace:STORY-583 | ai:codex
pub(crate) fn write_message_marker(project_root: &Path, msg: &Message) -> Result<()> {
    write_message(project_root, msg)
}

/// Per-agent read-watermark directory: `<project_root>/.aida/mailbox/.read/`.
/// One file per agent (`<agent>.txt`) holding the timestamp (epoch millis) of
/// the newest message that agent has seen. Lets the operator overview compute
/// unread counts without mutating the append-only message model. Lives under
/// the existing `.aida/*` deny-by-default gitignore — local runtime state.
/// trace:STORY-539 | ai:claude
fn read_marker_dir(project_root: &Path) -> PathBuf {
    mailbox_dir(project_root).join(".read")
}

/// Read one agent's read-watermark (epoch millis of newest seen message), or
/// `None` if the agent has never read its inbox.
pub(crate) fn read_watermark(project_root: &Path, agent: &str) -> Option<i64> {
    let path = read_marker_dir(project_root).join(format!("{}.txt", sanitize_id(agent)));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
}

/// Set one agent's read-watermark to `ts` (epoch millis). Monotonic: never
/// lowers an existing watermark, so re-reading an older view doesn't "un-read"
/// newer messages. trace:STORY-539 | ai:claude
pub(crate) fn set_watermark(project_root: &Path, agent: &str, ts: i64) -> Result<()> {
    let dir = read_marker_dir(project_root);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating read-marker dir {}", dir.display()))?;
    let path = dir.join(format!("{}.txt", sanitize_id(agent)));
    let current = read_watermark(project_root, agent).unwrap_or(i64::MIN);
    if ts <= current {
        return Ok(());
    }
    aida_core::write_atomic(&path, ts.to_string().as_bytes())
        .with_context(|| format!("writing read-marker {}", path.display()))?;
    Ok(())
}

/// Read every recorded read-watermark, keyed by agent id. Absent dir → empty.
pub(crate) fn read_all_watermarks(
    project_root: &Path,
) -> Result<std::collections::HashMap<String, i64>> {
    let dir = read_marker_dir(project_root);
    let mut out = std::collections::HashMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(ts) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            out.insert(stem.to_string(), ts);
        }
    }
    Ok(out)
}

/// Keep a message id safe as a filename component (ids are uuid/HLC strings, but
/// be defensive against path separators / traversal in a hand-set id).
fn sanitize_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aida_core::mailbox::{inbox_for, Recipient};
    use tempfile::tempdir;

    fn msg(id: &str, from: &str, to: Recipient, ts: i64) -> Message {
        Message {
            id: id.to_string(),
            thread_id: "t1".to_string(),
            from: from.to_string(),
            to,
            timestamp: ts,
            in_reply_to: None,
            body: format!("body-{id}"),
            urgent: false,
            intent: aida_core::mailbox::Intent::Fyi,
            retracted: false,
            deleted: false,
        }
    }

    #[test]
    fn write_then_read_roundtrips_all_messages() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_message(
            root,
            &msg("m1", "codex", Recipient::Agent("claude".into()), 10),
        )
        .unwrap();
        write_message(root, &msg("m2", "agy", Recipient::Broadcast, 20)).unwrap();

        let all = read_local_messages(root).unwrap();
        assert_eq!(all.len(), 2);
        // The pure core composes over what we read back.
        let inbox: Vec<&str> = inbox_for("claude", &all)
            .iter()
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            inbox,
            vec!["m1", "m2"],
            "direct + broadcast land in claude's inbox"
        );
    }

    #[test]
    fn read_is_empty_when_no_mailbox_dir() {
        let dir = tempdir().unwrap();
        assert!(read_local_messages(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn unparseable_file_is_skipped_not_fatal() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_message(root, &msg("good", "x", Recipient::Broadcast, 1)).unwrap();
        std::fs::write(mailbox_dir(root).join("junk.json"), b"{not valid json").unwrap();
        let all = read_local_messages(root).unwrap();
        assert_eq!(all.len(), 1, "the good message survives a junk neighbor");
        assert_eq!(all[0].id, "good");
    }

    #[test]
    fn sanitize_id_neutralizes_path_separators() {
        assert_eq!(sanitize_id("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_id("0193-abc_DEF"), "0193-abc_DEF");
    }

    #[test]
    fn digest_writes_new_messages_to_canonical_and_is_idempotent() {
        // trace:TASK-605 | ai:claude — local and canonical are separate dirs.
        let proj = tempdir().unwrap();
        let store = tempdir().unwrap();
        write_message(proj.path(), &msg("a", "codex", Recipient::Broadcast, 10)).unwrap();
        write_message(proj.path(), &msg("b", "agy", Recipient::Broadcast, 20)).unwrap();

        // First digest writes both into the canonical layer.
        let n = digest_local_to_canonical(store.path(), proj.path()).unwrap();
        assert_eq!(n, 2);
        let canon = read_canonical_messages(store.path()).unwrap();
        assert_eq!(canon.len(), 2);
        assert!(canon.iter().any(|m| m.id == "a") && canon.iter().any(|m| m.id == "b"));

        // Re-digesting writes nothing new (idempotent, id-keyed).
        assert_eq!(
            digest_local_to_canonical(store.path(), proj.path()).unwrap(),
            0
        );

        // A new local message digests incrementally.
        write_message(proj.path(), &msg("c", "claude", Recipient::Broadcast, 30)).unwrap();
        assert_eq!(
            digest_local_to_canonical(store.path(), proj.path()).unwrap(),
            1
        );
        assert_eq!(read_canonical_messages(store.path()).unwrap().len(), 3);
    }

    // trace:STORY-583 | ai:codex
    #[test]
    fn digest_writes_delete_marker_for_already_synced_message() {
        let proj = tempdir().unwrap();
        let store = tempdir().unwrap();
        let original = msg("a", "codex", Recipient::Broadcast, 10);
        write_message(proj.path(), &original).unwrap();
        assert_eq!(
            digest_local_to_canonical(store.path(), proj.path()).unwrap(),
            1
        );

        let marker = Message {
            deleted: true,
            body: String::new(),
            ..original
        };
        write_message_marker(proj.path(), &marker).unwrap();
        assert_eq!(
            digest_local_to_canonical(store.path(), proj.path()).unwrap(),
            1
        );

        let canon = read_canonical_messages(store.path()).unwrap();
        assert_eq!(canon.len(), 1);
        assert!(canon[0].deleted);
    }

    #[test]
    fn watermark_roundtrips_and_is_monotonic() {
        // trace:STORY-539 | ai:claude
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert_eq!(read_watermark(root, "claude"), None, "never read → None");
        set_watermark(root, "claude", 50).unwrap();
        assert_eq!(read_watermark(root, "claude"), Some(50));
        // A lower value never lowers the watermark.
        set_watermark(root, "claude", 10).unwrap();
        assert_eq!(read_watermark(root, "claude"), Some(50));
        // A higher value advances it.
        set_watermark(root, "claude", 99).unwrap();
        assert_eq!(read_watermark(root, "claude"), Some(99));
    }

    #[test]
    fn read_all_watermarks_collects_every_agent() {
        // trace:STORY-539 | ai:claude
        let dir = tempdir().unwrap();
        let root = dir.path();
        assert!(read_all_watermarks(root).unwrap().is_empty());
        set_watermark(root, "claude", 10).unwrap();
        set_watermark(root, "codex", 20).unwrap();
        let all = read_all_watermarks(root).unwrap();
        assert_eq!(all.get("claude"), Some(&10));
        assert_eq!(all.get("codex"), Some(&20));
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn read_markers_do_not_leak_into_message_reads() {
        // The `.read/` subdir must not be parsed as messages.
        let dir = tempdir().unwrap();
        let root = dir.path();
        write_message(root, &msg("m1", "codex", Recipient::Broadcast, 10)).unwrap();
        set_watermark(root, "claude", 10).unwrap();
        let all = read_local_messages(root).unwrap();
        assert_eq!(all.len(), 1, "read-markers are not messages");
    }
}
