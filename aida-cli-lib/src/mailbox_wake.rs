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

// Keep the complete cross-root receipt inventory. Independent guarded reads
// overlap disk latency, without a cache, a wider deadline, or partial evidence.
// trace:TASK-2347 | ai:codex
const RECEIPT_READERS: usize = 4;

fn read_receipts<F>(
    directory: &File,
    budget: &mut u64,
    deadline: Instant,
    readers: usize,
    read: &F,
) -> Result<std::collections::BTreeSet<String>>
where
    F: Fn(&Path, u64) -> Result<Vec<u8>> + Sync,
{
    read_receipts_with_clock(directory, budget, deadline, readers, read, &Instant::now)
}

fn read_receipts_with_clock<F, C>(
    directory: &File,
    budget: &mut u64,
    deadline: Instant,
    readers: usize,
    read: &F,
    now: &C,
) -> Result<std::collections::BTreeSet<String>>
where
    F: Fn(&Path, u64) -> Result<Vec<u8>> + Sync,
    C: Fn() -> Instant + Sync,
{
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let mut entries = Vec::new();
    for (count, entry) in std::fs::read_dir(pinned(directory))?.enumerate() {
        if count >= MAX_ENTRIES || now() >= deadline {
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
        entries.push((id.to_owned(), path));
    }
    let read_one = |path: &Path, limit: u64| -> Result<usize> {
        if now() >= deadline {
            bail!("Mailbox receipt scan budget exceeded");
        }
        let bytes = read(path, limit).context("Cannot safely read mailbox receipt")?;
        // Like the existing reader, the time bound is cooperative around IO.
        // Never accept a last read that completed after the shared deadline.
        if now() >= deadline {
            bail!("Mailbox receipt scan budget exceeded");
        }
        std::str::from_utf8(&bytes)?
            .trim()
            .parse::<i64>()
            .context("Invalid mailbox receipt")?;
        Ok(bytes.len())
    };
    let mut seen = std::collections::BTreeSet::new();
    // Reserve the 128-byte file bound plus read_regular's growth-detection
    // byte for every parallel entry. When the remaining aggregate budget
    // cannot cover that reservation, preserve the
    // original sequential actual-byte accounting rather than overspending.
    if readers <= 1 || (entries.len() as u64) * 129 > *budget {
        for (id, path) in entries {
            let used = read_one(&path, 128.min(*budget))?;
            *budget = budget
                .checked_sub(used as u64)
                .context("Mailbox byte budget exceeded")?;
            seen.insert(id);
        }
    } else {
        let next = AtomicUsize::new(0);
        let stopped = AtomicBool::new(false);
        let results: Vec<Result<Vec<(String, usize)>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..readers.min(RECEIPT_READERS).min(entries.len()))
                .map(|_| {
                    scope.spawn(|| -> Result<Vec<(String, usize)>> {
                        let mut completed = Vec::new();
                        while !stopped.load(Ordering::Relaxed) {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some((id, path)) = entries.get(index) else {
                                break;
                            };
                            match read_one(path, 128) {
                                Ok(used) => completed.push((id.clone(), used)),
                                Err(error) => {
                                    stopped.store(true, Ordering::Relaxed);
                                    return Err(error);
                                }
                            }
                        }
                        Ok(completed)
                    })
                })
                .collect();
            // Join every worker even after an error; no reader outlives its pin
            // and no partial inventory can escape on failure or panic.
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("Mailbox receipt reader failed")))
                })
                .collect()
        });
        for result in results {
            for (id, used) in result? {
                *budget = budget
                    .checked_sub(used as u64)
                    .context("Mailbox byte budget exceeded")?;
                seen.insert(id);
            }
        }
    }
    if now() >= deadline {
        bail!("Mailbox receipt scan budget exceeded");
    }
    Ok(seen)
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
            seen = read_receipts(
                &recipient_dir,
                budget,
                Instant::now() + Duration::from_secs(2),
                RECEIPT_READERS,
                &read_regular,
            )?;
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
    let local_ids: std::collections::BTreeSet<_> = local.iter().map(|m| m.id.as_str()).collect();
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
        if m.timestamp <= watermark && local_ids.contains(m.id.as_str()) {
            acknowledged.insert(m.id.clone());
        }
        if !m.archived && !acknowledged.contains(&m.id) {
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
        // Late publication to canonical is not proof that main previously read it.
        fixture(&store, "private-unseen", "advisor", 5);
        std::fs::rename(store.join(".aida/mailbox"), store.join("mailbox")).unwrap();
        let read = main.path().join(".aida/mailbox/.read");
        std::fs::create_dir(&read).unwrap();
        std::fs::write(read.join("advisor.txt"), "100").unwrap();
        let known = snapshot(main.path(), &store, "advisor").unwrap();
        let private = snapshot(sibling.path(), &store, "advisor").unwrap();
        assert_eq!(known.acknowledged_ids, vec!["replicated"]);
        assert_eq!(known.messages.len(), 1);
        assert_eq!(known.messages[0].id, "private-unseen");
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
    // trace:TASK-2347 | ai:codex
    #[test]
    fn task_2347_live_scale_receipts_overlap_latency_without_losing_evidence() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        const COUNT: usize = 1536; // above observed live 1139 and prior 1282 scale
        for n in 0..COUNT {
            std::fs::write(root.path().join(format!("receipt-{n}.txt")), "123\n").unwrap();
        }
        let anchor = open_directory(root.path()).unwrap().unwrap();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let delayed = |path: &Path, limit| {
            let running = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(running, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(2));
            let result = read_regular(path, limit);
            active.fetch_sub(1, Ordering::SeqCst);
            result
        };
        // Timing is diagnostic: shared-host contention must not make the CI
        // assertions flaky. Functional completeness uses a fixed test clock;
        // production always supplies Instant::now through read_receipts.
        let base = Instant::now();
        let mut budget = TOTAL_BYTES;
        let ids = read_receipts_with_clock(
            &anchor,
            &mut budget,
            base + Duration::from_secs(2),
            RECEIPT_READERS,
            &read_regular,
            &|| base,
        )
        .unwrap();
        assert_eq!(ids.len(), COUNT);
        assert!(ids.contains("receipt-0") && ids.contains("receipt-1535"));
        assert_eq!(budget, TOTAL_BYTES - 4 * COUNT as u64);
        let started = Instant::now();
        let mut serial_budget = TOTAL_BYTES;
        let serial = read_receipts(
            &anchor,
            &mut serial_budget,
            started + Duration::from_secs(2),
            1,
            &delayed,
        );
        let serial_elapsed = started.elapsed();
        let started = Instant::now();
        let mut parallel_budget = TOTAL_BYTES;
        let parallel = read_receipts(
            &anchor,
            &mut parallel_budget,
            started + Duration::from_secs(2),
            RECEIPT_READERS,
            &delayed,
        );
        let parallel_elapsed = started.elapsed();
        for result in [&serial, &parallel] {
            match result {
                Ok(ids) => assert_eq!(ids.len(), COUNT),
                Err(error) => assert!(error.to_string().contains("scan budget exceeded")),
            }
        }
        assert!(peak.load(Ordering::SeqCst) <= RECEIPT_READERS);
        eprintln!("receipt timing (diagnostic): count={COUNT} serial_complete={} serial_ms={} parallel_complete={} parallel_ms={} peak_readers={}", serial.is_ok(), serial_elapsed.as_millis(), parallel.is_ok(), parallel_elapsed.as_millis(), peak.load(Ordering::SeqCst));
    }

    #[test]
    fn task_2347_receipt_pool_keeps_unsafe_and_incomplete_evidence_fail_closed() {
        use std::os::unix::fs::PermissionsExt;
        for kind in [
            "malformed",
            "symlink",
            "hardlink",
            "fifo",
            "directory",
            "writable",
            "oversized",
            "removed",
        ] {
            let root = tempfile::tempdir().unwrap();
            let store = root.path().join("store");
            fixture(root.path(), "must-stay-unread", "advisor", 20);
            let directory = root.path().join(".aida/mailbox/.read-receipts/advisor");
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("unrelated-read.txt"), "30").unwrap();
            let bad = directory.join("bad-evidence.txt");
            let outside = root.path().join("outside");
            std::fs::write(&outside, "30").unwrap();
            match kind {
                "symlink" => symlink(&outside, &bad).unwrap(),
                "hardlink" => std::fs::hard_link(&outside, &bad).unwrap(),
                "fifo" => {
                    let path = std::ffi::CString::new(bad.as_os_str().as_encoded_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                "directory" => std::fs::create_dir(&bad).unwrap(),
                "malformed" => std::fs::write(&bad, "not a timestamp").unwrap(),
                "oversized" => std::fs::write(&bad, "1".repeat(129)).unwrap(),
                _ => std::fs::write(&bad, "30").unwrap(),
            }
            if kind == "writable" {
                std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o666)).unwrap();
            }
            if kind == "removed" {
                let anchor = open_directory(&directory).unwrap().unwrap();
                let mut budget = TOTAL_BYTES;
                assert!(read_receipts(
                    &anchor,
                    &mut budget,
                    Instant::now() + Duration::from_secs(2),
                    RECEIPT_READERS,
                    &|path, limit| {
                        if path.file_name().unwrap() == "bad-evidence.txt" {
                            std::fs::remove_file(path)?;
                        }
                        read_regular(path, limit)
                    }
                )
                .is_err());
            } else {
                assert!(snapshot(root.path(), &store, "advisor").is_err(), "{kind}");
            }
            assert!(!root.path().join(".aida/mailbox/.read").exists());
            assert_eq!(
                std::fs::read_to_string(directory.join("unrelated-read.txt")).unwrap(),
                "30"
            );
        }
    }

    #[test]
    fn task_2347_small_receipt_budget_retains_actual_byte_accounting() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.txt"), "1").unwrap();
        std::fs::write(root.path().join("two.txt"), "2").unwrap();
        let anchor = open_directory(root.path()).unwrap().unwrap();
        let mut budget = 2;
        let ids = read_receipts(
            &anchor,
            &mut budget,
            Instant::now() + Duration::from_secs(2),
            RECEIPT_READERS,
            &read_regular,
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(budget, 0);
        let mut budget = 1;
        assert!(read_receipts(
            &anchor,
            &mut budget,
            Instant::now() + Duration::from_secs(2),
            RECEIPT_READERS,
            &read_regular
        )
        .is_err());
    }
    #[test]
    fn task_2347_receipt_pool_reserves_growth_detection_bytes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        let timestamp = format!("{}1", " ".repeat(127));
        for id in ["one", "two"] {
            std::fs::write(root.path().join(format!("{id}.txt")), &timestamp).unwrap();
        }
        let anchor = open_directory(root.path()).unwrap().unwrap();
        let mut budget = 256;
        let active = AtomicUsize::new(0);
        let ids = read_receipts(
            &anchor,
            &mut budget,
            Instant::now() + Duration::from_secs(2),
            RECEIPT_READERS,
            &|path, limit| {
                let concurrent = active.fetch_add(1, Ordering::SeqCst) + 1;
                // Two guarded reads can consume 258 bytes with growth sentinels,
                // so this valid 256-byte input must retain sequential accounting.
                assert!(concurrent * 129 <= 256);
                std::thread::sleep(Duration::from_millis(2));
                let result = read_regular(path, limit);
                active.fetch_sub(1, Ordering::SeqCst);
                result
            },
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(budget, 0);
    }
    #[test]
    fn task_2347_receipt_deadline_exhaustion_is_deterministic_and_never_partial() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("last.txt"), "30").unwrap();
        let anchor = open_directory(root.path()).unwrap().unwrap();
        let base = Instant::now();
        let calls = AtomicUsize::new(0);
        let tick = AtomicUsize::new(0);
        let clock = || base + Duration::from_millis(tick.load(Ordering::SeqCst) as u64);
        let read = |path: &Path, limit| {
            calls.fetch_add(1, Ordering::SeqCst);
            let bytes = read_regular(path, limit)?;
            tick.store(2000, Ordering::SeqCst);
            Ok(bytes)
        };
        for readers in [1, RECEIPT_READERS] {
            tick.store(2000, Ordering::SeqCst);
            calls.store(0, Ordering::SeqCst);
            let mut budget = TOTAL_BYTES;
            assert!(read_receipts_with_clock(
                &anchor,
                &mut budget,
                base + Duration::from_secs(2),
                readers,
                &read,
                &clock
            )
            .is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(budget, TOTAL_BYTES);
            tick.store(0, Ordering::SeqCst);
            // The only/last guarded read crosses the deadline. There is no
            // next entry whose pre-read check could conceal a late success.
            assert!(read_receipts_with_clock(
                &anchor,
                &mut budget,
                base + Duration::from_secs(2),
                readers,
                &read,
                &clock
            )
            .is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(budget, TOTAL_BYTES);
        }
    }

    #[test]
    fn task_2347_panicking_receipt_reader_cannot_return_partial_ids() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("normal.txt"), "30").unwrap();
        std::fs::write(root.path().join("panic.txt"), "30").unwrap();
        let anchor = open_directory(root.path()).unwrap().unwrap();
        let base = Instant::now();
        let mut budget = TOTAL_BYTES;
        assert!(read_receipts_with_clock(
            &anchor,
            &mut budget,
            base + Duration::from_secs(2),
            RECEIPT_READERS,
            &|path, limit| {
                assert_ne!(
                    path.file_name().unwrap(),
                    "panic.txt",
                    "synthetic reader failure"
                );
                read_regular(path, limit)
            },
            &|| base
        )
        .is_err());
    }
}
