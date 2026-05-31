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

use aida_core::mailbox::Message;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The local mailbox directory: `<project_root>/.aida/mailbox/` (fast layer).
pub(crate) fn mailbox_dir(project_root: &Path) -> PathBuf {
    project_root.join(".aida").join("mailbox")
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

/// Append a message to the LOCAL layer.
pub(crate) fn write_message(project_root: &Path, msg: &Message) -> Result<()> {
    write_message_in(&mailbox_dir(project_root), msg)
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
/// agents digesting concurrently merge without edit conflict. Returns the count
/// newly written; the CALLER stages + commits the orphan-store change.
/// trace:TASK-605 | ai:claude
pub(crate) fn digest_local_to_canonical(store_root: &Path, project_root: &Path) -> Result<usize> {
    use std::collections::HashSet;
    let local = read_local_messages(project_root)?;
    let canonical_ids: HashSet<String> = read_canonical_messages(store_root)?
        .into_iter()
        .map(|m| m.id)
        .collect();
    let cdir = canonical_dir(store_root);
    let mut written = 0usize;
    for msg in &local {
        if !canonical_ids.contains(&msg.id) {
            write_message_in(&cdir, msg)?;
            written += 1;
        }
    }
    Ok(written)
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
}
