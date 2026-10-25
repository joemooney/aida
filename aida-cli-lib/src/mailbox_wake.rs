//! Bounded, non-consuming mailbox envelope for host notification adapters.
// trace:TASK-2347 | ai:codex
use aida_core::mailbox::{inbox_for, merge_dedup, Message};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LIMIT: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;
const TOTAL_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize)]
pub(crate) struct Snapshot {
    version: u8,
    messages: Vec<Envelope>,
    acknowledged_ids: Vec<String>,
}
#[derive(Serialize)]
struct Envelope {
    id: String,
}

fn open_directory(path: &Path) -> Result<Option<File>> {
    let result = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path);
    match result {
        Ok(file) => {
            let meta = file.metadata()?;
            if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o002 != 0 {
                bail!("Mailbox directory ownership or permissions refused");
            }
            Ok(Some(file))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("Cannot open mailbox directory"),
    }
}
fn pinned(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}
fn read_regular(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o002 != 0
        || meta.len() > limit
    {
        bail!("Mailbox entry is not a bounded owned regular file");
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("Mailbox entry grew beyond its bound");
    }
    Ok(bytes)
}
fn messages(dir: &Path, budget: &mut u64) -> Result<Vec<Message>> {
    let Some(anchor) = open_directory(dir)? else {
        return Ok(Vec::new());
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    for (count, entry) in std::fs::read_dir(pinned(&anchor))?.enumerate() {
        if count >= MAX_ENTRIES || Instant::now() >= deadline {
            bail!("Mailbox scan budget exceeded");
        }
        let entry = entry?;
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes =
            read_regular(&entry.path(), LIMIT).context("Cannot safely read mailbox entry")?;
        // Markers/receipts are not top-level message JSON; malformed messages refuse
        // the snapshot rather than silently treating them as delivered.
        *budget = budget
            .checked_sub(bytes.len() as u64)
            .context("Mailbox byte budget exceeded")?;
        output.push(serde_json::from_slice(&bytes).context("Invalid mailbox message")?);
    }
    Ok(output)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// Reuse the existing acknowledgement contract without moving any watermark or
// inventing a baseline. Explicit ID receipts also survive a stale watermark.
fn read_evidence(
    mail: &Path,
    recipient: &str,
    budget: &mut u64,
) -> Result<(i64, std::collections::BTreeSet<String>)> {
    let Some(anchor) = open_directory(mail)? else {
        return Ok((i64::MIN, Default::default()));
    };
    let watermark = if let Some(read) = open_directory(&pinned(&anchor).join(".read"))? {
        match read_regular(&pinned(&read).join(format!("{recipient}.txt")), 128) {
            Ok(bytes) => std::str::from_utf8(&bytes)?.trim().parse::<i64>()?,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                i64::MIN
            }
            Err(error) => return Err(error).context("Cannot safely read mailbox watermark"),
        }
    } else {
        i64::MIN
    };
    let mut seen = std::collections::BTreeSet::new();
    if let Some(receipts) = open_directory(&pinned(&anchor).join(".read-receipts"))? {
        if let Some(recipient_dir) = open_directory(&pinned(&receipts).join(recipient))? {
            let deadline = Instant::now() + Duration::from_secs(2);
            for (count, entry) in std::fs::read_dir(pinned(&recipient_dir))?.enumerate() {
                if count >= MAX_ENTRIES || Instant::now() >= deadline {
                    bail!("Mailbox receipt scan budget exceeded");
                }
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("txt") {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("Invalid receipt identity")?;
                if !safe_id(id) {
                    bail!("Invalid receipt identity");
                }
                let bytes = read_regular(&path, 128.min(*budget))
                    .context("Cannot safely read mailbox receipt")?;
                *budget = budget
                    .checked_sub(bytes.len() as u64)
                    .context("Mailbox byte budget exceeded")?;
                std::str::from_utf8(&bytes)?
                    .trim()
                    .parse::<i64>()
                    .context("Invalid mailbox receipt")?;
                seen.insert(id.to_owned());
            }
        }
    }
    Ok((watermark, seen))
}

pub(crate) fn snapshot(root: &Path, store: &Path, recipient: &str) -> Result<Snapshot> {
    if recipient.is_empty()
        || recipient.len() > 128
        || !recipient
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("Recipient must be a bounded exact mailbox identity");
    }
    // Pin .aida separately: a writable checkout cannot substitute a mailbox
    // ancestor while this host-side reader is accessing private mail.
    let local_meta = open_directory(&root.join(".aida"))?;
    let local_dir = local_meta.as_ref().map(|f| pinned(f).join("mailbox"));
    let mut budget = TOTAL_BYTES;
    let local = match &local_dir {
        Some(p) => messages(p, &mut budget)?,
        None => Vec::new(),
    };
    let canonical = messages(&store.join("mailbox"), &mut budget)?;
    let merged = merge_dedup(&local, &canonical);
    let (watermark, mut acknowledged) = match &local_dir {
        Some(dir) => read_evidence(dir, recipient, &mut budget)?,
        None => (i64::MIN, Default::default()),
    };
    let mut unread = Vec::new();
    for m in inbox_for(recipient, &merged) {
        if !safe_id(&m.id) {
            bail!("Mailbox message ID is not a bounded safe identity");
        }
        if m.timestamp <= watermark {
            acknowledged.insert(m.id.clone());
        }
        if !m.archived && m.timestamp > watermark && !acknowledged.contains(&m.id) {
            unread.push(Envelope { id: m.id.clone() });
        }
    }
    Ok(Snapshot {
        version: 1,
        messages: unread,
        acknowledged_ids: acknowledged.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn fixture(root: &Path, id: &str, recipient: &str, timestamp: i64) {
        let dir = root.join(".aida/mailbox");
        std::fs::create_dir_all(&dir).unwrap();
        let message = serde_json::json!({
            "id": id, "thread_id": id, "from": "sender", "to": {"kind": "agent", "agent": recipient},
            "timestamp": timestamp, "body": "PRIVATE BODY MUST NOT ENTER ENVELOPE", "subject": "PRIVATE SUBJECT"
        });
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&message).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn task_2347_snapshot_filters_unread_and_never_consumes_or_exposes_body() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        std::fs::create_dir(&store).unwrap();
        fixture(root.path(), "old-id", "advisor", 10);
        fixture(root.path(), "new-id", "advisor", 20);
        // Existing AIDA writers respect the user's group-sharing umask.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.path().join(".aida/mailbox/new-id.json"),
            std::fs::Permissions::from_mode(0o664),
        )
        .unwrap();
        fixture(root.path(), "other-id", "other", 30);
        let read = root.path().join(".aida/mailbox/.read");
        std::fs::create_dir(&read).unwrap();
        std::fs::write(read.join("advisor.txt"), "15").unwrap();
        let before = std::fs::read(read.join("advisor.txt")).unwrap();
        let value =
            serde_json::to_string(&snapshot(root.path(), &store, "advisor").unwrap()).unwrap();
        assert_eq!(
            value,
            r#"{"version":1,"messages":[{"id":"new-id"}],"acknowledged_ids":["old-id"]}"#
        );
        assert_eq!(std::fs::read(read.join("advisor.txt")).unwrap(), before);
    }

    #[test]
    fn task_2347_snapshot_refuses_symlink_fifo_and_hardlinked_mail() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        std::fs::create_dir(&store).unwrap();
        let mail = root.path().join(".aida/mailbox");
        std::fs::create_dir_all(&mail).unwrap();
        let outside = root.path().join("private");
        std::fs::write(&outside, "PRIVATE").unwrap();
        let path = mail.join("bad.json");
        symlink(&outside, &path).unwrap();
        assert!(snapshot(root.path(), &store, "advisor").is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::hard_link(&outside, &path).unwrap();
        assert!(snapshot(root.path(), &store, "advisor").is_err());
        std::fs::remove_file(&path).unwrap();
        let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        let started = Instant::now();
        assert!(snapshot(root.path(), &store, "advisor").is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn task_2347_snapshot_aggregate_bytes_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path(), "message-id", "advisor", 20);
        let mut budget = 8;
        assert!(messages(&root.path().join(".aida/mailbox"), &mut budget).is_err());
        assert!(!root.path().join(".aida/mailbox/.read").exists());
    }

    #[test]
    fn task_2347_snapshot_refuses_corrupt_watermark_and_unsafe_identity() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        fixture(root.path(), "message-id", "advisor", 20);
        let dir = root.path().join(".aida/mailbox/.read");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("advisor.txt"), "not a timestamp").unwrap();
        assert!(snapshot(root.path(), &store, "advisor").is_err());
        assert!(snapshot(root.path(), &store, "../advisor").is_err());
    }
    #[test]
    fn task_2347_read_metadata_is_id_scoped_and_preserves_unknown_private_mail() {
        let main = tempfile::tempdir().unwrap();
        let sibling = tempfile::tempdir().unwrap();
        let store = main.path().join("store");
        fixture(main.path(), "replicated", "advisor", 10);
        fixture(sibling.path(), "replicated", "advisor", 10);
        fixture(sibling.path(), "private-unseen", "advisor", 5);
        let read = main.path().join(".aida/mailbox/.read");
        std::fs::create_dir(&read).unwrap();
        std::fs::write(read.join("advisor.txt"), "100").unwrap();
        let known = snapshot(main.path(), &store, "advisor").unwrap();
        let private = snapshot(sibling.path(), &store, "advisor").unwrap();
        assert_eq!(known.acknowledged_ids, vec!["replicated"]);
        assert!(!known
            .acknowledged_ids
            .contains(&"private-unseen".to_string()));
        assert_eq!(private.messages.len(), 2);
        let remaining: Vec<_> = private
            .messages
            .iter()
            .filter(|m| !known.acknowledged_ids.contains(&m.id))
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(remaining, vec!["private-unseen"]);
        let receipts = sibling.path().join(".aida/mailbox/.read-receipts/advisor");
        std::fs::create_dir_all(&receipts).unwrap();
        std::fs::write(receipts.join("private-unseen.txt"), "120").unwrap();
        let acknowledged = snapshot(sibling.path(), &store, "advisor").unwrap();
        assert!(acknowledged
            .acknowledged_ids
            .contains(&"private-unseen".to_string()));
        assert_eq!(acknowledged.messages.len(), 1);
        assert!(!sibling.path().join(".aida/mailbox/.read").exists());
    }
}
